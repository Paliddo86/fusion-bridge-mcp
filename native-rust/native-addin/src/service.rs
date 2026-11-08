//! Own every worker so native DLL teardown can join them before SDK release.
use crate::{
    execution::{Executor, Reply, Task},
    http,
    operation_status::OperationStatusReader,
    Snapshot,
};
use fusion_native_dispatch::{CompleteError, DrainError, Outcome, Runtime};
use serde_json::{json, Value};
use std::{
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub type Queue = Runtime<Task, Result<Reply, String>>;
#[cfg(test)]
#[path = "wake_tests.rs"]
mod wake_tests;
/// Raw adapter acknowledgement is not a guarantee of callback delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeAck {
    Acknowledged,
    Unconfirmed,
    HardFailure(i32),
}
impl WakeAck {
    pub fn from_status(status: i32) -> Self {
        match status {
            0 => Self::Acknowledged,
            7 => Self::Unconfirmed,
            other => Self::HardFailure(other),
        }
    }
}
pub type RawWake = Arc<dyn Fn() -> WakeAck + Send + Sync>;
pub trait WakeDispatch: Send + Sync {
    fn request(&self) -> bool;
    fn settled(&self) {}
}
// Existing simple test/probe wakes have no multi-generation state.
impl<F: Fn() -> bool + Send + Sync> WakeDispatch for F {
    fn request(&self) -> bool {
        self()
    }
}
pub type Wake = Arc<dyn WakeDispatch>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeGeneration(u64);
struct Generation {
    id: WakeGeneration,
    observed: bool,
    delivery_deadline: Option<Instant>,
}
#[derive(Default)]
struct WakeState {
    next: u64,
    active: Option<Generation>,
    stopped: bool,
}
/// One pending native wake per generation. Only observed callbacks retire a
/// generation; an expired unobserved wake closes the epoch instead of resending.
pub struct WakeCoalescer {
    state: Mutex<WakeState>,
    queue: Arc<Queue>,
    fire: RawWake,
    stats: Arc<Stats>,
}
impl WakeCoalescer {
    pub fn new(queue: Arc<Queue>, fire: RawWake) -> Arc<Self> {
        Self::with_stats(queue, fire, Arc::new(Stats::default()))
    }
    pub fn with_stats(queue: Arc<Queue>, fire: RawWake, stats: Arc<Stats>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(WakeState::default()),
            queue,
            fire,
            stats,
        })
    }
    fn generation(
        state: &mut WakeState,
        observed: bool,
        deadline: Option<Instant>,
    ) -> Option<WakeGeneration> {
        state.next = state.next.checked_add(1)?;
        let id = WakeGeneration(state.next);
        state.active = Some(Generation {
            id,
            observed,
            delivery_deadline: deadline,
        });
        Some(id)
    }
    fn expired(state: &WakeState, now: Instant) -> bool {
        state
            .active
            .as_ref()
            .is_some_and(|g| !g.observed && g.delivery_deadline.is_some_and(|d| d <= now))
    }
    pub fn expire_unobserved(&self) -> bool {
        let should_stop = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.stopped || self.queue.is_shutdown() {
                return true;
            }
            if !Self::expired(&state, Instant::now()) {
                return false;
            }
            state.stopped = true;
            true
        };
        if should_stop {
            self.stats
                .shutdown(&self.queue, ShutdownReason::NativeWakeDeliveryTimeout);
        }
        true
    }
    pub fn request(&self) -> bool {
        if self.expire_unobserved() {
            return false;
        }
        let generation = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.stopped || self.queue.is_shutdown() {
                return false;
            }
            let deadline = self.queue.earliest_queued_deadline();
            if let Some(active) = state.active.as_mut() {
                if !active.observed {
                    if let Some(deadline) = deadline {
                        active.delivery_deadline = Some(
                            active
                                .delivery_deadline
                                .map_or(deadline, |old| old.min(deadline)),
                        );
                    }
                }
                drop(state);
                return !self.expire_unobserved();
            }
            let Some(deadline) = deadline else {
                return true;
            };
            if deadline <= Instant::now() {
                state.stopped = true;
                drop(state);
                self.stats
                    .shutdown(&self.queue, ShutdownReason::NativeWakeDeliveryTimeout);
                return false;
            }
            let Some(generation) = Self::generation(&mut state, false, Some(deadline)) else {
                state.stopped = true;
                drop(state);
                self.stats
                    .shutdown(&self.queue, ShutdownReason::NativeWakeGenerationExhausted);
                return false;
            };
            generation
        };
        self.fire_generation(generation)
    }
    fn fire_generation(&self, generation: WakeGeneration) -> bool {
        // No Rust lock spans the FFI call. It can return after this generation's
        // callback completed and a later generation was already armed.
        let ack = (self.fire)();
        if matches!(ack, WakeAck::HardFailure(_)) {
            let applies = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if !state.stopped && state.active.as_ref().is_some_and(|g| g.id == generation) {
                    state.stopped = true;
                    true
                } else {
                    false
                }
            };
            if applies {
                self.stats
                    .shutdown(&self.queue, ShutdownReason::NativeWakeFailed);
            }
        }
        // Acknowledged/unconfirmed returns never clear or rearm a generation.
        !self.queue.is_shutdown()
    }
    pub fn begin_callback(&self) -> Option<WakeGeneration> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped || self.queue.is_shutdown() {
            return None;
        }
        if Self::expired(&state, Instant::now()) {
            state.stopped = true;
            drop(state);
            self.stats
                .shutdown(&self.queue, ShutdownReason::NativeWakeDeliveryTimeout);
            return None;
        }
        if let Some(active) = state.active.as_mut() {
            if active.observed {
                return None;
            }
            active.observed = true;
            return Some(active.id);
        }
        // A harmless late event can race a producer before request(). Treat it
        // as an observed generation, so producers do not fire while it drains.
        let generation = Self::generation(&mut state, true, None);
        if generation.is_none() {
            state.stopped = true;
            drop(state);
            self.stats
                .shutdown(&self.queue, ShutdownReason::NativeWakeGenerationExhausted);
        }
        generation
    }
    pub fn finish_callback(&self, generation: WakeGeneration) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped || self.queue.is_shutdown() {
            return false;
        }
        if !state
            .active
            .as_ref()
            .is_some_and(|g| g.id == generation && g.observed)
        {
            return false;
        }
        state.active = None;
        drop(state);
        // Only queued work after a completed bounded drain justifies a new
        // generation. No claimed request is replayed and no missed wake retried.
        self.request()
    }
}
impl WakeDispatch for WakeCoalescer {
    fn request(&self) -> bool {
        WakeCoalescer::request(self)
    }
    fn settled(&self) {
        self.expire_unobserved();
    }
}

/// Bound one Fusion callback even when workers keep replenishing the queue.
/// The caller must rearm queued work after this bounded drain: a nested SDK
/// message pump may have consumed a request's original wake during a read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackDrainError {
    Queue(DrainError),
    Completion(CompleteError),
}
impl CallbackDrainError {
    fn code(self) -> &'static str {
        match self {
            Self::Queue(DrainError::StaleEpoch) => "queue_stale_epoch",
            Self::Queue(DrainError::WrongThread) => "queue_wrong_thread",
            Self::Queue(DrainError::ShuttingDown) => "queue_shutting_down",
            Self::Completion(CompleteError::ClaimMissing) => "completion_claim_missing",
            Self::Completion(CompleteError::ClaimNotExecuting) => "completion_claim_not_executing",
        }
    }
}
pub fn drain(
    queue: &Arc<Queue>,
    mut read: impl FnMut() -> Result<Snapshot, String>,
) -> Result<usize, CallbackDrainError> {
    drain_inner(queue, None, |task| match task {
        Task::Snapshot => read().map(Reply::Snapshot),
        _ => Err("mechanical executor not configured".into()),
    })
}
pub fn drain_with_stats(
    queue: &Arc<Queue>,
    stats: &Stats,
    mut read: impl FnMut() -> Result<Snapshot, String>,
) -> Result<usize, CallbackDrainError> {
    drain_inner(queue, Some(stats), |task| match task {
        Task::Snapshot => read().map(Reply::Snapshot),
        _ => Err("mechanical executor not configured".into()),
    })
}
pub fn drain_tasks_with_stats(
    queue: &Arc<Queue>,
    stats: &Stats,
    run: impl FnMut(&mut Task) -> Result<Reply, String>,
) -> Result<usize, CallbackDrainError> {
    drain_inner(queue, Some(stats), run)
}
fn drain_inner(
    queue: &Arc<Queue>,
    _stats: Option<&Stats>,
    mut read: impl FnMut(&mut Task) -> Result<Reply, String>,
) -> Result<usize, CallbackDrainError> {
    let start = Instant::now();
    let mut count = 0;
    for _ in 0..8 {
        if count > 0 && start.elapsed() >= Duration::from_millis(15) {
            break;
        }
        let Some(mut claim) = queue
            .take_next(queue.epoch())
            .map_err(CallbackDrainError::Queue)?
        else {
            break;
        };
        #[cfg(feature = "native-wake-probe")]
        if let Some(stats) = _stats {
            stats.probe_mark(ProbeMark::ClaimStarted);
        }
        let result = read(claim.task_mut());
        #[cfg(feature = "native-wake-probe")]
        if let Some(stats) = _stats {
            stats.probe_mark(ProbeMark::ReadCompleted);
        }
        claim
            .complete(result)
            .map_err(CallbackDrainError::Completion)?;
        #[cfg(feature = "native-wake-probe")]
        if let Some(stats) = _stats {
            stats.probe_mark(ProbeMark::ClaimCompleted);
        }
        count += 1;
        #[cfg(feature = "native-wake-probe")]
        if _stats.is_some() {
            // The dedicated diagnostic admits one lifetime read and never
            // re-arms; stop after publishing that one claim's completion.
            break;
        }
    }
    Ok(count)
}

pub fn rearm_if_queued(queue: &Arc<Queue>, wake: impl FnOnce() -> bool) -> bool {
    !queue.has_queued() || wake()
}

#[derive(Default)]
pub struct Stats {
    pub requests: AtomicU64,
    pub snapshot_calls: AtomicU64,
    pub event_callbacks: AtomicU64,
    pub native_thread: AtomicU64,
    native_wake_calls: AtomicU64,
    native_wake_failures: AtomicU64,
    callback_panics: AtomicU64,
    drain_errors: AtomicU64,
    cancelled_queued_on_shutdown: AtomicU64,
    partial_headers_stopped: AtomicU64,
    diagnostic: Mutex<Diagnostic>,
    #[cfg(feature = "native-wake-probe")]
    probe: Mutex<ProbeDiagnostic>,
}
#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownReason {
    NativeWakeFailed,
    NativeWakeDeliveryTimeout,
    NativeWakeGenerationExhausted,
    CallbackDrainFailed,
    CallbackPanicked,
    ServiceStopped,
    NativeCleanup,
    DocumentRegistryFaulted,
    #[cfg(feature = "native-wake-probe")]
    ProbeDeliveryTimeout,
    #[cfg(feature = "native-wake-probe")]
    ProbeFireLimit,
}
#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallbackStage {
    Entered,
    ThreadId,
    Drain,
    Snapshot,
    Rearm,
    Complete,
}
#[derive(Default)]
struct Diagnostic {
    last_native_wake_status: Option<i32>,
    callback_stage: Option<CallbackStage>,
    last_panic_stage: Option<CallbackStage>,
    // Bounded raw payload remains local to native-stop.json; HTTP never exposes it.
    last_panic_message: Option<String>,
    last_drain_error: Option<&'static str>,
    shutdown_reason: Option<ShutdownReason>,
}
fn increment(counter: &AtomicU64) {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        match counter.compare_exchange_weak(
            current,
            current.saturating_add(1),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
    }
}
impl Stats {
    pub fn native_wake(&self, fire: impl FnOnce() -> i32) -> bool {
        increment(&self.native_wake_calls);
        let status = fire();
        self.diagnostic
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last_native_wake_status = Some(status);
        if status != 0 {
            increment(&self.native_wake_failures);
        }
        status == 0
    }
    pub fn callback_stage(&self, stage: CallbackStage) {
        self.diagnostic
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .callback_stage = Some(stage);
    }
    pub fn shutdown(&self, queue: &Queue, reason: ShutdownReason) {
        // Preserve the first cause even when explicit stop follows a fault.
        self.diagnostic
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .shutdown_reason
            .get_or_insert(reason);
        if let Ok(summary) = queue.shutdown(queue.epoch()) {
            // Only the transition draining these entries reports them. Later
            // cleanup calls return zero; summing cannot invent cancellations.
            self.cancelled_queued_on_shutdown
                .fetch_add(summary.cancelled_queued as u64, Ordering::Relaxed);
        }
    }
    pub fn callback(
        &self,
        queue: &Queue,
        callback: impl FnOnce() -> Result<(), CallbackDrainError>,
    ) -> bool {
        increment(&self.event_callbacks);
        self.callback_stage(CallbackStage::Entered);
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback)) {
            Ok(Ok(())) => {
                self.callback_stage(CallbackStage::Complete);
                true
            }
            Ok(Err(failure)) => {
                increment(&self.drain_errors);
                self.diagnostic
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .last_drain_error = Some(failure.code());
                self.shutdown(queue, ShutdownReason::CallbackDrainFailed);
                false
            }
            Err(payload) => {
                increment(&self.callback_panics);
                let message = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("non-string panic payload");
                let mut end = message.len().min(256);
                while !message.is_char_boundary(end) {
                    end -= 1;
                }
                {
                    let mut diagnostic = self.diagnostic.lock().unwrap_or_else(|e| e.into_inner());
                    diagnostic.last_panic_stage = diagnostic.callback_stage;
                    diagnostic.last_panic_message = Some(message[..end].to_owned());
                }
                self.shutdown(queue, ShutdownReason::CallbackPanicked);
                false
            }
        }
    }
    pub fn json(&self) -> Value {
        let diagnostic = self.diagnostic.lock().unwrap_or_else(|e| e.into_inner());
        let value = json!({"httpRequests":self.requests.load(Ordering::Relaxed),
            "snapshotCalls":self.snapshot_calls.load(Ordering::Relaxed),
            "eventCallbacks":self.event_callbacks.load(Ordering::Relaxed),
            "nativeThreadId":self.native_thread.load(Ordering::Relaxed),
            "nativeWakeCalls":self.native_wake_calls.load(Ordering::Relaxed),
            "nativeWakeFailures":self.native_wake_failures.load(Ordering::Relaxed),
            "lastNativeWakeStatus":diagnostic.last_native_wake_status,
            "callbackPanics":self.callback_panics.load(Ordering::Relaxed),
            "lastCallbackStage":diagnostic.callback_stage,
            "lastCallbackPanicStage":diagnostic.last_panic_stage,
            "drainErrors":self.drain_errors.load(Ordering::Relaxed),
            "lastDrainError":diagnostic.last_drain_error,
            "cancelledQueuedOnShutdown":self.cancelled_queued_on_shutdown.load(Ordering::Relaxed),
            "partialHeadersStopped":self.partial_headers_stopped.load(Ordering::Relaxed),
            "shutdownReason":diagnostic.shutdown_reason});
        drop(diagnostic);
        #[cfg(feature = "native-wake-probe")]
        let value = {
            let mut value = value;
            value["singleReadProbe"] = self.probe_json();
            value
        };
        value
    }
    pub fn diagnostic_json(&self) -> Value {
        let mut value = self.json();
        let diagnostic = self.diagnostic.lock().unwrap_or_else(|e| e.into_inner());
        value["lastCallbackPanicMessage"] = json!(diagnostic.last_panic_message);
        value
    }
}

#[cfg(feature = "native-wake-probe")]
#[derive(Default)]
struct ProbeDiagnostic {
    // Started with the first authenticated admission; every timing uses this
    // one clock, including a callback that arrives before fire returns.
    origin: Option<Instant>,
    deadline: Option<Instant>,
    admitted: bool,
    fire_used: bool,
    admission_us: Option<u64>,
    fire_enter_us: Option<u64>,
    fire_return_us: Option<u64>,
    fire_status: Option<i32>,
    callbacks: u64,
    callbacks_after_deadline: u64,
    first_callback_us: Option<u64>,
    last_callback_us: Option<u64>,
    callback_thread: Option<u32>,
    claim_started_us: Option<u64>,
    read_completed_us: Option<u64>,
    claim_completed_us: Option<u64>,
    outcome_us: Option<u64>,
    outcome: Option<&'static str>,
}
#[cfg(feature = "native-wake-probe")]
impl ProbeDiagnostic {
    fn timestamp(&mut self, now: Instant) -> u64 {
        let origin = self.origin.get_or_insert(now);
        now.saturating_duration_since(*origin)
            .as_micros()
            .min(u64::MAX as u128) as u64
    }
}
#[cfg(feature = "native-wake-probe")]
enum ProbeMark {
    ClaimStarted,
    ReadCompleted,
    ClaimCompleted,
}
#[cfg(feature = "native-wake-probe")]
impl Stats {
    fn probe_admit(&self, deadline: Instant) -> bool {
        let mut probe = self.probe.lock().unwrap_or_else(|e| e.into_inner());
        if probe.admitted {
            return false;
        }
        probe.admitted = true;
        probe.deadline = Some(deadline);
        probe.admission_us = Some(probe.timestamp(Instant::now()));
        true
    }
    fn probe_available(&self) -> bool {
        !self
            .probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .admitted
    }
    pub fn probe_wake(&self, queue: &Queue, fire: impl FnOnce() -> i32) -> bool {
        {
            let mut probe = self.probe.lock().unwrap_or_else(|e| e.into_inner());
            if probe.fire_used {
                drop(probe);
                self.shutdown(queue, ShutdownReason::ProbeFireLimit);
                return false;
            }
            probe.fire_used = true;
            probe.fire_enter_us = Some(probe.timestamp(Instant::now()));
        }
        // No lock spans native fire. A callback may claim/complete the one
        // request before this returns, and those observations are never reset.
        let mut status = 0;
        self.native_wake(|| {
            status = fire();
            status
        });
        {
            let mut probe = self.probe.lock().unwrap_or_else(|e| e.into_inner());
            probe.fire_return_us = Some(probe.timestamp(Instant::now()));
            probe.fire_status = Some(status);
        }
        match status {
            0 | 7 => true, // This diagnostic still permits only one lifetime fire.
            _ => {
                self.shutdown(queue, ShutdownReason::NativeWakeFailed);
                false
            }
        }
    }
    pub fn probe_callback_arrived(&self, thread: u32) {
        let now = Instant::now();
        let mut probe = self.probe.lock().unwrap_or_else(|e| e.into_inner());
        let timestamp = probe.timestamp(now);
        probe.callbacks = probe.callbacks.saturating_add(1);
        if probe.deadline.is_some_and(|deadline| now >= deadline) {
            probe.callbacks_after_deadline = probe.callbacks_after_deadline.saturating_add(1);
        }
        probe.first_callback_us.get_or_insert(timestamp);
        probe.last_callback_us = Some(timestamp);
        probe.callback_thread = Some(thread);
    }
    fn probe_mark(&self, mark: ProbeMark) {
        let mut probe = self.probe.lock().unwrap_or_else(|e| e.into_inner());
        let timestamp = probe.timestamp(Instant::now());
        let target = match mark {
            ProbeMark::ClaimStarted => &mut probe.claim_started_us,
            ProbeMark::ReadCompleted => &mut probe.read_completed_us,
            ProbeMark::ClaimCompleted => &mut probe.claim_completed_us,
        };
        target.get_or_insert(timestamp);
    }
    fn probe_set_outcome(&self, outcome: &'static str) {
        let mut probe = self.probe.lock().unwrap_or_else(|e| e.into_inner());
        probe.outcome_us = Some(probe.timestamp(Instant::now()));
        probe.outcome = Some(outcome);
    }
    fn probe_outcome(&self, queue: &Queue, outcome: &Outcome<Result<Reply, String>>) {
        use fusion_native_dispatch::{NotExecutedReason, UnknownReason};
        let (label, timed_out) = match outcome {
            Outcome::Completed(Ok(_)) => ("completed", false),
            Outcome::Completed(Err(_)) => ("native_read_failed", false),
            Outcome::NotExecuted(NotExecutedReason::DeadlineElapsed) => {
                ("not_executed_deadline", true)
            }
            Outcome::Unknown(UnknownReason::DeadlineWhileExecuting) => ("unknown_deadline", true),
            Outcome::NotExecuted(_) => ("not_executed", false),
            Outcome::Unknown(_) => ("unknown", false),
        };
        self.probe_set_outcome(label);
        if timed_out {
            self.shutdown(queue, ShutdownReason::ProbeDeliveryTimeout);
        }
    }
    fn probe_json(&self) -> Value {
        let probe = self.probe.lock().unwrap_or_else(|e| e.into_inner());
        let deadline_us = probe.origin.zip(probe.deadline).map(|(origin, deadline)| {
            deadline
                .saturating_duration_since(origin)
                .as_micros()
                .min(u64::MAX as u128) as u64
        });
        json!({"experimental":true,"lifetimeDocumentsLimit":1,"lifetimeFireLimit":1,
            "rearmEnabled":false,"documentsAdmissionConsumed":probe.admitted,
            "fireConsumed":probe.fire_used,"timeUnit":"microseconds_from_first_observation",
            "admissionUs":probe.admission_us,"deadlineUs":deadline_us,
            "fireEnterUs":probe.fire_enter_us,"fireReturnUs":probe.fire_return_us,
            "fireStatus":probe.fire_status,"fireAcknowledgementUnconfirmed":probe.fire_status==Some(7),
            "callbackCount":probe.callbacks,"callbacksAfterDeadline":probe.callbacks_after_deadline,
            "firstCallbackUs":probe.first_callback_us,"lastCallbackUs":probe.last_callback_us,
            "callbackThreadId":probe.callback_thread,"claimStartedUs":probe.claim_started_us,
            "readCompletedUs":probe.read_completed_us,"claimCompletedUs":probe.claim_completed_us,
            "requestOutcomeUs":probe.outcome_us,"requestOutcome":probe.outcome})
    }
}

pub struct Service {
    address: SocketAddr,
    queue: Arc<Queue>,
    stop: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
    stats: Arc<Stats>,
}

impl Service {
    pub fn start(
        listener: TcpListener,
        queue: Arc<Queue>,
        token: String,
        wake: Wake,
        stats: Arc<Stats>,
    ) -> std::io::Result<Self> {
        Self::start_with_status(listener, queue, token, wake, stats, None)
    }
    pub fn start_with_status(
        listener: TcpListener,
        queue: Arc<Queue>,
        token: String,
        wake: Wake,
        stats: Arc<Stats>,
        operation_status: Option<Arc<OperationStatusReader>>,
    ) -> std::io::Result<Self> {
        Self::start_inner(listener, queue, token, wake, stats, operation_status, false)
    }
    pub fn start_with_operations(
        listener: TcpListener,
        queue: Arc<Queue>,
        token: String,
        wake: Wake,
        stats: Arc<Stats>,
        operation_status: Arc<OperationStatusReader>,
    ) -> std::io::Result<Self> {
        Self::start_inner(
            listener,
            queue,
            token,
            wake,
            stats,
            Some(operation_status),
            true,
        )
    }
    fn start_inner(
        listener: TcpListener,
        queue: Arc<Queue>,
        token: String,
        wake: Wake,
        stats: Arc<Stats>,
        operation_status: Option<Arc<OperationStatusReader>>,
        enable_operations: bool,
    ) -> std::io::Result<Self> {
        if operation_status
            .as_ref()
            .is_some_and(|reader| reader.epoch() != queue.epoch().to_string())
        {
            return Err(std::io::Error::other(
                "journal and transport runtime epochs differ",
            ));
        }
        let address = listener.local_addr()?;
        if !address.ip().is_loopback() || token.len() < 32 {
            return Err(std::io::Error::other(
                "local endpoint and strong token required",
            ));
        }
        listener.set_nonblocking(true)?;
        // Windows Rust 1.95 duplicate socket handles do not preserve the
        // nonblocking listener mode. Share this configured handle instead of
        // try_clone(), otherwise an idle accept can prevent worker teardown.
        let listener = Arc::new(listener);
        let mut service = Self {
            address,
            queue: Arc::clone(&queue),
            stop: Arc::new(AtomicBool::new(false)),
            workers: Vec::new(),
            stats: Arc::clone(&stats),
        };
        let executor = if enable_operations {
            operation_status.as_ref().map(|reader| {
                Executor::new(Arc::clone(&queue), Arc::clone(&wake), Arc::clone(reader))
            })
        } else {
            None
        };
        if let Some(reader) = operation_status.as_ref() {
            let reader = Arc::clone(reader);
            let stop = Arc::clone(&service.stop);
            let executor = executor.clone();
            service.workers.push(
                thread::Builder::new()
                    .name("fusion-rust-journal".into())
                    .spawn(move || {
                        if !stop.load(Ordering::Acquire) {
                            if let Some(executor) = executor {
                                executor.run(&stop);
                            } else {
                                reader.initialize();
                            }
                        }
                    })?,
            );
        }
        for index in 0..2 {
            let socket = Arc::clone(&listener);
            let queue = Arc::clone(&queue);
            let stop = Arc::clone(&service.stop);
            let token = token.clone();
            let wake = Arc::clone(&wake);
            let stats = Arc::clone(&stats);
            let operation_status = operation_status.clone();
            let executor = executor.clone();
            // On a later spawn failure, Service::drop joins the workers already
            // created. There are no unowned accept/parser/background threads.
            service.workers.push(
                thread::Builder::new()
                    .name(format!("fusion-rust-http-{index}"))
                    .spawn(move || {
                        let host = address.to_string();
                        let epoch = queue.epoch().to_string();
                        while !stop.load(Ordering::Acquire) {
                            match socket.accept() {
                                Ok((mut stream, peer)) => {
                                    if !peer.ip().is_loopback() {
                                        continue;
                                    }
                                    // Accepted-socket mode is platform-specific.
                                    // Header/response I/O uses bounded blocking
                                    // reads and writes, independent of accept.
                                    if stream.set_nonblocking(false).is_err() {
                                        continue;
                                    }
                                    stats.requests.fetch_add(1, Ordering::Relaxed);
                                    let (status, reply) = handle(
                                        &mut stream,
                                        &stop,
                                        &host,
                                        &token,
                                        &epoch,
                                        &queue,
                                        &wake,
                                        &stats,
                                        operation_status.as_deref(),
                                        executor.as_deref(),
                                    );
                                    let _ = http::respond(&mut stream, status, &reply);
                                    // Closing the stream terminates body/pipeline attempts.
                                }
                                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                    thread::sleep(Duration::from_millis(10))
                                }
                                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                                    continue
                                }
                                Err(_) => break,
                            }
                        }
                    })?,
            );
        }
        Ok(service)
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn stop(&mut self) -> bool {
        self.stop.store(true, Ordering::Release);
        self.stats
            .shutdown(&self.queue, ShutdownReason::ServiceStopped);
        let mut joined = true;
        for worker in self.workers.drain(..) {
            joined &= worker.join().is_ok();
        }
        joined
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.stop();
    }
}

fn handle(
    stream: &mut TcpStream,
    stop: &AtomicBool,
    host: &str,
    token: &str,
    epoch: &str,
    queue: &Arc<Queue>,
    wake: &Wake,
    stats: &Stats,
    operation_status: Option<&OperationStatusReader>,
    executor: Option<&Executor>,
) -> (u16, Value) {
    let mut stopped_header_bytes = 0;
    let request =
        match http::read_tracking(stream, stop, host, token, epoch, &mut stopped_header_bytes) {
            Ok(value) => value,
            Err(error) => {
                if stopped_header_bytes > 0 {
                    increment(&stats.partial_headers_stopped);
                }
                return (
                    error.status,
                    json!({"success":false,"errorCode":error.code}),
                );
            }
        };
    if stop.load(Ordering::Acquire) {
        return (503, json!({"success":false,"errorCode":"runtime_stopping"}));
    }
    if request.route == http::Route::Health {
        // Observers may run before a deadline waiter is scheduled. Reconcile
        // an overdue unobserved generation before reporting readiness.
        wake.settled();
        #[cfg(not(feature = "native-wake-probe"))]
        let (ready, scope) = (
            !queue.is_shutdown(),
            "queue accepting work; not proof of a completed SDK callback",
        );
        #[cfg(feature = "native-wake-probe")]
        let (ready, scope) = (!queue.is_shutdown() && stats.probe_available(), "EXPERIMENTAL single-read diagnostic; one lifetime documents admission, one fire, no rearm; not production dispatch qualification");
        let journal = operation_status
            .map(OperationStatusReader::health)
            .unwrap_or_else(|| json!({"state":"not_configured","connected":false}));
        return (
            200,
            json!({"success":true,"epoch":epoch,"implementation":"Rust with native C++ SDK adapter",
            "operations":if executor.is_some() {vec!["document_snapshot","operation_status","inspect","parameter_create","parameter_set","new_design","native_scratch_discard","native_document_activate","bom","bom_export","sketch_create","native_sketch_inspect","sketch_geometry","native_sketch_profiles","feature_create","native_sketch_entities","material_list","mcmaster_discover","assembly_properties"]} else if operation_status.is_some() {vec!["document_snapshot","operation_status"]} else {vec!["document_snapshot"]},
            "mutationSupport":executor.is_some_and(Executor::ready),"journalConnected":journal["connected"],
            "effectJournalConfigured":executor.is_some(),
            "effectJournalConnected":executor.is_some() && journal["connected"] == true,
            "journal":journal,
            "dispatchReady":ready,"dispatchReadyScope":scope, "stats":stats.json()}),
        );
    }
    if request.route == http::Route::Capabilities {
        return if executor.is_some() {
            (200, crate::execution::capabilities())
        } else {
            (
                200,
                json!({"success":true,"mutationSupport":false,"operations":["document_snapshot","operation_status"]}),
            )
        };
    }
    if request.route == http::Route::Mechanical {
        let Some(executor) = executor else {
            return (
                503,
                json!({"success":false,"errorCode":"native_execution_not_configured","nativeExecutionStarted":false}),
            );
        };
        let prepared = match fusion_mechanical_core::prepare_json(&request.body, epoch) {
            Ok(request) => request,
            Err(error) => {
                return (
                    400,
                    json!({"success":false,"errorCode":error.code,"nativeExecutionStarted":false}),
                )
            }
        };
        if crate::identity::document_lifecycle::is_lifecycle(prepared.operation())
            && !matches!(
                request.mechanical_contract.as_deref(),
                Some(
                    "native-lifecycle-v1"
                        | "native-bom-v1"
                        | "native-sketch-v1"
                        | "native-sketch-geometry-v1"
                        | "native-extrude-v1"
                        | "native-sketch-entities-v1"
                        | "native-material-catalog-v1"
                        | "native-purchased-discovery-v1"
                        | "native-root-physical-v1"
                        | "native-sketch-line-state-v1"
                        | "native-sketch-relations-v1"
                        | "native-sketch-dimensions-v1"
                        | "native-root-assembly-v1"
                )
            )
        {
            return (
                409,
                json!({"success":false,"errorCode":"native_lifecycle_contract_required","nativeExecutionStarted":false}),
            );
        }
        if matches!(prepared.operation(), "bom" | "bom_export")
            && !matches!(
                request.mechanical_contract.as_deref(),
                Some(
                    "native-bom-v1"
                        | "native-sketch-v1"
                        | "native-sketch-geometry-v1"
                        | "native-extrude-v1"
                        | "native-sketch-entities-v1"
                        | "native-material-catalog-v1"
                        | "native-purchased-discovery-v1"
                        | "native-root-physical-v1"
                        | "native-sketch-line-state-v1"
                        | "native-sketch-relations-v1"
                        | "native-sketch-dimensions-v1"
                        | "native-root-assembly-v1"
                )
            )
        {
            return (
                409,
                json!({"success":false,"errorCode":"native_bom_contract_required","nativeExecutionStarted":false}),
            );
        }
        if matches!(
            prepared.operation(),
            "sketch_create" | "native_sketch_inspect"
        ) && !matches!(
            request.mechanical_contract.as_deref(),
            Some(
                "native-sketch-v1"
                    | "native-sketch-geometry-v1"
                    | "native-extrude-v1"
                    | "native-sketch-entities-v1"
                    | "native-material-catalog-v1"
                    | "native-purchased-discovery-v1"
                    | "native-root-physical-v1"
                    | "native-sketch-line-state-v1"
                    | "native-sketch-relations-v1"
                    | "native-sketch-dimensions-v1"
                    | "native-root-assembly-v1"
            )
        ) {
            return (
                409,
                json!({"success":false,"errorCode":"native_sketch_contract_required","nativeExecutionStarted":false}),
            );
        }
        if prepared.operation() == "sketch_geometry"
            && !matches!(
                request.mechanical_contract.as_deref(),
                Some(
                    "native-sketch-geometry-v1"
                        | "native-extrude-v1"
                        | "native-sketch-entities-v1"
                        | "native-material-catalog-v1"
                        | "native-purchased-discovery-v1"
                        | "native-root-physical-v1"
                        | "native-sketch-line-state-v1"
                        | "native-sketch-relations-v1"
                        | "native-sketch-dimensions-v1"
                        | "native-root-assembly-v1"
                )
            )
        {
            return (
                409,
                json!({"success":false,"errorCode":"native_geometry_contract_required","nativeExecutionStarted":false}),
            );
        }
        if matches!(
            prepared.operation(),
            "feature_create" | "native_sketch_profiles"
        ) && !matches!(
            request.mechanical_contract.as_deref(),
            Some(
                "native-extrude-v1"
                    | "native-sketch-entities-v1"
                    | "native-material-catalog-v1"
                    | "native-purchased-discovery-v1"
                    | "native-root-physical-v1"
                    | "native-sketch-line-state-v1"
                    | "native-sketch-relations-v1"
                    | "native-sketch-dimensions-v1"
                    | "native-root-assembly-v1"
            )
        ) {
            return (
                409,
                json!({"success":false,"errorCode":"native_extrude_contract_required","nativeExecutionStarted":false}),
            );
        }
        if prepared.operation() == "native_sketch_entities"
            && !matches!(
                request.mechanical_contract.as_deref(),
                Some(
                    "native-sketch-entities-v1"
                        | "native-material-catalog-v1"
                        | "native-purchased-discovery-v1"
                        | "native-root-physical-v1"
                        | "native-sketch-line-state-v1"
                        | "native-sketch-relations-v1"
                        | "native-sketch-dimensions-v1"
                        | "native-root-assembly-v1"
                )
            )
        {
            return (
                409,
                json!({"success":false,"errorCode":"native_entities_contract_required","nativeExecutionStarted":false}),
            );
        }
        if prepared.operation() == "material_list"
            && !matches!(
                request.mechanical_contract.as_deref(),
                Some(
                    "native-material-catalog-v1"
                        | "native-purchased-discovery-v1"
                        | "native-root-physical-v1"
                        | "native-sketch-line-state-v1"
                        | "native-sketch-relations-v1"
                        | "native-sketch-dimensions-v1"
                        | "native-root-assembly-v1"
                )
            )
        {
            return (
                409,
                json!({"success":false,"errorCode":"native_material_catalog_contract_required","nativeExecutionStarted":false}),
            );
        }
        if prepared.operation() == "mcmaster_discover"
            && !matches!(
                request.mechanical_contract.as_deref(),
                Some(
                    "native-purchased-discovery-v1"
                        | "native-root-physical-v1"
                        | "native-sketch-line-state-v1"
                        | "native-sketch-relations-v1"
                        | "native-sketch-dimensions-v1"
                        | "native-root-assembly-v1"
                )
            )
        {
            return (
                409,
                json!({"success":false,"errorCode":"native_purchased_discovery_contract_required","nativeExecutionStarted":false}),
            );
        }
        if prepared.operation() == "assembly_properties"
            && !matches!(
                request.mechanical_contract.as_deref(),
                Some(
                    crate::physical::CONTRACT
                        | crate::sketches::line_state::CONTRACT
                        | crate::sketches::relations::CONTRACT
                        | crate::sketches::dimensions::CONTRACT
                        | crate::assembly::CONTRACT
                )
            )
        {
            return (
                409,
                json!({"success":false,"errorCode":"native_root_physical_contract_required","nativeExecutionStarted":false}),
            );
        }
        if prepared.operation() == "native_sketch_line_state"
            && !matches!(
                request.mechanical_contract.as_deref(),
                Some(
                    crate::sketches::line_state::CONTRACT
                        | crate::sketches::relations::CONTRACT
                        | crate::sketches::dimensions::CONTRACT
                        | crate::assembly::CONTRACT
                )
            )
        {
            return (
                409,
                json!({"success":false,"errorCode":"native_line_state_contract_required","nativeExecutionStarted":false}),
            );
        }
        if matches!(
            prepared.operation(),
            "native_sketch_relations" | "sketch_constraint"
        ) && !matches!(
            request.mechanical_contract.as_deref(),
            Some(
                crate::sketches::relations::CONTRACT
                    | crate::sketches::dimensions::CONTRACT
                    | crate::assembly::CONTRACT
            )
        ) {
            return (
                409,
                json!({"success":false,"errorCode":"native_relations_contract_required","nativeExecutionStarted":false}),
            );
        }
        if matches!(
            prepared.operation(),
            "native_sketch_dimensions" | "sketch_dimension"
        ) && !matches!(
            request.mechanical_contract.as_deref(),
            Some(crate::sketches::dimensions::CONTRACT | crate::assembly::CONTRACT)
        ) {
            return (
                409,
                json!({"success":false,"errorCode":"native_dimensions_contract_required","nativeExecutionStarted":false}),
            );
        }
        if prepared.operation() == "occurrence_update"
            && request.mechanical_contract.as_deref() != Some(crate::assembly::placement::CONTRACT)
        {
            return (
                409,
                json!({"success":false,"errorCode":"native_placement_contract_required","nativeExecutionStarted":false}),
            );
        }
        if ((crate::assembly::is_write(prepared.operation())
            && prepared.operation() != "occurrence_update")
            || prepared.operation() == "native_assembly_inspect")
            && request.mechanical_contract.as_deref() != Some(crate::assembly::CONTRACT)
        {
            return (
                409,
                json!({"success":false,"errorCode":"native_assembly_contract_required","nativeExecutionStarted":false}),
            );
        }
        let (code, mut body) = executor.request(prepared, Instant::now() + request.budget);
        body["epoch"] = json!(epoch);
        return (code, body);
    }
    if let http::Route::OperationStatus { session, operation } = &request.route {
        let (code, mut body) = match operation_status {
            Some(reader) => reader.query(session, operation, Instant::now() + request.budget),
            None => (
                503,
                json!({"success":false,"errorCode":"operation_status_not_configured","outcome":"unknown","reconciliationRequired":true,"nativeExecutionAuthorized":false}),
            ),
        };
        body["epoch"] = json!(epoch);
        body["effectJournalConfigured"] = json!(executor.is_some());
        body["effectJournalConnected"] = json!(
            executor.is_some()
                && operation_status.is_some_and(|reader| reader.health()["connected"] == true)
        );
        return (code, body);
    }
    let deadline = Instant::now() + request.budget;
    #[cfg(feature = "native-wake-probe")]
    if !stats.probe_admit(deadline) {
        return (
            503,
            json!({"success":false,"errorCode":"diagnostic_probe_already_used","nativeExecutionStarted":false}),
        );
    }
    let pending = match queue.submit(Task::Snapshot, deadline) {
        Ok(value) => value,
        Err(error) => {
            #[cfg(feature = "native-wake-probe")]
            stats.probe_set_outcome("admission_refused");
            return (
                503,
                json!({"success":false,"errorCode":"dispatch_refused","detail":error.to_string()}),
            );
        }
    };
    let request_id = pending.id().to_string();
    let outcome = if wake.request() {
        pending.wait()
    } else {
        pending.cancel()
    };
    wake.settled();
    #[cfg(feature = "native-wake-probe")]
    stats.probe_outcome(queue, &outcome);
    match outcome {
        Outcome::Completed(Ok(Reply::Snapshot(snapshot))) => (
            200,
            json!({"success":true,"epoch":epoch,"requestId":request_id,"data":snapshot}),
        ),
        Outcome::Completed(Ok(_)) => (
            500,
            json!({"success":false,"errorCode":"unexpected_native_reply"}),
        ),
        Outcome::Completed(Err(error)) => (
            500,
            json!({"success":false,"epoch":epoch,"requestId":request_id,"errorCode":"native_read_failed","error":error}),
        ),
        Outcome::NotExecuted(reason) => (
            503,
            json!({"success":false,"epoch":epoch,"requestId":request_id,"errorCode":"dispatch_not_executed","nativeExecutionStarted":false,"detail":format!("{reason:?}")}),
        ),
        Outcome::Unknown(reason) => (
            503,
            json!({"success":false,"epoch":epoch,"requestId":request_id,"errorCode":"operation_reconciliation_required","outcome":"unknown","detail":format!("{reason:?}")}),
        ),
    }
}

#[cfg(test)]
mod tests {
    include!("service_physical_tests.rs");
    include!("service_line_state_tests.rs");
    include!("service_relations_tests.rs");
    include!("service_dimensions_tests.rs");
    include!("service_assembly_tests.rs");
    use super::*;
    use std::io::{Read, Write};
    const TOKEN: &str = "local-test-token-of-at-least-thirty-two-bytes";
    #[test]
    fn repeated_shutdown_counts_only_actual_queued_cancellations() {
        let queue = Queue::new(2).unwrap();
        let stats = Stats::default();
        let pending = queue
            .submit(Task::Snapshot, Instant::now() + Duration::from_secs(2))
            .unwrap();
        stats.shutdown(&queue, ShutdownReason::ServiceStopped);
        stats.shutdown(&queue, ShutdownReason::NativeCleanup);
        assert!(matches!(
            pending.wait(),
            Outcome::NotExecuted(fusion_native_dispatch::NotExecutedReason::Shutdown)
        ));
        let evidence = stats.json();
        assert_eq!(evidence["cancelledQueuedOnShutdown"], 1);
        assert_eq!(evidence["partialHeadersStopped"], 0);
        assert_eq!(evidence["shutdownReason"], "service_stopped");
    }
    #[cfg(feature = "native-wake-probe")]
    #[test]
    fn probe_false_ack_waits_for_same_request_and_never_fires_twice() {
        let queue = Queue::new(2).unwrap();
        let stats = Stats::default();
        let deadline = Instant::now() + Duration::from_secs(2);
        assert!(stats.probe_admit(deadline));
        let pending = queue.submit(Task::Snapshot, deadline).unwrap();
        assert!(stats.probe_wake(&queue, || 7));
        assert!(!queue.is_shutdown());
        assert!(stats.callback(&queue, || {
            stats.probe_callback_arrived(42);
            drain_with_stats(&queue, &stats, || Err("read-only observation".into())).map(|_| ())
        }));
        let outcome = pending.wait();
        assert!(matches!(outcome, Outcome::Completed(Err(_))));
        stats.probe_outcome(&queue, &outcome);
        assert!(!stats.probe_admit(deadline));
        let data = stats.json();
        assert_eq!(data["nativeWakeCalls"], 1);
        assert_eq!(
            data["singleReadProbe"]["fireAcknowledgementUnconfirmed"],
            true
        );
        assert_eq!(
            data["singleReadProbe"]["requestOutcome"],
            "native_read_failed"
        );
        for key in ["claimStartedUs", "readCompletedUs", "claimCompletedUs"] {
            assert!(data["singleReadProbe"][key].is_u64());
        }
        assert!(!stats.probe_wake(&queue, || panic!("a second SDK fire is forbidden")));
        assert_eq!(stats.json()["nativeWakeCalls"], 1);
    }
    #[cfg(feature = "native-wake-probe")]
    #[test]
    fn probe_late_false_ack_does_not_erase_a_completed_claim() {
        let queue = Queue::new(2).unwrap();
        let stats = Stats::default();
        let deadline = Instant::now() + Duration::from_secs(2);
        assert!(stats.probe_admit(deadline));
        let pending = queue.submit(Task::Snapshot, deadline).unwrap();
        assert!(stats.probe_wake(&queue, || {
            assert!(stats.callback(&queue, || {
                stats.probe_callback_arrived(42);
                drain_with_stats(&queue, &stats, || {
                    Err("completed before acknowledgement".into())
                })
                .map(|_| ())
            }));
            7
        }));
        let outcome = pending.wait();
        assert!(matches!(outcome, Outcome::Completed(_)));
        stats.probe_outcome(&queue, &outcome);
        let probe = stats.probe_json();
        assert!(
            probe["claimCompletedUs"].as_u64().unwrap() <= probe["fireReturnUs"].as_u64().unwrap()
        );
        assert_eq!(probe["callbackCount"], 1);
        assert!(!queue.is_shutdown());
    }
    #[cfg(feature = "native-wake-probe")]
    #[test]
    fn probe_deadline_cancels_unclaimed_work_and_counts_late_callback_without_read() {
        let queue = Queue::new(2).unwrap();
        let stats = Stats::default();
        let deadline = Instant::now() + Duration::from_millis(30);
        assert!(stats.probe_admit(deadline));
        let pending = queue.submit(Task::Snapshot, deadline).unwrap();
        assert!(stats.probe_wake(&queue, || 7));
        let outcome = pending.wait();
        stats.probe_outcome(&queue, &outcome);
        assert!(matches!(
            outcome,
            Outcome::NotExecuted(fusion_native_dispatch::NotExecutedReason::DeadlineElapsed)
        ));
        assert!(queue.is_shutdown());
        assert!(!stats.callback(&queue, || {
            stats.probe_callback_arrived(42);
            drain_with_stats(&queue, &stats, || panic!("late event must never read SDK"))
                .map(|_| ())
        }));
        let data = stats.json();
        assert_eq!(data["singleReadProbe"]["callbacksAfterDeadline"], 1);
        assert_eq!(
            data["singleReadProbe"]["requestOutcome"],
            "not_executed_deadline"
        );
        assert!(data["singleReadProbe"]["claimStartedUs"].is_null());
        assert_eq!(data["shutdownReason"], "probe_delivery_timeout");
    }
    #[cfg(feature = "native-wake-probe")]
    #[test]
    fn probe_claimed_timeout_remains_unknown_after_late_read_completion() {
        let queue = Queue::new(2).unwrap();
        let stats = Arc::new(Stats::default());
        let deadline = Instant::now() + Duration::from_millis(200);
        assert!(stats.probe_admit(deadline));
        let pending = queue.submit(Task::Snapshot, deadline).unwrap();
        assert!(stats.probe_wake(&queue, || 7));
        let observer_queue = Arc::clone(&queue);
        let observer_stats = Arc::clone(&stats);
        let (settled, settled_rx) = std::sync::mpsc::channel();
        let observer = thread::spawn(move || {
            let outcome = pending.wait();
            observer_stats.probe_outcome(&observer_queue, &outcome);
            settled.send(()).unwrap();
            outcome
        });
        assert!(stats.callback(&queue, || {
            stats.probe_callback_arrived(42);
            drain_with_stats(&queue, &stats, || {
                // Publish the read only after the observer has settled its
                // timeout; thread scheduling cannot turn this into Completed.
                settled_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                Err("late read result".into())
            })
            .map(|_| ())
        }));
        assert!(matches!(
            observer.join().unwrap(),
            Outcome::Unknown(fusion_native_dispatch::UnknownReason::DeadlineWhileExecuting)
        ));
        let probe = stats.probe_json();
        assert_eq!(probe["requestOutcome"], "unknown_deadline");
        assert!(probe["claimCompletedUs"].is_u64());
        assert!(queue.is_shutdown());
    }
    #[cfg(feature = "native-wake-probe")]
    #[test]
    fn probe_non_delivery_adapter_faults_remain_hard_failures() {
        for status in [1, 4, 8] {
            let queue = Queue::new(2).unwrap();
            let stats = Stats::default();
            let deadline = Instant::now() + Duration::from_secs(2);
            assert!(stats.probe_admit(deadline));
            let pending = queue.submit(Task::Snapshot, deadline).unwrap();
            assert!(!stats.probe_wake(&queue, || status));
            assert!(matches!(pending.wait(), Outcome::NotExecuted(_)));
            assert!(queue.is_shutdown());
            let data = stats.json();
            assert_eq!(data["lastNativeWakeStatus"], status);
            assert_eq!(data["shutdownReason"], "native_wake_failed");
        }
    }
    #[cfg(feature = "native-wake-probe")]
    #[test]
    fn probe_lifetime_gate_rejects_second_authenticated_http_request() {
        let queue = Queue::new(2).unwrap();
        let stats = Arc::new(Stats::default());
        let (sender, receiver) = std::sync::mpsc::channel();
        let fire_stats = Arc::clone(&stats);
        let fire_queue = Arc::clone(&queue);
        let mut server = Service::start(
            TcpListener::bind("127.0.0.1:0").unwrap(),
            Arc::clone(&queue),
            TOKEN.into(),
            Arc::new(move || {
                fire_stats.probe_wake(&fire_queue, || {
                    sender.send(()).unwrap();
                    7
                })
            }),
            Arc::clone(&stats),
        )
        .unwrap();
        let address = server.address();
        let epoch = queue.epoch().to_string();
        let first_epoch = epoch.clone();
        let first = thread::spawn(move || exchange(address, &first_epoch, "/v1/documents", TOKEN));
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        // The other HTTP worker sees the same lifetime admission gate.
        let (status, body) = exchange(address, &epoch, "/v1/documents", TOKEN);
        assert_eq!(status, 503);
        assert_eq!(body["errorCode"], "diagnostic_probe_already_used");
        let health = exchange(address, &epoch, "/v1/health", TOKEN).1;
        assert_eq!(health["dispatchReady"], false);
        assert!(health["dispatchReadyScope"]
            .as_str()
            .unwrap()
            .contains("EXPERIMENTAL"));
        drain_with_stats(&queue, &stats, || Err("single read observed".into())).unwrap();
        assert_eq!(first.join().unwrap().0, 500);
        assert_eq!(
            exchange(address, &epoch, "/v1/documents", TOKEN).1["errorCode"],
            "diagnostic_probe_already_used"
        );
        assert_eq!(stats.json()["nativeWakeCalls"], 1);
        assert!(server.stop());
    }
    #[test]
    fn callback_panic_is_bounded_internal_only_and_preserves_first_shutdown_cause() {
        let queue = Queue::new(2).unwrap();
        let pending = queue
            .submit(Task::Snapshot, Instant::now() + Duration::from_secs(5))
            .unwrap();
        let stats = Stats::default();
        assert!(!stats.callback(&queue, || {
            stats.callback_stage(CallbackStage::Drain);
            panic!("{}", "private diagnostic 界".repeat(100));
        }));
        assert!(queue.is_shutdown());
        assert!(matches!(pending.wait(), Outcome::NotExecuted(_)));
        stats.shutdown(&queue, ShutdownReason::ServiceStopped);
        let public = stats.json();
        assert_eq!(public["callbackPanics"], 1);
        assert_eq!(public["eventCallbacks"], 1);
        assert_eq!(public["snapshotCalls"], 0);
        assert_eq!(public["lastCallbackPanicStage"], "drain");
        assert_eq!(public["shutdownReason"], "callback_panicked");
        assert!(public.get("lastCallbackPanicMessage").is_none());
        assert!(!public.to_string().contains("private diagnostic"));
        let private = stats.diagnostic_json();
        let message = private["lastCallbackPanicMessage"].as_str().unwrap();
        assert!(!message.is_empty() && message.len() <= 256);
        assert!(message.starts_with("private diagnostic"));
    }
    #[test]
    fn wrong_thread_drain_is_recorded_and_stops_queue_without_snapshot_or_retry() {
        let queue = Queue::new(2).unwrap();
        let pending = queue
            .submit(Task::Snapshot, Instant::now() + Duration::from_secs(5))
            .unwrap();
        let stats = Arc::new(Stats::default());
        let worker_queue = Arc::clone(&queue);
        let worker_stats = Arc::clone(&stats);
        assert!(
            !thread::spawn(move || worker_stats.callback(&worker_queue, || {
                worker_stats.callback_stage(CallbackStage::Drain);
                drain(&worker_queue, || {
                    panic!("SDK must never be reached on wrong thread")
                })
                .map(|_| ())
            }))
            .join()
            .unwrap()
        );
        assert!(queue.is_shutdown());
        assert!(matches!(pending.wait(), Outcome::NotExecuted(_)));
        let data = stats.json();
        assert_eq!(data["drainErrors"], 1);
        assert_eq!(data["lastDrainError"], "queue_wrong_thread");
        assert_eq!(data["snapshotCalls"], 0);
        assert_eq!(data["callbackPanics"], 0);
        assert_eq!(data["shutdownReason"], "callback_drain_failed");
    }
    #[test]
    fn native_wake_status_survives_boolean_failure_and_counters_never_wrap() {
        let stats = Arc::new(Stats::default());
        assert!(stats.native_wake(|| 0));
        let queue = Queue::new(2).unwrap();
        let pending = queue
            .submit(Task::Snapshot, Instant::now() + Duration::from_secs(5))
            .unwrap();
        let wake_stats = Arc::clone(&stats);
        let wake = WakeCoalescer::with_stats(
            Arc::clone(&queue),
            Arc::new(move || {
                wake_stats.native_wake(|| 4);
                WakeAck::HardFailure(4)
            }),
            Arc::clone(&stats),
        );
        assert!(!wake.request());
        assert!(!wake.request());
        assert!(matches!(pending.wait(), Outcome::NotExecuted(_)));
        let data = stats.json();
        assert_eq!(data["nativeWakeCalls"], 2);
        assert_eq!(data["nativeWakeFailures"], 1);
        assert_eq!(data["lastNativeWakeStatus"], 4);
        assert_eq!(data["shutdownReason"], "native_wake_failed");
        stats.native_wake_calls.store(u64::MAX, Ordering::Relaxed);
        stats
            .native_wake_failures
            .store(u64::MAX, Ordering::Relaxed);
        assert!(!stats.native_wake(|| -3));
        let data = stats.json();
        assert_eq!(data["nativeWakeCalls"], u64::MAX);
        assert_eq!(data["nativeWakeFailures"], u64::MAX);
        assert_eq!(data["lastNativeWakeStatus"], -3);
    }
    #[test]
    fn health_reports_stopped_dispatch_while_listener_remains_available() {
        let queue = Queue::new(2).unwrap();
        let stats = Arc::new(Stats::default());
        let mut server = Service::start(
            TcpListener::bind("127.0.0.1:0").unwrap(),
            Arc::clone(&queue),
            TOKEN.into(),
            Arc::new(|| panic!("health and refused admission must not wake SDK")),
            Arc::clone(&stats),
        )
        .unwrap();
        let address = server.address();
        let epoch = queue.epoch().to_string();
        assert_eq!(
            exchange(address, &epoch, "/v1/health", TOKEN).1["dispatchReady"],
            true
        );
        stats.shutdown(&queue, ShutdownReason::NativeWakeFailed);
        let (status, body) = exchange(address, &epoch, "/v1/health", TOKEN);
        assert_eq!(status, 200);
        assert_eq!(body["dispatchReady"], false);
        assert_eq!(body["stats"]["shutdownReason"], "native_wake_failed");
        assert_eq!(body["mutationSupport"], false);
        assert_eq!(exchange(address, &epoch, "/v1/documents", TOKEN).0, 503);
        assert!(server.stop());
    }
    #[test]
    fn stop_joins_idle_accept_workers_without_any_client() {
        let queue = Queue::new(2).unwrap();
        let mut server = Service::start(
            TcpListener::bind("127.0.0.1:0").unwrap(),
            queue,
            TOKEN.into(),
            Arc::new(|| panic!("no request")),
            Arc::new(Stats::default()),
        )
        .unwrap();
        // Give both workers time to reach accept, not merely the stop flag.
        thread::sleep(Duration::from_millis(30));
        let start = Instant::now();
        assert!(server.stop());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn concurrent_producers_keep_at_most_one_sdk_wake_pending() {
        let queue = Queue::new(32).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let posted = Arc::new(AtomicU64::new(0));
        let observed = Arc::clone(&posted);
        let wake = WakeCoalescer::new(
            Arc::clone(&queue),
            Arc::new(move || {
                assert_eq!(
                    observed.fetch_add(1, Ordering::SeqCst),
                    0,
                    "unbounded SDK wake backlog"
                );
                sender.send(()).unwrap();
                WakeAck::Acknowledged
            }),
        );
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let queue = Arc::clone(&queue);
                let wake = Arc::clone(&wake);
                thread::spawn(move || {
                    for _ in 0..64 {
                        let pending = queue
                            .submit(Task::Snapshot, Instant::now() + Duration::from_secs(10))
                            .unwrap();
                        assert!(wake.request());
                        assert!(matches!(pending.wait(), Outcome::Completed(_)));
                    }
                })
            })
            .collect();
        let mut completed = 0;
        while completed < 512 {
            receiver.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(posted.fetch_sub(1, Ordering::SeqCst), 1);
            let generation = wake.begin_callback().unwrap();
            completed += drain(&queue, || Err("simulated SDK observation".into())).unwrap();
            assert!(wake.finish_callback(generation));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        // A late producer can post one harmless wake for work drained just
        // before request() ran. It must never rearm an empty queue.
        if receiver.try_recv().is_ok() {
            assert_eq!(posted.fetch_sub(1, Ordering::SeqCst), 1);
            let generation = wake.begin_callback().unwrap();
            assert_eq!(drain(&queue, || panic!("no extra request")).unwrap(), 0);
            assert!(wake.finish_callback(generation));
        }
        assert_eq!(posted.load(Ordering::SeqCst), 0);
        assert_eq!(queue.outstanding(), 0);
    }
    #[test]
    fn failed_coalesced_wake_closes_admission_and_settles_all_waiters() {
        let queue = Queue::new(4).unwrap();
        let pending = queue
            .submit(Task::Snapshot, Instant::now() + Duration::from_secs(5))
            .unwrap();
        let wake = WakeCoalescer::new(Arc::clone(&queue), Arc::new(|| WakeAck::HardFailure(4)));
        assert!(!wake.request());
        assert!(matches!(pending.wait(), Outcome::NotExecuted(_)));
        assert!(queue.is_shutdown());
        assert!(!wake.request());
        assert!(wake.begin_callback().is_none());
    }
    #[test]
    fn consumed_nested_wake_is_rearmed_after_callback_budget_expires() {
        use crate::lifecycle::CallbackGate;
        let queue = Queue::new(4).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let pending = queue.submit(Task::Snapshot, deadline).unwrap();
        let gate = CallbackGate::default();
        let flight = gate.enter().unwrap();
        let mut nested = None;
        let count = drain(&queue, || {
            nested = Some(queue.submit(Task::Snapshot, deadline).unwrap());
            assert!(gate.enter().is_none()); // Its original event is consumed.
            thread::sleep(Duration::from_millis(20));
            Err("read finished".into())
        })
        .unwrap();
        assert_eq!(count, 1);
        let mut wakes = 0;
        assert!(rearm_if_queued(&queue, || {
            wakes += 1;
            true
        }));
        assert_eq!(wakes, 1);
        drop(flight); // SDK queues the new event; it never calls synchronously.
        let _second = gate.enter().unwrap();
        assert_eq!(drain(&queue, || Err("second read".into())).unwrap(), 1);
        assert!(!queue.has_queued()); // Both unobserved completions still exist.
        assert!(rearm_if_queued(&queue, || panic!(
            "no completion wake loop"
        )));
        assert!(matches!(pending.wait(), Outcome::Completed(_)));
        assert!(matches!(nested.unwrap().wait(), Outcome::Completed(_)));
        let _unstarted = queue.submit(Task::Snapshot, deadline).unwrap();
        queue.shutdown(queue.epoch()).unwrap();
        assert!(rearm_if_queued(&queue, || panic!("no wake after shutdown")));
    }
    #[test]
    fn callback_returns_to_fusion_despite_continuous_queue_replenishment() {
        let queue = Queue::new(32).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut pending = vec![queue.submit(Task::Snapshot, deadline).unwrap()];
        let count = drain(&queue, || {
            pending.push(queue.submit(Task::Snapshot, deadline).unwrap());
            Err("injected read error".into())
        })
        .unwrap();
        assert!(count > 0 && count <= 8);
        assert_eq!(pending.len(), count + 1);
        assert!(queue.take_next(queue.epoch()).unwrap().is_some());
    }
    fn exchange(address: SocketAddr, epoch: &str, path: &str, auth: &str) -> (u16, Value) {
        let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(socket,"GET {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {auth}\r\nX-Adom-Fusion-Epoch: {epoch}\r\n\r\n").unwrap();
        let mut output = String::new();
        socket.read_to_string(&mut output).unwrap();
        let (head, body) = output.split_once("\r\n\r\n").unwrap();
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            serde_json::from_str(body).unwrap(),
        )
    }
    fn post_mechanical(
        address: SocketAddr,
        epoch: &str,
        contract: &str,
        body: Value,
    ) -> (u16, Value) {
        let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let bytes = serde_json::to_vec(&body).unwrap();
        write!(socket,"POST /v1/mechanical HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {TOKEN}\r\nX-Adom-Fusion-Epoch: {epoch}\r\nX-Adom-Mechanical-Contract: {contract}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",bytes.len()).unwrap();
        socket.write_all(&bytes).unwrap();
        let mut output = String::new();
        socket.read_to_string(&mut output).unwrap();
        let (head, body) = output.split_once("\r\n\r\n").unwrap();
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            serde_json::from_str(body).unwrap(),
        )
    }
    #[test]
    #[cfg(not(feature = "native-wake-probe"))]
    fn purchased_contract_routes_only_read_discovery_and_never_journals_it() {
        let queue = Queue::new(4).unwrap();
        let epoch = queue.epoch().to_string();
        let directory = std::env::temp_dir().join(format!(
            "fusion-r15-contract-{}-{epoch}",
            std::process::id()
        ));
        let reader = OperationStatusReader::new(directory.clone(), epoch.clone());
        let wakes = Arc::new(AtomicU64::new(0));
        let count = wakes.clone();
        let mut server = Service::start_with_operations(
            TcpListener::bind("127.0.0.1:0").unwrap(),
            queue.clone(),
            TOKEN.into(),
            Arc::new(move || {
                count.fetch_add(1, Ordering::Relaxed);
                true
            }),
            Arc::new(Stats::default()),
            reader.clone(),
        )
        .unwrap();
        let address = server.address();
        let end = Instant::now() + Duration::from_secs(5);
        while reader.health()["connected"] != true {
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(1));
        }
        let request = json!({"op":"mcmaster_discover","sessionId":epoch,"operationId":"discovery-read","target":{"documentId":format!("d:{epoch}:0000000000000001")}});
        let old = post_mechanical(
            address,
            &epoch,
            "native-material-catalog-v1",
            request.clone(),
        );
        assert_eq!(old.0, 409);
        assert_eq!(
            old.1["errorCode"],
            "native_purchased_discovery_contract_required"
        );
        let mut missing = request.clone();
        missing.as_object_mut().unwrap().remove("sessionId");
        let absent = post_mechanical(address, &epoch, crate::purchased::CONTRACT, missing);
        assert_eq!(absent.0, 400);
        assert_eq!(
            absent.1["errorCode"],
            "native_mcmaster_current_session_required"
        );
        let mut opening = request.clone();
        opening["op"] = json!("mcmaster_open");
        opening["target"]["revision"] = json!("some-revision");
        assert_eq!(
            post_mechanical(address, &epoch, crate::purchased::CONTRACT, opening).0,
            400
        );
        assert_eq!(wakes.load(Ordering::Relaxed), 0);
        assert!(!queue.has_queued());
        let thread_epoch = epoch.clone();
        let client = thread::spawn(move || {
            post_mechanical(address, &thread_epoch, crate::purchased::CONTRACT, request)
        });
        let end = Instant::now() + Duration::from_secs(5);
        let claim = loop {
            assert!(Instant::now() < end);
            if let Some(claim) = queue.take_next(queue.epoch()).unwrap() {
                break claim;
            }
            thread::sleep(Duration::from_millis(1));
        };
        let Task::Inspect(request) = claim.task() else {
            panic!("discovery must be read-only inspect")
        };
        assert_eq!(request.operation(), "mcmaster_discover");
        assert!(!request.is_write());
        claim
            .complete(Ok(Reply::Value(
                json!({"success":true,"data":{"available":false}}),
            )))
            .unwrap();
        assert_eq!(client.join().unwrap().0, 200);
        assert_eq!(wakes.load(Ordering::Relaxed), 1);
        assert!(reader
            .journal()
            .unwrap()
            .status(&epoch, "discovery-read")
            .unwrap()
            .durable
            .unwrap()
            .record
            .is_none());
        let health = exchange(address, &epoch, "/v1/health", TOKEN).1;
        assert!(health["operations"]
            .as_array()
            .unwrap()
            .contains(&json!("mcmaster_discover")));
        assert!(server.stop());
        drop(server);
        drop(reader);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn actual_socket_auth_health_and_shutdown_without_sdk() {
        let queue = Queue::new(4).unwrap();
        let stats = Arc::new(Stats::default());
        let mut server = Service::start(
            TcpListener::bind("127.0.0.1:0").unwrap(),
            Arc::clone(&queue),
            TOKEN.into(),
            Arc::new(|| panic!("health must not wake SDK")),
            stats,
        )
        .unwrap();
        let address = server.address();
        let epoch = queue.epoch().to_string();
        assert_eq!(exchange(address, &epoch, "/v1/health", "bad").0, 401);
        let (status, body) = exchange(address, &epoch, "/v1/health", TOKEN);
        assert_eq!(status, 200);
        assert_eq!(body["mutationSupport"], false);
        assert_eq!(
            exchange(address, "old-epoch", "/v1/documents", TOKEN).0,
            409
        );
        assert!(server.stop());
        assert!(TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_err());
    }
    #[test]
    fn request_runs_only_when_creator_thread_claims_and_completes() {
        let queue = Queue::new(2).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut server = Service::start(
            TcpListener::bind("127.0.0.1:0").unwrap(),
            Arc::clone(&queue),
            TOKEN.into(),
            Arc::new(move || sender.send(()).is_ok()),
            Arc::new(Stats::default()),
        )
        .unwrap();
        let address = server.address();
        let epoch = queue.epoch().to_string();
        let client = thread::spawn(move || exchange(address, &epoch, "/v1/documents", TOKEN));
        receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        let claim = queue.take_next(queue.epoch()).unwrap().unwrap();
        claim
            .complete(Ok(Reply::Snapshot(Snapshot {
                implementation: "Rust test",
                scope: "test",
                fusion_version: "test".into(),
                thread_id: 42,
                creation_ids_unique: true,
                identity_scope: "read-only test inventory metadata",
                documents: vec![],
            })))
            .unwrap();
        let (status, body) = client.join().unwrap();
        assert_eq!(status, 200);
        assert_eq!(body["data"]["threadId"], 42);
        assert_eq!(queue.outstanding(), 0);
        assert!(server.stop());
    }
    #[test]
    fn lost_native_wake_cancels_before_execution() {
        let queue = Queue::new(2).unwrap();
        let mut server = Service::start(
            TcpListener::bind("127.0.0.1:0").unwrap(),
            Arc::clone(&queue),
            TOKEN.into(),
            Arc::new(|| false),
            Arc::new(Stats::default()),
        )
        .unwrap();
        let (status, body) = exchange(
            server.address(),
            &queue.epoch().to_string(),
            "/v1/documents",
            TOKEN,
        );
        assert_eq!(status, 503);
        assert_eq!(body["nativeExecutionStarted"], false);
        assert!(queue.take_next(queue.epoch()).unwrap().is_none());
        assert!(server.stop());
    }
    #[test]
    fn stop_interrupts_a_partial_header_and_joins_socket_workers() {
        let queue = Queue::new(2).unwrap();
        let stats = Arc::new(Stats::default());
        let mut server = Service::start(
            TcpListener::bind("127.0.0.1:0").unwrap(),
            queue,
            TOKEN.into(),
            Arc::new(|| true),
            Arc::clone(&stats),
        )
        .unwrap();
        let mut socket =
            TcpStream::connect_timeout(&server.address(), Duration::from_secs(2)).unwrap();
        socket.write_all(b"GET /v1/health HTTP/1.1\r\n").unwrap();
        let wait = Instant::now() + Duration::from_secs(1);
        while stats.requests.load(Ordering::Relaxed) == 0 && Instant::now() < wait {
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(stats.requests.load(Ordering::Relaxed), 1);
        let start = Instant::now();
        assert!(server.stop());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

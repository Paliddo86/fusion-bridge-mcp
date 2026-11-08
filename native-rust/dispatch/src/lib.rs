//! Bounded, in-process handoff from workers to Fusion's main thread.
//!
//! The native event is only a wake-up containing [`Epoch`]. Tasks and results
//! remain in Rust; neither event data nor this crate contains SDK handles.
//! This queue is not a durable journal or permission to mutate a document.
//! A timed-out running request is unknown, never retried or requeued.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, ThreadId};
use std::time::Instant;

/// A random runtime identity, independent of IDs from any previous DLL load.
/// Epochs prevent stale wakes; they are not authentication credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Epoch([u8; 16]);

impl Epoch {
    fn fresh() -> io::Result<Self> {
        let mut bytes = [0; 16];
        random_bytes(&mut bytes)?;
        Ok(Self(bytes))
    }
}

impl fmt::Display for Epoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidEpoch;

impl fmt::Display for InvalidEpoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an epoch must contain exactly 32 hexadecimal ASCII characters")
    }
}

impl std::error::Error for InvalidEpoch {}

impl FromStr for Epoch {
    type Err = InvalidEpoch;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let raw = input.as_bytes();
        if raw.len() != 32 || !raw.iter().all(u8::is_ascii_hexdigit) {
            return Err(InvalidEpoch);
        }
        let mut bytes = [0; 16];
        for (i, output) in bytes.iter_mut().enumerate() {
            // ASCII validation above also establishes valid UTF-8 boundaries.
            *output = u8::from_str_radix(&input[i * 2..i * 2 + 2], 16).map_err(|_| InvalidEpoch)?;
        }
        Ok(Self(bytes))
    }
}

#[cfg(unix)]
fn random_bytes(bytes: &mut [u8; 16]) -> io::Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(bytes)
}

#[cfg(windows)]
fn random_bytes(bytes: &mut [u8; 16]) -> io::Result<()> {
    #[link(name = "bcrypt")]
    extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut std::ffi::c_void,
            buffer: *mut u8,
            len: u32,
            flags: u32,
        ) -> i32;
    }
    // BCRYPT_USE_SYSTEM_PREFERRED_RNG; NTSTATUS success is nonnegative.
    let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), 16, 2) };
    if status < 0 {
        Err(io::Error::other(format!(
            "BCryptGenRandom failed: {status:#x}"
        )))
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn random_bytes(_bytes: &mut [u8; 16]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no supported OS random source",
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct RequestId {
    epoch: Epoch,
    sequence: u64,
}

impl RequestId {
    pub fn epoch(self) -> Epoch {
        self.epoch
    }
    pub fn sequence(self) -> u64 {
        self.sequence
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.epoch, self.sequence)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmitError {
    Full,
    ShuttingDown,
    DeadlineElapsed,
    IdsExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrainError {
    StaleEpoch,
    WrongThread,
    ShuttingDown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompleteError {
    ClaimMissing,
    ClaimNotExecuting,
}

macro_rules! debug_error {
    ($($name:ty),+ $(,)?) => {$ (
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{self:?}") }
        }
        impl std::error::Error for $name {}
    )+ };
}
debug_error!(SubmitError, DrainError, CompleteError);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotExecutedReason {
    DeadlineElapsed,
    Cancelled,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnknownReason {
    DeadlineWhileExecuting,
    CancelledWhileExecuting,
    ShutdownWhileExecuting,
    ExecutionAbandoned,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Outcome<R> {
    Completed(R),
    NotExecuted(NotExecutedReason),
    Unknown(UnknownReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Queued,
    Executing,
    Finished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShutdownSummary {
    pub cancelled_queued: usize,
    pub executing: usize,
}

enum Phase<R> {
    Queued,
    Executing,
    Finished(Outcome<R>),
}

struct Entry<T, R> {
    task: Option<T>,
    deadline: Instant,
    phase: Phase<R>,
    receiver_alive: bool,
}

struct Inner<T, R> {
    entries: BTreeMap<u64, Entry<T, R>>,
    queue: VecDeque<u64>,
    next_sequence: u64,
    stopped: bool,
}

pub struct Runtime<T, R> {
    epoch: Epoch,
    main_thread: ThreadId,
    capacity: usize,
    inner: Mutex<Inner<T, R>>,
    changed: Condvar,
}

impl<T, R> Runtime<T, R> {
    /// Call on the thread that will own all native SDK access. Admission and
    /// observation may happen on workers; draining is confined to this thread.
    pub fn new(capacity: usize) -> io::Result<Arc<Self>> {
        if capacity == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "queue capacity must be positive",
            ));
        }
        Ok(Arc::new(Self {
            epoch: Epoch::fresh()?,
            main_thread: thread::current().id(),
            capacity,
            inner: Mutex::new(Inner {
                entries: BTreeMap::new(),
                queue: VecDeque::new(),
                next_sequence: 1,
                stopped: false,
            }),
            changed: Condvar::new(),
        }))
    }

    // A poison event never makes work runnable again. There is no user callback
    // under this lock; recovery preserves the monotonic phase transitions.
    fn lock(&self) -> MutexGuard<'_, Inner<T, R>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// Includes unobserved completions and detached running work, so memory and
    /// outstanding execution remain bounded even if clients stop reading.
    pub fn outstanding(&self) -> usize {
        self.lock().entries.len()
    }

    pub fn is_shutdown(&self) -> bool {
        self.lock().stopped
    }

    /// Read-only wake decision. Completed/unobserved results and executing
    /// claims do not justify another callback. Never revives a stopped epoch.
    pub fn has_queued(&self) -> bool {
        let inner = self.lock();
        !inner.stopped
            && inner
                .entries
                .values()
                .any(|entry| matches!(entry.phase, Phase::Queued))
    }

    /// Earliest original deadline still awaiting a native claim. Expired queued
    /// entries remain visible until settled: a lost wake must not gain a later
    /// deadline merely because a newer request arrived.
    pub fn earliest_queued_deadline(&self) -> Option<Instant> {
        let inner = self.lock();
        if inner.stopped {
            return None;
        }
        inner
            .entries
            .values()
            .filter(|entry| matches!(entry.phase, Phase::Queued))
            .map(|entry| entry.deadline)
            .min()
    }

    pub fn status(&self, id: RequestId) -> Option<Status> {
        if id.epoch != self.epoch {
            return None;
        }
        self.lock()
            .entries
            .get(&id.sequence)
            .map(|entry| match entry.phase {
                Phase::Queued => Status::Queued,
                Phase::Executing => Status::Executing,
                Phase::Finished(_) => Status::Finished,
            })
    }

    pub fn submit(
        self: &Arc<Self>,
        task: T,
        deadline: Instant,
    ) -> Result<Pending<T, R>, SubmitError> {
        let mut inner = self.lock();
        if inner.stopped {
            return Err(SubmitError::ShuttingDown);
        }
        if deadline <= Instant::now() {
            return Err(SubmitError::DeadlineElapsed);
        }
        if inner.entries.len() >= self.capacity {
            return Err(SubmitError::Full);
        }
        // Refuse rather than wrap and accidentally alias any previous request.
        let sequence = inner.next_sequence;
        inner.next_sequence = sequence.checked_add(1).ok_or(SubmitError::IdsExhausted)?;
        inner.entries.insert(
            sequence,
            Entry {
                task: Some(task),
                deadline,
                phase: Phase::Queued,
                receiver_alive: true,
            },
        );
        inner.queue.push_back(sequence);
        Ok(Pending {
            runtime: Arc::clone(self),
            id: RequestId {
                epoch: self.epoch,
                sequence,
            },
            attached: true,
        })
    }

    /// Claim at most one task. Expired queued requests are settled as not
    /// executed. Extra or repeated native wake events cannot duplicate claims.
    pub fn take_next(self: &Arc<Self>, epoch: Epoch) -> Result<Option<Claim<T, R>>, DrainError> {
        if epoch != self.epoch {
            return Err(DrainError::StaleEpoch);
        }
        if thread::current().id() != self.main_thread {
            return Err(DrainError::WrongThread);
        }
        let mut inner = self.lock();
        if inner.stopped {
            return Err(DrainError::ShuttingDown);
        }
        let mut expired_tasks = Vec::new();
        while let Some(sequence) = inner.queue.pop_front() {
            let entry = inner
                .entries
                .get_mut(&sequence)
                .expect("queued entry exists");
            if entry.deadline <= Instant::now() {
                entry.phase =
                    Phase::Finished(Outcome::NotExecuted(NotExecutedReason::DeadlineElapsed));
                expired_tasks.push(entry.task.take());
                self.changed.notify_all();
                continue;
            }
            entry.phase = Phase::Executing;
            let task = entry.task.take().expect("queued entry owns task");
            let claim = Claim {
                runtime: Arc::clone(self),
                id: RequestId {
                    epoch: self.epoch,
                    sequence,
                },
                task,
                completed: false,
                main_thread_only: PhantomData,
            };
            drop(inner);
            drop(expired_tasks);
            return Ok(Some(claim));
        }
        drop(inner);
        drop(expired_tasks);
        Ok(None)
    }

    /// Stops admission and unstarted work, wakes observers immediately, and
    /// never revokes a running main-thread claim. Safe from any Rust thread.
    pub fn shutdown(&self, epoch: Epoch) -> Result<ShutdownSummary, DrainError> {
        if epoch != self.epoch {
            return Err(DrainError::StaleEpoch);
        }
        let mut inner = self.lock();
        inner.stopped = true;
        let queued: Vec<_> = inner.queue.drain(..).collect();
        let cancelled_queued = queued.len();
        let mut cancelled_tasks = Vec::new();
        for sequence in queued {
            let entry = inner
                .entries
                .get_mut(&sequence)
                .expect("queued entry exists");
            entry.phase = Phase::Finished(Outcome::NotExecuted(NotExecutedReason::Shutdown));
            cancelled_tasks.push(entry.task.take());
        }
        let executing = inner
            .entries
            .values()
            .filter(|e| matches!(e.phase, Phase::Executing))
            .count();
        self.changed.notify_all();
        drop(inner);
        drop(cancelled_tasks);
        Ok(ShutdownSummary {
            cancelled_queued,
            executing,
        })
    }

    fn finish(&self, id: RequestId, outcome: Outcome<R>) -> Result<(), CompleteError> {
        let mut inner = self.lock();
        let entry = inner
            .entries
            .get_mut(&id.sequence)
            .ok_or(CompleteError::ClaimMissing)?;
        if !matches!(entry.phase, Phase::Executing) {
            return Err(CompleteError::ClaimNotExecuting);
        }
        if entry.receiver_alive {
            entry.phase = Phase::Finished(outcome);
        } else {
            inner.entries.remove(&id.sequence);
        }
        self.changed.notify_all();
        Ok(())
    }
}

/// Unique observation handle. Dropping it cancels queued work but only detaches
/// from running work. It never duplicates, revokes, or retries native execution.
pub struct Pending<T, R> {
    runtime: Arc<Runtime<T, R>>,
    id: RequestId,
    attached: bool,
}

impl<T, R> Pending<T, R> {
    pub fn id(&self) -> RequestId {
        self.id
    }

    pub fn wait(self) -> Outcome<R> {
        self.wait_inner(false)
    }

    /// Owned execution coordinators must retain observation of claimed effects
    /// after the HTTP observer deadline or queue shutdown. Queued work still
    /// expires normally. A claimed task settles only on complete or Claim drop.
    /// The owner must arrange callback settlement before joining this waiter.
    pub fn wait_settled(self) -> Outcome<R> {
        self.wait_inner(true)
    }

    fn wait_inner(mut self, retain_executing: bool) -> Outcome<R> {
        let mut inner = self.runtime.lock();
        loop {
            let entry = inner
                .entries
                .get(&self.id.sequence)
                .expect("attached receiver has entry");
            if matches!(entry.phase, Phase::Finished(_)) {
                let entry = inner
                    .entries
                    .remove(&self.id.sequence)
                    .expect("finished entry exists");
                self.attached = false;
                if let Phase::Finished(outcome) = entry.phase {
                    return outcome;
                }
                unreachable!();
            }
            if retain_executing && matches!(entry.phase, Phase::Executing) {
                inner = self
                    .runtime
                    .changed
                    .wait(inner)
                    .unwrap_or_else(|e| e.into_inner());
                continue;
            }
            if inner.stopped {
                self.attached = false;
                let (outcome, task) = detach(
                    &mut inner,
                    self.id.sequence,
                    NotExecutedReason::Shutdown,
                    UnknownReason::ShutdownWhileExecuting,
                );
                drop(inner);
                drop(task);
                return outcome;
            }
            let now = Instant::now();
            if now >= entry.deadline {
                self.attached = false;
                let (outcome, task) = detach(
                    &mut inner,
                    self.id.sequence,
                    NotExecutedReason::DeadlineElapsed,
                    UnknownReason::DeadlineWhileExecuting,
                );
                drop(inner);
                drop(task);
                return outcome;
            }
            let duration = entry.deadline.saturating_duration_since(now);
            inner = self
                .runtime
                .changed
                .wait_timeout(inner, duration)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    pub fn cancel(mut self) -> Outcome<R> {
        let mut inner = self.runtime.lock();
        self.attached = false;
        let (outcome, task) = detach(
            &mut inner,
            self.id.sequence,
            NotExecutedReason::Cancelled,
            UnknownReason::CancelledWhileExecuting,
        );
        drop(inner);
        drop(task);
        outcome
    }
}

fn detach<T, R>(
    inner: &mut Inner<T, R>,
    sequence: u64,
    queued_reason: NotExecutedReason,
    executing_reason: UnknownReason,
) -> (Outcome<R>, Option<T>) {
    let entry = inner
        .entries
        .get_mut(&sequence)
        .expect("attached receiver has entry");
    match entry.phase {
        Phase::Queued => {
            inner.queue.retain(|id| *id != sequence);
            let entry = inner
                .entries
                .remove(&sequence)
                .expect("queued entry exists");
            (Outcome::NotExecuted(queued_reason), entry.task)
        }
        Phase::Executing => {
            entry.receiver_alive = false;
            (Outcome::Unknown(executing_reason), None)
        }
        Phase::Finished(_) => {
            let entry = inner
                .entries
                .remove(&sequence)
                .expect("finished entry exists");
            if let Phase::Finished(outcome) = entry.phase {
                (outcome, None)
            } else {
                unreachable!()
            }
        }
    }
}

impl<T, R> Drop for Pending<T, R> {
    fn drop(&mut self) {
        if self.attached {
            let mut inner = self.runtime.lock();
            let detached = detach(
                &mut inner,
                self.id.sequence,
                NotExecutedReason::Cancelled,
                UnknownReason::CancelledWhileExecuting,
            );
            drop(inner);
            drop(detached);
        }
    }
}

/// A unique main-thread execution claim. This guard is deliberately `!Send` and
/// `!Sync`, and contains no SDK handle. Dropping it without completing settles
/// the observation as unknown; the task is never made runnable again.
///
/// Completing twice is rejected by ownership:
/// ```compile_fail
/// use fusion_native_dispatch::Runtime;
/// use std::time::{Instant, Duration};
/// let queue = Runtime::<(), ()>::new(1).unwrap();
/// let pending = queue.submit((), Instant::now() + Duration::from_secs(1)).unwrap();
/// let claim = queue.take_next(queue.epoch()).unwrap().unwrap();
/// claim.complete(()).unwrap();
/// claim.complete(()).unwrap();
/// ```
/// A claim cannot cross to a worker thread:
/// ```compile_fail
/// use fusion_native_dispatch::Runtime;
/// use std::time::{Instant, Duration};
/// let queue = Runtime::<(), ()>::new(1).unwrap();
/// let pending = queue.submit((), Instant::now() + Duration::from_secs(1)).unwrap();
/// let claim = queue.take_next(queue.epoch()).unwrap().unwrap();
/// std::thread::spawn(move || claim.complete(()));
/// ```
pub struct Claim<T, R> {
    runtime: Arc<Runtime<T, R>>,
    id: RequestId,
    task: T,
    completed: bool,
    main_thread_only: PhantomData<Rc<()>>,
}

impl<T, R> Claim<T, R> {
    pub fn id(&self) -> RequestId {
        self.id
    }
    pub fn task(&self) -> &T {
        &self.task
    }

    /// Move non-cloneable task data out through an Option without transferring
    /// this main-thread-only claim or any permission to complete it twice.
    pub fn task_mut(&mut self) -> &mut T {
        &mut self.task
    }

    pub fn complete(mut self, result: R) -> Result<(), CompleteError> {
        let result = self.runtime.finish(self.id, Outcome::Completed(result));
        self.completed = true;
        result
    }
}

impl<T, R> Drop for Claim<T, R> {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self
                .runtime
                .finish(self.id, Outcome::Unknown(UnknownReason::ExecutionAbandoned));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    #[test]
    fn owned_waiter_keeps_claimed_result_after_deadline_and_shutdown() {
        let runtime = Runtime::<(), u32>::new(1).unwrap();
        let pending = runtime
            .submit((), Instant::now() + Duration::from_millis(20))
            .unwrap();
        let claim = runtime.take_next(runtime.epoch()).unwrap().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || sender.send(pending.wait_settled()).unwrap());
        std::thread::sleep(Duration::from_millis(30));
        runtime.shutdown(runtime.epoch()).unwrap();
        assert!(receiver.recv_timeout(Duration::from_millis(20)).is_err());
        claim.complete(17).unwrap();
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            Outcome::Completed(17)
        );
        waiter.join().unwrap();
        assert_eq!(runtime.outstanding(), 0);
    }
    #[test]
    fn owned_waiter_still_expires_unclaimed_work_and_observes_abandonment() {
        let runtime = Runtime::<(), u32>::new(1).unwrap();
        let pending = runtime
            .submit((), Instant::now() + Duration::from_millis(5))
            .unwrap();
        assert_eq!(
            pending.wait_settled(),
            Outcome::NotExecuted(NotExecutedReason::DeadlineElapsed)
        );
        let pending = runtime
            .submit((), Instant::now() + Duration::from_secs(1))
            .unwrap();
        let claim = runtime.take_next(runtime.epoch()).unwrap().unwrap();
        drop(claim);
        assert_eq!(
            pending.wait_settled(),
            Outcome::Unknown(UnknownReason::ExecutionAbandoned)
        );
    }
    use std::sync::{mpsc, Barrier, Weak};
    use std::time::Duration;

    fn future() -> Instant {
        Instant::now() + Duration::from_secs(30)
    }

    fn expire<T, R>(runtime: &Runtime<T, R>, id: RequestId) {
        runtime
            .lock()
            .entries
            .get_mut(&id.sequence)
            .unwrap()
            .deadline = Instant::now();
        runtime.changed.notify_all();
    }

    #[test]
    fn epochs_are_fresh_and_only_accept_exact_hex_wakes() {
        let a = Runtime::<(), ()>::new(1).unwrap();
        let b = Runtime::<(), ()>::new(1).unwrap();
        assert_ne!(a.epoch(), b.epoch());
        assert_eq!(a.epoch().to_string().parse::<Epoch>().unwrap(), a.epoch());
        for bad in [
            "",
            "a",
            "gggggggggggggggggggggggggggggggg",
            "00000000000000000000000000000000extra",
            "éééééééééééééééé",
        ] {
            assert_eq!(bad.parse::<Epoch>(), Err(InvalidEpoch));
        }
        assert!(matches!(
            a.take_next(b.epoch()),
            Err(DrainError::StaleEpoch)
        ));
        assert_eq!(a.shutdown(b.epoch()), Err(DrainError::StaleEpoch));
        assert!(!a.is_shutdown());
    }

    #[test]
    fn empty_and_repeated_wakes_never_duplicate_claims() {
        let runtime = Runtime::new(2).unwrap();
        assert!(runtime.take_next(runtime.epoch()).unwrap().is_none());
        let pending = runtime.submit("snapshot", future()).unwrap();
        let id = pending.id();
        let claim = runtime.take_next(runtime.epoch()).unwrap().unwrap();
        assert_eq!(claim.id(), id);
        assert_eq!(*claim.task(), "snapshot");
        assert_eq!(runtime.status(id), Some(Status::Executing));
        for _ in 0..10 {
            assert!(runtime.take_next(runtime.epoch()).unwrap().is_none());
        }
        claim.complete("result").unwrap();
        assert_eq!(runtime.status(id), Some(Status::Finished));
        assert_eq!(pending.wait(), Outcome::Completed("result"));
        assert_eq!(runtime.status(id), None);
        assert_eq!(runtime.outstanding(), 0);
    }

    #[test]
    fn worker_thread_cannot_take_sdk_claim() {
        let runtime = Runtime::<(), ()>::new(1).unwrap();
        let pending = runtime.submit((), future()).unwrap();
        let worker = Arc::clone(&runtime);
        let error = thread::spawn(move || worker.take_next(worker.epoch()).err())
            .join()
            .unwrap();
        assert_eq!(error, Some(DrainError::WrongThread));
        assert_eq!(runtime.status(pending.id()), Some(Status::Queued));
    }

    #[test]
    fn capacity_includes_unread_completion_and_no_wrapped_request_ids() {
        assert!(Runtime::<(), ()>::new(0).is_err());
        let runtime = Runtime::new(1).unwrap();
        let pending = runtime.submit(1, future()).unwrap();
        let first_id = pending.id();
        assert!(matches!(
            runtime.submit(2, future()),
            Err(SubmitError::Full)
        ));
        runtime
            .take_next(runtime.epoch())
            .unwrap()
            .unwrap()
            .complete(7)
            .unwrap();
        assert!(matches!(
            runtime.submit(3, future()),
            Err(SubmitError::Full)
        ));
        assert_eq!(pending.wait(), Outcome::Completed(7));
        let next = runtime.submit(4, future()).unwrap();
        assert_eq!(next.id().sequence(), first_id.sequence() + 1);
        drop(next);
        runtime.lock().next_sequence = u64::MAX;
        for _ in 0..2 {
            assert!(matches!(
                runtime.submit(5, future()),
                Err(SubmitError::IdsExhausted)
            ));
        }
        assert_eq!(runtime.outstanding(), 0);
    }

    #[test]
    fn deadline_before_admission_has_no_id_or_queued_effect() {
        let runtime = Runtime::<(), ()>::new(1).unwrap();
        assert!(matches!(
            runtime.submit((), Instant::now()),
            Err(SubmitError::DeadlineElapsed)
        ));
        assert_eq!(runtime.outstanding(), 0);
        assert_eq!(runtime.submit((), future()).unwrap().id().sequence(), 1);
    }

    #[test]
    fn expired_queued_task_cannot_execute_from_a_late_wake() {
        let runtime = Runtime::<(), ()>::new(2).unwrap();
        let expired = runtime.submit((), future()).unwrap();
        expire(&runtime, expired.id());
        let next = runtime.submit((), future()).unwrap();
        let claim = runtime.take_next(runtime.epoch()).unwrap().unwrap();
        assert_eq!(claim.id(), next.id());
        assert_eq!(
            expired.wait(),
            Outcome::NotExecuted(NotExecutedReason::DeadlineElapsed)
        );
        claim.complete(()).unwrap();
        assert_eq!(next.wait(), Outcome::Completed(()));
    }

    #[test]
    fn queued_wait_timeout_and_cancellation_release_queue_and_capacity() {
        let runtime = Runtime::<(), ()>::new(1).unwrap();
        let pending = runtime.submit((), future()).unwrap();
        expire(&runtime, pending.id());
        assert_eq!(
            pending.wait(),
            Outcome::NotExecuted(NotExecutedReason::DeadlineElapsed)
        );
        assert!(runtime.take_next(runtime.epoch()).unwrap().is_none());
        let pending = runtime.submit((), future()).unwrap();
        assert_eq!(
            pending.cancel(),
            Outcome::NotExecuted(NotExecutedReason::Cancelled)
        );
        assert!(runtime.take_next(runtime.epoch()).unwrap().is_none());
        assert_eq!(runtime.outstanding(), 0);
        assert_eq!(runtime.lock().queue.len(), 0);
    }

    #[test]
    fn executing_timeout_is_unknown_and_retains_capacity_until_completion() {
        let runtime = Runtime::new(1).unwrap();
        let pending = runtime.submit((), future()).unwrap();
        let id = pending.id();
        let claim = runtime.take_next(runtime.epoch()).unwrap().unwrap();
        expire(&runtime, id);
        assert_eq!(
            pending.wait(),
            Outcome::Unknown(UnknownReason::DeadlineWhileExecuting)
        );
        assert_eq!(runtime.status(id), Some(Status::Executing));
        assert!(matches!(
            runtime.submit((), future()),
            Err(SubmitError::Full)
        ));
        assert!(runtime.take_next(runtime.epoch()).unwrap().is_none());
        claim.complete("late known result").unwrap();
        assert_eq!(runtime.status(id), None);
        assert_eq!(runtime.outstanding(), 0);
        assert!(runtime.submit((), future()).is_ok());
    }

    #[test]
    fn published_completion_wins_over_late_observer_deadline_or_cancel() {
        let runtime = Runtime::new(1).unwrap();
        let pending = runtime.submit((), future()).unwrap();
        let id = pending.id();
        runtime
            .take_next(runtime.epoch())
            .unwrap()
            .unwrap()
            .complete(42)
            .unwrap();
        expire(&runtime, id);
        assert_eq!(pending.wait(), Outcome::Completed(42));
        let pending = runtime.submit((), future()).unwrap();
        runtime
            .take_next(runtime.epoch())
            .unwrap()
            .unwrap()
            .complete(43)
            .unwrap();
        assert_eq!(pending.cancel(), Outcome::Completed(43));
    }

    #[test]
    fn abandoned_claim_is_unknown_and_never_requeued() {
        let runtime = Runtime::<(), ()>::new(1).unwrap();
        let pending = runtime.submit((), future()).unwrap();
        drop(runtime.take_next(runtime.epoch()).unwrap().unwrap());
        assert!(runtime.take_next(runtime.epoch()).unwrap().is_none());
        assert_eq!(
            pending.wait(),
            Outcome::Unknown(UnknownReason::ExecutionAbandoned)
        );
        assert_eq!(runtime.outstanding(), 0);
    }

    #[test]
    fn dropped_receiver_cancels_queued_and_detaches_executing_work() {
        let runtime = Runtime::<(), ()>::new(1).unwrap();
        for _ in 0..100 {
            drop(runtime.submit((), future()).unwrap());
        }
        assert_eq!(runtime.outstanding(), 0);
        assert!(runtime.lock().queue.is_empty());
        let pending = runtime.submit((), future()).unwrap();
        let claim = runtime.take_next(runtime.epoch()).unwrap().unwrap();
        drop(pending);
        assert_eq!(runtime.outstanding(), 1);
        drop(claim);
        assert_eq!(runtime.outstanding(), 0);
    }

    #[test]
    fn shutdown_wakes_waiters_immediately_without_stealing_running_claim() {
        let runtime = Runtime::<(), ()>::new(2).unwrap();
        let running = runtime.submit((), future()).unwrap();
        let claim = runtime.take_next(runtime.epoch()).unwrap().unwrap();
        let queued = runtime.submit((), future()).unwrap();
        let (tx, rx) = mpsc::channel();
        let tx2 = tx.clone();
        let running_waiter = thread::spawn(move || tx.send(running.wait()).unwrap());
        let queued_waiter = thread::spawn(move || tx2.send(queued.wait()).unwrap());
        assert_eq!(
            runtime.shutdown(runtime.epoch()).unwrap(),
            ShutdownSummary {
                cancelled_queued: 1,
                executing: 1
            }
        );
        let outcomes = [
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        ];
        assert!(outcomes.contains(&Outcome::NotExecuted(NotExecutedReason::Shutdown)));
        assert!(outcomes.contains(&Outcome::Unknown(UnknownReason::ShutdownWhileExecuting)));
        assert_eq!(runtime.outstanding(), 1);
        assert!(matches!(
            runtime.take_next(runtime.epoch()),
            Err(DrainError::ShuttingDown)
        ));
        assert!(matches!(
            runtime.submit((), future()),
            Err(SubmitError::ShuttingDown)
        ));
        claim.complete(()).unwrap();
        assert_eq!(runtime.outstanding(), 0);
        assert_eq!(
            runtime.shutdown(runtime.epoch()).unwrap(),
            ShutdownSummary {
                cancelled_queued: 0,
                executing: 0
            }
        );
        running_waiter.join().unwrap();
        queued_waiter.join().unwrap();
    }

    #[test]
    fn condvar_deadline_and_completion_wake_real_waiters() {
        let runtime = Runtime::<(), u32>::new(1).unwrap();
        let pending = runtime
            .submit((), Instant::now() + Duration::from_secs(1))
            .unwrap();
        let (tx, rx) = mpsc::channel();
        let waiter = thread::spawn(move || tx.send(pending.wait()).unwrap());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Outcome::NotExecuted(NotExecutedReason::DeadlineElapsed)
        );
        waiter.join().unwrap();
        assert!(runtime.take_next(runtime.epoch()).unwrap().is_none());
        let pending = runtime.submit((), future()).unwrap();
        let claim = runtime.take_next(runtime.epoch()).unwrap().unwrap();
        let (tx, rx) = mpsc::channel();
        let waiter = thread::spawn(move || tx.send(pending.wait()).unwrap());
        claim.complete(123).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Outcome::Completed(123)
        );
        waiter.join().unwrap();
        assert_eq!(runtime.outstanding(), 0);
    }

    #[test]
    fn shutdown_admission_race_either_refuses_or_settles_not_executed() {
        let runtime = Runtime::<(), ()>::new(16).unwrap();
        let barrier = Arc::new(Barrier::new(17));
        let mut workers = Vec::new();
        for _ in 0..16 {
            let barrier = Arc::clone(&barrier);
            let runtime = Arc::clone(&runtime);
            workers.push(thread::spawn(move || {
                barrier.wait();
                match runtime.submit((), future()) {
                    Ok(pending) => Some(pending.wait()),
                    Err(error) => {
                        assert_eq!(error, SubmitError::ShuttingDown);
                        None
                    }
                }
            }));
        }
        barrier.wait();
        runtime.shutdown(runtime.epoch()).unwrap();
        for worker in workers {
            if let Some(outcome) = worker.join().unwrap() {
                assert_eq!(outcome, Outcome::NotExecuted(NotExecutedReason::Shutdown));
            }
        }
        assert_eq!(runtime.outstanding(), 0);
        assert!(matches!(
            runtime.submit((), future()),
            Err(SubmitError::ShuttingDown)
        ));
    }

    #[test]
    fn cancellation_race_has_exactly_one_owner_and_never_retries() {
        let runtime = Runtime::<(), ()>::new(1).unwrap();
        for _ in 0..128 {
            let pending = runtime.submit((), future()).unwrap();
            let barrier = Arc::new(Barrier::new(2));
            let worker_barrier = Arc::clone(&barrier);
            let worker = thread::spawn(move || {
                worker_barrier.wait();
                pending.cancel()
            });
            barrier.wait();
            let claim = runtime.take_next(runtime.epoch()).unwrap();
            let outcome = worker.join().unwrap();
            match claim {
                None => assert_eq!(outcome, Outcome::NotExecuted(NotExecutedReason::Cancelled)),
                Some(claim) => {
                    assert_eq!(
                        outcome,
                        Outcome::Unknown(UnknownReason::CancelledWhileExecuting)
                    );
                    assert_eq!(runtime.outstanding(), 1);
                    claim.complete(()).unwrap();
                }
            }
            assert!(runtime.take_next(runtime.epoch()).unwrap().is_none());
            assert_eq!(runtime.outstanding(), 0);
        }
    }

    #[test]
    fn concurrent_admission_produces_unique_ids_and_fifo_claims() {
        let runtime = Runtime::<usize, usize>::new(128).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut workers = Vec::new();
        for worker_id in 0..16 {
            let runtime = Arc::clone(&runtime);
            let tx = tx.clone();
            workers.push(thread::spawn(move || {
                for item in 0..8 {
                    tx.send(runtime.submit(worker_id * 8 + item, future()).unwrap())
                        .unwrap();
                }
            }));
        }
        drop(tx);
        let pending: Vec<_> = rx.into_iter().collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(pending.len(), 128);
        let ids: BTreeSet<_> = pending.iter().map(|p| p.id().sequence()).collect();
        assert_eq!(ids, (1..=128).collect());
        let mut tasks = BTreeSet::new();
        for sequence in 1..=128 {
            let claim = runtime.take_next(runtime.epoch()).unwrap().unwrap();
            assert_eq!(claim.id().sequence(), sequence);
            let task = *claim.task();
            assert!(tasks.insert(task));
            claim.complete(task).unwrap();
        }
        for pending in pending {
            assert!(matches!(pending.wait(), Outcome::Completed(_)));
        }
        assert_eq!(tasks, (0..128).collect());
        assert_eq!(runtime.outstanding(), 0);
    }

    #[test]
    fn task_destructors_run_outside_internal_mutex() {
        struct ReentrantTask {
            runtime: Weak<Runtime<ReentrantTask, ()>>,
        }
        impl Drop for ReentrantTask {
            fn drop(&mut self) {
                if let Some(runtime) = self.runtime.upgrade() {
                    assert!(
                        runtime.inner.try_lock().is_ok(),
                        "task destructor held queue lock"
                    );
                }
            }
        }
        let runtime = Runtime::new(2).unwrap();
        let make = || ReentrantTask {
            runtime: Arc::downgrade(&runtime),
        };
        drop(runtime.submit(make(), future()).unwrap());
        let pending = runtime.submit(make(), future()).unwrap();
        pending.cancel();
        let pending = runtime.submit(make(), future()).unwrap();
        expire(&runtime, pending.id());
        pending.wait();
        let pending = runtime.submit(make(), future()).unwrap();
        expire(&runtime, pending.id());
        assert!(runtime.take_next(runtime.epoch()).unwrap().is_none());
        pending.wait();
        let pending = runtime.submit(make(), future()).unwrap();
        runtime.shutdown(runtime.epoch()).unwrap();
        pending.wait();
    }

    #[test]
    fn unread_and_detached_result_destructors_run_outside_mutex() {
        struct ReentrantResult {
            runtime: Weak<Runtime<(), ReentrantResult>>,
        }
        impl Drop for ReentrantResult {
            fn drop(&mut self) {
                if let Some(runtime) = self.runtime.upgrade() {
                    assert!(
                        runtime.inner.try_lock().is_ok(),
                        "result destructor held queue lock"
                    );
                }
            }
        }
        let runtime = Runtime::new(1).unwrap();
        let make = || ReentrantResult {
            runtime: Arc::downgrade(&runtime),
        };
        let pending = runtime.submit((), future()).unwrap();
        runtime
            .take_next(runtime.epoch())
            .unwrap()
            .unwrap()
            .complete(make())
            .unwrap();
        drop(pending);
        let pending = runtime.submit((), future()).unwrap();
        let claim = runtime.take_next(runtime.epoch()).unwrap().unwrap();
        drop(pending);
        claim.complete(make()).unwrap();
        assert_eq!(runtime.outstanding(), 0);
    }
}

use super::*;
use crate::execution::Task;
use fusion_native_dispatch::{NotExecutedReason, UnknownReason};

fn coordinator(
    queue: &Arc<Queue>,
    ack: WakeAck,
) -> (Arc<WakeCoalescer>, Arc<AtomicU64>, Arc<Stats>) {
    let calls = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&calls);
    let stats = Arc::new(Stats::default());
    let wake = WakeCoalescer::with_stats(
        Arc::clone(queue),
        Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            ack
        }),
        Arc::clone(&stats),
    );
    (wake, calls, stats)
}

#[test]
fn unconfirmed_delivery_coalesces_then_allows_distinct_observed_generations() {
    let queue = Queue::new(8).unwrap();
    let (wake, calls, _) = coordinator(&queue, WakeAck::Unconfirmed);
    for _ in 0..3 {
        let a = queue
            .submit(Task::Snapshot, Instant::now() + Duration::from_secs(2))
            .unwrap();
        let b = queue
            .submit(Task::Snapshot, Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert!(wake.request());
        assert!(wake.request());
        let generation = wake.begin_callback().unwrap();
        assert_eq!(drain(&queue, || Err("read".into())).unwrap(), 2);
        assert!(wake.finish_callback(generation));
        assert!(matches!(a.wait(), Outcome::Completed(_)));
        assert!(matches!(b.wait(), Outcome::Completed(_)));
        wake.settled();
        assert!(!queue.is_shutdown());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[test]
fn no_observed_delivery_closes_epoch_at_earliest_coalesced_deadline_without_resend() {
    for ack in [WakeAck::Acknowledged, WakeAck::Unconfirmed] {
        let queue = Queue::new(4).unwrap();
        let (wake, calls, stats) = coordinator(&queue, ack);
        let long = queue
            .submit(Task::Snapshot, Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert!(wake.request());
        let short = queue
            .submit(Task::Snapshot, Instant::now() + Duration::from_millis(20))
            .unwrap();
        assert!(wake.request());
        assert!(matches!(
            short.wait(),
            Outcome::NotExecuted(NotExecutedReason::DeadlineElapsed)
        ));
        wake.settled();
        assert!(queue.is_shutdown());
        assert!(matches!(
            long.wait(),
            Outcome::NotExecuted(NotExecutedReason::Shutdown)
        ));
        assert!(wake.begin_callback().is_none());
        assert!(!wake.request());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            stats.json()["shutdownReason"],
            "native_wake_delivery_timeout"
        );
    }
}

#[test]
fn late_callback_cannot_rescue_expired_generation_before_waiter_settlement_hook() {
    let queue = Queue::new(2).unwrap();
    let (wake, calls, _) = coordinator(&queue, WakeAck::Unconfirmed);
    let pending = queue
        .submit(Task::Snapshot, Instant::now() + Duration::from_millis(20))
        .unwrap();
    assert!(wake.request());
    assert!(matches!(pending.wait(), Outcome::NotExecuted(_)));
    // Deliberately omit settled(): callback entry must enforce the same bound.
    assert!(wake.begin_callback().is_none());
    assert!(queue.is_shutdown());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn late_old_fire_return_and_finish_cannot_fault_or_clear_pending_successor() {
    for old_ack in [WakeAck::Unconfirmed, WakeAck::HardFailure(4)] {
        let queue = Queue::new(4).unwrap();
        let calls = Arc::new(AtomicU64::new(0));
        let counted = Arc::clone(&calls);
        let (entered, entered_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let wake = WakeCoalescer::new(
            Arc::clone(&queue),
            Arc::new(move || {
                if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                    entered.send(()).unwrap();
                    release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(3))
                        .unwrap();
                    old_ack
                } else {
                    WakeAck::Unconfirmed
                }
            }),
        );
        let first_deadline = Instant::now() + Duration::from_millis(200);
        let first = queue.submit(Task::Snapshot, first_deadline).unwrap();
        let worker_wake = Arc::clone(&wake);
        let old_fire = thread::spawn(move || worker_wake.request());
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let first_generation = wake.begin_callback().unwrap();
        assert_eq!(drain(&queue, || Err("first observed".into())).unwrap(), 1);
        assert!(wake.finish_callback(first_generation));
        let second = queue
            .submit(Task::Snapshot, Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert!(wake.request());
        assert!(!wake.finish_callback(first_generation));
        // The old caller resumes after its own deadline, while the successor's
        // original deadline remains in the future. Neither event targets it.
        thread::sleep(
            first_deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(1),
        );
        release.send(()).unwrap();
        assert!(old_fire.join().unwrap());
        assert!(matches!(first.wait(), Outcome::Completed(_)));
        wake.settled();
        assert!(!queue.is_shutdown());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let second_generation = wake.begin_callback().unwrap();
        assert_ne!(first_generation, second_generation);
        assert_eq!(drain(&queue, || Err("second observed".into())).unwrap(), 1);
        assert!(wake.finish_callback(second_generation));
        assert!(matches!(second.wait(), Outcome::Completed(_)));
    }
}

#[test]
fn nested_callback_does_not_retire_generation_and_bounded_drain_rearms_once() {
    let queue = Queue::new(16).unwrap();
    let (wake, calls, _) = coordinator(&queue, WakeAck::Unconfirmed);
    let pending: Vec<_> = (0..10)
        .map(|_| {
            queue
                .submit(Task::Snapshot, Instant::now() + Duration::from_secs(2))
                .unwrap()
        })
        .collect();
    assert!(wake.request());
    let first = wake.begin_callback().unwrap();
    assert!(wake.begin_callback().is_none());
    assert!(wake.request());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(drain(&queue, || Err("bounded read".into())).unwrap(), 8);
    assert!(wake.finish_callback(first));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let second = wake.begin_callback().unwrap();
    assert!(!wake.finish_callback(first));
    assert_eq!(drain(&queue, || Err("remaining read".into())).unwrap(), 2);
    assert!(wake.finish_callback(second));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    for item in pending {
        assert!(matches!(item.wait(), Outcome::Completed(_)));
    }
}

#[test]
fn stop_preserves_executing_uncertainty_and_never_rearms() {
    let queue = Queue::new(2).unwrap();
    let (wake, calls, _) = coordinator(&queue, WakeAck::Unconfirmed);
    let pending = queue
        .submit(Task::Snapshot, Instant::now() + Duration::from_secs(2))
        .unwrap();
    assert!(wake.request());
    let generation = wake.begin_callback().unwrap();
    let claim = queue.take_next(queue.epoch()).unwrap().unwrap();
    queue.shutdown(queue.epoch()).unwrap();
    assert!(matches!(
        pending.wait(),
        Outcome::Unknown(UnknownReason::ShutdownWhileExecuting)
    ));
    claim
        .complete(Err("observed after receiver stopped".into()))
        .unwrap();
    assert!(!wake.finish_callback(generation));
    assert!(!wake.request());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn current_hard_adapter_failures_close_admission_without_retry() {
    for status in [1, 4, 8] {
        let queue = Queue::new(2).unwrap();
        let (wake, calls, stats) = coordinator(&queue, WakeAck::from_status(status));
        let pending = queue
            .submit(Task::Snapshot, Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert!(!wake.request());
        assert!(matches!(pending.wait(), Outcome::NotExecuted(_)));
        assert!(wake.begin_callback().is_none());
        assert_eq!(stats.json()["shutdownReason"], "native_wake_failed");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

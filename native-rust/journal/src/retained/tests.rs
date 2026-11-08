use super::*;
use crate::{Fault, Limits, Phase};
use fusion_mechanical_core::prepare_request;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Barrier};
use std::thread;
use std::time::Duration;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "fusion-retained-test-{}-{}-{}",
            std::process::id(),
            crate::now_ms().unwrap(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn open(&self, session: &str) -> Journal {
        Journal::open_at(&self.0, session, "retention-test.1", Limits::default(), 100).unwrap()
    }
    fn retained(&self, limits: RetentionLimits) -> RetainedJournal {
        RetainedJournal::new(self.open("session-a"), limits).unwrap()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn request(session: &str, operation: &str, sample: usize) -> PreparedRequest {
    prepare_request(json!({"op":"exploded_preview_sample", "sessionId":session,"operationId":operation,"target":{"documentId":"document","revision":"revision"},"previewId":"preview","expectedGeneration":0,"sampleIndex":sample}), session).unwrap()
}
fn admit(store: &RetainedJournal, operation: &str) -> AdmissionToken {
    match store
        .admit(&request(store.session_id(), operation, 0), 101)
        .unwrap()
    {
        Admission::Created(token) => token,
        other => panic!("unexpected {other:?}"),
    }
}
fn dispatch(store: &RetainedJournal, operation: &str) -> DispatchToken {
    store
        .dispatch(admit(store, operation), "dispatch-one", 102)
        .unwrap()
}
fn executing(store: &RetainedJournal, operation: &str) -> ExecutionPermit {
    store
        .start(
            dispatch(store, operation),
            json!({"documentId":"document"}),
            103,
        )
        .unwrap()
}
fn payload() -> Value {
    json!({"success":true,"observed":{"angle":14.999999999999993,"object":"unit"}})
}
#[test]
fn ambiguous_native_failure_is_retained_without_committing_a_terminal_failure() {
    let dir = Directory::new();
    let store = dir.retained(RetentionLimits::default());
    let permit = executing(&store, "uncertain-native");
    let result = json!({"success":false,"nativeExecutionStarted":true,"outcome":"unknown","observedValue":17});
    let completion = store.finish_unknown(permit, result.clone()).unwrap();
    assert_eq!(completion.persistence, Persistence::Unconfirmed);
    assert_eq!(completion.observation.outcome, ObservedOutcome::Unknown);
    assert_eq!(completion.observation.result, Some(result.clone()));
    let status = store.status("session-a", "uncertain-native").unwrap();
    assert_eq!(status.authority, Authority::ObservedNondurable);
    assert!(status.reconciliation_required);
    assert_eq!(
        status.durable.unwrap().record.unwrap().phase,
        Phase::Executing
    );
    assert_eq!(status.observed.unwrap().result, Some(result));
    assert!(matches!(
        store
            .admit(&request("session-a", "uncertain-native", 0), 200)
            .unwrap(),
        Admission::Existing(_)
    ));
}
fn small(entries: usize, bytes: usize, total: usize) -> RetentionLimits {
    RetentionLimits {
        max_entries: entries,
        max_result_bytes: bytes,
        max_reserved_bytes: total,
    }
}

#[test]
fn durable_result_survives_restart_while_observation_is_process_local() {
    let dir = Directory::new();
    let store = dir.retained(RetentionLimits::default());
    let complete = store
        .finish(executing(&store, "one"), payload(), Ok(104))
        .unwrap();
    assert_eq!(complete.persistence, Persistence::Committed);
    assert_eq!(complete.observation.result, Some(payload()));
    let status = store.status("session-a", "one").unwrap();
    assert_eq!(status.authority, Authority::DurableTerminal);
    assert!(!status.reconciliation_required);
    let fresh = RetainedJournal::new(dir.open("session-b"), RetentionLimits::default()).unwrap();
    let old = fresh.status("session-a", "one").unwrap();
    assert!(old.historical && old.observed.is_none());
    assert_eq!(old.authority, Authority::DurableTerminal);
    assert_eq!(old.durable.unwrap().record.unwrap().result, Some(payload()));
}

#[test]
fn before_commit_failure_retains_exact_observation_without_replay() {
    let dir = Directory::new();
    let store = dir.retained(RetentionLimits::default());
    let permit = executing(&store, "one");
    *store.inner.journal.inner.fault.lock().unwrap() = Some(Fault::BeforeCommit);
    let completion = store.finish(permit, payload(), Ok(104)).unwrap();
    assert_eq!(completion.persistence, Persistence::Unconfirmed);
    assert!(
        completion
            .persistence_error
            .unwrap()
            .reconciliation_required
    );
    let status = store.status("session-a", "one").unwrap();
    assert_eq!(status.authority, Authority::ObservedNondurable);
    assert_eq!(status.observed.unwrap().result, Some(payload()));
    assert_eq!(
        status.durable.unwrap().record.unwrap().phase,
        Phase::Executing
    );
    assert!(matches!(
        store.admit(&request("session-a", "one", 0), 105).unwrap(),
        Admission::Existing(_)
    ));
}

#[test]
fn lost_terminal_ack_reconciles_committed_authority_without_rewrite() {
    let dir = Directory::new();
    let store = dir.retained(RetentionLimits::default());
    let permit = executing(&store, "one");
    *store.inner.journal.inner.fault.lock().unwrap() = Some(Fault::AfterCommit);
    let completion = store.finish(permit, payload(), Ok(104)).unwrap();
    assert_eq!(completion.persistence, Persistence::Unconfirmed);
    let status = store.status("session-a", "one").unwrap();
    assert_eq!(status.authority, Authority::DurableTerminal);
    assert!(!status.reconciliation_required);
    assert!(status.retention_error.is_some());
    assert_eq!(status.durable.unwrap().record.unwrap().events.len(), 4);
    assert!(matches!(
        store.admit(&request("session-a", "one", 0), 105).unwrap(),
        Admission::Existing(_)
    ));
}

#[test]
fn clock_errors_and_backwards_timestamps_keep_observed_payload() {
    for clock in [
        Err(error("clock_failed", "clock unavailable")),
        Ok(102),
        Ok(u64::MAX),
    ] {
        let dir = Directory::new();
        let store = dir.retained(RetentionLimits::default());
        let result = store
            .finish(executing(&store, "one"), payload(), clock)
            .unwrap();
        assert_eq!(result.persistence, Persistence::Unconfirmed);
        let status = store.status("session-a", "one").unwrap();
        assert_eq!(status.observed.unwrap().result, Some(payload()));
        assert_eq!(status.authority, Authority::ObservedNondurable);
        assert!(status.reconciliation_required);
    }
}

#[test]
fn reservation_exhaustion_precedes_admission_and_counts_unfilled_slots() {
    for limits in [small(1, 1024, 2048), small(2, 1024, 1024)] {
        let dir = Directory::new();
        let store = dir.retained(limits);
        let permit = executing(&store, "first");
        let failure = store
            .admit(&request("session-a", "second", 0), 104)
            .unwrap_err();
        assert_eq!(failure.code, "operation_retention_capacity");
        assert!(store
            .inner
            .journal
            .lookup("session-a", "second")
            .unwrap()
            .is_none());
        store.finish(permit, payload(), Ok(104)).unwrap();
        assert_eq!(
            store
                .admit(&request("session-a", "second", 0), 105)
                .unwrap_err()
                .code,
            "operation_retention_capacity"
        );
        assert_eq!(
            store
                .status("session-a", "first")
                .unwrap()
                .observed
                .unwrap()
                .result,
            Some(payload())
        );
    }
}

#[test]
fn duplicates_and_conflicting_requests_cannot_replace_terminal_observation() {
    let dir = Directory::new();
    let store = dir.retained(RetentionLimits::default());
    let permit = executing(&store, "one");
    // Private fixture duplicates the otherwise non-Clone permit to verify the
    // runtime guard also refuses stale/duplicate callback authority.
    let duplicate = ExecutionPermit(store.lease(
        "one",
        &permit.0.digest,
        permit.0.claim.as_ref().unwrap().clone(),
    ));
    store.finish(permit, payload(), Ok(104)).unwrap();
    assert_eq!(
        store
            .finish(duplicate, json!({"success":false}), Ok(105))
            .unwrap_err()
            .code,
        "operation_conflict"
    );
    assert!(matches!(
        store.admit(&request("session-a", "one", 0), 105).unwrap(),
        Admission::Existing(_)
    ));
    assert_eq!(
        store
            .admit(&request("session-a", "one", 1), 105)
            .unwrap_err()
            .code,
        "operation_conflict"
    );
    assert_eq!(
        store
            .status("session-a", "one")
            .unwrap()
            .observed
            .unwrap()
            .result,
        Some(payload())
    );
}

#[test]
fn oversized_result_keeps_explicit_bounded_omission_without_success_durability() {
    let dir = Directory::new();
    let store = dir.retained(small(2, 64, 128));
    let completion = store
        .finish(
            executing(&store, "one"),
            json!({"success":true,"large":"x".repeat(65)}),
            Ok(104),
        )
        .unwrap();
    assert_eq!(completion.persistence, Persistence::Unconfirmed);
    assert_eq!(
        completion.persistence_error.unwrap().code,
        "operation_observation_oversize"
    );
    assert_eq!(completion.observation.outcome, ObservedOutcome::Succeeded);
    assert!(completion.observation.payload_omitted && completion.observation.result.is_none());
    assert_eq!(
        store.status("session-a", "one").unwrap().authority,
        Authority::ObservedNondurable
    );
    assert_eq!(store.inner.state.lock().unwrap().reserved_bytes, 64);
}

#[test]
fn malformed_terminal_retains_exact_small_payload_without_committing_it() {
    let dir = Directory::new();
    let store = dir.retained(RetentionLimits::default());
    let invalid = json!({"success":"maybe","evidence":12});
    let completion = store
        .finish(executing(&store, "one"), invalid.clone(), Ok(104))
        .unwrap();
    assert_eq!(completion.persistence, Persistence::Unconfirmed);
    assert_eq!(completion.observation.result, Some(invalid));
    assert_eq!(completion.observation.outcome, ObservedOutcome::Unknown);
}

#[test]
fn deeply_nested_result_is_an_explicit_omission_not_an_unreadable_observation() {
    let dir = Directory::new();
    let store = dir.retained(RetentionLimits::default());
    let mut nested = Value::Null;
    for _ in 0..140 {
        nested = Value::Array(vec![nested]);
    }
    let completion = store
        .finish(
            executing(&store, "one"),
            json!({"success":true,"nested":nested}),
            Ok(104),
        )
        .unwrap();
    assert_eq!(completion.persistence, Persistence::Unconfirmed);
    assert_eq!(
        completion.persistence_error.unwrap().code,
        "operation_observation_depth"
    );
    assert!(completion.observation.payload_omitted);
    let status = store.status("session-a", "one").unwrap();
    assert_eq!(status.authority, Authority::ObservedNondurable);
    assert!(status.observed.unwrap().result.is_none());
}

#[test]
fn unavailable_database_does_not_hide_memory_observation_or_claim_missing_execution() {
    let dir = Directory::new();
    let store = dir.retained(RetentionLimits::default());
    let permit = executing(&store, "one");
    let path = store.inner.journal.database_path();
    let moved = dir.0.join("held.sqlite3");
    fs::rename(path, &moved).unwrap();
    let completion = store.finish(permit, payload(), Ok(104)).unwrap();
    assert_eq!(completion.persistence, Persistence::Unconfirmed);
    let status = store.status("session-a", "one").unwrap();
    assert!(status.durable.is_none() && status.storage_error.is_some());
    assert_eq!(status.observed.unwrap().result, Some(payload()));
    assert_eq!(status.authority, Authority::ObservedNondurable);
    fs::rename(&moved, path).unwrap();
    assert_eq!(
        store
            .status("session-a", "one")
            .unwrap()
            .durable
            .unwrap()
            .record
            .unwrap()
            .phase,
        Phase::Executing
    );
}

#[test]
fn historical_execution_is_unknown_without_reconstructing_lost_observation() {
    let dir = Directory::new();
    let store = dir.retained(RetentionLimits::default());
    let permit = executing(&store, "one");
    store
        .finish(permit, payload(), Err(error("clock_failed", "lost clock")))
        .unwrap();
    drop(store);
    let fresh = RetainedJournal::new(dir.open("session-b"), RetentionLimits::default()).unwrap();
    let old = fresh.status("session-a", "one").unwrap();
    assert_eq!(old.authority, Authority::Unknown);
    assert!(old.historical && old.observed.is_none() && old.reconciliation_required);
    assert_eq!(old.durable.unwrap().state, crate::State::Interrupted);
    assert_eq!(
        fresh
            .admit(&request("session-a", "one", 0), 105)
            .unwrap_err()
            .code,
        "session_mismatch"
    );
}

#[test]
fn clone_concurrent_admission_issues_one_token_and_one_reservation() {
    let dir = Directory::new();
    let store = dir.retained(small(1, 1024, 1024));
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let clone = store.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                clone.admit(&request("session-a", "one", 0), 101).unwrap()
            })
        })
        .collect();
    let mut count = 0;
    for handle in handles {
        if let Admission::Created(token) = handle.join().unwrap() {
            count += 1;
            drop(token);
        }
    }
    assert_eq!(count, 1);
    assert_eq!(store.inner.state.lock().unwrap().reserved_bytes, 1024);
    assert_eq!(
        store.status("session-a", "one").unwrap().authority,
        Authority::Unknown
    );
}

#[test]
fn wrapper_registration_cannot_be_reset_while_original_journal_clone_is_alive() {
    let dir = Directory::new();
    let journal = dir.open("session-a");
    let store = RetainedJournal::new(journal.clone(), RetentionLimits::default()).unwrap();
    assert!(
        RetainedJournal::new(journal.clone(), RetentionLimits::default())
            .err()
            .is_some_and(|error| error.code == "operation_retention_registered")
    );
    drop(store);
    assert!(RetainedJournal::new(journal, RetentionLimits::default())
        .err()
        .is_some_and(|error| error.code == "operation_retention_registered"));
}

#[test]
fn foreign_owner_token_cannot_advance_either_journal_and_dropped_permit_is_unknown() {
    let a = Directory::new();
    let b = Directory::new();
    let first = a.retained(RetentionLimits::default());
    let second = b.retained(RetentionLimits::default());
    let token = admit(&first, "one");
    assert_eq!(
        second.dispatch(token, "dispatch", 102).unwrap_err().code,
        "operation_conflict"
    );
    assert_eq!(
        first
            .inner
            .journal
            .lookup("session-a", "one")
            .unwrap()
            .unwrap()
            .phase,
        Phase::Admitted
    );
    assert!(second
        .inner
        .journal
        .lookup("session-a", "one")
        .unwrap()
        .is_none());
    drop(executing(&first, "two"));
    assert_eq!(
        first.status("session-a", "two").unwrap().authority,
        Authority::Unknown
    );
}

#[test]
fn slow_status_does_not_hold_retention_lock_and_block_observed_clock_failure() {
    let dir = Directory::new();
    let store = dir.retained(RetentionLimits::default());
    let permit = executing(&store, "one");
    let journal_guard = store.inner.journal.inner.uncertain.lock().unwrap();
    let status_store = store.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let status_thread = thread::spawn(move || {
        started_tx.send(()).unwrap();
        status_store.status("session-a", "one")
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let finish_store = store.clone();
    let (tx, rx) = mpsc::channel();
    let finish_thread = thread::spawn(move || {
        tx.send(finish_store.finish(
            permit,
            payload(),
            Err(error("clock_failed", "clock unavailable")),
        ))
        .unwrap()
    });
    let result = rx.recv_timeout(Duration::from_secs(2));
    drop(journal_guard);
    finish_thread.join().unwrap();
    let status = status_thread.join().unwrap().unwrap();
    assert_eq!(result.unwrap().unwrap().observation.result, Some(payload()));
    assert_eq!(status.authority, Authority::ObservedNondurable);
}

#[test]
fn preexecution_refusal_uses_distinct_observed_not_executed_outcome() {
    let dir = Directory::new();
    let store = dir.retained(RetentionLimits::default());
    let completion = store
        .not_executed(
            dispatch(&store, "one"),
            json!({"success":false,"nativeExecutionStarted":false,"reason":"target closed"}),
            Ok(103),
        )
        .unwrap();
    assert_eq!(completion.persistence, Persistence::Committed);
    assert_eq!(completion.observation.outcome, ObservedOutcome::NotExecuted);
    assert_eq!(
        store
            .status("session-a", "one")
            .unwrap()
            .durable
            .unwrap()
            .record
            .unwrap()
            .phase,
        Phase::NotExecuted
    );
}

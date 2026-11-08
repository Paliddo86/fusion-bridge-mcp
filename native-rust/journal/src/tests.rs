use super::*;
use fusion_mechanical_core::prepare_request;
use serde_json::json;
use std::sync::Barrier;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let value = Self(std::env::temp_dir().join(format!(
            "fusion-rust-journal-test-{}-{}-{}",
            std::process::id(),
            now_ms().unwrap(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        #[cfg(not(windows))]
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let mut builder = builder;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        #[cfg(not(windows))]
        builder.create(&value.0).unwrap();
        #[cfg(windows)]
        validate_directory(&value.0, true).unwrap();
        value
    }
    fn open(&self, session: &str) -> Journal {
        Journal::open_at(&self.0, session, "rust-test.1", Limits::default(), 100).unwrap()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn request(session: &str, operation: &str, sample: usize) -> PreparedRequest {
    prepare_request(json!({"op":"exploded_preview_sample", "sessionId":session, "operationId":operation,
        "target":{"documentId":"document", "revision":"revision"}, "previewId":"preview", "expectedGeneration":0, "sampleIndex":sample}), session).unwrap()
}
fn admit(journal: &Journal, operation: &str) -> AdmissionClaim {
    match journal
        .admit(&request(journal.session_id(), operation, 0), 101)
        .unwrap()
    {
        Admission::Created(claim) => claim,
        other => panic!("unexpected {other:?}"),
    }
}
fn dispatch(journal: &Journal, operation: &str) -> DispatchClaim {
    match journal
        .dispatch(&admit(journal, operation), "dispatch-a", 102)
        .unwrap()
    {
        DispatchOutcome::Claimed(claim) => claim,
        other => panic!("unexpected {other:?}"),
    }
}
fn executing(journal: &Journal, operation: &str) -> DispatchClaim {
    let claim = dispatch(journal, operation);
    assert!(
        journal
            .start(
                &claim,
                json!({"documentId":"document", "revision":"revision"}),
                103
            )
            .unwrap()
            .claimed
    );
    claim
}

#[test]
fn commits_admission_dispatch_execution_terminal_then_reopens_exact_history() {
    let directory = Directory::new();
    let journal = directory.open("session-a");
    let claim = executing(&journal, "op");
    let finished = journal
        .finish(
            &claim,
            json!({"success":true,"sampleIndex":0,"value":14.999999999999993}),
            104,
        )
        .unwrap();
    assert!(finished.claimed);
    assert_eq!(
        finished
            .record
            .events
            .iter()
            .map(|event| event.phase)
            .collect::<Vec<_>>(),
        [
            Phase::Admitted,
            Phase::Dispatched,
            Phase::Executing,
            Phase::Succeeded
        ]
    );
    assert_eq!(
        finished.record.request_digest,
        request_digest(request("session-a", "op", 0).canonical_bytes())
    );
    drop(journal);
    let new = directory.open("session-b");
    assert_eq!(
        new.lookup("session-a", "op").unwrap(),
        Some(finished.record)
    );
    let status = new.status("session-a", "op").unwrap();
    assert!(status.historical);
    assert_eq!(status.state, State::Succeeded);
    assert_eq!(status.outcome, Outcome::Recorded);
    assert!(!status.reconciliation_required);
    assert_eq!(
        new.start(&claim, json!({}), 105).unwrap_err().code,
        "session_mismatch"
    );
}

#[test]
fn same_session_cannot_be_reopened_and_missing_registered_database_is_not_recreated() {
    let directory = Directory::new();
    let journal = directory.open("session");
    assert_eq!(
        Journal::open(&directory.0, "session", "test", Limits::default())
            .err()
            .unwrap()
            .code,
        "session_reused"
    );
    fs::rename(
        journal.database_path(),
        directory.0.join("retained.sqlite3"),
    )
    .unwrap();
    assert!(
        journal
            .lookup("session", "absent")
            .unwrap_err()
            .reconciliation_required
    );
    assert!(!journal.database_path().exists());
}

#[test]
fn old_session_nonterminal_is_unknown_interrupted_and_never_replayed() {
    let directory = Directory::new();
    let old = directory.open("old");
    admit(&old, "admitted");
    dispatch(&old, "dispatched");
    let claim = executing(&old, "executing");
    let new = directory.open("new");
    for operation in ["admitted", "dispatched", "executing"] {
        let status = new.status("old", operation).unwrap();
        assert_eq!(status.state, State::Interrupted);
        assert_eq!(status.outcome, Outcome::Unknown);
        assert!(status.reconciliation_required);
    }
    assert_eq!(
        new.admit(&request("old", "new-request", 0), 200)
            .unwrap_err()
            .code,
        "session_mismatch"
    );
    assert_eq!(
        new.finish(&claim, json!({"success":true}), 200)
            .unwrap_err()
            .code,
        "session_mismatch"
    );
}

#[test]
fn duplicate_admission_never_reissues_effect_claim_and_conflict_preserves_original() {
    let directory = Directory::new();
    let journal = directory.open("session");
    let claim = admit(&journal, "op");
    assert!(matches!(
        journal.admit(&request("session", "op", 0), 102).unwrap(),
        Admission::Existing(_)
    ));
    assert_eq!(
        journal
            .admit(&request("session", "op", 1), 103)
            .unwrap_err()
            .code,
        "operation_conflict"
    );
    let record = journal.lookup("session", "op").unwrap().unwrap();
    assert_eq!(record.request_digest, claim.request_digest());
    assert_eq!(record.events.len(), 1);
    assert_eq!(
        journal.status("session", "op").unwrap().state,
        State::ReconciliationRequired
    );
}

#[test]
fn concurrent_duplicate_events_and_admission_issue_one_claim() {
    let directory = Directory::new();
    let journal = directory.open("session");
    let barrier = Arc::new(Barrier::new(12));
    let handles: Vec<_> = (0..12)
        .map(|_| {
            let journal = journal.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                journal.admit(&request("session", "op", 0), 101).unwrap()
            })
        })
        .collect();
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    let claims: Vec<_> = outcomes
        .into_iter()
        .filter_map(|value| match value {
            Admission::Created(claim) => Some(claim),
            _ => None,
        })
        .collect();
    assert_eq!(claims.len(), 1);
    let claim = match journal.dispatch(&claims[0], "dispatch", 102).unwrap() {
        DispatchOutcome::Claimed(claim) => claim,
        _ => panic!(),
    };
    let barrier = Arc::new(Barrier::new(12));
    let handles: Vec<_> = (0..12)
        .map(|_| {
            let journal = journal.clone();
            let barrier = barrier.clone();
            let claim = claim.clone();
            std::thread::spawn(move || {
                barrier.wait();
                journal
                    .start(&claim, json!({"documentId":"document"}), 103)
                    .unwrap()
                    .claimed
            })
        })
        .collect();
    assert_eq!(
        handles
            .into_iter()
            .map(|handle| usize::from(handle.join().unwrap()))
            .sum::<usize>(),
        1
    );
}

#[test]
fn concurrent_fresh_initializers_publish_one_valid_database_without_overwrite() {
    let directory = Directory::new();
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|number| {
            let directory = directory.0.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                Journal::open_at(
                    directory,
                    &format!("session-{number}"),
                    "test",
                    Limits::default(),
                    100,
                )
                .map(|journal| journal.session_id().to_owned())
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    let journal = directory.open("last-session");
    let connection = journal.connect().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row
                .get::<_, usize>(0))
            .unwrap(),
        9
    );
}

#[test]
fn terminal_is_immutable_and_delivery_failure_cannot_erase_execution() {
    let directory = Directory::new();
    let journal = directory.open("session");
    let claim = executing(&journal, "op");
    assert!(
        !journal
            .not_executed(
                &claim,
                json!({"success":false,"nativeExecutionStarted":false}),
                104
            )
            .unwrap()
            .claimed
    );
    let terminal = journal
        .finish(&claim, json!({"success":true}), 105)
        .unwrap()
        .record;
    for replacement in [
        json!({"success":false}),
        json!({"success":true,"changed":true}),
    ] {
        let duplicate = journal.finish(&claim, replacement, 106).unwrap();
        assert!(!duplicate.claimed);
        assert_eq!(duplicate.record, terminal);
    }
    assert!(!journal.start(&claim, json!({}), 107).unwrap().claimed);
}

#[test]
fn not_executed_requires_false_success_and_explicit_no_native_work() {
    let directory = Directory::new();
    let journal = directory.open("session");
    let claim = dispatch(&journal, "op");
    assert_eq!(
        journal
            .not_executed(&claim, json!({"success":false}), 103)
            .unwrap_err()
            .code,
        "operation_record_invalid"
    );
    assert_eq!(
        journal
            .not_executed(
                &claim,
                json!({"success":true,"nativeExecutionStarted":false}),
                103
            )
            .unwrap_err()
            .code,
        "operation_record_invalid"
    );
    assert!(
        journal
            .not_executed(
                &claim,
                json!({"success":false,"nativeExecutionStarted":false}),
                104
            )
            .unwrap()
            .claimed
    );
    assert_eq!(
        journal.status("session", "op").unwrap().state,
        State::NotExecuted
    );
    assert!(!journal.start(&claim, json!({}), 105).unwrap().claimed);
}

#[test]
fn uncertain_admission_before_or_after_commit_never_issues_a_claim_on_retry() {
    for fault in [Fault::BeforeCommit, Fault::AfterCommit] {
        let directory = Directory::new();
        let journal = directory.open("session");
        *journal.inner.fault.lock().unwrap() = Some(fault);
        assert!(
            journal
                .admit(&request("session", "op", 0), 101)
                .unwrap_err()
                .reconciliation_required
        );
        assert_eq!(
            journal
                .admit(&request("session", "op", 0), 102)
                .unwrap_err()
                .code,
            "operation_reconciliation_required"
        );
        let record = journal.lookup("session", "op").unwrap();
        assert_eq!(record.is_some(), fault == Fault::AfterCommit);
        let status = journal.status("session", "op").unwrap();
        assert_eq!(status.outcome, Outcome::Unknown);
        assert!(status.reconciliation_required);
    }
}

#[test]
fn lost_dispatch_acknowledgement_cannot_be_retried_even_when_commit_rolled_back() {
    for fault in [Fault::BeforeCommit, Fault::AfterCommit] {
        let directory = Directory::new();
        let journal = directory.open("session");
        let claim = admit(&journal, "op");
        *journal.inner.fault.lock().unwrap() = Some(fault);
        assert!(
            journal
                .dispatch(&claim, "dispatch", 102)
                .unwrap_err()
                .reconciliation_required
        );
        assert_eq!(
            journal.dispatch(&claim, "dispatch", 103).unwrap_err().code,
            "operation_reconciliation_required"
        );
        assert_eq!(
            journal.lookup("session", "op").unwrap().unwrap().phase,
            if fault == Fault::AfterCommit {
                Phase::Dispatched
            } else {
                Phase::Admitted
            }
        );
    }
}

#[test]
fn lost_start_acknowledgement_never_authorizes_native_execution() {
    for fault in [Fault::BeforeCommit, Fault::AfterCommit] {
        let directory = Directory::new();
        let journal = directory.open("session");
        let claim = dispatch(&journal, "op");
        *journal.inner.fault.lock().unwrap() = Some(fault);
        assert!(
            journal
                .start(&claim, json!({}), 103)
                .unwrap_err()
                .reconciliation_required
        );
        assert_eq!(
            journal.start(&claim, json!({}), 104).unwrap_err().code,
            "operation_reconciliation_required"
        );
        assert_eq!(
            journal.status("session", "op").unwrap().outcome,
            Outcome::Unknown
        );
    }
}

#[test]
fn lost_terminal_ack_reconciles_durable_terminal_or_retains_unknown_without_reexecution() {
    for fault in [Fault::BeforeCommit, Fault::AfterCommit] {
        let directory = Directory::new();
        let journal = directory.open("session");
        let claim = executing(&journal, "op");
        *journal.inner.fault.lock().unwrap() = Some(fault);
        assert!(
            journal
                .finish(&claim, json!({"success":true,"nativeObserved":true}), 104)
                .unwrap_err()
                .reconciliation_required
        );
        assert_eq!(
            journal.start(&claim, json!({}), 105).unwrap_err().code,
            "operation_reconciliation_required"
        );
        let status = journal.status("session", "op").unwrap();
        assert_eq!(
            status.outcome,
            if fault == Fault::AfterCommit {
                Outcome::Recorded
            } else {
                Outcome::Unknown
            }
        );
        assert_eq!(
            status.record.unwrap().phase,
            if fault == Fault::AfterCommit {
                Phase::Succeeded
            } else {
                Phase::Executing
            }
        );
    }
}

#[test]
fn refuses_foreign_empty_corrupt_database_without_changing_bytes() {
    for bytes in [Vec::new(), b"not a SQLite operation journal".to_vec()] {
        let directory = Directory::new();
        let path = directory.0.join(DATABASE_NAME);
        fs::write(&path, &bytes).unwrap();
        assert!(Journal::open(&directory.0, "session", "test", Limits::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    let directory = Directory::new();
    let path = directory.0.join(DATABASE_NAME);
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE unrelated (id INTEGER); INSERT INTO unrelated VALUES (7);")
        .unwrap();
    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        Journal::open(&directory.0, "session", "test", Limits::default())
            .err()
            .unwrap()
            .code,
        "operation_storage_corrupt"
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn refuses_newer_version_extra_schema_objects_and_checksum_tampering() {
    for mutation in [
        "PRAGMA user_version=2",
        "CREATE TRIGGER extra AFTER UPDATE ON operations BEGIN SELECT 1; END",
        "UPDATE operations SET record_digest='incorrect'",
    ] {
        let directory = Directory::new();
        let journal = directory.open("session");
        admit(&journal, "op");
        Connection::open(journal.database_path())
            .unwrap()
            .execute_batch(mutation)
            .unwrap();
        let before = fs::read(journal.database_path()).unwrap();
        assert!(journal.lookup("session", "op").is_err());
        assert!(journal.admit(&request("session", "next", 0), 200).is_err());
        assert_eq!(fs::read(journal.database_path()).unwrap(), before);
    }
}

#[test]
fn valid_checksum_does_not_hide_invalid_phase_history_or_request_identity() {
    for alteration in 0..3 {
        let directory = Directory::new();
        let journal = directory.open("session");
        let _claim = executing(&journal, "op");
        let mut record = journal.lookup("session", "op").unwrap().unwrap();
        match alteration {
            0 => record.events[1].phase = Phase::Succeeded,
            1 => record.operation_id = "other".into(),
            _ => record.request_json = record.request_json.replace("preview", "modified"),
        }
        let bytes = encode(&record, Limits::default().max_record_bytes).unwrap();
        Connection::open(journal.database_path())
            .unwrap()
            .execute(
                "UPDATE operations SET record_json=?1,record_digest=?2",
                params![bytes, record_digest(&bytes)],
            )
            .unwrap();
        assert_eq!(
            journal.lookup("session", "op").unwrap_err().code,
            "operation_storage_corrupt"
        );
    }
}

#[test]
fn capacity_retains_nonterminal_and_terminal_history_without_eviction() {
    let directory = Directory::new();
    let limits = Limits {
        max_records: 2,
        ..Limits::default()
    };
    let journal = Journal::open_at(&directory.0, "session", "test", limits, 100).unwrap();
    admit(&journal, "pending");
    let claim = executing(&journal, "finished");
    journal
        .finish(&claim, json!({"success":true}), 104)
        .unwrap();
    assert_eq!(
        journal
            .admit(&request("session", "overflow", 0), 200)
            .unwrap_err()
            .code,
        "operation_capacity"
    );
    assert_eq!(
        journal.lookup("session", "pending").unwrap().unwrap().phase,
        Phase::Admitted
    );
    assert_eq!(
        journal
            .lookup("session", "finished")
            .unwrap()
            .unwrap()
            .phase,
        Phase::Succeeded
    );
}

#[test]
fn record_byte_limit_refuses_large_terminal_without_truncating_existing_record() {
    let directory = Directory::new();
    let journal = Journal::open_at(
        &directory.0,
        "session",
        "test",
        Limits {
            max_record_bytes: 2048,
            ..Limits::default()
        },
        100,
    )
    .unwrap();
    let claim = executing(&journal, "op");
    let failure = journal
        .finish(
            &claim,
            json!({"success":true,"large":"x".repeat(4096)}),
            104,
        )
        .unwrap_err();
    assert_eq!(failure.code, "operation_record_too_large");
    assert!(failure.reconciliation_required);
    assert_eq!(
        journal.lookup("session", "op").unwrap().unwrap().phase,
        Phase::Executing
    );
    assert_eq!(
        journal.status("session", "op").unwrap().outcome,
        Outcome::Unknown
    );
    assert_eq!(
        journal
            .finish(&claim, json!({"success":true}), 105)
            .unwrap_err()
            .code,
        "operation_reconciliation_required"
    );
}

#[test]
fn refusal_of_reads_bad_clock_and_invalid_dispatch_does_not_mutate_history() {
    let directory = Directory::new();
    let journal = directory.open("session");
    let read = prepare_request(json!({"op":"inspect"}), "session").unwrap();
    assert_eq!(
        journal.admit(&read, 100).unwrap_err().code,
        "operation_record_invalid"
    );
    let claim = admit(&journal, "op");
    assert_eq!(
        journal.dispatch(&claim, "", 102).unwrap_err().code,
        "operation_record_invalid"
    );
    assert_eq!(
        journal.dispatch(&claim, "dispatch", 1).unwrap_err().code,
        "operation_record_invalid"
    );
    assert_eq!(
        journal
            .lookup("session", "op")
            .unwrap()
            .unwrap()
            .events
            .len(),
        1
    );
    let absent = journal.status("session", "absent").unwrap();
    assert_eq!(absent.state, State::Unknown);
    assert_eq!(absent.outcome, Outcome::Unknown);
}

#[test]
fn legacy_database_bytes_are_never_opened_or_migrated() {
    let directory = Directory::new();
    let legacy = directory.0.join("operations-v1.sqlite3");
    let bytes = b"legacy data must remain untouched";
    fs::write(&legacy, bytes).unwrap();
    let journal = directory.open("session");
    admit(&journal, "op");
    assert_eq!(fs::read(legacy).unwrap(), bytes);
    assert_ne!(
        journal.database_path().file_name().unwrap(),
        "operations-v1.sqlite3"
    );
}

#[cfg(unix)]
#[test]
fn symlink_hardlink_world_writable_and_parent_traversal_are_refused() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let directory = Directory::new();
    let linked = directory.0.join("link");
    let real = directory.0.join("real");
    fs::create_dir(&real).unwrap();
    symlink(&real, &linked).unwrap();
    assert_eq!(
        Journal::open(&linked, "session", "test", Limits::default())
            .err()
            .unwrap()
            .code,
        "operation_storage_invalid_path"
    );
    let journal = directory.open("session");
    let link = directory.0.join("external-link");
    fs::hard_link(journal.database_path(), &link).unwrap();
    assert_eq!(
        journal.lookup("session", "absent").unwrap_err().code,
        "operation_storage_invalid_path"
    );
    fs::remove_file(link).unwrap();
    let target = directory.0.join("other");
    fs::rename(journal.database_path(), &target).unwrap();
    symlink(&target, journal.database_path()).unwrap();
    assert_eq!(
        journal.lookup("session", "absent").unwrap_err().code,
        "operation_storage_invalid_path"
    );
    assert_eq!(
        Journal::open(
            directory.0.join("missing/../other"),
            "session",
            "test",
            Limits::default()
        )
        .err()
        .unwrap()
        .code,
        "operation_storage_invalid_path"
    );
    fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(
        Journal::open(&directory.0, "session", "test", Limits::default())
            .err()
            .unwrap()
            .code,
        "operation_storage_invalid_path"
    );
}

#[test]
fn sqlite_immediate_lock_contention_is_bounded_and_does_not_claim_admission() {
    let directory = Directory::new();
    let journal = directory.open("session");
    let blocker = Connection::open(journal.database_path()).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let failure = journal
        .admit(&request("session", "op", 0), 101)
        .unwrap_err();
    assert!(failure.reconciliation_required);
    blocker.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        journal
            .admit(&request("session", "op", 0), 102)
            .unwrap_err()
            .code,
        "operation_reconciliation_required"
    );
    assert!(journal.lookup("session", "op").unwrap().is_none());
}

// This worker exits without Rust destructors to model a process disappearing at
// persisted lifecycle boundaries and with an outstanding SQLite transaction.
#[test]
fn crash_worker() {
    let Ok(directory) = std::env::var("FUSION_JOURNAL_CRASH_DIRECTORY") else {
        return;
    };
    let stage = std::env::var("FUSION_JOURNAL_CRASH_STAGE").unwrap();
    if stage == "foreign-hot-rollback" {
        let connection = Connection::open(Path::new(&directory).join(DATABASE_NAME)).unwrap();
        connection.execute_batch("CREATE TABLE foreign_records (payload BLOB); INSERT INTO foreign_records VALUES (zeroblob(16)); BEGIN IMMEDIATE; PRAGMA cache_size=1; UPDATE foreign_records SET payload=zeroblob(1048576);").unwrap();
        std::process::exit(73);
    }
    let journal =
        Journal::open_at(directory, "crashed-session", "test", Limits::default(), 100).unwrap();
    let admission = admit(&journal, "op");
    if stage == "admitted" {
        std::process::exit(73);
    }
    let claim = match journal.dispatch(&admission, "dispatch", 102).unwrap() {
        DispatchOutcome::Claimed(claim) => claim,
        _ => panic!(),
    };
    if stage == "dispatched" {
        std::process::exit(73);
    }
    journal
        .start(&claim, json!({"documentId":"owned-fixture"}), 103)
        .unwrap();
    if stage == "executing" {
        std::process::exit(73);
    }
    if stage == "hot-rollback" {
        let connection = journal.connect().unwrap();
        connection
            .execute_batch("BEGIN IMMEDIATE; UPDATE operations SET record_digest='not-committed';")
            .unwrap();
        // Force dirty pages through SQLite's pager to leave a genuine rollback
        // journal; this is not a mocked in-memory failure.
        connection.execute_batch("PRAGMA cache_size=1; CREATE TABLE uncommitted (payload BLOB); INSERT INTO uncommitted VALUES (zeroblob(1048576));").unwrap();
        std::process::exit(73);
    }
    assert_eq!(stage, "terminal");
    journal
        .finish(&claim, json!({"success":true,"geometryObserved":true}), 104)
        .unwrap();
    std::process::exit(73);
}

#[test]
fn child_process_crash_reopens_history_and_recovers_hot_rollback_without_redispatch() {
    for (stage, phase) in [
        ("admitted", Phase::Admitted),
        ("dispatched", Phase::Dispatched),
        ("executing", Phase::Executing),
        ("terminal", Phase::Succeeded),
        ("hot-rollback", Phase::Executing),
    ] {
        let directory = Directory::new();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::crash_worker", "--nocapture"])
            .env("FUSION_JOURNAL_CRASH_DIRECTORY", &directory.0)
            .env("FUSION_JOURNAL_CRASH_STAGE", stage)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(73),
            "stage {stage}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if stage == "hot-rollback" {
            assert!(directory
                .0
                .join(format!("{DATABASE_NAME}-journal"))
                .exists());
        }
        let journal = directory.open("fresh-session");
        let status = journal.status("crashed-session", "op").unwrap();
        assert_eq!(status.record.as_ref().unwrap().phase, phase);
        assert_eq!(
            status.outcome,
            if phase.is_terminal() {
                Outcome::Recorded
            } else {
                Outcome::Unknown
            }
        );
        if !phase.is_terminal() {
            assert_eq!(status.state, State::Interrupted);
        }
        assert_eq!(
            journal
                .admit(&request("crashed-session", "op", 0), 200)
                .unwrap_err()
                .code,
            "session_mismatch"
        );
    }
}

#[test]
fn foreign_hot_rollback_is_refused_before_sqlite_can_modify_its_files() {
    let directory = Directory::new();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "tests::crash_worker", "--nocapture"])
        .env("FUSION_JOURNAL_CRASH_DIRECTORY", &directory.0)
        .env("FUSION_JOURNAL_CRASH_STAGE", "foreign-hot-rollback")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(73));
    let database = directory.0.join(DATABASE_NAME);
    let rollback = directory.0.join(format!("{DATABASE_NAME}-journal"));
    let before_database = fs::read(&database).unwrap();
    let before_rollback = fs::read(&rollback).unwrap();
    assert!(!before_rollback.is_empty());
    let failure = Journal::open(&directory.0, "session", "test", Limits::default())
        .err()
        .unwrap();
    assert_eq!(failure.code, "operation_storage_corrupt");
    assert_eq!(fs::read(database).unwrap(), before_database);
    assert_eq!(fs::read(rollback).unwrap(), before_rollback);
}

#[test]
fn deeply_nested_native_result_fails_closed_after_execution() {
    let directory = Directory::new();
    let journal = directory.open("session");
    let claim = executing(&journal, "op");
    let mut nested = Value::Null;
    for _ in 0..140 {
        nested = Value::Array(vec![nested]);
    }
    let failure = journal
        .finish(&claim, json!({"success":true,"result":nested}), 104)
        .unwrap_err();
    assert!(failure.reconciliation_required);
    assert_eq!(
        journal.status("session", "op").unwrap().outcome,
        Outcome::Unknown
    );
}

#[test]
fn observed_result_without_executing_claim_is_unknown_and_blocks_further_claims() {
    let directory = Directory::new();
    let journal = directory.open("session");
    let claim = dispatch(&journal, "op");
    let failure = journal
        .finish(&claim, json!({"success":true}), 103)
        .unwrap_err();
    assert_eq!(failure.code, "operation_conflict");
    assert!(failure.reconciliation_required);
    assert_eq!(
        journal.start(&claim, json!({}), 104).unwrap_err().code,
        "operation_reconciliation_required"
    );
    assert_eq!(
        journal.status("session", "op").unwrap().outcome,
        Outcome::Unknown
    );
}

#[test]
fn page_bound_failure_never_authorizes_terminal_or_evicts_admission() {
    let directory = Directory::new();
    let journal = Journal::open_at(
        &directory.0,
        "session",
        "test",
        Limits {
            max_database_bytes: 32_768,
            ..Limits::default()
        },
        100,
    )
    .unwrap();
    let claim = executing(&journal, "op");
    let failure = journal
        .finish(
            &claim,
            json!({"success":true,"large":"x".repeat(40_000)}),
            104,
        )
        .unwrap_err();
    assert!(failure.reconciliation_required);
    assert!(fs::metadata(journal.database_path()).unwrap().len() <= 32_768);
    assert_eq!(
        journal.lookup("session", "op").unwrap().unwrap().phase,
        Phase::Executing
    );
}

#[test]
fn fresh_session_capacity_refuses_registration_without_eviction() {
    let directory = Directory::new();
    let limits = Limits {
        max_sessions: 1,
        ..Limits::default()
    };
    let journal = Journal::open_at(&directory.0, "session", "test", limits, 100).unwrap();
    admit(&journal, "retained");
    assert_eq!(
        Journal::open_at(&directory.0, "second", "test", limits, 200)
            .err()
            .unwrap()
            .code,
        "operation_capacity"
    );
    assert!(journal.lookup("session", "retained").unwrap().is_some());
}

#[test]
fn uncertain_missing_admission_capacity_is_bounded_and_fails_closed() {
    let directory = Directory::new();
    let journal = Journal::open_at(
        &directory.0,
        "session",
        "test",
        Limits {
            max_records: 1,
            ..Limits::default()
        },
        100,
    )
    .unwrap();
    *journal.inner.fault.lock().unwrap() = Some(Fault::BeforeCommit);
    assert!(
        journal
            .admit(&request("session", "uncertain", 0), 101)
            .unwrap_err()
            .reconciliation_required
    );
    assert_eq!(
        journal
            .admit(&request("session", "different", 0), 102)
            .unwrap_err()
            .code,
        "operation_reconciliation_required"
    );
    assert_eq!(journal.inner.uncertain.lock().unwrap().len(), 1);
    assert!(journal.lookup("session", "different").unwrap().is_none());
}

#[test]
fn full_uncertainty_ledger_marks_existing_execution_unknown_without_memory_growth() {
    let directory = Directory::new();
    let journal = Journal::open_at(
        &directory.0,
        "session",
        "test",
        Limits {
            max_records: 2,
            ..Limits::default()
        },
        100,
    )
    .unwrap();
    let claim = executing(&journal, "running");
    for operation in ["unknown-a", "unknown-b"] {
        *journal.inner.fault.lock().unwrap() = Some(Fault::BeforeCommit);
        assert!(
            journal
                .admit(&request("session", operation, 0), 105)
                .unwrap_err()
                .reconciliation_required
        );
    }
    assert_eq!(
        journal.status("session", "running").unwrap().outcome,
        Outcome::Unknown
    );
    assert!(
        journal
            .finish(&claim, json!({"malformed":true}), 106)
            .unwrap_err()
            .reconciliation_required
    );
    assert_eq!(journal.inner.uncertain.lock().unwrap().len(), 2);
    assert_eq!(
        journal
            .finish(&claim, json!({"success":true}), 107)
            .unwrap_err()
            .code,
        "operation_reconciliation_required"
    );
}

#[test]
fn nested_private_storage_is_created_and_reopened_without_touching_its_parent() {
    let directory = Directory::new();
    let storage = directory.0.join("application/mechanical/rust");
    let journal = Journal::open_at(&storage, "session", "test", Limits::default(), 100).unwrap();
    admit(&journal, "op");
    assert!(storage.join(DATABASE_NAME).exists());
    let reopened = Journal::open_at(&storage, "fresh", "test", Limits::default(), 200).unwrap();
    assert_eq!(
        reopened.status("session", "op").unwrap().state,
        State::Interrupted
    );
    assert!(!directory.0.join(DATABASE_NAME).exists());
}

#[cfg(windows)]
#[test]
fn windows_hardlinked_database_is_refused_without_modification() {
    let directory = Directory::new();
    let journal = directory.open("session");
    admit(&journal, "retained");
    let original = journal.lookup("session", "retained").unwrap();
    let bytes = fs::read(journal.database_path()).unwrap();
    let alias = directory.0.join("unaccounted-hardlink.sqlite3");

    let before = file_identity(
        journal.database_path(),
        &fs::symlink_metadata(journal.database_path()).unwrap(),
    )
    .unwrap();
    assert_eq!(before.links, 1);
    fs::hard_link(journal.database_path(), &alias).unwrap();
    // On Windows these calls exercise the actual GetFileInformationByHandle
    // implementation, independently confirming two links to the same file.
    let linked = file_identity(
        journal.database_path(),
        &fs::symlink_metadata(journal.database_path()).unwrap(),
    )
    .unwrap();
    let alias_identity = file_identity(&alias, &fs::symlink_metadata(&alias).unwrap()).unwrap();
    assert_eq!(linked.links, 2);
    assert_eq!(alias_identity.links, 2);
    assert!(linked.same_file(alias_identity));

    assert_eq!(
        journal.lookup("session", "retained").unwrap_err().code,
        "operation_storage_invalid_path",
    );
    assert_eq!(
        journal
            .admit(&request("session", "refused", 0), 102)
            .unwrap_err()
            .code,
        "operation_storage_invalid_path",
    );
    assert_eq!(fs::read(journal.database_path()).unwrap(), bytes);
    assert_eq!(fs::read(&alias).unwrap(), bytes);

    fs::remove_file(alias).unwrap();
    assert_eq!(journal.lookup("session", "retained").unwrap(), original);
    assert!(journal.lookup("session", "refused").unwrap().is_none());
}

#[test]
fn transient_sidecar_disappearing_between_metadata_and_identity_is_safe() {
    let directory = Directory::new();
    let journal = directory.open("session");
    admit(&journal, "retained");
    let original = journal.lookup("session", "retained").unwrap();
    let bytes = fs::read(journal.database_path()).unwrap();
    for suffix in ["-journal", "-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", journal.database_path().display()));
        fs::write(&sidecar, b"transient inspection fixture").unwrap();
        let mut removed = false;
        validate_files_with(&directory.0, journal.database_path(), |path, metadata| {
            if path == sidecar && !removed {
                removed = true;
                fs::remove_file(path).unwrap();
                // This is the real I/O failure the Windows identity handle open
                // encounters after a concurrent SQLite commit deletes a sidecar.
                return Err(fs::File::open(path).unwrap_err().into());
            }
            file_identity(path, metadata)
        })
        .unwrap();
        assert!(removed);
        assert!(!sidecar.exists());
        assert_eq!(fs::read(journal.database_path()).unwrap(), bytes);
    }
    assert_eq!(journal.lookup("session", "retained").unwrap(), original);
}

#[test]
fn inspection_race_exception_never_hides_missing_database_or_existing_sidecar_error() {
    let directory = Directory::new();
    let journal = directory.open("session");
    let sidecar = PathBuf::from(format!("{}-journal", journal.database_path().display()));
    fs::write(&sidecar, b"transient inspection fixture").unwrap();
    let failure = validate_files_with(&directory.0, journal.database_path(), |path, metadata| {
        if path == sidecar {
            return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied).into());
        }
        file_identity(path, metadata)
    })
    .unwrap_err();
    assert_eq!(failure.code, "operation_storage_failed");
    assert!(sidecar.exists());

    let failure = validate_files_with(&directory.0, journal.database_path(), |path, metadata| {
        if path == sidecar {
            fs::remove_file(path).unwrap();
            return Err(error(
                "operation_storage_invalid_path",
                "Identity inspection found a forbidden reparse point.",
            ));
        }
        file_identity(path, metadata)
    })
    .unwrap_err();
    assert_eq!(failure.code, "operation_storage_invalid_path");

    let failure = validate_files_with(&directory.0, journal.database_path(), |path, _metadata| {
        assert_eq!(path, journal.database_path());
        fs::remove_file(path).unwrap();
        Err(fs::File::open(path).unwrap_err().into())
    })
    .unwrap_err();
    assert_eq!(failure.code, "operation_storage_failed");
    assert!(journal.lookup("session", "retained").is_err());
    assert!(!journal.database_path().exists());
}

#[test]
fn zero_link_sidecar_is_rechecked_after_disappearance_or_regular_replacement() {
    for replace in [false, true] {
        let directory = Directory::new();
        let journal = directory.open("session");
        let sidecar = directory.0.join(format!("{DATABASE_NAME}-journal"));
        fs::write(&sidecar, b"old sidecar").unwrap();
        #[cfg(unix)]
        let retained_handle = File::open(&sidecar).unwrap();
        let mut first = true;
        let mut sidecar_inspections = 0;
        let mut old_identity = None;
        validate_files_with(&directory.0, journal.database_path(), |path, metadata| {
            let mut identity = file_identity(path, metadata)?;
            if path == sidecar {
                sidecar_inspections += 1;
                if first {
                    first = false;
                    old_identity = Some(identity);
                    fs::remove_file(path).unwrap();
                    if replace {
                        fs::write(path, b"replacement sidecar").unwrap();
                    }
                    // The real Windows failure reports this state after an
                    // opened sidecar handle outlives its directory entry.
                    identity.links = 0;
                } else {
                    assert!(!identity.same_file(old_identity.unwrap()));
                }
            }
            Ok(identity)
        })
        .unwrap();
        assert_eq!(sidecar_inspections, if replace { 2 } else { 1 });
        #[cfg(unix)]
        drop(retained_handle);
    }
}

#[test]
fn zero_link_replacement_must_pass_regular_file_and_hardlink_validation() {
    for hardlink in [false, true] {
        let directory = Directory::new();
        let journal = directory.open("session");
        let sidecar = directory.0.join(format!("{DATABASE_NAME}-journal"));
        let other = directory.0.join("unaccounted-sidecar-owner");
        fs::write(&sidecar, b"old sidecar").unwrap();
        fs::write(&other, b"other").unwrap();
        let mut replaced = false;
        let failure =
            validate_files_with(&directory.0, journal.database_path(), |path, metadata| {
                let mut identity = file_identity(path, metadata)?;
                if path == sidecar && !replaced {
                    replaced = true;
                    fs::remove_file(path).unwrap();
                    if hardlink {
                        fs::hard_link(&other, path).unwrap();
                    } else {
                        fs::create_dir(path).unwrap();
                    }
                    identity.links = 0;
                }
                Ok(identity)
            })
            .unwrap_err();
        assert_eq!(failure.code, "operation_storage_invalid_path");
    }
}

#[cfg(unix)]
#[test]
fn zero_link_replacement_symlink_or_foreign_write_permission_is_refused() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    for link in [false, true] {
        let directory = Directory::new();
        let journal = directory.open("session");
        let sidecar = directory.0.join(format!("{DATABASE_NAME}-journal"));
        let other = directory.0.join("other-sidecar");
        fs::write(&sidecar, b"old sidecar").unwrap();
        fs::write(&other, b"other").unwrap();
        let mut replaced = false;
        let failure =
            validate_files_with(&directory.0, journal.database_path(), |path, metadata| {
                let mut identity = file_identity(path, metadata)?;
                if path == sidecar && !replaced {
                    replaced = true;
                    fs::remove_file(path).unwrap();
                    if link {
                        symlink(&other, path).unwrap();
                    } else {
                        fs::write(path, b"replacement").unwrap();
                        fs::set_permissions(path, fs::Permissions::from_mode(0o666)).unwrap();
                    }
                    identity.links = 0;
                }
                Ok(identity)
            })
            .unwrap_err();
        assert_eq!(failure.code, "operation_storage_invalid_path");
    }
}

#[test]
fn zero_link_sidecar_gets_one_fresh_validation_and_main_zero_remains_forbidden() {
    let directory = Directory::new();
    let journal = directory.open("session");
    let sidecar = directory.0.join(format!("{DATABASE_NAME}-journal"));
    fs::write(&sidecar, b"sidecar").unwrap();
    let mut attempts = 0;
    validate_files_with(&directory.0, journal.database_path(), |path, metadata| {
        let mut identity = file_identity(path, metadata)?;
        if path == sidecar {
            attempts += 1;
            identity.links = 0;
        }
        Ok(identity)
    })
    .unwrap();
    assert_eq!(attempts, 2);
    let mut main_inspections = 0;
    let failure = validate_files_with(&directory.0, journal.database_path(), |path, metadata| {
        let mut identity = file_identity(path, metadata)?;
        if path == journal.database_path() {
            main_inspections += 1;
            if main_inspections == 1 {
                identity.links = 0;
            }
        }
        Ok(identity)
    })
    .unwrap_err();
    assert_eq!(failure.code, "operation_storage_invalid_path");
    assert_eq!(main_inspections, 1);
}

#[test]
fn zero_link_fresh_inspection_keeps_existing_errors_and_invalid_identity_fatal() {
    for mode in ["existing-error", "invalid-then-absent", "absent-io-error"] {
        let directory = Directory::new();
        let journal = directory.open("session");
        let sidecar = directory.0.join(format!("{DATABASE_NAME}-journal"));
        fs::write(&sidecar, b"sidecar").unwrap();
        let mut inspections = 0;
        let result =
            validate_files_with(&directory.0, journal.database_path(), |path, metadata| {
                let mut identity = file_identity(path, metadata)?;
                if path == sidecar {
                    inspections += 1;
                    if inspections == 1 {
                        identity.links = 0;
                    } else {
                        match mode {
                            "existing-error" => {
                                return Err(std::io::Error::from(
                                    std::io::ErrorKind::PermissionDenied,
                                )
                                .into())
                            }
                            "invalid-then-absent" => {
                                fs::remove_file(path).unwrap();
                                return Err(error(
                                    "operation_storage_invalid_path",
                                    "Forbidden identity observed.",
                                ));
                            }
                            "absent-io-error" => {
                                fs::remove_file(path).unwrap();
                                return Err(File::open(path).unwrap_err().into());
                            }
                            _ => unreachable!(),
                        }
                    }
                }
                Ok(identity)
            });
        assert_eq!(inspections, 2);
        match mode {
            "existing-error" => assert_eq!(result.unwrap_err().code, "operation_storage_failed"),
            "invalid-then-absent" => {
                assert_eq!(result.unwrap_err().code, "operation_storage_invalid_path")
            }
            "absent-io-error" => result.unwrap(),
            _ => unreachable!(),
        }
    }
}

#[test]
fn staged_publication_match_cannot_excuse_replaced_main_or_extra_hardlink() {
    for change in [
        "replace-main",
        "third-link",
        "complete-publication",
        "replace-directory",
        "replace-staging-with-unaccounted-alias",
    ] {
        let directory = Directory::new();
        let journal = directory.open("session");
        let staged = directory.0.join(".operations-rust-v1-regression.tmp");
        let extra = directory.0.join("extra-hardlink");
        fs::hard_link(journal.database_path(), &staged).unwrap();
        let mut changed = false;
        let result =
            validate_files_with(&directory.0, journal.database_path(), |path, metadata| {
                let identity = file_identity(path, metadata)?;
                if path == staged && !changed {
                    changed = true;
                    match change {
                        "replace-main" => {
                            fs::remove_file(journal.database_path()).unwrap();
                            fs::write(journal.database_path(), b"different file identity").unwrap();
                        }
                        "third-link" => fs::hard_link(journal.database_path(), &extra).unwrap(),
                        "complete-publication" => fs::remove_file(&staged).unwrap(),
                        "replace-directory" => {
                            fs::remove_file(journal.database_path()).unwrap();
                            fs::create_dir(journal.database_path()).unwrap();
                        }
                        "replace-staging-with-unaccounted-alias" => {
                            fs::remove_file(&staged).unwrap();
                            fs::hard_link(journal.database_path(), &extra).unwrap();
                        }
                        _ => unreachable!(),
                    }
                }
                Ok(identity)
            });
        assert!(changed);
        if change == "complete-publication" {
            result.unwrap();
        } else {
            assert_eq!(result.unwrap_err().code, "operation_storage_invalid_path");
        }
    }
}

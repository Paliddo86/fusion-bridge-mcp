//! Historical operation inspection performs no SDK access and grants no effect
//! authority. One owned initializer and one lookup at a time bound disk work.
use fusion_mechanical_journal::{
    retained::RetainedJournal, retained::RetentionLimits, Journal, Limits, Outcome,
    State as JournalState,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

enum State {
    Pending,
    Ready(Arc<RetainedJournal>),
    Failed(&'static str),
}
pub struct OperationStatusReader {
    directory: PathBuf,
    epoch: String,
    state: Mutex<State>,
    lookup: Mutex<()>,
    started: AtomicBool,
}
impl OperationStatusReader {
    /// No filesystem work here: initialization belongs to a Service worker.
    pub fn new(directory: PathBuf, epoch: String) -> Arc<Self> {
        Arc::new(Self {
            directory,
            epoch,
            state: Mutex::new(State::Pending),
            lookup: Mutex::new(()),
            started: AtomicBool::new(false),
        })
    }
    pub(crate) fn epoch(&self) -> &str {
        &self.epoch
    }
    pub(crate) fn initialize(&self) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        // This read-only CAD pilot deliberately limits history scan size. A
        // larger future release must explicitly qualify its storage policy.
        let limits = Limits {
            max_records: 64,
            max_sessions: 256,
            max_database_bytes: 8 * 1024 * 1024,
            max_record_bytes: 256 * 1024,
        };
        let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Journal::open(&self.directory, &self.epoch, "native-parameters-r7", limits)
                .and_then(|journal| RetainedJournal::new(journal, RetentionLimits::default()))
        }));
        if let Ok(mut state) = self.state.lock() {
            *state = match opened {
                Ok(Ok(journal)) => State::Ready(Arc::new(journal)),
                Ok(Err(e)) => State::Failed(e.code),
                Err(_) => State::Failed("operation_status_initialization_panicked"),
            };
        }
    }
    pub fn health(&self) -> Value {
        match self.state.lock().as_deref() {
            Ok(State::Pending) => json!({"state":"initializing","connected":false}),
            Ok(State::Ready(_)) => json!({"state":"ready","connected":true}),
            Ok(State::Failed(code)) => {
                json!({"state":"unavailable","connected":false,"errorCode":code})
            }
            Err(_) => {
                json!({"state":"unavailable","connected":false,"errorCode":"operation_status_lock_poisoned"})
            }
        }
    }
    pub(crate) fn journal(&self) -> Result<Arc<RetainedJournal>, &'static str> {
        match self.state.lock().as_deref() {
            Ok(State::Ready(journal)) => Ok(Arc::clone(journal)),
            Ok(State::Pending) => Err("operation_status_initializing"),
            _ => Err("operation_status_unavailable"),
        }
    }
    pub fn query(&self, session: &str, operation: &str, deadline: Instant) -> (u16, Value) {
        let _admission = match self.lookup.try_lock() {
            Ok(guard) => guard,
            Err(_) => return unavailable("operation_status_busy"),
        };
        let journal = match self.state.lock().as_deref() {
            Ok(State::Ready(journal)) => Arc::clone(journal),
            Ok(State::Pending) => return unavailable("operation_status_initializing"),
            _ => return unavailable("operation_status_unavailable"),
        };
        if Instant::now() >= deadline {
            return unavailable("operation_status_deadline");
        }
        let status = match journal.status(session, operation) {
            Ok(status) => status,
            Err(_) => return unavailable("operation_status_unavailable"),
        };
        // This is a response deadline, not cancellation of SQLite or OS I/O.
        // The worker stays owned and is joined before native DLL teardown.
        if Instant::now() >= deadline {
            return unavailable("operation_status_deadline");
        }
        let durable = status.durable.as_ref().map(|d| json!({
            "record":d.record,"historical":d.historical,
            "state":match d.state {JournalState::Unknown=>"unknown",JournalState::ReconciliationRequired=>"reconciliation_required",JournalState::Running=>"running",JournalState::Interrupted=>"interrupted",JournalState::Succeeded=>"succeeded",JournalState::Failed=>"failed",JournalState::NotExecuted=>"not_executed"},
            "outcome":match d.outcome {Outcome::Unknown=>"unknown",Outcome::Pending=>"pending",Outcome::Recorded=>"recorded"},
            "reconciliationRequired":d.reconciliation_required
        }));
        let storage_error = status.storage_error.as_ref().map(
            |e| json!({"errorCode":e.code,"reconciliationRequired":e.reconciliation_required}),
        );
        let retention_error = status.retention_error.as_ref().map(
            |e| json!({"errorCode":e.code,"reconciliationRequired":e.reconciliation_required}),
        );
        let storage_ok = status.storage_error.is_none();
        let mut body = json!({"success":storage_ok,"sessionId":session,"operationId":operation,
            "authority":status.authority,"durable":durable,"historical":status.historical,
            "reconciliationRequired":status.reconciliation_required,
            "observed":status.observed,"storageError":storage_error,
            "retentionError":retention_error,
            "nativeExecutionAuthorized":false,"effectJournalConnected":false});
        if !storage_ok {
            body["errorCode"] = json!("operation_status_unavailable");
        }
        match serde_json::to_vec(&body) {
            Ok(_) if Instant::now() >= deadline => unavailable("operation_status_deadline"),
            // Service appends the 32-byte transport epoch after this projection.
            Ok(bytes) if bytes.len() <= crate::http::MAX_RESPONSE_BYTES - 128 => {
                (if storage_ok { 200 } else { 503 }, body)
            }
            _ => (
                507,
                json!({"success":false,"errorCode":"operation_status_too_large","outcome":"unknown","reconciliationRequired":true,"nativeExecutionAuthorized":false}),
            ),
        }
    }
}
fn unavailable(code: &'static str) -> (u16, Value) {
    (
        503,
        json!({"success":false,"errorCode":code,"outcome":"unknown","reconciliationRequired":true,"nativeExecutionAuthorized":false}),
    )
}

#[cfg(test)]
#[path = "operation_status_tests.rs"]
mod tests;

//! A separate, bounded Rust operation journal. This never reads, migrates,
//! resets or writes the legacy Python journal. Claims are returned only after
//! FULL synchronous SQLite commit; an ambiguous commit never authorizes retry.
//!
//! A new runtime must supply a fresh session ID. Cloning a Journal shares the
//! same runtime registration and uncertainty latch. There is no automatic
//! retention or deletion: capacity exhaustion fails closed, including when
//! all existing rows are terminal. Native work is outside this crate.

use fusion_mechanical_core::{PreparedRequest, CANONICAL_FORMAT, MAX_REQUEST_BYTES};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DATABASE_NAME: &str = "operations-rust-v1.sqlite3";
pub const STORAGE_FORMAT: &str = "adom-fusion-rust-journal-v1";
pub const STORAGE_VERSION: u32 = 1;
// Different from the Python journal's ADMM ID.
const APPLICATION_ID: u32 = 0x4146524a;
const SESSIONS_SQL: &str = "CREATE TABLE sessions (session_id TEXT NOT NULL PRIMARY KEY, source_version TEXT NOT NULL, started_at_ms INTEGER NOT NULL)";
const OPERATIONS_SQL: &str = "CREATE TABLE operations (session_id TEXT NOT NULL, operation_id TEXT NOT NULL, record_json BLOB NOT NULL, record_digest TEXT NOT NULL, PRIMARY KEY(session_id,operation_id), FOREIGN KEY(session_id) REFERENCES sessions(session_id))";
const MAX_DEPTH: usize = 128;

pub mod retained;

#[cfg(any(windows, test))]
mod storage_policy;
#[cfg(windows)]
mod windows_storage;

#[derive(Debug, Clone, thiserror::Error)]
#[error("{code}: {message}")]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    /// Storage failures have unknown commit outcome. This never claims that
    /// a native effect was rolled back, even when an offline test injected it.
    pub reconciliation_required: bool,
}
impl Error {
    fn at_path(mut self, stage: &str, path: &Path) -> Self {
        self.message = format!("{stage} [{}]: {}", path.display(), self.message);
        self
    }
}
fn io_at<T>(result: std::io::Result<T>, stage: &str, path: &Path) -> Result<T> {
    result.map_err(|failure| Error::from(failure).at_path(stage, path))
}

pub type Result<T> = std::result::Result<T, Error>;

fn error(code: &'static str, message: impl Into<String>) -> Error {
    Error {
        code,
        message: message.into(),
        reconciliation_required: false,
    }
}
fn storage_error(message: impl Into<String>) -> Error {
    Error {
        code: "operation_storage_failed",
        message: message.into(),
        reconciliation_required: true,
    }
}
fn unknown() -> Error {
    Error { code: "operation_reconciliation_required", message: "A previous storage acknowledgement is uncertain; no further execution claim may be issued.".into(), reconciliation_required: true }
}
impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        storage_error(value.to_string())
    }
}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        storage_error(value.to_string())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_records: usize,
    pub max_sessions: usize,
    pub max_database_bytes: u64,
    pub max_record_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_records: 10_000,
            max_sessions: 10_000,
            max_database_bytes: 64 * 1024 * 1024,
            max_record_bytes: 4 * 1024 * 1024,
        }
    }
}
impl Limits {
    fn validate(self) -> Result<Self> {
        let max = Self::default();
        if self.max_records == 0
            || self.max_records > max.max_records
            || self.max_sessions == 0
            || self.max_sessions > max.max_sessions
            || !(32_768..=max.max_database_bytes).contains(&self.max_database_bytes)
            || !(1024..=max.max_record_bytes).contains(&self.max_record_bytes)
        {
            return Err(error(
                "operation_record_invalid",
                "Journal limits must be positive and cannot exceed the hard bounds.",
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Admitted,
    Dispatched,
    Executing,
    Succeeded,
    Failed,
    NotExecuted,
}
impl Phase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::NotExecuted)
    }
    fn permits(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Admitted, Self::Dispatched | Self::NotExecuted)
                | (Self::Dispatched, Self::Executing | Self::NotExecuted)
                | (Self::Executing, Self::Succeeded | Self::Failed)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Event {
    pub phase: Phase,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub storage_version: u32,
    pub storage_format: String,
    pub canonical_format: String,
    pub session_id: String,
    pub operation_id: String,
    pub operation: String,
    pub source_version: String,
    pub request_digest: String,
    /// Exact canonical request, retained so its digest and identity can be
    /// checked after restart rather than trusting an unchecked digest string.
    pub request_json: String,
    pub requested_target: Option<Value>,
    pub observed_document: Option<Value>,
    pub phase: Phase,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub events: Vec<Event>,
    pub dispatch_id: Option<String>,
    pub result: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct AdmissionClaim {
    session_id: String,
    operation_id: String,
    request_digest: String,
}
impl AdmissionClaim {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn request_digest(&self) -> &str {
        &self.request_digest
    }
}
#[derive(Debug, Clone)]
pub struct DispatchClaim {
    admission: AdmissionClaim,
    dispatch_id: String,
}
impl DispatchClaim {
    pub fn session_id(&self) -> &str {
        self.admission.session_id()
    }
    pub fn operation_id(&self) -> &str {
        self.admission.operation_id()
    }
    pub fn request_digest(&self) -> &str {
        self.admission.request_digest()
    }
    pub fn dispatch_id(&self) -> &str {
        &self.dispatch_id
    }
}
#[derive(Debug, Clone)]
pub enum Admission {
    Created(AdmissionClaim),
    Existing(Record),
}
#[derive(Debug, Clone)]
pub enum DispatchOutcome {
    Claimed(DispatchClaim),
    Existing(Record),
}
#[derive(Debug, Clone)]
pub struct Transition {
    pub claimed: bool,
    pub record: Record,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Unknown,
    ReconciliationRequired,
    Running,
    Interrupted,
    Succeeded,
    Failed,
    NotExecuted,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Unknown,
    Pending,
    Recorded,
}
#[derive(Debug, Clone)]
pub struct Status {
    pub record: Option<Record>,
    pub historical: bool,
    pub state: State,
    pub outcome: Outcome,
    pub reconciliation_required: bool,
}

struct Inner {
    directory: PathBuf,
    path: PathBuf,
    session_id: String,
    source_version: String,
    limits: Limits,
    // Also serializes local calls so a lost acknowledgement is latched before
    // a racing thread can attempt another transition for the same operation.
    uncertain: Mutex<HashSet<(String, String)>>,
    #[cfg(test)]
    fault: Mutex<Option<Fault>>,
}
#[derive(Clone)]
pub struct Journal {
    inner: Arc<Inner>,
}

impl Journal {
    pub fn open(
        directory: impl AsRef<Path>,
        current_session: &str,
        source_version: &str,
        limits: Limits,
    ) -> Result<Self> {
        Self::open_at(
            directory,
            current_session,
            source_version,
            limits,
            now_ms()?,
        )
    }
    /// Explicit time injection supports deterministic tests. Production callers
    /// should use `open` and `now_ms`; the clock never authorizes an effect.
    pub fn open_at(
        directory: impl AsRef<Path>,
        current_session: &str,
        source_version: &str,
        limits: Limits,
        at_ms: u64,
    ) -> Result<Self> {
        bounded_id(current_session)?;
        bounded_id(source_version)?;
        timestamp(at_ms)?;
        let limits = limits.validate()?;
        let directory = directory.as_ref().to_path_buf();
        validate_directory(&directory, true)?;
        let journal = Self {
            inner: Arc::new(Inner {
                path: directory.join(DATABASE_NAME),
                directory,
                session_id: current_session.into(),
                source_version: source_version.into(),
                limits,
                uncertain: Mutex::new(HashSet::new()),
                #[cfg(test)]
                fault: Mutex::new(None),
            }),
        };
        if !path_exists(&journal.inner.path)? {
            journal.initialize()?;
        }
        journal.write(None, false, |connection| {
            let existing: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id=?1)", [current_session], |row| row.get(0))?;
            if existing { return Err(error("session_reused", "This runtime session is already registered. Reopened runtimes require a fresh session ID; historical records remain readable.")); }
            let count: usize = connection.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
            if count >= limits.max_sessions { return Err(error("operation_capacity", "Session history is full; no history is evicted.")); }
            connection.execute("INSERT INTO sessions VALUES (?1,?2,?3)", params![current_session, source_version, at_ms])?;
            Ok(())
        })?;
        Ok(journal)
    }

    pub fn session_id(&self) -> &str {
        &self.inner.session_id
    }
    pub fn database_path(&self) -> &Path {
        &self.inner.path
    }

    pub fn admit(&self, request: &PreparedRequest, at_ms: u64) -> Result<Admission> {
        timestamp(at_ms)?;
        if !request.is_write() {
            return Err(error(
                "operation_record_invalid",
                "Read-only requests do not receive durable effect claims.",
            ));
        }
        let value = request.arguments();
        let session = value["sessionId"]
            .as_str()
            .ok_or_else(|| error("session_mismatch", "A current session is required."))?;
        self.current_session(session)?;
        let operation_id = value["operationId"]
            .as_str()
            .ok_or_else(|| error("operation_record_invalid", "Operation ID is required."))?;
        bounded_id(operation_id)?;
        if request.canonical_format() != CANONICAL_FORMAT
            || request.canonical_bytes().len() > MAX_REQUEST_BYTES
        {
            return Err(error(
                "operation_record_invalid",
                "Unsupported request canonical format or size.",
            ));
        }
        let request_digest = request_digest(request.canonical_bytes());
        let claim = AdmissionClaim {
            session_id: session.into(),
            operation_id: operation_id.into(),
            request_digest: request_digest.clone(),
        };
        self.write(Some(&claim), false, |connection| {
            if let Some(record) = self.read(connection, session, operation_id)? {
                if record.request_digest != request_digest {
                    return Err(error("operation_conflict", format!("Operation ID already identifies a different request (existing digest {}).", record.request_digest)));
                }
                return Ok(Admission::Existing(record));
            }
            let count: usize = connection.query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))?;
            if count >= self.inner.limits.max_records { return Err(error("operation_capacity", "Operation history is full; nonterminal and terminal records are retained.")); }
            let record = Record {
                storage_version: STORAGE_VERSION, storage_format: STORAGE_FORMAT.into(), canonical_format: CANONICAL_FORMAT.into(),
                session_id: session.into(), operation_id: operation_id.into(), operation: request.operation().into(), source_version: self.inner.source_version.clone(),
                request_digest, request_json: String::from_utf8(request.canonical_bytes().to_vec()).map_err(|_| error("operation_record_invalid", "Canonical request is not UTF-8."))?,
                requested_target: value.get("target").cloned(), observed_document: None, phase: Phase::Admitted,
                created_at_ms: at_ms, updated_at_ms: at_ms, events: vec![Event { phase: Phase::Admitted, at_ms }], dispatch_id: None, result: None,
            };
            self.save(connection, &record, true)?;
            Ok(Admission::Created(claim.clone()))
        })
    }

    pub fn dispatch(
        &self,
        claim: &AdmissionClaim,
        dispatch_id: &str,
        at_ms: u64,
    ) -> Result<DispatchOutcome> {
        bounded_id(dispatch_id)?;
        let outcome = self.transition(
            claim,
            Some(dispatch_id),
            Phase::Dispatched,
            None,
            None,
            at_ms,
        )?;
        Ok(if outcome.claimed {
            DispatchOutcome::Claimed(DispatchClaim {
                admission: claim.clone(),
                dispatch_id: dispatch_id.into(),
            })
        } else {
            DispatchOutcome::Existing(outcome.record)
        })
    }

    /// Only `claimed: true` authorizes the caller to start native work. A cloned
    /// dispatch token, duplicate event, or replay gets `claimed: false`.
    pub fn start(
        &self,
        claim: &DispatchClaim,
        observed_document: Value,
        at_ms: u64,
    ) -> Result<Transition> {
        if !observed_document.is_object() {
            return Err(error(
                "operation_record_invalid",
                "Observed document must be an object.",
            ));
        }
        self.transition(
            &claim.admission,
            Some(&claim.dispatch_id),
            Phase::Executing,
            Some(observed_document),
            None,
            at_ms,
        )
    }

    pub fn finish(&self, claim: &DispatchClaim, result: Value, at_ms: u64) -> Result<Transition> {
        self.current_session(claim.session_id())?;
        let success = match result.get("success").and_then(Value::as_bool) {
            Some(success) => success,
            None => {
                let mut uncertain = self
                    .inner
                    .uncertain
                    .lock()
                    .map_err(|_| storage_error("Journal state lock was poisoned."))?;
                if uncertain.len() < self.inner.limits.max_records {
                    uncertain.insert((claim.session_id().into(), claim.operation_id().into()));
                }
                let mut failure = error("operation_record_invalid", "Observed terminal result requires a Boolean success field; execution outcome needs reconciliation.");
                failure.reconciliation_required = true;
                return Err(failure);
            }
        };
        self.transition(
            &claim.admission,
            Some(&claim.dispatch_id),
            if success {
                Phase::Succeeded
            } else {
                Phase::Failed
            },
            None,
            Some(result),
            at_ms,
        )
    }

    pub fn not_executed(
        &self,
        claim: &DispatchClaim,
        result: Value,
        at_ms: u64,
    ) -> Result<Transition> {
        if result.get("nativeExecutionStarted") != Some(&Value::Bool(false)) {
            return Err(error(
                "operation_record_invalid",
                "A not-executed result must explicitly state nativeExecutionStarted=false.",
            ));
        }
        self.transition(
            &claim.admission,
            Some(&claim.dispatch_id),
            Phase::NotExecuted,
            None,
            Some(result),
            at_ms,
        )
    }

    pub fn lookup(&self, session: &str, operation: &str) -> Result<Option<Record>> {
        bounded_id(session)?;
        bounded_id(operation)?;
        let _guard = self
            .inner
            .uncertain
            .lock()
            .map_err(|_| storage_error("Journal state lock was poisoned."))?;
        let connection = self.connect()?;
        // One snapshot includes all identity, digest, and metadata validation.
        connection.execute_batch("BEGIN DEFERRED")?;
        self.validate_all(&connection)?;
        self.read(&connection, session, operation)
    }

    pub fn status(&self, session: &str, operation: &str) -> Result<Status> {
        let record = self.lookup(session, operation)?;
        let historical = session != self.session_id();
        let ledger = self
            .inner
            .uncertain
            .lock()
            .map_err(|_| storage_error("Journal state lock was poisoned."))?;
        let uncertain = ledger.len() >= self.inner.limits.max_records
            || ledger.contains(&(session.into(), operation.into()));
        let (state, outcome, reconciliation_required) =
            match record.as_ref().map(|record| record.phase) {
                None => (State::Unknown, Outcome::Unknown, true),
                Some(Phase::Succeeded) => (State::Succeeded, Outcome::Recorded, false),
                Some(Phase::Failed) => (State::Failed, Outcome::Recorded, false),
                Some(Phase::NotExecuted) => (State::NotExecuted, Outcome::Recorded, false),
                Some(_) if historical => (State::Interrupted, Outcome::Unknown, true),
                Some(Phase::Admitted) => (State::ReconciliationRequired, Outcome::Unknown, true),
                Some(_) if uncertain => (State::ReconciliationRequired, Outcome::Unknown, true),
                Some(_) => (State::Running, Outcome::Pending, false),
            };
        Ok(Status {
            record,
            historical,
            state,
            outcome,
            reconciliation_required,
        })
    }

    fn current_session(&self, session: &str) -> Result<()> {
        if session != self.session_id() {
            return Err(error("session_mismatch", "Historical sessions are readable but cannot receive new writes or execution claims."));
        }
        Ok(())
    }

    fn transition(
        &self,
        claim: &AdmissionClaim,
        dispatch: Option<&str>,
        next: Phase,
        document: Option<Value>,
        result: Option<Value>,
        at_ms: u64,
    ) -> Result<Transition> {
        self.current_session(&claim.session_id)?;
        self.write(Some(claim), matches!(next, Phase::Succeeded | Phase::Failed), |connection| {
            timestamp(at_ms)?;
            let mut record = self.read(connection, &claim.session_id, &claim.operation_id)?.ok_or_else(|| error("operation_conflict", "No durable admission identifies this claim."))?;
            if record.request_digest != claim.request_digest { return Err(error("operation_conflict", "Claim digest does not match the durable admission.")); }
            if !record.phase.permits(next) {
                if matches!(next, Phase::Succeeded | Phase::Failed) && !record.phase.is_terminal() {
                    return Err(error("operation_conflict", "Observed native result has no durable executing claim; reconciliation is required."));
                }
                return Ok(Transition { claimed: false, record });
            }
            if matches!(record.phase, Phase::Dispatched | Phase::Executing) && record.dispatch_id.as_deref() != dispatch {
                return Err(error("operation_conflict", "Dispatch generation does not match the durable record."));
            }
            if at_ms < record.updated_at_ms { return Err(error("operation_record_invalid", "Operation timestamps must not move backwards.")); }
            if next == Phase::Dispatched { record.dispatch_id = dispatch.map(str::to_owned); }
            if next == Phase::Executing { record.observed_document = document; }
            if next.is_terminal() {
                let value = result.ok_or_else(|| error("operation_record_invalid", "A terminal transition requires a result."))?;
                if value.get("success").and_then(Value::as_bool) != Some(next == Phase::Succeeded) {
                    return Err(error("operation_record_invalid", "Terminal phase contradicts its Boolean success result."));
                }
                record.result = Some(value);
            }
            record.phase = next; record.updated_at_ms = at_ms; record.events.push(Event { phase: next, at_ms });
            self.save(connection, &record, false)?;
            Ok(Transition { claimed: true, record })
        })
    }

    fn write<T>(
        &self,
        claim: Option<&AdmissionClaim>,
        effect_observed: bool,
        action: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<T> {
        let mut uncertain = self
            .inner
            .uncertain
            .lock()
            .map_err(|_| storage_error("Journal state lock was poisoned."))?;
        let key = claim.map(|claim| (claim.session_id.clone(), claim.operation_id.clone()));
        if key.as_ref().is_some_and(|key| uncertain.contains(key)) {
            return Err(unknown());
        }
        if key.is_some() && uncertain.len() >= self.inner.limits.max_records {
            return Err(Error {
                code: "operation_reconciliation_required",
                message: "The bounded uncertainty ledger is full; further claims are disabled for this runtime.".into(),
                reconciliation_required: true,
            });
        }
        let mut operation = || -> Result<T> {
            let mut connection = self.connect()?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            self.validate_all(&transaction)?;
            let result = action(&transaction)?;
            #[cfg(test)]
            self.inject(Fault::BeforeCommit)?;
            transaction.commit()?;
            #[cfg(test)]
            self.inject(Fault::AfterCommit)?;
            Ok(result)
        }();
        // Once native work has returned an observed result, *any* failure to
        // record it (including a size/validation failure) is an unknown durable
        // outcome. Never let a non-I/O error erase that uncertainty.
        if let Err(failure) = &mut operation {
            if effect_observed {
                failure.reconciliation_required = true;
            }
        }
        if operation
            .as_ref()
            .is_err_and(|error| error.reconciliation_required)
        {
            if let Some(key) = key {
                uncertain.insert(key);
            }
        }
        operation
    }

    fn connect(&self) -> Result<Connection> {
        validate_directory(&self.inner.directory, false)?;
        validate_files(&self.inner.directory, &self.inner.path)?;
        if !path_exists(&self.inner.path)? {
            return Err(storage_error(
                "Registered operation journal is missing; it will not be recreated.",
            ));
        }
        if io_at(
            fs::metadata(&self.inner.path),
            "connect.database_metadata",
            &self.inner.path,
        )?
        .len()
            > self.inner.limits.max_database_bytes
        {
            return Err(error(
                "operation_storage_full",
                "Operation database exceeds its byte limit.",
            ));
        }
        // SQLite may recover a hot rollback journal as soon as a read starts.
        // Refuse foreign/unsupported files from their read-only header before
        // giving SQLite a writable handle, so refusal cannot alter foreign data.
        validate_header(&self.inner.path)?;
        let connection = Connection::open_with_flags(
            &self.inner.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|failure| Error::from(failure).at_path("connect.sqlite_open", &self.inner.path))?;
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.execute_batch(
            "PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;",
        )?;
        let app: u32 = connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if app != APPLICATION_ID {
            return Err(error(
                "operation_storage_corrupt",
                "This is not a Rust operation journal; it will not be reset.",
            ));
        }
        if version != STORAGE_VERSION {
            return Err(error(
                "operation_storage_version",
                "Unsupported Rust journal version; automatic migration is disabled.",
            ));
        }
        let mode: String = connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        if mode != "delete" {
            return Err(error(
                "operation_storage_corrupt",
                "Journal mode differs from the required rollback journal.",
            ));
        }
        let synchronous: u32 =
            connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
        if synchronous != 2 {
            return Err(storage_error(
                "FULL synchronous mode could not be established.",
            ));
        }
        let page_size: u64 = connection.pragma_query_value(None, "page_size", |row| row.get(0))?;
        let pages: u64 = connection.pragma_query_value(None, "page_count", |row| row.get(0))?;
        let maximum = self.inner.limits.max_database_bytes / page_size;
        if pages > maximum {
            return Err(error(
                "operation_storage_full",
                "Database page count exceeds the byte bound.",
            ));
        }
        connection.pragma_update(None, "max_page_count", maximum)?;
        Ok(connection)
    }

    fn validate_all(&self, connection: &Connection) -> Result<()> {
        let integrity: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(error(
                "operation_storage_corrupt",
                "SQLite integrity check failed.",
            ));
        }
        let mut schema = connection
            .prepare("SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")?;
        let rows = schema
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if rows
            != vec![
                ("operations".into(), OPERATIONS_SQL.into()),
                ("sessions".into(), SESSIONS_SQL.into()),
            ]
        {
            return Err(error(
                "operation_storage_corrupt",
                "Journal schema contains missing, altered, or unexpected objects.",
            ));
        }
        let malformed: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE typeof(session_id)!='text' OR length(session_id) NOT BETWEEN 1 AND 128 OR typeof(source_version)!='text' OR length(source_version) NOT BETWEEN 1 AND 128 OR typeof(started_at_ms)!='integer' OR started_at_ms<0)", [], |row| row.get(0))?;
        if malformed {
            return Err(error(
                "operation_storage_corrupt",
                "Session row column types or identity bounds are invalid.",
            ));
        }
        let mut sessions = connection
            .prepare("SELECT session_id,source_version,started_at_ms FROM sessions LIMIT ?1")?;
        let mut cursor = sessions.query([self.inner.limits.max_sessions + 1])?;
        let mut session_count = 0;
        while let Some(row) = cursor.next()? {
            session_count += 1;
            let session: String = row.get(0)?;
            let source: String = row.get(1)?;
            let at: u64 = row.get(2)?;
            bounded_id(&session).map_err(corrupt)?;
            bounded_id(&source).map_err(corrupt)?;
            timestamp(at).map_err(corrupt)?;
        }
        if session_count > self.inner.limits.max_sessions {
            return Err(error(
                "operation_storage_full",
                "Session history exceeds its record bound.",
            ));
        }
        let foreign_key_violation: Option<String> = connection
            .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
            .optional()?;
        if foreign_key_violation.is_some() {
            return Err(error(
                "operation_storage_corrupt",
                "Operation history references an unregistered session.",
            ));
        }
        let malformed: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE typeof(session_id)!='text' OR length(session_id) NOT BETWEEN 1 AND 128 OR typeof(operation_id)!='text' OR length(operation_id) NOT BETWEEN 1 AND 128 OR typeof(record_digest)!='text' OR length(record_digest)!=64 OR typeof(record_json)!='blob')", [], |row| row.get(0))?;
        if malformed {
            return Err(error(
                "operation_storage_corrupt",
                "Operation row column types or identity bounds are invalid.",
            ));
        }
        let mut records = connection.prepare("SELECT operations.session_id,operation_id,length(record_json),substr(record_json,1,?1),record_digest,sessions.source_version FROM operations JOIN sessions ON operations.session_id=sessions.session_id LIMIT ?2")?;
        let mut cursor = records.query(params![
            self.inner.limits.max_record_bytes + 1,
            self.inner.limits.max_records + 1
        ])?;
        let mut count = 0;
        while let Some(row) = cursor.next()? {
            count += 1;
            let session: String = row.get(0)?;
            let operation: String = row.get(1)?;
            let length: usize = row.get(2)?;
            let bytes: Vec<u8> = row.get(3)?;
            let digest: String = row.get(4)?;
            let record = self.decode(&session, &operation, length, &bytes, &digest)?;
            let source: String = row.get(5)?;
            if record.source_version != source {
                return Err(error(
                    "operation_storage_corrupt",
                    "Operation source version differs from its registered session.",
                ));
            }
        }
        if count > self.inner.limits.max_records {
            return Err(error(
                "operation_storage_full",
                "Operation history exceeds its record bound.",
            ));
        }
        Ok(())
    }

    fn read(
        &self,
        connection: &Connection,
        session: &str,
        operation: &str,
    ) -> Result<Option<Record>> {
        let raw = connection.query_row("SELECT length(record_json),substr(record_json,1,?3),record_digest FROM operations WHERE session_id=?1 AND operation_id=?2",
            params![session, operation, self.inner.limits.max_record_bytes + 1], |row| Ok((row.get::<_, usize>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, String>(2)?))).optional()?;
        raw.map(|(length, bytes, digest)| self.decode(session, operation, length, &bytes, &digest))
            .transpose()
    }

    fn decode(
        &self,
        session: &str,
        operation: &str,
        length: usize,
        bytes: &[u8],
        digest: &str,
    ) -> Result<Record> {
        if length > self.inner.limits.max_record_bytes
            || length != bytes.len()
            || record_digest(bytes) != digest
        {
            return Err(error(
                "operation_storage_corrupt",
                "Record size or digest is invalid.",
            ));
        }
        let record: Record = serde_json::from_slice(bytes)
            .map_err(|_| error("operation_storage_corrupt", "Malformed operation record."))?;
        validate_record(&record).map_err(corrupt)?;
        if record.session_id != session
            || record.operation_id != operation
            || encode(&record, self.inner.limits.max_record_bytes).map_err(corrupt)? != bytes
        {
            return Err(error(
                "operation_storage_corrupt",
                "Record identity or canonical encoding is invalid.",
            ));
        }
        Ok(record)
    }

    fn save(&self, connection: &Connection, record: &Record, insert: bool) -> Result<()> {
        validate_record(record)?;
        let bytes = encode(record, self.inner.limits.max_record_bytes)?;
        let digest = record_digest(&bytes);
        let changed = if insert {
            connection.execute(
                "INSERT INTO operations VALUES (?1,?2,?3,?4)",
                params![record.session_id, record.operation_id, bytes, digest],
            )?
        } else {
            connection.execute("UPDATE operations SET record_json=?3,record_digest=?4 WHERE session_id=?1 AND operation_id=?2", params![record.session_id, record.operation_id, bytes, digest])?
        };
        if changed != 1 {
            return Err(storage_error(
                "Operation write did not affect exactly one expected row.",
            ));
        }
        Ok(())
    }

    fn initialize(&self) -> Result<()> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let pending = io_at(
            fs::read_dir(&self.inner.directory),
            "initialize.scan_directory",
            &self.inner.directory,
        )?
        .filter_map(std::result::Result::ok)
        .filter(|item| {
            item.file_name()
                .to_string_lossy()
                .starts_with(".operations-rust-v1-")
        })
        .count();
        if pending >= 32 {
            return Err(error(
                "operation_storage_unavailable",
                "Too many retained initialization files; no automatic cleanup is attempted.",
            ));
        }
        let name = format!(
            ".operations-rust-v1-{}-{}-{}.tmp",
            std::process::id(),
            now_ms()?,
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let staged = self.inner.directory.join(name);
        validate_directory(&self.inner.directory, false)?;
        #[cfg(not(windows))]
        let mut options = OpenOptions::new();
        #[cfg(not(windows))]
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(not(windows))]
        let file = io_at(
            options.open(&staged),
            "initialize.create_staged_file",
            &staged,
        )?;
        #[cfg(windows)]
        let file = windows_storage::create_file(&staged)
            .map_err(|failure| failure.at_path("initialize.create_private_staged_file", &staged))?;
        drop(file);
        validate_files(&self.inner.directory, &staged)?;
        // On initialization failure retain this bounded staging artifact. Never
        // replace a preexisting public database, even an empty/corrupt one.
        let connection = Connection::open_with_flags(
            &staged,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|failure| Error::from(failure).at_path("initialize.sqlite_open", &staged))?;
        connection
            .execute_batch("PRAGMA synchronous=FULL; PRAGMA journal_mode=DELETE; BEGIN IMMEDIATE;")
            .map_err(|failure| Error::from(failure).at_path("initialize.begin", &staged))?;
        connection.execute_batch(SESSIONS_SQL).map_err(|failure| {
            Error::from(failure).at_path("initialize.sessions_schema", &staged)
        })?;
        connection
            .execute_batch(OPERATIONS_SQL)
            .map_err(|failure| {
                Error::from(failure).at_path("initialize.operations_schema", &staged)
            })?;
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .map_err(|failure| {
                Error::from(failure).at_path("initialize.application_id", &staged)
            })?;
        connection
            .pragma_update(None, "user_version", STORAGE_VERSION)
            .map_err(|failure| Error::from(failure).at_path("initialize.user_version", &staged))?;
        connection
            .execute_batch("COMMIT")
            .map_err(|failure| Error::from(failure).at_path("initialize.commit", &staged))?;
        connection
            .close()
            .map_err(|(_, error)| Error::from(error).at_path("initialize.sqlite_close", &staged))?;
        let staged_file = io_at(
            OpenOptions::new().read(true).write(true).open(&staged),
            "initialize.open_for_flush",
            &staged,
        )?;
        io_at(
            staged_file.sync_all(),
            "initialize.flush_staged_file",
            &staged,
        )?;
        drop(staged_file);
        match fs::hard_link(&staged, &self.inner.path) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => {
                return Err(Error::from(error).at_path(
                    &format!(
                        "initialize.publish_hardlink_to {}",
                        self.inner.path.display()
                    ),
                    &staged,
                ))
            }
        }
        io_at(
            fs::remove_file(&staged),
            "initialize.remove_staged_link",
            &staged,
        )?;
        #[cfg(unix)]
        io_at(
            io_at(
                File::open(&self.inner.directory),
                "initialize.open_directory_for_flush",
                &self.inner.directory,
            )?
            .sync_all(),
            "initialize.flush_directory",
            &self.inner.directory,
        )?;
        Ok(())
    }
}

fn corrupt(error_value: Error) -> Error {
    error("operation_storage_corrupt", error_value.message)
}

pub fn now_ms() -> Result<u64> {
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            error(
                "operation_record_invalid",
                "System clock predates Unix epoch.",
            )
        })?
        .as_millis();
    u64::try_from(value).map_err(|_| {
        error(
            "operation_record_invalid",
            "System clock exceeds timestamp limits.",
        )
    })
}
fn timestamp(value: u64) -> Result<()> {
    if value > i64::MAX as u64 {
        return Err(error(
            "operation_record_invalid",
            "Timestamp exceeds its storage bound.",
        ));
    }
    Ok(())
}
fn bounded_id(value: &str) -> Result<()> {
    if value.is_empty() || value.chars().count() > 128 || value.chars().any(char::is_control) {
        return Err(error(
            "operation_record_invalid",
            "Identity and source strings must contain 1–128 non-control characters.",
        ));
    }
    Ok(())
}
fn digest(namespace: &str, bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(namespace.as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
pub fn request_digest(bytes: &[u8]) -> String {
    digest(CANONICAL_FORMAT, bytes)
}
fn record_digest(bytes: &[u8]) -> String {
    digest(STORAGE_FORMAT, bytes)
}

fn sorted(value: Value, depth: usize) -> Result<Value> {
    if depth > MAX_DEPTH {
        return Err(error(
            "operation_record_invalid",
            "Record nesting exceeds its bound.",
        ));
    }
    Ok(match value {
        Value::Object(object) => {
            let mut entries: Vec<_> = object.into_iter().collect();
            entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            let mut output = serde_json::Map::new();
            for (key, value) in entries {
                output.insert(key, sorted(value, depth + 1)?);
            }
            Value::Object(output)
        }
        Value::Array(array) => Value::Array(
            array
                .into_iter()
                .map(|value| sorted(value, depth + 1))
                .collect::<Result<_>>()?,
        ),
        value => value,
    })
}
fn encode(value: &impl Serialize, maximum: usize) -> Result<Vec<u8>> {
    let value = serde_json::to_value(value).map_err(|_| {
        error(
            "operation_record_invalid",
            "Record is not serializable JSON.",
        )
    })?;
    let value = sorted(value, 0)?;
    struct Bounded {
        bytes: Vec<u8>,
        maximum: usize,
    }
    impl std::io::Write for Bounded {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            if data.len() > self.maximum - self.bytes.len() {
                return Err(std::io::Error::other("Record byte limit exceeded"));
            }
            self.bytes.extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Bounded {
        bytes: Vec::new(),
        maximum,
    };
    serde_json::to_writer(&mut output, &value).map_err(|_| {
        error(
            "operation_record_too_large",
            "Record exceeds its durable byte bound.",
        )
    })?;
    Ok(output.bytes)
}

fn validate_record(record: &Record) -> Result<()> {
    let invalid = || {
        error(
            "operation_record_invalid",
            "Operation record identity or state history is invalid.",
        )
    };
    if record.storage_version != STORAGE_VERSION
        || record.storage_format != STORAGE_FORMAT
        || record.canonical_format != CANONICAL_FORMAT
    {
        return Err(invalid());
    }
    for value in [
        &record.session_id,
        &record.operation_id,
        &record.operation,
        &record.source_version,
    ] {
        bounded_id(value)?;
    }
    timestamp(record.created_at_ms)?;
    timestamp(record.updated_at_ms)?;
    if record.request_json.len() > MAX_REQUEST_BYTES
        || request_digest(record.request_json.as_bytes()) != record.request_digest
    {
        return Err(invalid());
    }
    let request: Value = serde_json::from_str(&record.request_json).map_err(|_| invalid())?;
    if encode(&request, MAX_REQUEST_BYTES)? != record.request_json.as_bytes()
        || request.get("sessionId").and_then(Value::as_str) != Some(&record.session_id)
        || request.get("operationId").and_then(Value::as_str) != Some(&record.operation_id)
        || request.get("op").and_then(Value::as_str) != Some(&record.operation)
        || request.get("target") != record.requested_target.as_ref()
        || record.events.is_empty()
        || record.events.len() > 4
        || record.events[0]
            != (Event {
                phase: Phase::Admitted,
                at_ms: record.created_at_ms,
            })
        || record.events.last()
            != Some(&Event {
                phase: record.phase,
                at_ms: record.updated_at_ms,
            })
    {
        return Err(invalid());
    }
    for pair in record.events.windows(2) {
        if !pair[0].phase.permits(pair[1].phase) || pair[0].at_ms > pair[1].at_ms {
            return Err(invalid());
        }
    }
    let dispatched = record
        .events
        .iter()
        .any(|event| event.phase == Phase::Dispatched);
    let executed = record
        .events
        .iter()
        .any(|event| event.phase == Phase::Executing);
    if dispatched != record.dispatch_id.is_some()
        || executed != record.observed_document.is_some()
        || record
            .observed_document
            .as_ref()
            .is_some_and(|value| !value.is_object())
    {
        return Err(invalid());
    }
    if let Some(dispatch) = &record.dispatch_id {
        bounded_id(dispatch)?;
    }
    // Check trusted in-process Values before serde clones/serializes them. An
    // oversized or deeply nested native response must not bypass record bounds.
    for value in [
        record.requested_target.as_ref(),
        record.observed_document.as_ref(),
        record.result.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        bounded_value(value, 0, &mut (4 * 1024 * 1024))?;
    }
    if record.phase.is_terminal() {
        let result = record.result.as_ref().ok_or_else(invalid)?;
        if !result.is_object()
            || result.get("success").and_then(Value::as_bool)
                != Some(record.phase == Phase::Succeeded)
            || (record.phase == Phase::NotExecuted
                && result.get("nativeExecutionStarted") != Some(&Value::Bool(false)))
        {
            return Err(invalid());
        }
    } else if record.result.is_some() {
        return Err(invalid());
    }
    Ok(())
}

fn bounded_value(value: &Value, depth: usize, remaining: &mut usize) -> Result<()> {
    if depth >= MAX_DEPTH - 2 {
        return Err(error(
            "operation_record_invalid",
            "Native value nesting exceeds its journal bound.",
        ));
    }
    let mut debit = |count: usize| -> Result<()> {
        *remaining = remaining.checked_sub(count).ok_or_else(|| {
            error(
                "operation_record_too_large",
                "Native value exceeds its journal bound.",
            )
        })?;
        Ok(())
    };
    match value {
        Value::String(text) => debit(text.len() + 2)?,
        Value::Object(map) => {
            debit(2)?;
            for (key, item) in map {
                *remaining = remaining.checked_sub(key.len() + 4).ok_or_else(|| {
                    error(
                        "operation_record_too_large",
                        "Native value exceeds its journal bound.",
                    )
                })?;
                bounded_value(item, depth + 1, remaining)?;
            }
        }
        Value::Array(items) => {
            debit(2)?;
            for item in items {
                bounded_value(item, depth + 1, remaining)?;
            }
        }
        _ => debit(1)?,
    }
    Ok(())
}

fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Error::from(error).at_path("path_exists.metadata", path)),
    }
}

fn validate_header(path: &Path) -> Result<()> {
    let mut header = [0_u8; 100];
    match io_at(File::open(path), "header.open", path)?.read_exact(&mut header) {
        Ok(()) => (),
        Err(failure) if failure.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(error(
                "operation_storage_corrupt",
                "Journal file is empty or truncated; it will not be initialized or reset.",
            ));
        }
        Err(failure) => return Err(Error::from(failure).at_path("header.read", path)),
    }
    if &header[..16] != b"SQLite format 3\0"
        || u32::from_be_bytes(header[68..72].try_into().expect("fixed header slice"))
            != APPLICATION_ID
    {
        return Err(error(
            "operation_storage_corrupt",
            "This file is not a Rust operation journal; SQLite recovery is not authorized.",
        ));
    }
    if u32::from_be_bytes(header[60..64].try_into().expect("fixed header slice")) != STORAGE_VERSION
    {
        return Err(error(
            "operation_storage_version",
            "Unsupported Rust journal version; recovery or migration is disabled.",
        ));
    }
    if header[18] != 1 || header[19] != 1 {
        return Err(error(
            "operation_storage_corrupt",
            "Rust journal header does not use the required rollback-journal format.",
        ));
    }
    Ok(())
}
fn is_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}
#[cfg(unix)]
fn effective_uid() -> u32 {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // POSIX uid_t is u32 on the supported Linux/macOS hosts.
    unsafe { geteuid() }
}
fn validate_directory(directory: &Path, create: bool) -> Result<()> {
    let invalid = || {
        error("operation_storage_invalid_path", "Storage requires an absolute private directory without symlinks, junctions or parent traversal.")
    };
    if !directory.is_absolute()
        || directory
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid());
    }
    #[cfg(windows)]
    windows_storage::validate_path(directory)?;
    for ancestor in directory.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if is_link(&metadata) || !metadata.is_dir() => return Err(invalid()),
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => (),
            Err(error) => {
                return Err(Error::from(error).at_path("directory.ancestor_metadata", ancestor))
            }
        }
    }
    if create && !path_exists(directory)? {
        #[cfg(unix)]
        let mut created_directories = Vec::new();
        #[cfg(unix)]
        for ancestor in directory.ancestors() {
            if path_exists(ancestor)? {
                break;
            }
            created_directories.push(ancestor.to_path_buf());
        }
        #[cfg(not(windows))]
        let mut builder = fs::DirBuilder::new();
        #[cfg(not(windows))]
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        #[cfg(not(windows))]
        io_at(builder.create(directory), "directory.create", directory)?;
        #[cfg(windows)]
        windows_storage::create_directories(directory)?;
        // Revalidate after directory creation, including ancestors.
        validate_directory(directory, false)?;
        // A synced database inside a newly created but unsynced directory is
        // insufficient: persist each new directory entry in its parent too.
        #[cfg(unix)]
        for created in created_directories.iter().rev() {
            io_at(
                io_at(
                    File::open(created),
                    "directory.open_created_for_flush",
                    created,
                )?
                .sync_all(),
                "directory.flush_created",
                created,
            )?;
            if let Some(parent) = created.parent() {
                io_at(
                    io_at(
                        File::open(parent),
                        "directory.open_parent_for_flush",
                        parent,
                    )?
                    .sync_all(),
                    "directory.flush_parent",
                    parent,
                )?;
            }
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = io_at(
            fs::metadata(directory),
            "directory.owner_metadata",
            directory,
        )?;
        if metadata.mode() & 0o022 != 0 || metadata.uid() != effective_uid() {
            return Err(invalid());
        }
    }
    #[cfg(windows)]
    windows_storage::validate_directory(directory)?;
    Ok(())
}

fn validate_files(directory: &Path, path: &Path) -> Result<()> {
    validate_files_with(directory, path, file_identity)
}

fn validate_files_with(
    directory: &Path,
    path: &Path,
    mut inspect_identity: impl FnMut(&Path, &fs::Metadata) -> Result<FileIdentity>,
) -> Result<()> {
    'paths: for suffix in ["", "-journal", "-wal", "-shm"] {
        let file = PathBuf::from(format!("{}{}", path.display(), suffix));
        let metadata = match fs::symlink_metadata(&file) {
            Ok(metadata) => metadata,
            Err(error) if !suffix.is_empty() && error.kind() == std::io::ErrorKind::NotFound => {
                continue 'paths
            }
            Err(error) => {
                return Err(Error::from(error).at_path("validate_files.initial_metadata", &file))
            }
        };
        validate_file_metadata(&metadata, &file)?;
        let identity = match inspect_identity(&file, &metadata) {
            Ok(identity) => identity,
            // SQLite removes its rollback journal when another writer commits.
            // On Windows identity inspection opens a handle after the metadata
            // read, so that ordinary disappearance can occur in this gap. It is
            // acceptable only for an absent transient sidecar after an I/O
            // inspection error. Main-file absence, known invalid identities,
            // and failures on files that still exist remain hard refusals.
            Err(failure)
                if !suffix.is_empty()
                    && failure.code == "operation_storage_failed"
                    && !path_exists(&file)? =>
            {
                continue 'paths
            }
            Err(failure) => return Err(failure.at_path("validate_files.initial_identity", &file)),
        };
        if suffix.is_empty() && identity.links == 0 {
            return Err(error("operation_storage_invalid_path", "The main database identity is unlinked; it cannot be revalidated as a transient sidecar.")
                    .at_path("validate_files.unlinked_main", &file));
        }
        if !suffix.is_empty() && identity.links == 0 {
            // A concurrently unlinked sidecar is non-aliased, not corrupt
            // durable storage. Validate the pathname once more in case a new
            // generation appeared. A freshly validated zero-link transient
            // is safe too: legitimate journal deletion need not stabilize.
            // The durable main database never receives this exception.
            let fresh_metadata = match fs::symlink_metadata(&file) {
                Ok(metadata) => metadata,
                Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => continue 'paths,
                Err(failure) => {
                    return Err(Error::from(failure)
                        .at_path("validate_files.sidecar_fresh_metadata", &file))
                }
            };
            validate_file_metadata(&fresh_metadata, &file)?;
            let fresh_identity = match inspect_identity(&file, &fresh_metadata) {
                Ok(identity) => identity,
                Err(failure)
                    if failure.code == "operation_storage_failed" && !path_exists(&file)? =>
                {
                    continue 'paths
                }
                Err(failure) => {
                    return Err(failure.at_path("validate_files.sidecar_fresh_identity", &file))
                }
            };
            if fresh_identity.links > 1 {
                return Err(error(
                    "operation_storage_invalid_path",
                    format!(
                        "Fresh sidecar hardlinks are not permitted; identity={fresh_identity:?}."
                    ),
                )
                .at_path("validate_files.sidecar_fresh_hardlink_refusal", &file));
            }
            continue 'paths;
        }
        if !suffix.is_empty() && identity.links != 1 {
            return Err(error(
                "operation_storage_invalid_path",
                format!("Sidecar hardlinks are not permitted; identity={identity:?}."),
            )
            .at_path("validate_files.sidecar_hardlink_refusal", &file));
        }
        if identity.links != 1 {
            // A concurrent initializer may briefly publish its fully synced
            // file with two links. Account for exactly its own staging link.
            let mut matches = 0;
            let mut matched_staging_path = None;
            if suffix.is_empty() && identity.links == 2 {
                for entry in io_at(
                    fs::read_dir(directory),
                    "validate_files.scan_staging_links",
                    directory,
                )? {
                    let entry = io_at(entry, "validate_files.read_staging_entry", directory)?;
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if !name.starts_with(".operations-rust-v1-") || !name.ends_with(".tmp") {
                        continue;
                    }
                    let other = match fs::symlink_metadata(entry.path()) {
                        Ok(other) => other,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(error) => {
                            return Err(Error::from(error)
                                .at_path("validate_files.staging_metadata", &entry.path()))
                        }
                    };
                    if !other.is_file() || is_link(&other) {
                        continue;
                    }
                    match inspect_identity(&entry.path(), &other) {
                        Ok(other) if other.same_file(identity) => {
                            matches += 1;
                            matched_staging_path = Some(entry.path());
                        }
                        Ok(_) => (),
                        Err(_) if !path_exists(&entry.path())? => (),
                        Err(error) => {
                            return Err(
                                error.at_path("validate_files.staging_identity", &entry.path())
                            )
                        }
                    }
                }
            }
            let fresh_metadata = io_at(
                fs::symlink_metadata(&file),
                &format!(
                    "validate_files.fresh_metadata_after_identity_links_{}",
                    identity.links
                ),
                &file,
            )?;
            validate_file_metadata(&fresh_metadata, &file)?;
            let fresh = inspect_identity(&file, &fresh_metadata).map_err(|failure| {
                failure.at_path(
                    &format!(
                        "validate_files.fresh_identity_after_links_{}",
                        identity.links
                    ),
                    &file,
                )
            })?;
            if !fresh.same_file(identity)
                || !(fresh.links == 1 || (identity.links == 2 && fresh.links == 2 && matches == 1))
            {
                return Err(error(
                    "operation_storage_invalid_path",
                    format!("Journal hardlinks are not permitted; suffix={suffix:?}, initial={identity:?}, fresh={fresh:?}, matching_staging_links={matches}."),
                ).at_path("validate_files.hardlink_refusal", &file));
            }
            if fresh.links == 2 {
                // A prior directory scan is not current alias proof. The
                // publisher may have removed that name while a different
                // hardlink appeared. Verify the exact matched path again.
                let staged = matched_staging_path
                    .as_ref()
                    .expect("two links require one matched staging path");
                let staged_metadata = match fs::symlink_metadata(staged) {
                    Ok(metadata) => Some(metadata),
                    Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => None,
                    Err(failure) => {
                        return Err(Error::from(failure)
                            .at_path("validate_files.recheck_staging_metadata", staged))
                    }
                };
                let mut alias_still_verified = false;
                if let Some(metadata) = staged_metadata {
                    validate_file_metadata(&metadata, staged)?;
                    alias_still_verified = match inspect_identity(staged, &metadata) {
                        Ok(alias) => alias.same_file(identity) && alias.links == 2,
                        Err(failure)
                            if failure.code == "operation_storage_failed"
                                && !path_exists(staged)? =>
                        {
                            false
                        }
                        Err(failure) => {
                            return Err(
                                failure.at_path("validate_files.recheck_staging_identity", staged)
                            )
                        }
                    };
                }
                let final_metadata = io_at(
                    fs::symlink_metadata(&file),
                    "validate_files.final_main_metadata",
                    &file,
                )?;
                validate_file_metadata(&final_metadata, &file)?;
                let final_identity =
                    inspect_identity(&file, &final_metadata).map_err(|failure| {
                        failure.at_path("validate_files.final_main_identity", &file)
                    })?;
                if !final_identity.same_file(identity)
                    || !(final_identity.links == 1
                        || (final_identity.links == 2 && alias_still_verified))
                {
                    return Err(error("operation_storage_invalid_path", format!("Staging alias no longer accounts for the main database's hardlinks; alias_verified={alias_still_verified}, initial={identity:?}, final={final_identity:?}."))
                            .at_path("validate_files.stale_staging_alias", &file));
                }
            }
        }
        continue 'paths;
    }
    Ok(())
}

fn validate_file_metadata(metadata: &fs::Metadata, path: &Path) -> Result<()> {
    if is_link(metadata) || !metadata.is_file() {
        return Err(error(
            "operation_storage_invalid_path",
            "Journal files must be regular files without links.",
        )
        .at_path("validate_files.regular_metadata", path));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o022 != 0 || metadata.uid() != effective_uid() {
            return Err(error(
                "operation_storage_invalid_path",
                "Journal files must be owned by the current user and cannot be writable by others.",
            )
            .at_path("validate_files.owner_metadata", path));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct FileIdentity {
    volume: u64,
    index: u64,
    links: u64,
}
impl FileIdentity {
    fn same_file(self, other: Self) -> bool {
        self.volume == other.volume && self.index == other.index
    }
}
#[cfg(unix)]
fn file_identity(_path: &Path, metadata: &fs::Metadata) -> Result<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    Ok(FileIdentity {
        volume: metadata.dev(),
        index: metadata.ino(),
        links: metadata.nlink(),
    })
}
#[cfg(windows)]
fn file_identity(path: &Path, _metadata: &fs::Metadata) -> Result<FileIdentity> {
    windows_storage::identity(path)
}
#[cfg(not(any(unix, windows)))]
fn file_identity(_path: &Path, _metadata: &fs::Metadata) -> Result<FileIdentity> {
    Err(error(
        "operation_storage_unavailable",
        "Storage identity validation is unavailable on this platform.",
    ))
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    BeforeCommit,
    AfterCommit,
}
#[cfg(test)]
impl Journal {
    fn inject(&self, point: Fault) -> Result<()> {
        let mut fault = self.inner.fault.lock().unwrap();
        if *fault == Some(point) {
            *fault = None;
            return Err(storage_error("Injected commit acknowledgement loss."));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

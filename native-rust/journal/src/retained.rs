//! Bounded, process-local observations around durable journal claims.
//!
//! Use one wrapper for the lifetime of a registered Journal. Only a returned
//! ExecutionPermit authorizes a first effect; status and observations never do.
//! Native code runs between start and finish, with no retention lock held.

use crate::{error, AdmissionClaim, DispatchClaim, Error, Journal, Result};
use fusion_mechanical_core::PreparedRequest;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock, Weak};

const MAX_ENTRIES: usize = 256;
const MAX_RESULT_BYTES: usize = 1024 * 1024;
const MAX_RESERVED_BYTES: usize = 16 * 1024 * 1024;
const MAX_REGISTRATIONS: usize = 256;
const MAX_ERROR_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy)]
pub struct RetentionLimits {
    pub max_entries: usize,
    pub max_result_bytes: usize,
    pub max_reserved_bytes: usize,
}
impl Default for RetentionLimits {
    fn default() -> Self {
        Self {
            max_entries: 64,
            max_result_bytes: 256 * 1024,
            max_reserved_bytes: MAX_RESERVED_BYTES,
        }
    }
}
impl RetentionLimits {
    fn validate(self) -> Result<Self> {
        if !(1..=MAX_ENTRIES).contains(&self.max_entries)
            || !(1..=MAX_RESULT_BYTES).contains(&self.max_result_bytes)
            || !(self.max_result_bytes..=MAX_RESERVED_BYTES).contains(&self.max_reserved_bytes)
        {
            return Err(error(
                "operation_retention_invalid",
                "Retention limits exceed hard bounds or cannot reserve one result.",
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Authority {
    DurableTerminal,
    ObservedNondurable,
    Pending,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedOutcome {
    Succeeded,
    Failed,
    NotExecuted,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub session_id: String,
    pub operation_id: String,
    pub request_digest: String,
    pub dispatch_id: String,
    pub outcome: ObservedOutcome,
    /// Exact observed JSON, or None when the payload exceeded the bound.
    pub result: Option<Value>,
    pub payload_omitted: bool,
}

/// Errors retain bounded diagnostics for trusted callers. HTTP adapters must
/// project error codes only: journal messages can contain storage paths.
#[derive(Debug, Clone)]
pub struct Status {
    pub durable: Option<crate::Status>,
    pub observed: Option<Observation>,
    pub storage_error: Option<Error>,
    pub retention_error: Option<Error>,
    pub authority: Authority,
    pub historical: bool,
    pub reconciliation_required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Persistence {
    Committed,
    Unconfirmed,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub observation: Observation,
    pub persistence: Persistence,
    pub persistence_error: Option<Error>,
}

#[derive(Debug)]
pub enum Admission {
    Created(AdmissionToken),
    /// Evidence only. This variant never authorizes another dispatch/effect.
    Existing(Status),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Admitting,
    Admitted,
    Dispatching,
    Dispatched,
    Starting,
    Executing,
    Observed,
    Existing,
    Unknown,
}

#[derive(Clone)]
struct StoredObservation {
    dispatch_id: String,
    outcome: ObservedOutcome,
    // Exact JSON bytes, not a potentially disproportionate heap Value graph.
    bytes: Option<Arc<Vec<u8>>>,
}
#[derive(Clone)]
struct Entry {
    digest: String,
    stage: Stage,
    dispatch_id: Option<String>,
    observed: Option<StoredObservation>,
    failure: Option<Error>,
}
struct State {
    entries: BTreeMap<String, Entry>,
    reserved_bytes: usize,
}
struct Inner {
    journal: Journal,
    limits: RetentionLimits,
    state: Mutex<State>,
}

#[derive(Clone)]
pub struct RetainedJournal {
    inner: Arc<Inner>,
}

/// Non-cloneable ownership of a reserved operation's next lifecycle step.
struct Lease<C> {
    owner: Weak<Inner>,
    operation: String,
    digest: String,
    claim: Option<C>,
}
impl<C> std::fmt::Debug for Lease<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetainedClaim")
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}
impl<C> Drop for Lease<C> {
    fn drop(&mut self) {
        if self.claim.is_some() {
            if let Some(owner) = self.owner.upgrade() {
                if let Ok(mut state) = owner.state.lock() {
                    if let Some(entry) = state.entries.get_mut(&self.operation) {
                        // A dropped permit cannot prove whether a native effect
                        // happened. Keep the reservation and refuse replay.
                        if entry.observed.is_none() {
                            entry.stage = Stage::Unknown;
                        }
                    }
                }
            }
        }
    }
}
#[derive(Debug)]
pub struct AdmissionToken(Lease<AdmissionClaim>);
#[derive(Debug)]
pub struct DispatchToken(Lease<DispatchClaim>);
#[derive(Debug)]
pub struct ExecutionPermit(Lease<DispatchClaim>);

// Tracking uses Weak references, does not prolong Journal lifetimes, and has a
// separate finite bound. Even dropping a wrapper cannot reset its domain while
// an underlying Journal clone remains live. Only Clone shares a registration.
static REGISTRATIONS: OnceLock<Mutex<Vec<Weak<crate::Inner>>>> = OnceLock::new();

fn lock_error() -> Error {
    error(
        "operation_retention_unavailable",
        "Retention state lock was poisoned; no claims are available.",
    )
}
fn uncertain(mut failure: Error) -> Error {
    failure.reconciliation_required = true;
    if failure.message.len() > MAX_ERROR_BYTES {
        let mut end = MAX_ERROR_BYTES;
        while !failure.message.is_char_boundary(end) {
            end -= 1;
        }
        failure.message = failure.message[..end].to_owned();
    }
    failure
}
fn conflict() -> Error {
    error(
        "operation_conflict",
        "Operation identity, owner, or lifecycle token does not match its reserved slot.",
    )
}

impl RetainedJournal {
    pub fn new(journal: Journal, limits: RetentionLimits) -> Result<Self> {
        let limits = limits.validate()?;
        let mut registrations = REGISTRATIONS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .map_err(|_| lock_error())?;
        registrations.retain(|weak| weak.strong_count() != 0);
        let identity = Arc::downgrade(&journal.inner);
        if registrations.iter().any(|weak| weak.ptr_eq(&identity)) {
            return Err(error("operation_retention_registered", "This Journal registration already has a retention owner; clone that owner instead."));
        }
        if registrations.len() >= MAX_REGISTRATIONS {
            return Err(error(
                "operation_retention_capacity",
                "The bounded live retention registration table is full.",
            ));
        }
        registrations.push(identity);
        Ok(Self {
            inner: Arc::new(Inner {
                journal,
                limits,
                state: Mutex::new(State {
                    entries: BTreeMap::new(),
                    reserved_bytes: 0,
                }),
            }),
        })
    }

    pub fn session_id(&self) -> &str {
        self.inner.journal.session_id()
    }

    pub fn admit(&self, request: &PreparedRequest, at_ms: u64) -> Result<Admission> {
        if !request.is_write() {
            return Err(error(
                "operation_record_invalid",
                "Read-only requests cannot reserve effect claims.",
            ));
        }
        let session = request.arguments()["sessionId"]
            .as_str()
            .ok_or_else(conflict)?;
        if session != self.session_id() {
            return Err(error(
                "session_mismatch",
                "Historical sessions cannot reserve new effects.",
            ));
        }
        let operation = request.arguments()["operationId"]
            .as_str()
            .ok_or_else(conflict)?;
        crate::bounded_id(operation)?;
        let digest = crate::request_digest(request.canonical_bytes());
        let existing = {
            let mut state = self.inner.state.lock().map_err(|_| lock_error())?;
            if let Some(entry) = state.entries.get(operation) {
                if entry.digest != digest {
                    return Err(conflict());
                }
                true
            } else {
                let total = state
                    .reserved_bytes
                    .checked_add(self.inner.limits.max_result_bytes)
                    .ok_or_else(conflict)?;
                if state.entries.len() >= self.inner.limits.max_entries
                    || total > self.inner.limits.max_reserved_bytes
                {
                    return Err(error("operation_retention_capacity", "Observed-result slots or reserved bytes are full; no admission or native effect is allowed."));
                }
                state.entries.insert(
                    operation.into(),
                    Entry {
                        digest: digest.clone(),
                        stage: Stage::Admitting,
                        dispatch_id: None,
                        observed: None,
                        failure: None,
                    },
                );
                state.reserved_bytes = total;
                false
            }
        };
        if existing {
            return self.status(session, operation).map(Admission::Existing);
        }
        match self.inner.journal.admit(request, at_ms) {
            Ok(crate::Admission::Created(claim)) => {
                self.update(operation, Stage::Admitted, None)?;
                Ok(Admission::Created(AdmissionToken(
                    self.lease(operation, &digest, claim),
                )))
            }
            Ok(crate::Admission::Existing(_)) => {
                self.update(operation, Stage::Existing, None)?;
                self.status(session, operation).map(Admission::Existing)
            }
            Err(failure) => {
                self.update(operation, Stage::Unknown, Some(failure.clone()))?;
                Err(failure)
            }
        }
    }

    pub fn dispatch(
        &self,
        mut token: AdmissionToken,
        dispatch_id: &str,
        at_ms: u64,
    ) -> Result<DispatchToken> {
        crate::bounded_id(dispatch_id)?;
        let claim = self.take(&mut token.0, Stage::Admitted, Stage::Dispatching)?;
        match self.inner.journal.dispatch(&claim, dispatch_id, at_ms) {
            Ok(crate::DispatchOutcome::Claimed(claim)) => {
                let mut state = self.inner.state.lock().map_err(|_| lock_error())?;
                let entry = state
                    .entries
                    .get_mut(&token.0.operation)
                    .ok_or_else(conflict)?;
                entry.stage = Stage::Dispatched;
                entry.dispatch_id = Some(dispatch_id.into());
                Ok(DispatchToken(self.lease(
                    &token.0.operation,
                    &token.0.digest,
                    claim,
                )))
            }
            Ok(crate::DispatchOutcome::Existing(_)) => {
                self.update(&token.0.operation, Stage::Unknown, None)?;
                Err(conflict())
            }
            Err(failure) => {
                self.update(&token.0.operation, Stage::Unknown, Some(failure.clone()))?;
                Err(failure)
            }
        }
    }

    /// The returned permit is the only positive execution authorization from
    /// this layer. Consume it with finish after the one native invocation.
    pub fn start(
        &self,
        mut token: DispatchToken,
        document: Value,
        at_ms: u64,
    ) -> Result<ExecutionPermit> {
        let claim = self.take(&mut token.0, Stage::Dispatched, Stage::Starting)?;
        match self.inner.journal.start(&claim, document, at_ms) {
            Ok(transition) if transition.claimed => {
                self.update(&token.0.operation, Stage::Executing, None)?;
                Ok(ExecutionPermit(self.lease(
                    &token.0.operation,
                    &token.0.digest,
                    claim,
                )))
            }
            Ok(_) => {
                self.update(&token.0.operation, Stage::Unknown, None)?;
                Err(conflict())
            }
            Err(failure) => {
                self.update(&token.0.operation, Stage::Unknown, Some(failure.clone()))?;
                Err(failure)
            }
        }
    }

    /// Pass `now_ms()` directly, without `?`, so clock failure cannot discard
    /// the result. A persistence failure returns Ok with Unconfirmed and keeps
    /// the exact bounded observation; it never returns another execution token.
    pub fn finish(
        &self,
        mut token: ExecutionPermit,
        result: Value,
        time: Result<u64>,
    ) -> Result<Completion> {
        self.complete(&mut token.0, Stage::Executing, result, time, false, false)
    }

    /// Preserve an ambiguous native result without committing a terminal state.
    /// HTTP success:false alone does not prove an effect failed or did not run.
    pub fn finish_unknown(&self, mut token: ExecutionPermit, result: Value) -> Result<Completion> {
        self.complete(&mut token.0, Stage::Executing, result, Ok(0), false, true)
    }

    pub fn not_executed(
        &self,
        mut token: DispatchToken,
        result: Value,
        time: Result<u64>,
    ) -> Result<Completion> {
        self.complete(&mut token.0, Stage::Dispatched, result, time, true, false)
    }

    pub fn status(&self, session: &str, operation: &str) -> Result<Status> {
        crate::bounded_id(session)?;
        crate::bounded_id(operation)?;
        // Never block retention insertion on disk access. After the journal
        // read, snapshot the latest in-memory observation with a short lock.
        let (durable, storage_error) = match self.inner.journal.status(session, operation) {
            Ok(status) => (Some(status), None),
            Err(failure) => (None, Some(uncertain(failure))),
        };
        let historical = session != self.session_id();
        let entry = if historical {
            None
        } else {
            self.inner
                .state
                .lock()
                .map_err(|_| lock_error())?
                .entries
                .get(operation)
                .cloned()
        };
        if let (Some(entry), Some(record)) = (
            &entry,
            durable.as_ref().and_then(|status| status.record.as_ref()),
        ) {
            if record.request_digest != entry.digest
                || entry.dispatch_id.as_ref().is_some_and(|id| {
                    record
                        .dispatch_id
                        .as_ref()
                        .is_some_and(|actual| actual != id)
                })
            {
                return Err(conflict());
            }
        }
        let observed = entry
            .as_ref()
            .and_then(|entry| {
                entry
                    .observed
                    .as_ref()
                    .map(|observation| self.observation(operation, &entry.digest, observation))
            })
            .transpose()?;
        let terminal = durable
            .as_ref()
            .and_then(|status| status.record.as_ref())
            .is_some_and(|record| record.phase.is_terminal());
        let authority = if terminal {
            Authority::DurableTerminal
        } else if observed.is_some() {
            Authority::ObservedNondurable
        } else if entry.as_ref().is_some_and(|entry| {
            matches!(
                entry.stage,
                Stage::Admitting | Stage::Unknown | Stage::Existing
            )
        }) {
            Authority::Unknown
        } else if durable.as_ref().is_some_and(|status| {
            status.outcome == crate::Outcome::Pending && !status.reconciliation_required
        }) {
            Authority::Pending
        } else {
            Authority::Unknown
        };
        Ok(Status {
            durable,
            observed,
            storage_error,
            retention_error: entry.and_then(|entry| entry.failure),
            authority,
            historical,
            reconciliation_required: matches!(
                authority,
                Authority::ObservedNondurable | Authority::Unknown
            ),
        })
    }

    fn complete(
        &self,
        token: &mut Lease<DispatchClaim>,
        expected: Stage,
        result: Value,
        time: Result<u64>,
        not_executed: bool,
        force_unknown: bool,
    ) -> Result<Completion> {
        let outcome = if force_unknown {
            ObservedOutcome::Unknown
        } else {
            match result.get("success").and_then(Value::as_bool) {
                Some(false)
                    if not_executed
                        && result.get("nativeExecutionStarted") == Some(&Value::Bool(false)) =>
                {
                    ObservedOutcome::NotExecuted
                }
                Some(true) if !not_executed => ObservedOutcome::Succeeded,
                Some(false) if !not_executed => ObservedOutcome::Failed,
                _ => ObservedOutcome::Unknown,
            }
        };
        // Encoding is capped while writing; a large Value cannot cause a second
        // unbounded allocation. Its caller-owned allocation is outside cache.
        let (bytes, encoding_error) = match encode(&result, self.inner.limits.max_result_bytes) {
            Ok(bytes) => (Some(bytes), None),
            Err(failure) => (None, Some(failure)),
        };
        let invalid = if encoding_error.is_some() {
            encoding_error
        } else if force_unknown {
            Some(error(
                "operation_outcome_unknown",
                "Native effect outcome is uncertain; terminal persistence is forbidden.",
            ))
        } else if outcome == ObservedOutcome::Unknown {
            Some(error(
                "operation_record_invalid",
                "Observed terminal result has invalid success/execution fields.",
            ))
        } else {
            None
        };
        let claim = {
            let mut state = self.inner.state.lock().map_err(|_| lock_error())?;
            self.check_owner(token)?;
            let entry = state
                .entries
                .get_mut(&token.operation)
                .ok_or_else(conflict)?;
            if entry.digest != token.digest || entry.stage != expected || entry.observed.is_some() {
                return Err(conflict());
            }
            let claim = token.claim.take().ok_or_else(conflict)?;
            if entry.dispatch_id.as_deref() != Some(claim.dispatch_id()) {
                return Err(conflict());
            }
            entry.observed = Some(StoredObservation {
                dispatch_id: claim.dispatch_id().into(),
                outcome,
                bytes,
            });
            entry.stage = Stage::Observed;
            claim
        };
        // From this point, every ordinary clock/validation/storage error leaves
        // the observation available. No cache lock spans Journal or filesystem.
        let persistence = if let Some(failure) = invalid {
            Err(failure)
        } else {
            time.and_then(|at_ms| {
                if not_executed {
                    self.inner.journal.not_executed(&claim, result, at_ms)
                } else {
                    self.inner.journal.finish(&claim, result, at_ms)
                }
            })
            .and_then(|transition| {
                if transition.claimed {
                    Ok(())
                } else {
                    Err(error(
                        "operation_conflict",
                        "A terminal observation cannot replace an existing durable state.",
                    ))
                }
            })
        };
        let failure = persistence.err().map(uncertain);
        self.update(&token.operation, Stage::Observed, failure.clone())?;
        let entry = self
            .inner
            .state
            .lock()
            .map_err(|_| lock_error())?
            .entries
            .get(&token.operation)
            .cloned()
            .ok_or_else(conflict)?;
        let observation = self.observation(
            &token.operation,
            &token.digest,
            entry.observed.as_ref().ok_or_else(conflict)?,
        )?;
        Ok(Completion {
            observation,
            persistence: if failure.is_none() {
                Persistence::Committed
            } else {
                Persistence::Unconfirmed
            },
            persistence_error: failure,
        })
    }

    fn observation(
        &self,
        operation: &str,
        digest: &str,
        stored: &StoredObservation,
    ) -> Result<Observation> {
        let result = stored
            .bytes
            .as_ref()
            .map(|bytes| {
                serde_json::from_slice(bytes.as_slice()).map_err(|_| {
                    error(
                        "operation_retention_invalid",
                        "Retained observation encoding is invalid.",
                    )
                })
            })
            .transpose()?;
        Ok(Observation {
            session_id: self.session_id().into(),
            operation_id: operation.into(),
            request_digest: digest.into(),
            dispatch_id: stored.dispatch_id.clone(),
            outcome: stored.outcome,
            result,
            payload_omitted: stored.bytes.is_none(),
        })
    }
    fn lease<C>(&self, operation: &str, digest: &str, claim: C) -> Lease<C> {
        Lease {
            owner: Arc::downgrade(&self.inner),
            operation: operation.into(),
            digest: digest.into(),
            claim: Some(claim),
        }
    }
    fn check_owner<C>(&self, lease: &Lease<C>) -> Result<()> {
        if !lease.owner.ptr_eq(&Arc::downgrade(&self.inner)) {
            return Err(conflict());
        }
        Ok(())
    }
    fn take<C>(&self, lease: &mut Lease<C>, expected: Stage, next: Stage) -> Result<C> {
        self.check_owner(lease)?;
        let mut state = self.inner.state.lock().map_err(|_| lock_error())?;
        let entry = state
            .entries
            .get_mut(&lease.operation)
            .ok_or_else(conflict)?;
        if entry.digest != lease.digest || entry.stage != expected {
            return Err(conflict());
        }
        let claim = lease.claim.take().ok_or_else(conflict)?;
        entry.stage = next;
        Ok(claim)
    }
    fn update(&self, operation: &str, stage: Stage, failure: Option<Error>) -> Result<()> {
        let mut state = self.inner.state.lock().map_err(|_| lock_error())?;
        let entry = state.entries.get_mut(operation).ok_or_else(conflict)?;
        entry.stage = stage;
        entry.failure = failure.map(uncertain);
        Ok(())
    }
}

fn encode(value: &Value, limit: usize) -> Result<Arc<Vec<u8>>> {
    // Keep iteration storage proportional to depth, never to array length.
    // The stricter depth bound ensures the retained bytes can be parsed by
    // serde_json's default recursion limit when status is projected later.
    enum Children<'a> {
        Array(std::slice::Iter<'a, Value>),
        Object(serde_json::map::Values<'a>),
    }
    impl<'a> Children<'a> {
        fn next(&mut self) -> Option<&'a Value> {
            match self {
                Self::Array(iter) => iter.next(),
                Self::Object(iter) => iter.next(),
            }
        }
    }
    let oversize = || {
        error("operation_observation_oversize", "Observed result exceeded its reserved byte bound; outcome metadata retained, payload omitted.")
    };
    let mut stack = Vec::new();
    let mut current = Some(value);
    let mut nodes = 0usize;
    while let Some(node) = current {
        nodes += 1;
        if nodes > limit {
            return Err(oversize());
        }
        let children = match node {
            Value::Array(values) => Some(Children::Array(values.iter())),
            Value::Object(values) => Some(Children::Object(values.values())),
            _ => None,
        };
        if let Some(children) = children {
            if stack.len() >= 64 {
                return Err(error("operation_observation_depth", "Observed result exceeds the retained payload nesting bound; outcome metadata retained, payload omitted."));
            }
            stack.push(children);
        }
        current = loop {
            match stack.last_mut() {
                Some(children) => match children.next() {
                    Some(value) => break Some(value),
                    None => {
                        stack.pop();
                    }
                },
                None => break None,
            }
        };
    }
    struct Bounded {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for Bounded {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if buffer.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(io::Error::other("result exceeds reserved bytes"));
            }
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    // Fixed capacity matches reservation accounting and prevents geometric
    // Vec growth from retaining more bytes than the reservation permits.
    let mut writer = Bounded {
        bytes: Vec::with_capacity(limit),
        limit,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| oversize())?;
    Ok(Arc::new(writer.bytes))
}

#[cfg(test)]
mod tests;

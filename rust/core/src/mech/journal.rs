//! The durable operation journal (the add-in's journal.py), ported. Records are canonical JSON
//! rows keyed by (session, operation); every read re-validates the digest, identity, phase and
//! canonical encoding, so a row written by the Python add-in decodes here and the reverse.
//! Storage itself is behind [`Table`]: SQLite in the exe (`bridge/src/mech_store.rs`), memory
//! in the golden tests. No replay, no reset of corrupt storage, no eviction.

use super::canonical;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Mutex;

pub const STORAGE_VERSION: u64 = 1;
pub const APPLICATION_ID: i64 = 0x41444D4D;
pub const MAX_RECORDS: u64 = 10000;
pub const MAX_DATABASE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub const TERMINAL: [&str; 3] = ["succeeded", "failed", "not_executed"];
pub const PHASES: [&str; 6] = ["admitted", "dispatched", "executing", "succeeded", "failed", "not_executed"];

#[derive(Debug, Clone, PartialEq)]
pub struct JournalError {
    pub code: String,
    pub message: String,
    /// Set for `operation_conflict` raised by admission: (session, operation, submitted, existing).
    pub conflict: Option<(String, String, String, String)>,
}

impl JournalError {
    pub fn new(code: &str, message: impl Into<String>) -> Self { JournalError { code: code.into(), message: message.into(), conflict: None } }
}

fn require(ok: bool, code: &str, message: &str) -> Result<(), JournalError> {
    if ok { Ok(()) } else { Err(JournalError::new(code, message)) }
}

pub fn is_terminal(phase: &str) -> bool { TERMINAL.contains(&phase) }

pub fn sha256_hex(bytes: &[u8]) -> String { Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect() }

pub fn canonical(value: &Value, maximum: usize) -> Result<Vec<u8>, JournalError> {
    let data = canonical::encode(value);
    require(data.len() <= maximum, "operation_record_too_large", "Operation record exceeds its durable byte limit.")?;
    Ok(data)
}

pub fn fingerprint(request: &Value) -> Result<String, JournalError> { Ok(sha256_hex(&canonical(request, MAX_REQUEST_BYTES)?)) }

/// One row: session, operation, canonical record bytes, sha256 of those bytes.
pub type Row = (String, String, Vec<u8>, String);

/// The storage under the journal. `open(false)` answers None when no journal exists yet (a
/// history read never creates storage); `open(true)` begins an immediate write transaction.
pub trait Table: Send + Sync {
    fn open(&self, create: bool) -> Result<Option<Box<dyn Tx + '_>>, JournalError>;
}

pub trait Tx {
    fn get(&mut self, session: &str, operation: &str) -> Result<Option<(Vec<u8>, String)>, JournalError>;
    fn all(&mut self) -> Result<Vec<Row>, JournalError>;
    fn count(&mut self) -> Result<u64, JournalError>;
    fn put(&mut self, row: &Row) -> Result<(), JournalError>;
    fn commit(self: Box<Self>) -> Result<(), JournalError>;
}

/// `_decode`: identity, phase, result shape and canonical encoding must all hold.
pub fn decode(raw: &[u8], digest: &str, session: &str, operation: &str, max_record_bytes: usize) -> Result<Value, JournalError> {
    require(raw.len() <= max_record_bytes && sha256_hex(raw) == digest, "operation_storage_corrupt", "Operation record digest or size is invalid.")?;
    let malformed = || JournalError::new("operation_storage_corrupt", "Operation record is malformed.");
    let record: Value = serde_json::from_slice(raw).map_err(|_| malformed())?;
    let field = |k: &str| record.get(k).ok_or_else(malformed);
    let finite = |v: &Value| match v { Value::Number(n) => Ok(n.as_f64().is_some_and(f64::is_finite)), Value::Bool(_) => Ok(true), _ => Err(malformed()) };
    let phase = field("phase")?;
    let events = field("events")?;
    let digest_ok = field("requestDigest")?.as_str().is_some_and(|d| d.chars().count() == 64);
    let mut valid = super::validate::py_eq(field("storageVersion")?, &json!(STORAGE_VERSION))
        && field("sessionId")? == session && field("operationId")? == operation
        && phase.as_str().is_some_and(|p| PHASES.contains(&p)) && digest_ok
        && events.as_array().is_some_and(|e| (1..=6).contains(&e.len()))
        && finite(field("createdAt")?)? && finite(field("updatedAt")?)?;
    let result = field("result")?;
    if phase.as_str().is_some_and(is_terminal) {
        let success = result.as_object().and_then(|r| r.get("success")).and_then(Value::as_bool);
        valid = valid && success.is_some() && success == Some(phase == "succeeded");
        if result.as_object().is_none() { valid = false; }
    } else {
        valid = valid && result.is_null();
    }
    require(valid && canonical(&record, max_record_bytes)? == raw, "operation_storage_corrupt", "Operation record identity, phase or canonical encoding is invalid.")?;
    Ok(record)
}

pub struct Journal {
    table: Box<dyn Table>,
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
    pub max_records: u64,
    pub max_record_bytes: usize,
    lock: Mutex<()>,
}

pub struct Admitted { pub created: bool, pub record: Value }
pub struct Transitioned { pub claimed: bool, pub record: Value }

impl Journal {
    pub fn new(table: Box<dyn Table>, clock: Box<dyn Fn() -> f64 + Send + Sync>) -> Self {
        Journal { table, clock, max_records: MAX_RECORDS, max_record_bytes: MAX_RECORD_BYTES, lock: Mutex::new(()) }
    }

    fn read(&self, tx: &mut dyn Tx, session: &str, operation: &str) -> Result<Option<Value>, JournalError> {
        match tx.get(session, operation)? {
            Some((raw, digest)) => decode(&raw, &digest, session, operation, self.max_record_bytes).map(Some),
            None => Ok(None),
        }
    }

    fn save(&self, tx: &mut dyn Tx, record: &Value) -> Result<(), JournalError> {
        let raw = canonical(record, self.max_record_bytes)?;
        let digest = sha256_hex(&raw);
        let (session, operation) = (record["sessionId"].as_str().unwrap_or_default().to_string(), record["operationId"].as_str().unwrap_or_default().to_string());
        decode(&raw, &digest, &session, &operation, self.max_record_bytes)?;
        tx.put(&(session, operation, raw, digest))
    }

    /// `_run`: one transaction per call; any non-journal failure becomes operation_storage_failed
    /// (the store maps its own errors), and nothing is retried or reset here.
    fn run<T>(&self, create: bool, action: impl FnOnce(&mut dyn Tx) -> Result<T, JournalError>) -> Result<Option<T>, JournalError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut tx) = self.table.open(create)? else { return Ok(None) };
        let out = action(&mut *tx)?;
        if create { tx.commit()?; }
        Ok(Some(out))
    }

    pub fn lookup(&self, session: &str, operation: &str) -> Result<Option<Value>, JournalError> {
        Ok(self.run(false, |tx| self.read(tx, session, operation))?.flatten())
    }

    pub fn admit(&self, request: &Value, source_version: &str) -> Result<Admitted, JournalError> {
        let raw_request = canonical(request, MAX_REQUEST_BYTES)?;
        let request: Value = serde_json::from_slice(&raw_request).map_err(|e| JournalError::new("operation_record_invalid", e.to_string()))?;
        let digest = sha256_hex(&raw_request);
        require(request.is_object(), "operation_record_invalid", "Operation request must be an object.")?;
        let bounded = |v: &Value| v.as_str().is_some_and(|s| (1..=128).contains(&s.chars().count()));
        require(bounded(&request["sessionId"]) && bounded(&request["operationId"]), "operation_record_invalid", "Session and operation IDs must be bounded strings.")?;
        require(bounded(&request["op"]) && (1..=128).contains(&source_version.chars().count()), "operation_record_invalid", "Operation name and source version must be bounded strings.")?;
        let (session, operation) = (request["sessionId"].as_str().unwrap().to_string(), request["operationId"].as_str().unwrap().to_string());
        let out = self.run(true, |tx| {
            for (sid, oid, payload, checksum) in tx.all()? { decode(&payload, &checksum, &sid, &oid, self.max_record_bytes)?; }
            if let Some(existing) = self.read(tx, &session, &operation)? {
                let existing_digest = existing["requestDigest"].as_str().unwrap_or_default().to_string();
                if existing_digest != digest {
                    let mut e = JournalError::new("operation_conflict", "operationId was already used with different arguments.");
                    e.conflict = Some((session.clone(), operation.clone(), digest.clone(), existing_digest));
                    return Err(e);
                }
                return Ok(Admitted { created: false, record: existing });
            }
            require(tx.count()? < self.max_records, "operation_capacity", "Durable operation history is full; no records are evicted.")?;
            let now = (self.clock)();
            let record = json!({"storageVersion": STORAGE_VERSION, "sessionId": session, "operationId": operation,
                "operation": request["op"], "sourceVersion": source_version, "requestDigest": digest,
                "requestedTarget": request.get("target").cloned().unwrap_or(Value::Null), "observedDocument": null,
                "phase": "admitted", "createdAt": now, "updatedAt": now,
                "events": [{"phase": "admitted", "at": now}], "dispatchId": null, "result": null});
            self.save(tx, &record)?;
            Ok(Admitted { created: true, record })
        })?;
        out.ok_or_else(|| JournalError::new("operation_storage_failed", "Operation storage could not be opened."))
    }

    pub fn transition(&self, session: &str, operation: &str, digest: &str, phase: &str, dispatch_id: Option<&str>,
                      document: Option<&Value>, result: Option<&Value>) -> Result<Transitioned, JournalError> {
        let out = self.run(true, |tx| {
            let record = self.read(tx, session, operation)?;
            let Some(mut record) = record.filter(|r| r["requestDigest"] == digest) else {
                return Err(JournalError::new("operation_conflict", "Operation claim does not match its durable admission."));
            };
            let old = record["phase"].as_str().unwrap_or_default().to_string();
            let allowed: &[&str] = match phase {
                "dispatched" => &["admitted"], "executing" => &["dispatched"], "not_executed" => &["admitted", "dispatched"],
                "succeeded" | "failed" => &["executing"],
                _ => return Err(JournalError::new("operation_record_invalid", "Unsupported operation phase transition.")),
            };
            if !allowed.contains(&old.as_str()) { return Ok(Transitioned { claimed: false, record }); }
            if old == "dispatched" || old == "executing" {
                require(dispatch_id.is_some_and(|d| !d.is_empty()) && record["dispatchId"].as_str() == dispatch_id, "operation_conflict", "Dispatch generation does not match the durable record.")?;
            }
            if phase == "dispatched" {
                require(dispatch_id.is_some_and(|d| (1..=128).contains(&d.chars().count())), "operation_record_invalid", "Dispatch ID is required.")?;
                record["dispatchId"] = json!(dispatch_id);
            }
            if phase == "executing" { record["observedDocument"] = document.cloned().unwrap_or(Value::Null); }
            if is_terminal(phase) {
                let ok = result.and_then(|r| r.get("success")).and_then(Value::as_bool);
                require(result.is_some_and(Value::is_object) && ok == Some(phase == "succeeded"), "operation_record_invalid", "Terminal phase contradicts its result.")?;
                record["result"] = result.cloned().unwrap_or(Value::Null);
            }
            let now = (self.clock)();
            record["phase"] = json!(phase);
            record["updatedAt"] = json!(now);
            if let Some(events) = record["events"].as_array_mut() { events.push(json!({"phase": phase, "at": now})); }
            self.save(tx, &record)?;
            Ok(Transitioned { claimed: true, record })
        })?;
        out.ok_or_else(|| JournalError::new("operation_storage_failed", "Operation storage could not be opened."))
    }
}

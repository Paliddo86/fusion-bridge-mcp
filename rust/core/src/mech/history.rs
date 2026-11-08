//! Admission, status and persistence boundaries for synchronous mechanical writes (the add-in's
//! operation_history.py), ported. The exe outlives add-in sessions, so the current session is an
//! argument rather than a field; everything else, including the in-memory evidence kept when the
//! journal fails after native work ran, follows the Python verbatim (tests/golden/mech_history.json).

use super::journal::{self, Journal, JournalError, STORAGE_VERSION};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Mutex;

type Key = (String, String);

pub struct History {
    pub source_version: String,
    journal: Journal,
    state: Mutex<Cache>,
}

#[derive(Default)]
struct Cache { unconfirmed: HashMap<Key, Value>, known: HashMap<Key, String>, observation_bytes: i64 }

fn truncate(s: &str) -> String { s.chars().take(4096).collect() }

fn key_of(d: &Value) -> Option<Key> {
    Some((d.get("sessionId")?.as_str()?.to_string(), d.get("operationId")?.as_str()?.to_string()))
}

impl History {
    pub fn new(source_version: &str, journal: Journal) -> Self {
        History { source_version: source_version.into(), journal, state: Mutex::new(Cache::default()) }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> { self.state.lock().unwrap_or_else(|e| e.into_inner()) }

    pub fn failure(&self, current: Option<&str>, error: &JournalError, descriptor: Option<&Value>, observed: Option<&Value>, not_started: bool, mut remember: bool) -> Value {
        let mut result = json!({"success": false, "sessionId": current, "errorCode": error.code, "error": truncate(&error.message),
            "outcome": if not_started { "not_executed" } else { "unknown" }, "nativeExecutionStarted": if not_started { json!(false) } else { Value::Null }});
        if let Some(d) = descriptor {
            result["operationId"] = d["operationId"].clone();
            result["requestDigest"] = d.get("requestDigest").cloned().unwrap_or(Value::Null);
        }
        if let Some(obs) = observed {
            result["errorCode"] = json!("operation_reconciliation_required");
            result["rollbackClaimed"] = json!(false);
            result["nativeExecutionStarted"] = json!(true);
            result["persistenceError"] = json!({"code": error.code, "message": truncate(&error.message)});
            match journal::canonical(obs, journal::MAX_RECORD_BYTES) {
                Ok(raw) => {
                    result["observation"] = json!({"sha256": journal::sha256_hex(&raw), "bytes": raw.len()});
                    if raw.len() <= 65536 { result["observedResult"] = obs.clone(); } else { result["observation"]["resultOmitted"] = json!(true); }
                }
                Err(_) => { result["observation"] = json!({"resultOmitted": true, "reason": "Observed result exceeds record limits or is not canonical JSON."}); }
            }
            remember = true;
        }
        let key = descriptor.and_then(key_of);
        let known;
        {
            let cache = self.cache();
            known = key.as_ref().is_some_and(|k| cache.known.get(k).map(|d| Some(&Value::String(d.clone())) == descriptor.and_then(|x| x.get("requestDigest"))).unwrap_or(false));
            let old = key.as_ref().and_then(|k| cache.unconfirmed.get(k)).cloned().unwrap_or_else(|| json!({}));
            if known && old.get("observation").is_some_and(|o| o.as_object().is_some_and(|m| !m.is_empty())) && observed.is_none() {
                result = old;
                if let Some(m) = result.as_object_mut() { m.remove("_payloadBytes"); }
                result["laterPersistenceFailure"] = json!({"code": error.code, "message": truncate(&error.message)});
                remember = true;
            }
        }
        if observed.is_some() {
            result["reconciliationRequired"] = json!(true);
            result["durability"] = json!({"terminalRecorded": false});
        }
        if remember && known {
            result["reconciliationRequired"] = json!(true);
            result["durability"] = json!({"terminalRecorded": false});
            let mut cached = result.clone();
            let mut cache = self.cache();
            let k = key.clone().unwrap();
            let old = cache.unconfirmed.get(&k).cloned().unwrap_or_else(|| json!({}));
            if old.get("observation").is_some() && observed.is_none() {
                cached = old.clone();
                if let Some(m) = cached.as_object_mut() { m.remove("_payloadBytes"); }
                cached["laterPersistenceFailure"] = json!({"code": error.code, "message": truncate(&error.message)});
                result = cached.clone();
            }
            let mut payload = crate::mech::canonical::encode(cached.get("observedResult").unwrap_or(&Value::Null)).len() as i64;
            cache.observation_bytes -= old.get("_payloadBytes").and_then(Value::as_i64).unwrap_or(0);
            if cache.observation_bytes + payload > 4 * 1024 * 1024 {
                if let Some(m) = cached.as_object_mut() { m.remove("observedResult"); }
                if cached.get("observation").is_some() { cached["observation"]["resultOmitted"] = json!(true); }
                payload = 0;
            }
            cached["_payloadBytes"] = json!(payload);
            cache.observation_bytes += payload;
            cache.unconfirmed.insert(k, cached);
        }
        result
    }

    pub fn unconfirmed(&self, key: &Key) -> Option<Value> {
        let mut r = self.cache().unconfirmed.get(key).cloned()?;
        if let Some(m) = r.as_object_mut() { m.remove("_payloadBytes"); }
        Some(r).filter(|r| r.as_object().is_some_and(|m| !m.is_empty()))
    }

    fn remember_record(&self, current: Option<&str>, record: &Value) {
        if record["sessionId"].as_str().is_some() && record["sessionId"].as_str() == current {
            let k = key_of(record).unwrap();
            let digest = record["requestDigest"].as_str().unwrap_or_default().to_string();
            let mut cache = self.cache();
            if cache.known.get(&k).is_none_or(|d| *d == digest) { cache.known.insert(k, digest); }
        }
    }

    pub fn metadata(record: &Value) -> Value {
        let mut m = Map::new();
        for k in ["storageVersion", "sessionId", "operationId", "operation", "sourceVersion", "requestDigest", "requestedTarget", "observedDocument", "phase", "createdAt", "updatedAt", "events"] {
            m.insert(k.into(), record.get(k).cloned().unwrap_or(Value::Null));
        }
        Value::Object(m)
    }

    pub fn terminal_reply(record: &Value, replayed: bool) -> Value {
        let mut result = record["result"].clone();
        result["durability"] = json!({"terminalRecorded": true, "storageVersion": STORAGE_VERSION, "recordedSessionId": record["sessionId"], "phase": record["phase"]});
        if replayed { result["replayed"] = json!(true); }
        result
    }

    pub fn pending_reply(&self, current: Option<&str>, record: &Value) -> Value {
        let phase = record["phase"].as_str().unwrap_or_default();
        if journal::is_terminal(phase) { return Self::terminal_reply(record, true); }
        if let Some(u) = key_of(record).and_then(|k| self.unconfirmed(&k)) { return u; }
        if phase == "admitted" {
            return json!({"success": false, "sessionId": current, "operationId": record["operationId"],
                "errorCode": "operation_reconciliation_required", "outcome": "unknown",
                "reconciliationRequired": true, "durability": {"terminalRecorded": false},
                "error": "Admission has no durable dispatch claim. Query status; automatic redispatch is disabled."});
        }
        json!({"success": false, "sessionId": current, "operationId": record["operationId"],
            "errorCode": "operation_in_progress", "error": "Operation is admitted; query its recorded status. No duplicate dispatch occurred.",
            "phase": phase, "outcome": "pending"})
    }

    /// Durable admission plus the dispatch claim. Ok(descriptor) to dispatch, Err(reply) to answer.
    pub fn admit(&self, current: Option<&str>, request: &Value, dispatch_id: &str) -> Result<Value, Value> {
        let descriptor = json!({"sessionId": request["sessionId"], "operationId": request["operationId"],
            "requestDigest": journal::fingerprint(request).unwrap_or_default(), "dispatchId": dispatch_id});
        if request["sessionId"].as_str().is_none() || request["sessionId"].as_str() != current {
            return Err(json!({"success": false, "sessionId": current, "errorCode": "session_mismatch",
                "error": "Old-session operation history is readable, but native execution is never replayed across sessions."}));
        }
        let (session, operation, digest) = (request["sessionId"].as_str().unwrap(), request["operationId"].as_str().unwrap_or_default(), descriptor["requestDigest"].as_str().unwrap().to_string());
        let accepted = match self.journal.admit(request, &self.source_version) {
            Ok(a) => a,
            Err(e) if e.conflict.is_some() => {
                let (sid, oid, submitted, existing) = e.conflict.clone().unwrap();
                if sid == session && oid == operation && submitted == digest && existing != submitted {
                    let mut reply = self.failure(current, &e, Some(&descriptor), None, true, false);
                    reply["conflict"] = json!({"scope": "submitted_request", "existingRequestDigest": existing});
                    return Err(reply);
                }
                return Err(self.failure(current, &e, Some(&descriptor), None, false, false));
            }
            Err(e) => {
                // The admission COMMIT may have succeeded before its acknowledgement was lost.
                if let Ok(Some(record)) = self.journal.lookup(session, operation) {
                    if record["requestDigest"] == digest.as_str() { self.remember_record(current, &record); }
                }
                return Err(self.failure(current, &e, Some(&descriptor), None, false, true));
            }
        };
        self.remember_record(current, &accepted.record);
        if !accepted.created { return Err(self.pending_reply(current, &accepted.record)); }
        match self.journal.transition(session, operation, &digest, "dispatched", Some(dispatch_id), None, None) {
            Ok(t) if t.claimed => Ok(descriptor),
            Ok(t) => Err(self.pending_reply(current, &t.record)),
            Err(e) => Err(self.failure(current, &e, Some(&descriptor), None, true, true)),
        }
    }

    pub fn start(&self, current: Option<&str>, descriptor: &Value, request: &Value, document: &Value) -> Option<Value> {
        let digest = journal::fingerprint(request).ok();
        if descriptor["sessionId"].as_str() != current || descriptor["sessionId"] != request["sessionId"]
            || descriptor["operationId"] != request["operationId"] || descriptor["requestDigest"].as_str() != digest.as_deref() {
            return Some(json!({"success": false, "sessionId": current, "errorCode": "operation_conflict",
                "error": "Dispatch identity does not match this session and canonical operation."}));
        }
        let (s, o, d) = (descriptor["sessionId"].as_str().unwrap_or_default(), descriptor["operationId"].as_str().unwrap_or_default(), descriptor["requestDigest"].as_str().unwrap_or_default());
        match self.journal.transition(s, o, d, "executing", descriptor["dispatchId"].as_str(), Some(document), None) {
            Ok(t) if t.claimed => None,
            Ok(t) => Some(self.pending_reply(current, &t.record)),
            Err(e) => Some(self.failure(current, &e, Some(descriptor), None, false, true)),
        }
    }

    /// `start` for a dispatch the exe lost track of (its reply never arrived): the journal
    /// record is the identity proof, and the add-in's claim supplies the observed document.
    pub fn start_recorded(&self, current: Option<&str>, record: &Value, document: &Value) -> Option<Value> {
        if record["sessionId"].as_str().is_none() || record["sessionId"].as_str() != current {
            return Some(json!({"success": false, "sessionId": current, "errorCode": "operation_conflict",
                "error": "Dispatch identity does not match this session and canonical operation."}));
        }
        let (s, o, d) = (record["sessionId"].as_str().unwrap_or_default(), record["operationId"].as_str().unwrap_or_default(), record["requestDigest"].as_str().unwrap_or_default());
        match self.journal.transition(s, o, d, "executing", record["dispatchId"].as_str(), Some(document), None) {
            Ok(t) if t.claimed => None,
            Ok(t) => Some(self.pending_reply(current, &t.record)),
            Err(e) => Some(self.failure(current, &e, Some(&json!({"sessionId": s, "operationId": o, "requestDigest": d})), None, false, true)),
        }
    }

    pub fn finish(&self, current: Option<&str>, descriptor: &Value, result: &Value) -> Value {
        let (s, o, d) = (descriptor["sessionId"].as_str().unwrap_or_default(), descriptor["operationId"].as_str().unwrap_or_default(), descriptor["requestDigest"].as_str().unwrap_or_default());
        let phase = if result["success"].as_bool() == Some(true) { "succeeded" } else { "failed" };
        match self.journal.transition(s, o, d, phase, descriptor["dispatchId"].as_str(), None, Some(result)) {
            Ok(t) if t.claimed => Self::terminal_reply(&t.record, false),
            Ok(_) => self.failure(current, &JournalError::new("operation_conflict", "Terminal transition lost its executing claim; outcome requires reconciliation."), Some(descriptor), Some(result), false, false),
            Err(e) => self.failure(current, &e, Some(descriptor), Some(result), false, false),
        }
    }

    pub fn not_executed(&self, current: Option<&str>, descriptor: Option<&Value>, result: &Value) -> Value {
        let Some(descriptor) = descriptor else { return result.clone() };
        if descriptor["sessionId"].as_str() != current || current.is_none() {
            return json!({"success": false, "sessionId": current, "errorCode": "session_mismatch",
                "error": "A stale dispatch cannot update history from a previous session.", "outcome": "unknown"});
        }
        let mut result = result.clone();
        result["sessionId"] = descriptor["sessionId"].clone();
        result["operationId"] = descriptor["operationId"].clone();
        result["nativeExecutionStarted"] = json!(false);
        let (s, o, d) = (descriptor["sessionId"].as_str().unwrap_or_default(), descriptor["operationId"].as_str().unwrap_or_default(), descriptor["requestDigest"].as_str().unwrap_or_default());
        match self.journal.transition(s, o, d, "not_executed", descriptor["dispatchId"].as_str(), None, Some(&result)) {
            Ok(t) if t.claimed => Self::terminal_reply(&t.record, false),
            // A fast native handler may already have claimed or completed the event.
            Ok(t) => self.pending_reply(current, &t.record),
            Err(e) => self.failure(current, &e, Some(descriptor), None, false, true),
        }
    }

    pub fn lookup(&self, session: &str, operation: &str) -> Result<Option<Value>, JournalError> { self.journal.lookup(session, operation) }

    pub fn status(&self, current: Option<&str>, args: &Value) -> Value {
        let bounded = |v: &Value| v.as_str().is_some_and(|s| (1..=128).contains(&s.chars().count()));
        if !(bounded(&args["sessionId"]) && bounded(&args["operationId"])) {
            return json!({"success": false, "errorCode": "invalid_input", "error": "Bounded sessionId and operationId strings are required."});
        }
        let (session, operation) = (args["sessionId"].as_str().unwrap(), args["operationId"].as_str().unwrap());
        let key = (session.to_string(), operation.to_string());
        let record = match self.journal.lookup(session, operation) {
            Ok(r) => r,
            Err(e) => {
                let retained = self.unconfirmed(&key);
                return json!({"success": false, "sessionId": session, "currentSessionId": current, "operationId": operation,
                    "errorCode": e.code, "error": e.message, "outcome": "unknown", "reconciliationRequired": true,
                    "persistenceFailure": retained, "durability": {"terminalRecorded": false},
                    "observedResult": retained.as_ref().and_then(|r| r.get("observedResult").cloned())});
            }
        };
        let Some(record) = record else {
            return json!({"success": false, "sessionId": session, "currentSessionId": current, "operationId": operation,
                "errorCode": "operation_unknown", "error": "No durable record identifies this operation; no execution inference is made.", "outcome": "unknown"});
        };
        let historical = Some(session) != current;
        let phase = record["phase"].as_str().unwrap_or_default();
        let terminal = journal::is_terminal(phase);
        let unconfirmed = self.unconfirmed(&key);
        let state = if terminal { if phase == "succeeded" { "succeeded" } else { "failed" } }
            else if historical { "interrupted" } else if unconfirmed.is_some() || phase == "admitted" { "reconciliation_required" } else { "running" };
        let outcome = if terminal { json!("recorded") } else if historical { json!("unknown") }
            else if let Some(u) = &unconfirmed { u["outcome"].clone() } else if phase == "admitted" { json!("unknown") } else { json!("pending") };
        json!({"success": true, "sessionId": session, "currentSessionId": current, "operationId": operation,
            "state": state, "phase": phase, "historical": historical, "history": Self::metadata(&record),
            "outcome": outcome,
            "nativeExecutionStarted": match &unconfirmed { Some(u) if !historical && !terminal => u.get("nativeExecutionStarted").cloned().unwrap_or(Value::Null), _ => Value::Null },
            "reconciliationRequired": !terminal && (historical || unconfirmed.is_some() || phase == "admitted"),
            "result": if terminal { Self::terminal_reply(&record, false) } else { Value::Null },
            "observedResult": match &unconfirmed { Some(u) if !terminal => u.get("observedResult").cloned().unwrap_or(Value::Null), _ => Value::Null },
            "persistenceFailure": match &unconfirmed { Some(u) if !terminal => u.clone(), _ => Value::Null }})
    }
}

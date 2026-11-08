//! The mechanical verbs, owned by the exe (John's pattern: validation, state and replies here,
//! the Python add-in only runs the Fusion API work in `commands/mechanical`).
//!
//! - `fusion_mech_capabilities`: the schema the add-in executes plus its live session, read from
//!   the add-in's session file, so it never waits on Fusion's main thread.
//! - `fusion_mech_operation_status`: the journal, read here: it answers while Fusion is busy,
//!   hung or closed, and never launches, repairs or focuses anything.
//! - `fusion_mechanical`: validate, then (writes) admit and journal, then the ordinary add-in
//!   pass-through (gate, busy check, etiquette) carries the frozen request to the add-in.
//!
//! A write's dispatch claim file arbitrates "never ran" against "may have run": the add-in
//! creates `<dispatchId>.claim` before executing, the exe creates it only after the pass-through
//! returned without the add-in's answer, and whoever creates it first wins. The add-in also
//! leaves its exact result in `<dispatchId>.result.json`, so a reply lost to a timeout is still
//! recorded later (status, or the next write) - the guarantee the add-in's own journal gave.

use fusion_core::mech::{history::History, journal::{self, Journal}, validate};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const SCHEMA_JSON: &str = include_str!("../../../addin/AdomBridge/commands/mechanical/schema.json");
/// The add-in protocol this exe speaks: it journals nothing itself and claims each dispatch.
pub const PROTOCOL: &str = "exe-journal-1";
/// Add-in answer fields that are not part of its operation result.
const DISPATCH_FIELDS: [&str; 5] = ["success", "dispatchId", "nativeExecutionStarted", "observedDocument", "result"];

static IN_FLIGHT: Mutex<Option<HashSet<String>>> = Mutex::new(None);

pub fn owns_status(verb: &str) -> bool { matches!(verb, "fusion_mech_operation_status" | "mech_operation_status") }
pub fn owns_capabilities(verb: &str) -> bool { matches!(verb, "fusion_mech_capabilities" | "mech_capabilities") }

fn schema() -> &'static Map<String, Value> {
    static S: OnceLock<Map<String, Value>> = OnceLock::new();
    S.get_or_init(|| serde_json::from_str(SCHEMA_JSON).expect("mechanical schema.json is valid JSON"))
}

fn now_f() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

/// One history per journal directory (one in production; tests each use their own).
fn history(state: &Path) -> &'static History {
    static H: Mutex<Vec<(PathBuf, &'static History)>> = Mutex::new(Vec::new());
    let mut all = H.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, h)) = all.iter().find(|(p, _)| p == state) { return h; }
    let h: &'static History = Box::leak(Box::new(History::new(crate::VERSION, Journal::new(Box::new(crate::mech_store::SqliteTable::new(state.to_path_buf())), Box::new(now_f)))));
    all.push((state.to_path_buf(), h));
    h
}

pub struct Session { pub id: String, pub transactions: Value }

/// The add-in's `session.json`, trusted only while the Fusion process that wrote it runs.
pub fn live_session(state: &Path, fusion_pids: &[u32]) -> Option<Session> {
    let s: Value = serde_json::from_str(&std::fs::read_to_string(state.join("session.json")).ok()?).ok()?;
    if s["protocol"] != PROTOCOL || !s["pid"].as_u64().is_some_and(|p| fusion_pids.contains(&(p as u32))) { return None; }
    Some(Session { id: s["sessionId"].as_str()?.to_string(), transactions: s["transactions"].clone() })
}

fn dispatch_dir(state: &Path) -> PathBuf { state.join("dispatch") }
fn claim_path(state: &Path, id: &str) -> PathBuf { dispatch_dir(state).join(format!("{id}.claim")) }
fn result_path(state: &Path, id: &str) -> PathBuf { dispatch_dir(state).join(format!("{id}.result.json")) }

enum Claim { Won, Started(Value), Expired, Unavailable(String) }

/// Try to expire a dispatch. Losing means the add-in already claimed it.
fn expire(state: &Path, id: &str) -> Claim {
    if std::fs::create_dir_all(dispatch_dir(state)).is_err() { return Claim::Unavailable("dispatch directory unavailable".into()); }
    match std::fs::OpenOptions::new().write(true).create_new(true).open(claim_path(state, id)) {
        Ok(mut f) => { let _ = f.write_all(json!({"state": "expired", "at": now_f()}).to_string().as_bytes()); let _ = f.sync_all(); Claim::Won }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => read_claim(state, id).unwrap_or(Claim::Started(Value::Null)),
        Err(e) => Claim::Unavailable(e.to_string()),
    }
}

/// An existing claim. A file still being written by the add-in reads as started.
fn read_claim(state: &Path, id: &str) -> Option<Claim> {
    let text = std::fs::read_to_string(claim_path(state, id)).ok()?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Some(if v["state"] == "expired" { Claim::Expired } else { Claim::Started(v["observedDocument"].clone()) })
}

fn read_result(state: &Path, id: &str) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(result_path(state, id)).ok()?).ok().filter(Value::is_object)
}

fn clear_dispatch(state: &Path, id: &str) {
    let _ = std::fs::remove_file(result_path(state, id));
    let _ = std::fs::remove_file(claim_path(state, id));
}

fn descriptor_of(record: &Value) -> Value {
    json!({"sessionId": record["sessionId"], "operationId": record["operationId"], "requestDigest": record["requestDigest"], "dispatchId": record["dispatchId"]})
}

/// Bring one current-session record up to date with its dispatch files: a result the add-in left
/// after its reply was lost, a claim the exe never saw, or a dispatch the exe lost before any claim.
fn reconcile(state: &Path, current: Option<&str>, record: &Value) {
    let h = history(state);
    let phase = record["phase"].as_str().unwrap_or_default();
    let Some(id) = record["dispatchId"].as_str().filter(|_| matches!(phase, "dispatched" | "executing")) else { return };
    if record["sessionId"].as_str() != current { return; }
    if IN_FLIGHT.lock().unwrap().as_ref().is_some_and(|s| s.contains(id)) { return; }
    let descriptor = descriptor_of(record);
    let claim = read_claim(state, id).unwrap_or_else(|| expire(state, id));
    match claim {
        Claim::Won | Claim::Expired => {
            h.not_executed(current, Some(&descriptor), &json!({"success": false, "errorCode": "request_expired_before_execution",
                "error": "The bridge lost this dispatch before the add-in claimed it; it never ran."}));
            clear_dispatch(state, id);
        }
        Claim::Started(doc) => {
            if phase == "dispatched" { let _ = h.start_recorded(current, record, &doc); }
            if let Some(result) = read_result(state, id) {
                h.finish(current, &descriptor, &result);
                clear_dispatch(state, id);
            }
        }
        Claim::Unavailable(_) => {}
    }
}

/// Results the add-in left behind are recorded before anything new is admitted, so the add-in's
/// own history proofs (exploded previews) see them. Old orphaned files are dropped after a day.
fn reconcile_all(state: &Path, current: Option<&str>) {
    let Ok(dir) = std::fs::read_dir(dispatch_dir(state)) else { return };
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".claim") else { continue };
        let old = entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 86400);
        let found = read_result(state, id).or_else(|| std::fs::read_to_string(entry.path()).ok().and_then(|t| serde_json::from_str(&t).ok()));
        let (s, o) = found.as_ref().map(|v| (v["sessionId"].as_str().map(str::to_string), v["operationId"].as_str().map(str::to_string))).unwrap_or((None, None));
        match (s, o) {
            (Some(s), Some(o)) => {
                if let Ok(Some(record)) = history(state).lookup(&s, &o) {
                    if record["dispatchId"] == id { reconcile(state, current, &record); }
                    if journal::is_terminal(record["phase"].as_str().unwrap_or_default()) || record["sessionId"].as_str() != current { clear_dispatch(state, id); }
                } else if old { clear_dispatch(state, id); }
            }
            _ if old => clear_dispatch(state, id),
            _ => {}
        }
    }
}

pub fn capabilities(state: &Path, pids: &dyn Fn() -> Vec<u32>, ensure_ready: &dyn Fn() -> Value) -> Value {
    let mut session = live_session(state, &pids());
    if session.is_none() {
        // Fusion closed or its add-in still starting: the ordinary gate launches and waits.
        let _ = ensure_ready();
        session = live_session(state, &pids());
    }
    let transactions = session.as_ref().map(|s| s.transactions.clone()).filter(Value::is_object)
        .unwrap_or_else(|| json!({"verified": false, "backend": "PTransaction", "evidence": null, "scope": "one active Fusion Design; no document lifecycle, files or cloud effects"}));
    let mut r = validate::capabilities(session.as_ref().map(|s| s.id.as_str()), &Value::Object(schema().clone()), &transactions);
    if session.is_none() {
        r["_hint"] = json!("No add-in session: Fusion is not running, its add-in is still loading, or the add-in predates 1.12.0-dev.10. Reads work; writes need the sessionId this verb returns once fusion_readiness is ready.");
    }
    r
}

pub fn status(state: &Path, pids: &[u32], args: &Value) -> Value {
    let current = live_session(state, pids).map(|s| s.id);
    if let (Some(s), Some(o)) = (args["sessionId"].as_str(), args["operationId"].as_str()) {
        if let Ok(Some(record)) = history(state).lookup(s, o) { reconcile(state, current.as_deref(), &record); }
    }
    history(state).status(current.as_deref(), args)
}

/// Carry the pass-through's own additions (dialogs, etiquette notes, timing) onto the reply.
fn with_envelope(mut reply: Value, answer: &Value) -> Value {
    if let (Some(r), Some(a)) = (reply.as_object_mut(), answer.as_object()) {
        for (k, v) in a {
            if !DISPATCH_FIELDS.contains(&k.as_str()) && !r.contains_key(k) { r.insert(k.clone(), v.clone()); }
        }
    }
    reply
}

pub fn mechanical(state: &Path, pids: &[u32], args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let current_session = live_session(state, pids).map(|s| s.id);
    let current = current_session.as_deref();
    // The window etiquette reads and strips these before the add-in hop; they are not operation
    // arguments, so they never reach validation or the retry digest.
    let mut user = args.clone();
    let mut envelope = Map::new();
    if let Some(o) = user.as_object_mut() {
        for k in ["foreground", "foregroundReason"] { if let Some(v) = o.remove(k) { envelope.insert(k.into(), v); } }
    }
    let (frozen, write) = match validate::prepare_request(&user, schema(), current) {
        Ok(x) => x,
        Err(e) => return validate::error_result(current, &e),
    };
    let mut sent = frozen.clone();
    for (k, v) in &envelope { sent[k] = v.clone(); }
    if !write { return pass("mechanical", sent); }
    reconcile_all(state, current);
    let id = crate::mech_store::hex32();
    let descriptor = match history(state).admit(current, &frozen, &id) { Ok(d) => d, Err(reply) => return reply };
    IN_FLIGHT.lock().unwrap().get_or_insert_with(HashSet::new).insert(id.clone());
    sent["_dispatch"] = json!({"dispatchId": id});
    let answer = pass("mechanical", sent);
    let reply = settle(state, current, &descriptor, &frozen, &id, &answer);
    if let Some(s) = IN_FLIGHT.lock().unwrap().as_mut() { s.remove(&id); }
    reply
}

fn settle(state: &Path, current: Option<&str>, descriptor: &Value, frozen: &Value, id: &str, answer: &Value) -> Value {
    let h = history(state);
    let answered = answer["dispatchId"].as_str() == Some(id);
    if answered && answer["nativeExecutionStarted"] == true {
        return record_started(state, current, descriptor, frozen, id, &answer["observedDocument"], answer);
    }
    match expire(state, id) {
        Claim::Won | Claim::Expired => {
            // Never ran: the add-in refused before claiming, or the pass-through never delivered.
            let result = if answered { answer["result"].clone() } else { answer.clone() };
            let reply = h.not_executed(current, Some(descriptor), &result);
            clear_dispatch(state, id);
            if answered { with_envelope(reply, answer) } else { reply }
        }
        Claim::Started(doc) => record_started(state, current, descriptor, frozen, id, &doc, answer),
        Claim::Unavailable(why) => {
            let mut reply = h.pending_reply(current, &h.lookup(descriptor["sessionId"].as_str().unwrap_or_default(), descriptor["operationId"].as_str().unwrap_or_default()).ok().flatten().unwrap_or(Value::Null));
            reply["dispatchArbitration"] = json!(why);
            reply
        }
    }
}

fn record_started(state: &Path, current: Option<&str>, descriptor: &Value, frozen: &Value, id: &str, doc: &Value, answer: &Value) -> Value {
    let h = history(state);
    if let Some(pending) = h.start(current, descriptor, frozen, doc) { return with_envelope(pending, answer); }
    let answered = answer["dispatchId"].as_str() == Some(id);
    let result = read_result(state, id).or_else(|| if answered && answer["result"].is_object() { Some(answer["result"].clone()) } else { None });
    match result {
        Some(result) => {
            let reply = h.finish(current, descriptor, &result);
            clear_dispatch(state, id);
            with_envelope(reply, answer)
        }
        // Executing in the add-in and its reply is lost: status (or the next write) records it.
        None => {
            let record = h.lookup(descriptor["sessionId"].as_str().unwrap_or_default(), descriptor["operationId"].as_str().unwrap_or_default()).ok().flatten().unwrap_or(Value::Null);
            with_envelope(h.pending_reply(current, &record), answer)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mech-{tag}-{}", crate::mech_store::hex32()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn session(state: &Path, id: &str, pid: u32) {
        std::fs::write(state.join("session.json"), json!({"protocol": PROTOCOL, "sessionId": id, "pid": pid, "transactions": {"verified": true}}).to_string()).unwrap();
    }
    fn write_args(op_id: &str, sid: &str) -> Value {
        json!({"op": "parameter_set", "sessionId": sid, "operationId": op_id, "target": {"documentId": "d", "revision": "r"}, "name": "w", "expression": "42 mm", "foreground": false})
    }
    /// The add-in side of one dispatch, as commands/mechanical does it.
    fn addin(state: &Path, sent: &Value, claim_first: bool) -> Value {
        let id = sent["_dispatch"]["dispatchId"].as_str().unwrap().to_string();
        assert!(sent.get("foreground").is_some() && sent["_dispatch"].is_object());
        if claim_first {
            std::fs::create_dir_all(dispatch_dir(state)).unwrap();
            std::fs::OpenOptions::new().write(true).create_new(true).open(claim_path(state, &id)).unwrap()
                .write_all(json!({"state": "started", "observedDocument": {"documentId": "d"}}).to_string().as_bytes()).unwrap();
        }
        let result = json!({"success": true, "sessionId": sent["sessionId"], "operation": "parameter_set", "data": {"value": 42}, "operationId": sent["operationId"]});
        std::fs::write(result_path(state, &id), result.to_string()).unwrap();
        json!({"success": true, "dispatchId": id, "nativeExecutionStarted": true, "observedDocument": {"documentId": "d"}, "result": result, "windowEtiquette": "background"})
    }

    #[test]
    fn a_write_is_validated_journaled_dispatched_and_replayed() {
        let st = scratch("ok");
        session(&st, "S1", 7);
        let sent = RefCell::new(Value::Null);
        let r = mechanical(&st, &[7], &write_args("w1", "S1"), &|_, a| { *sent.borrow_mut() = a.clone(); addin(&st, &a, true) });
        assert_eq!(r["success"], true, "{r}");
        assert_eq!(r["durability"]["terminalRecorded"], true);
        assert_eq!(r["windowEtiquette"], "background");
        assert!(!sent.borrow().get("foreground").is_none() && sent.borrow()["_dispatch"]["dispatchId"].is_string());
        // a retry with the same ID replays the recorded result without dispatching again
        let again = mechanical(&st, &[7], &write_args("w1", "S1"), &|_, _| panic!("no second dispatch"));
        assert_eq!(again["replayed"], true);
        let s = status(&st, &[7], &json!({"sessionId": "S1", "operationId": "w1"}));
        assert_eq!(s["state"], "succeeded");
        assert!(std::fs::read_dir(dispatch_dir(&st)).unwrap().next().is_none(), "dispatch files cleared");
    }

    #[test]
    fn a_refusal_before_the_add_in_claims_is_recorded_as_never_run() {
        let st = scratch("busy");
        session(&st, "S1", 7);
        let r = mechanical(&st, &[7], &write_args("w2", "S1"), &|_, _| json!({"success": false, "errorCode": "main_thread_busy", "error": "busy"}));
        assert_eq!(r["durability"]["phase"], "not_executed", "{r}");
        assert_eq!(r["nativeExecutionStarted"], false);
        assert_eq!(status(&st, &[7], &json!({"sessionId": "S1", "operationId": "w2"}))["state"], "failed");
    }

    #[test]
    fn a_reply_lost_after_the_claim_is_recorded_from_the_result_file() {
        let st = scratch("lost");
        session(&st, "S1", 7);
        let r = mechanical(&st, &[7], &write_args("w3", "S1"), &|_, a| {
            let id = a["_dispatch"]["dispatchId"].as_str().unwrap().to_string();
            std::fs::create_dir_all(dispatch_dir(&st)).unwrap();
            std::fs::write(claim_path(&st, &id), json!({"state": "started", "observedDocument": null}).to_string()).unwrap();
            json!({"success": false, "error": "Add-in request failed: timed out"})
        });
        assert_eq!(r["errorCode"], "operation_in_progress", "{r}");
        // the add-in finishes later and leaves its result; status records it
        let rec = history(&st).lookup("S1", "w3").unwrap().unwrap();
        let id = rec["dispatchId"].as_str().unwrap();
        std::fs::write(result_path(&st, id), json!({"success": true, "sessionId": "S1", "operation": "parameter_set", "data": {}, "operationId": "w3"}).to_string()).unwrap();
        let s = status(&st, &[7], &json!({"sessionId": "S1", "operationId": "w3"}));
        assert_eq!(s["state"], "succeeded", "{s}");
    }

    #[test]
    fn a_ray_batch_is_validated_and_answered_without_a_journal_record() {
        let st = scratch("rays");
        session(&st, "S1", 7);
        let ray = json!({"origin": [30, 30, 100], "direction": [0, 0, -1], "length": 200, "against": [0]});
        let batch = |rays: Value| json!({"op": "ray_cast", "target": {"documentId": "d"}, "unit": "mm", "occurrences": ["Plate:1"], "rays": rays});
        let sent = RefCell::new(Value::Null);
        let r = mechanical(&st, &[7], &batch(json!([ray])), &|cmd, a| {
            assert_eq!(cmd, "mechanical");
            *sent.borrow_mut() = a.clone();
            json!({"success": true, "operation": "ray_cast", "data": {"hits": [[[0, 90.0]]], "cast": 1, "complete": true}})
        });
        assert_eq!(r["data"]["hits"], json!([[[0, 90.0]]]), "{r}");
        assert!(sent.borrow().get("_dispatch").is_none(), "a read carries no dispatch");
        assert!(!st.join("operations-v1.sqlite3").exists(), "a ray batch leaves no journal record");
        // a malformed ray, or one too many, is refused here and never reaches Fusion
        let mut too_many = Vec::new();
        too_many.resize(8193, ray.clone());
        for bad in [json!([{"origin": [0, 0], "direction": [0, 0, 1], "length": 1, "against": [0]}]),
                    json!([{"origin": [0, 0, 0], "direction": [0, 0, 1], "length": 1, "against": ["0"]}]),
                    json!([{"origin": [0, 0, 0], "direction": [0, 0, 1], "length": 0, "against": [0]}]),
                    json!([{"origin": [0, 0, 0], "direction": [0, 0, 1], "against": [0]}]),
                    json!([{"origin": [0, 0, 0], "direction": [0, 0, 1], "length": 1, "against": [0], "weight": 2}]),
                    json!(too_many)] {
            let r = mechanical(&st, &[7], &batch(bad), &|_, _| panic!("refused before the add-in"));
            assert_eq!(r["errorCode"], "invalid_input", "{r}");
        }
    }

    #[test]
    fn an_assembly_layout_is_journaled_like_the_other_long_reads() {
        let st = scratch("layout");
        session(&st, "S1", 7);
        let args = json!({"op": "assembly_layout", "sessionId": "S1", "operationId": "l1", "scope": "Panel:1"});
        let r = mechanical(&st, &[7], &args, &|_, a| {
            let id = a["_dispatch"]["dispatchId"].as_str().unwrap().to_string();
            std::fs::create_dir_all(dispatch_dir(&st)).unwrap();
            std::fs::write(claim_path(&st, &id), json!({"state": "started", "observedDocument": {"documentId": "d"}}).to_string()).unwrap();
            let result = json!({"success": true, "sessionId": "S1", "operation": "assembly_layout", "operationId": "l1",
                                "data": {"target": {"documentId": "d", "revision": "r"}, "units": [], "joints": [], "rigidGroups": []}});
            json!({"success": true, "dispatchId": id, "nativeExecutionStarted": true, "observedDocument": {"documentId": "d"}, "result": result})
        });
        assert_eq!(r["durability"]["terminalRecorded"], true, "{r}");
        assert_eq!(r["data"]["target"]["revision"], "r");
        assert_eq!(status(&st, &[7], &json!({"sessionId": "S1", "operationId": "l1"}))["state"], "succeeded");
        // journaled, so it needs the live session and an operation ID like any write
        let r = mechanical(&st, &[7], &json!({"op": "assembly_layout", "sessionId": "S0", "operationId": "l2"}), &|_, _| panic!("never dispatched"));
        assert_eq!(r["errorCode"], "session_mismatch");
    }

    #[test]
    fn writes_need_the_live_session_and_reads_do_not() {
        let st = scratch("session");
        session(&st, "S1", 7);
        let r = mechanical(&st, &[8], &write_args("w4", "S1"), &|_, _| panic!("never dispatched"));
        assert_eq!(r["errorCode"], "session_mismatch");
        assert_eq!(r["sessionId"], Value::Null);
        let read = mechanical(&st, &[8], &json!({"op": "inspect"}), &|cmd, a| json!({"cmd": cmd, "args": a}));
        assert_eq!(read["cmd"], "mechanical");
        assert_eq!(read["args"], json!({"op": "inspect"}));
        let caps = capabilities(&st, &|| vec![7], &|| panic!("session already live"));
        assert_eq!(caps["sessionId"], "S1");
        assert_eq!(caps["transactions"]["typedModelOperations"]["verified"], true);
    }
}

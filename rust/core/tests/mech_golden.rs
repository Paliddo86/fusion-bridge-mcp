//! Every case under tests/golden/mech_*.json was produced by the add-in's Python admission layer
//! (tools/gen-mech-goldens.py); the Rust port must answer the same, down to the journal bytes.
use fusion_core::mech::{canonical, history::History, journal::{self, Journal, JournalError, Row, Table, Tx}, validate};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::sync::{Arc, Mutex};

fn golden(name: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap()
}

fn schema() -> Map<String, Value> {
    let p = format!("{}/../../addin/AdomBridge/commands/mechanical/schema.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn canonical_json_matches_python_json_dumps() {
    for case in golden("mech_canonical.json")["cases"].as_array().unwrap() {
        let raw = canonical::encode(&case["value"]);
        assert_eq!(String::from_utf8(raw.clone()).unwrap(), case["canonical"].as_str().unwrap(), "value {}", case["value"]);
        assert_eq!(journal::sha256_hex(&raw), case["sha256"].as_str().unwrap());
    }
}

#[test]
fn prepare_request_matches_every_python_outcome() {
    let g = golden("mech_validation.json");
    let schema = schema();
    let session = g["session"].as_str().unwrap();
    let (mut n, mut failures) = (0, vec![]);
    for case in g["cases"].as_array().unwrap() {
        n += 1;
        let got = validate::prepare_request(&case["args"], &schema, Some(session));
        let ok = match (&got, case.get("ok"), case.get("error")) {
            (Ok((frozen, write)), Some(want), None) => String::from_utf8(canonical::encode(frozen)).unwrap() == want["canonical"].as_str().unwrap() && Some(*write) == want["write"].as_bool(),
            (Err(e), None, Some(want)) => e.code == want["code"] && e.message == want["message"] && Value::Object(e.details.clone()) == want["details"],
            _ => false,
        };
        if !ok { failures.push(format!("args {}\n  rust:   {:?}\n  python: {}", case["args"], got, case.get("ok").or(case.get("error")).unwrap())); }
    }
    assert!(failures.is_empty(), "{} of {n} cases differ; first:\n{}", failures.len(), failures.iter().take(3).cloned().collect::<Vec<_>>().join("\n"));
}

#[test]
fn capabilities_and_writes_match_the_add_in() {
    let g = golden("mech_capabilities.json");
    let schema = schema();
    let want = &g["capabilities"];
    let got = validate::capabilities(Some("<session>"), &Value::Object(schema.clone()), &want["transactions"]["typedModelOperations"]);
    assert_eq!(&got, want);
    let mut writes: Vec<&str> = schema.keys().filter(|op| validate::is_write(&schema, op, &json!({}))).map(String::as_str).collect();
    writes.sort();
    assert_eq!(json!(writes), g["writes"]);
}

/// The journal's storage in memory: same rows, same commit semantics, optional injected failure
/// of the Nth row update (what the Python golden injects into its Nth transition).
struct MemTable { rows: Arc<Mutex<Option<BTreeMap<(String, String), (Vec<u8>, String)>>>>, updates: Arc<Mutex<u32>>, fail_on: Option<u32> }
struct MemTx { rows: Arc<Mutex<Option<BTreeMap<(String, String), (Vec<u8>, String)>>>>, work: BTreeMap<(String, String), (Vec<u8>, String)>, updates: Arc<Mutex<u32>>, fail_on: Option<u32> }

impl Table for MemTable {
    fn open(&self, create: bool) -> Result<Option<Box<dyn Tx + '_>>, JournalError> {
        let mut rows = self.rows.lock().unwrap();
        if rows.is_none() && !create { return Ok(None); }
        let work = rows.get_or_insert_with(BTreeMap::new).clone();
        Ok(Some(Box::new(MemTx { rows: self.rows.clone(), work, updates: self.updates.clone(), fail_on: self.fail_on })))
    }
}

impl Tx for MemTx {
    fn get(&mut self, s: &str, o: &str) -> Result<Option<(Vec<u8>, String)>, JournalError> { Ok(self.work.get(&(s.into(), o.into())).cloned()) }
    fn all(&mut self) -> Result<Vec<Row>, JournalError> { Ok(self.work.iter().map(|((s, o), (r, d))| (s.clone(), o.clone(), r.clone(), d.clone())).collect()) }
    fn count(&mut self) -> Result<u64, JournalError> { Ok(self.work.len() as u64) }
    fn put(&mut self, row: &Row) -> Result<(), JournalError> {
        let key = (row.0.clone(), row.1.clone());
        if self.work.contains_key(&key) {
            let mut n = self.updates.lock().unwrap();
            *n += 1;
            if Some(*n) == self.fail_on { return Err(JournalError::new("operation_storage_failed", "OSError: injected persistence failure")); }
        }
        self.work.insert(key, (row.2.clone(), row.3.clone()));
        Ok(())
    }
    fn commit(self: Box<Self>) -> Result<(), JournalError> { *self.rows.lock().unwrap() = Some(self.work); Ok(()) }
}

#[test]
fn history_scenarios_match_python_replies_and_journal_bytes() {
    let g = golden("mech_history.json");
    let session = g["session"].as_str().unwrap().to_string();
    for sc in g["scenarios"].as_array().unwrap() {
        let name = sc["name"].as_str().unwrap();
        let rows = Arc::new(Mutex::new(None));
        let table = MemTable { rows: rows.clone(), updates: Arc::new(Mutex::new(0)), fail_on: sc["options"]["fail_on"].as_u64().map(|n| n as u32) };
        let step = Arc::new(Mutex::new(0u32));
        let clock = move || { let mut s = step.lock().unwrap(); *s += 1; let n = *s as f64; 1792500000.0 + n * 0.125 + if *s % 5 == 0 { 0.000001 * n } else { 0.0 } };
        let mut j = Journal::new(Box::new(table), Box::new(clock));
        if let Some(m) = sc["options"]["max_records"].as_u64() { j.max_records = m; }
        let h = History::new(g["sourceVersion"].as_str().unwrap(), j);
        let cur = Some(session.as_str());
        for (i, st) in sc["steps"].as_array().unwrap().iter().enumerate() {
            let a = st["args"].as_array().unwrap();
            let got = match st["call"].as_str().unwrap() {
                "admit" => match h.admit(cur, &a[0], a[1].as_str().unwrap()) { Ok(d) => json!([d, null]), Err(r) => json!([null, r]) },
                "start" => h.start(cur, &a[0], &a[1], &a[2]).unwrap_or(Value::Null),
                "finish" => h.finish(cur, &a[0], &a[1]),
                "not_executed" => h.not_executed(cur, if a[0].is_null() { None } else { Some(&a[0]) }, &a[1]),
                "status" => h.status(cur, &a[0]),
                other => panic!("unknown call {other}"),
            };
            assert_eq!(got, st["out"], "{name} step {i} ({})", st["call"]);
        }
        let mem = rows.lock().unwrap().clone().unwrap_or_default();
        let got: Vec<Value> = mem.iter().map(|((s, o), (r, d))| json!({"sessionId": s, "operationId": o, "record": String::from_utf8(r.clone()).unwrap(), "digest": d})).collect();
        assert_eq!(json!(got), sc["rows"], "{name}: journal rows");
    }
}

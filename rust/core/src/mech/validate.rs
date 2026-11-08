//! The add-in's request admission, ported: `validate` (the schema subset the mechanical schema
//! uses) and `prepare_request` (strip caller annotations, check the op, freeze the canonical
//! request, decide whether it is a write). Messages are the Python ones verbatim; every distinct
//! outcome is pinned by tests/golden/mech_validation.json.
//!
//! Parity notes: `minLength`/`maxLength` appear in the schema but Python never enforced them, so
//! neither does this; enum membership uses Python equality (1 == 1.0 == True).

use super::canonical;
use super::journal::{self, JournalError};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct MechError { pub code: String, pub message: String, pub details: Map<String, Value> }

impl MechError {
    pub fn new(code: &str, message: impl Into<String>) -> Self { MechError { code: code.into(), message: message.into(), details: Map::new() } }
}

impl From<JournalError> for MechError {
    fn from(e: JournalError) -> Self { MechError::new(&e.code, e.message) }
}

fn require(ok: bool, code: &str, message: impl FnOnce() -> String) -> Result<(), MechError> {
    if ok { Ok(()) } else { Err(MechError::new(code, message())) }
}

/// `error_result` for a failure raised before any native work.
pub fn error_result(session: Option<&str>, e: &MechError) -> Value {
    let mut r = json!({"success": false, "sessionId": session, "errorCode": e.code, "error": e.message});
    for (k, v) in &e.details { r[k] = v.clone(); }
    r
}

fn as_num(v: &Value) -> Option<f64> {
    match v { Value::Number(n) => n.as_f64(), Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }), _ => None }
}

/// Python `==` between decoded JSON values: numbers (and bools) compare by value.
pub fn py_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64()) { (Some(p), Some(q)) => p == q, _ => x.as_f64() == y.as_f64() },
        (Value::Number(_), Value::Bool(_)) | (Value::Bool(_), Value::Number(_)) => as_num(a) == as_num(b),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| py_eq(v, w))),
        _ => a == b,
    }
}

fn py_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Bool(true)) => true,
    }
}

fn is_kind(value: &Value, kind: &str) -> Option<bool> {
    Some(match kind {
        "object" => value.is_object(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "array" => value.is_array(),
        _ => return None,
    })
}

fn cmp_gt(v: &Value, bound: &Value) -> bool { as_num(v).zip(as_num(bound)).is_some_and(|(a, b)| a > b) }
fn cmp_ge(v: &Value, bound: &Value) -> bool { as_num(v).zip(as_num(bound)).is_some_and(|(a, b)| a >= b) }
fn cmp_le(v: &Value, bound: &Value) -> bool { as_num(v).zip(as_num(bound)).is_some_and(|(a, b)| a <= b) }

pub fn validate(value: &Value, schema: &Value, path: &str) -> Result<(), MechError> {
    if let Some(all) = schema.get("oneOf").and_then(Value::as_array) {
        let mut variants: Vec<&Value> = all.iter().collect();
        if let Some(obj) = value.as_object() {
            for discriminator in ["type", "action"] {
                if let Some(d) = obj.get(discriminator) {
                    let matching: Vec<&Value> = variants.iter().copied().filter(|variant| {
                        variant.get("properties").and_then(|p| p.get(discriminator)).and_then(|p| p.get("enum")).and_then(Value::as_array)
                            .is_some_and(|e| e.iter().any(|x| py_eq(d, x)))
                    }).collect();
                    if matching.len() == 1 {
                        validate(value, matching[0], path)?;
                        variants.clear();
                        break;
                    }
                }
            }
        }
        if !variants.is_empty() {
            let successes = variants.iter().filter(|variant| validate(value, variant, path).is_ok()).count();
            require(successes == 1, "invalid_input", || format!("{path} must match exactly one supported schema variant"))?;
        }
    }
    let kind = schema.get("type").and_then(Value::as_str);
    if let Some(kind) = kind {
        match is_kind(value, kind) {
            Some(ok) => require(ok, "invalid_input", || format!("{path} must be {kind}"))?,
            // Python's `kinds[kind]` raised KeyError here
            None => return Err(MechError::new("mechanical_failed", format!("'{kind}'"))),
        }
    }
    if matches!(kind, Some("number" | "integer")) {
        if let Some(b) = schema.get("exclusiveMinimum") { require(cmp_gt(value, b), "invalid_input", || format!("{path} is too small"))?; }
        if let Some(b) = schema.get("minimum") { require(cmp_ge(value, b), "invalid_input", || format!("{path} is too small"))?; }
        if let Some(b) = schema.get("maximum") { require(cmp_le(value, b), "invalid_input", || format!("{path} is too large"))?; }
    }
    if kind == Some("array") {
        let items = value.as_array().map(Vec::as_slice).unwrap_or(&[]);
        let min = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
        let max = schema.get("maxItems").and_then(Value::as_u64).unwrap_or(10000) as usize;
        require(min <= items.len() && items.len() <= max, "invalid_input", || format!("{path} has an invalid item count"))?;
        let item_schema = schema.get("items").cloned().unwrap_or_else(|| json!({}));
        for (i, item) in items.iter().enumerate() { validate(item, &item_schema, &format!("{path}[{i}]"))?; }
    }
    if let Some(choices) = schema.get("enum").and_then(Value::as_array) {
        require(choices.iter().any(|c| py_eq(value, c)), "invalid_input", || format!("{path} has an unsupported value"))?;
    }
    if kind == Some("object") {
        let obj = value.as_object().cloned().unwrap_or_default();
        let min = schema.get("minProperties").and_then(Value::as_u64).unwrap_or(0) as usize;
        require(obj.len() >= min, "invalid_input", || format!("{path} requires at least one property"))?;
        for key in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            require(obj.contains_key(key), "invalid_input", || format!("{path}.{key} is required"))?;
        }
        let empty = Map::new();
        let props = schema.get("properties").and_then(Value::as_object).unwrap_or(&empty);
        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            require(obj.keys().all(|k| props.contains_key(k)), "invalid_input", || format!("{path} has unknown fields"))?;
        }
        for (key, v) in &obj {
            if let Some(p) = props.get(key) { validate(v, p, &format!("{path}.{key}"))?; }
        }
        if let Some(extra) = schema.get("additionalProperties").filter(|a| a.is_object()) {
            for (key, v) in &obj {
                if !props.contains_key(key) { validate(v, extra, &format!("{path}.{key}"))?; }
            }
        }
    }
    Ok(())
}

/// A write is every operation whose schema requires an `operationId` (the add-in's WRITES set,
/// pinned by the golden), plus `health` asked to recompute.
pub fn is_write(schema: &Map<String, Value>, op: &str, args: &Value) -> bool {
    let requires_id = schema.get(op).and_then(|s| s.get("required")).and_then(Value::as_array)
        .is_some_and(|r| r.iter().any(|k| k == "operationId"));
    requires_id || (op == "health" && py_truthy(args.get("recompute")))
}

/// `prepare_request`: validate and freeze user input; returns the canonical request and whether
/// it is a write. `session` is the current add-in session (None when no add-in is serving).
pub fn prepare_request(args: &Value, schema: &Map<String, Value>, session: Option<&str>) -> Result<(Value, bool), MechError> {
    let obj = args.as_object().ok_or_else(|| MechError::new("invalid_input", "Arguments must be an object."))?;
    // Authorization stays in the bridge envelope; time budgets and caller annotations never
    // alter the canonical request or its retry digest.
    let args: Map<String, Value> = obj.iter().filter(|(k, _)| k.as_str() != "caller" && k.as_str() != "_callerBudgetMs").map(|(k, v)| (k.clone(), v.clone())).collect();
    let op = args.get("op").and_then(Value::as_str).filter(|op| schema.contains_key(*op))
        .ok_or_else(|| MechError::new("unsupported_operation", "Discover supported operations with fusion_mech_capabilities."))?.to_string();
    let args = Value::Object(args);
    validate(&args, &schema[&op], "args")?;
    let props = schema[&op].get("properties").and_then(Value::as_object);
    let mut unknown: Vec<&String> = args.as_object().unwrap().keys()
        .filter(|k| !props.is_some_and(|p| p.contains_key(*k)) && !matches!(k.as_str(), "op" | "sessionId" | "operationId" | "target")).collect();
    unknown.sort();
    require(unknown.is_empty(), "invalid_input", || format!("Unknown operation arguments: {}", unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")))?;
    let write = is_write(schema, &op, &args);
    if write {
        require(session.is_some() && args.get("sessionId").and_then(Value::as_str) == session, "session_mismatch",
            || "Old-session history is readable, but native execution requires the current add-in session.".into())?;
        require(args.get("operationId").and_then(Value::as_str).is_some_and(|s| (1..=128).contains(&s.chars().count())), "operation_id_required",
            || "Every write needs a unique operationId; retries must preserve the exact request.".into())?;
    }
    let raw = journal::canonical(&args, journal::MAX_REQUEST_BYTES)?;
    let frozen: Value = serde_json::from_slice(&raw).map_err(|e| MechError::new("operation_record_invalid", e.to_string()))?;
    Ok((frozen, write))
}

/// `fusion_mech_capabilities`, served by the exe. `transactions` is the add-in's runtime probe
/// state (from its session file) and `operations` the schema the add-in executes.
pub fn capabilities(session: Option<&str>, schema: &Value, transactions: &Value) -> Value {
    json!({"success": true, "sessionId": session, "schemaVersion": 1, "units": {"length": "mm", "angle": "deg", "mass": "kg", "area": "mm2", "volume": "mm3"},
        "operations": schema, "transactions": {"genericAtomicBatch": false, "typedModelOperations": transactions,
        "parameterRecovery": "compensating expression restore with semantic readback",
        "newDesignRecovery": "discard only the new unsaved document",
        "externalEffects": "exports publish atomically; model recovery does not undo files"},
        "idempotency": {"scope": "durable history keyed by add-in session and operation ID", "capacity": journal::MAX_RECORDS,
            "storageVersion": journal::STORAGE_VERSION, "databaseByteLimit": journal::MAX_DATABASE_BYTES,
            "recordByteLimit": journal::MAX_RECORD_BYTES, "admission": "FULL SQLite commit before native dispatch",
            "restartBehavior": "old-session history is readable; native execution never replays across sessions"}})
}

pub fn canonical_bytes(v: &Value) -> Vec<u8> { canonical::encode(v) }

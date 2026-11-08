//! Rust request preparation for the Fusion mechanical bridge.
//!
//! This crate implements contracts only. Successful validation does not imply
//! that a native CAD handler exists, that durable admission happened, or that a
//! request may execute. No Python interpreter, process, Fusion object, journal,
//! native callback, or file system operation is used here.

mod operations;
mod schema;
mod wire;

pub use operations::{operation_contract, Access, OperationContract, OPERATION_CONTRACTS};
pub use schema::Schema;
use serde_json::Value;
use std::{collections::BTreeMap, sync::OnceLock};

pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
/// Bound the entire transport body before parsing, allowing at most 64 KiB of
/// caller metadata beyond the maximum stripped operation request.
pub const MAX_WIRE_BYTES: usize = MAX_REQUEST_BYTES + 64 * 1024;
pub const MAX_REQUEST_DEPTH: usize = 128;
pub const SCHEMA_JSON: &str = include_str!("../schema.json");
/// New namespace: never use these bytes to reinterpret legacy Python digests.
pub const CANONICAL_FORMAT: &str = "adom-fusion-rust-json-v1";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub code: &'static str,
    pub message: String,
}

impl Error {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn input(message: impl Into<String>) -> Self {
        Self::new("invalid_input", message)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct ValidatedSchema {
    operations: BTreeMap<String, Schema>,
    raw: Value,
}

impl ValidatedSchema {
    pub fn operation(&self, name: &str) -> Option<&Schema> {
        self.operations.get(name)
    }

    /// Contract inventory only; callers must separately advertise implemented handlers.
    pub fn contracts_json(&self) -> &Value {
        &self.raw
    }
}

pub fn validated_schema() -> Result<&'static ValidatedSchema> {
    static SCHEMA: OnceLock<Result<ValidatedSchema>> = OnceLock::new();
    SCHEMA
        .get_or_init(|| {
            let operations: BTreeMap<String, Schema> = serde_json::from_str(SCHEMA_JSON)
                .map_err(|error| Error::new("schema_invalid", error.to_string()))?;
            if operations.len() != OPERATION_CONTRACTS.len()
                || OPERATION_CONTRACTS
                    .iter()
                    .any(|contract| !operations.contains_key(contract.name))
            {
                return Err(Error::new(
                    "schema_invalid",
                    "Schema and operation classification differ",
                ));
            }
            let raw = serde_json::from_str(SCHEMA_JSON)
                .map_err(|error| Error::new("schema_invalid", error.to_string()))?;
            Ok(ValidatedSchema { operations, raw })
        })
        .as_ref()
        .map_err(Clone::clone)
}

#[derive(Debug, Clone)]
pub struct PreparedRequest {
    arguments: Value,
    contract: &'static OperationContract,
    write: bool,
    canonical: Vec<u8>,
}

impl PreparedRequest {
    pub fn operation(&self) -> &'static str {
        self.contract.name
    }
    pub fn arguments(&self) -> &Value {
        &self.arguments
    }
    pub fn is_write(&self) -> bool {
        self.write
    }
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical
    }
    pub fn canonical_format(&self) -> &'static str {
        CANONICAL_FORMAT
    }
}

/// Parse untrusted JSON before preparing a request. This additionally refuses
/// integer literals outside i64/u64, which serde_json could otherwise round to f64.
pub fn prepare_json(input: &[u8], current_session: &str) -> Result<PreparedRequest> {
    if input.len() > MAX_WIRE_BYTES {
        return Err(Error::new(
            "operation_record_too_large",
            "Wire request exceeds its byte limit.",
        ));
    }
    let value: wire::UniqueValue = serde_json::from_slice(input)
        .map_err(|error| Error::input(format!("Arguments must be finite JSON: {error}")))?;
    reject_oversized_integer_literals(input)?;
    prepare_request(value.0, current_session)
}

/// Validate an already-allocated, trusted in-process value without admitting or
/// executing it. Untrusted transport bytes must enter through `prepare_json` so
/// the wire cap applies before allocation. Transport authorization must be
/// enforced before this function is called.
pub fn prepare_request(mut arguments: Value, current_session: &str) -> Result<PreparedRequest> {
    let object = arguments
        .as_object_mut()
        .ok_or_else(|| Error::input("Arguments must be an object."))?;
    for name in ["noAutoStart", "noAutoRepair"] {
        if object.get(name).is_some_and(|value| !value.is_boolean()) {
            return Err(Error::input(format!(
                "{name} must be a Boolean transport option."
            )));
        }
    }
    for name in ["caller", "_callerBudgetMs", "noAutoStart", "noAutoRepair"] {
        object.remove(name);
    }
    let contract = object
        .get("op")
        .and_then(Value::as_str)
        .and_then(operation_contract)
        .ok_or_else(|| {
            Error::new(
                "unsupported_operation",
                "Discover supported operations with fusion_mech_capabilities.",
            )
        })?;
    check_depth(&arguments, 0)?;
    let schema = validated_schema()?
        .operation(contract.name)
        .ok_or_else(|| Error::new("schema_invalid", "Operation schema is missing"))?;
    schema.validate(&arguments, "args")?;
    let object = arguments.as_object().expect("validated request object");
    let mut unknown: Vec<_> = object
        .keys()
        .filter(|key| {
            !schema.has_property(key)
                && !["op", "sessionId", "operationId", "target"].contains(&key.as_str())
        })
        .map(String::as_str)
        .collect();
    unknown.sort_unstable();
    if !unknown.is_empty() {
        return Err(Error::input(format!(
            "Unknown operation arguments: {}",
            unknown.join(", ")
        )));
    }
    let write = contract.access == Access::Write
        || (contract.access == Access::Health
            && object.get("recompute") == Some(&Value::Bool(true)));
    if write {
        if current_session.is_empty()
            || object.get("sessionId").and_then(Value::as_str) != Some(current_session)
        {
            return Err(Error::new("session_mismatch",
                "Old-session history is readable, but native execution requires the current add-in session."));
        }
        if !object
            .get("operationId")
            .and_then(Value::as_str)
            .is_some_and(|id| (1..=128).contains(&id.chars().count()))
        {
            return Err(Error::new(
                "operation_id_required",
                "Every write needs a unique operationId; retries must preserve the exact request.",
            ));
        }
    }
    let canonical = canonical_bytes(&arguments)?;
    Ok(PreparedRequest {
        arguments,
        contract,
        write,
        canonical,
    })
}

fn check_depth(value: &Value, depth: usize) -> Result<()> {
    if depth > MAX_REQUEST_DEPTH {
        return Err(Error::new(
            "operation_record_invalid",
            "Operation record exceeds its nesting limit.",
        ));
    }
    match value {
        Value::Array(array) => {
            for value in array {
                check_depth(value, depth + 1)?;
            }
        }
        Value::Object(object) => {
            for value in object.values() {
                check_depth(value, depth + 1)?;
            }
        }
        _ => (),
    }
    Ok(())
}

fn canonical_bytes(value: &Value) -> Result<Vec<u8>> {
    // Check serialized size without building another allocation first. Sorting
    // object keys cannot alter the serialized length of this JSON value.
    struct SizeLimit(usize);
    impl std::io::Write for SizeLimit {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.0 {
                return Err(std::io::Error::other("Request byte limit exceeded"));
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(SizeLimit(MAX_REQUEST_BYTES), value).map_err(|_| {
        Error::new(
            "operation_record_too_large",
            "Operation record exceeds its durable byte limit.",
        )
    })?;
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                let fields: BTreeMap<_, _> = object.iter().collect();
                Value::Object(
                    fields
                        .into_iter()
                        .map(|(key, value)| (key.clone(), sorted(value)))
                        .collect(),
                )
            }
            Value::Array(array) => Value::Array(array.iter().map(sorted).collect()),
            value => value.clone(),
        }
    }
    let bytes = serde_json::to_vec(&sorted(value))
        .map_err(|error| Error::new("operation_record_invalid", error.to_string()))?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(Error::new(
            "operation_record_too_large",
            "Operation record exceeds its durable byte limit.",
        ));
    }
    Ok(bytes)
}

fn reject_oversized_integer_literals(input: &[u8]) -> Result<()> {
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'"' {
            index += 1;
            while index < input.len() {
                if input[index] == b'\\' {
                    index += 2;
                } else if input[index] == b'"' {
                    index += 1;
                    break;
                } else {
                    index += 1;
                }
            }
        } else if input[index] == b'-' || input[index].is_ascii_digit() {
            let start = index;
            while index < input.len()
                && (input[index].is_ascii_digit()
                    || [b'-', b'+', b'.', b'e', b'E'].contains(&input[index]))
            {
                index += 1;
            }
            let literal = &input[start..index];
            if !literal.iter().any(|byte| [b'.', b'e', b'E'].contains(byte)) {
                // serde_json already validated UTF-8 and JSON syntax.
                let text = std::str::from_utf8(literal).expect("valid JSON number");
                if text.parse::<i64>().is_err() && text.parse::<u64>().is_err() {
                    return Err(Error::input(
                        "Integer literal is outside the exact supported i64/u64 range.",
                    ));
                }
            }
        } else {
            index += 1;
        }
    }
    Ok(())
}

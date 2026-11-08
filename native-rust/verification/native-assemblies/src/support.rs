// Transport and terminal-durability helpers reused from the qualified r8 client.
// This client permits one new document, one fixed line and one authorized H/V relation and one driving length dimension.
// Any failed oracle stops without automatic cleanup, retry, or host controls.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, String>;
const CONTRACT: &str = "native-root-assembly-v1";
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const MAX_REQUESTS: usize = 96;
const CAMPAIGN_LIMIT: Duration = Duration::from_secs(300);
fn is_effect(body: &Value) -> bool {
    matches!(
        body["op"].as_str(),
        Some("new_design" | "component_create" | "occurrence_insert" | "component_metadata")
    )
}
fn ensure(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value[field]
        .as_str()
        .ok_or_else(|| format!("missing string field {field}"))
}
fn read_json(path: &Path) -> Result<Value> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take((MAX_RESPONSE + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    ensure(bytes.len() <= MAX_RESPONSE, "input file too large")?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}
fn write_new(path: &Path, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    ensure(bytes.len() <= 4 * MAX_RESPONSE, "receipt size bound")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|v| !v.is_zero())
        .ok_or("absolute HTTP deadline exceeded".into())
}
#[derive(Debug)]
struct Response {
    code: u16,
    body: Value,
}
fn parse_response(bytes: &[u8]) -> Result<Response> {
    let split = bytes
        .windows(4)
        .position(|v| v == b"\r\n\r\n")
        .ok_or("missing HTTP header terminator")?;
    ensure(split <= 8192, "response headers too large")?;
    let header = std::str::from_utf8(&bytes[..split]).map_err(|_| "invalid response headers")?;
    ensure(header.is_ascii(), "non-ASCII HTTP headers")?;
    let mut lines = header.split("\r\n");
    let status: Vec<_> = lines
        .next()
        .ok_or("missing HTTP status")?
        .split(' ')
        .collect();
    ensure(
        status.len() >= 3 && status[0] == "HTTP/1.1",
        "invalid HTTP status line",
    )?;
    let code = status[1]
        .parse::<u16>()
        .map_err(|_| "invalid HTTP status code")?;
    let mut headers = BTreeMap::new();
    for line in lines {
        let (key, value) = line.split_once(':').ok_or("invalid response header")?;
        ensure(
            !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "invalid header name",
        )?;
        ensure(
            !value.bytes().any(|b| b < 32 || b == 127),
            "invalid header value",
        )?;
        ensure(
            headers
                .insert(key.to_ascii_lowercase(), value.trim())
                .is_none(),
            "duplicate response header",
        )?;
    }
    ensure(
        !headers.contains_key("transfer-encoding"),
        "transfer-encoding refused",
    )?;
    ensure(
        headers.get("content-type") == Some(&"application/json"),
        "JSON content type required",
    )?;
    let length = headers
        .get("content-length")
        .ok_or("missing content length")?;
    ensure(
        !length.is_empty() && length.bytes().all(|b| b.is_ascii_digit()),
        "invalid content length",
    )?;
    let length = length
        .parse::<usize>()
        .map_err(|_| "invalid content length")?;
    ensure(
        length <= MAX_RESPONSE && bytes.len() - split - 4 == length,
        "response length mismatch/budget",
    )?;
    let body = serde_json::from_slice(&bytes[split + 4..]).map_err(|_| "invalid JSON response")?;
    Ok(Response { code, body })
}
fn exchange(address: SocketAddr, bytes: &[u8], deadline: Instant) -> Result<Response> {
    let mut stream =
        TcpStream::connect_timeout(&address, remaining(deadline)?.min(Duration::from_secs(2)))
            .map_err(|e| e.to_string())?;
    let mut sent = 0;
    while sent < bytes.len() {
        stream
            .set_write_timeout(Some(remaining(deadline)?))
            .map_err(|e| e.to_string())?;
        let n = stream.write(&bytes[sent..]).map_err(|e| e.to_string())?;
        ensure(n > 0, "socket closed during request")?;
        sent += n;
    }
    let mut response = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(|e| e.to_string())?;
        let n = stream.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        ensure(
            response.len() + n <= MAX_RESPONSE + 8192,
            "response byte budget exceeded",
        )?;
        response.extend_from_slice(&chunk[..n]);
    }
    parse_response(&response)
}
fn redact(value: Value, token: &str) -> Value {
    match value {
        Value::String(s) => Value::String(s.replace(token, "[REDACTED]")),
        Value::Array(rows) => Value::Array(rows.into_iter().map(|v| redact(v, token)).collect()),
        Value::Object(rows) => Value::Object(
            rows.into_iter()
                .map(|(key, value)| {
                    let sensitive = matches!(
                        key.to_ascii_lowercase().as_str(),
                        "token"
                            | "authorization"
                            | "access_token"
                            | "refresh_token"
                            | "accesstoken"
                            | "refreshtoken"
                    );
                    (
                        key.replace(token, "[REDACTED]"),
                        if sensitive {
                            json!("[REDACTED]")
                        } else {
                            redact(value, token)
                        },
                    )
                })
                .collect(),
        ),
        other => other,
    }
}

fn active_id(documents: &BTreeMap<String, Value>) -> Result<&str> {
    let active: Vec<_> = documents
        .iter()
        .filter(|(_, d)| d["active"] == true)
        .collect();
    ensure(active.len() == 1, "unique active document required")?;
    Ok(active[0].0)
}
fn current_document_id(id: &str, epoch: &str) -> bool {
    id.strip_prefix(&format!("d:{epoch}:"))
        .is_some_and(|serial| {
            serial.len() == 16
                && serial
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                && serial.bytes().any(|b| b != b'0')
        })
}
fn no_admission(status: &Value) -> Result<()> {
    let durable = &status["durable"];
    ensure(
        durable.is_object()
            && durable.get("record") == Some(&Value::Null)
            && durable["state"] == "unknown"
            && durable["outcome"] == "unknown"
            && durable["historical"] == false
            && durable["reconciliationRequired"] == true
            && status["authority"] == "unknown"
            && status["historical"] == false
            && status["reconciliationRequired"] == true
            && status.get("observed") == Some(&Value::Null)
            && status.get("storageError") == Some(&Value::Null)
            && status.get("retentionError") == Some(&Value::Null)
            && status["nativeExecutionAuthorized"] == false
            && status["effectJournalConnected"] == true
            && status["effectJournalConfigured"] == true,
        "unsupported operation absence was not established by a successful empty journal lookup",
    )
}
fn document_map(snapshot: &Value) -> Result<BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    for doc in snapshot["documents"]
        .as_array()
        .ok_or("missing document list")?
    {
        ensure(
            out.insert(text(doc, "documentId")?.to_owned(), doc.clone())
                .is_none(),
            "duplicate opaque document ID",
        )?;
    }
    Ok(out)
}
fn terminal(response: &Response, request: &Value, success: bool, replayed: bool) -> Result<()> {
    ensure(
        response.code == if success { 200 } else { 400 },
        "expected durable terminal HTTP status",
    )?;
    let body = &response.body;
    let status = &body["operationStatus"];
    let record = &status["durable"]["record"];
    ensure(
        body["success"] == success && body["replayed"] == replayed,
        "terminal success/replay flags differ",
    )?;
    ensure(
        body["epoch"] == request["sessionId"]
            && body["sessionId"] == request["sessionId"]
            && body["operationId"] == request["operationId"],
        "terminal transport/operation identity differs",
    )?;
    ensure(
        status["authority"] == "durable_terminal"
            && status["historical"] == false
            && status["reconciliationRequired"] == false
            && status["sessionId"] == request["sessionId"]
            && status["operationId"] == request["operationId"],
        "terminal durability unavailable",
    )?;
    ensure(
        record["sessionId"] == request["sessionId"]
            && record["operationId"] == request["operationId"]
            && record["operation"] == request["op"],
        "durable record aliases another request",
    )?;
    let phase = record["phase"]
        .as_str()
        .ok_or("missing raw durable phase")?;
    ensure(
        status["durable"]["state"] == phase
            && status["durable"]["outcome"] == "recorded"
            && if success {
                phase == "succeeded"
            } else {
                matches!(phase, "not_executed" | "failed")
            },
        "raw durable phase/state/outcome does not support the terminal result",
    )?;
    let prepared =
        fusion_mechanical_core::prepare_request(request.clone(), text(request, "sessionId")?)
            .map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    hash.update(fusion_mechanical_core::CANONICAL_FORMAT.as_bytes());
    hash.update([0]);
    hash.update(prepared.canonical_bytes());
    let digest = format!("{:x}", hash.finalize());
    ensure(
        record["requestDigest"] == digest,
        "durable digest differs from submitted request",
    )?;
    let mut result = body.clone();
    for field in [
        "epoch",
        "sessionId",
        "operationId",
        "operationStatus",
        "replayed",
    ] {
        result.as_object_mut().ok_or("result object")?.remove(field);
    }
    ensure(
        record["result"] == result,
        "HTTP operation result differs from durable authority",
    )
}

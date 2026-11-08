//! Bounded authenticated local HTTP. Bodies are accepted only for the native
//! mechanical route after transport authentication; no connection reuse/scripts.
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpStream,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub const MAX_HEADER_BYTES: usize = 8192;
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_millis(100);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Health,
    Documents,
    Capabilities,
    Mechanical,
    OperationStatus { session: String, operation: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub route: Route,
    pub budget: Duration,
    pub content_length: usize,
    pub body: Vec<u8>,
    pub mechanical_contract: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub status: u16,
    pub code: &'static str,
}
fn error(status: u16, code: &'static str) -> Error {
    Error { status, code }
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

pub fn parse(bytes: &[u8], host: &str, token: &str, epoch: &str) -> Result<Request, Error> {
    if bytes.len() > MAX_HEADER_BYTES {
        return Err(error(431, "headers_too_large"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| error(400, "invalid_headers"))?;
    if !text.is_ascii() || !text.ends_with("\r\n\r\n") {
        return Err(error(400, "invalid_headers"));
    }
    let mut lines = text[..text.len() - 4].split("\r\n");
    let first = lines.next().ok_or(error(400, "invalid_request"))?;
    let parts: Vec<_> = first.split(' ').collect();
    if parts.len() != 3 || parts[2] != "HTTP/1.1" {
        return Err(error(400, "invalid_request"));
    }
    let mechanical = parts[0] == "POST" && parts[1] == "/v1/mechanical";
    if parts[0] != "GET" && !mechanical {
        return Err(error(405, "read_only_method"));
    }
    let mut headers = BTreeMap::new();
    for line in lines {
        let (key, value) = line.split_once(':').ok_or(error(400, "invalid_headers"))?;
        if key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.bytes().any(|b| b < 32 || b == 127)
        {
            return Err(error(400, "invalid_headers"));
        }
        if headers
            .insert(key.to_ascii_lowercase(), value.trim())
            .is_some()
        {
            return Err(error(400, "duplicate_header"));
        }
    }
    if headers.get("host") != Some(&host) {
        return Err(error(403, "host_refused"));
    }
    if headers.contains_key("origin") {
        return Err(error(403, "browser_origin_refused"));
    }
    for forbidden in ["transfer-encoding", "upgrade", "expect"] {
        if headers.contains_key(forbidden) {
            return Err(error(400, "unsupported_http_framing"));
        }
    }
    if !mechanical && headers.get("content-length").is_some_and(|v| *v != "0") {
        return Err(error(400, "body_refused"));
    }
    let expected_auth = format!("Bearer {token}");
    if !headers
        .get("authorization")
        .is_some_and(|value| constant_eq(value.as_bytes(), expected_auth.as_bytes()))
    {
        return Err(error(401, "authentication_required"));
    }
    if headers.get("x-adom-fusion-epoch") != Some(&epoch) {
        return Err(error(409, "stale_runtime"));
    }
    let content_length = if mechanical {
        if !matches!(
            headers.get("x-adom-mechanical-contract"),
            Some(
                &"native-parameters-v1"
                    | &"native-lifecycle-v1"
                    | &"native-bom-v1"
                    | &"native-sketch-v1"
                    | &"native-sketch-geometry-v1"
                    | &"native-extrude-v1"
                    | &"native-sketch-entities-v1"
                    | &"native-material-catalog-v1"
                    | &"native-purchased-discovery-v1"
                    | &"native-root-physical-v1"
                    | &"native-sketch-line-state-v1"
                    | &"native-sketch-relations-v1"
                    | &"native-sketch-dimensions-v1"
                    | &"native-root-assembly-v1"
                    | &"native-root-occurrence-translation-v1"
            )
        ) {
            return Err(error(409, "native_contract_required"));
        }
        if headers.get("content-type") != Some(&"application/json") {
            return Err(error(415, "json_content_type_required"));
        }
        let raw = headers
            .get("content-length")
            .ok_or(error(411, "content_length_required"))?;
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(error(400, "invalid_content_length"));
        }
        let length = raw
            .parse::<usize>()
            .map_err(|_| error(413, "body_too_large"))?;
        if length == 0 || length > fusion_mechanical_core::MAX_WIRE_BYTES {
            return Err(error(413, "body_too_large"));
        }
        length
    } else {
        0
    };
    let budget = match headers.get("x-adom-timeout-ms") {
        None => Duration::from_millis(1500),
        Some(value) => {
            if !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(error(400, "invalid_timeout"));
            }
            let ms = value
                .parse::<u64>()
                .map_err(|_| error(400, "invalid_timeout"))?;
            if !(25..=2000).contains(&ms) {
                return Err(error(400, "invalid_timeout"));
            }
            Duration::from_millis(ms)
        }
    };
    let route = match parts[1] {
        "/v1/health" => Route::Health,
        "/v1/documents" => Route::Documents,
        "/v1/capabilities" if parts[0] == "GET" => Route::Capabilities,
        "/v1/mechanical" if mechanical => Route::Mechanical,
        path if path.starts_with("/v1/operations/") => {
            let selectors: Vec<_> = path["/v1/operations/".len()..].split('/').collect();
            if selectors.len() != 2 || !selectors.iter().all(|id| status_selector(id)) {
                return Err(error(400, "invalid_operation_selector"));
            }
            Route::OperationStatus {
                session: selectors[0].into(),
                operation: selectors[1].into(),
            }
        }
        _ => return Err(error(404, "unsupported_route")),
    };
    Ok(Request {
        route,
        budget,
        content_length,
        body: Vec::new(),
        mechanical_contract: headers
            .get("x-adom-mechanical-contract")
            .map(|v| (*v).to_owned()),
    })
}

// Deliberately narrower than the journal's Unicode identifiers. This protocol
// performs no percent decoding, query parsing, filesystem lookup or fallback.
pub(crate) fn status_selector(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}

pub fn read(
    stream: &mut TcpStream,
    stop: &AtomicBool,
    host: &str,
    token: &str,
    epoch: &str,
) -> Result<Request, Error> {
    read_tracking(stream, stop, host, token, epoch, &mut 0)
}

/// Preserve the wire response while recording evidence that stop interrupted
/// a header already partly received. This is observation, not a parser hold.
pub(crate) fn read_tracking(
    stream: &mut TcpStream,
    stop: &AtomicBool,
    host: &str,
    token: &str,
    epoch: &str,
    stopped_header_bytes: &mut usize,
) -> Result<Request, Error> {
    *stopped_header_bytes = 0;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|_| error(500, "socket_setup_failed"))?;
    receive_header(stream, stop, host, token, epoch, stopped_header_bytes)
}

fn receive_header(
    stream: &mut impl Read,
    stop: &AtomicBool,
    host: &str,
    token: &str,
    epoch: &str,
    stopped_header_bytes: &mut usize,
) -> Result<Request, Error> {
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut bytes = Vec::with_capacity(1024);
    loop {
        if stop.load(Ordering::Acquire) {
            *stopped_header_bytes = bytes.len();
            return Err(error(503, "runtime_stopping"));
        }
        if Instant::now() >= deadline {
            return Err(error(408, "header_timeout"));
        }
        if bytes.len() >= MAX_HEADER_BYTES {
            return Err(error(431, "headers_too_large"));
        }
        let mut chunk = [0u8; 1024];
        let room = chunk.len().min(MAX_HEADER_BYTES - bytes.len());
        match stream.read(&mut chunk[..room]) {
            Ok(0) => return Err(error(400, "incomplete_request")),
            Ok(count) => bytes.extend_from_slice(&chunk[..count]),
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue
            }
            Err(_) => return Err(error(400, "read_failed")),
        }
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let mut request = parse(&bytes[..end + 4], host, token, epoch)?;
            let tail = &bytes[end + 4..];
            if tail.len() > request.content_length {
                return Err(error(400, "body_or_pipeline_refused"));
            }
            request.body = Vec::with_capacity(request.content_length);
            request.body.extend_from_slice(tail);
            while request.body.len() < request.content_length {
                if stop.load(Ordering::Acquire) {
                    return Err(error(503, "runtime_stopping"));
                }
                if Instant::now() >= deadline {
                    return Err(error(408, "body_timeout"));
                }
                let mut chunk = [0u8; 4096];
                let room = chunk.len().min(request.content_length - request.body.len());
                match stream.read(&mut chunk[..room]) {
                    Ok(0) => return Err(error(400, "incomplete_body")),
                    Ok(n) => request.body.extend_from_slice(&chunk[..n]),
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::Interrupted
                        ) =>
                    {
                        continue
                    }
                    Err(_) => return Err(error(400, "read_failed")),
                }
            }
            return Ok(request);
        }
    }
}

pub fn respond(
    stream: &mut TcpStream,
    status: u16,
    value: &serde_json::Value,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(value)?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(std::io::Error::other("response limit exceeded"));
    }
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        507 => "Insufficient Storage",
        _ => "Internal Server Error",
    };
    let header=format!("HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",body.len());
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    for buffer in [header.as_bytes(), body.as_slice()] {
        let mut remaining = buffer;
        while !remaining.is_empty() {
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "response deadline",
                ));
            }
            match stream.write(remaining) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "closed response stream",
                    ))
                }
                Ok(count) => remaining = &remaining[count..],
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    continue
                }
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(extra: &str) -> String {
        format!("GET /v1/documents HTTP/1.1\r\nHost: 127.0.0.1:54321\r\nAuthorization: Bearer secret\r\nX-Adom-Fusion-Epoch: epoch\r\n{extra}\r\n")
    }
    fn parse_test(s: &str) -> Result<Request, Error> {
        parse(s.as_bytes(), "127.0.0.1:54321", "secret", "epoch")
    }
    fn post_header(length: &str) -> String {
        request(&format!("X-Adom-Mechanical-Contract: native-parameters-v1\r\nContent-Type: application/json\r\nContent-Length: {length}\r\n"))
            .replace("GET /v1/documents", "POST /v1/mechanical")
    }
    #[test]
    fn mechanical_post_requires_authenticated_explicit_contract_and_bounded_body() {
        let valid = post_header("2");
        let parsed = parse_test(&valid).unwrap();
        assert_eq!(parsed.route, Route::Mechanical);
        assert_eq!(parsed.content_length, 2);
        assert_eq!(
            parsed.mechanical_contract.as_deref(),
            Some("native-parameters-v1")
        );
        let lifecycle =
            parse_test(&valid.replace("native-parameters-v1", "native-lifecycle-v1")).unwrap();
        assert_eq!(
            lifecycle.mechanical_contract.as_deref(),
            Some("native-lifecycle-v1")
        );
        assert_eq!(lifecycle.route, Route::Mechanical);
        let bom = parse_test(&valid.replace("native-parameters-v1", "native-bom-v1")).unwrap();
        assert_eq!(bom.mechanical_contract.as_deref(), Some("native-bom-v1"));
        let sketch =
            parse_test(&valid.replace("native-parameters-v1", "native-sketch-v1")).unwrap();
        assert_eq!(
            sketch.mechanical_contract.as_deref(),
            Some("native-sketch-v1")
        );
        let geometry =
            parse_test(&valid.replace("native-parameters-v1", "native-sketch-geometry-v1"))
                .unwrap();
        assert_eq!(
            geometry.mechanical_contract.as_deref(),
            Some("native-sketch-geometry-v1")
        );
        let entities =
            parse_test(&valid.replace("native-parameters-v1", "native-sketch-entities-v1"))
                .unwrap();
        assert_eq!(
            entities.mechanical_contract.as_deref(),
            Some("native-sketch-entities-v1")
        );
        let extrude =
            parse_test(&valid.replace("native-parameters-v1", "native-extrude-v1")).unwrap();
        assert_eq!(
            extrude.mechanical_contract.as_deref(),
            Some("native-extrude-v1")
        );
        for (from, to, expected) in [
            ("Bearer secret", "Bearer wrong", "authentication_required"),
            ("Epoch: epoch", "Epoch: prior", "stale_runtime"),
            ("native-parameters-v1", "legacy", "native_contract_required"),
            (
                "application/json",
                "text/plain",
                "json_content_type_required",
            ),
            ("Content-Length: 2\r\n", "", "content_length_required"),
            (
                "Content-Length: 2",
                "Content-Length: -2",
                "invalid_content_length",
            ),
            ("Content-Length: 2", "Content-Length: 0", "body_too_large"),
        ] {
            assert_eq!(
                parse_test(&valid.replace(from, to)).unwrap_err().code,
                expected
            );
        }
        for length in [
            (fusion_mechanical_core::MAX_WIRE_BYTES + 1).to_string(),
            "9".repeat(100),
        ] {
            let oversized = post_header(&length);
            assert_eq!(parse_test(&oversized).unwrap_err().code, "body_too_large");
            assert_eq!(
                parse_test(&oversized.replace("Bearer secret", "Bearer wrong"))
                    .unwrap_err()
                    .code,
                "authentication_required"
            );
        }
    }
    #[test]
    fn fragmented_mechanical_body_is_exact_and_truncation_is_refused() {
        struct Chunks(std::collections::VecDeque<Vec<u8>>);
        impl Read for Chunks {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                let Some(front) = self.0.front_mut() else {
                    return Ok(0);
                };
                let n = front.len().min(out.len());
                out[..n].copy_from_slice(&front[..n]);
                front.drain(..n);
                if front.is_empty() {
                    self.0.pop_front();
                }
                Ok(n)
            }
        }
        for (tail, expected) in [
            (vec![b"{".to_vec(), b"}".to_vec()], Ok(())),
            (vec![b"{".to_vec()], Err("incomplete_body")),
            (vec![b"{}EXTRA".to_vec()], Err("body_or_pipeline_refused")),
        ] {
            let mut chunks = if expected == Err("body_or_pipeline_refused") {
                Chunks([format!("{}{}", post_header("2"), "{}EXTRA").into_bytes()].into())
            } else {
                Chunks(
                    std::iter::once(post_header("2").into_bytes())
                        .chain(tail)
                        .collect(),
                )
            };
            let result = receive_header(
                &mut chunks,
                &AtomicBool::new(false),
                "127.0.0.1:54321",
                "secret",
                "epoch",
                &mut 0,
            );
            match expected {
                Ok(()) => assert_eq!(result.unwrap().body, b"{}"),
                Err(code) => assert_eq!(result.unwrap_err().code, code),
            }
        }
    }
    #[test]
    fn stop_during_body_prevents_completed_mechanical_request() {
        struct StopAfterHeader<'a> {
            bytes: &'a [u8],
            stop: &'a AtomicBool,
        }
        impl Read for StopAfterHeader<'_> {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                let n = self.bytes.read(out)?;
                self.stop.store(true, Ordering::Release);
                Ok(n)
            }
        }
        let stop = AtomicBool::new(false);
        let header = post_header("2");
        let mut reader = StopAfterHeader {
            bytes: header.as_bytes(),
            stop: &stop,
        };
        assert_eq!(
            receive_header(
                &mut reader,
                &stop,
                "127.0.0.1:54321",
                "secret",
                "epoch",
                &mut 0
            )
            .unwrap_err()
            .code,
            "runtime_stopping"
        );
    }
    #[test]
    fn historical_status_selectors_are_explicit_and_authenticated() {
        let valid = request("").replace("/v1/documents", "/v1/operations/old-session/op:1");
        assert_eq!(
            parse_test(&valid).unwrap().route,
            Route::OperationStatus {
                session: "old-session".into(),
                operation: "op:1".into()
            }
        );
        for path in [
            "/v1/operations/a",
            "/v1/operations/a/",
            "/v1/operations//b",
            "/v1/operations/a/b/c",
            "/v1/operations/a/..",
            "/v1/operations/./b",
            "/v1/operations/a/%62",
            "/v1/operations/a/b?session=c",
            "/v1/operations/a/b#c",
        ] {
            let bytes = request("").replace("/v1/documents", path);
            assert_eq!(
                parse_test(&bytes).unwrap_err().code,
                "invalid_operation_selector",
                "{path}"
            );
            assert_eq!(
                parse_test(&bytes.replace("Bearer secret", "Bearer wrong"))
                    .unwrap_err()
                    .code,
                "authentication_required",
                "authentication must precede selector handling"
            );
        }
        let too_long = request("").replace(
            "/v1/documents",
            &format!("/v1/operations/a/{}", "b".repeat(129)),
        );
        assert_eq!(
            parse_test(&too_long).unwrap_err().code,
            "invalid_operation_selector"
        );
        assert_eq!(
            parse_test(&valid.replace("Epoch: epoch", "Epoch: old-session"))
                .unwrap_err()
                .code,
            "stale_runtime"
        );
    }
    #[test]
    fn stop_evidence_requires_an_incomplete_header_already_received() {
        // Force stop at the input boundary, without wall-clock sleeps or a
        // diagnostic hold in the production parser.
        struct StopAfterRead<'a> {
            input: &'a [u8],
            stop: &'a AtomicBool,
        }
        impl Read for StopAfterRead<'_> {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                let n = self.input.read(output)?;
                self.stop.store(true, Ordering::Release);
                Ok(n)
            }
        }
        let full = request("");
        for (input, stopped_before_read, expected_bytes, completes) in [
            (
                b"GET /v1/documents HTTP/1.1\r\n".as_slice(),
                false,
                28,
                false,
            ),
            (full.as_bytes(), false, 0, true),
            (full.as_bytes(), true, 0, false),
        ] {
            let stop = AtomicBool::new(stopped_before_read);
            let mut reader = StopAfterRead { input, stop: &stop };
            let mut observed = 0;
            let result = receive_header(
                &mut reader,
                &stop,
                "127.0.0.1:54321",
                "secret",
                "epoch",
                &mut observed,
            );
            assert_eq!(observed, expected_bytes);
            if completes {
                assert!(result.is_ok());
            } else {
                assert_eq!(result.unwrap_err().code, "runtime_stopping");
            }
        }
    }
    #[test]
    fn authenticates_and_bounds_read_only_routes() {
        assert_eq!(
            parse_test(&request("")).unwrap(),
            Request {
                route: Route::Documents,
                budget: Duration::from_millis(1500),
                content_length: 0,
                body: Vec::new(),
                mechanical_contract: None,
            }
        );
        assert_eq!(
            parse_test(&request("X-Adom-Timeout-Ms: 25\r\n"))
                .unwrap()
                .budget,
            Duration::from_millis(25)
        );
        for timeout in ["0", "24", "2001", "-1", "1.5", "9999999999999999999999999"] {
            assert_eq!(
                parse_test(&request(&format!("X-Adom-Timeout-Ms: {timeout}\r\n")))
                    .unwrap_err()
                    .code,
                "invalid_timeout"
            );
        }
    }
    #[test]
    fn refuses_browser_cross_origin_and_wrong_endpoint_authority() {
        assert_eq!(
            parse_test(&request("Origin: null\r\n")).unwrap_err().code,
            "browser_origin_refused"
        );
        assert_eq!(
            parse_test(&request("").replace("127.0.0.1:54321", "attacker.invalid"))
                .unwrap_err()
                .code,
            "host_refused"
        );
        assert_eq!(
            parse_test(&request("").replace("Bearer secret", "Bearer other!"))
                .unwrap_err()
                .code,
            "authentication_required"
        );
        assert_eq!(
            parse_test(&request("").replace("Epoch: epoch", "Epoch: old"))
                .unwrap_err()
                .code,
            "stale_runtime"
        );
    }
    #[test]
    fn ambiguous_http_framing_and_duplicates_fail_closed() {
        for extra in [
            "Transfer-Encoding: chunked\r\n",
            "Upgrade: websocket\r\n",
            "Expect: 100-continue\r\n",
        ] {
            assert_eq!(
                parse_test(&request(extra)).unwrap_err().code,
                "unsupported_http_framing"
            );
        }
        for extra in [
            "authorization: Bearer other\r\n",
            "HOST: 127.0.0.1:54321\r\n",
        ] {
            assert_eq!(
                parse_test(&request(extra)).unwrap_err().code,
                "duplicate_header"
            );
        }
        assert_eq!(
            parse_test(&request("Content-Length: 1\r\n"))
                .unwrap_err()
                .code,
            "body_refused"
        );
    }
    #[test]
    fn no_mutations_scripts_queries_or_absolute_urls() {
        assert_eq!(
            parse_test(&request("").replace("GET ", "POST "))
                .unwrap_err()
                .status,
            405
        );
        for path in [
            "/v1/mechanical",
            "/v1/script",
            "/v1/documents?x=1",
            "http://127.0.0.1:54321/v1/documents",
        ] {
            assert_eq!(
                parse_test(&request("").replace("/v1/documents", path))
                    .unwrap_err()
                    .status,
                404
            );
        }
    }
    #[test]
    fn rejects_oversized_and_control_character_headers() {
        assert_eq!(
            parse_test(&request(&format!("X-Large: {}\r\n", "a".repeat(8192))))
                .unwrap_err()
                .status,
            431
        );
        for bad in ["X-A: a\0b\r\n", " X-A: a\r\n", "X-A: \té\r\n"] {
            assert_eq!(parse_test(&request(bad)).unwrap_err().status, 400);
        }
    }
}

#[cfg(test)]
mod material_contract_test {
    use super::*;
    #[test]
    fn parses_material_catalog_contract_without_weakening_unknown_header_refusal() {
        let bytes=b"POST /v1/mechanical HTTP/1.1\r\nHost: 127.0.0.1:54321\r\nAuthorization: Bearer secret\r\nX-Adom-Fusion-Epoch: epoch\r\nContent-Type: application/json\r\nContent-Length: 2\r\nX-Adom-Mechanical-Contract: native-material-catalog-v1\r\n\r\n";
        let v = parse(bytes, "127.0.0.1:54321", "secret", "epoch");
        assert_eq!(
            v.unwrap().mechanical_contract.as_deref(),
            Some("native-material-catalog-v1")
        );
        let unknown = String::from_utf8(bytes.to_vec()).unwrap().replace(
            "native-material-catalog-v1",
            "native-material-catalog-future",
        );
        assert!(parse(unknown.as_bytes(), "127.0.0.1:54321", "secret", "epoch").is_err());
    }
}

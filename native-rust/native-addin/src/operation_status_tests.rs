use super::*;
use crate::service::{Queue, Service, Stats};
use fusion_mechanical_core::prepare_request;
use fusion_mechanical_journal::{Admission, DispatchOutcome};
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

const TOKEN: &str = "bounded-native-status-test-authentication";
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let mut bytes = [0u8; 16];
        getrandom::getrandom(&mut bytes).unwrap();
        Self(std::env::temp_dir().join(format!(
            "fusion-native-status-{}-{}",
            std::process::id(),
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        )))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn seed(directory: &Directory) -> Value {
    let journal = Journal::open_at(
        &directory.0,
        "fixture-history",
        "synthetic-status-test",
        Limits::default(),
        100,
    )
    .unwrap();
    let mut terminal = Value::Null;
    for op in ["complete", "interrupted"] {
        let request = prepare_request(json!({"op":"exploded_preview_sample","sessionId":"fixture-history","operationId":op,
            "target":{"documentId":"synthetic","revision":"fixture"},"previewId":"p","expectedGeneration":0,"sampleIndex":0}), "fixture-history").unwrap();
        let Admission::Created(admission) = journal.admit(&request, 101).unwrap() else {
            panic!()
        };
        let DispatchOutcome::Claimed(dispatch) = journal.dispatch(&admission, op, 102).unwrap()
        else {
            panic!()
        };
        assert!(
            journal
                .start(&dispatch, json!({"synthetic":true}), 103)
                .unwrap()
                .claimed
        );
        if op == "complete" {
            terminal = serde_json::to_value(
                journal
                    .finish(&dispatch, json!({"success":true,"fixtureOnly":true}), 104)
                    .unwrap()
                    .record,
            )
            .unwrap();
        }
    }
    terminal
}
fn request(server: &Service, epoch: &str, route: &str) -> (u16, Value) {
    let mut socket = TcpStream::connect_timeout(&server.address(), Duration::from_secs(2)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(4)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(socket, "GET {route} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {TOKEN}\r\nX-Adom-Fusion-Epoch: {epoch}\r\n\r\n", server.address()).unwrap();
    let mut bytes = Vec::new();
    socket
        .take((crate::http::MAX_RESPONSE_BYTES + 8193) as u64)
        .read_to_end(&mut bytes)
        .unwrap();
    let split = bytes.windows(4).position(|b| b == b"\r\n\r\n").unwrap();
    let code = std::str::from_utf8(&bytes[..split])
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (code, serde_json::from_slice(&bytes[split + 4..]).unwrap())
}
fn server(reader: Arc<OperationStatusReader>, queue: Arc<Queue>) -> Service {
    Service::start_with_status(
        TcpListener::bind("127.0.0.1:0").unwrap(),
        queue,
        TOKEN.into(),
        Arc::new(|| panic!("status must never wake the SDK")),
        Arc::new(Stats::default()),
        Some(reader),
    )
    .unwrap()
}
fn ready(server: &Service, epoch: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (_, health) = request(server, epoch, "/v1/health");
        if health["journal"]["state"] == "ready" {
            break;
        }
        assert_eq!(health["journal"]["state"], "initializing", "{health}");
        assert!(Instant::now() < deadline, "journal initialization deadline");
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn historical_and_missing_status_never_dispatch_and_busy_does_not_block_health() {
    let directory = Directory::new();
    let terminal = seed(&directory);
    let queue = Queue::new(4).unwrap();
    let epoch = queue.epoch().to_string();
    let reader = OperationStatusReader::new(directory.0.clone(), epoch.clone());
    let mut service = server(Arc::clone(&reader), Arc::clone(&queue));
    ready(&service, &epoch);
    let (code, complete) = request(&service, &epoch, "/v1/operations/fixture-history/complete");
    assert_eq!(code, 200);
    assert_eq!(complete["authority"], "durable_terminal");
    assert_eq!(complete["durable"]["record"], terminal);
    assert_eq!(complete["historical"], true);
    assert_eq!(complete["nativeExecutionAuthorized"], false);
    let (_, interrupted) = request(
        &service,
        &epoch,
        "/v1/operations/fixture-history/interrupted",
    );
    assert_eq!(interrupted["durable"]["state"], "interrupted");
    assert_eq!(interrupted["authority"], "unknown");
    assert_eq!(interrupted["reconciliationRequired"], true);
    let (_, missing) = request(&service, &epoch, &format!("/v1/operations/{epoch}/missing"));
    assert_eq!(missing["authority"], "unknown");
    assert_eq!(missing["durable"]["record"], Value::Null);
    assert_eq!(missing["nativeExecutionAuthorized"], false);
    {
        let _busy = reader.lookup.lock().unwrap();
        let (code, reply) = request(&service, &epoch, "/v1/operations/fixture-history/complete");
        assert_eq!(code, 503);
        assert_eq!(reply["errorCode"], "operation_status_busy");
        let (_, health) = request(&service, &epoch, "/v1/health");
        assert_eq!(health["dispatchReady"], true);
        assert_eq!(health["stats"]["snapshotCalls"], 0);
        assert_eq!(health["stats"]["nativeWakeCalls"], 0);
    }
    assert_eq!(queue.outstanding(), 0);
    assert!(service.stop());
}
#[test]
fn failed_background_journal_does_not_disable_document_dispatch_or_expose_paths() {
    let directory = Directory::new();
    fs::create_dir(&directory.0).unwrap();
    let bad = directory.0.join("secret-fixture-path");
    fs::write(&bad, b"not a directory").unwrap();
    let queue = Queue::new(4).unwrap();
    let epoch = queue.epoch().to_string();
    let reader = OperationStatusReader::new(bad.clone(), epoch.clone());
    let mut service = server(reader, queue);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (_, health) = request(&service, &epoch, "/v1/health");
        if health["journal"]["state"] == "unavailable" {
            assert_eq!(health["dispatchReady"], true);
            assert!(!health.to_string().contains("secret-fixture-path"));
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    let (code, reply) = request(&service, &epoch, &format!("/v1/operations/{epoch}/missing"));
    assert_eq!(code, 503);
    assert_eq!(reply["errorCode"], "operation_status_unavailable");
    assert!(!reply.to_string().contains("secret-fixture-path"));
    assert!(service.stop());
}
#[test]
fn current_transport_epoch_must_match_owned_journal_epoch() {
    let directory = Directory::new();
    let queue = Queue::new(4).unwrap();
    let result = Service::start_with_status(
        TcpListener::bind("127.0.0.1:0").unwrap(),
        queue,
        TOKEN.into(),
        Arc::new(|| false),
        Arc::new(Stats::default()),
        Some(OperationStatusReader::new(
            directory.0.clone(),
            "wrong-epoch".into(),
        )),
    );
    assert!(result.is_err());
    assert!(!directory.0.exists());
}

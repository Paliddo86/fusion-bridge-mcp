#[test]
#[cfg(not(feature = "native-wake-probe"))]
fn relations_headers_admission_unknown_reads_and_no_effect_journal() {
    let queue = Queue::new(4).unwrap();
    let epoch = queue.epoch().to_string();
    let directory = std::env::temp_dir().join(format!(
        "fusion-r18-relation-{}-{epoch}",
        std::process::id()
    ));
    let reader = OperationStatusReader::new(directory.clone(), epoch.clone());
    let wakes = Arc::new(AtomicU64::new(0));
    let count = wakes.clone();
    let mut server = Service::start_with_operations(
        TcpListener::bind("127.0.0.1:0").unwrap(),
        queue.clone(),
        TOKEN.into(),
        Arc::new(move || {
            count.fetch_add(1, Ordering::Relaxed);
            true
        }),
        Arc::new(Stats::default()),
        reader.clone(),
    )
    .unwrap();
    let address = server.address();
    let end = Instant::now() + Duration::from_secs(5);
    while reader.health()["connected"] != true {
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(1));
    }
    let doc = format!("d:{epoch}:0000000000000001");
    let sketch = format!("s:{doc}:0000000000000001");
    let request = json!({"op":"native_sketch_relations","sessionId":epoch,"operationId":"relation-read","target":{"documentId":doc},"sketchId":sketch,"entityId":format!("e:{sketch}:0000000000000001")});
    let denied = post_mechanical(address, &epoch, crate::physical::CONTRACT, request.clone());
    assert_eq!(denied.0, 409);
    assert_eq!(denied.1["errorCode"], "native_relations_contract_required");
    let (book, backend) = crate::sketches::relations::tests::setup();
    let mutation = crate::sketches::relations::tests::request(
        &book,
        &backend,
        &epoch,
        "old-write-contract",
        "horizontal",
    );
    let denied = post_mechanical(
        address,
        &epoch,
        crate::sketches::line_state::CONTRACT,
        mutation.arguments().clone(),
    );
    assert_eq!(denied.0, 409);
    assert_eq!(denied.1["errorCode"], "native_relations_contract_required");
    let mut bad = request.clone();
    bad["target"]["revision"] = json!("sketch-projection-v1:wrong");
    let denied = post_mechanical(address, &epoch, crate::sketches::relations::CONTRACT, bad);
    assert_eq!(denied.0, 400);
    assert_eq!(denied.1["errorCode"], "native_relation_revision_required");
    assert_eq!(wakes.load(Ordering::Relaxed), 0);
    assert!(!queue.has_queued());
    for unknown in [false, true] {
        let client_epoch = epoch.clone();
        let r = request.clone();
        let client = thread::spawn(move || {
            post_mechanical(
                address,
                &client_epoch,
                crate::sketches::relations::CONTRACT,
                r,
            )
        });
        let end = Instant::now() + Duration::from_secs(5);
        let claim = loop {
            assert!(Instant::now() < end);
            if let Some(c) = queue.take_next(queue.epoch()).unwrap() {
                break c;
            }
            thread::sleep(Duration::from_millis(1));
        };
        let Task::Inspect(r) = claim.task() else {
            panic!("line state must be a read")
        };
        assert!(!r.is_write());
        assert_eq!(r.operation(), "native_sketch_relations");
        let result = if unknown {
            json!({"success":false,"errorCode":"native_line_sdk_read_failed_status_7_stage_404_error_3","outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":false,"readBackendInvoked":true,"sdkDiagnostic":{"stage":404,"sdkError":3}})
        } else {
            json!({"success":true,"data":{"constraintMutationSupported":false}})
        };
        claim.complete(Ok(Reply::Value(result))).unwrap();
        let response = client.join().unwrap();
        assert_eq!(response.0, if unknown { 503 } else { 200 });
        if unknown {
            assert_eq!(response.1["outcome"], "unknown");
            assert_eq!(response.1["sdkDiagnostic"]["sdkError"], 3);
        }
    }
    assert!(reader
        .journal()
        .unwrap()
        .status(&epoch, "relation-read")
        .unwrap()
        .durable
        .unwrap()
        .record
        .is_none());
    assert!(server.stop());
    drop(server);
    drop(reader);
    std::fs::remove_dir_all(directory).unwrap();
}

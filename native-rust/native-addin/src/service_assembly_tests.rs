#[test]
#[cfg(not(feature = "native-wake-probe"))]
fn assembly_headers_admission_unknown_reads_and_no_effect_journal() {
    let queue = Queue::new(4).unwrap();
    let epoch = queue.epoch().to_string();
    let directory = std::env::temp_dir().join(format!(
        "fusion-r20-assembly-{}-{epoch}",
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
    let request = json!({"op":"native_assembly_inspect","sessionId":epoch,"operationId":"assembly-read","target":{"documentId":doc}});
    let denied = post_mechanical(
        address,
        &epoch,
        crate::sketches::dimensions::CONTRACT,
        request.clone(),
    );
    assert_eq!(denied.0, 409);
    assert_eq!(denied.1["errorCode"], "native_assembly_contract_required");
    let mut bad = request.clone();
    bad["target"]["revision"] = json!("wrong-projection");
    let denied = post_mechanical(address, &epoch, crate::assembly::CONTRACT, bad);
    assert_eq!(denied.0, 400);
    assert_eq!(denied.1["errorCode"], "native_assembly_revision_required");
    assert_eq!(wakes.load(Ordering::Relaxed), 0);
    assert!(!queue.has_queued());
    for unknown in [false, true] {
        let client_epoch = epoch.clone();
        let r = request.clone();
        let client = thread::spawn(move || {
            post_mechanical(address, &client_epoch, crate::assembly::CONTRACT, r)
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
        assert_eq!(r.operation(), "native_assembly_inspect");
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
        .status(&epoch, "assembly-read")
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

#[test]
#[cfg(not(feature = "native-wake-probe"))]
fn placement_header_and_static_refusal_do_not_admit_or_wake() {
    let queue = Queue::new(4).unwrap();
    let epoch = queue.epoch().to_string();
    let directory =
        std::env::temp_dir().join(format!("fusion-r21-header-{}-{epoch}", std::process::id()));
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
    let end = Instant::now() + Duration::from_secs(5);
    while reader.health()["connected"] != true {
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(1));
    }
    let doc = format!("d:{epoch}:0000000000000001");
    let revision = format!("{}{}", crate::assembly::PREFIX, "a".repeat(64));
    let request = json!({"op":"occurrence_update","sessionId":epoch,"operationId":"header","target":{"documentId":doc,"revision":revision},"occurrence":{"documentId":doc,"revision":revision,"objectType":"Occurrence","occurrenceId":"owned"},"transform":{"position":[25,-10,5]}});
    let denied = post_mechanical(
        server.address(),
        &epoch,
        crate::assembly::CONTRACT,
        request.clone(),
    );
    assert_eq!(denied.0, 409);
    assert_eq!(denied.1["errorCode"], "native_placement_contract_required");
    let mut bad = request;
    bad["transform"]["angle"] = json!(0);
    let denied = post_mechanical(
        server.address(),
        &epoch,
        crate::assembly::placement::CONTRACT,
        bad,
    );
    assert_eq!(denied.0, 400);
    assert_eq!(denied.1["errorCode"], "native_placement_position_only");
    assert_eq!(wakes.load(Ordering::Relaxed), 0);
    assert!(!queue.has_queued());
    assert!(reader
        .journal()
        .unwrap()
        .status(&epoch, "header")
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

#[test]
#[cfg(not(feature = "native-wake-probe"))]
fn physical_contract_refuses_before_dispatch_and_success_never_journals_read() {
    let queue = Queue::new(4).unwrap();
    let epoch = queue.epoch().to_string();
    let directory = std::env::temp_dir().join(format!(
        "fusion-r16-physical-{}-{epoch}",
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
    let request = json!({"op":"assembly_properties","sessionId":epoch,"operationId":"physical-read","target":{"documentId":doc}});
    let refusal = post_mechanical(address, &epoch, crate::purchased::CONTRACT, request.clone());
    assert_eq!(refusal.0, 409);
    assert_eq!(
        refusal.1["errorCode"],
        "native_root_physical_contract_required"
    );
    let mut explicit = request.clone();
    explicit["entity"] = json!({"documentId":doc,"revision":"rev","objectType":"Component"});
    let refusal = post_mechanical(address, &epoch, crate::physical::CONTRACT, explicit);
    assert_eq!(refusal.0, 400);
    assert_eq!(refusal.1["errorCode"], "native_physical_root_only");
    let mut missing = request.clone();
    missing.as_object_mut().unwrap().remove("sessionId");
    assert_eq!(
        post_mechanical(address, &epoch, crate::physical::CONTRACT, missing).1["errorCode"],
        "native_physical_current_session_required"
    );
    assert_eq!(wakes.load(Ordering::Relaxed), 0);
    assert!(!queue.has_queued());
    let client_epoch = epoch.clone();
    let client = thread::spawn(move || {
        post_mechanical(address, &client_epoch, crate::physical::CONTRACT, request)
    });
    let end = Instant::now() + Duration::from_secs(5);
    let claim = loop {
        assert!(Instant::now() < end);
        if let Some(claim) = queue.take_next(queue.epoch()).unwrap() {
            break claim;
        }
        thread::sleep(Duration::from_millis(1));
    };
    let Task::Inspect(request) = claim.task() else {
        panic!("physical properties must use read-only Inspect")
    };
    assert_eq!(request.operation(), "assembly_properties");
    assert!(!request.is_write());
    claim
        .complete(Ok(Reply::Value(
            json!({"success":true,"data":{"mass":0.1}}),
        )))
        .unwrap();
    assert_eq!(client.join().unwrap().0, 200);
    assert_eq!(wakes.load(Ordering::Relaxed), 1);
    assert!(reader
        .journal()
        .unwrap()
        .status(&epoch, "physical-read")
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

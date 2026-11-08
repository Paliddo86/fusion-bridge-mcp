struct AssemblyRig {
    common: Rig,
    backend: crate::assembly::tests::Fake,
    book: RefCell<crate::assembly::Book<u64>>,
}
impl AssemblyRig {
    fn new() -> Self {
        Self {
            common: Rig::new(),
            backend: crate::assembly::tests::Fake::default(),
            book: RefCell::new(crate::assembly::Book::default()),
        }
    }
    fn request(&self, id: &str) -> PreparedRequest {
        crate::assembly::tests::request(
            &mut self.book.borrow_mut(),
            &self.backend,
            &self.common.queue.epoch().to_string(),
            id,
        )
    }
    fn reply(&self, t: &mut Task) -> Result<Reply, String> {
        match t {
            Task::AssemblyPreflight(r) => {
                Ok(match self.book.borrow_mut().preflight(&self.backend, r) {
                    Ok(p) => Reply::AssemblyPlan(p),
                    Err(e) => Reply::Value(
                        json!({"success":false,"errorCode":e,"nativeExecutionStarted":false}),
                    ),
                })
            }
            Task::AssemblyExecute(p) => Ok(Reply::Value(
                self.book
                    .borrow_mut()
                    .execute(&self.backend, p.take().unwrap()),
            )),
            _ => Err("unexpected assembly task".into()),
        }
    }
    fn execute(&self, r: PreparedRequest) -> Response {
        let op = r.arguments()["operationId"].as_str().unwrap().to_owned();
        let client = self.common.spawn(r, Duration::from_secs(5));
        let end = Instant::now() + Duration::from_secs(6);
        while !client.is_finished() {
            assert!(Instant::now() < end);
            if let Some(mut c) = self
                .common
                .queue
                .take_next(self.common.queue.epoch())
                .unwrap()
            {
                if matches!(c.task(), Task::AssemblyExecute(_)) {
                    assert_eq!(
                        self.common
                            .reader
                            .journal()
                            .unwrap()
                            .status(&self.common.queue.epoch().to_string(), &op)
                            .unwrap()
                            .durable
                            .unwrap()
                            .record
                            .unwrap()
                            .phase,
                        Phase::Executing
                    );
                }
                let v = self.reply(c.task_mut());
                c.complete(v).unwrap();
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        client.join().unwrap()
    }
}
#[test]
fn assembly_durable_start_success_replay_conflict_and_shared_insert_never_duplicate() {
    let rig = AssemblyRig::new();
    let r = rig.request("create");
    let (code, v) = rig.execute(r.clone());
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["operationStatus"]["authority"], "durable_terminal");
    let (_, again) = rig.execute(r.clone());
    assert_eq!(again["replayed"], true);
    assert_eq!(again["data"], v["data"]);
    assert_eq!(rig.backend.add_calls.get(), 1);
    let mut a = r.arguments().clone();
    a["name"] = json!("conflict");
    assert_eq!(
        rig.execute(prepare_request(a, &rig.common.queue.epoch().to_string()).unwrap())
            .0,
        409
    );
    assert_eq!(rig.backend.add_calls.get(), 1);
    let state = rig
        .book
        .borrow_mut()
        .inspect(&rig.backend, crate::assembly::tests::DOC, None, false)
        .unwrap();
    let insert=prepare_request(json!({"op":"occurrence_insert","sessionId":rig.common.queue.epoch().to_string(),"operationId":"insert","target":state["data"]["target"],"component":state["data"]["components"][0]["ref"]}),&rig.common.queue.epoch().to_string()).unwrap();
    assert_eq!(rig.execute(insert.clone()).0, 200);
    assert_eq!(rig.execute(insert).1["replayed"], true);
    assert_eq!(rig.backend.add_calls.get(), 2);
    assert_eq!(rig.backend.state.borrow().components.len(), 1);
    assert_eq!(rig.backend.state.borrow().occurrences.len(), 2);
}
#[test]
fn assembly_partial_metadata_prefix_is_observed_unknown_and_never_replayed() {
    let rig = AssemblyRig::new();
    let mut a = rig.request("partial").arguments().clone();
    a["metadata"] = json!({"supplier":"quote\"\\"});
    let r = prepare_request(a, &rig.common.queue.epoch().to_string()).unwrap();
    rig.backend.mode.set(2);
    let (_, v) = rig.execute(r.clone());
    assert_eq!(v["outcome"], "unknown", "{v}");
    assert_eq!(rig.backend.add_calls.get(), 1);
    assert_eq!(rig.backend.change_calls.get(), 2);
    assert_eq!(v["stages"].as_array().unwrap().len(), 3);
    let (_, again) = rig.execute(r);
    assert_eq!(again["outcome"], "unknown");
    assert_eq!(again["replayed"], true);
    assert_eq!(rig.backend.add_calls.get(), 1);
    assert_eq!(rig.backend.change_calls.get(), 2);
}
#[test]
fn assembly_unsupported_translation_has_no_admission_or_sdk_calls() {
    let rig = AssemblyRig::new();
    let mut a = rig.request("translate").arguments().clone();
    a["transform"] = json!({"position":[1,0,0]});
    let r = prepare_request(a, &rig.common.queue.epoch().to_string()).unwrap();
    let (code, v) = rig.execute(r);
    assert_eq!(code, 400);
    assert_eq!(
        v["errorCode"],
        "native_assembly_identity_placement_required"
    );
    assert_eq!(rig.backend.add_calls.get(), 0);
    assert!(rig
        .common
        .reader
        .journal()
        .unwrap()
        .status(&rig.common.queue.epoch().to_string(), "translate")
        .unwrap()
        .durable
        .unwrap()
        .record
        .is_none());
}
#[test]
fn assembly_terminal_storage_loss_retains_complete_observation_without_reexecution() {
    let rig = AssemblyRig::new();
    let r = rig.request("lost-terminal");
    let client = rig.common.spawn(r.clone(), Duration::from_secs(5));
    let end = Instant::now() + Duration::from_secs(6);
    loop {
        assert!(Instant::now() < end);
        if let Some(mut c) = rig
            .common
            .queue
            .take_next(rig.common.queue.epoch())
            .unwrap()
        {
            let execute = matches!(c.task(), Task::AssemblyExecute(_));
            let v = rig.reply(c.task_mut());
            if execute {
                let path = rig
                    .common
                    .directory
                    .join(fusion_mechanical_journal::DATABASE_NAME);
                let held = rig.common.directory.with_extension("assembly-held");
                std::fs::rename(&path, &held).unwrap();
                c.complete(v).unwrap();
                let (code, body) = client.join().unwrap();
                assert_eq!(code, 503, "{body}");
                assert_ne!(body["operationStatus"]["authority"], "durable_terminal");
                assert_eq!(rig.backend.add_calls.get(), 1);
                std::fs::rename(held, path).unwrap();
                let (_, again) = rig.execute(r);
                assert_eq!(again["replayed"], true);
                assert_eq!(rig.backend.add_calls.get(), 1);
                break;
            }
            c.complete(v).unwrap();
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

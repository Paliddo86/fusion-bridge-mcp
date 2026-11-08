struct PlacementRig {
    common: Rig,
    backend: crate::assembly::placement_tests::PlacementFake,
    book: RefCell<crate::assembly::Book<u64>>,
}
impl PlacementRig {
    fn new() -> Self {
        Self {
            common: Rig::new(),
            backend: Default::default(),
            book: Default::default(),
        }
    }
    fn request(&self, id: &str) -> PreparedRequest {
        crate::assembly::placement_tests::request_for(
            &mut self.book.borrow_mut(),
            &self.backend,
            &self.common.queue.epoch().to_string(),
            id,
            [25., -10., 5.],
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
            _ => Err("unexpected placement task".into()),
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
fn placement_durable_start_two_stages_replay_conflict_and_noop() {
    let rig = PlacementRig::new();
    let r = rig.request("move");
    let (code, v) = rig.execute(r.clone());
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["operationStatus"]["authority"], "durable_terminal");
    assert_eq!(v["stages"].as_array().unwrap().len(), 2);
    let (_, again) = rig.execute(r.clone());
    assert_eq!(again["replayed"], true);
    assert_eq!(again["data"], v["data"]);
    assert_eq!(
        (rig.backend.transforms.get(), rig.backend.captures.get()),
        (1, 1)
    );
    let mut conflict = r.arguments().clone();
    conflict["transform"]["position"] = json!([1, 2, 3]);
    assert_eq!(
        rig.execute(prepare_request(conflict, &rig.common.queue.epoch().to_string()).unwrap())
            .0,
        409
    );
    let fresh = rig.request("noop");
    let (_, no) = rig.execute(fresh);
    assert_eq!(no["errorCode"], "native_placement_no_change");
    assert_eq!(no["operationStatus"]["authority"], "durable_terminal");
    assert_eq!(
        (rig.backend.transforms.get(), rig.backend.captures.get()),
        (1, 1)
    );
}
#[test]
fn placement_unknown_snapshot_ack_remains_unknown_without_any_effect_replay() {
    let rig = PlacementRig::new();
    let r = rig.request("unknown-capture");
    rig.backend.mode.set(7);
    let (code, v) = rig.execute(r.clone());
    assert_eq!(code, 503, "{v}");
    assert_eq!(v["outcome"], "unknown");
    assert_eq!(v["stages"][0]["attempt"]["acknowledged"], true);
    assert_eq!(v["stages"][1]["attempt"]["invoked"], true);
    assert_eq!(v["stages"][1]["attempt"]["acknowledged"], false);
    let (_, again) = rig.execute(r);
    assert_eq!(again["replayed"], true);
    assert_eq!(again["outcome"], "unknown");
    assert_eq!(
        (rig.backend.transforms.get(), rig.backend.captures.get()),
        (1, 1)
    );
}
#[test]
fn placement_terminal_storage_loss_retains_pose_and_snapshot_evidence() {
    let rig = PlacementRig::new();
    let r = rig.request("lost-placement");
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
                let held = rig.common.directory.with_extension("placement-held");
                std::fs::rename(&path, &held).unwrap();
                c.complete(v).unwrap();
                let (code, v) = client.join().unwrap();
                assert_eq!(code, 503, "{v}");
                assert_ne!(v["operationStatus"]["authority"], "durable_terminal");
                assert_eq!(
                    v["placementEvidence"]["after"]["snapshots"]
                        .as_array()
                        .unwrap()
                        .len(),
                    1
                );
                std::fs::rename(held, path).unwrap();
                let (_, again) = rig.execute(r);
                assert_eq!(again["replayed"], true);
                assert_eq!(
                    (rig.backend.transforms.get(), rig.backend.captures.get()),
                    (1, 1)
                );
                break;
            }
            c.complete(v).unwrap();
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

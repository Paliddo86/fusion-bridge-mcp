struct DimensionRig {
    common: Rig,
    backend: crate::sketches::dimensions::tests::Fake,
    book: RefCell<crate::sketches::Book<u64>>,
}
#[test]
fn dimension_durable_start_exact_replay_and_request_conflict_never_repeat_add() {
    let rig = DimensionRig::new();
    let request = rig.request("dimension-once");
    let (code, body) = rig.execute(request.clone());
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["operationStatus"]["authority"], "durable_terminal");
    assert_eq!(rig.backend.add_calls.get(), 1);
    let (_, replay) = rig.execute(request.clone());
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["data"], body["data"]);
    assert_eq!(rig.backend.add_calls.get(), 1);
    assert_eq!(rig.backend.set_calls.get(), 1);
    let mut args = request.arguments().clone();
    args["expression"] = json!(43);
    assert_eq!(
        rig.execute(prepare_request(args, &rig.common.queue.epoch().to_string()).unwrap())
            .0,
        409
    );
    assert_eq!(rig.backend.add_calls.get(), 1);
}
#[test]
fn dimension_unknown_ack_is_retained_without_replay_or_success_substitution() {
    let rig = DimensionRig::new();
    let request = rig.request("dimension-unknown");
    rig.backend.mode.set(3);
    let (_, body) = rig.execute(request.clone());
    assert_eq!(body["outcome"], "unknown", "{body}");
    assert_eq!(rig.backend.add_calls.get(), 1);
    let (_, replay) = rig.execute(request);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["outcome"], "unknown");
    assert_eq!(rig.backend.add_calls.get(), 1);
    assert_eq!(rig.backend.set_calls.get(), 0);
}
#[test]
fn dimension_late_observed_result_survives_http_deadline_without_second_add() {
    let rig = DimensionRig::new();
    let request = rig.request("dimension-late");
    let client = rig.common.spawn(request.clone(), Duration::from_secs(5));
    let end = Instant::now() + Duration::from_secs(6);
    let mut claim = loop {
        assert!(Instant::now() < end);
        if let Some(mut c) = rig
            .common
            .queue
            .take_next(rig.common.queue.epoch())
            .unwrap()
        {
            if matches!(c.task(), Task::DimensionExecute(_)) {
                break c;
            }
            let reply = rig.reply(c.task_mut());
            c.complete(reply).unwrap();
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    };
    assert_eq!(rig.backend.add_calls.get(), 0);
    let (code, body) = client.join().unwrap();
    assert_eq!(code, 503);
    assert_eq!(body["errorCode"], "operation_observer_deadline");
    let reply = rig.reply(claim.task_mut());
    claim.complete(reply).unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        let s = rig
            .common
            .reader
            .journal()
            .unwrap()
            .status(&rig.common.queue.epoch().to_string(), "dimension-late")
            .unwrap();
        if s.authority == fusion_mechanical_journal::retained::Authority::DurableTerminal {
            break;
        }
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(1));
    }
    let (code, replay) = rig.execute(request);
    assert_eq!(code, 200, "{replay}");
    assert_eq!(replay["replayed"], true);
    assert_eq!(rig.backend.add_calls.get(), 1);
}
#[test]
fn dimension_unsupported_shape_has_no_effect_or_journal_admission() {
    let rig = DimensionRig::new();
    let mut args = rig.request("dimension-invalid").arguments().clone();
    args["type"] = json!("radius");
    let (code, _) =
        rig.execute(prepare_request(args, &rig.common.queue.epoch().to_string()).unwrap());
    assert_eq!(code, 400);
    assert_eq!(rig.backend.add_calls.get(), 0);
    assert!(rig
        .common
        .reader
        .journal()
        .unwrap()
        .status(&rig.common.queue.epoch().to_string(), "dimension-invalid")
        .unwrap()
        .durable
        .unwrap()
        .record
        .is_none());
}
#[test]
fn dimension_storage_loss_before_start_or_terminal_commit_never_repeats_effect() {
    for after_effect in [false, true] {
        let rig = DimensionRig::new();
        let request = rig.request("dimension-storage");
        let client = rig.common.spawn(request.clone(), Duration::from_secs(5));
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut claim = loop {
            assert!(Instant::now() < deadline);
            if let Some(mut c) = rig
                .common
                .queue
                .take_next(rig.common.queue.epoch())
                .unwrap()
            {
                if !after_effect || matches!(c.task(), Task::DimensionExecute(_)) {
                    break c;
                }
                let result = rig.reply(c.task_mut());
                c.complete(result).unwrap();
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        };
        let result = rig.reply(claim.task_mut());
        assert_eq!(rig.backend.add_calls.get(), usize::from(after_effect));
        assert_eq!(rig.backend.set_calls.get(), usize::from(after_effect));
        let path = rig
            .common
            .directory
            .join(fusion_mechanical_journal::DATABASE_NAME);
        let held = rig.common.directory.with_extension("held-dimension");
        std::fs::rename(&path, &held).unwrap();
        claim.complete(result).unwrap();
        let (code, body) = client.join().unwrap();
        assert_eq!(code, 503, "{body}");
        assert_ne!(body["operationStatus"]["authority"], "durable_terminal");
        std::fs::rename(held, path).unwrap();
        let (_, again) = rig.execute(request);
        assert_ne!(again["operationStatus"]["authority"], "durable_terminal");
        assert_eq!(rig.backend.add_calls.get(), usize::from(after_effect));
        if after_effect {
            let status = rig
                .common
                .reader
                .journal()
                .unwrap()
                .status(&rig.common.queue.epoch().to_string(), "dimension-storage")
                .unwrap();
            assert_eq!(status.observed.unwrap().outcome, ObservedOutcome::Succeeded);
        }
    }
}
impl DimensionRig {
    fn new() -> Self {
        let (book, backend) = crate::sketches::dimensions::tests::setup(1);
        Self {
            common: Rig::new(),
            backend,
            book: RefCell::new(book),
        }
    }
    fn request(&self, id: &str) -> PreparedRequest {
        crate::sketches::dimensions::tests::request(
            &self.book.borrow(),
            &self.backend,
            &self.common.queue.epoch().to_string(),
            id,
        )
    }
    fn reply(&self, task: &mut Task) -> Result<Reply, String> {
        match task {
            Task::DimensionPreflight(request) => Ok(
                match self
                    .book
                    .borrow()
                    .dimension_preflight(&self.backend, request)
                {
                    Ok(p) => Reply::DimensionPlan(p),
                    Err(code) => Reply::Value(
                        json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
                    ),
                },
            ),
            Task::DimensionExecute(plan) => {
                // File-backed public journal must already prove executing before
                // this instrumented effect. This tests coordinator ordering itself.
                Ok(Reply::Value(
                    self.book
                        .borrow_mut()
                        .execute_dimension(&self.backend, plan.take().unwrap()),
                ))
            }
            _ => Err("unexpected geometry fixture task".into()),
        }
    }
    fn execute(&self, request: PreparedRequest) -> Response {
        let operation = request.arguments()["operationId"]
            .as_str()
            .unwrap()
            .to_owned();
        let client = self.common.spawn(request, Duration::from_secs(5));
        let deadline = Instant::now() + Duration::from_secs(6);
        while !client.is_finished() {
            assert!(Instant::now() < deadline);
            if let Some(mut claim) = self
                .common
                .queue
                .take_next(self.common.queue.epoch())
                .unwrap()
            {
                if matches!(claim.task_mut(), Task::DimensionExecute(_)) {
                    let status = self
                        .common
                        .reader
                        .journal()
                        .unwrap()
                        .status(&self.common.queue.epoch().to_string(), &operation)
                        .unwrap();
                    assert_eq!(
                        status.durable.unwrap().record.unwrap().phase,
                        fusion_mechanical_journal::Phase::Executing
                    );
                }
                let reply = self.reply(claim.task_mut());
                claim.complete(reply).unwrap();
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        client.join().unwrap()
    }
}

use super::*;
use crate::mechanics::{self, Backend, DesignState, HealthItem, Parameter, ParameterValue};
use fusion_mechanical_core::prepare_request;
use fusion_mechanical_journal::{retained::ObservedOutcome, Phase};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    thread::{self, JoinHandle},
};

struct BomRig {
    common: Rig,
    backend: crate::bom::tests::Fake,
    captures: Cell<usize>,
}
impl BomRig {
    fn new() -> Self {
        Self {
            common: Rig::new(),
            backend: crate::bom::tests::Fake::new(),
            captures: Cell::new(0),
        }
    }
    fn request(&self, operation: &str, path: &std::path::Path) -> PreparedRequest {
        let captured = crate::bom::generate(
            &self.backend,
            &self.common.queue.epoch().to_string(),
            "bom-doc",
            None,
            false,
        )
        .unwrap();
        prepare_request(json!({"op":"bom_export","sessionId":self.common.queue.epoch().to_string(),"operationId":operation,"target":captured["target"],"filename":path}),&self.common.queue.epoch().to_string()).unwrap()
    }
    fn reply(&self, task: &Task) -> Result<Reply, String> {
        let Task::BomExportPreflight(request) = task else {
            return Err("unexpected BOM fixture task".into());
        };
        self.captures.set(self.captures.get() + 1);
        let result = crate::bom::capture(
            &self.backend,
            &self.common.queue.epoch().to_string(),
            "bom-doc",
            request.arguments()["target"]["revision"].as_str(),
            false,
        )
        .and_then(|capture| crate::bom_export::Plan::prepare(request, capture));
        Ok(match result {
            Ok(plan) => Reply::BomExportPlan(plan),
            Err(code) => Reply::Value(
                json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
            ),
        })
    }
    fn execute(&self, request: PreparedRequest) -> Response {
        let client = self.common.spawn(request, Duration::from_secs(2));
        let deadline = Instant::now() + Duration::from_secs(3);
        while !client.is_finished() {
            assert!(Instant::now() < deadline);
            if let Some(claim) = self
                .common
                .queue
                .take_next(self.common.queue.epoch())
                .unwrap()
            {
                let reply = self.reply(claim.task());
                claim.complete(reply).unwrap();
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        client.join().unwrap()
    }
}
#[test]
fn bom_csv_journal_replay_and_conflict_do_not_republish_or_recollect() {
    let rig = BomRig::new();
    let path = rig.common.directory.join("bom.csv");
    let request = rig.request("export", &path);
    let (status, result) = rig.execute(request.clone());
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["data"]["published"], true);
    assert_eq!(result["operationStatus"]["authority"], "durable_terminal");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(result["data"]["fileSize"], bytes.len());
    assert_eq!(
        result["data"]["sha256"],
        format!("{:x}", Sha256::digest(&bytes))
    );
    assert!(String::from_utf8(bytes)
        .unwrap()
        .contains("bom-projection-v1:"));
    std::fs::write(&path, b"independent later content").unwrap();
    let (status, replayed) = rig.execute(request);
    assert_eq!(status, 200);
    assert_eq!(replayed["replayed"], true);
    assert_eq!(std::fs::read(&path).unwrap(), b"independent later content");
    assert_eq!(rig.captures.get(), 1);
    assert_eq!(
        rig.execute(rig.request("export", &rig.common.directory.join("other.csv")))
            .0,
        409
    );
    assert_eq!(rig.captures.get(), 1);
    assert!(!rig.common.directory.join("other.csv").exists());
}
#[test]
fn bom_csv_existing_destination_and_stale_revision_refuse_without_overwrite() {
    let rig = BomRig::new();
    let path = rig.common.directory.join("preserve.csv");
    std::fs::write(&path, b"existing").unwrap();
    let (status, result) = rig.execute(rig.request("exclusive", &path));
    assert_eq!(status, 400);
    assert_eq!(result["publicationState"], "not_attempted");
    assert_eq!(std::fs::read(&path).unwrap(), b"existing");
    let request = rig.request("stale", &rig.common.directory.join("stale.csv"));
    let mut arguments = request.arguments().clone();
    arguments["target"]["revision"] = json!("bom-projection-v1:stale");
    let request = prepare_request(arguments, &rig.common.queue.epoch().to_string()).unwrap();
    let (_, result) = rig.execute(request);
    assert_eq!(result["errorCode"], "target_revision_mismatch");
    assert!(!rig.common.directory.join("stale.csv").exists());
}
#[test]
fn bom_csv_durable_start_failure_cannot_publish_even_with_completed_capture() {
    let rig = BomRig::new();
    let path = rig.common.directory.join("never.csv");
    let request = rig.request("start-failed", &path);
    let client = rig.common.spawn(request, Duration::from_secs(2));
    let deadline = Instant::now() + Duration::from_secs(3);
    let claim = loop {
        assert!(Instant::now() < deadline);
        if let Some(claim) = rig
            .common
            .queue
            .take_next(rig.common.queue.epoch())
            .unwrap()
        {
            break claim;
        }
        thread::sleep(Duration::from_millis(1));
    };
    let reply = rig.reply(claim.task());
    let database = rig
        .common
        .directory
        .join(fusion_mechanical_journal::DATABASE_NAME);
    let held = rig.common.directory.with_extension("held-sqlite");
    std::fs::rename(&database, &held).unwrap();
    claim.complete(reply).unwrap();
    let (status, result) = client.join().unwrap();
    assert_eq!(status, 503, "{result}");
    assert!(!path.exists());
    std::fs::rename(held, database).unwrap();
}
#[test]
fn bom_structured_csv_requires_explicit_overwrite_and_preserves_snapshot_provenance() {
    let rig = BomRig::new();
    let path = rig.common.directory.join("structured.csv");
    std::fs::write(&path, b"old contents").unwrap();
    let request = rig.request("overwrite", &path);
    let mut arguments = request.arguments().clone();
    arguments["overwrite"] = json!(true);
    arguments["mode"] = json!("structured");
    let (status, result) =
        rig.execute(prepare_request(arguments, &rig.common.queue.epoch().to_string()).unwrap());
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["data"]["overwrite"], true);
    assert_eq!(result["data"]["snapshotExport"], true);
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.starts_with("path,parentPath,depth,componentId,"));
    assert!(text.contains("bom-doc"));
    assert!(text.contains(result["data"]["target"]["revision"].as_str().unwrap()));
    assert_eq!(text.lines().count(), 6);
}
#[test]
fn bom_invalid_export_path_refuses_before_admission_or_sdk_capture() {
    let rig = BomRig::new();
    let request = rig.request("bad-path", std::path::Path::new("relative.csv"));
    let (status, result) = rig.execute(request);
    assert_eq!(status, 400);
    assert_eq!(result["errorCode"], "native_bom_invalid_destination");
    assert_eq!(rig.captures.get(), 0);
    let journal = rig.common.reader.journal().unwrap();
    let status = journal
        .status(&rig.common.queue.epoch().to_string(), "bad-path")
        .unwrap();
    assert!(status.durable.unwrap().record.is_none());
}
#[test]
fn bom_read_never_falls_back_to_the_active_document_for_a_missing_exact_target() {
    let rig = BomRig::new();
    for target in [
        json!({}),
        json!({"documentId":""}),
        json!({"documentId":"bom-doc","revision":null}),
        json!({"documentId":"bom-doc","componentId":"foreign"}),
    ] {
        let request = prepare_request(
            json!({"op":"bom","target":target}),
            &rig.common.queue.epoch().to_string(),
        )
        .unwrap();
        let (status, result) = rig.execute(request);
        assert_eq!(status, 400, "{result}");
        assert_eq!(result["nativeExecutionStarted"], false);
        assert_eq!(rig.captures.get(), 0);
    }
}

struct LifecycleRig {
    common: Rig,
    registry: crate::identity::Registry<crate::identity::document_lifecycle::tests::Fake>,
    fake: crate::identity::document_lifecycle::tests::Fake,
}
impl LifecycleRig {
    fn new() -> Self {
        let common = Rig::new();
        let fake = crate::identity::document_lifecycle::tests::Fake::new();
        let registry =
            crate::identity::Registry::new(common.queue.epoch().to_string(), fake.clone()).unwrap();
        Self {
            common,
            registry,
            fake,
        }
    }
    fn create(&self, id: &str) -> PreparedRequest {
        crate::identity::document_lifecycle::tests::create_request(
            &self.common.queue.epoch().to_string(),
            id,
        )
    }
    fn run(&self, task: &mut Task) -> Result<Reply, String> {
        match task {
            Task::LifecyclePreflight(request, proof) => Ok(
                match self.registry.lifecycle_preflight(request, proof.clone()) {
                    Ok(plan) => Reply::LifecyclePlan(plan),
                    Err(error) => Reply::Value(
                        json!({"success":false,"errorCode":error,"nativeExecutionStarted":false}),
                    ),
                },
            ),
            Task::LifecycleExecute(plan) => Ok(Reply::Value(
                self.registry.lifecycle_execute(plan.take().unwrap()),
            )),
            _ => Err("unexpected task in instrumented lifecycle SDK fixture".into()),
        }
    }
    fn execute(&self, request: PreparedRequest) -> Response {
        let client = self.common.spawn(request, Duration::from_secs(2));
        let end = Instant::now() + Duration::from_secs(3);
        while !client.is_finished() {
            assert!(Instant::now() < end);
            if let Some(mut claim) = self
                .common
                .queue
                .take_next(self.common.queue.epoch())
                .unwrap()
            {
                let reply = self.run(claim.task_mut());
                claim.complete(reply).unwrap();
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        client.join().unwrap()
    }
    fn next_execute(&self) -> fusion_native_dispatch::Claim<Task, Result<Reply, String>> {
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            assert!(Instant::now() < end);
            if let Some(mut claim) = self
                .common
                .queue
                .take_next(self.common.queue.epoch())
                .unwrap()
            {
                if matches!(claim.task(), Task::LifecycleExecute(_)) {
                    return claim;
                }
                let reply = self.run(claim.task_mut());
                claim.complete(reply).unwrap();
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
    fn discard(&self, created: &Value, operation: &str) -> PreparedRequest {
        prepare_request(json!({"op":"native_scratch_discard","sessionId":self.common.queue.epoch().to_string(),"operationId":operation,"creationOperationId":"create","target":created["data"]["target"]}),&self.common.queue.epoch().to_string()).unwrap()
    }
}
impl Drop for LifecycleRig {
    fn drop(&mut self) {
        self.registry.close().unwrap();
    }
}

#[test]
fn lifecycle_journal_replays_without_second_create_or_close_and_refuses_conflict() {
    let rig = LifecycleRig::new();
    let request = rig.create("create");
    let (status, created) = rig.execute(request.clone());
    assert_eq!(status, 200, "{created}");
    assert_eq!(created["operationStatus"]["authority"], "durable_terminal");
    let (status, replay) = rig.execute(request);
    assert_eq!(status, 200);
    assert_eq!(replay["replayed"], true);
    assert_eq!(rig.fake.0.borrow().creates, 1);
    let changed=prepare_request(json!({"op":"new_design","sessionId":rig.common.queue.epoch().to_string(),"operationId":"create","name":"different"}),&rig.common.queue.epoch().to_string()).unwrap();
    assert_eq!(rig.execute(changed).0, 409);
    assert_eq!(rig.fake.0.borrow().creates, 1);
    let close = rig.discard(&created, "close");
    let (status, closed) = rig.execute(close.clone());
    assert_eq!(status, 200, "{closed}");
    assert_eq!(rig.fake.0.borrow().closes, 1);
    assert_eq!(rig.execute(close).0, 200);
    assert_eq!(rig.fake.0.borrow().closes, 1);
}
#[test]
fn lifecycle_unknown_close_is_retained_and_duplicate_never_replays() {
    let rig = LifecycleRig::new();
    let (_, created) = rig.execute(rig.create("create"));
    rig.fake.0.borrow_mut().close_loss = true;
    let close = rig.discard(&created, "close");
    let (status, observed) = rig.execute(close.clone());
    assert_eq!(status, 503, "{observed}");
    assert_eq!(
        observed["operationStatus"]["authority"],
        "observed_nondurable"
    );
    assert_eq!(observed["outcome"], "unknown");
    assert_eq!(rig.execute(close).0, 503);
    assert_eq!(rig.fake.0.borrow().closes, 1);
}
#[test]
fn lifecycle_observer_timeout_does_not_lose_late_creation_result() {
    let rig = LifecycleRig::new();
    let request = rig.create("create");
    let client = rig.common.spawn(request.clone(), Duration::from_secs(5));
    // Claim the actual effect before waiting for the HTTP observer to expire.
    // A 100 ms admission budget raced journal/preflight work in a parallel
    // workspace run, testing a legitimate pre-execution refusal instead.
    let admission_end = Instant::now() + Duration::from_secs(6);
    let mut claim = loop {
        assert!(Instant::now() < admission_end, "effect was not admitted");
        assert!(
            !client.is_finished(),
            "observer expired before effect claim"
        );
        if let Some(mut claim) = rig
            .common
            .queue
            .take_next(rig.common.queue.epoch())
            .unwrap()
        {
            if matches!(claim.task(), Task::LifecycleExecute(_)) {
                break claim;
            }
            let reply = rig.run(claim.task_mut());
            claim.complete(reply).unwrap();
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    };
    // The claimed SDK effect remains held. Join observes the real deadline;
    // no assumed scheduling interval decides when completion may proceed.
    assert_eq!(client.join().unwrap().0, 503);
    let reply = rig.run(claim.task_mut());
    claim.complete(reply).unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        let status = rig
            .common
            .reader
            .journal()
            .unwrap()
            .status(&rig.common.queue.epoch().to_string(), "create")
            .unwrap();
        if status.authority == fusion_mechanical_journal::retained::Authority::DurableTerminal {
            break;
        }
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(rig.execute(request).0, 200);
    assert_eq!(rig.fake.0.borrow().creates, 1);
}
#[test]
fn lifecycle_created_observation_survives_storage_loss_without_granting_discard() {
    let rig = LifecycleRig::new();
    let request = rig.create("create");
    let client = rig.common.spawn(request.clone(), Duration::from_secs(2));
    let mut claim = rig.next_execute();
    let reply = rig.run(claim.task_mut());
    let created = match &reply {
        Ok(Reply::Value(value)) => value.clone(),
        _ => panic!("creation observation absent"),
    };
    assert_eq!(created["success"], true);
    let path = rig
        .common
        .directory
        .join(fusion_mechanical_journal::DATABASE_NAME);
    let held = rig.common.directory.with_extension("held-sqlite");
    std::fs::rename(&path, &held).unwrap();
    claim.complete(reply).unwrap();
    let (status, observed) = client.join().unwrap();
    assert_eq!(status, 503, "{observed}");
    assert_eq!(
        observed["operationStatus"]["authority"],
        "observed_nondurable"
    );
    assert_eq!(
        observed["operationStatus"]["observed"]["result"]["data"]["ownedScratch"],
        true
    );
    std::fs::rename(&held, &path).unwrap();
    assert_eq!(rig.execute(request).0, 503);
    assert_eq!(rig.fake.0.borrow().creates, 1);
    let (_, discard) = rig.execute(rig.discard(&created, "discard"));
    assert_ne!(discard["success"], true);
    assert_eq!(rig.fake.0.borrow().closes, 0);
}
#[test]
fn lifecycle_box_is_refused_before_admission_or_sdk_dispatch() {
    let rig = LifecycleRig::new();
    let request=prepare_request(json!({"op":"new_design","sessionId":rig.common.queue.epoch().to_string(),"operationId":"box","box":{"width":1,"height":2,"depth":3}}),&rig.common.queue.epoch().to_string()).unwrap();
    let (status, body) = rig.execute(request);
    assert_eq!(status, 400);
    assert_eq!(body["nativeExecutionStarted"], false);
    assert!(rig
        .common
        .reader
        .journal()
        .unwrap()
        .status(&rig.common.queue.epoch().to_string(), "box")
        .unwrap()
        .durable
        .unwrap()
        .record
        .is_none());
    assert_eq!(rig.fake.0.borrow().creates, 0);
}

struct BackendFixture {
    parameters: RefCell<Vec<Parameter>>,
    calls: Cell<usize>,
    lose_ack: Cell<bool>,
}
impl BackendFixture {
    fn new() -> Self {
        Self {
            parameters: RefCell::new(vec![]),
            calls: Cell::new(0),
            lose_ack: Cell::new(false),
        }
    }
}
impl Backend for BackendFixture {
    fn design_state(&self) -> Result<DesignState, String> {
        Ok(DesignState {
            active: true,
            parametric: true,
            root_name: "owned-test".into(),
            root_revision: "fixture".into(),
            default_length_units: "mm".into(),
            component_count: 1,
            body_count: 0,
            sketch_count: 0,
            timeline_count: 0,
            timeline_position: 0,
        })
    }
    fn parameter_count(&self) -> Result<usize, String> {
        Ok(self.parameters.borrow().len())
    }
    fn parameter(&self, i: usize) -> Result<Parameter, String> {
        self.parameters
            .borrow()
            .get(i)
            .cloned()
            .ok_or("missing".into())
    }
    fn timeline_health(&self, _: usize) -> Result<HealthItem, String> {
        Err("no timeline".into())
    }
    fn evaluate_expression(&self, e: &str, _: &str) -> Result<f64, String> {
        e.trim_end_matches(" mm")
            .parse::<f64>()
            .map(|n| n / 10.)
            .map_err(|_| "bad expression".into())
    }
    fn create_parameter(&self, n: &str, e: &str, u: &str, c: &str) -> Result<(), String> {
        self.calls.set(self.calls.get() + 1);
        self.parameters.borrow_mut().push(Parameter {
            name: n.into(),
            expression: e.into(),
            unit: u.into(),
            comment: c.into(),
            is_user: true,
            is_deletable: true,
            value: ParameterValue::Numeric(self.evaluate_expression(e, u)?),
        });
        if self.lose_ack.get() {
            Err("ack lost after effect".into())
        } else {
            Ok(())
        }
    }
    fn set_parameter(&self, n: &str, e: &str) -> Result<(), String> {
        self.calls.set(self.calls.get() + 1);
        let mut ps = self.parameters.borrow_mut();
        let p = ps.iter_mut().find(|p| p.name == n).ok_or("missing")?;
        p.expression = e.into();
        p.value = ParameterValue::Numeric(self.evaluate_expression(e, &p.unit)?);
        Ok(())
    }
    fn compute_all(&self) -> Result<(), String> {
        Ok(())
    }
}
struct Rig {
    queue: Arc<Queue>,
    executor: Arc<Executor>,
    reader: Arc<OperationStatusReader>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    directory: PathBuf,
    backend: BackendFixture,
}
impl Rig {
    fn new() -> Self {
        let queue = Queue::new(8).unwrap();
        let directory = std::env::temp_dir().join(format!("fusion-r7-executor-{}", queue.epoch()));
        let reader = OperationStatusReader::new(directory.clone(), queue.epoch().to_string());
        let executor = Executor::new(queue.clone(), Arc::new(|| true), reader.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let e = executor.clone();
        let s = stop.clone();
        let worker = thread::spawn(move || e.run(&s));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !executor.ready() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        Self {
            queue,
            executor,
            reader,
            stop,
            worker: Some(worker),
            directory,
            backend: BackendFixture::new(),
        }
    }
    fn request(&self, id: &str) -> PreparedRequest {
        let inspect = mechanics::inspect(&self.backend, "doc").unwrap();
        prepare_request(json!({"op":"parameter_create","sessionId":self.queue.epoch().to_string(),"operationId":id,"target":inspect.target,"name":"width","expression":"10 mm","unit":"mm"}),&self.queue.epoch().to_string()).unwrap()
    }
    fn spawn(&self, r: PreparedRequest, budget: Duration) -> JoinHandle<Response> {
        let e = self.executor.clone();
        thread::spawn(move || e.request(r, Instant::now() + budget))
    }
    fn run_task(&self, task: &mut Task) -> Result<Reply, String> {
        match task {
            Task::Preflight(r) => Ok(match mechanics::preflight(&self.backend, r, "doc") {
                Ok(p) => Reply::Plan(p),
                Err(e) => Reply::Value(
                    json!({"success":false,"errorCode":e.code,"nativeExecutionStarted":false}),
                ),
            }),
            Task::Execute(p) => Ok(Reply::Value(mechanics::execute(
                &self.backend,
                p.take().unwrap(),
            ))),
            Task::Inspect(_) => Ok(Reply::Value(
                json!({"success":true,"data":mechanics::inspect(&self.backend,"doc").unwrap()}),
            )),
            Task::Snapshot => Err("not part of synthetic mechanic fixture".into()),
            Task::LifecyclePreflight(..)
            | Task::LifecycleExecute(..)
            | Task::SketchPreflight(..)
            | Task::SketchExecute(..)
            | Task::GeometryPreflight(..)
            | Task::GeometryExecute(..)
            | Task::DimensionPreflight(..)
            | Task::DimensionExecute(..)
            | Task::RelationPreflight(..)
            | Task::RelationExecute(..)
            | Task::ExtrudePreflight(..)
            | Task::ExtrudeExecute(..)
            | Task::AssemblyPreflight(..)
            | Task::AssemblyExecute(..)
            | Task::BomExportPreflight(..) => Err("lifecycle fixture required".into()),
        }
    }
    fn pump(&self, client: &JoinHandle<Response>) {
        let end = Instant::now() + Duration::from_secs(3);
        while !client.is_finished() {
            assert!(Instant::now() < end);
            if let Some(mut claim) = self.queue.take_next(self.queue.epoch()).unwrap() {
                let reply = self.run_task(claim.task_mut());
                claim.complete(reply).unwrap();
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
    fn execute(&self, request: PreparedRequest) -> Response {
        let client = self.spawn(request, Duration::from_secs(2));
        self.pump(&client);
        client.join().unwrap()
    }
    fn next_execute(&self) -> fusion_native_dispatch::Claim<Task, Result<Reply, String>> {
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            assert!(Instant::now() < end);
            if let Some(mut claim) = self.queue.take_next(self.queue.epoch()).unwrap() {
                if matches!(claim.task(), Task::Execute(_)) {
                    return claim;
                }
                let reply = self.run_task(claim.task_mut());
                claim.complete(reply).unwrap();
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}
impl Drop for Rig {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.queue.shutdown(self.queue.epoch()).unwrap();
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn create_replay_conflict_and_stale_target_never_repeat_native_effect() {
    let rig = Rig::new();
    let request = rig.request("create");
    let original = request.arguments().clone();
    let target = original["target"].clone();
    let (code, body) = rig.execute(request);
    assert_eq!(code, 200, "{body}");
    assert_eq!(rig.backend.calls.get(), 1);
    assert_eq!(body["operationStatus"]["authority"], "durable_terminal");
    let (code, replay) =
        rig.execute(prepare_request(original.clone(), &rig.queue.epoch().to_string()).unwrap());
    assert_eq!(code, 200);
    assert_eq!(replay["replayed"], true);
    assert_eq!(rig.backend.calls.get(), 1);
    let mut conflict = original;
    conflict["expression"] = json!("20 mm");
    assert_eq!(
        rig.execute(prepare_request(conflict, &rig.queue.epoch().to_string()).unwrap())
            .0,
        409
    );
    let stale=prepare_request(json!({"op":"parameter_set","sessionId":rig.queue.epoch().to_string(),"operationId":"stale","target":target,"name":"width","expression":"20 mm"}),&rig.queue.epoch().to_string()).unwrap();
    let (code, body) = rig.execute(stale);
    assert_eq!(code, 400, "{body}");
    assert_eq!(body["nativeExecutionStarted"], false);
    assert_eq!(rig.backend.calls.get(), 1);
}
#[test]
fn actual_mutation_followed_by_lost_ack_stays_unknown_and_cannot_replay() {
    let rig = Rig::new();
    rig.backend.lose_ack.set(true);
    let request = rig.request("lost-ack");
    let original = request.arguments().clone();
    let (code, body) = rig.execute(request);
    assert_eq!(code, 503, "{body}");
    assert_eq!(rig.backend.calls.get(), 1);
    assert_eq!(rig.backend.parameters.borrow().len(), 1);
    let journal = rig.reader.journal().unwrap();
    let status = journal
        .status(&rig.queue.epoch().to_string(), "lost-ack")
        .unwrap();
    assert_eq!(
        status.durable.unwrap().record.unwrap().phase,
        Phase::Executing
    );
    assert_eq!(status.observed.unwrap().outcome, ObservedOutcome::Unknown);
    assert_eq!(
        rig.execute(prepare_request(original, &rig.queue.epoch().to_string()).unwrap())
            .0,
        503
    );
    assert_eq!(rig.backend.calls.get(), 1);
}
#[test]
fn late_completion_survives_http_observer_deadline() {
    let rig = Rig::new();
    // Admission is allowed to finish before this test withholds the claimed
    // effect. The old150 ms budget raced journal I/O in full-workspace runs.
    let client = rig.spawn(rig.request("late"), Duration::from_secs(5));
    let mut claim = rig.next_execute();
    // Await the actual observer deadline while the SDK claim is still held;
    // a scheduling-dependent sleep cannot decide the ordering under test.
    let (code, body) = client.join().unwrap();
    assert_eq!(code, 503);
    assert_eq!(body["errorCode"], "operation_observer_deadline");
    let reply = rig.run_task(claim.task_mut());
    claim.complete(reply).unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        let s = rig
            .reader
            .journal()
            .unwrap()
            .status(&rig.queue.epoch().to_string(), "late")
            .unwrap();
        if s.authority == fusion_mechanical_journal::retained::Authority::DurableTerminal {
            break;
        }
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(rig.backend.calls.get(), 1);
}
#[test]
fn shutdown_after_claim_preserves_actual_result_before_coordinator_join() {
    let mut rig = Rig::new();
    let client = rig.spawn(rig.request("stopping"), Duration::from_secs(2));
    let mut claim = rig.next_execute();
    rig.stop.store(true, Ordering::Release);
    rig.queue.shutdown(rig.queue.epoch()).unwrap();
    let reply = rig.run_task(claim.task_mut());
    claim.complete(reply).unwrap();
    assert_eq!(client.join().unwrap().0, 200);
    rig.worker.take().unwrap().join().unwrap();
    assert_eq!(
        rig.reader
            .journal()
            .unwrap()
            .status(&rig.queue.epoch().to_string(), "stopping")
            .unwrap()
            .authority,
        fusion_mechanical_journal::retained::Authority::DurableTerminal
    );
}
#[test]
fn changed_revision_between_preflight_and_effect_refuses_mutation() {
    let rig = Rig::new();
    let client = rig.spawn(rig.request("changed"), Duration::from_secs(2));
    let mut claim = rig.next_execute();
    rig.backend.parameters.borrow_mut().push(Parameter {
        name: "intervening".into(),
        expression: "1 mm".into(),
        unit: "mm".into(),
        comment: "".into(),
        is_user: true,
        is_deletable: true,
        value: ParameterValue::Numeric(0.1),
    });
    let reply = rig.run_task(claim.task_mut());
    claim.complete(reply).unwrap();
    let (code, body) = client.join().unwrap();
    assert_eq!(code, 400, "{body}");
    assert_eq!(body["nativeExecutionStarted"], false);
    assert_eq!(rig.backend.calls.get(), 0);
}

struct SketchRig {
    common: Rig,
    backend: crate::sketches::tests::Fake,
    book: RefCell<crate::sketches::Book<usize>>,
}
impl SketchRig {
    fn new() -> Self {
        Self {
            common: Rig::new(),
            backend: Default::default(),
            book: RefCell::new(Default::default()),
        }
    }
    fn request(&self, id: &str) -> PreparedRequest {
        let target = self
            .book
            .borrow()
            .inspect(&self.backend, "sketch-doc", None)
            .unwrap()["target"]
            .clone();
        prepare_request(json!({"op":"sketch_create","sessionId":self.common.queue.epoch().to_string(),"operationId":id,"target":target,"plane":"xy"}),&self.common.queue.epoch().to_string()).unwrap()
    }
    fn reply(&self, task: &mut Task) -> Result<Reply, String> {
        match task {
            Task::SketchPreflight(r) => Ok(match self.book.borrow().preflight(&self.backend, r) {
                Ok(p) => Reply::SketchPlan(p),
                Err(code) => Reply::Value(
                    json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
                ),
            }),
            Task::SketchExecute(p) => Ok(Reply::Value(
                self.book
                    .borrow_mut()
                    .execute(&self.backend, p.take().unwrap()),
            )),
            _ => Err("unexpected sketch fixture task".into()),
        }
    }
    fn execute(&self, request: PreparedRequest) -> Response {
        let client = self.common.spawn(request, Duration::from_secs(2));
        let deadline = Instant::now() + Duration::from_secs(3);
        while !client.is_finished() {
            assert!(Instant::now() < deadline);
            if let Some(mut claim) = self
                .common
                .queue
                .take_next(self.common.queue.epoch())
                .unwrap()
            {
                let reply = self.reply(claim.task_mut());
                claim.complete(reply).unwrap();
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        client.join().unwrap()
    }
}
#[test]
fn sketch_journal_create_replay_and_conflict_never_repeat_sdk_add() {
    let rig = SketchRig::new();
    let request = rig.request("create-once");
    let (status, result) = rig.execute(request.clone());
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["operationStatus"]["authority"], "durable_terminal");
    assert_eq!(rig.backend.creates.get(), 1);
    let (_, replayed) = rig.execute(request.clone());
    assert_eq!(replayed["replayed"], true);
    assert_eq!(replayed["data"]["sketchId"], result["data"]["sketchId"]);
    assert_eq!(rig.backend.creates.get(), 1);
    let mut args = request.arguments().clone();
    args["plane"] = json!("yz");
    assert_eq!(
        rig.execute(prepare_request(args, &rig.common.queue.epoch().to_string()).unwrap())
            .0,
        409
    );
    assert_eq!(rig.backend.creates.get(), 1);
}
#[test]
fn sketch_durable_start_failure_prevents_sdk_add() {
    let rig = SketchRig::new();
    let request = rig.request("no-start");
    let client = rig.common.spawn(request, Duration::from_secs(2));
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut claim = loop {
        assert!(Instant::now() < deadline);
        if let Some(c) = rig
            .common
            .queue
            .take_next(rig.common.queue.epoch())
            .unwrap()
        {
            break c;
        }
        thread::sleep(Duration::from_millis(1));
    };
    let reply = rig.reply(claim.task_mut());
    let database = rig
        .common
        .directory
        .join(fusion_mechanical_journal::DATABASE_NAME);
    let held = rig.common.directory.with_extension("held-sketch-sqlite");
    std::fs::rename(&database, &held).unwrap();
    claim.complete(reply).unwrap();
    assert_eq!(client.join().unwrap().0, 503);
    assert_eq!(rig.backend.creates.get(), 0);
    std::fs::rename(held, database).unwrap();
}
#[test]
fn sketch_unknown_native_ack_remains_unknown_and_replay_does_not_execute() {
    let rig = SketchRig::new();
    let request = rig.request("unknown-create");
    rig.backend.mode.set(1);
    let (_, result) = rig.execute(request.clone());
    assert_eq!(result["outcome"], "unknown", "{result}");
    assert_eq!(rig.backend.creates.get(), 1);
    let (_, replay) = rig.execute(request);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["outcome"], "unknown");
    assert_eq!(rig.backend.creates.get(), 1);
}
#[test]
fn sketch_name_and_invalid_geometry_scope_are_refused_before_admission() {
    let rig = SketchRig::new();
    let request = rig.request("unsupported-name");
    let mut value = request.arguments().clone();
    value["name"] = json!("ignored is forbidden");
    let request = prepare_request(value, &rig.common.queue.epoch().to_string()).unwrap();
    let (code, result) = rig.execute(request);
    assert_eq!(code, 400);
    assert_eq!(result["errorCode"], "native_sketch_root_default_name_only");
    assert_eq!(rig.backend.creates.get(), 0);
    assert!(rig
        .common
        .reader
        .journal()
        .unwrap()
        .status(&rig.common.queue.epoch().to_string(), "unsupported-name")
        .unwrap()
        .durable
        .unwrap()
        .record
        .is_none());
    let mut value = rig.request("unsupported-geometry").arguments().clone();
    value["op"] = json!("sketch_geometry");
    value.as_object_mut().unwrap().remove("plane");
    value["sketch"] =
        json!({"documentId":"sketch-doc","revision":"unsupported","objectType":"Sketch"});
    value["items"] = json!([{"type":"point","position":[0,0]}]);
    let (code, result) =
        rig.execute(prepare_request(value, &rig.common.queue.epoch().to_string()).unwrap());
    assert_eq!(code, 400);
    assert_eq!(
        result["errorCode"],
        "native_geometry_sketch_revision_required"
    );
    assert!(rig
        .common
        .reader
        .journal()
        .unwrap()
        .status(
            &rig.common.queue.epoch().to_string(),
            "unsupported-geometry"
        )
        .unwrap()
        .durable
        .unwrap()
        .record
        .is_none());
}

struct GeometryRig {
    common: Rig,
    backend: crate::sketches::geometry::tests::Fake,
    book: RefCell<crate::sketches::Book<u64>>,
}
impl GeometryRig {
    fn new() -> Self {
        Self {
            common: Rig::new(),
            backend: Default::default(),
            book: RefCell::new(crate::sketches::geometry::tests::book()),
        }
    }
    fn request(&self, id: &str) -> PreparedRequest {
        let request = crate::sketches::geometry::tests::request(
            &self.book.borrow(),
            &self.backend,
            json!([{"type":"line","start":[0,0],"end":[10,20]},{"type":"circle","center":[30,40],"radius":5}]),
            id,
        );
        let mut args = request.arguments().clone();
        args["sessionId"] = json!(self.common.queue.epoch().to_string());
        prepare_request(args, &self.common.queue.epoch().to_string()).unwrap()
    }
    fn reply(&self, task: &mut Task) -> Result<Reply, String> {
        match task {
            Task::GeometryPreflight(request) => Ok(
                match self
                    .book
                    .borrow()
                    .geometry_preflight(&self.backend, request)
                {
                    Ok(p) => Reply::GeometryPlan(p),
                    Err(code) => Reply::Value(
                        json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
                    ),
                },
            ),
            Task::GeometryExecute(plan) => {
                // File-backed public journal must already prove executing before
                // this instrumented effect. This tests coordinator ordering itself.
                Ok(Reply::Value(
                    self.book
                        .borrow_mut()
                        .execute_geometry(&self.backend, plan.take().unwrap()),
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
        let client = self.common.spawn(request, Duration::from_secs(2));
        let deadline = Instant::now() + Duration::from_secs(3);
        while !client.is_finished() {
            assert!(Instant::now() < deadline);
            if let Some(mut claim) = self
                .common
                .queue
                .take_next(self.common.queue.epoch())
                .unwrap()
            {
                if matches!(claim.task_mut(), Task::GeometryExecute(_)) {
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
#[test]
fn geometry_durable_start_success_replay_conflict_never_repeat_insertion() {
    let rig = GeometryRig::new();
    let request = rig.request("geometry-once");
    let (code, body) = rig.execute(request.clone());
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["operationStatus"]["authority"], "durable_terminal");
    assert_eq!(rig.backend.calls.get(), 2);
    let (_, replay) = rig.execute(request.clone());
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["geometry"], body["geometry"]);
    assert_eq!(rig.backend.calls.get(), 2);
    let mut changed = request.arguments().clone();
    changed["items"][0]["end"] = json!([20, 20]);
    let (code, _) =
        rig.execute(prepare_request(changed, &rig.common.queue.epoch().to_string()).unwrap());
    assert_eq!(code, 409);
    assert_eq!(rig.backend.calls.get(), 2);
}
#[test]
fn geometry_partial_unknown_retains_actual_prefix_and_cannot_replay() {
    let rig = GeometryRig::new();
    let request = rig.request("geometry-unknown");
    rig.backend.mode.set(2);
    rig.backend.fail_at.set(2);
    let (_, body) = rig.execute(request.clone());
    assert_eq!(body["outcome"], "unknown", "{body}");
    assert_eq!(body["completedEntities"].as_array().unwrap().len(), 1);
    assert_eq!(rig.backend.calls.get(), 2);
    let (_, replay) = rig.execute(request);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["completedEntities"], body["completedEntities"]);
    assert_eq!(rig.backend.calls.get(), 2);
}
#[test]
fn geometry_durable_start_storage_loss_prevents_first_sdk_effect() {
    let rig = GeometryRig::new();
    let client = rig
        .common
        .spawn(rig.request("geometry-no-start"), Duration::from_secs(2));
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut claim = loop {
        assert!(Instant::now() < deadline);
        if let Some(c) = rig
            .common
            .queue
            .take_next(rig.common.queue.epoch())
            .unwrap()
        {
            break c;
        }
        thread::sleep(Duration::from_millis(1));
    };
    assert!(matches!(claim.task_mut(), Task::GeometryPreflight(_)));
    let reply = rig.reply(claim.task_mut());
    let database = rig
        .common
        .directory
        .join(fusion_mechanical_journal::DATABASE_NAME);
    let held = rig.common.directory.with_extension("held-geometry-sqlite");
    std::fs::rename(&database, &held).unwrap();
    claim.complete(reply).unwrap();
    assert_eq!(client.join().unwrap().0, 503);
    assert_eq!(rig.backend.calls.get(), 0);
    std::fs::rename(held, database).unwrap();
}
#[test]
fn geometry_entire_plan_scope_is_checked_before_journal_admission() {
    let rig = GeometryRig::new();
    let mut a = rig.request("geometry-pure-invalid").arguments().clone();
    a["items"][1] = json!({"type":"point","position":[0,0,1]});
    let (code, body) =
        rig.execute(prepare_request(a, &rig.common.queue.epoch().to_string()).unwrap());
    assert_eq!(code, 400);
    assert_eq!(body["errorCode"], "native_geometry_plan_scope_refused");
    assert_eq!(rig.backend.calls.get(), 0);
    assert!(rig
        .common
        .reader
        .journal()
        .unwrap()
        .status(
            &rig.common.queue.epoch().to_string(),
            "geometry-pure-invalid"
        )
        .unwrap()
        .durable
        .unwrap()
        .record
        .is_none());
}

#[test]
fn geometry_observed_completion_survives_terminal_storage_loss_without_retry() {
    let rig = GeometryRig::new();
    let request = rig.request("geometry-observed-storage-loss");
    let client = rig.common.spawn(request.clone(), Duration::from_secs(2));
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut claim = loop {
        assert!(Instant::now() < deadline);
        if let Some(mut c) = rig
            .common
            .queue
            .take_next(rig.common.queue.epoch())
            .unwrap()
        {
            if matches!(c.task_mut(), Task::GeometryExecute(_)) {
                break c;
            }
            let reply = rig.reply(c.task_mut());
            c.complete(reply).unwrap();
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    };
    let reply = rig.reply(claim.task_mut());
    assert_eq!(rig.backend.calls.get(), 2);
    let database = rig
        .common
        .directory
        .join(fusion_mechanical_journal::DATABASE_NAME);
    let held = rig
        .common
        .directory
        .with_extension("held-geometry-terminal-sqlite");
    std::fs::rename(&database, &held).unwrap();
    claim.complete(reply).unwrap();
    let (code, body) = client.join().unwrap();
    assert_eq!(code, 503);
    assert_eq!(body["geometry"]["entitiesCompleted"], 2, "{body}");
    assert_ne!(body["operationStatus"]["authority"], "durable_terminal");
    std::fs::rename(held, database).unwrap();
    let (_, replay) = rig.execute(request);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["geometry"]["entitiesCompleted"], 2);
    assert_eq!(rig.backend.calls.get(), 2);
}

struct ExtrudeRig {
    common: Rig,
    backend: crate::sketches::extrusion::tests::Fake,
    book: RefCell<crate::sketches::Book<u64>>,
}
impl ExtrudeRig {
    fn new() -> Self {
        Self {
            common: Rig::new(),
            backend: Default::default(),
            book: RefCell::new(crate::sketches::extrusion::tests::book()),
        }
    }
    fn request(&self, id: &str) -> PreparedRequest {
        let r = crate::sketches::extrusion::tests::request(&self.book.borrow(), &self.backend, id);
        let mut a = r.arguments().clone();
        a["sessionId"] = json!(self.common.queue.epoch().to_string());
        prepare_request(a, &self.common.queue.epoch().to_string()).unwrap()
    }
    fn reply(&self, task: &mut Task) -> Result<Reply, String> {
        match task {
            Task::ExtrudePreflight(r) => Ok(
                match self.book.borrow().extrude_preflight(&self.backend, r) {
                    Ok(p) => Reply::ExtrudePlan(p),
                    Err(code) => Reply::Value(
                        json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
                    ),
                },
            ),
            Task::ExtrudeExecute(p) => Ok(Reply::Value(
                self.book
                    .borrow_mut()
                    .execute_extrude(&self.backend, p.take().unwrap()),
            )),
            _ => Err("unexpected extrude task".into()),
        }
    }
    fn execute(&self, r: PreparedRequest) -> Response {
        let op = r.arguments()["operationId"].as_str().unwrap().to_owned();
        let client = self.common.spawn(r, Duration::from_secs(2));
        let end = Instant::now() + Duration::from_secs(3);
        while !client.is_finished() {
            assert!(Instant::now() < end);
            if let Some(mut c) = self
                .common
                .queue
                .take_next(self.common.queue.epoch())
                .unwrap()
            {
                if matches!(c.task_mut(), Task::ExtrudeExecute(_)) {
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
                        fusion_mechanical_journal::Phase::Executing
                    );
                }
                let reply = self.reply(c.task_mut());
                c.complete(reply).unwrap();
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        client.join().unwrap()
    }
}
#[test]
fn extrude_durable_start_and_replay_conflict_never_create_another_body() {
    let rig = ExtrudeRig::new();
    let r = rig.request("extrude-once");
    let (code, body) = rig.execute(r.clone());
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["data"]["volumeMm3"], 24000.);
    assert_eq!(body["operationStatus"]["authority"], "durable_terminal");
    let (_, replay) = rig.execute(r.clone());
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["data"], body["data"]);
    let mut a = r.arguments().clone();
    a["distance"] = json!(50);
    assert_eq!(
        rig.execute(prepare_request(a, &rig.common.queue.epoch().to_string()).unwrap())
            .0,
        409
    );
    assert_eq!(rig.backend.calls.get(), 1);
}
#[test]
fn extrude_unknown_ack_is_retained_without_reinvocation() {
    let rig = ExtrudeRig::new();
    let r = rig.request("extrude-unknown");
    rig.backend.mode.set(2);
    let (_, body) = rig.execute(r.clone());
    assert_eq!(body["outcome"], "unknown");
    let (_, replay) = rig.execute(r);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["outcome"], "unknown");
    assert_eq!(rig.backend.calls.get(), 1);
}
#[test]
fn extrude_explicit_start_input_failure_is_durable_and_replay_cannot_add() {
    for (mode, stage) in [(22, 213), (23, 214)] {
        let rig = ExtrudeRig::new();
        let r = rig.request("extrude-explicit-start-refused");
        rig.backend.mode.set(mode);
        let (status, body) = rig.execute(r.clone());
        assert_eq!(status, 400, "{body}");
        assert_eq!(body["errorCode"], "native_extrude_not_invoked");
        assert_eq!(body["nativeExecutionStarted"], false);
        assert_eq!(body["sdkDiagnostic"]["stage"], stage);
        assert_eq!(body["operationStatus"]["authority"], "durable_terminal");
        assert_eq!(body["operationStatus"]["reconciliationRequired"], false);
        // Even if a later invocation would now work, this operation is terminal.
        rig.backend.mode.set(0);
        let (_, replay) = rig.execute(r);
        assert_eq!(replay["replayed"], true);
        assert_eq!(replay["errorCode"], "native_extrude_not_invoked");
        assert_eq!(replay["sdkDiagnostic"], body["sdkDiagnostic"]);
        assert_eq!(rig.backend.calls.get(), 0);
    }
}
#[test]
fn extrude_unsupported_taper_is_refused_before_admission() {
    let rig = ExtrudeRig::new();
    let mut a = rig.request("extrude-taper").arguments().clone();
    a["taper"] = json!(1);
    let (code, body) =
        rig.execute(prepare_request(a, &rig.common.queue.epoch().to_string()).unwrap());
    assert_eq!(code, 400);
    assert_eq!(body["errorCode"], "native_extrude_subset_required");
    assert_eq!(rig.backend.calls.get(), 0);
    assert!(rig
        .common
        .reader
        .journal()
        .unwrap()
        .status(&rig.common.queue.epoch().to_string(), "extrude-taper")
        .unwrap()
        .durable
        .unwrap()
        .record
        .is_none());
}
#[test]
fn extrude_start_storage_failure_prevents_add() {
    let rig = ExtrudeRig::new();
    let client = rig
        .common
        .spawn(rig.request("extrude-no-start"), Duration::from_secs(2));
    let end = Instant::now() + Duration::from_secs(3);
    let mut claim = loop {
        assert!(Instant::now() < end);
        if let Some(c) = rig
            .common
            .queue
            .take_next(rig.common.queue.epoch())
            .unwrap()
        {
            break c;
        }
        thread::sleep(Duration::from_millis(1));
    };
    let reply = rig.reply(claim.task_mut());
    let database = rig
        .common
        .directory
        .join(fusion_mechanical_journal::DATABASE_NAME);
    let held = rig.common.directory.with_extension("held-extrude-start");
    std::fs::rename(&database, &held).unwrap();
    claim.complete(reply).unwrap();
    assert_eq!(client.join().unwrap().0, 503);
    assert_eq!(rig.backend.calls.get(), 0);
    std::fs::rename(held, database).unwrap();
}
#[test]
fn extrude_observed_solid_survives_terminal_storage_loss_without_retry() {
    let rig = ExtrudeRig::new();
    let r = rig.request("extrude-terminal-loss");
    let client = rig.common.spawn(r.clone(), Duration::from_secs(2));
    let end = Instant::now() + Duration::from_secs(3);
    let mut claim = loop {
        assert!(Instant::now() < end);
        if let Some(mut c) = rig
            .common
            .queue
            .take_next(rig.common.queue.epoch())
            .unwrap()
        {
            if matches!(c.task_mut(), Task::ExtrudeExecute(_)) {
                break c;
            }
            let reply = rig.reply(c.task_mut());
            c.complete(reply).unwrap();
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    };
    let reply = rig.reply(claim.task_mut());
    let database = rig
        .common
        .directory
        .join(fusion_mechanical_journal::DATABASE_NAME);
    let held = rig.common.directory.with_extension("held-extrude-terminal");
    std::fs::rename(&database, &held).unwrap();
    claim.complete(reply).unwrap();
    let (code, body) = client.join().unwrap();
    assert_eq!(code, 503);
    assert_eq!(body["data"]["volumeMm3"], 24000.);
    assert_ne!(body["operationStatus"]["authority"], "durable_terminal");
    std::fs::rename(held, database).unwrap();
    let (_, replay) = rig.execute(r);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["data"]["volumeMm3"], 24000.);
    assert_eq!(rig.backend.calls.get(), 1);
}

struct RelationRig {
    common: Rig,
    backend: crate::sketches::relations::tests::Fake,
    book: RefCell<crate::sketches::Book<u64>>,
}
#[test]
fn relation_durable_start_exact_replay_and_request_conflict_never_repeat_add() {
    let rig = RelationRig::new();
    let request = rig.request("relation-once");
    let (code, body) = rig.execute(request.clone());
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["operationStatus"]["authority"], "durable_terminal");
    assert_eq!(rig.backend.calls_add.get(), 1);
    let (_, replay) = rig.execute(request.clone());
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["data"], body["data"]);
    assert_eq!(rig.backend.calls_add.get(), 1);
    let mut args = request.arguments().clone();
    args["type"] = json!("vertical");
    assert_eq!(
        rig.execute(prepare_request(args, &rig.common.queue.epoch().to_string()).unwrap())
            .0,
        409
    );
    assert_eq!(rig.backend.calls_add.get(), 1);
}
#[test]
fn relation_unknown_ack_is_retained_without_replay_or_success_substitution() {
    let rig = RelationRig::new();
    let request = rig.request("relation-unknown");
    rig.backend.post_mode.set(12);
    let (_, body) = rig.execute(request.clone());
    assert_eq!(body["outcome"], "unknown", "{body}");
    assert_eq!(rig.backend.calls_add.get(), 1);
    let (_, replay) = rig.execute(request);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["outcome"], "unknown");
    assert_eq!(rig.backend.calls_add.get(), 1);
}
#[test]
fn relation_late_observed_result_survives_http_deadline_without_second_add() {
    let rig = RelationRig::new();
    let request = rig.request("relation-late");
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
            if matches!(c.task(), Task::RelationExecute(_)) {
                break c;
            }
            let reply = rig.reply(c.task_mut());
            c.complete(reply).unwrap();
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    };
    assert_eq!(rig.backend.calls_add.get(), 0);
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
            .status(&rig.common.queue.epoch().to_string(), "relation-late")
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
    assert_eq!(rig.backend.calls_add.get(), 1);
}
#[test]
fn relation_unsupported_shape_has_no_effect_or_journal_admission() {
    let rig = RelationRig::new();
    let mut args = rig.request("relation-invalid").arguments().clone();
    args["type"] = json!("fix");
    let (code, _) =
        rig.execute(prepare_request(args, &rig.common.queue.epoch().to_string()).unwrap());
    assert_eq!(code, 400);
    assert_eq!(rig.backend.calls_add.get(), 0);
    assert!(rig
        .common
        .reader
        .journal()
        .unwrap()
        .status(&rig.common.queue.epoch().to_string(), "relation-invalid")
        .unwrap()
        .durable
        .unwrap()
        .record
        .is_none());
}
#[test]
fn relation_storage_loss_before_start_or_terminal_commit_never_repeats_effect() {
    for after_effect in [false, true] {
        let rig = RelationRig::new();
        let request = rig.request("relation-storage");
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
                if !after_effect || matches!(c.task(), Task::RelationExecute(_)) {
                    break c;
                }
                let result = rig.reply(c.task_mut());
                c.complete(result).unwrap();
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        };
        let result = rig.reply(claim.task_mut());
        assert_eq!(rig.backend.calls_add.get(), usize::from(after_effect));
        let path = rig
            .common
            .directory
            .join(fusion_mechanical_journal::DATABASE_NAME);
        let held = rig.common.directory.with_extension("held-relation");
        std::fs::rename(&path, &held).unwrap();
        claim.complete(result).unwrap();
        let (code, body) = client.join().unwrap();
        assert_eq!(code, 503, "{body}");
        assert_ne!(body["operationStatus"]["authority"], "durable_terminal");
        std::fs::rename(held, path).unwrap();
        let (_, again) = rig.execute(request);
        assert_ne!(again["operationStatus"]["authority"], "durable_terminal");
        assert_eq!(rig.backend.calls_add.get(), usize::from(after_effect));
        if after_effect {
            let status = rig
                .common
                .reader
                .journal()
                .unwrap()
                .status(&rig.common.queue.epoch().to_string(), "relation-storage")
                .unwrap();
            assert_eq!(status.observed.unwrap().outcome, ObservedOutcome::Succeeded);
        }
    }
}
impl RelationRig {
    fn new() -> Self {
        let (book, backend) = crate::sketches::relations::tests::setup();
        Self {
            common: Rig::new(),
            backend,
            book: RefCell::new(book),
        }
    }
    fn request(&self, id: &str) -> PreparedRequest {
        crate::sketches::relations::tests::request(
            &self.book.borrow(),
            &self.backend,
            &self.common.queue.epoch().to_string(),
            id,
            "horizontal",
        )
    }
    fn reply(&self, task: &mut Task) -> Result<Reply, String> {
        match task {
            Task::RelationPreflight(request) => Ok(
                match self
                    .book
                    .borrow()
                    .relation_preflight(&self.backend, request)
                {
                    Ok(p) => Reply::RelationPlan(p),
                    Err(code) => Reply::Value(
                        json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
                    ),
                },
            ),
            Task::RelationExecute(plan) => {
                // File-backed public journal must already prove executing before
                // this instrumented effect. This tests coordinator ordering itself.
                Ok(Reply::Value(
                    self.book
                        .borrow_mut()
                        .execute_relation(&self.backend, plan.take().unwrap()),
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
                if matches!(claim.task_mut(), Task::RelationExecute(_)) {
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

include!("execution_dimensions_tests.rs");

include!("execution_assembly_tests.rs");

include!("execution_placement_tests.rs");

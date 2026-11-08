use super::placement::*;
use super::*;
use std::cell::{Cell, RefCell};
pub(crate) struct PlacementFake {
    pub state: RefCell<Projection>,
    pub snapshots: RefCell<Vec<SnapshotFact>>,
    pub transforms: Cell<usize>,
    pub captures: Cell<usize>,
    pub mode: Cell<u32>,
    reads: Cell<usize>,
    position_reads: Cell<usize>,
    diagnostic: RefCell<Value>,
}
impl Default for PlacementFake {
    fn default() -> Self {
        let mut p = tests::empty();
        let mut c = tests::component();
        c.part_number = None;
        c.description = None;
        p.components = vec![c.clone(), c];
        p.occurrences = vec![
            tests::occurrence(0),
            tests::occurrence(1),
            tests::occurrence(0),
        ];
        p.root.components = 3;
        p.root.occurrences = 3;
        p.root.timeline = 3;
        p.root.timeline_position = 3;
        Self {
            state: RefCell::new(p),
            snapshots: RefCell::new(vec![]),
            transforms: Cell::new(0),
            captures: Cell::new(0),
            mode: Cell::new(0),
            reads: Cell::new(0),
            position_reads: Cell::new(0),
            diagnostic: RefCell::new(Value::Null),
        }
    }
}
impl Backend for PlacementFake {
    type Handle = u64;
    fn open(&self) -> Open<u64> {
        Open {
            handle: Some(1),
            acknowledged: true,
        }
    }
    fn command(&self) -> Result<String> {
        if self.mode.get() == 13 && self.transforms.get() > 0 {
            panic!("command after transform")
        }
        Ok("SelectCommand".into())
    }
    fn snapshot(&self, _: &u64, cloud: bool) -> Result<Projection> {
        assert!(!cloud);
        self.reads.set(self.reads.get() + 1);
        if self.transforms.get() > 0 {
            if self.mode.get() == 12 {
                panic!("capture after effect")
            }
            if self.mode.get() == 14 {
                return Err("sdk_read_error".into());
            }
        }
        let mut p = self.state.borrow().clone();
        if self.mode.get() == 15 && self.transforms.get() > 0 {
            p.root.revision = format!("read{}", self.reads.get())
        }
        Ok(p)
    }
    fn capture(&self, h: &u64, cloud: bool) -> Result<Capture> {
        let projection = self.snapshot(h, cloud)?;
        let mut root_after = projection.root.clone();
        if self.mode.get() == 16 && self.transforms.get() > 0 {
            root_after.revision = "drift".into()
        }
        Ok(Capture {
            projection,
            root_after,
        })
    }
    fn add(&self, _: &u64, _: Option<usize>) -> Attempt {
        panic!("not allowed")
    }
    fn change(&self, _: &u64, _: usize, _: &Change) -> Attempt {
        panic!("not allowed")
    }
    fn positions(&self, _: &u64) -> Result<PositionState> {
        if self.transforms.get() > 0 {
            self.position_reads.set(self.position_reads.get() + 1);
            if self.mode.get() == 23 && self.position_reads.get() == 2 {
                self.state.borrow_mut().root.revision = "position-getter-drift".into();
            }
        }
        let p = self.state.borrow();
        Ok(PositionState {
            pending: p.root.pending_snapshot,
            timeline: p.root.timeline,
            timeline_position: p.root.timeline_position,
            snapshots: self.snapshots.borrow().clone(),
        })
    }
    fn translate(&self, _: &u64, oi: usize, m: &[f64; 16]) -> Attempt {
        self.transforms.set(self.transforms.get() + 1);
        let mode = self.mode.get();
        if mode == 1 {
            return Attempt::default();
        }
        if mode == 11 {
            panic!("setter panic")
        }
        let mut p = self.state.borrow_mut();
        p.occurrences[oi].matrix = *m;
        p.root.revision = format!("pose{}", self.transforms.get());
        p.root.pending_snapshot = mode != 3;
        if mode == 4 {
            p.occurrences[0].matrix[3] = 99.
        }
        if mode == 5 {
            p.components[1].revision = "foreign".into()
        }
        if mode == 6 {
            p.root.name = "foreign".into()
        }
        if mode == 17 {
            p.occurrences[oi].matrix[7] += 1.
        }
        Attempt {
            invoked: true,
            acknowledged: mode != 2,
            component: Some(if mode == 18 {
                1
            } else {
                p.occurrences[oi].component
            }),
            occurrence: Some(oi),
        }
    }
    fn capture_position(&self, _: &u64) -> SnapshotAttempt {
        self.captures.set(self.captures.get() + 1);
        if self.mode.get() == 19 {
            panic!("snapshot panic")
        }
        let mut p = self.state.borrow_mut();
        let mut ss = self.snapshots.borrow_mut();
        let slot = ss.len();
        ss.push(SnapshotFact {
            slot,
            timeline_index: p.root.timeline,
            health: if self.mode.get() == 10 { 2 } else { 0 },
        });
        p.root.timeline += 1;
        p.root.timeline_position = p.root.timeline;
        p.root.pending_snapshot = self.mode.get() == 8;
        p.root.revision = format!("snapshot{}", self.captures.get());
        if self.mode.get() == 9 {
            ss.last_mut().unwrap().timeline_index = 0
        }
        if self.mode.get() == 20 {
            p.occurrences[1].matrix[11] = 1.
        }
        if self.mode.get() == 21 {
            p.components[0].revision = "changed-after-capture".into()
        }
        SnapshotAttempt {
            attempt: Attempt {
                invoked: true,
                acknowledged: self.mode.get() != 7,
                component: None,
                occurrence: None,
            },
            snapshot_slot: Some(if self.mode.get() == 22 {
                slot + 1
            } else {
                slot
            }),
        }
    }
    fn record_observation_diagnostic(&self, v: Value) {
        self.diagnostic.replace(v);
    }
    fn diagnostic(&self) -> Value {
        self.diagnostic.borrow().clone()
    }
}
pub(crate) fn request_for(
    book: &mut Book<u64>,
    b: &PlacementFake,
    epoch: &str,
    id: &str,
    position: [f64; 3],
) -> PreparedRequest {
    let v = book.inspect(b, tests::DOC, None, false).unwrap();
    let t = &v["data"]["target"];
    tests::prepare(
        json!({"op":"occurrence_update","sessionId":epoch,"operationId":id,"target":t,"occurrence":{"documentId":t["documentId"],"revision":t["revision"],"objectType":"Occurrence","occurrenceId":v["data"]["occurrences"][2]["occurrenceId"]},"transform":{"position":position}}),
        epoch,
    )
}
const E: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
#[test]
fn placement_two_absolute_moves_preserve_definitions_siblings_and_publish_exact_stages() {
    let b = PlacementFake::default();
    let mut book = Book::default();
    let original = b.state.borrow().clone();
    for (n, p) in [[25., -10., 5.], [-5., 20., 10.]].into_iter().enumerate() {
        let r = request_for(&mut book, &b, E, &format!("move{n}"), p);
        let plan = book.preflight(&b, &r).unwrap();
        let v = book.execute(&b, plan);
        assert_eq!(v["success"], true, "{v}");
        assert_eq!(
            v["placementEvidence"]["after"]["snapshots"]
                .as_array()
                .unwrap()
                .len(),
            n + 1
        );
        assert_eq!(v["stages"][0]["attempt"]["component"], 0);
        assert!(v["stages"][1]["attempt"]["component"].is_null());
        assert_eq!(v["stages"][1]["snapshotSlot"], n);
        assert_eq!(v["data"]["occurrences"][2]["matrix"], json!(matrix(&p)));
        assert_eq!(v["occurrenceRef"]["occurrenceId"], v["occurrenceId"]);
        let state = b.state.borrow();
        assert_eq!(state.components, original.components);
        assert_eq!(state.occurrences[..2], original.occurrences[..2]);
        drop(state);
        assert!(book.inspect(&b, tests::DOC, None, false).is_ok());
    }
    assert_eq!(b.transforms.get(), 2);
    assert_eq!(b.captures.get(), 2);
}
#[test]
fn placement_partial_effect_failures_never_capture_or_retry() {
    for mode in [2, 3, 4, 5, 6, 11, 12, 13, 14, 15, 16, 17, 18] {
        let b = PlacementFake::default();
        let mut book = Book::default();
        let r = request_for(&mut book, &b, E, "fault", [25., -10., 5.]);
        let plan = book.preflight(&b, &r).unwrap();
        b.mode.set(mode);
        let v = book.execute(&b, plan);
        assert_eq!(v["outcome"], "unknown", "mode{mode}: {v}");
        assert_eq!(v["reconciliationRequired"], true);
        assert_eq!(b.transforms.get(), 1);
        assert_eq!(b.captures.get(), 0);
        assert!(book.faulted);
        assert!(book.preflight(&b, &r).is_err());
        if matches!(mode, 15 | 16) {
            assert!(v["sdkDiagnostic"].is_object(), "{v}")
        }
    }
}
#[test]
fn placement_snapshot_failures_preserve_both_attempts_and_block_reentry() {
    for mode in [7, 8, 9, 10, 19, 20, 21, 22] {
        let b = PlacementFake::default();
        let mut book = Book::default();
        let r = request_for(&mut book, &b, E, "fault", [25., -10., 5.]);
        let plan = book.preflight(&b, &r).unwrap();
        b.mode.set(mode);
        let v = book.execute(&b, plan);
        assert_eq!(v["outcome"], "unknown", "mode{mode}: {v}");
        assert_eq!(v["stages"].as_array().unwrap().len(), 2);
        assert_eq!((b.transforms.get(), b.captures.get()), (1, 1));
        assert!(book.faulted);
    }
}
#[test]
fn placement_proven_pre_invocation_refusal_does_not_poison_book() {
    let b = PlacementFake::default();
    let mut book = Book::default();
    let r = request_for(&mut book, &b, E, "no-call", [25., -10., 5.]);
    let plan = book.preflight(&b, &r).unwrap();
    b.mode.set(1);
    let v = book.execute(&b, plan);
    assert_eq!(v["nativeExecutionStarted"], false);
    assert!(v.get("outcome").is_none());
    assert!(!book.faulted);
    assert_eq!(b.captures.get(), 0);
}
#[test]
fn placement_noop_grounding_stale_foreign_capacity_refuse_before_effect() {
    for mode in 0..8 {
        let b = PlacementFake::default();
        let mut book = Book::default();
        if mode == 1 {
            b.state.borrow_mut().occurrences[2].grounded = true
        }
        if mode == 2 {
            b.state.borrow_mut().occurrences[2].grounded_to_parent = true
        }
        if mode == 5 {
            let mut p = b.state.borrow_mut();
            p.root.timeline = 67;
            p.root.timeline_position = 67;
            *b.snapshots.borrow_mut() = (0..64)
                .map(|i| SnapshotFact {
                    slot: i,
                    timeline_index: 3 + i as u64,
                    health: 0,
                })
                .collect();
        }
        if mode == 7 {
            let mut p = b.state.borrow_mut();
            p.root.timeline = 4096;
            p.root.timeline_position = 4096;
        }
        let r = request_for(
            &mut book,
            &b,
            E,
            "refuse",
            if mode == 0 { [0.; 3] } else { [25., -10., 5.] },
        );
        let mut a = r.arguments().clone();
        if mode == 3 {
            a["target"]["revision"] = json!(format!("{PREFIX}{}", "0".repeat(64)));
            a["occurrence"]["revision"] = a["target"]["revision"].clone();
        }
        if mode == 4 {
            a["occurrence"]["occurrenceId"] = json!("foreign");
        }
        if mode == 6 {
            b.state.borrow_mut().root.pending_snapshot = true;
        }
        let r = tests::prepare(a, E);
        assert!(book.preflight(&b, &r).is_err(), "mode{mode}");
        assert_eq!((b.transforms.get(), b.captures.get()), (0, 0));
    }
}
#[test]
fn placement_argument_refusals_are_static_and_do_not_discard_legacy_fields() {
    let b = PlacementFake::default();
    let mut book = Book::default();
    let r = request_for(&mut book, &b, E, "args", [25., -10., 5.]);
    for (key, value, code) in [
        (
            "grounded",
            json!(false),
            "native_placement_grounding_changes_unsupported",
        ),
        (
            "groundedToParent",
            json!(true),
            "native_placement_grounding_changes_unsupported",
        ),
        (
            "transform",
            json!({"position":[1,2,3],"axis":[1,0,0]}),
            "native_placement_position_only",
        ),
        (
            "transform",
            json!({"position":[1,2,3],"angle":0}),
            "native_placement_position_only",
        ),
        (
            "transform",
            json!({"position":[1000001,0,0]}),
            "native_placement_position_bound",
        ),
    ] {
        let mut a = r.arguments().clone();
        a[key] = value;
        let r = tests::prepare(a, E);
        assert_eq!(supported_arguments(&r).unwrap_err(), code);
    }
    let mut a = r.arguments().clone();
    a["occurrence"]["documentId"] = json!("foreign");
    assert_eq!(
        supported_arguments(&tests::prepare(a, E)).unwrap_err(),
        "native_placement_exact_occurrence_required"
    );
    assert_eq!(b.transforms.get(), 0);
}
#[test]
fn placement_state_between_preflight_and_execute_refuses_without_effect() {
    let b = PlacementFake::default();
    let mut book = Book::default();
    let r = request_for(&mut book, &b, E, "race", [25., -10., 5.]);
    let plan = book.preflight(&b, &r).unwrap();
    b.state.borrow_mut().occurrences[0].matrix[3] = 2.;
    let v = book.execute(&b, plan);
    assert_eq!(v["errorCode"], "target_revision_mismatch");
    assert_eq!(v["nativeExecutionStarted"], false);
    assert_eq!((b.transforms.get(), b.captures.get()), (0, 0));
}
#[test]
fn placement_maximum_snapshot_result_stays_within_reserved_result_budget() {
    let b = PlacementFake::default();
    let mut book = Book::default();
    {
        let mut p = b.state.borrow_mut();
        p.root.timeline = 66;
        p.root.timeline_position = 66;
        *b.snapshots.borrow_mut() = (0..63)
            .map(|i| SnapshotFact {
                slot: i,
                timeline_index: 3 + i as u64,
                health: 0,
            })
            .collect();
        p.components = (0..MAX_COMPONENTS)
            .map(|_| {
                let mut c = tests::component();
                c.part_number = None;
                c.description = None;
                c
            })
            .collect();
        p.occurrences = (0..MAX_OCCURRENCES)
            .map(|i| tests::occurrence(i % MAX_COMPONENTS))
            .collect();
        p.root.components = MAX_COMPONENTS as u64 + 1;
        p.root.occurrences = MAX_OCCURRENCES as u64;
        for i in 0..120 {
            p.components[0]
                .attributes
                .insert(format!("padding{i}"), "x".repeat(1024));
            if serde_json::to_vec(&*p).unwrap().len() > MAX_BYTES {
                p.components[0].attributes.remove(&format!("padding{i}"));
                break;
            }
        }
    }
    let r = request_for(&mut book, &b, E, "max", [1000000., -1000000., 1000000.]);
    let plan = book.preflight(&b, &r).unwrap();
    let v = book.execute(&b, plan);
    assert_eq!(v["success"], true, "{v}");
    assert!(serde_json::to_vec(&v).unwrap().len() < 256 * 1024);
}

#[test]
fn placement_position_getter_revision_change_is_inside_final_root_bracket() {
    let b = PlacementFake::default();
    let mut book = Book::default();
    let r = request_for(&mut book, &b, E, "getter-drift", [25., -10., 5.]);
    let plan = book.preflight(&b, &r).unwrap();
    b.mode.set(23);
    let v = book.execute(&b, plan);
    assert_eq!(v["outcome"], "unknown", "{v}");
    assert_eq!(v["errorCode"], "native_placement_projection_changed");
    assert_eq!((b.transforms.get(), b.captures.get()), (1, 0));
    assert_eq!(
        v["sdkDiagnostic"]["firstProjectionDifference"]["path"],
        "/root/revision"
    );
}

#[test]
fn placement_unsupported_read_projection_is_known_refusal_but_sdk_failure_is_not() {
    for (cell, value, expected) in [
        (0, 0.5, "native_assembly_identity_orientation_required"),
        (3, 100001., "native_placement_position_bound"),
    ] {
        let b = PlacementFake::default();
        b.state.borrow_mut().occurrences[2].matrix[cell] = value;
        let mut book = Book::default();
        let error = book.inspect(&b, tests::DOC, None, false).unwrap_err();
        assert_eq!(error, expected);
        assert!(known_refusal(&error));
        assert_eq!((b.transforms.get(), b.captures.get()), (0, 0));
    }
    assert!(!known_refusal("native_assembly_sdk_status_7"));
    assert!(!known_refusal("native_placement_projection_changed"));
    assert!(!known_refusal("native_placement_transform_unconfirmed"));
}

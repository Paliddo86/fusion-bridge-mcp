use super::*;
use crate::sketches::geometry::{GeometryBackend, InsertAttempt, ObservedEntity};
use crate::sketches::line_state::{Flags, Point, State};
use std::cell::{Cell, RefCell};
const DOC: &str = "d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001";
const ID: &str = "s:d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001:0000000000000001";
fn eid() -> String {
    format!("e:{ID}:0000000000000001")
}
fn flags() -> Flags {
    Flags {
        fixed: false,
        reference: false,
        has_referenced_entity: false,
        fully_constrained: false,
        is_2d: true,
        constraints: 0,
        dimensions: 0,
        attributes: 0,
    }
}
fn state() -> State {
    let p = |xyz, c| Point {
        position_cm: xyz,
        flags: flags(),
        connected_entities: c,
    };
    State {
        line: flags(),
        construction: false,
        start: p([1., 2., 0.], 1),
        end: p([3., 5., 0.], 1),
        origin: p([0.; 3], 0),
        root_bodies: 0,
        root_features: 0,
        root_occurrences: 0,
        root_meshes: 0,
        design_components: 1,
        sketch_texts: 0,
        sketch_attributes: 0,
    }
}
pub(crate) struct Fake {
    pub calls_add: Cell<usize>,
    kind: Cell<u32>,
    pub post_reads: Cell<usize>,
    pub post_mode: Cell<u8>,
    state: RefCell<State>,
    calls: Cell<usize>,
    reads: Cell<usize>,
    mode: Cell<u8>,
}
impl Backend for Fake {
    type Handle = u64;
    fn root(&self) -> Result<RootState, String> {
        Ok(RootState {
            active: true,
            parametric: true,
            hybrid: true,
            configured: false,
            root_revision: if self.kind.get() == 0 {
                "root"
            } else {
                "after"
            }
            .into(),
            sketch_count: 1,
            timeline_count: 1,
            timeline_position: 1,
        })
    }
    fn command(&self) -> Result<String, String> {
        if self.kind.get() != 0 {
            if self.post_mode.get() == 13
                || (self.post_mode.get() == 14 && self.post_reads.get() >= 2)
            {
                return Ok("OtherCommand".into());
            }
            if self.post_mode.get() == 15 && self.post_reads.get() >= 2 {
                panic!("late command getter")
            }
        }
        Ok("SelectCommand".into())
    }
    fn create(&self, _: u32) -> CreateAttempt<u64> {
        panic!("read must not create")
    }
    fn read(&self, h: &u64) -> Result<SketchState, String> {
        assert_eq!(*h, 7);
        self.reads.set(self.reads.get() + 1);
        if self.mode.get() == 4 {
            return Err("native_sdk_error_7".into());
        }
        Ok(SketchState {
            name: "Sketch1".into(),
            sdk_revision: if self.kind.get() == 0 {
                "sketch"
            } else {
                "after"
            }
            .into(),
            plane: 1,
            transform: [0.; 16],
            curves: 1,
            points: 3,
            profiles: 0,
            dimensions: 0,
            constraints: u64::from(self.kind.get() != 0),
            health: 0,
            deferred: false,
        })
    }
}
impl GeometryBackend for Fake {
    fn empty_origin_only(&self, _: &u64) -> Result<bool, String> {
        panic!("read mutation preparation")
    }
    fn insert(&self, _: &u64, _: &geometry_preparation::Entity) -> InsertAttempt {
        panic!("read mutation")
    }
    fn entity(&self, _: &u64, lease: u64) -> Result<ObservedEntity, String> {
        assert_eq!(lease, 19);
        let l = self.state.borrow();
        Ok(ObservedEntity {
            geometry: geometry_preparation::Geometry::Line {
                start_cm: l.start.position_cm,
                end_cm: l.end.position_cm,
            },
            reference: l.line.reference,
            construction: Some(l.construction),
        })
    }
    fn entity_equalities(&self, _: &u64, _: &[u64]) -> Result<Vec<Vec<bool>>, String> {
        panic!("published identity must be reused")
    }
}
impl LineBackend for Fake {
    fn line_state(&self, _: &u64, lease: u64) -> Result<State, String> {
        assert_eq!(lease, 19);
        let n = self.calls.get() + 1;
        self.calls.set(n);
        match self.mode.get() {
            1 => return Err("native_line_sdk_read_failed_status_6_stage_425_error_0".into()),
            2 => panic!("readback panic"),
            _ => {}
        }
        let mut s = self.state.borrow().clone();
        if self.mode.get() == 3 && n % 2 == 0 {
            s.line.fixed = true;
        }
        Ok(s)
    }
}
pub(crate) fn setup() -> (Book<u64>, Fake) {
    let mut b = Book::default();
    b.entries.push(Entry {
        id: ID.into(),
        document_id: DOC.into(),
        handle: Some(7),
        retired: Cell::new(false),
        profile_published: Cell::new(false),
        profile_retired: Cell::new(false),
        geometry_leases: Some(vec![19]),
        entities: RefCell::new(Some(entities::Publication { aliases: vec![0] })),
        entities_retired: Cell::new(false),
        dimension_endpoints_published: Cell::new(false),
    });
    (
        b,
        Fake {
            state: RefCell::new(state()),
            calls: Cell::new(0),
            reads: Cell::new(0),
            mode: Cell::new(0),
            calls_add: Cell::new(0),
            kind: Cell::new(0),
            post_reads: Cell::new(0),
            post_mode: Cell::new(0),
        },
    )
}
impl RelationBackend for Fake {
    fn relation_state(&self, h: &u64, lease: u64) -> Result<RelationState, String> {
        assert_eq!((*h, lease), (7, 19));
        let n = self.post_reads.get() + 1;
        self.post_reads.set(n);
        if self.kind.get() != 0 {
            match self.post_mode.get() {
                7 => {
                    return Err("native_relation_sdk_read_failed_status_6_stage_506_error_0".into())
                }
                8 => panic!("post-add relation getter"),
                9 if n >= 2 => panic!("late post-add getter"),
                _ => {}
            }
        }
        let mut line = self.state.borrow().clone();
        let mut kind = self.kind.get();
        if kind != 0 {
            match self.post_mode.get() {
                1 => kind = if kind == 1 { 2 } else { 1 },
                2 => line.origin.flags.fixed = true,
                3 => line.start.flags.fixed = true,
                4 => line.end.flags.constraints = 2,
                5 => line.end.position_cm[1] += 0.5,
                6 if n >= 2 => line.end.position_cm[0] += 0.25,
                _ => {}
            }
        }
        Ok(RelationState { kind, line })
    }
    fn constrain(&self, h: &u64, lease: u64, kind: u32) -> Attempt {
        assert_eq!((*h, lease), (7, 19));
        let mode = self.post_mode.get();
        if mode == 10 {
            return Attempt {
                invoked: false,
                acknowledged: false,
            };
        }
        self.calls_add.set(self.calls_add.get() + 1);
        assert_eq!(self.calls_add.get(), 1, "duplicate effect forbidden");
        if mode == 11 {
            panic!("unknown add")
        }
        self.kind.set(kind);
        let mut l = self.state.borrow_mut();
        l.line.constraints = 1;
        if kind == 1 {
            l.end.position_cm[1] = l.start.position_cm[1]
        } else {
            l.end.position_cm[0] = l.start.position_cm[0]
        }
        Attempt {
            invoked: true,
            acknowledged: mode != 12,
        }
    }
}
pub(crate) fn request(
    b: &Book<u64>,
    s: &Fake,
    epoch: &str,
    op: &str,
    typ: &str,
) -> PreparedRequest {
    let observed = b.inspect_line_state(s, DOC, ID, &eid(), None).unwrap();
    let rev = &observed["target"]["revision"];
    fusion_mechanical_core::prepare_request(json!({"op":"sketch_constraint","sessionId":epoch,"operationId":op,"target":observed["target"],"sketch":{"documentId":DOC,"revision":rev,"objectType":"Sketch","sketchId":ID},"type":typ,"entities":[{"documentId":DOC,"revision":rev,"objectType":"SketchLine","sketchId":ID,"entityId":eid()}]}),epoch).unwrap()
}
fn req(b: &Book<u64>, s: &Fake, typ: &str) -> PreparedRequest {
    request(b, s, &"a".repeat(32), "relation", typ)
}
#[test]
fn both_directions_have_exact_graph_stable_readback_and_no_length_promise() {
    for typ in ["horizontal", "vertical"] {
        let (mut b, s) = setup();
        let r = req(&b, &s, typ);
        let p = b.relation_preflight(&s, &r).unwrap();
        let before = s.state.borrow().clone();
        let v = b.execute_relation(&s, p);
        assert_eq!(v["success"], true, "{v}");
        assert_eq!(s.calls_add.get(), 1);
        assert!(!b.faulted);
        let data = b
            .inspect_relations(
                &s,
                DOC,
                ID,
                &eid(),
                v["data"]["target"]["revision"].as_str(),
            )
            .unwrap();
        assert_eq!(data, v["data"]);
        assert_eq!(data["solverPlacementUniquelySpecified"], false);
        assert_eq!(s.state.borrow().origin, before.origin);
        assert!(b.relation_preflight(&s, &r).is_err());
        assert_eq!(s.calls_add.get(), 1);
    }
}
#[test]
fn unsupported_type_refs_and_arity_refuse_before_backend() {
    let (b, s) = setup();
    let r = req(&b, &s, "horizontal");
    for case in 0..7 {
        let mut a = r.arguments().clone();
        match case {
            0 => a["type"] = json!("fix"),
            1 => a["entities"][0]["entityToken"] = json!("substitute"),
            2 => a["sketch"]["objectType"] = json!("SketchLine"),
            3 => a["entities"][0]["documentId"] = json!("other"),
            4 => {
                let e = a["entities"][0].clone();
                a["entities"].as_array_mut().unwrap().push(e);
            }
            5 => a["target"]["revision"] = json!("profile-projection-v1:bad"),
            _ => a["entities"][0]["revision"] = json!("other"),
        };
        let p = fusion_mechanical_core::prepare_request(a, &"a".repeat(32)).unwrap();
        let count = s.reads.get();
        assert!(b.relation_preflight(&s, &p).is_err());
        assert_eq!(s.reads.get(), count);
        assert_eq!(s.calls_add.get(), 0);
    }
}
#[test]
fn stale_unfixed_alignment_and_final_read_guards_prevent_invocation() {
    for case in 0..5 {
        let (mut b, s) = setup();
        let r = req(&b, &s, "horizontal");
        if case == 0 {
            s.state.borrow_mut().line.fixed = true;
            assert!(b.relation_preflight(&s, &r).is_err());
        } else {
            let p = b.relation_preflight(&s, &r).unwrap();
            match case {
                1 => s.state.borrow_mut().end.position_cm[0] += 1.,
                2 => s.mode.set(4),
                3 => s.mode.set(2),
                _ => s.post_mode.set(10),
            };
            let v = b.execute_relation(&s, p);
            assert_eq!(v["nativeExecutionStarted"], false, "{v}");
            assert_ne!(v["success"], true);
        }
        assert_eq!(s.calls_add.get(), 0);
    }
}
#[test]
fn acknowledged_but_wrong_or_unreadable_relation_stays_unknown_and_poisoned() {
    for mode in [1, 2, 3, 4, 5, 6, 7, 8, 9, 13, 14, 15] {
        let (mut b, s) = setup();
        let r = req(&b, &s, "horizontal");
        let p = b.relation_preflight(&s, &r).unwrap();
        s.post_mode.set(mode);
        let v = b.execute_relation(&s, p);
        assert_eq!(v["outcome"], "unknown", "mode{mode}: {v}");
        assert_eq!(v["reconciliationRequired"], true);
        assert_eq!(s.calls_add.get(), 1);
        assert!(b.faulted);
        assert!(b.relation_preflight(&s, &r).is_err());
        assert_eq!(s.calls_add.get(), 1);
        if matches!(mode, 9 | 14 | 15) {
            assert!(
                !v["observedRelation"].is_null(),
                "confirmed first observation must survive later panic"
            );
        }
    }
}
#[test]
fn uncertain_add_has_no_automatic_retry_or_fake_rollback() {
    for mode in [11, 12] {
        let (mut b, s) = setup();
        let r = req(&b, &s, "vertical");
        let p = b.relation_preflight(&s, &r).unwrap();
        s.post_mode.set(mode);
        let v = b.execute_relation(&s, p);
        assert_eq!(v["outcome"], "unknown");
        assert_eq!(v["automaticReplayAllowed"], false);
        assert_eq!(v["rollbackAttempted"], false);
        assert!(b.faulted);
        assert_eq!(s.calls_add.get(), 1);
    }
}
#[test]
fn relation_read_accepts_zero_graph_but_stale_and_exact_identity_never_retarget() {
    let (b, s) = setup();
    let v = b.inspect_relations(&s, DOC, ID, &eid(), None).unwrap();
    assert_eq!(v["relationState"]["kind"], 0);
    assert!(b.inspect_relations(&s, DOC, ID, "other", None).is_err());
    assert_eq!(s.calls_add.get(), 0);
    assert_eq!(
        b.inspect_relations(&s, DOC, ID, &eid(), Some("old"))
            .unwrap_err(),
        "target_revision_mismatch"
    );
    assert_eq!(b.inspect_relations(&s, DOC, ID, &eid(), None).unwrap(), v);
}
#[test]
fn current_fixed_aligned_or_axis_touching_input_refuses_without_solver_invocation() {
    for mode in 0..5 {
        let (b, s) = setup();
        {
            let mut state = s.state.borrow_mut();
            match mode {
                0 => state.line.fixed = true,
                1 => state.start.flags.fully_constrained = true,
                2 => state.end.position_cm[1] = state.start.position_cm[1],
                3 => state.end.position_cm[0] = state.start.position_cm[0],
                _ => state.end.position_cm[0] = 0.,
            }
        }
        let r = req(&b, &s, "horizontal");
        assert_eq!(
            b.relation_preflight(&s, &r).unwrap_err(),
            "native_relation_unfixed_sloped_line_required"
        );
        assert_eq!(s.calls_add.get(), 0);
    }
}
#[test]
fn relation_capability_scope_and_new_read_classification_are_explicit() {
    let cap = crate::execution::capabilities();
    assert_eq!(cap["contractVersion"], crate::assembly::CONTRACT);
    assert_eq!(
        cap["sketchConstraintTypes"],
        json!(["horizontal", "vertical"])
    );
    assert_eq!(cap["legacyParity"], false);
    assert_eq!(cap["transactional"], false);
    assert_eq!(
        cap["lineStateCompatibilityHeader"],
        "X-Adom-Mechanical-Contract: native-sketch-line-state-v1"
    );
    let r=fusion_mechanical_core::prepare_request(json!({"op":"native_sketch_relations","sessionId":"session","target":{"documentId":DOC},"sketchId":ID,"entityId":eid()}),"session").unwrap();
    assert!(!r.is_write());
    assert!(supported_arguments(&r).is_ok());
}

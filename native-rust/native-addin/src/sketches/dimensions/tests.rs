use super::*;
use crate::sketches::geometry::{GeometryBackend, InsertAttempt, ObservedEntity};
use crate::sketches::line_state::{Flags, LineBackend, Point, State as LineState};
use std::cell::{Cell, RefCell};
pub(crate) const DOC: &str = "d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001";
pub(crate) const ID: &str =
    "s:d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001:0000000000000001";
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
fn state(kind: u32) -> LineState {
    let p = |a, n| Point {
        position_cm: a,
        flags: flags(),
        connected_entities: n,
    };
    let mut l = LineState {
        line: flags(),
        construction: false,
        start: p([1., 2., 0.], 1),
        end: p(
            if kind == 1 {
                [4., 2., 0.]
            } else {
                [1., 5., 0.]
            },
            1,
        ),
        origin: p([0.; 3], 0),
        root_bodies: 0,
        root_features: 0,
        root_occurrences: 0,
        root_meshes: 0,
        design_components: 1,
        sketch_texts: 0,
        sketch_attributes: 0,
    };
    l.line.constraints = 1;
    l
}
pub(crate) struct Fake {
    pub add_calls: Cell<usize>,
    pub set_calls: Cell<usize>,
    pub mode: Cell<u32>,
    kind: u32,
    dimension: RefCell<Option<Dimension>>,
    line: RefCell<LineState>,
    reads: Cell<usize>,
}
impl Backend for Fake {
    type Handle = u64;
    fn root(&self) -> Result<RootState, String> {
        if self.set_calls.get() > 0 && self.mode.get() == 20 {
            panic!("post-set root")
        }
        Ok(RootState {
            active: true,
            parametric: true,
            hybrid: true,
            configured: false,
            root_revision: if self.set_calls.get() > 0 {
                "set"
            } else if self.add_calls.get() > 0 {
                "added"
            } else {
                "before"
            }
            .into(),
            sketch_count: 1,
            timeline_count: 1,
            timeline_position: 1,
        })
    }
    fn command(&self) -> Result<String, String> {
        if self.add_calls.get() > 0 && self.mode.get() == 8 {
            return Ok("OtherCommand".into());
        }
        Ok("SelectCommand".into())
    }
    fn create(&self, _: u32) -> CreateAttempt<u64> {
        panic!("dimension never creates sketch")
    }
    fn read(&self, h: &u64) -> Result<SketchState, String> {
        assert_eq!(*h, 7);
        Ok(SketchState {
            name: "Sketch1".into(),
            sdk_revision: self.root()?.root_revision,
            plane: 1,
            transform: [0.; 16],
            curves: 1,
            points: 3,
            profiles: 0,
            dimensions: u64::from(self.dimension.borrow().is_some()),
            constraints: 1,
            health: 0,
            deferred: false,
        })
    }
}
impl GeometryBackend for Fake {
    fn empty_origin_only(&self, _: &u64) -> Result<bool, String> {
        panic!("wrong preparation")
    }
    fn insert(&self, _: &u64, _: &geometry_preparation::Entity) -> InsertAttempt {
        panic!("dimension never inserts geometry")
    }
    fn entity(&self, _: &u64, lease: u64) -> Result<ObservedEntity, String> {
        assert_eq!(lease, 19);
        let l = self.line.borrow();
        Ok(ObservedEntity {
            geometry: geometry_preparation::Geometry::Line {
                start_cm: l.start.position_cm,
                end_cm: l.end.position_cm,
            },
            construction: Some(false),
            reference: false,
        })
    }
    fn entity_equalities(&self, _: &u64, _: &[u64]) -> Result<Vec<Vec<bool>>, String> {
        panic!("must reuse publication")
    }
}
impl LineBackend for Fake {
    fn line_state(&self, _: &u64, _: u64) -> Result<LineState, String> {
        Ok(self.line.borrow().clone())
    }
}
impl RelationBackend for Fake {
    fn relation_state(&self, _: &u64, _: u64) -> Result<RelationState, String> {
        Ok(RelationState {
            line: self.line.borrow().clone(),
            kind: self.kind,
        })
    }
    fn constrain(&self, _: &u64, _: u64, _: u32) -> Attempt {
        panic!("dimension must not add relation")
    }
}
impl DimensionBackend for Fake {
    fn dimension_state(&self, _: &u64, lease: u64) -> Result<State, String> {
        assert_eq!(lease, 19);
        let n = self.reads.get() + 1;
        self.reads.set(n);
        let mut s = State {
            relation: RelationState {
                line: self.line.borrow().clone(),
                kind: self.kind,
            },
            dimension: self.dimension.borrow().clone(),
            model_parameters: u64::from(self.dimension.borrow().is_some()),
            user_parameters: 0,
        };
        if self.set_calls.get() > 0 {
            match self.mode.get() {
                9 => s.dimension.as_mut().unwrap().value_cm += 1.,
                10 => s.dimension.as_mut().unwrap().parameter_value_cm += 1.,
                11 => s.relation.kind = 3,
                12 => s.relation.line.origin.flags.fixed = true,
                13 => s.relation.line.root_bodies = 1,
                14 => s.model_parameters = 2,
                15 => s.dimension.as_mut().unwrap().driving = false,
                16 => s.dimension.as_mut().unwrap().parameter_unit.clear(),
                17 => return Err("typed holder mismatch".into()),
                18 => panic!("post-effect getter"),
                19 if n % 2 == 0 => s.dimension.as_mut().unwrap().text_position_cm[0] += 1.,
                21 if n % 2 == 0 => panic!("later read"),
                _ => {}
            }
        }
        Ok(s)
    }
    fn add_dimension(&self, _: &u64, lease: u64, position: [f64; 3]) -> Attempt {
        assert_eq!(lease, 19);
        self.add_calls.set(self.add_calls.get() + 1);
        match self.mode.get() {
            1 => {
                return Attempt {
                    invoked: false,
                    acknowledged: false,
                }
            }
            2 => panic!("add panic"),
            _ => {}
        }
        self.dimension.replace(Some(Dimension {
            driving: true,
            aligned: true,
            value_cm: 3.,
            parameter_value_cm: 3.,
            text_position_cm: position,
            parameter_name: "d1".into(),
            parameter_expression: "3 cm".into(),
            parameter_unit: "cm".into(),
        }));
        self.line.borrow_mut().start.flags.dimensions = 1;
        self.line.borrow_mut().end.flags.dimensions = 1;
        Attempt {
            invoked: true,
            acknowledged: self.mode.get() != 3,
        }
    }
    fn set_dimension_value(&self, _: &u64, lease: u64, value: f64) -> Attempt {
        assert_eq!(lease, 19);
        self.set_calls.set(self.set_calls.get() + 1);
        match self.mode.get() {
            4 => {
                return Attempt {
                    invoked: false,
                    acknowledged: false,
                }
            }
            5 => panic!("set panic"),
            _ => {}
        }
        let mut d = self.dimension.borrow_mut();
        let d = d.as_mut().unwrap();
        d.value_cm = value;
        d.parameter_value_cm = value;
        d.parameter_expression = format!("{value} cm");
        let mut l = self.line.borrow_mut();
        let axis = if self.kind == 1 { 0 } else { 1 };
        l.end.position_cm[axis] = l.start.position_cm[axis] + value;
        Attempt {
            invoked: true,
            acknowledged: self.mode.get() != 6,
        }
    }
}
pub(crate) fn setup(kind: u32) -> (Book<u64>, Fake) {
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
            add_calls: Cell::new(0),
            set_calls: Cell::new(0),
            mode: Cell::new(0),
            kind,
            dimension: RefCell::new(None),
            line: RefCell::new(state(kind)),
            reads: Cell::new(0),
        },
    )
}
pub(crate) fn request(b: &Book<u64>, f: &Fake, session: &str, operation: &str) -> PreparedRequest {
    let v = b.inspect_dimensions(f, DOC, ID, &eid(ID), None).unwrap();
    fusion_mechanical_core::prepare_request(json!({"op":"sketch_dimension","sessionId":session,"operationId":operation,"target":v["target"],"sketch":{"documentId":DOC,"revision":v["target"]["revision"],"objectType":"Sketch","sketchId":ID},"type":"distance","entities":v["endpointRefs"],"expression":42,"position":[20,30]}),session).unwrap()
}
#[test]
fn h_and_v_solve_requested_mm_and_publish_ordered_exact_endpoints() {
    for kind in [1, 2] {
        let (mut b, f) = setup(kind);
        let r = request(&b, &f, "session", "once");
        let p = b.dimension_preflight(&f, &r).unwrap();
        assert_eq!(p.value_cm, 4.2);
        assert_eq!(p.position_cm, [2., 3., 0.]);
        let v = b.execute_dimension(&f, p);
        assert_eq!(v["success"], true, "{v}");
        assert_eq!(v["data"]["dimensionState"]["dimension"]["valueCm"], 4.2);
        assert_eq!(v["stages"]["dimensionAdd"]["invoked"], true);
        assert_eq!(v["stages"]["parameterValueSet"]["acknowledged"], true);
        let after = b.inspect_dimensions(&f, DOC, ID, &eid(ID), None).unwrap();
        assert_eq!(v["data"], after);
        assert_eq!(after["endpointOrder"], json!(["start", "end"]));
        assert!(after["dimensionId"].as_str().unwrap().starts_with("n:"));
        assert_eq!(
            b.dimension_preflight(&f, &r).unwrap_err(),
            "native_dimension_already_exists"
        );
        assert_eq!((f.add_calls.get(), f.set_calls.get()), (1, 1));
    }
}
#[test]
fn endpoint_publication_is_required_even_when_ids_and_revision_are_guessed() {
    let (b, f) = setup(1);
    let r = request(&b, &f, "session", "guess");
    b.entries[0].dimension_endpoints_published.set(false);
    assert_eq!(
        b.dimension_preflight(&f, &r).unwrap_err(),
        "native_dimension_endpoints_not_published"
    );
    assert_eq!(f.add_calls.get(), 0);
}
#[test]
fn wrong_order_foreign_revision_and_unsupported_semantics_refuse_before_effect() {
    let (b, f) = setup(1);
    let r = request(&b, &f, "session", "bad");
    for mode in 0..9 {
        let mut a = r.arguments().clone();
        match mode {
            0 => a["entities"].as_array_mut().unwrap().swap(0, 1),
            1 => a["entities"][0]["documentId"] = json!("other"),
            2 => {
                a["entities"][1]["revision"] = json!(format!(
                    "sketch-dimensions-projection-v1:{}",
                    "f".repeat(64)
                ))
            }
            3 => a["expression"] = json!("42 mm"),
            4 => a["expression"] = json!(0.),
            5 => a["expression"] = json!(10001.),
            6 => a["parameterName"] = json!("width"),
            7 => a["type"] = json!("horizontal"),
            _ => a["position"] = json!([0, 0, 1]),
        }
        let r = fusion_mechanical_core::prepare_request(a, "session").unwrap();
        assert!(supported_arguments(&r).is_err(), "mode {mode}");
        assert!(b.dimension_preflight(&f, &r).is_err());
    }
    assert_eq!(f.add_calls.get(), 0);
}
#[test]
fn stale_revision_is_known_before_add() {
    let (b, f) = setup(1);
    let r = request(&b, &f, "session", "stale");
    f.line.borrow_mut().start.position_cm[0] -= 1.;
    assert_eq!(
        b.dimension_preflight(&f, &r).unwrap_err(),
        "target_revision_mismatch"
    );
    assert_eq!(f.add_calls.get(), 0);
}
#[test]
fn mutable_graph_or_fixed_endpoint_is_not_admitted() {
    for mode in 0..5 {
        let (b, f) = setup(1);
        let r = request(&b, &f, "session", "dirty");
        match mode {
            0 => f.line.borrow_mut().start.flags.fixed = true,
            1 => f.line.borrow_mut().root_bodies = 1,
            2 => f.line.borrow_mut().line.constraints = 2,
            3 => f.line.borrow_mut().origin.connected_entities = 1,
            _ => f.line.borrow_mut().start.flags.has_referenced_entity = true,
        }
        assert!(b.dimension_preflight(&f, &r).is_err());
        assert_eq!(f.add_calls.get(), 0);
    }
}
#[test]
fn no_call_is_known_but_add_or_value_uncertainty_permanently_blocks_replay() {
    for mode in [1, 2, 3, 4, 5, 6, 8] {
        let (mut b, f) = setup(1);
        let r = request(&b, &f, "session", "fail");
        let p = b.dimension_preflight(&f, &r).unwrap();
        f.mode.set(mode);
        let v = b.execute_dimension(&f, p);
        assert_eq!(v["success"], false, "{mode}:{v}");
        if mode == 1 {
            assert_eq!(v["nativeExecutionStarted"], false);
            assert!(!b.faulted);
        } else {
            assert_eq!(v["outcome"], "unknown");
            assert!(b.faulted);
            assert!(b.dimension_preflight(&f, &r).is_err());
        }
        assert_eq!(f.add_calls.get(), 1);
        assert_eq!(f.set_calls.get(), usize::from(matches!(mode, 4 | 5 | 6)));
        if matches!(mode, 4 | 5 | 6 | 8) {
            assert_eq!(v["stages"]["dimensionAdd"]["acknowledged"], true);
            assert!(f.dimension.borrow().is_some());
        }
    }
}
#[test]
fn post_effect_invariants_panics_and_late_read_changes_keep_unknown_without_second_effect() {
    for mode in 9..=21 {
        let (mut b, f) = setup(1);
        let r = request(&b, &f, "session", "readback");
        let p = b.dimension_preflight(&f, &r).unwrap();
        f.mode.set(mode);
        let v = b.execute_dimension(&f, p);
        assert_eq!(v["outcome"], "unknown", "{mode}:{v}");
        assert!(b.faulted);
        assert_eq!((f.add_calls.get(), f.set_calls.get()), (1, 1));
        assert_eq!(v["stages"]["parameterValueSet"]["acknowledged"], true);
        if matches!(mode, 19 | 21) {
            assert!(v["observedDimension"].is_object());
        }
    }
}
#[test]
fn incomplete_read_retires_endpoints_instead_of_publishing() {
    let (b, f) = setup(1);
    f.line.borrow_mut().root_bodies = 1;
    assert!(b.inspect_dimensions(&f, DOC, ID, &eid(ID), None).is_err());
    assert!(!b.entries[0].dimension_endpoints_published.get());
    assert!(b.entries[0].entities_retired.get());
}
#[test]
fn dimension_contract_is_explicit_and_preserves_prior_scopes() {
    let cap = crate::execution::capabilities();
    assert_eq!(cap["contractVersion"], crate::assembly::CONTRACT);
    assert_eq!(
        cap["dimensionsCompatibilityHeader"],
        format!("X-Adom-Mechanical-Contract: {CONTRACT}")
    );
    assert_eq!(cap["operations"].as_array().unwrap().len(), 27);
    assert_eq!(cap["sketchDimensionSupport"], true);
    assert_eq!(cap["legacyParity"], false);
    assert_eq!(cap["transactional"], false);
    assert_eq!(
        cap["relationsCompatibilityHeader"],
        "X-Adom-Mechanical-Contract: native-sketch-relations-v1"
    );
    assert_eq!(
        cap["schemas"]["sketch_dimension"]["properties"]["expression"]["type"],
        "number"
    );
    assert!(cap["schemas"]["sketch_dimension"]["properties"]
        .get("parameterName")
        .is_none());
    for name in [
        "sketch_constraint",
        "native_sketch_relations",
        "native_sketch_line_state",
        "native_sketch_dimensions",
        "sketch_dimension",
        "assembly_properties",
        "mcmaster_discover",
        "bom_export",
        "feature_create",
    ] {
        assert!(crate::execution::supported(name));
    }
    let r=fusion_mechanical_core::prepare_request(json!({"op":"native_sketch_dimensions","sessionId":"session","target":{"documentId":DOC},"sketchId":ID,"entityId":eid(ID)}),"session").unwrap();
    assert!(!r.is_write());
    assert!(supported_arguments(&r).is_ok());
}

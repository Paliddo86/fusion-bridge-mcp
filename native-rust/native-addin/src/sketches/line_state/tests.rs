use super::*;
use crate::sketches::geometry::{InsertAttempt, ObservedEntity};
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
struct Fake {
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
            root_revision: "root".into(),
            sketch_count: 1,
            timeline_count: 1,
            timeline_position: 1,
        })
    }
    fn command(&self) -> Result<String, String> {
        panic!("read must not execute command")
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
            sdk_revision: "sketch".into(),
            plane: 1,
            transform: [0.; 16],
            curves: 1,
            points: 3,
            profiles: 0,
            dimensions: 0,
            constraints: 0,
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
fn setup() -> (Book<u64>, Fake) {
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
        },
    )
}
fn inspect(b: &Book<u64>, s: &Fake, rev: Option<&str>) -> Result<Value, String> {
    b.inspect_line_state(s, DOC, ID, &eid(), rev)
}
#[test]
fn exact_published_line_reads_stably_without_effects_or_registry_growth() {
    let (b, s) = setup();
    let v = inspect(&b, &s, None).unwrap();
    assert_eq!(v["constraintMutationSupported"], false);
    assert_eq!(v["isolatedInventoryComplete"], true);
    assert_eq!(v["unfixedLineAndEndpoints"], true);
    for _ in 0..8 {
        assert_eq!(
            inspect(&b, &s, v["target"]["revision"].as_str()).unwrap(),
            v
        );
    }
    assert_eq!(s.calls.get(), 18);
    assert_eq!(b.entries.len(), 1);
    assert_eq!(b.entries[0].geometry_leases.as_ref().unwrap(), &vec![19]);
}
#[test]
fn never_published_foreign_and_guessed_selectors_are_refused_before_backend() {
    for case in 0..5 {
        let (mut b, s) = setup();
        let mut id = eid();
        match case {
            0 => id = "line at [1,2]".into(),
            1 => id = id.replace("aaaa", "bbbb"),
            2 => *b.entries[0].entities.borrow_mut() = None,
            3 => b.entries[0].geometry_leases = Some(vec![19, 20]),
            _ => b.entries[0].retired.set(true),
        }
        assert!(b.inspect_line_state(&s, DOC, ID, &id, None).is_err());
        assert_eq!(s.reads.get(), 0);
        assert_eq!(s.calls.get(), 0);
    }
    let (b, s) = setup();
    assert!(b
        .inspect_line_state(&s, "foreign", ID, &eid(), None)
        .is_err());
    assert_eq!(s.reads.get(), 0);
}
#[test]
fn fixed_state_changes_scoped_revision_without_sdk_revision_change_or_retirement() {
    let (b, s) = setup();
    let a = inspect(&b, &s, None).unwrap();
    s.state.borrow_mut().start.flags.fixed = true;
    assert_eq!(
        inspect(&b, &s, a["target"]["revision"].as_str()).unwrap_err(),
        "target_revision_mismatch"
    );
    let v = inspect(&b, &s, None).unwrap();
    assert_eq!(v["unfixedLineAndEndpoints"], false);
    assert_ne!(a["target"], v["target"]);
    assert!(!b.entries[0].entities_retired.get());
}
#[test]
fn failure_change_or_panic_retires_identity_and_never_rebinds_after_restoration() {
    for mode in 1..=4 {
        let (b, s) = setup();
        s.mode.set(mode);
        let e = inspect(&b, &s, None).unwrap_err();
        assert!(!known_refusal(&e));
        assert!(b.entries[0].entities_retired.get());
        s.mode.set(0);
        let n = s.reads.get();
        assert_eq!(
            inspect(&b, &s, None).unwrap_err(),
            "native_entities_id_retired"
        );
        assert_eq!(n, s.reads.get());
    }
}
#[test]
fn complete_graph_and_downstream_exclusions_fail_closed() {
    for case in 0..15 {
        let (b, s) = setup();
        {
            let mut v = s.state.borrow_mut();
            match case {
                0 => v.root_bodies = 1,
                1 => v.root_features = 1,
                2 => v.root_occurrences = 1,
                3 => v.root_meshes = 1,
                4 => v.design_components = 2,
                5 => v.sketch_texts = 1,
                6 => v.line.constraints = 1,
                7 => v.start.flags.dimensions = 1,
                8 => v.end.connected_entities = 2,
                9 => v.origin.connected_entities = 1,
                10 => v.line.reference = true,
                11 => v.end.flags.has_referenced_entity = true,
                12 => v.construction = true,
                13 => v.start.position_cm[0] = f64::NAN,
                _ => v.end.position_cm = v.start.position_cm,
            }
        }
        assert!(inspect(&b, &s, None).is_err(), "case{case}");
    }
}
#[test]
fn known_scope_refusal_never_includes_genuine_sdk_or_capture_uncertainty() {
    for e in [
        "native_sdk_error_7",
        "native_line_sdk_read_failed_status_7_stage_404_error_3",
        "native_line_capture_panicked",
        "native_line_projection_changed",
        "native_sketch_projection_changed",
        "native_line_geometry_membership_changed",
    ] {
        assert!(!known_refusal(e));
    }
    assert!(known_refusal("target_revision_mismatch"));
}
#[test]
fn shared_schema_and_narrow_arguments_keep_constraint_mutation_unsupported() {
    let raw = json!({"op":"native_sketch_line_state","sessionId":"a".repeat(32),"target":{"documentId":DOC},"sketchId":ID,"entityId":eid()});
    let r = fusion_mechanical_core::prepare_request(raw.clone(), &"a".repeat(32)).unwrap();
    assert!(!r.is_write());
    supported_arguments(&r).unwrap();
    assert!(crate::execution::supported("sketch_constraint")); // R18 explicit relation contract only.
    let mut bad = raw;
    bad["target"]["revision"] = json!("sketch-projection-v1:foo");
    let r = fusion_mechanical_core::prepare_request(bad, &"a".repeat(32)).unwrap();
    assert_eq!(
        supported_arguments(&r),
        Err("native_line_revision_required")
    );
}

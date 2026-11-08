use super::*;
use std::cell::{Cell, RefCell};
pub(crate) const DOC: &str = "d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001";
pub(crate) fn empty() -> Projection {
    Projection {
        root: Root {
            revision: "r0".into(),
            name: "Root".into(),
            active: true,
            parametric: true,
            hybrid: true,
            configured: false,
            offline: false,
            pending_snapshot: false,
            components: 1,
            occurrences: 0,
            timeline: 0,
            timeline_position: 0,
            bodies: 0,
            sketches: 0,
            mesh_bodies: 0,
            joints: 0,
            as_built_joints: 0,
            rigid_groups: 0,
            assembly_constraints: 0,
            motion_links: 0,
            joint_origins: 0,
        },
        components: vec![],
        occurrences: vec![],
        cloud_metadata: false,
    }
}
pub(super) fn component() -> Component {
    Component {
        name: "Component1".into(),
        revision: "c0".into(),
        attributes: BTreeMap::new(),
        part_number: Some("Component1".into()),
        description: Some(String::new()),
        bodies: 0,
        sketches: 0,
        occurrences: 0,
        mesh_bodies: 0,
        joints: 0,
        as_built_joints: 0,
        rigid_groups: 0,
        assembly_constraints: 0,
        motion_links: 0,
    }
}
pub(super) fn occurrence(ci: usize) -> Occurrence {
    Occurrence {
        component: ci,
        name: format!("Component{}:1", ci + 1),
        path: format!("Component{}:1", ci + 1),
        matrix: [
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
        ],
        grounded: false,
        grounded_to_parent: false,
        referenced: false,
        configured: false,
        suppressed: false,
        rolled_back: false,
        native: true,
        children: 0,
    }
}
pub(crate) struct Fake {
    pub state: RefCell<Projection>,
    pub add_calls: Cell<usize>,
    pub change_calls: Cell<usize>,
    pub mode: Cell<u32>,
    reads: Cell<usize>,
    pub changes: RefCell<Vec<String>>,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            state: RefCell::new(empty()),
            add_calls: Cell::new(0),
            change_calls: Cell::new(0),
            mode: Cell::new(0),
            reads: Cell::new(0),
            changes: RefCell::new(vec![]),
        }
    }
}
impl Backend for Fake {
    type Handle = u64;
    fn open(&self) -> Open<u64> {
        Open {
            handle: Some(9),
            acknowledged: self.mode.get() != 20,
        }
    }
    fn command(&self) -> Result<String> {
        Ok("SelectCommand".into())
    }
    fn snapshot(&self, h: &u64, cloud: bool) -> Result<Projection> {
        assert_eq!(*h, 9);
        self.reads.set(self.reads.get() + 1);
        if self.mode.get() == 4 && self.add_calls.get() > 0 {
            panic!("post-effect read");
        }
        let mut p = self.state.borrow().clone();
        p.cloud_metadata = cloud;
        if cloud && p.root.offline {
            return Err("native_assembly_native_metadata_requires_online".into());
        }
        if !cloud {
            for c in &mut p.components {
                c.part_number = None;
                c.description = None;
            }
        }
        if self.mode.get() == 8 && !p.components.is_empty() {
            p.components[0].part_number = Some("leaked".into());
            p.components[0].description = None;
        }
        if self.mode.get() == 9 {
            p.cloud_metadata = !cloud;
        }
        if self.mode.get() == 10 {
            p.root.revision = format!("read{}", self.reads.get());
        }
        Ok(p)
    }
    fn add(&self, _: &u64, existing: Option<usize>) -> Attempt {
        self.add_calls.set(self.add_calls.get() + 1);
        if self.mode.get() == 12 {
            panic!("add boundary panic");
        }
        if self.mode.get() == 3 {
            return Attempt::default();
        }
        let mut p = self.state.borrow_mut();
        let ci = existing.unwrap_or(p.components.len());
        if existing.is_none() {
            p.components.push(component());
            p.root.components += 1;
        }
        let oi = p.occurrences.len();
        p.occurrences.push(occurrence(ci));
        p.root.occurrences += 1;
        p.root.timeline += 1;
        p.root.timeline_position = p.root.timeline;
        p.root.revision = format!("add{}", self.add_calls.get());
        if self.mode.get() == 5 {
            p.root.pending_snapshot = true;
        }
        if self.mode.get() == 7 {
            p.root.name = "Foreign edit".into();
        }
        Attempt {
            invoked: true,
            acknowledged: self.mode.get() != 1,
            component: Some(if self.mode.get() == 6 { ci + 1 } else { ci }),
            occurrence: Some(oi),
        }
    }
    fn change(&self, _: &u64, ci: usize, c: &Change) -> Attempt {
        self.change_calls.set(self.change_calls.get() + 1);
        if self.mode.get() == 13 {
            panic!("metadata boundary panic");
        }
        self.changes.borrow_mut().push(c.key.clone());
        let mut p = self.state.borrow_mut();
        let comp = &mut p.components[ci];
        match c.key.as_str() {
            "name" => comp.name = c.value.clone(),
            "partNumber" => {
                comp.part_number = Some(if c.value.is_empty() {
                    comp.name.clone()
                } else {
                    c.value.clone()
                })
            }
            "description" => comp.description = Some(c.value.clone()),
            _ => {
                comp.attributes.insert(c.key.clone(), c.value.clone());
            }
        }
        comp.revision = format!("m{}", self.change_calls.get());
        if c.key == "name" {
            for o in &mut p.occurrences {
                if o.component == ci {
                    o.name = format!("{}:1", c.value);
                    o.path = o.name.clone();
                }
            }
        }
        p.root.revision = format!("m{}", self.change_calls.get());
        Attempt {
            invoked: true,
            acknowledged: self.mode.get() != 2 || self.change_calls.get() != 2,
            component: Some(ci),
            occurrence: None,
        }
    }
}
pub(crate) fn request(book: &mut Book<u64>, b: &Fake, epoch: &str, id: &str) -> PreparedRequest {
    let value = book.inspect(b, DOC, None, false).unwrap();
    prepare(
        json!({"op":"component_create","sessionId":epoch,"operationId":id,"target":value["data"]["target"],"name":"A"}),
        epoch,
    )
}
pub(super) fn prepare(v: Value, epoch: &str) -> PreparedRequest {
    fusion_mechanical_core::prepare_request(v, epoch).unwrap()
}
fn create(book: &mut Book<u64>, b: &Fake, name: &str) -> Value {
    let r = request(book, b, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", name);
    let mut a = r.arguments().clone();
    a["name"] = json!(name);
    let p = book
        .preflight(b, &prepare(a, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))
        .unwrap();
    book.execute(b, p)
}
fn edit_request(book: &mut Book<u64>, b: &Fake, cloud: bool, changes: Value) -> PreparedRequest {
    let v = book.inspect(b, DOC, None, cloud).unwrap();
    prepare(
        json!({"op":"component_metadata","sessionId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","operationId":"metadata","target":v["data"]["target"],"component":v["data"]["components"][0]["ref"],"changes":changes}),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
}
#[test]
fn exact_two_definitions_three_instances_have_separate_ids_and_shared_component() {
    let b = Fake::default();
    let mut book = Book::default();
    assert_eq!(create(&mut book, &b, "A")["success"], true);
    assert_eq!(create(&mut book, &b, "B")["success"], true);
    let s = book.inspect(&b, DOC, None, false).unwrap();
    let r = prepare(
        json!({"op":"occurrence_insert","sessionId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","operationId":"shared","target":s["data"]["target"],"component":s["data"]["components"][0]["ref"]}),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    let p = book.preflight(&b, &r).unwrap();
    let v = book.execute(&b, p);
    assert_eq!(v["success"], true, "{v}");
    let d = &v["data"];
    assert_eq!(d["components"].as_array().unwrap().len(), 2);
    assert_eq!(d["occurrences"].as_array().unwrap().len(), 3);
    assert_eq!(
        d["occurrences"][0]["componentId"],
        d["occurrences"][2]["componentId"]
    );
    assert_ne!(
        d["occurrences"][0]["occurrenceId"],
        d["occurrences"][2]["occurrenceId"]
    );
    assert_ne!(
        d["components"][0]["componentId"],
        d["components"][1]["componentId"]
    );
    assert_eq!(d["complete"], false);
    assert!(d["components"][0]["partNumber"].is_null());
}
#[test]
fn metadata_order_escaping_and_empty_part_number_reset_readback() {
    let b = Fake::default();
    let mut book = Book::default();
    create(&mut book, &b, "A");
    b.changes.borrow_mut().clear();
    let r = edit_request(
        &mut book,
        &b,
        true,
        json!({"supplier":"quote\"\\line\n","excludeFromBom":true,"name":"Renamed","description":"desc","partNumber":""}),
    );
    let p = book.preflight(&b, &r).unwrap();
    let v = book.execute(&b, p);
    assert_eq!(v["success"], true, "{v}");
    assert_eq!(
        *b.changes.borrow(),
        vec![
            "name",
            "partNumber",
            "description",
            "excludeFromBom",
            "supplier"
        ]
    );
    assert_eq!(v["data"]["components"][0]["partNumber"], "Renamed");
    let raw = v["data"]["components"][0]["attributes"]["supplier"]
        .as_str()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(raw).unwrap(),
        json!("quote\"\\line\n")
    );
}
#[test]
fn online_refusal_does_not_retire_offline_safe_owned_view() {
    let b = Fake::default();
    let mut book = Book::default();
    create(&mut book, &b, "A");
    b.state.borrow_mut().root.offline = true;
    assert_eq!(
        book.inspect(&b, DOC, None, true).unwrap_err(),
        "native_assembly_native_metadata_requires_online"
    );
    assert!(book.inspect(&b, DOC, None, false).is_ok());
    let r = edit_request(&mut book, &b, false, json!({"supplier":"local"}));
    let p = book.preflight(&b, &r).unwrap();
    assert_eq!(book.execute(&b, p)["success"], true);
}
#[test]
fn partial_cloud_fields_and_wrong_requested_mode_are_rejected() {
    for mode in [8, 9] {
        let b = Fake::default();
        let mut book = Book::default();
        create(&mut book, &b, "A");
        b.mode.set(mode);
        assert!(book.inspect(&b, DOC, None, false).is_err());
        b.mode.set(0);
        assert_eq!(
            book.inspect(&b, DOC, None, false).unwrap_err(),
            "native_assembly_document_retired"
        );
    }
    let mut p = empty();
    p.cloud_metadata = true;
    p.components.push(component());
    p.occurrences.push(occurrence(0));
    p.root.components = 2;
    p.root.occurrences = 1;
    p.components[0].description = None;
    assert_eq!(
        validate(&p).unwrap_err(),
        "native_assembly_metadata_incomplete"
    );
}
#[test]
fn foreign_component_and_foreign_plan_never_invoke_effect() {
    let b = Fake::default();
    let mut book = Book::default();
    create(&mut book, &b, "A");
    let r = edit_request(&mut book, &b, false, json!({"name":"N"}));
    let mut args = r.arguments().clone();
    args["component"]["componentId"] = json!("c:foreign:0000000000000001");
    assert_eq!(
        book.preflight(&b, &prepare(args, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))
            .unwrap_err(),
        "native_assembly_component_not_owned"
    );
    let p = book.preflight(&b, &r).unwrap();
    let mut other = Book::default();
    other.inspect(&b, DOC, None, false).unwrap();
    let changes = b.change_calls.get();
    assert_eq!(
        other.execute(&b, p)["errorCode"],
        "native_assembly_foreign_plan"
    );
    assert_eq!(b.change_calls.get(), changes);
}
#[test]
fn stale_plan_after_preflight_refuses_before_add() {
    let b = Fake::default();
    let mut book = Book::default();
    let r = request(&mut book, &b, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "stale");
    let p = book.preflight(&b, &r).unwrap();
    b.state.borrow_mut().root.revision = "user edit".into();
    let v = book.execute(&b, p);
    assert_eq!(v["errorCode"], "target_revision_mismatch");
    assert_eq!(v["nativeExecutionStarted"], false);
    assert_eq!(b.add_calls.get(), 0);
}
#[test]
fn sdk_unknown_prefixes_are_retained_and_book_cannot_repeat() {
    for mode in [1, 2, 4, 5, 6, 7, 12, 13] {
        let b = Fake::default();
        let mut book = Book::default();
        let mut r = request(&mut book, &b, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "fault")
            .arguments()
            .clone();
        r["metadata"] = json!({"supplier":"x"});
        let r = prepare(r, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let p = book.preflight(&b, &r).unwrap();
        b.mode.set(mode);
        let v = book.execute(&b, p);
        assert_eq!(v["outcome"], "unknown", "mode {mode}: {v}");
        assert_eq!(v["nativeExecutionStarted"], true);
        assert_eq!(b.add_calls.get(), 1);
        assert!(!v["stages"].as_array().unwrap().is_empty());
        b.mode.set(0);
        assert_eq!(
            book.preflight(&b, &r).unwrap_err(),
            "native_assembly_book_unavailable"
        );
        assert_eq!(b.add_calls.get(), 1);
    }
}
#[test]
fn noninvoked_add_is_known_refusal_and_does_not_poison_book() {
    let b = Fake::default();
    let mut book = Book::default();
    let r = request(&mut book, &b, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "refused");
    let p = book.preflight(&b, &r).unwrap();
    b.mode.set(3);
    let v = book.execute(&b, p);
    assert_eq!(v["nativeExecutionStarted"], false);
    assert!(v.get("outcome").is_none());
    b.mode.set(0);
    assert!(book.inspect(&b, DOC, None, false).is_ok());
}
#[test]
fn effect_observation_capacity_refusal_is_before_add() {
    let b = Fake::default();
    let mut book = Book::default();
    let r = request(
        &mut book,
        &b,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "capacity",
    );
    let mut p = book.preflight(&b, &r).unwrap();
    p.action = Action::Create {
        changes: vec![
            Change {
                key: "name".into(),
                value: "x".into(),
                native: true
            };
            10
        ],
    };
    let v = book.execute(&b, p);
    assert_eq!(v["errorCode"], "native_assembly_observation_capacity");
    assert_eq!(b.add_calls.get(), 0);
    assert_eq!(b.change_calls.get(), 0);
}
#[test]
fn changing_projection_retires_identity_without_silent_read_recovery() {
    let b = Fake::default();
    let mut book = Book::default();
    book.inspect(&b, DOC, None, false).unwrap();
    b.mode.set(10);
    assert_eq!(
        book.inspect(&b, DOC, None, false).unwrap_err(),
        "native_assembly_projection_changed"
    );
    b.mode.set(0);
    assert_eq!(
        book.inspect(&b, DOC, None, false).unwrap_err(),
        "native_assembly_document_retired"
    );
}
#[test]
fn no_implicit_snapshot_capture_or_foreign_graph_is_accepted() {
    for field in 0..5 {
        let mut p = empty();
        match field {
            0 => p.root.pending_snapshot = true,
            1 => p.root.assembly_constraints = 1,
            2 => p.root.motion_links = 1,
            3 => p.root.joint_origins = 1,
            _ => p.root.configured = true,
        }
        assert!(validate(&p).is_err());
    }
}
#[test]
fn slot_bounds_no_eviction_and_release_failure_no_retry() {
    let b = Fake::default();
    let mut book = Book::default();
    for i in 0..MAX_DOCUMENTS {
        book.inspect(&b, &format!("doc{i}"), None, false).unwrap();
    }
    assert_eq!(
        book.inspect(&b, "extra", None, false).unwrap_err(),
        "native_assembly_document_capacity"
    );
    let calls = Cell::new(0);
    assert!(book
        .close(|_| {
            calls.set(calls.get() + 1);
            Err("release lost".into())
        })
        .is_err());
    assert!(book
        .close(|_| {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .is_err());
    assert_eq!(calls.get(), 1);
    assert!(book.ensure_released().is_err());
}
#[test]
fn partial_open_retains_handle_until_one_explicit_release() {
    let b = Fake::default();
    b.mode.set(20);
    let mut book = Book::default();
    assert_eq!(
        book.inspect(&b, DOC, None, false).unwrap_err(),
        "native_assembly_open_unconfirmed"
    );
    assert!(book.ensure_released().is_err());
    let released = Cell::new(0);
    book.close(|h| {
        assert_eq!(h, 9);
        released.set(released.get() + 1);
        Ok(())
    })
    .unwrap();
    assert_eq!(released.get(), 1);
}
#[test]
fn unsupported_transform_parent_native_create_metadata_and_implicit_name_dependency_refuse() {
    let b = Fake::default();
    let mut book = Book::default();
    let request = request(&mut book, &b, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "bad");
    for extra in [
        json!({"transform":{"position":[1,0,0]}}),
        json!({"transform":{"axis":[0,0,1],"angle":90}}),
        json!({"parent":{"documentId":DOC,"revision":"r","objectType":"Component"}}),
        json!({"metadata":{"partNumber":"cloud"}}),
    ] {
        let mut a = request.arguments().clone();
        a.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(supported_arguments(&prepare(a, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")).is_err());
    }
    create(&mut book, &b, "A");
    let r = edit_request(&mut book, &b, true, json!({"name":"N","description":"D"}));
    assert_eq!(
        supported_arguments(&r),
        Err("native_assembly_rename_cloud_update_requires_explicit_part_number")
    );
    assert_eq!(b.add_calls.get(), 1);
}
#[test]
fn definition_and_occurrence_capacity_checked_before_any_effect() {
    for definitions in [true, false] {
        let b = Fake::default();
        let mut book = Book::default();
        book.inspect(&b, DOC, None, false).unwrap();
        {
            let mut p = b.state.borrow_mut();
            let n = if definitions { MAX_COMPONENTS } else { 1 };
            p.components = (0..n).map(|_| component()).collect();
            let nocc = if definitions {
                MAX_COMPONENTS
            } else {
                MAX_OCCURRENCES
            };
            p.occurrences = (0..nocc).map(|i| occurrence(i % n)).collect();
            p.root.components = n as u64 + 1;
            p.root.occurrences = nocc as u64;
        }
        let r = request(&mut book, &b, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "full");
        assert_eq!(
            book.preflight(&b, &r).unwrap_err(),
            "native_assembly_capacity"
        );
        assert_eq!(b.add_calls.get(), 0);
    }
}
#[test]
fn complete_online_snapshot_has_all_fields_or_refuses() {
    let mut p = empty();
    p.components.push(component());
    p.occurrences.push(occurrence(0));
    p.root.components = 2;
    p.root.occurrences = 1;
    p.cloud_metadata = true;
    assert!(validate(&p).is_ok());
    p.root.offline = true;
    assert_eq!(
        validate(&p).unwrap_err(),
        "native_assembly_native_metadata_requires_online"
    );
}
#[test]
fn encoded_attribute_bound_is_checked_before_any_add() {
    let b = Fake::default();
    let mut book = Book::default();
    let r = request(&mut book, &b, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "escaped");
    let mut a = r.arguments().clone();
    a["metadata"] = json!({"supplier":"\u{0001}".repeat(1000)});
    let r = prepare(a, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert_eq!(
        book.preflight(&b, &r).unwrap_err(),
        "native_assembly_metadata_encoded_bound"
    );
    assert_eq!(b.add_calls.get(), 0);
}
#[test]
fn wrong_metadata_readback_or_unrelated_instance_change_is_not_success() {
    let b = Fake::default();
    let mut book = Book::default();
    create(&mut book, &b, "A");
    let r = edit_request(&mut book, &b, false, json!({"name":"N"}));
    let p = book.preflight(&b, &r).unwrap();
    let mut after = p.before.clone();
    after.components[0].name = "wrong".into();
    assert_eq!(
        verify_delta(&p, &after, 0).unwrap_err(),
        "native_assembly_metadata_readback_mismatch"
    );
    after.components[0].name = "N".into();
    after.occurrences[0].matrix[3] = 1.;
    assert_eq!(
        verify_delta(&p, &after, 0).unwrap_err(),
        "native_assembly_unrelated_occurrence_changed"
    );
}
#[test]
fn metadata_projection_modes_have_distinct_revision_and_stale_ref_refuses() {
    let b = Fake::default();
    let mut book = Book::default();
    create(&mut book, &b, "A");
    let local = book.inspect(&b, DOC, None, false).unwrap();
    let online = book.inspect(&b, DOC, None, true).unwrap();
    assert_ne!(
        local["data"]["target"]["revision"],
        online["data"]["target"]["revision"]
    );
    let r = prepare(
        json!({"op":"component_metadata","sessionId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","operationId":"wrong-mode","target":local["data"]["target"],"component":local["data"]["components"][0]["ref"],"changes":{"partNumber":"PN"}}),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    assert_eq!(
        book.preflight(&b, &r).unwrap_err(),
        "target_revision_mismatch"
    );
    assert_eq!(b.change_calls.get(), 1);
}
#[test]
fn maximum_published_reference_overhead_fits_native_retained_record_budget() {
    let mut p = empty();
    p.components = (0..MAX_COMPONENTS).map(|_| component()).collect();
    for c in &mut p.components {
        c.part_number = None;
        c.description = None;
    }
    p.occurrences = (0..MAX_OCCURRENCES)
        .map(|i| occurrence(i % MAX_COMPONENTS))
        .collect();
    p.root.components = MAX_COMPONENTS as u64 + 1;
    p.root.occurrences = MAX_OCCURRENCES as u64;
    for i in 0..120 {
        p.components[0]
            .attributes
            .insert(format!("padding{i}"), "x".repeat(1024));
        if serde_json::to_vec(&p).unwrap().len() > MAX_BYTES {
            p.components[0].attributes.remove(&format!("padding{i}"));
            break;
        }
    }
    validate(&p).unwrap();
    let doc = "d".repeat(128);
    let data = output(&doc, &p).unwrap();
    let stages = prepare_stages(&Action::Metadata {
        component_id: String::new(),
        changes: (0..8)
            .map(|i| Change {
                key: format!("field{i}"),
                value: "v".repeat(4096),
                native: false,
            })
            .collect(),
    })
    .unwrap();
    let result = json!({"success":true,"data":data,"componentId":component_id(&doc,0),"stages":stages,"nativeExecutionStarted":true,"automaticReplayAllowed":false,"rollbackAttempted":false,"legacyParity":false});
    assert!(serde_json::to_vec(&result).unwrap().len() < 240 * 1024);
    // Successful settling evidence reserves at most 8KiB COMBINED for the
    // changed root and all component revisions, plus bounded capture hashes.
    let mut with_evidence = result;
    with_evidence["readbackEvidence"] = json!({"combinedRootAndComponentRevisionDiagnostics":"x".repeat(readback::MAX_SETTLING_DIAGNOSTIC_BYTES),"captureHashes":"h".repeat(2048)});
    assert!(serde_json::to_vec(&with_evidence).unwrap().len() < 256 * 1024);
}
#[test]
fn assembly_capability_inventory_preserves_prior_operations_and_narrows_native_arguments() {
    let c = crate::execution::capabilities();
    assert_eq!(c["contractVersion"], CONTRACT);
    assert_eq!(c["operations"].as_array().unwrap().len(), 27);
    assert_eq!(c["rootAssemblySupport"], true);
    assert_eq!(c["assemblyTransformsSupported"], false);
    assert_eq!(c["legacyParity"], false);
    assert!(c["schemas"]["component_create"]["properties"]
        .get("parent")
        .is_none());
    assert!(
        c["schemas"]["component_create"]["properties"]["metadata"]["properties"]
            .get("partNumber")
            .is_none()
    );
    assert_eq!(
        c["schemas"]["occurrence_insert"]["properties"]["independentCopy"]["enum"],
        json!([false])
    );
    for op in [
        "component_create",
        "occurrence_insert",
        "component_metadata",
        "native_assembly_inspect",
        "sketch_dimension",
        "sketch_constraint",
        "bom",
        "parameter_create",
    ] {
        assert!(c["operations"].as_array().unwrap().contains(&json!(op)));
    }
}

#[test]
fn capture_change_diagnostic_keeps_revision_only_difference_and_equal_is_empty() {
    let before = empty().root;
    assert!(root_capture_diagnostic(&before, &before).is_none());
    let mut after = before.clone();
    after.revision = "observed-revision".into();
    let d = root_capture_diagnostic(&before, &after).unwrap();
    assert_eq!(d["changedFields"], json!(["revision"]));
    assert_eq!(d["expected"], json!(before));
    assert_eq!(d["observed"], json!(after));
    assert_eq!(d["additionalSdkReads"], 0);
    assert_eq!(d["retryAttempted"], false);
}
#[test]
fn capture_change_diagnostic_keeps_nonrevision_fields_without_reclassifying_outcome() {
    let before = empty().root;
    let mut after = before.clone();
    after.pending_snapshot = true;
    after.timeline = 1;
    after.timeline_position = 1;
    after.offline = true;
    let d = root_capture_diagnostic(&before, &after).unwrap();
    let changed: std::collections::BTreeSet<_> = d["changedFields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        changed,
        std::collections::BTreeSet::from([
            "pendingSnapshot",
            "timeline",
            "timelinePosition",
            "offline"
        ])
    );
    assert_eq!(d["expected"]["revision"], d["observed"]["revision"]);
    assert_eq!(
        d["errorCode"],
        "native_assembly_root_changed_during_capture"
    );
    assert_eq!(d["comparison"], "exact_equality");
    assert!(d.get("outcome").is_none());
    assert!(d.get("nativeExecutionStarted").is_none());
}

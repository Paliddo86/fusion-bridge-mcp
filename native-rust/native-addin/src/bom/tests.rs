use super::*;
use std::cell::{Cell, RefCell};

#[derive(Clone, Copy)]
pub(crate) struct Handle {
    key: usize,
    generation: usize,
}
pub(crate) struct Fake {
    pass: Cell<usize>,
    offline_after: Cell<Option<usize>>,
    connectivity_error: Cell<bool>,
    replace: Cell<bool>,
    change: Cell<bool>,
    fault: Cell<bool>,
    design: RefCell<Design>,
    definitions: RefCell<Vec<Definition>>,
    instances: RefCell<Vec<Instance>>,
    purchased: Cell<bool>,
    count_override: Cell<Option<usize>>,
    body_override: Cell<Option<usize>>,
    cycle: Cell<bool>,
}
fn properties(mass: f64) -> core::PhysicalProperties {
    core::PhysicalProperties {
        mass_kg: mass,
        volume_mm3: mass * 1000.0,
    }
}
impl Fake {
    pub(crate) fn new() -> Self {
        Self {
            pass: Cell::new(0),
            offline_after: Cell::new(None),
            connectivity_error: Cell::new(false),
            replace: Cell::new(false),
            change: Cell::new(false),
            fault: Cell::new(false),
            purchased: Cell::new(false),
            count_override: Cell::new(None),
            body_override: Cell::new(None),
            cycle: Cell::new(false),
            design: RefCell::new(Design {
                active: true,
                hybrid: true,
                configured: false,
                configuration: false,
                table_present: false,
                row_id: String::new(),
                components: 3,
                timeline_count: 0,
                timeline_position: 0,
            }),
            definitions: RefCell::new(
                (0..3)
                    .map(|i| Definition {
                        name: if i == 0 { "Root" } else { "same name" }.into(),
                        part_number: "same-part".into(),
                        description: "description".into(),
                        revision: format!("revision-{i}"),
                        token: "same-token-does-not-identify".into(),
                        attributes: BTreeMap::new(),
                        material: Some(core::Material {
                            id: "steel-id".into(),
                            name: "Steel".into(),
                        }),
                        mesh_bodies: 0,
                    })
                    .collect(),
            ),
            instances: RefCell::new(
                ["A:1", "A:1+B:1", "A:2", "A:2+B:1"]
                    .iter()
                    .map(|p| Instance {
                        path: (*p).into(),
                        token: "same-occ-token".into(),
                        referenced: false,
                        configured: false,
                        timeline_present: true,
                        suppressed: false,
                        rolled_back: false,
                        visible: true,
                    })
                    .collect(),
            ),
        }
    }
    fn h(&self, key: usize) -> Handle {
        Handle {
            key,
            generation: self.pass.get(),
        }
    }
}
impl Backend for Fake {
    type Handle = Handle;
    fn is_offline(&self) -> Result<bool> {
        if self.connectivity_error.get() {
            return Err("injected_connectivity_unavailable".into());
        }
        Ok(self
            .offline_after
            .get()
            .is_some_and(|after| self.pass.get() >= after))
    }
    fn design(&self) -> Result<Design> {
        Ok(self.design.borrow().clone())
    }
    fn root(&self) -> Result<Handle> {
        self.pass.set(self.pass.get() + 1);
        Ok(self.h(0))
    }
    fn same(&self, a: Handle, b: Handle) -> Result<bool> {
        Ok(a.key == b.key && (!self.replace.get() || a.generation == b.generation))
    }
    fn definition(&self, h: Handle) -> Result<Definition> {
        let mut value = self.definitions.borrow()[h.key].clone();
        if self.change.get() && self.pass.get() > 1 {
            value.description = "changed between complete passes".into();
        }
        if self.purchased.get() && h.key == 1 {
            value.attributes.insert("asPurchased".into(), "true".into());
        }
        Ok(value)
    }
    fn component(&self, h: Handle) -> Result<Handle> {
        Ok(self.h(if self.cycle.get() {
            0
        } else if matches!(h.key, 10 | 12) {
            1
        } else {
            2
        }))
    }
    fn instance(&self, h: Handle) -> Result<Instance> {
        Ok(self.instances.borrow()[h.key - 10].clone())
    }
    fn child_count(&self, h: Handle) -> Result<usize> {
        Ok(self.count_override.get().unwrap_or(match h.key {
            0 => 2,
            10 | 12 => 1,
            _ => 0,
        }))
    }
    fn child(&self, h: Handle, index: usize) -> Result<Handle> {
        Ok(self.h(match (h.key, index) {
            (0, 0) => 10,
            (0, 1) => 12,
            (10, 0) => 11,
            (12, 0) => 13,
            _ => return Err("bad fake child".into()),
        }))
    }
    fn body_count(&self, _h: Handle) -> Result<usize> {
        Ok(self.body_override.get().unwrap_or(1))
    }
    fn body(&self, h: Handle, _index: usize) -> Result<core::PhysicalProperties> {
        if self.fault.get() {
            return Err("injected physical read failure".into());
        }
        Ok(properties(match h.key {
            0 => 1.0,
            1 | 10 => 2.0,
            2 | 11 => 3.0,
            12 => 4.0,
            13 => 5.0,
            _ => unreachable!(),
        }))
    }
    fn subtree_properties(&self, _h: Handle) -> Result<core::PhysicalProperties> {
        Ok(properties(5.0))
    }
}
fn run(fake: &Fake) -> Result<Value> {
    generate(fake, "session", "document", None, false)
}
#[test]
fn exact_definition_grouping_and_context_physical_totals_have_independent_oracles() {
    let fake = Fake::new();
    let value = run(&fake).unwrap();
    assert_eq!(value["complete"], true);
    assert_eq!(value["partCount"], 3);
    assert_eq!(value["totalPurchasingInstances"], 5);
    assert_eq!(value["physical"]["mass"], 15.0);
    assert_eq!(value["physical"]["volume"], 15000.0);
    assert_eq!(value["physical"]["bodyInstances"], 5);
    assert_eq!(value["parts"][1]["quantity"], 2);
    assert_eq!(value["parts"][1]["unitMass"], 2.0);
    assert_eq!(value["parts"][2]["quantity"], 2);
    assert_ne!(
        value["parts"][1]["componentId"],
        value["parts"][2]["componentId"]
    );
    assert_eq!(
        value["instances"][3]["componentId"],
        value["instances"][1]["componentId"]
    );
}
#[test]
fn purchased_collapse_and_bom_exclusion_never_remove_physical_mass() {
    let fake = Fake::new();
    fake.purchased.set(true);
    let value = run(&fake).unwrap();
    assert_eq!(value["partCount"], 2);
    assert_eq!(value["parts"][1]["scope"], "component_subtree");
    assert_eq!(value["parts"][1]["unitMass"], 5.0);
    assert_eq!(value["physical"]["mass"], 15.0);
    let exploded = generate(&fake, "session", "document", None, true).unwrap();
    assert_eq!(exploded["partCount"], 3);
    assert_eq!(exploded["physical"]["mass"], 15.0);
    fake.definitions.borrow_mut()[1]
        .attributes
        .insert("excludeFromBom".into(), "true".into());
    let excluded = run(&fake).unwrap();
    assert_eq!(excluded["partCount"], 1);
    assert_eq!(excluded["physical"]["mass"], 15.0);
}
#[test]
fn visibility_does_not_change_quantities_and_snapshot_ids_never_imply_persistent_identity() {
    let fake = Fake::new();
    fake.instances.borrow_mut()[0].visible = false;
    let first = run(&fake).unwrap();
    let second = run(&fake).unwrap();
    assert_eq!(first["totalPurchasingInstances"], 5);
    assert_eq!(first["target"]["revision"], second["target"]["revision"]);
    assert_ne!(
        first["parts"][0]["componentId"],
        second["parts"][0]["componentId"]
    );
    assert!(generate(
        &fake,
        "session",
        "document",
        Some(first["target"]["revision"].as_str().unwrap()),
        false
    )
    .is_ok());
    assert_eq!(
        generate(
            &fake,
            "session",
            "document",
            Some("parameter-projection-v1:old"),
            false
        )
        .unwrap_err(),
        "target_revision_mismatch"
    );
}
#[test]
fn configured_external_suppressed_or_rolled_back_scope_refuses_without_partial_bom() {
    for variant in 0..10 {
        let fake = Fake::new();
        match variant {
            0 => fake.design.borrow_mut().configured = true,
            1 => fake.design.borrow_mut().configuration = true,
            2 => fake.design.borrow_mut().table_present = true,
            3 => fake.design.borrow_mut().row_id = "row".into(),
            4 => fake.instances.borrow_mut()[0].referenced = true,
            5 => fake.instances.borrow_mut()[0].configured = true,
            6 => fake.instances.borrow_mut()[0].suppressed = true,
            7 => fake.instances.borrow_mut()[0].rolled_back = true,
            8 => fake.design.borrow_mut().timeline_count = 1,
            _ => fake.definitions.borrow_mut()[1].mesh_bodies = 1,
        }
        assert!(run(&fake).is_err(), "variant {variant}");
    }
}
#[test]
fn equal_metadata_replacement_and_changed_metadata_fail_the_second_pass() {
    let replacement = Fake::new();
    replacement.replace.set(true);
    assert_eq!(
        run(&replacement).unwrap_err(),
        "native_bom_replaced_identity"
    );
    let changed = Fake::new();
    changed.change.set(true);
    assert_eq!(run(&changed).unwrap_err(), "native_bom_changed_snapshot");
}
#[test]
fn bounds_cycles_metadata_and_physical_failures_are_explicit() {
    let fake = Fake::new();
    fake.count_override.set(Some(MAX_OCCURRENCES + 1));
    assert_eq!(run(&fake).unwrap_err(), "native_bom_occurrence_bound");
    let fake = Fake::new();
    fake.body_override.set(Some(MAX_BODIES + 1));
    assert_eq!(run(&fake).unwrap_err(), "native_bom_body_bound");
    let fake = Fake::new();
    fake.cycle.set(true);
    assert_eq!(run(&fake).unwrap_err(), "native_bom_cycle");
    let fake = Fake::new();
    fake.definitions.borrow_mut()[0]
        .attributes
        .insert("asPurchased".into(), "\"not boolean\"".into());
    assert_eq!(run(&fake).unwrap_err(), "native_bom_invalid_metadata");
    let fake = Fake::new();
    fake.fault.set(true);
    assert_eq!(run(&fake).unwrap_err(), "injected physical read failure");
    assert!(physical(properties(f64::NAN)).is_err());
    assert!(physical(properties(-1.0)).is_err());
}
#[test]
fn empty_observed_collections_produce_zero_without_inventing_mass() {
    let fake = Fake::new();
    fake.design.borrow_mut().components = 1;
    fake.count_override.set(Some(0));
    fake.body_override.set(Some(0));
    let value = run(&fake).unwrap();
    assert_eq!(value["complete"], true);
    assert_eq!(value["partCount"], 0);
    assert_eq!(value["physical"]["mass"], 0.0);
    assert_eq!(value["physical"]["bodyInstances"], 0);
    assert_eq!(value["instances"].as_array().unwrap().len(), 1);
    fake.design.borrow_mut().components = 2;
    assert_eq!(run(&fake).unwrap_err(), "native_bom_unreachable_definition");
}
#[test]
fn public_export_plan_refuses_a_read_operation_without_panicking() {
    let fake = Fake::new();
    let captured = capture(&fake, "session", "document", None, false).unwrap();
    let request = fusion_mechanical_core::prepare_request(
        json!({"op":"bom","target":{"documentId":"document"}}),
        "session",
    )
    .unwrap();
    assert_eq!(
        crate::bom_export::Plan::prepare(&request, captured).unwrap_err(),
        "native_bom_export_operation_required"
    );
}

#[test]
fn sdk_failure_retains_call_and_design_getter_scope() {
    assert!(sdk_status("definition", 0).is_ok());
    assert_eq!(
        sdk_status("definition", 7).unwrap_err(),
        "native_bom_sdk_definition_status_7"
    );
    assert_eq!(
        sdk_status("design_configuration_top_table", 7).unwrap_err(),
        "native_bom_sdk_design_configuration_top_table_status_7"
    );
    assert_eq!(
        sdk_status("body", 4).unwrap_err(),
        "native_bom_sdk_body_status_4"
    );
}

#[test]
fn sdk_diagnostic_preserves_failure_when_detail_missing_and_bounds_sdk_text() {
    assert_eq!(
        sdk_diagnostic("definition", 7, None, None),
        "native_bom_sdk_definition_unavailable_status_7"
    );
    assert_eq!(
        sdk_diagnostic("definition", 4, Some("part_number"), None),
        "native_bom_sdk_definition_part_number_status_4"
    );
    let value = sdk_diagnostic(
        "definition",
        7,
        Some("part_number"),
        Some((-42, 3, Some("a\n\"b"))),
    );
    assert_eq!(value, "native_bom_sdk_definition_part_number_status_7; sdkError=-42; definitionHandle=3; description=\"a\\n\\\"b\"");
    let long = "é".repeat(2048);
    let bounded = sdk_diagnostic("definition", 7, Some("name"), Some((17, 0, Some(&long))));
    let description: String =
        serde_json::from_str(bounded.split("; description=").nth(1).unwrap()).unwrap();
    assert_eq!(description.chars().count(), 1024);
}

#[test]
fn optional_diagnostic_text_cannot_erase_stage_or_error_identity() {
    let mut fields = DiagnosticFields::default();
    fields.store(0, b"part_number").unwrap();
    let bytes = format!("{}é", "x".repeat(1023)).into_bytes();
    fields.store(1, &bytes[..1024]).unwrap();
    assert_eq!(fields.stage.as_deref(), Some("part_number"));
    assert!(fields.description.as_ref().unwrap().ends_with('�'));
    let message = sdk_diagnostic(
        "definition",
        7,
        fields.stage.as_deref(),
        Some((17, 3, fields.description.as_deref())),
    );
    assert!(message.contains("definition_part_number_status_7; sdkError=17; definitionHandle=3;"));
    let mut missing = DiagnosticFields::default();
    missing.store(0, b"part_number").unwrap();
    assert!(missing.store(1, &vec![b'x'; 1025]).is_err());
    assert_eq!(missing.stage.as_deref(), Some("part_number"));
    assert!(missing.description.is_none());
    missing.store(1, &[255]).unwrap();
    assert_eq!(missing.description.as_deref(), Some("�"));
    assert!(missing.store(0, b"changed").is_err());
    assert_eq!(missing.stage.as_deref(), Some("part_number"));
}

#[test]
fn offline_and_unknown_connectivity_cannot_produce_a_complete_bom() {
    let fake = Fake::new();
    fake.offline_after.set(Some(0));
    assert_eq!(
        run(&fake).unwrap_err(),
        "native_bom_online_required_for_component_metadata"
    );
    assert_eq!(
        fake.pass.get(),
        0,
        "offline refusal precedes root/definition/physical traversal"
    );
    fake.offline_after.set(None);
    fake.connectivity_error.set(true);
    assert_eq!(run(&fake).unwrap_err(), "injected_connectivity_unavailable");
    assert_eq!(fake.pass.get(), 0);
    fake.connectivity_error.set(false);
    fake.offline_after.set(Some(1));
    assert_eq!(
        run(&fake).unwrap_err(),
        "native_bom_online_required_for_component_metadata"
    );
    assert_eq!(
        fake.pass.get(),
        1,
        "online-to-offline transition invalidates the capture before a second traversal"
    );
    assert_eq!(crate::execution::capabilities()["bomRequiresOnline"], true);
}

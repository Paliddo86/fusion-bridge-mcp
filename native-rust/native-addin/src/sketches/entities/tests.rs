use super::super::geometry::tests::{book, request, Fake};
use super::*;
use std::cell::Cell;
const DOC: &str = "d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001";
const ID: &str = "s:d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001:0000000000000001";
#[derive(Default)]
struct Sdk {
    base: Fake,
    mode: Cell<u8>,
    entity_reads: Cell<usize>,
    equality_reads: Cell<usize>,
}
impl Backend for Sdk {
    type Handle = u64;
    fn root(&self) -> Result<RootState, String> {
        self.base.root()
    }
    fn command(&self) -> Result<String, String> {
        self.base.command()
    }
    fn create(&self, _: u32) -> CreateAttempt<u64> {
        panic!("read created a sketch")
    }
    fn read(&self, h: &u64) -> Result<SketchState, String> {
        self.base.read(h)
    }
}
impl GeometryBackend for Sdk {
    fn empty_origin_only(&self, _: &u64) -> Result<bool, String> {
        panic!("read preflighted mutation")
    }
    fn insert(&self, _: &u64, _: &geometry_preparation::Entity) -> geometry::InsertAttempt {
        panic!("read inserted geometry")
    }
    fn entity(&self, h: &u64, l: u64) -> Result<ObservedEntity, String> {
        let n = self.entity_reads.get() + 1;
        self.entity_reads.set(n);
        if self.mode.get() == 8 {
            panic!("exact entity getter panicked");
        }
        if self.mode.get() == 2 {
            return Err("exact member missing".into());
        }
        let mut v = self.base.entity(h, l)?;
        if self.mode.get() == 3 && n > 2 {
            if let geometry_preparation::Geometry::Point { position_cm } = &mut v.geometry {
                position_cm[0] += 1.;
            }
        }
        if self.mode.get() == 4 {
            v.construction = Some(false);
        }
        if self.mode.get() == 5 {
            if let geometry_preparation::Geometry::Point { position_cm } = &mut v.geometry {
                position_cm[0] = f64::NAN;
            }
        }
        Ok(v)
    }
    fn entity_equalities(&self, _: &u64, leases: &[u64]) -> Result<Vec<Vec<bool>>, String> {
        self.equality_reads.set(self.equality_reads.get() + 1);
        Ok(leases
            .iter()
            .map(|a| {
                leases
                    .iter()
                    .map(|b| match self.mode.get() {
                        1 => true,
                        6 => a <= b,
                        7 => false,
                        _ => a == b,
                    })
                    .collect()
            })
            .collect())
    }
}
fn setup(equal_coordinates: bool) -> (Book<u64>, Sdk) {
    let mut b = book();
    let sdk = Sdk::default();
    let second = if equal_coordinates {
        json!([10, 20])
    } else {
        json!([30, 40])
    };
    let r = request(
        &b,
        &sdk.base,
        json!([{"type":"point","position":[10,20]},{"type":"point","position":second}]),
        "entities-source",
    );
    let p = b.geometry_preflight(&sdk.base, &r).unwrap();
    assert_eq!(b.execute_geometry(&sdk.base, p)["success"], true);
    assert!(b.entries[0].entities.borrow().is_none());
    (b, sdk)
}
fn inspect(b: &Book<u64>, sdk: &Sdk) -> Result<Value, String> {
    b.inspect_entities(sdk, DOC, ID, None)
}
#[test]
fn equal_coordinates_remain_distinct_ids_and_repeated_reads_have_no_effect_or_holder_growth() {
    let (b, sdk) = setup(true);
    let first = inspect(&b, &sdk).unwrap();
    assert_eq!(first["uniqueEntityCount"], 2);
    assert_ne!(
        first["entities"][0]["entityId"],
        first["entities"][1]["entityId"]
    );
    assert_eq!(
        first["entities"][0]["observed"],
        first["entities"][1]["observed"]
    );
    for _ in 0..20 {
        assert_eq!(inspect(&b, &sdk).unwrap(), first);
    }
    assert_eq!(sdk.base.calls.get(), 2);
    assert_eq!(sdk.base.values.borrow().len(), 2);
    assert_eq!(b.entries[0].geometry_leases.as_ref().unwrap().len(), 2);
    assert_eq!(
        b.entries[0]
            .entities
            .borrow()
            .as_ref()
            .unwrap()
            .aliases
            .len(),
        2
    );
    assert_eq!(first["complete"], false);
    assert_eq!(first["constraintsEnumerated"], false);
    assert_eq!(first["entities"][0]["ref"]["documentId"], DOC);
    assert_eq!(
        first["entities"][0]["ref"]["revision"],
        first["target"]["revision"]
    );
}
#[test]
fn actual_sdk_aliases_deduplicate_and_later_merges_retire_without_rebinding() {
    let (b, sdk) = setup(true);
    sdk.mode.set(1);
    let one = inspect(&b, &sdk).unwrap();
    assert_eq!(one["uniqueEntityCount"], 1);
    assert_eq!(one["returnedLeaseCount"], 2);
    sdk.mode.set(0);
    assert_eq!(
        inspect(&b, &sdk).unwrap_err(),
        "native_entities_identity_changed"
    );
    let reads = sdk.entity_reads.get();
    sdk.mode.set(1);
    assert_eq!(inspect(&b, &sdk).unwrap_err(), "native_entities_id_retired");
    assert_eq!(reads, sdk.entity_reads.get());
}
#[test]
fn alias_with_different_observations_is_rejected() {
    let (b, sdk) = setup(false);
    sdk.mode.set(1);
    assert_eq!(
        inspect(&b, &sdk).unwrap_err(),
        "native_entities_alias_observation_mismatch"
    );
}
#[test]
fn complete_second_capture_catches_coordinate_change_without_sdk_revision_change() {
    let (b, sdk) = setup(false);
    sdk.mode.set(3);
    assert_eq!(
        inspect(&b, &sdk).unwrap_err(),
        "native_entities_projection_changed"
    );
    assert!(b.entries[0].entities.borrow().is_none());
}
#[test]
fn current_geometry_changes_revision_but_not_identity_and_stale_guard_is_nonretiring() {
    let (b, sdk) = setup(false);
    let old = inspect(&b, &sdk).unwrap();
    if let geometry_preparation::Geometry::Point { position_cm } =
        &mut sdk.base.values.borrow_mut()[0].geometry
    {
        position_cm[0] += 1.;
    }
    assert_eq!(
        b.inspect_entities(&sdk, DOC, ID, old["target"]["revision"].as_str())
            .unwrap_err(),
        "target_revision_mismatch"
    );
    let new = inspect(&b, &sdk).unwrap();
    assert_ne!(old["target"], new["target"]);
    assert_eq!(
        old["entities"][0]["entityId"],
        new["entities"][0]["entityId"]
    );
    assert!(!b.entries[0].entities_retired.get());
}
#[test]
fn missing_exact_member_permanently_retires_even_if_coordinates_return_later() {
    let (b, sdk) = setup(false);
    inspect(&b, &sdk).unwrap();
    sdk.mode.set(2);
    assert!(inspect(&b, &sdk).is_err());
    sdk.mode.set(0);
    let n = sdk.entity_reads.get();
    assert_eq!(inspect(&b, &sdk).unwrap_err(), "native_entities_id_retired");
    assert_eq!(n, sdk.entity_reads.get());
}
#[test]
fn malformed_flags_nonfinite_and_broken_equality_fail_closed() {
    for mode in [4, 5, 6, 7] {
        let (b, sdk) = setup(true);
        sdk.mode.set(mode);
        assert!(inspect(&b, &sdk).is_err(), "mode {mode}");
        assert!(b.entries[0].entities.borrow().is_none());
    }
}
#[test]
fn selectors_require_exact_runtime_document_and_sketch_without_sdk_reads() {
    let (b, sdk) = setup(false);
    for (doc, id) in [
        ("d:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb:0000000000000001", ID),
        (DOC, "Sketch1"),
        (DOC, "0"),
        (
            DOC,
            "s:d:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb:0000000000000001:0000000000000001",
        ),
    ] {
        assert_eq!(
            b.inspect_entities(&sdk, doc, id, None).unwrap_err(),
            "native_sketch_id_unknown"
        );
    }
    assert_eq!(sdk.entity_reads.get(), 0);
}
#[test]
fn unconfirmed_or_unknown_geometry_cannot_publish() {
    let mut b = book();
    let sdk = Sdk::default();
    assert_eq!(
        inspect(&b, &sdk).unwrap_err(),
        "native_entities_confirmed_geometry_required"
    );
    sdk.base.mode.set(2);
    sdk.base.fail_at.set(2);
    let r = request(
        &b,
        &sdk.base,
        json!([{"type":"point","position":[10,20]},{"type":"point","position":[30,40]}]),
        "failed-source",
    );
    let p = b.geometry_preflight(&sdk.base, &r).unwrap();
    assert_eq!(b.execute_geometry(&sdk.base, p)["outcome"], "unknown");
    assert!(b.entries[0].geometry_leases.is_none());
    assert!(inspect(&b, &sdk).is_err());
    assert_eq!(sdk.entity_reads.get(), 0);
}
#[test]
fn stale_first_request_does_not_publish() {
    let (b, sdk) = setup(false);
    assert_eq!(
        b.inspect_entities(&sdk, DOC, ID, Some("sketch-entities-projection-v1:stale"))
            .unwrap_err(),
        "target_revision_mismatch"
    );
    assert!(b.entries[0].entities.borrow().is_none());
    assert!(inspect(&b, &sdk).is_ok());
}
#[test]
fn invalid_or_oversized_private_lease_registry_refuses_before_sdk_reads() {
    for count in [0, 65] {
        let (mut b, sdk) = setup(false);
        b.entries[0].geometry_leases = Some((0..count).collect());
        assert_eq!(inspect(&b, &sdk).unwrap_err(), "native_entities_bound");
        assert_eq!(sdk.entity_reads.get(), 0);
    }
}
#[test]
fn exact_arguments_and_distinct_projection_contract_are_required() {
    for (target, ok) in [
        (json!({"documentId":DOC}), true),
        (
            json!({"documentId":DOC,"revision":"sketch-entities-projection-v1:abc"}),
            true,
        ),
        (
            json!({"documentId":DOC,"revision":"sketch-projection-v1:abc"}),
            false,
        ),
        (json!({"documentId":""}), false),
    ] {
        let request=fusion_mechanical_core::prepare_request(json!({"op":"native_sketch_entities","sessionId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","target":target,"sketchId":ID}),"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        assert_eq!(supported_arguments(&request).is_ok(), ok);
        assert!(!request.is_write());
    }
}
#[test]
fn sixty_four_entities_use_two_bounded_equality_captures_and_refuse_later_capacity_growth() {
    let mut b = book();
    let sdk = Sdk::default();
    let r = request(
        &b,
        &sdk.base,
        json!([{"type":"polygon","center":[0,0],"radius":10,"sides":64}]),
        "max-entities",
    );
    let p = b.geometry_preflight(&sdk.base, &r).unwrap();
    assert_eq!(b.execute_geometry(&sdk.base, p)["success"], true);
    let read = inspect(&b, &sdk).unwrap();
    assert_eq!(read["uniqueEntityCount"], 64);
    assert_eq!(sdk.equality_reads.get(), 2);
    assert_eq!(sdk.base.calls.get(), 64);
    assert!(serde_json::to_vec(&read).unwrap().len() < MAX_BYTES);
    b.entries[0].geometry_leases.as_mut().unwrap().push(64);
    assert_eq!(inspect(&b, &sdk).unwrap_err(), "native_entities_bound");
}
#[test]
fn five_hundred_twelve_spline_points_remain_bounded_and_observable() {
    let mut b = book();
    let sdk = Sdk::default();
    let items:Vec<Value>=(0..4).map(|k|json!({"type":"spline","points":(0..128).map(|i|json!([i as f64,(i%2) as f64+k as f64*10.])).collect::<Vec<_>>(),"closed":false})).collect();
    let r = request(&b, &sdk.base, json!(items), "max-points");
    let p = b.geometry_preflight(&sdk.base, &r).unwrap();
    assert_eq!(b.execute_geometry(&sdk.base, p)["success"], true);
    let read = inspect(&b, &sdk).unwrap();
    assert_eq!(read["uniqueEntityCount"], 4);
    assert_eq!(
        read["entities"][3]["observed"]["geometry"]["fit_points_cm"]
            .as_array()
            .unwrap()
            .len(),
        128
    );
    assert!(serde_json::to_vec(&read).unwrap().len() < MAX_BYTES);
}
#[test]
fn capabilities_keep_all_prior_operations_and_mark_new_read_as_incomplete() {
    let cap = crate::execution::capabilities();
    assert_eq!(cap["contractVersion"], crate::assembly::CONTRACT);
    assert_eq!(
        cap["entitiesCompatibilityHeader"],
        format!("X-Adom-Mechanical-Contract: {CONTRACT}")
    );
    assert_eq!(cap["legacyParity"], false);
    assert_eq!(cap["sketchCompleteInspection"], false);
    let ops = cap["operations"].as_array().unwrap();
    assert_eq!(ops.len(), 27);
    for name in [
        "parameter_create",
        "new_design",
        "bom_export",
        "sketch_geometry",
        "feature_create",
        "native_sketch_entities",
        "material_list",
        "mcmaster_discover",
        "assembly_properties",
    ] {
        assert!(ops.contains(&json!(name)));
        assert!(crate::execution::supported(name));
    }
    assert!(cap["schemas"]["native_sketch_entities"]["required"]
        .as_array()
        .unwrap()
        .contains(&json!("sketchId")));
}

#[test]
fn getter_panic_retires_ids_before_return_and_never_retries_read() {
    let (b, sdk) = setup(false);
    sdk.mode.set(8);
    assert_eq!(
        inspect(&b, &sdk).unwrap_err(),
        "native_entities_capture_panicked"
    );
    let reads = sdk.entity_reads.get();
    sdk.mode.set(0);
    assert_eq!(inspect(&b, &sdk).unwrap_err(), "native_entities_id_retired");
    assert_eq!(reads, sdk.entity_reads.get());
}

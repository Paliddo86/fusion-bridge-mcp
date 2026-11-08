use super::*;
use std::cell::Cell;
const EPOCH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DOC: &str = "d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001";
fn request(extra: Value) -> PreparedRequest {
    let mut v = json!({"op":"assembly_properties","sessionId":EPOCH,"target":{"documentId":DOC}});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    fusion_mechanical_core::prepare_request(v, EPOCH).unwrap()
}
fn sample() -> Snapshot {
    Snapshot {
        revision_before: "root-revision".into(),
        revision_after: "root-revision".into(),
        design_intent: 2,
        configured: false,
        root_bodies: 1,
        root_occurrences: 0,
        mesh_bodies: 0,
        face_count: 3,
        non_solid_bodies: 0,
        accuracy: 2,
        mass_kg: 0.01696460032938488,
        volume_cm3: 2.0 * std::f64::consts::PI,
        area_cm2: 6.0 * std::f64::consts::PI,
        center_cm: [0.0, 0.0, -1.0],
    }
}
struct Fake {
    first: Snapshot,
    second: Snapshot,
    calls: Cell<usize>,
    fail: usize,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            first: sample(),
            second: sample(),
            calls: Cell::new(0),
            fail: 0,
        }
    }
}
impl Backend for Fake {
    fn snapshot(&self) -> Result<Snapshot> {
        let call = self.calls.get() + 1;
        self.calls.set(call);
        if call == self.fail {
            return Err("exact SDK failure".into());
        }
        Ok(if call == 1 {
            self.first.clone()
        } else {
            self.second.clone()
        })
    }
}
#[test]
fn analytic_cylinder_units_centroid_and_density_are_independent_of_sdk_units() {
    let b = Fake::default();
    let q = request(json!({}));
    assert!(!q.is_write());
    let value = inspect(&b, EPOCH, DOC, &q).unwrap();
    assert_eq!(b.calls.get(), 2);
    assert_eq!(value["centerOfMass"], json!([0.0, 0.0, -10.0]));
    assert!((value["volume"].as_f64().unwrap() - 2000.0 * std::f64::consts::PI).abs() < 1e-10);
    assert!((value["area"].as_f64().unwrap() - 600.0 * std::f64::consts::PI).abs() < 1e-10);
    assert!((value["averageDensity"].as_f64().unwrap() - 2.7e-6).abs() < 1e-18);
    assert_eq!(value["arbitraryEntitySelectionSupported"], false);
    assert_eq!(value["assemblyOccurrencesSupported"], false);
}
#[test]
fn empty_root_has_null_density_and_no_fabricated_positive_geometry() {
    let mut s = sample();
    s.root_bodies = 0;
    s.face_count = 0;
    s.mass_kg = 0.0;
    s.volume_cm3 = 0.0;
    s.area_cm2 = 0.0;
    s.center_cm = [0.0; 3];
    let b = Fake {
        first: s.clone(),
        second: s,
        ..Default::default()
    };
    let v = inspect(&b, EPOCH, DOC, &request(json!({}))).unwrap();
    assert!(v["averageDensity"].is_null());
    assert_eq!(v["bodyCount"], 0);
}
#[test]
fn native_failures_and_every_changed_physical_field_refuse_partial_success() {
    for fail in [1, 2] {
        let b = Fake {
            fail,
            ..Default::default()
        };
        assert_eq!(
            inspect(&b, EPOCH, DOC, &request(json!({}))).unwrap_err(),
            "exact SDK failure"
        );
        assert_eq!(b.calls.get(), fail);
    }
    for mode in 0..6 {
        let mut b = Fake::default();
        match mode {
            0 => b.second.mass_kg += 0.001,
            1 => b.second.volume_cm3 += 0.001,
            2 => b.second.area_cm2 += 0.001,
            3 => b.second.center_cm[2] += 0.001,
            4 => b.second.face_count += 1,
            _ => {
                b.second.revision_before = "new".into();
                b.second.revision_after = "new".into();
            }
        };
        assert!(inspect(&b, EPOCH, DOC, &request(json!({}))).is_err());
    }
}
#[test]
fn unsupported_scope_accuracy_and_nonfinite_values_never_publish() {
    for mode in 0..14 {
        let mut b = Fake::default();
        match mode {
            0 => b.first.configured = true,
            1 => b.first.root_occurrences = 1,
            2 => b.first.mesh_bodies = 1,
            3 => b.first.root_bodies = MAX_BODIES + 1,
            4 => b.first.face_count = MAX_FACES + 1,
            5 => b.first.non_solid_bodies = 1,
            6 => b.first.accuracy = 1,
            7 => b.first.mass_kg = f64::NAN,
            8 => b.first.volume_cm3 = f64::INFINITY,
            9 => b.first.area_cm2 = -1.0,
            10 => b.first.center_cm[0] = f64::NEG_INFINITY,
            11 => b.first.revision_after = "changed".into(),
            12 => b.first.design_intent = 1,
            _ => b.first.face_count = 0,
        };
        assert!(
            inspect(&b, EPOCH, DOC, &request(json!({}))).is_err(),
            "{mode}"
        );
        assert_eq!(b.calls.get(), 1);
    }
}
#[test]
fn conversion_and_density_overflow_refuse_json_null_substitution() {
    for mode in 0..4 {
        let mut s = sample();
        match mode {
            0 => s.volume_cm3 = f64::MAX,
            1 => s.area_cm2 = f64::MAX,
            2 => s.center_cm[1] = f64::MAX,
            _ => {
                s.volume_cm3 = f64::MIN_POSITIVE;
                s.mass_kg = f64::MAX;
            }
        };
        let b = Fake {
            first: s.clone(),
            second: s,
            ..Default::default()
        };
        assert_eq!(
            inspect(&b, EPOCH, DOC, &request(json!({}))).unwrap_err(),
            "native_physical_unit_conversion_overflow"
        );
    }
}
#[test]
fn explicit_selectors_and_unknown_target_fields_are_refused_before_backend() {
    assert!(fusion_mechanical_core::prepare_request(json!({"op":"assembly_properties","sessionId":EPOCH,"target":{"documentId":DOC},"entity":null}),EPOCH).is_err());
    for extra in [
        json!({"entity":{"documentId":DOC,"revision":"some-revision","objectType":"Component"}}),
        json!({"target":{}}),
        json!({"target":{"documentId":DOC,"name":"Untitled"}}),
        json!({"target":{"documentId":DOC,"revision":null}}),
        json!({"sessionId":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}),
    ] {
        let b = Fake::default();
        assert!(inspect(&b, EPOCH, DOC, &request(extra)).is_err());
        assert_eq!(b.calls.get(), 0);
    }
}
#[test]
fn revision_binds_properties_document_and_session_and_checks_staleness() {
    let b = Fake::default();
    let first = inspect(&b, EPOCH, DOC, &request(json!({}))).unwrap();
    assert!(inspect(
        &Fake::default(),
        EPOCH,
        DOC,
        &request(json!({"target":first["target"]}))
    )
    .is_ok());
    let mut s = sample();
    s.mass_kg *= 2.0;
    let changed = Fake {
        first: s.clone(),
        second: s,
        ..Default::default()
    };
    assert_eq!(
        inspect(
            &changed,
            EPOCH,
            DOC,
            &request(json!({"target":first["target"]}))
        )
        .unwrap_err(),
        "target_revision_mismatch"
    );
    let doc = "d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000002";
    let next = inspect(
        &Fake::default(),
        EPOCH,
        doc,
        &request(json!({"target":{"documentId":doc}})),
    )
    .unwrap();
    assert_ne!(first["target"]["revision"], next["target"]["revision"]);
}
#[test]
fn exact_bounds_and_backend_target_mismatch() {
    let mut s = sample();
    s.root_bodies = MAX_BODIES;
    s.face_count = MAX_FACES;
    let b = Fake {
        first: s.clone(),
        second: s,
        ..Default::default()
    };
    assert_eq!(
        inspect(&b, EPOCH, DOC, &request(json!({}))).unwrap()["bodyCount"],
        MAX_BODIES
    );
    let b = Fake::default();
    assert!(inspect(&b, EPOCH, "different-document", &request(json!({}))).is_err());
    assert_eq!(b.calls.get(), 0);
}
#[test]
fn capabilities_describe_finite_scope_and_preserve_prior_operations() {
    let cap = crate::execution::capabilities();
    assert_eq!(cap["contractVersion"], crate::assembly::CONTRACT);
    assert_eq!(
        cap["physicalCompatibilityHeader"],
        format!("X-Adom-Mechanical-Contract: {CONTRACT}")
    );
    assert_eq!(cap["rootPhysicalSupport"], true);
    assert_eq!(cap["assemblyOccurrencePhysicalSupport"], false);
    assert_eq!(cap["rootPhysicalMaxBodies"], MAX_BODIES);
    assert_eq!(cap["rootPhysicalMaxFaces"], MAX_FACES);
    assert!(cap["schemas"]["assembly_properties"]["properties"]
        .get("entity")
        .is_none());
    for op in [
        "assembly_properties",
        "mcmaster_discover",
        "material_list",
        "native_sketch_entities",
        "feature_create",
        "sketch_geometry",
        "bom_export",
        "new_design",
        "parameter_set",
    ] {
        assert!(crate::execution::supported(op));
        assert!(cap["operations"].as_array().unwrap().contains(&json!(op)));
    }
    assert_eq!(
        cap["purchasedCompatibilityHeader"],
        "X-Adom-Mechanical-Contract: native-purchased-discovery-v1"
    );
}

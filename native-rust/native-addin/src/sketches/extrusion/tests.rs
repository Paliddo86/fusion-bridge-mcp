use super::*;
use std::cell::{Cell, RefCell};
const DOC: &str = "d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001";
const ID: &str = "s:d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001:0000000000000001";
#[derive(Default)]
pub(crate) struct Fake {
    pub(crate) calls: Cell<usize>,
    pub(crate) mode: Cell<u32>,
    pub(crate) changed: Cell<bool>,
    solid: RefCell<Option<SolidState>>,
    reads: Cell<usize>,
    profile_verifications: Cell<usize>,
    profile_confirmed: Cell<bool>,
    late_root_change: Cell<bool>,
}
impl Backend for Fake {
    type Handle = u64;
    fn root(&self) -> Result<RootState, String> {
        if self.mode.get() == 11 && self.calls.get() > 0 {
            panic!("post-effect root getter");
        }
        Ok(RootState {
            active: true,
            parametric: true,
            hybrid: true,
            configured: false,
            root_revision: format!(
                "r{}-profile{}-late{}",
                self.calls.get(),
                self.profile_verifications.get(),
                self.late_root_change.get()
            ),
            sketch_count: 1,
            timeline_count: 1 + self.calls.get() as u64,
            timeline_position: 1 + self.calls.get() as u64,
        })
    }
    fn command(&self) -> Result<String, String> {
        Ok(if self.mode.get() == 7 {
            "OtherCommand"
        } else {
            "SelectCommand"
        }
        .into())
    }
    fn create(&self, _: u32) -> CreateAttempt<u64> {
        panic!("extrusion cannot create/adopt another sketch")
    }
    fn read(&self, handle: &u64) -> Result<SketchState, String> {
        assert_eq!(*handle, 42);
        if self.mode.get() == 32 && self.reads.get() >= 4 {
            self.late_root_change.set(true);
        }
        if self.mode.get() == 19 && self.calls.get() > 0 {
            panic!("post-effect sketch getter");
        }
        Ok(SketchState {
            name: "Sketch1".into(),
            sdk_revision: format!("s-{}", self.changed.get()),
            plane: 1,
            transform: [0.; 16],
            curves: 4,
            points: 5,
            profiles: if self.mode.get() == 8 { 2 } else { 1 },
            dimensions: 0,
            constraints: 0,
            health: if self.mode.get() == 18 && self.calls.get() > 0 {
                1
            } else {
                0
            },
            deferred: false,
        })
    }
}
impl ExtrusionBackend for Fake {
    fn diagnostic(&self) -> Value {
        match self.mode.get() {
            22 | 23 => sdk_diagnostic(
                7,
                if self.mode.get() == 22 { 213 } else { 214 },
                2,
                b"explicit profile start input failure",
                b"explicit profile start input failure".len(),
                false,
            ),
            _ => Value::Null,
        }
    }
    fn profile(&self, handle: &u64) -> Result<ProfileState, String> {
        assert_eq!(*handle, 42);
        if self.mode.get() == 31 && self.reads.get() >= 4 {
            self.late_root_change.set(true);
        }
        if self.mode.get() == 9 {
            return Err("profile exact membership failed".into());
        }
        Ok(ProfileState {
            area_cm2: if self.mode.get() == 17 && self.calls.get() > 0 {
                7.
            } else {
                6.
            },
            perimeter_cm: 10.,
            centroid_cm: [1., 1.5, 0.],
            loops: if self.mode.get() == 10 { 2 } else { 1 },
            curves: 4,
            root_bodies: self.calls.get() as u64,
            root_extrudes: self.calls.get() as u64,
            on_plane: true,
            outer_only: true,
            high_accuracy: true,
        })
    }
    fn extrude(&self, handle: &u64, distance_cm: f64, negative: bool) -> ExtrudeAttempt {
        assert_eq!(*handle, 42);
        if matches!(self.mode.get(), 1 | 22 | 23) {
            return ExtrudeAttempt {
                invoked: false,
                acknowledged: false,
                stage: "input",
            };
        }
        self.calls.set(self.calls.get() + 1);
        self.solid.replace(Some(SolidState {
            volume_cm3: 6. * distance_cm,
            centroid_sketch_cm: [
                1.,
                1.5,
                if negative {
                    -distance_cm / 2.
                } else {
                    distance_cm / 2.
                },
            ],
            distance_cm,
            taper_radians: 0.,
            feature_health: 0,
            body_solid: true,
            feature_solid: true,
            new_body: true,
            one_side: true,
            profile_plane_start: true,
            high_accuracy: true,
        }));
        if self.mode.get() == 3 {
            panic!("ack loss after actual solid created");
        }
        ExtrudeAttempt {
            invoked: true,
            acknowledged: self.mode.get() != 2,
            stage: "feature_add",
        }
    }
    fn verify_extrude_profile(&self, handle: &u64) -> Result<(), String> {
        assert_eq!(*handle, 42);
        assert_eq!(self.calls.get(), 1);
        let prior = self
            .profile_verifications
            .replace(self.profile_verifications.get() + 1);
        assert_eq!(prior, 0, "profile barrier repeated");
        match self.mode.get() {
            26 => return Err("exact feature/profile mismatch".into()),
            27 => return Err("profile getter SDK error".into()),
            28 => panic!("profile getter panicked"),
            _ => {}
        }
        self.profile_confirmed.set(true);
        Ok(())
    }
    fn solid(&self, handle: &u64) -> Result<SolidState, String> {
        assert_eq!(*handle, 42);
        assert!(self.profile_confirmed.get());
        if self.mode.get() == 30 {
            self.late_root_change.set(true);
        }
        self.reads.set(self.reads.get() + 1);
        if self.mode.get() == 4 {
            return Err("read unavailable".into());
        }
        let mut s = self.solid.borrow().clone().unwrap();
        match self.mode.get() {
            5 => s.volume_cm3 *= 1.1,
            6 => s.centroid_sketch_cm[2] *= -1.,
            12 => s.body_solid = false,
            13 => s.feature_health = 1,
            14 => s.one_side = false,
            15 => s.taper_radians = 0.1,
            16 if self.reads.get() > 2 => s.volume_cm3 += 1.,
            20 => s.centroid_sketch_cm[0] += 0.5,
            21 => s.centroid_sketch_cm[1] -= 0.5,
            24 => s.profile_plane_start = false,
            _ => {}
        }
        Ok(s)
    }
}
pub(crate) fn book() -> Book<u64> {
    let mut b = Book::default();
    b.entries.push(Entry {
        id: ID.into(),
        document_id: DOC.into(),
        handle: Some(42),
        retired: Cell::new(false),
        profile_published: Cell::new(false),
        profile_retired: Cell::new(false),
        geometry_leases: None,
        entities: RefCell::new(None),
        entities_retired: Cell::new(false),
        dimension_endpoints_published: Cell::new(false),
    });
    b
}
pub(crate) fn request(b: &Book<u64>, sdk: &Fake, operation: &str) -> PreparedRequest {
    let data = b.inspect_profiles(sdk, DOC, ID, None).unwrap();
    let profile = data["profiles"][0]["reference"].clone();
    fusion_mechanical_core::prepare_request(json!({"op":"feature_create","sessionId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","operationId":operation,"type":"extrude","target":data["target"],"profiles":[profile],"distance":40.,"operation":"new_body"}),"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap()
}
#[test]
fn positive_and_negative_extrusions_convert_mm_once_and_verify_actual_volume() {
    for negative in [false, true] {
        let mut b = book();
        let sdk = Fake::default();
        let r = request(&b, &sdk, "solid");
        let mut a = r.arguments().clone();
        a["direction"] = json!(if negative { "negative" } else { "positive" });
        let r =
            fusion_mechanical_core::prepare_request(a, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let p = b.extrude_preflight(&sdk, &r).unwrap();
        assert_eq!(sdk.calls.get(), 0);
        let out = b.execute_extrude(&sdk, p);
        assert_eq!(out["success"], true, "{out}");
        assert_eq!(out["data"]["observed"]["distanceCm"], 4.);
        assert_eq!(out["data"]["observed"]["volumeCm3"], 24.);
        assert_eq!(out["data"]["volumeMm3"], 24000.);
        assert_eq!(
            out["data"]["observed"]["centroidSketchCm"][2],
            if negative { -2. } else { 2. }
        );
        assert_eq!(out["rollbackAttempted"], false);
        assert_eq!(sdk.calls.get(), 1);
    }
}
#[test]
fn same_volume_translated_body_is_unknown_in_both_extent_directions() {
    for negative in [false, true] {
        for mode in [20, 21] {
            let mut b = book();
            let sdk = Fake::default();
            let r = request(&b, &sdk, "translated");
            let mut args = r.arguments().clone();
            args["direction"] = json!(if negative { "negative" } else { "positive" });
            let r =
                fusion_mechanical_core::prepare_request(args, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                    .unwrap();
            let plan = b.extrude_preflight(&sdk, &r).unwrap();
            sdk.mode.set(mode);
            let out = b.execute_extrude(&sdk, plan);
            assert_eq!(out["outcome"], "unknown");
            assert_eq!(out["observedSolid"]["volumeCm3"], 24.);
            assert_eq!(
                out["observedSolid"]["centroidSketchCm"][2],
                if negative { -2. } else { 2. }
            );
            assert!(b.extrude_preflight(&sdk, &r).is_err());
            assert_eq!(sdk.calls.get(), 1);
        }
    }
}
#[test]
fn explicit_published_profile_identity_required_and_never_retargets_another_id() {
    let b = book();
    let sdk = Fake::default();
    let r = request(&b, &sdk, "explicit");
    let mut a = r.arguments().clone();
    a["profiles"][0]["profileId"] = json!("another-profile-with-identical-area");
    let r = fusion_mechanical_core::prepare_request(a, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    assert!(b.extrude_preflight(&sdk, &r).is_err());
    let unpublished = book();
    assert!(unpublished
        .extrude_preflight(&sdk, &request(&b, &sdk, "published-elsewhere"))
        .is_err());
    assert_eq!(sdk.calls.get(), 0);
}
#[test]
fn unsupported_extent_quantity_operation_and_target_keys_are_rejected_purely() {
    let b = book();
    let sdk = Fake::default();
    let r = request(&b, &sdk, "scope");
    for (key, value) in [
        ("direction", json!("symmetric")),
        ("distance", json!("40 mm")),
        ("distance", json!(0)),
        ("distance", json!(-10)),
        ("distance", json!(100001)),
        ("operation", json!("join")),
        ("taper", json!(0)),
        ("name", json!("ignored")),
        (
            "component",
            json!({"documentId":DOC,"revision":"x","objectType":"Component"}),
        ),
    ] {
        let mut a = r.arguments().clone();
        a[key] = value;
        let r =
            fusion_mechanical_core::prepare_request(a, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        assert!(supported_arguments(&r).is_err(), "{key}");
    }
    assert_eq!(sdk.calls.get(), 0);
}
#[test]
fn multiple_profiles_and_holes_never_publish_implicit_selection() {
    for mode in [8, 10] {
        let b = book();
        let sdk = Fake::default();
        sdk.mode.set(mode);
        assert!(b.inspect_profiles(&sdk, DOC, ID, None).is_err());
        assert!(!b.entries[0].profile_published.get());
        assert_eq!(sdk.calls.get(), 0);
    }
}
#[test]
fn failed_exact_profile_membership_is_permanently_retired() {
    let b = book();
    let sdk = Fake::default();
    let r = request(&b, &sdk, "retired");
    sdk.mode.set(9);
    assert!(b.inspect_profiles(&sdk, DOC, ID, None).is_err());
    sdk.mode.set(0);
    assert!(b.inspect_profiles(&sdk, DOC, ID, None).is_err());
    assert!(b.extrude_preflight(&sdk, &r).is_err());
    assert_eq!(sdk.calls.get(), 0);
}
#[test]
fn stale_between_preflight_and_effect_refuses_before_add() {
    let mut b = book();
    let sdk = Fake::default();
    let p = b
        .extrude_preflight(&sdk, &request(&b, &sdk, "stale"))
        .unwrap();
    sdk.changed.set(true);
    let out = b.execute_extrude(&sdk, p);
    assert_eq!(out["nativeExecutionStarted"], false);
    assert_eq!(out["errorCode"], "target_revision_mismatch");
    assert_eq!(sdk.calls.get(), 0);
}
#[test]
fn noninvoked_input_failure_has_no_effect_but_never_automatically_retries() {
    let mut b = book();
    let sdk = Fake::default();
    let p = b
        .extrude_preflight(&sdk, &request(&b, &sdk, "input"))
        .unwrap();
    sdk.mode.set(1);
    let out = b.execute_extrude(&sdk, p);
    assert_eq!(out["nativeExecutionStarted"], false);
    assert_eq!(sdk.calls.get(), 0);
}
#[test]
fn explicit_start_input_failures_preserve_diagnostics_without_invoking_add() {
    for (mode, stage) in [(22, 213), (23, 214)] {
        let mut b = book();
        let sdk = Fake::default();
        let p = b
            .extrude_preflight(&sdk, &request(&b, &sdk, "explicit-start"))
            .unwrap();
        sdk.mode.set(mode);
        let out = b.execute_extrude(&sdk, p);
        assert_eq!(out["errorCode"], "native_extrude_not_invoked");
        assert_eq!(out["nativeExecutionStarted"], false);
        assert_eq!(out["sdkDiagnostic"]["stage"], stage);
        assert_eq!(out["sdkDiagnostic"]["sdkError"], 2);
        assert_eq!(sdk.calls.get(), 0);
        assert_eq!(sdk.reads.get(), 0);
        assert!(sdk.solid.borrow().is_none());
    }
}
#[test]
fn explicit_input_does_not_replace_actual_feature_start_readback() {
    let mut b = book();
    let sdk = Fake::default();
    let r = request(&b, &sdk, "wrong-actual-start");
    let p = b.extrude_preflight(&sdk, &r).unwrap();
    sdk.mode.set(24);
    let out = b.execute_extrude(&sdk, p);
    assert_eq!(out["outcome"], "unknown");
    assert_eq!(out["observedSolid"]["profilePlaneStart"], false);
    assert_eq!(sdk.calls.get(), 1);
    assert!(b.extrude_preflight(&sdk, &r).is_err());
}
#[test]
fn every_post_effect_failure_is_unknown_and_faults_future_authoring() {
    for mode in [2, 3, 4, 5, 6, 11, 12, 13, 14, 15, 16, 17, 18, 19] {
        let mut b = book();
        let sdk = Fake::default();
        let r = request(&b, &sdk, "fault");
        let p = b.extrude_preflight(&sdk, &r).unwrap();
        sdk.mode.set(mode);
        let out = b.execute_extrude(&sdk, p);
        assert_eq!(out["outcome"], "unknown", "mode {mode}: {out}");
        assert_eq!(sdk.calls.get(), 1);
        assert!(b.extrude_preflight(&sdk, &r).is_err());
        if matches!(mode, 5 | 6 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19) {
            assert!(out["observedSolid"].is_object());
        }
    }
}
#[test]
fn final_projection_failure_preserves_decisive_values_without_retry() {
    let mut b = book();
    let sdk = Fake::default();
    let r = request(&b, &sdk, "final-diagnostic");
    let p = b.extrude_preflight(&sdk, &r).unwrap();
    sdk.mode.set(16);
    let out = b.execute_extrude(&sdk, p);
    assert_eq!(out["errorCode"], "native_extrude_final_projection_changed");
    assert_eq!(out["outcome"], "unknown");
    assert_eq!(out["projectionDiagnostic"]["stage"], "solid");
    assert_eq!(out["projectionDiagnostic"]["expected"]["volumeCm3"], 24.0);
    assert_eq!(out["projectionDiagnostic"]["observed"]["volumeCm3"], 25.0);
    assert_eq!(
        out["projectionDiagnostic"]["additionalReadAttempted"],
        false
    );
    assert_eq!(sdk.calls.get(), 1);
    assert_eq!(sdk.reads.get(), 4);
    assert!(b.extrude_preflight(&sdk, &r).is_err());
}
#[test]
fn final_projection_getter_failure_retains_stage_without_fabricated_observation() {
    let got =
        final_observation::<u64>("profile", &7, Err("getter unavailable".into())).unwrap_err();
    assert_eq!(got["stage"], "profile");
    assert_eq!(got["expected"], 7);
    assert!(got["observed"].is_null());
    assert_eq!(got["error"], "getter unavailable");
    assert_eq!(got["additionalReadAttempted"], false);
}
#[test]
fn already_existing_root_body_refuses_even_with_fresh_explicit_profile() {
    let mut b = book();
    let sdk = Fake::default();
    let p = b
        .extrude_preflight(&sdk, &request(&b, &sdk, "first"))
        .unwrap();
    assert_eq!(b.execute_extrude(&sdk, p)["success"], true);
    let r = request(&b, &sdk, "second");
    assert!(b.extrude_preflight(&sdk, &r).is_err());
    assert_eq!(sdk.calls.get(), 1);
}
#[test]
fn feature_body_leases_release_only_with_exact_parent_and_uncertainty_blocks_teardown() {
    let mut b = book();
    let sdk = Fake::default();
    let p = b
        .extrude_preflight(&sdk, &request(&b, &sdk, "release"))
        .unwrap();
    assert_eq!(b.execute_extrude(&sdk, p)["success"], true);
    let mut seen = vec![];
    assert!(b
        .close(|h| {
            seen.push(h);
            Err("child feature release uncertain".into())
        })
        .is_err());
    assert_eq!(seen, vec![42]);
    assert!(b.ensure_released().is_err());
    assert!(b.close(|_| panic!("retry release")).is_err());
}

#[test]
fn malformed_or_oversized_optional_sdk_description_preserves_status_stage_and_code() {
    let malformed = sdk_diagnostic(7, 113, 3, &[0x66, 0x80, 0x00], 3, false);
    assert_eq!(malformed["abiStatus"], 7);
    assert_eq!(malformed["stage"], 113);
    assert_eq!(malformed["sdkError"], 3);
    assert_eq!(malformed["descriptionLossy"], true);
    let oversized = sdk_diagnostic(4, 209, 17, &vec![b'x'; 4096], 9000, false);
    assert_eq!(oversized["sdkError"], 17);
    assert_eq!(oversized["descriptionTruncated"], true);
    assert_eq!(oversized["description"].as_str().unwrap().len(), 4096);
    let unavailable = sdk_diagnostic(7, 1, -1, &[], 0, false);
    assert!(unavailable["sdkError"].is_null());
}
#[test]
fn scoped_profile_inspection_is_read_only_and_profile_ref_is_explicit() {
    let b = book();
    let sdk = Fake::default();
    let request=fusion_mechanical_core::prepare_request(json!({"op":"native_sketch_profiles","sessionId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","target":{"documentId":DOC},"sketchId":ID}),"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    assert!(!request.is_write());
    supported_arguments(&request).unwrap();
    let data = b.inspect_profiles(&sdk, DOC, ID, None).unwrap();
    assert_eq!(data["profiles"].as_array().unwrap().len(), 1);
    assert_eq!(
        data["profiles"][0]["reference"]["profileId"],
        profile_id(ID)
    );
    assert_eq!(data["profiles"][0]["observed"]["areaCm2"], 6.);
    assert_eq!(sdk.calls.get(), 0);
}

#[test]
fn profile_link_revision_advance_precedes_freshness_baseline_and_keeps_four_solid_reads() {
    let mut b = book();
    let sdk = Fake::default();
    let r = request(&b, &sdk, "profile-barrier");
    let p = b.extrude_preflight(&sdk, &r).unwrap();
    let out = b.execute_extrude(&sdk, p);
    assert_eq!(out["success"], true, "{out}");
    assert_eq!(sdk.profile_verifications.get(), 1);
    assert_eq!(sdk.reads.get(), 4);
    assert_eq!(sdk.calls.get(), 1);
}
#[test]
fn profile_link_mismatch_error_or_panic_remains_unknown_without_later_read_or_retry() {
    for mode in [26, 27, 28] {
        let mut b = book();
        let sdk = Fake::default();
        let r = request(&b, &sdk, "profile-barrier-fail");
        let p = b.extrude_preflight(&sdk, &r).unwrap();
        sdk.mode.set(mode);
        let out = b.execute_extrude(&sdk, p);
        assert_eq!(out["errorCode"], "native_extrude_profile_link_unconfirmed");
        assert_eq!(out["outcome"], "unknown");
        assert_eq!(out["nativeExecutionStarted"], true);
        assert_eq!(sdk.reads.get(), 0);
        assert_eq!(sdk.profile_verifications.get(), 1);
        assert!(!sdk.profile_confirmed.get());
        assert_eq!(sdk.calls.get(), 1);
        assert!(b.extrude_preflight(&sdk, &r).is_err());
        assert_eq!(sdk.profile_verifications.get(), 1);
    }
}
#[test]
fn freshness_guard_detects_later_changes_during_solid_profile_or_final_sketch_read() {
    for mode in [30, 31, 32] {
        let mut b = book();
        let sdk = Fake::default();
        let r = request(&b, &sdk, "late-root-change");
        let p = b.extrude_preflight(&sdk, &r).unwrap();
        sdk.mode.set(mode);
        let out = b.execute_extrude(&sdk, p);
        assert_eq!(
            out["errorCode"], "native_extrude_final_projection_changed",
            "{out}"
        );
        assert_eq!(out["outcome"], "unknown");
        assert_eq!(out["projectionDiagnostic"]["stage"], "root");
        assert_ne!(
            out["projectionDiagnostic"]["expected"]["rootRevision"],
            out["projectionDiagnostic"]["observed"]["rootRevision"]
        );
        assert_eq!(sdk.profile_verifications.get(), 1);
        assert_eq!(sdk.reads.get(), 4);
        assert_eq!(sdk.calls.get(), 1);
        assert!(b.extrude_preflight(&sdk, &r).is_err());
    }
}

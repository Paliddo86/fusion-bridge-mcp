//! First finite solid extrusion contract. No boolean, expression, taper or rollback parity.
use super::*;
const SCOPE:&str="profile-projection-v1: exact retained sole profile and parent sketch/root SDK revision, profile area/perimeter/loop/curve counts and root body/extrude counts. Not a complete model revision.";
pub const CONTRACT: &str = "native-extrude-v1";
/// Diagnostics never replace status/stage/error authority with a text decode error.
#[cfg(any(windows, test))]
pub(crate) fn sdk_diagnostic(
    status: i32,
    stage: u32,
    sdk_error: i32,
    bytes: &[u8],
    reported: usize,
    truncated: bool,
) -> Value {
    let n = reported.min(bytes.len()).min(4096);
    let text = &bytes[..n];
    json!({"abiStatus":status,"stage":stage,"sdkError":if sdk_error<0 {None}else{Some(sdk_error)},"description":String::from_utf8_lossy(text),"descriptionLossy":std::str::from_utf8(text).is_err(),"descriptionTruncated":truncated||reported>n,"scope":if sdk_error<0{"inherited shared validation; original SDK error unavailable"}else{"immediate SDK error observation; no reset or retry"}})
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileState {
    pub area_cm2: f64,
    pub perimeter_cm: f64,
    pub centroid_cm: [f64; 3],
    pub loops: u64,
    pub curves: u64,
    pub root_bodies: u64,
    pub root_extrudes: u64,
    pub on_plane: bool,
    pub outer_only: bool,
    pub high_accuracy: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SolidState {
    pub volume_cm3: f64,
    pub centroid_sketch_cm: [f64; 3],
    pub distance_cm: f64,
    pub taper_radians: f64,
    pub feature_health: u32,
    pub body_solid: bool,
    pub feature_solid: bool,
    pub new_body: bool,
    pub one_side: bool,
    pub profile_plane_start: bool,
    pub high_accuracy: bool,
}
pub struct ExtrudeAttempt {
    pub invoked: bool,
    pub acknowledged: bool,
    pub stage: &'static str,
}
pub trait ExtrusionBackend: Backend {
    fn clear_diagnostic(&self) {}
    fn diagnostic(&self) -> Value {
        Value::Null
    }
    /// First successful call retains the sole exact current profile. Subsequent
    /// calls must compare that exact retained object; replacement is never adopted.
    fn profile(&self, sketch: &Self::Handle) -> Result<ProfileState, String>;
    /// Retain the exact feature return and its exact sole body beneath the sketch
    /// before any fallible readback. These leases share the existing release fence.
    fn extrude(&self, sketch: &Self::Handle, distance_cm: f64, negative: bool) -> ExtrudeAttempt;
    /// Once after acknowledged add: validate the exact returned feature's profile.
    /// This getter may advance the component revision, so Rust captures the
    /// post-effect freshness baseline only after it succeeds. Never retry.
    fn verify_extrude_profile(&self, sketch: &Self::Handle) -> Result<(), String>;
    /// Requires the once-verified exact profile relationship on these retained
    /// handles; exact feature/body/root membership and parent checks stay fresh.
    fn solid(&self, sketch: &Self::Handle) -> Result<SolidState, String>;
}
#[derive(Debug)]
pub struct Plan {
    document_id: String,
    sketch_id: String,
    profile_id: String,
    revision: String,
    root: RootState,
    sketch: SketchState,
    profile: ProfileState,
    distance_cm: f64,
    negative: bool,
}
impl Plan {
    pub fn document_id(&self) -> &str {
        &self.document_id
    }
    pub fn observed_document(&self) -> Value {
        json!({"documentId":self.document_id,"revision":self.revision,"sketchId":self.sketch_id,"profileId":self.profile_id,"revisionScope":SCOPE})
    }
}
fn profile_id(sketch_id: &str) -> String {
    format!("p:{sketch_id}:0000000000000001")
}
fn safe<T>(call: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(call))
        .map_err(|_| "native_extrude_getter_panicked".to_owned())?
}
// Retain the decisive final observation without changing the existing comparison
// policy or issuing additional SDK reads after the first failed comparison.
fn final_observation<T: PartialEq + Serialize>(
    stage: &str,
    expected: &T,
    observed: Result<T, String>,
) -> Result<(), Value> {
    match observed {
        Ok(actual) if &actual == expected => Ok(()),
        Ok(actual) => Err(
            json!({"stage":stage,"expected":expected,"observed":actual,"error":null,"comparison":"exact","additionalReadAttempted":false}),
        ),
        Err(error) => Err(
            json!({"stage":stage,"expected":expected,"observed":null,"error":error,"comparison":"exact","additionalReadAttempted":false}),
        ),
    }
}
fn profile_snapshot<B: ExtrusionBackend>(
    backend: &B,
    handle: &B::Handle,
) -> Result<ProfileState, String> {
    let a = backend.profile(handle)?;
    let b = backend.profile(handle)?;
    if a != b
        || !a.high_accuracy
        || !a.on_plane
        || !a.outer_only
        || a.loops != 1
        || !(1..=256).contains(&a.curves)
        || !a.area_cm2.is_finite()
        || !(1e-10..=1e8).contains(&a.area_cm2)
        || !a.perimeter_cm.is_finite()
        || !(1e-5..=1e6).contains(&a.perimeter_cm)
        || !a
            .centroid_cm
            .iter()
            .all(|x| x.is_finite() && x.abs() <= 10000.)
        || a.centroid_cm[2].abs() > 1e-7
        || a.root_bodies > 512
        || a.root_extrudes > 512
    {
        return Err("native_extrude_profile_scope_refused".into());
    }
    Ok(a)
}
fn solid_snapshot<B: ExtrusionBackend>(
    backend: &B,
    handle: &B::Handle,
) -> Result<SolidState, String> {
    let a = backend.solid(handle)?;
    let b = backend.solid(handle)?;
    if a != b
        || !a.volume_cm3.is_finite()
        || !a.distance_cm.is_finite()
        || !a.taper_radians.is_finite()
        || !a.centroid_sketch_cm.iter().all(|x| x.is_finite())
    {
        return Err("native_extrude_readback_changed".into());
    }
    Ok(a)
}
fn revision(
    doc: &str,
    id: &str,
    root: &RootState,
    sketch: &SketchState,
    profile: &ProfileState,
) -> Result<String, String> {
    digest(
        "profile-projection-v1",
        &(doc, id, profile_id(id), root, sketch, profile),
    )
}
pub fn supported_arguments(request: &PreparedRequest) -> Result<(), &'static str> {
    if !matches!(
        request.operation(),
        "feature_create" | "native_sketch_profiles"
    ) {
        return Ok(());
    }
    let a = request.arguments();
    let target = a["target"]
        .as_object()
        .ok_or("native_extrude_exact_target_required")?;
    let document = target
        .get("documentId")
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .ok_or("native_extrude_exact_target_required")?;
    if target
        .keys()
        .any(|k| !matches!(k.as_str(), "documentId" | "revision"))
    {
        return Err("native_extrude_exact_target_required");
    }
    if request.operation() == "native_sketch_profiles" {
        if a["sketchId"].as_str().is_none_or(str::is_empty) {
            return Err("native_extrude_exact_profile_required");
        }
        if target.get("revision").is_some_and(|v| {
            !v.as_str()
                .is_some_and(|s| s.starts_with("profile-projection-v1:"))
        }) {
            return Err("native_extrude_profile_revision_required");
        }
        return Ok(());
    }
    if a["type"] != "extrude"
        || a.get("operation").is_some_and(|v| v != "new_body")
        || a.get("direction")
            .is_some_and(|v| !matches!(v.as_str(), Some("positive" | "negative")))
        || a.get("taper").is_some()
        || a.get("component").is_some()
        || a.get("name").is_some()
        || a.get("bodies").is_some()
    {
        return Err("native_extrude_subset_required");
    }
    let distance = a["distance"]
        .as_f64()
        .filter(|v| v.is_finite() && (0.0001..=100000.).contains(v))
        .ok_or("native_extrude_numeric_distance_required")?;
    if !(distance / 10.).is_finite() {
        return Err("native_extrude_numeric_distance_required");
    }
    let revision = target
        .get("revision")
        .and_then(Value::as_str)
        .filter(|s| s.starts_with("profile-projection-v1:"))
        .ok_or("native_extrude_profile_revision_required")?;
    let profiles = a["profiles"]
        .as_array()
        .filter(|p| p.len() == 1)
        .ok_or("native_extrude_one_profile_required")?;
    let p = profiles[0]
        .as_object()
        .ok_or("native_extrude_exact_profile_required")?;
    if p.keys().any(|k| {
        !matches!(
            k.as_str(),
            "documentId" | "revision" | "objectType" | "sketchId" | "profileId"
        )
    }) || p.get("documentId").and_then(Value::as_str) != Some(document)
        || p.get("revision").and_then(Value::as_str) != Some(revision)
        || p.get("objectType").and_then(Value::as_str) != Some("Profile")
        || p.get("sketchId")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || p.get("profileId")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err("native_extrude_exact_profile_required");
    }
    Ok(())
}
fn root_allowed(root: &RootState) -> bool {
    root.active
        && root.parametric
        && root.hybrid
        && !root.configured
        && root.timeline_count == root.timeline_position
}
fn sketch_allowed(sketch: &SketchState) -> bool {
    matches!(sketch.plane, 1..=3) && sketch.profiles == 1 && sketch.health == 0 && !sketch.deferred
}
fn unknown_result(
    code: &str,
    plan: &Plan,
    observed: Option<&SolidState>,
    stage: Option<&str>,
) -> Value {
    json!({"success":false,"errorCode":code,"outcome":"unknown","nativeExecutionStarted":true,"reconciliationRequired":true,"sketchId":plan.sketch_id,"profileId":plan.profile_id,"observedSolid":observed,"failedStage":stage,"rollbackAttempted":false,"automaticReplayAllowed":false})
}
impl<H> Book<H> {
    pub fn inspect_profiles<B: ExtrusionBackend<Handle = H>>(
        &self,
        backend: &B,
        document: &str,
        id: &str,
        expected: Option<&str>,
    ) -> Result<Value, String> {
        backend.clear_diagnostic();
        self.ready()?;
        let entry = self
            .entries
            .iter()
            .find(|e| e.document_id == document && e.id == id && !e.retired.get())
            .ok_or("native_sketch_id_unknown")?;
        let handle = entry
            .handle
            .as_ref()
            .ok_or("native_sketch_creation_unknown")?;
        if entry.profile_retired.get() {
            return Err("native_extrude_profile_retired".into());
        }
        let root = root_snapshot(backend)?;
        let sketch = sketch_snapshot(backend, handle)?;
        if !root_allowed(&root) || !sketch_allowed(&sketch) {
            return Err("native_extrude_single_healthy_profile_required".into());
        }
        let profile = profile_snapshot(backend, handle).map_err(|e| {
            entry.profile_retired.set(true);
            e
        })?;
        if entry.profile_retired.get() {
            return Err("native_extrude_profile_retired".into());
        }
        if root_snapshot(backend)? != root || sketch_snapshot(backend, handle)? != sketch {
            return Err("native_extrude_projection_changed".into());
        }
        let revision = revision(document, id, &root, &sketch, &profile)?;
        if expected.is_some_and(|s| s != revision) {
            return Err("target_revision_mismatch".into());
        }
        entry.profile_published.set(true);
        Ok(
            json!({"target":{"documentId":document,"revision":revision},"sketchId":id,"profiles":[{"reference":{"documentId":document,"revision":revision,"objectType":"Profile","sketchId":id,"profileId":profile_id(id)},"observed":profile}],"revisionScope":SCOPE,"root":root,"sketch":sketch,"complete":false}),
        )
    }
    pub fn extrude_preflight<B: ExtrusionBackend<Handle = H>>(
        &self,
        backend: &B,
        request: &PreparedRequest,
    ) -> Result<Plan, String> {
        backend.clear_diagnostic();
        self.ready()?;
        supported_arguments(request).map_err(str::to_string)?;
        if request.operation() != "feature_create" {
            return Err("native_extrude_operation_required".into());
        }
        let a = request.arguments();
        let p = &a["profiles"][0];
        let document = a["target"]["documentId"].as_str().unwrap();
        let id = p["sketchId"].as_str().unwrap();
        let entry = self
            .entries
            .iter()
            .find(|e| {
                e.document_id == document
                    && e.id == id
                    && !e.retired.get()
                    && e.profile_published.get()
                    && !e.profile_retired.get()
            })
            .ok_or("native_extrude_profile_not_published")?;
        if p["profileId"] != profile_id(id) {
            return Err("native_extrude_profile_id_unknown".into());
        }
        let handle = entry
            .handle
            .as_ref()
            .ok_or("native_sketch_creation_unknown")?;
        let root = root_snapshot(backend)?;
        let sketch = sketch_snapshot(backend, handle)?;
        let profile = profile_snapshot(backend, handle).map_err(|e| {
            entry.profile_retired.set(true);
            e
        })?;
        let revision = revision(document, id, &root, &sketch, &profile)?;
        if a["target"]["revision"] != revision {
            return Err("target_revision_mismatch".into());
        }
        if !root_allowed(&root)
            || !sketch_allowed(&sketch)
            || profile.root_bodies != 0
            || profile.root_extrudes != 0
            || backend.command()? != "SelectCommand"
        {
            return Err("native_extrude_empty_body_root_required".into());
        }
        Ok(Plan {
            document_id: document.into(),
            sketch_id: id.into(),
            profile_id: profile_id(id),
            revision,
            root,
            sketch,
            profile,
            distance_cm: a["distance"].as_f64().unwrap() / 10.,
            negative: a["direction"] == "negative",
        })
    }
    pub fn execute_extrude<B: ExtrusionBackend<Handle = H>>(
        &mut self,
        backend: &B,
        plan: Plan,
    ) -> Value {
        backend.clear_diagnostic();
        let mut value = self.execute_extrude_inner(backend, plan);
        if value["success"] != true {
            value["sdkDiagnostic"] = backend.diagnostic();
        }
        value
    }
    fn execute_extrude_inner<B: ExtrusionBackend<Handle = H>>(
        &mut self,
        backend: &B,
        plan: Plan,
    ) -> Value {
        if let Err(code) = self.ready() {
            return err(&code);
        }
        let Some(entry) = self.entries.iter().find(|e| {
            e.document_id == plan.document_id
                && e.id == plan.sketch_id
                && !e.retired.get()
                && e.profile_published.get()
                && !e.profile_retired.get()
        }) else {
            return err("native_extrude_profile_not_published");
        };
        let Some(handle) = entry.handle.as_ref() else {
            return err("native_sketch_creation_unknown");
        };
        if root_snapshot(backend).ok().as_ref() != Some(&plan.root)
            || sketch_snapshot(backend, handle).ok().as_ref() != Some(&plan.sketch)
        {
            return err("target_revision_mismatch");
        }
        let current_profile = match profile_snapshot(backend, handle) {
            Ok(value) => value,
            Err(code) => {
                entry.profile_retired.set(true);
                return err(&code);
            }
        };
        if current_profile != plan.profile {
            return err("target_revision_mismatch");
        }
        if backend.command().ok().as_deref() != Some("SelectCommand") {
            return err("native_extrude_command_refused");
        }
        self.faulted = true;
        let attempt = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            backend.extrude(handle, plan.distance_cm, plan.negative)
        })) {
            Ok(a) => a,
            Err(_) => {
                return unknown_result("native_extrude_invocation_panicked", &plan, None, None)
            }
        };
        if !attempt.invoked && !attempt.acknowledged {
            self.faulted = false;
            return err("native_extrude_not_invoked");
        }
        if !attempt.invoked || !attempt.acknowledged {
            return unknown_result(
                "native_extrude_unconfirmed",
                &plan,
                None,
                Some(attempt.stage),
            );
        }
        // Fusion 2705.1.30 advances the root revision while reading the
        // returned feature's profile (observed in both O3 getter traces).
        // Verify that relationship once, then bracket ALL repeated readback
        // with the resulting exact root revision. No equality is relaxed.
        if safe(|| backend.verify_extrude_profile(handle)).is_err() {
            return unknown_result("native_extrude_profile_link_unconfirmed", &plan, None, None);
        }
        let root = match safe(|| root_snapshot(backend)) {
            Ok(v) => v,
            Err(_) => return unknown_result("native_extrude_root_unconfirmed", &plan, None, None),
        };
        let observed = match safe(|| solid_snapshot(backend, handle)) {
            Ok(v) => v,
            Err(_) => {
                return unknown_result("native_extrude_readback_unconfirmed", &plan, None, None)
            }
        };
        let expected_volume = plan.profile.area_cm2 * plan.distance_cm;
        let expected_center_z = if plan.negative {
            -plan.distance_cm / 2.
        } else {
            plan.distance_cm / 2.
        };
        // Both independent high-accuracy property estimates have documented
        // +/-0.1% error; allow 0.3% combined with a tiny absolute tolerance.
        if !observed.body_solid
            || !observed.feature_solid
            || !observed.new_body
            || !observed.one_side
            || !observed.profile_plane_start
            || !observed.high_accuracy
            || observed.feature_health != 0
            || observed.volume_cm3 <= 0.
            || (observed.volume_cm3 - expected_volume).abs() > 1e-8 + 0.003 * expected_volume
            || (observed.distance_cm.abs() - plan.distance_cm).abs()
                > 1e-7 + 1e-12 * plan.distance_cm
            || observed.taper_radians.abs() > 1e-10
            || observed.centroid_sketch_cm[..2]
                .iter()
                .zip(&plan.profile.centroid_cm[..2])
                .any(|(actual, expected)| {
                    (actual - expected).abs() > 1e-6 + 0.003 * plan.profile.area_cm2.sqrt()
                })
            || (observed.centroid_sketch_cm[2] - expected_center_z).abs()
                > 1e-6 + 0.003 * plan.distance_cm
        {
            return unknown_result(
                "native_extrude_readback_mismatch",
                &plan,
                Some(&observed),
                None,
            );
        }
        let profile = match safe(|| profile_snapshot(backend, handle)) {
            Ok(v) => v,
            Err(_) => {
                return unknown_result(
                    "native_extrude_profile_unconfirmed",
                    &plan,
                    Some(&observed),
                    None,
                )
            }
        };
        let sketch = match safe(|| sketch_snapshot(backend, handle)) {
            Ok(v) => v,
            Err(_) => {
                return unknown_result(
                    "native_extrude_sketch_unconfirmed",
                    &plan,
                    Some(&observed),
                    None,
                )
            }
        };
        // Extrusion may advance a parent's SDK revision; it must not silently
        // replace or change the geometry of the exact input we authorized.
        let mut expected_profile = plan.profile.clone();
        expected_profile.root_bodies = 1;
        expected_profile.root_extrudes = 1;
        let mut expected_sketch = plan.sketch.clone();
        expected_sketch.sdk_revision = sketch.sdk_revision.clone();
        if !root_allowed(&root)
            || root.sketch_count != plan.root.sketch_count
            || root.timeline_count != plan.root.timeline_count + 1
            || root.root_revision == plan.root.root_revision
            || profile != expected_profile
            || sketch != expected_sketch
            || !sketch_allowed(&sketch)
        {
            return unknown_result(
                "native_extrude_final_state_mismatch",
                &plan,
                Some(&observed),
                None,
            );
        }
        let final_check = (|| {
            final_observation("solid", &observed, safe(|| solid_snapshot(backend, handle)))?;
            final_observation(
                "profile",
                &profile,
                safe(|| profile_snapshot(backend, handle)),
            )?;
            final_observation("sketch", &sketch, safe(|| sketch_snapshot(backend, handle)))?;
            final_observation("root", &root, safe(|| root_snapshot(backend)))
        })();
        if let Err(diagnostic) = final_check {
            let mut result = unknown_result(
                "native_extrude_final_projection_changed",
                &plan,
                Some(&observed),
                None,
            );
            result["projectionDiagnostic"] = diagnostic;
            return result;
        }
        self.faulted = false;
        json!({"success":true,"nativeExecutionStarted":true,"legacyParity":false,"rollbackAttempted":false,"data":{"documentId":plan.document_id,"sketchId":plan.sketch_id,"profileId":plan.profile_id,"featureId":format!("f:{}:0000000000000001",plan.sketch_id),"bodyId":format!("b:{}:0000000000000001",plan.sketch_id),"observed":observed,"volumeMm3":observed.volume_cm3*1000.,"expectedPrismaticVolumeMm3":expected_volume*1000.,"distanceMm":plan.distance_cm*10.,"direction":if plan.negative{"negative"}else{"positive"},"identityScope":"Exact retained feature and sole body; IDs are current-session receipts, not general topology references.","complete":false},"rootTarget":{"documentId":plan.document_id,"revision":digest("sketch-root-projection-v1",&root).unwrap()}})
    }
}
#[cfg(test)]
pub(crate) mod tests;

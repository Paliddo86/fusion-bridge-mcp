//! Root-component physical-property projection. No SDK pointers or mutations.
//! Native typed acquisition remains owner-thread-only; this policy owns units and revisions.
use fusion_mechanical_core::PreparedRequest;
use serde::Serialize;
use serde_json::{json, Value};

pub const CONTRACT: &str = "native-root-physical-v1";
pub const SCOPE: &str = "root-physical-projection-v1: exact current document, root revision and two equal high-accuracy mass/volume/area/center-of-mass observations; local root solid bodies only, no occurrence contexts, mesh bodies or arbitrary entity references";
pub const MAX_BODIES: u64 = 128;
pub const MAX_FACES: u64 = 20_000;
pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub revision_before: String,
    pub revision_after: String,
    pub design_intent: u32,
    pub configured: bool,
    pub root_bodies: u64,
    pub root_occurrences: u64,
    pub mesh_bodies: u64,
    pub face_count: u64,
    pub non_solid_bodies: u64,
    pub accuracy: u32,
    pub mass_kg: f64,
    pub volume_cm3: f64,
    pub area_cm2: f64,
    pub center_cm: [f64; 3],
}
pub trait Backend {
    /// SDK adapter must verify exact root membership and enforce collection
    /// bounds before invoking the expensive physical-property calculation.
    fn snapshot(&self) -> Result<Snapshot>;
}
pub fn supported_arguments(request: &PreparedRequest, epoch: &str) -> Result<()> {
    if request.operation() != "assembly_properties" {
        return Ok(());
    }
    let args = request.arguments();
    if args["sessionId"].as_str() != Some(epoch) {
        return Err("native_physical_current_session_required".into());
    }
    // An explicit null is not an omission; do not silently broaden a selector.
    if args.get("entity").is_some() {
        return Err("native_physical_root_only".into());
    }
    let target = args["target"]
        .as_object()
        .ok_or("native_physical_exact_target_required")?;
    if target
        .keys()
        .any(|key| !matches!(key.as_str(), "documentId" | "revision"))
        || target
            .get("documentId")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err("native_physical_exact_target_required".into());
    }
    if target.get("revision").is_some_and(|v| {
        v.as_str()
            .is_none_or(|s| s.is_empty() || s.len() > 256 || s.contains('\0'))
    }) {
        return Err("native_physical_revision_required".into());
    }
    Ok(())
}
fn validate(snapshot: &Snapshot) -> Result<()> {
    let s = snapshot;
    if s.revision_before.is_empty()
        || s.revision_before.len() > 4096
        || s.revision_before.contains('\0')
        || s.revision_before != s.revision_after
    {
        return Err("native_physical_root_revision_changed_or_invalid".into());
    }
    if s.design_intent != 2
        || s.configured
        || s.root_occurrences != 0
        || s.mesh_bodies != 0
        || s.non_solid_bodies != 0
        || s.root_bodies > MAX_BODIES
        || s.face_count > MAX_FACES
        || (s.root_bodies == 0) != (s.face_count == 0)
        || s.face_count < s.root_bodies
    {
        return Err("native_physical_root_scope_unsupported".into());
    }
    // Autodesk HighCalculationAccuracy is enum value 2, not a decimal tolerance.
    if s.accuracy != 2 {
        return Err("native_physical_high_accuracy_required".into());
    }
    if [s.mass_kg, s.volume_cm3, s.area_cm2]
        .into_iter()
        .any(|v| !v.is_finite() || v < 0.0)
        || s.center_cm.iter().any(|v| !v.is_finite())
        || (s.root_bodies == 0 && (s.mass_kg != 0.0 || s.volume_cm3 != 0.0 || s.area_cm2 != 0.0))
        || (s.root_bodies > 0 && (s.volume_cm3 <= 0.0 || s.area_cm2 <= 0.0))
    {
        return Err("native_physical_invalid_numeric_result".into());
    }
    Ok(())
}
pub fn inspect(
    backend: &impl Backend,
    epoch: &str,
    document_id: &str,
    request: &PreparedRequest,
) -> Result<Value> {
    supported_arguments(request, epoch)?;
    if request.operation() != "assembly_properties"
        || request.arguments()["target"]["documentId"] != document_id
    {
        return Err("native_physical_exact_target_required".into());
    }
    let first = backend.snapshot()?;
    validate(&first)?;
    let second = backend.snapshot()?;
    validate(&second)?;
    if first != second {
        return Err("native_physical_projection_changed".into());
    }
    let volume = first.volume_cm3 * 1000.0;
    let area = first.area_cm2 * 100.0;
    let center = first.center_cm.map(|v| v * 10.0);
    let density = (volume > 0.0).then(|| first.mass_kg / volume);
    if !volume.is_finite()
        || !area.is_finite()
        || center.iter().any(|v| !v.is_finite())
        || density.is_some_and(|v| !v.is_finite())
        || (first.volume_cm3 > 0.0 && volume <= 0.0)
    {
        return Err("native_physical_unit_conversion_overflow".into());
    }
    let bytes =
        serde_json::to_vec(&json!({"epoch":epoch,"documentId":document_id,"snapshot":first}))
            .map_err(|_| "native_physical_projection_encoding")?;
    let revision = format!(
        "root-physical-projection-v1:{}",
        fusion_mechanical_journal::request_digest(&bytes)
    );
    if request.arguments()["target"]["revision"]
        .as_str()
        .is_some_and(|value| value != revision)
    {
        return Err("target_revision_mismatch".into());
    }
    Ok(json!({
        "target":{"documentId":document_id,"revision":revision},
        "revisionScope":SCOPE,"entitySelection":"document_root_component",
        "mass":first.mass_kg,"volume":volume,"area":area,"centerOfMass":center,
        "averageDensity":density,"units":{"mass":"kg","volume":"mm3","area":"mm2","centerOfMass":"mm","averageDensity":"kg/mm3"},
        "scope":"root_component_solids","space":"component","bodyCount":first.root_bodies,
        "calculationAccuracy":"high","sdkAccuracyCode":first.accuracy,
        "accuracyScope":"Autodesk HighCalculationAccuracy; repeat equality is consistency, not exact analytic truth",
        "materialBasis":"Fusion assigned/default material; supplier provenance does not certify modeled material density",
        "arbitraryEntitySelectionSupported":false,"assemblyOccurrencesSupported":false,"legacyParity":false
    }))
}

#[cfg(test)]
mod tests;

//! Exact returned-geometry IDs; no SDK adoption, implicit geometry or solver graph.
use super::{
    geometry::{GeometryBackend, ObservedEntity, MAX_ENTITIES, MAX_POINTS},
    *,
};
pub const CONTRACT: &str = "native-sketch-entities-v1";
pub const SCOPE: &str = "sketch-entities-projection-v1: exact explicitly returned geometry with parent sketch summary, cm/radian coordinates and construction/reference flags. No implicit endpoints/origin, fixed/linked flags, relationship graph, NURBS shape or complete inventory.";
pub const MAX_BYTES: usize = 131072;
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Publication {
    /// Private canonical creation-order ordinals, never SDK collection positions.
    pub(super) aliases: Vec<usize>,
}
#[derive(Debug, PartialEq)]
struct Capture {
    sketch: SketchState,
    values: Vec<ObservedEntity>,
    publication: Publication,
}
pub fn supported_arguments(request: &PreparedRequest) -> Result<(), &'static str> {
    if request.operation() != "native_sketch_entities" {
        return Ok(());
    }
    let a = request.arguments();
    let target = a["target"]
        .as_object()
        .ok_or("native_entities_exact_target_required")?;
    if target
        .keys()
        .any(|k| !matches!(k.as_str(), "documentId" | "revision"))
        || target
            .get("documentId")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || a["sketchId"].as_str().is_none_or(str::is_empty)
    {
        return Err("native_entities_exact_target_required");
    }
    if target.get("revision").is_some_and(|v| {
        !v.as_str()
            .is_some_and(|r| r.starts_with("sketch-entities-projection-v1:"))
    }) {
        return Err("native_entities_revision_required");
    }
    Ok(())
}
fn capture<B: GeometryBackend>(
    backend: &B,
    handle: &B::Handle,
    leases: &[u64],
) -> Result<Capture, String> {
    if leases.is_empty() || leases.len() > MAX_ENTITIES {
        return Err("native_entities_bound".into());
    }
    let sketch = sketch_snapshot(backend, handle)?;
    let mut values = Vec::with_capacity(leases.len());
    let mut aliases = Vec::with_capacity(leases.len());
    let mut points = 0usize;
    let equality = backend.entity_equalities(handle, leases)?;
    if equality.len() != leases.len() || equality.iter().any(|r| r.len() != leases.len()) {
        return Err("native_entities_identity_invalid".into());
    }
    for (i, lease) in leases.iter().enumerate() {
        // entity() verifies validity, exact parent and current membership.
        let value = backend.entity(handle, *lease)?;
        let count = match &value.geometry {
            geometry_preparation::Geometry::Point { .. }
            | geometry_preparation::Geometry::Circle { .. } => 1,
            geometry_preparation::Geometry::Line { .. } => 2,
            geometry_preparation::Geometry::Arc { .. } => 3,
            geometry_preparation::Geometry::FittedSpline { fit_points_cm, .. } => {
                if !(2..=128).contains(&fit_points_cm.len()) {
                    return Err("native_entities_bound".into());
                }
                fit_points_cm.len()
            }
        };
        points = points.checked_add(count).ok_or("native_entities_bound")?;
        if points > MAX_POINTS
            || !geometry::entity_supported(&geometry_preparation::Entity {
                geometry: value.geometry.clone(),
                construction: value.construction,
            })
        {
            return Err("native_entities_geometry_invalid".into());
        }
        if matches!(value.geometry, geometry_preparation::Geometry::Point { .. })
            != value.construction.is_none()
        {
            return Err("native_entities_flags_invalid".into());
        }
        // Exact bounded equivalence relation; equal coordinates are insufficient.
        let mut canonical = i;
        if !equality[i][i] {
            return Err("native_entities_identity_invalid".into());
        }
        for j in 0..i {
            let forward = equality[j][i];
            let reverse = equality[i][j];
            if forward != reverse {
                return Err("native_entities_identity_invalid".into());
            }
            if forward {
                if values[j] != value {
                    return Err("native_entities_alias_observation_mismatch".into());
                }
                canonical = canonical.min(aliases[j]);
            }
        }
        for j in 0..i {
            if equality[j][i] != (aliases[j] == canonical) {
                return Err("native_entities_identity_invalid".into());
            }
        }
        aliases.push(canonical);
        values.push(value);
    }
    if sketch_snapshot(backend, handle)? != sketch {
        return Err("native_entities_projection_changed".into());
    }
    Ok(Capture {
        sketch,
        values,
        publication: Publication { aliases },
    })
}
fn object_type(value: &ObservedEntity) -> &'static str {
    match &value.geometry {
        geometry_preparation::Geometry::Point { .. } => "SketchPoint",
        geometry_preparation::Geometry::Line { .. } => "SketchLine",
        geometry_preparation::Geometry::Circle { .. } => "SketchCircle",
        geometry_preparation::Geometry::Arc { .. } => "SketchArc",
        geometry_preparation::Geometry::FittedSpline { .. } => "SketchFittedSpline",
    }
}
impl<H> Book<H> {
    pub fn inspect_entities<B: GeometryBackend<Handle = H>>(
        &self,
        backend: &B,
        document_id: &str,
        sketch_id: &str,
        expected_revision: Option<&str>,
    ) -> Result<Value, String> {
        self.ready()?;
        let entry = self
            .entries
            .iter()
            .find(|e| e.id == sketch_id && e.document_id == document_id)
            .ok_or("native_sketch_id_unknown")?;
        if entry.retired.get() || entry.entities_retired.get() {
            return Err("native_entities_id_retired".into());
        }
        let handle = entry
            .handle
            .as_ref()
            .ok_or("native_sketch_creation_unknown")?;
        let leases = entry
            .geometry_leases
            .as_ref()
            .ok_or("native_entities_confirmed_geometry_required")?;
        let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let a = capture(backend, handle, leases)?;
            let b = capture(backend, handle, leases)?;
            if a != b {
                return Err("native_entities_projection_changed".to_string());
            }
            if entry
                .entities
                .borrow()
                .as_ref()
                .is_some_and(|p| *p != a.publication)
            {
                return Err("native_entities_identity_changed".into());
            }
            Ok(a)
        }))
        .unwrap_or_else(|_| Err("native_entities_capture_panicked".into()))
        .map_err(|error| {
            entry.entities_retired.set(true);
            error
        })?;
        let rows: Vec<Value> = observed.values.iter().enumerate().filter(|(i,_)| observed.publication.aliases[*i] == *i).map(|(i, value)| json!({"entityId":format!("e:{sketch_id}:{:016x}", i+1),"objectType":object_type(value),"observed":value})).collect();
        let projection = json!({"documentId":document_id,"sketchId":sketch_id,"sketch":observed.sketch,"entities":rows});
        let bytes =
            serde_json::to_vec(&projection).map_err(|_| "native_entities_serialization_failed")?;
        if bytes.len() > MAX_BYTES {
            return Err("native_entities_projection_bound".into());
        }
        let revision = format!(
            "sketch-entities-projection-v1:{}",
            fusion_mechanical_journal::request_digest(&bytes)
        );
        if expected_revision.is_some_and(|v| v != revision) {
            return Err("target_revision_mismatch".into());
        }
        // Stale requests do not publish IDs or retire valid existing IDs.
        *entry.entities.borrow_mut() = Some(observed.publication);
        let entities: Vec<Value> = rows.into_iter().map(|mut row| { row["ref"] = json!({"documentId":document_id,"sketchId":sketch_id,"entityId":row["entityId"],"objectType":row["objectType"],"revision":revision}); row }).collect();
        Ok(
            json!({"target":{"documentId":document_id,"revision":revision},"sketchId":sketch_id,"revisionScope":SCOPE,"sketch":observed.sketch,"entities":entities,"returnedLeaseCount":leases.len(),"uniqueEntityCount":entities.len(),"complete":false,"implicitGeometryEnumerated":false,"constraintsEnumerated":false,"units":"centimeters/radians","sketchTarget":sketch_data(document_id,sketch_id,&observed.sketch)?["target"]}),
        )
    }
}
#[cfg(test)]
mod tests;

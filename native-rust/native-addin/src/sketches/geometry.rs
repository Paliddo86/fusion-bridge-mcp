//! Bounded, non-atomic insertion into an exact retained empty origin-plane sketch.
//! Preparation is pure. The coordinator must durably start before execute_geometry.
use super::*;
use geometry_preparation::{Entity, Geometry, Limits, PreparedPlan, Request};
pub const MAX_ITEMS: usize = 32;
pub const MAX_ENTITIES: usize = 64;
pub const MAX_POINTS: usize = 512;
pub const MAX_PLAN_BYTES: usize = 65536;
pub const CONTRACT: &str = "native-sketch-geometry-v1";
pub const READBACK_SCOPE: &str = "Exact returned entity references, sketch-space centimeters/radians, construction/periodic closure and final sketch counts/profiles/health. No constraints, complete geometry inventory, NURBS shape or atomicity guarantee.";
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedEntity {
    pub geometry: Geometry,
    pub construction: Option<bool>,
    pub reference: bool,
}
/// An opaque backend-owned lease index, never a collection index or user identity.
/// Every possibly returned entity must be retained beneath the sketch even on error.
pub struct InsertAttempt {
    pub lease: Option<u64>,
    pub invoked: bool,
    pub acknowledged: bool,
    pub stage: &'static str,
}
pub trait GeometryBackend: Backend {
    /// Exact origin-only point membership, no text and no prior geometry leases.
    fn empty_origin_only(&self, sketch: &Self::Handle) -> Result<bool, String>;
    fn insert(&self, sketch: &Self::Handle, entity: &Entity) -> InsertAttempt;
    fn entity(&self, sketch: &Self::Handle, lease: u64) -> Result<ObservedEntity, String>;
    /// Exact SDK object equality between two retained leases, never coordinates.
    fn entity_equalities(
        &self,
        sketch: &Self::Handle,
        leases: &[u64],
    ) -> Result<Vec<Vec<bool>>, String>;
}
#[derive(Debug)]
pub struct Plan {
    document_id: String,
    sketch_id: String,
    revision: String,
    root: RootState,
    before: SketchState,
    prepared: PreparedPlan,
}
impl Plan {
    pub fn document_id(&self) -> &str {
        &self.document_id
    }
    pub fn observed_document(&self) -> Value {
        json!({"documentId":self.document_id,"revision":self.revision,"sketchId":self.sketch_id,"revisionScope":SKETCH_SCOPE})
    }
}
fn point_supported(p: &[f64; 3]) -> bool {
    p.iter().all(|v| v.is_finite() && v.abs() <= 10000.) && p[2] == 0.
}
fn span(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt()
}
pub(super) fn entity_supported(e: &Entity) -> bool {
    match &e.geometry {
        Geometry::Point { position_cm } => point_supported(position_cm),
        Geometry::Line { start_cm, end_cm } => {
            point_supported(start_cm) && point_supported(end_cm) && span(start_cm, end_cm) >= 1e-5
        }
        Geometry::Circle {
            center_cm,
            radius_cm,
        } => point_supported(center_cm) && (1e-5..=10000.).contains(radius_cm),
        Geometry::Arc {
            center_cm,
            start_cm,
            end_cm,
            sweep_radians,
        } => {
            [center_cm, start_cm, end_cm]
                .into_iter()
                .all(point_supported)
                && span(center_cm, start_cm) >= 1e-5
                && sweep_radians.abs() > 1e-8
                && sweep_radians.abs() < std::f64::consts::TAU - 1e-8
        }
        Geometry::FittedSpline {
            fit_points_cm,
            closed,
        } => {
            fit_points_cm.iter().all(point_supported)
                && fit_points_cm.windows(2).all(|p| span(&p[0], &p[1]) >= 1e-5)
                && (!closed || span(&fit_points_cm[0], fit_points_cm.last().unwrap()) >= 1e-5)
        }
    }
}
/// Called before journal admission; no SDK calls or retained-reference acquisition.
pub fn prepare_arguments(request: &PreparedRequest) -> Result<PreparedPlan, &'static str> {
    if request.operation() != "sketch_geometry" {
        return Err("native_geometry_operation_required");
    }
    let a = request.arguments();
    let target = a["target"]
        .as_object()
        .ok_or("native_geometry_exact_target_required")?;
    let sketch = a["sketch"]
        .as_object()
        .ok_or("native_geometry_exact_sketch_required")?;
    let doc = target
        .get("documentId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("native_geometry_exact_target_required")?;
    let revision = target
        .get("revision")
        .and_then(Value::as_str)
        .filter(|s| s.starts_with("sketch-projection-v1:"))
        .ok_or("native_geometry_sketch_revision_required")?;
    if target
        .keys()
        .any(|k| !matches!(k.as_str(), "documentId" | "revision"))
        || sketch.keys().any(|k| {
            !matches!(
                k.as_str(),
                "documentId" | "revision" | "objectType" | "sketchId"
            )
        })
        || sketch.get("documentId").and_then(Value::as_str) != Some(doc)
        || sketch.get("revision").and_then(Value::as_str) != Some(revision)
        || sketch.get("objectType").and_then(Value::as_str) != Some("Sketch")
        || sketch
            .get("sketchId")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err("native_geometry_exact_sketch_required");
    }
    let parsed: Request = serde_json::from_value(json!({"items":a["items"]}))
        .map_err(|_| "native_geometry_plan_invalid")?;
    if parsed.items.is_empty() || parsed.items.len() > MAX_ITEMS {
        return Err("native_geometry_plan_bound");
    }
    let prepared = geometry_preparation::prepare_with_limits(
        &parsed,
        Limits {
            max_entities: MAX_ENTITIES,
            max_stored_points: MAX_POINTS,
            max_output_bytes: MAX_PLAN_BYTES,
        },
    )
    .map_err(|_| "native_geometry_plan_invalid")?;
    if !prepared
        .items()
        .iter()
        .flat_map(|i| &i.entities)
        .all(entity_supported)
    {
        return Err("native_geometry_plan_scope_refused");
    }
    Ok(prepared)
}
fn root_allowed(r: &RootState) -> bool {
    r.active && r.parametric && r.hybrid && !r.configured && r.timeline_position == r.timeline_count
}
fn empty(s: &SketchState) -> bool {
    matches!(s.plane, 1..=3)
        && s.curves == 0
        && s.profiles == 0
        && s.dimensions == 0
        && s.constraints == 0
        && s.points <= 1
        && !s.deferred
        && s.health == 0
}
fn near(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-7 + 1e-12 * a.abs().max(b.abs())
}
fn point_near(a: &[f64; 3], b: &[f64; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| near(*a, *b))
}
/// SDK arcs are counterclockwise even when requested with a negative sweep.
fn matches_geometry(expected: &Geometry, actual: &Geometry) -> bool {
    match (expected, actual) {
        (Geometry::Point { position_cm: a }, Geometry::Point { position_cm: b }) => {
            point_near(a, b)
        }
        (
            Geometry::Line {
                start_cm: a,
                end_cm: b,
            },
            Geometry::Line {
                start_cm: c,
                end_cm: d,
            },
        ) => point_near(a, c) && point_near(b, d),
        (
            Geometry::Circle {
                center_cm: a,
                radius_cm: b,
            },
            Geometry::Circle {
                center_cm: c,
                radius_cm: d,
            },
        ) => point_near(a, c) && near(*b, *d),
        (
            Geometry::Arc {
                center_cm: a,
                start_cm: b,
                end_cm: c,
                sweep_radians: d,
            },
            Geometry::Arc {
                center_cm: e,
                start_cm: f,
                end_cm: g,
                sweep_radians: h,
            },
        ) => {
            point_near(a, e)
                && near(d.abs(), *h)
                && if *d > 0. {
                    point_near(b, f) && point_near(c, g)
                } else {
                    point_near(b, g) && point_near(c, f)
                }
        }
        (
            Geometry::FittedSpline {
                fit_points_cm: a,
                closed: b,
            },
            Geometry::FittedSpline {
                fit_points_cm: c,
                closed: d,
            },
        ) => b == d && a.len() == c.len() && a.iter().zip(c).all(|(a, b)| point_near(a, b)),
        _ => false,
    }
}
fn observed_call<T>(call: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(call))
        .map_err(|_| "native_geometry_getter_panicked".to_owned())?
}
fn observe<B: GeometryBackend>(
    backend: &B,
    handle: &B::Handle,
    lease: u64,
    expected: &Entity,
) -> Result<ObservedEntity, String> {
    let a = observed_call(|| backend.entity(handle, lease))?;
    let b = observed_call(|| backend.entity(handle, lease))?;
    if a != b
        || a.reference
        || !matches_geometry(&expected.geometry, &a.geometry)
        || match &expected.geometry {
            Geometry::Point { .. } => a.construction.is_some(),
            _ => a.construction != Some(expected.construction.unwrap_or(false)),
        }
    {
        return Err("native_geometry_readback_mismatch".into());
    }
    Ok(a)
}
fn uncertain(code: &str, id: &str, progress: &[Value], failed: Option<Value>) -> Value {
    json!({"success":false,"errorCode":code,"outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":true,"sketchId":id,"completedEntities":progress,"prefixObservationScope":"Each entry was read back before a later attempt; final solver state is unconfirmed.","failedAttempt":failed,"atomic":false,"rollbackAttempted":false,"automaticReplayAllowed":false})
}
impl<H> Book<H> {
    pub fn geometry_preflight<B: GeometryBackend<Handle = H>>(
        &self,
        backend: &B,
        request: &PreparedRequest,
    ) -> Result<Plan, String> {
        self.ready()?;
        let prepared = prepare_arguments(request).map_err(str::to_string)?;
        let a = request.arguments();
        let document_id = a["target"]["documentId"].as_str().unwrap();
        let sketch_id = a["sketch"]["sketchId"].as_str().unwrap();
        let entry = self
            .entries
            .iter()
            .find(|e| e.id == sketch_id && e.document_id == document_id && !e.retired.get())
            .ok_or("native_sketch_id_unknown")?;
        let handle = entry
            .handle
            .as_ref()
            .ok_or("native_sketch_creation_unknown")?;
        let root = root_snapshot(backend)?;
        let before = sketch_snapshot(backend, handle).map_err(|error| {
            entry.retired.set(true);
            error
        })?;
        let revision = digest("sketch-projection-v1", &(sketch_id, document_id, &before))?;
        if a["target"]["revision"] != revision {
            return Err("target_revision_mismatch".into());
        }
        if !root_allowed(&root)
            || !empty(&before)
            || !backend.empty_origin_only(handle)?
            || backend.command()? != "SelectCommand"
        {
            return Err("native_geometry_empty_origin_sketch_required".into());
        }
        Ok(Plan {
            document_id: document_id.into(),
            sketch_id: sketch_id.into(),
            revision,
            root,
            before,
            prepared,
        })
    }
    pub fn execute_geometry<B: GeometryBackend<Handle = H>>(
        &mut self,
        backend: &B,
        plan: Plan,
    ) -> Value {
        if let Err(code) = self.ready() {
            return err(&code);
        }
        let Some(entry) = self.entries.iter().find(|e| {
            e.id == plan.sketch_id && e.document_id == plan.document_id && !e.retired.get()
        }) else {
            return err("native_sketch_id_unknown");
        };
        let Some(handle) = entry.handle.as_ref() else {
            return err("native_sketch_creation_unknown");
        };
        if root_snapshot(backend).ok().as_ref() != Some(&plan.root)
            || sketch_snapshot(backend, handle).ok().as_ref() != Some(&plan.before)
        {
            return err("target_revision_mismatch");
        }
        if backend.empty_origin_only(handle) != Ok(true)
            || backend.command().ok().as_deref() != Some("SelectCommand")
        {
            return err("native_geometry_empty_origin_sketch_required");
        }
        let mut retained = Vec::with_capacity(plan.prepared.entity_count());
        let mut progress = Vec::with_capacity(plan.prepared.entity_count());
        let mut publication_leases = Vec::with_capacity(plan.prepared.entity_count());
        self.faulted = true; // a panic after the first possible effect cannot reopen this book
        for item in plan.prepared.items() {
            for (ordinal, entity) in item.entities.iter().enumerate() {
                if observed_call(|| backend.command()).ok().as_deref() != Some("SelectCommand") {
                    if progress.is_empty() {
                        self.faulted = false;
                        return err("native_geometry_command_changed");
                    }
                    return uncertain(
                        "native_geometry_command_changed",
                        &plan.sketch_id,
                        &progress,
                        None,
                    );
                }
                let attempt = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    backend.insert(handle, entity)
                })) {
                    Ok(attempt) => attempt,
                    Err(_) => {
                        return uncertain(
                            "native_geometry_insert_panicked",
                            &plan.sketch_id,
                            &progress,
                            Some(
                                json!({"inputIndex":item.input_index,"entityIndex":ordinal,"primitive":item.primitive,"invoked":null,"acknowledged":false,"stage":"panic"}),
                            ),
                        )
                    }
                };
                let failed = json!({"inputIndex":item.input_index,"entityIndex":ordinal,"primitive":item.primitive,"invoked":attempt.invoked,"acknowledged":attempt.acknowledged,"stage":attempt.stage});
                if !attempt.invoked
                    && !attempt.acknowledged
                    && attempt.lease.is_none()
                    && progress.is_empty()
                {
                    self.faulted = false;
                    return err("native_geometry_not_invoked");
                }
                if !attempt.invoked || !attempt.acknowledged || attempt.lease.is_none() {
                    return uncertain(
                        "native_geometry_insert_unconfirmed",
                        &plan.sketch_id,
                        &progress,
                        Some(failed),
                    );
                }
                let lease = attempt.lease.unwrap();
                if retained.iter().any(|(previous, _, _)| *previous == lease) {
                    return uncertain(
                        "native_geometry_lease_reused",
                        &plan.sketch_id,
                        &progress,
                        Some(failed),
                    );
                }
                let observed = match observe(backend, handle, lease, entity) {
                    Ok(v) => v,
                    Err(_) => {
                        return uncertain(
                            "native_geometry_readback_unconfirmed",
                            &plan.sketch_id,
                            &progress,
                            Some(failed),
                        )
                    }
                };
                progress.push(json!({"inputIndex":item.input_index,"entityIndex":ordinal,"primitive":item.primitive,"observed":observed,"acknowledged":true}));
                retained.push((lease, entity, observed));
                publication_leases.push(lease);
            }
        }
        // Later solver activity can change earlier geometry. Re-read every exact
        // lease after the entire plan, then require the sketch/root to stay fixed.
        let final_state = match observed_call(|| sketch_snapshot(backend, handle)) {
            Ok(v) => v,
            Err(_) => {
                return uncertain(
                    "native_geometry_final_sketch_unconfirmed",
                    &plan.sketch_id,
                    &progress,
                    None,
                )
            }
        };
        let root = match observed_call(|| root_snapshot(backend)) {
            Ok(v) => v,
            Err(_) => {
                return uncertain(
                    "native_geometry_final_root_unconfirmed",
                    &plan.sketch_id,
                    &progress,
                    None,
                )
            }
        };
        let curves = retained
            .iter()
            .filter(|(_, e, _)| !matches!(e.geometry, Geometry::Point { .. }))
            .count() as u64;
        if !root_allowed(&root)
            || root.sketch_count != plan.root.sketch_count
            || root.timeline_count != plan.root.timeline_count
            || final_state.curves != curves
            || final_state.plane != plan.before.plane
            || final_state.transform != plan.before.transform
            || final_state.name != plan.before.name
            || final_state.deferred
            || final_state.health != 0
            || final_state.dimensions != 0
            || final_state.sdk_revision == plan.before.sdk_revision
        {
            return uncertain(
                "native_geometry_final_state_mismatch",
                &plan.sketch_id,
                &progress,
                None,
            );
        }
        for (lease, expected, old) in &retained {
            if observe(backend, handle, *lease, expected).ok().as_ref() != Some(old) {
                return uncertain(
                    "native_geometry_final_entities_changed",
                    &plan.sketch_id,
                    &progress,
                    None,
                );
            }
        }
        if observed_call(|| sketch_snapshot(backend, handle))
            .ok()
            .as_ref()
            != Some(&final_state)
            || observed_call(|| root_snapshot(backend)).ok().as_ref() != Some(&root)
        {
            return uncertain(
                "native_geometry_final_projection_changed",
                &plan.sketch_id,
                &progress,
                None,
            );
        }
        let data = match sketch_data(&plan.document_id, &plan.sketch_id, &final_state) {
            Ok(v) => v,
            Err(_) => {
                return uncertain(
                    "native_geometry_final_projection_failed",
                    &plan.sketch_id,
                    &progress,
                    None,
                )
            }
        };
        // Retain only lease indices after every original success check. The SDK
        // references remain owned by the existing sketch holder. Publication is a
        // separate read operation and cannot change this established result.
        self.entries
            .iter_mut()
            .find(|e| e.id == plan.sketch_id)
            .unwrap()
            .geometry_leases = Some(publication_leases);
        self.faulted = false;
        json!({"success":true,"nativeExecutionStarted":true,"legacyParity":false,"atomic":false,"rollbackAttempted":false,"data":data,"rootTarget":{"documentId":plan.document_id,"revision":digest("sketch-root-projection-v1",&root).unwrap()},"geometry":{"itemsCompleted":plan.prepared.items().len(),"entitiesCompleted":progress.len(),"observed":progress,"readbackScope":READBACK_SCOPE,"units":"centimeters/radians","complete":false}})
    }
}
#[cfg(test)]
pub(crate) mod tests;

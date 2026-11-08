//! Read prerequisite for finite horizontal/vertical authoring. No mutation API.
use super::{geometry::GeometryBackend, *};
pub const CONTRACT: &str = "native-sketch-line-state-v1";
pub const SCOPE:&str="sketch-line-state-projection-v1: one exactly published retained root-origin SketchLine, exact retained start/end/origin points, complete bounded zero-constraint/zero-dimension membership, flags/connectivity and current root/sketch revisions; no arbitrary solver graph or mutation";
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Flags {
    pub fixed: bool,
    pub reference: bool,
    pub has_referenced_entity: bool,
    pub fully_constrained: bool,
    pub is_2d: bool,
    pub constraints: u64,
    pub dimensions: u64,
    pub attributes: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    pub position_cm: [f64; 3],
    pub flags: Flags,
    pub connected_entities: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub line: Flags,
    pub construction: bool,
    pub start: Point,
    pub end: Point,
    pub origin: Point,
    pub root_bodies: u64,
    pub root_features: u64,
    pub root_occurrences: u64,
    pub root_meshes: u64,
    pub design_components: u64,
    pub sketch_texts: u64,
    pub sketch_attributes: u64,
}
pub trait LineBackend: GeometryBackend {
    /// Complete exact membership and endpoint identity are checked and retained by
    /// the backend beneath the sketch. Repeated reads may not replace endpoints.
    fn line_state(&self, handle: &Self::Handle, lease: u64) -> Result<State, String>;
}
pub fn known_refusal(code: &str) -> bool {
    matches!(
        code,
        "native_sketch_id_unknown"
            | "native_entities_id_retired"
            | "native_line_confirmed_geometry_required"
            | "native_line_single_returned_entity_required"
            | "native_line_published_entity_required"
            | "native_line_entity_selector_refused"
            | "target_revision_mismatch"
            | "native_line_isolated_root_required"
            | "native_line_empty_solver_graph_required"
            | "native_line_foreign_relationship_refused"
            | "native_line_geometry_bound"
            | "native_line_nonorigin_distinct_endpoints_required"
    )
}
pub fn supported_arguments(r: &PreparedRequest) -> Result<(), &'static str> {
    if r.operation() != "native_sketch_line_state" {
        return Ok(());
    }
    let a = r.arguments();
    let t = a["target"]
        .as_object()
        .ok_or("native_line_exact_target_required")?;
    if t.keys()
        .any(|k| !matches!(k.as_str(), "documentId" | "revision"))
        || t.get("documentId")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || a["sketchId"].as_str().is_none_or(str::is_empty)
        || a["entityId"].as_str().is_none_or(str::is_empty)
    {
        return Err("native_line_exact_target_required");
    }
    if t.get("revision").is_some_and(|v| {
        !v.as_str().is_some_and(|v| {
            v.strip_prefix("sketch-line-state-projection-v1:")
                .is_some_and(|s| {
                    s.len() == 64
                        && s.bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
        })
    }) {
        return Err("native_line_revision_required");
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct Capture {
    pub(super) root: RootState,
    pub(super) sketch: SketchState,
    pub(super) line: State,
}
pub(super) fn validate(c: &Capture) -> Result<(), String> {
    let r = &c.root;
    let s = &c.sketch;
    let l = &c.line;
    if !r.active
        || !r.parametric
        || !r.hybrid
        || r.configured
        || r.sketch_count != 1
        || r.timeline_count != 1
        || r.timeline_position != 1
        || l.root_bodies != 0
        || l.root_features != 0
        || l.root_occurrences != 0
        || l.root_meshes != 0
        || l.design_components != 1
    {
        return Err("native_line_isolated_root_required".into());
    }
    if s.curves != 1
        || s.points != 3
        || s.profiles != 0
        || s.constraints != 0
        || s.dimensions != 0
        || s.health != 0
        || s.deferred
        || !(1..=3).contains(&s.plane)
        || l.sketch_texts != 0
        || l.sketch_attributes != 0
        || l.construction
    {
        return Err("native_line_empty_solver_graph_required".into());
    }
    for f in [&l.line, &l.start.flags, &l.end.flags, &l.origin.flags] {
        if !f.is_2d || f.constraints != 0 || f.dimensions != 0 || f.attributes != 0 {
            return Err("native_line_empty_solver_graph_required".into());
        }
    }
    if [&l.line, &l.start.flags, &l.end.flags]
        .into_iter()
        .any(|f| f.reference || f.has_referenced_entity)
        || l.start.connected_entities != 1
        || l.end.connected_entities != 1
        || l.origin.connected_entities != 0
    {
        return Err("native_line_foreign_relationship_refused".into());
    }
    for p in [
        &l.start.position_cm,
        &l.end.position_cm,
        &l.origin.position_cm,
    ] {
        if !p.iter().all(|v| v.is_finite() && v.abs() <= 10000.) || p[2] != 0. {
            return Err("native_line_geometry_bound".into());
        }
    }
    if l.origin.position_cm != [0.; 3]
        || l.start.position_cm == l.origin.position_cm
        || l.end.position_cm == l.origin.position_cm
        || (l
            .start
            .position_cm
            .iter()
            .zip(l.end.position_cm)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>())
        .sqrt()
            < 1e-5
    {
        return Err("native_line_nonorigin_distinct_endpoints_required".into());
    }
    Ok(())
}
pub(super) fn capture<B: LineBackend>(b: &B, h: &B::Handle, lease: u64) -> Result<Capture, String> {
    let root = root_snapshot(b)?;
    let sketch = sketch_snapshot(b, h)?;
    let line = b.line_state(h, lease)?;
    let observed = b.entity(h, lease)?;
    if observed.geometry
        != (geometry_preparation::Geometry::Line {
            start_cm: line.start.position_cm,
            end_cm: line.end.position_cm,
        })
        || observed.reference != line.line.reference
        || observed.construction != Some(line.construction)
    {
        return Err("native_line_geometry_membership_changed".into());
    }
    let c = Capture { root, sketch, line };
    validate(&c)?;
    if sketch_snapshot(b, h)? != c.sketch || root_snapshot(b)? != c.root {
        return Err("native_line_projection_changed".into());
    }
    Ok(c)
}
impl<H> Book<H> {
    pub fn inspect_line_state<B: LineBackend<Handle = H>>(
        &self,
        b: &B,
        doc: &str,
        sketch_id: &str,
        entity_id: &str,
        revision: Option<&str>,
    ) -> Result<Value, String> {
        self.ready()?;
        let e = self
            .entries
            .iter()
            .find(|e| e.id == sketch_id && e.document_id == doc)
            .ok_or("native_sketch_id_unknown")?;
        if e.retired.get() || e.entities_retired.get() {
            return Err("native_entities_id_retired".into());
        }
        let h = e.handle.as_ref().ok_or("native_sketch_creation_unknown")?;
        let leases = e
            .geometry_leases
            .as_ref()
            .ok_or("native_line_confirmed_geometry_required")?;
        if leases.len() != 1 {
            return Err("native_line_single_returned_entity_required".into());
        }
        let published = e.entities.borrow();
        let p = published
            .as_ref()
            .ok_or("native_line_published_entity_required")?;
        if p.aliases.as_slice() != [0] || entity_id != format!("e:{sketch_id}:{:016x}", 1) {
            return Err("native_line_entity_selector_refused".into());
        }
        drop(published);
        let c = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let a = capture(b, h, leases[0])?;
            let b = capture(b, h, leases[0])?;
            if a != b {
                return Err("native_line_projection_changed".to_string());
            }
            Ok(a)
        }))
        .unwrap_or_else(|_| Err("native_line_capture_panicked".into()))
        .map_err(|e2| {
            e.entities_retired.set(true);
            e2
        })?;
        let current = digest(
            "sketch-line-state-projection-v1",
            &(doc, sketch_id, entity_id, &c),
        )?;
        if revision.is_some_and(|r| r != current) {
            return Err("target_revision_mismatch".into());
        }
        let mutable = [&c.line.line, &c.line.start.flags, &c.line.end.flags]
            .into_iter()
            .all(|f| !f.fixed && !f.fully_constrained);
        Ok(
            json!({"target":{"documentId":doc,"revision":current},"sketchId":sketch_id,"entityId":entity_id,"revisionScope":SCOPE,
   "root":c.root,"sketch":c.sketch,"lineState":c.line,"units":"centimeters","isolatedInventoryComplete":true,
   "unfixedLineAndEndpoints":mutable,"constraintMutationSupported":false,"legacyParity":false,
   "endpointIdentity":"exact_retained_sdk_objects; not_public_selectors","scope":"single_native_owned_line_empty_solver_graph"}),
        )
    }
}
#[cfg(test)]
mod tests;

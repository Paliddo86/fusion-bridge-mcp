//! Once-only horizontal/vertical constraint on a completely isolated owned line.
use super::{
    line_state::{self, LineBackend, State as LineState},
    *,
};
pub const CONTRACT: &str = "native-sketch-relations-v1";
pub const SCOPE: &str = "sketch-relations-projection-v1: one exact owned line and endpoints/origin, zero or one exact returned horizontal/vertical relationship, full bounded isolated membership; no arbitrary solver graph, dimensions or rollback";
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationState {
    pub kind: u32,
    pub line: LineState,
}
pub struct Attempt {
    pub invoked: bool,
    pub acknowledged: bool,
}
pub trait RelationBackend: LineBackend {
    fn clear_relation_diagnostic(&self) {}
    fn relation_diagnostic(&self) -> Value {
        Value::Null
    }
    /// Zero or the one exact owned returned relationship; never adopt an existing one.
    fn relation_state(&self, h: &Self::Handle, lease: u64) -> Result<RelationState, String>;
    /// Reserve a fixed returned-constraint slot and latch BEFORE one SDK call.
    fn constrain(&self, h: &Self::Handle, lease: u64, kind: u32) -> Attempt;
}
#[derive(Clone, Debug, PartialEq, Serialize)]
struct Projection {
    root: RootState,
    sketch: SketchState,
    relation: RelationState,
}
#[derive(Debug)]
pub struct Plan {
    document_id: String,
    sketch_id: String,
    entity_id: String,
    revision: String,
    before: line_state::Capture,
    kind: u32,
}
impl Plan {
    pub fn document_id(&self) -> &str {
        &self.document_id
    }
    pub fn observed_document(&self) -> Value {
        json!({"documentId":self.document_id,"sketchId":self.sketch_id,"entityId":self.entity_id,"revision":self.revision,"revisionScope":line_state::SCOPE})
    }
}
fn valid_revision(v: &str, prefix: &str) -> bool {
    v.strip_prefix(prefix).is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
pub fn supported_arguments(r: &PreparedRequest) -> Result<(), &'static str> {
    if !matches!(
        r.operation(),
        "sketch_constraint" | "native_sketch_relations"
    ) {
        return Ok(());
    }
    let a = r.arguments();
    let t = a["target"]
        .as_object()
        .ok_or("native_relation_exact_target_required")?;
    let doc = t
        .get("documentId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("native_relation_exact_target_required")?;
    if t.keys()
        .any(|k| !matches!(k.as_str(), "documentId" | "revision"))
    {
        return Err("native_relation_exact_target_required");
    }
    if r.operation() == "native_sketch_relations" {
        if a["sketchId"].as_str().is_none_or(str::is_empty)
            || a["entityId"].as_str().is_none_or(str::is_empty)
        {
            return Err("native_relation_exact_selector_required");
        }
        if t.get("revision").is_some_and(|v| {
            !v.as_str()
                .is_some_and(|s| valid_revision(s, "sketch-relations-projection-v1:"))
        }) {
            return Err("native_relation_revision_required");
        }
        return Ok(());
    }
    if !matches!(a["type"].as_str(), Some("horizontal" | "vertical")) {
        return Err("native_relation_horizontal_vertical_only");
    }
    let rev = t
        .get("revision")
        .and_then(Value::as_str)
        .filter(|s| valid_revision(s, "sketch-line-state-projection-v1:"))
        .ok_or("native_relation_line_revision_required")?;
    let sketch = a["sketch"]
        .as_object()
        .ok_or("native_relation_exact_selector_required")?;
    let entities = a["entities"]
        .as_array()
        .filter(|v| v.len() == 1)
        .ok_or("native_relation_one_line_required")?;
    let entity = entities[0]
        .as_object()
        .ok_or("native_relation_exact_selector_required")?;
    let sid = sketch
        .get("sketchId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("native_relation_exact_selector_required")?;
    for (obj, typ, allowed) in [(sketch, "Sketch", false), (entity, "SketchLine", true)] {
        if obj.keys().any(|k| {
            !matches!(
                k.as_str(),
                "documentId" | "revision" | "objectType" | "sketchId"
            ) && !(allowed && k == "entityId")
        }) || obj.get("documentId").and_then(Value::as_str) != Some(doc)
            || obj.get("revision").and_then(Value::as_str) != Some(rev)
            || obj.get("objectType").and_then(Value::as_str) != Some(typ)
            || obj.get("sketchId").and_then(Value::as_str) != Some(sid)
        {
            return Err("native_relation_exact_selector_required");
        }
    }
    if entity
        .get("entityId")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err("native_relation_exact_selector_required");
    }
    Ok(())
}
pub(super) fn select<'a, H>(
    book: &'a Book<H>,
    doc: &str,
    id: &str,
    eid: &str,
) -> Result<(&'a H, u64), String> {
    book.ready()?;
    let e = book
        .entries
        .iter()
        .find(|e| e.document_id == doc && e.id == id)
        .ok_or("native_sketch_id_unknown")?;
    if e.retired.get() || e.entities_retired.get() {
        return Err("native_entities_id_retired".into());
    }
    let h = e.handle.as_ref().ok_or("native_sketch_creation_unknown")?;
    let leases = e
        .geometry_leases
        .as_ref()
        .filter(|v| v.len() == 1)
        .ok_or("native_line_single_returned_entity_required")?;
    if e.entities
        .borrow()
        .as_ref()
        .is_none_or(|p| p.aliases.as_slice() != [0])
    {
        return Err("native_line_published_entity_required".into());
    }
    if eid != format!("e:{id}:0000000000000001") {
        return Err("native_line_entity_selector_refused".into());
    }
    Ok((h, leases[0]))
}
fn safe<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .map_err(|_| "native_relation_getter_panicked".to_owned())?
}
fn capture<B: RelationBackend>(b: &B, h: &B::Handle, lease: u64) -> Result<Projection, String> {
    let root = root_snapshot(b)?;
    let sketch = sketch_snapshot(b, h)?;
    let relation = b.relation_state(h, lease)?;
    if relation.kind > 2
        || sketch.constraints != u64::from(relation.kind != 0)
        || relation.line.line.constraints != sketch.constraints
        || relation.line.start.flags.constraints > sketch.constraints
        || relation.line.end.flags.constraints > sketch.constraints
        || relation.line.origin.flags.constraints != 0
    {
        return Err("native_relation_exact_graph_required".into());
    }
    let mut plain = line_state::Capture {
        root: root.clone(),
        sketch: sketch.clone(),
        line: relation.line.clone(),
    };
    plain.sketch.constraints = 0;
    plain.line.line.constraints = 0;
    plain.line.start.flags.constraints = 0;
    plain.line.end.flags.constraints = 0;
    line_state::validate(&plain)?;
    let e = b.entity(h, lease)?;
    if e.geometry
        != (geometry_preparation::Geometry::Line {
            start_cm: relation.line.start.position_cm,
            end_cm: relation.line.end.position_cm,
        })
        || e.construction != Some(false)
        || e.reference
    {
        return Err("native_relation_geometry_changed".into());
    }
    if sketch_snapshot(b, h)? != sketch || root_snapshot(b)? != root {
        return Err("native_relation_projection_changed".into());
    }
    Ok(Projection {
        root,
        sketch,
        relation,
    })
}
fn snapshot<B: RelationBackend>(b: &B, h: &B::Handle, lease: u64) -> Result<Projection, String> {
    let a = capture(b, h, lease)?;
    if capture(b, h, lease)? != a {
        return Err("native_relation_projection_changed".into());
    }
    Ok(a)
}
fn value(doc: &str, id: &str, eid: &str, p: &Projection) -> Result<Value, String> {
    let rev = digest("sketch-relations-projection-v1", &(doc, id, eid, p))?;
    Ok(
        json!({"target":{"documentId":doc,"revision":rev},"sketchId":id,"entityId":eid,"root":p.root,"sketch":p.sketch,"relationState":p.relation,"revisionScope":SCOPE,"units":"centimeters","completeWithinIsolatedScope":true,"legacyParity":false,"solverPlacementUniquelySpecified":false}),
    )
}
impl<H> Book<H> {
    pub fn inspect_relations<B: RelationBackend<Handle = H>>(
        &self,
        b: &B,
        doc: &str,
        id: &str,
        eid: &str,
        expected: Option<&str>,
    ) -> Result<Value, String> {
        let (h, lease) = select(self, doc, id, eid)?;
        let p = safe(|| snapshot(b, h, lease)).map_err(|e| {
            self.entries
                .iter()
                .find(|e| e.id == id && e.document_id == doc)
                .unwrap()
                .entities_retired
                .set(true);
            e
        })?;
        let v = value(doc, id, eid, &p)?;
        if expected.is_some_and(|r| v["target"]["revision"] != r) {
            return Err("target_revision_mismatch".into());
        }
        Ok(v)
    }
    pub fn relation_preflight<B: RelationBackend<Handle = H>>(
        &self,
        b: &B,
        r: &PreparedRequest,
    ) -> Result<Plan, String> {
        b.clear_relation_diagnostic();
        supported_arguments(r).map_err(str::to_string)?;
        if r.operation() != "sketch_constraint" {
            return Err("native_relation_operation_required".into());
        }
        let a = r.arguments();
        let doc = a["target"]["documentId"].as_str().unwrap();
        let id = a["sketch"]["sketchId"].as_str().unwrap();
        let eid = a["entities"][0]["entityId"].as_str().unwrap();
        let (h, lease) = select(self, doc, id, eid)?;
        let before = safe(|| {
            let a = line_state::capture(b, h, lease)?;
            if line_state::capture(b, h, lease)? != a {
                return Err("native_relation_projection_changed".into());
            }
            Ok(a)
        })?;
        let revision = digest("sketch-line-state-projection-v1", &(doc, id, eid, &before))?;
        if a["target"]["revision"] != revision {
            return Err("target_revision_mismatch".into());
        }
        let l = &before.line;
        if [&l.line, &l.start.flags, &l.end.flags]
            .iter()
            .any(|f| f.fixed || f.fully_constrained)
            || (l.start.position_cm[0] - l.end.position_cm[0]).abs() < 1e-4
            || (l.start.position_cm[1] - l.end.position_cm[1]).abs() < 1e-4
            || [l.start.position_cm, l.end.position_cm]
                .iter()
                .any(|p| p[0].abs() < 1e-4 || p[1].abs() < 1e-4)
        {
            return Err("native_relation_unfixed_sloped_line_required".into());
        }
        if b.command()? != "SelectCommand" {
            return Err("native_relation_command_refused".into());
        }
        Ok(Plan {
            document_id: doc.into(),
            sketch_id: id.into(),
            entity_id: eid.into(),
            revision,
            before,
            kind: if a["type"] == "horizontal" { 1 } else { 2 },
        })
    }
    pub fn execute_relation<B: RelationBackend<Handle = H>>(&mut self, b: &B, plan: Plan) -> Value {
        b.clear_relation_diagnostic();
        let mut out = self.execute_relation_inner(b, plan);
        if out["success"] != true {
            out["sdkDiagnostic"] = b.relation_diagnostic()
        }
        out
    }
    fn execute_relation_inner<B: RelationBackend<Handle = H>>(&mut self, b: &B, p: Plan) -> Value {
        let (h, lease) = match select(self, &p.document_id, &p.sketch_id, &p.entity_id) {
            Ok(v) => v,
            Err(e) => return err(&e),
        };
        match safe(|| {
            let a = line_state::capture(b, h, lease)?;
            if a != p.before || line_state::capture(b, h, lease)? != a {
                return Err("target_revision_mismatch".into());
            }
            if b.command()? != "SelectCommand" {
                return Err("native_relation_command_refused".into());
            }
            Ok(())
        }) {
            Ok(()) => {}
            Err(e) => return err(&e),
        }
        // Extract the handle again without a borrow of all self before setting the fence.
        let index = self
            .entries
            .iter()
            .position(|e| e.id == p.sketch_id && e.document_id == p.document_id)
            .unwrap();
        self.faulted = true;
        let h = self.entries[index].handle.as_ref().unwrap();
        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            b.constrain(h, lease, p.kind)
        }));
        let mut observed: Option<Projection> = None;
        let unknown = |code: &str, observed: &Option<Projection>| json!({"success":false,"errorCode":code,"outcome":"unknown","nativeExecutionStarted":true,"reconciliationRequired":true,"sketchId":p.sketch_id,"entityId":p.entity_id,"observedRelation":observed,"automaticReplayAllowed":false,"rollbackAttempted":false});
        let a = match attempt {
            Ok(a) => a,
            Err(_) => return unknown("native_relation_invocation_panicked", &observed),
        };
        if !a.invoked && !a.acknowledged {
            self.faulted = false;
            return err("native_relation_not_invoked");
        }
        if !a.invoked || !a.acknowledged {
            return unknown("native_relation_unconfirmed", &observed);
        }
        let result = safe(|| {
            if b.command()? != "SelectCommand" {
                return Err("native_relation_post_command_refused".into());
            }
            let root = root_snapshot(b)?;
            let first = capture(b, h, lease)?;
            observed = Some(first.clone());
            validate_after(&p, &first)?;
            if first.root != root {
                return Err("native_relation_root_changed".into());
            }
            let final_state = capture(b, h, lease)?;
            if final_state != first {
                return Err("native_relation_final_projection_changed".into());
            }
            if b.command()? != "SelectCommand" {
                return Err("native_relation_post_command_refused".into());
            }
            // Parent revision is checked last, after every geometry/relationship getter.
            if root_snapshot(b)? != root {
                return Err("native_relation_final_root_changed".into());
            }
            value(&p.document_id, &p.sketch_id, &p.entity_id, &first)
        });
        match result {
            Ok(data) => {
                self.faulted = false;
                json!({"success":true,"nativeExecutionStarted":true,"data":data,"rollbackAttempted":false,"automaticReplayAllowed":false})
            }
            Err(e) => unknown(&e, &observed),
        }
    }
}
fn validate_after(p: &Plan, a: &Projection) -> Result<(), String> {
    let mut r = a.root.clone();
    r.root_revision = p.before.root.root_revision.clone();
    let mut s = a.sketch.clone();
    s.sdk_revision = p.before.sketch.sdk_revision.clone();
    s.constraints = 0;
    let l = &a.relation.line;
    if r != p.before.root
        || s != p.before.sketch
        || a.relation.kind != p.kind
        || l.origin != p.before.line.origin
        || [&l.line, &l.start.flags, &l.end.flags]
            .iter()
            .any(|f| f.fixed || f.fully_constrained)
    {
        return Err("native_relation_postcondition_failed".into());
    }
    let axis = if p.kind == 1 { 1 } else { 0 };
    let other = 1 - axis;
    if (l.start.position_cm[axis] - l.end.position_cm[axis]).abs() > 1e-8
        || (l.start.position_cm[other] - l.end.position_cm[other]).abs() < 1e-5
    {
        return Err("native_relation_alignment_unconfirmed".into());
    }
    Ok(())
}
#[cfg(test)]
pub(crate) mod tests;

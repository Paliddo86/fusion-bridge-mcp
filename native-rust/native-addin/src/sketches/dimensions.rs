//! One driving aligned length dimension on the exact retained H/V line endpoints.
use super::{
    line_state,
    relations::{self, Attempt, RelationBackend, RelationState},
    *,
};
pub const CONTRACT: &str = "native-sketch-dimensions-v1";
pub const SCOPE:&str="sketch-dimensions-projection-v1: one owned H/V line, exact retained start/end/origin, zero or one exact returned driving aligned dimension and model parameter; numeric millimeters only, no arbitrary graph/formula/name/rollback parity";
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dimension {
    pub driving: bool,
    pub aligned: bool,
    pub value_cm: f64,
    pub parameter_value_cm: f64,
    pub text_position_cm: [f64; 3],
    pub parameter_name: String,
    pub parameter_expression: String,
    pub parameter_unit: String,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub relation: RelationState,
    pub dimension: Option<Dimension>,
    pub model_parameters: u64,
    pub user_parameters: u64,
}
pub trait DimensionBackend: RelationBackend {
    fn dimension_state(&self, h: &Self::Handle, lease: u64) -> Result<State, String>;
    fn add_dimension(&self, h: &Self::Handle, lease: u64, position_cm: [f64; 3]) -> Attempt;
    fn set_dimension_value(&self, h: &Self::Handle, lease: u64, value_cm: f64) -> Attempt;
}
#[derive(Clone, Debug, PartialEq, Serialize)]
struct Projection {
    root: RootState,
    sketch: SketchState,
    state: State,
}
#[derive(Debug)]
pub struct Plan {
    document_id: String,
    sketch_id: String,
    entity_id: String,
    before: Projection,
    revision: String,
    position_cm: [f64; 3],
    value_cm: f64,
}
impl Plan {
    pub fn document_id(&self) -> &str {
        &self.document_id
    }
    pub fn observed_document(&self) -> Value {
        json!({"documentId":self.document_id,"sketchId":self.sketch_id,"entityId":self.entity_id,"revision":self.revision,"revisionScope":SCOPE})
    }
}
fn revision(v: &Value) -> bool {
    v.as_str().is_some_and(|v| {
        v.strip_prefix("sketch-dimensions-projection-v1:")
            .is_some_and(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
    })
}
fn eid(sid: &str) -> String {
    format!("e:{sid}:0000000000000001")
}
fn point_id(sid: &str, i: usize) -> String {
    format!("p:{}:{:016x}", eid(sid), i + 1)
}
fn quantity(a: &Value) -> Result<(f64, [f64; 3]), &'static str> {
    let mm = a["expression"]
        .as_f64()
        .filter(|v| v.is_finite() && (0.001..=10000.).contains(v))
        .ok_or("native_dimension_numeric_mm_required")?;
    let p = a["position"]
        .as_array()
        .filter(|v| (2..=3).contains(&v.len()))
        .ok_or("native_dimension_position_required")?;
    let mut cm = [0.; 3];
    for (i, v) in p.iter().enumerate() {
        cm[i] = v
            .as_f64()
            .filter(|v| v.is_finite() && v.abs() <= 100000.)
            .ok_or("native_dimension_position_required")?
            / 10.;
    }
    if cm[2] != 0. {
        return Err("native_dimension_position_required");
    }
    Ok((mm / 10., cm))
}
pub fn supported_arguments(r: &PreparedRequest) -> Result<(), &'static str> {
    if !matches!(
        r.operation(),
        "sketch_dimension" | "native_sketch_dimensions"
    ) {
        return Ok(());
    }
    let a = r.arguments();
    let t = a["target"]
        .as_object()
        .ok_or("native_dimension_exact_target_required")?;
    let doc = t
        .get("documentId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("native_dimension_exact_target_required")?;
    if t.keys()
        .any(|k| !matches!(k.as_str(), "documentId" | "revision"))
    {
        return Err("native_dimension_exact_target_required");
    }
    if r.operation() == "native_sketch_dimensions" {
        if a["sketchId"].as_str().is_none_or(str::is_empty)
            || a["entityId"].as_str().is_none_or(str::is_empty)
        {
            return Err("native_dimension_exact_selector_required");
        }
        if t.get("revision").is_some_and(|v| !revision(v)) {
            return Err("native_dimension_revision_required");
        }
        return Ok(());
    }
    if a["type"] != "distance" {
        return Err("native_dimension_aligned_distance_only");
    }
    if a.get("parameterName").is_some() {
        return Err("native_dimension_parameter_rename_unsupported");
    }
    quantity(a)?;
    if !revision(&a["target"]["revision"]) {
        return Err("native_dimension_revision_required");
    }
    let s = a["sketch"]
        .as_object()
        .ok_or("native_dimension_exact_selector_required")?;
    let sid = s
        .get("sketchId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("native_dimension_exact_selector_required")?;
    if s.keys().any(|k| {
        !matches!(
            k.as_str(),
            "documentId" | "revision" | "objectType" | "sketchId"
        )
    }) || s.get("documentId").and_then(Value::as_str) != Some(doc)
        || s.get("revision") != t.get("revision")
        || s.get("objectType").and_then(Value::as_str) != Some("Sketch")
    {
        return Err("native_dimension_exact_selector_required");
    }
    let points = a["entities"]
        .as_array()
        .filter(|v| v.len() == 2)
        .ok_or("native_dimension_two_ordered_endpoints_required")?;
    for (i, p) in points.iter().enumerate() {
        let p = p
            .as_object()
            .ok_or("native_dimension_exact_selector_required")?;
        if p.keys().any(|k| {
            !matches!(
                k.as_str(),
                "documentId" | "revision" | "objectType" | "sketchId" | "entityId"
            )
        }) || p.get("documentId").and_then(Value::as_str) != Some(doc)
            || p.get("revision") != t.get("revision")
            || p.get("objectType").and_then(Value::as_str) != Some("SketchPoint")
            || p.get("sketchId").and_then(Value::as_str) != Some(sid)
            || p.get("entityId").and_then(Value::as_str) != Some(point_id(sid, i).as_str())
        {
            return Err("native_dimension_two_ordered_endpoints_required");
        }
    }
    Ok(())
}
fn safe<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .map_err(|_| "native_dimension_getter_panicked".to_string())?
}
fn capture<B: DimensionBackend>(b: &B, h: &B::Handle, lease: u64) -> Result<Projection, String> {
    let root = root_snapshot(b)?;
    let sketch = sketch_snapshot(b, h)?;
    let state = b.dimension_state(h, lease)?;
    let n = u64::from(state.dimension.is_some());
    let r = &state.relation;
    let l = &r.line;
    if !(1..=2).contains(&r.kind)
        || sketch.constraints != 1
        || sketch.dimensions != n
        || state.model_parameters != n
        || state.user_parameters != 0
        || l.line.constraints != 1
        || l.start.flags.constraints > 1
        || l.end.flags.constraints > 1
        || l.origin.flags.constraints != 0
        || l.line.dimensions > n
        || l.start.flags.dimensions > n
        || l.end.flags.dimensions > n
        || l.origin.flags.dimensions != 0
    {
        return Err("native_dimension_isolated_graph_required".into());
    }
    let mut plain = line_state::Capture {
        root: root.clone(),
        sketch: sketch.clone(),
        line: l.clone(),
    };
    plain.sketch.constraints = 0;
    plain.sketch.dimensions = 0;
    for f in [
        &mut plain.line.line,
        &mut plain.line.start.flags,
        &mut plain.line.end.flags,
    ] {
        f.constraints = 0;
        f.dimensions = 0;
    }
    line_state::validate(&plain)?;
    if [&l.line, &l.start.flags, &l.end.flags]
        .iter()
        .any(|f| f.fixed || f.fully_constrained)
    {
        return Err("native_dimension_unfixed_line_required".into());
    }
    let a = l.start.position_cm;
    let z = l.end.position_cm;
    let axis = if r.kind == 1 { 1 } else { 0 };
    if (a[axis] - z[axis]).abs() > 1e-8 || (a[1 - axis] - z[1 - axis]).abs() < 1e-5 {
        return Err("native_dimension_hv_alignment_required".into());
    }
    if let Some(d) = &state.dimension {
        if !d.driving
            || !d.aligned
            || !d.value_cm.is_finite()
            || d.value_cm <= 0.
            || !d.parameter_value_cm.is_finite()
            || !d
                .text_position_cm
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 10000.)
            || [
                &d.parameter_name,
                &d.parameter_expression,
                &d.parameter_unit,
            ]
            .iter()
            .any(|s| s.is_empty() || s.len() > 1024 || s.contains('\0'))
        {
            return Err("native_dimension_driving_parameter_required".into());
        }
        let length = ((a[0] - z[0]).powi(2) + (a[1] - z[1]).powi(2)).sqrt();
        if !near(length, d.value_cm) || !near(d.parameter_value_cm, d.value_cm) {
            return Err("native_dimension_length_unconfirmed".into());
        }
    }
    let e = b.entity(h, lease)?;
    if e.geometry
        != (geometry_preparation::Geometry::Line {
            start_cm: a,
            end_cm: z,
        })
        || e.construction != Some(false)
        || e.reference
    {
        return Err("native_dimension_line_geometry_changed".into());
    }
    if sketch_snapshot(b, h)? != sketch || root_snapshot(b)? != root {
        return Err("native_dimension_projection_changed".into());
    }
    Ok(Projection {
        root,
        sketch,
        state,
    })
}
fn snapshot<B: DimensionBackend>(b: &B, h: &B::Handle, lease: u64) -> Result<Projection, String> {
    let a = capture(b, h, lease)?;
    if capture(b, h, lease)? != a {
        return Err("native_dimension_projection_changed".into());
    }
    Ok(a)
}
fn near(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-8 + 1e-10 * a.abs().max(b.abs())
}
fn value(doc: &str, sid: &str, eid: &str, p: &Projection) -> Result<Value, String> {
    let rev = digest("sketch-dimensions-projection-v1", &(doc, sid, eid, p))?;
    let refs:Vec<_>=(0..2).map(|i|json!({"documentId":doc,"revision":rev,"objectType":"SketchPoint","sketchId":sid,"entityId":point_id(sid,i)})).collect();
    Ok(
        json!({"target":{"documentId":doc,"revision":rev},"sketchId":sid,"entityId":eid,"endpointRefs":refs,"endpointOrder":["start","end"],"dimensionId":p.state.dimension.as_ref().map(|_|format!("n:{sid}:0000000000000001")),"root":p.root,"sketch":p.sketch,"dimensionState":p.state,"revisionScope":SCOPE,"units":"centimeters","completeWithinIsolatedScope":true,"legacyParity":false,"solverPlacementUniquelySpecified":false}),
    )
}
pub fn known_refusal(code: &str) -> bool {
    line_state::known_refusal(code)
        || matches!(
            code,
            "native_dimension_isolated_graph_required"
                | "native_dimension_unfixed_line_required"
                | "native_dimension_hv_alignment_required"
                | "native_dimension_already_exists"
                | "native_dimension_endpoints_not_published"
        )
}
impl<H> Book<H> {
    pub fn inspect_dimensions<B: DimensionBackend<Handle = H>>(
        &self,
        b: &B,
        doc: &str,
        sid: &str,
        line_id: &str,
        expected: Option<&str>,
    ) -> Result<Value, String> {
        let (h, lease) = relations::select(self, doc, sid, line_id)?;
        let p = safe(|| snapshot(b, h, lease)).map_err(|e| {
            self.entries
                .iter()
                .find(|e| e.id == sid && e.document_id == doc)
                .unwrap()
                .entities_retired
                .set(true);
            e
        })?;
        let v = value(doc, sid, line_id, &p)?;
        if expected.is_some_and(|r| v["target"]["revision"] != r) {
            return Err("target_revision_mismatch".into());
        }
        self.entries
            .iter()
            .find(|e| e.id == sid && e.document_id == doc)
            .unwrap()
            .dimension_endpoints_published
            .set(true);
        Ok(v)
    }
    pub fn dimension_preflight<B: DimensionBackend<Handle = H>>(
        &self,
        b: &B,
        r: &PreparedRequest,
    ) -> Result<Plan, String> {
        b.clear_relation_diagnostic();
        supported_arguments(r).map_err(str::to_string)?;
        if r.operation() != "sketch_dimension" {
            return Err("native_dimension_operation_required".into());
        }
        let a = r.arguments();
        let doc = a["target"]["documentId"].as_str().unwrap();
        let sid = a["sketch"]["sketchId"].as_str().unwrap();
        let line_id = eid(sid);
        let (h, lease) = relations::select(self, doc, sid, &line_id)?;
        if !self
            .entries
            .iter()
            .find(|e| e.id == sid && e.document_id == doc)
            .unwrap()
            .dimension_endpoints_published
            .get()
        {
            return Err("native_dimension_endpoints_not_published".into());
        }
        let before = safe(|| snapshot(b, h, lease))?;
        if before.state.dimension.is_some() {
            return Err("native_dimension_already_exists".into());
        }
        let revision = digest(
            "sketch-dimensions-projection-v1",
            &(doc, sid, &line_id, &before),
        )?;
        if a["target"]["revision"] != revision {
            return Err("target_revision_mismatch".into());
        }
        if b.command()? != "SelectCommand" {
            return Err("native_dimension_command_refused".into());
        }
        let (value_cm, position_cm) = quantity(a).map_err(str::to_string)?;
        Ok(Plan {
            document_id: doc.into(),
            sketch_id: sid.into(),
            entity_id: line_id,
            before,
            revision,
            value_cm,
            position_cm,
        })
    }
    pub fn execute_dimension<B: DimensionBackend<Handle = H>>(&mut self, b: &B, p: Plan) -> Value {
        b.clear_relation_diagnostic();
        let mut v = self.execute_dimension_inner(b, p);
        if v["success"] != true {
            v["sdkDiagnostic"] = b.relation_diagnostic()
        }
        v
    }
    fn execute_dimension_inner<B: DimensionBackend<Handle = H>>(
        &mut self,
        b: &B,
        p: Plan,
    ) -> Value {
        let (h, lease) = match relations::select(self, &p.document_id, &p.sketch_id, &p.entity_id) {
            Ok(v) => v,
            Err(e) => return err(&e),
        };
        if let Err(e) = safe(|| {
            if snapshot(b, h, lease)? != p.before {
                return Err("target_revision_mismatch".into());
            }
            if b.command()? != "SelectCommand" {
                return Err("native_dimension_command_refused".into());
            }
            Ok(())
        }) {
            return err(&e);
        }
        let index = self
            .entries
            .iter()
            .position(|e| e.id == p.sketch_id && e.document_id == p.document_id)
            .unwrap();
        self.faulted = true;
        let h = self.entries[index].handle.as_ref().unwrap();
        let mut stages = json!({"dimensionAdd":null,"parameterValueSet":null});
        let mut observed: Option<Projection> = None;
        let unknown = |code: &str, stages: &Value, observed: &Option<Projection>| json!({"success":false,"errorCode":code,"outcome":"unknown","nativeExecutionStarted":true,"reconciliationRequired":true,"sketchId":p.sketch_id,"entityId":p.entity_id,"stages":stages,"observedDimension":observed,"automaticReplayAllowed":false,"rollbackAttempted":false});
        let a = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            b.add_dimension(h, lease, p.position_cm)
        })) {
            Ok(a) => a,
            Err(_) => return unknown("native_dimension_add_panicked", &stages, &observed),
        };
        stages["dimensionAdd"] = json!({"invoked":a.invoked,"acknowledged":a.acknowledged});
        if !a.invoked && !a.acknowledged {
            self.faulted = false;
            return err("native_dimension_not_invoked");
        }
        if !a.invoked || !a.acknowledged {
            return unknown("native_dimension_add_unconfirmed", &stages, &observed);
        }
        if let Err(e) = safe(|| {
            if b.command()? != "SelectCommand" {
                return Err("native_dimension_post_add_command_refused".into());
            }
            Ok(())
        }) {
            return unknown(&e, &stages, &observed);
        }
        let a = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            b.set_dimension_value(h, lease, p.value_cm)
        })) {
            Ok(a) => a,
            Err(_) => return unknown("native_dimension_set_panicked", &stages, &observed),
        };
        stages["parameterValueSet"] = json!({"invoked":a.invoked,"acknowledged":a.acknowledged});
        if !a.invoked || !a.acknowledged {
            return unknown("native_dimension_set_unconfirmed", &stages, &observed);
        }
        let result = safe(|| {
            let root = root_snapshot(b)?;
            let first = capture(b, h, lease)?;
            observed = Some(first.clone());
            let d = first
                .state
                .dimension
                .as_ref()
                .ok_or("native_dimension_missing_after_add")?;
            if !near(d.value_cm, p.value_cm) {
                return Err("native_dimension_requested_length_unconfirmed".into());
            }
            let mut r = first.root.clone();
            r.root_revision = p.before.root.root_revision.clone();
            let mut s = first.sketch.clone();
            s.sdk_revision = p.before.sketch.sdk_revision.clone();
            s.dimensions = 0;
            if r != p.before.root
                || s != p.before.sketch
                || first.state.relation.kind != p.before.state.relation.kind
                || first.state.relation.line.origin != p.before.state.relation.line.origin
            {
                return Err("native_dimension_postcondition_failed".into());
            }
            if first.root != root || capture(b, h, lease)? != first {
                return Err("native_dimension_final_projection_changed".into());
            }
            if b.command()? != "SelectCommand" {
                return Err("native_dimension_post_command_refused".into());
            }
            if root_snapshot(b)? != root {
                return Err("native_dimension_final_root_changed".into());
            }
            value(&p.document_id, &p.sketch_id, &p.entity_id, &first)
        });
        match result {
            Ok(data) => {
                self.faulted = false;
                json!({"success":true,"nativeExecutionStarted":true,"data":data,"stages":stages,"rollbackAttempted":false,"automaticReplayAllowed":false,"legacyParity":false})
            }
            Err(e) => unknown(&e, &stages, &observed),
        }
    }
}
#[cfg(test)]
pub(crate) mod tests;

//! Native parameter slice. JSON, bounds, projection revisions and effect policy
//! live in Rust; Backend implementations only perform typed SDK operations.
//! No transaction/rollback, geometry-wide revision or legacy full-inspect parity
//! is claimed. Each Plan is consumed once and is not an execution permit: the
//! caller must obtain its durable retained-journal permit before execute.

use fusion_mechanical_core::PreparedRequest;
use serde::Serialize;
use serde_json::{json, Value};

pub const MAX_PARAMETERS: usize = 512;
pub const MAX_TIMELINE: usize = 1024;
pub const MAX_PROJECTION_BYTES: usize = 128 * 1024;
pub const REVISION_SCOPE: &str = "parameter-projection-v1: all local numeric/text parameter names, expressions, units, values, comments and flags; document default length unit; design summary, root component revision and timeline health. Excludes full geometry, referenced designs and assembly graph.";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignState {
    pub active: bool,
    pub parametric: bool,
    pub root_name: String,
    pub root_revision: String,
    pub default_length_units: String,
    pub component_count: u64,
    pub body_count: u64,
    pub sketch_count: u64,
    pub timeline_count: u64,
    pub timeline_position: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ParameterValue {
    Numeric(f64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Parameter {
    pub name: String,
    pub expression: String,
    pub unit: String,
    pub comment: String,
    pub is_user: bool,
    pub is_deletable: bool,
    /// Numeric values use Autodesk database units, not display units.
    pub value: ParameterValue,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthItem {
    pub index: usize,
    /// Autodesk FeatureHealthStates, 0..=5. Unknown is explicit, not healthy.
    pub state: u32,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub document_id: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub target: Target,
    pub revision_scope: &'static str,
    pub complete: bool,
    pub scope: &'static str,
    pub design: DesignState,
    pub parameters: Vec<Parameter>,
    pub health: Vec<HealthItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
}
type Result<T> = std::result::Result<T, Failure>;
fn failure(code: &'static str, message: impl Into<String>) -> Failure {
    let message = message.into();
    let mut end = message.len().min(1024);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    Failure {
        code,
        message: message[..end].into(),
    }
}
fn sdk(message: String) -> Failure {
    failure("native_sdk_failed", message)
}

/// Bound to one exact, currently retained SDK document. Callers must keep that
/// lease alive and forbid registry reentrancy throughout each call sequence.
pub trait Backend {
    fn design_state(&self) -> std::result::Result<DesignState, String>;
    fn parameter_count(&self) -> std::result::Result<usize, String>;
    fn parameter(&self, index: usize) -> std::result::Result<Parameter, String>;
    fn timeline_health(&self, index: usize) -> std::result::Result<HealthItem, String>;
    /// Validate and evaluate in one document's UnitsManager. Return database
    /// units, and Err for invalid expressions (including SDK getter failures).
    fn evaluate_expression(&self, expression: &str, unit: &str)
        -> std::result::Result<f64, String>;
    fn create_parameter(
        &self,
        name: &str,
        expression: &str,
        unit: &str,
        comment: &str,
    ) -> std::result::Result<(), String>;
    fn set_parameter(&self, name: &str, expression: &str) -> std::result::Result<(), String>;
    fn compute_all(&self) -> std::result::Result<(), String>;
}

#[derive(Debug)]
enum Change {
    Create {
        name: String,
        expression: String,
        unit: String,
        comment: String,
    },
    Set {
        name: String,
        expression: String,
    },
}
impl Change {
    fn name(&self) -> &str {
        match self {
            Self::Create { name, .. } | Self::Set { name, .. } => name,
        }
    }
    fn expression(&self) -> &str {
        match self {
            Self::Create { expression, .. } | Self::Set { expression, .. } => expression,
        }
    }
}

#[derive(Debug)]
pub struct Plan {
    before: Inspection,
    change: Change,
    expected_value: f64,
}
impl Plan {
    pub fn document_id(&self) -> &str {
        &self.before.target.document_id
    }
    pub fn observed_document(&self) -> Value {
        json!({"documentId":self.before.target.document_id,"revision":self.before.target.revision,"revisionScope":REVISION_SCOPE,"scope":"native parameter slice"})
    }
}

fn bounded_text(value: &str, max: usize, empty: bool) -> Result<()> {
    if value.len() > max || (!empty && value.trim().is_empty()) || value.contains('\0') {
        Err(failure(
            "invalid_input",
            "Text is empty, exceeds the native slice bound, or contains NUL.",
        ))
    } else {
        Ok(())
    }
}
fn projection(
    backend: &(impl Backend + ?Sized),
) -> Result<(DesignState, Vec<Parameter>, Vec<HealthItem>)> {
    let design = backend.design_state().map_err(sdk)?;
    bounded_text(&design.root_name, 4096, true)?;
    bounded_text(&design.root_revision, 4096, false)?;
    bounded_text(&design.default_length_units, 64, false)?;
    if design.timeline_count > MAX_TIMELINE as u64
        || design.timeline_position > design.timeline_count
    {
        return Err(failure(
            "native_projection_limit",
            "Timeline exceeds the bounded native inspection or has an invalid marker.",
        ));
    }
    let count = backend.parameter_count().map_err(sdk)?;
    if count > MAX_PARAMETERS {
        return Err(failure(
            "native_projection_limit",
            "Parameter count exceeds the native slice bound.",
        ));
    }
    let mut parameters = Vec::with_capacity(count);
    let mut byte_count =
        design.root_name.len() + design.root_revision.len() + design.default_length_units.len();
    for index in 0..count {
        let parameter = backend.parameter(index).map_err(sdk)?;
        bounded_text(&parameter.name, 256, false)?;
        bounded_text(&parameter.expression, 4096, true)?;
        bounded_text(&parameter.unit, 64, true)?;
        bounded_text(&parameter.comment, 4096, true)?;
        byte_count += parameter.name.len()
            + parameter.expression.len()
            + parameter.unit.len()
            + parameter.comment.len();
        match &parameter.value {
            ParameterValue::Numeric(value) if !value.is_finite() => {
                return Err(failure(
                    "native_projection_invalid",
                    "Nonfinite parameter readback.",
                ))
            }
            ParameterValue::Text(text) => {
                bounded_text(text, 4096, true)?;
                byte_count += text.len();
            }
            _ => {}
        }
        if byte_count > MAX_PROJECTION_BYTES {
            return Err(failure(
                "native_projection_limit",
                "Parameter projection exceeds its byte bound.",
            ));
        }
        parameters.push(parameter);
    }
    parameters.sort_by(|left, right| left.name.cmp(&right.name));
    if parameters
        .windows(2)
        .any(|pair| pair[0].name == pair[1].name)
    {
        return Err(failure(
            "native_projection_invalid",
            "Duplicate parameter name prevents unique mutation targeting.",
        ));
    }
    let mut health = Vec::with_capacity(design.timeline_count as usize);
    for index in 0..design.timeline_count as usize {
        let item = backend.timeline_health(index).map_err(sdk)?;
        if item.index != index || item.state > 5 {
            return Err(failure(
                "native_projection_invalid",
                "Invalid timeline health readback.",
            ));
        }
        bounded_text(&item.message, 4096, true)?;
        byte_count += item.message.len();
        if byte_count > MAX_PROJECTION_BYTES {
            return Err(failure(
                "native_projection_limit",
                "Health projection exceeds its byte bound.",
            ));
        }
        health.push(item);
    }
    if backend.parameter_count().map_err(sdk)? != count
        || backend.design_state().map_err(sdk)? != design
    {
        return Err(failure(
            "native_projection_changed",
            "Design changed during parameter inspection.",
        ));
    }
    Ok((design, parameters, health))
}

pub fn inspect(backend: &(impl Backend + ?Sized), document_id: &str) -> Result<Inspection> {
    bounded_text(document_id, 128, false)?;
    let first = projection(backend)?;
    if first != projection(backend)? {
        return Err(failure(
            "native_projection_changed",
            "Repeated parameter/health observations disagree.",
        ));
    }
    let mut bytes = b"adom-fusion-parameter-projection-v1\0".to_vec();
    let body = serde_json::to_vec(&first).map_err(|_| {
        failure(
            "native_projection_invalid",
            "Projection serialization failed.",
        )
    })?;
    if body.len() > MAX_PROJECTION_BYTES {
        return Err(failure(
            "native_projection_limit",
            "Serialized parameter projection exceeds its byte bound.",
        ));
    }
    bytes.extend_from_slice(&body);
    let revision = format!(
        "parameters-v1:{}",
        fusion_mechanical_journal::request_digest(&bytes)
    );
    Ok(Inspection { target: Target { document_id: document_id.into(), revision }, revision_scope: REVISION_SCOPE, complete: false,
        scope: "Native parameter projection, root design counts and timeline health only; body geometry, occurrences, assembly and full legacy inspection are not included.",
        design: first.0, parameters: first.1, health: first.2 })
}

fn writable(inspection: &Inspection) -> Result<()> {
    if !inspection.design.active {
        return Err(failure(
            "wrong_document",
            "The exact target document must be active for parameter mutation.",
        ));
    }
    if !inspection.design.parametric {
        return Err(failure(
            "unsupported_capability",
            "This slice mutates only parametric Designs.",
        ));
    }
    if inspection.design.timeline_position != inspection.design.timeline_count {
        return Err(failure(
            "timeline_not_at_end",
            "Timeline marker must be at the end before parameter mutation.",
        ));
    }
    if inspection
        .health
        .iter()
        .any(|item| matches!(item.state, 1 | 2 | 4))
    {
        return Err(failure(
            "unhealthy_design",
            "Timeline has a warning, error or rolled-back item.",
        ));
    }
    Ok(())
}

pub fn preflight(
    backend: &(impl Backend + ?Sized),
    request: &PreparedRequest,
    document_id: &str,
) -> Result<Plan> {
    if !matches!(request.operation(), "parameter_create" | "parameter_set") {
        return Err(failure("unsupported_capability", "Only parameter_create/set are supported by this native slice; atomic parameter_batch is unqualified."));
    }
    let args = request.arguments();
    if args["target"]["documentId"].as_str() != Some(document_id) {
        return Err(failure(
            "wrong_document",
            "Request target does not identify the leased document.",
        ));
    }
    let before = inspect(backend, document_id)?;
    if args["target"]["revision"].as_str() != Some(before.target.revision.as_str()) {
        return Err(failure(
            "stale_revision",
            "Parameter projection changed; inspect before a new operation.",
        ));
    }
    writable(&before)?;
    let name = args["name"]
        .as_str()
        .ok_or_else(|| failure("invalid_input", "Parameter name is required."))?;
    let expression = args["expression"]
        .as_str()
        .ok_or_else(|| failure("invalid_input", "Expression is required."))?;
    bounded_text(name, 256, false)?;
    bounded_text(expression, 4096, false)?;
    let existing = before
        .parameters
        .iter()
        .find(|parameter| parameter.name == name);
    let (change, unit) = if request.operation() == "parameter_create" {
        if name.len() > 64
            || !name.as_bytes()[0].is_ascii_alphabetic()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(failure(
                "invalid_input",
                "Use a short ASCII parameter identifier beginning with a letter.",
            ));
        }
        if existing.is_some() {
            return Err(failure(
                "parameter_exists",
                "Parameter name already exists.",
            ));
        }
        let unit = args["unit"]
            .as_str()
            .ok_or_else(|| failure("invalid_input", "Parameter unit is required."))?;
        if !matches!(unit, "mm" | "deg" | "kg" | "") {
            return Err(failure(
                "unsupported_capability",
                "Unsupported parameter unit.",
            ));
        }
        let comment = args.get("comment").and_then(Value::as_str).unwrap_or("");
        bounded_text(comment, 4096, true)?;
        (
            Change::Create {
                name: name.into(),
                expression: expression.into(),
                unit: unit.into(),
                comment: comment.into(),
            },
            unit.to_owned(),
        )
    } else {
        let parameter = existing
            .ok_or_else(|| failure("parameter_missing", "Parameter name does not exist."))?;
        if !matches!(parameter.value, ParameterValue::Numeric(_)) {
            return Err(failure(
                "unsupported_capability",
                "Text parameter mutation is not qualified in this slice.",
            ));
        }
        (
            Change::Set {
                name: name.into(),
                expression: expression.into(),
            },
            parameter.unit.clone(),
        )
    };
    let expected_value = backend
        .evaluate_expression(expression, &unit)
        .map_err(|message| failure("invalid_expression", message))?;
    if !expected_value.is_finite() {
        return Err(failure(
            "invalid_expression",
            "Expression evaluated to a nonfinite value.",
        ));
    }
    // Evaluation must not silently change the projection used for the guard.
    if inspect(backend, document_id)?.target != before.target {
        return Err(failure(
            "native_projection_changed",
            "Projection changed during expression validation.",
        ));
    }
    Ok(Plan {
        before,
        change,
        expected_value,
    })
}

/// Call exactly once after a durable executing claim. There is no automatic
/// recovery, inverse mutation or retry. Any SDK error after the first mutator
/// call leaves the effect outcome unknown even if the SDK returned false/null.
pub fn execute(backend: &(impl Backend + ?Sized), plan: Plan) -> Value {
    let before = &plan.before.target;
    let refusal = |error: Failure| json!({"success":false,"nativeExecutionStarted":false,"outcome":"not_executed","errorCode":error.code,"message":error.message,"target":before,"revisionScope":REVISION_SCOPE,"transactional":false,"recoveryAttempted":false});
    match inspect(backend, &before.document_id) {
        Ok(current) if current.target == *before => {
            if let Err(error) = writable(&current) {
                return refusal(error);
            }
        }
        Ok(_) => {
            return refusal(failure(
                "stale_revision",
                "Projection changed after preflight; no parameter setter was invoked.",
            ))
        }
        Err(error) => return refusal(error),
    }
    let changed = match &plan.change {
        Change::Create {
            name,
            expression,
            unit,
            comment,
        } => backend.create_parameter(name, expression, unit, comment),
        Change::Set { name, expression } => backend.set_parameter(name, expression),
    };
    let outcome =
        changed
            .map_err(sdk)
            .and_then(|_| backend.compute_all().map_err(sdk))
            .and_then(|_| {
                let after = inspect(backend, &before.document_id)?;
                writable(&after)?;
                let parameter = after
                    .parameters
                    .iter()
                    .find(|parameter| parameter.name == plan.change.name())
                    .ok_or_else(|| {
                        failure(
                            "native_readback_mismatch",
                            "Mutated parameter was not present in readback.",
                        )
                    })?;
                match parameter.value {
                    ParameterValue::Numeric(actual)
                        if (actual - plan.expected_value).abs()
                            <= 1e-9 * plan.expected_value.abs().max(1.0) => {}
                    _ => return Err(failure(
                        "native_readback_mismatch",
                        "Parameter numeric readback does not match the preflight expression value.",
                    )),
                }
                // Matching evaluated values alone is not proof that a driving formula
                // was applied: "width * 2" and a literal may coincide only right now.
                // Until SDK normalization is qualified, require the expression itself
                // (apart from outer whitespace) and fail conservatively on rewrites.
                if parameter.expression.trim() != plan.change.expression().trim() {
                    return Err(failure(
                        "native_readback_mismatch",
                        "Stored parameter expression differs from the requested expression.",
                    ));
                }
                if let Change::Create { unit, comment, .. } = &plan.change {
                    if !parameter.is_user
                        || parameter.unit != *unit
                        || parameter.comment != *comment
                        || after.parameters.len() != plan.before.parameters.len() + 1
                    {
                        return Err(failure(
                        "native_readback_mismatch",
                        "Created parameter metadata/count does not match the requested operation.",
                    ));
                    }
                } else if after.parameters.len() != plan.before.parameters.len() {
                    return Err(failure(
                        "native_readback_mismatch",
                        "Parameter inventory count changed unexpectedly.",
                    ));
                }
                let readback = parameter.clone();
                Ok((after, readback))
            });
    match outcome {
        Ok((after, parameter)) => {
            json!({"success":true,"nativeExecutionStarted":true,"outcome":"observed_success","target":after.target,"revisionScope":REVISION_SCOPE,"parameter":parameter,"health":after.health,"transactional":false,"recoveryAttempted":false,"scope":"Native parameter create/set with compute and numeric/metadata readback; no rollback guarantee."})
        }
        Err(error) => {
            json!({"success":false,"nativeExecutionStarted":true,"outcome":"unknown","reconciliationRequired":true,"errorCode":error.code,"message":error.message,"beforeTarget":before,"revisionScope":REVISION_SCOPE,"transactional":false,"recoveryAttempted":false,"retryAuthorized":false})
        }
    }
}

#[cfg(test)]
mod tests;

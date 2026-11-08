//! Read-only native supplier command discovery. Availability is registration,
//! never permission/applicability or evidence of an opened/downloaded part.
use fusion_mechanical_core::PreparedRequest;
use serde::Serialize;
use serde_json::{json, Value};
pub const COMMAND_ID: &str = "InsertMcMasterCarrComponentCommand";
pub const SCOPE:&str="mcmaster-discovery-projection-v1: exact target document/runtime, design intent, and native command registration ID/name only; no applicability, online access, UI launch, download, insertion or geometry";
pub const CONTRACT: &str = "native-purchased-discovery-v1";
pub type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    pub id: String,
    pub name: String,
    pub native: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub design_intent: u32,
    pub command: Option<Command>,
}
pub trait Backend {
    fn snapshot(&self) -> Result<Snapshot>;
}
pub fn supported_arguments(
    request: &PreparedRequest,
    epoch: &str,
) -> std::result::Result<(), &'static str> {
    if request.operation() != "mcmaster_discover" {
        return Ok(());
    }
    let args = request.arguments();
    if args["sessionId"].as_str() != Some(epoch) {
        return Err("native_mcmaster_current_session_required");
    }
    let target = args["target"]
        .as_object()
        .ok_or("native_mcmaster_exact_target_required")?;
    if target
        .keys()
        .any(|k| !matches!(k.as_str(), "documentId" | "revision"))
        || target
            .get("documentId")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err("native_mcmaster_exact_target_required");
    }
    if target.get("revision").is_some_and(|v| {
        v.as_str()
            .is_none_or(|s| s.is_empty() || s.len() > 256 || s.contains('\0'))
    }) {
        return Err("native_mcmaster_revision_required");
    }
    Ok(())
}
fn validate(s: &Snapshot) -> Result<()> {
    if s.design_intent > 2 {
        return Err("native_mcmaster_unknown_design_intent".into());
    }
    if let Some(c) = &s.command {
        if c.id != COMMAND_ID || !c.native || c.name.len() > 4096 || c.name.contains('\0') {
            return Err("native_mcmaster_invalid_command_registration".into());
        }
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
    if request.operation() != "mcmaster_discover"
        || request.arguments()["target"]["documentId"] != document_id
    {
        return Err("native_mcmaster_exact_target_required".into());
    }
    let first = backend.snapshot()?;
    validate(&first)?;
    let second = backend.snapshot()?;
    validate(&second)?;
    if first != second {
        return Err("native_mcmaster_discovery_changed".into());
    }
    let projection = json!({"epoch":epoch,"documentId":document_id,"snapshot":first});
    let bytes =
        serde_json::to_vec(&projection).map_err(|_| "native_mcmaster_serialization_failed")?;
    let revision = format!(
        "mcmaster-discovery-projection-v1:{}",
        fusion_mechanical_journal::request_digest(&bytes)
    );
    if request.arguments()["target"]["revision"]
        .as_str()
        .is_some_and(|r| r != revision)
    {
        return Err("target_revision_mismatch".into());
    }
    let intent = match first.design_intent {
        0 => "part",
        1 => "assembly",
        _ => "hybrid",
    };
    Ok(
        json!({"target":{"documentId":document_id,"revision":revision},"revisionScope":SCOPE,"commandId":COMMAND_ID,"available":first.command.is_some(),"availabilityScope":"registered native command definition only; applicability and online access unverified","name":first.command.as_ref().map(|c|&c.name),"definitionIsNative":first.command.as_ref().map(|c|c.native),"designIntent":intent,"designIntentCode":first.design_intent,"hybridTarget":first.design_intent==2,"requirements":["Hybrid design","online supplier access","interactive selection and download"],"commandExecutionSupported":false,"downloadSupported":false,"applicabilityVerified":false,"onlineAccessVerified":false,"downloadedCadAlternative":{"operation":"cad_import","nativeSupported":false}}),
    )
}
#[cfg(test)]
mod tests;

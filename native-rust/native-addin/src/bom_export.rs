//! A captured BOM is an immutable export input. SDK capture precedes execution
//! admission; this module publishes once on the coordinator worker, never on the
//! Fusion thread. Model changes after capture do not rewrite snapshot provenance.
use crate::bom::Captured;
use fusion_artifact_publication::{publish_csv, PublicationState};
use fusion_mechanical_core::PreparedRequest;
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Debug)]
pub struct Plan {
    path: PathBuf,
    bytes: Vec<u8>,
    overwrite: bool,
    target: Value,
    part_count: usize,
    mode: String,
    configuration: Value,
}
pub fn supported_arguments(request: &PreparedRequest) -> Result<(), &'static str> {
    if !matches!(request.operation(), "bom" | "bom_export") {
        return Ok(());
    }
    let args = request.arguments();
    let target = args["target"]
        .as_object()
        .ok_or("native_bom_exact_target_required")?;
    if !target
        .get("documentId")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
        || target
            .keys()
            .any(|key| !matches!(key.as_str(), "documentId" | "revision"))
    {
        return Err("native_bom_exact_target_required");
    }
    if target.contains_key("revision")
        && !target["revision"]
            .as_str()
            .is_some_and(|value| value.starts_with("bom-projection-v1:"))
    {
        return Err("native_bom_revision_required");
    }
    if request.operation() == "bom" {
        return Ok(());
    }
    let name = args["filename"]
        .as_str()
        .filter(|name| name.len() <= 8192 && !name.contains('\0'))
        .ok_or("native_bom_invalid_destination")?;
    let path = std::path::Path::new(name);
    if !path.is_absolute()
        || path
            .extension()
            .and_then(|s| s.to_str())
            .is_none_or(|s| !s.eq_ignore_ascii_case("csv"))
    {
        return Err("native_bom_invalid_destination");
    }
    if !args["target"]["revision"]
        .as_str()
        .is_some_and(|value| value.starts_with("bom-projection-v1:"))
    {
        return Err("native_bom_revision_required");
    }
    Ok(())
}
impl Plan {
    pub fn prepare(request: &PreparedRequest, captured: Captured) -> Result<Self, String> {
        if request.operation() != "bom_export" {
            return Err("native_bom_export_operation_required".into());
        }
        supported_arguments(request)?;
        let args = request.arguments();
        if args["target"]["documentId"] != captured.data["target"]["documentId"]
            || args["target"]["revision"] != captured.data["target"]["revision"]
        {
            return Err("target_revision_mismatch".into());
        }
        let mode = args["mode"].as_str().unwrap_or("flat");
        let mode_kind = match mode {
            "flat" => fusion_bom_core::CsvMode::Flat,
            "structured" => fusion_bom_core::CsvMode::Structured,
            _ => return Err("native_bom_invalid_mode".into()),
        };
        let bytes = fusion_bom_core::csv_bytes(&captured.calculation, mode_kind)
            .map_err(|error| error.code.to_owned())?;
        if bytes.len() > crate::bom::MAX_BYTES {
            return Err("native_bom_csv_bound".into());
        }
        Ok(Self {
            path: PathBuf::from(args["filename"].as_str().unwrap()),
            bytes,
            overwrite: args["overwrite"].as_bool().unwrap_or(false),
            target: captured.data["target"].clone(),
            part_count: captured.calculation.part_count,
            mode: mode.into(),
            configuration: captured.data["configuration"].clone(),
        })
    }
    pub fn observed_document(&self) -> Value {
        json!({"target":self.target,"scope":"immutable native BOM snapshot exported after durable journal start; no model mutation or live-model lock"})
    }
    /// Consumes the single-use plan. The coordinator must supply its durable
    /// retained execution permit before calling; an I/O error never retries.
    pub fn execute(self) -> Value {
        match publish_csv(&self.path, &self.bytes, self.overwrite) {
            Ok(published) => {
                json!({"success":true,"nativeExecutionStarted":true,"data":{"target":self.target,"partCount":self.part_count,"mode":self.mode,"configuration":self.configuration,"calculationAccuracy":"high","outputPath":published.output_path,"fileSize":published.file_size,"sha256":published.sha256,"published":published.published,"overwrite":published.overwrite,"cleanupWarning":published.cleanup_warning,"snapshotExport":true,"powerLossDurabilityGuaranteed":false,"modelMutation":false}})
            }
            Err(error) => {
                let uncertain = error.publication == PublicationState::Unknown;
                json!({"success":false,"errorCode":"native_bom_publication_failed","stage":error.stage,"publicationState":if uncertain{"unknown"}else{"not_attempted"},"outcome":if uncertain{"unknown"}else{"failed"},"reconciliationRequired":uncertain,"nativeExecutionStarted":true,"cleanupWarningPresent":error.cleanup_warning.is_some(),"modelMutation":false})
            }
        }
    }
}

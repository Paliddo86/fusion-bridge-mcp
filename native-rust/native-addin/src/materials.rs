//! Read-only catalog policy. SDK IDs identify catalog records, never CAD entities.
use fusion_mechanical_core::PreparedRequest;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
pub type Result<T> = std::result::Result<T, String>;
pub const MAX_LIBRARIES: usize = 64;
pub const MAX_MATERIALS: usize = 20_000;
pub const MAX_BYTES: usize = 1024 * 1024;
pub const SCOPE: &str = "material-catalog-projection-v1: loaded library IDs, names and counts plus every material ID/name in the selected source; no physical properties, appearances, geometry or assignment capability";
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Library {
    pub id: String,
    pub name: String,
    pub count: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Material {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Snapshot {
    libraries: Vec<Library>,
    materials: Vec<Material>,
}
pub trait Backend {
    fn library_count(&self) -> Result<usize>;
    fn library(&self, index: usize) -> Result<Library>;
    fn material_count(&self, library_id: Option<&str>) -> Result<usize>;
    fn material(&self, library_id: Option<&str>, index: usize) -> Result<Material>;
}
pub fn supported_arguments(request: &PreparedRequest) -> std::result::Result<(), &'static str> {
    if request.operation() != "material_list" {
        return Ok(());
    }
    let args = request.arguments();
    let target = args["target"]
        .as_object()
        .ok_or("native_material_exact_target_required")?;
    if target
        .keys()
        .any(|k| !matches!(k.as_str(), "documentId" | "revision"))
        || target
            .get("documentId")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err("native_material_exact_target_required");
    }
    for (key, max) in [("libraryId", 4096), ("query", 1024)] {
        if args
            .get(key)
            .is_some_and(|v| v.as_str().is_none_or(|s| s.len() > max || s.contains('\0')))
        {
            return Err("native_material_argument_bound");
        }
    }
    if target
        .get("revision")
        .is_some_and(|v| v.as_str().is_none_or(|s| s.is_empty() || s.len() > 256))
    {
        return Err("native_material_revision_required");
    }
    Ok(())
}
fn check_record(id: &str, name: &str, bytes: &mut usize) -> Result<()> {
    if id.is_empty()
        || id.len() > 4096
        || name.len() > 4096
        || id.contains('\0')
        || name.contains('\0')
    {
        return Err("native_material_invalid_record".into());
    }
    *bytes = bytes
        .checked_add(id.len() + name.len())
        .ok_or("native_material_size_overflow")?;
    if *bytes > MAX_BYTES {
        return Err("native_material_catalog_bound".into());
    }
    Ok(())
}
fn capture(backend: &impl Backend, source: Option<&str>) -> Result<Snapshot> {
    let count = backend.library_count()?;
    if count > MAX_LIBRARIES {
        return Err("native_material_library_bound".into());
    }
    let mut libraries = Vec::with_capacity(count);
    let mut ids = BTreeSet::new();
    let mut bytes = 0;
    for index in 0..count {
        let row = backend.library(index)?;
        check_record(&row.id, &row.name, &mut bytes)?;
        if !ids.insert(row.id.clone()) {
            return Err("native_material_duplicate_library_id".into());
        }
        libraries.push(row);
    }
    let selected = match source {
        Some(id) => Some(
            libraries
                .iter()
                .find(|v| v.id == id)
                .ok_or("material_library_missing")?,
        ),
        None => None,
    };
    let material_count = backend.material_count(source)?;
    if material_count > MAX_MATERIALS {
        return Err("native_material_count_bound".into());
    }
    if selected.is_some_and(|v| v.count != material_count) {
        return Err("native_material_catalog_changed".into());
    }
    let mut materials = Vec::with_capacity(material_count);
    let mut ids = BTreeSet::new();
    for index in 0..material_count {
        let row = backend.material(source, index)?;
        check_record(&row.id, &row.name, &mut bytes)?;
        if !ids.insert(row.id.clone()) {
            return Err("native_material_duplicate_material_id".into());
        }
        materials.push(row);
    }
    if backend.material_count(source)? != material_count || backend.library_count()? != count {
        return Err("native_material_catalog_changed".into());
    }
    Ok(Snapshot {
        libraries,
        materials,
    })
}
pub fn inspect(
    backend: &impl Backend,
    document_id: &str,
    request: &PreparedRequest,
) -> Result<Value> {
    supported_arguments(request)?;
    if request.operation() != "material_list"
        || request.arguments()["target"]["documentId"] != document_id
    {
        return Err("native_material_exact_target_required".into());
    }
    let args = request.arguments();
    let requested = args["libraryId"].as_str();
    let source = requested.filter(|s| !s.is_empty());
    let first = capture(backend, source)?;
    if first != capture(backend, source)? {
        return Err("native_material_catalog_changed".into());
    }
    let projection = json!({"documentId":document_id,"sourceLibraryId":source,"catalog":first});
    let bytes =
        serde_json::to_vec(&projection).map_err(|_| "native_material_serialization_failed")?;
    if bytes.len() > MAX_BYTES {
        return Err("native_material_projection_bound".into());
    }
    let revision = format!(
        "material-catalog-projection-v1:{}",
        fusion_mechanical_journal::request_digest(&bytes)
    );
    if args["target"]["revision"]
        .as_str()
        .is_some_and(|v| v != revision)
    {
        return Err("target_revision_mismatch".into());
    }
    let query = args["query"].as_str().unwrap_or("").to_lowercase();
    let limit = args["limit"].as_u64().unwrap_or(100) as usize;
    let matching: Vec<_> = first
        .materials
        .iter()
        .filter(|v| v.name.to_lowercase().contains(&query))
        .collect();
    let result = json!({"target":{"documentId":document_id,"revision":revision},"revisionScope":SCOPE,"libraries":first.libraries,"materials":matching.iter().take(limit).collect::<Vec<_>>(),"complete":matching.len()<=limit,"matchingCount":matching.len(),"libraryId":requested,"catalogRecordCount":first.materials.len(),"assignmentSupported":false});
    if serde_json::to_vec(&result)
        .map_err(|_| "native_material_serialization_failed")?
        .len()
        > MAX_BYTES
    {
        return Err("native_material_response_bound".into());
    }
    Ok(result)
}
#[cfg(test)]
mod tests;

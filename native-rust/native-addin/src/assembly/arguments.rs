use super::*;
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub key: String,
    pub value: String,
    pub native: bool,
}
#[derive(Clone, Debug)]
pub enum Action {
    Create {
        changes: Vec<Change>,
    },
    Insert {
        component_id: String,
    },
    Metadata {
        component_id: String,
        changes: Vec<Change>,
    },
    Translate {
        occurrence_id: String,
        position_mm: [f64; 3],
    },
}
impl Action {
    pub fn cloud(&self) -> bool {
        match self {
            Self::Create { changes } | Self::Metadata { changes, .. } => changes
                .iter()
                .any(|c| matches!(c.key.as_str(), "partNumber" | "description")),
            _ => false,
        }
    }
    pub fn changes(&self) -> &[Change] {
        match self {
            Self::Create { changes } | Self::Metadata { changes, .. } => changes,
            _ => &[],
        }
    }
}
pub fn is_write(op: &str) -> bool {
    matches!(
        op,
        "component_create" | "occurrence_insert" | "component_metadata" | "occurrence_update"
    )
}
fn name(value: &Value) -> std::result::Result<String, &'static str> {
    let s = value.as_str().ok_or("native_assembly_name_required")?;
    if s.trim().is_empty() || s.len() > 256 || s.contains(['\0', '\n', '\r']) {
        return Err("native_assembly_name_bound");
    }
    Ok(s.into())
}
fn changes(raw: &Value) -> std::result::Result<Vec<Change>, &'static str> {
    let obj = raw
        .as_object()
        .ok_or("native_assembly_metadata_object_required")?;
    if obj.is_empty() || obj.len() > 8 {
        return Err("native_assembly_metadata_bound");
    }
    let mut map = BTreeMap::new();
    for (k, v) in obj {
        let native = matches!(k.as_str(), "name" | "partNumber" | "description");
        let value = match k.as_str() {
            "name" => name(v)?,
            "partNumber" | "description" => {
                let s = v.as_str().ok_or("native_assembly_metadata_type")?;
                if !text(s) {
                    return Err("native_assembly_metadata_bound");
                }
                s.to_owned()
            }
            "supplier" | "supplierPartNumber" | "sourceUrl" => {
                if !v.as_str().is_some_and(text) {
                    return Err("native_assembly_metadata_type");
                }
                serde_json::to_string(v).map_err(|_| "native_assembly_metadata_serialization")?
            }
            "asPurchased" | "excludeFromBom" => {
                if !v.is_boolean() {
                    return Err("native_assembly_metadata_type");
                }
                v.to_string()
            }
            _ => return Err("native_assembly_metadata_key_unsupported"),
        };
        if !text(&value) {
            return Err("native_assembly_metadata_encoded_bound");
        }
        map.insert(
            k.clone(),
            Change {
                key: k.clone(),
                value,
                native,
            },
        );
    }
    let mut out = vec![];
    if let Some(v) = map.remove("name") {
        out.push(v);
    }
    for key in ["partNumber", "description"] {
        if let Some(v) = map.remove(key) {
            out.push(v);
        }
    }
    out.extend(map.into_values());
    Ok(out)
}
fn reference(
    a: &Value,
    key: &str,
    doc: &str,
    rev: &str,
) -> std::result::Result<String, &'static str> {
    let r = a[key]
        .as_object()
        .ok_or("native_assembly_exact_component_required")?;
    if r.len() != 4
        || r.get("documentId").and_then(Value::as_str) != Some(doc)
        || r.get("revision").and_then(Value::as_str) != Some(rev)
        || r.get("objectType").and_then(Value::as_str) != Some("Component")
    {
        return Err("native_assembly_exact_component_required");
    }
    let id = r
        .get("componentId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("native_assembly_exact_component_required")?;
    Ok(id.into())
}
fn identity_transform(t: &Value) -> bool {
    let Some(o) = t.as_object() else { return false };
    if o.keys()
        .any(|k| !matches!(k.as_str(), "position" | "axis" | "angle"))
    {
        return false;
    }
    if let Some(a) = o.get("angle") {
        if a.as_f64() != Some(0.) {
            return false;
        }
    }
    if let Some(p) = o.get("position") {
        if !p
            .as_array()
            .is_some_and(|a| a.len() == 3 && a.iter().all(|v| v.as_f64() == Some(0.)))
        {
            return false;
        }
    }
    if let Some(v) = o.get("axis") {
        if !v.as_array().is_some_and(|a| {
            a.len() == 3
                && a.iter()
                    .all(|v| v.as_f64().is_some_and(|x| x.is_finite() && x.abs() <= 1e6))
                && a.iter().any(|v| v.as_f64().unwrap().abs() > 1e-9)
        }) {
            return false;
        }
    }
    true
}
pub fn supported_arguments(r: &PreparedRequest) -> std::result::Result<(), &'static str> {
    if !is_write(r.operation()) && r.operation() != "native_assembly_inspect" {
        return Ok(());
    }
    let a = r.arguments();
    let t = a["target"]
        .as_object()
        .ok_or("native_assembly_exact_target_required")?;
    if t.keys()
        .any(|k| !matches!(k.as_str(), "documentId" | "revision"))
        || t.get("documentId")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err("native_assembly_exact_target_required");
    }
    if is_write(r.operation()) || t.contains_key("revision") {
        if !t
            .get("revision")
            .and_then(Value::as_str)
            .is_some_and(|s| s.starts_with(PREFIX))
        {
            return Err("native_assembly_revision_required");
        }
    }
    if r.operation() == "native_assembly_inspect" {
        if a.get("nativeMetadata").is_some_and(|v| !v.is_boolean()) {
            return Err("native_assembly_metadata_type");
        }
        return Ok(());
    }
    parse(r).map(|_| ())
}
pub(super) fn parse(r: &PreparedRequest) -> std::result::Result<Action, &'static str> {
    let a = r.arguments();
    let doc = a["target"]["documentId"]
        .as_str()
        .ok_or("native_assembly_exact_target_required")?;
    let rev = a["target"]["revision"]
        .as_str()
        .ok_or("native_assembly_revision_required")?;
    if r.operation() == "occurrence_update" {
        if a.get("grounded").is_some() || a.get("groundedToParent").is_some() {
            return Err("native_placement_grounding_changes_unsupported");
        }
        let reference = a["occurrence"]
            .as_object()
            .ok_or("native_placement_exact_occurrence_required")?;
        if reference.len() != 4
            || reference.get("documentId").and_then(Value::as_str) != Some(doc)
            || reference.get("revision").and_then(Value::as_str) != Some(rev)
            || reference.get("objectType").and_then(Value::as_str) != Some("Occurrence")
        {
            return Err("native_placement_exact_occurrence_required");
        }
        let id = reference
            .get("occurrenceId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("native_placement_exact_occurrence_required")?;
        let transform = a["transform"]
            .as_object()
            .ok_or("native_placement_position_only")?;
        if transform.len() != 1 {
            return Err("native_placement_position_only");
        }
        let p = transform
            .get("position")
            .and_then(Value::as_array)
            .filter(|p| p.len() == 3)
            .ok_or("native_placement_position_only")?;
        let mut position_mm = [0.; 3];
        for (i, v) in p.iter().enumerate() {
            position_mm[i] = v
                .as_f64()
                .filter(|v| v.is_finite() && v.abs() <= placement::MAX_MM)
                .ok_or("native_placement_position_bound")?;
        }
        return Ok(Action::Translate {
            occurrence_id: id.into(),
            position_mm,
        });
    }
    if a.get("parent").is_some() {
        return Err("native_assembly_implicit_root_parent_only");
    }
    if a.get("transform").is_some_and(|t| !identity_transform(t)) {
        return Err("native_assembly_identity_placement_required");
    }
    match r.operation() {
        "component_create" => {
            let n = name(&a["name"])?;
            let mut cs = if let Some(m) = a.get("metadata") {
                changes(m)?
            } else {
                vec![]
            };
            if cs
                .iter()
                .any(|c| matches!(c.key.as_str(), "partNumber" | "description"))
            {
                return Err("native_assembly_native_metadata_separate_update_required");
            }
            if cs.iter().any(|c| c.key == "name" && c.value != n) {
                return Err("native_assembly_name_metadata_conflict");
            }
            cs.retain(|c| c.key != "name");
            cs.insert(
                0,
                Change {
                    key: "name".into(),
                    value: n,
                    native: true,
                },
            );
            Ok(Action::Create { changes: cs })
        }
        "occurrence_insert" => {
            if a.get("independentCopy").is_some_and(|v| v != false) {
                return Err("native_assembly_shared_instances_only");
            }
            Ok(Action::Insert {
                component_id: reference(a, "component", doc, rev)?,
            })
        }
        "component_metadata" => {
            let cs = changes(&a["changes"])?;
            if cs.iter().any(|c| c.key == "name")
                && cs.iter().any(|c| c.key == "description")
                && !cs.iter().any(|c| c.key == "partNumber")
            {
                return Err("native_assembly_rename_cloud_update_requires_explicit_part_number");
            }
            Ok(Action::Metadata {
                component_id: reference(a, "component", doc, rev)?,
                changes: cs,
            })
        }
        _ => Err("native_assembly_operation_required"),
    }
}

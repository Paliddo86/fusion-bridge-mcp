fn hash_value(v: &Value) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(v).map_err(|e| e.to_string())?)
    ))
}
fn revision(v: &Value, prefix: &str) -> Result<String> {
    let s = v.as_str().ok_or("revision string")?;
    ensure(
        s.strip_prefix(prefix).is_some_and(|x| {
            x.len() == 64
                && x.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }),
        "revision scope/digest",
    )?;
    Ok(s.into())
}
fn false_revision(prefix: &str, current: &Value) -> Result<Value> {
    let s = revision(current, prefix)?;
    Ok(json!(format!(
        "{prefix}{}",
        if s.ends_with('0') {
            "1".repeat(64)
        } else {
            "0".repeat(64)
        }
    )))
}
fn same_except(a: &Value, b: &Value, omit: &[&str]) -> bool {
    let (mut a, mut b) = (a.clone(), b.clone());
    let (Some(a), Some(b)) = (a.as_object_mut(), b.as_object_mut()) else {
        return false;
    };
    for k in omit {
        a.remove(*k);
        b.remove(*k);
    }
    a == b
}
fn validate_authority(runtime: &Value, a: &Value, raw_runtime: &[u8]) -> Result<()> {
    let fields = [
        "schemaVersion",
        "purpose",
        "pid",
        "epoch",
        "runtimeSha256",
        "processCreationTime100ns",
        "allowCreateOneTaskDocument",
        "allowTwoNativeComponentDefinitions",
        "allowOneSharedOccurrence",
        "allowTwoLocalMetadataUpdates",
        "allowTwoAbsoluteOccurrenceTranslations",
        "allowTwoPositionSnapshots",
        "fixture",
        "campaignId",
        "baselineDocumentCount",
        "baselineSha256",
        "priorTaskDocumentIds",
    ];
    ensure(
        a.as_object()
            .is_some_and(|m| m.len() == fields.len() && fields.iter().all(|f| m.contains_key(*f))),
        "exact authority fields",
    )?;
    ensure(
        a["schemaVersion"] == 1
            && a["purpose"] == "r21-native-placement-qualification"
            && a["pid"] == runtime["pid"]
            && runtime["pid"].as_u64().is_some_and(|n| n > 0)
            && a["epoch"] == runtime["epoch"]
            && a["runtimeSha256"] == format!("{:x}", Sha256::digest(raw_runtime)),
        "authority runtime binding",
    )?;
    ensure(
        a["processCreationTime100ns"]
            .as_str()
            .and_then(|x| x.parse::<u64>().ok())
            .is_some_and(|x| x > 0),
        "external process preflight required",
    )?;
    ensure(
        [
            "allowCreateOneTaskDocument",
            "allowTwoNativeComponentDefinitions",
            "allowOneSharedOccurrence",
            "allowTwoLocalMetadataUpdates",
            "allowTwoAbsoluteOccurrenceTranslations",
            "allowTwoPositionSnapshots",
        ]
        .iter()
        .all(|k| a[*k] == true)
            && a["fixture"] == "two-definitions-three-occurrences-two-translations-v1",
        "effect authority",
    )?;
    let id = text(a, "campaignId")?;
    ensure(
        (8..=32).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric()),
        "campaign namespace",
    )?;
    let count = a["baselineDocumentCount"]
        .as_u64()
        .filter(|n| (1..=3).contains(n))
        .ok_or("baseline count1..3")?;
    let before = document_map(&runtime["before"])?;
    let prior = a["priorTaskDocumentIds"]
        .as_array()
        .ok_or("prior task list")?;
    ensure(
        before.len() == count as usize
            && prior.len() + 1 == before.len()
            && a["baselineSha256"] == hash_value(&runtime["before"])?
            && active_id(&before).is_ok(),
        "exact bounded baseline",
    )?;
    let mut unique = std::collections::BTreeSet::new();
    for id in prior {
        let id = id.as_str().ok_or("prior task ID")?;
        ensure(
            unique.insert(id)
                && before.contains_key(id)
                && current_document_id(id, text(runtime, "epoch")?)
                && before[id]["saved"] == false
                && before[id]["name"] == "Untitled"
                && before[id]["modified"] == true,
            "prior task missing/duplicate/state",
        )?;
    }
    let blank: Vec<_> = before
        .iter()
        .filter(|(id, _)| !prior.iter().any(|p| p.as_str() == Some(id.as_str())))
        .collect();
    ensure(
        blank.len() == 1
            && blank[0].1["saved"] == false
            && blank[0].1["modified"] == false
            && blank[0].1["name"] == "Untitled",
        "one pristine original blank metadata required",
    )?;
    Ok(())
}
fn assert_roster(
    before: &BTreeMap<String, Value>,
    after: &BTreeMap<String, Value>,
    created: &str,
) -> Result<()> {
    ensure(
        after.len() == before.len() + 1
            && after.contains_key(created)
            && !before.contains_key(created),
        "one exact returned created document",
    )?;
    for (id, d) in before {
        ensure(
            after
                .get(id)
                .is_some_and(|v| same_except(d, v, &["active"])),
            "original document metadata changed",
        )?;
    }
    ensure(
        active_id(after)? == created
            && after[created]["saved"] == false
            && after[created]["name"] == "Untitled",
        "created document state",
    )
}
fn response_failure(r: &Response, epoch: &Value, code: &str) -> Result<()> {
    ensure(
        r.code == 400
            && r.body["success"] == false
            && r.body["epoch"] == *epoch
            && r.body["errorCode"] == code
            && r.body["nativeExecutionStarted"] == false,
        "precise known refusal",
    )
}
const ASSEMBLY_PREFIX: &str = "root-assembly-projection-v1:";
fn capabilities(r: &Response) -> Result<()> {
    let b = &r.body;
    ensure(
        r.code == 200
            && b["contractVersion"] == CONTRACT
            && b["requiredHeader"] == format!("X-Adom-Mechanical-Contract: {CONTRACT}")
            && b["legacyParity"] == false
            && b["transactional"] == false
            && b["placementContract"] == PLACEMENT_CONTRACT
            && b["placementRequiredHeader"]
                == format!("X-Adom-Mechanical-Contract: {PLACEMENT_CONTRACT}"),
        "R21 additive assembly and placement capability contracts",
    )?;
    let ops = b["operations"].as_array().ok_or("operation inventory")?;
    let expected = [
        "inspect",
        "parameter_create",
        "parameter_set",
        "new_design",
        "native_scratch_discard",
        "native_document_activate",
        "bom",
        "bom_export",
        "sketch_create",
        "native_sketch_inspect",
        "sketch_geometry",
        "native_sketch_profiles",
        "feature_create",
        "native_sketch_entities",
        "material_list",
        "mcmaster_discover",
        "assembly_properties",
        "native_sketch_line_state",
        "native_sketch_relations",
        "sketch_constraint",
        "native_sketch_dimensions",
        "sketch_dimension",
        "native_assembly_inspect",
        "component_create",
        "occurrence_insert",
        "component_metadata",
        "occurrence_update",
    ];
    ensure(
        ops.len() == expected.len()
            && expected
                .iter()
                .all(|e| ops.iter().filter(|v| v.as_str() == Some(e)).count() == 1),
        "exact additive operation inventory",
    )
}
fn expected_definition(index: usize, changed: bool) -> (String, Value) {
    let mut attrs = if index == 0 {
        a_metadata()
    } else {
        b_metadata()
    };
    let mut name = if index == 0 {
        "Assembly fixture A"
    } else {
        "Assembly fixture B"
    }
    .to_owned();
    if changed {
        let changes = if index == 0 { a_changes() } else { b_changes() };
        for (k, v) in changes.as_object().unwrap() {
            if k == "name" {
                name = v.as_str().unwrap().into()
            } else {
                attrs[k] = v.clone();
            }
        }
    }
    (name, attrs)
}
fn assembly(
    v: &Value,
    doc: &Value,
    definitions: usize,
    occurrences: usize,
    changed_a: bool,
    changed_b: bool,
) -> Result<()> {
    ensure(
        definitions <= 2
            && occurrences <= 3
            && v["target"]["documentId"] == *doc
            && v["complete"] == false
            && v["nativeMetadataObserved"] == false
            && v["cloudMetadata"] == false,
        "finite local assembly scope",
    )?;
    let rev = revision(&v["target"]["revision"], ASSEMBLY_PREFIX)?;
    let d = doc.as_str().ok_or("document string")?;
    let root = &v["root"];
    ensure(
        root["active"] == true
            && root["parametric"] == true
            && root["hybrid"] == true
            && root["configured"] == false
            && root["offline"].is_boolean()
            && root["pendingSnapshot"] == false
            && root["components"] == definitions + 1
            && root["occurrences"] == occurrences,
        "root shape/membership",
    )?;
    for k in [
        "bodies",
        "sketches",
        "meshBodies",
        "joints",
        "asBuiltJoints",
        "rigidGroups",
        "assemblyConstraints",
        "motionLinks",
        "jointOrigins",
    ] {
        ensure(root[k] == 0, "unexpected root resource")?;
    }
    ensure(
        root["revision"]
            .as_str()
            .is_some_and(|s| !s.is_empty() && s.len() <= 4096)
            && root["timeline"].as_u64().is_some_and(|n| n <= 4)
            && root["timelinePosition"] == root["timeline"],
        "root revision/timeline",
    )?;
    let cs = v["components"].as_array().ok_or("component list")?;
    let os = v["occurrences"].as_array().ok_or("occurrence list")?;
    ensure(
        cs.len() == definitions && os.len() == occurrences,
        "collection counts",
    )?;
    for (i, c) in cs.iter().enumerate() {
        let id = format!("c:{d}:{:016x}", i + 1);
        let (name, attrs) = expected_definition(i, if i == 0 { changed_a } else { changed_b });
        ensure(
            c["componentId"] == id
                && c["ref"]
                    == json!({"documentId":doc,"revision":rev,"objectType":"Component","componentId":id})
                && c["name"] == name
                && c["revision"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && s.len() <= 4096),
            "component identity/name/revision",
        )?;
        ensure(
            c.get("partNumber") == Some(&Value::Null) && c.get("description") == Some(&Value::Null),
            "unobserved cloud metadata fabricated",
        )?;
        for k in [
            "bodies",
            "sketches",
            "occurrences",
            "meshBodies",
            "joints",
            "asBuiltJoints",
            "rigidGroups",
            "assemblyConstraints",
            "motionLinks",
        ] {
            ensure(c[k] == 0, "nonempty component")?;
        }
        let actual = c["attributes"].as_object().ok_or("attributes")?;
        ensure(
            actual.len() == attrs.as_object().unwrap().len(),
            "attribute membership",
        )?;
        for (k, e) in attrs.as_object().unwrap() {
            let raw = actual
                .get(k)
                .and_then(Value::as_str)
                .ok_or("encoded metadata field")?;
            let decoded: Value = serde_json::from_str(raw).map_err(|_| "metadata JSON encoding")?;
            ensure(decoded == *e, "metadata value/escaping")?;
        }
    }
    let mapping = [0usize, 1, 0];
    for (i, o) in os.iter().enumerate() {
        let ci = mapping[i];
        ensure(
            ci < definitions
                && o["component"] == ci
                && o["componentId"] == cs[ci]["componentId"]
                && o["occurrenceId"] == format!("o:{d}:{:016x}", i + 1),
            "distinct occurrence/shared definition binding",
        )?;
        ensure(
            o["native"] == true
                && o["children"] == 0
                && o["name"].as_str().is_some_and(|s| !s.is_empty())
                && o["path"].as_str().is_some_and(|s| !s.is_empty()),
            "occurrence native/path",
        )?;
        for k in ["grounded", "groundedToParent"] {
            ensure(o[k].is_boolean(), "grounding observation missing")?;
        }
        for k in ["referenced", "configured", "suppressed", "rolledBack"] {
            ensure(o[k] == false, "unsupported occurrence state")?;
        }
        let m = o["matrix"].as_array().ok_or("placement matrix")?;
        ensure(m.len() == 16, "matrix length")?;
        for (i, x) in m.iter().enumerate() {
            let expected = if i % 5 == 0 { 1. } else { 0. };
            ensure(
                x.as_f64()
                    .is_some_and(|x| x.is_finite() && (x - expected).abs() <= 1e-10),
                "identity placement",
            )?;
        }
    }
    Ok(())
}
fn stages(body: &Value, op: &str, changes: Option<&Value>, component: usize) -> Result<()> {
    ensure(
        body["nativeExecutionStarted"] == true
            && body["automaticReplayAllowed"] == false
            && body["rollbackAttempted"] == false
            && body["legacyParity"] == false,
        "effect stage policy",
    )?;
    let ss = body["stages"].as_array().ok_or("stages")?;
    let added = matches!(op, "component_create" | "occurrence_insert");
    let mut keys: Vec<_> = changes
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    if let Some(i) = keys.iter().position(|k| k == "name") {
        keys.remove(i);
        keys.insert(0, "name".into());
    }
    ensure(
        ss.len() == keys.len() + usize::from(added),
        "exact stage count",
    )?;
    for (i, s) in ss.iter().enumerate() {
        ensure(
            s["attempt"]["invoked"] == true && s["attempt"]["acknowledged"] == true,
            "unconfirmed native stage",
        )?;
        if added && i == 0 {
            ensure(
                s["stage"] == "occurrenceAdd" && s["attempt"]["component"] == component,
                "add stage identity",
            )?;
            ensure(
                s["attempt"]["occurrence"].as_u64().is_some_and(|n| {
                    Some(n as usize + 1) == body["data"]["occurrences"].as_array().map(Vec::len)
                }),
                "add occurrence binding",
            )?;
        } else {
            ensure(
                s["stage"] == "componentMetadata" && s["key"] == keys[i - usize::from(added)],
                "metadata stage order",
            )?;
            ensure(
                s["attempt"]["component"] == component && s["attempt"]["occurrence"].is_null(),
                "metadata component binding",
            )?;
        }
    }
    ensure(
        body["componentId"] == body["data"]["components"][component]["componentId"],
        "result component binding",
    )
}

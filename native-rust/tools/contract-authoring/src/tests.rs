use super::*;
const SOURCE: &[u8] = include_bytes!("../data/catalog.json");
const SCHEMA: &[u8] = include_bytes!("../data/mechanical-schema.json");
const OLD: &[u8] = include_bytes!("../evidence/baseline/describe.json");
const MANIFEST: &[u8] = include_bytes!("../evidence/baseline/bridge.json");
fn catalog() -> Catalog {
    Catalog::load(SOURCE, SCHEMA).unwrap()
}
fn altered(f: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut v = parse(SOURCE).unwrap();
    f(&mut v);
    encode(&v, false).unwrap()
}
#[test]
fn complete_frozen_contract_matches_all_665236_baseline_bytes() {
    let c = catalog();
    assert_eq!(c.check(OLD).unwrap()["completeContractByteEqual"], true);
    assert_eq!(encode(&c.frozen(), true).unwrap(), OLD);
    assert_eq!(c.frozen()["verbs"].as_array().unwrap().len(), 179);
    assert_eq!(c.manifest(MANIFEST).unwrap(), MANIFEST);
}
#[test]
fn exact_legacy_extraction_reproduces_canonical_data_without_executing_it() {
    let result = literal::extract(
        include_str!("../evidence/baseline/describe.py"),
        SCHEMA,
        OLD,
    )
    .unwrap();
    assert_eq!(encode(&result, false).unwrap(), SOURCE);
    let bad = include_str!("../evidence/baseline/describe.py").to_string() + "\n";
    assert!(literal::extract(&bad, SCHEMA, OLD)
        .unwrap_err()
        .contains("exact baseline"));
}
#[test]
fn runtime_order_note_filter_and_duplicated_envelope_are_exact() {
    let source = parse(SOURCE).unwrap();
    let c = catalog();
    let runtime = c.describe();
    let names: Vec<_> = source["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r[0].clone())
        .filter(|n| !n.as_str().unwrap().ends_with("_note"))
        .collect();
    let rows = runtime["verbs"].as_array().unwrap();
    assert_eq!(rows.len(), 178);
    assert_eq!(
        rows.iter().map(|r| r["name"].clone()).collect::<Vec<_>>(),
        names
    );
    assert_eq!(runtime["verbs"], runtime["data"]["verbs"]);
    assert_eq!(runtime["verbCount"], runtime["data"]["verbCount"]);
    assert_eq!(runtime["bridge"], runtime["data"]["bridge"]);
    assert_eq!(rows[0]["name"], "fusion_mech_capabilities");
    assert_eq!(rows.last().unwrap()["name"], "fusion_describe");
    assert_eq!(
        rows[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![
            "name",
            "summary",
            "input",
            "output",
            "timeoutSeconds",
            "statusVerb",
            "longRunning",
            "example"
        ]
    );
    assert!(c.frozen()["verbs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["name"] == "fusion_open_by_urn_note"));
}
#[test]
fn status_alias_null_and_false_are_not_coerced() {
    let source = parse(SOURCE).unwrap();
    let c = catalog();
    let runtime = c.describe();
    let mut alias = 0;
    for row in source["rows"].as_array().unwrap() {
        if row[0].as_str().unwrap().ends_with("_note") {
            continue;
        }
        let found = runtime["verbs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == row[0])
            .unwrap();
        if row[5] == json!({"$statusDefault":true}) {
            assert_eq!(found["statusVerb"], "fusion_get_app_state");
            alias += 1;
        } else {
            assert_eq!(found["statusVerb"], row[5]);
        }
        assert_eq!(found["longRunning"], row[6]);
    }
    assert!(alias > 10);
}
#[test]
fn mechanical_expansion_preserves_all61_keys_and_enum_order() {
    let c = catalog();
    let d = c.describe();
    let schema = parse(SCHEMA).unwrap();
    let op = d["verbs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "fusion_mechanical")
        .unwrap();
    assert_eq!(op["input"]["operationSchemas"], schema);
    assert_eq!(
        op["input"]["operationSchemas"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        schema.as_object().unwrap().keys().collect::<Vec<_>>()
    );
    assert_eq!(schema.as_object().unwrap().len(), 61);
    assert_eq!(
        op["input"]["op"],
        format!(
            "required enum: {}",
            schema
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join("|")
        )
    );
    assert_eq!(d["verbs"][0]["output"]["operations"], schema);
}
#[test]
fn malformed_duplicate_keys_rows_and_unknown_markers_refuse() {
    for b in [
        b"{\"a\":1,\"a\":2}".as_slice(),
        b"{\"nested\":{\"x\":0,\"x\":1}}",
        b"[] trailing",
        b"[NaN]",
        b"[",
        b"{\"bad\":\"\\uD800\"}",
    ] {
        assert!(parse(b).is_err());
    }
    for bytes in [
        altered(|v| {
            let row = v["rows"][0].clone();
            v["rows"].as_array_mut().unwrap().push(row)
        }),
        altered(|v| {
            v["rows"][0].as_array_mut().unwrap().pop();
        }),
        altered(|v| v["rows"][0][6] = json!(1)),
        altered(|v| v["rows"][0][3]["operations"] = json!({"$mechanicalSchema":false})),
        altered(|v| v["extra"] = json!(true)),
    ] {
        assert!(Catalog::load(&bytes, SCHEMA).is_err());
    }
}
#[test]
fn schema_mismatch_order_change_and_duplicate_schema_refuse() {
    let changed = SCHEMA.iter().copied().chain([b'\n']).collect::<Vec<_>>();
    assert!(Catalog::load(SOURCE, &changed)
        .unwrap_err_text()
        .contains("digest"));
    let bytes = altered(|v| {
        v["mechanicalOperationOrder"]
            .as_array_mut()
            .unwrap()
            .reverse()
    });
    assert!(Catalog::load(&bytes, SCHEMA).is_err());
    assert!(Catalog::load(SOURCE, b"{\"x\":{},\"x\":{}}").is_err());
}
trait ErrorText {
    fn unwrap_err_text(self) -> String;
}
impl<T> ErrorText for Result<T> {
    fn unwrap_err_text(self) -> String {
        match self {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        }
    }
}
#[test]
fn complete_baseline_discrepancy_or_encoding_difference_refuses_freeze() {
    let mut v = parse(OLD).unwrap();
    v["verbs"][0]["summary"] = json!("changed");
    assert!(catalog()
        .freeze(&encode(&v, true).unwrap(), MANIFEST)
        .is_err());
    let compact = serde_json::to_vec(&parse(OLD).unwrap()).unwrap();
    assert!(catalog()
        .check(&compact)
        .unwrap_err()
        .contains("encoding/order"));
    let mut bytes = OLD.to_vec();
    bytes.push(b'\n');
    assert!(catalog().check(&bytes).is_err());
}
#[test]
fn manifest_sync_changes_only_verbs_preserving_other_field_order() {
    let mut m = parse(MANIFEST).unwrap();
    m["verbs"] = json!(["stale"]);
    m["localPreference"] = json!({"preserve":true});
    let before = m.clone();
    let c = catalog();
    let out = parse(&c.manifest(&encode(&m, false).unwrap()).unwrap()).unwrap();
    assert_eq!(
        out.as_object().unwrap().keys().collect::<Vec<_>>(),
        before.as_object().unwrap().keys().collect::<Vec<_>>()
    );
    for (k, v) in before.as_object().unwrap() {
        if k != "verbs" {
            assert_eq!(&out[k], v);
        }
    }
    assert_eq!(out["verbs"].as_array().unwrap().len(), 179);
}
#[test]
fn encoding_preserves_python_exponents_ascii_escapes_and_strings() {
    let out = String::from_utf8(
        encode(
            &json!({"n":0.000001,"plus":1e30,"text":"1e-6 \\ 雪 🚀"}),
            true,
        )
        .unwrap(),
    )
    .unwrap();
    assert!(out.contains("1e-06"));
    assert!(out.contains("1e+30"));
    assert!(out.contains("1e-6 \\\\ \\u96ea \\ud83d\\ude80"));
}
fn temp() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "fusion-contract-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&p).unwrap();
    p
}
#[test]
fn publication_exposes_whole_bundle_once_and_never_overwrites() {
    let root = temp();
    let out = root.join("out");
    let files = catalog().freeze(OLD, MANIFEST).unwrap();
    publish(&out, &files).unwrap();
    assert_eq!(fs::read(out.join("describe.json")).unwrap(), OLD);
    assert_eq!(fs::read(out.join("bridge.json")).unwrap(), MANIFEST);
    assert!(publish(&out, &files).is_err());
    let empty = root.join("existing-empty");
    fs::create_dir(&empty).unwrap();
    assert!(publish(&empty, &files).is_err());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn failed_second_staged_file_leaves_no_public_or_staging_output() {
    let root = temp();
    let out = root.join("out");
    let files = vec![
        ("first".into(), b"complete".to_vec()),
        ("x".repeat(300), b"fail".to_vec()),
    ];
    assert!(publish(&out, &files).is_err());
    assert!(!out.exists());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn concurrent_publishers_cannot_mix_or_replace_bundles() {
    let root = temp();
    let dest = root.join("out");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let jobs: Vec<_> = (0..2)
        .map(|i| {
            let dest = dest.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                publish(&dest, &[("a".into(), vec![i]), ("b".into(), vec![i])])
            })
        })
        .collect();
    let results: Vec<_> = jobs.into_iter().map(|j| j.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        fs::read(dest.join("a")).unwrap(),
        fs::read(dest.join("b")).unwrap()
    );
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn filename_duplicates_traversal_and_oversize_refuse_before_creation() {
    let root = temp();
    for files in [
        vec![("../outside".into(), vec![])],
        vec![("x".into(), vec![]), ("x".into(), vec![])],
        vec![("x".into(), vec![0; MAX_BYTES + 1])],
    ] {
        assert!(publish(&root.join("out"), &files).is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_schema_expansions_refuse_before_large_allocations() {
    let bytes = altered(|v| {
        for i in 0..100 {
            v["rows"][0][2][format!("large{i}")] = json!({"$mechanicalSchema":true});
        }
    });
    assert!(Catalog::load(&bytes, SCHEMA)
        .unwrap_err_text()
        .contains("expansion"));
}

#[test]
fn explicit_refreeze_changes_rows_date_and_manifest_as_one_consistent_bundle() {
    let source = altered(|v| {
        v["rows"][0][1] = json!("Deliberately updated summary");
        v["rows"][1][0] = json!("authored_replacement");
    });
    let c = Catalog::load(&source, SCHEMA).unwrap();
    assert!(c.freeze(OLD, MANIFEST).is_err());
    let bundle = c.refreeze(Some(OLD), MANIFEST, "2026-10-16").unwrap();
    let doc = parse(&bundle[0].1).unwrap();
    assert_eq!(doc["frozen"], "2026-10-16");
    assert_eq!(doc["verbs"], c.frozen()["verbs"]);
    for key in ["contract", "source", "note"] {
        assert_eq!(doc[key], c.frozen()[key]);
    }
    let manifest = parse(&bundle[1].1).unwrap();
    assert_eq!(manifest["verbs"][0], "authored_replacement");
    let updated = Catalog::load(&bundle[2].1, SCHEMA).unwrap();
    updated.check(&bundle[0].1).unwrap();
    let again = updated
        .refreeze(Some(&bundle[0].1), &bundle[1].1, "2099-01-01")
        .unwrap();
    assert_eq!(again, bundle);
}

#[test]
fn unchanged_refreeze_preserves_old_metadata_layout_and_syncs_stale_manifest() {
    let mut old = parse(OLD).unwrap();
    old["frozen"] = json!("2024-02-29");
    old["note"] = json!("Retain this exact prior note");
    let bytes = serde_json::to_vec(&old).unwrap();
    let mut man = parse(MANIFEST).unwrap();
    man["verbs"] = json!([]);
    let bundle = catalog()
        .refreeze(Some(&bytes), &encode(&man, false).unwrap(), "2026-10-16")
        .unwrap();
    assert_eq!(bundle[0].1, bytes);
    assert_eq!(bundle[1].1, MANIFEST);
    assert_eq!(
        parse(&bundle[2].1).unwrap()["metadata"]["note"],
        old["note"]
    );
    assert_eq!(
        parse(&bundle[2].1).unwrap()["metadata"]["frozen"],
        old["frozen"]
    );
    assert!(catalog().check(&bytes).is_err()); // Historical strict check is separate.
}

#[test]
fn missing_old_contract_initializes_but_malformed_old_or_dates_refuse() {
    let c = catalog();
    let fresh = c.refreeze(None, MANIFEST, "2000-02-29").unwrap();
    assert_eq!(parse(&fresh[0].1).unwrap()["frozen"], "2000-02-29");
    assert_eq!(
        fresh,
        c.refreeze(Some(b"{}"), MANIFEST, "2000-02-29").unwrap()
    );
    for date in [
        "today",
        "2026-1-01",
        "2026-02-29",
        "1900-02-29",
        "0000-01-01",
        "2026-00-01",
        "2026-04-31",
        "2026-12-00",
        "2026-10-16Z",
        "2026-é-16",
    ] {
        assert!(c.refreeze(Some(OLD), MANIFEST, date).is_err(), "{date}");
    }
    for old in [
        b"[]".as_slice(),
        b"{",
        b"{\"verbs\":null}",
        b"{\"verbs\":[],\"verbs\":[]}",
    ] {
        assert!(c.refreeze(Some(old), MANIFEST, "2026-10-16").is_err());
    }
}

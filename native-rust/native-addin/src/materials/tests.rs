use super::*;
use std::cell::Cell;
const DOC: &str = "d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001";
fn request(extra: Value) -> PreparedRequest {
    let mut v = json!({"op":"material_list","target":{"documentId":DOC}});
    for (k, vv) in extra.as_object().unwrap() {
        v[k] = vv.clone();
    }
    fusion_mechanical_core::prepare_request(v, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap()
}
struct Fake {
    libraries: Vec<Library>,
    materials: Vec<Material>,
    calls: Cell<usize>,
    library_reads: Cell<usize>,
    fail: usize,
    change: bool,
    count_override: Option<usize>,
    seen_sources: std::cell::RefCell<Vec<Option<String>>>,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            libraries: vec![
                Library {
                    id: "lib-a".into(),
                    name: "Same Name".into(),
                    count: 2,
                },
                Library {
                    id: "lib-b".into(),
                    name: "Same Name".into(),
                    count: 2,
                },
            ],
            materials: vec![
                Material {
                    id: "a".into(),
                    name: "Aluminum".into(),
                },
                Material {
                    id: "b".into(),
                    name: "Aluminum".into(),
                },
            ],
            calls: Cell::new(0),
            library_reads: Cell::new(0),
            fail: 0,
            change: false,
            count_override: None,
            seen_sources: Default::default(),
        }
    }
}
impl Fake {
    fn step(&self) -> Result<()> {
        let n = self.calls.get() + 1;
        self.calls.set(n);
        if n == self.fail {
            Err("getter failed".into())
        } else {
            Ok(())
        }
    }
    fn source(&self, id: Option<&str>) {
        self.seen_sources.borrow_mut().push(id.map(str::to_owned));
    }
}
impl Backend for Fake {
    fn library_count(&self) -> Result<usize> {
        self.step()?;
        self.library_reads.set(self.library_reads.get() + 1);
        Ok(self.libraries.len())
    }
    fn library(&self, i: usize) -> Result<Library> {
        self.step()?;
        Ok(self.libraries[i].clone())
    }
    fn material_count(&self, id: Option<&str>) -> Result<usize> {
        self.step()?;
        self.source(id);
        Ok(self.count_override.unwrap_or(self.materials.len()))
    }
    fn material(&self, id: Option<&str>, i: usize) -> Result<Material> {
        self.step()?;
        self.source(id);
        let mut m = self.materials[i].clone();
        if self.change && self.library_reads.get() >= 3 {
            m.name.push('!');
        }
        Ok(m)
    }
}
#[test]
fn document_materials_preserve_distinct_ids_with_same_names() {
    let b = Fake::default();
    let v = inspect(&b, DOC, &request(json!({}))).unwrap();
    assert_eq!(v["matchingCount"], 2);
    assert_eq!(v["complete"], true);
    assert_ne!(v["materials"][0]["id"], v["materials"][1]["id"]);
    assert_eq!(v["libraryId"], Value::Null);
    assert!(b.seen_sources.borrow().iter().all(Option::is_none));
    assert!(!request(json!({})).is_write());
}
#[test]
fn library_selection_uses_id_not_duplicate_display_name() {
    let b = Fake::default();
    let v = inspect(&b, DOC, &request(json!({"libraryId":"lib-b"}))).unwrap();
    assert_eq!(v["libraryId"], "lib-b");
    assert!(b
        .seen_sources
        .borrow()
        .iter()
        .all(|v| v.as_deref() == Some("lib-b")));
    assert_eq!(
        inspect(
            &Fake::default(),
            DOC,
            &request(json!({"libraryId":"Same Name"}))
        )
        .unwrap_err(),
        "material_library_missing"
    );
}
#[test]
fn unicode_search_counts_all_matches_before_limiting() {
    let mut b = Fake::default();
    b.materials[0].name = "ÉTAIN".into();
    b.materials[1].name = "Étain alloy".into();
    let v = inspect(&b, DOC, &request(json!({"query":"étain","limit":1}))).unwrap();
    assert_eq!(v["matchingCount"], 2);
    assert_eq!(v["materials"].as_array().unwrap().len(), 1);
    assert_eq!(v["complete"], false);
}
#[test]
fn no_match_and_empty_document_are_complete() {
    for empty in [false, true] {
        let mut b = Fake::default();
        if empty {
            b.materials.clear();
        }
        let v = inspect(&b, DOC, &request(json!({"query":"missing"}))).unwrap();
        assert_eq!(v["matchingCount"], 0);
        assert_eq!(v["complete"], true);
    }
}
#[test]
fn empty_library_id_keeps_legacy_document_source_semantics() {
    let b = Fake::default();
    let v = inspect(&b, DOC, &request(json!({"libraryId":""}))).unwrap();
    assert_eq!(v["libraryId"], "");
    assert!(b.seen_sources.borrow().iter().all(Option::is_none));
}
#[test]
fn changing_material_name_with_identical_counts_refuses() {
    let b = Fake {
        change: true,
        ..Default::default()
    };
    assert_eq!(
        inspect(&b, DOC, &request(json!({}))).unwrap_err(),
        "native_material_catalog_changed"
    );
}
#[test]
fn every_sdk_getter_failure_refuses_whole_response() {
    let good = Fake::default();
    inspect(&good, DOC, &request(json!({}))).unwrap();
    for fail in 1..=good.calls.get() {
        let b = Fake {
            fail,
            ..Default::default()
        };
        assert_eq!(
            inspect(&b, DOC, &request(json!({}))).unwrap_err(),
            "getter failed"
        );
    }
}
#[test]
fn stale_catalog_revision_detects_changes_outside_query() {
    let b = Fake::default();
    let first = inspect(&b, DOC, &request(json!({"query":"missing"}))).unwrap();
    let mut changed = Fake::default();
    changed.materials[1].name = "Steel".into();
    assert_eq!(
        inspect(
            &changed,
            DOC,
            &request(json!({"query":"missing","target":first["target"]}))
        )
        .unwrap_err(),
        "target_revision_mismatch"
    );
}
#[test]
fn query_and_limit_do_not_redefine_catalog_revision() {
    let b = Fake::default();
    let first = inspect(&b, DOC, &request(json!({}))).unwrap();
    let again = inspect(
        &b,
        DOC,
        &request(json!({"query":"ALU","limit":1,"target":first["target"]})),
    )
    .unwrap();
    assert_eq!(first["target"], again["target"]);
}
#[test]
fn revision_binds_document_and_source() {
    let a = inspect(&Fake::default(), DOC, &request(json!({}))).unwrap();
    let b = inspect(
        &Fake::default(),
        DOC,
        &request(json!({"libraryId":"lib-a"})),
    )
    .unwrap();
    assert_ne!(a["target"]["revision"], b["target"]["revision"]);
    assert!(inspect(&Fake::default(), "other", &request(json!({}))).is_err());
}
#[test]
fn duplicate_ids_fail_even_when_names_differ() {
    let mut b = Fake::default();
    b.libraries[1].id = b.libraries[0].id.clone();
    assert_eq!(
        inspect(&b, DOC, &request(json!({}))).unwrap_err(),
        "native_material_duplicate_library_id"
    );
    let mut b = Fake::default();
    b.materials[1].id = b.materials[0].id.clone();
    assert_eq!(
        inspect(&b, DOC, &request(json!({}))).unwrap_err(),
        "native_material_duplicate_material_id"
    );
}
#[test]
fn invalid_record_bytes_refuse() {
    for field in ["id", "name"] {
        for value in ["x\0y".to_owned(), "x".repeat(4097)] {
            let mut b = Fake::default();
            if field == "id" {
                b.materials[0].id = value;
            } else {
                b.materials[0].name = value;
            }
            assert_eq!(
                inspect(&b, DOC, &request(json!({}))).unwrap_err(),
                "native_material_invalid_record"
            );
        }
    }
}
#[test]
fn collection_bounds_refuse_before_item_access() {
    let b = Fake {
        count_override: Some(MAX_MATERIALS + 1),
        ..Default::default()
    };
    assert_eq!(
        inspect(&b, DOC, &request(json!({}))).unwrap_err(),
        "native_material_count_bound"
    );
    let mut b = Fake::default();
    b.libraries
        .resize(MAX_LIBRARIES + 1, b.libraries[0].clone());
    assert_eq!(
        inspect(&b, DOC, &request(json!({}))).unwrap_err(),
        "native_material_library_bound"
    );
    assert_eq!(b.calls.get(), 1);
}
#[test]
fn library_and_selected_source_counts_must_agree() {
    let mut b = Fake::default();
    b.libraries[0].count = 3;
    assert_eq!(
        inspect(&b, DOC, &request(json!({"libraryId":"lib-a"}))).unwrap_err(),
        "native_material_catalog_changed"
    );
}
#[test]
fn arguments_refuse_before_sdk_reads() {
    for extra in [
        json!({"query":"x".repeat(1025)}),
        json!({"libraryId":"x\0y"}),
        json!({"target":{"documentId":DOC,"revision":""}}),
    ] {
        let b = Fake::default();
        assert!(inspect(&b, DOC, &request(extra)).is_err());
        assert_eq!(b.calls.get(), 0);
    }
}
#[test]
fn catalog_and_json_encoding_bounds_are_enforced() {
    let mut b = Fake::default();
    b.materials = (0..400)
        .map(|i| Material {
            id: format!("id{i}"),
            name: "x".repeat(4096),
        })
        .collect();
    assert_eq!(
        inspect(&b, DOC, &request(json!({}))).unwrap_err(),
        "native_material_catalog_bound"
    );
    let mut b = Fake::default();
    b.materials = (0..100)
        .map(|i| Material {
            id: format!("id{i}"),
            name: "\u{1}".repeat(2048),
        })
        .collect();
    assert_eq!(
        inspect(&b, DOC, &request(json!({}))).unwrap_err(),
        "native_material_projection_bound"
    );
}
#[test]
fn maximum_short_record_catalog_is_complete_before_query_limit() {
    let mut b = Fake::default();
    b.materials = (0..MAX_MATERIALS)
        .map(|i| Material {
            id: format!("i{i}"),
            name: "A".into(),
        })
        .collect();
    b.libraries = (0..MAX_LIBRARIES)
        .map(|i| Library {
            id: format!("l{i}"),
            name: "L".into(),
            count: MAX_MATERIALS,
        })
        .collect();
    let v = inspect(&b, DOC, &request(json!({"limit":1000,"libraryId":"l63"}))).unwrap();
    assert_eq!(v["matchingCount"], MAX_MATERIALS);
    assert_eq!(v["libraries"].as_array().unwrap().len(), MAX_LIBRARIES);
    assert_eq!(v["materials"].as_array().unwrap().len(), 1000);
    assert_eq!(v["complete"], false);
}

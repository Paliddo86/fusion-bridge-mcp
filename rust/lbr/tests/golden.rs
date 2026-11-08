//! Every fixture's artifact names must equal what the Python bridge's
//! `_library_artifact_names` produced for the same file (tests/golden/<name>.json).
use std::fs;

#[test]
fn artifact_names_match_the_python_bridge() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let mut checked = 0;
    for e in fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("lbrx") { continue; }
        // syn_*.lbr are deliberately broken preflight fixtures
        if p.file_name().unwrap().to_string_lossy().starts_with("syn_") { continue; }
        let xml = fs::read_to_string(&p).unwrap();
        let got = fusion_lbr::artifact_names(&xml).unwrap();
        let golden_path = format!("{}/tests/golden/{}.json", env!("CARGO_MANIFEST_DIR"), p.file_stem().unwrap().to_str().unwrap());
        let want: fusion_lbr::ArtifactNames = serde_json::from_str(&fs::read_to_string(&golden_path).unwrap()).unwrap();
        assert_eq!(got, want, "artifact names differ from the Python golden for {}", p.display());
        checked += 1;
    }
    assert!(checked >= 6, "expected the Demo Board merged library plus five parts, checked {checked}");
}

#[test]
fn a_non_library_is_refused_not_guessed() {
    assert!(fusion_lbr::artifact_names("<html></html>").is_err());
    assert!(fusion_lbr::artifact_names("not xml").is_err());
}

#[test]
fn binding_injection_matches_python_and_accumulates() {
    let root = env!("CARGO_MANIFEST_DIR");
    let src = fs::read_to_string(format!("{root}/tests/fixtures/Demo_Board.lbrx")).unwrap();
    let plan: serde_json::Value = serde_json::from_str(&fs::read_to_string(format!("{root}/tests/golden/inject_bindings.json")).unwrap()).unwrap();
    let parse = |k: &str| -> Vec<fusion_lbr::Binding> { serde_json::from_value(plan[k].clone()).unwrap() };
    let first = fusion_lbr::inject_package3d_bindings(&src, &parse("first"));
    assert_eq!(first, fs::read_to_string(format!("{root}/tests/golden/Demo_Board.bound3.lbrx")).unwrap(), "first injection differs from the Python golden");
    let second = fusion_lbr::inject_package3d_bindings(&first, &parse("second"));
    assert_eq!(second, fs::read_to_string(format!("{root}/tests/golden/Demo_Board.bound5.lbrx")).unwrap(), "subset re-injection must accumulate exactly as Python does");
    // Python is not byte-idempotent (each re-run leaves one more blank line before <symbols>);
    // the port reproduces that exactly rather than "fixing" the oracle.
    let again = fusion_lbr::inject_package3d_bindings(&second, &parse("second"));
    assert_eq!(again, fs::read_to_string(format!("{root}/tests/golden/Demo_Board.bound5again.lbrx")).unwrap(), "re-injection differs from the Python golden");
    assert_eq!(again.replace('\n', ""), second.replace('\n', ""), "re-injection must be idempotent up to whitespace");
    assert_eq!(fusion_lbr::artifact_names(&second).unwrap().p3d.len(), 5);
}

#[test]
fn metadata_matches_the_python_bridge() {
    let root = env!("CARGO_MANIFEST_DIR");
    let xml = fs::read_to_string(format!("{root}/tests/fixtures/Demo_Board.lbrx")).unwrap();
    let got = fusion_lbr::library_part_metadata(&xml).unwrap();
    let want: fusion_lbr::LibraryMetadata = serde_json::from_str(&fs::read_to_string(format!("{root}/tests/golden/Demo_Board.metadata.json")).unwrap()).unwrap();
    assert_eq!(got, want);
}

#[test]
fn pairing_matches_the_python_bridge() {
    let root = env!("CARGO_MANIFEST_DIR");
    let cases: serde_json::Value = serde_json::from_str(&fs::read_to_string(format!("{root}/tests/golden/pair_parts.json")).unwrap()).unwrap();
    for name in ["demo_board", "variants", "only_one_of_each"] {
        let c = &cases[name];
        let md: fusion_lbr::LibraryMetadata = serde_json::from_value(c["metadata"].clone()).unwrap();
        let models: Vec<String> = serde_json::from_value(c["models"].clone()).unwrap();
        let got = fusion_lbr::pair_parts(c["lbrPath"].as_str().unwrap(), &models, &md);
        let want: fusion_lbr::Pairing = serde_json::from_value(c["expected"].clone()).unwrap();
        assert_eq!(got, want, "pairing differs from the Python golden in case {name}");
    }
    for n in cases["names"]["ok"].as_array().unwrap() { assert!(fusion_lbr::library_name_ok(n.as_str().unwrap()), "{n} should be ok"); }
    for n in cases["names"]["bad"].as_array().unwrap() { assert!(!fusion_lbr::library_name_ok(n.as_str().unwrap()), "{n} should be refused"); }
    for (k, v) in cases["names"]["norm"].as_object().unwrap() { assert_eq!(fusion_lbr::norm(k), v.as_str().unwrap()); }
}

#[test]
fn split_per_part_matches_python_byte_for_byte() {
    let root = env!("CARGO_MANIFEST_DIR");
    let xml = fs::read_to_string(format!("{root}/tests/fixtures/Demo_Board.lbrx")).unwrap();
    let out = std::env::temp_dir().join(format!("fusion-lbr-split-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out);
    let got = fusion_lbr::split_per_part(&xml, &out).unwrap();
    let index: serde_json::Value = serde_json::from_str(&fs::read_to_string(format!("{root}/tests/golden/split/index.json")).unwrap()).unwrap();
    assert_eq!(got.len(), index.as_object().unwrap().len(), "entry count");
    for (key, want) in index.as_object().unwrap() {
        let e = got.get(key).unwrap_or_else(|| panic!("missing entry {key}"));
        let file = want["file"].as_str().unwrap();
        assert!(e.lbr_path.ends_with(file.trim_end_matches('x')), "{key}: path {} should end with {file}", e.lbr_path);
        assert_eq!(e.package, want["package"].as_str().unwrap(), "{key}: package");
        assert_eq!(serde_json::to_value(&e.symbols).unwrap(), want["symbols"], "{key}: symbols");
        assert_eq!(serde_json::to_value(&e.packages).unwrap(), want["packages"], "{key}: packages");
        let mine = fs::read_to_string(&e.lbr_path).unwrap();
        let golden = fs::read_to_string(format!("{root}/tests/golden/split/{file}")).unwrap();
        if mine != golden {
            let (a, b): (Vec<&str>, Vec<&str>) = (mine.lines().collect(), golden.lines().collect());
            let first = a.iter().zip(b.iter()).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
            panic!("{file} differs from the Python golden at line {}:\n  rust:   {:?}\n  python: {:?}", first + 1, a.get(first), b.get(first));
        }
    }
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn bundle_staging_matches_python() {
    let root = env!("CARGO_MANIFEST_DIR");
    let want: serde_json::Value = serde_json::from_str(&fs::read_to_string(format!("{root}/tests/golden/bundle_files.json")).unwrap()).unwrap();
    let tmp = std::env::temp_dir().join(format!("fusion-lbr-bundle-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let n = fusion_lbr::safe_extract(std::path::Path::new(&format!("{root}/tests/fixtures/Demo_Board.bundle.zip")), &tmp).unwrap();
    assert_eq!(n as u64, want["extracted"].as_u64().unwrap());
    let found = fusion_lbr::find_bundle_files(&tmp);
    let rel = |p: &str| p.trim_start_matches(&tmp.to_string_lossy().to_string()).trim_start_matches(std::path::MAIN_SEPARATOR).replace('\\', "/");
    assert_eq!(rel(found.lbr.as_deref().unwrap()), want["lbr"].as_str().unwrap());
    let models: Vec<String> = found.models.iter().map(|m| rel(m)).collect();
    assert_eq!(serde_json::to_value(&models).unwrap(), want["models"]);
    let dest = tmp.join("placed");
    let (new_lbr, map) = fusion_lbr::place_library(std::path::Path::new(found.lbr.as_deref().unwrap()), &found.models, &dest).unwrap();
    assert!(new_lbr.ends_with("/placed/Demo_Board.lbr"), "{new_lbr}");
    assert_eq!(map.len(), found.models.len());
    assert!(map.values().all(|v| std::path::Path::new(v).is_file()));
    let slip = fusion_lbr::safe_extract(std::path::Path::new(&format!("{root}/tests/fixtures/zipslip.zip")), &tmp.join("slip"));
    assert!(matches!(slip, Err(fusion_lbr::BundleError::UnsafeEntry(_))), "zip-slip must be refused: {slip:?}");
    assert!(!tmp.join("evil.lbr").exists());
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn preflight_and_lints_match_the_python_bridge() {
    let root = env!("CARGO_MANIFEST_DIR");
    let g: serde_json::Value = serde_json::from_str(&fs::read_to_string(format!("{root}/tests/golden/preflight_and_parts.json")).unwrap()).unwrap();
    let mut checked = 0;
    for (name, case) in g.as_object().unwrap() {
        if name == "__missing__" { continue; }
        let path = std::path::PathBuf::from(format!("{root}/tests/fixtures/{name}"));
        let pf = fusion_lbr::preflight_lbr(&path);
        let want = &case["preflight"];
        assert_eq!(pf.ok, want["ok"].as_bool().unwrap(), "{name}: preflight ok");
        let want_checks = want["checks"].as_array().unwrap();
        assert_eq!(pf.checks.len(), want_checks.len(), "{name}: check count {:?}", pf.checks);
        for (mine, theirs) in pf.checks.iter().zip(want_checks) {
            assert_eq!(mine.check, theirs["check"].as_str().unwrap(), "{name}: check order");
            assert_eq!(mine.ok, theirs["ok"].as_bool().unwrap(), "{name}: {} ok", mine.check);
            // parser error strings legitimately differ between roxmltree and ElementTree
            if mine.ok || mine.check != "xml-parses" { assert_eq!(mine.detail, theirs["detail"].as_str().unwrap(), "{name}: {} detail", mine.check); }
        }
        assert_eq!(pf.metadata_ok, want["metadataOk"].as_bool(), "{name}: metadataOk");
        assert_eq!(pf.metadata_hint.as_deref(), want["_metadataHint"].as_str(), "{name}: metadata hint");
        if let Some(mw) = &pf.metadata_warnings {
            assert_eq!(serde_json::to_value(mw).unwrap(), want["metadataWarnings"], "{name}: preflight metadata warnings");
        }
        // the standalone lint
        if let Ok(xml) = fs::read_to_string(&path) {
            let lint = fusion_lbr::lint_part_metadata(&xml);
            let want_l = &case["lint"];
            assert_eq!(lint.ok, want_l["ok"].as_bool().unwrap(), "{name}: lint ok");
            if want_l["error"].is_null() {
                assert_eq!(serde_json::to_value(&lint.warnings).unwrap(), want_l["warnings"], "{name}: lint warnings");
                assert_eq!(serde_json::to_value(&lint.totals).unwrap(), want_l["totals"], "{name}: lint totals");
            }
        }
        checked += 1;
    }
    assert!(checked >= 10, "checked {checked}");
    let missing = fusion_lbr::preflight_lbr(std::path::Path::new("C:/nope/missing.lbr"));
    assert!(!missing.ok && missing.checks.len() == 1 && missing.checks[0].check == "file-readable");
}

#[test]
fn read_footprint_from_a_library_and_from_a_board() {
    let lbr = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/Demo_Board.bound3.lbr")).unwrap();
    let fp = fusion_lbr::read_footprint(&lbr, None, "QFN-48_7x7mm").unwrap();
    let pads = fp["pads"].as_array().unwrap();
    assert!(pads.len() >= 48, "{}", pads.len());
    assert_eq!(fp["attributes"]["mount"], "smd");
    assert_eq!(fp["models"][0]["name"], "QFN-48_7x7mm");
    let c = fp["courtyardMm"].as_array().unwrap();
    assert!(c[0].as_f64().unwrap() < c[2].as_f64().unwrap() && c[1].as_f64().unwrap() < c[3].as_f64().unwrap());
    assert!(fusion_lbr::read_footprint(&lbr, None, "NOPE").is_err());
    // a board embeds its libraries by name: Lib:Name picks the one the part comes from
    let brd = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../demo/routing/fixture/fusion-routing.brd")).unwrap();
    let doc = roxml_first_package(&brd);
    let fp = fusion_lbr::read_footprint(&brd, Some(&doc.0), &doc.1).unwrap();
    assert!(!fp["pads"].as_array().unwrap().is_empty());
    assert!(fusion_lbr::read_footprint(&brd, Some("no-such-lib"), &doc.1).is_err());
}

fn roxml_first_package(xml: &str) -> (String, String) {
    // the first <element library= package=> on the board
    let i = xml.find("<element ").unwrap();
    let tag = &xml[i..i + xml[i..].find('>').unwrap()];
    let attr = |k: &str| { let j = tag.find(&format!(" {k}=\"")).unwrap() + k.len() + 3; tag[j..j + tag[j..].find('"').unwrap()].to_string() };
    (attr("library"), attr("package"))
}

#[test]
fn edit_footprint_pads_graphics_and_connects() {
    let lbr = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/Demo_Board.bound3.lbr")).unwrap();
    let edits = serde_json::json!({
        "pads": [{"number": "1", "set": {"sizeMm": [0.3, 0.9], "newNumber": "A1"}}, {"number": "2", "delete": true}],
        "addPads": [{"number": "EP2", "type": "smd", "at": [0, 0], "sizeMm": [1, 1]}],
        "graphics": [{"layer": "F.SilkS", "mode": "append", "widthMm": 0.15, "shapes": [{"type": "line", "start": [0, 0], "end": [1, 1]}, {"type": "circle", "center": [0, 0], "radius": 0.2}]}],
        "metadata": {"description": "edited"}});
    let (text, summary) = fusion_lbr::edit_footprint(&lbr, "QFN-48_7x7mm", &edits).unwrap();
    let fp = fusion_lbr::read_footprint(&text, None, "QFN-48_7x7mm").unwrap();
    let pads = fp["pads"].as_array().unwrap();
    let a1 = pads.iter().find(|p| p["number"] == "A1").unwrap();
    assert_eq!(serde_json::json!([0.3, 0.9]), a1["sizeMm"]);
    assert!(pads.iter().all(|p| p["number"] != "2" && p["number"] != "1"));
    assert!(pads.iter().any(|p| p["number"] == "EP2"));
    assert_eq!(fp["description"], "edited");
    assert!(!summary["connectsDropped"].as_array().unwrap().is_empty());
    // the device's connect for old pad 1 now names A1
    assert!(text.contains("pad=\"A1\""));
    assert!(fusion_lbr::edit_footprint(&lbr, "QFN-48_7x7mm", &serde_json::json!({"models": {"mode": "delete"}})).is_err());
    assert!(fusion_lbr::edit_footprint(&lbr, "QFN-48_7x7mm", &serde_json::json!({"renumber": {"map": {"3": "4"}}})).is_err());
}

#[test]
fn authoring_builds_a_part_fusion_will_open() {
    use fusion_lbr::authoring as a;
    let pre = |t: &str| { let p = std::env::temp_dir().join(format!("adom-auth-{}.lbr", std::process::id())); std::fs::write(&p, t).unwrap(); let r = fusion_lbr::authoring::preflight_text(&p); let _ = std::fs::remove_file(&p); r };
    // an SOIC-8 footprint in a new library (pads in mil, the standard's string form)
    let pads = "1,-108,75,60,24,rect,0;2,-108,25,60,24,rect,0;3,-108,-25,60,24,rect,0;4,-108,-75,60,24,rect,0;5,108,-75,60,24,rect,0;6,108,-25,60,24,rect,0;7,108,25,60,24,rect,0;8,108,75,60,24,rect,0";
    let (t, s) = a::create_footprint(None, &serde_json::json!({"name": "SOIC-8", "pads": pads})).unwrap();
    assert_eq!(s["libraryState"], "created");
    assert!(pre(&t).0, "{:?}", pre(&t).1);
    // a dual op-amp: parts A and B, supply pins on a power gate
    let pins = "1,OUTA,right,100,1;2,-INA,left,100,1;3,+INA,left,200,1;7,OUTB,right,100,2;6,-INB,left,100,2;5,+INB,left,200,2;8,V+,left,100,0;4,V-,left,200,0";
    let (t, s) = a::create_symbol(Some(&t), &serde_json::json!({"name": "LM358", "pins": pins, "designator": "U"})).unwrap();
    assert_eq!(s["symbols"][0]["parts"], 2);
    assert!(pre(&t).0, "a symbol not yet on a footprint is fine mid-build: {:?}", pre(&t).1);
    // join pins to pads and set parameters
    let (t, s) = a::set_params(&t, Some("LM358"), &serde_json::json!({"MPN": "LM358DR", "Manufacturer": "TI"}), Some("LM358"), Some("SOIC-8")).unwrap();
    assert_eq!(s["footprint"], "SOIC-8");
    assert!(pre(&t).0, "{:?}", pre(&t).1);
    assert!(fusion_lbr::preflight_lbr(&{ let p = std::env::temp_dir().join("adom-auth-full.lbr"); std::fs::write(&p, &t).unwrap(); p }).ok, "a complete part passes the full preflight");
    let r = a::read_symbol(&t, "lm358").unwrap();
    assert_eq!(r["pinsMode0"], 8);
    assert_eq!(r["models"][0]["name"], "SOIC-8");
    let outa = r["pins"].as_array().unwrap().iter().find(|p| p["name"] == "OUTA").unwrap();
    assert_eq!(outa["des"], "1");
    assert!(r["params"].as_array().unwrap().iter().any(|p| p["name"] == "MPN" && p["value"] == "LM358DR"));
    // the library tools
    let (t, _) = a::copy_footprint(&t, None, "SOIC-8", "SOIC-8-COPY").unwrap();
    let (t, s) = a::rename_footprint(&t, "SOIC-8", "SOIC127P600X175-8").unwrap();
    assert_eq!(s["devicesUpdated"], 1);
    assert!(a::remove_footprint(&t, "SOIC127P600X175-8").is_err(), "a used footprint must be refused");
    let (t, _) = a::remove_footprint(&t, "SOIC-8-COPY").unwrap();
    let (t, _) = a::clone_symbol(&t, "LM358", "LM2904").unwrap();
    let (t, _) = a::rename_symbol(&t, "LM2904", "LM2904B").unwrap();
    let f = a::find_symbol(&[("lib".into(), t.clone())], "lm", false);
    assert_eq!(f["matches"].as_array().unwrap().len(), 2);
    let (t, s) = a::remove_symbol(&t, "LM2904B").unwrap();
    assert!(!s["symbolsRemoved"].as_array().unwrap().is_empty());
    assert!(a::read_symbol(&t, "LM358").is_ok());
    // xcopy into a second new library
    let (t2, _) = a::copy_footprint(&a::new_library("dest"), Some(&t), "SOIC127P600X175-8", "SO8").unwrap();
    assert!(pre(&t2).0);
    assert!(pre(&t).0, "{:?}", pre(&t).1);
}

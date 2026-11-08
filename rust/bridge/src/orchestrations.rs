//! Bridge-level orchestrations, native: `open_lbr` first (the most used one), with the
//! part-metadata report, the open-library memory and the deterministic bridge-doc close it
//! leans on. Each Fusion hop goes through the add-in pass-through, so the etiquette, gates
//! and dialog handling are the same as for a direct verb.

use serde_json::{json, Value};
use std::time::{Duration, Instant};

use crate::addin::Ctx;

fn now_f() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

/// `_part_metadata_report`: the short, user-facing ordering report an import carries up the chain.
pub fn part_metadata_report(path: &str) -> Value {
    let Ok(xml) = std::fs::read_to_string(path) else { return json!({"ok": Value::Null, "unavailable": "could not read the library"}) };
    let md = match fusion_lbr::library_part_metadata(&xml) { Ok(m) if m.ok => m, Ok(_) => return json!({"ok": Value::Null, "unavailable": "could not read the library"}), Err(e) => return json!({"ok": Value::Null, "unavailable": e.to_string()}) };
    let lint = fusion_lbr::lint_part_metadata(&xml);
    let dist = ["MOUSER_PN", "DIGIKEY_PN", "LCSC_PN"];
    let fields = ["MPN", "MANUFACTURER", "MOUSER_PN", "DIGIKEY_PN", "LCSC_PN"];
    let mut comps: Vec<Value> = Vec::new();
    let mut missing: std::collections::BTreeMap<&str, i64> = fields.iter().map(|f| (*f, 0)).collect();
    let mut n_no_dist = 0i64;
    for ds in &md.devicesets {
        let absent: Vec<&str> = fields.iter().filter(|f| !ds.variants.iter().any(|v| v.attributes.get(**f).map(|s| !s.is_empty()).unwrap_or(false))).cloned().collect();
        for f in &absent { *missing.get_mut(f).unwrap() += 1; }
        let mut blocking: Vec<String> = ["MPN", "MANUFACTURER"].iter().filter(|f| absent.contains(f)).map(|s| s.to_string()).collect();
        let absent_dist: Vec<&str> = dist.iter().filter(|d| absent.contains(d)).cloned().collect();
        if absent_dist.len() == dist.len() { blocking.push("a distributor part number".into()); n_no_dist += 1; }
        if !blocking.is_empty() || !absent_dist.is_empty() {
            comps.push(json!({"component": ds.name, "variants": ds.variants.len(), "missing": blocking, "absentDistributors": absent_dist, "variantNames": ds.variants.iter().map(|v| v.full_name.clone()).collect::<Vec<_>>()}));
        }
    }
    let total = md.devicesets.len() as i64;
    let label = |f: &str| match f { "MPN" => "no manufacturer part number", "MANUFACTURER" => "no manufacturer", "MOUSER_PN" => "no Mouser part number", "DIGIKEY_PN" => "no DigiKey part number", _ => "no LCSC part number" };
    let n_no_desc = md.devicesets.iter().filter(|d| d.description.trim().is_empty()).count() as i64;
    for ds in &md.devicesets {
        if ds.description.trim().is_empty() {
            if let Some(hit) = comps.iter_mut().find(|c| c["component"].as_str() == Some(&ds.name)) { hit["missing"].as_array_mut().unwrap().push(json!("a description")); }
            else { comps.push(json!({"component": ds.name, "variants": ds.variants.len(), "missing": ["a description"], "absentDistributors": [], "variantNames": ds.variants.iter().map(|v| v.full_name.clone()).collect::<Vec<_>>()})); }
        }
    }
    let n_phrase = |n: i64, what: &str| format!("{n} of {total} component{} ha{} {what}", if total == 1 { "" } else { "s" }, if n == 1 { "s" } else { "ve" });
    let mut bits: Vec<String> = ["MPN", "MANUFACTURER"].iter().filter(|f| missing[*f] > 0).map(|f| n_phrase(missing[f], label(f))).collect();
    if n_no_desc > 0 { bits.push(n_phrase(n_no_desc, "no description")); }
    if n_no_dist > 0 { bits.push(n_phrase(n_no_dist, "no distributor part number at all")); }
    let mut mc: serde_json::Map<String, Value> = missing.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
    mc.insert("DESCRIPTION".into(), json!(n_no_desc));
    let dist_note: Vec<String> = dist.iter().filter(|d| missing[*d] > 0).map(|d| n_phrase(missing[d], label(d))).collect();
    json!({"ok": lint.ok, "componentsTotal": total, "variantsTotal": md.totals.variants, "missingCounts": mc, "componentsWithoutAnyDistributor": n_no_dist,
        "distributorCoverage": dist.iter().map(|d| (d.to_string(), json!(total - missing[d]))).collect::<serde_json::Map<_, _>>(),
        "components": comps.into_iter().take(40).collect::<Vec<_>>(), "warnings": lint.warnings,
        "summary": if bits.is_empty() { format!("All {total} component(s) carry a manufacturer part number and at least one distributor part number.") } else { format!("{}. The library imported fine - this is about ordering, not geometry.", bits.join("; ")) },
        "distributorNote": if dist_note.is_empty() { Value::Null } else { json!(dist_note.join("; ")) },
        "userAction": if bits.is_empty() { Value::Null } else { json!("Add the missing part numbers where the component metadata lives (upstream of this bridge), then re-export.") }})
}

/// `_remember_open_library`: persisted so a later capture can pre-check names without a path.
pub fn remember_open_library(state_dir: &std::path::Path, lbr: &str) {
    let _ = std::fs::create_dir_all(state_dir);
    let _ = std::fs::write(state_dir.join("open_library.json"), json!({"libraryPath": lbr.replace('\\', "/"), "at": now_f()}).to_string());
}

/// `_close_bridge_doc_verified`: close a bridge-owned scratch document and VERIFY it is gone,
/// answering a Save prompt inside the same call; never over an in-flight Hub upload.
pub fn close_bridge_doc_verified(ctx: &Ctx, name: &str, timeout_s: u64, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let n = name.trim_end_matches('*').trim().to_string();
    if n.is_empty() { return json!({"closed": false, "why": "no name"}); }
    let jobs_deadline = Instant::now() + Duration::from_secs(120);
    let mut drained = false;
    while Instant::now() < jobs_deadline {
        let jr = pass("run_modeling_script", json!({"script": "import adsk.core\nresult={'jobs': adsk.core.Application.get().hasActiveJobs}"}));
        if jr["success"].as_bool() != Some(true) || jr["data"]["result"]["jobs"].as_bool() != Some(true) { drained = true; break; }
        eprintln!("[fusion-bridge] close({n:?}): Fusion Hub upload in flight - waiting, not closing (a close now would raise the lose-changes modal)");
        std::thread::sleep(Duration::from_secs(5));
    }
    if !drained { return json!({"closed": false, "deferred": true, "why": "Fusion Hub uploads still active after 120s - close deferred so the upload is not destroyed; retry after the jobs drain"}); }
    let _ = pass("close_document", json!({"name": n, "save": false, "saveToClose": true}));
    let deadline = Instant::now() + Duration::from_secs(timeout_s);
    while Instant::now() < deadline {
        let di = pass("document_info", json!({}));
        if di["success"].as_bool() == Some(true) {
            let docs: Vec<String> = di["data"]["openDocuments"].as_array().map(|a| a.iter().filter_map(|d| d["name"].as_str()).map(|s| s.trim_end_matches('*').trim().to_lowercase()).collect()).unwrap_or_default();
            if !docs.contains(&n.to_lowercase()) { return json!({"closed": true}); }
        }
        let got = crate::etiquette::dismiss_save_prompt_if_bridge_doc(&ctx.state_dir, &ctx.caller, &ctx.pids, "");
        if got["handled"].as_bool() == Some(true) { eprintln!("[fusion-bridge] close({n:?}): answered the Save prompt inside the same call (deterministic close, never left on screen)"); }
        std::thread::sleep(Duration::from_secs(2));
    }
    json!({"closed": false, "why": format!("document still open after {timeout_s}s")})
}

/// `fusion_open_lbr`.
pub fn open_lbr(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    { let stem = args["filePath"].as_str().unwrap_or("").replace('\\', "/").rsplit('/').next().unwrap_or("").trim_end_matches(".lbr").to_string(); crate::palette::say(&format!("Opening library {stem}"), "Fusion's Electronics Library editor, in the background", None, crate::palette::eta_for_verb(&ctx.state_dir, "open_lbr")); }
    let file_path = ["filePath", "lbrPath", "libraryPath", "path"].iter().find_map(|k| args[k].as_str()).unwrap_or("").to_string();
    if file_path.trim().is_empty() {
        let mut received: Vec<String> = args.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default(); received.sort();
        return json!({"success": false, "errorCode": "no_path_argument", "error": "No library path was given, so nothing was opened. This verb accepts filePath, lbrPath, libraryPath or path - the file itself was never examined, so do NOT re-stage it.",
            "acceptedArgs": ["filePath", "lbrPath", "libraryPath", "path"], "receivedArgs": received, "_hint": "Re-call with a Windows-local path, e.g. fusion_open_lbr {\"lbrPath\": \"C:/tmp/rayparts/tja1050.lbr\"}."});
    }
    let pf = fusion_lbr::preflight_lbr(std::path::Path::new(&file_path));
    let hard_ok = ["file-readable", "size-sane", "xml-parses", "root-is-eagle", "package3d-refs-resolve"].iter().all(|k| pf.checks.iter().find(|c| c.check == *k).map(|c| c.ok).unwrap_or(true));
    let file_path = file_path.replace('\\', "/");
    if !hard_ok {
        let sz = std::fs::metadata(&file_path).ok().map(|m| m.len());
        let head: Vec<u8> = std::fs::read(&file_path).ok().map(|b| b.into_iter().take(32).collect()).unwrap_or_default();
        return json!({"success": false, "errorCode": "lbr_invalid", "preflight": pf,
            "error": format!("{file_path} will not open in Fusion: {} - refusing to hand it over (the only possible outcome is a blocking Error modal plus an orphaned empty design).", pf.errors.join("; ").chars().take(240).collect::<String>()),
            "fileSize": sz, "fileHead": py_repr_bytes(&head),
            "_hint": "The file on THIS box is bad. Re-stage it from the source and compare hashes - a transfer that truncates or re-encodes is the usual culprit. fusion_preflight_lbr re-runs these checks in isolation. Nothing was opened and no dialog was raised."});
    }
    let stem = file_path.rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_default();
    let di = pass("document_info", json!({}));
    for d in di["data"]["openDocuments"].as_array().cloned().unwrap_or_default() {
        let n0 = d["name"].as_str().unwrap_or("").trim_end_matches('*').trim().to_string();
        if doc_matches(&n0, &stem) { let _ = close_bridge_doc_verified(ctx, &n0, 30, pass); }
    }
    let symbol_name = args["symbolName"].as_str().unwrap_or("").to_string();
    let verify = args["verify"].as_bool().unwrap_or(false);
    crate::chains::note_bridge_doc_pub(&ctx.state_dir, &file_path);
    let mut results: Vec<String> = Vec::new();
    let fusion_path = match crate::fusionpath::for_fusion(&ctx.state_dir, &file_path) { Ok(p) => p, Err(e) => return json!({"success": false, "errorCode": "stage_failed", "error": e}) };
    let open_result = pass("execute_text_command", json!({"command": format!("Document.newDesignFromLocal {}", fusion_path)}));
    if open_result["success"].as_bool() != Some(true) {
        let expected = stem.to_lowercase();
        let mut opened_anyway = false;
        for _ in 0..20 {
            std::thread::sleep(Duration::from_secs(3));
            let st = pass("get_app_state", json!({}));
            let active = st["data"]["activeDocument"].as_str().unwrap_or("").to_lowercase();
            if !expected.is_empty() && doc_matches(&active, &expected) { opened_anyway = true; break; }
        }
        if !opened_anyway {
            return json!({"success": false, "error": format!("Document.newDesignFromLocal failed: {}", open_result["error"].as_str().unwrap_or("unknown")),
                "_hint": "The open did not complete AND the document never became active. On slow/software-rendered machines opens can take 60s+; this call already waited + verified by state. Check fusion_check_dialogs for a blocking modal, then retry.", "data": {"filePath": file_path}});
        }
        results.push("Document.newDesignFromLocal: ok (verified by app state after slow open)".into());
    } else { results.push("Document.newDesignFromLocal: ok".into()); }
    if !symbol_name.is_empty() {
        std::thread::sleep(Duration::from_secs(3));
        let sym_ref = if symbol_name.ends_with(".sym") { symbol_name.clone() } else { format!("{symbol_name}.sym") };
        let er = pass("electron_run", json!({"command": format!("EDIT {sym_ref}")}));
        results.push(format!("EDIT {sym_ref}: success={}", if er["success"].as_bool() == Some(true) { "True" } else { "False" }));
        let _ = pass("electron_run", json!({"command": "WINDOW FIT"}));
        results.push("WINDOW FIT: ok".into());
    }
    let mut verification = Value::Null;
    if verify {
        std::thread::sleep(Duration::from_secs(2));
        let export_path = std::env::temp_dir().join(format!("_adom_verify_{:08x}.scr", (now_f() * 1000.0) as u64 & 0xffff_ffff)).to_string_lossy().replace('\\', "/");
        let ex = pass("export_lbr", json!({"outputPath": export_path}));
        if ex["success"].as_bool() == Some(true) {
            let preview = ex["data"]["preview"].as_str().unwrap_or("").to_string();
            let has_symbol = !symbol_name.is_empty() && (preview.contains(&format!("'{}.sym'", symbol_name.to_uppercase())) || preview.contains(&format!("'{symbol_name}.sym'")));
            verification = json!({"exported": true, "fileSize": ex["data"]["fileSize"].as_i64().unwrap_or(0), "hasSymbol": has_symbol, "preview": preview.chars().take(500).collect::<String>()});
        } else { verification = json!({"exported": false, "error": ex["error"].as_str().unwrap_or("unknown")}); }
        results.push(format!("Verification: exported={}", if verification["exported"].as_bool() == Some(true) { "True" } else { "False" }));
    }
    remember_open_library(&ctx.state_dir, &file_path);
    let pm = part_metadata_report(&file_path);
    let mut response = json!({"success": true, "output": format!("Opened library: {}", file_path.rsplit('/').next().unwrap_or("")), "partMetadata": pm, "data": {"results": results, "filePath": file_path},
        "_hint": "Library opened in the Electronics Library editor (Content Manager). ALWAYS VERIFY VIA SCREENSHOT: a .lbr can FAIL to open ('<file>.lbr has errors and cannot be opened') while this call still returns success - that error is an OWNED POPUP. Grab desktop_screenshot_window on the Fusion main hwnd and CHECK ownedPopupCount + read the _screenshots[] array (ab v1.8.177+ captures owned dialogs invisible to a plain capture); ownedPopupCount>0 means an error/confirm dialog is up. NOTE: an adom-lbr .lbr is 2D ONLY - symbol + footprint + a PLACEHOLDER 3D package. To attach the real 3D chip, use fusion_attach_3d_package (it runs the Package3D generator + FINISH, which binds the 3D onto the deviceset). See the 'fusion-libraries' skill / LIBRARY_FINDINGS.md."});
    if !symbol_name.is_empty() { response["data"]["symbolName"] = json!(symbol_name); }
    if !verification.is_null() { response["data"]["verification"] = verification; }
    let (shots, _) = crate::lifecycle::post_open_screenshot(ctx, 2.0);
    response["data"]["postOpenScreenshot"] = shots;
    // Fusion reports a refused library only through its Error modal: the command itself succeeds
    // and leaves an empty design. Without this, every caller reads the refusal as a good open.
    crate::small_verbs::apply_failure_dialogs(response, &ctx.caller, &ctx.pids)
}

/// Python's `repr(bytes)`.
fn py_repr_bytes(b: &[u8]) -> String {
    let mut out = String::from("b'");
    for &c in b {
        match c { b'\\' => out.push_str("\\\\"), b'\'' => out.push_str("\\'"), b'\n' => out.push_str("\\n"), b'\r' => out.push_str("\\r"), b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(c as char), _ => out.push_str(&format!("\\x{c:02x}")) }
    }
    out.push('\''); out
}

#[cfg(test)]
mod tests {
    #[test]
    fn ratio_matches_difflib_on_known_pairs() {
        // difflib: ratio("LAN7800","LAN7800A")=0.9333, ratio("abcd","bcde")=0.75, ratio("xyz","abc")=0.0
        assert!((super::seq_ratio("LAN7800", "LAN7800A") - 0.9333).abs() < 0.01);
        assert!((super::seq_ratio("abcd", "bcde") - 0.75).abs() < 0.001);
        assert_eq!(super::seq_ratio("xyz", "abc"), 0.0);
        assert_eq!(super::close_matches("LAN780", &["LAN7800".into(), "CH340G".into()], 5, 0.6), vec!["LAN7800".to_string()]);
    }
    #[test]
    fn wrap_skeleton_pinned_to_server_py() {
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../server.py")).unwrap();
        let parts: serde_json::Value = serde_json::from_str(super::WRAP_PARTS).unwrap();
        let grid = parts["_LBR_WRAP_GRID"].as_str().unwrap();
        assert!(src.contains(grid), "lbr_wrap_parts.json drifted from server.py's _LBR_WRAP_GRID");
    }
    #[test]
    fn bytes_repr_matches_python() { assert_eq!(super::py_repr_bytes(b"<?xml\n\x00\xff'"), "b'<?xml\\n\\x00\\xff\\''"); }
}

// ───────────────────────── the show-surface family ─────────────────────────

const WRAP_PARTS: &str = include_str!("lbr_wrap_parts.json");

/// `_artifact_name_from`: the explicit name, else the fragment's name attribute.
pub fn artifact_name_from(args: &Value, frag_keys: &[&str]) -> String {
    let n = args["name"].as_str().unwrap_or("").trim().to_string();
    if !n.is_empty() { return n; }
    for k in frag_keys {
        if let Some(frag) = args[k].as_str() { if let Some(c) = regex::Regex::new(r#"name="([^"]+)""#).unwrap().captures(frag) { return c[1].to_string(); } }
    }
    String::new()
}

/// `_lbr_wrap_build`: a minimal, Fusion-parseable EAGLE library around bare artifacts.
pub fn lbr_wrap_build(name: &str, symbol_xml: Option<&str>, package_xml: Option<&str>) -> String {
    let parts: Value = serde_json::from_str(WRAP_PARTS).unwrap_or(json!({}));
    let mut devicesets = String::new();
    if let Some(_) = symbol_xml {
        let variant = match package_xml {
            Some(p) => { let pkg = regex::Regex::new(r#"<package\s+name="([^"]+)""#).unwrap().captures(p).map(|c| c[1].to_string()).unwrap_or_else(|| name.to_string()); format!(r#"<device name="" package="{pkg}"><technologies><technology name=""/></technologies></device>"#) }
            None => r#"<device name=""/>"#.to_string(),
        };
        devicesets = format!(r#"<deviceset name="{name}" prefix="U"><description>Adom wrapper</description><gates><gate name="G$1" symbol="{name}" x="0" y="0"/></gates><devices>{variant}</devices></deviceset>"#);
    }
    format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE eagle SYSTEM \"eagle.dtd\">\n<eagle version=\"9.6.2\">\n<drawing>\n{}\n{}\n{}\n<library>\n<description>Adom wrapper library: {name}</description>\n<packages>{}</packages>\n<symbols>{}</symbols>\n<devicesets>{devicesets}</devicesets>\n</library>\n</drawing>\n</eagle>\n",
        parts["_LBR_WRAP_SETTINGS"].as_str().unwrap_or(""), parts["_LBR_WRAP_GRID"].as_str().unwrap_or(""), parts["_LBR_WRAP_LAYERS"].as_str().unwrap_or(""), package_xml.unwrap_or(""), symbol_xml.unwrap_or(""))
}

/// `_lbr_wrap_write`: (path, None) or (None, error).
pub fn lbr_wrap_write(state_dir: &std::path::Path, name: &str, symbol_xml: Option<&str>, package_xml: Option<&str>) -> (Option<String>, Option<String>) {
    for (frag, kind, tag) in [(symbol_xml, "symbolXml", "symbol"), (package_xml, "packageXml", "package")] {
        if let Some(f) = frag { if !regex::Regex::new(&format!(r"^\s*<{tag}[\s>]")).unwrap().is_match(f) { return (None, Some(format!("{kind} must be a bare <{tag} ...>...</{tag}> XML fragment (got something else). To open a whole library, pass libraryPath instead."))); } }
    }
    let mut safe: String = regex::Regex::new(r"[^A-Za-z0-9_-]").unwrap().replace_all(name, "_").chars().take(60).collect();
    if safe.is_empty() { safe = "wrapped".into(); }
    let d = std::env::temp_dir().join("adom-fusion-wrap");
    let _ = std::fs::create_dir_all(&d);
    let path = d.join(format!("{safe}.lbr")).to_string_lossy().replace('\\', "/");
    crate::chains::note_bridge_doc_pub(state_dir, &safe);
    match std::fs::write(&path, lbr_wrap_build(name, symbol_xml, package_xml)) { Ok(_) => (Some(path), None), Err(e) => (None, Some(format!("could not write wrapper library: {e}"))) }
}

fn last_library(state_dir: &std::path::Path) -> String { crate::small_verbs::get_pref(state_dir, "lastLibraryPath").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default() }
fn remember_library(state_dir: &std::path::Path, p: &str) { crate::small_verbs::set_pref(state_dir, "lastLibraryPath", json!(p)); }

/// Python's difflib.SequenceMatcher.ratio() over matching blocks (a faithful port of the
/// longest-matching-block recursion without the junk heuristic, which these short names never trigger).
fn seq_ratio(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    fn matches(a: &[char], b: &[char]) -> usize {
        if a.is_empty() || b.is_empty() { return 0; }
        let (mut bi, mut bj, mut bk) = (0usize, 0usize, 0usize);
        for i in 0..a.len() { for j in 0..b.len() { let mut k = 0; while i + k < a.len() && j + k < b.len() && a[i + k] == b[j + k] { k += 1; } if k > bk { bi = i; bj = j; bk = k; } } }
        if bk == 0 { return 0; }
        bk + matches(&a[..bi], &b[..bj]) + matches(&a[bi + bk..], &b[bj + bk..])
    }
    let m = matches(&a, &b); let t = a.len() + b.len();
    if t == 0 { 1.0 } else { 2.0 * m as f64 / t as f64 }
}
fn close_matches(word: &str, have: &[String], n: usize, cutoff: f64) -> Vec<String> {
    let mut scored: Vec<(f64, &String)> = have.iter().map(|h| (seq_ratio(word, h), h)).filter(|(r, _)| *r >= cutoff).collect();
    scored.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap());
    scored.into_iter().take(n).map(|(_, h)| h.clone()).collect()
}

/// `_resolve_artifact_name`: a name that EXISTS in the .lbr, or the refusal, without asking Fusion.
pub fn resolve_artifact_name(name: &str, suffix: &str, lbr_path: &str) -> Result<String, Value> {
    let names = std::fs::read_to_string(lbr_path).ok().and_then(|x| fusion_lbr::artifact_names(&x).ok()).unwrap_or_default();
    let have: Vec<String> = match suffix { "sym" => names.sym.clone(), "pac" => names.pac.clone(), "dev" => names.dev.clone(), _ => vec![] };
    let kind = match suffix { "sym" => "symbol", "pac" => "footprint", "dev" => "device", s => s };
    if !have.is_empty() && !have.iter().any(|h| h == name) && (suffix == "sym" || suffix == "pac") {
        let via = (if suffix == "sym" { &names.dev_sym } else { &names.dev_pac }).get(name).cloned().unwrap_or_default();
        if via.len() == 1 && have.contains(&via[0]) { eprintln!("[fusion-bridge] resolved {name}.{suffix} via its deviceset -> {}", via[0]); return Ok(via[0].clone()); }
    }
    if have.is_empty() {
        return Err(json!({"success": false, "errorCode": "library_unreadable", "error": format!("Cannot read any {suffix} names from {lbr_path} - the library is unparseable or empty, so EDIT was NOT issued (it would certainly raise Fusion's Create-new prompt)."),
            "_hint": "Validate the .lbr first (fusion_open_lbr now refuses garbage with lbr_invalid + the file size/head). Re-stage the file from its source and compare hashes."}));
    }
    if have.iter().any(|h| h == name) { return Ok(name.to_string()); }
    let ci: Vec<&String> = have.iter().filter(|h| h.to_lowercase() == name.to_lowercase()).collect();
    if ci.len() == 1 { return Ok(ci[0].clone()); }
    let close = close_matches(name, &have, 5, 0.6);
    let stem = name.rsplit('/').next().unwrap_or(name).rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_else(|| name.to_string());
    let plen = std::cmp::max(4, stem.len().saturating_sub(2));
    let prefix: String = stem.to_lowercase().chars().take(plen).collect();
    let pref: Vec<&String> = have.iter().filter(|h| h.to_lowercase().starts_with(&prefix)).collect();
    if close.len() == 1 && close[0].to_lowercase() != name.to_lowercase() { eprintln!("[fusion-bridge] resolved near-miss {name:?} -> {:?} (issue #42)", close[0]); return Ok(close[0].clone()); }
    if pref.len() == 1 { eprintln!("[fusion-bridge] resolved by stem {name:?} -> {:?} (issue #42)", pref[0]); return Ok(pref[0].clone()); }
    Err(json!({"success": false, "error": format!("No {kind} named {} in that library, so the EDIT was NOT issued.", fusion_lbr::py_repr_str(name)), "errorCode": "artifact_not_in_library",
        "available": have.iter().take(40).cloned().collect::<Vec<_>>(), "closestMatches": close,
        "_hint": "Deliberately refused BEFORE touching Fusion: EDIT on a missing name pops a 'Create new ...?' modal that blocks the add-in's main thread and makes every later verb look like a crash. Pick a name from `available` (they are the <symbol>/<package>/<deviceset> name attributes in the .lbr, the source of truth) and re-call. Nothing was opened and no dialog was raised."}))
}

fn stem_of(p: &str) -> String { p.replace('\\', "/").rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_else(|| p.to_string()).to_lowercase() }

/// `_fusion_front_doc_title`: the document IN FRONT, from the main window title.
fn front_doc_title(ctx: &Ctx) -> Option<String> {
    let ab = fusion_core::ab::Loopback::from_env()?;
    let facts = fusion_core::windows_facts::facts_from_ab(&ab, &ctx.caller);
    let t = facts.main_title?;
    if !t.to_lowercase().contains("autodesk fusion") { return None; }
    Some(t.to_lowercase().replace(" - autodesk fusion", "").trim().to_string())
}

/// `_ensure_library_active`: the .lbr is the ACTIVE document, verified, activating or reopening as needed.
/// Does a Fusion document title name this file? Fusion titles a document opened through a DOS
/// short path by the 8.3 stem ("0603WA~1" for 0603WAF3300T5E.lbr under a path with spaces), so
/// the long stem and the short form both count (issue #83 verification, ConfRoomROG 2026-09-10).
pub fn doc_matches(active: &str, stem: &str) -> bool {
    let a = active.trim_end_matches('*').trim().to_lowercase();
    let s = stem.trim().to_lowercase();
    if s.is_empty() || a.is_empty() { return false; }
    if a.contains(&s) { return true; }
    let prefix: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).take(6).collect();
    // the short form may be followed by anything (a window title carries "* (Adom) - Autodesk Fusion")
    if let Some(rest) = a.strip_prefix(&prefix) {
        if let Some(after) = rest.strip_prefix('~') { return after.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false); }
    }
    false
}

pub fn ensure_library_active(ctx: &Ctx, lbr_path: &str, timeout_s: u64, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let stem = stem_of(lbr_path);
    let deadline = Instant::now() + Duration::from_secs(timeout_s);
    let (mut last_ws, mut reopened, mut tries, mut reopened2) = (Value::Null, false, 0u32, false);
    while Instant::now() < deadline {
        let st = pass("get_app_state", json!({}));
        let active = st["data"]["activeDocument"].as_str().unwrap_or("").to_string();
        let ws = st["data"]["activeWorkspace"].as_str().unwrap_or("").to_string();
        last_ws = json!(ws);
        let wl = ws.to_lowercase();
        let ws_ok = (wl.contains("librar") || wl.contains("schematic") || wl.contains("board") || wl.contains("pcb")) && !wl.contains("electronics design");
        let ok = st["success"].as_bool() == Some(true);
        if ok && doc_matches(&active, &stem) && ws_ok {
            let front = front_doc_title(ctx);
            if front.as_ref().map(|f| doc_matches(f, &stem)).unwrap_or(true) { return json!({"ok": true, "activeDocument": active, "activeWorkspace": ws, "frontWindow": front}); }
            if !reopened { reopened = true; let _ = close_bridge_doc_verified(ctx, &stem, 30, pass); let _ = open_lbr(ctx, &json!({"lbrPath": lbr_path}), pass); std::thread::sleep(Duration::from_secs(3)); continue; }
        }
        tries += 1;
        if ok && doc_matches(&active, &stem) && !ws_ok && tries >= 4 && !reopened2 { reopened2 = true; let _ = open_lbr(ctx, &json!({"lbrPath": lbr_path}), pass); std::thread::sleep(Duration::from_secs(4)); continue; }
        if ok { let _ = pass("activate_document", json!({"name": stem})); }
        std::thread::sleep(Duration::from_millis(1500));
    }
    json!({"ok": false, "activeDocument": Value::Null, "activeWorkspace": last_ws})
}

pub fn open_doc_names(pass: &dyn Fn(&str, Value) -> Value) -> Vec<String> {
    pass("document_info", json!({}))["data"]["openDocuments"].as_array().map(|a| a.iter().filter_map(|d| d["name"].as_str().map(str::to_string)).filter(|n| !n.is_empty()).collect()).unwrap_or_default()
}

/// `_show_library_view`: EDIT to a library view, fit, background screenshot. Library must be open.
pub fn show_library_view(ctx: &Ctx, name: &str, suffix: &str, lbr_path: Option<&str>, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    { let what = match suffix { "sym" => "symbol", "pac" => "footprint", "dev" => "component view", _ => suffix }; let lib = lbr_path.unwrap_or("").replace('\\', "/").rsplit('/').next().unwrap_or("").trim_end_matches(".lbr").to_string(); crate::palette::say(&format!("{}: {what} of {name}", if lib.is_empty() { "Library".to_string() } else { lib }), &format!("EDIT {name}.{suffix}, then WINDOW FIT"), None, crate::palette::eta_for_verb(&ctx.state_dir, "show_library_view")); }
    let mut name = name.to_string();
    if let Some(lp) = lbr_path {
        match resolve_artifact_name(&name, suffix, lp) { Ok(r) => { if r != name { eprintln!("[fusion-bridge] resolved {name:?} -> {r:?} from the library (avoids a Create-new prompt)"); name = r; } }, Err(e) => return e }
        let mut act = ensure_library_active(ctx, lp, 30, pass);
        if act["ok"].as_bool() != Some(true) {
            std::thread::sleep(Duration::from_secs(8));
            let stem = stem_of(lp);
            let names = open_doc_names(pass);
            let lb: Vec<String> = names.iter().map(|n| regex::Regex::new(r"\s*\([^)]*\)\s*$").unwrap().replace(&n.replace('*', ""), "").trim().to_lowercase()).collect();
            if !names.is_empty() && !lb.contains(&stem) { let _ = open_lbr(ctx, &json!({"filePath": lp}), pass); }
            act = ensure_library_active(ctx, lp, 60, pass);
        }
        if act["ok"].as_bool() != Some(true) {
            return json!({"success": false, "errorCode": "library_not_active", "error": format!("Could not make {} the ACTIVE document (active: {}), so EDIT was not issued - it would have driven the wrong library.", lp.rsplit('/').next().unwrap_or(lp), fusion_lbr::py_repr_str(act["activeDocument"].as_str().unwrap_or("None"))),
                "_hint": "fusion_open_lbr the library, then re-call. In a batch, close the previous part's documents first (the create_library chain now does this itself)."});
        }
    }
    let mut edit = json!({});
    for busy_try in 0..7 {
        edit = pass("electron_run", json!({"command": format!("EDIT {name}.{suffix}"), "confirm": "no"}));
        let etxt = edit["error"].as_str().unwrap_or("").to_string();
        if edit["success"].as_bool() == Some(true) { break; }
        if lbr_path.is_some() && (etxt.to_lowercase().contains("not in electronics editor") || etxt.to_lowercase().contains("current: design")) {
            eprintln!("[fusion-bridge] EDIT {name}.{suffix} hit Design workspace (attempt {}) - re-entering the library editor and retrying", busy_try + 1);
            let _ = ensure_library_active(ctx, lbr_path.unwrap(), 20, pass); std::thread::sleep(Duration::from_secs(1)); continue;
        }
        if !etxt.contains("BUSY") && !etxt.contains("busy") { break; }
        eprintln!("[fusion-bridge] EDIT {name}.{suffix} deferred - main thread busy (attempt {}), waiting 15s", busy_try + 1);
        std::thread::sleep(Duration::from_secs(15));
    }
    if let Some(ab) = fusion_core::ab::Loopback::from_env() {
        use fusion_core::ab::AbClient;
        let pending: Vec<Value> = crate::small_verbs::classify_blocking_dialogs(&ab, &ctx.caller, &ctx.pids).into_iter().filter(|d| d["category"].as_str() == Some("create_confirm")).collect();
        let mut about_this = false;
        for d in &pending {
            let body = d["hwnd"].as_i64().and_then(|h| ab.call("desktop_find_controls", json!({"hwnd": h}), &ctx.caller, 10).ok()).map(|r| { let dd = if r["data"].is_object() { r["data"].clone() } else { r }; dd["controls"].as_array().or(dd["matches"].as_array()).cloned().unwrap_or_default().iter().filter_map(|c| c["name"].as_str().map(str::to_string)).collect::<Vec<_>>().join(" ") }).unwrap_or_default();
            if format!("{body} {}", d["title"].as_str().unwrap_or("")).to_lowercase().contains(&name.to_lowercase()) { about_this = true; }
            if let Some(h) = d["hwnd"].as_i64() { let _ = ab.call("desktop_ui_click", json!({"hwnd": h, "name": "No"}), &ctx.caller, 12); }
        }
        if !pending.is_empty() && !about_this { eprintln!("[fusion-bridge] declined a STALE Create-new prompt that does not mention {name:?} - clearing it and trusting the EDIT's own result"); }
        if !pending.is_empty() && about_this {
            eprintln!("[fusion-bridge] declined a Create-new prompt for {name:?} - the name is not in the library (answering Yes would fabricate an empty artifact)");
            let kind = match suffix { "sym" => "symbol", "pac" => "footprint", "dev" => "device", s => s };
            return json!({"success": false, "error": format!("No {kind} named {} in the open library. Fusion offered to CREATE it; the bridge declined.", fusion_lbr::py_repr_str(&name)), "errorCode": "artifact_not_in_library", "promptDeclined": true,
                "_hint": "The prompt was answered (No) inside this call, so nothing is left blocking Fusion's main thread. Answering Yes would have created an EMPTY artifact and made this look like it worked. Pass libraryPath so the bridge can verify names up front and never raise the prompt at all."});
        }
    }
    if edit["success"].as_bool() != Some(true) {
        return json!({"success": false, "error": format!("EDIT {name}.{suffix} failed: {}", edit["error"].as_str().unwrap_or("None")),
            "_hint": "The name must match the deviceset/artifact inside the OPEN library exactly (EDIT on a missing name pops a 'Create new?' dialog - run fusion_dismiss_blocking_dialogs if one appeared). Not sure of the name? fusion_get_app_state shows the active document; the .lbr's <symbol>/<package> name attributes are the source of truth."});
    }
    let _ = pass("electron_run", json!({"command": "WINDOW FIT"}));
    std::thread::sleep(Duration::from_secs(1));
    let mut out = json!({"success": true, "view": suffix, "name": name, "resolvedName": name});
    if let Some(lp) = lbr_path {
        if let Some(n) = std::fs::read_to_string(lp).ok().and_then(|x| fusion_lbr::artifact_names(&x).ok()) {
            out["libraryPath"] = json!(lp); out["symbolNames"] = json!(n.sym); out["packageNames"] = json!(n.pac); out["devicesetNames"] = json!(n.dev); out["has3d"] = json!(!n.p3d.is_empty()); out["package3dNames"] = json!(n.p3d);
        }
    }
    let shot = crate::small_verbs::screenshot_fusion(&json!({}), &ctx.caller);
    if shot["success"].as_bool() == Some(true) { if let Some(p) = shot["savedTo"].as_str() { out["screenshot"] = json!(p); } }
    out
}

/// `_show_prepare_library`: explicit path, generated wrapper, or already open. Ok(wrapped path).
fn show_prepare_library(ctx: &Ctx, args: &Value, symbol_xml: Option<&str>, package_xml: Option<&str>, name: &str, pass: &dyn Fn(&str, Value) -> Value) -> Result<Option<String>, Value> {
    let mut library_path = args["libraryPath"].as_str().unwrap_or("").replace('\\', "/");
    let mut wrapped: Option<String> = None;
    if symbol_xml.is_some() || package_xml.is_some() {
        if !library_path.is_empty() { return Err(json!({"success": false, "error": "Pass EITHER libraryPath OR a bare XML fragment, not both.", "_hint": "libraryPath opens an existing library; a fragment gets wrapped in a generated one."})); }
        match lbr_wrap_write(&ctx.state_dir, name, symbol_xml, package_xml) { (Some(p), _) => { wrapped = Some(p.clone()); library_path = p; }, (_, Some(e)) => return Err(json!({"success": false, "error": e})), _ => unreachable!() }
    }
    if !library_path.is_empty() {
        let opened = open_lbr(ctx, &json!({"filePath": library_path}), pass);
        if opened["success"].as_bool() != Some(true) { return Err(json!({"success": false, "error": format!("open_lbr failed: {}", opened["error"].as_str().unwrap_or("None")), "data": {"libraryPath": library_path, "wrapped": wrapped.is_some()}})); }
        remember_library(&ctx.state_dir, &library_path);
    }
    Ok(wrapped)
}

/// `fusion_show_symbol`.
pub fn show_symbol(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let symbol_xml = args["symbolXml"].as_str();
    let name = artifact_name_from(args, &["symbolXml"]);
    if name.is_empty() { return json!({"success": false, "error": "name required (or a symbolXml fragment with a name attribute)."}); }
    let wrapped = match show_prepare_library(ctx, args, symbol_xml, None, &name, pass) { Ok(w) => w, Err(e) => return e };
    let lp = wrapped.clone().or_else(|| args["libraryPath"].as_str().map(|s| s.replace('\\', "/")).filter(|s| !s.is_empty())).unwrap_or_else(|| last_library(&ctx.state_dir));
    let mut out = show_library_view(ctx, &name, "sym", if lp.is_empty() { None } else { Some(&lp) }, pass);
    if let (Some(w), Some(true)) = (&wrapped, out["success"].as_bool()) {
        out["wrappedLibrary"] = json!(w);
        out["_hint"] = json!(format!("Bare symbol was wrapped in a generated library at {w} (a deviceset with one gate was synthesized so the Library Manager lists it). To pair it with a footprint, re-call with both symbolXml and packageXml."));
    }
    out
}

/// `fusion_show_footprint`.
pub fn show_footprint(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let package_xml = args["packageXml"].as_str();
    let name = artifact_name_from(args, &["packageXml"]);
    if name.is_empty() { return json!({"success": false, "error": "name required (or a packageXml fragment with a name attribute)."}); }
    let wrapped = match show_prepare_library(ctx, args, None, package_xml, &name, pass) { Ok(w) => w, Err(e) => return e };
    let lp = wrapped.clone().or_else(|| args["libraryPath"].as_str().map(|s| s.replace('\\', "/")).filter(|s| !s.is_empty())).unwrap_or_else(|| last_library(&ctx.state_dir));
    let mut out = show_library_view(ctx, &name, "pac", if lp.is_empty() { None } else { Some(&lp) }, pass);
    if let (Some(w), Some(true)) = (&wrapped, out["success"].as_bool()) { out["wrappedLibrary"] = json!(w); out["_hint"] = json!(format!("Bare footprint was wrapped in a generated library at {w}.")); }
    out
}

/// `fusion_show_library`: the LIBRARY surface by its smallest name.
pub fn show_library(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let lbr = args["libraryPath"].as_str().unwrap_or("").replace('\\', "/");
    let name = args["name"].as_str().unwrap_or("").trim().to_string();
    if !lbr.is_empty() {
        let opened = open_lbr(ctx, &json!({"filePath": lbr}), pass);
        if opened["success"].as_bool() != Some(true) { return json!({"success": false, "error": format!("open_lbr failed: {}", opened["error"].as_str().unwrap_or("None").chars().take(150).collect::<String>())}); }
        std::thread::sleep(Duration::from_secs(2));
    } else if !name.is_empty() {
        let active = pass("get_app_state", json!({}))["data"]["activeDocument"].as_str().unwrap_or("").to_lowercase();
        if !active.contains(&name.to_lowercase()) {
            return json!({"success": false, "error": format!("library {} is not the active document and no libraryPath was given", fusion_lbr::py_repr_str(&name)), "_hint": "Pass libraryPath (WINDOWS .lbr path) to open it; name alone only handles the already-open case."});
        }
    } else { return json!({"success": false, "error": "libraryPath or name required."}); }
    let want = if name.is_empty() { stem_of(&lbr) } else { name.to_lowercase() };
    let active = pass("get_app_state", json!({}))["data"]["activeDocument"].as_str().unwrap_or("").to_string();
    if !want.is_empty() && !active.is_empty() && !active.to_lowercase().contains(&want) {
        return json!({"success": false, "errorCode": "library_not_active", "error": format!("opened {} but the ACTIVE document is {} - the view you would screenshot is not the library.", fusion_lbr::py_repr_str(if name.is_empty() { &want } else { &name }), fusion_lbr::py_repr_str(&active)),
            "activeDocument": active, "expected": if name.is_empty() { want } else { name }, "_hint": "Fusion switched or left another tab in front. Re-call fusion_show_library {libraryPath} once; if it repeats, close the stray document (fusion_close_document) and retry."});
    }
    let mut out = json!({"success": true, "view": "library", "activeDocument": if active.is_empty() { Value::Null } else { json!(active) }});
    let bound: Option<bool> = if lbr.is_empty() { None } else { std::fs::read_to_string(&lbr).ok().and_then(|x| fusion_lbr::artifact_names(&x).ok()).map(|n| !n.p3d.is_empty()) };
    out["threeDBound"] = json!(bound);
    if bound == Some(false) { out["_hint"] = json!("This library has NO package3d, so Fusion renders a grey Placeholder slab in the 3D pane - that is not a captured 3D artifact. Run fusion_bind_library_3d {libraryPath, stepPath} FIRST, then re-open. (fusion_create_library does the whole chain for you.)"); }
    else if bound.is_none() && !lbr.is_empty() { out["_hint"] = json!("Could not read the .lbr to tell whether a package3d is bound; if the 3D pane shows a grey Placeholder, run fusion_bind_library_3d first."); }
    let docs = open_doc_names(pass);
    if docs.len() > 3 { out["openDocuments"] = json!(docs.len()); out["_hintDocs"] = json!(format!("{} documents are open in Fusion. show_library deliberately leaves the library on screen, so a batch of calls accumulates tabs - close them with fusion_close_all_documents between runs.", docs.len())); }
    let shot = crate::small_verbs::screenshot_fusion(&json!({}), &ctx.caller);
    if shot["success"].as_bool() == Some(true) { if let Some(p) = shot["savedTo"].as_str() { out["screenshot"] = json!(p); } }
    out
}

/// `fusion_show_project`: query or urn (the generated-test-project case stays with the sidecar).
pub fn show_project(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Option<Value> {
    let query = args["query"].as_str().or(args["fileName"].as_str()).unwrap_or("").trim().to_string();
    let urn = args["urn"].as_str().unwrap_or("").trim().to_string();
    if query.is_empty() && urn.is_empty() {
        let Some(tp) = crate::testproject::test_project(&ctx.state_dir) else { return Some(json!({"success": false, "error": "query (cloud design name) or urn required - or run fusion_make_test_project first."})) };
        let ok = crate::testproject::test_project_activate(ctx, "brd", pass);
        let mut out = json!({"success": ok, "view": "project", "document": tp["docBase"], "source": "generated test project"});
        if !ok { out["error"] = json!("could not activate the test project's board document"); }
        else { let shot = crate::small_verbs::screenshot_fusion(&json!({}), &ctx.caller); if let Some(p) = shot["savedTo"].as_str().or(shot["localSafePath"].as_str()).filter(|_| shot["success"].as_bool() == Some(true)) { out["screenshot"] = json!(p); } }
        return Some(out);
    }
    if !urn.is_empty() { return Some(pass("open_by_urn", json!({"urn": urn}))); }
    let active = pass("get_app_state", json!({}))["data"]["activeDocument"].as_str().unwrap_or("").to_lowercase();
    let mut out = if active.contains(&query.to_lowercase()) { json!({"success": true, "view": "project", "document": active, "source": "already open"}) } else {
        let _ = open_cloud_file(ctx, &json!({"fileName": query, "wait": false}), pass);
        let deadline = Instant::now() + Duration::from_secs(170);
        let mut opened = false;
        while Instant::now() < deadline { if pass("get_app_state", json!({}))["data"]["activeDocument"].as_str().unwrap_or("").to_lowercase().contains(&query.to_lowercase()) { opened = true; break; } std::thread::sleep(Duration::from_secs(5)); }
        if !opened { return Some(json!({"success": false, "error": format!("{} never became the active document within 170s", fusion_lbr::py_repr_str(&query)), "_hint": "Check the name with fusion_aps_search; cloud opens on cold sessions take minutes."})); }
        json!({"success": true, "view": "project", "document": query, "source": "cloud open"})
    };
    let shot = crate::small_verbs::screenshot_fusion(&json!({}), &ctx.caller);
    if shot["success"].as_bool() == Some(true) { if let Some(p) = shot["savedTo"].as_str() { out["screenshot"] = json!(p); } }
    Some(out)
}

/// `fusion_export_source` / `fusion_export_eagle_source` write whatever editor is ACTIVE, and an
/// electronics design's schematic, board and 3D view are separate tabs with ONE name. From the
/// board editor a .sch export answered "No .sch file found inside .fsch ZIP container" (reads as a
/// corrupt file) and a .fsch export silently wrote a container holding only the .brd (page issue
/// #95, adonis/nametag-blank-large-led). The extension says which editor the caller wants, so the
/// bridge switches to it first (the same SwitchSchDocCmd / SwitchPcbDocCmd the show verbs run)
/// and says so. A .flbr needs a library, which is not a view of a design: left alone.
pub fn export_in_right_view(cmd: &str, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let out = args["outputPath"].as_str().unwrap_or("").to_ascii_lowercase();
    let (want_ws, switch_verb, label) = if out.ends_with(".sch") || out.ends_with(".fsch") { ("Schematic Editor", "show_schematic", "schematic") }
        else if out.ends_with(".brd") || out.ends_with(".fbrd") { ("PCB Editor", "show_2d_board", "2D board") }
        else { return pass(cmd, args.clone()) };
    let ws_now = |r: &Value| r["data"]["activeWorkspace"].as_str().or(r["activeWorkspace"].as_str()).unwrap_or("").to_string();
    let before = ws_now(&pass("get_app_state", json!({})));
    let mut switched = Value::Null;
    if !before.is_empty() && before != want_ws {
        let sw = pass(switch_verb, json!({}));
        if sw["success"].as_bool() == Some(true) { switched = json!({"from": before, "to": want_ws, "via": format!("fusion_{switch_verb}")}); }
        else {
            return json!({"success": false, "errorCode": "wrong_editor_active",
                "error": format!("{} needs the {label} editor active, and the {before} is. Switching with fusion_{switch_verb} failed: {}", out.rsplit(['/', '\\']).next().unwrap_or(&out), sw["error"].as_str().unwrap_or("unknown")),
                "_hint": format!("Open the electronics DESIGN first (fusion_open_cloud_file / fusion_open_by_urn), then fusion_{switch_verb} {{documentName}} and retry. fusion_document_info lists the open tabs.")});
        }
    }
    let mut r = pass(cmd, args.clone());
    if r["success"].as_bool() == Some(false) && r["error"].as_str().map(|e| e.contains("found inside")).unwrap_or(false) {
        r["errorCode"] = json!("wrong_editor_active");
        r["_hint"] = json!(format!("The file is not corrupt: the container Fusion wrote holds the view that was ACTIVE, not the {label}. Run fusion_{switch_verb} {{documentName}} and retry (the bridge normally does this itself; it could not confirm the switch this time)."));
    }
    if !switched.is_null() && r.is_object() {
        r["switchedView"] = switched;
        let h = r["_hint"].as_str().unwrap_or("").to_string();
        r["_hint"] = json!(format!("The bridge switched to the {label} editor first, because a {} export writes the ACTIVE editor. {h}", out.rsplit('.').next().unwrap_or("")).trim().to_string());
    }
    r
}

/// `fusion_activate_document`: an electronics design opens as several tabs that all carry the
/// design's name, and Fusion reports their type as unknown, so "activate by name" picks one of
/// them. Say so, and name the verbs that reach a specific view (page issue #95).
pub fn activate_with_same_name_note(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut r = pass("activate_document", args.clone());
    let want = args["name"].as_str().unwrap_or("").trim().to_lowercase();
    if want.is_empty() { return r; }
    let info = pass("document_info", json!({}));
    let docs: Vec<String> = info["data"]["openDocuments"].as_array().or(info["openDocuments"].as_array()).map(|a| a.iter().filter_map(|d| d["name"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    let exact = docs.iter().filter(|n| n.to_lowercase() == want).count();
    let loose = docs.iter().filter(|n| n.to_lowercase().contains(&want)).count();
    let views = "To reach a specific view of an electronics design use fusion_show_schematic, fusion_show_2d_board or fusion_show_3d_board with {documentName}: each activates the design and switches to that editor.";
    if exact > 1 || (exact == 0 && loose > 1) {
        let n = if exact > 1 { exact } else { loose };
        r["sameNameDocuments"] = json!(n);
        let h = r["_hint"].as_str().unwrap_or("").to_string();
        r["_hint"] = json!(format!("{n} open tabs match '{}': an electronics design's schematic, board and 3D views are separate tabs with ONE name, and Fusion reports their type as unknown, so this activated one of them. {views} {h}", args["name"].as_str().unwrap_or("")).trim().to_string());
    } else if r["success"].as_bool() == Some(false) && docs.is_empty() == false {
        let h = r["_hint"].as_str().unwrap_or("").to_string();
        r["_hint"] = json!(format!("{h} {views}").trim().to_string());
    }
    r
}

/// The three view-switch verbs act on the ACTIVE document; this names the one the caller means
/// and explains the wrong-document failure. Called from the dispatcher's add-in arm (which is
/// where show_* really run) and from `open_with_screenshot`.
pub fn show_with_document(cmd: &str, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    // An electronics design is FOUR tabs with one name, told apart only by product type:
    // EcadDesignProductType (the container), SchematicProductType, BoardProductType and
    // DesignProductType (3D). Activating by name lands on whichever comes first, often the
    // container, where no editor switch works ("Not in Electronics editor (current: Electronics
    // Design)", ConfRoomROG 2026-09-22). The editor switches work from the schematic, board and 3D
    // tabs alike, so the tab is changed ONLY when the container (or another document) is in front:
    // then the schematic tab for show_schematic, the board tab for the board views. Done
    // through run_modeling_script so it works with every add-in version (no Fusion restart).
    let is_show = matches!(cmd, "show_schematic" | "show_2d_board" | "show_3d_board");
    if is_show {
        let want_pt = if cmd == "show_schematic" { "SchematicProductType" } else { "BoardProductType" };
        let name = args["documentName"].as_str().or_else(|| args["name"].as_str()).unwrap_or("").trim().to_string();
        let name_js = serde_json::to_string(&name).unwrap_or_else(|_| "\"\"".into());
        let script = format!(r#"import adsk.core
app = adsk.core.Application.get()
want = {name_js}.lower() or (app.activeDocument.name.lower() if app.activeDocument else '')
cur_pt = ''
try:
    cur_pt = app.activeDocument.products.item(0).productType if app.activeDocument and app.activeDocument.products.count else ''
except Exception:
    pass
hit = None
if not (app.activeDocument and app.activeDocument.name.lower() == want and cur_pt in ('SchematicProductType', 'BoardProductType', 'DesignProductType')):
    for i in range(app.documents.count):
        d = app.documents.item(i)
        try:
            pt = d.products.item(0).productType if d.products.count else ''
        except Exception:
            pt = ''
        if d.name.lower() == want and pt == {want_pt:?}:
            hit = d
            break
if hit is not None:
    # the UI's tab too: activate() can leave the UI (and so every editor switch) on another tab
    idx = next((k for k in range(app.documents.count) if app.documents.item(k) == hit), None)
    if idx is None:
        hit.activate()
    else:
        app.executeTextCommand('UI.SwitchToTab %d' % idx)
result = {{"wanted": want, "tabType": {want_pt:?}, "switchedTab": hit is not None, "activeNow": app.activeDocument.name if app.activeDocument else None}}
"#);
        // straight to the add-in: this is the bridge's own bookkeeping, not a user script, so it skips
        // the window-etiquette envelope (its dialog scans before and after cost more than the script)
        let sr = crate::addin::addin_http("run_modeling_script", &json!({"script": script}), 20);
        let res = if sr["data"]["result"].is_object() { sr["data"]["result"].clone() } else { sr["result"].clone() };
        let switched = res["switchedTab"].as_bool() == Some(true);
        // no typed tab matched (a standalone local .sch/.brd, or an older Fusion): fall back to name
        if !switched && !name.is_empty() && res["activeNow"].as_str().map(|a| !a.eq_ignore_ascii_case(&name)).unwrap_or(true) {
            let a = pass("activate_document", json!({"name": name}));
            if a["success"].as_bool() == Some(false) {
                let mut a = a; a["_hint"] = json!(format!("{cmd} was asked to show '{name}' but that document could not be activated. fusion_state lists the open documents by name."));
                return a;
            }
        }
    }
    let mut r = pass(cmd, args.clone());
    if is_show && r["success"].as_bool() == Some(false) && r["error"].as_str().map(|e| e.contains("open or create an electronics design")).unwrap_or(false) {
        let docs: Vec<Value> = r["documentHygiene"]["documents"].as_array().map(|a| a.iter().map(|d| d["name"].clone()).collect()).unwrap_or_default();
        r["errorCode"] = json!("not_an_electronics_design");
        r["openDocuments"] = json!(docs);
        r["_hint"] = json!(format!("The ACTIVE document is not an electronics design (a library or a 3D design is in front). Pass documentName to {cmd} and it activates that document first, e.g. {cmd} {{documentName:\"<board doc>\", capture:true}}. openDocuments lists what is open. Nothing open yet? fusion_schematic {{action:\"create\"}} or fusion_make_showcase_project make one, dialog-free."));
    }
    r
}

/// `_handle_open_with_screenshot`: the add-in open or switch, then the post-open capture.
/// What Fusion would refuse in a .brd, checked before it sees the file. Each of these makes it draw its
/// "has errors and cannot be opened" modal, leaving an empty design behind (each tried live on 2706):
/// a part whose library or package isn't in the file, and a net naming a part that isn't.
pub fn board_file_problems(text: &str) -> Vec<String> {
    let Ok(doc) = roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }) else { return vec!["the file is not well-formed XML".into()] };
    let Some(board) = doc.descendants().find(|n| n.has_tag_name("board")) else { return vec![] };
    let low = |s: Option<&str>| s.unwrap_or("").to_lowercase();
    let mut packages = std::collections::HashSet::new();
    let mut libraries = std::collections::HashSet::new();
    for lib in board.descendants().filter(|n| n.has_tag_name("library")) {
        libraries.insert(low(lib.attribute("name")));
        for p in lib.descendants().filter(|n| n.has_tag_name("package")) { packages.insert((low(lib.attribute("name")), low(p.attribute("name")))); }
    }
    let mut out = Vec::new();
    let mut elements = std::collections::HashSet::new();
    for e in board.descendants().filter(|n| n.has_tag_name("element")) {
        elements.insert(low(e.attribute("name")));
        let (name, lib, pkg) = (e.attribute("name").unwrap_or("?"), e.attribute("library").unwrap_or(""), e.attribute("package").unwrap_or(""));
        if !libraries.contains(&lib.to_lowercase()) { out.push(format!("part {name} names library {lib}, which isn't in the file")); }
        else if !packages.contains(&(lib.to_lowercase(), pkg.to_lowercase())) { out.push(format!("part {name} names package {pkg}, which isn't in its library {lib}")); }
    }
    for c in board.descendants().filter(|n| n.has_tag_name("contactref")) {
        if !elements.contains(&low(c.attribute("element"))) {
            let net = c.parent().and_then(|p| p.attribute("name")).unwrap_or("?");
            out.push(format!("net {net} names part {}, which isn't on the board", c.attribute("element").unwrap_or("?")));
        }
    }
    out
}

/// The parts on a just-imported board. Fusion answers before the board view has loaded, with no parts or some of
/// them, so a read straight after the import called a good board nothing_instantiated (#139). It's read again until
/// every part is there, or the count has held still, about 30 s at most.
fn read_placed_parts(expected: usize, read: &dyn Fn() -> Value, wait: &dyn Fn()) -> (usize, Vec<Value>) {
    let mut last = None;
    let (mut placed, mut names) = (0, vec![]);
    for _ in 0..10 {
        let bi = read();
        let d = if bi["data"].is_object() { bi["data"].clone() } else { bi.clone() };
        let sm = if d["summary"].is_object() { d["summary"].clone() } else { d.clone() };
        placed = sm["componentCount"].as_u64().map(|n| n as usize).or_else(|| sm["components"].as_array().map(|a| a.len())).unwrap_or(0);
        names = sm["components"].as_array().cloned().unwrap_or_default().iter().take(40).map(|c| c["name"].clone()).collect();
        if placed >= expected || (placed > 0 && last == Some(placed)) { break; }
        last = Some(placed);
        wait();
    }
    (placed, names)
}

/// fusion_open_board and fusion_open_schematic: a file Fusion would refuse never reaches it, and one it
/// refuses anyway is answered as that, not as a hung call or a success with an empty design.
pub fn open_file_guarded(ctx: &Ctx, cmd: &str, args: &Value, run: &dyn Fn(&str, Value) -> Value) -> Value {
    let file = args["filePath"].as_str().unwrap_or("").to_string();
    if cmd == "open_board" {
        if let Ok(text) = std::fs::read_to_string(&file) {
            let problems = board_file_problems(&text);
            if !problems.is_empty() {
                return json!({"success": false, "errorCode": "board_file_has_errors", "filePath": file, "problems": problems.iter().take(10).collect::<Vec<_>>(), "problemCount": problems.len(), "mutated": false,
                    "error": format!("Fusion can't open this board: {}{}. Nothing was opened.", problems[0], if problems.len() > 1 { format!(", and {} more", problems.len() - 1) } else { String::new() }),
                    "_hint": "Fix the file, or build it again with fusion_board_seed. No dialog was drawn."});
            }
        }
    }
    // the backstop, for a refusal the check above doesn't know: Fusion's modal stops the open until it's
    // answered, so it's watched for from here, closed, and said out loud. The open itself then reports
    // success with an empty design behind it.
    let (stop, refused) = (std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), std::sync::Arc::new(std::sync::Mutex::new(None::<String>)));
    if matches!(cmd, "open_board" | "open_schematic") {
        if let Some(ab) = fusion_core::ab::Loopback::from_env() {
            let (stop2, refused2, caller, pids) = (stop.clone(), refused.clone(), ctx.caller.clone(), ctx.pids.clone());
            std::thread::spawn(move || {
                use std::sync::atomic::Ordering;
                while !stop2.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_secs(2));
                    if let Some((h, text)) = crate::small_verbs::file_refused_dialog(&ab, &caller, &pids) {
                        let (gone, _) = crate::small_verbs::close_native_dialog(&ab, &caller, h);
                        *refused2.lock().unwrap() = Some(if gone { text } else { format!("{text} (the dialog did not close: Fusion is still blocked)") });
                        break;
                    }
                }
            });
        }
    }
    let r = run(cmd, args.clone());
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(text) = refused.lock().unwrap().clone() {
        let stem = std::path::Path::new(&file.replace('\\', "/")).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let closed = run("close_document", json!({"name": stem}));
        return json!({"success": false, "errorCode": "file_refused_by_fusion", "filePath": file, "mutated": false, "emptyDesignClosed": closed["success"].as_bool() == Some(true),
            "error": format!("Fusion refused the file: {text}"),
            "_hint": "Fusion drew its error dialog and the bridge closed it. Fix the file; for a .brd, fusion_board_seed builds one Fusion can open."});
    }
    r
}

pub fn open_with_screenshot(ctx: &Ctx, cmd: &str, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut r = show_with_document(cmd, args, pass);
    if r["success"].as_bool() == Some(true) {
        let (shots, _) = crate::lifecycle::post_open_screenshot(ctx, 2.0);
        if !r["data"].is_object() { r["data"] = json!({}); }
        r["data"]["postOpenScreenshot"] = shots;
    }
    r
}

/// `fusion_state`: the render-me verb. Server-side only: it answers while the add-in is busy.
pub fn fusion_state(ctx: &Ctx, args: &Value) -> Value {
    let sd = ctx.state_dir.clone();
    let running = !ctx.host.fusion_processes().is_empty();
    let mut result = json!({"success": true, "running": running});
    let focus: Vec<Value> = std::fs::read_to_string(sd.join("focus-events.jsonl")).ok().map(|t| t.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect::<Vec<_>>()).unwrap_or_default();
    result["focusEvents"] = json!(focus[focus.len().saturating_sub(20)..]);
    let timings = fusion_core::readiness::timings_summary(&crate::timings::load(&sd));
    result["timings"] = timings.clone();
    if let Some(s) = crate::launch::timings_sentence(&sd) { result["timingsSentence"] = json!(s); }
    if !running {
        result["_hint"] = json!("Fusion is not running - nothing to render. fusion_start launches it in the background; fusion_demo {mode:'tour'} cold-starts it itself. focusEvents and timings ARE still reported - they do not need Fusion.");
        result["output"] = json!(json!({"running": false}).to_string());
        return result;
    }
    let ab = fusion_core::ab::Loopback::from_env();
    let facts = ab.as_ref().map(|a| fusion_core::windows_facts::facts_from_ab(a, &ctx.caller)).unwrap_or_default();
    result["hwnd"] = json!(facts.main_hwnd); result["title"] = json!(facts.main_title);
    let main_row = facts.family.iter().find(|w| w["hwnd"].as_i64() == facts.main_hwnd).cloned();
    result["rect"] = main_row.map(|m| fusion_core::dialogs::rect_of(&m["rect"])).unwrap_or(Value::Null);
    let (dialogs, mut blocking) = match &ab {
        Some(a) => {
            use fusion_core::ab::AbClient;
            let rows = a.call("desktop_enum_windows", json!({"pids": ctx.pids}), &ctx.caller, 10).ok().map(|v| v["windows"].as_array().or_else(|| v["data"]["windows"].as_array()).cloned().unwrap_or_default()).unwrap_or_default();
            (fusion_core::dialogs::qt_dialog_rows(&rows), crate::small_verbs::classify_blocking_dialogs(a, &ctx.caller, &ctx.pids))
        }
        None => (vec![], vec![]),
    };
    result["dialogs"] = json!(dialogs);
    if !blocking.iter().any(|d| d["category"].as_str() == Some("license_sessions_exceeded")) {
        if let Some(dlg) = &facts.licensing_dialog {
            let auto = crate::small_verbs::get_pref(&sd, "autoTakeOverSeat").map(|v| v != json!(false)).unwrap_or(true);
            blocking.push(json!({"category": "license_sessions_exceeded", "title": dlg["title"], "hwnd": dlg["hwnd"], "resolution": format!("Autodesk seat modal: this account's Fusion is active on another machine. {}", if auto { "Auto-takeover is ON - the bridge clicks Continue through both dialogs in the background (~20s sweep; a fusion_start resolves it immediately). The other machine's Fusion suspends RESUMABLY." } else { "Auto-takeover is OPTED OUT (fusion_set_seat_takeover) - resolve the dialog yourself or re-enable takeover." })}));
        }
    }
    result["blockingDialogs"] = json!(blocking);
    let st = crate::addin::addin_status_probe(Duration::from_secs(3));
    let busy = st.as_ref().filter(|s| s["busy"].as_bool() == Some(true)).map(|s| json!({"command": s["busyCommand"], "elapsedSeconds": s["elapsedSeconds"]}));
    result["addinResponding"] = json!(st.is_some());
    result["mainThreadBusy"] = busy.clone().unwrap_or(Value::Null);
    // silent but loaded (a long native call starves even /status): say so, so no caller restarts it
    result["addinBusy"] = if st.is_none() { crate::addin::loaded_evidence(&ctx.pids).unwrap_or(Value::Null) } else { Value::Null };
    if let Some(lsa) = crate::small_verbs::get_pref(&sd, "launchStartedAt").and_then(|v| v.as_f64()) {
        if st.is_none() && now_f() - lsa < 900.0 {
            let stats = fusion_core::readiness::timings_stats(&crate::timings::load(&sd), "launch", "cold");
            result["progress"] = fusion_core::readiness::progress_block(&stats, lsa, now_f(), "Fusion is starting - waiting for the Adom add-in to serve", 150.0);
        }
    }
    result["drivingBadge"] = json!({"active": crate::etiquette::badge_active(), "lastDrivingVerbAt": crate::etiquette::last_drive_at(), "idleShedSeconds": 300, "enabled": crate::small_verbs::get_pref(&sd, "showDrivingBadge").map(|v| v != json!(false)).unwrap_or(true), "lastError": Value::Null});
    if st.is_some() && busy.is_none() {
        let app = ureq::post("http://127.0.0.1:8774/command").timeout(Duration::from_secs(5)).send_json(json!({"command": "get_app_state", "args": {}})).ok().and_then(|r| r.into_json::<Value>().ok());
        if let Some(a) = app { result["activeDocument"] = a["data"]["activeDocument"].clone(); }
    }
    let want_native = args["nativeViewport"].as_bool() != Some(false) && args["screenshot"].as_bool() != Some(false);
    let native_only = args["viewport"].as_str() == Some("native");
    if want_native {
        // Issue #84: a WGC window capture of a GPU Design viewport can be blank or stale while
        // the document holds hundreds of bodies. Ask Fusion itself for the frame and the body
        // count, so geometry presence is reported separately from window pixels.
        let png_path = crate::fusionpath::stage_root(&sd).map(|r| r.join(format!("native-viewport-{}.png", (now_f() * 1000.0) as u64)).to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| sd.join("native-viewport.png").to_string_lossy().replace('\\', "/"));
        let script = format!("import adsk.core, adsk.fusion\napp=adsk.core.Application.get(); res={{}}\nd=app.activeDocument; res['document']=d.name if d else None\np=app.activeProduct; res['productType']=p.productType if p else None\nacc=[0]\ndef _walk(c):\n    acc[0]+=c.bRepBodies.count\n    for i in range(c.occurrences.count): _walk(c.occurrences.item(i).component)\ntry:\n    ds=adsk.fusion.Design.cast(p)\n    if ds: _walk(ds.rootComponent); res['bodies']=acc[0]\nexcept Exception as e: res['bodiesError']=str(e)[:80]\nvp=app.activeViewport\nif vp:\n    try:\n        ok=vp.saveAsImageFile('{png_path}', 1600, 1000); res['path']='{png_path}' if ok else None; res['ok']=bool(ok)\n    except Exception as e: res['viewportError']=str(e)[:100]\nresult=res\n");
        let r = crate::addin::addin_http("run_modeling_script", &json!({"script": script}), 25);
        let nv = r["data"]["result"].clone();
        if nv.is_object() {
            result["nativeViewport"] = json!({"path": nv["path"], "ok": nv["ok"], "bodies": nv["bodies"], "productType": nv["productType"], "document": nv["document"], "capturedAt": now_f(), "source": "app.activeViewport.saveAsImageFile through the add-in (no foreground, no window pixels)"});
            result["geometry"] = json!({"bodies": nv["bodies"], "productType": nv["productType"], "source": "Fusion API body count, independent of any capture"});
        } else {
            result["nativeViewport"] = json!({"ok": false, "error": r["error"].as_str().unwrap_or("add-in did not answer").chars().take(160).collect::<String>()});
        }
    }
    if args["screenshot"].as_bool() != Some(false) && !native_only {
        let shot = crate::small_verbs::screenshot_fusion(&json!({}), &ctx.caller);
        if shot["success"].as_bool() == Some(true) {
            let p = shot["savedTo"].as_str().unwrap_or("").to_string();
            result["screenshot"] = json!(p);
            if let Some(frac) = png_dominant_fraction(&p) {
                result["windowPixels"] = json!({"dominantColorFraction": (frac * 1000.0).round() / 1000.0, "blank": frac >= 0.97,
                    "note": "fraction of the window capture that is one colour; above 0.97 the capture shows no geometry (a GPU Design viewport under WGC can render blank or stale while the model is fine)"});
            }
            if args["imageAs"].as_str() == Some("base64") {
                if let Ok(raw) = std::fs::read(&p) {
                    if raw.len() <= 4 * 1024 * 1024 { result["screenshotBase64"] = json!(crate::etiquette::b64_pub(&raw)); }
                    else { result["screenshotNote"] = json!(format!("capture is {}KB (>4MB inline cap) and could not be downscaled - pull result.screenshot via read_file.", raw.len() / 1024)); }
                }
            }
        } else { result["screenshotError"] = json!(shot["error"].as_str().unwrap_or("capture failed")); }
    }
    let mut hints: Vec<String> = Vec::new();
    if let Some(b) = &busy { hints.push(format!("Main thread busy ({}, {}s) - add-in verbs are rejected right now; window/screenshot verbs still work.", b["command"].as_str().unwrap_or("?"), b["elapsedSeconds"])); }
    if !blocking.is_empty() { hints.push("A modal is up - see blockingDialogs for the classified cause + resolution.".into()); }
    if result["windowPixels"]["blank"].as_bool() == Some(true) {
        let bodies = result["geometry"]["bodies"].as_u64().unwrap_or(0);
        if result["nativeViewport"]["ok"].as_bool() == Some(true) {
            hints.push(format!("The window capture is blank (GPU Design viewport under WGC) while Fusion itself reports {bodies} bod{} and rendered nativeViewport.path: trust the native frame for geometry, never the window pixels, and do NOT foreground Fusion to fix a capture.", if bodies == 1 { "y" } else { "ies" }));
        } else {
            hints.push("The window capture is blank and the native viewport export did not answer; nothing here substantiates visible geometry. Re-call with screenshot:true once the add-in answers, or read geometry through fusion_inspect_bodies.".into());
        }
    } else if result["nativeViewport"]["ok"].as_bool() == Some(true) && result["nativeViewport"]["productType"].as_str() == Some("DesignProductType") {
        hints.push("nativeViewport.path is the frame Fusion itself rendered (activeViewport.saveAsImageFile); for a Design document prefer it over the window capture, which can lag the model.".into());
    }
    if hints.is_empty() { hints.push("Fusion is clear. The screenshot shows the current surface - this is the verb to render any fusion_show_* result (they open in the background by design).".into()); }
    result["_hint"] = json!(hints.join(" "));
    result["output"] = json!(json!({"running": true, "activeDocument": result["activeDocument"], "mainThreadBusy": busy, "blockingDialogs": blocking, "screenshot": result["screenshot"]}).to_string());
    result
}

/// Where the bridge's own captures go: %TEMP%\\adom-fusion-shots. ab auto-pulls a reply's
/// `fullPath` to the caller only from the user-profile and temp roots, so the old
/// C:/tmp/conduit-screenshots folder was never delivered (page issue #98).
pub fn shots_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join("adom-fusion-shots");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// `_capture_labeled`: the main window to a labeled PNG under shots_dir()
/// (ab captures to its own shot dir; the file is copied so pull paths stay where the skills say).
pub fn capture_labeled(ctx: &Ctx, label: &str) -> Option<String> {
    if label.is_empty() { return None; }
    let shot = crate::small_verbs::screenshot_fusion(&json!({}), &ctx.caller);
    let src = shot["savedTo"].as_str().filter(|_| shot["success"].as_bool() == Some(true))?.to_string();
    let safe: String = regex::Regex::new(r#"[<>:"/\\|?*]"#).unwrap().replace_all(label, "").replace(' ', "_").chars().take(40).collect();
    let dir_buf = shots_dir(); let dir = dir_buf.as_path();
    let ts = (now_f() * 1000.0) as u64;
    let ext = std::path::Path::new(&src).extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "png".into());
    let dst = dir.join(format!("fusion-{safe}-{ts}.{ext}"));
    match std::fs::copy(&src, &dst) { Ok(_) => Some(dst.to_string_lossy().replace('\\', "/")), Err(_) => Some(src) }
}

fn remembered_open_library(state_dir: &std::path::Path) -> Option<String> {
    let t = std::fs::read_to_string(state_dir.join("open_library.json")).ok()?;
    let lp = serde_json::from_str::<Value>(&t).ok()?["libraryPath"].as_str()?.to_string();
    if !lp.is_empty() && std::path::Path::new(&lp).exists() { Some(lp) } else { None }
}

/// `_build_in_flight`: paths whose async 3D build (on the sidecar) is still running, from disk.
fn build_in_flight(state_dir: &std::path::Path) -> std::collections::BTreeMap<String, f64> {
    let t = std::fs::read_to_string(state_dir.join("builds_in_flight.json")).unwrap_or_default();
    let m: std::collections::BTreeMap<String, f64> = serde_json::from_str::<Value>(&t).ok().and_then(|v| v.as_object().map(|o| o.iter().filter_map(|(k, v)| v.as_f64().map(|f| (k.clone(), f))).collect())).unwrap_or_default();
    m.into_iter().filter(|(_, v)| now_f() - v < 3600.0).collect()
}

/// `_dismiss_dialogs_bg`: WM_CLOSE (Cancel/No) on every Fusion dialog, never a focus steal. Titles dismissed.
fn dismiss_dialogs_bg(ctx: &Ctx) -> Vec<String> {
    let Some(ab) = fusion_core::ab::Loopback::from_env() else { return vec![] };
    use fusion_core::ab::AbClient;
    let rows = ab.call("desktop_enum_windows", json!({"pids": ctx.pids}), &ctx.caller, 10).ok().map(|v| v["windows"].as_array().or_else(|| v["data"]["windows"].as_array()).cloned().unwrap_or_default()).unwrap_or_default();
    let mut out = Vec::new();
    for d in fusion_core::dialogs::qt_dialog_rows(&rows) { if let Some(h) = d["hwnd"].as_i64() { if ab.call("desktop_close_window", json!({"hwnd": h}), &ctx.caller, 10).is_ok() { out.push(d["title"].as_str().unwrap_or("").to_string()); } } }
    out
}

/// `_create_3d_package`: Electron.Create3DPackage, only once the library is verified active.
pub fn create_3d_package_pub(ctx: &Ctx, lbr_path: &str, pass: &dyn Fn(&str, Value) -> Value) -> Value { create_3d_package(ctx, lbr_path, pass) }
fn create_3d_package(ctx: &Ctx, lbr_path: &str, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let act = ensure_library_active(ctx, lbr_path, 60, pass);
    if act["ok"].as_bool() != Some(true) {
        return json!({"success": false, "errorCode": "library_not_active", "error": "The library opened but never became the ACTIVE document, so Create3DPackage would refuse ('Please run the command in library document').", "_hint": "Another document may be mid-operation on Fusion's main thread. Retry: the bridge polls 60s and activates by name before giving up."});
    }
    pass("execute_text_command", json!({"command": format!("Electron.Create3DPackage {lbr_path}")}))
}

fn capture_3d_view(ctx: &Ctx, pkg: &str, lbr_path: &str, settle: f64, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut entry = json!({"package": pkg, "view": "3d", "path": Value::Null, "ok": false});
    if !lbr_path.is_empty() {
        if let Some(n) = std::fs::read_to_string(lbr_path).ok().and_then(|x| fusion_lbr::artifact_names(&x).ok()) {
            if n.p3d.is_empty() {
                entry["errorCode"] = json!("no_3d_package_bound"); entry["error"] = json!("No <package3d> is bound in that library, so the only thing to photograph is Fusion's grey Placeholder - refusing to return that as the 3D artifact.");
                entry["_hint"] = json!("Run fusion_build_library_3d (or fusion_bind_library_3d) first, then re-capture. A Placeholder looks like a successful 3D view in a screenshot, which is why this refuses instead.");
                return entry;
            }
        }
    }
    // Photograph the BOUND 3D asset, not Fusion's Create3DPackage generator: the generator loads
    // only the footprint (no chip body) and always for the library's FIRST package, and its
    // Package3DStop leaves an unsaved generator tab behind (#109). The bound f3d is the real
    // artifact (footprint + chip), opened by its urn, framed, captured and closed unsaved.
    if lbr_path.is_empty() { entry["errorCode"] = json!("library_path_required"); entry["error"] = json!("The 3d view needs libraryPath to find the part's bound 3D package."); return entry; }
    let Some((p3d_pkg, urn)) = std::fs::read_to_string(lbr_path).ok().and_then(|x| bound_p3d_urn(&x, pkg)) else {
        entry["errorCode"] = json!("no_3d_package_bound"); entry["error"] = json!(format!("{pkg} has no <package3dinstance> in that library, so there is no 3D body to show."));
        entry["_hint"] = json!("fusion_build_library_3d (or fusion_bind_library_3d) binds one; then re-capture."); return entry;
    };
    entry["package3d"] = json!(p3d_pkg); entry["urn"] = json!(urn);
    let mut opened = Value::Null;
    for attempt in 0..2 { opened = pass("open_by_urn", json!({"urn": urn})); if opened["success"].as_bool() == Some(true) { break; } if attempt == 0 { std::thread::sleep(Duration::from_secs(10)); } }
    if opened["success"].as_bool() != Some(true) {
        entry["errorCode"] = json!("open_3d_failed"); entry["error"] = json!(format!("Could not open the bound f3d: {}", opened["error"].as_str().unwrap_or("None").chars().take(160).collect::<String>()));
        let _ = ensure_library_active(ctx, lbr_path, 30, pass); return entry;
    }
    let _ = pass("run_modeling_script", json!({"script": "import adsk.core, time\napp=adsk.core.Application.get()\ntry:\n    w=app.userInterface.workspaces.itemById('FusionSolidEnvironment')\n    if w and not w.isActive: w.activate()\n    adsk.doEvents(); time.sleep(1)\nexcept Exception: pass\ntry:\n    app.activeViewport.goHome(False)\n    app.activeViewport.fit()\nexcept Exception: pass\nresult={}\n"}));
    if settle > 0.0 { std::thread::sleep(Duration::from_secs_f64(settle)); }
    let (bodies, _) = crate::prove::count_3d_bodies(pass);
    entry["bodies"] = json!(bodies);
    entry["path"] = json!(capture_labeled(ctx, &format!("{pkg}_3d")));
    entry["ok"] = json!(!entry["path"].is_null() && bodies != 0);
    if bodies == 0 { entry["errorCode"] = json!("no_bodies"); entry["error"] = json!("The bound f3d opened but holds no bodies, so the frame shows no chip."); }
    // close the f3d unsaved (it is the stored asset, nothing here edited it), then put the library back in front
    let _ = pass("run_modeling_script", json!({"script": "import adsk.core, time\napp=adsk.core.Application.get()\nfor _ in range(12):\n    if not app.hasActiveJobs: break\n    time.sleep(2)\napp.activeDocument.close(False)\nresult={}"}));
    let _ = ensure_library_active(ctx, lbr_path, 45, pass);
    entry
}

/// The bound 3D package of a deviceset: (package name, urn) from its device's package3dinstance,
/// falling back to the <package3d> whose packageinstance names the device's package.
pub fn bound_p3d_urn(xml: &str, deviceset: &str) -> Option<(String, String)> {
    let doc = roxmltree::Document::parse_with_options(xml, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).ok()?;
    let ds = doc.descendants().find(|n| n.has_tag_name("deviceset") && n.attribute("name").map(|v| v.eq_ignore_ascii_case(deviceset)).unwrap_or(false))?;
    let dv = ds.descendants().find(|n| n.has_tag_name("device") && n.attribute("package").map(|p| !p.is_empty()).unwrap_or(false))?;
    let pkg = dv.attribute("package").unwrap_or("").to_string();
    if let Some(u) = dv.descendants().find(|n| n.has_tag_name("package3dinstance")).and_then(|n| n.attribute("package3d_urn")).map(str::trim).filter(|u| !u.is_empty()) { return Some((pkg, u.to_string())); }
    doc.descendants().filter(|n| n.has_tag_name("package3d")).find(|p3| p3.descendants().any(|n| n.has_tag_name("packageinstance") && n.attribute("name").map(|v| v.eq_ignore_ascii_case(&pkg)).unwrap_or(false)))
        .and_then(|p3| p3.attribute("urn").filter(|u| !u.trim().is_empty()).or(p3.attribute("wip_urn")).map(|u| (pkg.clone(), u.trim().to_string())))
}

/// `fusion_capture_library_views`: symbol, footprint, 3D and component views of each part.
pub fn capture_library_views(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let pkgs: Vec<String> = args["packages"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).filter(|v: &Vec<String>| !v.is_empty()).or_else(|| args["package"].as_str().map(|p| vec![p.to_string()])).unwrap_or_default();
    if pkgs.is_empty() { return json!({"success": false, "error": "packages: [<deviceset name>] (or package: <name>) required.", "_hint": "Open the .lbr first (fusion_open_lbr). Names are the deviceset names, e.g. ESP32-S3FN8."}); }
    fn view_ext(v: &str) -> String { match v { "component" | "dev" => "dev", "symbol" | "sym" => "sym", "footprint" | "pac" => "pac", "3d" | "p3d" | "package3d" => "3d", other => other }.to_string() }
    fn label_of(e: &str) -> String { match e { "dev" => "component", "sym" => "symbol", "pac" => "footprint", "3d" => "3d", o => o }.to_string() }
    let views: Vec<String> = args["views"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).filter(|v: &Vec<String>| !v.is_empty()).unwrap_or_else(|| vec!["symbol".into(), "footprint".into(), "3d".into(), "component".into()]);
    let settle = args["settle"].as_f64().unwrap_or(1.5);
    let flight = build_in_flight(&ctx.state_dir);
    if !flight.is_empty() && args["ignoreBuildInProgress"].as_bool() != Some(true) {
        return json!({"success": false, "errorCode": "build_in_progress", "error": format!("A 3D build is still running for {} - refusing to capture, because the build holds the 3D-package editor on screen and every view would show that editor instead of the view it is labelled with.", flight.keys().cloned().collect::<Vec<_>>().join(", ")),
            "buildsInFlight": flight.iter().map(|(k, v)| json!({"lbrPath": k, "runningSec": (now_f() - v) as i64})).collect::<Vec<_>>(),
            "_hint": "Wait for the build to finish, then re-capture - the same call returns correct content once it is done. Detect completion by polling fusion_get_app_state until the <part>_3d document reports isSaved:true, or read the bound .lbr until it has one <package3d wip_urn=...> per part. Pass ignoreBuildInProgress:true only if you genuinely want a mid-build frame."});
    }
    let lbr_path = args["libraryPath"].as_str().map(|s| s.replace('\\', "/")).filter(|s| !s.is_empty()).or_else(|| remembered_open_library(&ctx.state_dir)).unwrap_or_default();
    let mut resolved: std::collections::BTreeMap<(String, String), String> = Default::default();
    let mut name_errors: Vec<Value> = Vec::new();
    if !lbr_path.is_empty() {
        for pkg in &pkgs { for v in &views { let ext = view_ext(v); if ext == "3d" { continue; }
            match resolve_artifact_name(pkg, &ext, &lbr_path) { Ok(r) => { resolved.insert((pkg.clone(), ext.to_string()), r); }, Err(e) => name_errors.push(json!({"package": pkg, "view": label_of(&ext), "error": e["error"], "available": e["available"], "closestMatches": e["closestMatches"]})) } } }
    }
    let mut out: Vec<Value> = Vec::new();
    for pkg in &pkgs {
        for v in &views {
            let ext = view_ext(v);
            let ext = ext.as_str();
            if ext == "3d" { out.push(capture_3d_view(ctx, pkg, &lbr_path, settle, pass)); continue; }
            if !lbr_path.is_empty() && !resolved.contains_key(&(pkg.clone(), ext.to_string())) {
                let bad = name_errors.iter().find(|e| e["package"].as_str() == Some(pkg) && e["view"].as_str() == Some(label_of(ext).as_str())).cloned().unwrap_or(json!({}));
                out.push(json!({"package": pkg, "view": label_of(ext), "path": Value::Null, "ok": false, "skipped": "name_not_in_library", "error": bad["error"], "available": bad["available"].as_array().map(|a| a.iter().take(20).cloned().collect::<Vec<_>>()).unwrap_or_default(), "closestMatches": bad["closestMatches"],
                    "_hint": "No EDIT was issued and no dialog was raised - this name is not in the library, so opening it would have popped Fusion's blocking 'Create new ...?' modal. Pick a name from `available`."}));
                continue;
            }
            let name = resolved.get(&(pkg.clone(), ext.to_string())).cloned().unwrap_or_else(|| pkg.clone());
            let er = pass("electron_run", json!({"command": format!("EDIT {name}.{ext}"), "confirm": "no"}));
            std::thread::sleep(Duration::from_secs_f64(settle));
            let dismissed = dismiss_dialogs_bg(ctx);
            if !dismissed.is_empty() {
                eprintln!("[fusion-bridge] PRE-CHECK HOLE: EDIT {name}.{ext} still raised a dialog (lbr_path={lbr_path:?}). Prevention failed; dismissal should not be load-bearing.");
                crate::fusion_prefs::action_ledger(&ctx.state_dir, "dialog_precheck_hole", json!({"name": name, "ext": ext, "lbrPath": lbr_path, "dismissed": dismissed}));
            }
            let _ = pass("electron_run", json!({"command": "WINDOW FIT"}));
            std::thread::sleep(Duration::from_millis(600));
            let shot = capture_labeled(ctx, &format!("{pkg}_{}", label_of(ext)));
            let st = pass("get_app_state", json!({}));
            let editor_ws = st["data"]["activeWorkspace"].as_str().or(st["activeWorkspace"].as_str()).map(str::to_string);
            // the name (Electronics Library) or, from add-in 1.0.29, the workspace id
            let editor_ok = match &editor_ws { Some(w) => { let l = w.to_lowercase(); l.contains("electronics library") || (l.contains("librar") && !l.contains("design")) }, None => false };
            let mut entry = json!({"package": pkg, "view": label_of(ext), "path": shot, "dialogDismissed": if dismissed.is_empty() { Value::Null } else { json!(dismissed) }, "capturedInWorkspace": editor_ws,
                "ok": er["success"].as_bool().unwrap_or(true) && !entry_null(&shot) && dismissed.is_empty() && editor_ok});
            if !editor_ok && editor_ws.is_none() { entry["errorCode"] = json!("editor_unverified"); entry["error"] = json!("Could not read which editor was active at capture time (the add-in did not answer), so this frame cannot be confirmed to show the view it is labelled with."); entry["_hint"] = json!("Not necessarily a bad frame - it is an unverified one. Check fusion_addin_status and re-capture."); }
            else if !editor_ok { entry["errorCode"] = json!("wrong_editor_captured"); entry["error"] = json!(format!("Captured while {} was active, not the Electronics Library editor - the frame does not show the {} view it is labelled with.", fusion_lbr::py_repr_str(editor_ws.as_deref().unwrap_or("None")), label_of(ext))); }
            out.push(entry);
        }
    }
    let n_ok = out.iter().filter(|o| o["ok"].as_bool() == Some(true)).count();
    json!({"success": n_ok > 0, "captured": out,
        "_hint": format!("Captured {n_ok}/{} library-editor views to %TEMP%/adom-fusion-shots (ab pulls them to you as localFullPath; or pull with desktop_pull_file). SYMBOL = full pinout; FOOTPRINT = pads + layer stack; COMPONENT = the Content Manager device view with the footprint<->package Mapped check + pin/pad counts. ⚠️ NAMES: the COMPONENT (.dev) view opens by DEVICESET name (e.g. R-0603-10K). But SYMBOL (.sym) and FOOTPRINT (.pac) open by the SYMBOL / PACKAGE name - in a SHARED-FOOTPRINT library (one symbol/footprint per package, many value devicesets) that is the COMBO name (e.g. 'R-0603'), NOT the deviceset name ('R-0603-10K'). Passing a deviceset name to .sym/.pac makes Fusion pop 'Create new symbol/footprint?'; the bridge now auto-dismisses that in the BACKGROUND (Cancel/No, no focus steal - see entries with dialogDismissed) and marks the entry ok:false. Pass libraryPath so names are resolved from the file up front.", out.len())})
}
fn entry_null(v: &Option<String>) -> bool { v.is_none() }

const INSPECT_BODIES_SCRIPT: &str = include_str!("inspect_bodies_script.py");

/// `fusion_open_cloud_file`: the add-in open, then the classified blocker and the post-open capture.
pub fn open_cloud_file(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    // A saved electronics design is several files of one name (.fprj, .fsch, .fbrd, .f3d). Opening
    // the board or schematic file raises Fusion's "links to multiple Electronics designs" chooser,
    // which cannot be answered headlessly (live 2026-10-27): with no extension or id named, the
    // design (.fprj) of that name is tried first, and the plain name match only when there is none.
    let named = args["fileExtension"].as_str().map_or(false, |e| !e.is_empty()) || args["fileId"].is_string() || args["urn"].is_string();
    let mut r = if named { pass("open_cloud_file", args.clone()) } else {
        let mut a = args.clone();
        a["fileExtension"] = json!("fprj");
        let first = pass("open_cloud_file", a);
        let missing = first["success"].as_bool() != Some(true)
            && first["error"].as_str().map_or(false, |e| { let e = e.to_lowercase(); e.contains("not found") || e.contains("no file") || e.contains("could not find") });
        if missing { pass("open_cloud_file", args.clone()) } else { let mut f = first; f["openedDesign"] = json!(true); f }
    };
    let settle = if r["success"].as_bool() == Some(true) { 2.0 } else {
        if let Some(ab) = fusion_core::ab::Loopback::from_env() {
            let dialogs = crate::small_verbs::classify_blocking_dialogs(&ab, &ctx.caller, &ctx.pids);
            if !dialogs.is_empty() { r["dialogBlocking"] = json!(true); r["blockingDialogs"] = json!(dialogs); r["hint"] = json!("A modal dialog is blocking Fusion — see blockingDialogs for the classified cause + resolution. The multi-link 'Select Electronics Design File' picker is a CEF dialog that must be resolved on the desktop (its list isn't keyboard/Win32 navigable); recovery prompts use fusion_dismiss_recovery; an update nag must be cleared on the desktop."); }
        }
        1.0
    };
    let (shots, _) = crate::lifecycle::post_open_screenshot(ctx, settle);
    if !r["data"].is_object() { r["data"] = json!({}); }
    r["data"]["postOpenScreenshot"] = shots;
    r
}

/// `fusion_save_lbr`: the open library as a .flbr through Document.CopyToDesktop.
pub fn save_lbr(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut out = args["outputPath"].as_str().unwrap_or("").replace('\\', "/");
    if out.is_empty() { return json!({"success": false, "error": "No outputPath specified"}); }
    if !out.to_lowercase().ends_with(".flbr") { out.push_str(".flbr"); }
    if let Some(parent) = std::path::Path::new(&out).parent() { if !parent.as_os_str().is_empty() && !parent.exists() { if let Err(e) = std::fs::create_dir_all(parent) { return json!({"success": false, "error": format!("Cannot create directory {}: {e}", parent.to_string_lossy())}); } } }
    let (target, staged) = match crate::fusionpath::output_for_fusion(&ctx.state_dir, &out) { Ok(t) => t, Err(e) => return json!({"success": false, "errorCode": "stage_failed", "error": e}) };
    let r = pass("execute_text_command", json!({"command": format!("Document.CopyToDesktop {}", target)}));
    if r["success"].as_bool() != Some(true) { return json!({"success": false, "error": format!("Document.CopyToDesktop failed: {}", r["error"].as_str().unwrap_or("unknown")), "data": {"outputPath": out}}); }
    if let Some(s) = staged { if let Err(e) = crate::fusionpath::finish_output(&s, &out) { return json!({"success": false, "errorCode": "stage_failed", "error": e, "data": {"outputPath": out, "stagedAt": s}}); } }
    let mut result = match std::fs::metadata(&out) { Ok(m) => json!({"success": true, "output": format!("Saved library to {} ({} bytes)", out.rsplit('/').next().unwrap_or(&out), m.len()), "data": {"outputPath": out, "fileSize": m.len()}}), Err(_) => json!({"success": true, "output": "Document.CopyToDesktop executed (file may still be writing)", "data": {"outputPath": out}}) };
    let (shots, _) = crate::lifecycle::post_open_screenshot(ctx, 1.0);
    result["data"]["postOpenScreenshot"] = shots;
    result
}

/// `fusion_cleanup_cloud_files`: delete an exact list of lineage urns, never by name.
pub fn cleanup_cloud_files(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let ids: Vec<String> = args["fileIds"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if ids.is_empty() { return json!({"success": false, "error": "fileIds [lineage urns] required.", "_hint": "Get them from fusion_aps_browse {projectId, folderId} (item .id)."}); }
    let (project_name, folder_path) = (args["projectName"].as_str().unwrap_or(""), args["folderPath"].as_str().unwrap_or(""));
    let (mut deleted, mut failed): (Vec<String>, Vec<Value>) = (vec![], vec![]);
    for fid in &ids {
        let r = pass("delete_cloud_file", json!({"fileId": fid, "projectName": project_name, "folderPath": folder_path}));
        if r["success"].as_bool() == Some(true) { deleted.push(fid.clone()); } else { failed.push(json!({"fileId": fid, "error": r["error"].as_str().unwrap_or("").chars().take(90).collect::<String>()})); }
    }
    json!({"success": !deleted.is_empty() || failed.is_empty(), "deletedCount": deleted.len(), "failedCount": failed.len(), "failed": failed.iter().take(25).cloned().collect::<Vec<_>>(),
        "_hint": format!("Deleted {}/{} cloud files by PRECISE lineage urn (no name-guessing, so teammates' files are untouched). NOTE: a large list takes minutes and the relay request may time out at ~60s while the deletes continue server-side - re-browse the folder to confirm the count dropped. Anything in failed[] usually means the file was already gone or you lack delete permission. See the fusion-cloud-hygiene skill.", deleted.len(), ids.len())})
}

/// `fusion_inspect_bodies`: what actually got built, in millimetres.
pub fn inspect_bodies(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let world = args["worldSpace"].as_bool().unwrap_or(false);
    let occ = args["occurrence"].as_str().unwrap_or("");
    let script = INSPECT_BODIES_SCRIPT.replace("__WORLD__", if world { "True" } else { "False" }).replace("__OCC__", &if occ.is_empty() { "None".to_string() } else { fusion_lbr::py_repr_str(occ) });
    let mut res = pass("run_modeling_script", json!({"script": script}));
    // the 2D board or schematic in front has no Design to read: the design's 3D board is brought
    // forward and the read runs again (the battery met this right after review_board)
    let mut shown_3d = false;
    if res["success"].as_bool() != Some(true) && res["error"].as_str().map(|e| e.contains("rootComponent")).unwrap_or(false) {
        if pass("show_3d_board", json!({}))["success"].as_bool() == Some(true) { shown_3d = true; res = pass("run_modeling_script", json!({"script": script})); }
    }
    if res["success"].as_bool() != Some(true) { return res; }
    let data = res["data"].clone();
    let Some(mut payload) = data.get("result").filter(|p| p.is_object()).cloned() else { return json!({"success": false, "error": "inspect_bodies returned no structured result", "errorCode": "inspect_failed", "data": data, "_hint": "The read-back script ran but produced no result dict. Usually means no active Design; open a document and retry."}) };
    let n = payload["bodyCount"].as_i64().unwrap_or(0);
    payload["success"] = json!(true);
    if shown_3d { payload["shown3d"] = json!(true); }
    payload["_hint"] = json!(if n > 0 { format!("Bounding boxes, volumes and areas are in MILLIMETRES (Fusion's API is centimetres; converted here so you do not have to). Check bbox Z against where the part SHOULD sit - a wrong offset renders perfectly and is invisible in a screenshot. cylindricalFaceCount is the cheap way to confirm holes were actually cut: a symmetric cut that reaches nothing fails SILENTLY with no exception, and the count is what surfaces it. {}", if !world { "Pass worldSpace:true to resolve bodies through their occurrence transforms and verify assembly placement." } else { "worldSpace:true - occurrence bodies are proxies already resolved into assembly space." }) } else { "No bodies found. If you expected some, the active document may not be the one you built into (pass expectDocument on mutating verbs to catch that), or the design is empty.".to_string() });
    payload
}

const SUPPORTED_DESIGN_EXT: &[&str] = &[".f3d", ".f3z", ".step", ".stp", ".iges", ".igs", ".sat", ".stl", ".3mf", ".obj"];

/// `fusion_open_design`: a local design file (f3d, STEP, IGES, ...) through the add-in's import.
pub fn open_design(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let file_path = args["filePath"].as_str().unwrap_or("").to_string();
    if ctx.fusion_exe.is_none() { return json!({"success": false, "error": "Fusion 360 is not installed on this machine.", "errorCode": "fusion_not_installed", "_hint": "Fusion 360 isn't installed. Do NOT tell the user to install it themselves - this claim was simply STALE (the bridge silent-installs Fusion via the fusion-onboarding skill and drives the Autodesk sign-in). OFFER to install it for them and do it on a yes, then retry this open."}); }
    if ctx.host.fusion_processes().is_empty() { return json!({"success": false, "error": "Fusion 360 is installed but not running.", "errorCode": "fusion_not_running", "_hint": "Call fusion_start to start Fusion 360 and wait for the add-in to be ready, then retry this command."}); }
    if file_path.is_empty() { return json!({"success": true, "output": "Fusion 360 is running."}); }
    let p = std::path::Path::new(&file_path);
    if !p.exists() { return json!({"success": false, "error": format!("File not found: {file_path}"), "errorCode": "file_not_found", "_hint": "Verify the path with `ls` or check if the file is in the cloud instead of local — use fusion_aps_search {query} to find cloud files, then fusion_open_cloud_file to open them."}); }
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
    if !SUPPORTED_DESIGN_EXT.contains(&ext.as_str()) { return json!({"success": false, "error": format!("Unsupported file type: {ext}. Supported: {}", { let mut v: Vec<&str> = SUPPORTED_DESIGN_EXT.to_vec(); v.sort(); v.join(", ") }), "errorCode": "unsupported_extension", "_hint": "Use a supported extension, or for electronics files (.fsch/.fbrd/.flbr) use fusion_import_electronics instead."}); }
    if !crate::launch::wait_for_addin(10) { return json!({"success": false, "error": "Fusion 360 is running but AdomBridge add-in not responding.", "errorCode": "fusion_addin_not_responding", "_hint": "Fix it YOURSELF - never ask the user: (1) run install_addin() so the add-in lands in EVERY Fusion add-in dir (Fusion moved it to %APPDATA%\\Autodesk\\FusionAddins on 2025+ builds; an add-in in the legacy API\\AddIns dir is silently ignored - issue #63), then (2) restart Fusion via fusion_stop + fusion_start so it rescans."}); }
    pass("import_step", json!({"filePath": file_path}))
}

/// `fusion_new_electronics_from_eagle`: Fusion's own EAGLE importer, its two Open dialogs driven through ab.
/// A blank EAGLE schematic: one empty sheet, the schematic layers, nothing else. Opened through
/// Document.newDesignFromLocal it becomes a new local electronics design with the schematic
/// editor active, with no dialog anywhere (the create path of `fusion_schematic`).
const BLANK_SCH: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE eagle SYSTEM "eagle.dtd">
<eagle version="9.6.2">
<drawing>
<settings><setting alwaysvectorfont="no"/><setting verticaltext="up"/></settings>
<grid distance="0.1" unitdist="inch" unit="inch" style="lines" multiple="1" display="no" altdistance="0.01" altunitdist="inch" altunit="inch"/>
<layers>
<layer number="88" name="SimResults" color="9" fill="1" visible="yes" active="yes"/>
<layer number="89" name="SimProbes" color="9" fill="1" visible="yes" active="yes"/>
<layer number="90" name="Modules" color="5" fill="1" visible="yes" active="yes"/>
<layer number="91" name="Nets" color="2" fill="1" visible="yes" active="yes"/>
<layer number="92" name="Busses" color="1" fill="1" visible="yes" active="yes"/>
<layer number="93" name="Pins" color="2" fill="1" visible="no" active="yes"/>
<layer number="94" name="Symbols" color="4" fill="1" visible="yes" active="yes"/>
<layer number="95" name="Names" color="7" fill="1" visible="yes" active="yes"/>
<layer number="96" name="Values" color="7" fill="1" visible="yes" active="yes"/>
<layer number="97" name="Info" color="7" fill="1" visible="yes" active="yes"/>
<layer number="98" name="Guide" color="6" fill="1" visible="yes" active="yes"/>
</layers>
<schematic xreflabel="%F%N/%S.%C%R" xrefpart="/%S.%C%R">
<libraries/>
<attributes/>
<variantdefs/>
<classes><class number="0" name="default" width="0" drill="0"/></classes>
<parts/>
<sheets><sheet><plain/><instances/><busses/><nets/></sheet></sheets>
</schematic>
</drawing>
</eagle>
"#;

/// `fusion_schematic`: THE front door for a schematic. `action:"open"` takes whatever names
/// the design (a cloud fileName / urn / query, a local .sch with or without its .brd, a
/// Fusion-native .fsch) and lands in the schematic editor; `action:"create"` writes a blank
/// EAGLE schematic and opens it as a new local design, dialog-free; `action:"show"` switches
/// the open design to its schematic view. Every path says which verb it used (`via`).
pub fn schematic(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let action = args["action"].as_str().unwrap_or("").trim().to_lowercase();
    let a = |k: &str| args[k].as_str().map(|s| s.trim().replace('\\', "/")).filter(|s| !s.is_empty());
    let capture = args["capture"].clone();
    let menu = "fusion_schematic takes action:\"open\" with ONE of: fileName (+projectName) for a cloud design, urn for a cloud design by lineage, query to find-and-open by name, schPath for a local EAGLE .sch (its sibling .brd is imported too unless withBoard:false), or filePath for a Fusion-native .fsch; action:\"create\" with an optional name for a new blank schematic (dialog-free); action:\"show\" to switch the open design to its schematic view. A schematic is a VIEW of an electronics design, never a file on its own.";
    match action.as_str() {
        "show" => { let mut r = show_with_document("show_schematic", &json!({"capture": capture, "documentName": args["documentName"], "name": args["name"]}), pass); r["via"] = json!("show_schematic"); r }
        "create" => {
            let name: String = a("name").unwrap_or_else(|| format!("adom-schematic-{}", (now_f() as u64) % 100000)).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
            let root = match crate::fusionpath::stage_root(&ctx.state_dir) { Ok(r) => r.join("new"), Err(e) => return json!({"success": false, "errorCode": "stage_failed", "error": e}) };
            let _ = std::fs::create_dir_all(&root);
            let path = root.join(format!("{name}.sch")).to_string_lossy().replace('\\', "/");
            if let Err(e) = std::fs::write(&path, BLANK_SCH) { return json!({"success": false, "errorCode": "write_failed", "error": format!("cannot write {path}: {e}")}); }
            let mut r = pass("open_schematic", json!({"filePath": path}));
            r["via"] = json!("blank EAGLE .sch written on the box, then open_schematic (Document.newDesignFromLocal, no dialog)");
            r["schPath"] = json!(path); r["documentName"] = json!(name);
            if r["success"].as_bool() == Some(true) {
                r["_hint"] = json!(format!("A new electronics design named {name} is open with its (empty) schematic editor active. NEXT: put circuitry on it. Interactive editor commands (ADD, MOVE) are blocked because they wait for a mouse; author the schematic as an EAGLE .sch offline (adom-schematic / adom-lbr generate real parts and nets), then fusion_schematic {{action:\"open\", schPath}} re-opens it, or fusion_new_electronics_from_eagle {{schPath, brdPath}} when a placed board should come with it. fusion_export_eagle_source saves what is open; fusion_save_to_cloud puts it in the hub."));
            }
            r
        }
        "open" => {
            if let Some(p) = a("schPath").or_else(|| a("filePath")).filter(|p| p.to_lowercase().ends_with(".sch")) {
                let sibling = format!("{}.brd", &p[..p.len() - 4]);
                let brd = a("brdPath").or_else(|| if std::path::Path::new(&sibling).is_file() { Some(sibling.clone()) } else { None });
                if args["withBoard"].as_bool() != Some(false) {
                    if let Some(b) = brd {
                        let mut r = new_electronics_from_eagle(ctx, &json!({"schPath": p, "brdPath": b}), pass);
                        r["via"] = json!("new_electronics_from_eagle (schematic + board, parts instantiated; this one drives Fusion's own Open dialogs in the background because Autodesk offers no API for it)");
                        return r;
                    }
                }
                let mut r = pass("open_schematic", json!({"filePath": p})); r["via"] = json!("open_schematic (Document.newDesignFromLocal, dialog-free, schematic only)"); return r;
            }
            if let Some(p) = a("filePath").filter(|p| p.to_lowercase().ends_with(".fsch")) { let mut r = pass("import_electronics", json!({"filePath": p})); r["via"] = json!("import_electronics (Fusion-native .fsch)"); return r; }
            let opened = if let Some(u) = a("urn") { Some(("open_by_urn", pass("open_by_urn", json!({"urn": u, "intent": "electronics"})))) }
                else if let Some(f) = a("fileName") { Some(("open_cloud_file", open_cloud_file(ctx, &json!({"fileName": f, "projectName": args["projectName"], "folderPath": args["folderPath"]}), pass))) }
                else if let Some(q) = a("query") { Some(("aps_open", aps_open(ctx, &json!({"query": q, "wait": true}), pass))) }
                else { None };
            let Some((how, r)) = opened else { return json!({"success": false, "errorCode": "schematic_source_required", "error": "Nothing names the design to open.", "_hint": menu}); };
            if r["success"].as_bool() != Some(true) { let mut r = r; r["via"] = json!(how); return r; }
            std::thread::sleep(Duration::from_secs(2));
            // The schematic lives in the electronics DESIGN document. A name search can land on
            // the design's 3D PCB or a mechanical file (DesignProductType), which has no
            // schematic; then try the exact cloud name once, and otherwise say which documents
            // exist so the caller picks the design, instead of a bare "Please open or create".
            let mut how = how.to_string();
            let kind = |st: &Value| st["data"]["productType"].as_str().or(st["productType"].as_str()).unwrap_or("").to_string();
            let mut st = pass("get_app_state", json!({}));
            if kind(&st) == "DesignProductType" && how != "open_cloud_file" {
                if let Some(nm) = a("query").or_else(|| a("fileName")) {
                    let r2 = open_cloud_file(ctx, &json!({"fileName": nm, "projectName": args["projectName"]}), pass);
                    if r2["success"].as_bool() == Some(true) { how = format!("{how} landed on a 3D/mechanical document, then open_cloud_file by exact name"); std::thread::sleep(Duration::from_secs(2)); st = pass("get_app_state", json!({})); }
                }
            }
            if kind(&st) == "DesignProductType" {
                let cands = crate::native_aps_search(&json!({"query": a("query").or_else(|| a("fileName")).unwrap_or_default(), "limit": 8}));
                let names: Vec<Value> = cands["data"]["results"].as_array().cloned().unwrap_or_default().iter().map(|c| json!({"name": c["name"], "projectName": c["projectName"], "id": c["id"]})).collect();
                return json!({"success": false, "errorCode": "not_an_electronics_design", "via": how, "activeDocument": st["data"]["activeDocument"], "productType": "DesignProductType", "candidates": names,
                    "error": format!("'{}' opened, but it is a 3D or mechanical document (DesignProductType), which has no schematic.", st["data"]["activeDocument"].as_str().unwrap_or("?")),
                    "_hint": "Open the electronics DESIGN document (EcadDesignProductType): pick it from candidates (the design usually carries the plain name; its 3D PCB and drawings are siblings) and call fusion_schematic {action:\"open\", urn} with that lineage, or {fileName, projectName} with its exact name."});
            }
            let mut s = pass("show_schematic", json!({"capture": capture}));
            s["opened"] = json!({"via": how.clone(), "output": r["output"], "hint": r["_hint"]}); s["via"] = json!(format!("{how} + show_schematic"));
            if s["success"].as_bool() != Some(true) { s["_hint"] = json!(format!("The design opened ({how}) but the switch to its schematic view failed: {}. If the open landed on a mechanical design (DesignProductType) it has no schematic; check fusion_get_app_state and open the electronics DESIGN document instead.", s["error"].as_str().unwrap_or("unknown"))); }
            s
        }
        _ => json!({"success": false, "errorCode": "schematic_action_required", "error": format!("action must be open, create or show (got '{action}')."), "_hint": menu}),
    }
}

/// `fusion_show_attributes`: a component's metadata, as data AND on screen. The attributes
/// (MPN, manufacturer, distributor part numbers, datasheet, wiki link) are read from the
/// library file; the DEVICE editor is opened for the deviceset (EDIT <name>.dev, no dialog: the
/// symbol, the package table with the Mapped check, the footprint and 3D previews in one
/// frame) and captured. Fusion's own "Variant Attributes" table lives behind a "..." button in
/// an opaque Qt panel with no UIA children, so it is reachable only by positioned clicks; the
/// reply carries that recipe instead of pretending (page issue #93).
pub fn show_attributes(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let ds = args["deviceset"].as_str().unwrap_or("").trim().to_string();
    let lbr = args["libraryPath"].as_str().unwrap_or("").replace('\\', "/");
    if ds.is_empty() { return json!({"success": false, "errorCode": "deviceset_required", "error": "deviceset is required (the component's name in the library).", "_hint": "fusion_library_parts {libraryPath} lists the devicesets of a library."}); }
    let mut attributes = Value::Null;
    if !lbr.is_empty() {
        let xml = match std::fs::read_to_string(&lbr) { Ok(x) => x, Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": format!("could not read {lbr}: {e}")}) };
        let md = match fusion_lbr::library_part_metadata(&xml) { Ok(m) => m, Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": e.to_string()}) };
        let Some(dsm) = md.devicesets.iter().find(|d| d.name.eq_ignore_ascii_case(&ds)) else {
            return json!({"success": false, "errorCode": "deviceset_not_in_library", "deviceset": ds, "available": md.devicesets.iter().map(|d| d.name.clone()).collect::<Vec<_>>(), "error": format!("{ds} is not in {lbr}")});
        };
        attributes = json!({"deviceset": dsm.name, "variants": dsm.variants.iter().map(|v| json!({"variant": v.full_name, "attributes": v.attributes})).collect::<Vec<_>>()});
        let op = open_lbr(ctx, &json!({"filePath": lbr}), pass);
        if op["success"].as_bool() != Some(true) { return json!({"success": false, "errorCode": "open_lbr_failed", "error": op["error"], "attributes": attributes}); }
        std::thread::sleep(Duration::from_secs(1));
    }
    let ed = pass("electron_run", json!({"command": format!("EDIT {ds}.dev"), "confirm": "no"}));
    if ed["success"].as_bool() != Some(true) { return json!({"success": false, "errorCode": "device_editor_failed", "attributes": attributes, "error": format!("EDIT {ds}.dev failed: {}", ed["error"].as_str().unwrap_or("unknown")), "_hint": "A library must be the active document (pass libraryPath, or fusion_open_lbr first)."}); }
    std::thread::sleep(Duration::from_millis(1200));
    let _ = pass("execute_text_command", json!({"command": "Electron.run WINDOW FIT;"}));
    let shot = if args["capture"].as_bool() != Some(false) { capture_labeled(ctx, &format!("attributes_{ds}")) } else { None };
    // Fusion's own "Variant Attributes" table. EAGLE's ATTRIBUTE command, run in the device
    // editor, opens it directly: no "..." button, no positioned clicks (found 2026-09-20; the
    // first cut of this verb only described a click recipe). The command blocks Fusion's main
    // thread until the dialog closes, so it is fired from a side thread while this one reads the
    // dialog through UI Automation (full values, the on-screen column truncates them), captures
    // the dialog's own window, and then Cancels it. The bridge never leaves it on screen.
    let mut dialog = json!({"opened": false});
    if args["dialog"].as_bool() != Some(false) {
        let fire = std::thread::spawn(|| { let _ = crate::addin::addin_http("electron_run", &json!({"command": "ATTRIBUTE", "confirm": "no"}), 45); });
        let mut found: Option<Value> = None;
        for _ in 0..24 {
            std::thread::sleep(Duration::from_millis(500));
            if let Some(d) = crate::lifecycle::dialog_inventory(ctx).into_iter().find(|d| d["title"].as_str().map(|t| t.contains("Variant Attributes") || t.contains("Attributes")).unwrap_or(false)) { found = Some(d); break; }
        }
        if let Some(d) = found {
            let hwnd = d["hwnd"].as_i64().unwrap_or(0);
            let mut dshot = Value::Null;
            if let Some(ab) = fusion_core::ab::Loopback::from_env() {
                use fusion_core::ab::AbClient; use base64::Engine;
                if args["capture"].as_bool() != Some(false) {
                    if let Ok(r) = ab.call("desktop_screenshot_window", json!({"hwnd": hwnd}), &ctx.caller, 20) {
                        let img = r["image"].as_str().or(r["data"]["image"].as_str()).unwrap_or("");
                        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(img) {
                            let dir_buf = shots_dir(); let dir = dir_buf.as_path();
                            let dst = dir.join(format!("fusion-variant-attributes_{}-{}.png", ds.replace(|c: char| !c.is_ascii_alphanumeric(), "_"), (now_f() * 1000.0) as u64));
                            if std::fs::write(&dst, bytes).is_ok() { dshot = json!(dst.to_string_lossy().replace('\\', "/")); }
                        }
                    }
                }
                let _ = ab.call("desktop_ui_click", json!({"hwnd": hwnd, "name": "Cancel", "role": "button"}), &ctx.caller, 12);
                std::thread::sleep(Duration::from_millis(700));
                // Cancel did not land: WM_CLOSE is the same answer, and nothing may be left open
                if crate::lifecycle::dialog_inventory(ctx).iter().any(|x| x["hwnd"].as_i64() == Some(hwnd)) { let _ = ab.call("desktop_close_window", json!({"hwnd": hwnd}), &ctx.caller, 10); }
            }
            dialog = json!({"opened": true, "closed": true, "title": d["title"], "hwnd": hwnd, "text": d["text"], "screenshot": dshot,
                "note": "text is the whole table read through UI Automation (full values). The screenshot shows Fusion's own dialog, where the DEFAULT column truncates long URLs."});
        } else {
            dialog = json!({"opened": false, "note": "ATTRIBUTE did not raise the Variant Attributes dialog within 12 s (a deviceset with no device, or the device editor did not take focus). attributes in this reply is still complete."});
        }
        let _ = fire.join();
    }
    json!({"success": true, "deviceset": ds, "attributes": attributes, "deviceEditor": true, "screenshot": shot, "variantAttributesDialog": dialog,
        "onScreen": "the DEVICE editor for the deviceset (symbol, Packages table with the Mapped check, footprint and 3D previews), then Fusion's own Variant Attributes table, opened and closed by the bridge",
        "recording": "To FILM the table: fusion_show_attributes {dialog:false}, start desktop_record_window_start on Fusion, run fusion_electron_run ATTRIBUTE yourself from a second call, record the dialog's own hwnd (fusion_state.dialogs[]), then desktop_ui_click Cancel. A recording of the main window does not include an owned dialog.",
        "_hint": "attributes = the component's metadata from the library file, both EAGLE <attribute> and Fusion <att_value> dialects, full values. variantAttributesDialog.text = the same table as Fusion shows it, read by UI Automation; .screenshot = the dialog itself. The dialog is already closed. Pass dialog:false to skip it."})
}

/// Where an EAGLE import run keeps its state (`fusion_eagle_import_status {runId}`).
fn eagle_run_path(state_dir: &std::path::Path, run_id: &str) -> std::path::PathBuf { state_dir.join("eagle-imports").join(format!("{run_id}.json")) }
pub fn now_pub() -> f64 { now_f() }
pub fn eagle_run_write_pub(state_dir: &std::path::Path, run_id: &str, v: &Value) { eagle_run_write(state_dir, run_id, v) }
fn eagle_run_write(state_dir: &std::path::Path, run_id: &str, v: &Value) { let p = eagle_run_path(state_dir, run_id); let _ = std::fs::create_dir_all(p.parent().unwrap()); let _ = std::fs::write(&p, v.to_string()); }

/// `fusion_eagle_import_status`: the run record of a background EAGLE import.
pub fn eagle_import_status(ctx: &Ctx, args: &Value) -> Value {
    // no runId: the latest import on this box (a caller that lost the runId can still read the result, #133)
    let latest = || -> Option<String> {
        std::fs::read_dir(ctx.state_dir.join("eagle-imports")).ok()?.flatten()
            .filter_map(|e| { let m = e.metadata().ok()?.modified().ok()?; Some((m, e.path().file_stem()?.to_string_lossy().to_string())) })
            .max_by_key(|(m, _)| *m).map(|(_, id)| id)
    };
    let Some(rid) = args["runId"].as_str().filter(|s| !s.is_empty()).map(str::to_string).or_else(latest) else { return json!({"success": false, "errorCode": "no_import_run", "error": "No EAGLE import has run on this box."}) };
    let rid = rid.as_str();
    match std::fs::read_to_string(eagle_run_path(&ctx.state_dir, rid)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(mut v) => { v["success"] = json!(true); v["runId"] = json!(rid); v }
        None => json!({"success": false, "errorCode": "unknown_run_id", "error": format!("no EAGLE import run {rid} on this box")}),
    }
}

/// A board or schematic editor in front, by the workspace's name (PCB Editor, Schematic Editor) or, as
/// add-in 1.0.29 answered, its id (BoardLayoutEnvironement, SchematicEnvironment ...).
fn electronics_editor(ws: &str) -> bool {
    let l = ws.to_lowercase();
    ["pcb editor", "schematic editor", "board layout", "3d pcb"].contains(&l.as_str()) || l.contains("boardlayout") || l.starts_with("schematic")
}

/// Close the Untitled documents a failed import opened (more Untitled than before it started); a
/// never-saved Untitled holds nothing of the user's. Returns how many were closed.
fn close_new_untitled(before: &[String], pass: &dyn Fn(&str, Value) -> Value) -> usize {
    let now: Vec<String> = pass("get_app_state", json!({}))["data"]["documents"].as_array().cloned().unwrap_or_default().iter().filter_map(|d| d["name"].as_str().map(str::to_string)).collect();
    let count = |v: &[String]| v.iter().filter(|n| n.starts_with("Untitled")).count();
    let extra = count(&now).saturating_sub(count(before));
    let mut closed = 0;
    for _ in 0..extra { if pass("close_document", json!({"name": "Untitled"}))["success"].as_bool() == Some(true) { closed += 1; } }
    closed
}

/// Bumped by each import: a late-dialog watcher left by an earlier import stops when a new one starts,
/// so it can never close the new import's dialog.
static EAGLE_IMPORT_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The importer's Open dialog on this machine: by title in ab's window list, else any native file
/// dialog on Fusion's processes (a Windows that is not in English titles it differently: the common
/// dialog's File name box, control 1148, is what proves it). An ab failure is noted in `ab_error`.
fn eagle_open_dialog(ab: &dyn fusion_core::ab::AbClient, caller: &fusion_core::ab::Caller, pids: &[u32], ab_error: &std::cell::RefCell<Option<String>>) -> Option<i64> {
    match ab.call("desktop_list_windows", json!({}), caller, 10) {
        Ok(v) => {
            let found = v["data"]["windows"].as_array().or_else(|| v["windows"].as_array()).cloned().unwrap_or_default().iter()
                .find(|w| w["title"].as_str().unwrap_or("").trim().to_lowercase() == "open").and_then(|w| w["hwnd"].as_i64());
            if found.is_some() { return found; }
        }
        Err(e) => { *ab_error.borrow_mut() = Some(format!("desktop_list_windows: {e}")); }
    }
    crate::small_verbs::native_file_dialogs(ab, caller, pids).iter().find_map(|w| {
        let h = w["hwnd"].as_i64()?;
        if w["title"].as_str().unwrap_or("").trim().eq_ignore_ascii_case("open") { return Some(h); }
        ab.call("desktop_find_controls", json!({"hwnd": h}), caller, 10).ok().filter(|c| c.to_string().contains("1148")).map(|_| h)
    })
}

/// The import's command is queued and cannot be taken back, so when the bridge stops waiting Fusion may
/// still open the dialog later, with nobody left to fill it, blocking Fusion on the user's screen (#136).
/// For ten minutes this closes what that import opens (the .sch dialog, then the .brd one it chains to),
/// and stops at once if another import starts.
fn watch_for_late_open_dialog(state_dir: std::path::PathBuf, caller: fusion_core::ab::Caller, pids: Vec<u32>, wanted: String, gen: u64) {
    std::thread::spawn(move || {
        use std::sync::atomic::Ordering;
        let Some(ab) = fusion_core::ab::Loopback::from_env() else { return };
        let noted = std::cell::RefCell::new(None);
        let (mut closed, mut last_close) = (0, None::<std::time::Instant>);
        for _ in 0..200 {
            std::thread::sleep(Duration::from_secs(3));
            if EAGLE_IMPORT_GEN.load(Ordering::SeqCst) != gen { return; }
            if let Some(h) = eagle_open_dialog(&ab, &caller, &pids, &noted) {
                if crate::small_verbs::close_native_dialog(&ab, &caller, h).0 { closed += 1; last_close = Some(std::time::Instant::now()); }
            } else if last_close.map(|t| t.elapsed() > Duration::from_secs(30)).unwrap_or(false) { break; }
        }
        if closed > 0 { crate::fusion_prefs::action_ledger(&state_dir, "eagle_import_late_dialog_closed", json!({"wanted": wanted, "closed": closed})); }
    });
}

pub fn new_electronics_from_eagle(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    // Async by contract (issue #90): the dialog-driven import outlives ab's relay budget, so the
    // default returns a runId at once and the work runs on this exe's own thread; the caller
    // polls fusion_eagle_import_status while fusion_get_app_state keeps answering.
    if args["wait"].as_bool() != Some(true) {
        let sch0 = args["schPath"].as_str().or(args["filePath"].as_str()).unwrap_or("").replace('\\', "/");
        if sch0.is_empty() || !sch0.to_lowercase().ends_with(".sch") { return json!({"success": false, "errorCode": "sch_path_required", "error": "schPath (a .sch file on the Windows host) is required.", "_hint": "fusion_schematic {action:\"open\", schPath} is the front door; it takes this route only when a .brd sits next to the .sch."}); }
        let brd0 = args["brdPath"].as_str().map(|s| s.replace('\\', "/")).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("{}.brd", &sch0[..sch0.len() - 4]));
        let missing: Vec<&str> = [(&sch0, "schPath"), (&brd0, "brdPath")].iter().filter(|(p, _)| !std::path::Path::new(p.as_str()).is_file()).map(|(_, k)| *k).collect();
        if !missing.is_empty() {
            return json!({"success": false, "errorCode": "eagle_files_missing", "schPath": sch0, "brdPath": brd0, "missing": missing,
                "error": format!("Not started: {} not found on the Fusion machine. This verb drives Fusion's Open dialogs, so it refuses before touching them.", missing.join(" and ")),
                "_hint": "Push both files to the box first (send_files, a space-free folder is best) and pass their absolute Windows paths. No dialog was opened. If you only have a .sch, fusion_schematic {action:\"open\", schPath, withBoard:false} opens it dialog-free (schematic only); if the .brd already carries placed parts, fusion_open_board {filePath} instantiates them with no dialog either."});
        }
        let run_id = format!("{:x}", (now_f() * 1000.0) as u64 ^ (std::process::id() as u64) << 20);
        let expected = std::fs::read_to_string(&brd0).map(|t| t.matches("<element ").count()).unwrap_or(0);
        eagle_run_write(&ctx.state_dir, &run_id, &json!({"stage": "started", "startedAt": now_f(), "schPath": sch0, "brdPath": brd0, "partsExpected": expected}));
        let url = crate::launch::SELF_URL.lock().unwrap().clone();
        let mut a2 = args.clone(); a2["wait"] = json!(true); a2["_runId"] = json!(run_id);
        let (sd, rid) = (ctx.state_dir.clone(), run_id.clone());
        std::thread::spawn(move || {
            let r = url.and_then(|u| ureq::post(&u).timeout(Duration::from_secs(900)).send_json(json!({"command": "fusion_new_electronics_from_eagle", "args": a2})).ok()).and_then(|r| r.into_json::<Value>().ok()).unwrap_or(json!({"success": false, "error": "the exe could not post the import to itself"}));
            let mut v = std::fs::read_to_string(eagle_run_path(&sd, &rid)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(json!({}));
            v["stage"] = json!(if r["success"].as_bool() == Some(true) { "done" } else { "failed" }); v["finishedAt"] = json!(now_f()); v["result"] = r;
            eagle_run_write(&sd, &rid, &v);
        });
        let needs_ok = fusion_core::ab::Loopback::from_env().and_then(|ab| { use fusion_core::ab::AbClient; ab.call("status", json!({}), &ctx.caller, 10).ok() })
            .map(|s| { let x = &s["permissions"]["shell"]; let x = if x.is_object() { x } else { &s["data"]["permissions"]["shell"] }; x["autoApproved"].as_bool() == Some(false) }).unwrap_or(false);
        return crate::display::annotate(json!({"success": true, "started": true, "runId": run_id, "statusVerb": "fusion_eagle_import_status", "partsExpected": expected, "needsApproval": needs_ok,
            "tellUser": if needs_ok { json!("Adom Bridge is asking for your OK to fill in Fusion's Open dialog for the import: click Approve in the Adom Bridge window, on its notification, or in Hydrogen.") } else { Value::Null },
            "dialogsDriven": "Fusion's own Open dialogs (sch, then brd), driven in the background by accessible name; Autodesk offers no API for a sch+brd import",
            "_hint": format!("Import running in the background (it outlives the relay window). Poll fusion_eagle_import_status {{runId:\"{run_id}\"}} until stage is done or failed; fusion_get_app_state keeps answering meanwhile. The record reports documentName, partsExpected ({expected} elements in the .brd) and partsPlaced from the live board; success needs parts on the board.")}));
    }
    let sch = args["schPath"].as_str().or(args["filePath"].as_str()).unwrap_or("").replace('\\', "/");
    if sch.is_empty() || !sch.to_lowercase().ends_with(".sch") { return json!({"success": false, "error": "schPath (a .sch file on the Windows host) is required.", "_hint": "Author the EAGLE .sch + paired .brd (same basename, same folder), stage both to Windows (send_files), then call with {schPath, brdPath}."}); }
    let brd = args["brdPath"].as_str().map(|s| s.replace('\\', "/")).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("{}.brd", &sch[..sch.len() - 4]));
    // Preflight (John, 2026-09-20): this verb drives Fusion's own Open dialogs, so a wrong path
    // used to leave a real Open dialog on the user's screen. Both files must exist on this box
    // before any dialog is touched.
    let missing: Vec<&str> = [(&sch, "schPath"), (&brd, "brdPath")].iter().filter(|(p, _)| !std::path::Path::new(p.as_str()).is_file()).map(|(_, k)| *k).collect();
    if !missing.is_empty() {
        return json!({"success": false, "errorCode": "eagle_files_missing", "schPath": sch, "brdPath": brd, "missing": missing,
            "error": format!("Not started: {} not found on the Fusion machine. This verb drives Fusion's Open dialogs, so it refuses before touching them.", missing.join(" and ")),
            "_hint": "Push both files to the box first (send_files, a space-free folder is best) and pass their absolute Windows paths. No dialog was opened. If you only have a .sch, fusion_schematic {action:\"open\", schPath, withBoard:false} opens it dialog-free (schematic only); if the .brd already carries placed parts, fusion_open_board {filePath} instantiates them with no dialog either."});
    }
    let Some(ab) = fusion_core::ab::Loopback::from_env() else {
        return json!({"success": false, "errorCode": "use_open_board_instead", "error": "ab direct API unavailable - and this dialog-driven import FOREGROUNDS Fusion anyway.",
            "_hint": format!("Don't drive the Open dialogs. If the parts are placed in the .brd (elements + embedded library), call `fusion_open_board {{filePath:\"{brd}\"}}` - it instantiates them via newDesignFromLocal with NO dialogs and NO foreground (the background way). For 3D bodies, embed <packages3d> + per-element package3d_urn in the .brd (see this verb's docstring), then fusion_show_3d_board. Only if you truly have a bare .sch that Fusion must import: fire 'Commands.Start ImportSCHAndBRDCmd' via fusion_execute_text_command and drive the two Open dialogs (foreground; last resort).")});
    };
    use fusion_core::ab::AbClient;
    // Fusion's own crash dialogs (the D3D12 device loss on DisplayLink boxes, the router's
    // "unexpected condition"): an import interrupted by one never finishes, so each wait below
    // checks for them and stops at once, cancelling its Open dialog, instead of running its clock out
    // (page issue #121: the import sat 2.5 min at "started" behind a d3d12 dialog)
    let crash_dialog = || -> Option<Value> {
        crate::lifecycle::dialog_inventory(ctx).into_iter().find(|d| {
            let cat = d["category"].as_str().unwrap_or("");
            let words = format!("{} {}", d["title"].as_str().unwrap_or(""), d["text"].as_str().unwrap_or("")).to_lowercase();
            cat == "fusion_gpu_device_lost" || cat == "fusion_unexpected_condition" || words.contains("d3d12") || words.contains("unexpected condition")
        })
    };
    let crashed = |dlg: Value, open_dialog: Option<i64>, stage: &str, picked: &[String]| -> Value {
        if let Some(h) = open_dialog { let _ = crate::small_verbs::close_native_dialog(&ab, &ctx.caller, h); }
        crate::fusion_prefs::action_ledger(&ctx.state_dir, "eagle_import_crash_dialog", json!({"stage": stage, "dialog": dlg}));
        crate::display::annotate(json!({"success": false, "errorCode": if dlg["category"] == "fusion_gpu_device_lost" { "fusion_graphics_error" } else { "blocked_by_dialog" },
            "stage": stage, "imported": picked, "dialogs": [dlg.clone()], "openDialogCancelled": open_dialog.is_some(),
            "error": format!("Fusion raised '{}' during the import ({stage}); the import cannot finish, so the bridge stopped and cancelled its Open dialog.", dlg["title"].as_str().unwrap_or("a crash dialog")),
            "_hint": format!("{} Save the open documents, restart Fusion (fusion_restart_for_addin {{save:true}}), then run the import again.", dlg["resolution"].as_str().unwrap_or(""))}))
    };
    let import_gen = EAGLE_IMPORT_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let ab_error: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
    let find_open_dialog = || -> Option<i64> { eagle_open_dialog(&ab, &ctx.caller, &ctx.pids, &ab_error) };
    let (sch_base, brd_base) = (sch.rsplit('/').next().unwrap_or("").to_string(), brd.rsplit('/').next().unwrap_or("").to_string());
    // The Open dialogs are filled with UI Automation and keystrokes, which ab gates behind its shell
    // approval even for a bridge (#133: on a PC with no grant every fill was refused, needs_approval).
    // Ask BEFORE any dialog opens: the prompt shows in Adom Bridge (or Hydrogen), and the run waits up to
    // five minutes for the click; no grant, no dialog.
    let approved = |st: &Value| { let s = &st["permissions"]["shell"]; let s = if s.is_object() { s } else { &st["data"]["permissions"]["shell"] }; s["autoApproved"].as_bool() == Some(true) || s["willPrompt"].as_bool() == Some(false) };
    // a status that cannot be read is not an approval: every step of the import needs ab, so opening a
    // dialog now would leave it on the user's screen with nothing able to fill or close it (#136)
    let mut status_now = Err(String::new());
    for attempt in 0..3 {
        if attempt > 0 { std::thread::sleep(Duration::from_secs(3)); }
        status_now = ab.call("status", json!({}), &ctx.caller, 10).map_err(|e| format!("{e}"));
        if status_now.is_ok() { break; }
    }
    let status_now = match status_now {
        Ok(v) => v,
        Err(e) => return json!({"success": false, "errorCode": "ab_unreachable", "imported": [], "dialogOpened": false, "abError": e,
            "error": "Not started: the bridge could not reach Adom Bridge, which it needs to fill in and close Fusion's Open dialog. No dialog was opened.",
            "_hint": "Retry in a moment; if it keeps failing, check Adom Bridge is running (bridge_list) and let it respawn this bridge."}),
    };
    if !approved(&status_now) {
        let tell = "Adom Bridge needs your OK to fill in Fusion's Open dialog for the EAGLE import: click Approve (1 hr, 24 hr or Auto) in the Adom Bridge window, on its notification, or in Hydrogen.";
        if let Some(rid) = args["_runId"].as_str() {
            let mut v = std::fs::read_to_string(eagle_run_path(&ctx.state_dir, rid)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(json!({}));
            v["stage"] = json!("waiting_for_approval"); v["tellUser"] = json!(tell);
            eagle_run_write(&ctx.state_dir, rid, &v);
        }
        let asked_at = std::time::Instant::now();
        let g = ab.call("request_shell_approval", json!({"wait": true, "timeoutSec": 300, "reason": "Fill in Fusion's Open dialog with the EAGLE .sch and .brd paths for the design import"}), &ctx.caller, 320);
        let mut granted = g.as_ref().map(|v| v["granted"].as_bool() == Some(true) || v["data"]["granted"].as_bool() == Some(true)).unwrap_or(false);
        // the wait itself can break (a 504 from ab, #136) while the prompt still stands: keep reading
        // the grant for the rest of the five minutes instead of giving up on a click still to come
        if !granted && g.is_err() {
            while asked_at.elapsed() < Duration::from_secs(300) {
                std::thread::sleep(Duration::from_secs(3));
                if ab.call("status", json!({}), &ctx.caller, 10).map(|s| approved(&s)).unwrap_or(false) { granted = true; break; }
            }
        }
        if let Some(rid) = args["_runId"].as_str() {
            let mut v = std::fs::read_to_string(eagle_run_path(&ctx.state_dir, rid)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(json!({}));
            v["stage"] = json!("started"); v["approval"] = json!({"granted": granted});
            eagle_run_write(&ctx.state_dir, rid, &v);
        }
        if !granted {
            let answer = g.as_ref().map(|v| if v["data"].is_object() { v["data"].clone() } else { v.clone() }).unwrap_or_else(|e| json!({"error": format!("{e}")}));
            return json!({"success": false, "errorCode": "needs_approval", "imported": [], "dialogOpened": false, "approvalAnswer": answer,
                "error": "Not started: the import fills Fusion's Open dialog, which needs Adom Bridge's approval, and Adom Bridge reported no approval (approvalAnswer has its reply). No dialog was opened.",
                "tellUser": tell, "_hint": "Ask the user to click Approve in Adom Bridge (or Hydrogen), then run the import again. With approval on (Auto mode) the import needs no prompt."});
        }
    }
    let mut stale_cancelled = false;
    if let Some(h) = find_open_dialog() { stale_cancelled = crate::small_verbs::close_native_dialog(&ab, &ctx.caller, h).0; }
    // the documents open before the import: a failed import closes only the empty Untitled ones it made
    let docs_before: Vec<String> = pass("get_app_state", json!({}))["data"]["documents"].as_array().cloned().unwrap_or_default().iter().filter_map(|d| d["name"].as_str().map(str::to_string)).collect();
    // QUEUE the import, never run it inline: the command opens a modal Open dialog, so a blocking
    // `Commands.Start` holds the add-in's main thread until the dialog closes; the call then
    // timed out and the timeout path swept the very dialog this verb was about to drive
    // ("the 'sch' Open dialog never appeared", arav-rog 2026-10-05). execute() only queues it.
    // a minimized Fusion opens the import's Open dialog minimized too (a bare title bar with no
    // controls): restore it first, in the background (no activation), then send it back behind
    let restore_minimized = |pred: &dyn Fn(&str) -> bool| {
        let ws = ab.call("desktop_list_windows", json!({}), &ctx.caller, 10).ok().and_then(|v| v["data"]["windows"].as_array().or_else(|| v["windows"].as_array()).cloned()).unwrap_or_default();
        for w in ws.iter().filter(|w| w["minimized"].as_bool() == Some(true) && pred(w["title"].as_str().unwrap_or(""))) {
            if let Some(h) = w["hwnd"].as_i64() { let _ = ab.call("desktop_set_window_state", json!({"hwnd": h, "state": "restore"}), &ctx.caller, 10); let _ = ab.call("desktop_set_window_state", json!({"hwnd": h, "state": "bottom"}), &ctx.caller, 10); }
        }
    };
    restore_minimized(&|t: &str| t.contains("Autodesk Fusion"));
    let _ = pass("run_modeling_script", json!({"script": "app.userInterface.commandDefinitions.itemById('ImportSCHAndBRDCmd').execute()\nresult={'queued':True}"}));
    let mut picked: Vec<String> = Vec::new();
    let mut answered: Option<i64> = None;
    for (want, base) in [("sch", &sch_base), ("brd", &brd_base)] {
        let mut dlg = None;
        // the queued import runs once Fusion's main thread is free (a new design still settling can
        // hold it for half a minute), so wait up to 90 s for the first dialog, 30 s for the second
        // never the dialog just answered: it can linger a moment while the board's dialog replaces it
        // up to five minutes: a busy Fusion opened it after the old 90 s, with nobody left to fill it (#136)
        let first_wait = args["_openDialogWaitSec"].as_u64().map(|s| s * 2 / 3).unwrap_or(200);
        if want == "sch" { if let Some(rid) = args["_runId"].as_str() {
            let mut v = std::fs::read_to_string(eagle_run_path(&ctx.state_dir, rid)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(json!({}));
            v["stage"] = json!("waiting_for_open_dialog");
            eagle_run_write(&ctx.state_dir, rid, &v);
        } }
        for i in 0..(if want == "sch" { first_wait } else { 40 }) {
            std::thread::sleep(Duration::from_millis(1500)); dlg = find_open_dialog().filter(|h| Some(*h) != answered); if dlg.is_some() { break; }
            if i % 4 == 3 { if let Some(c) = crash_dialog() { return crashed(c, None, &format!("waiting for the {want} Open dialog"), &picked); } }
        }
        if dlg.is_some() { restore_minimized(&|t: &str| t.trim().eq_ignore_ascii_case("open")); std::thread::sleep(Duration::from_millis(800)); }
        let Some(h) = dlg else {
            if want == "brd" && !picked.is_empty() { break; }
            watch_for_late_open_dialog(ctx.state_dir.clone(), ctx.caller.clone(), ctx.pids.clone(), base.to_string(), import_gen);
            let ab_note = ab_error.borrow().clone();
            return json!({"success": false, "errorCode": "open_dialog_not_seen", "imported": picked, "abError": ab_note, "lateDialogWatch": true,
                "error": format!("Fusion's '{want}' Open dialog did not appear in the time the bridge waits{}. Nothing was imported. Fusion may still open it: for the next ten minutes the bridge closes a late one, so it cannot stay on your screen.", if ab_note.is_some() { ", and Adom Bridge could not list the windows" } else { "" }),
                "_hint": "Fusion was most likely busy. Let it settle (fusion_get_app_state answers again), close any empty Untitled documents, and run the import again; starting a new import stops the watch. Autodesk errors on the EAGLE source would show as a dialog in fusion_check_dialogs."});
        };
        // the FULL path into "File name:": the dialog opens on whatever folder Fusion used last, so
        // clicking the bare name only worked when that happened to be the file's folder (2026-10-05)
        if let Some(c) = crash_dialog() { return crashed(c, Some(h), &format!("before typing the {want} path"), &picked); }
        let full = (if want == "sch" { &sch } else { &brd }).replace('/', "\\");
        // role Edit: the bare name also matches the combo box, which has no ValuePattern. A value
        // set by UIA is not committed by the Open button, so submit (Enter in the field; ab puts
        // the user's foreground window back afterwards)
        // the File name Edit by its name, then by the common dialog's control id (1148), then as the
        // dialog's first Edit: Windows 11 24H2 named it differently and the name alone set nothing (#133)
        let mut type_errors: Vec<Value> = Vec::new();
        let mut set_ok = |sel: Value| -> bool {
            let mut a = sel.clone(); a["hwnd"] = json!(h); a["text"] = json!(full); a["submit"] = json!(true);
            match ab.call("desktop_ui_set", a, &ctx.caller, 20) {
                Ok(v) if v["success"].as_bool() == Some(true) || v["data"]["set"].as_bool() == Some(true) => true,
                Ok(v) => { type_errors.push(json!({"via": "desktop_ui_set", "selector": sel, "errorCode": v["errorCode"], "error": v["error"]})); false }
                Err(e) => { type_errors.push(json!({"via": "desktop_ui_set", "selector": sel, "error": format!("{e}")})); false }
            }
        };
        let mut typed = false;
        let mut typed_via = Value::Null;
        if args["_forceTypeFallback"].as_bool() != Some(true) {
            for sel in [json!({"name": "File name:", "role": "Edit"}), json!({"automationId": "1148"}), json!({"role": "Edit", "nth": 0})] {
                if set_ok(sel.clone()) { typed = true; typed_via = json!({"uia": sel}); std::thread::sleep(Duration::from_millis(800)); if find_open_dialog().map(|x| x != h).unwrap_or(true) { break; } }
            }
        }
        // UI Automation could not set the box (a PC where the dialog's Edit is not exposed, #133): type
        // the path. A fresh Open dialog has the cursor in its File name box; this brings the dialog in
        // front for the moment it takes (ab gives the window back afterwards)
        if find_open_dialog() == Some(h) {
            let t1 = ab.call("desktop_type", json!({"hwnd": h, "text": full}), &ctx.caller, 20);
            let t2 = ab.call("desktop_press_key", json!({"hwnd": h, "keys": ["enter"]}), &ctx.caller, 15);
            match (&t1, &t2) {
                (Ok(a), Ok(b)) if a["success"].as_bool() != Some(false) && b["success"].as_bool() != Some(false) => { typed = true; typed_via = json!("keyboard"); }
                _ => type_errors.push(json!({"via": "desktop_type", "error": t1.as_ref().map(|v| v["error"].clone()).unwrap_or_else(|e| json!(format!("{e}"))), "enter": t2.as_ref().map(|v| v["error"].clone()).unwrap_or_else(|e| json!(format!("{e}")))})),
            }
        }
        if !typed { let _ = ab.call("desktop_ui_click", json!({"hwnd": h, "name": base}), &ctx.caller, 15); std::thread::sleep(Duration::from_secs(1)); let _ = ab.call("desktop_ui_click", json!({"hwnd": h, "name": "Open"}), &ctx.caller, 15); }
        // the dialog must be gone now; if the file was not in the folder the dialog showed, the
        // click missed and the dialog stays. Never leave it on the user's screen.
        let mut gone = false;
        for _ in 0..8 { std::thread::sleep(Duration::from_secs(1)); if find_open_dialog().map(|x| x != h).unwrap_or(true) { gone = true; break; } }
        if !gone { if let Some(c) = crash_dialog() { return crashed(c, Some(h), &format!("submitting the {want} path"), &picked); } }
        if !gone {
            let (mut closed, how) = crate::small_verbs::close_native_dialog(&ab, &ctx.caller, h);
            // cancelling the .sch dialog makes the importer open its .brd dialog at once: close every
            // Open dialog it opens until none is left (#133: the one left on screen was that next one)
            let mut chained = 0;
            for _ in 0..3 {
                std::thread::sleep(Duration::from_millis(1500));
                let Some(next) = find_open_dialog() else { break };
                let (c, _) = crate::small_verbs::close_native_dialog(&ab, &ctx.caller, next);
                if c { chained += 1; } else { closed = false; break; }
            }
            if find_open_dialog().is_some() { closed = false; }
            let cleaned = if closed { close_new_untitled(&docs_before, pass) } else { 0 };
            crate::fusion_prefs::action_ledger(&ctx.state_dir, "eagle_import_dialog_cancelled", json!({"wanted": base, "kind": want, "typed": typed, "typedVia": typed_via, "typeErrors": type_errors, "closed": closed, "how": how, "chainedClosed": chained, "untitledClosed": cleaned}));
            return json!({"success": false, "errorCode": "open_dialog_stuck", "wanted": base, "imported": picked, "typed": typed, "typedVia": typed_via, "typeErrors": type_errors,
                "dialogClosed": closed, "dialogHwnd": h, "closedBy": how, "chainedDialogsClosed": chained, "untitledClosed": cleaned,
                "error": if closed { format!("Fusion's Open dialog did not take '{base}' ({}); the bridge closed the dialog ({how}) and the empty documents the import had opened.", if typed { "the path was typed and submitted, but the dialog stayed" } else { "the File name box could not be set" }) }
                         else { format!("Fusion's Open dialog did not take '{base}', and it is STILL OPEN (hwnd {h}): Cancel and WM_CLOSE did not close it. Fusion stays blocked until it is closed.") },
                "_hint": "Nothing was imported. Put the .sch and .brd together in one space-free folder, pass both absolute paths, and retry; or use the dialog-free routes: fusion_schematic {action:\"open\", schPath, withBoard:false} for the schematic alone, fusion_open_board {filePath} for a .brd with placed parts."});
        }
        picked.push(base.clone()); answered = Some(h);
        std::thread::sleep(Duration::from_secs(2));
    }
    let mut ready = false;
    let mut doc_name = String::new();
    for i in 0..40 {
        std::thread::sleep(Duration::from_secs(3));
        if i % 3 == 2 { if let Some(c) = crash_dialog() { return crashed(c, None, "waiting for the imported design", &picked); } }
        let st = pass("get_app_state", json!({}));
        let d = &st["data"];
        if d["isElectronics"].as_bool() == Some(true) && electronics_editor(d["activeWorkspace"].as_str().unwrap_or("")) { ready = true; doc_name = d["activeDocument"].as_str().unwrap_or("").to_string(); break; }
    }
    // honest instantiation report (issue #90): what the .brd promised against what the board holds
    let expected = std::fs::read_to_string(&brd).map(|t| t.matches("<element ").count()).unwrap_or(0);
    let (mut placed, mut placed_names): (usize, Vec<Value>) = (0, vec![]);
    if ready {
        (placed, placed_names) = read_placed_parts(expected, &|| pass("board_info", json!({})), &|| std::thread::sleep(Duration::from_secs(3)));
    }
    let instantiated = expected == 0 || placed > 0;
    if let Some(rid) = args["_runId"].as_str() {
        let mut v = std::fs::read_to_string(eagle_run_path(&ctx.state_dir, rid)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(json!({}));
        v["documentName"] = json!(doc_name); v["partsExpected"] = json!(expected); v["partsPlaced"] = json!(placed); v["placed"] = json!(placed_names); v["imported"] = json!(picked);
        eagle_run_write(&ctx.state_dir, rid, &v);
    }
    let ready = ready && instantiated;
    json!({"success": ready, "imported": picked, "documentName": doc_name, "partsExpected": expected, "partsPlaced": placed, "placed": placed_names,
        "errorCode": if ready { Value::Null } else if picked.len() == 2 && expected > 0 && placed == 0 { json!("nothing_instantiated") } else { json!("import_not_ready") }, "staleDialogCancelled": stale_cancelled, "dialogsDriven": "Fusion's own Open dialogs (sch, then brd), driven in the background by accessible name; this is Autodesk's only route to a populated board from EAGLE source", "statusVerb": "fusion_board_info",
        "_hint": if !ready && placed == 0 && expected > 0 { "The import opened documents but placed NONE of the parts the .brd carries (Fusion names the new design Untitled until it is saved). The usual cause is a library the .brd references that Fusion cannot resolve, or a .sch/.brd pair that is not consistent; check the .brd's <libraries> block and fusion_check_dialogs. fusion_open_board {filePath} instantiates parts from a .brd whose elements embed their library, with no dialog." } else if ready { "EAGLE design imported via Fusion's own ImportSCHAndBRDCmd (both Open dialogs driven in the background). It is now an electronics design in the PCB editor. NEXT: fusion_show_2d_board, then RATSNEST + 'AUTO ;' (fusion_electron_run) to autoroute, fusion_show_3d_board for the 3D. NOTE: board_info can read 0 right after import (wrong-view query) even though the board is populated - verify by screenshotting the Fusion hwnd (read screenshots[]) or fusion_show_2d_board + WINDOW FIT. If 3D bodies are missing, the placed parts' package3d urns must resolve in THIS project - build the library's 3D into this project with fusion_build_library_3d (do NOT reuse another library's urns; they don't resolve cross-project)." } else { "Import fired + both Open dialogs driven, but the design did not confirm as electronics within the budget (a slow VM import can run longer). Poll fusion_get_app_state, and screenshot the Fusion hwnd reading screenshots[] for a blocking import dialog." }})
}

const READONLY_OPEN_NOTE: &str = "NOTE: a 'Read Only'/expired/trial Fusion still opens+views files — if it never opens, debug the open path (URN/picker/slow assembly), NOT the license. Only save/export are blocked in read-only.";

/// `fusion_aps_open`: search the cloud by name (APS), then open the best match in Fusion.
pub fn aps_open(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let query = args["query"].as_str().or(args["fileName"].as_str()).unwrap_or("").trim().to_string();
    if query.is_empty() { return json!({"success": false, "error": "Missing query/fileName."}); }
    let res = crate::native_aps_search(&json!({"query": query, "limit": 5}));
    let Some(m) = res["data"]["results"].as_array().and_then(|a| a.first().cloned()) else { return json!({"success": false, "error": format!("No cloud file matching '{query}'."), "_hint": format!("Run fusion_aps_search to see candidates, or refine the query. {READONLY_OPEN_NOTE}")}) };
    let name = m["name"].as_str().unwrap_or("").to_string();
    let open_args = json!({"projectName": m["projectName"], "fileName": name});
    if ctx.fusion_exe.is_none() || ctx.host.fusion_processes().is_empty() {
        return json!({"success": true, "output": format!("Found '{name}' in project {}.", crate::fusion_prefs::py_str(&m["projectName"])), "data": {"match": m, "openArgs": open_args}, "_hint": "Fusion isn't running — call fusion_start, then fusion_aps_open again to open it."});
    }
    let urn = m["id"].as_str().unwrap_or("").to_string();
    if args["wait"].as_bool() == Some(true) {
        let mut r = pass("open_by_urn", json!({"urn": urn}));
        if r["success"].as_bool() != Some(true) { let _ = pass("open_cloud_file", open_args.clone()); }
        if !r["data"].is_object() { r["data"] = json!({}); }
        r["data"]["match"] = m;
        if r["success"].as_bool() == Some(true) { let (shots, _) = crate::lifecycle::post_open_screenshot(ctx, 2.0); r["data"]["postOpenScreenshot"] = shots; }
        return r;
    }
    // the background open: a thread with its own context (the borrowed one cannot cross)
    let (sd, caller, urn2, oa) = (ctx.state_dir.clone(), ctx.caller.clone(), urn.clone(), open_args.clone());
    std::thread::spawn(move || {
        let host = crate::host_for_diagnosis();
        let side_url = crate::launch::SIDECAR_URL.lock().unwrap().clone();
        let side = move |verb: &str, a: Value, t: u64| -> Value { match &side_url { Some(u) => ureq::post(u).timeout(Duration::from_secs(t)).send_json(json!({"command": verb, "args": a})).ok().and_then(|r| r.into_json().ok()).unwrap_or(json!({"success": false})), None => json!({"success": false}) } };
        let pids: Vec<u32> = host.fusion_processes().iter().map(|p| p.pid).collect();
        let dlg = |r: Value| r;
        let ctx2 = Ctx { host: &*host, fusion_exe: crate::host_for_diagnosis_paths(), short_path: |p| crate::host().short_path(p), state_dir: sd, caller, pids, sidecar: &side, dialogs: &dlg };
        let pass2 = |verb: &str, a: Value| crate::addin::pass_through(verb, &a, &ctx2);
        let r = pass2("open_by_urn", json!({"urn": urn2}));
        if r["success"].as_bool() != Some(true) { let _ = pass2("open_cloud_file", oa); }
    });
    json!({"success": true, "output": format!("Found '{name}' in project {} — opening in Fusion.", crate::fusion_prefs::py_str(&m["projectName"])), "data": {"match": m, "opening": true}, "statusVerb": "fusion_get_app_state",
        "_hint": format!("The cloud file is opening in Fusion in the background (first open can take ~60-90s; large ASSEMBLIES download all referenced parts and take longer). Poll fusion_get_app_state until activeDocument is the file. Pass {{\"wait\": true}} to block instead. {READONLY_OPEN_NOTE}")})
}

#[cfg(test)]
mod doc_name_tests {
    #[test]
    fn short_names_count() {
        assert!(super::doc_matches("0603WA~1", "0603WAF3300T5E"));
        assert!(super::doc_matches("0603WA~1*", "0603waf3300t5e"));
        assert!(super::doc_matches("0603WAF3300T5E (v3)", "0603WAF3300T5E"));
        assert!(!super::doc_matches("0603WA~x", "0603WAF3300T5E"));
        assert!(super::doc_matches("0603WA~1* (Adom) - Autodesk Fusion ", "0603WAF3300T5E"));
        assert!(!super::doc_matches("Untitled", "0603WAF3300T5E"));
        assert!(!super::doc_matches("ADS858~1", "0603WAF3300T5E"));
    }
}

/// Fraction of a PNG's sampled pixels that share the dominant (quantised) colour. 1.0 is a
/// flat image; a Fusion window capture above ~0.97 shows chrome and nothing else (issue #84).
pub fn png_dominant_fraction(path: &str) -> Option<f64> {
    let f = std::fs::File::open(path).ok()?;
    let dec = png::Decoder::new(std::io::BufReader::new(f));
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let bpp = match info.color_type { png::ColorType::Rgba => 4, png::ColorType::Rgb => 3, png::ColorType::GrayscaleAlpha => 2, png::ColorType::Grayscale => 1, _ => return None };
    let (w, h) = (info.width as usize, info.height as usize);
    if w == 0 || h == 0 { return None; }
    let step = ((w * h) / 20000).max(1);
    let mut counts: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut total = 0u32;
    let mut i = 0usize;
    while i < w * h {
        let o = i * bpp;
        if o + bpp > buf.len() { break; }
        let (r, g, b) = if bpp >= 3 { (buf[o], buf[o + 1], buf[o + 2]) } else { (buf[o], buf[o], buf[o]) };
        let key = ((r as u32 >> 3) << 10) | ((g as u32 >> 3) << 5) | (b as u32 >> 3);
        *counts.entry(key).or_insert(0) += 1; total += 1;
        i += step;
    }
    let top = counts.values().copied().max().unwrap_or(0);
    if total == 0 { None } else { Some(top as f64 / total as f64) }
}

#[cfg(test)]
mod png_tests {
    #[test]
    fn flat_and_busy_images_are_told_apart() {
        let dir = std::env::temp_dir().join(format!("pngtest-{}", std::process::id())); std::fs::create_dir_all(&dir).unwrap();
        for (name, busy) in [("flat.png", false), ("busy.png", true)] {
            let p = dir.join(name); let f = std::fs::File::create(&p).unwrap();
            let mut enc = png::Encoder::new(std::io::BufWriter::new(f), 64, 64); enc.set_color(png::ColorType::Rgb); enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            let mut data = vec![255u8; 64 * 64 * 3];
            if busy { for (i, px) in data.chunks_mut(3).enumerate() { px[0] = (i % 251) as u8; px[1] = (i * 7 % 241) as u8; px[2] = (i * 13 % 239) as u8; } }
            w.write_image_data(&data).unwrap();
        }
        assert!(super::png_dominant_fraction(&dir.join("flat.png").to_string_lossy()).unwrap() > 0.99);
        assert!(super::png_dominant_fraction(&dir.join("busy.png").to_string_lossy()).unwrap() < 0.5);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod bound_p3d_tests {
    const LBR: &str = r#"<?xml version="1.0"?><eagle><drawing><library>
<packages3d>
<package3d name="A" urn="" wip_urn="urn:adsk.wipprod:fs.file:vf.AAA?version=1" type="model"><packageinstances><packageinstance name="A"/></packageinstances></package3d>
<package3d name="B" urn="" wip_urn="urn:adsk.wipprod:fs.file:vf.BBB?version=1" type="model"><packageinstances><packageinstance name="B"/></packageinstances></package3d>
</packages3d>
<devicesets>
<deviceset name="PartA"><devices><device name="" package="A"><package3dinstances><package3dinstance package3d_urn="urn:adsk.wipprod:fs.file:vf.AAA?version=1"/></package3dinstances></device></devices></deviceset>
<deviceset name="PartB"><devices><device name="" package="B"></device></devices></deviceset>
<deviceset name="PartC"><devices><device name="" package="C"></device></devices></deviceset>
</devicesets></library></drawing></eagle>"#;
    #[test]
    fn picks_each_parts_own_binding() {
        // the device's own instance wins; this is the part asked for, never the library's first package
        assert_eq!(super::bound_p3d_urn(LBR, "parta"), Some(("A".into(), "urn:adsk.wipprod:fs.file:vf.AAA?version=1".into())));
        // no instance on the device: the package3d whose packageinstance names its package
        assert_eq!(super::bound_p3d_urn(LBR, "PartB"), Some(("B".into(), "urn:adsk.wipprod:fs.file:vf.BBB?version=1".into())));
        assert_eq!(super::bound_p3d_urn(LBR, "PartC"), None);
        assert_eq!(super::bound_p3d_urn(LBR, "Missing"), None);
    }
}

#[cfg(test)]
mod eagle_wait_tests {
    #[test]
    fn the_import_wait_knows_editors_by_name_or_id() {
        for w in ["PCB Editor", "Schematic Editor", "BoardLayoutEnvironement", "SchematicEnvironment"] { assert!(super::electronics_editor(w), "{w}"); }
        for w in ["Design", "FusionSolidEnvironment", "unknown", ""] { assert!(!super::electronics_editor(w), "{w}"); }
    }
}

#[cfg(test)] mod placed_parts_tests {
    use super::read_placed_parts;
    use serde_json::json;
    fn run(answers: Vec<usize>, expected: usize) -> (usize, usize) {
        let i = std::cell::Cell::new(0);
        let (placed, _) = read_placed_parts(expected, &|| { let n = answers[i.get().min(answers.len() - 1)]; i.set(i.get() + 1); json!({"data": {"summary": {"componentCount": n}}}) }, &|| {});
        (placed, i.get())
    }
    #[test] fn a_board_still_loading_is_read_again() { assert_eq!(run(vec![0, 0, 5], 5), (5, 3)); }
    #[test] fn all_there_answers_at_once() { assert_eq!(run(vec![5], 5), (5, 1)); }
    #[test] fn a_count_that_holds_still_is_the_answer() { assert_eq!(run(vec![0, 3, 3, 5], 5), (3, 3)); }
    #[test] fn nothing_is_given_up_on_in_the_end() { assert_eq!(run(vec![0], 5), (0, 10)); }
    #[test] fn an_empty_board_expects_nothing() { assert_eq!(run(vec![0], 0), (0, 1)); }
}

#[cfg(test)] mod board_file_tests {
    use super::board_file_problems;
    const GOOD: &str = r#"<eagle><drawing><board><libraries><library name="L"><packages><package name="R0603"/></packages></library></libraries><elements><element name="R1" library="L" package="r0603"/></elements><signals><signal name="N"><contactref element="R1" pad="1"/></signal></signals></board></drawing></eagle>"#;
    #[test] fn a_good_board_has_no_problems() { assert!(board_file_problems(GOOD).is_empty()); }
    #[test] fn each_thing_fusion_refuses_is_named() {
        assert!(board_file_problems(&GOOD.replace(r#"<package name="R0603"/>"#, ""))[0].contains("package r0603"));
        assert!(board_file_problems(&GOOD.replace(r#"library="L""#, r#"library="NOPE""#))[0].contains("library NOPE"));
        assert!(board_file_problems(&GOOD.replace(r#"element="R1""#, r#"element="R9""#))[0].contains("net N names part R9"));
        assert!(board_file_problems("<eagle>")[0].contains("well-formed"));
    }
    #[test] fn real_boards_in_this_repo_pass() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/routing");
        let mut seen = 0;
        fn walk(d: &std::path::Path, seen: &mut i32) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() { walk(&p, seen); }
                else if p.extension().map(|x| x == "brd").unwrap_or(false) {
                    let t = std::fs::read_to_string(&p).unwrap();
                    let bad = board_file_problems(&t);
                    assert!(bad.is_empty(), "{}: {:?}", p.display(), &bad[..bad.len().min(3)]);
                    *seen += 1;
                }
            }
        }
        walk(&root, &mut seen);
        assert!(seen > 3, "no boards found");
    }
}

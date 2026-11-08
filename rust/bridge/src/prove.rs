//! `fusion_prove_library` and the thumbnail check, native. The verdict is decided by what EXISTS
//! in the .lbr (offline, the lbr crate); the four frames are evidence attached to that verdict:
//! symbol editor, footprint editor, the bound f3d in the 3D canvas (bodies counted, not just
//! photographed), and the ganged Library Manager view. Each frame is measured and its workspace
//! stamped, and streams to `_frameSink` the instant it lands.

use serde_json::{json, Value};
use std::time::Duration;

use crate::addin::Ctx;

fn expected_workspace(view: &str) -> Option<&'static str> { match view { "symbol-editor" | "footprint-editor" | "library-manager" => Some("Electronics Library"), "3d-editor" => Some("Design"), _ => None } }

/// `_image_dimensions`: PNG or WebP, from the header.
pub fn image_dimensions(path: &str) -> Option<(u32, u32)> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 64];
    use std::io::Read;
    let n = f.read(&mut head).ok()?;
    if n < 30 { return None; }
    if &head[..8] == b"\x89PNG\r\n\x1a\n" { return Some((u32::from_be_bytes([head[16], head[17], head[18], head[19]]), u32::from_be_bytes([head[20], head[21], head[22], head[23]]))); }
    if &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        match &head[12..16] {
            b"VP8 " => return Some(((u16::from_le_bytes([head[26], head[27]]) & 0x3FFF) as u32, (u16::from_le_bytes([head[28], head[29]]) & 0x3FFF) as u32)),
            b"VP8L" => { let b = &head[21..25]; return Some((1 + (((b[1] as u32 & 0x3F) << 8) | b[0] as u32), 1 + (((b[3] as u32 & 0xF) << 10) | ((b[2] as u32) << 2) | ((b[1] as u32 & 0xC0) >> 6)))); }
            b"VP8X" => return Some((1 + u32::from_le_bytes([head[24], head[25], head[26], 0]), 1 + u32::from_le_bytes([head[27], head[28], head[29], 0]))),
            _ => {}
        }
    }
    None
}

/// `_frame_data_url`: the frame inline when small enough (no downscale without PIL, same as Python).
pub fn frame_data_url(path: &str) -> String {
    let Ok(raw) = std::fs::read(path) else { return String::new() };
    if raw.len() > 300_000 { return String::new(); }
    let mime = if path.to_lowercase().ends_with(".webp") { "image/webp" } else { "image/png" };
    format!("data:{mime};base64,{}", crate::etiquette::b64_pub(&raw))
}

/// `_prove_capture`: settle, capture, MEASURE, stamp the workspace.
pub fn prove_capture(ctx: &Ctx, label: &str, view: &str, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    for _ in 0..40 { if pass("get_app_state", json!({}))["success"].as_bool() == Some(true) { break; } std::thread::sleep(Duration::from_secs(3)); }
    std::thread::sleep(Duration::from_secs(2));
    let mut out = json!({"step": label, "view": view, "ok": false, "screenshot": Value::Null});
    let wsr = pass("execute_text_command", json!({"command": "DebugCommands.ActiveWorkspace"}));
    let wsv = wsr["data"]["output"].as_str().or(wsr["data"]["result"].as_str()).unwrap_or("").trim().to_string();
    if !wsv.is_empty() { out["activeWorkspace"] = json!(wsv.chars().take(60).collect::<String>()); }
    let shot = crate::small_verbs::screenshot_fusion(&json!({}), &ctx.caller);
    if shot["success"].as_bool() == Some(true) {
        if let Some(p) = shot["savedTo"].as_str().map(str::to_string) {
            out["screenshot"] = json!(p);
            let (mut w, mut h) = (shot["fullWidth"].as_u64().map(|v| v as u32), shot["fullHeight"].as_u64().map(|v| v as u32));
            if w.is_none() || h.is_none() { if let Some((iw, ih)) = image_dimensions(&p) { w = Some(iw); h = Some(ih); } }
            out["width"] = json!(w); out["height"] = json!(h);
            match (w, h) {
                (Some(w), Some(h)) if w >= 400 && h >= 300 => { out["ok"] = json!(true); }
                (Some(w), Some(h)) => { out["error"] = json!(format!("wrong_window_captured: {w}x{h} is not the Fusion editor (a title-bar sliver or a stray dialog)")); }
                _ => { out["error"] = json!("frame could not be measured - not accepted as evidence"); }
            }
            let stamp = out["activeWorkspace"].as_str().map(str::to_string);
            if let Some(exp) = expected_workspace(view) {
                match &stamp {
                    Some(s) if out["ok"].as_bool() == Some(true) && s.to_lowercase() != exp.to_lowercase() => { out["ok"] = json!(false); out["errorCode"] = json!("wrong_workspace"); out["error"] = json!(format!("wrong_workspace: the {view} frame was captured in the {} workspace, expected {} - Fusion was showing a different editor when the shot was taken, so this is not evidence of the {label} view.", fusion_lbr::py_repr_str(s), fusion_lbr::py_repr_str(exp))); }
                    None if out["ok"].as_bool() == Some(true) => { out["workspaceStamp"] = json!("missing (not gated)"); }
                    _ => {}
                }
            }
            let du = frame_data_url(&p);
            if !du.is_empty() { out["thumbnailDataUrl"] = json!(du); }
        }
    }
    out
}

/// `_count_3d_bodies`: how many REAL bodies the active design holds (a state check).
pub fn count_3d_bodies(pass: &dyn Fn(&str, Value) -> Value) -> (i64, f64) {
    let script = "import adsk.core, adsk.fusion\nd = app.activeProduct\nn = 0; vol = 0.0\ntry:\n    root = d.rootComponent\n    for occ in [None] + list(root.allOccurrences):\n        comp = root if occ is None else occ.component\n        for b in comp.bRepBodies:\n            n += 1\n            bb = b.boundingBox\n            dx = (bb.maxPoint.x - bb.minPoint.x) * 10\n            dy = (bb.maxPoint.y - bb.minPoint.y) * 10\n            dz = (bb.maxPoint.z - bb.minPoint.z) * 10\n            vol += abs(dx * dy * dz)\nexcept Exception:\n    n = -1\nresult = {'bodies': n, 'bboxMm3': round(vol, 2)}\n";
    let r = pass("run_modeling_script", json!({"script": script}));
    let data = if r["data"]["result"].is_object() { r["data"]["result"].clone() } else { r["data"].clone() };
    if r["success"].as_bool() == Some(true) && data.get("bodies").is_some() { return (data["bodies"].as_i64().unwrap_or(-1), data["bboxMm3"].as_f64().unwrap_or(0.0)); }
    (-1, 0.0)
}

fn norm(s: &str) -> String { s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect() }
fn vf_token(u: &str) -> String { let u = u.split('?').next().unwrap_or(""); if u.contains("vf.") { u.rsplit("vf.").next().unwrap_or("").to_string() } else { u.to_string() } }

/// `aps.thumbnail_state`: is the cloud thumbnail rendered for the bound file?
pub fn thumbnail_state(project_id: &str, folder_id: &str, wip_urn: &str, file_name: &str) -> Value {
    if project_id.is_empty() || folder_id.is_empty() { return json!({"state": "error", "why": "projectId and folderId required"}); }
    let store = fusion_core::aps::ApsStore::default_location();
    let want_urn = wip_urn.split('?').next().unwrap_or("").trim().to_string();
    let (status, data) = match fusion_core::aps::aps_get(&store, &format!("/data/v1/projects/{project_id}/folders/{folder_id}/contents"), &[("include", "versions")]) { Ok(x) => x, Err(_) => return json!({"state": "not_signed_in"}) };
    if status != 200 || !data.is_object() { return json!({"state": "error", "http": status, "why": data["error"]}); }
    let items = data["data"].as_array().cloned().unwrap_or_default();
    let included = data["included"].as_array().cloned().unwrap_or_default();
    let want_tok = vf_token(&want_urn);
    let fname = norm(file_name);
    let hit_version = included.iter().find(|v| { let vid = vf_token(v["id"].as_str().unwrap_or("")); let vname = norm(v["attributes"]["displayName"].as_str().unwrap_or("")); let name_hit = !fname.is_empty() && (vname.starts_with(&fname) || vname.contains(&fname)); (!want_tok.is_empty() && vid == want_tok) || name_hit }).cloned();
    let hit_item = if hit_version.is_none() && !file_name.is_empty() { items.iter().find(|it| { let nm = norm(it["attributes"]["displayName"].as_str().unwrap_or("")); nm.starts_with(&fname) || nm.contains(&fname) }).cloned() } else { None };
    let Some(src) = hit_version.or(hit_item) else { return json!({"state": "not_found", "searched": {"urn": want_urn, "name": file_name}, "itemCount": items.len()}) };
    let name = src["attributes"]["displayName"].as_str().unwrap_or("").to_string();
    match src["relationships"]["thumbnails"]["data"]["id"].as_str() { Some(tid) => json!({"state": "ready", "thumbnailId": tid, "item": name}), None => json!({"state": "pending", "item": name, "why": "version exists but Fusion's cloud has not rendered a thumbnail yet"}) }
}

/// `fusion_check_3d_thumbnail`.
pub fn check_3d_thumbnail(args: &Value) -> Value {
    let mut urn = args["urn"].as_str().unwrap_or("").trim().to_string();
    let lbr = args["libraryPath"].as_str().unwrap_or("").replace('\\', "/");
    let mut file_name = args["fileName"].as_str().unwrap_or("").to_string();
    if urn.is_empty() && !lbr.is_empty() {
        if let Ok(t) = std::fs::read_to_string(&lbr) {
            for c in regex::Regex::new(r#"<package3d\b([^>]*)>"#).unwrap().captures_iter(&t) {
                let attrs = &c[1];
                let get = |k: &str| regex::Regex::new(&format!(r#"\b{k}="([^"]*)""#)).unwrap().captures(attrs).map(|m| m[1].to_string()).unwrap_or_default();
                let u = get("urn"); let w = get("wip_urn");
                let pick = if !u.trim().is_empty() { u.trim().to_string() } else { w.trim().to_string() };
                if !pick.is_empty() { urn = pick; }
                if file_name.is_empty() { file_name = get("name"); }
            }
        }
    }
    if urn.is_empty() && file_name.is_empty() { return json!({"success": false, "errorCode": "no_3d_reference", "error": "No urn/fileName given and the .lbr carries no package3d urn.", "_hint": "Bind first (fusion_bind_library_3d), then check the thumbnail."}); }
    let (project_id, folder_id) = crate::mint::discover_upload_target_pub(args);
    let Some(folder_id) = folder_id else { return json!({"success": false, "errorCode": "no_work_folder", "error": "Cannot resolve the upload folder to look in (APS signed in?)."}) };
    let deadline = std::time::Instant::now() + Duration::from_secs_f64(args["waitSeconds"].as_f64().unwrap_or(0.0));
    let st = loop { let st = thumbnail_state(&project_id, &folder_id, &urn, &file_name); if st["state"].as_str() == Some("ready") || std::time::Instant::now() >= deadline { break st; } std::thread::sleep(Duration::from_secs(10)); };
    let ready = st["state"].as_str() == Some("ready");
    json!({"success": ready, "thumbnail": st, "urn": if urn.is_empty() { Value::Null } else { json!(urn) },
        "_hint": if ready { "Thumbnail is RENDERED in the cloud - the Library Manager will show the real chip.".to_string() } else { format!("Not rendered yet (state={}). Fusion uploads + renders asynchronously after a bind - re-call with waitSeconds:180 to poll, and treat a still-pending thumbnail as NOT proven.", st["state"].as_str().unwrap_or("None")) }})
}

/// `fusion_prove_library`.
pub fn prove_library(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let sink = args["_frameSink"].as_str().unwrap_or("").trim().to_string();
    let emit = |e: Value| -> Value { if !sink.is_empty() { use std::io::Write; if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&sink) { let _ = writeln!(f, "{e}"); } } e };
    let lbr = args["libraryPath"].as_str().or(args["filePath"].as_str()).map(|s| s.replace('\\', "/")).filter(|s| !s.is_empty()).unwrap_or_else(|| crate::small_verbs::get_pref(&ctx.state_dir, "lastLibraryPath").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default());
    if lbr.is_empty() { return json!({"success": false, "error": "libraryPath required (or open a library first).", "errorCode": "library_path_required", "_hint": "If a library IS open on screen, the bridge probably respawned since it was opened (ab auto-updates on publish, even mid-run) and forgot it. Pass libraryPath explicitly - for a bundle install it is C:/Users/<user>/Documents/AdomLibraries/<bundle>/<Library>.lbr."}); }
    let xml = std::fs::read_to_string(&lbr).unwrap_or_default();
    let Some(names) = fusion_lbr::artifact_names(&xml).ok().filter(|n| !(n.sym.is_empty() && n.pac.is_empty() && n.dev.is_empty())) else { return json!({"success": false, "error": format!("Could not read or parse that .lbr: {lbr}"), "errorCode": "library_unreadable"}) };
    let want = args["name"].as_str().unwrap_or("").trim().to_string();
    let want = if want.is_empty() { names.dev.first().cloned().unwrap_or_default() } else { want };
    let mut artifacts = json!({
        "symbol": {"present": !names.sym.is_empty(), "names": names.sym.iter().take(10).cloned().collect::<Vec<_>>()},
        "footprint": {"present": !names.pac.is_empty(), "names": names.pac.iter().take(10).cloned().collect::<Vec<_>>()},
        "package3d": {"present": !names.p3d.is_empty(), "names": names.p3d.iter().take(10).cloned().collect::<Vec<_>>()},
        "deviceset": {"present": !names.dev.is_empty(), "names": names.dev.iter().take(10).cloned().collect::<Vec<_>>()}});
    let mut missing: Vec<String> = ["symbol", "footprint", "package3d"].iter().filter(|k| artifacts[*k]["present"].as_bool() != Some(true)).map(|s| s.to_string()).collect();
    let conn = fusion_lbr::check_connects(&xml);
    artifacts["connects"] = json!(conn);
    if artifacts["symbol"]["present"].as_bool() == Some(true) && artifacts["footprint"]["present"].as_bool() == Some(true) && !conn.ok { missing.push(format!("connects({})", conn.why.clone().unwrap_or_else(|| "invalid".into()))); }
    let opened = crate::orchestrations::open_lbr(ctx, &json!({"filePath": lbr}), pass);
    if opened["success"].as_bool() != Some(true) { return json!({"success": false, "error": format!("open_lbr failed: {}", opened["error"].as_str().unwrap_or("None")), "errorCode": "library_open_failed", "artifacts": artifacts}); }
    crate::small_verbs::set_pref(&ctx.state_dir, "lastLibraryPath", json!(lbr));
    let mut evidence: Vec<Value> = Vec::new();
    let skip: Vec<String> = args["_skipSteps"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let shared_with = args["_sharedWith"].clone();
    let mut pkgs: Vec<String> = names.dev_pac.get(&want).cloned().unwrap_or_default();
    let mut pkg_fallback_note: Option<String> = None;
    if pkgs.is_empty() {
        if names.pac.len() == 1 { pkgs = names.pac.clone(); pkg_fallback_note = Some(format!("deviceset {} is not in this library; it has exactly one package ({}), so the footprint was proved against that", fusion_lbr::py_repr_str(&want), names.pac[0])); }
        else if names.pac.contains(&want) { pkgs = vec![want.clone()]; }
    }
    let pk_iter: Vec<Option<String>> = if pkgs.is_empty() { vec![None] } else { pkgs.iter().cloned().map(Some).collect() };
    if !skip.iter().any(|s| s == "symbol") {
        let r = crate::orchestrations::show_library_view(ctx, &want, "sym", Some(&lbr), pass);
        if r["success"].as_bool() != Some(true) { evidence.push(emit(json!({"step": "symbol", "view": "symbol-editor", "ok": false, "error": r["error"].as_str().unwrap_or("None").chars().take(160).collect::<String>()}))); }
        else { evidence.push(emit(prove_capture(ctx, "symbol", "symbol-editor", pass))); }
    }
    if !skip.iter().any(|s| s == "footprint") {
        for pk in &pk_iter {
            let Some(pk) = pk else {
                evidence.push(emit(json!({"step": "footprint", "view": "footprint-editor", "ok": false, "package": Value::Null, "error": format!("Cannot resolve a package to prove: {} is neither a deviceset nor a package in this library.", fusion_lbr::py_repr_str(&want)),
                    "_hint": format!("Pass the DEVICESET name (the part), not the library name. This library has devicesets {} and packages {}.", py_list(&names.dev, 8), py_list(&names.pac, 8))})));
                continue;
            };
            let r = crate::orchestrations::show_library_view(ctx, pk, "pac", Some(&lbr), pass);
            if r["success"].as_bool() != Some(true) { evidence.push(emit(json!({"step": "footprint", "view": "footprint-editor", "ok": false, "package": pk, "error": r["error"].as_str().unwrap_or("None").chars().take(160).collect::<String>()}))); continue; }
            let mut ef = prove_capture(ctx, "footprint", "footprint-editor", pass); ef["package"] = json!(pk);
            if let Some(n) = &pkg_fallback_note { ef["_note"] = json!(n); }
            evidence.push(emit(ef));
        }
    }
    let p3d_present = artifacts["package3d"]["present"].as_bool() == Some(true);
    if p3d_present && !skip.iter().any(|s| s == "package3d") {
        for pk in &pk_iter {
            let (mut p3d_urn, mut p3d_pkg) = (String::new(), String::new());
            if let Some(p) = pk { p3d_pkg = p.clone(); } else if let Ok(doc) = roxmltree::Document::parse_with_options(&xml, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }) {
                if let Some(ds) = doc.descendants().find(|n| n.has_tag_name("deviceset") && n.attribute("name").map(|v| v.to_lowercase() == want.to_lowercase()).unwrap_or(false)) {
                    if let Some(dv) = ds.descendants().find(|n| n.has_tag_name("device") && n.attribute("package").map(|p| !p.is_empty()).unwrap_or(false)) { p3d_pkg = dv.attribute("package").unwrap_or("").to_string(); }
                }
            }
            if let Ok(doc) = roxmltree::Document::parse_with_options(&xml, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }) {
                let all3: Vec<roxmltree::Node> = doc.descendants().filter(|n| n.has_tag_name("package3d")).collect();
                for p3 in &all3 {
                    let insts: Vec<String> = p3.descendants().filter(|n| n.has_tag_name("packageinstance")).map(|n| n.attribute("name").unwrap_or("").to_lowercase()).collect();
                    if !p3d_pkg.is_empty() && insts.contains(&p3d_pkg.to_lowercase()) { p3d_urn = p3.attribute("urn").filter(|u| !u.trim().is_empty()).or(p3.attribute("wip_urn")).unwrap_or("").trim().to_string(); break; }
                }
                if p3d_urn.is_empty() && all3.len() == 1 { p3d_urn = all3[0].attribute("urn").filter(|u| !u.trim().is_empty()).or(all3[0].attribute("wip_urn")).unwrap_or("").trim().to_string(); }
            }
            if p3d_urn.is_empty() && p3d_pkg.is_empty() { if let Some(u) = args["f3dUrn"].as_str().map(str::trim).filter(|s| !s.is_empty()) { p3d_urn = u.to_string(); } }
            let mut opened3d = Value::Null;
            if !p3d_urn.is_empty() {
                for attempt in 0..3 { opened3d = pass("open_by_urn", json!({"urn": p3d_urn})); if opened3d["success"].as_bool() == Some(true) { break; } if attempt < 2 { eprintln!("[fusion-bridge] bound f3d not openable yet (attempt {}) - waiting 15s for the lineage to commit", attempt + 1); std::thread::sleep(Duration::from_secs(15)); } }
            }
            if opened3d["success"].as_bool() == Some(true) {
                let wsfix = pass("run_modeling_script", json!({"script": "import adsk.core, time\napp=adsk.core.Application.get()\ndef _ws():\n    try: return app.executeTextCommand('DebugCommands.ActiveWorkspace').strip()\n    except Exception: return ''\nbefore=_ws()\nif 'design' not in before.lower():\n    try:\n        w=app.userInterface.workspaces.itemById('FusionSolidEnvironment')\n        if w: w.activate()\n        adsk.doEvents(); time.sleep(2)\n    except Exception: pass\ntry:\n    app.activeViewport.goHome(False)\n    app.activeViewport.fit()\nexcept Exception: pass\nresult={'before': before, 'after': _ws()}\n"}));
                let ws_after = wsfix["data"]["result"]["after"].as_str().unwrap_or("").to_string();
                let doc3 = pass("get_app_state", json!({}))["data"]["activeDocument"].as_str().unwrap_or("").replace('*', "").trim().to_string();
                let mut e3 = prove_capture(ctx, "package3d", "3d-editor", pass);
                e3["package"] = json!(pk); e3["activeDocument"] = json!(doc3);
                e3["expectedDocument"] = if p3d_pkg.is_empty() { Value::Null } else { json!(format!("{p3d_pkg}_3d")) };
                if !p3d_pkg.is_empty() && !doc3.is_empty() && !doc3.to_lowercase().contains(&format!("{p3d_pkg}_3d").to_lowercase()) { e3["ok"] = json!(false); e3["error"] = json!(format!("wrong_document_captured: the 3D frame shows {}, not this part's design {}", fusion_lbr::py_repr_str(&doc3), fusion_lbr::py_repr_str(&format!("{p3d_pkg}_3d")))); }
                e3["activeWorkspace"] = json!(ws_after);
                if !ws_after.is_empty() && !ws_after.to_lowercase().contains("design") { e3["ok"] = json!(false); e3["error"] = json!(format!("package3d frame captured in workspace {}, not the Design 3D canvas - a 2D package sheet is not evidence of a 3D chip (the workspace switch did not take).", fusion_lbr::py_repr_str(&ws_after))); }
                let (bodies, bbox) = count_3d_bodies(pass);
                e3["bodies"] = json!(bodies); e3["bboxMm3"] = json!(bbox);
                if bodies == 0 { e3["ok"] = json!(false); e3["error"] = json!("The bound f3d opened but contains NO bodies - an empty viewport is not evidence of a 3D chip."); }
                else if bodies < 0 { e3["bodiesUnverified"] = json!(true); }
                evidence.push(emit(e3));
                let _ = pass("run_modeling_script", json!({"script": "import adsk.core, time\napp=adsk.core.Application.get()\nfor _ in range(24):\n    if not app.hasActiveJobs: break\n    time.sleep(5)\napp.activeDocument.close(False)\nresult={}"}));
            } else {
                evidence.push(emit(json!({"step": "package3d", "view": "3d-editor", "ok": false, "error": format!("Could not open the bound f3d ({}): {}", if p3d_urn.is_empty() { "no urn".to_string() } else { p3d_urn.chars().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect() }, opened3d["error"].as_str().unwrap_or("None").chars().take(100).collect::<String>())})));
            }
        }
    }
    if !p3d_present && !skip.iter().any(|s| s == "package3d") { evidence.push(emit(json!({"step": "package3d", "view": "3d-editor", "ok": false, "error": "No package3d bound - the 3D editor would only show Fusion's grey Placeholder, which is not evidence."}))); }
    let reopen = crate::orchestrations::open_lbr(ctx, &json!({"filePath": lbr}), pass);
    if reopen["success"].as_bool() == Some(true) { let _ = crate::orchestrations::ensure_library_active(ctx, &lbr, 45, pass); }
    let r = crate::orchestrations::show_library_view(ctx, &want, "dev", Some(&lbr), pass);
    if r["success"].as_bool() == Some(true) { evidence.push(emit(prove_capture(ctx, "ganged", "library-manager", pass))); }
    else { evidence.push(json!({"step": "ganged", "view": "library-manager", "ok": false, "error": r["error"].as_str().unwrap_or("None").chars().take(160).collect::<String>()})); }
    let mut thumb_unverified: Option<&str> = None;
    if p3d_present {
        let thumb = check_3d_thumbnail(&json!({"libraryPath": lbr, "waitSeconds": 180}));
        artifacts["package3d"]["thumbnail"] = thumb["thumbnail"]["state"].clone();
        if thumb["success"].as_bool() != Some(true) {
            if thumb["errorCode"].as_str() == Some("no_work_folder") { thumb_unverified = Some("aps_unavailable"); artifacts["package3d"]["thumbnail"] = json!("unverified(aps_unavailable)"); }
            else { missing.push(format!("thumbnail({})", thumb["thumbnail"]["state"].as_str().or(thumb["errorCode"].as_str()).unwrap_or("unknown"))); }
        }
    }
    let captured: Vec<&Value> = evidence.iter().filter(|e| e["ok"].as_bool() == Some(true) && e["screenshot"].is_string()).collect();
    for e in &evidence { if !(e["ok"].as_bool() == Some(true) && e["screenshot"].is_string()) { missing.push(format!("frame:{}({})", e["step"].as_str().unwrap_or(""), crate::fusion_prefs::py_str(&e["error"]))); } }
    let npk = pkgs.len().max(1);
    let has = |s: &str| skip.iter().any(|x| x == s);
    let expected = (if has("symbol") { 0 } else { 1 }) + (if has("ganged") { 0 } else { 1 }) + (if has("footprint") { 0 } else { npk }) + (if has("package3d") { 0 } else { npk });
    let proven = missing.is_empty() && captured.len() == expected;
    let n_captured = captured.len();
    let mut skip_sorted = skip.clone(); skip_sorted.sort();
    let mut out = json!({"success": proven, "proven": proven, "libraryPath": lbr, "deviceset": want, "artifacts": artifacts, "missing": missing, "stepsSkipped": skip_sorted, "sharedPackageProof": shared_with, "evidence": evidence, "imagesCaptured": n_captured, "imagesRequired": 4});
    if let Some(t) = thumb_unverified { out["thumbnailUnverified"] = json!(t); out["_thumbnailNote"] = json!("The cloud thumbnail could not be observed (APS not signed in on this box) - the 4-frame local evidence stands, but verify the Library Manager preview by eye or sign in APS and run fusion_check_3d_thumbnail."); }
    let stem = lbr.rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_default();
    crate::chains::note_bridge_doc_pub(&ctx.state_dir, &stem);
    out["scratchDocClosed"] = json!(crate::orchestrations::close_bridge_doc_verified(ctx, &stem, 30, pass)["closed"].as_bool().unwrap_or(false));
    let _ = std::fs::write(std::env::temp_dir().join("fusion-prove-last.json"), out.to_string());
    if proven { out["_hint"] = json!("PROVEN: all three artifacts exist and all FOUR evidence frames were captured app-sized - symbol editor, footprint editor, 3D editor, and the ganged Library Manager view with pin connections. Attach evidence[].screenshot in that order."); }
    else {
        let mut why = Vec::new();
        let m: Vec<String> = out["missing"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
        if !m.is_empty() { why.push(format!("missing artifacts: {}", m.join(", "))); }
        if n_captured != 4 { why.push(format!("only {n_captured} of 4 evidence frames captured")); }
        out["error"] = json!(format!("NOT proven - {}", why.join("; "))); out["errorCode"] = json!("library_not_proven");
        out["_hint"] = json!("Do NOT present these frames as proof. A missing package3d renders as a grey Placeholder that photographs like a real chip, and a frame that failed its size check is not evidence of anything. Bind the 3D with fusion_show_3d_package {stepPath} and re-run.");
    }
    out
}

fn py_list(v: &[String], n: usize) -> String { format!("[{}]", v.iter().take(n).map(|s| fusion_lbr::py_repr_str(s)).collect::<Vec<_>>().join(", ")) }

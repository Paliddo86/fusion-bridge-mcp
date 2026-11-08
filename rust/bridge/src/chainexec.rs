//! The chain executors the exe used to ask the sidecar for (`chain_*`), native.
//! Each mirrors one `_handle_chain_*` in server.py; the chain itself (chains.rs) is unchanged
//! and reaches these through `ExeChainHost::call` when the exe owns the add-in.
use crate::addin::Ctx;
use fusion_core::ab::{AbClient, Loopback};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

/// `chain_docs_open`: every document open right now (the chain's `before` snapshot).
pub fn docs_open(pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut names = crate::orchestrations::open_doc_names(pass);
    names.sort();
    json!({"success": true, "documents": names})
}

/// `chain_close_docs_since`: close what was not in `before`, keeping `keep`.
pub fn close_docs_since(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let strs = |k: &str| -> Vec<String> { args[k].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default() };
    json!({"success": true, "closed": crate::mint::close_docs_opened_since(ctx, &strs("before"), &strs("keep"), pass)})
}

/// `chain_close_bridge_doc`: close one bridge document, verified.
pub fn close_bridge_doc(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut r = crate::orchestrations::close_bridge_doc_verified(ctx, args["name"].as_str().unwrap_or(""), 60, pass);
    r["success"] = json!(true); r
}

/// `chain_sweep_orphans`: close scratch documents a chain that died with a bridge respawn left
/// open. Only runs on file as bridge_restarted_mid_run and not yet swept; only never-modified
/// Untitled* and <pkg>_3d designs. A user's named or modified document is never touched.
pub fn sweep_orphans(ctx: &Ctx, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut runs = crate::chains::read_runs();
    let pending: Vec<String> = runs.iter().filter(|(_, st)| st["errorCode"].as_str() == Some("bridge_restarted_mid_run") && st["scratchSwept"].as_bool() != Some(true)).map(|(k, _)| k.clone()).collect();
    if pending.is_empty() { return json!({"success": true, "swept": 0}); }
    if !crate::launch::round_trip_ready(1) { return json!({"success": true, "swept": 0, "why": "add-in not answering"}); }
    let di = pass("document_info", json!({}));
    let mut closed: Vec<String> = Vec::new();
    for d in di["data"]["openDocuments"].as_array().cloned().unwrap_or_default() {
        let raw = d["name"].as_str().unwrap_or("").to_string();
        let n = raw.trim_end_matches('*').trim().to_string();
        let modified = d["isModified"].as_bool().unwrap_or(false) || d["modified"].as_bool().unwrap_or(false) || raw.ends_with('*');
        let l = n.to_lowercase();
        if (l.starts_with("untitled") && !modified) || l.ends_with("_3d") {
            if crate::orchestrations::close_bridge_doc_verified(ctx, &n, 60, pass)["closed"].as_bool() == Some(true) { closed.push(n); }
        }
    }
    for rid in &pending { if let Some(st) = runs.get_mut(rid) { st["scratchSwept"] = json!(true); st["scratchSweptDocs"] = json!(closed); } }
    crate::chains::write_runs_pub(&runs);
    crate::fusion_prefs::action_ledger(&ctx.state_dir, "orphan_scratch_docs_swept", json!({"runs": pending, "closed": closed}));
    json!({"success": true, "swept": closed.len(), "closed": closed})
}

/// `chain_publish_library` / `_publish_library_to_cloud`: save the bound library into the
/// user's cloud project so Fusion's Library Manager lists it. Folder created if missing; a
/// same-named library already there is reported (this saves a NEW document and says so).
pub fn publish_library(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let p = args["libraryPath"].as_str().unwrap_or("").replace('\\', "/");
    if p.is_empty() { return json!({"success": false, "error": "libraryPath required"}); }
    let folder = args["folder"].as_str().filter(|s| !s.is_empty()).unwrap_or("Adom Libraries").to_string();
    let stem = p.rsplit('/').next().unwrap_or("").rsplitn(2, '.').last().unwrap_or("").to_string();
    let name = { let n = args["name"].as_str().unwrap_or("").trim().to_string(); if n.is_empty() { stem.clone() } else { n } };
    let mut out = json!({"success": true, "saved": false, "name": name, "folder": folder});
    let short = |s: &str| s.chars().take(120).collect::<String>();
    // the project: the default upload project by id, else "Main", else the first
    let pr = pass("list_cloud_projects", json!({}));
    let mut projects: Vec<Value> = Vec::new();
    for hub in pr["data"]["hubs"].as_array().cloned().unwrap_or_default() { for pj in hub["projects"].as_array().cloned().unwrap_or_default() { if pj["name"].as_str().map(|s| !s.is_empty()).unwrap_or(false) { projects.push(pj); } } }
    if pr["success"].as_bool() != Some(true) { out["projectNote"] = json!(short(pr["error"].as_str().unwrap_or("list_cloud_projects failed"))); }
    let mut project_name = projects.iter().find(|pj| pj["id"].as_str() == Some(crate::mint::DEFAULT_UPLOAD_PROJECT)).and_then(|pj| pj["name"].as_str()).unwrap_or("").to_string();
    if project_name.is_empty() { project_name = projects.iter().find(|pj| pj["name"].as_str() == Some("Main")).or(projects.first()).and_then(|pj| pj["name"].as_str()).unwrap_or("").to_string(); }
    out["project"] = if project_name.is_empty() { Value::Null } else { json!(project_name) };
    let pn = |mut a: Value| -> Value { if !project_name.is_empty() { a["projectName"] = json!(project_name); } a };
    let r = pass("create_cloud_folder", pn(json!({"folderName": folder})));
    if r["success"].as_bool() == Some(true) { out["folderCreated"] = json!(r["data"]["existed"].as_bool() != Some(true)); } else { out["folderNote"] = json!(short(r["error"].as_str().unwrap_or("create_cloud_folder failed"))); }
    let lst = pass("list_cloud_files", pn(json!({"folderPath": folder})));
    if lst["success"].as_bool() == Some(true) {
        let older: Vec<Value> = lst["data"]["files"].as_array().cloned().unwrap_or_default().into_iter().filter(|f| f["name"].as_str().unwrap_or("").to_lowercase() == name.to_lowercase()).collect();
        out["duplicatesBefore"] = json!(older.len());
        out["olderSameName"] = json!(older.iter().map(|f| f["id"].clone()).collect::<Vec<_>>());
    } else { out["duplicatesBefore"] = Value::Null; }
    // the library must be open to be saved
    let names = crate::orchestrations::open_doc_names(pass);
    let bare: Vec<String> = names.iter().map(|n| { let s = n.replace('*', ""); let s = match s.find(" (") { Some(i) => s[..i].to_string(), None => s }; s.trim().to_lowercase() }).collect();
    if !names.is_empty() && !bare.contains(&stem.to_lowercase()) {
        let op = crate::orchestrations::open_lbr(ctx, &json!({"filePath": p}), pass);
        out["reopened"] = json!(op["success"].as_bool() == Some(true));
        if op["success"].as_bool() != Some(true) { out["error"] = json!(format!("could not reopen the library to save it: {}", op["error"].as_str().unwrap_or("").chars().take(160).collect::<String>())); return out; }
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut seen = json!(null); let mut active_ok = false;
    while Instant::now() < deadline {
        let _ = pass("activate_document", json!({"name": stem}));
        let st = pass("get_app_state", json!({}));
        seen = st["data"].clone();
        if seen["activeDocument"].as_str().unwrap_or("").to_lowercase().contains(&stem.to_lowercase()) { active_ok = true; break; }
        std::thread::sleep(Duration::from_secs(2));
    }
    if !active_ok { out["error"] = json!(format!("library document {} could not be made active (last seen: {}), so it was not saved", fusion_lbr::py_repr_str(&stem), seen.to_string().chars().take(160).collect::<String>())); return out; }
    let r = pass("save_to_cloud", pn(json!({"name": name, "folderPath": folder, "description": "Imported by Adom (bound 3D packages)"})));
    let d = r["data"].clone();
    if r["success"].as_bool() != Some(true) { out["error"] = json!(short(r["error"].as_str().unwrap_or("save_to_cloud failed"))); return out; }
    out["saved"] = json!(true); out["project"] = d["project"].clone(); if d["folder"].as_str().map(|s| !s.is_empty()).unwrap_or(false) { out["folder"] = d["folder"].clone(); }
    out["fileId"] = d["fileId"].clone(); out["versionId"] = d["versionId"].clone(); out["versionNumber"] = d["versionNumber"].clone();
    let mut clean = false;
    for _ in 0..2 {
        let di = pass("document_info", json!({}));
        for dd in di["data"]["openDocuments"].as_array().cloned().unwrap_or_default() {
            let n = dd["name"].as_str().unwrap_or("");
            if n.trim_end_matches('*').trim().to_lowercase() == stem.to_lowercase() { clean = dd["isSaved"].as_bool().unwrap_or(false) && !(dd["isModified"].as_bool().unwrap_or(false) || n.ends_with('*')); }
        }
        if clean { break; }
        let _ = pass("run_modeling_script", json!({"script": "import adsk.core\napp=adsk.core.Application.get()\nd=app.activeDocument\nok=d.save('Imported by Adom') if d and not d.isSaved else True\nresult={'saved': bool(ok), 'isSaved': bool(d.isSaved) if d else None}"}));
    }
    out["savedClean"] = json!(clean);
    out
}

fn user_unsaved_docs(state_dir: &std::path::Path, names: &[String]) -> Vec<String> {
    names.iter().filter(|raw| {
        let s = raw.replace('*', ""); let bare = match s.find(" (") { Some(i) => s[..i].to_string(), None => s }; let bare = bare.trim().to_lowercase();
        !bare.is_empty() && crate::etiquette::bridge_doc_in(state_dir, &bare).is_none() && !bare.starts_with("untitled") && !bare.ends_with("_3d")
    }).cloned().collect()
}

/// `chain_addin_restart_info` / `_addin_restart_info`: everything a caller needs to tell a
/// human that Fusion must restart, plus a toast (throttled to one per 15 minutes) whose
/// answer a background watcher honours. Document counts come from the add-in, which by
/// definition may not be answering here; `documentsKnown` says which. A count we cannot
/// take is not a count we may invent.
pub fn addin_restart_info(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let fire_toast = args["fireToast"].as_bool().unwrap_or(true);
    let mut info = json!({"success": true, "restartRequired": true, "reason": "addin_installed_while_fusion_running",
        "explanation": "The AdomBridge add-in was installed or updated while Fusion was already running. Fusion loads add-ins only at launch, so this Fusion cannot see it until it restarts. Nothing is wrong with the install and nothing is lost - it just needs a relaunch.",
        "userMessage": "Fusion needs a quick restart to finish setting up the Adom add-in. We can save your open documents first.",
        "remedyVerb": "fusion_restart_for_addin", "documentsKnown": false, "openDocuments": null, "unsavedDocuments": null});
    let st = pass("get_app_state", json!({}));
    let docs = if st["documents"].is_array() { st["documents"].clone() } else { st["data"]["documents"].clone() };
    let mut unames: Option<Vec<String>> = None;
    if let Some(ds) = docs.as_array() {
        info["documentsKnown"] = json!(true); info["openDocuments"] = json!(ds.len());
        let un: Vec<String> = ds.iter().filter(|d| d["isSaved"].as_bool() != Some(true)).filter_map(|d| d["name"].as_str().map(str::to_string)).collect();
        info["unsavedDocuments"] = json!(un.len());
        info["unsavedDocumentNames"] = json!(un.iter().take(20).collect::<Vec<_>>());
        info["documentNames"] = json!(ds.iter().filter_map(|d| d["name"].as_str()).take(20).collect::<Vec<_>>());
        unames = Some(un);
    }
    let yours: Vec<String> = match &unames { Some(u) => { let y = user_unsaved_docs(&ctx.state_dir, u); info["userUnsaved"] = json!(y); info["autoRestart"] = json!(y.is_empty()); y } None => { info["autoRestart"] = json!(false); vec![] } };
    let policy = crate::lifecycle::restart_policy(&ctx.state_dir);
    info["restartPolicy"] = json!(policy);
    let declined = crate::lifecycle::auto_restart_declined_recently(&ctx.state_dir);
    if declined { info["autoRestartDeclinedRecently"] = json!(true); }
    let yours = if policy == "force" {
        // an AI-only machine: every open document is the AI's own, nothing to ask
        if !yours.is_empty() { info["userUnsavedIgnored"] = json!(yours); info["userUnsaved"] = json!([]); }
        info["autoRestart"] = json!(true);
        info["userMessage"] = json!("Fusion is restarting itself to load the Adom add-in (restartPolicy force). Open documents are saved where Fusion can save them; never-saved documents are discarded.");
        Vec::new()
    } else if policy == "auto" && yours.is_empty() && !declined {
        // no unsaved work that is not the bridge's (or none known): restart without asking; the
        // restart itself stops short of any unsaved work it meets and asks then
        info["autoRestart"] = json!(true);
        info["userMessage"] = json!("Fusion is restarting itself to load the Adom add-in (restartPolicy auto). Open documents are saved first; nobody's unsaved work is discarded.");
        Vec::new()
    } else { yours };
    if fire_toast && info["autoRestart"].as_bool() != Some(true) {
        let f = ctx.state_dir.join("addin_restart_toast.json");
        let _ = std::fs::create_dir_all(&ctx.state_dir);
        let last = std::fs::read_to_string(&f).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["at"].as_f64()).unwrap_or(0.0);
        if now() - last > 900.0 {
            let body = if !yours.is_empty() { format!("Fusion needs a quick restart to finish setting up the Adom add-in. Blocked only by {} unsaved document(s) that look like YOURS: {}. Everything else open is Adom's own scratch and is handled automatically. We will save yours first.", yours.len(), yours.iter().take(5).cloned().collect::<Vec<_>>().join(", ")) }
                else if info["documentsKnown"].as_bool() == Some(true) { format!("Fusion needs a quick restart to finish setting up the Adom add-in. You have {} document(s) open{} - we will save them first.", info["openDocuments"], if info["unsavedDocuments"].as_u64().unwrap_or(0) > 0 { format!(", {} unsaved", info["unsavedDocuments"]) } else { String::new() }) }
                else { info["userMessage"].as_str().unwrap_or("").to_string() };
            let r = match Loopback::from_env() {
                Some(ab) => ab.call("notify_user", json!({"title": "Fusion needs a restart", "body": body, "level": "info", "target": "all", "buttons": [{"label": "Save and restart", "id": "restart"}, {"label": "I'll handle it", "id": "later"}]}), &ctx.caller, 20).unwrap_or_else(|e| json!({"error": format!("{e:?}")})),
                None => json!({"error": "ab loopback unavailable"}),
            };
            let delivered = r["success"].as_bool() == Some(true) || r["status"].as_str() == Some("ok") || r["summary"]["ok"].as_f64().unwrap_or(0.0) > 0.0 || r["delivered"].as_bool() == Some(true);
            let id = r["id"].clone().as_str().map(str::to_string).or_else(|| r["data"]["id"].as_str().map(str::to_string)).or_else(|| r["notificationId"].as_str().map(str::to_string));
            info["toast"] = json!({"delivered": delivered, "id": id, "error": r["error"], "choices": ["Save and restart", "I'll handle it"]});
            let _ = std::fs::write(&f, json!({"at": now(), "id": id}).to_string());
            if let Some(nid) = id { watch_restart_choice(ctx.state_dir.clone(), ctx.caller.clone(), nid); }
        } else { info["toast"] = json!({"delivered": false, "suppressed": "already toasted recently"}); }
    }
    info
}

/// `_watch_restart_choice`: wait in the background for the user's answer to the restart
/// notice and honour it. "Save and restart" -> fusion_restart_for_addin {save:true} on this
/// exe; "I'll handle it" -> nothing but the ledger entry. Either way "why did my Fusion
/// restart" has an answer with a timestamp.
fn watch_restart_choice(state_dir: std::path::PathBuf, caller: fusion_core::ab::Caller, nid: String) {
    std::thread::spawn(move || {
        let Some(ab) = Loopback::from_env() else { return };
        let deadline = Instant::now() + Duration::from_secs(1800);
        while Instant::now() < deadline {
            if let Ok(x) = ab.call("notify_response", json!({"id": nid, "target": "all"}), &caller, 30) {
                let x = if x["data"].is_object() { x["data"].clone() } else { x };
                let resp = if x["response"].is_object() { x["response"]["action"].clone().as_str().or(x["response"]["button"].as_str()).or(x["response"]["id"].as_str()).map(str::to_string) } else { x["response"].as_str().map(str::to_string) };
                let btn = x["buttonId"].as_str().or(x["button"].as_str()).map(str::to_string).or(resp);
                if x["pending"].as_bool() == Some(false) || btn.is_some() || x["dismissed"].as_bool() == Some(true) {
                    crate::fusion_prefs::action_ledger(&state_dir, "restart_choice_seen", json!({"raw": btn.clone().unwrap_or_default().chars().take(40).collect::<String>(), "notification": nid}));
                    match btn.as_deref() {
                        Some("restart") | Some("Save and restart") => {
                            crate::fusion_prefs::action_ledger(&state_dir, "restart_choice", json!({"choice": "restart", "notification": nid}));
                            let r = crate::launch::SELF_URL.lock().unwrap().clone().and_then(|u| ureq::post(&u).timeout(Duration::from_secs(900)).send_json(json!({"command": "fusion_restart_for_addin", "args": {"save": true, "wait": true}})).ok()).and_then(|r| r.into_json::<Value>().ok()).unwrap_or(json!({}));
                            crate::fusion_prefs::action_ledger(&state_dir, "restart_choice_result", json!({"success": r["success"], "errorCode": r["errorCode"], "error": r["error"]}));
                        }
                        _ => { crate::fusion_prefs::action_ledger(&state_dir, "restart_choice", json!({"choice": "later", "raw": btn, "notification": nid})); }
                    }
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

//! The standard's app-tier verbs in Fusion's terms: diagnostics (facts for a bug report), instances
//! (every running Fusion), progress (the bridge's timing book and what is in flight) and foreground
//! (Fusion brought in front, the sanctioned exception, with the user told first).

use serde_json::{json, Value};
use std::path::Path;

use fusion_core::ab::{AbClient, Caller, Loopback};

/// Environment variable names that may hold a credential: their values are never shown.
fn secret(name: &str) -> bool {
    let n = name.to_uppercase();
    ["TOKEN", "KEY", "SECRET", "PASS", "TICKET", "AUTH", "COOKIE", "CREDENTIAL", "SESSION"].iter().any(|w| n.contains(w))
}

/// `fusion_instances`: every running Fusion process (Fusion runs one per user; its helper
/// processes are listed under their parent).
pub fn instances(procs: &[fusion_core::host::FusionProcess]) -> Value {
    let rows: Vec<Value> = procs.iter().map(|p| json!({"pid": p.pid, "image": p.image, "parentPid": p.parent_pid})).collect();
    let n = rows.len();
    json!({"success": true, "mutated": false, "count": n, "instances": rows,
           "_hint": if n == 0 { "Fusion is not running: fusion_start launches it." } else { "Fusion runs one instance per user; verbs drive the one that is running." }})
}

/// `fusion_progress`: what the bridge is doing right now (its long operation, if any), the phase
/// timings it has measured and its recent verb runs.
pub fn progress(state_dir: &Path, long_op: Option<String>, since: f64, recent: usize) -> Value {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let active: Vec<Value> = long_op.map(|op| json!({"verb": op, "runningSec": if since > 0.0 { ((now - since) * 10.0).round() / 10.0 } else { 0.0 },
        "estimateSec": crate::timings::verb_estimate(state_dir, &op).0})).into_iter().collect();
    let book = crate::timings::load(state_dir);
    let mut verb_times = serde_json::Map::new();
    let mut history = serde_json::Map::new();
    for (k, v) in book.as_object().into_iter().flatten() {
        let s: Vec<f64> = v["samples"].as_array().into_iter().flatten().filter_map(|x| x.as_f64()).collect();
        if s.is_empty() { continue; }
        let mut sorted = s.clone(); sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let row = json!({"runs": s.len(), "p50": sorted[sorted.len() / 2], "max": sorted[sorted.len() - 1], "lastAt": v["lastAt"]});
        match k.strip_prefix("verb:") { Some(verb) => { verb_times.insert(verb.trim_end_matches("|any").to_string(), row); } None => { history.insert(k.clone(), row); } }
    }
    let last: Vec<Value> = std::fs::read_to_string(state_dir.join("verb-times.jsonl")).unwrap_or_default().lines().rev().take(recent).filter_map(|l| serde_json::from_str(l).ok()).collect();
    json!({"success": true, "mutated": false, "active": active, "history": history, "verbTimes": verb_times, "recentRuns": last})
}

/// `fusion_diagnostics`: the facts a bug report needs, read without driving Fusion.
pub fn diagnostics(procs: &[fusion_core::host::FusionProcess], exe: Option<std::path::PathBuf>, state_dir: &Path, caller: &Caller) -> Value {
    let env: serde_json::Map<String, Value> = std::env::vars()
        .filter(|(k, _)| { let u = k.to_uppercase(); u.starts_with("ADOM") || u.contains("FUSION") || u.contains("AUTODESK") || ["APPDATA", "LOCALAPPDATA", "USERPROFILE", "PROCESSOR_ARCHITECTURE", "OS"].contains(&u.as_str()) })
        .map(|(k, v)| { let shown = if secret(&k) { json!("(set, not shown)") } else { json!(v) }; (k, shown) }).collect();
    let windows = Loopback::from_env().map(|ab| { let f = fusion_core::windows_facts::facts_from_ab(&ab, caller); json!({"mainTitle": f.main_title, "mainHwnd": f.main_hwnd, "needsSignin": f.needs_signin, "licensingDialog": f.licensing_dialog, "windows": f.family}) }).unwrap_or(json!(null));
    let addin = crate::addin::addin_http("get_app_state", &json!({}), 3);
    let manifest = |p: &Path| std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).map(|v| v["version"].clone()).unwrap_or(Value::Null);
    let here = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf()));
    json!({"success": true, "mutated": false,
           "bridge": {"version": env!("CARGO_PKG_VERSION"), "exe": std::env::current_exe().ok().map(|p| p.to_string_lossy().to_string()),
                      "bundledAddin": here.as_ref().map(|d| manifest(&d.join("addin").join("AdomBridge").join("AdomBridge.manifest"))), "cacheDir": here.map(|d| d.to_string_lossy().to_string()),
                      "stateDir": state_dir.to_string_lossy()},
           "installs": {"fusionLauncher": exe.map(|p| p.to_string_lossy().to_string())},
           "running": procs.iter().map(|p| json!({"pid": p.pid, "image": p.image})).collect::<Vec<_>>(),
           "addin": {"answering": addin["success"].as_bool() == Some(true), "activeDocument": addin["data"]["activeDocument"], "error": addin["error"]},
           "windows": windows, "env": env,
           "_hint": "Facts for a bug report, read without driving Fusion. Values of variables named like credentials are never shown. fusion_action_ledger has what the bridge did lately; fusion_progress its timings."})
}

/// `fusion_foreground {reason}`: Fusion brought in front of the user, the one sanctioned exception
/// to running in the background. The user is told why first and given three seconds to yield.
pub fn foreground(args: &Value, state_dir: &Path, caller: &Caller) -> Value {
    let reason = args["reason"].as_str().or(args["foregroundReason"].as_str()).unwrap_or("").trim().to_string();
    if reason.chars().count() < 10 {
        return json!({"success": false, "errorCode": "foreground_reason_required", "error": "reason is required: a sentence of 10 characters or more, written for the person at the PC",
                      "_hint": "Fusion verbs run in the background so the user is never interrupted. Bring it forward only when the user wants to watch, and say why in words they will read."});
    }
    let Some(ab) = Loopback::from_env() else { return json!({"success": false, "errorCode": "ab_unreachable", "error": "Adom Bridge did not answer on this machine"}) };
    let facts = fusion_core::windows_facts::facts_from_ab(&ab, caller);
    let Some(hwnd) = facts.main_hwnd else { return json!({"success": false, "errorCode": "fusion_not_running", "error": "no Fusion window to bring forward", "_hint": "fusion_start launches it."}) };
    crate::etiquette::note_focus_event(state_dir, "foreground", "foreground", &reason);
    let _ = ab.call("notify_user", json!({"title": "Fusion is coming to the foreground", "body": format!("{reason} (Requested by your AI - Fusion verbs otherwise always run in the background.)"), "level": "info", "target": "all"}), caller, 10);
    std::thread::sleep(std::time::Duration::from_secs(3));
    let r = ab.call("desktop_bring_to_front", json!({"hwnd": hwnd}), caller, 15);
    let ok = r.as_ref().map(|v| v["success"].as_bool() != Some(false)).unwrap_or(false);
    json!({"success": ok, "mutated": false, "hwnd": hwnd, "title": facts.main_title, "windowEtiquette": "foreground (sanctioned, the user was told first)",
           "error": if ok { Value::Null } else { json!(r.err().map(|e| e.to_string()).unwrap_or_else(|| "ab could not bring the window forward".into())) }})
}

/// `fusion_list_installed_libraries`: the libraries Fusion offers to place from are its hub's
/// electronics libraries (.flbr files: what Library Manager's Hub Libraries tab shows, and where
/// fusion_install_library_bundle saves). Fusion keeps no local list a script can read and draws that
/// tab without UI Automation, so this walks the hub's projects and folders (breadth first, to a depth,
/// within a time budget) through the signed-in session and lists the .flbr files.
pub fn list_installed_libraries(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let started = std::time::Instant::now();
    let budget = std::time::Duration::from_secs(args["budgetSeconds"].as_u64().unwrap_or(150).clamp(10, 900));
    let depth = args["depth"].as_u64().unwrap_or(3) as usize;
    let projects: Vec<String> = match args["projectName"].as_str() {
        Some(p) => vec![p.to_string()],
        None => {
            let r = pass("list_cloud_projects", json!({}));
            if r["success"].as_bool() == Some(false) { return r; }
            let d = if r["data"].is_object() { &r["data"] } else { &r };
            d["hubs"].as_array().into_iter().flatten().flat_map(|h| h["projects"].as_array().cloned().unwrap_or_default()).filter_map(|p| p["name"].as_str().map(str::to_string)).collect()
        }
    };
    let (mut libs, mut walked, mut complete) = (Vec::new(), 0usize, true);
    'projects: for proj in &projects {
        let mut queue: std::collections::VecDeque<(String, usize)> = std::collections::VecDeque::from([(String::new(), 0)]);
        while let Some((folder, lvl)) = queue.pop_front() {
            if started.elapsed() > budget { complete = false; break 'projects; }
            let mut a = json!({"projectName": proj});
            if !folder.is_empty() { a["folderPath"] = json!(folder); }
            let r = pass("list_cloud_files", a);
            let d = if r["data"].is_object() { r["data"].clone() } else { r.clone() };
            walked += 1;
            if d["listTimedOut"] == true { complete = false; }
            for f in d["files"].as_array().into_iter().flatten().filter(|f| f["fileExtension"].as_str().map(|e| e.eq_ignore_ascii_case("flbr")).unwrap_or(false)) {
                libs.push(json!({"name": f["name"], "project": proj, "folder": folder, "version": f["versionNumber"], "id": f["id"], "modified": f["dateModified"]}));
            }
            if lvl < depth {
                // list_cloud_files names each subfolder as {name, ...} (a bare string from older add-ins)
                for sf in d["subfolders"].as_array().into_iter().flatten().filter_map(|x| x["name"].as_str().or(x.as_str())) {
                    queue.push_back((if folder.is_empty() { sf.to_string() } else { format!("{folder}/{sf}") }, lvl + 1));
                }
            }
        }
    }
    let n = libs.len();
    json!({"success": true, "mutated": false, "count": n, "libraries": libs, "projects": projects.len(), "foldersWalked": walked, "complete": complete,
           "elapsedSec": (started.elapsed().as_secs_f64() * 10.0).round() / 10.0,
           "_hint": if complete { "Fusion's hub libraries (.flbr): Library Manager's Hub Libraries tab shows these. fusion_install_library_bundle adds one; fusion_open_cloud_file opens one." }
                    else { "Cut short (budgetSeconds or a folder listing timed out): pass projectName to walk one project, or a larger budgetSeconds." }})
}

/// `fusion_panel {name?}`: the standard's panel in Fusion's terms. Fusion's docked panels
/// (Browser, Inspector, Display Layers, Selection Filter, Design Manager, Place Components, Errors,
/// Bill of Materials, Text Commands, ...) answer its Panels.* text commands: Panels.List names them,
/// Panels.Expose opens one, and Panels.ValidateState <title> EXPANDED proves it is open (checked live
/// on Fusion 2706, 2026-10-30). No window takes the foreground.
fn panel_titles(pass: &dyn Fn(&str, Value) -> Value) -> Result<Vec<String>, Value> {
    let r = pass("execute_text_command", json!({"command": "Panels.List"}));
    if r["success"].as_bool() == Some(false) { return Err(r); }
    let text = r["data"]["output"].as_str().or(r["output"].as_str()).or(r["data"]["result"].as_str()).unwrap_or("").to_string();
    Ok(panel_titles_from(&text))
}

fn panel_titles_from(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.ends_with(':')) {
        let l = l.trim_start_matches("All Panels:").trim();
        if !l.is_empty() && !out.iter().any(|o| o == l) { out.push(l.to_string()); }
    }
    out
}

fn quoted(title: &str) -> String { if title.contains(' ') { format!("\"{title}\"") } else { title.to_string() } }

pub fn panel(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let t0 = std::time::Instant::now();
    let titles = match panel_titles(pass) { Ok(t) => t, Err(e) => return e };
    let Some(want) = args["name"].as_str().or(args["id"].as_str()).map(|s| s.trim().to_string()) else {
        return json!({"success": true, "mutated": false, "panels": titles, "_hint": "Pass name (a title from panels, case does not matter) to open that panel."});
    };
    let w = want.to_uppercase();
    let Some(title) = titles.iter().find(|t| **t == w).or_else(|| titles.iter().find(|t| t.contains(&w))).cloned() else {
        return json!({"success": false, "errorCode": "panel_not_found", "error": format!("The editor in front has no panel named {want} (Fusion lists panels per editor: Inspector and Display Layers belong to the electronics editors)"), "panels": titles, "_hint": "panels lists the ones this editor has; fusion_show_2d_board or fusion_show_schematic brings an electronics editor forward."});
    };
    let state = |s: &str| pass("execute_text_command", json!({"command": format!("Panels.ValidateState {} {s}", quoted(&title))}))["success"].as_bool() == Some(true);
    let already = state("EXPANDED");
    if !already {
        let r = pass("execute_text_command", json!({"command": format!("Panels.Expose {}", quoted(&title))}));
        if r["success"].as_bool() == Some(false) { return json!({"success": false, "errorCode": "panel_not_selected", "error": r["error"], "panel": title}); }
    }
    let verified = already || state("EXPANDED");
    json!({"success": verified, "mutated": !already, "panel": title, "selected": verified, "verified": verified, "lane": if already { "already" } else { "panels_expose" },
           "docked": state("DOCKED"), "elapsedSec": (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0,
           "errorCode": if verified { Value::Null } else { json!("panel_not_selected") }})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panel_titles_come_from_panels_list() {
        let t = panel_titles_from("All Panels: \nBROWSER\nINSPECTOR\nDISPLAY LAYERS\nAUTODESK ASSISTANT\nAUTODESK ASSISTANT\n");
        assert_eq!(t, vec!["BROWSER", "INSPECTOR", "DISPLAY LAYERS", "AUTODESK ASSISTANT"]);
        assert_eq!(quoted("DISPLAY LAYERS"), "\"DISPLAY LAYERS\"");
    }
    #[test]
    fn credential_names_are_never_shown() {
        assert!(secret("ADOM_API_KEY") && secret("ADOM_SESSION") && secret("FUSION_AUTH_TOKEN") && secret("password_file"));
        assert!(!secret("ADOM_AI_THREAD") && !secret("LOCALAPPDATA"));
        let p = progress(Path::new("/nonexistent"), Some("export_gerbers".into()), 0.0, 5);
        assert_eq!(p["active"][0]["verb"], "export_gerbers");
    }
}

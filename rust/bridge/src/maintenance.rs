//! Fusion's own update offer, the silent installer and the bridge uninstall, native.

use serde_json::{json, Value};
use std::time::Duration;

use crate::addin::Ctx;
use fusion_core::ab::{AbClient, Loopback};

const UPDATE_BUTTONS: &[&str] = &["Update Now", "Update now", "Install Now", "Restart Now"];
const UPDATE_BUTTONS_EXIT_CLASS: &[&str] = &["Install Now", "Restart Now"];

fn find_controls_any(v: &Value) -> Value {
    fn walk(v: &Value, depth: u8) -> Value {
        if depth > 5 { return json!({}); }
        if let Some(s) = v.as_str() { if s.starts_with('{') || s.starts_with('[') { if let Ok(p) = serde_json::from_str::<Value>(s) { return walk(&p, depth + 1); } } return json!({}); }
        if let Some(o) = v.as_object() {
            if o.get("best").map(|b| b.is_object()).unwrap_or(false) || o.get("controls").map(|c| c.is_array()).unwrap_or(false) { return json!({"best": o.get("best").cloned().unwrap_or(json!({})), "controls": o.get("controls").cloned().unwrap_or(json!([]))}); }
            for (_, x) in o { let r = walk(x, depth + 1); if r.as_object().map(|m| !m.is_empty()).unwrap_or(false) { return r; } }
        }
        json!({})
    }
    walk(v, 0)
}

/// `_find_update_offer`: is Fusion offering an update (a button on any family window)?
pub fn find_update_offer(ctx: &Ctx) -> Value { find_update_offer_within(ctx, None) }

/// The same search, stopped once `budget` has passed (`complete:false` then): every label on every
/// family window is a UI Automation search, which can outlast a relay's budget.
pub fn find_update_offer_within(ctx: &Ctx, budget: Option<Duration>) -> Value {
    let t0 = std::time::Instant::now();
    let Some(ab) = Loopback::from_env() else { return json!({"offered": false}) };
    let facts = fusion_core::windows_facts::facts_from_ab(&ab, &ctx.caller);
    let Some(hw) = facts.main_hwnd else { return json!({"offered": false}) };
    let mut candidates = vec![hw];
    for w in &facts.family { if let Some(h) = w["hwnd"].as_i64() { if h != hw { candidates.push(h); } } }
    for h in candidates {
        for label in UPDATE_BUTTONS {
            if budget.map(|b| t0.elapsed() > b).unwrap_or(false) { return json!({"offered": false, "hwnd": hw, "complete": false}); }
            let r = ab.call("desktop_find_control", json!({"hwnd": h, "name": label}), &ctx.caller, 20).map(|v| find_controls_any(&v)).unwrap_or(json!({}));
            let best = &r["best"];
            if best["name"].as_str().map(|n| !n.is_empty()).unwrap_or(false) && best["invokable"].as_bool().unwrap_or(false) {
                crate::small_verbs::set_pref(&ctx.state_dir, "lastUpdateOffer", json!({"offered": true, "at": now_f(), "button": best["name"]}));
                return json!({"offered": true, "button": best["name"], "hwnd": h, "detail": format!("Fusion is offering an update ('{}' is on screen)", best["name"].as_str().unwrap_or(""))});
            }
        }
    }
    json!({"offered": false, "hwnd": hw})
}

fn now_f() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

/// `_fusion_verifiably_idle`: may something EXIT Fusion right now?
pub fn fusion_verifiably_idle(pass: &dyn Fn(&str, Value) -> Value) -> Value {
    if let Some((name, _)) = crate::long_op_marker() { return json!({"idle": false, "why": format!("long op in flight: {name}")}); }
    if let Some(t) = crate::etiquette::last_drive_at() { let ago = now_f() - t; if ago < 180.0 { return json!({"idle": false, "why": format!("driving verb {}s ago", ago as i64)}); } }
    let st = pass("document_info", json!({}));
    if st["success"].as_bool() != Some(true) { return json!({"idle": false, "why": "add-in unreachable - cannot prove no open docs"}); }
    let n = st["data"]["openDocuments"].as_array().map(|a| a.len()).unwrap_or(0);
    if n > 0 { return json!({"idle": false, "why": format!("{n} document(s) open")}); }
    json!({"idle": true, "why": "no long op, no recent driving, no open documents"})
}

/// `_apply_fusion_update_silently` + the verb shape (`fusion_apply_fusion_update`).
pub fn apply_fusion_update(ctx: &Ctx, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let enabled = crate::small_verbs::get_pref(&ctx.state_dir, "autoUpdateFusion").map(|v| v != json!(false)).unwrap_or(true);
    let offered_before = crate::small_verbs::get_pref(&ctx.state_dir, "lastUpdateOffer").map(|v| v["offered"].as_bool().unwrap_or(false)).unwrap_or(false);
    let r = if !enabled { json!({"applied": false, "why": "user opted out (sticky pref autoUpdateFusion=false)"}) } else {
        let offer = find_update_offer(ctx);
        if offer["offered"].as_bool() != Some(true) { json!({"applied": false, "why": "no update offered"}) } else {
            let button = offer["button"].as_str().unwrap_or("").to_string();
            let exit_class = UPDATE_BUTTONS_EXIT_CLASS.contains(&button.as_str());
            let mut deferred = None;
            if exit_class { let idle = fusion_verifiably_idle(pass); if idle["idle"].as_bool() != Some(true) { crate::fusion_prefs::action_ledger(&ctx.state_dir, "auto_update_deferred", json!({"button": button, "notIdle": idle["why"]})); eprintln!("[fusion-bridge] auto-update: {button:?} on screen but Fusion is NOT idle ({}) - DEFERRED, will retry when idle", idle["why"].as_str().unwrap_or("")); deferred = Some(json!({"applied": false, "deferred": true, "button": button, "why": format!("exit-class update button deferred: {}", idle["why"].as_str().unwrap_or(""))})); } }
            match deferred { Some(d) => d, None => {
                let ok = Loopback::from_env().and_then(|ab| ab.call("desktop_ui_click", json!({"hwnd": offer["hwnd"], "name": button}), &ctx.caller, 30).ok()).map(|r| r["invoked"].as_bool().unwrap_or(false) || r["success"].as_bool().unwrap_or(false)).unwrap_or(false);
                if ok { eprintln!("[fusion-bridge] auto-update: clicked {button:?} in the background"); }
                crate::fusion_prefs::action_ledger(&ctx.state_dir, "auto_update_click", json!({"button": button, "invoked": ok, "exitClass": exit_class}));
                json!({"applied": ok, "button": button, "why": if ok { format!("clicked {button:?} in the background; Fusion downloads now and swaps on next launch") } else { "found the button but the UIA invoke did not take".to_string() }})
            } }
        }
    };
    let mut out = json!({"success": true, "offered": offered_before || r["button"].is_string(), "autoUpdateEnabled": enabled});
    if let (Some(o), Some(ro)) = (out.as_object_mut(), r.as_object()) { for (k, v) in ro { o.insert(k.clone(), v.clone()); } }
    out
}

pub fn installer_running(ctx: &Ctx) -> bool {
    let Some(ab) = Loopback::from_env() else { return false };
    for name in ["streamer", "FusionDL", "Fusion Client Downloader", "FusionClientDownloader"] {
        if let Ok(r) = ab.call("process_list", json!({"name": name}), &ctx.caller, 10) { if r["processes"].as_array().map(|a| !a.is_empty()).unwrap_or(false) { return true; } }
    }
    false
}

/// `fusion_install_fusion`: the official streamer, silently; per-user when not elevated.
pub fn install_fusion(ctx: &Ctx) -> Value {
    // a copy on disk isn't enough: a launcher pointing at a stripped folder needs the same installer, which repairs it
    if ctx.fusion_exe.as_deref().is_some_and(|e| fusion_core::install_check::launch_problem(e).is_none()) { return json!({"success": true, "alreadyInstalled": true, "_hint": "Fusion 360 is already installed - call fusion_start to launch it."}); }
    if installer_running(ctx) { return json!({"success": true, "installing": true, "statusVerb": "fusion_readiness", "_hint": "An install is ALREADY streaming (10-30 min). Poll fusion_readiness until installed:true - do not start a second installer."}); }
    let stub = std::env::temp_dir().join("FusionClientDownloader.exe");
    let (code, bytes) = fusion_core::aps::http_bytes("https://dl.appstreaming.autodesk.com/production/installers/Fusion%20Client%20Downloader.exe");
    if code != 200 || bytes.is_empty() || std::fs::write(&stub, &bytes).is_err() { return json!({"success": false, "error": format!("Installer download failed: HTTP {code}"), "errorCode": "installer_download_failed", "_hint": "Check the box is online; retry fusion_install_fusion. The stub URL is Autodesk's official appstreaming installer."}); }
    let elevated = ctx.host.is_elevated();
    let mut cmd = std::process::Command::new(&stub);
    cmd.arg("--quiet"); if elevated { cmd.arg("--globalinstall"); }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; cmd.creation_flags(0x0800_0000 | 0x0000_0008); }
    if let Err(e) = cmd.spawn() { return json!({"success": false, "error": format!("Installer launch failed: {e}"), "errorCode": "installer_launch_failed"}); }
    json!({"success": true, "installing": true, "elevated": elevated, "mode": if elevated { "globalinstall" } else { "per-user" }, "statusVerb": "fusion_readiness",
        "_hint": format!("Fusion 360 install STARTED ({}). It streams several GB (10-30 min): poll fusion_readiness until installed:true (it also reports installing:true while the streamer runs), then fusion_start. NOTIFY the user it is underway (progress toast) and again at the Autodesk sign-in.", if elevated { "system-wide" } else { "per-user - no admin needed; a non-elevated --globalinstall dies silently on UAC" })})
}

/// `fusion_uninstall`: the add-in out of Autodesk's dirs, this bridge's prefs and APS tokens.
pub fn uninstall(ctx: &Ctx, addin_dirs: &[std::path::PathBuf], args: &Value) -> Value {
    let dry = args["dryRun"].as_bool().unwrap_or(false);
    let home = std::path::PathBuf::from(std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default());
    let addin_targets: Vec<std::path::PathBuf> = addin_dirs.iter().map(|d| d.join("AdomBridge")).collect();
    let data_targets = vec![home.join(".adom").join("fusion-bridge"), home.join(".adom").join("fusion-aps")];
    let (mut removed, mut failed, mut skipped): (Vec<Value>, Vec<Value>, Vec<Value>) = (vec![], vec![], vec![]);
    for (p, kind) in addin_targets.iter().map(|p| (p, "addin")).chain(data_targets.iter().map(|p| (p, "userdata"))) {
        let ps = p.to_string_lossy().to_string();
        if !p.exists() { skipped.push(json!({"path": ps, "kind": kind, "why": "not present"})); continue; }
        if dry { removed.push(json!({"path": ps, "kind": kind, "dryRun": true})); continue; }
        match std::fs::remove_dir_all(p) { Ok(_) => removed.push(json!({"path": ps, "kind": kind})), Err(e) => failed.push(json!({"path": ps, "kind": kind, "error": e.to_string()})) }
    }
    let running = !ctx.host.fusion_processes().is_empty();
    let mut out = json!({"success": failed.is_empty(), "dryRun": dry, "cascade": args["cascade"].as_bool().unwrap_or(false), "fusionRunning": running, "removed": removed, "failed": failed, "skipped": skipped});
    let nf = out["failed"].as_array().map(|a| a.len()).unwrap_or(0);
    out["_hint"] = json!(if nf > 0 && running { format!("Fusion is RUNNING and holds the add-in files open, so {nf} path(s) could not be removed. Close Fusion (fusion_stop) and re-run fusion_uninstall, or delete those paths by hand - otherwise the stale add-in keeps loading inside Fusion on every launch and binding its port.") }
        else if nf > 0 { format!("{nf} path(s) could not be removed (permissions?). They are listed in `failed`; delete them by hand so no stale add-in is left inside Fusion.") }
        else if dry { "DRY RUN - nothing was touched. These are the paths a real uninstall removes: the Fusion add-in in Autodesk's AddIns dirs, plus this bridge's prefs and APS OAuth tokens under ~/.adom.".to_string() }
        else { "Cleaned up: the Fusion add-in is gone from Autodesk's directories, and this bridge's prefs + APS OAuth tokens are removed. Fusion itself is untouched. Re-installing the bridge redeploys the add-in on its next start.".to_string() });
    out
}

const PREF_SECTIONS: &[(&str, Option<&str>, f64, f64)] = &[("general", None, 0.071, 0.092), ("api", Some("general"), 0.071, 0.135), ("design", Some("general"), 0.071, 0.179), ("manufacture", Some("general"), 0.071, 0.222), ("electronics", Some("general"), 0.071, 0.265), ("render", Some("general"), 0.071, 0.310), ("drawing", Some("general"), 0.071, 0.353), ("material", None, 0.071, 0.483), ("graphics", None, 0.071, 0.527), ("network", None, 0.071, 0.570), ("preview features", None, 0.071, 0.744)];

fn prefs_dialog_hwnd(ab: &dyn AbClient, caller: &fusion_core::ab::Caller) -> Option<i64> {
    let v = ab.call("desktop_list_windows", json!({}), caller, 40).ok()?;
    let rows = v["data"]["windows"].as_array().or_else(|| v["windows"].as_array()).cloned().unwrap_or_default();
    rows.iter().find(|w| w["title"].as_str().unwrap_or("").trim() == "Preferences").and_then(|w| w["hwnd"].as_i64())
}
fn prefs_shot(ab: &dyn AbClient, caller: &fusion_core::ab::Caller, hwnd: i64) -> (Value, f64, f64, Value) {
    let s = ab.call("desktop_screenshot_window", json!({"hwnd": hwnd}), caller, 45).unwrap_or(json!({}));
    let cm = &s["coordMap"]; let img = &cm["image"];
    (cm["shotId"].clone(), img["w"].as_f64().unwrap_or(1400.0), img["h"].as_f64().unwrap_or(836.0), if s["localSafePath"].is_null() { s["safePath"].clone() } else { s["localSafePath"].clone() })
}
fn image_click(ab: &dyn AbClient, caller: &fusion_core::ab::Caller, shot_id: &Value, hwnd: i64, x: i64, y: i64) { let _ = ab.call("desktop_click", json!({"space": "image", "shotId": shot_id, "hwnd": hwnd, "x": x, "y": y}), caller, 30); }

/// `fusion_prefs_open`: Fusion Preferences, optionally at a section, with a screenshot to click in.
pub fn prefs_open(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    if ctx.host.fusion_processes().is_empty() { return json!({"success": false, "error": "Fusion is not running.", "_hint": "fusion_start first."}); }
    let Some(ab) = Loopback::from_env() else { return json!({"success": false, "error": "ab loopback unavailable"}) };
    let section = args["section"].as_str().unwrap_or("").trim().to_lowercase();
    let mut steps: Vec<String> = Vec::new();
    let mut hwnd = prefs_dialog_hwnd(&ab, &ctx.caller);
    if hwnd.is_none() {
        let _ = ab.call("desktop_caption", json!({"text": "Adom: opening Fusion preferences", "id": "fusion-bridge-foreground", "expiresInMs": 2500}), &ctx.caller, 15);
        let _ = pass("execute_text_command", json!({"command": "Commands.Start PreferencesCommand"}));
        std::thread::sleep(Duration::from_secs(3));
        hwnd = prefs_dialog_hwnd(&ab, &ctx.caller);
        steps.push("opened Preferences".into());
    }
    let Some(hwnd) = hwnd else { return json!({"success": false, "error": "Preferences dialog did not open.", "steps": steps}) };
    let known: Vec<&str> = { let mut k: Vec<&str> = PREF_SECTIONS.iter().map(|s| s.0).collect(); k.sort(); k };
    if !section.is_empty() {
        let Some(spec) = PREF_SECTIONS.iter().find(|s| s.0 == section) else { return json!({"success": false, "error": format!("Unknown section {}.", fusion_lbr::py_repr_str(&section)), "knownSections": known, "_hint": "Pass one of knownSections, or omit `section` and click the returned shotId yourself with desktop_click {space:'image'}."}) };
        let (mut sid, mut w, mut h, _) = prefs_shot(&ab, &ctx.caller, hwnd);
        if let Some(parent) = spec.1 {
            let ps = PREF_SECTIONS.iter().find(|s| s.0 == parent).unwrap();
            image_click(&ab, &ctx.caller, &sid, hwnd, (w * 0.029) as i64, (h * ps.3) as i64);
            steps.push(format!("expanded {parent}")); std::thread::sleep(Duration::from_secs(2));
            let t = prefs_shot(&ab, &ctx.caller, hwnd); sid = t.0; w = t.1; h = t.2;
        }
        image_click(&ab, &ctx.caller, &sid, hwnd, (w * spec.2) as i64, (h * spec.3) as i64);
        steps.push(format!("selected {section}")); std::thread::sleep(Duration::from_secs(2));
    }
    let (sid, w, h, path) = prefs_shot(&ab, &ctx.caller, hwnd);
    json!({"success": true, "hwnd": hwnd, "section": if section.is_empty() { "general".to_string() } else { section }, "shotId": sid, "imageWidth": w, "imageHeight": h, "screenshot": path, "steps": steps, "knownSections": known,
        "_hint": "Preferences is OPEN and showing this section. LOOK at the screenshot, then click any control with desktop_click {space:'image', shotId, x, y, hwnd}. ⛔ Nothing is saved until you call fusion_prefs_close {save:true} (Apply+OK) - killing or restarting Fusion first DISCARDS the change. Child sections under General are lazily rendered and absent from the UIA tree, which is why this verb hands you an image to click rather than control names.", "statusVerb": "fusion_get_preferences"})
}

/// `fusion_prefs_close`: Apply+OK (save:true, default) or Cancel.
pub fn prefs_close(ctx: &Ctx, args: &Value) -> Value {
    let save = args["save"].as_bool().unwrap_or(true);
    let Some(ab) = Loopback::from_env() else { return json!({"success": false, "error": "ab loopback unavailable"}) };
    let Some(hwnd) = prefs_dialog_hwnd(&ab, &ctx.caller) else { return json!({"success": true, "closed": false, "_hint": "Preferences was not open; nothing to do."}) };
    let (sid, w, h, _) = prefs_shot(&ab, &ctx.caller, hwnd);
    if save { image_click(&ab, &ctx.caller, &sid, hwnd, (w * 0.839) as i64, (h * 0.948) as i64); std::thread::sleep(Duration::from_secs(2)); image_click(&ab, &ctx.caller, &sid, hwnd, (w * 0.894) as i64, (h * 0.948) as i64); }
    else { image_click(&ab, &ctx.caller, &sid, hwnd, (w * 0.951) as i64, (h * 0.948) as i64); }
    std::thread::sleep(Duration::from_secs(2));
    json!({"success": true, "closed": prefs_dialog_hwnd(&ab, &ctx.caller).is_none(), "saved": save, "narrate": if save { "Saved your Fusion preferences." } else { "Closed preferences without saving." }, "_hint": if save { "Applied and closed. Some settings (the MCP server is one) only take effect once saved this way." } else { "Cancelled; nothing was changed." }, "statusVerb": "fusion_get_preferences"})
}

/// `fusion_tool_update {action}`: the standard's tool_update. check reads the update Fusion offers
/// (nothing is clicked), apply takes it the way the readiness gate does (in the background, deferred
/// while Fusion is busy), auto reads or sets automatic updating.
pub fn tool_update(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let auto = crate::small_verbs::get_pref(&ctx.state_dir, "autoUpdateFusion").map(|v| v != json!(false)).unwrap_or(true);
    let v = pass("run_modeling_script", json!({"script": "result = {'version': app.version}"}));
    let current = v["data"]["result"]["version"].clone();
    match args["action"].as_str().unwrap_or("check") {
        "check" => {
            let offer = find_update_offer_within(ctx, Some(Duration::from_secs(25)));
            let offered = offer["offered"].as_bool() == Some(true);
            json!({"success": true, "mutated": false, "action": "check", "current": current, "offered": if offered { offer["button"].clone() } else { Value::Null }, "complete": offer["complete"].as_bool().unwrap_or(true),
                   "updateOffered": offered, "auto": auto, "applied": false, "needsRestart": false,
                   "_hint": if offered { "Fusion offers an update: tool_update {action:'apply'} takes it in the background (deferred while Fusion is busy)." } else { "No update offered: Fusion is current, or Autodesk has not offered one in this session yet." }})
        }
        "apply" => {
            let mut r = apply_fusion_update(ctx, pass);
            let applied = r["applied"].as_bool() == Some(true);
            r["action"] = json!("apply"); r["current"] = current; r["auto"] = json!(auto); r["needsRestart"] = json!(applied);
            r["mutated"] = json!(applied);
            r
        }
        "auto" => {
            let a = if args["on"].is_boolean() { json!({"enabled": args["on"]}) } else { json!({}) };
            let mut r = crate::small_verbs::set_auto_update(&ctx.state_dir, &a);
            r["action"] = json!("auto"); r["auto"] = r["autoUpdateFusion"].clone(); r["current"] = current;
            r
        }
        other => json!({"success": false, "errorCode": "bad_args", "error": format!("action is check, apply or auto, not {other}")}),
    }
}

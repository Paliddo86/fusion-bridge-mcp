//! State-only verbs the exe answers natively: the sticky preference toggles, cancel_run,
//! addin_status and plan_library_import. Each mirrors its `_handle_*` in server.py to the
//! word (hints included), so a caller cannot tell which shell answered except by `shell`.

use serde_json::{json, Value};
use std::path::Path;

fn prefs_path(state_dir: &Path) -> std::path::PathBuf { state_dir.join("prefs.json") }

pub fn get_pref(state_dir: &Path, key: &str) -> Option<Value> {
    let t = std::fs::read_to_string(prefs_path(state_dir)).ok()?;
    serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).ok()?.get(key).cloned()
}

fn pref_bool(state_dir: &Path, key: &str, default: bool) -> bool {
    match get_pref(state_dir, key) { Some(Value::Bool(b)) => b, Some(Value::Null) | None => default, Some(v) => !(v == json!(0) || v == json!("")) }
}

/// `_set_bridge_pref`: read-modify-write of prefs.json (indent 2, BOM-free, atomic).
pub fn set_pref(state_dir: &Path, key: &str, value: Value) -> bool {
    let p = prefs_path(state_dir);
    let mut cur = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).ok()).unwrap_or(json!({}));
    if !cur.is_object() { cur = json!({}); }
    cur[key] = value;
    let _ = std::fs::create_dir_all(state_dir);
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&cur).unwrap_or_default()).is_ok() && std::fs::rename(&tmp, &p).is_ok()
}

fn on_off(b: bool) -> &'static str { if b { "ON" } else { "OFF" } }

pub fn set_auto_update(state_dir: &Path, args: &Value) -> Value {
    if args.get("enabled").is_none() {
        let cur = pref_bool(state_dir, "autoUpdateFusion", true);
        return json!({"success": true, "autoUpdateFusion": cur,
            "_hint": format!("Fusion auto-update is currently {}. It is ON BY DEFAULT: the bridge clicks Fusion's 'Update Now' for the user in the BACKGROUND so they always run the latest build and never see the nag. To opt out: fusion_set_auto_update {{\"enabled\": false}} - that choice is STICKY.", on_off(cur)),
            "statusVerb": "fusion_readiness"});
    }
    let enabled = truthy(&args["enabled"]);
    set_pref(state_dir, "autoUpdateFusion", json!(enabled));
    json!({"success": true, "autoUpdateFusion": enabled,
        "narrate": if enabled { "I'll keep Fusion updated automatically in the background." } else { "I'll stop auto-updating Fusion. You'll see Autodesk's own update prompts." },
        "_hint": format!("Saved and sticky (~/.adom/fusion-bridge/prefs.json). {}", if enabled {
            "Auto-update ON: the bridge silently clicks 'Update Now' whenever Fusion offers one, so the user always runs the newest build. Tell them it is handled and they never need to click it."
        } else { "Auto-update OFF: the bridge will NOT touch update prompts; the user handles Autodesk's nag themselves. Do not override this." }),
        "statusVerb": "fusion_readiness"})
}

pub fn set_seat_takeover(state_dir: &Path, args: &Value) -> Value {
    if args.get("enabled").is_none() {
        let cur = pref_bool(state_dir, "autoTakeOverSeat", true);
        return json!({"success": true, "autoTakeOverSeat": cur,
            "_hint": format!("Seat takeover is currently {}. ON BY DEFAULT: when Autodesk pops the 'Active Sessions Exceeded' dialogs mid-session, the bridge clicks Continue through BOTH in the background - this machine takes the seat, the other machine's Fusion suspends RESUMABLY, one toast tells the user. Opt out: fusion_set_seat_takeover {{\"enabled\": false}} - STICKY.", on_off(cur)),
            "statusVerb": "fusion_readiness"});
    }
    let enabled = truthy(&args["enabled"]);
    set_pref(state_dir, "autoTakeOverSeat", json!(enabled));
    json!({"success": true, "autoTakeOverSeat": enabled,
        "narrate": if enabled { "I'll automatically take the Fusion seat on this machine whenever Autodesk flags the session limit." } else { "I'll leave Autodesk seat-conflict dialogs for you to resolve yourself." },
        "_hint": format!("Sticky (~/.adom/fusion-bridge/prefs.json). {}", if enabled { "Takeover ON: mid-session seat dialogs are clicked through silently." } else { "Takeover OFF: the sweep will NOT touch seat dialogs; fusion_start still grabs the seat when the user asks for Fusion." }),
        "statusVerb": "fusion_readiness"})
}

pub fn set_window_sizing(state_dir: &Path, args: &Value) -> Value {
    if args.get("enabled").is_none() {
        let cur = pref_bool(state_dir, "sizeFusionWindow", true);
        return json!({"success": true, "sizeFusionWindow": cur,
            "_hint": format!("Fusion window auto-sizing is {}. ON BY DEFAULT: after a launch the bridge grows a small Fusion window to ~92% of the monitor work area, centered, WITHOUT activating it (SetWindowPos NOACTIVATE, never ShowWindow/maximize, so your focus is never stolen). A window you maximized or already sized large is left alone. Opt out: fusion_set_window_sizing {{\"enabled\": false}} - STICKY.", on_off(cur)),
            "statusVerb": "fusion_state"});
    }
    let enabled = truthy(&args["enabled"]);
    set_pref(state_dir, "sizeFusionWindow", json!(enabled));
    json!({"success": true, "sizeFusionWindow": enabled,
        "narrate": if enabled { "I'll size Fusion's window to fill most of your screen when it launches." } else { "I'll leave Fusion's window size exactly as Fusion opens it." },
        "statusVerb": "fusion_state"})
}

/// The read path and the pref write are native; turning the badge OFF also clears the
/// live overlay, which still lives in the sidecar's taskbar handler (`clear` is the proxy).
pub fn set_driving_badge(state_dir: &Path, args: &Value, clear: &dyn Fn()) -> Value {
    if args.get("enabled").is_none() {
        let cur = pref_bool(state_dir, "showDrivingBadge", true);
        return json!({"success": true, "showDrivingBadge": cur,
            "_hint": format!("The driving badge is currently {}. ON BY DEFAULT: while Adom drives Fusion, its taskbar button wears a small Adom mark so the user knows at a glance who is in control; it sheds after 5 minutes idle. Opt out: fusion_set_driving_badge {{\"enabled\": false}} - STICKY.", on_off(cur)),
            "statusVerb": "fusion_state"});
    }
    let enabled = truthy(&args["enabled"]);
    set_pref(state_dir, "showDrivingBadge", json!(enabled));
    if !enabled { clear(); }
    json!({"success": true, "showDrivingBadge": enabled,
        "narrate": if enabled { "I'll badge Fusion's taskbar button while I'm driving it." } else { "I'll stop badging Fusion's taskbar button." },
        "_hint": "Sticky (~/.adom/fusion-bridge/prefs.json). Cleared immediately when turned off.",
        "statusVerb": "fusion_state"})
}

fn truthy(v: &Value) -> bool {
    match v { Value::Bool(b) => *b, Value::Null => false, Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0, Value::String(s) => !s.is_empty(), Value::Array(a) => !a.is_empty(), Value::Object(o) => !o.is_empty() }
}

/// `fusion_cancel_run`: cooperative stop at the next part boundary; shared run store.
pub fn cancel_run(args: &Value, banner: &dyn Fn(Value)) -> Value {
    let terminal = ["done", "failed", "cancelled"];
    let mut runs = crate::chains::read_runs();
    let mut rid = args["runId"].as_str().unwrap_or("").trim().to_string();
    if rid.is_empty() {
        let mut live: Vec<&Value> = runs.values().filter(|r| !terminal.contains(&r["stage"].as_str().unwrap_or(""))).collect();
        live.sort_by(|a, b| a["startedAt"].as_f64().unwrap_or(0.0).partial_cmp(&b["startedAt"].as_f64().unwrap_or(0.0)).unwrap());
        if let Some(r) = live.last() { rid = r["runId"].as_str().unwrap_or("").to_string(); }
    }
    if rid.is_empty() {
        return json!({"success": true, "cancelRequested": false, "reason": "no_run_in_flight", "note": "No import is running on this box, so there is nothing to cancel.", "_hint": "fusion_prove_result {} shows the latest run and its stage."});
    }
    let Some(mut st) = runs.get(&rid).cloned() else {
        return json!({"success": false, "errorCode": "unknown_run_id", "error": format!("no run with runId {} on this box", fusion_lbr::py_repr_str(&rid))});
    };
    let stage = st["stage"].clone();
    if terminal.contains(&stage.as_str().unwrap_or("")) {
        return json!({"success": true, "runId": rid, "alreadyFinished": true, "stage": stage, "_hint": format!("That run had already ended ({}); nothing to cancel.", stage.as_str().unwrap_or(""))});
    }
    let by: String = args["caller"]["aiThread"].as_str().or(args["by"].as_str()).unwrap_or("user").chars().take(60).collect();
    st["cancelRequested"] = json!(true);
    st["cancelledBy"] = json!(by);
    st["stageLabel"] = json!(format!("stopping at the next part boundary ({})", st["stageLabel"].as_str().or(st["stage"].as_str()).unwrap_or("")));
    runs.insert(rid.clone(), st.clone());
    let p = std::env::temp_dir().join("fusion-prove-runs.json");
    let _ = std::fs::write(&p, serde_json::to_string(&runs).unwrap_or_default());
    let lp = std::env::temp_dir().join("fusion-prove-last.json");
    if let Some(mut last) = std::fs::read_to_string(&lp).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        if last["runId"].as_str() == Some(&rid) { last["cancelRequested"] = json!(true); let _ = std::fs::write(&lp, last.to_string()); }
    }
    banner(json!({"action": "update", "title": "Stopping the import", "detail": "Finishing the current part, then stopping", "percent": Value::Null, "etaSec": Value::Null, "tone": "working"}));
    json!({"success": true, "runId": rid, "cancelRequested": true, "stage": stage, "statusVerb": "fusion_prove_result",
        "_hint": "The run stops at its next part boundary (a STEP import in progress finishes first, up to ~60s). Poll fusion_prove_result {runId}: stage becomes 'cancelled' with cancelledAt and what was completed. Parts already bound keep their 3D; the library stays open in Fusion, unsaved to the cloud."})
}

/// The bundled add-in's manifest version (beside the exe, `addin/AdomBridge/AdomBridge.manifest`).
pub fn expected_addin_version() -> String {
    let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_default();
    std::fs::read_to_string(dir.join("addin").join("AdomBridge").join("AdomBridge.manifest")).ok()
        .and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).ok())
        .and_then(|v| v["version"].as_str().map(str::to_string)).unwrap_or_else(|| "unknown".into())
}

/// `_addin_staleness`.
pub fn addin_staleness(reported: Option<&str>) -> Value {
    let expected = expected_addin_version();
    let reported_s = reported.unwrap_or("unknown");
    let stale = reported.map(|r| r != "unknown").unwrap_or(false) && expected != "unknown" && reported_s != expected;
    let mut v = json!({"addinVersion": reported_s, "expectedAddinVersion": expected, "addinStale": stale});
    if stale {
        v["_staleHint"] = json!(format!("STALE add-in: Fusion is running add-in v{reported_s} but this bridge bundles v{expected}. Newer verbs (e.g. fusion_aps_open/open_by_urn) can fail silently. Fix: fusion_stop then fusion_start - the bridge re-syncs the add-in from its cache on start (only possible with Fusion CLOSED)."));
    }
    v
}

/// `_addin_start_failure`: the add-in's own start-failure marker.
pub fn addin_start_failure() -> Option<Value> {
    let base = std::env::var("TEMP").ok().filter(|s| !s.is_empty()).map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let fp = base.join("adom-fusion").join("addin-start-failed.json");
    let t = std::fs::read_to_string(&fp).ok()?;
    let mut m: Value = serde_json::from_str(&t).ok()?;
    if !m.is_object() { return None; }
    m["markerPath"] = json!(fp.to_string_lossy().replace('\\', "/"));
    m["_hint"] = json!(format!("Fusion loaded a BROKEN copy of the add-in from {}. The bridge re-syncs every AddIns dir on start; restart Fusion with fusion_restart_for_addin {{save:true}} so it loads the repaired copy. If it keeps failing, delete that folder and restart Fusion: the bridge reinstalls it.", m["addinDir"].as_str().unwrap_or("None")));
    Some(m)
}

/// `fusion_addin_status`: GET /status on the add-in, staleness, the start-failure marker.
pub fn addin_status(fusion_running: bool) -> Value {
    let status: Option<Value> = ureq::get("http://127.0.0.1:8774/status").timeout(std::time::Duration::from_secs(3)).call().ok().and_then(|r| r.into_json().ok());
    let Some(mut status) = status else {
        return json!({"success": false, "status": "unreachable", "errorCode": "addin_not_answering",
            "error": format!("The Adom add-in did not answer on port 8774. Fusion is {}running.", if fusion_running { "" } else { "NOT " }),
            "_hint": "If Fusion is running, the add-in was installed or updated after launch (or its server stopped): fusion_restart_for_addin {save:true} asks and restarts. If Fusion is not running, fusion_start."});
    };
    let stale = addin_staleness(status["version"].as_str());
    if let (Some(so), Some(st)) = (status.as_object_mut(), stale.as_object()) { for (k, v) in st { if k != "_staleHint" { so.insert(k.clone(), v.clone()); } } }
    let mut result = json!({"success": true, "output": serde_json::to_string(&status).unwrap_or_default()});
    if let (Some(ro), Some(so)) = (result.as_object_mut(), status.as_object()) { for (k, v) in so { ro.insert(k.clone(), v.clone()); } }
    if stale["addinStale"].as_bool() == Some(true) { result["_hint"] = stale["_staleHint"].clone(); }
    if let Some(asf) = addin_start_failure() { result["addinStartFailed"] = asf; }
    result
}

fn human_secs(v: f64) -> String {
    let v = v.round() as i64;
    if v < 60 { return format!("{v}s"); }
    let (m, s) = (v / 60, v % 60);
    if s != 0 { format!("{m}m {s:02}s") } else { format!("{m}m") }
}

/// `fusion_plan_library_import`: the pre-flight summary, no work done.
pub fn plan_library_import(state_dir: &Path, installed: bool, running: bool, addin_ok: bool, args: &Value) -> Value {
    let parts = args["parts"].as_array().cloned().unwrap_or_default();
    let n_parts = parts.len().max(1);
    let has_steps = parts.iter().any(|p| p["modelPath"].as_str().map(|s| !s.is_empty()).unwrap_or(false)) || args["stepPath"].as_str().map(|s| !s.is_empty()).unwrap_or(false);
    let inst = installed || running;
    let (status, note) = if !inst { ("not_installed", "Fusion does not look installed on this machine. I can install it for you - it is a silent install plus an Autodesk sign-in.") }
        else if !running { ("installed_not_running", "Fusion is installed but not running. I can launch it, which usually takes about a minute, and then carry on.") }
        else if !addin_ok { ("addin_restart_required", "Fusion is running but the Adom add-in is not loaded - it needs a quick restart to finish setup. I can save your documents and do it.") }
        else { ("ready", "Fusion is running and ready - I can start straight away.") };
    let est = |k: &str, f: f64| { let (v, c) = crate::chains::estimate(state_dir, k, f); ((v * 10.0).round() / 10.0, c) };
    let mut steps: Vec<Value> = Vec::new();
    if !inst { let (v, c) = est("stage:install_fusion", 1200.0); steps.push(json!({"stage": "install", "label": "install Fusion (streams the app)", "estimatedSec": v, "confidence": c, "needsUserConsent": true, "note": "10-30 minutes depending on connection, plus an Autodesk sign-in"})); }
    if inst && !running { let (v, c) = est("stage:launch", 60.0); steps.push(json!({"stage": "launch", "label": "open Fusion", "estimatedSec": v, "confidence": c})); }
    if status == "addin_restart_required" { let (v, c) = est("stage:launch", 60.0); steps.push(json!({"stage": "restart", "label": "restart Fusion to load the add-in", "estimatedSec": v, "confidence": c, "needsUserConsent": true})); }
    if has_steps { let (per, c) = est("stage:bind_per_part", 60.0); steps.push(json!({"stage": "bind", "label": format!("attach 3D to {} part{}", n_parts, if n_parts == 1 { "" } else { "s" }), "estimatedSec": ((per * n_parts as f64) * 10.0).round() / 10.0, "confidence": c, "parts": n_parts, "perPartSec": per, "note": "one STEP import per part - the slowest stage"})); }
    { let (v, c) = est("stage:prove", 300.0); steps.push(json!({"stage": "prove", "label": "capture 4 evidence views", "estimatedSec": v, "confidence": c})); }
    let total = (steps.iter().map(|s| s["estimatedSec"].as_f64().unwrap_or(0.0)).sum::<f64>() * 10.0).round() / 10.0;
    let ready_total = (steps.iter().filter(|s| !matches!(s["stage"].as_str(), Some("launch") | Some("restart"))).map(|s| s["estimatedSec"].as_f64().unwrap_or(0.0)).sum::<f64>() * 10.0).round() / 10.0;
    let conf = if steps.iter().all(|s| s["confidence"].as_str() == Some("measured")) { "measured" } else { "typical" };
    let tail = if ready_total != total { format!("Had Fusion already been open, about {}.", human_secs(ready_total)) } else { String::new() };
    json!({"success": true,
        "fusion": {"installed": inst, "running": running, "addinReady": addin_ok, "status": status, "message": note},
        "plan": steps, "partsTotal": n_parts, "estimatedSec": total, "estimatedSecIfAlreadyOpen": ready_total, "confidence": conf,
        "summary": format!("{} This will take about {}. {}", note, human_secs(total), tail).trim().to_string(),
        "_hint": "Pre-flight only - NOTHING was launched, imported or changed. Render `summary` plus `plan[]` as the progress bar's steps, then call fusion_create_library (or open_lbr + build_library_3d) and poll fusion_prove_result, which reports the SAME stage names so one renderer draws both. Estimates marked confidence:'measured' come from this machine's own recorded runs; 'typical' means seeded and not yet observed here."})
}

/// The host app's CLI (`adom-bridge-cli.exe` beside the app, else `adom-bridge` on PATH). The
/// Rust exe knows only the current name: the naming guard keeps retired names out of rust/.
fn find_ab_cli() -> Option<std::path::PathBuf> {
    let la = std::env::var("LOCALAPPDATA").unwrap_or_default();
    if !la.is_empty() {
        let p = std::path::Path::new(&la).join("Adom Bridge").join("adom-bridge-cli.exe");
        if p.is_file() { return Some(p); }
    }
    for name in ["adom-bridge-cli", "adom-bridge"] {
        if let Ok(path) = std::env::var("PATH") { for d in std::env::split_paths(&path) { for ext in ["", ".exe"] { let p = d.join(format!("{name}{ext}")); if p.is_file() { return Some(p); } } } }
    }
    None
}

/// `_cli_notify_all`: a sticky toast through the CLI when the loopback API is down.
fn cli_notify_all(state_dir: &Path, title: &str, body: &str, level: &str) -> Value {
    let Some(exe) = find_ab_cli() else { return json!({"delivered": false, "error": "adom-bridge CLI not found"}) };
    let payload = json!({"title": title, "body": body, "level": level, "scenario": "reminder", "durationLong": true, "buttons": [{"label": "Got it"}]}).to_string();
    let mut cmd = std::process::Command::new(&exe);
    if pref_bool(state_dir, "notifyBroadcast", false) { cmd.args(["--target", "all"]); }
    cmd.arg("notify_user").arg(&payload);
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; cmd.creation_flags(0x0800_0000); }
    match cmd.output() {
        Ok(o) => {
            let out = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            let delivered = o.status.success() || out.contains("displayed") || out.contains("\"status\": \"ok\"") || out.contains("status':'ok");
            let nid = regex::Regex::new(r#""id"\s*:\s*"([0-9a-f]{8,})""#).ok().and_then(|re| re.captures(&out).map(|c| c[1].to_string()));
            json!({"delivered": delivered, "targets": "all", "id": nid, "error": if delivered { Value::Null } else { json!(out.chars().take(200).collect::<String>()) }})
        }
        Err(e) => json!({"delivered": false, "error": e.to_string()}),
    }
}

/// `fusion_notify_owner`: the last-resort toast to the human, wherever they are.
pub fn notify_owner(state_dir: &Path, args: &Value, caller: &fusion_core::ab::Caller) -> Value {
    let title = args["title"].as_str().filter(|s| !s.is_empty()).unwrap_or("Fusion bridge needs you").to_string();
    let body = args["body"].as_str().or(args["message"].as_str()).filter(|s| !s.is_empty()).unwrap_or("A human step is required to continue.").to_string();
    let level = args["level"].as_str().filter(|s| !s.is_empty()).unwrap_or("warning").to_string();
    let via_api = fusion_core::ab::Loopback::from_env().map(|ab| { use fusion_core::ab::AbClient; ab.call("notify_user", json!({"title": title, "body": body, "level": level, "target": "all"}), caller, 20) });
    match via_api {
        Some(Ok(res)) => {
            let summary_ok = res["summary"]["ok"].as_f64().unwrap_or(0.0) > 0.0;
            let ok = res["success"].as_bool() == Some(true) || res["status"].as_str() == Some("ok") || summary_ok;
            let reached = if res["targets"].is_null() { if ok { json!(["self"]) } else { json!([]) } } else { res["targets"].clone() };
            json!({"success": ok, "via": "ab_client:direct(all)", "targets": reached,
                "_hint": "Toast fanned out to ALL connected desktops (the user's machine included) via the in-process ab direct API. This is the LAST RESORT - only use after exhausting programmatic options (UIA background clicks, seat auto-resolve, warm-SSO sign-in), and only when the box is ACTUALLY ready for the user to act. Now WAIT and poll fusion_readiness for the state to clear; do not re-toast within a few minutes."})
        }
        _ => {
            let cli = cli_notify_all(state_dir, &title, &body, &level);
            if cli["delivered"].as_bool() == Some(true) {
                return json!({"success": true, "via": "cli:--target all", "targets": cli["targets"],
                    "_hint": "Toast fanned out to ALL connected desktops (the user's main machine included) via the adom-bridge CLI, because the bridge's in-process ab API was unavailable (this VM). WAIT and poll fusion_readiness; do not re-toast within a few minutes. Only notify when the box is ACTUALLY ready for the user to act - do not toast for a step you can do yourself."});
            }
            json!({"success": false,
                "error": format!("ab in-process API unavailable AND the adom-bridge CLI fallback failed: {}", cli["error"].as_str().unwrap_or("None").chars().take(200).collect::<String>()),
                "notifyUser": {"title": title, "body": body, "level": level},
                "_hint": "Relay the notifyUser payload yourself: `adom-bridge --target all notify_user {title, body, level}` (or `--target <the user's host>`)."})
        }
    }
}

/// `fusion_close_window`: WM_CLOSE to a dialog, through ab's graceful close (never a kill).
pub fn close_window(args: &Value, caller: &fusion_core::ab::Caller) -> Value {
    let Some(hwnd) = args["hwnd"].as_i64().or_else(|| args["hwnd"].as_str().and_then(|s| s.trim().parse().ok())).filter(|h| *h != 0) else {
        return json!({"success": false, "error": "Missing required arg: hwnd"});
    };
    let Some(ab) = fusion_core::ab::Loopback::from_env() else {
        return json!({"success": false, "error": format!("HWND {hwnd} could not be closed: the ab loopback API is not available to this bridge."), "_hint": "The exe was not spawned by ab (ADOM_DIRECT_API_URL unset). Let ab spawn it, or use desktop_close_window {hwnd} directly."});
    };
    use fusion_core::ab::AbClient;
    match ab.call("desktop_close_window", json!({"hwnd": hwnd}), caller, 15) {
        Ok(r) => {
            if r["resolvedBy"].is_null() && r["ok"].as_bool() != Some(true) && r["success"].as_bool() != Some(true) {
                return json!({"success": false, "error": format!("HWND {hwnd} is not a valid window.")});
            }
            json!({"success": true, "hwnd": hwnd, "action": "WM_CLOSE sent", "output": format!("Sent WM_CLOSE to hwnd {hwnd}."), "closed": r["closed"]})
        }
        Err(fusion_core::ab::AbError::Refused { message, .. }) if message.to_lowercase().contains("not found") || message.to_lowercase().contains("no window") || message.to_lowercase().contains("invalid") => json!({"success": false, "error": format!("HWND {hwnd} is not a valid window.")}),
        Err(e) => json!({"success": false, "error": format!("HWND {hwnd} could not be closed: {e:?}")}),
    }
}

/// `fusion_screenshot_fusion`: the main window (or a dialog by hwnd), background PrintWindow via ab.
pub fn screenshot_fusion(args: &Value, caller: &fusion_core::ab::Caller) -> Value {
    let Some(ab) = fusion_core::ab::Loopback::from_env() else {
        return json!({"success": false, "error": "Screenshot unavailable: the ab loopback API is not available to this bridge.", "_hint": "The exe was not spawned by ab (ADOM_DIRECT_API_URL unset). Let ab spawn it, or use desktop_screenshot_window directly."});
    };
    use fusion_core::ab::AbClient;
    let asked = args["hwnd"].as_i64().or(args["dialogHwnd"].as_i64()).or_else(|| args["hwnd"].as_str().and_then(|s| s.trim().parse().ok())).filter(|h| *h != 0);
    let hwnd = match asked {
        Some(h) => h,
        None => {
            let facts = fusion_core::windows_facts::facts_from_ab(&ab, caller);
            match facts.main_hwnd {
                Some(h) => h,
                None => return json!({"success": false, "error": "Fusion 360 window not found (no window with 'Autodesk Fusion' in title).", "_hint": "Call fusion_start to start Fusion 360 and wait for the add-in to be ready, then retry."}),
            }
        }
    };
    match ab.call("desktop_screenshot_window", json!({"hwnd": hwnd}), caller, 40) {
        Ok(r) => {
            let saved = r["fullPath"].as_str().or(r["safePath"].as_str()).map(|s| s.replace('\\', "/"));
            let Some(saved) = saved else { return json!({"success": false, "error": "ab captured nothing for that window (no fullPath in the answer)", "hwnd": hwnd}) };
            let bytes = r["fullBytesHost"].as_f64().or(r["fullBytes"].as_f64()).unwrap_or(0.0);
            let size_kb = (bytes / 1024.0 * 10.0).round() / 10.0;
            let (w, h) = (r["fullWidth"].clone(), r["fullHeight"].clone());
            json!({"success": true, "savedTo": saved, "sizeKB": size_kb, "hwnd": hwnd,
                "fullPath": saved, "safePath": r["safePath"].as_str().map(|s| s.replace('\\', "/")).unwrap_or_else(|| saved.clone()), "fullPathHost": saved, "safePathHost": saved,
                "fullWidth": w, "fullHeight": h, "dimensions": {"width": w, "height": h}, "ownedPopupCount": r["ownedPopupCount"], "screenshots": r["screenshots"],
                "output": json!({"savedTo": saved, "sizeKB": size_kb, "fullPath": saved, "safePath": saved, "fullPathHost": saved, "safePathHost": saved, "fullWidth": w, "fullHeight": h}).to_string()})
        }
        Err(e) => json!({"success": false, "error": format!("Screenshot failed: {e:?}"), "hwnd": hwnd}),
    }
}

/// UI Automation reads of a window's controls cost ~1-5 s on a busy Fusion, and the classifier,
/// the closable check and the inventory each wanted one for the same window on the same verb.
/// One read per window per 20 s is plenty: a dialog's buttons do not change that fast.
pub fn controls_cached(ab: &dyn fusion_core::ab::AbClient, caller: &fusion_core::ab::Caller, hwnd: i64) -> Vec<Value> {
    use std::collections::HashMap; use std::sync::Mutex; use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<HashMap<i64, (Instant, Vec<Value>)>>> = Mutex::new(None);
    if let Some((t, v)) = CACHE.lock().unwrap().get_or_insert_with(HashMap::new).get(&hwnd) { if t.elapsed() < Duration::from_secs(20) { return v.clone(); } }
    let v: Vec<Value> = ab.call("desktop_find_controls", json!({"hwnd": hwnd}), caller, 10).ok().map(|r| { let d = if r["data"].is_object() { r["data"].clone() } else { r }; d["controls"].as_array().or(d["matches"].as_array()).cloned().unwrap_or_default() }).unwrap_or_default();
    let mut g = CACHE.lock().unwrap(); let m = g.get_or_insert_with(HashMap::new);
    m.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(60));
    m.insert(hwnd, (Instant::now(), v.clone()));
    v
}

/// ab's image-delivery contract: a reply that carries `fullPath` (and `safePath`), or
/// `screenshots[]` entries with `fullPath`, is auto-pulled to the CALLER's machine, so an AI in a
/// container can read the picture with no second call. Older replies named the Windows path
/// `path`, `savedTo`, `screenshot` or `postOpenScreenshot`, which ab does not pull (page issue
/// #98). This lifts every Windows image path in a reply into that shape and keeps the old keys.
pub fn add_image_delivery(r: &mut Value) {
    if !r.is_object() { return; }
    fn is_img(s: &str) -> bool {
        let l = s.to_ascii_lowercase();
        let b = l.as_bytes();
        b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\') && (l.ends_with(".png") || l.ends_with(".jpg") || l.ends_with(".jpeg") || l.ends_with(".webp"))
    }
    fn walk(v: &Value, label: &str, depth: u8, out: &mut Vec<(String, String)>) {
        if depth > 7 { return; }
        match v {
            Value::String(s) if is_img(s) => { if !out.iter().any(|(p, _)| p == s) { out.push((s.replace('\\', "/"), label.to_string())); } }
            Value::Object(m) => for (k, x) in m { if k == "output" || k == "_hint" || k.starts_with("_") { continue; } walk(x, &if label.is_empty() { k.clone() } else { format!("{label}.{k}") }, depth + 1, out); },
            Value::Array(a) => for (i, x) in a.iter().enumerate() { walk(x, &format!("{label}[{i}]"), depth + 1, out); },
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(r, "", 0, &mut found);
    if found.is_empty() { return; }
    if r["fullPath"].is_null() {
        r["fullPath"] = json!(found[0].0);
        if r["safePath"].is_null() { r["safePath"] = json!(found[0].0); }
    }
    let has_shots = r["screenshots"].as_array().map(|a| a.iter().any(|e| e["fullPath"].is_string())).unwrap_or(false);
    if !has_shots {
        r["screenshots"] = json!(found.iter().map(|(p, l)| json!({"fullPath": p, "safePath": p, "label": l})).collect::<Vec<_>>());
    }
}

/// May the bridge send WM_CLOSE to a Fusion window that no rule recognises? Only when it is
/// plainly a DIALOG. Fusion keeps hidden and visible top-level Qt windows that are not dialogs
/// (EAGLE's own schematic and board editor windows, detached document frames), and WM_CLOSE on
/// one of those closes documents and can exit Fusion: that is what happened on ConfRoomROG on
/// 2026-09-22 (a window titled "Fusion360" got WM_CLOSE, Fusion closed every document and ran
/// ExitApplicationCommand, the daily run went red, page issue #97) and on 2026-09-03. A window
/// that fails a test is left open and reported with its text, so the caller decides.
pub fn unknown_window_is_closable(ab: &dyn fusion_core::ab::AbClient, caller: &fusion_core::ab::Caller, dlg: &Value, rows: &[Value]) -> (bool, String) {
    let title = dlg["title"].as_str().unwrap_or("");
    if title.contains("EAGLE") { return (false, "an EAGLE editor window (closing it closes a document)".into()); }
    let r = fusion_core::dialogs::rect_of(&dlg["rect"]);
    let (w, h) = (r["width"].as_i64().unwrap_or(0), r["height"].as_i64().unwrap_or(0));
    let main = rows.iter().filter(|x| x["visible"].as_bool() != Some(false) && x["title"].as_str().map(|t| t.contains("Autodesk Fusion")).unwrap_or(false))
        .map(|x| fusion_core::dialogs::rect_of(&x["rect"])).max_by_key(|m| m["width"].as_i64().unwrap_or(0) * m["height"].as_i64().unwrap_or(0));
    if let Some(m) = main {
        let (mw, mh) = (m["width"].as_i64().unwrap_or(0), m["height"].as_i64().unwrap_or(0));
        if mw > 0 && mh > 0 && w * 10 >= mw * 8 && h * 10 >= mh * 8 { return (false, format!("window-sized ({w}x{h} against the main window's {mw}x{mh}): a main or document window, not a dialog")); }
    }
    let Some(hwnd) = dlg["hwnd"].as_i64() else { return (false, "no window handle".into()) };
    let buttons: Vec<String> = controls_cached(ab, caller, hwnd).iter()
        .filter(|c| c["controlType"].as_str().or(c["type"].as_str()).or(c["role"].as_str()).unwrap_or("").to_lowercase().contains("button"))
        .filter_map(|c| c["name"].as_str().map(|n| n.trim().to_lowercase())).collect();
    const DISMISS: &[&str] = &["ok", "cancel", "close", "no", "dismiss", "not now", "later", "skip", "got it", "continue"];
    if buttons.iter().any(|b| DISMISS.iter().any(|d| b == d || b.starts_with(&format!("{d} ")))) { (true, String::new()) }
    else { (false, format!("no Cancel/Close/OK-style button (buttons: {})", if buttons.is_empty() { "none readable".to_string() } else { buttons.join(", ") })) }
}

/// `classify_blocking_dialogs`: Fusion's Qt dialogs (from ab's window rows), classified by
/// title and, for a generic "Fusion" title, by the body text UIA exposes.
pub fn classify_blocking_dialogs(ab: &dyn fusion_core::ab::AbClient, caller: &fusion_core::ab::Caller, fusion_pids: &[u32]) -> Vec<Value> {
    let mut args = json!({});
    if !fusion_pids.is_empty() { args["pids"] = json!(fusion_pids); }
    let rows = match ab.call("desktop_enum_windows", args, caller, 10) { Ok(v) => v["windows"].as_array().or_else(|| v["data"]["windows"].as_array()).cloned().unwrap_or_default(), Err(_) => return vec![] };
    let mut out = Vec::new();
    for dlg in fusion_core::dialogs::qt_dialog_rows(&rows) {
        let title = dlg["title"].as_str().unwrap_or("").to_string();
        let mut classified = fusion_core::dialogs::category_for_title(&title);
        let mut matched_on: Option<&str> = None;
        if classified.as_ref().map(|(c, _)| c == "generic_modal_read_screenshot" || c == "unknown").unwrap_or(true) {
            if let Some(h) = dlg["hwnd"].as_i64() {
                let body: String = controls_cached(ab, caller, h).iter().filter_map(|c| c["name"].as_str().filter(|s| !s.is_empty()).map(str::to_string)).collect::<Vec<_>>().join(" ").chars().take(400).collect();
                if body.contains("ToolbarFramework") || body.contains("QTView") { continue; } // a Fusion panel (a floating BROWSER titled "Fusion360"), not a dialog
                if !body.is_empty() { if let Some(bt) = fusion_core::dialogs::category_for_title(&body) { classified = Some(bt); matched_on = Some("bodyText"); } }
            }
        }
        let Some((category, resolution)) = classified else { continue };
        let mut e = json!({"hwnd": dlg["hwnd"], "title": title, "category": category, "resolution": resolution});
        if let Some(m) = matched_on { e["matchedOn"] = json!(m); }
        if category == "unknown" || category == "generic_modal_read_screenshot" {
            let (ok, why) = unknown_window_is_closable(ab, caller, &dlg, &rows);
            e["closable"] = json!(ok);
            if !ok { e["notClosableWhy"] = json!(why); }
        }
        out.push(e);
    }
    out
}

/// While a verb that ADDs from a library runs: Fusion's Select Library picker, which it opens when two
/// libraries in use share a name. Its in-use list survives a restart, so the bridge can't always see this
/// coming, and answering it doesn't help: picking a row and OK closes it, but that ADD lands nothing and the
/// next one asks again (live 2026-11-03). So it is closed, and the verb says why, naming both files.
pub struct PickerWatch { stop: std::sync::Arc<std::sync::atomic::AtomicBool>, found: std::sync::Arc<std::sync::Mutex<Option<Value>>> }

pub fn watch_for_library_picker(caller: &fusion_core::ab::Caller, fusion_pids: &[u32]) -> Option<PickerWatch> {
    let ab = fusion_core::ab::Loopback::from_env()?;
    let (stop, found) = (std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), std::sync::Arc::new(std::sync::Mutex::new(None)));
    let (stop2, found2, caller, pids) = (stop.clone(), found.clone(), caller.clone(), fusion_pids.to_vec());
    std::thread::spawn(move || {
        use fusion_core::ab::AbClient;
        while !stop2.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let mut args = json!({});
            if !pids.is_empty() { args["pids"] = json!(pids); }
            let Ok(rows) = ab.call("desktop_enum_windows", args, &caller, 10) else { continue };
            let rows = rows["windows"].as_array().or_else(|| rows["data"]["windows"].as_array()).cloned().unwrap_or_default();
            let Some(dlg) = fusion_core::dialogs::qt_dialog_rows(&rows).into_iter().find(|d| d["title"].as_str().map(|t| t.contains("Select Library")).unwrap_or(false)) else { continue };
            let Some(h) = dlg["hwnd"].as_i64() else { continue };
            let body = controls_cached(&ab, &caller, h).iter().filter_map(|c| c["name"].as_str().map(str::to_string)).collect::<Vec<_>>().join(" ");
            let (gone, how) = close_native_dialog(&ab, &caller, h);
            *found2.lock().unwrap() = Some(json!({"paths": library_picker_paths(&body), "closed": gone, "how": how}));
            break;
        }
    });
    Some(PickerWatch { stop, found })
}

/// The files the picker lists: each row reads "name (path)".
pub fn library_picker_paths(body: &str) -> Vec<String> {
    // a file row names its .lbr, a cloud copy's row its place in the hub: either has a path separator or a colon
    let mut out: Vec<String> = regex::Regex::new(r"\(([^()]*[/\\:][^()]*)\)").unwrap().captures_iter(body).map(|c| c[1].to_string()).collect();
    out.dedup();
    out
}

/// A verb's reply when the picker came up during it: what ran stays reported, the reason replaces Fusion's assertion.
pub fn finish_picker_watch(watch: Option<PickerWatch>, mut r: Value) -> Value {
    let Some(w) = watch else { return r };
    w.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let Some(found) = w.found.lock().unwrap().clone() else { return r };
    let paths: Vec<String> = found["paths"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    r["success"] = json!(false);
    r["errorCode"] = json!("library_name_in_use");
    r["librariesInUse"] = json!(paths);
    r["pickerClosed"] = found["closed"].clone();
    r["error"] = json!(format!("Fusion has two libraries with one name in use{} and opened its Select Library picker. The bridge closed it, and that part didn't land.{}", if paths.is_empty() { String::new() } else { format!(" ({})", paths.join(" and ")) }, if found["closed"] == true { "" } else { " The picker did not close, so Fusion is still blocked." }));
    r["_hint"] = json!("Give this file another name, or take the other file away and restart Fusion (fusion_restart_for_addin). A restart alone doesn't clear it: Fusion keeps its in-use list until the file is gone.");
    r
}

/// While a script runs: Fusion's "board file has errors and will be opened with warnings" notice. A script
/// that opens such a design doesn't return until it's answered, and a call between machines gives up at 120 s
/// with nothing to say why (AI Flow, 2026-11-03). OK is its only answer, so the bridge gives it. A script
/// shorter than the first look costs nothing.
pub struct NoticeWatch { stop: std::sync::Arc<std::sync::atomic::AtomicBool>, answered: std::sync::Arc<std::sync::Mutex<Vec<Value>>> }

pub fn watch_for_open_notice(caller: &fusion_core::ab::Caller, fusion_pids: &[u32]) -> Option<NoticeWatch> {
    let ab = fusion_core::ab::Loopback::from_env()?;
    let (stop, answered) = (std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), std::sync::Arc::new(std::sync::Mutex::new(Vec::new())));
    let (stop2, answered2, caller, pids) = (stop.clone(), answered.clone(), caller.clone(), fusion_pids.to_vec());
    std::thread::spawn(move || {
        use fusion_core::ab::AbClient;
        use std::sync::atomic::Ordering;
        let pause = |secs: u64| -> bool { for _ in 0..(secs * 2) { if stop2.load(Ordering::SeqCst) { return false; } std::thread::sleep(std::time::Duration::from_millis(500)); } true };
        if !pause(6) { return; }
        loop {
            for d in classify_blocking_dialogs(&ab, &caller, &pids) {
                if d["category"] != "board_opened_with_warnings" { continue; }
                if let Some(h) = d["hwnd"].as_i64() {
                    let clicked = ab.call("desktop_ui_click", json!({"hwnd": h, "name": "OK", "role": "Button"}), &caller, 15).map(|v| v["success"].as_bool() != Some(false)).unwrap_or(false);
                    answered2.lock().unwrap().push(json!({"title": d["title"], "answeredWith": "OK", "clicked": clicked}));
                }
            }
            if !pause(3) { return; }
        }
    });
    Some(NoticeWatch { stop, answered })
}

pub fn finish_notice_watch(watch: Option<NoticeWatch>, mut r: Value) -> Value {
    let Some(w) = watch else { return r };
    w.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let answered = w.answered.lock().unwrap().clone();
    if !answered.is_empty() && r.is_object() {
        r["noticesAnswered"] = json!(answered);
        r["_hintNotice"] = json!("Fusion opened a board file with errors and said so; the bridge answered its notice with OK. Read the board before trusting it: fusion_routing_state lists any invalidViaNets, and fusion_set_layer_count at the same count repairs them.");
    }
    r
}

/// Fusion's "<file> has errors and cannot be opened" modal, by its text: (hwnd, what it says).
pub fn file_refused_dialog(ab: &dyn fusion_core::ab::AbClient, caller: &fusion_core::ab::Caller, fusion_pids: &[u32]) -> Option<(i64, String)> {
    let mut args = json!({});
    if !fusion_pids.is_empty() { args["pids"] = json!(fusion_pids); }
    let rows = ab.call("desktop_enum_windows", args, caller, 10).ok()?;
    let rows = rows["windows"].as_array().or_else(|| rows["data"]["windows"].as_array()).cloned().unwrap_or_default();
    for dlg in fusion_core::dialogs::qt_dialog_rows(&rows) {
        let Some(h) = dlg["hwnd"].as_i64() else { continue };
        let body: String = controls_cached(ab, caller, h).iter().filter_map(|c| c["name"].as_str().filter(|s| !s.is_empty()).map(str::to_string)).collect::<Vec<_>>().join(" ");
        if body.contains("has errors and cannot be opened") { return Some((h, body.chars().take(300).collect())); }
    }
    None
}

/// Is this window still on screen (a visible top-level window with that hwnd)?
fn window_present(ab: &dyn fusion_core::ab::AbClient, caller: &fusion_core::ab::Caller, hwnd: i64) -> bool {
    ab.call("desktop_list_windows", json!({}), caller, 10).ok()
        .and_then(|v| v["data"]["windows"].as_array().or_else(|| v["windows"].as_array()).cloned())
        .map(|ws| ws.iter().any(|w| w["hwnd"].as_i64() == Some(hwnd) && w["visible"].as_bool() != Some(false)))
        .unwrap_or(false)
}

/// Close a native Windows file dialog (class #32770: Open, Save As) and PROVE it is gone (issue #133):
/// its Cancel button first (a common dialog always honours IDCANCEL, in the background by UI
/// Automation), then WM_CLOSE. Returns (closed, how).
pub fn close_native_dialog(ab: &dyn fusion_core::ab::AbClient, caller: &fusion_core::ab::Caller, hwnd: i64) -> (bool, &'static str) {
    let gone_within = |secs: u64| -> bool { for _ in 0..(secs * 2) { std::thread::sleep(std::time::Duration::from_millis(500)); if !window_present(ab, caller, hwnd) { return true; } } false };
    if !window_present(ab, caller, hwnd) { return (true, "already gone"); }
    let _ = ab.call("desktop_ui_click", json!({"hwnd": hwnd, "name": "Cancel", "role": "Button"}), caller, 15);
    if gone_within(4) { return (true, "Cancel button"); }
    let _ = ab.call("desktop_close_window", json!({"hwnd": hwnd}), caller, 15);
    if gone_within(4) { return (true, "WM_CLOSE"); }
    (false, "still open after Cancel and WM_CLOSE")
}

/// Native file dialogs (class #32770) open on Fusion's processes: an import that failed can leave its
/// Open dialog, which blocks Fusion's main thread like any modal (issue #133).
pub fn native_file_dialogs(ab: &dyn fusion_core::ab::AbClient, caller: &fusion_core::ab::Caller, fusion_pids: &[u32]) -> Vec<Value> {
    let mut args = json!({});
    if !fusion_pids.is_empty() { args["pids"] = json!(fusion_pids); }
    ab.call("desktop_enum_windows", args, caller, 10).ok()
        .map(|v| v["windows"].as_array().or_else(|| v["data"]["windows"].as_array()).cloned().unwrap_or_default()).unwrap_or_default()
        .into_iter().filter(|w| w["className"].as_str().or(w["class"].as_str()).map(|c| c.contains("#32770")).unwrap_or(false) && w["visible"].as_bool() != Some(false))
        .collect()
}

/// `fusion_dismiss_blocking_dialogs`: answer every blocking dialog SAFELY in the background
/// (WM_CLOSE = No/Cancel, never Yes), reporting what each one was.
pub fn dismiss_blocking_dialogs(state_dir: &Path, args: &Value, caller: &fusion_core::ab::Caller, fusion_pids: &[u32], taskbar: &dyn Fn(Option<f64>)) -> Value {
    let Some(ab) = fusion_core::ab::Loopback::from_env() else {
        return json!({"success": false, "errorCode": "ab_loopback_unavailable", "error": "Cannot see Fusion's dialogs: the ab loopback API is not available to this bridge.", "_hint": "The exe was not spawned by ab (ADOM_DIRECT_API_URL unset). Let ab spawn it."});
    };
    use fusion_core::ab::AbClient;
    let mut classified = classify_blocking_dialogs(&ab, caller, fusion_pids);
    let want: Vec<String> = args["categories"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())).collect()).unwrap_or_default();
    if !want.is_empty() { classified.retain(|d| d["category"].as_str().map(|c| want.iter().any(|w| w == c)).unwrap_or(false)); }
    // native Open/Save dialogs are not Qt dialogs, but they block Fusion all the same: fusion_check_dialogs
    // lists them, so this answers them too, with Cancel, and proves each is gone (issue #133)
    let mut native_answered = Vec::new();
    if want.is_empty() || want.iter().any(|w| w == "file_dialog") {
        // a cancelled import opens its next Open dialog (the .brd after the .sch) at once: scan again
        // until none is left, a few rounds at most
        for round in 0..3 {
            let found = native_file_dialogs(&ab, caller, fusion_pids);
            if found.is_empty() { break; }
            if round > 0 { std::thread::sleep(std::time::Duration::from_millis(500)); }
            for w in found {
                let Some(h) = w["hwnd"].as_i64() else { continue };
                let (closed, how) = close_native_dialog(&ab, caller, h);
                crate::fusion_prefs::action_ledger(state_dir, "dialog_dismissed", json!({"title": w["title"], "category": "file_dialog", "hwnd": h, "closed": closed, "how": how}));
                native_answered.push(json!({"title": w["title"], "category": "file_dialog", "answeredWith": format!("Cancel ({how})"), "closed": closed, "hwnd": h}));
            }
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }
    }
    if classified.is_empty() && !native_answered.is_empty() {
        let all_closed = native_answered.iter().all(|d| d["closed"] == true);
        return json!({"success": all_closed, "count": native_answered.len(), "dismissed": native_answered,
            "_hint": if all_closed { "Closed the leftover native Open/Save dialog(s) with Cancel; nothing was opened or saved." } else { "A native Open/Save dialog did not close with Cancel or WM_CLOSE: it is still on screen (dismissed[].hwnd)." }});
    }
    if classified.is_empty() {
        return json!({"success": true, "dismissed": [], "count": 0, "_hint": if want.is_empty() { "No blocking Fusion dialog is open." } else { "No blocking dialog in the requested categories." }});
    }
    let mut answered = Vec::new();
    let mut upload_active = false;
    let mut left_open = Vec::new();
    for d in &classified {
        let cat = d["category"].as_str().unwrap_or("");
        if d["closable"].as_bool() == Some(false) {
            eprintln!("[fusion-bridge] dismiss_blocking_dialogs: LEFT OPEN {} {:?}: {}", d["hwnd"], d["title"].as_str().unwrap_or(""), d["notClosableWhy"].as_str().unwrap_or(""));
            crate::fusion_prefs::action_ledger(state_dir, "dialog_left_open", json!({"title": d["title"], "category": cat, "hwnd": d["hwnd"], "why": d["notClosableWhy"]}));
            left_open.push(json!({"title": d["title"], "hwnd": d["hwnd"], "why": d["notClosableWhy"]}));
            continue;
        }
        let mut safe = "No/Cancel (WM_CLOSE, background)";
        if cat == "upload_in_progress" { upload_active = true; safe = "No - declined close, Hub upload left to drain (NEVER Yes)"; }
        // closed means it is gone from the screen, not that the close was accepted: this notice ignores
        // WM_CLOSE, and the verb used to report it closed while it was still up
        let mut click_error = Value::Null;
        if cat == "board_opened_with_warnings" {
            safe = "OK (the only answer: it's a notice)";
            if let Some(h) = d["hwnd"].as_i64() {
                match ab.call("desktop_ui_click", json!({"hwnd": h, "name": "OK", "role": "Button"}), caller, 15) {
                    Ok(v) if v["success"].as_bool() == Some(false) => { click_error = json!(v["errorCode"].as_str().or(v["error"].as_str()).unwrap_or("refused")); }
                    Err(e) => { click_error = json!(format!("{e}")); }
                    _ => {}
                }
            }
        }
        let gone_within = |h: i64, secs: u64| -> bool { for _ in 0..(secs * 2) { std::thread::sleep(std::time::Duration::from_millis(500)); if !window_present(&ab, caller, h) { return true; } } false };
        let ok = d["hwnd"].as_i64().map(|h| {
            if cat == "board_opened_with_warnings" && gone_within(h, 3) { return true; }
            let _ = ab.call("desktop_close_window", json!({"hwnd": h}), caller, 15);
            gone_within(h, 3)
        }).unwrap_or(false);
        // what was closed is evidence (2026-09-03: a post-drill dismiss closed an 'unknown' window
        // and the add-in went silent; nothing had recorded which window it was)
        eprintln!("[fusion-bridge] dismiss_blocking_dialogs: {} {:?} category={cat} closed={ok}", d["hwnd"], d["title"].as_str().unwrap_or(""));
        crate::fusion_prefs::action_ledger(state_dir, "dialog_dismissed", json!({"title": d["title"], "category": cat, "hwnd": d["hwnd"], "closed": ok, "how": "WM_CLOSE"}));
        answered.push(json!({"title": d["title"], "category": cat, "answeredWith": safe, "closed": ok, "clickError": click_error, "resolution": d["resolution"]}));
    }
    let mut hint = format!("Answered {} blocking dialog(s) in the background (No/Cancel - never Yes, never focus-stolen).", answered.len());
    if upload_active { hint.push_str(" One was a Hub-UPLOAD 'lose these changes?' prompt: it was declined (No), but the upload is STILL in flight - poll app.hasActiveJobs (fusion_state) until it drains before any close/saveAs, or you WILL lose the 3D package."); }
    taskbar(if upload_active { Some(1.0) } else { None });
    if !left_open.is_empty() { hint.push_str(&format!(" LEFT OPEN {} window(s) no rule knows that do not look like a dialog (see leftOpen[].why): WM_CLOSE on a main, document or EAGLE editor window closes documents and can exit Fusion. fusion_check_dialogs shows their text; decide from that.", left_open.len())); }
    answered.extend(native_answered);
    let still_up: Vec<Value> = answered.iter().filter(|d| d["closed"] != true).map(|d| d["title"].clone()).collect();
    if !still_up.is_empty() { hint.push_str(&format!(" STILL UP: {} did not close (closed:false above), so Fusion is still blocked.", still_up.len())); }
    json!({"success": still_up.is_empty(), "count": answered.len(), "dismissed": answered, "stillUp": still_up, "leftOpen": left_open, "uploadStillActive": upload_active, "_hint": hint})
}

fn screenshot_hwnd(ab: &dyn fusion_core::ab::AbClient, caller: &fusion_core::ab::Caller, hwnd: i64) -> Option<String> {
    use serde_json::json as j;
    ab.call("desktop_screenshot_window", j!({"hwnd": hwnd}), caller, 40).ok().and_then(|r| r["fullPath"].as_str().or(r["safePath"].as_str()).map(|s| s.replace('\\', "/")))
}

/// `_capture_dialog_array`: after a mutating op, every Fusion dialog with a screenshot, or None.
pub fn capture_dialog_array(caller: &fusion_core::ab::Caller, fusion_pids: &[u32], settle_ms: u64) -> Option<Value> {
    let ab = fusion_core::ab::Loopback::from_env()?;
    use fusion_core::ab::AbClient;
    if settle_ms > 0 { std::thread::sleep(std::time::Duration::from_millis(settle_ms)); }
    let mut args = json!({});
    if !fusion_pids.is_empty() { args["pids"] = json!(fusion_pids); }
    let rows = ab.call("desktop_enum_windows", args, caller, 10).ok().map(|v| v["windows"].as_array().or_else(|| v["data"]["windows"].as_array()).cloned().unwrap_or_default())?;
    let raw = fusion_core::dialogs::qt_dialog_rows(&rows);
    if raw.is_empty() { return None; }
    // ab's rows do not carry IsWindowEnabled for the main window; Python treats an unreadable
    // state as enabled, which is the conservative branch (generic docked panels are suppressed)
    let main_enabled = true;
    let classified: std::collections::HashMap<i64, Value> = classify_blocking_dialogs(&ab, caller, fusion_pids).into_iter().filter_map(|c| c["hwnd"].as_i64().map(|h| (h, c))).collect();
    let mut out = Vec::new();
    for d in raw {
        let (w, h) = (d["rect"]["width"].as_i64().unwrap_or(0), d["rect"]["height"].as_i64().unwrap_or(0));
        if w < 80 || h < 50 { continue; }
        let hwnd = d["hwnd"].as_i64().unwrap_or(0);
        let title = d["title"].as_str().unwrap_or("").to_string();
        let c = classified.get(&hwnd);
        let cat = c.and_then(|c| c["category"].as_str());
        if main_enabled && matches!(cat, None | Some("generic_modal_read_screenshot") | Some("unknown")) { continue; }
        let mut entry = json!({"hwnd": hwnd, "title": title});
        if let Some(cat) = cat { entry["category"] = json!(cat); entry["resolution"] = c.map(|c| c["resolution"].clone()).unwrap_or(Value::Null); }
        if let Some(p) = screenshot_hwnd(&ab, caller, hwnd) { entry["screenshot"] = json!(p); }
        out.push(entry);
    }
    if out.is_empty() { return None; }
    let n = out.len();
    Some(json!({"dialogsDetected": n, "dialogs": out, "mainWindowEnabled": main_enabled,
        "_hint": format!("⚠️ {n} Fusion dialog(s)/owned popup(s) are up after this operation. STOP and ANALYZE before the next call: READ each dialogs[].screenshot and use its category/resolution. Do NOT blind-dismiss - some confirms LOSE work (clicking Yes on a Hub-upload 'are you sure you want to close?' destroys the uploaded packages). Take the work-preserving action (often: click No, then wait for the upload to drain). See the fusion-driving skill.")}))
}

/// `_apply_failure_dialogs`: an Error dialog after an op means the op FAILED, whatever the API said.
pub fn apply_failure_dialogs(mut result: Value, caller: &fusion_core::ab::Caller, fusion_pids: &[u32]) -> Value {
    let Some(ab) = fusion_core::ab::Loopback::from_env() else { return result };
    use fusion_core::ab::AbClient;
    let failure_cats = fusion_core::dialogs::category_set("FAILURE");
    let failures: Vec<Value> = classify_blocking_dialogs(&ab, caller, fusion_pids).into_iter().filter(|d| d["category"].as_str().map(|c| failure_cats.iter().any(|f| f == c)).unwrap_or(false)).collect();
    if failures.is_empty() || !result.is_object() { return result; }
    let f = &failures[0];
    let shots: Vec<String> = failures.iter().filter_map(|d| d["hwnd"].as_i64().and_then(|h| screenshot_hwnd(&ab, caller, h))).collect();
    let prev = result["error"].as_str().unwrap_or("").to_string();
    result["success"] = json!(false);
    result["errorCode"] = json!("fusion_operation_error");
    result["error"] = json!(format!("Operation FAILED - Fusion '{}' dialog is up{}", f["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("Error"), if prev.is_empty() { String::new() } else { format!(" (prior: {prev})") }));
    result["_hint"] = json!(f["resolution"].as_str().filter(|r| !r.is_empty()).unwrap_or("Fusion shows an Error dialog; the operation failed. Read the screenshot."));
    if !result["data"].is_object() { result["data"] = json!({}); }
    result["data"]["errorDialog"] = json!({"title": f["title"], "category": f["category"], "resolution": f["resolution"], "screenshots": shots});
    for d in &failures { if let Some(h) = d["hwnd"].as_i64() { let _ = ab.call("desktop_close_window", json!({"hwnd": h}), caller, 15); } }
    result
}

/// `_merge_dialog_array`: the dialog array onto a mutating verb's result, then the failure guard.
pub fn merge_dialog_array(mut result: Value, caller: &fusion_core::ab::Caller, fusion_pids: &[u32]) -> Value {
    if !result.is_object() { return result; }
    let Some(arr) = capture_dialog_array(caller, fusion_pids, 600) else { return result };
    if !result["data"].is_object() { result["data"] = json!({}); }
    result["data"]["dialogArray"] = arr.clone();
    let existing = result["_hint"].as_str().map(str::to_string);
    result["_hint"] = json!(format!("{}{}", arr["_hint"].as_str().unwrap_or(""), existing.map(|e| format!(" | {e}")).unwrap_or_default()));
    apply_failure_dialogs(result, caller, fusion_pids)
}

#[cfg(test)]
mod closable_tests {
    use super::*;
    struct FakeAb(Value);
    impl fusion_core::ab::AbClient for FakeAb {
        fn call(&self, _verb: &str, _args: Value, _caller: &fusion_core::ab::Caller, _t: u64) -> Result<Value, fusion_core::ab::AbError> { Ok(self.0.clone()) }
    }
    fn main_row() -> Value { json!({"hwnd": 1, "title": "BQ25792 - Autodesk Fusion ", "className": "Qt683QWindowIcon", "visible": true, "rect": [0, 0, 2400, 1270]}) }
    fn buttons(names: &[&str]) -> FakeAb { FakeAb(json!({"controls": names.iter().map(|n| json!({"name": n, "controlType": "Button"})).collect::<Vec<_>>()})) }

    #[test]
    fn a_small_dialog_with_cancel_may_be_closed() {
        let dlg = json!({"hwnd": 2, "title": "Fusion360", "rect": {"left": 800, "top": 400, "width": 520, "height": 240}});
        let (ok, why) = unknown_window_is_closable(&buttons(&["OK", "Cancel"]), &fusion_core::ab::Caller::default(), &dlg, &[main_row()]);
        assert!(ok, "{why}");
    }
    #[test]
    fn a_window_sized_frame_is_never_closed() {
        // 2026-09-22 (#97): WM_CLOSE on a window like this made Fusion close every document and exit
        let dlg = json!({"hwnd": 3, "title": "Fusion360", "rect": {"left": 0, "top": 0, "width": 2300, "height": 1200}});
        let (ok, why) = unknown_window_is_closable(&buttons(&["Close"]), &fusion_core::ab::Caller::default(), &dlg, &[main_row()]);
        assert!(!ok); assert!(why.contains("window-sized"), "{why}");
    }
    #[test]
    fn an_eagle_editor_window_is_never_closed() {
        let dlg = json!({"hwnd": 4, "title": "4 Board - C:/x/BQ25792.brd - EAGLE 9.7.0 premium", "rect": {"left": 700, "top": 180, "width": 900, "height": 600}});
        let (ok, _) = unknown_window_is_closable(&buttons(&["OK"]), &fusion_core::ab::Caller::default(), &dlg, &[main_row()]);
        assert!(!ok);
    }
    #[test]
    fn a_window_with_no_dismiss_button_is_left_open() {
        let dlg = json!({"hwnd": 5, "title": "Fusion360", "rect": {"left": 800, "top": 400, "width": 600, "height": 400}});
        let (ok, why) = unknown_window_is_closable(&buttons(&["Save", "Export"]), &fusion_core::ab::Caller::default(), &dlg, &[main_row()]);
        assert!(!ok); assert!(why.contains("no Cancel/Close/OK"), "{why}");
    }
}

#[cfg(test)]
mod image_delivery_tests {
    use super::*;
    #[test]
    fn old_path_keys_gain_full_path_and_screenshots() {
        let mut r = json!({"success": true, "path": "C:\\tmp\\conduit-screenshots\\viewport.png", "data": {"postOpenScreenshot": {"screenshots": [{"path": "C:/tmp/b.png"}]}}});
        add_image_delivery(&mut r);
        assert_eq!(r["fullPath"], "C:/tmp/conduit-screenshots/viewport.png");
        assert_eq!(r["screenshots"].as_array().unwrap().len(), 2);
        assert_eq!(r["path"], "C:\\tmp\\conduit-screenshots\\viewport.png", "old key kept");
    }
    #[test]
    fn a_reply_that_already_has_full_path_is_left_alone() {
        let mut r = json!({"fullPath": "C:/x/a.png", "screenshots": [{"fullPath": "C:/x/a.png"}]});
        let before = r.clone(); add_image_delivery(&mut r); assert_eq!(r, before);
    }
    #[test]
    fn replies_without_images_are_untouched() {
        let mut r = json!({"success": true, "outputPath": "C:/tmp/board.sch"});
        let before = r.clone(); add_image_delivery(&mut r); assert_eq!(r, before);
    }
}

#[cfg(test)] mod picker_tests {
    use super::*;
    fn watch(found: Option<Value>) -> Option<PickerWatch> { Some(PickerWatch { stop: Default::default(), found: std::sync::Arc::new(std::sync::Mutex::new(found)) }) }
    #[test] fn the_picker_rows_give_both_paths() {
        let body = "System There are multiple libraries named lane!\nPick one. lane (C:/a/lane.lbr) lane (C:/b/lane.lbr)";
        assert_eq!(library_picker_paths(body), vec!["C:/a/lane.lbr".to_string(), "C:/b/lane.lbr".to_string()]);
    }
    #[test] fn a_cloud_copys_row_counts_and_plain_words_dont() {
        assert_eq!(library_picker_paths("Pick one. TI772 (C:/x/TI772.lbr) TI772 (urn:adsk.wipprod:dm.lineage:AB) (not this)"), vec!["C:/x/TI772.lbr".to_string(), "urn:adsk.wipprod:dm.lineage:AB".to_string()]);
        let r = finish_picker_watch(watch(Some(json!({"paths": [], "closed": true}))), json!({"success": false}));
        assert!(!r["error"].as_str().unwrap().contains("()"));
    }
    #[test] fn a_picker_turns_the_reply_into_the_reason() {
        let r = finish_picker_watch(watch(Some(json!({"paths": ["C:/a/l.lbr", "C:/b/l.lbr"], "closed": true}))), json!({"success": false, "errorCode": "fusion_assert", "commands": 3}));
        assert_eq!(r["errorCode"], "library_name_in_use");
        assert_eq!(r["commands"], 3);
        assert_eq!(r["pickerClosed"], true);
        assert!(r["error"].as_str().unwrap().contains("C:/a/l.lbr and C:/b/l.lbr"));
    }
    #[test] fn an_answered_notice_is_said_in_the_reply() {
        let w = Some(NoticeWatch { stop: Default::default(), answered: std::sync::Arc::new(std::sync::Mutex::new(vec![json!({"answeredWith": "OK"})])) });
        let r = finish_notice_watch(w, json!({"success": true}));
        assert_eq!(r["noticesAnswered"][0]["answeredWith"], "OK");
        assert!(finish_notice_watch(None, json!({"success": true}))["noticesAnswered"].is_null());
    }
    #[test] fn no_picker_leaves_the_reply_alone() {
        assert_eq!(finish_picker_watch(watch(None), json!({"success": true}))["success"], true);
        assert_eq!(finish_picker_watch(None, json!({"success": true}))["success"], true);
    }
}

//! Stop, kill and restart-for-add-in, native. Ported from handlers/close_fusion.py and
//! server.py's `_handle_restart_for_addin`. Process operations go through ab's `process_list`
//! and `process_kill` (verb for OPERATIONS, the Toolhelp snapshot for POLLS), with a hidden
//! taskkill only as the degraded path; windows are closed through `desktop_close_window`.

use serde_json::{json, Value};
use std::time::{Duration, Instant};

use crate::addin::Ctx;
use fusion_core::ab::{AbClient, Loopback};

pub const FUSION_PROCESS_NAMES: &[&str] = &["Fusion360.exe", "FusionLauncher.exe", "Fusion.exe", "FusionService.exe", "FusionCEF.exe", "Fusion360Helper.exe"];
const BRIDGE_MARKS: &[&str] = &["adom-test-", "adomdemosample", "adom-fusion-wrap", "ads8588sipm", "0603waf"];

fn addin_post(cmd: &str, args: Value, timeout: u64) -> Option<Value> {
    ureq::post("http://127.0.0.1:8774/command").timeout(Duration::from_secs(timeout)).send_json(json!({"command": cmd, "args": args})).ok().and_then(|r| r.into_json().ok())
}

/// `_close_all_documents_via_addin`: close every open document without saving, so the next
/// launch does not raise Recovered Documents. Waits ~40s for a slow add-in, then gives up.
pub fn close_all_documents_via_addin() -> Value {
    let mut out = json!({"closed": [], "errors": [], "count": 0});
    let mut info = addin_post("document_info", json!({}), 5);
    if info.is_none() {
        for w in [3u64, 6, 10, 10, 10] { std::thread::sleep(Duration::from_secs(w)); info = addin_post("document_info", json!({}), 10); if info.is_some() { break; } }
    }
    let Some(info) = info else { return out };
    if info["success"].as_bool() != Some(true) { return out; }
    let docs = info["data"]["openDocuments"].as_array().cloned().unwrap_or_default();
    out["count"] = json!(docs.len());
    for d in docs.iter().rev() {
        let name = d["name"].as_str().unwrap_or("").to_string();
        if name.is_empty() { continue; }
        match addin_post("close_document", json!({"name": name, "save": false, "saveToClose": true}), 10) {
            Some(r) if r["success"].as_bool() == Some(true) => out["closed"].as_array_mut().unwrap().push(json!(name)),
            Some(r) => out["errors"].as_array_mut().unwrap().push(json!(format!("{name}: {}", r["error"].as_str().unwrap_or("?")))),
            None => out["errors"].as_array_mut().unwrap().push(json!(format!("{name}: no answer"))),
        }
    }
    out
}

/// ab's process rows for the Fusion family, or None when ab is unreachable.
fn ab_process_rows(ctx: &Ctx) -> Option<Vec<Value>> {
    let ab = Loopback::from_env()?;
    let r = ab.call("process_list", json!({"name": "Fusion"}), &ctx.caller, 10).ok()?;
    r["processes"].as_array().cloned()
}

fn family_pids(ctx: &Ctx) -> Vec<u32> {
    if let Some(rows) = ab_process_rows(ctx) {
        return rows.iter().filter(|r| r["name"].as_str().map(|n| FUSION_PROCESS_NAMES.iter().any(|f| f.eq_ignore_ascii_case(n))).unwrap_or(false)).filter_map(|r| r["pid"].as_u64().map(|p| p as u32)).collect();
    }
    ctx.host.fusion_processes().iter().map(|p| p.pid).collect()
}

fn has_fusion_process(ctx: &Ctx) -> bool { !family_pids(ctx).is_empty() }

/// `_force_kill_fusion`: ab's process_kill per row, then the hidden taskkill sweep, then per pid /T.
fn force_kill_fusion(ctx: &Ctx) -> u32 {
    let mut killed = 0u32;
    if let (Some(ab), Some(rows)) = (Loopback::from_env(), ab_process_rows(ctx)) {
        for r in rows {
            let name = r["name"].as_str().unwrap_or("");
            if let Some(pid) = r["pid"].as_u64() {
                if FUSION_PROCESS_NAMES.iter().any(|f| f.eq_ignore_ascii_case(name)) && ab.call("process_kill", json!({"pid": pid, "tree": true, "reason": format!("close Fusion: force-kill surviving {name}")}), &ctx.caller, 15).is_ok() { killed += 1; }
            }
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        for name in FUSION_PROCESS_NAMES {
            if std::process::Command::new("taskkill").args(["/F", "/IM", name]).creation_flags(0x0800_0000).output().map(|o| o.status.success()).unwrap_or(false) { killed += 1; }
        }
        std::thread::sleep(Duration::from_secs(1));
        for pid in family_pids(ctx) {
            if std::process::Command::new("taskkill").args(["/F", "/T", "/PID", &pid.to_string()]).creation_flags(0x0800_0000).output().map(|o| o.status.success()).unwrap_or(false) { killed += 1; }
        }
    }
    // macOS: there is no `taskkill` and, on a Mac, no `ab` either — `Loopback::from_env()` above
    // yields nothing, so WITHOUT this block force-kill simply does nothing and a wedged Fusion
    // survives. That is the exact hole closed in the Python shell (`handlers/close_fusion.py`)
    // after a main thread parked in Fusion's crash-recovery dialog had to be killed by hand.
    //
    // Graded, bounded, and every step confirms with the host poll:
    //   a) `osascript ... quit` — Fusion's own graceful path, honours its save prompts
    //   b) SIGTERM
    //   c) SIGKILL
    // Measured on that wedge: osascript returns -1712 (AppleEvent timeout) because the main
    // thread is not in its message loop, SIGTERM is ignored for the same reason, and only
    // SIGKILL lands. Waiting between steps is what makes the escalation meaningful.
    #[cfg(target_os = "macos")]
    {
        let still_running = || !ctx.host.fusion_processes().is_empty();
        if still_running() {
            let _ = std::process::Command::new("osascript")
                .args(["-e", "tell application \"Autodesk Fusion\" to quit"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline && still_running() {
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        for sig in [libc::SIGTERM, libc::SIGKILL] {
            if !still_running() { break; }
            for p in ctx.host.fusion_processes() {
                if p.pid == 0 { continue; }
                // SAFETY: kill(2) with a pid the host just reported; a dead pid is ESRCH, ignored.
                if unsafe { libc::kill(p.pid as libc::pid_t, sig) } == 0 { killed += 1; }
            }
            let deadline = Instant::now() + Duration::from_secs(8);
            while Instant::now() < deadline && still_running() {
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    killed
}

/// Visible windows whose title contains "Autodesk Fusion", as (hwnd, title).
fn fusion_windows(ctx: &Ctx) -> Vec<(i64, String)> {
    let Some(ab) = Loopback::from_env() else { return vec![] };
    let Ok(v) = ab.call("desktop_enum_windows", json!({}), &ctx.caller, 10) else { return vec![] };
    v["windows"].as_array().or_else(|| v["data"]["windows"].as_array()).cloned().unwrap_or_default().iter()
        .filter(|w| w["visible"].as_bool() != Some(false) && w["title"].as_str().map(|t| t.contains("Autodesk Fusion")).unwrap_or(false))
        .filter_map(|w| w["hwnd"].as_i64().map(|h| (h, w["title"].as_str().unwrap_or("").to_string()))).collect()
}

/// What became of a WM_CLOSE on a Fusion window.
#[derive(Debug, PartialEq)]
pub enum Closed { Gone, UnsavedWork(String), StillClosing }

/// What to do with a save prompt Fusion raised while closing: the bridge's own document is
/// answered Don't Save; anyone else's unsaved work (or a prompt naming no document) is cancelled
/// and ends the stop, so the decision goes back to the caller.
pub fn prompt_answer(owned_by_bridge: bool) -> &'static str { if owned_by_bridge { "dont_save" } else { "cancel_and_stop" } }

/// `_close_and_wait`: WM_CLOSE, then wait while Fusion closes. A heavy design takes far longer than
/// the old 15 s to close, so the wait is `max_secs` (fusion_stop's `closeTimeoutSec`, default 120),
/// and it ends early on a save prompt for work the bridge does not own.
fn close_and_wait(ctx: &Ctx, hwnd: i64, max_secs: f64) -> Closed {
    let Some(ab) = Loopback::from_env() else { return Closed::StillClosing };
    let _ = ab.call("desktop_close_window", json!({"hwnd": hwnd}), &ctx.caller, 15);
    let deadline = Instant::now() + Duration::from_secs_f64(max_secs);
    let mut next_look = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(300));
        if !fusion_windows(ctx).iter().any(|(h, _)| *h == hwnd) { return Closed::Gone; }
        if Instant::now() < next_look { continue; }
        next_look = Instant::now() + Duration::from_secs(1);
        let p = crate::etiquette::find_save_prompt(&ab, &ctx.caller, &ctx.pids);
        if p["found"].as_bool() != Some(true) { continue; }
        let text = p["text"].as_str().unwrap_or("").to_string();
        match prompt_answer(crate::etiquette::bridge_doc_in(&ctx.state_dir, &text).is_some()) {
            "dont_save" => { let _ = crate::etiquette::dismiss_save_prompt_if_bridge_doc(&ctx.state_dir, &ctx.caller, &ctx.pids, ""); }
            _ => {
                // WM_CLOSE on the prompt is its Cancel: nothing saved, nothing lost, no dialog left up
                if let Some(h) = p["hwnd"].as_i64() { let _ = ab.call("desktop_close_window", json!({"hwnd": h}), &ctx.caller, 10); }
                crate::fusion_prefs::action_ledger(&ctx.state_dir, "stop_cancelled_for_unsaved_work", json!({"prompt": text}));
                return Closed::UnsavedWork(text);
            }
        }
    }
    Closed::StillClosing
}

fn survivors(ctx: &Ctx) -> Value {
    json!({"success": false, "error": "Some Fusion processes survived force-kill.",
        "_hint": "The bridge tried taskkill /F /IM for every Fusion process and then taskkill /F /T per surviving PID, and something still holds on. Do NOT hand this to the user as a Task Manager chore: check whether the survivor is a crash-report host (fusion_readiness reports fusionCrashed) or is pinned by a modal, and re-call fusion_kill after clearing that.",
        "survivingPids": family_pids(ctx)})
}

/// `fusion_stop`: documents closed cleanly, then WM_CLOSE on every Fusion window; never a kill.
pub fn stop(ctx: &Ctx, args: &Value) -> Value {
    let mut report = json!({"closed": [], "errors": [], "count": 0});
    if args["skipCleanClose"].as_bool() != Some(true) { report = close_all_documents_via_addin(); if !report["closed"].as_array().map(|a| a.is_empty()).unwrap_or(true) { std::thread::sleep(Duration::from_millis(500)); } }
    let max_secs = args["closeTimeoutSec"].as_f64().unwrap_or(120.0).clamp(5.0, 900.0);
    let windows = fusion_windows(ctx);
    if windows.is_empty() {
        if has_fusion_process(ctx) { return json!({"success": false, "error": "No Fusion windows found, but Fusion processes are still running.", "_hint": "A graceful stop cannot reach them - use fusion_kill to force-kill."}); }
        return json!({"success": true, "output": "Fusion 360 is not running."});
    }
    let (mut closed, mut failed, mut unsaved) = (Vec::new(), Vec::new(), Vec::new());
    for (h, t) in &windows {
        match close_and_wait(ctx, *h, max_secs) {
            Closed::Gone => closed.push(t.clone()),
            Closed::UnsavedWork(prompt) => { unsaved.push(json!({"window": t, "prompt": prompt})); failed.push(t.clone()); }
            Closed::StillClosing => failed.push(t.clone()),
        }
    }
    if !unsaved.is_empty() {
        return json!({"success": false, "errorCode": "unsaved_work", "unsavedWork": unsaved,
            "error": format!("Fusion was not stopped: it asked to save changes the bridge did not make ({}). The prompt was cancelled, so nothing was saved, discarded or closed past that point.", unsaved.iter().map(|u| u["prompt"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join("; ")),
            "_hint": "Those changes are someone's work. Ask whoever owns them, or save them (fusion_save_document) and stop again. fusion_kill force-closes Fusion and loses them."});
    }
    if !failed.is_empty() {
        let bridge_only = failed.iter().all(|t| { let l = t.to_lowercase(); BRIDGE_MARKS.iter().any(|m| l.contains(m)) });
        return json!({"success": false, "error": format!("Graceful stop failed for: {}", failed.join(", ")), "output": if closed.is_empty() { String::new() } else { format!("Closed: {}", closed.join(", ")) }, "bridgeDocsOnly": bridge_only,
            "_hint": if bridge_only { "The unsaved document(s) blocking the close are BRIDGE-created throwaways (sample/test docs) - fusion_kill is SAFE here, nothing of the user's is at risk. Kill, then retry." } else { "A window would not close gracefully (a modal dialog often blocks WM_CLOSE). Use fusion_kill to force-kill, then retry." }});
    }
    for _ in 0..10 { if !has_fusion_process(ctx) { break; } std::thread::sleep(Duration::from_millis(500)); }
    let n = closed.len(); let dc = report["closed"].as_array().map(|a| a.len()).unwrap_or(0);
    json!({"success": true, "output": format!("Fusion 360 stopped gracefully ({n} window{}); {dc} document(s) closed cleanly first.", if n != 1 { "s" } else { "" }), "data": {"documentsClosed": report["closed"]}})
}

/// `fusion_kill`: force-kill (documents closed cleanly first when the add-in is reachable).
pub fn kill(ctx: &Ctx, args: &Value) -> Value {
    let mut report = json!({"closed": [], "errors": [], "count": 0});
    if args["skipCleanClose"].as_bool() != Some(true) { report = close_all_documents_via_addin(); if !report["closed"].as_array().map(|a| a.is_empty()).unwrap_or(true) { std::thread::sleep(Duration::from_millis(500)); } }
    let killed = force_kill_fusion(ctx);
    std::thread::sleep(Duration::from_secs(1));
    if has_fusion_process(ctx) { return survivors(ctx); }
    let dc = report["closed"].as_array().map(|a| a.len()).unwrap_or(0);
    json!({"success": true, "output": format!("Fusion 360 force-killed ({killed} process group{}); {dc} document(s) closed cleanly first.", if killed != 1 { "s" } else { "" }), "data": {"documentsClosed": report["closed"]}})
}

/// `fusion_restart_for_addin`: save, stop (or kill when only bridge scratch is unsaved), start, wait.
/// Who owns a Fusion restart. "auto" (the default): the bridge restarts Fusion itself whenever
/// the add-in needs it, without a toast, saving what can be saved (John, 2026-09-07: "you should
/// literally know to just allow fully restarting fusion"), but it stops short of losing work it
/// could not save: a never-saved document or a save prompt for a document the bridge did not
/// make turns the restart into the ask below. The machines it ran on are not all the AI's alone:
/// on a person's own PC it killed Fusion six times between 2026-10-14 and 10-28 with their
/// documents open, each time because the save step could not run. "force" is the old auto, for a
/// machine only an AI drives: never-saved documents are discarded, and Fusion is killed when
/// nothing can be saved. "ask" keeps the toast with the Save-and-restart choice. Pref key
/// `restartPolicy` in prefs.json, or the env ADOM_FUSION_RESTART_POLICY.
pub fn restart_policy(state_dir: &std::path::Path) -> String {
    if let Ok(v) = std::env::var("ADOM_FUSION_RESTART_POLICY") { if !v.trim().is_empty() { return v.trim().to_lowercase(); } }
    crate::small_verbs::get_pref(state_dir, "restartPolicy").and_then(|v| v.as_str().map(|s| s.to_lowercase())).unwrap_or_else(|| "auto".into())
}

/// The add-in request never reached Fusion (a dropped or refused connection), as opposed to the
/// add-in answering that it could not save.
fn save_unreachable(r: &Value) -> bool {
    r["error"].as_str().map(|e| e.starts_with("Add-in request failed") && !e.contains("non-JSON")).unwrap_or(false)
}

/// An auto restart stopped short of someone's unsaved work: for the next 30 minutes the gate asks
/// (the toast with Save and restart) instead of trying again behind the person's back.
pub fn note_auto_restart_declined(state_dir: &std::path::Path, why: &Value) {
    let _ = std::fs::create_dir_all(state_dir);
    let _ = std::fs::write(state_dir.join("auto_restart_declined.json"), json!({"at": now_secs(), "why": why}).to_string());
    crate::fusion_prefs::action_ledger(state_dir, "auto_restart_declined", json!({"why": why}));
}
pub fn auto_restart_declined_recently(state_dir: &std::path::Path) -> bool {
    std::fs::read_to_string(state_dir.join("auto_restart_declined.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["at"].as_f64()).map(|at| now_secs() - at < 1800.0).unwrap_or(false)
}

/// The ledger's one-line account of a restart, true to what each step did.
pub fn restart_summary(why: &str, policy: &str, steps: &[Value], discarded: &[String]) -> String {
    let saved = steps.iter().any(|s| s["step"] == "save" && s["ok"] == json!(true));
    let killed = steps.iter().any(|s| s["step"] == "kill");
    format!("The bridge restarted Fusion {why} (restartPolicy {policy}). {}{}",
        if saved { "Documents were saved first. " } else if killed { "Nothing could be saved first, and Fusion was force-closed. " } else { "Nothing could be saved first, so Fusion was closed the way a person closes it, with nobody's unsaved work at risk. " },
        if discarded.is_empty() { String::new() } else { format!("Discarded never-saved documents: {}. ", discarded.join(", ")) })
}

/// A plain informational toast on this machine (no buttons, nothing to answer).
pub fn notify_info(ctx: &Ctx, title: &str, body: &str) -> bool {
    match Loopback::from_env() {
        Some(ab) => { let r = ab.call("notify_user", json!({"title": title, "body": body, "level": "info"}), &ctx.caller, 15); r.is_ok() }
        None => false,
    }
}

/// When the add-in's main thread has been blocked by a modal since this instant (0 = not blocked).
static BLOCKED_SINCE: std::sync::Mutex<f64> = std::sync::Mutex::new(0.0);
/// How long a modal may hold the add-in before the bridge restarts Fusion itself (restartPolicy auto).
const BLOCKED_RESTART_AFTER_SECS: f64 = 90.0;

fn now_secs() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

/// Restart Fusion in the background because a modal nobody answers has held the add-in
/// (restartPolicy auto). Save is impossible (the add-in cannot answer), so this is a kill +
/// start; the ledger and the reply carry what was on screen.
fn begin_blocked_restart(ctx: &Ctx, blocking: &[Value], held: f64) -> bool {
    crate::fusion_prefs::action_ledger(&ctx.state_dir, "auto_restart_blocked_modal", json!({"heldSec": (held * 10.0).round() / 10.0,
        "dialogs": blocking.iter().map(|d| json!({"title": d["title"], "category": d["category"]})).collect::<Vec<_>>()}));
    let url = crate::launch::SELF_URL.lock().unwrap().clone().or_else(|| crate::launch::SIDECAR_URL.lock().unwrap().clone());
    let Some(u) = url else { return false; };
    std::thread::spawn(move || {
        let r = ureq::post(&u).timeout(Duration::from_secs(900)).send_json(json!({"command": "fusion_restart_for_addin", "args": {"save": false, "auto": true, "wait": true, "why": "modal blocked the add-in"}})).ok().and_then(|r| r.into_json::<Value>().ok()).unwrap_or(json!({}));
        eprintln!("[fusion-bridge] auto restart after a blocking modal: {{success: {}, errorCode: {}}}", r["success"], r["errorCode"]);
    });
    true
}

/// Every dialog in front of Fusion, read programmatically through UIA: title, category (when a
/// rule matches), the visible text and the button names. No pixels involved.
pub fn dialog_inventory(ctx: &Ctx) -> Vec<Value> {
    let Some(ab) = Loopback::from_env() else { return vec![] };
    let mut args = json!({}); if !ctx.pids.is_empty() { args["pids"] = json!(ctx.pids); }
    let rows = match ab.call("desktop_enum_windows", args, &ctx.caller, 10) { Ok(v) => v["windows"].as_array().or_else(|| v["data"]["windows"].as_array()).cloned().unwrap_or_default(), Err(_) => return vec![] };
    let classified = crate::small_verbs::classify_blocking_dialogs(&ab, &ctx.caller, &ctx.pids);
    let mut out = Vec::new();
    // native Windows file dialogs (class #32770: Open, Save As) are not Qt dialogs; a
    // dialog-driven import (ImportSCHAndBRDCmd) can leave one up, and it blocks Fusion just the same
    for w in &rows {
        let class = w["className"].as_str().or(w["class"].as_str()).unwrap_or("");
        if !class.contains("#32770") || w["visible"].as_bool() == Some(false) { continue; }
        let h = w["hwnd"].as_i64().unwrap_or(0);
        let (mut text, mut buttons): (Vec<String>, Vec<String>) = (vec![], vec![]);
        if let Ok(r) = ab.call("desktop_find_controls", json!({"hwnd": h}), &ctx.caller, 10) {
            let dd = if r["data"].is_object() { r["data"].clone() } else { r };
            for c in dd["controls"].as_array().or(dd["matches"].as_array()).cloned().unwrap_or_default() {
                let name = c["name"].as_str().unwrap_or("").trim().to_string(); if name.is_empty() { continue; }
                let ty = c["controlType"].as_str().or(c["type"].as_str()).or(c["role"].as_str()).unwrap_or("").to_lowercase();
                if ty.contains("button") { buttons.push(name); } else if text.len() < 12 { text.push(name); }
            }
        }
        out.push(json!({"hwnd": h, "title": w["title"], "category": "file_dialog", "resolution": "a native Windows Open/Save dialog (left by a dialog-driven import or a manual action): cancelled safely with WM_CLOSE, nothing is saved or discarded",
            "text": text.join(" ").chars().take(300).collect::<String>(), "buttons": buttons}));
    }
    for d in fusion_core::dialogs::qt_dialog_rows(&rows) {
        let h = d["hwnd"].as_i64().unwrap_or(0);
        let (mut text, mut buttons): (Vec<String>, Vec<String>) = (vec![], vec![]);
        {
            for c in crate::small_verbs::controls_cached(&ab, &ctx.caller, h) {
                let name = c["name"].as_str().unwrap_or("").trim().to_string();
                if name.is_empty() { continue; }
                let ty = c["controlType"].as_str().or(c["type"].as_str()).or(c["role"].as_str()).unwrap_or("").to_lowercase();
                if ty.contains("button") { buttons.push(name); } else { text.push(name); }
            }
        }
        if text.iter().any(|t| t.contains("QTView") || t.contains("ToolbarFramework")) && buttons.is_empty() { continue; } // a Fusion panel (a floating BROWSER titled "Fusion360"), not a dialog
        let known = classified.iter().find(|c| c["hwnd"].as_i64() == Some(h)).cloned();
        let mut e = json!({"hwnd": h, "title": d["title"], "category": known.as_ref().map(|k| k["category"].clone()).unwrap_or(json!("unknown")),
            "resolution": known.as_ref().map(|k| k["resolution"].clone()).unwrap_or(Value::Null),
            "text": text.join(" ").chars().take(600).collect::<String>(), "buttons": buttons});
        if let Some(k) = &known { if !k["closable"].is_null() { e["closable"] = k["closable"].clone(); } if !k["notClosableWhy"].is_null() { e["notClosableWhy"] = k["notClosableWhy"].clone(); } }
        out.push(e);
    }
    out
}

/// Try to expire the dialogs in front of Fusion without a person and without pictures: the
/// known ones through their rule (fusion_dismiss_blocking_dialogs, WM_CLOSE semantics, never a
/// Yes), the unknown ones with WM_CLOSE (the same as pressing Cancel or the X: nothing is saved
/// or discarded by it). Returns what was tried and what is still up.
pub fn expire_dialogs(ctx: &Ctx, before: &[Value]) -> (Vec<Value>, Vec<Value>) {
    let mut tried = Vec::new();
    let Some(ab) = Loopback::from_env() else { return (tried, before.to_vec()) };
    if before.iter().any(|d| d["category"].as_str() != Some("unknown")) {
        let r = crate::small_verbs::dismiss_blocking_dialogs(&ctx.state_dir, &json!({}), &ctx.caller, &ctx.pids, &|_| {});
        tried.push(json!({"step": "fusion_dismiss_blocking_dialogs (known rules)", "dismissed": r["dismissed"]}));
    }
    for d in before.iter().filter(|d| matches!(d["category"].as_str(), Some("unknown") | Some("file_dialog"))) {
        if d["closable"].as_bool() == Some(false) {
            tried.push(json!({"step": "left open (not a dialog)", "hwnd": d["hwnd"], "title": d["title"], "why": d["notClosableWhy"]}));
            continue;
        }
        if let Some(h) = d["hwnd"].as_i64() {
            let r = ab.call("desktop_close_window", json!({"hwnd": h}), &ctx.caller, 10);
            tried.push(json!({"step": "WM_CLOSE (Cancel/X semantics)", "hwnd": h, "title": d["title"], "ok": r.is_ok()}));
        }
    }
    std::thread::sleep(Duration::from_millis(800));
    let after = dialog_inventory(ctx);
    (tried, after)
}

/// When a dialog is in front of Fusion, the reply the gate gives INSTEAD of any restart: first
/// an automatic, safe expiry (rules, then WM_CLOSE); if that clears it the verb simply runs. Only
/// a dialog that survives comes back to the caller, as text and buttons read through UIA first
/// and ab's parent/child screenshots second (issue #87, John: read the text, send the escape,
/// pictures only when that is not enough).
/// What the dialog gate found: nothing, a dialog it expired itself, or one that survived.
pub enum DialogGate { Clear, Expired(Vec<Value>, Vec<Value>), Blocked(Value) }

pub fn blocked_by_dialog_reply(ctx: &Ctx, cmd: &str) -> DialogGate {
    let before = dialog_inventory(ctx);
    // only real dialogs count: Fusion's own tool windows (Data panel, message tray) are owned
    // popups too, and counting them refused every verb for hours on 2026-09-19
    if before.is_empty() { return DialogGate::Clear; }
    let (tried, after) = expire_dialogs(ctx, &before);
    let shot = crate::small_verbs::screenshot_fusion(&json!({}), &ctx.caller);
    let popups = shot["ownedPopupCount"].as_u64().unwrap_or(0);
    crate::fusion_prefs::action_ledger(&ctx.state_dir, "blocked_by_dialog", json!({"forCommand": cmd, "before": before.iter().map(|d| d["title"].clone()).collect::<Vec<_>>(), "tried": tried, "after": after.iter().map(|d| d["title"].clone()).collect::<Vec<_>>(), "ownedPopups": popups}));
    if after.is_empty() {
        eprintln!("[fusion-bridge] {cmd}: expired {} dialog(s) programmatically before running", before.len());
        return DialogGate::Expired(before, tried);
    }
    let titles = after.iter().map(|d| format!("'{}' ({})", d["title"].as_str().unwrap_or(""), d["category"].as_str().unwrap_or("unknown"))).collect::<Vec<_>>().join(", ");
    DialogGate::Blocked(json!({"success": false, "errorCode": "blocked_by_dialog", "hostResponding": true, "dialogs": after, "expiryTried": tried,
        "screenshots": {"mainWindow": shot["savedTo"], "ownedPopupCount": popups, "windows": shot["screenshots"]},
        "error": format!("'{cmd}' cannot run: a dialog is in front of Fusion ({}) and the bridge's safe expiry (rules, then WM_CLOSE) did not clear it. The bridge does not restart Fusion over a dialog.", if titles.is_empty() { format!("{popups} owned popup window(s) with no readable controls") } else { titles }),
        "_hint": "dialogs[] carries each box read through UIA: its text and its button names, plus the bridge's resolution when a rule matched. Answer it by name: desktop_ui_click {hwnd, name:\"<button>\"} for the button that does what you want (never a Save/Yes on a scratch document you do not own), or desktop_close_window {hwnd} to cancel. screenshots.windows is ab's parent/child array for when the text is not enough. Then re-run the verb; never restart Fusion for this."}))
}

pub fn restart_for_addin(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let sd = ctx.state_dir.clone();
    if args["decline"].as_bool() == Some(true) {
        let nid = args["notificationId"].as_str().unwrap_or("page");
        let _ = std::fs::create_dir_all(&sd);
        let _ = std::fs::write(sd.join("user_restart_hold.json"), json!({"at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0), "notification": nid}).to_string());
        crate::fusion_prefs::action_ledger(&sd, "restart_choice", json!({"choice": "declined-from-page"}));
        return json!({"success": true, "declined": true, "hold": true, "_hint": "Recorded: the user owns the restart. Any chain waiting on the notice stops with user_owns_restart; nothing restarts Fusion."});
    }
    if args["wait"].as_bool() != Some(true) && args["_sync"].as_bool() != Some(true) {
        // the background form: the sidecar's own handler carries it (a thread that outlives this call)
        let url = crate::launch::SELF_URL.lock().unwrap().clone().or_else(|| crate::launch::SIDECAR_URL.lock().unwrap().clone());
        let a = args.clone();
        std::thread::spawn(move || {
            if let Some(u) = url {
                let mut a2 = a; a2["wait"] = json!(true);
                let r = ureq::post(&u).timeout(Duration::from_secs(900)).send_json(json!({"command": "fusion_restart_for_addin", "args": a2})).ok().and_then(|r| r.into_json::<Value>().ok()).unwrap_or(json!({}));
                eprintln!("[fusion-bridge] async restart_for_addin finished: {}", json!({"success": r["success"], "errorCode": r["errorCode"], "error": r["error"]}));
            }
        });
        return json!({"success": true, "started": true, "async": true, "statusVerb": "fusion_readiness",
            "_hint": "Restart under way in the background (a Fusion restart takes 60-120s, longer than the relay budget, so this returns immediately). Poll fusion_readiness until ready:true and addinVersion matches; the outcome is also stored in the lastRestartResult preference. A timeout on this verb no longer exists and never meant failure anyway."});
    }
    let t0 = Instant::now();
    let save = args["save"].as_bool().unwrap_or(true);
    let policy = restart_policy(&sd);
    let force = policy == "force";
    let auto_policy = args["auto"].as_bool() == Some(true) || policy == "auto" || force;
    let mut steps: Vec<Value> = Vec::new();
    let mut hard = false;
    let mut graceful = false;
    let mut discarded: Vec<String> = Vec::new();
    if save {
        let mut sv = pass("close_all_documents", json!({"save": true}));
        // issue #129: right after bridge_install the add-in's connection can drop for a moment even
        // though it answered seconds before; one retry before treating the save as impossible
        if sv["success"].as_bool() != Some(true) && save_unreachable(&sv) {
            steps.push(json!({"step": "save", "ok": false, "error": sv["error"].as_str().unwrap_or("").chars().take(160).collect::<String>(), "retry": true}));
            std::thread::sleep(Duration::from_secs(5));
            sv = pass("close_all_documents", json!({"save": true}));
        }
        steps.push(json!({"step": "save", "ok": sv["success"].as_bool().unwrap_or(false), "closed": sv["data"]["closed"], "wouldPrompt": sv["data"]["wouldPrompt"]}));
        let wp: Vec<String> = sv["data"]["wouldPrompt"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
        if !wp.is_empty() {
            let users: Vec<&String> = wp.iter().filter(|n| { let bare = n.replace('*', "").split(" (").next().unwrap_or("").trim().to_lowercase(); !bare.is_empty() && crate::etiquette::bridge_doc_in(&sd, &bare).is_none() && !bare.starts_with("untitled") && !bare.ends_with("_3d") }).collect();
            if users.is_empty() { steps.push(json!({"step": "discard-bridge-scratch", "docs": wp})); hard = true; }
            else if force {
                // restartPolicy force: every open document is the AI's own; a never-saved one is
                // scratch, so it is discarded rather than left to block the restart
                discarded = users.iter().map(|s| s.to_string()).collect();
                crate::fusion_prefs::action_ledger(&sd, "restart_discarded_unsaved", json!({"docs": discarded}));
                steps.push(json!({"step": "discard-under-auto-policy", "docs": discarded})); hard = true;
            }
            else {
                if auto_policy { note_auto_restart_declined(&sd, &json!({"neverSaved": users})); }
                return json!({"success": false, "errorCode": "unsaved_named_document", "error": format!("Cannot restart safely: {} has never been saved, so Fusion would raise its Save box and block the restart.", users.iter().map(|n| fusion_lbr::py_repr_str(n)).collect::<Vec<_>>().join(", ")), "steps": steps,
                    "_hint": "Ask the user where to save it (fusion_save_to_cloud {name, projectName}), then retry. Nothing was closed and no dialog was raised."});
            }
        }
        if sv["success"].as_bool() != Some(true) && !force && crate::addin::addin_status_probe(Duration::from_secs(10)).is_some() {
            // issue #129: the add-in is loaded and answering but could not save. A close now would
            // only meet Fusion's own Unsaved changes box, so stop here: nothing closed, no dialog.
            let why = json!({"save": sv["errorCode"], "error": sv["error"]});
            if auto_policy { note_auto_restart_declined(&sd, &why); }
            crate::fusion_prefs::action_ledger(&sd, "restart_stopped_short", json!({"steps": steps, "why": why}));
            return json!({"success": false, "errorCode": "restart_needs_decision", "restartAborted": true, "steps": steps,
                "error": format!("Fusion was not restarted: the add-in answers but could not save the open documents ({})", sv["error"].as_str().unwrap_or("no reason given").chars().take(200).collect::<String>()),
                "userMessage": "Fusion needs a restart to load the Adom add-in, and the bridge could not save the open documents first. Save or close them, then restart.",
                "_hint": "Nothing was closed and no dialog was raised. Save the documents (fusion_save_document) or close them, then call fusion_restart_for_addin {save:true} again."});
        }
        if sv["success"].as_bool() != Some(true) && auto_policy {
            // the add-in is not loaded at all (it is the thing that is not answering). Close Fusion the way
            // a person would: it closes at once when nothing is unsaved, and a save prompt for
            // someone's work stops the restart instead of a kill losing it (force: kill, as before)
            steps.push(json!({"step": "save-unavailable", "error": sv["error"].as_str().unwrap_or("").chars().take(160).collect::<String>()}));
            if force { hard = true; } else { graceful = true; }
        }
        if sv["success"].as_bool() != Some(true) && !auto_policy && sv["errorCode"].is_string() && sv["error"].as_str().map(|e| e.contains("Add-in request failed")).unwrap_or(false) {
            return json!({"success": false, "errorCode": "save_failed", "steps": steps, "error": format!("Refusing to restart because saving failed: {}", sv["error"].as_str().unwrap_or("").chars().take(200).collect::<String>()), "_hint": "Nothing was restarted; the user's documents are untouched."});
        }
    }
    if hard || !save { let st = kill(ctx, &json!({"reason": "restart for add-in; only bridge scratch unsaved"})); steps.push(json!({"step": "kill", "ok": st["success"].as_bool().unwrap_or(true)})); }
    else {
        let st = stop(ctx, &json!({"skipCleanClose": graceful}));
        steps.push(json!({"step": "stop", "ok": st["success"].as_bool().unwrap_or(true), "errorCode": st["errorCode"]}));
        if st["success"].as_bool() == Some(false) && !force {
            // a stop that did not finish is never escalated to a kill (start() used to do exactly
            // that on a Fusion still running): the work it stopped for is someone's to decide on
            let why = json!({"stop": st["errorCode"], "unsavedWork": st["unsavedWork"], "error": st["error"]});
            if auto_policy { note_auto_restart_declined(&sd, &why); }
            crate::fusion_prefs::action_ledger(&sd, "restart_stopped_short", json!({"steps": steps, "why": why}));
            return json!({"success": false, "errorCode": "restart_needs_decision", "restartAborted": true, "steps": steps, "unsavedWork": st["unsavedWork"],
                "error": format!("Fusion was not restarted: {}", st["error"].as_str().unwrap_or("it did not close")),
                "userMessage": "Fusion needs a restart to load the Adom add-in, and it has unsaved changes the bridge will not discard. Save or close them, then restart.",
                "_hint": "Ask the user (userMessage). Nothing was killed or discarded. When the changes are saved, call fusion_restart_for_addin {save:true} again."});
        }
    }
    std::thread::sleep(Duration::from_secs(3));
    let lr = crate::launch::start(ctx, &json!({"waitForAddin": true}));
    steps.push(json!({"step": "start", "ok": lr["success"].as_bool().unwrap_or(false)}));
    let ready = crate::launch::round_trip_ready_gap(6, 5.0);
    if auto_policy {
        let why = args["why"].as_str().unwrap_or("to load the Adom add-in");
        let body = restart_summary(why, &policy, &steps, &discarded);
        // the ledger is the record; no toast (John, 2026-09-19: "fairly pointless")
        crate::fusion_prefs::action_ledger(&sd, "auto_restart_done", json!({"ready": ready, "why": why, "steps": steps, "discarded": discarded, "summary": body}));
    }
    json!({"success": ready, "addinReady": ready, "restartPolicy": if force { "force" } else if auto_policy { "auto" } else { "ask" }, "discarded": discarded, "tookSec": (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0, "steps": steps, "errorCode": if ready { Value::Null } else { json!("addin_still_not_responding") },
        "_hint": if ready { "Fusion restarted and the add-in answered - re-run the verb that failed." } else { "Fusion restarted but the add-in still is not answering. That is a real defect, not a user problem: check fusion_addin_status and file it." }})
}

/// `_diagnose_addin_timeout`: after a command timed out, say WHY (busy, blocked by a modal,
/// hung host, or a dead add-in), with screenshots of what is up.
/// `_post_open_screenshot`: settle, then the main window and every dialog, plus the blocked flag.
pub fn post_open_screenshot(ctx: &Ctx, settle_secs: f64) -> (Value, bool) {
    std::thread::sleep(Duration::from_secs_f64(settle_secs));
    let blocked = !crate::launch::round_trip_ready(1);
    let mut shots = json!({"screenshots": []});
    let main = crate::small_verbs::screenshot_fusion(&json!({}), &ctx.caller);
    if main["success"].as_bool() == Some(true) { shots["screenshots"].as_array_mut().unwrap().push(json!({"type": "main_window", "savedTo": main["savedTo"], "sizeKB": main["sizeKB"]})); }
    else { shots["error"] = json!(main["error"].as_str().unwrap_or("Screenshot failed")); }
    if let Some(ab) = Loopback::from_env() {
        let rows = ab.call("desktop_enum_windows", json!({"pids": ctx.pids}), &ctx.caller, 10).ok().map(|v| v["windows"].as_array().or_else(|| v["data"]["windows"].as_array()).cloned().unwrap_or_default()).unwrap_or_default();
        for d in fusion_core::dialogs::qt_dialog_rows(&rows) {
            let (w, h) = (d["rect"]["width"].as_i64().unwrap_or(0), d["rect"]["height"].as_i64().unwrap_or(0));
            if w < 50 || h < 50 { continue; }
            if let Some(hw) = d["hwnd"].as_i64() {
                if let Ok(r) = ab.call("desktop_screenshot_window", json!({"hwnd": hw}), &ctx.caller, 40) {
                    if let Some(p) = r["fullPath"].as_str().or(r["safePath"].as_str()) { shots["screenshots"].as_array_mut().unwrap().push(json!({"type": "dialog", "title": d["title"], "hwnd": hw, "savedTo": p.replace('\\', "/"), "sizeKB": (r["fullBytesHost"].as_f64().unwrap_or(0.0) / 1024.0 * 10.0).round() / 10.0})); }
                }
            }
        }
    }
    let n = shots["screenshots"].as_array().map(|a| a.len()).unwrap_or(0);
    shots["message"] = json!(format!("Auto-captured {n} Fusion window(s) after open. READ EACH screenshot file to check for blocking dialogs ('What to design?', 'PCB out of date', 'Save changes?'). PCB/ELECTRONICS NAV RULE: a board is NOT a standalone file - the schematic, 2D board and 3D board are VIEWS inside ONE electronics DESIGN. Open the DESIGN document first (fusion_open_cloud_file or fusion_open_by_urn), THEN switch to its schematic and its 2D board (fusion_show_2d_board) and 3D board (fusion_show_3d_board). A Select-Electronics-Design-File picker IS that design list of its schematic + board. An EMPTY 2D board usually means you opened a derivative or the wrong file, or it is out of sync - reopen the parent electronics DESIGN and switch to its board view; do NOT open a .brd or .sch as a standalone file. Dismiss with fusion_dismiss_blocking_dialogs if needed."));
    if blocked { shots["dialogBlocking"] = json!(true); shots["message"] = json!(format!("WARNING: A modal dialog is blocking Fusion. {}", shots["message"].as_str().unwrap_or(""))); }
    (shots, blocked)
}

pub fn diagnose_addin_timeout(ctx: &Ctx, command: &str, original: &Value) -> Value {
    let _ = original;
    let (shots, _blocked) = post_open_screenshot(ctx, 2.0);
    let mut blocking: Vec<Value> = Vec::new();
    if let Some(ab) = Loopback::from_env() {
        blocking = crate::small_verbs::classify_blocking_dialogs(&ab, &ctx.caller, &ctx.pids);
        // `_license_classify_extend`: the seat modal by ownership and shape when the title is bare
        if !blocking.iter().any(|d| d["category"].as_str() == Some("license_sessions_exceeded")) {
            if let Some(dlg) = fusion_core::windows_facts::facts_from_ab(&ab, &ctx.caller).licensing_dialog {
                let auto = crate::small_verbs::get_pref(&ctx.state_dir, "autoTakeOverSeat").map(|v| v != json!(false)).unwrap_or(true);
                blocking.push(json!({"category": "license_sessions_exceeded", "title": dlg["title"], "hwnd": dlg["hwnd"],
                    "resolution": format!("Autodesk seat modal: this account's Fusion is active on another machine. {}", if auto { "Auto-takeover is ON - the bridge clicks Continue through both dialogs in the background (~20s sweep; a fusion_start resolves it immediately). The other machine's Fusion suspends RESUMABLY." } else { "Auto-takeover is OPTED OUT (fusion_set_seat_takeover) - resolve the dialog yourself or re-enable takeover." })}));
            }
        }
    }
    let health: Option<Value> = ureq::get("http://127.0.0.1:8774/health").timeout(Duration::from_secs(2)).call().ok().and_then(|r| r.into_json().ok());
    if let Some(h) = &health {
        if h["status"].as_str() == Some("ok") {
            let main_thread = h["main_thread"].as_str().unwrap_or("unknown").to_string();
            let pending = h["pending_commands"].as_i64().unwrap_or(0);
            if main_thread == "blocked" || pending > 0 {
                if let Some(st) = crate::addin::addin_status_probe(Duration::from_secs(3)).filter(|s| s["busy"].as_bool() == Some(true)) {
                    let mut resp = json!({"success": false, "error": format!("Fusion main thread busy — {} running for {}s.", st["busyCommand"].as_str().unwrap_or("unknown"), st["elapsedSeconds"]), "errorCode": "main_thread_busy", "busyCommand": st["busyCommand"], "elapsedSeconds": st["elapsedSeconds"],
                        "_hint": "Add-in is busy with a long-running command. Do NOT retry add-in commands. Do NOT press Escape — the add-in is working, not stuck on a dialog. Commands that still work: fusion_window_info, fusion_screenshot_fusion, fusion_close_window."});
                    if !st["walkProgress"].is_null() { resp["progress"] = st["walkProgress"].clone(); }
                    return resp;
                }
                // Issue #87: a modal that blocks the add-in is handed to the caller as pictures
                // (the main window plus every owned child window) and classified rows; the bridge
                // never restarts Fusion over a dialog. The 90 s auto-restart of 1.10.6 is gone.
                let inv = dialog_inventory(ctx);
                let (tried, after) = expire_dialogs(ctx, &inv);
                let popups = crate::small_verbs::screenshot_fusion(&json!({}), &ctx.caller);
                if !inv.is_empty() && after.is_empty() {
                    crate::fusion_prefs::action_ledger(&ctx.state_dir, "dialog_expired_after_timeout", json!({"forCommand": command, "dialogs": inv.iter().map(|d| d["title"].clone()).collect::<Vec<_>>(), "tried": tried}));
                    return json!({"success": false, "errorCode": "dialog_expired_retry", "dialogsExpired": inv, "expiryTried": tried,
                        "error": format!("'{command}' timed out behind a dialog ({}); the bridge expired it programmatically (rules, then WM_CLOSE). Re-run the verb.", inv.iter().map(|d| d["title"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join(", ")),
                        "_hint": "The dialog is gone and Fusion answers again: re-run the same verb now. Nothing was restarted."});
                }
                let auto_note = " The bridge will NOT restart Fusion over a dialog: dialogs[] carries each box's text and buttons read through UIA; answer it by button name (desktop_ui_click {hwnd, name}) or cancel it (desktop_close_window {hwnd}), then re-run the verb.".to_string();
                let blocking = if after.is_empty() { blocking } else { after.clone() };
                let _ = &popups;
                let message = if !blocking.is_empty() {
                    let titles = blocking.iter().map(|d| format!("'{}' ({})", d["title"].as_str().unwrap_or(""), d["category"].as_str().unwrap_or(""))).collect::<Vec<_>>().join(", ");
                    let mut seen = Vec::new(); for d in &blocking { let r = d["resolution"].as_str().unwrap_or("").to_string(); if !seen.contains(&r) { seen.push(r); } }
                    format!("The AdomBridge add-in is alive but its main thread is blocked by a modal dialog: {titles}.{auto_note} {}", seen.join(" "))
                } else {
                    format!("The AdomBridge add-in is alive but its main thread is not responding (status: {main_thread}, pending: {pending}). Fusion 360 likely has a modal dialog open (Document Recovery, error, or update prompt) that is blocking execution. READ the screenshots to identify the dialog, then fusion_dismiss_blocking_dialogs answers it in the background.")
                };
                return json!({"success": false, "error": "addin_main_thread_blocked", "message": message, "blockingDialogs": blocking, "data": {"command": command, "health": h, "blockingDialogs": blocking, "postOpenScreenshot": shots}});
            }
            *BLOCKED_SINCE.lock().unwrap() = 0.0;
            return json!({"success": false, "error": "addin_command_timeout", "message": format!("Command '{command}' timed out but the add-in reports main thread is {main_thread}. The command may be long-running or stuck."), "data": {"command": command, "health": h, "postOpenScreenshot": shots}});
        }
    }
    if let Some(st) = crate::addin::addin_status_probe(Duration::from_secs(3)).filter(|s| s["busy"].as_bool() == Some(true)) {
        return json!({"success": false, "error": "The add-in is BUSY running a long command, not dead - your command is queued behind it.", "errorCode": "fusion_addin_busy", "busyCommand": st["busyCommand"], "elapsedSeconds": st["elapsedSeconds"], "hostResponding": true,
            "_hint": "Do NOT restart Fusion and do not treat this as a crash. Fusion runs one command at a time on its main thread, and some surfaces genuinely take minutes (a staged first-open has measured 546s here). WAIT and re-poll - fusion_state answers while the add-in is busy and reports mainThreadBusy with the running command. Only treat it as broken if elapsedSeconds keeps climbing past what fusion_state.timings says this machine has ever taken."});
    }
    let hung = crate::readiness_exec::confirmed_hung(ctx);
    if matches!(hung["phase"].as_str(), Some("suspect") | Some("busy-not-hung") | Some("long-op-in-flight")) {
        return json!({"success": false, "error": "Fusion's main thread is not answering right now - most likely WORKING on something heavy, not hung (the probes it starves are the very ones this gate reads).", "errorCode": "fusion_addin_busy", "hostResponding": hung["phase"].as_str() == Some("busy-not-hung"), "hungHeldSeconds": hung["heldSeconds"], "keptBy": hung["keptBy"],
            "_hint": "WAIT and re-poll. A real hang is only declared after 180s of continuous silence with Fusion idle (not using CPU), including a 20s probe, and then the bridge restarts Fusion itself."});
    }
    if hung["hung"].as_bool() == Some(true) {
        let _ = kill(ctx, &json!({})); std::thread::sleep(Duration::from_secs(6)); let _ = crate::launch::start(ctx, &json!({}));
        let ghost = hung["signals"].as_array().map(|a| a.iter().any(|s| s.as_str() == Some("GhostWindow"))).unwrap_or(false);
        return json!({"success": false, "error": "Fusion 360 itself is NOT RESPONDING (the host process stopped pumping its message queue), so the add-in cannot answer.", "errorCode": "fusion_host_hung", "hungSignals": hung["signals"], "ghostWindow": ghost, "restartAttempted": true,
            "_hint": "This is NOT a blocking dialog and NOT an add-in crash - do not go looking for a modal, there is not one. Nothing reaches Fusion over IPC while it is hung; only a restart clears it. The bridge already force-restarted it (kill + start) - poll fusion_readiness until ready:true."});
    }
    // a timeout from an add-in that answered inside this Fusion (or holds another of our
    // requests) is a busy add-in, not a crashed one: never tell the caller to restart for it
    if let Some(ev) = crate::addin::loaded_evidence(&ctx.pids) { return crate::launch::busy_not_missing(&ctx.state_dir, command, &ev); }
    json!({"success": false, "error": "AdomBridge add-in not responding (it may have crashed).", "errorCode": "fusion_addin_not_responding", "hostResponding": true,
        "_hint": "Fix it YOURSELF - never ask the user: restart Fusion via fusion_stop + fusion_start (the bridge installs the add-in to ALL Fusion add-in dirs incl. %APPDATA%\\Autodesk\\FusionAddins, and runOnStartup reloads it). The host process IS responding - this is the add-in only, so no dialog is implied."})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_a_connection_that_never_reached_fusion_is_retried() {
        assert!(save_unreachable(&json!({"success": false, "error": "Add-in request failed: Network Error: A connection attempt failed"})));
        assert!(!save_unreachable(&json!({"success": false, "error": "Add-in request failed: non-JSON answer"})));
        assert!(!save_unreachable(&json!({"success": false, "error": "could not save lane-BQ25792-copy"})));
    }
    #[test]
    fn a_save_prompt_is_answered_only_for_the_bridges_own_document() {
        assert_eq!(prompt_answer(true), "dont_save");
        assert_eq!(prompt_answer(false), "cancel_and_stop");
    }
    #[test]
    fn the_restart_summary_says_what_happened() {
        // 2026-10-28: the save step failed and Fusion was killed, yet the ledger said "saved first"
        let failed_then_killed = [json!({"step": "save", "ok": false}), json!({"step": "save-unavailable"}), json!({"step": "kill", "ok": true}), json!({"step": "start", "ok": true})];
        let s = restart_summary("to load the Adom add-in", "force", &failed_then_killed, &[]);
        assert!(s.contains("Nothing could be saved first, and Fusion was force-closed") && !s.contains("were saved first"), "{s}");
        let saved = [json!({"step": "save", "ok": true}), json!({"step": "stop", "ok": true})];
        assert!(restart_summary("x", "auto", &saved, &[]).contains("Documents were saved first"));
        let graceful = [json!({"step": "save", "ok": false}), json!({"step": "stop", "ok": true})];
        assert!(restart_summary("x", "auto", &graceful, &[]).contains("the way a person closes it"));
    }
    #[test]
    fn a_declined_auto_restart_turns_into_the_ask_for_a_while() {
        let d = std::env::temp_dir().join(format!("fb-restart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert!(!auto_restart_declined_recently(&d));
        note_auto_restart_declined(&d, &json!({"unsavedWork": [{"prompt": "Save changes to Bracket?"}]}));
        assert!(auto_restart_declined_recently(&d));
        let _ = std::fs::write(d.join("auto_restart_declined.json"), json!({"at": now_secs() - 1900.0}).to_string());
        assert!(!auto_restart_declined_recently(&d));
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn the_policy_defaults_to_auto_and_names_force() {
        let d = std::env::temp_dir().join(format!("fb-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        if std::env::var("ADOM_FUSION_RESTART_POLICY").is_err() {
            assert_eq!(restart_policy(&d), "auto");
            crate::small_verbs::set_pref(&d, "restartPolicy", json!("Force"));
            assert_eq!(restart_policy(&d), "force");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}

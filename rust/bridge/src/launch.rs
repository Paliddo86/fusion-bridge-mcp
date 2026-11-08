//! `fusion_start` and the add-in preamble, native. Ported from server.py's `_handle_launch`
//! and `_addin_preamble`: the warm path (busy guard, the user's restart hold, the persistence
//! gate), the breakaway launch through `HostProcess::launch_fusion`, the launch-dialog sweep,
//! the seat resolver and recovery dismissal (sidecar executors, UIA and Win32 work the exe
//! does not do itself), window sizing through ab, the add-in wait with measured progress, and
//! the launch timings. `gate` is the preamble every add-in verb passes before it is sent.

use serde_json::{json, Value};
use std::time::{Duration, Instant};

use crate::addin::Ctx;
use fusion_core::ab::{AbClient, Loopback};

fn now_f() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

fn health_probe(timeout: Duration) -> bool {
    let ok = ureq::get("http://127.0.0.1:8774/health").timeout(timeout).call().is_ok();
    if ok { crate::addin::note_answered(); }
    ok
}

static BUSY_NOTED_AT: std::sync::Mutex<f64> = std::sync::Mutex::new(0.0);

/// The reply for a silent add-in that is loaded and working, ledgered at most once a minute.
pub fn busy_not_missing(state_dir: &std::path::Path, cmd: &str, ev: &Value) -> Value {
    let mut at = BUSY_NOTED_AT.lock().unwrap();
    if now_f() - *at > 60.0 {
        *at = now_f();
        crate::fusion_prefs::action_ledger(state_dir, "addin_busy_not_restarted", json!({"forCommand": cmd, "evidence": ev}));
    }
    let mut r = json!({"success": false, "errorCode": "fusion_addin_busy", "hostResponding": true, "addinLoaded": true,
        "error": "The Adom add-in is loaded but not answering right now: it is working. A long command or a heavy Fusion operation holds its main thread and starves even its status probe. It was not installed while Fusion was running, so a restart would not help.",
        "_hint": "WAIT and re-poll fusion_readiness; do not restart Fusion (a restart closes every open document and loses the work in flight). The add-in answers again when the operation in front of it finishes."});
    if let Some(o) = ev.as_object() { for (k, v) in o { r[k] = v.clone(); } }
    r
}

/// `wait_for_addin(timeout)`: the health endpoint, once a second.
pub fn wait_for_addin(seconds: u64) -> bool {
    for _ in 0..seconds { if health_probe(Duration::from_secs(1)) { return true; } std::thread::sleep(Duration::from_secs(1)); }
    false
}

/// `_addin_round_trip_ready(tries)`: a real get_app_state, `tries` times in a row.
pub fn round_trip_ready(tries: u32) -> bool { round_trip_ready_gap(tries, 1.5) }
pub fn round_trip_ready_gap(tries: u32, gap_secs: f64) -> bool {
    for i in 0..tries {
        let ok = ureq::post("http://127.0.0.1:8774/command").timeout(Duration::from_secs(6)).send_json(json!({"command": "get_app_state", "args": {}})).ok()
            .and_then(|r| r.into_json::<Value>().ok()).map(|v| v["success"].as_bool().unwrap_or(false)).unwrap_or(false);
        if !ok { return false; }
        if i + 1 < tries { std::thread::sleep(Duration::from_secs_f64(gap_secs)); }
    }
    true
}

fn running(ctx: &Ctx) -> bool { !ctx.host.fusion_processes().is_empty() }

/// `_user_restart_hold_active`.
pub fn user_restart_hold_active(state_dir: &std::path::Path) -> bool {
    let f = state_dir.join("user_restart_hold.json");
    let Ok(t) = std::fs::read_to_string(&f) else { return false };
    let at = serde_json::from_str::<Value>(&t).ok().and_then(|v| v["at"].as_f64()).unwrap_or(0.0);
    if now_f() - at > 4.0 * 3600.0 { let _ = std::fs::remove_file(&f); return false; }
    if round_trip_ready(1) { let _ = std::fs::remove_file(&f); return false; }
    true
}

fn timings_launch(state_dir: &std::path::Path) -> Value { fusion_core::readiness::timings_summary(&crate::timings::load(state_dir))["launch"].clone() }

/// `timings.sentence()`.
pub fn timings_sentence(state_dir: &std::path::Path) -> Option<String> {
    let s = timings_launch(state_dir);
    let bits: Vec<String> = ["cold", "warm"].iter().filter_map(|k| s[k]["p50"].as_f64().map(|p| format!("{k} {}s", p as i64))).collect();
    if bits.is_empty() { None } else { Some(format!("on this machine Fusion starts in {} (measured)", bits.join(", "))) }
}

/// `timings.terminal_block`.
fn terminal_block(started: f64, label: &str, start_kind: &str, failed: bool) -> Value {
    let elapsed = (now_f() - started).max(0.0);
    json!({"phase": "launch", "stepLabel": label, "percent": if failed { Value::Null } else { json!(100) }, "elapsedSec": elapsed.round() as i64, "estimatedSec": elapsed.round() as i64,
        "etaSec": if failed { Value::Null } else { json!(0) }, "confidence": "measured", "startKind": start_kind, "failed": failed,
        "measured": {"phase": "launch", "seconds": (elapsed * 10.0).round() / 10.0, "startKind": start_kind}})
}

fn busy_status() -> Option<Value> { crate::addin::addin_status_probe(Duration::from_secs(3)).filter(|s| s["busy"].as_bool() == Some(true)) }

fn busy_return(st: &Value, hint: &str, error: &str) -> Value {
    json!({"success": false, "error": error, "errorCode": "fusion_addin_busy", "busyCommand": st["busyCommand"], "elapsedSeconds": st["elapsedSeconds"], "alreadyRunning": true, "startKind": "warm", "_hint": hint})
}

/// `ensure_reasonable_window`: grow a small Fusion window to 92% of its monitor's work area,
/// centred, without activating it (ab's desktop_set_window_bounds with restore:false).
fn ensure_reasonable_window(ctx: &Ctx) -> Value {
    let Some(ab) = Loopback::from_env() else { return json!({"sized": false, "why": "no ab loopback"}) };
    let facts = fusion_core::windows_facts::facts_from_ab(&ab, &ctx.caller);
    let Some(h) = facts.main_hwnd else { return json!({"sized": false, "why": "no fusion window"}) };
    let main = facts.family.iter().find(|w| w["hwnd"].as_i64() == Some(h)).cloned().unwrap_or(json!({}));
    if main["maximized"].as_bool() == Some(true) || main["state"].as_str() == Some("maximized") { return json!({"sized": false, "why": "already maximized (user's choice)"}); }
    let rect = fusion_core::dialogs::rect_of(&main["rect"]);
    let (cur_w, cur_h, left, top) = (rect["width"].as_i64().unwrap_or(0), rect["height"].as_i64().unwrap_or(0), rect["left"].as_i64().unwrap_or(0), rect["top"].as_i64().unwrap_or(0));
    let Ok(disp) = ab.call("display", json!({}), &ctx.caller, 10) else { return json!({"sized": false, "why": "display query failed"}) };
    let mons = disp["monitors"].as_array().cloned().unwrap_or_default();
    let cx = left + cur_w / 2; let cy = top + cur_h / 2;
    let mon = mons.iter().find(|m| { let b = &m["bounds"]; let (x, y, w, hh) = (b["x"].as_i64().unwrap_or(0), b["y"].as_i64().unwrap_or(0), b["width"].as_i64().unwrap_or(0), b["height"].as_i64().unwrap_or(0)); cx >= x && cx < x + w && cy >= y && cy < y + hh }).or_else(|| mons.iter().find(|m| m["primary"].as_bool() == Some(true))).cloned().unwrap_or(json!({}));
    let wa = if mon["workArea"].is_object() { mon["workArea"].clone() } else { mon["bounds"].clone() };
    let (wa_x, wa_y, wa_w, wa_h) = (wa["x"].as_i64().unwrap_or(0), wa["y"].as_i64().unwrap_or(0), wa["width"].as_i64().unwrap_or(0), wa["height"].as_i64().unwrap_or(0));
    if wa_w <= 0 || wa_h <= 0 { return json!({"sized": false, "why": "GetMonitorInfo failed"}); }
    if (cur_w * cur_h) as f64 >= 0.60 * (wa_w * wa_h) as f64 { return json!({"sized": false, "why": "already large enough", "current": [cur_w, cur_h], "workArea": [wa_w, wa_h]}); }
    let (new_w, new_h) = ((wa_w as f64 * 0.92) as i64, (wa_h as f64 * 0.92) as i64);
    let (x, y) = (wa_x + (wa_w - new_w) / 2, wa_y + (wa_h - new_h) / 2);
    let ok = ab.call("desktop_set_window_bounds", json!({"hwnd": h, "x": x, "y": y, "w": new_w, "h": new_h, "restore": false}), &ctx.caller, 10).is_ok();
    json!({"sized": ok, "from": [cur_w, cur_h], "to": [new_w, new_h], "workArea": [wa_w, wa_h], "hwnd": h, "why": "grew to 92% of the work area without activating it"})
}

/// The taskbar launch bar after a stillLaunching answer (`_start_launch_progress_watch`).
fn start_launch_progress_watch(ctx: &Ctx) {
    let mode = crate::small_verbs::get_pref(&ctx.state_dir, "activityIndicator").and_then(|v| v.as_str().map(|s| s.to_lowercase())).unwrap_or_else(|| "progress".into());
    if mode != "progress" && mode != "both" { return; }
    static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if ACTIVE.swap(true, std::sync::atomic::Ordering::Relaxed) { return; }
    let (sd, caller) = (ctx.state_dir.clone(), ctx.caller.clone());
    std::thread::spawn(move || {
        let started = crate::small_verbs::get_pref(&sd, "launchStartedAt").and_then(|v| v.as_f64()).unwrap_or_else(now_f);
        let (est, _) = { let st = fusion_core::readiness::timings_stats(&crate::timings::load(&sd), "launch", "cold"); match (st["count"].as_u64().unwrap_or(0), st["p50"].as_f64()) { (c, Some(p)) if c >= 2 && p > 0.0 => (p, "measured"), (1, Some(p)) if p > 0.0 => (p, "typical"), _ => (150.0, "typical") } };
        let give_up = started + 2.0 * est + 120.0;
        while now_f() < give_up {
            if round_trip_ready(1) { crate::timings::taskbar_progress(&caller, "normal", Some(100.0)); std::thread::sleep(Duration::from_secs(1)); crate::timings::taskbar_progress(&caller, "none", None); ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed); return; }
            crate::timings::taskbar_progress(&caller, "normal", Some((95.0 * (now_f() - started) / est).min(95.0)));
            std::thread::sleep(Duration::from_secs(2));
        }
        crate::timings::taskbar_progress(&caller, "error", Some(100.0)); std::thread::sleep(Duration::from_millis(1500)); crate::timings::taskbar_progress(&caller, "none", None);
        ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    });
}

fn not_installed() -> Value {
    json!({"success": false, "error": "Fusion 360 is not installed on this machine.", "errorCode": "fusion_not_installed",
        "_hint": "Fusion 360 isn't installed. Do NOT tell the user to install it themselves - OFFER to install it FOR them and do it on a yes: the fusion-onboarding skill silent-installs Fusion + drives the Autodesk sign-in. (For a plain 'what EDA tools are installed' check, ab's bridge_readiness reports this cleanly without erroring.)"})
}

/// `fusion_start`.
pub fn start(ctx: &Ctx, _args: &Value) -> Value {
    if _args["_fromGate"].is_null() { let _ = std::fs::remove_file(autostart_path(&ctx.state_dir)); }
    if !running(ctx) {
        if let Some(problem) = ctx.fusion_exe.as_deref().and_then(fusion_core::install_check::launch_problem) { return broken_install_refusal(&problem); }
    }
    let sd = ctx.state_dir.clone();
    let Some(exe) = ctx.fusion_exe.clone() else { return not_installed() };
    let exe_path = exe.to_string_lossy().replace('\\', "/");
    let warm_start = now_f();
    if running(ctx) {
        let mut addin_ok = wait_for_addin(5);
        if addin_ok { crate::timings::record(&sd, "launch", now_f() - warm_start, Some("warm")); }
        if !addin_ok {
            if let Some(st) = busy_status() {
                return busy_return(&st, "Do NOT call fusion_start again on this - it is exactly how a healthy run gets killed. Poll fusion_readiness (it answers while the add-in is busy) and compare elapsedSeconds against fusion_state.timings for this machine; only past roughly 2x p90 is a restart justified.",
                    "Fusion is running and the add-in is BUSY with a long command - NOT restarting it. Waiting is the remedy here, not a relaunch.");
            }
            if user_restart_hold_active(&sd) {
                return json!({"success": false, "errorCode": "user_owns_restart", "alreadyRunning": true, "startKind": "warm",
                    "error": "Fusion is running without its add-in, and the user chose 'I'll handle it' on the restart notice - so the bridge will NOT restart Fusion. The user owns that restart now.",
                    "_hint": "Do not call fusion_start or fusion_restart_for_addin. Wait for the user to restart Fusion themselves (the hold releases the moment the add-in answers), or ask them again with fusion_restart_for_addin only if THEY change their mind."});
            }
            crate::fusion_prefs::action_ledger(&sd, "fusion_restart_for_silent_addin_considered", json!({"reason": "running but add-in not answering; entering persistence gate"}));
            for _ in 0..3 {
                std::thread::sleep(Duration::from_secs(8));
                if wait_for_addin(5) { addin_ok = true; break; }
                if let Some(st) = busy_status() {
                    return busy_return(&st, "Do NOT call fusion_start again - poll fusion_readiness and wait; a busy add-in answers again when its command finishes.",
                        "Fusion is running and the add-in reported BUSY during the persistence re-probe - NOT restarting it.");
                }
            }
        }
        if !addin_ok {
            // silent is not missing: an add-in that answered inside this Fusion, or holds one of
            // our requests, is working, and a kill would lose that work and every open document
            if let Some(ev) = crate::addin::loaded_evidence(&ctx.pids) {
                let mut r = busy_not_missing(&sd, "start", &ev);
                r["alreadyRunning"] = json!(true); r["startKind"] = json!("warm");
                return r;
            }
            eprintln!("[fusion-bridge] warm start found NO add-in after a ~30s persistence window (never busy, never answered) - restarting Fusion so the add-in loads (issue #39, #41 busy guard, + the 2026-08-22 library-matrix persistence gate)");
            let _ = crate::lifecycle::kill(ctx, &json!({}));
            std::thread::sleep(Duration::from_secs(3));
        } else {
            return json!({"success": true, "output": "Fusion 360 is already running.", "addinConnected": true, "alreadyRunning": true, "startKind": "warm",
                "progress": terminal_block(warm_start, "Fusion is already running - reusing it", "warm", false), "timings": timings_launch(&sd), "resolvedPath": exe_path});
        }
    }
    let reloc = crate::readiness_exec::relocate_recovery_files();
    let reloc_moved = reloc["moved"].as_i64().unwrap_or(0);
    let start = warm_start;
    crate::small_verbs::set_pref(&sd, "launchStartedAt", json!(start));
    // ensure_fusion_running (wait_addin false): the breakaway launch, then the process must appear
    if !running(ctx) {
        if let Err(e) = ctx.host.launch_fusion() { return json!({"success": false, "error": format!("Failed to launch Fusion 360: {e:?}")}); }
        ctx.host.background_envelope(150.0, true);
        let mut appeared = false;
        for _ in 0..60 { std::thread::sleep(Duration::from_millis(500)); if running(ctx) { appeared = true; break; } }
        if !appeared {
            let mut launch_err: Option<Value> = None;
            if let Some(ab) = Loopback::from_env() {
                for d in crate::small_verbs::classify_blocking_dialogs(&ab, &ctx.caller, &[]) {
                    if d["category"].as_str() == Some("launch_error") || d["title"].as_str().unwrap_or("").to_lowercase().contains("streamed application") {
                        if let Some(h) = d["hwnd"].as_i64() { let _ = ab.call("desktop_close_window", json!({"hwnd": h}), &ctx.caller, 10); }
                        launch_err = Some(d);
                    }
                }
            }
            let complete = { #[cfg(windows)] { fusion_os_win::any_app_complete() } #[cfg(not(windows))] { false } };
            if complete {
                return json!({"success": false, "error": format!("Fusion is INSTALLED (Fusion360.exe present) but did not start within 30s{}.", if launch_err.is_some() { " - a launch-error dialog was dismissed" } else { "" }),
                    "errorCode": "fusion_launch_failed", "installed": true, "dialog": launch_err.as_ref().map(|d| d["title"].clone()).unwrap_or(Value::Null),
                    "_hint": "The install is COMPLETE - do NOT re-stream. Retry fusion_start (it now picks the FusionLauncher.exe that has its .ini beside it; a stale lock from a just-killed instance also clears on a retry). If it keeps failing, fusion_kill then wait a few seconds before fusion_start."});
            }
            let removed = json!(crate::host().clean_incomplete_webdeploy());
            return json!({"success": false, "error": "Fusion launch failed: the streamed install is INCOMPLETE (no Fusion360.exe yet).", "errorCode": "fusion_incomplete_install",
                "cleanedWebdeployDirs": removed, "dialog": launch_err.as_ref().map(|d| d["title"].clone()).unwrap_or(Value::Null),
                "_hint": "AUTO-RECOVERED: dismissed the launch-error dialog and removed the partial webdeploy dir(s). Re-run fusion_install_fusion, then WAIT for fusion_readiness to report installed:true BEFORE fusion_start - starting mid-stream is what corrupts the install."});
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    // launch dialogs: the benign acks are closed
    if let Some(ab) = Loopback::from_env() {
        let auto = fusion_core::dialogs::category_set("AUTO_DISMISS");
        for d in crate::small_verbs::classify_blocking_dialogs(&ab, &ctx.caller, &[]) {
            if d["category"].as_str().map(|c| auto.iter().any(|a| a == c)).unwrap_or(false) { if let Some(h) = d["hwnd"].as_i64() { let _ = ab.call("desktop_close_window", json!({"hwnd": h}), &ctx.caller, 10); } }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(((50.0 - (now_f() - start)).max(6.0)) as u64);
    let mut seat_clicks: i64 = 0;
    let mut addin_ok = false;
    while Instant::now() < deadline {
        let lic = Loopback::from_env().map(|ab| fusion_core::windows_facts::facts_from_ab(&ab, &ctx.caller).licensing_dialog.is_some()).unwrap_or(false);
        if lic {
            if seat_clicks < 10 { seat_clicks += crate::readiness_exec::resolve_seat_dialog(ctx, 3)["clicks"].as_i64().unwrap_or(0); }
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        if health_probe(Duration::from_millis(1500)) && round_trip_ready(2) {
            addin_ok = true;
            if crate::small_verbs::get_pref(&sd, "sizeFusionWindow").map(|v| v != json!(false)).unwrap_or(true) {
                let sz = ensure_reasonable_window(ctx);
                if sz["sized"].as_bool() == Some(true) { eprintln!("[fusion-bridge] window sized: {}", sz["why"].as_str().unwrap_or("")); }
            }
            break;
        }
        let _ = crate::readiness_exec::dismiss_recovery_dialog(ctx, 5.0, true);
        std::thread::sleep(Duration::from_millis(2500));
    }
    if !addin_ok {
        let stats = fusion_core::readiness::timings_stats(&crate::timings::load(&sd), "launch", "cold");
        let prog = fusion_core::readiness::progress_block(&stats, start, now_f(), "Fusion is starting - waiting for the Adom add-in to serve", 150.0);
        start_launch_progress_watch(ctx);
        let sentence = timings_sentence(&sd).map(|s| format!("{s}. ")).unwrap_or_default();
        let eta_bit = match prog["etaSec"].as_i64() { Some(e) => format!("The add-in isn't up yet - about {e}s remaining by this machine's own measured history (read progress.percent / progress.etaSec on this response; the estimate is revised as the launch proceeds). This is "), None => "The add-in isn't up yet and this machine has no measured launch history yet, so progress.confidence is 'typical' until it learns. This is ".to_string() };
        return json!({"success": true, "stillLaunching": true, "addinConnected": false, "startKind": "cold", "progress": prog, "timings": timings_launch(&sd), "statusVerb": "fusion_readiness",
            "recoveryFilesRelocated": reloc_moved, "resolvedPath": exe_path, "seatDialogResolved": seat_clicks > 0, "backgroundLaunch": true,
            "_hint": format!("Fusion was launched IN THE BACKGROUND (minimized, foreground-lock on) so it does not steal the user's focus - do NOT foreground it. {}{sentence}{eta_bit}NOT a failure and NOT a retry-able timeout: do NOT re-call fusion_start. WHAT HAPPENS NEXT: POLL fusion_readiness every ~10s; it returns needsSignin/licensingDialog/updating if something is mid-flight (the bridge handles the seat dialog itself), and ready:true when it is drivable - then run your fusion_* verbs. NEVER poll blind past ~2 min: desktop_screenshot_window the Fusion hwnd and READ the response's screenshots[] array - every OWNED POPUP rides back as its own child shot (this is how you SEE a blocking dialog; never desktop_screenshot_screen, the user is doing other things).",
                if seat_clicks > 0 { "A seat 'Active Sessions Exceeded' dialog appeared and was AUTO-RESOLVED in the background via UIA (no foreground). " } else { "" })});
    }
    let recovery_dismissed = crate::readiness_exec::dismiss_recovery_dialog(ctx, 5.0, true).0;
    let dialog_blocking = !round_trip_ready(1);
    crate::timings::record(&sd, "launch", now_f() - start, Some("cold"));
    crate::small_verbs::set_pref(&sd, "launchStartedAt", Value::Null);
    let mut output = "Fusion 360 launched successfully.".to_string();
    if reloc_moved > 0 { output.push_str(&format!(" Relocated {reloc_moved} recovery file(s) to {} (preserved for manual recovery if needed).", reloc["dest"].as_str().unwrap_or("None"))); }
    if recovery_dismissed { output.push_str(" Dismissed 'Recovered Documents' dialog."); }
    if dialog_blocking { output = "Fusion 360 launched but a MODAL DIALOG is blocking the UI. This is likely the 'Recovered Documents' dialog from a previous crash. Take a screenshot with desktop_screenshot_window to see what's showing, then dismiss it manually or use fusion_dismiss_recovery.".into(); }
    json!({"success": true, "output": output, "addinConnected": true, "dialogBlocking": dialog_blocking, "recoveryDialogDismissed": recovery_dismissed, "recoveryFilesRelocated": reloc_moved, "recoveryFilesPath": reloc["dest"],
        "startKind": "cold", "progress": terminal_block(start, "Fusion is up and the add-in is serving", "cold", false), "timings": timings_launch(&sd), "resolvedPath": exe_path})
}

static AUTO_RESTART_AT: std::sync::Mutex<f64> = std::sync::Mutex::new(0.0);

/// `_begin_auto_addin_restart`: once per four minutes, in the background, saving what can be saved.
fn begin_auto_addin_restart(_ctx: &Ctx) -> bool {
    let mut at = AUTO_RESTART_AT.lock().unwrap();
    if now_f() - *at < 240.0 { return false; }
    *at = now_f();
    // the restart itself (save, relaunch, wait) is a thread that outlives this call, so it is
    // sent to this exe's own /command rather than run on the borrowed context
    let url = crate::launch::SELF_URL.lock().unwrap().clone().or_else(|| SIDECAR_URL.lock().unwrap().clone());
    std::thread::spawn(move || {
        if let Some(u) = url {
            let r = ureq::post(&u).timeout(Duration::from_secs(900)).send_json(json!({"command": "fusion_restart_for_addin", "args": {"save": true, "auto": true, "wait": true}})).ok().and_then(|r| r.into_json::<Value>().ok()).unwrap_or(json!({}));
            eprintln!("[fusion-bridge] auto add-in restart: {{success: {}, errorCode: {}}}", r["success"], r["errorCode"]);
        }
    });
    true
}

/// The sidecar's /command URL, published by the shell so background threads can reach it.
pub static SIDECAR_URL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
/// This exe's own /command URL (set once it listens), for work that must outlive a request.
pub static SELF_URL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// A call that finds Fusion closed starts it. If it keeps dying straight after, that must not repeat: a poller
/// once relaunched a broken Fusion every minute, a fresh round of popups each time (2026-11-04). Two starts within
/// RESTART_WINDOW_SECS means it isn't staying up, so calls stop starting it for AUTOSTART_PAUSE_SECS and say why.
/// fusion_start, a person or an AI asking outright, always tries and clears this.
const RESTART_WINDOW_SECS: f64 = 300.0;
const AUTOSTART_PAUSE_SECS: f64 = 900.0;

fn autostart_path(state_dir: &std::path::Path) -> std::path::PathBuf { state_dir.join("autostart.json") }

fn autostart_record(state_dir: &std::path::Path) -> Value {
    std::fs::read_to_string(autostart_path(state_dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}))
}

/// Seconds left on the pause, or None to go ahead.
fn autostart_paused(state_dir: &std::path::Path, now: f64) -> Option<f64> {
    let r = autostart_record(state_dir);
    let last = r["lastStart"].as_f64()?;
    (r["strikes"].as_u64().unwrap_or(0) >= 2 && now - last < AUTOSTART_PAUSE_SECS).then(|| AUTOSTART_PAUSE_SECS - (now - last))
}

fn note_autostart(state_dir: &std::path::Path, now: f64) {
    let r = autostart_record(state_dir);
    let soon = r["lastStart"].as_f64().map(|l| now - l < RESTART_WINDOW_SECS).unwrap_or(false);
    let strikes = if soon { r["strikes"].as_u64().unwrap_or(0) + 1 } else { 1 };
    let _ = std::fs::create_dir_all(state_dir);
    let _ = std::fs::write(autostart_path(state_dir), json!({"lastStart": now, "strikes": strikes}).to_string());
}

fn broken_install_refusal(problem: &str) -> Value {
    json!({"success": false, "errorCode": "fusion_install_broken", "launchAttempted": false,
        "error": format!("Fusion can't start on this machine, so it wasn't launched: {problem}."),
        "_hint": "Launching it would only raise one Windows error popup per missing DLL. fusion_install_fusion repairs it with Autodesk's own installer (about 5 minutes, designs are in the cloud and aren't touched). Tell the user before starting it."})
}

/// `_addin_preamble`: the gate every add-in-dependent verb passes. None means send it.
pub fn gate(ctx: &Ctx, cmd: &str, args: &Value) -> Option<Value> {
    let installed = ctx.fusion_exe.is_some();
    let mut fusion_running = running(ctx);
    if !installed && !fusion_running {
        return Some(json!({"success": false, "error": "Fusion 360 is not installed on this machine.", "errorCode": "fusion_not_installed",
            "_hint": "Fusion 360 isn't installed. Do NOT tell the user to install it themselves - OFFER to install it FOR them and do it on a yes: the fusion-onboarding skill silent-installs Fusion + drives the Autodesk sign-in."}));
    }
    if !fusion_running {
        for _ in 0..2 {
            std::thread::sleep(Duration::from_millis(1500));
            let hwnd = Loopback::from_env().and_then(|ab| fusion_core::windows_facts::facts_from_ab(&ab, &ctx.caller).main_hwnd).is_some();
            if running(ctx) || health_probe(Duration::from_millis(1500)) || hwnd { fusion_running = true; break; }
        }
    }
    if !fusion_running && args["noAutoStart"].as_bool() != Some(true) {
        if let Some(problem) = ctx.fusion_exe.as_deref().and_then(fusion_core::install_check::launch_problem) {
            crate::fusion_prefs::action_ledger(&ctx.state_dir, "autostart_refused_broken_install", json!({"forCommand": cmd, "problem": problem}));
            return Some(broken_install_refusal(&problem));
        }
        if crate::maintenance::installer_running(ctx) {
            return Some(json!({"success": false, "errorCode": "fusion_updating", "launchAttempted": false, "error": "Fusion's installer is running, so Fusion wasn't launched: starting it mid-update can fail or disturb the update.",
                "_hint": "Wait for it to finish, then call again. fusion_readiness says when it's done."}));
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        if let Some(left) = autostart_paused(&ctx.state_dir, now) {
            return Some(json!({"success": false, "errorCode": "fusion_autostart_paused", "launchAttempted": false, "retryAfterSec": left.round(),
                "error": "Fusion was started twice in the last few minutes and closed again each time, so it wasn't started again.",
                "_hint": "It may have been closed by the user, or it may be failing to stay up: look before retrying. fusion_start tries again now."}));
        }
        note_autostart(&ctx.state_dir, now);
        let t0 = Instant::now();
        eprintln!("[fusion-bridge] {cmd} needs Fusion; it is closed - launching it (page issue #64)");
        let auto = start(ctx, &json!({"waitForAddin": true, "_fromGate": true}));
        let took = (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0;
        let up = auto["success"].as_bool() == Some(true) && running(ctx);
        crate::fusion_prefs::action_ledger(&ctx.state_dir, "fusion_auto_started", json!({"forCommand": cmd, "ok": up, "tookSec": took}));
        if up { fusion_running = true; }
        else {
            return Some(json!({"success": false, "errorCode": "fusion_autostart_failed",
                "error": format!("Fusion was closed so the bridge tried to open it for you, and the launch did not come up: {}", auto["error"].as_str().unwrap_or("no error reported").chars().take(300).collect::<String>()),
                "autoStartAttempted": true, "autoStartSec": took,
                "_hint": "This is a launch failure, NOT a bad request - the command itself was never run. Check fusion_readiness (needsSignin true means Autodesk sign-in is blocking the launch, which is a human step). Pass noAutoStart:true to get the old refuse-immediately behaviour."}));
        }
    }
    if !fusion_running {
        return Some(json!({"success": false, "error": "Fusion 360 is installed but not running.", "errorCode": "fusion_not_running",
            "_hint": "Call fusion_start to start Fusion 360, then wait for the add-in to become ready (fusion_start blocks until ready). Then retry this command."}));
    }
    if busy_status().is_some() { return None; } // the busy gate answers this one
    // Sign-in short-circuit (page issue #79): while the main window says "Signing in" the add-in
    // is not loaded yet, so probing it for 5s, then a 20s hang probe, then the restart-info
    // walk turned a known readiness state into a 60s+ addin_restart_required. Answer from the
    // window title instead, in well under a second.
    if let Some(facts) = Loopback::from_env().map(|ab| fusion_core::windows_facts::facts_from_ab(&ab, &ctx.caller)) {
        if facts.needs_signin {
            crate::fusion_prefs::action_ledger(&ctx.state_dir, "signin_gate", json!({"forCommand": cmd, "mainTitle": facts.main_title}));
            return Some(json!({"success": false, "errorCode": "needs_signin", "needsSignin": true, "hostResponding": true,
                "mainTitle": facts.main_title,
                "error": format!("Fusion is at the Autodesk sign-in screen ({}), so the add-in is not loaded yet and '{cmd}' cannot run. This is NOT an add-in fault and NOT a hang.", facts.main_title.as_deref().unwrap_or("Signing in - Autodesk Fusion")),
                "_hint": "Do NOT restart Fusion and do NOT diagnose the add-in: nothing loads until the user is signed in. Drive the sign-in with the fusion-autodesk-signin skill (fusion_signin), or ask the user to sign in, then poll fusion_readiness until needsSignin is false and ready is true, and retry this verb."}));
        }
    }
    if wait_for_addin(5) { return None; }
    let h = crate::readiness_exec::confirmed_hung(ctx);
    if h["hung"].as_bool() == Some(true) {
        crate::fusion_prefs::action_ledger(&ctx.state_dir, "hang_gate_kill", json!({"heldSeconds": h["heldSeconds"], "signals": h["signals"]}));
        let _ = crate::lifecycle::kill(ctx, &json!({}));
        std::thread::sleep(Duration::from_secs(6));
        let _ = start(ctx, &json!({}));
        return Some(json!({"success": false, "error": format!("Fusion 360 itself is NOT RESPONDING (hung host process, held {}s with the add-in refusing a 20s probe); the add-in cannot answer while it is.", h["heldSeconds"]),
            "errorCode": "fusion_host_hung", "hungSignals": h["signals"], "restartAttempted": true,
            "_hint": "Do NOT hunt for a blocking dialog - there is not one. Nothing reaches Fusion over IPC while the host is hung. The bridge force-restarted it; poll fusion_readiness."}));
    }
    if matches!(h["phase"].as_str(), Some("suspect") | Some("busy-not-hung") | Some("long-op-in-flight")) {
        return Some(json!({"success": false, "error": "Fusion's main thread is not answering right now - most likely WORKING on something heavy (a STEP import or save starves the add-in's probes), not hung.",
            "errorCode": "fusion_addin_busy", "hostResponding": h["phase"].as_str() == Some("busy-not-hung"), "hungHeldSeconds": h["heldSeconds"], "keptBy": h["keptBy"],
            "_hint": "WAIT and re-poll; do not restart anything. The bridge only declares a real hang after 180s of continuous silence with Fusion idle (not using CPU), including a 20s probe - if that happens it restarts Fusion itself and says so."}));
    }
    // A modal in front of the add-in is never a reason to restart (issue #87, John: "screenshot
    // itself using ab ... give the ai the parent/child array of screenshots so the ai can figure
    // out how to expire that dialog box"). Hand the pictures over instead.
    match crate::lifecycle::blocked_by_dialog_reply(ctx, cmd) {
        crate::lifecycle::DialogGate::Blocked(b) => return Some(b),
        crate::lifecycle::DialogGate::Expired(dialogs, tried) => {
            // the box is gone; give the add-in a moment to pump again, then run the verb. If it
            // still does not answer, say so and stop here: a dialog was the reason, never a
            // missing add-in, so no restart follows.
            if wait_for_addin(20) { return None; }
            return Some(json!({"success": false, "errorCode": "dialog_expired_retry", "dialogsExpired": dialogs, "expiryTried": tried,
                "error": format!("'{cmd}' was blocked by a dialog ({}); the bridge expired it programmatically, and the add-in has not resumed answering yet.", dialogs.iter().map(|d| d["title"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join(", ")),
                "_hint": "Re-run the verb in a few seconds. Nothing was restarted and nothing will be: the add-in was blocked by that dialog, not missing."}));
        }
        crate::lifecycle::DialogGate::Clear => {}
    }
    // Silence alone never means "not loaded": an add-in that answered inside this Fusion, or that
    // holds one of our requests, is loaded and working. Only an add-in with no such evidence goes
    // on to the restart below.
    if let Some(ev) = crate::addin::loaded_evidence(&ctx.pids) { return Some(busy_not_missing(&ctx.state_dir, cmd, &ev)); }
    let ri = crate::chainexec::addin_restart_info(ctx, &json!({"fireToast": true}), &|v, a| crate::addin::addin_http(v, &a, 8));
    if ri["autoRestart"].as_bool() == Some(true) {
        let started = begin_auto_addin_restart(ctx);
        return Some(json!({"success": false, "errorCode": "addin_restarting",
            "error": "Fusion is running without the Adom add-in and no unsaved document belongs to the user, so the bridge is restarting Fusion itself (documents are saved first). No toast, no ask.",
            "restartStarted": started, "userUnsaved": ri["userUnsaved"].as_array().cloned().unwrap_or_default(),
            "_hint": "Poll fusion_readiness; ready flips true in ~60-90s, then retry this verb."}));
    }
    Some(json!({"success": false, "error": "Fusion 360 is running but the AdomBridge add-in is not responding - it was installed or updated while Fusion was already open, and Fusion only loads add-ins at launch.",
        "errorCode": "addin_restart_required", "hostResponding": true, "restartRequired": true, "userMessage": ri["userMessage"], "openDocuments": ri["openDocuments"], "unsavedDocuments": ri["unsavedDocuments"],
        "documentsKnown": ri["documentsKnown"], "remedyVerb": "fusion_restart_for_addin", "toast": ri["toast"],
        "_hint": "ASK THE USER, do not silently restart their Fusion: show userMessage, and if documentsKnown say how many documents are open. On yes, call fusion_restart_for_addin {save:true} - it saves open documents, restarts Fusion, waits for the add-in and reports back. An AI driving with no human present may call it directly. (Legacy callers: this was errorCode fusion_addin_not_responding.)"}))
}

#[cfg(test)] mod autostart_tests {
    use super::*;
    fn dir(tag: &str) -> std::path::PathBuf { let d = std::env::temp_dir().join(format!("adom-autostart-{tag}-{}", std::process::id())); let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).unwrap(); d }
    #[test] fn a_first_start_goes_ahead_and_so_does_a_second() {
        let d = dir("two");
        assert!(autostart_paused(&d, 1000.0).is_none());
        note_autostart(&d, 1000.0);
        assert!(autostart_paused(&d, 1060.0).is_none());
    }
    #[test] fn a_third_start_within_minutes_is_paused_then_the_pause_ends() {
        let d = dir("three");
        note_autostart(&d, 1000.0); note_autostart(&d, 1060.0);
        assert!(autostart_paused(&d, 1120.0).is_some());
        assert!(autostart_paused(&d, 1060.0 + AUTOSTART_PAUSE_SECS + 1.0).is_none());
    }
    #[test] fn starts_far_apart_never_add_up() {
        let d = dir("apart");
        note_autostart(&d, 1000.0); note_autostart(&d, 1000.0 + RESTART_WINDOW_SECS + 1.0); note_autostart(&d, 1000.0 + 2.0 * RESTART_WINDOW_SECS + 2.0);
        assert!(autostart_paused(&d, 1000.0 + 2.0 * RESTART_WINDOW_SECS + 3.0).is_none());
    }
}

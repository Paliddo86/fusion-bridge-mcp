//! The sidecar's janitor tick, native: every 20 s take the Autodesk seat if a mid-session
//! conflict popped, grow a squint-sized Fusion window (six tries, then leave it alone), keep
//! the driving badge honest, answer a bridge document's Unsaved-changes prompt; every 5 min
//! click Fusion's update button in the background when Fusion is verifiably idle.
use crate::addin::Ctx;
use fusion_core::ab::{AbClient, Loopback};
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;

const BADGE_IDLE_SECONDS: f64 = 300.0;
static SIZE_ATTEMPTS: Mutex<Vec<(i64, u32)>> = Mutex::new(Vec::new());
static LAST_UPDATE_SWEEP: Mutex<f64> = Mutex::new(0.0);

fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }
fn pref_on(sd: &std::path::Path, key: &str) -> bool { crate::small_verbs::get_pref(sd, key).map(|v| v != json!(false)).unwrap_or(true) }

/// `ensure_reasonable_window`: grow a small Fusion window to 92% of its monitor's work area,
/// centered, without activating it (ab moves it; Win32 only answers the two facts).
fn ensure_reasonable_window(ctx: &Ctx, hwnd: i64, rect: &Value) -> Value {
    if ctx.host.window_is_maximized(hwnd) { return json!({"sized": false, "why": "already maximized (user's choice)"}); }
    let Some((wl, wt, ww, wh)) = ctx.host.window_work_area(hwnd) else { return json!({"sized": false, "why": "GetMonitorInfo failed"}) };
    let (cw, ch) = (rect["width"].as_i64().unwrap_or(0), rect["height"].as_i64().unwrap_or(0));
    if (cw * ch) as f64 >= 0.60 * (ww as f64) * (wh as f64) { return json!({"sized": false, "why": "already large enough", "current": [cw, ch], "workArea": [ww, wh]}); }
    let (nw, nh) = ((ww as f64 * 0.92) as i64, (wh as f64 * 0.92) as i64);
    let (x, y) = (wl as i64 + (ww as i64 - nw) / 2, wt as i64 + (wh as i64 - nh) / 2);
    let ok = Loopback::from_env().map(|ab| ab.call("desktop_set_window_state", json!({"hwnd": hwnd, "left": x, "top": y, "width": nw, "height": nh, "reason": "Fusion's window was small enough to squint at; grown to 92% of the work area without activating it"}), &ctx.caller, 20).is_ok()).unwrap_or(false);
    json!({"sized": ok, "from": [cw, ch], "to": [nw, nh], "workArea": [ww, wh], "hwnd": hwnd, "why": "grew to 92% of the work area without activating it"})
}

fn tick(ctx: &Ctx, pass: &dyn Fn(&str, Value) -> Value) {
    let sd = ctx.state_dir.clone();
    let running = !ctx.host.fusion_processes().is_empty();
    let facts = Loopback::from_env().map(|ab| fusion_core::windows_facts::facts_from_ab(&ab, &ctx.caller)).unwrap_or_default();
    // the seat
    if running && pref_on(&sd, "autoTakeOverSeat") && facts.licensing_dialog.is_some() {
        let r = crate::readiness_exec::resolve_seat_dialog(ctx, 3);
        if r["clicks"].as_u64().unwrap_or(0) > 0 {
            // Toast only a SURPRISE takeover. When an AI moved the seat on purpose (fusion_claim_seat
            // or fusion_release_seat here or on a peer within the last 10 minutes) the user already
            // knows, and two boxes trading the seat used to toast them repeatedly (John, 2026-09-23).
            let intentional = crate::seat::recent_intentional_move(&sd, 600.0);
            // At most one notice per 24 hours, remembered ON DISK: the old in-memory hour reset on
            // every bridge restart, so a run of reinstalls toasted the user ten times (John, 2026-10-05).
            let stamp = sd.join("seat-notice-last");
            let last = std::fs::read_to_string(&stamp).ok().and_then(|t| t.trim().parse::<f64>().ok()).unwrap_or(0.0);
            if !intentional && now() - last > 86400.0 {
                let _ = std::fs::write(&stamp, format!("{}", now()));
                let _ = crate::small_verbs::notify_owner(&sd, &json!({"title": "Fusion is running here now", "body": "Autodesk only allows one active Fusion session, so this machine picked it up. Fusion on your other machine is paused and will resume when you go back to it. You won't see this again today.", "level": "info"}), &ctx.caller);
            }
            eprintln!("[fusion-bridge] seat sweep: took over the session (clicks={}, verified={})", r["clicks"], r["verified"]);
            crate::fusion_prefs::action_ledger(&sd, "seat_takeover", json!({"clicks": r["clicks"], "verified": r["verified"]}));
        }
    }
    // the window size (six sweeps, then it is the user's)
    if running && pref_on(&sd, "sizeFusionWindow") { if let Some(h) = facts.main_hwnd {
        let n = SIZE_ATTEMPTS.lock().unwrap().iter().find(|(k, _)| *k == h).map(|(_, n)| *n).unwrap_or(0);
        if n < 6 {
            let rect = facts.family.iter().find(|w| w["hwnd"].as_i64() == Some(h)).map(|w| w["rect"].clone()).unwrap_or(json!({}));
            let r = ensure_reasonable_window(ctx, h, &rect);
            let why = r["why"].as_str().unwrap_or("");
            let next = if why.contains("already") { 99 } else { n + 1 };
            { let mut v = SIZE_ATTEMPTS.lock().unwrap(); v.retain(|(k, _)| *k != h); v.push((h, next)); }
            if r["sized"].as_bool() == Some(true) { eprintln!("[fusion-bridge] window sized on sweep (attempt {}): {why}", n + 1); }
        }
    } }
    // the badge: on while driving, shed after five idle minutes
    if running && pref_on(&sd, "showDrivingBadge") {
        let idle = crate::etiquette::last_drive_at().map(|t| now() - t);
        match (crate::etiquette::badge_active(), idle) {
            (false, Some(i)) if i <= BADGE_IDLE_SECONDS => { if let Some(h) = facts.main_hwnd { crate::etiquette::set_badge(&ctx.caller, h); } }
            (true, Some(i)) if i > BADGE_IDLE_SECONDS => { if crate::etiquette::clear_badge(&ctx.caller) { eprintln!("[fusion-bridge] driving badge shed after {BADGE_IDLE_SECONDS}s idle"); } }
            _ => {}
        }
    }
    // a bridge document's Unsaved-changes prompt
    if running { let _ = crate::etiquette::dismiss_save_prompt_if_bridge_doc(&sd, &ctx.caller, &ctx.pids, ""); }
    // dirty bridge documents left open while nothing drives: ours, never the user's
    if running && crate::etiquette::last_drive_at().map(|t| now() - t >= 180.0).unwrap_or(true) {
        let di = pass("document_info", json!({}));
        for d in di["data"]["openDocuments"].as_array().cloned().unwrap_or_default() {
            let name = d["name"].as_str().unwrap_or("").to_string();
            let dirty = d["isModified"].as_bool().unwrap_or(false) || d["modified"].as_bool().unwrap_or(false) || name.ends_with('*');
            if name.is_empty() || !dirty || crate::etiquette::bridge_doc_in(&sd, &name.to_lowercase()).is_none() { continue; }
            if crate::orchestrations::close_bridge_doc_verified(ctx, &name, 60, pass)["closed"].as_bool() == Some(true) {
                eprintln!("[fusion-bridge] janitor: closed dirty bridge doc {} (verified gone)", fusion_lbr::py_repr_str(&name));
                crate::fusion_prefs::action_ledger(&sd, "idle_doc_close", json!({"doc": name}));
            }
        }
    }
    // a stray Create-new prompt, and Fusion's crash reporter (never Send Report for the user)
    if let Some(ab) = Loopback::from_env() {
        for d in crate::small_verbs::classify_blocking_dialogs(&ab, &ctx.caller, &ctx.pids) {
            if d["category"].as_str() == Some("create_confirm") {
                let _ = ab.call("desktop_ui_click", json!({"hwnd": d["hwnd"], "name": "No"}), &ctx.caller, 12);
                eprintln!("[fusion-bridge] janitor: declined a stray Create-new prompt ({})", d["matchedOn"].as_str().unwrap_or("title"));
                crate::fusion_prefs::action_ledger(&sd, "create_confirm_declined", json!({"matchedOn": d["matchedOn"]}));
            }
        }
        let busy = crate::long_op_marker().is_some() || crate::etiquette::last_drive_at().map(|t| now() - t < 180.0).unwrap_or(false);
        if !busy { if let Ok(v) = ab.call("desktop_list_windows", json!({}), &ctx.caller, 20) {
            let wins = v["windows"].as_array().or_else(|| v["data"]["windows"].as_array()).cloned().unwrap_or_default();
            if let Some(w) = wins.iter().find(|w| w["title"].as_str().unwrap_or("").contains("Fusion Error Report")) {
                let h = w["hwnd"].clone();
                crate::fusion_prefs::action_ledger(&sd, "crash_reporter_seen", json!({"hwnd": h, "pid": w["pid"]}));
                let _ = ab.call("desktop_close_window", json!({"hwnd": h}), &ctx.caller, 15);
                std::thread::sleep(Duration::from_secs(2));
                let still = ab.call("desktop_list_windows", json!({}), &ctx.caller, 20).ok().map(|v2| v2["windows"].as_array().or_else(|| v2["data"]["windows"].as_array()).cloned().unwrap_or_default().iter().any(|x| x["hwnd"] == h)).unwrap_or(false);
                crate::fusion_prefs::action_ledger(&sd, if still { "crash_reporter_wedged" } else { "crash_reporter_dismissed" }, json!({"hwnd": h, "closed": !still}));
                eprintln!("[fusion-bridge] janitor: Fusion crash reporter handled (ledgered; never Send Report on the user's behalf)");
            }
        } }
    }
    // the silent update, every five minutes, only when nothing is driving Fusion
    if running && pref_on(&sd, "autoUpdateFusion") {
        let mut last = LAST_UPDATE_SWEEP.lock().unwrap();
        if now() - *last > 300.0 {
            *last = now();
            let busy = crate::long_op_marker().is_some() || crate::etiquette::last_drive_at().map(|t| now() - t < 180.0).unwrap_or(false);
            if !busy {
                let _ = crate::maintenance::apply_fusion_update(ctx, pass);
                let _ = crate::readiness_exec::dismiss_recovery_dialog(ctx, 0.0, true);
            }
        }
    }
}

/// Start the tick on its own thread; a fresh host each pass (the borrowed one cannot cross).
pub fn start() {
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(20));
        let host = crate::host_for_diagnosis();
        let sd = crate::host().bridge_state_dir();
        let side = |_v: &str, _a: Value, _t: u64| -> Value { json!({"success": false, "error": "no sidecar"}) };
        let pids: Vec<u32> = host.fusion_processes().iter().map(|p| p.pid).collect();
        let dlg = |r: Value| r;
        let ctx = Ctx { host: &*host, fusion_exe: crate::host_for_diagnosis_paths(), short_path: |p| crate::host().short_path(p), state_dir: sd, caller: fusion_core::ab::Caller::default(), pids, sidecar: &side, dialogs: &dlg };
        let pass = |verb: &str, a: Value| crate::addin::addin_http(verb, &a, 30);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tick(&ctx, &pass)));
    });
}

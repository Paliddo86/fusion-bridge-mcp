//! The readiness executors, native: the seat resolver, the recovery dismissal and the hang
//! verdict. Ported from server.py (`_resolve_seat_dialog`, `_confirmed_hung`), handlers/
//! dismiss_recovery.py and handlers/fusion_ui.py (`is_fusion_hung`). UIA and window closes go
//! through ab; the two Win32 reads live in os-win behind HostProcess.

use serde_json::{json, Value};
use std::time::{Duration, Instant};

use crate::addin::Ctx;
use fusion_core::ab::{AbClient, Loopback};

fn now_f() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

/// `_owned_popup_count`: ab's screenshot signal, -1 when unavailable.
fn owned_popup_count(ab: &dyn AbClient, ctx: &Ctx, main_hwnd: i64) -> i64 {
    ab.call("desktop_screenshot_window", json!({"hwnd": main_hwnd}), &ctx.caller, 8).ok().and_then(|r| r["ownedPopupCount"].as_i64().or(r["data"]["ownedPopupCount"].as_i64())).unwrap_or(-1)
}

/// `_resolve_seat_dialog`: Continue (or Check again) through UIA in the background, verified by
/// the parent's owned-popup count. {sawDialog, clicks, verified}.
pub fn resolve_seat_dialog(ctx: &Ctx, max_clicks: u32) -> Value {
    let (mut clicks, mut saw, mut verified, mut needs_approval) = (0u32, false, false, false);
    let Some(ab) = Loopback::from_env() else { return json!({"sawDialog": false, "clicks": 0, "verified": false}) };
    for _ in 0..max_clicks {
        let facts = fusion_core::windows_facts::facts_from_ab(&ab, &ctx.caller);
        let Some(dlg) = facts.licensing_dialog else { verified = true; break };
        saw = true;
        let hwnd = dlg["hwnd"].as_i64().unwrap_or(0);
        let parent = dlg["owner"].as_i64().unwrap_or(0);
        let mut landed = false;
        if hwnd != 0 {
            for btn in ["Continue", "Check again"] {
                match ab.call("desktop_ui_click", json!({"hwnd": hwnd, "name": btn}), &ctx.caller, 4) {
                    Ok(r) => {
                        if r["success"].as_bool() == Some(true) || r["status"].as_str() == Some("ok") {
                            if btn == "Check again" { eprintln!("[fusion-bridge] seat: clicked 'Check again' to REGRAB the suspended session (Session Suspended variant)"); }
                            landed = true; break;
                        }
                        if r["errorCode"].as_str() == Some("needs_approval") || r["data"]["errorCode"].as_str() == Some("needs_approval") { needs_approval = true; }
                    }
                    Err(e) => { if format!("{e}").contains("needs_approval") { needs_approval = true; } }
                }
            }
        }
        if landed { clicks += 1; }
        std::thread::sleep(Duration::from_millis(1500));
        if parent != 0 && owned_popup_count(&ab, ctx, parent) == 0 { verified = true; break; }
    }
    // ab gates UI clicks behind its shell approval, even for a bridge (#133, #136: with approval off the
    // click was refused and the run sat at the seat dialog with nothing said). Raise the prompt, at most
    // every five minutes; once it is approved the next pass clicks through.
    if needs_approval {
        static ASKED: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
        let due = { let mut a = ASKED.lock().unwrap(); let due = a.map(|t| t.elapsed() > Duration::from_secs(300)).unwrap_or(true); if due { *a = Some(std::time::Instant::now()); } due };
        if due { let _ = ab.call("request_shell_approval", json!({"reason": "Answer Autodesk's Active Sessions Exceeded dialog so this PC takes the Fusion seat"}), &ctx.caller, 10); }
    }
    json!({"sawDialog": saw, "clicks": clicks, "verified": verified, "needsApproval": needs_approval})
}

/// The verb-shaped answer (`fusion_resolve_seat_dialog`).
pub fn resolve_seat_dialog_verb(ctx: &Ctx, args: &Value) -> Value {
    let r = resolve_seat_dialog(ctx, args["maxClicks"].as_u64().unwrap_or(3).max(1) as u32);
    let still = Loopback::from_env().map(|ab| fusion_core::windows_facts::facts_from_ab(&ab, &ctx.caller).licensing_dialog.is_some()).unwrap_or(false);
    if still && r["needsApproval"].as_bool() == Some(true) {
        return json!({"success": false, "errorCode": "needs_approval", "clicks": r["clicks"], "resolved": false, "detail": r,
            "error": "Autodesk's seat dialog is up, and Adom Bridge needs approval before the bridge can answer it. The prompt has been raised.",
            "tellUser": "Adom Bridge needs your OK to answer Autodesk's Active Sessions Exceeded dialog: click Approve (1 hr, 24 hr or Auto) in the Adom Bridge window, on its notification, or in Hydrogen.",
            "_hint": "Once approved, call this again (or fusion_claim_seat). Continue makes this PC the active seat; Fusion on the other machine suspends and keeps its documents."});
    }
    json!({"success": true, "clicks": r["clicks"], "resolved": !still, "detail": r, "_hint": "Readiness executor: the seat dialog is clicked Continue (Suspend pre-selected) and verified gone by screenshot; poll fusion_readiness."})
}

/// `relocate_recovery_files`: move CrashRecovery contents to ~/.adom/recovery/fusion/<stamp>/.
pub fn relocate_recovery_files() -> Value {
    let base = std::path::Path::new(&std::env::var("LOCALAPPDATA").unwrap_or_default()).join("Autodesk").join("Autodesk Fusion 360");
    let Ok(entries) = std::fs::read_dir(&base) else { return json!({"moved": 0, "dest": Value::Null, "sources": []}) };
    let cr_dirs: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path().join("CrashRecovery")).filter(|d| d.is_dir() && std::fs::read_dir(d).map(|mut r| r.next().is_some()).unwrap_or(false)).collect();
    if cr_dirs.is_empty() { return json!({"moved": 0, "dest": Value::Null, "sources": []}); }
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let dest = std::path::Path::new(&home).join(".adom").join("recovery").join("fusion").join(chrono::Local::now().format("%Y%m%d_%H%M%S").to_string());
    let _ = std::fs::create_dir_all(&dest);
    let mut moved = 0i64;
    let mut sources = Vec::new();
    for cr in &cr_dirs {
        sources.push(cr.to_string_lossy().replace('\\', "/"));
        if let Ok(items) = std::fs::read_dir(cr) {
            for it in items.flatten() {
                let src = it.path();
                let mut dst = dest.join(it.file_name());
                if dst.exists() { let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(); let ext = src.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default(); dst = dest.join(format!("{stem}_{moved}{ext}")); }
                if std::fs::rename(&src, &dst).is_ok() { moved += 1; continue; }
                let copied = if src.is_dir() { copy_dir(&src, &dst).is_ok() } else { std::fs::copy(&src, &dst).is_ok() };
                if copied { let _ = if src.is_dir() { std::fs::remove_dir_all(&src) } else { std::fs::remove_file(&src) }; moved += 1; }
            }
        }
        let _ = std::fs::remove_dir(cr); // only when empty
    }
    json!({"moved": moved, "dest": if moved > 0 { json!(dest.to_string_lossy().replace('\\', "/")) } else { Value::Null }, "sources": sources})
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? { let e = e?; let p = e.path(); let d = dst.join(e.file_name()); if p.is_dir() { copy_dir(&p, &d)?; } else { std::fs::copy(&p, &d)?; } }
    Ok(())
}

fn windows(ab: &dyn AbClient, ctx: &Ctx) -> Vec<Value> {
    ab.call("desktop_enum_windows", json!({}), &ctx.caller, 10).ok().map(|v| v["windows"].as_array().or_else(|| v["data"]["windows"].as_array()).cloned().unwrap_or_default()).unwrap_or_default()
}
fn find_by_title(rows: &[Value], sub: &str) -> Option<i64> { rows.iter().find(|w| w["visible"].as_bool() != Some(false) && w["title"].as_str().map(|t| t.contains(sub)).unwrap_or(false)).and_then(|w| w["hwnd"].as_i64()) }
/// `_find_fusion_dialog_windows`: small Qt windows titled exactly "Fusion360" (the recovery list).
fn recovery_list_windows(rows: &[Value]) -> Vec<i64> {
    rows.iter().filter(|w| {
        let cls = w["className"].as_str().unwrap_or("");
        if w["visible"].as_bool() == Some(false) || !cls.contains("QWindowIcon") || cls.contains("ToolSaveBits") || w["title"].as_str() != Some("Fusion360") { return false; }
        let r = fusion_core::dialogs::rect_of(&w["rect"]);
        r["width"].as_i64().unwrap_or(0) < 700 && r["height"].as_i64().unwrap_or(0) < 500
    }).filter_map(|w| w["hwnd"].as_i64()).collect()
}

/// `dismiss_recovery_dialog`: relocate the files, then answer every recovery prompt in the
/// background (No through UIA where Python used Tab+Enter after a foreground grab; WM_CLOSE
/// for the list). True when anything was dismissed.
pub fn dismiss_recovery_dialog(ctx: &Ctx, max_wait: f64, relocate: bool) -> (bool, Value) {
    let reloc = if relocate { relocate_recovery_files() } else { json!({"moved": 0, "dest": Value::Null}) };
    let Some(ab) = Loopback::from_env() else { return (false, reloc) };
    let deadline = Instant::now() + Duration::from_secs_f64(max_wait.max(0.5));
    let mut dismissed_any = false;
    while Instant::now() < deadline {
        let rows = windows(&ab, ctx);
        if let Some(h) = find_by_title(&rows, "Open recovery document instead?") {
            if ab.call("desktop_ui_click", json!({"hwnd": h, "name": "No"}), &ctx.caller, 8).is_err() { let _ = ab.call("desktop_close_window", json!({"hwnd": h}), &ctx.caller, 8); }
            dismissed_any = true; std::thread::sleep(Duration::from_millis(500)); continue;
        }
        let lst = recovery_list_windows(&rows);
        if !lst.is_empty() {
            for h in &lst { let _ = ab.call("desktop_close_window", json!({"hwnd": h}), &ctx.caller, 8); }
            crate::fusion_prefs::action_ledger(&ctx.state_dir, "recovery_wm_close", json!({"windows": lst}));
            dismissed_any = true; std::thread::sleep(Duration::from_millis(500)); continue;
        }
        if let Some(h) = find_by_title(&rows, "Recovered Documents") {
            if ab.call("desktop_ui_click", json!({"hwnd": h, "name": "Close"}), &ctx.caller, 8).is_err() { let _ = ab.call("desktop_close_window", json!({"hwnd": h}), &ctx.caller, 8); }
            dismissed_any = true; std::thread::sleep(Duration::from_millis(500)); continue;
        }
        if dismissed_any { std::thread::sleep(Duration::from_millis(500)); let rows = windows(&ab, ctx); if find_by_title(&rows, "Open recovery document instead?").is_none() && recovery_list_windows(&rows).is_empty() && find_by_title(&rows, "Recovered Documents").is_none() { break; } continue; }
        break;
    }
    (dismissed_any, reloc)
}

static HUNG_SINCE: std::sync::Mutex<f64> = std::sync::Mutex::new(0.0);

/// `is_fusion_hung`: IsHungAppWindow on the main window plus a visible "Ghost" window naming Fusion.
pub fn is_fusion_hung(ctx: &Ctx) -> Value {
    let mut out = json!({"hung": false, "signals": [], "hwnd": Value::Null});
    if let Some(ab) = Loopback::from_env() {
        let facts = fusion_core::windows_facts::facts_from_ab(&ab, &ctx.caller);
        if let Some(h) = facts.main_hwnd { out["hwnd"] = json!(h); if ctx.host.window_hung(h) { out["hung"] = json!(true); out["signals"].as_array_mut().unwrap().push(json!("IsHungAppWindow")); } }
        let ghosts: Vec<i64> = windows(&ab, ctx).iter().filter(|w| w["className"].as_str() == Some("Ghost") && w["visible"].as_bool() != Some(false) && w["title"].as_str().map(|t| t.contains("Fusion")).unwrap_or(false)).filter_map(|w| w["hwnd"].as_i64()).collect();
        if !ghosts.is_empty() { out["hung"] = json!(true); out["signals"].as_array_mut().unwrap().push(json!("GhostWindow")); out["ghostHwnds"] = json!(ghosts); }
    }
    out
}

/// `fusion_using_cpu`: did the family's CPU time advance over 0.7s?
pub fn fusion_using_cpu(ctx: &Ctx) -> bool {
    let t0 = ctx.host.family_cpu_ticks(); std::thread::sleep(Duration::from_millis(700)); let t1 = ctx.host.family_cpu_ticks();
    t1.saturating_sub(t0) > 50_000
}

/// CPU over a sample window that says Fusion is working: at least 15% of one core across the family
/// (GetProcessTimes counts 100 ns units). A hung host's background threads stay well under it.
pub fn working_from_ticks(delta: u64, window: Duration) -> bool {
    delta as f64 >= 0.15 * window.as_secs_f64() * 10_000_000.0
}

/// A Fusion still using CPU is working, whatever its window says: a script or native call that
/// outlived the request that started it burns a core on the main thread, where a real hang (a
/// deadlock, a wait on a lost modal) sits near idle.
pub fn fusion_working(ctx: &Ctx) -> bool {
    let window = Duration::from_millis(1500);
    let t0 = ctx.host.family_cpu_ticks(); std::thread::sleep(window); let t1 = ctx.host.family_cpu_ticks();
    working_from_ticks(t1.saturating_sub(t0), window)
}

/// `_confirmed_hung`: kill-grade only when the OS says hung AND a 20s probe gets nothing AND the host
/// is idle AND all of that has persisted 180s; a long op in flight (under 30 minutes, or with CPU
/// still moving) protects.
pub fn confirmed_hung(ctx: &Ctx) -> Value {
    if let Some(lo) = crate::long_op_marker() {
        let el = now_f() - lo.1;
        let protected = el < 1800.0 || fusion_using_cpu(ctx);
        if protected { return json!({"hung": false, "phase": "long-op-in-flight", "longOp": lo.0, "longOpElapsedSec": (el * 10.0).round() / 10.0, "keptBy": if el < 1800.0 { "under-ceiling" } else { "fusion-cpu-active" }}); }
    }
    let h = is_fusion_hung(ctx);
    if h["hung"].as_bool() != Some(true) { *HUNG_SINCE.lock().unwrap() = 0.0; return json!({"hung": false, "phase": "pumping"}); }
    if crate::addin::addin_status_probe(Duration::from_secs(20)).is_some() { *HUNG_SINCE.lock().unwrap() = 0.0; return json!({"hung": false, "phase": "busy-not-hung", "signals": h["signals"]}); }
    // 2026-10-30: the gate killed a Fusion 37 minutes into a long build script, after the request
    // that started the script had timed out (so no long op was on record), and closed the user's
    // 24 documents. A kill now also needs the host to have gone idle.
    if fusion_working(ctx) { *HUNG_SINCE.lock().unwrap() = 0.0; return json!({"hung": false, "phase": "busy-not-hung", "keptBy": "fusion-cpu-active", "signals": h["signals"]}); }
    let now = now_f();
    let mut since = HUNG_SINCE.lock().unwrap();
    if *since == 0.0 { *since = now; }
    let held = now - *since;
    if held < 180.0 { return json!({"hung": false, "phase": "suspect", "heldSeconds": (held * 10.0).round() / 10.0, "signals": h["signals"]}); }
    json!({"hung": true, "phase": "confirmed", "heldSeconds": (held * 10.0).round() / 10.0, "signals": h["signals"]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fusion_burning_a_core_is_working_not_hung() {
        let window = Duration::from_millis(1500);
        assert!(working_from_ticks(15_000_000, window)); // a whole core: a script on the main thread
        assert!(working_from_ticks(2_250_000, window)); // 15%
        assert!(!working_from_ticks(300_000, window)); // 2%: a hung host's background threads
        assert!(!working_from_ticks(0, window));
    }
}

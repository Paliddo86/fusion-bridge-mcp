//! The window etiquette around an add-in verb, native. Ported from server.py's
//! `_window_etiquette_pre` / `_window_etiquette_post` / `_SavePromptSentinel` and their
//! helpers: the document registry (what we open is ours to clean up), the foreground rule
//! (a reason and a 3s heads-up, else the background envelope), the save-prompt sentinel that
//! discards ONLY bridge documents, the post-verb chokepoint sweep, and the etiquette note.
//! Desktop-shaped work goes through ab verbs (desktop_find_control, desktop_ui_click,
//! desktop_close_window, desktop_taskbar); the Win32 envelope is `HostProcess::background_envelope`.

use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fusion_core::ab::{AbClient, Caller, Loopback};

static BADGED: AtomicBool = AtomicBool::new(false);
/// Every Fusion assertion the bridge clicked OK on (the background sentinel included), with when:
/// a verb's reply names the ones raised while it ran (issue #135: a fatal PlacePartWorker assert was
/// clicked away silently and the run went on).
static ASSERTS_SEEN: std::sync::Mutex<Vec<(f64, String)>> = std::sync::Mutex::new(Vec::new());
fn now_secs() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }
/// The routing assertion Fusion raises when a LINE completes a net's last airwire (pusher.cpp
/// inLocator) is informational; any other one means the command did not do its job.
pub fn assert_is_harmless(body: &str) -> bool { body.to_lowercase().contains("pusher.cpp") }
static BADGED_HWND: std::sync::Mutex<i64> = std::sync::Mutex::new(0);
/// `set_driving_badge`: the Adom mark on Fusion's taskbar button while Adom drives it.
pub fn set_badge(caller: &Caller, hwnd: i64) -> bool {
    // The SAME badge the Python bridge painted (handlers/taskbar_overlay.py): our own 32 px PNG as
    // base64, not ab's built-in "adom" badge, which draws larger (John, 2026-09-05).
    static OVERLAY_PNG: &[u8] = include_bytes!("../../../resources/adom-overlay-32.png");
    let ok = Loopback::from_env().map(|ab| ab.call("desktop_taskbar", json!({"hwnd": hwnd, "overlay": {"icon": b64_pub(OVERLAY_PNG), "tooltip": "Adom is driving Fusion"}}), caller, 10).is_ok()).unwrap_or(false);
    if ok { BADGED.store(true, Ordering::Relaxed); *BADGED_HWND.lock().unwrap() = hwnd; }
    ok
}
static BADGE_AT: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
/// set_badge, but a repeat on the same window within 60 s is a no-op (per-verb callers must not
/// spam desktop_taskbar), exactly the Python bridge's throttle.
pub fn set_badge_throttled(caller: &Caller, hwnd: i64) -> bool {
    let same = *BADGED_HWND.lock().unwrap() == hwnd && BADGE_AT.lock().unwrap().map(|t| t.elapsed() < Duration::from_secs(60)).unwrap_or(false);
    if same { return true; }
    let ok = set_badge(caller, hwnd);
    if ok { *BADGE_AT.lock().unwrap() = Some(Instant::now()); }
    ok
}
/// `clear_driving_badge`: remove the taskbar badge from whichever window we badged last.
pub fn clear_badge(caller: &Caller) -> bool {
    let h = *BADGED_HWND.lock().unwrap();
    if h == 0 { return false; }
    let ok = Loopback::from_env().map(|ab| ab.call("desktop_taskbar", json!({"hwnd": h, "overlay": {"badge": "none"}}), caller, 10).is_ok()).unwrap_or(false);
    if ok { BADGED.store(false, Ordering::Relaxed); *BADGED_HWND.lock().unwrap() = 0; }
    ok
}
static LAST_DRIVE_AT: std::sync::Mutex<f64> = std::sync::Mutex::new(0.0);
pub fn badge_active() -> bool { BADGED.load(Ordering::Relaxed) }
pub fn last_drive_at() -> Option<f64> { let v = *LAST_DRIVE_AT.lock().unwrap(); if v > 0.0 { Some(v) } else { None } }
pub fn b64_pub(b: &[u8]) -> String { b64(b) }

pub const SHOW_SURFACE_VERBS: &[&str] = &["show_symbol", "show_footprint", "show_3d_package", "show_library", "show_project", "show_schematic", "show_2d_board", "show_3d_board"];
const DONT_SAVE_LABELS: &[&str] = &["Don't Save", "Don\u{2019}t Save"];
const BRIDGE_DOC_BUILTINS: &[&str] = &["adom-test-", "adomdemosample", "adom-fusion-wrap", "ads8588sipm", "0603waf"];

pub struct Pre { pub args: Value, pub ran_backgrounded: bool, pub refusal: Option<Value>, pub docs_before: Option<Vec<String>>, pub fg: bool }


/// `_note_focus_event`: the quietness audit log, mirrored to focus-events.jsonl.
pub fn note_focus_event(state_dir: &Path, verb: &str, outcome: &str, reason: &str) {
    let v = if verb.starts_with("fusion_") { verb.to_string() } else { format!("fusion_{verb}") };
    let rec = json!({"at": chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(), "verb": v, "outcome": outcome, "reason": reason.chars().take(200).collect::<String>()});
    use std::io::Write;
    let _ = std::fs::create_dir_all(state_dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(state_dir.join("focus-events.jsonl")) { let _ = writeln!(f, "{rec}"); }
}

/// `_snapshot_open_docs`: lowercase names of the documents open right now (empty on failure).
pub fn snapshot_open_docs(call: &dyn Fn(&str, Value, u64) -> Value) -> Vec<String> {
    let r = call("document_info", json!({}), 5);
    if r["success"].as_bool() != Some(true) { return vec![]; }
    let mut v: Vec<String> = r["data"]["openDocuments"].as_array().map(|a| a.iter().filter_map(|d| d["name"].as_str()).filter(|n| !n.is_empty()).map(|n| n.trim_end_matches('*').trim().to_lowercase()).collect()).unwrap_or_default();
    v.sort(); v.dedup(); v
}

/// Each open document's lowercase name and whether it is a saved cloud document (empty on failure).
fn snapshot_open_docs_detail(call: &dyn Fn(&str, Value, u64) -> Value) -> Vec<(String, bool)> {
    let r = call("document_info", json!({}), 5);
    if r["success"].as_bool() != Some(true) { return vec![]; }
    r["data"]["openDocuments"].as_array().map(|a| a.iter().filter_map(|d| {
        let n = d["name"].as_str().filter(|n| !n.is_empty())?.trim_end_matches('*').trim().to_lowercase();
        Some((n, d["isCloudDocument"].as_bool() == Some(true)))
    }).collect()).unwrap_or_default()
}

/// Is a document that appeared while `cmd` ran the bridge's own scratch? A saved cloud document
/// that appeared was OPENED (open_by_urn, a script's documents.open): it is its owner's work, and
/// registering it let the save-prompt sentinel answer Don't Save for it and the restart discard
/// it. So were the documents Fusion opens at launch (recovered work). Only a document the verb
/// made, or a Save As the bridge itself did, is scratch.
pub fn appeared_doc_is_scratch(cmd: &str, cloud: bool) -> bool { cmd != "start" && (!cloud || cmd == "save_to_cloud") }

fn bridge_docs(state_dir: &Path) -> Vec<String> {
    crate::small_verbs::get_pref(state_dir, "bridgeDocs").and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).filter(|s| s.len() >= 3).collect())).unwrap_or_default()
}

/// `_is_bridge_doc_text`: the bridge-doc name the text mentions, if any.
pub fn bridge_doc_in(state_dir: &Path, text: &str) -> Option<String> {
    let t = text.to_lowercase();
    bridge_docs(state_dir).into_iter().find(|n| t.contains(n.as_str())).or_else(|| BRIDGE_DOC_BUILTINS.iter().find(|p| t.contains(*p)).map(|s| s.to_string()))
}

/// `_find_controls_any`: {best, controls} from anywhere in ab's answer.
pub fn find_controls_any(v: &Value, depth: u8) -> Value {
    if depth > 5 { return json!({}); }
    if let Some(s) = v.as_str() { if s.starts_with('{') || s.starts_with('[') { if let Ok(p) = serde_json::from_str::<Value>(s) { return find_controls_any(&p, depth + 1); } } return json!({}); }
    if let Some(o) = v.as_object() {
        if o.get("best").map(|b| b.is_object()).unwrap_or(false) || o.get("controls").map(|c| c.is_array()).unwrap_or(false) {
            return json!({"best": o.get("best").cloned().unwrap_or(json!({})), "controls": o.get("controls").cloned().unwrap_or(json!([]))});
        }
        for (_, x) in o { let r = find_controls_any(x, depth + 1); if r.is_object() && !r.as_object().unwrap().is_empty() { return r; } }
    }
    if let Some(a) = v.as_array() { for x in a { let r = find_controls_any(x, depth + 1); if r.is_object() && !r.as_object().unwrap().is_empty() { return r; } } }
    json!({})
}

/// `_find_save_prompt`: is the unsaved-doc Save prompt up? {found, hwnd, button, text}.
pub fn find_save_prompt(ab: &dyn AbClient, caller: &Caller, pids: &[u32]) -> Value {
    let dialogs = {
        let mut a = json!({}); if !pids.is_empty() { a["pids"] = json!(pids); }
        match ab.call("desktop_enum_windows", a, caller, 10) { Ok(v) => fusion_core::dialogs::qt_dialog_rows(&v["windows"].as_array().or_else(|| v["data"]["windows"].as_array()).cloned().unwrap_or_default()), Err(_) => return json!({"found": false}) }
    };
    for d in dialogs {
        let Some(h) = d["hwnd"].as_i64().filter(|h| *h != 0) else { continue };
        let title = d["title"].as_str().unwrap_or("").trim().to_lowercase();
        for label in DONT_SAVE_LABELS {
            let r = ab.call("desktop_find_control", json!({"hwnd": h, "name": label}), caller, 10).map(|v| find_controls_any(&v, 0)).unwrap_or(json!({}));
            let best = &r["best"];
            if best["name"].as_str().map(|n| !n.is_empty()).unwrap_or(false) && best["invokable"].as_bool().unwrap_or(false) {
                let mut text = String::new();
                if let Ok(t) = ab.call("desktop_find_control", json!({"hwnd": h, "name": "save"}), caller, 10) {
                    let t = find_controls_any(&t, 0);
                    let mut cands = vec![t["best"].clone()]; cands.extend(t["controls"].as_array().cloned().unwrap_or_default());
                    for c in cands { let nm = c["name"].as_str().unwrap_or(""); if nm.len() > text.len() { text = nm.to_string(); } }
                }
                return json!({"found": true, "hwnd": h, "button": best["name"], "text": text});
            }
        }
        if ["save", "save changes", "unsaved changes"].contains(&title.as_str()) { return json!({"found": true, "hwnd": h, "button": Value::Null, "text": ""}); }
    }
    json!({"found": false})
}

/// Fusion Electronics' own "unexpected condition" assertion (pusher.cpp inLocator, seen when a
/// LINE completes a net's last airwire): OK is its only button and is informational, the copper
/// has already landed. Click it and hand back the body text so the caller can checkpoint.
pub fn dismiss_unexpected_condition(ab: &dyn AbClient, caller: &Caller, pids: &[u32]) -> Option<String> {
    for d in crate::small_verbs::classify_blocking_dialogs(ab, caller, pids) {
        if d["category"].as_str() != Some("fusion_unexpected_condition") { continue; }
        let Some(h) = d["hwnd"].as_i64() else { continue };
        let body = ab.call("desktop_find_controls", json!({"hwnd": h}), caller, 10).ok().map(|r| { let dd = if r["data"].is_object() { r["data"].clone() } else { r }; dd["controls"].as_array().or(dd["matches"].as_array()).cloned().unwrap_or_default().iter().filter_map(|c| c["name"].as_str()).filter(|n| n.to_lowercase().contains("unexpected condition")).map(str::to_string).next().unwrap_or_default() }).unwrap_or_default();
        let clicked = ab.call("desktop_ui_click", json!({"hwnd": h, "name": "OK"}), caller, 12).map(|r| r["invoked"].as_bool().unwrap_or(false) || r["success"].as_bool().unwrap_or(false)).unwrap_or(false);
        eprintln!("[fusion-bridge] Fusion Electronics assertion dialog {}: {}", if clicked { "dismissed (OK)" } else { "seen, OK did not land" }, body.chars().take(160).collect::<String>());
        if let Ok(mut v) = ASSERTS_SEEN.lock() { v.push((now_secs(), body.clone())); if v.len() > 50 { v.remove(0); } }
        return Some(if body.is_empty() { "Fusion Electronics has detected an unexpected condition".into() } else { body });
    }
    None
}
/// `_dismiss_save_prompt_if_bridge_doc`: Don't Save for OUR documents only; a user's prompt is never touched.
pub fn dismiss_save_prompt_if_bridge_doc(state_dir: &Path, caller: &Caller, pids: &[u32], main_title: &str) -> Value {
    let Some(ab) = Loopback::from_env() else { return json!({"handled": false, "why": "no ab loopback"}) };
    let prompt = find_save_prompt(&ab, caller, pids);
    if prompt["found"].as_bool() != Some(true) { return json!({"handled": false, "why": "no save prompt"}); }
    let hwnd = prompt["hwnd"].as_i64().unwrap_or(0);
    let text = prompt["text"].as_str().unwrap_or("").to_string();
    let matched = bridge_doc_in(state_dir, &text).or_else(|| bridge_doc_in(state_dir, main_title));
    let Some(matched) = matched else {
        // an unattributed prompt: only if the main thread is actually blocked, and only after
        // two sightings, answer Cancel (WM_CLOSE: non-destructive, the doc stays open unsaved)
        let st = crate::addin::addin_status_probe(Duration::from_secs(2));
        let blocked = st.as_ref().map(|s| s["busy"].as_bool().unwrap_or(false)).unwrap_or(true);
        if !blocked { return json!({"handled": false, "why": "unattributed dialog but the main thread is responsive - zombie window, leaving it alone", "hwnd": hwnd}); }
        let n = { let mut g = STRIKES.lock().unwrap(); let e = g.entry(hwnd).or_insert(0); *e += 1; *e };
        if n >= 2 {
            if ab.call("desktop_close_window", json!({"hwnd": hwnd}), caller, 15).is_ok() {
                eprintln!("[fusion-bridge] save-prompt sentinel: cancelled an unattributed prompt after {n} sightings (WM_CLOSE - non-destructive, main thread freed)");
                STRIKES.lock().unwrap().remove(&hwnd);
                return json!({"handled": true, "why": format!("escaped unattributed prompt (Cancel) after {n} sightings")});
            }
        }
        return json!({"handled": false, "why": format!("unattributed save prompt - strike {n} of 2 before Escape"), "hwnd": hwnd});
    };
    if let Some(b) = prompt["button"].as_str() {
        if let Ok(r) = ab.call("desktop_ui_click", json!({"hwnd": hwnd, "name": b}), caller, 15) {
            if r["invoked"].as_bool().unwrap_or(false) || r["success"].as_bool().unwrap_or(false) {
                eprintln!("[fusion-bridge] save-prompt sentinel: clicked {b:?} for bridge doc {matched:?} (background)");
                return json!({"handled": true, "why": format!("clicked '{b}' for bridge doc '{matched}'")});
            }
        }
    }
    if let Ok(r) = ab.call("desktop_ui_click", json!({"hwnd": hwnd, "name": "Don't Save"}), caller, 15) {
        if r["invoked"].as_bool().unwrap_or(false) || r["success"].as_bool().unwrap_or(false) {
            crate::fusion_prefs::action_ledger(state_dir, "save_prompt_dismiss", json!({"doc": matched, "how": "dont_save"}));
            return json!({"handled": true, "why": format!("clicked Don't Save for bridge doc '{matched}'")});
        }
    }
    if ab.call("desktop_close_window", json!({"hwnd": hwnd}), caller, 15).is_ok() {
        crate::fusion_prefs::action_ledger(state_dir, "save_prompt_dismiss", json!({"doc": matched, "how": "escape"}));
        return json!({"handled": true, "why": format!("escaped (Cancel) for bridge doc '{matched}'")});
    }
    json!({"handled": false, "why": "dismiss attempts did not land", "hwnd": hwnd})
}

static STRIKES: std::sync::Mutex<std::collections::BTreeMap<i64, u32>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// `_SavePromptSentinel`: poll for the Save prompt while a doc-switching call runs.
pub struct Sentinel { stop: Arc<AtomicBool>, deadline: Arc<std::sync::Mutex<Instant>> }
impl Sentinel {
    pub fn start(state_dir: std::path::PathBuf, caller: Caller, pids: Vec<u32>, max_seconds: f64) -> Sentinel {
        let stop = Arc::new(AtomicBool::new(false));
        let deadline = Arc::new(std::sync::Mutex::new(Instant::now() + Duration::from_secs_f64(max_seconds.max(5.0))));
        let (s2, d2) = (stop.clone(), deadline.clone());
        std::thread::spawn(move || {
            while !s2.load(Ordering::Relaxed) && Instant::now() < *d2.lock().unwrap() {
                let _ = dismiss_save_prompt_if_bridge_doc(&state_dir, &caller, &pids, "");
                if let Some(ab) = Loopback::from_env() { let _ = dismiss_unexpected_condition(&ab, &caller, &pids); }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        Sentinel { stop, deadline }
    }
    /// keep covering the just-returned call for a short grace (the post-timeout prompt is the wedge)
    pub fn soft_stop(&self, grace: f64) { let mut d = self.deadline.lock().unwrap(); let g = Instant::now() + Duration::from_secs_f64(grace); if g < *d { *d = g; } }
    #[allow(dead_code)]
    pub fn hard_stop(&self) { self.stop.store(true, Ordering::Relaxed); }
}

const BACKGROUND_NOTE: &str = "Window opened/changed in the BACKGROUND - the user's focus was NOT touched; do NOT foreground anything to compensate. Render results via fusion_state. If the USER asks to watch, re-call with {\"foreground\": true, \"foregroundReason\": \"<why, shown to the user as a 3s caption>\"}.";

/// `_window_etiquette_pre` plus the document snapshot and the long-running pre-dismiss.
pub fn pre(host: &dyn fusion_core::HostProcess, state_dir: &Path, caller: &Caller, cmd: &str, args: &Value, pids: &[u32], call: &dyn Fn(&str, Value, u64) -> Value) -> Pre {
    let window_raising = crate::addin::WINDOW_RAISING.contains(&cmd);
    let docs_before = if window_raising { Some(snapshot_open_docs(call)) } else { None };
    let fg = args["foreground"].as_bool().unwrap_or(false) || (args["foreground"].is_string() && !args["foreground"].as_str().unwrap_or("").is_empty());
    let mut out = Pre { args: args.clone(), ran_backgrounded: false, refusal: None, docs_before, fg };
    if window_raising {
        if fg {
            let reason = args["foregroundReason"].as_str().unwrap_or("").trim().to_string();
            if reason.is_empty() {
                note_focus_event(state_dir, cmd, "refused", "");
                out.refusal = Some(json!({"success": false, "errorCode": "foreground_reason_required",
                    "error": "foreground:true needs foregroundReason (a short user-facing sentence).",
                    "_hint": "Fusion verbs ALWAYS run in the background so the user is never interrupted mid-typing. If the user genuinely wants to WATCH, re-call with foreground:true AND foregroundReason:\"<why, in words the user will read>\" - it is shown as an ab caption for 3 seconds before the window comes forward, so they can yield."}));
                return out;
            }
            note_focus_event(state_dir, cmd, "foreground", &reason);
            if let Some(ab) = Loopback::from_env() {
                let _ = ab.call("notify_user", json!({"title": "Fusion is coming to the foreground", "body": format!("{reason} (Requested by your AI - Fusion verbs otherwise always run in the background.)"), "level": "info", "target": "all"}), caller, 10);
                std::thread::sleep(Duration::from_secs(3));
            }
            if let Some(o) = out.args.as_object_mut() { o.remove("foreground"); o.remove("foregroundReason"); }
        } else {
            let secs = crate::addin::timeout_secs_or(cmd, 60) as f64 + 20.0;
            host.background_envelope(secs, cmd == "start");
            if crate::small_verbs::get_pref(state_dir, "showDrivingBadge").map(|v| v != json!(false)).unwrap_or(true) {
                // the SAME small PNG badge as set_badge (John, 2026-09-07: this path painted ab's
                // built-in "adom" badge, which draws far larger than pup's overlays and than the
                // Python bridge ever did), throttled like the Python bridge's set_driving_badge
                if let Some(ab) = Loopback::from_env() { if let Some(h) = fusion_core::windows_facts::facts_from_ab(&ab, caller).main_hwnd { set_badge_throttled(caller, h); } }
            *LAST_DRIVE_AT.lock().unwrap() = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            }
            out.ran_backgrounded = true;
        }
    }
    if ["walk_cloud_tree", "search_cloud_files"].contains(&cmd) {
        if let Some(ab) = Loopback::from_env() {
            let dialogs = crate::small_verbs::classify_blocking_dialogs(&ab, caller, pids);
            for d in &dialogs { if let Some(h) = d["hwnd"].as_i64() { let _ = ab.call("desktop_close_window", json!({"hwnd": h}), caller, 10); } }
            if !dialogs.is_empty() { std::thread::sleep(Duration::from_millis(500)); }
        }
    }
    out
}

/// `_window_etiquette_post` plus the recent-actions ledger on a failed slow verb.
pub fn post(state_dir: &Path, caller: &Caller, cmd: &str, args: &Value, mut res: Value, pre: &Pre, since: f64, pids: &[u32], call: &dyn Fn(&str, Value, u64) -> Value) -> Value {
    let window_raising = crate::addin::WINDOW_RAISING.contains(&cmd);
    let failed = res["success"].as_bool() == Some(false);
    if failed && (window_raising || ["walk_cloud_tree", "search_cloud_files"].contains(&cmd)) {
        let acts = ledger_entries_since(state_dir, since, 12);
        if !acts.is_empty() && res.is_object() {
            if res.get("recentActions").is_none() { res["recentActions"] = json!(acts); }
            if res.get("_ledgerHint").is_none() { res["_ledgerHint"] = json!("recentActions = every background action the bridge took on the host app while this verb ran (newest last); fusion_action_ledger {sinceSeconds} for more."); }
        }
    }
    // Fusion's own router assertion modal blocks the main thread; clear it BEFORE the document
    // snapshot below asks the add-in anything (2026-09-07: that snapshot hung 19 s per verb and
    // the routing test read as an add-in crash)
    if window_raising {
        if let Some(ab) = Loopback::from_env() {
            // the assertion lands a moment AFTER the command returns: look for it a few times
            let mut found = None;
            for i in 0..4 { found = dismiss_unexpected_condition(&ab, caller, pids); if found.is_some() { break; } if i < 3 { std::thread::sleep(Duration::from_millis(300)); } }
            let _ = found.take();
            // every assertion clicked while this verb ran (the sentinel's too): a harmless routing one is
            // noted; any other one fails the verb (#135: PlacePartWorker assetAdded = the ADD did not land)
            let seen: Vec<String> = ASSERTS_SEEN.lock().map(|v| v.iter().filter(|(t, _)| *t >= since).map(|(_, b)| b.clone()).collect()).unwrap_or_default();
            let fatal: Vec<&String> = seen.iter().filter(|b| !assert_is_harmless(b)).collect();
            if !fatal.is_empty() && res.is_object() {
                res["success"] = json!(false);
                if res["errorCode"].is_null() || res["errorCode"] == json!("") { res["errorCode"] = json!("fusion_assert"); }
                res["fusionAsserts"] = json!(seen);
                let e = res["error"].as_str().unwrap_or("").to_string();
                res["error"] = json!(format!("Fusion raised an internal error during this verb ({}); the bridge clicked OK so Fusion is not blocked, but the command did not complete. {e}", fatal[0].chars().take(160).collect::<String>()).trim().to_string());
            }
            let found = seen.into_iter().last();
            if let Some(body) = found { if res.is_object() { res["fusionUnexpectedCondition"] = json!(body); let h = res["_hint"].as_str().unwrap_or("").to_string(); res["_hint"] = json!(format!("{h} Fusion Electronics raised its 'unexpected condition' assertion during this verb and the bridge clicked OK; checkpoint the board now (fusion_export_eagle_source) and verify the copper with fusion_routing_state.").trim().to_string()); } }
        }
    }
    if let Some(before) = &pre.docs_before {
        // a short, undiagnosed add-in hop: a hung main thread must cost 3 s here, not the 5 s
        // call plus a 14 s timeout diagnosis (2026-09-07, the routing test's 60 s relay window)
        let quick = |verb: &str, a: Value, _t: u64| crate::addin::addin_http(verb, &a, 3);
        let _ = call;
        for (name, cloud) in snapshot_open_docs_detail(&quick) {
            if before.contains(&name) || name.is_empty() || name == "untitled" { continue; }
            if appeared_doc_is_scratch(cmd, cloud) { crate::chains::note_bridge_doc_pub(state_dir, &name); eprintln!("[fusion-bridge] registered {name:?} as a bridge doc (it appeared while we drove)"); }
            else { eprintln!("[fusion-bridge] {name:?} appeared during {cmd} but is an existing document someone opened, not bridge scratch"); }
        }
    }
    if SHOW_SURFACE_VERBS.contains(&cmd) {
        for k in ["name", "deviceset", "libraryPath", "fileName", "query"] { if let Some(v) = args[k].as_str().filter(|s| !s.is_empty()) { crate::chains::note_bridge_doc_pub(state_dir, v); } }
        if args["capture"].as_bool() == Some(true) && res["success"].as_bool() != Some(false) { res["shot"] = capture_surface_shot(caller, cmd, args, call); }
    }
    if window_raising {
        let main_title = Loopback::from_env().and_then(|ab| fusion_core::windows_facts::facts_from_ab(&ab, caller).main_title).unwrap_or_default();
        let sw = dismiss_save_prompt_if_bridge_doc(state_dir, caller, pids, &main_title);
        if sw["handled"].as_bool() == Some(true) && res.is_object() { res["savePromptCleared"] = sw["why"].clone(); }
        if let Some(ab) = Loopback::from_env() {
            for d in crate::small_verbs::classify_blocking_dialogs(&ab, caller, pids) {
                let (cat, h) = (d["category"].as_str().unwrap_or(""), d["hwnd"].as_i64().unwrap_or(0));
                match cat {
                    "upload_in_progress" => { let _ = ab.call("desktop_ui_click", json!({"hwnd": h, "name": "No"}), caller, 12); if res.is_object() { res["uploadCloseDeclined"] = json!(true); } eprintln!("[fusion-bridge] answered the Hub upload-close confirm with No (work-preserving; close retried after jobs drain)"); }
                    "create_confirm" => { let _ = ab.call("desktop_ui_click", json!({"hwnd": h, "name": "No"}), caller, 12); if res.is_object() { res["createPromptDeclined"] = json!(true); } eprintln!("[fusion-bridge] declined a standing Create-new prompt at the chokepoint (raised by a raw EDIT; Yes would fabricate an empty artifact)"); }
                    "launch_error" | "lbr_open_error" | "fusion_unexpected_condition" | "board_opened_with_warnings" => { let _ = ab.call("desktop_ui_click", json!({"hwnd": h, "name": "OK"}), caller, 12); eprintln!("[fusion-bridge] dismissed a standing {cat} dialog at the chokepoint"); }
                    _ => {}
                }
            }
        }
        if res.is_object() {
            if pre.fg { if res.get("windowEtiquette").is_none() { res["windowEtiquette"] = json!("foreground"); } }
            else {
                if res.get("windowEtiquette").is_none() { res["windowEtiquette"] = json!("background"); }
                let h = res["_hint"].as_str().map(str::to_string);
                if !h.as_deref().map(|h| h.contains("focus was NOT touched")).unwrap_or(false) { res["_hint"] = json!(match h { Some(h) => format!("{h} {BACKGROUND_NOTE}"), None => BACKGROUND_NOTE.to_string() }); }
            }
        }
    }
    res
}

/// `_ledger_entries_since`.
pub fn ledger_entries_since(state_dir: &Path, t0: f64, limit: usize) -> Vec<Value> {
    let Ok(t) = std::fs::read_to_string(state_dir.join("actions.jsonl")) else { return vec![] };
    let lines: Vec<&str> = t.lines().collect();
    let tail = &lines[lines.len().saturating_sub(400)..];
    let out: Vec<Value> = tail.iter().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|e| e["epoch"].as_f64().unwrap_or(0.0) >= t0 - 1.0).collect();
    out[out.len().saturating_sub(limit)..].to_vec()
}

/// `_capture_surface_shot`: settle on the add-in going idle, then the frame ab already resized.
fn capture_surface_shot(caller: &Caller, verb: &str, args: &Value, call: &dyn Fn(&str, Value, u64) -> Value) -> Value {
    let label = match verb { "show_symbol" => "Symbol", "show_footprint" => "Footprint", "show_3d_package" => "3D package", "show_library" => "Library", "show_project" => "Project", "show_schematic" => "Schematic", "show_2d_board" => "2D board", "show_3d_board" => "3D board", v => v };
    let deadline = Instant::now() + Duration::from_secs_f64(args["captureTimeoutSeconds"].as_f64().unwrap_or(120.0));
    while Instant::now() < deadline { if call("get_app_state", json!({}), 6)["success"].as_bool() == Some(true) { break; } std::thread::sleep(Duration::from_secs(3)); }
    std::thread::sleep(Duration::from_secs_f64(args["capturePaintDelay"].as_f64().unwrap_or(2.5)));
    let shot = crate::small_verbs::screenshot_fusion(&json!({}), caller);
    let Some(path) = shot["savedTo"].as_str().filter(|_| shot["success"].as_bool() == Some(true)).map(str::to_string) else { return json!({"title": label, "image": Value::Null, "error": shot["error"].as_str().unwrap_or("capture failed")}) };
    let max_w = (args["captureMaxWidth"].as_i64().unwrap_or(640)).clamp(120, 1600);
    // the ~640px frame Python resamples is served here as ab's own safe-size copy (<=1400px)
    let small = shot["safePath"].as_str().map(str::to_string).unwrap_or_else(|| path.clone());
    let img = std::fs::read(&small).ok().map(|b| b64(&b));
    json!({"title": label, "image": img, "path": path, "maxWidth": max_w, "thumbnailDataUrl": img.as_ref().map(|i| format!("data:image/png;base64,{i}"))})
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char); out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' }); out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn bridge_doc_text_and_ledger_window() {
        let d = std::env::temp_dir().join(format!("fb-etq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        crate::small_verbs::set_pref(&d, "bridgeDocs", serde_json::json!(["demo_board", "lan7800"]));
        assert_eq!(super::bridge_doc_in(&d, "Save changes to Demo_Board?").as_deref(), Some("demo_board"));
        assert_eq!(super::bridge_doc_in(&d, "ADOM-TEST-42 has unsaved changes").as_deref(), Some("adom-test-"));
        assert_eq!(super::bridge_doc_in(&d, "MyRobotArm v3"), None);
        crate::fusion_prefs::action_ledger(&d, "x", serde_json::json!({"k": 1}));
        assert_eq!(super::ledger_entries_since(&d, 0.0, 12).len(), 1);
        assert_eq!(super::ledger_entries_since(&d, 4e9, 12).len(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn opened_documents_are_not_bridge_scratch() {
        use super::appeared_doc_is_scratch as scratch;
        // 2026-10-28: four of the user's assemblies, reopened with open_by_urn, were registered
        assert!(!scratch("open_by_urn", true));
        assert!(!scratch("run_modeling_script", true));
        // Fusion's own launch (recovered documents) is never the bridge's
        assert!(!scratch("start", false));
        // what a verb made is scratch, and so is the bridge's own Save As
        assert!(scratch("import_step", false));
        assert!(scratch("make_3d_package", false));
        assert!(scratch("save_to_cloud", true));
    }
    #[test]
    fn b64_matches_python() { assert_eq!(super::b64(b"hello"), "aGVsbG8="); assert_eq!(super::b64(b""), ""); assert_eq!(super::b64(b"ab"), "YWI="); }
}

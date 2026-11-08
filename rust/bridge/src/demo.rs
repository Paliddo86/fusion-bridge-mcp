//! `fusion_demo` (the staged first-time-user demo and the async six-content-type tour),
//! `fusion_demo_progress`, `fusion_demo_control`. The tour runs detached on a fresh context,
//! narrates each step with a desktop caption, answers the desktop remote (play, pause, next,
//! back, stop) between steps, and snapshots to disk so the done frame survives a respawn.
use crate::addin::Ctx;
use fusion_core::ab::{AbClient, Loopback};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub static DEMO_TOURS: Mutex<Option<HashMap<String, Value>>> = Mutex::new(None);
const TOUR_PANEL_ID: &str = "fusion-tour";
const VIEW_STEPS: &[&str] = &["symbol", "footprint", "device", "schematic", "2d-board", "3d-board", "library"];
const TOUR_PRESET_ELECTRONICS: &[&str] = &["launch", "stage", "library", "symbol", "footprint", "3d-package", "device", "make-project", "open-design", "schematic", "2d-board", "3d-board"];

fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }
fn sleep_s(s: f64) { std::thread::sleep(Duration::from_secs_f64(s)); }
fn ab_call(caller: &fusion_core::ab::Caller, verb: &str, args: Value, timeout: u64) -> Value {
    match Loopback::from_env() { Some(ab) => match ab.call(verb, args, caller, timeout) { Ok(v) => v, Err(e) => json!({"_adCallError": format!("{e:?}")}) }, None => json!({"_adCallError": "ab loopback unavailable"}) }
}
fn tour_explain(key: &str) -> Option<&'static str> {
    Some(match key {
        "launch" => "Starting Fusion in the background. The bridge launches it detached and minimized, takes the one Autodesk seat if another machine holds it, and never steals your focus - and the AdomBridge add-in loads itself inside Fusion and opens a local command port the bridge drives everything through.",
        "stage" => "Staging the sample part. This library was built from open KiCad source by adom dash l b r and lands on disk as a real EAGLE library file Fusion can open natively.",
        "library" => "Opening the electronics library. The bridge drives Fusion's own Electronics Library editor through the add-in - no clicks, no foreground, the same path a human would take.",
        "symbol" => "The symbol editor: the part's full pinout, converted pin for pin from KiCad source. The add-in switches views with Fusion's own edit commands.",
        "footprint" => "The footprint editor: real pads and layers, exactly the copper a board would get - never a placeholder.",
        "device" => "The component view: every symbol pin mapped to its footprint pad. The bridge verifies this mapping offline before Fusion ever opens the file, so dead wiring cannot photograph well.",
        "3d-package" => "The 3D chip - the showpiece. The bridge imports the part's STEP model onto the footprint in Fusion's 3D package generator, orients it flat automatically, checks pin one against pad one, saves it into your Autodesk cloud hub, and binds the resulting cloud model into the library, so every future board carries the real body. This takes a couple of minutes; it is real work, not a preview.",
        "datahub" => "Your Fusion Data Hub, through the add-in: hubs, projects and folders listed over Fusion's own cloud API - the same storage the 3D chip was just saved into.",
        "aps" => "APS cloud search: the bridge's own server-indexed search over your Fusion cloud files, about two seconds per query after a single sign-in, and never charged.",
        "mcp" => "Autodesk's official MCP server, answering over the bridge. Useful - but its toolset is small and read-mostly. The bridge's one hundred plus fusion verbs go far beyond it: electronics editors, text commands, cloud files, evidence captures and a dozen export formats.",
        "make-project" => "Generating a complete test project around the part: schematic and board authored from EAGLE source and instantiated by Fusion, entirely in the background.",
        "open-design" => "Opening the cloud electronics design from your hub - the bridge resolves it by name and opens the parent design, never a child view.",
        "schematic" => "The schematic view of the generated project - the part wired into a real sheet.",
        "2d-board" => "The 2D board layout - the same footprint you saw in the library, now placed on copper.",
        "3d-board" => "The 3D board - and the chip body on it is the exact cloud model the bridge minted and bound earlier in this tour.",
        "3d-board-unbound" => "The 3D board. The 3D bind did not complete on this run, so the chip shows Fusion's placeholder outline here; the library import chain binds the real body.",
        "exports" => "Exports: STEP and STL written straight from the add-in. The same pipeline covers F 3 D, G L B, 3 M F, IGES, SAT, D X F, gerbers and BOM - every format a downstream tool could ask for.",
        _ => return None,
    })
}

fn tours_dir(state_dir: &std::path::Path) -> std::path::PathBuf { let d = state_dir.join("tours"); let _ = std::fs::create_dir_all(&d); d }
/// `_demo_tour_persist`: snapshot a FINISHED tour so its done frame is pollable across respawns; keep the last 8.
pub fn sample_lbr_pub() -> Option<String> { sample_lbr() }
pub fn load_pub(state_dir: &std::path::Path, id: &str) -> Option<Value> { load(state_dir, id) }
pub fn persist_pub(state_dir: &std::path::Path, state: &Value) { persist(state_dir, state) }
fn persist(state_dir: &std::path::Path, state: &Value) {
    let d = tours_dir(state_dir);
    let mut snap = state.clone(); if let Some(o) = snap.as_object_mut() { o.remove("started"); }
    let _ = std::fs::write(d.join(format!("{}.json", state["demoId"].as_str().unwrap_or("tour"))), snap.to_string());
    let mut old: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(&d).map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("tour-") && e.path().extension().map(|x| x == "json").unwrap_or(false)).filter_map(|e| e.metadata().ok().and_then(|m| m.modified().ok()).map(|t| (t, e.path()))).collect()).unwrap_or_default();
    old.sort();
    if old.len() > 8 { for (_, p) in &old[..old.len() - 8] { let _ = std::fs::remove_file(p); } }
}
fn load(state_dir: &std::path::Path, id: &str) -> Option<Value> { std::fs::read_to_string(tours_dir(state_dir).join(format!("{id}.json"))).ok().and_then(|t| serde_json::from_str(&t).ok()) }
fn get_state(id: &str) -> Option<Value> {
    let g = DEMO_TOURS.lock().unwrap(); let m = g.as_ref()?;
    if let Some(s) = m.get(id) { return Some(s.clone()); }
    if id.is_empty() && m.len() == 1 { return m.values().next().cloned(); }
    None
}
fn set_state(id: &str, s: Value) { let mut g = DEMO_TOURS.lock().unwrap(); g.get_or_insert_with(HashMap::new).insert(id.to_string(), s); }
fn update_state(id: &str, f: impl FnOnce(&mut Value)) { let mut g = DEMO_TOURS.lock().unwrap(); if let Some(s) = g.get_or_insert_with(HashMap::new).get_mut(id) { f(s); } }
fn push_to(s: &mut Value, key: &str, v: Value) { let mut a = s[key].as_array().cloned().unwrap_or_default(); a.push(v); s[key] = json!(a); }
/// `_shot_b64_small`: an evidence frame small enough for the relay envelope (the raw file when it is).
pub fn shot_b64_small(path: &str) -> Option<String> {
    let raw = std::fs::read(path).ok()?;
    if raw.len() <= 1_500_000 { Some(crate::etiquette::b64_pub(&raw)) } else { None }
}
fn sample_lbr() -> Option<String> {
    let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()))?;
    let src = dir.join("resources").join("samples").join("ADS8588SIPM.fusion.lbr");
    if !src.is_file() { return None; }
    let d = std::env::temp_dir().join("adom-fusion-wrap"); let _ = std::fs::create_dir_all(&d);
    let step = src.with_file_name("ADS8588SIPM.step"); if step.is_file() { let _ = std::fs::copy(&step, d.join("AdomDemoSample.step")); }
    let dst = d.join("AdomDemoSample.lbr");
    std::fs::copy(&src, &dst).ok()?;
    Some(dst.to_string_lossy().replace('\\', "/"))
}
fn path_sibling(path: &str, ext: &str) -> Option<String> { let p = std::path::Path::new(path).with_extension(ext.trim_start_matches('.')); if p.is_file() { Some(p.to_string_lossy().replace('\\', "/")) } else { None } }

/// `fusion_demo_progress`.
pub fn demo_progress(state_dir: &std::path::Path, args: &Value) -> Value {
    let id = args["demoId"].as_str().unwrap_or("").to_string();
    let mut persisted = false;
    let state = match get_state(&id) { Some(s) => Some(s), None if !id.is_empty() => { let l = load(state_dir, &id); persisted = l.is_some(); l } None => None };
    let Some(state) = state else { return json!({"success": false, "error": format!("unknown demoId {id}"), "_hint": "Start a tour first: fusion_demo {\"mode\": \"tour\"}. Its response carries the demoId. Finished tours stay pollable (disk snapshot) even across bridge respawns - an unknown id here means it never ran on this box."}) };
    let mut out = json!({"success": true});
    if let Some(o) = state.as_object() { for (k, v) in o { if k != "started" { out[k] = v.clone(); } } }
    if persisted { out["fromSnapshot"] = json!(true); }
    if !persisted && args["sinceShot"].is_null() {
        let sent = state["stepShotSent"].as_u64().unwrap_or(0) as usize;
        let all = state["shots"].as_array().cloned().unwrap_or_default();
        if sent < all.len() {
            let s = &all[sent];
            let img = shot_b64_small(s["path"].as_str().unwrap_or(""));
            update_state(&id, |st| st["stepShotSent"] = json!(sent + 1));
            if let Some(img) = img { out["stepShot"] = json!({"step": s["label"], "title": if s["title"].is_null() { s["label"].clone() } else { s["title"].clone() }, "image": img}); }
        }
    }
    let mut shots = out["shots"].as_array().cloned().unwrap_or_default();
    out["shotCount"] = json!(shots.len());
    let since = args["sinceShot"].as_u64().unwrap_or(0) as usize;
    if since > 0 { shots = shots.into_iter().skip(since).collect(); }
    let imgs = args["imagesAs"].as_str().map(str::to_string).unwrap_or_else(|| if state["done"].as_bool() == Some(true) { "base64".into() } else { "paths".into() });
    if imgs == "base64" || imgs == "both" {
        shots = shots.into_iter().map(|mut s| { if let Some(raw) = s["path"].as_str().and_then(|p| std::fs::read(p).ok()) { if raw.len() <= 4 * 1024 * 1024 { s["image"] = json!(crate::etiquette::b64_pub(&raw)); } else { s["imageNote"] = json!(format!("capture is {}MB; pull_file the path for full size", raw.len() / (1024 * 1024))); } } s }).collect();
    }
    out["shots"] = json!(shots);
    if state["done"].as_bool() != Some(true) { out["_hint"] = json!(format!("Tour running ({}). shots[] carries evidence for every COMPLETED step already - render it live; pass sinceShot={} on the next poll to receive only new shots.", state["stepLabel"].as_str().unwrap_or(""), out["shotCount"])); }
    out
}

/// `fusion_demo_control`: pause | resume | next | prev | exit, at the next step boundary.
pub fn demo_control(args: &Value) -> Value {
    let id = args["demoId"].as_str().unwrap_or("").to_string();
    let action = args["action"].as_str().unwrap_or("").to_lowercase();
    let Some(state) = get_state(&id).filter(|s| s["done"].as_bool() != Some(true)) else { return json!({"success": false, "error": format!("no running tour{}", if id.is_empty() { String::new() } else { format!(" with id {id}") }), "_hint": "Controls only apply while a tour is running."}) };
    let real_id = state["demoId"].as_str().unwrap_or("").to_string();
    let mut paused = false;
    let ok = match action.as_str() {
        "pause" => { update_state(&real_id, |s| s["paused"] = json!(true)); paused = true; true }
        "resume" => { update_state(&real_id, |s| s["paused"] = json!(false)); true }
        "next" => { update_state(&real_id, |s| { s["skipNext"] = json!(true); s["paused"] = json!(false); }); true }
        "prev" => { update_state(&real_id, |s| { s["replayPrev"] = json!(true); s["paused"] = json!(false); }); true }
        "exit" => { update_state(&real_id, |s| { s["abort"] = json!(true); s["paused"] = json!(false); }); true }
        _ => false,
    };
    if !ok { return json!({"success": false, "error": "action must be pause|resume|next|prev|exit"}); }
    json!({"success": true, "action": action, "paused": paused, "_hint": "Takes effect at the next step boundary - a running Fusion verb finishes first."})
}

/// What the tour thread needs from the bridge around it (owned; the thread outlives the request).
pub struct TourEnv { pub readiness: Arc<dyn Fn() -> Value + Send + Sync>, pub aps_state: Arc<dyn Fn() -> (bool, bool) + Send + Sync>, pub version: &'static str }

fn panel_show(caller: &fusion_core::ab::Caller, id: &str, stops: &[String], stage: usize, playing: bool) {
    let _ = ab_call(caller, "desktop_demo_panel", json!({"action": "show", "id": TOUR_PANEL_ID, "title": "Fusion bridge tour", "stops": stops, "stage": stage, "state": if playing { "playing" } else { "paused" }, "position": "bottom-right", "reason": "Guided Fusion tour: this remote pauses, skips, or replays the tour steps"}), 10);
    update_state(id, |s| s["remote"] = json!({"shown": true, "stage": stage}));
}
fn panel_pump(caller: &fusion_core::ab::Caller, id: &str) -> Vec<String> {
    let r = ab_call(caller, "desktop_demo_panel", json!({"action": "poll", "id": TOUR_PANEL_ID}), 8);
    let events = crate::managed::find_key(&r, "events", 0).and_then(|e| e.as_array().cloned()).unwrap_or_default();
    let seen: Vec<String> = events.iter().map(|e| e["button"].as_str().unwrap_or("").to_lowercase()).collect();
    if !seen.is_empty() { update_state(id, |s| { for b in &seen { match b.as_str() { "pause" => s["paused"] = json!(true), "play" => s["paused"] = json!(false), "next" => { s["skipNext"] = json!(true); s["paused"] = json!(false); } "back" => { s["replayPrev"] = json!(true); s["paused"] = json!(false); } "stop" => { s["abort"] = json!(true); s["paused"] = json!(false); } _ => {} } } let mut p = s["remotePresses"].as_array().cloned().unwrap_or_default(); p.extend(seen.iter().rev().take(8).rev().map(|b| json!(b))); s["remotePresses"] = json!(p); }); }
    seen
}
fn tour_caption(caller: &fusion_core::ab::Caller, text: &str, seconds: u64) { let _ = ab_call(caller, "desktop_caption", json!({"text": text, "id": TOUR_PANEL_ID, "duration": (seconds * 2000).clamp(15000, 240000)}), 10); }
fn take_flag(id: &str, key: &str) -> bool { let mut got = false; update_state(id, |s| { if s[key].as_bool() == Some(true) { got = true; s.as_object_mut().map(|o| o.remove(key)); } }); got }

/// `fusion_demo {mode:"tour"}`: start the async tour; poll fusion_demo_progress {demoId}.
pub fn demo_tour(ctx: &Ctx, args: &Value, env: TourEnv, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut idb = [0u8; 4]; let _ = getrandom::getrandom(&mut idb);
    let demo_id = format!("tour-{:08x}", u32::from_le_bytes(idb));
    let include_design = args["includeDesign"].as_bool().unwrap_or(true);
    let query = args["query"].as_str().unwrap_or("").to_string();
    let cold = pass("get_app_state", json!({}))["success"].as_bool() != Some(true);
    let sd = ctx.state_dir.clone();
    let est = |phase: &str, seeded: f64| -> u64 { crate::chains::estimate(&sd, phase, seeded).0.round() as u64 };
    let mut steps: Vec<(String, String, u64)> = Vec::new();
    if cold { steps.push(("launch".into(), "Starting Fusion (cold start)".into(), est("launch", 120.0))); }
    for (k, l, s) in [("stage", "Staging the sample library", 3.0), ("library", "Opening the electronics library", 35.0), ("symbol", "Symbol: full pinout in the symbol editor", 8.0), ("footprint", "Footprint: pads + layers in the footprint editor", 8.0), ("device", "Component view: symbol + package mapping", 8.0), ("3d-package", "3D chip: minted from the STEP, saved to your hub, bound into the library", 150.0), ("datahub", "Data Hub: your cloud projects through the add-in", 10.0), ("aps", "APS cloud search: ~2s indexed lookups", 8.0), ("mcp", "MCP: Autodesk's own AI server (and its limits)", 10.0)] { steps.push((k.into(), l.into(), est(k, s))); }
    if include_design {
        if !query.is_empty() { steps.push(("open-design".into(), "Opening the cloud electronics design".into(), est("open-design", 150.0))); } else { steps.push(("make-project".into(), "Generating + importing a test project around the sample part".into(), est("make-project", 60.0))); }
        for (k, l, s) in [("schematic", "Schematic view", 12.0), ("2d-board", "2D board layout", 12.0), ("3d-board", "3D board", 20.0), ("exports", "Exporting: STEP + STL (of a dozen formats)", 25.0)] { steps.push((k.into(), l.into(), est(k, s))); }
    }
    if args["preset"].as_str().map(|p| p.to_lowercase()) == Some("electronics".into()) {
        let keep = TOUR_PRESET_ELECTRONICS;
        let mut kept: Vec<_> = steps.into_iter().filter(|s| keep.contains(&s.0.as_str())).collect();
        kept.sort_by_key(|s| keep.iter().position(|k| *k == s.0).unwrap_or(99));
        steps = kept;
    }
    let remote = args["remote"].as_bool().unwrap_or(false);
    let total_est: u64 = steps.iter().map(|s| s.2).sum();
    let state = json!({"demoId": demo_id, "percent": 0, "elapsedSec": 0, "estimatedSec": total_est, "step": steps[0].0, "stepLabel": steps[0].1, "done": false, "failed": false, "steps": steps.iter().map(|s| s.0.clone()).collect::<Vec<_>>(), "shots": [], "started": now()});
    set_state(&demo_id, state);
    let (id, caller, args2, steps2) = (demo_id.clone(), ctx.caller.clone(), args.clone(), steps.clone());
    let fg = args["foreground"].as_bool().unwrap_or(false);
    let captions = args["captions"].as_bool().unwrap_or(true);
    let sd2 = sd.clone();
    std::thread::spawn(move || {
        let host = crate::host_for_diagnosis();
        let side_url = crate::launch::SIDECAR_URL.lock().unwrap().clone();
        let side = move |verb: &str, a: Value, t: u64| -> Value { match &side_url { Some(u) => ureq::post(u).timeout(Duration::from_secs(t)).send_json(json!({"command": verb, "args": a})).ok().and_then(|r| r.into_json().ok()).unwrap_or(json!({"success": false, "error": "sidecar unreachable"})), None => json!({"success": false, "error": "no sidecar"}) } };
        let pids: Vec<u32> = host.fusion_processes().iter().map(|p| p.pid).collect();
        let dlg = |r: Value| r;
        let ctx2 = Ctx { host: &*host, fusion_exe: crate::host_for_diagnosis_paths(), short_path: |p| crate::host().short_path(p), state_dir: sd2.clone(), caller: caller.clone(), pids: pids.clone(), sidecar: &side, dialogs: &dlg };
        let pass2 = |verb: &str, a: Value| crate::addin::pass_through(verb, &a, &ctx2);
        let started = now();
        if !fg { host.background_envelope(total_est as f64 + 60.0, false); }
        let docs_before = crate::orchestrations::open_doc_names(&pass2);
        let stop_labels: Vec<String> = steps2.iter().map(|s| s.1.clone()).collect();
        let pump_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let step_ref = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        if remote {
            panel_show(&caller, &id, &stop_labels, 0, true);
            let (ps, sr, c2, id2, labels) = (pump_stop.clone(), step_ref.clone(), caller.clone(), id.clone(), stop_labels.clone());
            std::thread::spawn(move || { let mut last: Option<bool> = None; while !ps.load(std::sync::atomic::Ordering::Relaxed) { let seen = panel_pump(&c2, &id2); let face = get_state(&id2).map(|s| s["paused"].as_bool() == Some(true)).unwrap_or(false); if !seen.is_empty() || Some(face) != last { panel_show(&c2, &id2, &labels, sr.load(std::sync::atomic::Ordering::Relaxed).saturating_sub(1), !face); last = Some(face); } sleep_s(2.0); } });
        }
        let snap = |label: &str| { let r = crate::small_verbs::screenshot_fusion(&json!({}), &caller); if let Some(p) = r["savedTo"].as_str().or(r["localSafePath"].as_str()).filter(|_| r["success"].as_bool() == Some(true)) { let p = p.to_string(); update_state(&id, |s| push_to(s, "shots", json!({"title": label, "label": label, "path": p}))); } };
        let wait_idle = |max_s: f64| -> bool { let deadline = Instant::now() + Duration::from_secs_f64(max_s); while Instant::now() < deadline { if pass2("get_app_state", json!({}))["success"].as_bool() == Some(true) { return true; } sleep_s(5.0); } false };
        let mut done_est: u64 = 0;
        let mut last_view: Option<(String, String, u64)> = None;
        let mut i = 0usize;
        let mut sample_lbr = String::new(); let mut deviceset = String::new(); let mut bound3d = false; let mut design_open_failed = false;
        let result: Result<(), String> = (|| {
            while i < steps2.len() {
                let (mut key, mut label, mut dur) = steps2[i].clone();
                i += 1;
                if remote { step_ref.store(i, std::sync::atomic::Ordering::Relaxed); let paused = get_state(&id).map(|s| s["paused"].as_bool() == Some(true)).unwrap_or(false); panel_show(&caller, &id, &stop_labels, i - 1, !paused); }
                if take_flag(&id, "abort") { return Err("tour exited by the user (transport control)".into()); }
                while get_state(&id).map(|s| s["paused"].as_bool() == Some(true) && s["abort"].as_bool() != Some(true)).unwrap_or(false) { update_state(&id, |s| { s["stepLabel"] = json!("Paused - press play to continue"); s["stepCaption"] = json!("Paused."); }); sleep_s(1.5); }
                if take_flag(&id, "abort") { return Err("tour exited by the user (transport control)".into()); }
                if take_flag(&id, "skipNext") { update_state(&id, |s| push_to(s, "skippedSteps", json!(key))); done_est += dur; continue; }
                if take_flag(&id, "replayPrev") { if let Some(lv) = last_view.clone() { i -= 1; key = lv.0; label = lv.1; dur = lv.2; } }
                if VIEW_STEPS.contains(&key.as_str()) { last_view = Some((key.clone(), label.clone(), dur)); }
                if !fg { host.background_envelope(dur as f64 * 3.0 + 240.0, false); }
                let step_started = now();
                let explain = if key == "3d-board" && !bound3d { tour_explain("3d-board-unbound").unwrap() } else { tour_explain(&key).unwrap_or(label.as_str()) };
                if captions { tour_caption(&caller, explain, dur); }
                let (conf, _) = (crate::chains::estimate(&sd2, &key, dur as f64).1, 0);
                let pct = (100 * done_est / total_est.max(1)) as u64; let el = (now() - started) as u64; let eta = total_est.saturating_sub(el);
                update_state(&id, |s| { s["step"] = json!(key); s["stepLabel"] = json!(label); s["stepCaption"] = json!(explain); s["percent"] = json!(pct); s["elapsedSec"] = json!(el); s["estimatedSec"] = json!(total_est); s["etaSec"] = json!(eta); s["confidence"] = json!(conf); s["startKind"] = json!(if cold { "cold" } else { "warm" });
                    s["progress"] = json!({"phase": key, "stepLabel": label, "percent": pct, "elapsedSec": el, "estimatedSec": total_est, "etaSec": eta, "confidence": conf, "startKind": if cold { "cold" } else { "warm" }}); });
                match key.as_str() {
                    "launch" => {
                        let _ = crate::launch::start(&ctx2, &json!({}));
                        let mut ready = false; let mut seat_attempts = 0;
                        for _ in 0..30 {
                            sleep_s(5.0);
                            if pass2("get_app_state", json!({}))["success"].as_bool() == Some(true) { ready = true; break; }
                            let _ = crate::etiquette::dismiss_save_prompt_if_bridge_doc(&sd2, &caller, &pids, "");
                            let lic = Loopback::from_env().map(|ab| fusion_core::windows_facts::facts_from_ab(&ab, &caller).licensing_dialog.is_some()).unwrap_or(false);
                            if lic {
                                let auto = crate::small_verbs::get_pref(&sd2, "autoTakeOverSeat").map(|v| v != json!(false)).unwrap_or(true);
                                if auto {
                                    update_state(&id, |s| s["stepLabel"] = json!("Taking over the Autodesk seat (another machine holds it)"));
                                    let r = crate::readiness_exec::resolve_seat_dialog(&ctx2, 3); seat_attempts += 1;
                                    if r["clicks"].as_u64().unwrap_or(0) > 0 { update_state(&id, |s| { s["seatTakeover"] = json!(true); push_to(s, "warnings", json!("license_sessions_exceeded: took the seat over in the background (the other machine's Fusion suspended resumably)")); }); }
                                    let still = Loopback::from_env().map(|ab| fusion_core::windows_facts::facts_from_ab(&ab, &caller).licensing_dialog.is_some()).unwrap_or(false);
                                    if seat_attempts >= 3 && still { return Err("license_sessions_exceeded: the Autodesk seat is held on another machine and background takeover did not land. Free it there (fusion_stop on that box) and re-run the tour.".into()); }
                                } else { return Err("license_sessions_exceeded: seat held on another machine and auto-takeover is opted out (fusion_set_seat_takeover). Resolve the 'Active Sessions Exceeded' dialog or re-enable takeover, then re-run.".into()); }
                            }
                        }
                        if !ready {
                            let rd = (env.readiness)();
                            if rd["needsSignin"].as_bool() == Some(true) {
                                update_state(&id, |s| { s["step"] = json!("signin"); s["stepLabel"] = json!("Signing in (one-time, driving it for you)"); });
                                let readiness = || (env.readiness)(); let start = || crate::launch::start(&ctx2, &json!({})); let aps_state = || (env.aps_state)(); let aps_signin = |p: &str| crate::aps_signin::signin(&ctx2, &json!({"profile": p}));
                                let si = crate::signin::signin(&ctx2, &json!({"auto": true}), &crate::signin::Env { readiness: &readiness, start: &start, aps_state: &aps_state, aps_signin: &aps_signin });
                                let wall = si["signinWall"].as_str().unwrap_or("credentials").to_string();
                                update_state(&id, |s| push_to(s, "warnings", json!(format!("sign-in required: drove to wall {}; user toasted; {}", fusion_lbr::py_repr_str(&wall), si["stage"].as_str().unwrap_or("None")))));
                                return Err(format!("One-time Autodesk sign-in needed on this machine. I drove it as far as possible and toasted the user at the {} step - after they finish, re-run the tour and it completes.", fusion_lbr::py_repr_str(&wall)));
                            }
                            return Err("Fusion launched but its add-in never became ready (150s)".into());
                        }
                    }
                    "stage" => {
                        let custom = args2["sampleLbr"].as_str().unwrap_or("").replace('\\', "/");
                        if !custom.is_empty() {
                            if !std::path::Path::new(&custom).is_file() { return Err(format!("sampleLbr not found on this machine: {custom}")); }
                            sample_lbr = custom.clone();
                            deviceset = std::fs::read_to_string(&custom).ok().and_then(|t| regex::Regex::new(r#"<deviceset name="([^"]+)""#).ok().and_then(|re| re.captures(&t).map(|c| c[1].to_string()))).unwrap_or_else(|| "UNKNOWN".into());
                        } else {
                            sample_lbr = sample_lbr_or(|| Err("bundled sample library missing from the bridge install".to_string()))?;
                            deviceset = "ADS8588SIPM".into();
                        }
                        let (sl, ds) = (sample_lbr.clone(), deviceset.clone()); update_state(&id, |s| { s["sampleLbr"] = json!(sl); s["deviceset"] = json!(ds); });
                    }
                    "library" => { let r = crate::orchestrations::open_lbr(&ctx2, &json!({"filePath": sample_lbr}), &pass2); if r["success"].as_bool() != Some(true) { return Err(format!("open_lbr: {}", r["error"].as_str().unwrap_or("None"))); } snap(&label); }
                    "symbol" | "footprint" | "device" => {
                        let suffix = match key.as_str() { "symbol" => "sym", "footprint" => "pac", _ => "dev" };
                        let r = crate::orchestrations::show_library_view(&ctx2, if deviceset.is_empty() { "ADS8588SIPM" } else { &deviceset }, suffix, Some(&sample_lbr), &pass2);
                        if r["success"].as_bool() != Some(true) { return Err(r["error"].as_str().unwrap_or("None").to_string()); }
                        if let Some(p) = r["screenshot"].as_str() { let p = p.to_string(); let l = label.clone(); update_state(&id, |s| push_to(s, "shots", json!({"title": l, "label": l, "path": p}))); }
                    }
                    "3d-package" => {
                        let step_path = path_sibling(&sample_lbr, ".step");
                        let pac = std::fs::read_to_string(&sample_lbr).ok().and_then(|x| fusion_lbr::artifact_names(&x).ok()).and_then(|n| n.pac.first().cloned());
                        let mut bound = false;
                        if let (Some(sp), Some(pac)) = (step_path, pac) {
                            wait_idle(180.0);
                            let br = crate::mint::build_library_3d_core(&ctx2, &json!({"lbrPath": sample_lbr, "parts": [{"package": pac, "lbrPath": sample_lbr, "modelPath": sp}], "outLbrPath": sample_lbr, "capture": true}), &pass2, &|_, _, _| {});
                            bound = br["success"].as_bool() == Some(true) && br["partsBound"].as_u64().unwrap_or(0) >= 1;
                            let l = label.clone(); let shot = br["screenshots"].as_array().and_then(|a| a.iter().find(|s| matches!(s["stage"].as_str(), Some("after") | Some("3d")) && s["path"].is_string()).cloned());
                            update_state(&id, |s| { s["bind"] = json!({"success": bound, "partsBound": br["partsBound"], "error": br["error"]}); if let Some(sh) = shot { push_to(s, "shots", json!({"title": l, "label": l, "path": sh["path"]})); } });
                        }
                        bound3d = bound; update_state(&id, |s| s["bound3d"] = json!(bound));
                        if bound {
                            wait_idle(180.0);
                            let _ = crate::orchestrations::open_lbr(&ctx2, &json!({"filePath": sample_lbr}), &pass2);
                            if !get_state(&id).map(|s| s["shots"].as_array().map(|a| a.iter().any(|x| x["title"].as_str() == Some(&label))).unwrap_or(false)).unwrap_or(false) { snap(&label); }
                        } else {
                            let why = get_state(&id).and_then(|s| s["bind"]["error"].as_str().map(str::to_string)).unwrap_or_else(|| "no STEP staged".into());
                            update_state(&id, |s| push_to(s, "warnings", json!(format!("3D bind did not complete ({why}); showing the package generator instead"))));
                            let cp = crate::orchestrations::create_3d_package_pub(&ctx2, &sample_lbr, &pass2);
                            if cp["success"].as_bool() == Some(true) {
                                sleep_s(3.0); snap(&label);
                                let _ = pass2("run_modeling_script", json!({"script": "import adsk.core\ncd=ui.commandDefinitions.itemById('Package3DStop')\nresult={'finished': bool(cd) and cd.execute()}\n"}));
                                sleep_s(2.0);
                                let _ = crate::small_verbs::dismiss_blocking_dialogs(&sd2, &json!({}), &caller, &pids, &|_| {});
                                let _ = pass2("close_document", json!({"save": false, "saveToClose": true})); sleep_s(1.0);
                            } else { update_state(&id, |s| push_to(s, "failedSteps", json!(key))); }
                        }
                    }
                    "make-project" => {
                        wait_idle(180.0);
                        let r = crate::testproject::make_test_project(&ctx2, &json!({"libraryPath": sample_lbr}), &pass2);
                        if r["success"].as_bool() == Some(true) { sleep_s(3.0); snap(&label); } else { design_open_failed = true; update_state(&id, |s| { push_to(s, "failedSteps", json!(key)); push_to(s, "warnings", json!(format!("make-project: {}", r["error"].to_string().chars().take(120).collect::<String>()))); s["designOpenFailed"] = json!(true); }); }
                    }
                    "open-design" => {
                        wait_idle(180.0);
                        let _ = crate::orchestrations::open_cloud_file(&ctx2, &json!({"fileName": query, "wait": false}), &pass2);
                        let deadline = Instant::now() + Duration::from_secs(170); let mut open = false;
                        while Instant::now() < deadline { let st = pass2("get_app_state", json!({})); if st["data"]["activeDocument"].as_str().unwrap_or("").to_lowercase().contains(&query.to_lowercase()) { open = true; break; } sleep_s(5.0); }
                        if open { sleep_s(3.0); snap(&label); } else { design_open_failed = true; update_state(&id, |s| { push_to(s, "failedSteps", json!(key)); push_to(s, "warnings", json!(format!("open-design: {} never became the active document within 170s", fusion_lbr::py_repr_str(&query)))); s["designOpenFailed"] = json!(true); }); }
                    }
                    "datahub" => {
                        wait_idle(180.0);
                        let r = pass2("list_cloud_projects", json!({}));
                        if r["success"].as_bool() == Some(true) { let mut projs = r["data"]["projects"].as_array().or(r["data"]["dataProjects"].as_array()).cloned().unwrap_or_default(); if projs.is_empty() { for hub in r["data"]["hubs"].as_array().cloned().unwrap_or_default() { projs.extend(hub["projects"].as_array().cloned().unwrap_or_default()); } } let names: Vec<String> = projs.iter().take(6).map(|p| p["name"].as_str().map(str::to_string).unwrap_or_else(|| p.to_string()).chars().take(40).collect()).collect(); update_state(&id, |s| s["highlights"]["datahub"] = json!({"projects": projs.len(), "names": names})); }
                        else { update_state(&id, |s| push_to(s, "warnings", json!(format!("datahub: {}", r["error"].to_string().chars().take(100).collect::<String>())))); }
                    }
                    "aps" => {
                        let q: String = deviceset.rsplit('/').next().unwrap_or("ADS8588").chars().take(20).collect();
                        let t0 = now();
                        let r = crate::native_aps_search(&json!({"query": q, "limit": 5}));
                        if r["success"].as_bool() == Some(true) { let hits = r["results"].as_array().or(r["items"].as_array()).or(r["data"]["results"].as_array()).map(|a| a.len()).unwrap_or(0); update_state(&id, |s| s["highlights"]["aps"] = json!({"query": q, "hits": hits, "seconds": ((now() - t0) * 10.0).round() / 10.0})); }
                        else { update_state(&id, |s| push_to(s, "warnings", json!(format!("aps: {} (sign in once with fusion_aps_signin and this step lights up)", r["error"].to_string().chars().take(80).collect::<String>())))); }
                    }
                    "mcp" => {
                        let r = crate::mcp::tools(env.version, (env.aps_state)());
                        if r["success"].as_bool() == Some(true) { let tools = r["tools"].as_array().cloned().unwrap_or_default(); let names: Vec<String> = tools.iter().take(20).map(|t| t["name"].as_str().map(str::to_string).unwrap_or_else(|| t.to_string()).chars().take(32).collect()).collect(); update_state(&id, |s| s["highlights"]["mcp"] = json!({"toolCount": tools.len(), "tools": names, "beyond": "The bridge's fusion_* surface is ~120 verbs: electronics editors, EAGLE text commands, cloud files, APS search, evidence captures, and 14 export formats - none of which the MCP toolset offers."})); }
                        else { update_state(&id, |s| push_to(s, "warnings", json!(format!("mcp: {} (fusion_mcp_enable turns it on in ~10s)", r["error"].to_string().chars().take(80).collect::<String>())))); }
                    }
                    "exports" => {
                        wait_idle(180.0);   // the 3D board step leaves a design active; that is what the exports read
                        let dir = std::env::temp_dir().join("fusion-tour-exports").to_string_lossy().replace('\\', "/");
                        let mut got: Vec<String> = Vec::new();
                        for (fmt, verb) in [("step", "export_step"), ("stl", "export_stl")] {
                            let p = format!("{dir}/{}.{fmt}", if deviceset.is_empty() { "tour" } else { &deviceset });
                            let r = pass2(verb, json!({"outputPath": p}));
                            if r["success"].as_bool() == Some(true) { got.push(fmt.to_uppercase()); } else { update_state(&id, |s| push_to(s, "warnings", json!(format!("{verb}: {}", r["error"].to_string().chars().take(80).collect::<String>())))); }
                        }
                        if !got.is_empty() { update_state(&id, |s| s["highlights"]["exports"] = json!({"formats": got, "dir": dir, "also": ["F3D", "GLB", "3MF", "IGES", "SAT", "DXF", "DWG", "OBJ", "FBX", "SKP", "USDZ", "gerbers", "BOM", "CPL"]})); snap(&label); }
                    }
                    "schematic" | "2d-board" | "3d-board" => {
                        if design_open_failed { update_state(&id, |s| push_to(s, "failedSteps", json!(key))); continue; }
                        let cmd = match key.as_str() { "schematic" => "show_schematic", "2d-board" => "show_2d_board", _ => "show_3d_board" };
                        // the generated test project opens as two documents and leaves the SCHEMATIC one
                        // active; the board switches need the BOARD document in front
                        if key != "schematic" && query.is_empty() && crate::testproject::test_project(&sd2).is_some() { let _ = crate::testproject::test_project_activate(&ctx2, "brd", &pass2); }
                        let r = crate::orchestrations::open_with_screenshot(&ctx2, cmd, &json!({}), &pass2);
                        if r["success"].as_bool() == Some(true) { sleep_s(2.0); snap(&label); } else { update_state(&id, |s| { push_to(s, "failedSteps", json!(key)); push_to(s, "warnings", json!(format!("{key}: {}", r["error"].to_string().chars().take(120).collect::<String>()))); }); }
                    }
                    _ => {}
                }
                crate::timings::record(&sd2, &key, now() - step_started, if key == "launch" && cold { Some("cold") } else { None });
                done_est += dur;
                let pct = (100 * done_est / total_est.max(1)) as u64; let el = (now() - started) as u64;
                update_state(&id, |s| { s["percent"] = json!(pct); s["elapsedSec"] = json!(el); });
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                let n = get_state(&id).map(|s| s["shots"].as_array().map(|a| a.len()).unwrap_or(0)).unwrap_or(0);
                if captions { tour_caption(&caller, "Tour complete: library, symbol, footprint, 3D chip, Data Hub, APS search, MCP, a generated project in schematic, 2D and 3D, and real exports - all driven in the background by the bridge.", 20); }
                update_state(&id, |s| { s["done"] = json!(true); s["percent"] = json!(100); s["step"] = json!("done"); s["stepLabel"] = json!("Tour complete"); s["narrate"] = json!(format!("Fusion tour complete: {n} views captured across the electronics library{}.", if include_design && !query.is_empty() { " and the board design" } else { "" })); s["elapsedSec"] = json!((now() - started) as u64); });
            }
            Err(e) => { update_state(&id, |s| { let st = s["step"].clone(); push_to(s, "failedSteps", st); s["failed"] = json!(true); s["done"] = json!(true); s["error"] = json!(e.chars().take(300).collect::<String>()); s["narrate"] = json!(format!("Fusion tour stopped at '{}': {}", s["stepLabel"].as_str().unwrap_or(""), e.chars().take(120).collect::<String>())); s["elapsedSec"] = json!((now() - started) as u64); }); }
        }
        let closed = crate::mint::close_docs_opened_since(&ctx2, &docs_before, &[], &pass2);
        update_state(&id, |s| s["tourDocsClosed"] = json!(closed));
        if captions { let _ = ab_call(&caller, "desktop_caption", json!({"action": "hide", "id": TOUR_PANEL_ID}), 8); }
        if remote { pump_stop.store(true, std::sync::atomic::Ordering::Relaxed); let _ = ab_call(&caller, "desktop_demo_panel", json!({"action": "hide", "id": TOUR_PANEL_ID}), 8); }
        if let Some(s) = get_state(&id) { persist(&sd2, &s); }
    });
    json!({"success": true, "demoId": demo_id, "estimatedSec": total_est, "steps": steps.iter().map(|s| json!({"step": s.0, "label": s.1, "estimatedSec": s.2, "caption": tour_explain(&s.0).unwrap_or(&s.1)})).collect::<Vec<_>>(), "statusVerb": "fusion_demo_progress",
        "_hint": format!("Tour started ASYNC (background by default - pass foreground:true only if the user wants to WATCH). Poll fusion_demo_progress {{\"demoId\": \"{demo_id}\"}} about once a second and render percent/stepLabel as a real progress bar. The final poll (done:true) carries the screenshot array.")})
}
fn sample_lbr_or(err: impl FnOnce() -> Result<String, String>) -> Result<String, String> { match sample_lbr() { Some(p) => Ok(p), None => err() } }

/// What the staged demo needs around it.
pub struct DemoEnv<'a> { pub readiness: &'a dyn Fn() -> Value, pub signin: &'a dyn Fn(&Value) -> Value, pub aps_state: &'a dyn Fn() -> Value, pub aps_search: &'a dyn Fn(&Value) -> Value }

/// `fusion_demo` (staged, resumable): from nothing to project -> schematic -> 2D -> 3D plus a live APS search.
pub fn demo(ctx: &Ctx, args: &Value, env: &DemoEnv, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let stage = args["stage"].as_str().unwrap_or("auto").trim().to_string();
    let demo_query = args["query"].as_str().unwrap_or("").to_string();
    let mut steps: Vec<String> = Vec::new();
    let mut shots: Vec<Value> = Vec::new();
    let shot = |label: &str, shots: &mut Vec<Value>| { let r = crate::small_verbs::screenshot_fusion(&json!({}), &ctx.caller); if let Some(p) = r["savedTo"].as_str().or(r["localSafePath"].as_str()).filter(|_| r["success"].as_bool() == Some(true)) { shots.push(json!({"title": label, "label": label, "path": p})); } };
    let out = |stage: &str, narrate: String, hint: String, done: bool, steps: &Vec<String>, shots: &Vec<Value>, extra: Value| { let mut o = json!({"success": true, "demo": true, "stage": stage, "done": done, "steps": steps, "narrate": narrate, "screenshots": shots, "statusVerb": "fusion_demo", "_hint": hint}); if let Some(e) = extra.as_object() { for (k, v) in e { o[k] = v.clone(); } } o };
    let mut rd = (env.readiness)();
    if rd["installed"].as_bool() != Some(true) {
        return out("not_installed", "Fusion 360 isn't installed yet - want me to install it? It's a free 30-day trial with everything switched on, so the whole tour works: real boards, schematics, 3D, Gerber and BOM exports, cloud search. After the trial, Fusion for Personal Use stays free for non-commercial work (hobby PCBs up to 2 layers), and even an expired install can still OPEN and VIEW your designs.".into(),
            "OFFER the install - do not just report it is missing. Say what they GET: the free 30-day trial is FULL-FEATURED (every step of this demo works: electronics, schematic, 2D/3D board, Gerbers/BOM/CPL, APS cloud search). Be honest about after: Fusion for Personal Use is free for non-commercial use but LIMITS electronics (about 2 layers / 2 schematic sheets / small board area) and some exports; an expired or read-only install can still OPEN + VIEW + browse designs (it only blocks save/export/modify), so their work is never locked away. Then call fusion_install_fusion (no shell approval needed; streams the installer, 10-30 min), poll fusion_readiness until installed:true, and call fusion_demo again to continue the tour. Full flow: the fusion-onboarding skill.".into(), false, &steps, &shots, json!({}));
    }
    if rd["running"].as_bool() != Some(true) { steps.push("launched Fusion".into()); let _ = crate::launch::start(ctx, &json!({})); rd = (env.readiness)(); }
    if rd["needsSignin"].as_bool() == Some(true) {
        let mut si = (env.signin)(&json!({"profile": args["signinProfile"]}));
        si["demo"] = json!(true);
        if matches!(si["stage"].as_str(), Some("opened") | Some("click_signin") | Some("ask_profile") | Some("open_failed")) {
            if si["done"].is_null() { si["done"] = json!(false); }
            si["_hint"] = json!(format!("{}  (This is the fusion_demo sign-in stage - after the user is signed in and fusion_readiness is ready:true, call fusion_demo again to continue the tour: APS -> project -> schematic -> 2D -> 3D.)", si["_hint"].as_str().unwrap_or("")));
            return si;
        }
        let nb = ab_call(&ctx.caller, "nbrowser_readiness", json!({}), 40);
        let nb_data = if nb["data"].is_object() { nb["data"].clone() } else { nb.clone() };
        let nb_state = nb_data["state"].as_str().map(str::to_string);
        let (mut opened, mut where_, mut profile_used, mut pick_reason, mut ambiguous) = (false, String::new(), String::new(), String::new(), false);
        let mut choices: Vec<Value> = Vec::new();
        if nb_state.as_deref() == Some("ready") {
            let profs = ab_call(&ctx.caller, "nbrowser_profiles", json!({}), 40);
            let pdata = if profs["data"].is_object() { profs["data"].clone() } else { profs.clone() };
            for p in pdata["profiles"].as_array().cloned().unwrap_or_default() {
                if !p.is_object() || p["blocked"].as_bool() == Some(true) || p["unresolved"].as_bool() == Some(true) || p["profile"].as_str().map(|s| s.is_empty()).unwrap_or(true) { continue; }
                choices.push(json!({"profile": p["profile"], "browser": p["browser"].as_str().unwrap_or("").to_lowercase(), "email": p["email"].as_str().unwrap_or(""), "displayName": p["displayName"].as_str().unwrap_or(""), "profileDir": p["profileDir"].as_str().unwrap_or(""), "active": p["active"].as_bool().unwrap_or(false), "live": p["live"].as_bool().unwrap_or(false), "asleep": p["asleep"].as_bool().unwrap_or(false), "extensionInstalled": p.get("extensionInstalled").cloned().unwrap_or(json!(true)), "describe": crate::signin::describe_profile(&p)}));
            }
            for c in choices.iter_mut() {
                if c["live"].as_bool() != Some(true) { c["autodesk"] = json!("unprobed(asleep)"); continue; }
                let ls = ab_call(&ctx.caller, "nbrowser_login_state", json!({"profile": c["profile"], "url": "https://accounts.autodesk.com/"}), 45);
                let lsd = if ls["data"].is_object() { ls["data"].clone() } else { ls.clone() };
                c["autodesk"] = json!(if lsd["ok"].is_null() && lsd["loggedIn"].is_null() { "unprobed" } else if lsd["loggedIn"].as_bool() == Some(true) { "signed-in" } else { "signed-out" });
                c["autodeskConfidence"] = lsd["confidence"].clone(); c["autodeskCookies"] = lsd["cookieCount"].clone();
            }
            let conf = |c: &Value| match c["autodeskConfidence"].as_str() { Some("high") => 3, Some("medium") => 2, Some("low") => 1, _ => 0 };
            let mut signed: Vec<Value> = choices.iter().filter(|c| c["autodesk"].as_str() == Some("signed-in")).cloned().collect();
            signed.sort_by_key(|c| std::cmp::Reverse((conf(c), c["autodeskCookies"].as_i64().unwrap_or(0))));
            let pick = if let Some(p) = signed.first() { pick_reason = format!("it is the profile actually signed into Autodesk ({} auth cookies, {} confidence)", p["autodeskCookies"], p["autodeskConfidence"].as_str().unwrap_or("None")); Some(p.clone()) } else {
                let consumer = ["gmail.com", "outlook.com", "hotmail.com", "yahoo.com", "icloud.com", "live.com"];
                let mut ranked: Vec<Value> = choices.iter().filter(|c| c["live"].as_bool() == Some(true)).cloned().collect(); if ranked.is_empty() { ranked = choices.clone(); }
                ranked.sort_by_key(|c| { let em = c["email"].as_str().unwrap_or("").to_lowercase(); (if !em.is_empty() && !consumer.iter().any(|d| em.ends_with(&format!("@{d}"))) { 0 } else { 1 }, if c["live"].as_bool() == Some(true) { 0 } else { 1 }) });
                pick_reason = "no profile has a live Autodesk session, so I picked the most likely one (work/corporate identity first) - CONFIRM WITH THE USER".into(); ranked.first().cloned() };
            ambiguous = signed.is_empty() || signed.len() > 1;
            if let Some(p) = &pick { profile_used = p["profile"].as_str().unwrap_or("").to_string(); where_ = p["describe"].as_str().unwrap_or("").to_string(); }
            let mut oa = json!({"sessionId": "fusion-signin", "url": "https://accounts.autodesk.com/", "background": true, "thread": "fusion-demo", "purpose": "Autodesk sign-in for Fusion"});
            if !profile_used.is_empty() { oa["profile"] = json!(profile_used); }
            let r = ab_call(&ctx.caller, "nbrowser_open_window", oa, 60);
            let rd_open = if r["data"].is_object() { r["data"].clone() } else { r.clone() };
            opened = !rd_open["sessionId"].is_null() || r["ok"].as_bool() == Some(true) || r["success"].as_bool() == Some(true);
            if where_.is_empty() { where_ = "your browser".into(); }
        }
        steps.push(if opened { format!("opened Autodesk sign-in in {} (background)", if where_.is_empty() { "browser" } else { &where_ }) } else { format!("could not auto-open the browser sign-in: {}", nb["_adCallError"].as_str().map(str::to_string).or(nb_state.clone()).unwrap_or_else(|| "unknown".into())) });
        let narrate = if opened { format!("I've opened the Autodesk sign-in in {where_} - I chose that profile because {pick_reason}. It's sitting in the background so it won't interrupt what you're doing. Finish it whenever you like, tell me to bring it up front, or I can complete it for you. If that's the wrong account, say which profile to use.") } else { "Fusion needs your Autodesk sign-in. I'd like to use your own browser - you're probably already signed in there, so it should take one click.".to_string() };
        let hint = format!("FINISH THE SIGN-IN - never leave a first-time user parked here. {}The profile was CHOSEN BY EVIDENCE, not guessed: each live profile was probed with nbrowser_login_state against accounts.autodesk.com, and the one holding a real Autodesk session wins (see signinProfileReason + per-profile autodesk/autodeskConfidence/autodeskCookies in signinProfileChoices). RELAY THAT REASON to the user. If signinProfileAmbiguous is true (nothing signed in, or several are), ASK them which account their Autodesk login belongs to instead of assuming. NAME THE EXACT PROFILE - never say just 'your browser' or 'your Chrome'. Power users run several profiles (personal / work / media), so say the browser AND the identity from `signinWhere` (e.g. 'Chrome - John Personal (jlauer12@gmail.com)'). `signinProfileChoices` lists every profile with describe/email/displayName/active - if there is more than one, say which you used and OFFER TO SWITCH (re-run with a different profile). If the one you want shows extensionInstalled:false it needs the one-time extension install in THAT profile. TELL THE USER WHERE IT IS WAITING and OFFER ALL THREE: (a) they finish it themselves whenever they want, (b) you foreground that window for them (nbrowser_switch_window / browser window state), or (c) THEY LET YOU DRIVE IT - ask first (AskUserQuestion), then click through 'Continue with Google/Apple/Microsoft' using their warm session. Use the NATIVE browser (ABE), NEVER pup: pup is anonymous so Autodesk demands a full fresh login, while their real profile is usually already signed in. NEVER type their password or 2FA - if a secret is demanded, fusion_notify_owner toasts them to type it. Then click Fusion's own 'Sign In' (the webview exposes no UIA control, so use an image-space desktop_click on the button, or foreground + click) to fire the OAuth handoff; accept the 'Autodesk Identity Manager' overlay ('Always allow' + 'Open'). Codes expire in ~2 min - if it expires, click Sign In again. Poll fusion_readiness until ready:true, then call fusion_demo again. Playbook: fusion-autodesk-signin.",
            if opened { format!("A background sign-in tab is ALREADY OPEN in {where_} (session 'fusion-signin'). ") } else { "Auto-open failed - open it yourself: nbrowser_open_window {sessionId:'fusion-signin', url:'https://accounts.autodesk.com/', background:true, profile:'<chrome:their-account>'}. ".to_string() });
        return out("signin", narrate, hint, false, &steps, &shots, json!({"needsSignin": true, "signinOpened": opened, "signinWhere": where_, "signinProfile": profile_used, "signinSession": if opened { json!("fusion-signin") } else { Value::Null }, "signinProfileChoices": choices, "signinProfileReason": pick_reason, "signinProfileAmbiguous": ambiguous, "nbState": nb_state}));
    }
    if rd["ready"].as_bool() != Some(true) {
        return out("waiting", "Fusion is starting up - one moment.".into(), format!("Not ready yet (add-in still loading, or a licensing dialog). Poll fusion_readiness; it auto-resolves seat dialogs. When ready:true, call fusion_demo again. Reason: {}", rd["_hint"].as_str().unwrap_or("").chars().take(220).collect::<String>()), false, &steps, &shots, json!({}));
    }
    steps.push("Fusion ready".into());
    let ap = (env.aps_state)();
    let aps_ready = ap["configured"].as_bool() == Some(true) && ap["signedIn"].as_bool() == Some(true) && ap["tokenLive"].as_bool() == Some(true);
    if !aps_ready && stage != "skip_aps" {
        return out("aps_setup", "Next I'll turn on cloud search - it finds any design in your Autodesk cloud in about two seconds instead of a 30-minute folder crawl. It's a one-time sign-in.".into(),
            "SET APS UP FOR THEM - do not merely mention it. (1) fusion_aps_status for exact state. (2) If not configured, an admin registers a PKCE app once, then fusion_aps_set_client_id. (3) fusion_aps_signin - drive it in their NATIVE browser (ABE), same reasoning as the Fusion sign-in: they're already logged into Autodesk there. (4) Poll fusion_aps_status until tokenLive:true, then call fusion_demo again - the demo then runs a LIVE sample search so they SEE the speed. If they decline setup, call fusion_demo {stage:'skip_aps'} and TELL them what they're missing (2s server-indexed search across the whole team hub; the old in-app search took 30+ min and crashed Fusion, so it is disabled). Skills: fusion-aps-search, fusion-aps-signin.".into(),
            false, &steps, &shots, json!({"apsConfigured": ap["configured"].as_bool().unwrap_or(false), "apsSignedIn": ap["signedIn"].as_bool().unwrap_or(false)}));
    }
    let mut aps_hits: Vec<Value> = Vec::new();
    if aps_ready {
        let t0 = now();
        let sr = (env.aps_search)(&json!({"query": if demo_query.is_empty() { "board" } else { &demo_query }, "limit": 5}));
        let res = sr["data"]["results"].as_array().or(sr["results"].as_array()).cloned().unwrap_or_default();
        if sr["success"].as_bool() == Some(false) { steps.push(format!("APS sample search failed: {}", sr["error"].as_str().unwrap_or(""))); } else { aps_hits = res.iter().take(5).map(|r| json!({"name": r["name"], "project": r["projectName"]})).collect(); steps.push(format!("APS sample search: {} hits in {:.1}s", aps_hits.len(), now() - t0)); }
    }
    let st = pass("get_app_state", json!({}));
    let d = if st["data"].is_object() { st["data"].clone() } else { st.clone() };
    let doc = d["activeDocument"].as_str().unwrap_or("").to_string();
    if d["isElectronics"].as_bool() != Some(true) {
        let target = if !demo_query.is_empty() { demo_query.clone() } else { aps_hits.first().and_then(|h| h["name"].as_str()).unwrap_or("").to_string() };
        if target.is_empty() {
            return out("need_design", "I need an electronics design to show off. Which board should I open?".into(), "No electronics design open and nothing to pick. If APS is live, fusion_aps_search {query:'<board>'} then fusion_aps_open {query:'<name>'}; else ask the user for a design name / open one via fusion_open_cloud_file. ALWAYS open the PROJECT (EcadDesignProductType), never a .brd/.sch/3D child. Then call fusion_demo again.".into(), false, &steps, &shots, json!({}));
        }
        return out("open_design", format!("Opening {target} so you can see a real board end to end."), format!("OPEN THE PROJECT then re-call fusion_demo: fusion_aps_open {{query:'{target}'}} (APS finds it at any folder depth in seconds). CRITICAL: open the electronics PROJECT file, not the schematic/.brd/3D child - a child opens an isolated, often empty view. Poll fusion_get_app_state until isElectronics:true, then call fusion_demo again."), false, &steps, &shots, json!({"target": target}));
    }
    steps.push(format!("electronics project open: {doc}"));
    shot("project", &mut shots);
    for (verb, label, name) in [("show_schematic", "schematic", "schematic"), ("show_2d_board", "board_2d", "2D board"), ("show_3d_board", "board_3d", "3D board")] {
        let r = pass(verb, json!({}));
        if r["success"].as_bool() == Some(false) { steps.push(format!("{name} failed: {}", r["error"].as_str().unwrap_or(""))); } else { steps.push(format!("showed {name}")); shot(label, &mut shots); }
    }
    let aps_line = if aps_hits.is_empty() { String::new() } else { format!("Cloud search is live - I searched your whole Autodesk hub in about two seconds and found: {}. ", aps_hits.iter().take(3).filter_map(|h| h["name"].as_str()).collect::<Vec<_>>().join(", ")) };
    out("done", format!("That's the tour: your {doc} project, its schematic, the 2D board layout, and the real 3D board. {aps_line}From here just ask - export Gerbers/BOM/CPL for the fab, generate a laser-etched IPC package, load JLCPCB design rules, or open any design in your cloud."),
        "DEMO COMPLETE. SHOW the user the screenshots in order (project, schematic, board_2d, board_3d) - they are in `screenshots` with labels. Narrate the `narrate` line. Then offer concrete next steps in THEIR words: 'export the Gerbers', 'make me a SOIC-8 with my part number etched on it' (fusion_generate_package), 'check this against JLCPCB rules' (the Adom DRU rules ship in resources/ - apply them in the Fusion DRC dialog), 'find my other boards' (fusion_aps_search). Full catalog: fusion_describe. Demo playbook: the fusion-demo skill.".into(),
        true, &steps, &shots, json!({"document": doc, "apsSampleSearch": aps_hits}))
}

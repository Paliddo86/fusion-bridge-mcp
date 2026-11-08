//! Phase 3: the import chains, orchestrated by the exe. `create_library` (bind + prove) and
//! `install_library_bundle` (unpack, pair, place, split, then the chain) are ported from
//! server.py step for step; the leaves that still carry Fusion-driving lessons run on the
//! sidecar through its executor verbs (`chain_*`, `build_library_3d`, `prove_library`).
//! The run store is the SAME two files the Python bridge writes (`%TEMP%/fusion-prove-runs.json`
//! and `fusion-prove-last.json`), so `fusion_prove_result` and `fusion_cancel_run` work from
//! either side and a caller cannot tell who ran the chain except by `owner: "exe"`.
//!
//! AI hint: the chain is a background thread; the verb answers at once with a runId and the
//! caller polls `fusion_prove_result`. Cancel is cooperative (`cancelRequested` in the run
//! store, set by fusion_cancel_run) and lands at the next part boundary.

use serde_json::{json, Map, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What the chain needs from the shell: a way to call the sidecar / add-in, the state dir.
pub trait ChainHost: Send + Sync {
    fn call(&self, verb: &str, args: Value, timeout_secs: u64) -> Value;
    fn state_dir(&self) -> std::path::PathBuf;
    fn addin_round_trip_ready(&self) -> bool;
    fn addin_status(&self) -> Value;
    fn fusion_running(&self) -> bool;
    fn set_long_op(&self, name: Option<String>);
    fn taskbar(&self, pct: Option<f64>);
    /// The AI thread that asked (empty for web control and other anonymous callers).
    fn caller_thread(&self) -> String { String::new() }
}

fn now() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }
fn runs_path() -> std::path::PathBuf { std::env::temp_dir().join("fusion-prove-runs.json") }
fn last_path() -> std::path::PathBuf { std::env::temp_dir().join("fusion-prove-last.json") }

pub fn read_runs() -> Map<String, Value> {
    std::fs::read_to_string(runs_path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

pub fn write_runs_pub(runs: &Map<String, Value>) { write_runs(runs) }
fn write_runs(runs: &Map<String, Value>) {
    let p = runs_path();
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_string(runs).unwrap_or_default()).is_ok() { let _ = std::fs::rename(&tmp, &p); }
}

/// Persist one run's state (per-run store, capped at 20, plus the "last" file).
pub fn save_state_pub(run_id: &str, state: &Value) { save_state(run_id, state) }
pub fn lib_label_pub(p: &str) -> String { lib_label(p) }
fn save_state(run_id: &str, state: &Value) {
    let _ = std::fs::write(last_path(), serde_json::to_string(state).unwrap_or_default());
    let mut runs = read_runs();
    runs.insert(run_id.to_string(), state.clone());
    while runs.len() > 20 { let k = runs.keys().next().cloned().unwrap(); runs.remove(&k); }
    write_runs(&runs);
}

pub fn cancel_requested_pub(run_id: &str) -> bool { cancel_requested(run_id).is_some() }
fn cancel_requested(run_id: &str) -> Option<Value> {
    let runs = read_runs();
    let r = runs.get(run_id)?;
    if r["cancelRequested"].as_bool() == Some(true) { Some(json!({"by": r["cancelledBy"].as_str().unwrap_or("user")})) } else { None }
}

/// timings.json `phase|any` estimate: (seconds, confidence) like `timings.estimate`.
pub fn estimate(state_dir: &std::path::Path, phase: &str, fallback: f64) -> (f64, &'static str) {
    let t: Value = std::fs::read_to_string(state_dir.join("timings.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
    let st = fusion_core::readiness::timings_stats(&t, phase, "any");
    let (count, p50) = (st["count"].as_u64().unwrap_or(0), st["p50"].as_f64());
    match (count, p50) { (c, Some(p)) if c >= 2 && p > 0.0 => (p, "measured"), (1, Some(p)) if p > 0.0 => (p, "typical"), _ => (fallback, "typical") }
}

fn log_run(state_dir: &std::path::Path, verb: &str, seconds: f64, ok: bool) { crate::timings::log_run(state_dir, verb, seconds, ok) }

fn lib_label(p: &str) -> String {
    let stem = p.replace('\\', "/").rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_else(|| p.rsplit('/').next().unwrap_or("").to_string());
    if stem.is_empty() { "library".into() } else { stem }
}
fn lib_stem(p: &str) -> String { lib_label(p) }

fn named(name: &str, descs: &Map<String, Value>) -> String {
    let d = descs.get(&name.to_lowercase()).and_then(|v| v.as_str()).unwrap_or("");
    if d.is_empty() { return if name.is_empty() { "library".into() } else { name.into() }; }
    let d: String = if d.chars().count() <= 70 { d.to_string() } else { format!("{}…", d.chars().take(69).collect::<String>().trim_end()) };
    format!("{name}: {d}")
}

fn library_descriptions(lbr_path: &str) -> Map<String, Value> {
    let mut out = Map::new();
    if let Ok(xml) = std::fs::read_to_string(lbr_path) {
        if let Ok(md) = fusion_lbr::library_part_metadata(&xml) {
            for ds in md.devicesets {
                let d = ds.description.trim().to_string();
                if d.is_empty() { continue; }
                out.insert(ds.name.to_lowercase(), json!(d));
                for v in ds.variants { if !v.package.is_empty() { out.entry(v.package.to_lowercase()).or_insert(json!(d)); } }
            }
        }
    }
    out
}

/// The in-Fusion palette, fire-and-forget on a thread; a watchdog re-sends the last state
/// every 10 s while a run is active (a sibling add-in copy's stop() can tear it down).
pub struct Banner { host: Arc<dyn ChainHost>, last: Mutex<Option<Value>>, active: Mutex<bool> }
impl Banner {
    pub fn new(host: Arc<dyn ChainHost>) -> Arc<Banner> { Arc::new(Banner { host, last: Mutex::new(None), active: Mutex::new(false) }) }
    pub fn send(self: &Arc<Self>, action: &str, title: &str, detail: &str, percent: Option<f64>, eta: Option<f64>, tone: &str) {
        let args = json!({"action": action, "title": title, "detail": detail, "percent": percent, "etaSec": eta.map(|e| e.max(0.0)), "tone": tone});
        if action == "hide" { *self.active.lock().unwrap() = false; } else { *self.active.lock().unwrap() = true; *self.last.lock().unwrap() = Some(args.clone()); }
        let host = self.host.clone();
        std::thread::spawn(move || { let _ = host.call("activity_banner", args, 20); });
    }
    pub fn watchdog(self: &Arc<Self>, run_id: String) {
        let me = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(10));
            if !*me.active.lock().unwrap() { break; }
            if !read_runs().get(&run_id).map(|r| !matches!(r["stage"].as_str(), Some("done") | Some("failed") | Some("cancelled"))).unwrap_or(false) { break; }
            let last = me.last.lock().unwrap().clone();
            if let Some(mut a) = last { a["action"] = json!("update"); let _ = me.host.call("activity_banner", a, 20); }
        });
    }
    pub fn hide_later(self: &Arc<Self>, secs: u64) { let me = self.clone(); std::thread::spawn(move || { std::thread::sleep(Duration::from_secs(secs)); me.send("hide", "", "", None, None, "working"); }); }
}

fn taskbar(host: &dyn ChainHost, pct: Option<f64>) { host.taskbar(pct); }

pub fn note_bridge_doc_pub(state_dir: &std::path::Path, name: &str) { note_bridge_doc(state_dir, name) }
fn note_bridge_doc(state_dir: &std::path::Path, name: &str) {
    let n = name.replace('\\', "/").rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_else(|| name.to_string()).trim().to_lowercase();
    if n.len() < 3 { return; }
    let p = state_dir.join("prefs.json");
    let mut d: Value = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok()).unwrap_or(json!({}));
    let mut docs: Vec<String> = d["bridgeDocs"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    // oldest first: a sorted list evicted the alphabetically first name, not the oldest one
    if !docs.contains(&n) { docs.push(n); }
    while docs.len() > 24 { docs.remove(0); }
    d["bridgeDocs"] = json!(docs);
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(&d).unwrap_or_default()).is_ok() { let _ = std::fs::rename(&tmp, &p); }
}

fn hold_active(state_dir: &std::path::Path) -> bool {
    let f = state_dir.join("user_restart_hold.json");
    let Ok(t) = std::fs::read_to_string(&f) else { return false };
    let at = serde_json::from_str::<Value>(&t).ok().and_then(|v| v["at"].as_f64()).unwrap_or(0.0);
    now() - at < 4.0 * 3600.0
}

fn contains_any(s: &str, needles: &[&str]) -> bool { needles.iter().any(|n| s.contains(n)) }

/// `fusion_create_library`: the whole "create a Fusion library" task, one call, one poll verb.
pub fn create_library(host: Arc<dyn ChainHost>, args: Value) -> Value {
    let lbr_p = args["libraryPath"].as_str().unwrap_or("").replace('\\', "/");
    let sym_p = args["kicadSymbolPath"].as_str().unwrap_or("").replace('\\', "/");
    let mod_p = args["kicadFootprintPath"].as_str().unwrap_or("").replace('\\', "/");
    let step_p = args["stepPath"].as_str().unwrap_or("").replace('\\', "/");
    let parts_in: Vec<Value> = args["parts"].as_array().cloned().unwrap_or_default();
    // an import already running? attach, never start a second one
    let runs = read_runs();
    if let Some((rid, rv)) = runs.iter().filter(|(_, v)| matches!(v["verb"].as_str(), Some("create_library") | None)
        && !matches!(v["stage"].as_str(), Some("done") | Some("failed") | Some("cancelled") | None)
        && v["startedAt"].as_f64().map(|s| now() - s < 1800.0).unwrap_or(false)).last() {
        return json!({"success": true, "started": false, "attached": true, "runId": rid, "statusVerb": "fusion_prove_result", "stage": rv["stage"],
                      "_hint": format!("An import is ALREADY running ({rid}, stage: {}). Not starting a second one; two chains on one Fusion would fight over documents. Poll this runId.", rv["stage"].as_str().unwrap_or("?"))});
    }
    let _ = host.call("chain_sweep_orphans", json!({}), 60);
    if lbr_p.is_empty() && sym_p.is_empty() && mod_p.is_empty() {
        return json!({"success": false, "errorCode": "no_input", "error": "Give me EITHER libraryPath (a ready .lbr) OR kicadSymbolPath / kicadFootprintPath (KiCad sources).",
                      "_hint": "Shape A: {libraryPath, stepPath}. Shape B: {kicadSymbolPath, kicadFootprintPath, stepPath, pinMap?}. Importing a whole wiki Cloud Library? fusion_install_library_bundle {zipPath} pairs every deviceset to its STEP for you."});
    }
    if !sym_p.is_empty() || !mod_p.is_empty() {
        return json!({"success": false, "errorCode": "adom_lbr_unavailable", "kicadIngest": false,
                      "error": "adom-lbr (the canonical KiCad->EAGLE converter) does not run on this platform - Adom binaries are Linux-container-only by design - and this bridge deliberately does NOT convert KiCad formats itself.",
                      "_hint": "Convert at AUTHORING time, where adom-lbr genuinely runs: in a container, `adom-lbr generate --sym <.kicad_sym> --fp <.kicad_mod> -o <out.lbr>` (auto-linted), publish the .lbr on the part page, then call fusion_create_library shape A {libraryPath, stepPath}."});
    }
    // preflight, re-checked a few times (a file still landing)
    let mut pf = fusion_lbr::preflight_lbr(std::path::Path::new(&lbr_p));
    let mut tries = 0;
    while !pf.ok && tries < 3 { std::thread::sleep(Duration::from_millis(600)); pf = fusion_lbr::preflight_lbr(std::path::Path::new(&lbr_p)); tries += 1; }
    if !pf.ok {
        return json!({"success": false, "errorCode": "lbr_preflight_failed", "preflight": pf, "runId": Value::Null, "preflightRetries": tries,
                      "error": format!("Pre-analysis says Fusion will NOT open this library (re-checked {} times over {:.1}s): {}", tries + 1, tries as f64 * 0.6, pf.errors.join("; ").chars().take(360).collect::<String>()),
                      "_hint": "This is NOT 'Fusion failed to start' - there is no runId because the chain refused to begin. Fix the named checks; fusion_preflight_lbr runs these checks in isolation."});
    }
    let run_id: String = format!("{:x}", md5_like(&format!("{}{}", now(), std::process::id())))[..12].to_string();
    {
        let mut runs = read_runs();
        runs.insert(run_id.clone(), json!({"runId": run_id, "verb": "create_library", "stage": "starting", "stageLabel": "accepted, chain starting", "startedAt": now(), "libraryPath": if lbr_p.is_empty() { Value::Null } else { json!(lbr_p) }, "percent": 0, "owner": "exe"}));
        write_runs(&runs);
    }
    let (h2, rid2, a2) = (host.clone(), run_id.clone(), args.clone());
    std::thread::spawn(move || run_chain(h2, rid2, a2, lbr_p, step_p, parts_in));
    json!({"success": true, "started": true, "runId": run_id, "statusVerb": "fusion_prove_result", "shape": "A (.lbr direct)", "owner": "exe",
           "_hint": "The chain runs in the background (it outlives the relay window by design): poll fusion_prove_result {runId} for measured progress, streamed proof frames and the verdict. ONE call, ONE consent, ONE poll verb."})
}

fn md5_like(s: &str) -> u64 { use std::hash::{Hash, Hasher}; let mut h = std::collections::hash_map::DefaultHasher::new(); s.hash(&mut h); h.finish() }

fn run_chain(host: Arc<dyn ChainHost>, run_id: String, args: Value, lbr_p: String, step_p: String, parts_in: Vec<Value>) {
    let sd = host.state_dir();
    let banner = Banner::new(host.clone());
    let mut state = json!({"stage": "starting", "success": Value::Null, "libraryPath": if lbr_p.is_empty() { Value::Null } else { json!(lbr_p) }, "runId": run_id, "startedAt": now(), "verb": "create_library", "owner": "exe"});
    host.set_long_op(Some(format!("create_library:{run_id}")));
    banner.watchdog(run_id.clone());
    banner.send("show", &format!("{}: starting", lib_label(&lbr_p)), "checking the bundle and Fusion", None, None, "working");
    let docs_before: Vec<String> = host.call("chain_docs_open", json!({}), 15)["documents"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if !lbr_p.is_empty() { note_bridge_doc(&sd, &lbr_p); }
    let save = |st: &Value| save_state(&run_id, st);
    save(&state);
    // the plan and its estimates
    let needs_launch = !host.addin_round_trip_ready();
    let n_parts = parts_in.len().max(1);
    let (per_part, _) = estimate(&sd, "stage:bind_per_part", 60.0);
    // prove is for web control (the wiki page shows the frames); an AI thread binds and places
    // only unless it asks (issue #88). An explicit prove:true/false always wins.
    let do_prove = args["prove"].as_bool().unwrap_or_else(|| host.caller_thread().trim().is_empty());
    state["prove"] = json!({"planned": do_prove, "decidedBy": if args["prove"].is_boolean() { "argument" } else if do_prove { "caller: web control / anonymous" } else { "caller: AI thread (bind only; pass prove:true or use fusion_capture_library_views afterwards)" }});
    let stages: Vec<(&str, f64, bool)> = vec![("launch", 60.0, needs_launch), ("convert", 5.0, false), ("bind", per_part * n_parts as f64, !step_p.is_empty() || !parts_in.is_empty()), ("prove", 300.0, do_prove)];
    let plan: Vec<Value> = stages.iter().filter(|(_, _, a)| *a).map(|(n, f, _)| { let (e, c) = estimate(&sd, &format!("stage:{n}"), *f); json!({"stage": n, "estimatedSec": (e.max(*f).max(1.0) * 10.0).round() / 10.0, "confidence": c}) }).collect();
    let est_by: Map<String, Value> = plan.iter().map(|p| (p["stage"].as_str().unwrap().to_string(), p["estimatedSec"].clone())).collect();
    let est_total: f64 = est_by.values().filter_map(|v| v.as_f64()).sum::<f64>().max(1.0);
    let est_of = |k: &str| est_by.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
    state["plan"] = json!(plan); state["stagesDone"] = json!([]);
    let est_done = std::cell::Cell::new(0.0f64);
    let target_lbr = lbr_p.clone();
    let descs = library_descriptions(&target_lbr);
    let enter = |state: &mut Value, stage_name: &str| {
        state["stage"] = json!(stage_name); state["stageStartedAt"] = json!(now()); save(state);
        let pct = 100.0 * est_done.get() / est_total;
        taskbar(&*host, Some(pct));
        let n = state["partsTotal"].as_u64().unwrap_or(1).max(1);
        let key = stage_name.split(" (").next().unwrap_or(stage_name).to_lowercase();
        let what = if key.starts_with("starting") { "Unpacking the bundle and pairing every deviceset with its STEP".to_string() }
            else if key.starts_with("launch") { "Starting Fusion and waiting for the Adom add-in".into() }
            else if key.starts_with("convert") { "Converting KiCad sources into an EAGLE library".into() }
            else if key.starts_with("binding 3d") { format!("Minting one 3D package per footprint ({} STEP import{}), then binding the urns into the library", n, if n == 1 { "" } else { "s" }) }
            else if key.starts_with("proving") { format!("Opening every deviceset and capturing its symbol, footprint, 3D and component views ({} part{})", n, if n == 1 { "" } else { "s" }) }
            else { stage_name.to_string() };
        banner.send("update", &format!("{}: {}", lib_label(&target_lbr), stage_name.split(" (").next().unwrap_or(stage_name)), &what, Some(pct), Some(est_total - est_done.get()), "working");
    };
    let leave = |state: &mut Value, key: &str| {
        let took = now() - state["stageStartedAt"].as_f64().unwrap_or(now());
        log_run(&sd, &format!("stage:{key}"), took, true);
        crate::timings::record(&sd, &format!("stage:{key}"), took, None); // what estimate("stage:<key>") reads next run
        let mut done = state["stagesDone"].as_array().cloned().unwrap_or_default(); done.push(json!({"stage": key, "seconds": (took * 10.0).round() / 10.0})); state["stagesDone"] = json!(done);
        est_done.set(est_done.get() + est_of(key));
        taskbar(&*host, Some(100.0 * est_done.get() / est_total));
    };
    let fail = |state: &mut Value, code: &str, msg: &str| { state["stage"] = json!("failed"); state["success"] = json!(false); state["verdict"] = json!("not_proven"); state["errorCode"] = json!(code); state["error"] = json!(msg); save(state); };
    let cancelled = |state: &mut Value, at_stage: &str, detail: &str| {
        let by = cancel_requested(&run_id).and_then(|c| c["by"].as_str().map(str::to_string)).unwrap_or_else(|| "user".into());
        state["stage"] = json!("cancelled"); state["success"] = json!(false); state["verdict"] = json!("cancelled"); state["errorCode"] = json!("cancelled_by_user");
        state["cancelRequested"] = json!(true); state["cancelledAt"] = json!(at_stage); state["cancelledBy"] = json!(by);
        state["error"] = json!(format!("Cancelled by {by} while {at_stage}. {detail}")); state["stageLabel"] = json!(format!("cancelled while {at_stage}")); save(state);
        banner.send("update", "Import cancelled", &detail.chars().take(120).collect::<String>(), None, Some(0.0), "failed"); banner.hide_later(20);
    };
    let outcome: Result<(), ()> = (|| {
        // ── launch ──
        if needs_launch {
            if host.fusion_running() {
                let info = host.call("chain_addin_restart_info", json!({"fireToast": false}), 30);
                let mine: Vec<Value> = info["userUnsaved"].as_array().cloned().unwrap_or_default();
                let mut chosen = String::new();
                if mine.is_empty() && args["allowRestart"].as_bool() == Some(true) {
                    // only with the caller's explicit allowRestart (issue #87): a chain never relaunches Fusion on its own
                    state["restarts"] = json!([{"at": now(), "why": args["restartReason"].as_str().unwrap_or("add-in not answering; caller passed allowRestart:true"), "kind": "fusion"}]);
                    enter(&mut state, "Fusion is open but its Adom add-in is not answering - restarting Fusion (caller allowed it; only bridge documents are open)");
                    state["stageReason"] = json!("addin_not_answering"); state["restartChoice"] = json!("auto");
                    state["restartReason"] = json!(format!("no unsaved user documents; open: {}", info["openDocuments"])); save(&state);
                    let rr = host.call("restart_for_addin", json!({"save": true, "auto": true, "wait": true}), 900);
                    state["restartResult"] = json!({"success": rr["success"], "error": rr["error"], "errorCode": rr["errorCode"]}); save(&state);
                    chosen = "restarted".into();
                }
                if chosen != "restarted" {
                    let names: Vec<String> = mine.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
                    enter(&mut state, &format!("Fusion is open but its Adom add-in is not answering - not restarting on my own (pass allowRestart:true with a reason, or restart yourself); {} unsaved document(s) look like the user's: {}", names.len(), names.join(", ").chars().take(70).collect::<String>()));
                    state["stageReason"] = json!("addin_not_answering");
                    let ri = host.call("chain_addin_restart_info", json!({"fireToast": true}), 30);
                    state["errorCode"] = json!("addin_restart_required"); state["userMessage"] = ri["userMessage"].clone(); state["documentsKnown"] = ri["documentsKnown"].clone();
                    state["openDocuments"] = ri["openDocuments"].clone(); state["unsavedDocuments"] = ri["unsavedDocuments"].clone(); state["remedyVerb"] = json!("fusion_restart_for_addin");
                    state["notificationId"] = ri["toast"]["id"].clone(); save(&state);
                    let deadline = Instant::now() + Duration::from_secs(1800);
                    while Instant::now() < deadline {
                        if host.addin_round_trip_ready() { chosen = "restarted".into(); break; }
                        if hold_active(&sd) { chosen = "hold".into(); break; }
                        std::thread::sleep(Duration::from_secs(4));
                    }
                    if chosen == "hold" { fail(&mut state, "user_owns_restart", "You chose 'I'll handle it' on the restart notice, so nothing was restarted for you. Save and restart Fusion yourself, then start the import again."); return Err(()); }
                    if chosen != "restarted" { fail(&mut state, "awaiting_user_restart", "Fusion's add-in stayed silent for 30 minutes and nobody restarted Fusion. Save and restart Fusion, then start the import again."); return Err(()); }
                    if let Some(o) = state.as_object_mut() { o.remove("errorCode"); o.entry("restartChoice").or_insert(json!("restart")); }
                    save(&state);
                }
            } else {
                enter(&mut state, "launching Fusion (add-in loads at start)");
                state["stageReason"] = json!("fusion_not_running"); save(&state);
                let _ = host.call("start", json!({"wait": true}), 400);
            }
            let deadline = Instant::now() + Duration::from_secs(240);
            while Instant::now() < deadline && !host.addin_round_trip_ready() { std::thread::sleep(Duration::from_secs(5)); }
            if !host.addin_round_trip_ready() { state["launch"] = json!({"success": false, "error": "Fusion did not become ready within 240s of the chain launching it."}); fail(&mut state, "launch_timeout", "Fusion did not become ready within 240s of the chain launching it."); return Err(()); }
            leave(&mut state, "launch");
        }
        state["libraryPath"] = json!(target_lbr);
        let mut parts = parts_in.clone();
        if parts.is_empty() && !step_p.is_empty() {
            let pkgs = std::fs::read_to_string(&target_lbr).ok().and_then(|x| fusion_lbr::artifact_names(&x).ok()).map(|n| n.pac).unwrap_or_default();
            parts.push(json!({"package": pkgs.first().cloned().unwrap_or_default(), "lbrPath": target_lbr, "modelPath": step_p}));
        }
        state["partsTotal"] = json!(parts.len());
        // ── bind ──
        if !parts.is_empty() {
            enter(&mut state, "binding 3D (STEP -> f3d -> package3d)");
            for _ in 0..60 {
                let st = host.addin_status();
                if !st["busy"].as_bool().unwrap_or(false) || matches!(st["busyCommand"].as_str(), Some("activity_banner") | Some("get_app_state")) { break; }
                state["stageLabel"] = json!("waiting for Fusion to finish a previous command before binding"); save(&state);
                std::thread::sleep(Duration::from_secs(2));
            }
            let mut bind = json!({});
            for _ in 0..40 {
                bind = host.call("build_library_3d", json!({"lbrPath": target_lbr, "parts": parts, "outLbrPath": target_lbr, "async": false, "_cancelKey": run_id}), 3600);
                let berr = format!("{}{}", bind["error"].as_str().unwrap_or(""), bind["errorCode"].as_str().unwrap_or("")).to_lowercase();
                if bind["success"].as_bool() == Some(true) || (!berr.contains("main thread busy") && !berr.contains("main_thread_busy")) { break; }
                std::thread::sleep(Duration::from_secs(3));
            }
            state["bind"] = json!({"success": bind["success"], "error": bind["error"], "wip_urn": bind["wip_urn"], "reopened": bind["reopened"], "reopenError": bind["reopenError"],
                                   "parts": if bind["results"].is_null() { bind["parts"].clone() } else { bind["results"].clone() }, "partsBound": bind["partsBound"]});
            if bind["cancelled"].as_bool() == Some(true) || cancel_requested(&run_id).is_some() {
                let detail = format!("{} of {} part(s) were bound before the stop; their urns are in the library.", bind["partsBound"].as_u64().unwrap_or(0), parts.len());
                cancelled(&mut state, "binding 3D", &detail); return Err(());
            }
            if bind["success"].as_bool() != Some(true) {
                if state["bind"]["error"].is_null() || state["bind"]["error"].as_str() == Some("") {
                    let u = state["bind"]["wip_urn"].as_str().unwrap_or("").to_string();
                    state["bind"]["error"] = json!(if u.is_empty() { "bind failed and returned no reason and no urn".to_string() } else { format!("bind reported failure but gave no reason; a urn WAS minted ({u}) silently") });
                }
                let msg = format!("3D bind failed: {}", state["bind"]["error"].as_str().unwrap_or("").chars().take(300).collect::<String>());
                let code = bind["errorCode"].as_str().unwrap_or("bind_failed").to_string();
                fail(&mut state, &code, &msg); return Err(());
            }
            leave(&mut state, "bind");
            for cd in args["_cleanupAfterBind"].as_array().cloned().unwrap_or_default() {
                if let Some(p) = cd.as_str() { if p.starts_with(&std::env::temp_dir().to_string_lossy().to_string()) { let _ = std::fs::remove_dir_all(p); } }
            }
        }
        // ── prove ──
        if !do_prove {
            state["stage"] = json!("done"); state["success"] = json!(state["bind"]["success"].as_bool().unwrap_or(true) || state["bind"].is_null()); state["verdict"] = json!("bound_not_proven"); state["evidenceCount"] = json!(0);
            state["prove"]["skipped"] = json!(true); state["prove"]["why"] = json!("called from an AI thread: bind and place only (issue #88). Proof frames are for web control; ask for them with prove:true or fusion_capture_library_views {packages} for one part.");
            state["_hint"] = json!("Bound, not proven: every part with a urn in bind.parts is placed in the library. No evidence frames were captured because an AI thread asked; call fusion_capture_library_views {packages:[...]} to look at one part, or re-run with prove:true for the four frames per part (about 300 s per library).");
        } else {
        enter(&mut state, "proving (four evidence frames)");
        let names = std::fs::read_to_string(&target_lbr).ok().and_then(|x| fusion_lbr::artifact_names(&x).ok()).unwrap_or_default();
        let want = args["name"].as_str().unwrap_or("").trim().to_string();
        let targets: Vec<String> = if !want.is_empty() { vec![want] } else if names.dev.is_empty() { vec![String::new()] } else { names.dev.clone() };
        state["partsToProve"] = json!(targets.len()); state["provedParts"] = json!([]);
        let dev_pac = names.dev_pac.clone();
        let uniq: std::collections::HashSet<Vec<String>> = dev_pac.values().cloned().collect();
        let uniq_pkgs = if uniq.is_empty() { targets.len() } else { uniq.len() };
        state["framesExpected"] = json!(4 * uniq_pkgs.min(targets.len()) + 2 * targets.len().saturating_sub(uniq_pkgs));
        let mut results: Vec<Value> = Vec::new();
        let mut pkg_proved_by: std::collections::HashMap<Vec<String>, String> = std::collections::HashMap::new();
        let mut last_beat = now();
        let wip = state["bind"]["wip_urn"].as_str().unwrap_or("").to_string();
        let lib_stem = lib_stem(&target_lbr).to_lowercase();
        let publish = |state: &mut Value, mut entry: Value, dev: &str, retry: bool| {
            entry["part"] = json!(dev); entry["kind"] = json!("proof"); if retry { entry["retry"] = json!(true); }
            let mut ev = state["evidence"].as_array().cloned().unwrap_or_default(); ev.push(entry.clone());
            let n = ev.len(); state["evidence"] = json!(ev); state["evidenceCount"] = json!(n); state["lastArtifact"] = entry["step"].clone();
            let tot = state["framesExpected"].as_u64().unwrap_or(0).max(1) as usize;
            state["stageLabel"] = json!(format!("part {} of {}: captured {} (frame {} of {})", state["partIndex"], state["partsToProve"], entry["step"].as_str().unwrap_or("?"), n, tot));
            let pct = 100.0 * (est_done.get() + est_of("prove") * n as f64 / tot as f64) / est_total;
            banner.send("update", &format!("{}: proving part {} of {}", lib_label(&target_lbr), state["partIndex"], state["partsToProve"]),
                        &format!("{}. Captured {} (frame {} of {})", named(if dev.is_empty() { "library" } else { dev }, &descs), entry["step"].as_str().unwrap_or("?"), n, tot), Some(pct), Some(est_total - est_done.get() - est_of("prove") * n as f64 / tot as f64), "working");
            save(state);
        };
        let mut prove_one = |state: &mut Value, dev: &str, pi: usize, retry: bool| -> Value {
            let key: Vec<String> = dev_pac.get(dev).cloned().unwrap_or_default();
            let shared = if key.is_empty() || retry { None } else { pkg_proved_by.get(&key).cloned() };
            // frames stream through a sink file the sidecar appends to; we tail it while the
            // call is in flight so a poller sees each artifact the instant it lands
            let sink = std::env::temp_dir().join(format!("fusion-prove-frames-{}-{}.jsonl", run_id, md5_like(&format!("{dev}{retry}{}", now()))));
            let _ = std::fs::remove_file(&sink);
            let mut a = json!({"libraryPath": target_lbr, "name": dev, "f3dUrn": wip, "_frameSink": sink.to_string_lossy().replace('\\', "/")});
            if let Some(s) = &shared { a["_skipSteps"] = json!(["footprint", "package3d"]); a["_sharedWith"] = json!(s); }
            let (tx, rx) = std::sync::mpsc::channel::<Value>();
            let h2 = host.clone();
            std::thread::spawn(move || { let _ = tx.send(h2.call("prove_library", a, 3600)); });
            let mut seen = 0usize;
            let mut drain = |state: &mut Value| {
                let lines: Vec<String> = std::fs::read_to_string(&sink).map(|t| t.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()).unwrap_or_default();
                for l in lines.iter().skip(seen) { if let Ok(e) = serde_json::from_str::<Value>(l) { publish(state, e, dev, retry); } }
                seen = seen.max(lines.len());
            };
            let r = loop {
                match rx.recv_timeout(Duration::from_millis(1500)) {
                    Ok(r) => { drain(state); break r; }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => drain(state),
                    Err(_) => break json!({"success": false, "proven": false, "error": "prove_library call thread ended without a result"}),
                }
            };
            let _ = std::fs::remove_file(&sink);
            if seen == 0 { for e in r["evidence"].as_array().cloned().unwrap_or_default() { publish(state, e, dev, retry); } }
            let proven = r["proven"].as_bool().unwrap_or(false);
            if proven && !key.is_empty() && shared.is_none() { pkg_proved_by.insert(key, dev.to_string()); }
            let closed = host.call("chain_close_docs_since", json!({"before": docs_before, "keep": [lib_stem]}), 120)["closed"].clone();
            let _ = pi;
            json!({"part": dev, "proven": proven, "error": r["error"], "sharedPackageProof": shared, "scratchClosed": closed})
        };
        for (i, dev) in targets.iter().enumerate() {
            let pi = i + 1;
            let gap = now() - last_beat;
            if gap > 900.0 {
                state["provedParts"] = json!(results);
                cancelled(&mut state, "proving", &format!("The machine was asleep or the bridge was frozen for {} minutes; {} of {} parts were proven before that. Run the import again for a full proof.", (gap / 60.0) as i64, pi - 1, targets.len()));
                state["errorCode"] = json!("interrupted_by_sleep"); state["cancelledBy"] = json!("sleep");
                state["error"] = json!(state["error"].as_str().unwrap_or("").replace("Cancelled by user while", "Stopped while")); save(&state); return Err(());
            }
            if cancel_requested(&run_id).is_some() {
                state["provedParts"] = json!(results);
                cancelled(&mut state, "proving", &format!("{} of {} parts were proven before the stop; the library keeps every binding. Run the import again for a full proof for every part", pi - 1, targets.len())); return Err(());
            }
            state["stageLabel"] = json!(format!("proving {} (part {} of {})", if dev.is_empty() { "library" } else { dev }, pi, targets.len()));
            state["partIndex"] = json!(pi); save(&state);
            let frac = (pi - 1) as f64 / targets.len().max(1) as f64;
            banner.send("update", &format!("{}: proving part {} of {}", lib_label(&target_lbr), pi, targets.len()),
                        &format!("{}. Opening its symbol, footprint, 3D package and ganged view", named(if dev.is_empty() { "library" } else { dev }, &descs)),
                        Some(100.0 * (est_done.get() + est_of("prove") * frac) / est_total), Some(est_total - est_done.get() - est_of("prove") * frac), "working");
            let r = prove_one(&mut state, dev, pi, false);
            results.push(r); state["provedParts"] = json!(results); last_beat = now(); save(&state);
        }
        let transient = ["ACTIVE document", "addin_command_timeout", "not responding", "main thread busy", "main_thread_busy"];
        let retry_idx: Vec<usize> = results.iter().enumerate().filter(|(_, r)| r["proven"].as_bool() != Some(true) && contains_any(r["error"].as_str().unwrap_or(""), &transient)).map(|(i, _)| i).collect();
        for i in retry_idx {
            if cancel_requested(&run_id).is_some() { break; }
            let dev = results[i]["part"].as_str().unwrap_or("").to_string();
            state["stageLabel"] = json!(format!("re-proving {dev} (its first pass hit a timing miss, not a part fault)")); save(&state);
            let r2 = prove_one(&mut state, &dev, i + 1, true);
            results[i]["proven"] = r2["proven"].clone(); results[i]["error"] = r2["error"].clone(); results[i]["retried"] = json!(true); results[i]["scratchClosed"] = r2["scratchClosed"].clone();
            state["provedParts"] = json!(results); save(&state);
        }
        let all_ok = results.iter().all(|r| r["proven"].as_bool() == Some(true));
        let prove = json!({"proven": all_ok, "parts": results, "partsProven": results.iter().filter(|r| r["proven"].as_bool() == Some(true)).count(), "partsTotal": results.len()});
        leave(&mut state, "prove");
        let frames = state["evidence"].as_array().map(|a| a.len()).unwrap_or(0);
        if frames == 0 {
            state["stage"] = json!("failed"); state["success"] = json!(false); state["verdict"] = json!("not_proven"); state["errorCode"] = json!("no_evidence_captured");
            state["error"] = json!("The chain ended with zero evidence frames: nothing was photographed, so the library is reported as not proven rather than done.");
            state["evidenceCount"] = json!(0); state["prove"] = prove;
            state["_hint"] = json!("stage is 'failed', not 'done', because zero of the four evidence frames exist; the bind likely never produced a document to photograph.");
        } else {
            state["stage"] = json!("done"); state["success"] = json!(all_ok); state["verdict"] = json!(if all_ok { "proven" } else { "not_proven" }); state["evidenceCount"] = json!(frames); state["prove"] = prove;
        }
        }
        if let Some(pub_) = args["_publishToCloud"].as_object() {
            if state["bind"]["success"].as_bool() == Some(true) {
                state["stageLabel"] = json!("saving the library to your cloud Library Manager"); save(&state);
                let r = host.call("chain_publish_library", json!({"libraryPath": target_lbr, "name": pub_.get("name").cloned().unwrap_or(json!("")), "folder": pub_.get("folder").cloned().unwrap_or(json!("Adom Libraries")), "replaceExisting": pub_.get("replaceExisting").and_then(|v| v.as_bool()).unwrap_or(false)}), 300);
                let mut cl = r.clone(); if let Some(o) = cl.as_object_mut() { o.remove("success"); o.remove("_hint"); }
                state["cloudLibrary"] = cl; save(&state);
            }
        }
        let ok = state["stage"].as_str() == Some("done") && state["success"].as_bool() == Some(true);
        banner.send("update", if ok { "Library imported" } else { "Import did not complete" },
                    &(if ok && state["prove"]["skipped"].as_bool() == Some(true) { format!("{} part(s) bound; proof skipped (AI thread)", state["bind"]["partsBound"]) } else if ok { format!("{} of {} parts proven", state["prove"]["partsProven"], state["prove"]["partsTotal"]) } else { state["error"].as_str().or(state["errorCode"].as_str()).unwrap_or("").chars().take(120).collect() }),
                    if ok { Some(100.0) } else { None }, Some(0.0), if ok { "done" } else { "failed" });
        banner.hide_later(20);
        state["partMetadata"] = crate::orchestrations::part_metadata_report(&target_lbr);
        save(&state);
        Ok(())
    })();
    let _ = outcome;
    // ── finally: hygiene ──
    host.set_long_op(None);
    taskbar(&*host, None);
    let keep_open = args["_publishToCloud"].is_object();
    let stem_done = lib_stem(state["libraryPath"].as_str().unwrap_or(&lbr_p));
    if !stem_done.is_empty() && !keep_open {
        let c = host.call("chain_close_bridge_doc", json!({"name": stem_done}), 60);
        state["libraryDocClosed"] = json!(c["closed"].as_bool().unwrap_or(false)); save(&state);
    }
    let keep: Vec<String> = if keep_open { vec![stem_done.to_lowercase()] } else { vec![] };
    state["scratchDocsClosed"] = host.call("chain_close_docs_since", json!({"before": docs_before, "keep": keep}), 120)["closed"].clone(); save(&state);
    if keep_open && !stem_done.is_empty() {
        let _ = host.call("activate_document", json!({"name": stem_done}), 10);
        state["libraryDocClosed"] = json!(false); state["libraryLeftOpen"] = json!(true); save(&state);
    }
}

/// `fusion_install_library_bundle`: unpack, pair every deviceset with its STEP, place the
/// library under Documents/AdomLibraries, split one library per part, then the chain.
pub fn install_library_bundle(host: Arc<dyn ChainHost>, args: Value) -> Value {
    use fusion_lbr::*;
    let zip_p = args["zipPath"].as_str().unwrap_or("").replace('\\', "/");
    let lbr_in = args["lbrPath"].as_str().or(args["libraryPath"].as_str()).unwrap_or("").replace('\\', "/");
    if zip_p.is_empty() && lbr_in.is_empty() {
        return json!({"success": false, "errorCode": "no_input", "error": "Give me zipPath (a staged Cloud Library bundle) or lbrPath (a staged .lbr with its STEP models beside it).",
                      "_hint": "Stage with write_file / write_file_chunk first; the path must exist on THIS machine. Then poll fusion_prove_result with the runId."});
    }
    let src = if zip_p.is_empty() { lbr_in.clone() } else { zip_p.clone() };
    if !std::path::Path::new(&src).exists() { return json!({"success": false, "errorCode": "not_found", "error": format!("Not on this machine: {src}"), "_hint": "Stage it with write_file / write_file_chunk first, then pass that path."}); }
    let mut lib_name = args["libraryName"].as_str().unwrap_or("").trim().to_string();
    let mut work: Option<std::path::PathBuf> = None;
    let result = (|| -> Value {
        let (lbr_found, models, all_lbrs);
        if !zip_p.is_empty() {
            if std::fs::metadata(&zip_p).map(|m| m.len() > MAX_ZIP_BYTES).unwrap_or(false) { return json!({"success": false, "errorCode": "too_large", "error": "Bundle zip is larger than the 256 MB cap"}); }
            let w = std::env::temp_dir().join(format!("adom-lib-bundle-{}", md5_like(&format!("{}", now()))));
            work = Some(w.clone());
            let n = match safe_extract(std::path::Path::new(&zip_p), &w) { Ok(n) => n, Err(e) => return json!({"success": false, "errorCode": "bad_bundle", "error": format!("Could not unpack the bundle: {}", e.to_string().chars().take(160).collect::<String>())}) };
            if n == 0 { return json!({"success": false, "errorCode": "bad_bundle", "error": "The bundle zip is empty"}); }
            let f = find_bundle_files(&w);
            let Some(l) = f.lbr else { return json!({"success": false, "errorCode": "no_lbr", "error": "No .lbr in the bundle", "_hint": "Expected an Adom Cloud Library bundle for Fusion: <Lib>.lbr plus <Part>.step models (beside it or under 3dmodels/)."}) };
            lbr_found = l; models = f.models; all_lbrs = f.all_lbrs;
            if lib_name.is_empty() { lib_name = std::path::Path::new(&zip_p).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(); }
        } else {
            lbr_found = lbr_in.clone();
            let d = std::path::Path::new(&lbr_in).parent().map(|p| p.to_path_buf()).unwrap_or_else(|| ".".into());
            let f = find_bundle_files(&d); models = f.models; all_lbrs = f.all_lbrs;
            if lib_name.is_empty() { lib_name = std::path::Path::new(&lbr_in).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(); }
        }
        if !library_name_ok(&lib_name) { return json!({"success": false, "errorCode": "bad_name", "error": format!("Library name not usable: {}", fusion_lbr::py_repr_str(&lib_name)), "_hint": "Letters, digits, space, dot, dash, underscore; up to 64 chars."}); }
        let pf = preflight_lbr(std::path::Path::new(&lbr_found));
        if !pf.ok { return json!({"success": false, "errorCode": "lbr_preflight_failed", "error": format!("The .lbr will not open in Fusion: {}", pf.errors.join("; ").chars().take(300).collect::<String>()), "preflight": pf}); }
        let xml = std::fs::read_to_string(&lbr_found).unwrap_or_default();
        let meta = match library_part_metadata(&xml) { Ok(m) => m, Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": e.to_string()}) };
        let mut pairing = pair_parts(&lbr_found, &models, &meta);
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
        let dest = std::path::Path::new(&home).join("Documents").join("AdomLibraries").join(regex::Regex::new(r"\s+").unwrap().replace_all(&lib_name, "_").to_string());
        // carry prior 3D bindings from an earlier install of the same library
        let dest_lbr = dest.join(std::path::Path::new(&lbr_found).file_name().unwrap_or_default());
        let old_bindings: Vec<Binding> = std::fs::read_to_string(&dest_lbr).ok().map(|t| {
            let re = regex::Regex::new(r#"<package3d name="([^"]+)"[^>]*wip_urn="([^"]+)""#).unwrap();
            re.captures_iter(&t).map(|c| Binding { package: c[1].to_string(), wip_urn: c[2].to_string() }).collect() }).unwrap_or_default();
        let (lbr_path, placed_models) = match place_library(std::path::Path::new(&lbr_found), &models, &dest) { Ok(x) => x, Err(e) => return json!({"success": false, "errorCode": "place_failed", "error": e.to_string()}) };
        if !old_bindings.is_empty() {
            if let Ok(t) = std::fs::read_to_string(&lbr_path) { let _ = std::fs::write(&lbr_path, inject_package3d_bindings(&t, &old_bindings)); eprintln!("[fusion-bridge] carried {} prior 3D binding(s) into the fresh install", old_bindings.len()); }
        }
        let split_dir = std::env::temp_dir().join(format!("adom-lib-parts-{}", md5_like(&lbr_path)));
        let placed_xml = std::fs::read_to_string(&lbr_path).unwrap_or_default();
        let split = match split_per_part(&placed_xml, &split_dir) { Ok(s) => s, Err(e) => return json!({"success": false, "errorCode": "split_failed", "error": e.to_string()}) };
        let mut parts: Vec<Value> = Vec::new();
        for p in &pairing.parts {
            let ds = if p.deviceset.is_empty() { p.package.clone() } else { p.deviceset.clone() };
            let sp = (if p.package.is_empty() { None } else { split.get(&format!("{ds}::{}", p.package)) }).or_else(|| split.get(&ds));
            match sp {
                Some(e) if !e.lbr_path.is_empty() && !e.package.is_empty() => parts.push(json!({"deviceset": ds, "package": e.package, "lbrPath": e.lbr_path, "modelPath": placed_models.get(&p.model_path).cloned().unwrap_or(p.model_path.clone()).replace('\\', "/"), "orient": false})),
                _ => pairing.unbound.push(Unbound { package: ds.clone(), deviceset: ds.clone(), device: String::new(), why: "could not split this deviceset into its own library (no package on its devices?)".into() }),
            }
        }
        let report = crate::orchestrations::part_metadata_report(&lbr_path.replace('\\', "/"));
        let plan = host.call("plan_library_import", json!({"parts": parts}), 20);
        let mut out = json!({"success": true, "installed": true, "libraryName": lib_name, "libraryPath": lbr_path, "installDir": dest.to_string_lossy().replace('\\', "/"),
            "partsTotal": meta.devicesets.len(), "partsToBind": parts.len(),
            "parts": parts.iter().map(|p| json!({"deviceset": p["deviceset"], "package": p["package"], "modelPath": p["modelPath"], "mintedOn": p["lbrPath"]})).collect::<Vec<_>>(),
            "mint": "each part minted on its own single-deviceset library so the generator loads that part's footprint; STEP placed as published (orient:false, origin trusted)",
            "unbound": pairing.unbound, "modelsUnused": pairing.models_unused.iter().map(|m| std::path::Path::new(m).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()).collect::<Vec<_>>(),
            "shared": pairing.shared, "partMetadata": report, "fusion": plan["fusion"], "plan": plan["plan"], "estimatedSec": plan["estimatedSec"],
            "otherLbrsInBundle": all_lbrs.iter().skip(1).map(|x| std::path::Path::new(x).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()).collect::<Vec<_>>(), "owner": "exe"});
        if args["capture"].as_bool() == Some(false) {
            out["started"] = json!(false); out["statusVerb"] = Value::Null;
            out["_hint"] = json!("capture:false - the library is placed at libraryPath and nothing was opened in Fusion. Call fusion_create_library {parts} later to bind and prove.");
            return out;
        }
        if parts.is_empty() {
            out["started"] = json!(false); out["errorCode"] = json!("nothing_to_bind");
            out["_hint"] = json!("Installed, but no deviceset could be paired with a STEP, so there is nothing to bind or prove. Add STEPs named like the deviceset or its package and call again.");
            return out;
        }
        let ai_thread = !host.caller_thread().trim().is_empty();
        let prove = args["prove"].as_bool().unwrap_or(!ai_thread);
        let chain = create_library(host.clone(), json!({"libraryPath": lbr_path, "parts": parts, "prove": prove, "allowRestart": args["allowRestart"], "restartReason": args["restartReason"], "_cleanupAfterBind": [split_dir.to_string_lossy()],
            "_publishToCloud": {"name": lib_name, "folder": "Adom Libraries", "replaceExisting": args["replaceExisting"].as_bool().unwrap_or(false)}}));
        for k in ["started", "attached", "runId", "statusVerb", "stage"] { out[k] = chain[k].clone(); }
        if chain["success"].as_bool() != Some(true) { out["success"] = json!(false); out["error"] = chain["error"].clone(); out["errorCode"] = chain["errorCode"].clone(); }
        let prove_est = plan["plan"].as_array().and_then(|a| a.iter().find(|s| s["stage"] == "prove")).and_then(|s| s["estimatedSec"].as_f64()).unwrap_or(300.0);
        out["prove"] = json!({"planned": prove, "decidedBy": if args["prove"].is_boolean() { "argument" } else if ai_thread { "caller: AI thread, bind only" } else { "caller: web control / anonymous, proves" }, "estimatedSec": if prove { prove_est } else { 0.0 }});
        out["_hint"] = json!(if prove { format!("Installed to libraryPath. The bind chain runs in the background, then PROVE captures four evidence frames per part (about {prove_est:.0} s more): poll fusion_prove_result {{runId}}. From an AI thread pass prove:false to skip proof (bind and place only).") } else { "Installed to libraryPath. The bind chain runs in the background (bind and place only, no proof frames: this call came from an AI thread): poll fusion_prove_result {runId} for partsBound and the library path; fusion_capture_library_views {packages} shows one part when you want to look; prove:true re-runs with the frames.".to_string() });
        out
    })();
    if let Some(w) = work { let _ = std::fs::remove_dir_all(w); }
    result
}

/// `fusion_prove_result`: the run store, with the measured progress block for a live run.
pub fn prove_result(args: &Value) -> Value {
    let rid = args["runId"].as_str().unwrap_or("").trim().to_string();
    let runs = read_runs();
    let mut res: Option<Value> = None;
    if !rid.is_empty() {
        res = runs.get(&rid).cloned();
        if res.is_none() {
            return json!({"success": false, "errorCode": "unknown_run_id", "error": format!("no prove run with runId {} on this box", fusion_lbr::py_repr_str(&rid)),
                          "_hint": "runId comes from fusion_create_library's response. Omit runId for the latest completed run (single-caller convenience only)."});
        }
    }
    if res.is_none() { res = std::fs::read_to_string(last_path()).ok().and_then(|t| serde_json::from_str(&t).ok()); }
    let Some(mut r) = res else {
        return json!({"success": false, "errorCode": "no_prove_run", "error": "No fusion_prove_library run has completed on this box yet.",
                      "_hint": "Run fusion_prove_library {libraryPath}; if the relay times out, the bridge keeps working - poll this verb until the result appears."});
    };
    if args["inlineFrames"].as_bool() != Some(true) {
        for key in ["evidence"] { if let Some(a) = r[key].as_array_mut() { for e in a { if let Some(o) = e.as_object_mut() { o.remove("thumbnailDataUrl"); } } } }
        if let Some(a) = r["prove"]["evidence"].as_array_mut() { for e in a { if let Some(o) = e.as_object_mut() { o.remove("thumbnailDataUrl"); } } }
    }
    if !matches!(r["stage"].as_str(), None | Some("done") | Some("failed")) && r["plan"].is_array() {
        let plan = r["plan"].as_array().cloned().unwrap_or_default();
        let total: f64 = plan.iter().map(|p| p["estimatedSec"].as_f64().unwrap_or(0.0)).sum::<f64>().max(1.0);
        let done: std::collections::HashSet<String> = r["stagesDone"].as_array().map(|a| a.iter().filter_map(|d| d["stage"].as_str().map(str::to_string)).collect()).unwrap_or_default();
        let done_est: f64 = plan.iter().filter(|p| done.contains(p["stage"].as_str().unwrap_or(""))).map(|p| p["estimatedSec"].as_f64().unwrap_or(0.0)).sum();
        let cur = plan.iter().find(|p| !done.contains(p["stage"].as_str().unwrap_or("")));
        let cur_elapsed = (now() - r["stageStartedAt"].as_f64().unwrap_or(now())).max(0.0);
        let cur_est = cur.and_then(|c| c["estimatedSec"].as_f64()).unwrap_or(1.0);
        let cur_credit = cur_elapsed.min(cur_est * 0.95);
        let pct = ((100.0 * (done_est + cur_credit) / total) as i64).min(95);
        let eta = (total - done_est - cur_credit).max(0.0) as i64;
        let confs: Vec<&str> = plan.iter().map(|p| p["confidence"].as_str().unwrap_or("")).collect();
        let conf = if confs.iter().all(|c| *c == "measured") { "measured" } else if confs.iter().all(|c| *c == "unknown") { "unknown" } else { "typical" };
        r["percent"] = json!(pct); r["etaSec"] = json!(eta);
        r["progress"] = json!({"percent": pct, "etaSec": eta, "elapsedSec": cur_elapsed as i64, "stepLabel": r["stage"], "estimatedSec": total as i64, "confidence": conf, "overdue": cur_elapsed > cur_est});
    }
    if !rid.is_empty() { return json!({"success": true, "result": r, "_hint": "Your run, from the per-run store (concurrent chains no longer overwrite each other)."}); }
    json!({"success": true, "result": r, "_hint": "The last completed prove run (survives relay timeouts and respawns)."})
}

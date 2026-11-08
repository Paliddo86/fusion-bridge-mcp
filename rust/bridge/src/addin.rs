//! Phase 4: the add-in pass-through, owned by the exe behind the pref `addinOwner: "exe"`.
//! Mirrors server.py's ADDIN_COMMANDS path: the preamble (installed / running / auto-start /
//! hang gate) is native in `launch.rs` (`gate`), leaning on the sidecar only for the hang
//! verdict, the recovery dismissal and the restart-info toast,
//! the two busy gates and the HTTP call to the add-in are native, a timeout is diagnosed by
//! `chain_diagnose_addin_timeout`, and a mutating verb carries the post-op dialog array (native:
//! `small_verbs::merge_dialog_array`, the classifier plus ab screenshots). The
//! taskbar indicator around the call is native (`timings::Indicator`, ab's desktop_taskbar).
//! The window etiquette (document registry, background envelope, save-prompt sentinel, the
//! post-verb sweep) is native in `etiquette.rs`; only the preamble and the timeout diagnosis
//! still run on the sidecar.

use serde_json::{json, Value};
use std::time::{Duration, Instant};
use std::sync::Mutex;

pub const ADDIN_COMMANDS: &[&str] = &[
    "mechanical",
    "activity_banner", "assembly_bom", "physical_properties", "get_app_state", "document_info", "activate_document", "import_step",
    "export_step", "export_stl", "export_3mf", "export_f3d", "export_fbx", "export_usdz", "export_dxf", "export_dwg", "export_iges", "export_obj", "export_sat", "export_skp",
    "get_design_info", "get_parameters", "set_parameter", "take_screenshot", "electron_run", "execute_text_command", "electron_zoom", "electron_pan", "electron_select",
    "open_electronics", "list_text_commands", "export_source", "export_eagle_source", "export_lbr", "close_document", "close_all_documents", "board_info", "open_by_urn",
    "run_modeling_script", "save_to_cloud", "list_cloud_projects", "list_cloud_files", "delete_cloud_file", "create_cloud_folder", "check_recovery", "search_cloud_files",
    "walk_cloud_tree", "export_cloud_file", "export_bom", "export_cpl", "export_gerbers", "export_board_image", "detect_layers",
];
/// `_WINDOW_RAISING` ∩ ADDIN_COMMANDS: these keep the sidecar's etiquette envelope.
pub const WINDOW_RAISING: &[&str] = &[
    "close_document", "close_window", "open_cloud_file", "open_by_urn", "aps_open", "open_design", "open_lbr", "open_schematic", "open_board", "import_electronics", "import_step",
    "new_electronics_from_eagle", "show_schematic", "show_2d_board", "show_3d_board", "run_modeling_script", "execute_text_command", "electron_run", "attach_3d_package",
    "make_3d_package", "build_library_3d", "capture_library_views", "generate_package", "export_optimized_glb", "board_stackup", "save_to_cloud", "start",
    "show_symbol", "show_footprint", "show_3d_package", "make_test_project",
];
const LONG_RUNNING: &[&str] = &["walk_cloud_tree", "search_cloud_files"];
const BUSY_EXEMPT: &[&str] = &["activity_banner"];
const MUTATING: &[&str] = &["mechanical", "run_modeling_script", "execute_text_command", "electron_run", "close_document", "close_all_documents", "import_step", "set_parameter", "save_to_cloud", "delete_cloud_file"];
const ASSEMBLY_BOM_UNIT_DEFAULTS: &[&str] = &["with fasteners", "with fastener", "with bearings", "bearing", "idler pulley", "pulley", "idler"];

fn addin_name(cmd: &str) -> &str { if cmd == "import_step" { "import_file" } else { cmd } }

pub fn timeout_secs(cmd: &str) -> u64 { timeout_secs_or(cmd, 30) }
/// `ADDIN_COMMAND_TIMEOUTS.get(cmd, default)`.
pub fn timeout_secs_or(cmd: &str, default: u64) -> u64 {
    match cmd {
        "show_symbol" | "show_footprint" => 180, "show_3d_package" | "make_test_project" => 300, "bind_library_3d" => 480, "show_library" | "show_project" => 240, "demo_progress" => 15,
        "mechanical" | "assembly_bom" | "physical_properties" | "export_step" | "export_iges" | "export_sat" | "export_stl" | "export_3mf" | "export_usdz" | "export_obj" | "export_f3d" | "export_fbx" | "export_skp" => 300,
        "export_dxf" | "export_dwg" => 120, "export_gerbers" => 180, "export_bom" | "export_cpl" | "export_board_image" | "close_all_documents" => 60,
        "walk_cloud_tree" => 600, "search_cloud_files" => 180, "open_by_urn" => 240, "run_modeling_script" => 180, _ => default,
    }
}

/// Does the exe take this verb natively? Every add-in verb: the window-raising ones carry the
/// sidecar's etiquette through `chain_pre_verb` / `chain_post_verb` / `chain_sentinel`.
pub fn owns(cmd: &str) -> bool { ADDIN_COMMANDS.contains(&cmd) || OPEN_WITH_SCREENSHOT.contains(&cmd) }
/// `_OPEN_WITH_SCREENSHOT`: add-in opens the orchestration layer wraps with a capture.
pub const OPEN_WITH_SCREENSHOT: &[&str] = &["open_schematic", "open_board", "show_3d_board", "show_2d_board", "show_schematic", "import_electronics"];
const DOC_SWITCHING: &[&str] = &["open_lbr", "open_cloud_file", "open_by_urn", "open_schematic", "open_board", "show_schematic", "show_2d_board", "show_3d_board", "import_electronics", "execute_text_command", "electron_run", "new_electronics_from_eagle"];

/// The native timeout diagnosis needs the shell context; `call_addin` reaches it through the
/// same per-call registry the sentinel uses (the borrowed `Ctx` cannot cross into `call_addin`).
fn diagnose(cmd: &str, original: &Value, sidecar: &dyn Fn(&str, Value, u64) -> Value) -> Value {
    let reg = SENTINEL_CTX.lock().unwrap().clone();
    match reg {
        Some((sd, ca, pids)) => {
            let host = crate::host_for_diagnosis();
            let dlg = |r: Value| r;
            let ctx = Ctx { host: &*host, fusion_exe: host_paths_exe(), short_path: |p| crate::host().short_path(p), state_dir: sd, caller: ca, pids, sidecar, dialogs: &dlg };
            crate::lifecycle::diagnose_addin_timeout(&ctx, cmd, original)
        }
        None => {
            // no per-call registry (a bare probe): diagnose on a fresh context all the same
            let host = crate::host_for_diagnosis();
            let pids: Vec<u32> = host.fusion_processes().iter().map(|p| p.pid).collect();
            let dlg = |r: Value| r;
            let ctx = Ctx { host: &*host, fusion_exe: host_paths_exe(), short_path: |p| crate::host().short_path(p), state_dir: crate::host().bridge_state_dir(), caller: fusion_core::ab::Caller::default(), pids, sidecar, dialogs: &dlg };
            crate::lifecycle::diagnose_addin_timeout(&ctx, cmd, original)
        }
    }
}
fn host_paths_exe() -> Option<std::path::PathBuf> { crate::host_for_diagnosis_paths() }

/// What a sentinel needs (set per pass-through call, read by call_addin's doc-switching path).
static SENTINEL_CTX: Mutex<Option<(std::path::PathBuf, fusion_core::ab::Caller, Vec<u32>)>> = Mutex::new(None);

/// The exe's own long-command marker (`_long_command`): a walk or search in flight.
static LONG_COMMAND: Mutex<Option<(String, Instant)>> = Mutex::new(None);

fn now_f() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

pub fn addin_status_probe(timeout: Duration) -> Option<Value> {
    let r = ureq::get("http://127.0.0.1:8774/status").timeout(timeout).call().ok().and_then(|r| r.into_json().ok());
    if r.is_some() { note_answered(); }
    r
}

// A loaded add-in can go completely silent: while a long native Fusion call holds Python's lock
// on the main thread, not even /status or /health is served. That silence is work, not an add-in
// Fusion never loaded, and restarting for it closed every open document of a busy Fusion twice on
// 2026-10-28 (a 93 s script in flight). Two facts tell the cases apart without the add-in's help.

/// Add-in requests this exe has sent and not yet seen answered: (id, command, sent). A request in
/// flight means the add-in took the connection, so it is loaded and working.
static IN_FLIGHT: Mutex<Vec<(u64, String, Instant)>> = Mutex::new(Vec::new());
static IN_FLIGHT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Holds one request's place in IN_FLIGHT until it is answered, fails or times out.
struct InFlight(u64);
impl InFlight {
    fn begin(cmd: &str) -> InFlight {
        let id = IN_FLIGHT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        IN_FLIGHT.lock().unwrap().push((id, cmd.to_string(), Instant::now()));
        InFlight(id)
    }
}
impl Drop for InFlight { fn drop(&mut self) { IN_FLIGHT.lock().unwrap().retain(|(id, _, _)| *id != self.0); } }

/// The oldest add-in request still in flight: (command, seconds since it was sent).
pub fn oldest_in_flight() -> Option<(String, f64)> {
    IN_FLIGHT.lock().unwrap().first().map(|(_, c, at)| (c.clone(), at.elapsed().as_secs_f64()))
}

/// The Fusion360.exe processes the add-in last answered from, and when. Also kept in
/// `<state>/addin-answered.json` so a respawned bridge still knows this Fusion's add-in loaded.
static ANSWERED: Mutex<Option<(Vec<u32>, f64)>> = Mutex::new(None);

fn answered_file() -> std::path::PathBuf { crate::host().bridge_state_dir().join("addin-answered.json") }

/// The add-in just answered: record it against the Fusion processes running now.
pub fn note_answered() {
    let pids: Vec<u32> = crate::host().fusion_processes().iter().map(|p| p.pid).collect();
    if pids.is_empty() { return; }
    let mut g = ANSWERED.lock().unwrap();
    let changed = g.as_ref().map(|(p, _)| *p != pids).unwrap_or(true);
    *g = Some((pids.clone(), now_f()));
    if changed { let _ = std::fs::write(answered_file(), json!({"pids": pids, "at": now_f()}).to_string()); }
}

/// Seconds since the add-in last answered, when it answered from one of these Fusion processes.
pub fn answered_in(pids: &[u32]) -> Option<f64> {
    let mut g = ANSWERED.lock().unwrap();
    if g.is_none() {
        *g = std::fs::read_to_string(answered_file()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| Some((v["pids"].as_array()?.iter().filter_map(|p| p.as_u64().map(|p| p as u32)).collect(), v["at"].as_f64()?)));
    }
    g.as_ref().filter(|(seen, _)| seen.iter().any(|p| pids.contains(p))).map(|(_, at)| (now_f() - at).max(0.0))
}

/// Nothing listens on the add-in's port: no add-in server in this Fusion (never loaded, or it died).
fn port_refused() -> bool {
    matches!(std::net::TcpStream::connect_timeout(&"127.0.0.1:8774".parse().unwrap(), Duration::from_secs(1)), Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused)
}

/// Why a silent add-in is loaded and working rather than missing, or None when nothing says so.
pub fn loaded_evidence_from(in_flight: Option<(String, f64)>, answered_secs_ago: Option<f64>, port_refused: bool) -> Option<Value> {
    if let Some((cmd, secs)) = in_flight {
        return Some(json!({"evidence": "request_in_flight", "busyCommand": cmd, "elapsedSeconds": (secs * 10.0).round() / 10.0}));
    }
    match answered_secs_ago {
        Some(ago) if !port_refused => Some(json!({"evidence": "answered_in_this_fusion", "lastAnswerSecondsAgo": ago.round()})),
        _ => None,
    }
}

/// The same from the live signals, for the Fusion processes running now.
pub fn loaded_evidence(pids: &[u32]) -> Option<Value> {
    if let Some(fl) = oldest_in_flight() { return loaded_evidence_from(Some(fl), None, false); }
    let ago = answered_in(pids)?;
    loaded_evidence_from(None, Some(ago), port_refused())
}

/// The bridge's own quick reads. With ~90 documents open a document_info takes ~5 s, longer than
/// the 3 s the post-verb document snapshot waits, so it is still running when the caller's next
/// verb lands, and that verb was refused as main_thread_busy (2026-10-28, a mechanical inspect in
/// the assembly campaign). Such a read ends in seconds: the gate waits it out instead.
const SHORT_READS: &[&str] = &["document_info", "get_app_state"];
const SHORT_READ_WAIT: Duration = Duration::from_secs(15);

/// Is the add-in busy with one of the bridge's own quick reads, which the gate should wait out?
fn busy_with_short_read(st: &Value) -> bool {
    st["busy"].as_bool() == Some(true) && SHORT_READS.contains(&st["busyCommand"].as_str().unwrap_or(""))
}

/// The two busy gates, in server.py's order: our own long command, then the add-in's /status.
fn busy_gate(cmd: &str) -> Option<Value> {
    if LONG_RUNNING.contains(&cmd) || BUSY_EXEMPT.contains(&cmd) { return None; }
    let busy = LONG_COMMAND.lock().unwrap().clone();
    // the palette (activity_banner) is advisory and never blocks a real verb: a beat's
    // show_2d_board was refused with "activity_banner running for 0.1s" (ConfRoomROG, 2026-09-05)
    if let Some((bc, at)) = busy.filter(|(bc, _)| bc != "activity_banner") {
        let st = addin_status_probe(Duration::from_secs(3));
        let progress = st.as_ref().and_then(|s| s.get("walkProgress").cloned()).filter(|p| !p.is_null());
        let elapsed = (at.elapsed().as_secs_f64() * 10.0).round() / 10.0;
        let pct = progress.as_ref().and_then(|p| { let v = p["foldersVisited"].as_f64()?; let q = p["queueSize"].as_f64()?; let t = v + q; if v > 0.0 && t > 0.0 { Some((100.0 * v / t).round() as i64) } else { None } });
        return Some(json!({"success": false,
            "error": format!("Fusion main thread busy — {bc} has been running for {elapsed}s. Your command '{cmd}' cannot execute until it finishes."),
            "errorCode": "main_thread_busy", "busyCommand": bc, "elapsedSeconds": elapsed, "progress": progress,
            "_hint": format!("A cloud search ({bc}) is in progress{}, running for {elapsed}s. Do NOT retry add-in commands (get_app_state, document_info, etc.) — they will all be rejected until the search finishes. Commands that still work right now: fusion_window_info, fusion_screenshot_fusion, fusion_close_window. Wait for the search to complete, then retry.", pct.map(|p| format!(" (~{p}% done)")).unwrap_or_default())}));
    }
    if let Some(mut st) = addin_status_probe(Duration::from_secs(3)) {
        let deadline = Instant::now() + SHORT_READ_WAIT;
        while busy_with_short_read(&st) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(250));
            match addin_status_probe(Duration::from_secs(3)) { Some(next) => st = next, None => break }
        }
        // the palette (activity_banner) repaints in a fraction of a second and is advisory: a verb
        // that lands while it paints waits its turn in the add-in instead of being refused
        // a sub-second probe from another caller (the dashboard polls get_app_state every half
        // second while a tour runs) is not "busy": the add-in queues the next command behind it.
        // Only a LONG command in flight (a cloud walk, a search, an import, or anything past 2 s)
        // earns a refusal. ConfRoomROG 2026-09-05: a 0.1 s get_app_state cost the 2D board beat.
        let busy_cmd = st["busyCommand"].as_str().unwrap_or("").to_string();
        let busy_for = st.get("elapsedSeconds").and_then(|e| e.as_f64()).unwrap_or(0.0);
        let long_one = LONG_RUNNING.contains(&busy_cmd.as_str()) || busy_for >= 2.0 || st.get("walkProgress").map(|w| !w.is_null()).unwrap_or(false);
        if st["busy"].as_bool() == Some(true) && busy_cmd != "activity_banner" && long_one {
            let elapsed = st.get("elapsedSeconds").cloned().unwrap_or(json!(0));
            let bc = busy_cmd.clone();
            let mut resp = json!({"success": false,
                "error": format!("Fusion main thread busy — {bc} running for {elapsed}s (from another bridge/session). Your command '{cmd}' cannot execute until it finishes."),
                "errorCode": "main_thread_busy", "busyCommand": bc, "elapsedSeconds": elapsed,
                "_hint": format!("A long-running command ({bc}) is in progress from another session. Do NOT retry add-in commands — they will all be rejected until it finishes. Do NOT press Escape — the add-in is working, not stuck on a dialog. Commands that still work: fusion_window_info, fusion_screenshot_fusion, fusion_close_window.")});
            if let Some(w) = st.get("walkProgress").filter(|w| !w.is_null()) { resp["progress"] = w.clone(); }
            return Some(resp);
        }
    }
    None
}

/// `_API_ERROR_PATTERNS`: a raw Fusion API exception gets a stable errorCode and remedy.
fn classify_api_error(mut r: Value) -> Value {
    if !r.is_object() || r["success"].as_bool() == Some(true) || !r["errorCode"].is_null() { return r; }
    let tb = r["data"]["traceback"].as_str().unwrap_or("").to_string();
    let blob = format!("{} {} {}", r["error"].as_str().unwrap_or(""), r["output"].as_str().unwrap_or(""), tb).to_lowercase();
    let pats: [(&str, &str, &str); 3] = [
        ("part design documents can only contain one component", "part_template_single_component", "This document came from Fusion's PART template, which permits exactly ONE component, so occurrences cannot be added. Documents created via documents.add(FusionDesignDocumentType) ARE assembly-capable. Either model with BODIES inside the single component, or create a new document through the API and build there. Fusion's API and UI defaults differ here, which is why this is surprising."),
        ("root component name cannot be changed", "root_rename_unsupported", "The root component always takes the DOCUMENT's name and cannot be renamed directly. Rename the document on save instead, or rename a sub-occurrence."),
        ("refers to a deleted object", "stale_api_handle", "A handle you are holding points at an object that was closed or deleted. Re-fetch it from the CURRENT app.activeDocument / activeProduct instead of reusing one from an earlier call."),
    ];
    for (needle, code, hint) in pats { if blob.contains(needle) { r["errorCode"] = json!(code); r["_hint"] = json!(hint); break; } }
    r
}

/// `_proxy_to_addin` minus the save-prompt sentinel (window-raising verbs are not taken here).
/// A bare add-in hop (no etiquette, no gate, no diagnosis): banners and probes.
pub fn addin_http(cmd: &str, args: &Value, timeout: u64) -> Value {
    let body = json!({"command": addin_name(cmd), "args": args});
    let flight = InFlight::begin(cmd);
    let sent = ureq::post("http://127.0.0.1:8774/command").timeout(Duration::from_secs(timeout)).set("Content-Type", "application/json").send_string(&body.to_string());
    drop(flight);
    if matches!(sent, Ok(_) | Err(ureq::Error::Status(..))) { note_answered(); }
    match sent {
        Ok(r) => r.into_json::<Value>().unwrap_or(json!({"success": false, "error": "Add-in request failed: non-JSON answer"})),
        Err(ureq::Error::Status(_, r)) => r.into_json::<Value>().unwrap_or(json!({"success": false, "error": "Add-in request failed: non-JSON answer"})),
        Err(e) => json!({"success": false, "error": format!("Add-in request failed: {e}")}),
    }
}
fn call_addin(cmd: &str, args: &Value, timeout: u64, sidecar: &dyn Fn(&str, Value, u64) -> Value) -> Value {
    let mut args = args.clone();
    if cmd == "assembly_bom" && args.get("treatAsUnit").map(|v| v.is_null() || v.as_array().map(|a| a.is_empty()).unwrap_or(false) || v == &json!("")).unwrap_or(true) {
        args["treatAsUnit"] = json!(ASSEMBLY_BOM_UNIT_DEFAULTS);
    }
    let body = json!({"command": addin_name(cmd), "args": args});
    let sentinel = if DOC_SWITCHING.contains(&cmd) { SENTINEL_CTX.lock().unwrap().clone().map(|(sd, ca, pids)| crate::etiquette::Sentinel::start(sd, ca, pids, timeout as f64 + 10.0)) } else { None };
    let flight = InFlight::begin(cmd);
    let sent = ureq::post("http://127.0.0.1:8774/command").timeout(Duration::from_secs(timeout)).set("Content-Type", "application/json").send_string(&body.to_string());
    drop(flight);
    if matches!(sent, Ok(_) | Err(ureq::Error::Status(..))) { note_answered(); }
    if let Some(s) = &sentinel { s.soft_stop(12.0); }
    let result: Value = match sent {
        Ok(resp) => resp.into_json::<Value>().unwrap_or(json!({"success": false, "error": "Add-in request failed: non-JSON answer"})),
        Err(ureq::Error::Status(_, resp)) => resp.into_json::<Value>().unwrap_or(json!({"success": false, "error": "Add-in request failed: non-JSON answer"})),
        Err(e) => {
            let s = e.to_string();
            let sl = s.to_lowercase();
            if sl.contains("connection refused") || sl.contains("no connection") || sl.contains("actively refused") {
                return json!({"success": false, "error": "Fusion 360 AdomBridge add-in not running. Install it with: python plugins/fusion360/install_addin.py, then restart Fusion 360."});
            }
            if sl.contains("timed out") || sl.contains("timeout") {
                return diagnose(cmd, &json!({"error": s}), sidecar);
            }
            return json!({"success": false, "error": format!("Add-in request failed: {s}")});
        }
    };
    if result["success"].as_bool() != Some(true) && result["error"].as_str().unwrap_or("").to_lowercase().contains("timed out") {
        return diagnose(cmd, &result, sidecar);
    }
    let mut result = result;
    if result["success"].as_bool() != Some(true) {
        let err = result["error"].as_str().unwrap_or("").to_lowercase();
        if ["unknown command", "no such command", "not supported", "unsupported command"].iter().any(|s| err.contains(s)) {
            let ver = addin_status_probe(Duration::from_secs(1)).and_then(|s| s["version"].as_str().map(str::to_string));
            let stale = crate::small_verbs::addin_staleness(ver.as_deref());
            if stale["addinStale"].as_bool() == Some(true) {
                result["errorCode"] = json!("addin_stale"); result["_hint"] = stale["_staleHint"].clone();
                for k in ["addinVersion", "expectedAddinVersion", "addinStale"] { result[k] = stale[k].clone(); }
            }
        }
    }
    classify_api_error(result)
}

/// Everything the pass-through needs from the shell.
pub struct Ctx<'a> {
    pub host: &'a dyn fusion_core::HostProcess,
    /// `HostPaths::fusion_executable()` at call time (None = not installed)
    pub fusion_exe: Option<std::path::PathBuf>,
    /// `HostPaths::short_path`. Unused since 1.10.11: paths with spaces are STAGED into a
    /// space-free folder (`fusionpath`) instead of shortened, because Fusion titles a document
    /// after the path it is given and the 8.3 alias ("0603WA~1") broke every name check.
    pub short_path: fn(&str) -> String,
    pub state_dir: std::path::PathBuf,
    pub caller: fusion_core::ab::Caller,
    pub pids: Vec<u32>,
    /// the sidecar executors (`chain_addin_gate`, `chain_diagnose_addin_timeout`)
    pub sidecar: &'a dyn Fn(&str, Value, u64) -> Value,
    /// the post-op dialog array for a mutating verb
    pub dialogs: &'a dyn Fn(Value) -> Value,
}

impl<'a> Ctx<'a> { #[deprecated(note = "stage with crate::fusionpath::for_fusion; never hand Fusion an 8.3 alias")] #[allow(dead_code)] pub fn host_paths_short(&self, p: &str) -> String { (self.short_path)(p) } }

/// The whole pass-through for one verb.
pub fn pass_through(cmd: &str, args: &Value, ctx: &Ctx) -> Value {
    if matches!(cmd, "open_board" | "open_schematic") {
        return crate::orchestrations::open_file_guarded(ctx, cmd, args, &|v: &str, a: Value| pass_through_checked(v, &a, ctx));
    }
    if cmd == "export_gerbers" && !matches!(args["layers"].as_str(), Some("2") | Some("4")) {
        return export_gerbers_counted(args, ctx);
    }
    if cmd == "run_modeling_script" {
        let watch = crate::small_verbs::watch_for_open_notice(&ctx.caller, &ctx.pids);
        return crate::small_verbs::finish_notice_watch(watch, pass_through_checked(cmd, args, ctx));
    }
    pass_through_checked(cmd, args, ctx)
}

/// Gerbers for as many copper layers as the board has. The add-in's detection counts layers carrying traces, so inner
/// planes that are only pours read as a 2-layer board, and the export dropped them and said success (#142). The board's
/// own stack says how many it has. When it can't be read, the add-in's detection still runs, and the reply says so.
fn export_gerbers_counted(args: &Value, ctx: &Ctx) -> Value {
    let pass = |v: &str, a: Value| pass_through_checked(v, &a, ctx);
    let doc = pass("get_app_state", json!({}))["data"]["activeDocument"].clone();
    let layers = crate::routing::call("copper_layers", &json!({"expectDocument": doc}), &pass);
    let count = gerber_layer_count(&layers);
    let mut a = args.clone();
    if !a.is_object() { a = json!({}); }
    match count {
        Some(n @ (2 | 4)) => { a["layers"] = json!(n.to_string()); }
        Some(n) => return json!({"success": false, "errorCode": "unsupported_layer_count", "layerCount": n, "layerSource": "board",
            "error": format!("This board has {n} copper layers and the bridge's Gerber jobs cover 2 and 4, so nothing was exported rather than leave layers out."),
            "_hint": "Export the rest with fusion_export_manufacturing, or Fusion's own CAM processor."}),
        None => {}
    }
    let mut r = pass_through_checked("export_gerbers", &a, ctx);
    if r.is_object() { r["layerSource"] = json!(if count.is_some() { "board" } else { "addin_detection" }); }
    if let (Some(n), Some(d)) = (count, r["data"].as_object_mut()) { d.insert("detectionMethod".into(), json!("board_stack")); d.insert("boardCopperLayers".into(), json!(n)); }
    r
}

fn gerber_layer_count(layers: &Value) -> Option<usize> {
    if layers["success"].as_bool() == Some(false) { return None; }
    layers["copperLayers"].as_array().or(layers["layers"].as_array()).map(|a| a.len()).filter(|n| *n >= 2)
}

fn pass_through_checked(cmd: &str, args: &Value, ctx: &Ctx) -> Value {
    let r = pass_through_inner(cmd, args, ctx);
    // Fusion 2706 dropped the Electron.boardInfo text command (live 2026-10-28): the same answer
    // is built from the board's own export, so board_info and everything built on it keeps working
    if cmd == "board_info" && r["success"].as_bool() == Some(false) && r["error"].as_str().map(|e| e.contains("no command Electron.boardInfo")).unwrap_or(false) {
        let c = crate::routing::call("board_info_compat", &json!({}), &|v: &str, a: Value| pass_through_inner(v, &a, ctx));
        if c["success"].as_bool() == Some(true) { return c; }
        let mut out = r; out["compatError"] = c["error"].clone(); return out;
    }
    if cmd == "export_cpl" && r["success"].as_bool() == Some(false) && r["error"].as_str().map(|e| e.contains("no command Electron.boardInfo")).unwrap_or(false) {
        return cpl_from_board_info(args, &pass_through("board_info", &json!({}), ctx));
    }
    r
}

/// fusion_export_cpl from the board_info answer (the compat one on Fusion 2706): the add-in's own
/// CSV (Designator, Mid X, Mid Y, Layer, Rotation), written here because its path read boardInfo.
pub fn cpl_from_board_info(args: &Value, bi: &Value) -> Value {
    if bi["success"].as_bool() != Some(true) { return bi.clone(); }
    let out = args["outputPath"].as_str().unwrap_or("C:/tmp/adom-cpl.csv").replace('\\', "/");
    let side = args["side"].as_str().unwrap_or("all").to_lowercase();
    let mut rows: Vec<(String, String, String, String, f64)> = bi["data"]["summary"]["components"].as_array().cloned().unwrap_or_default().iter().map(|c| {
        let rot = c["rotation"].as_str().unwrap_or("R0");
        let s = if rot.starts_with('M') { "Bottom" } else { "Top" };
        let deg = rot.replace(['M', 'R', 'S'], "").parse::<f64>().unwrap_or(0.0);
        (c["name"].as_str().unwrap_or("").to_string(), c["x"].as_str().unwrap_or("0").to_string(), c["y"].as_str().unwrap_or("0").to_string(), s.to_string(), deg)
    }).filter(|r| side == "all" || r.3.eq_ignore_ascii_case(&side)).collect();
    if rows.is_empty() { return json!({"success": false, "error": if side == "all" { "No components found in board. Is a populated .brd file open?".to_string() } else { format!("No {side}-side components found.") }}); }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let mut csv = String::from("Designator,Mid X,Mid Y,Layer,Rotation\r\n");
    for (n, x, y, s, d) in &rows { csv.push_str(&format!("{n},{x},{y},{s},{d:?}\r\n")); }
    if let Some(dir) = std::path::Path::new(&out).parent() { let _ = std::fs::create_dir_all(dir); }
    if let Err(e) = std::fs::write(&out, csv) { return json!({"success": false, "error": format!("could not write {out}: {e}")}); }
    let top = rows.iter().filter(|r| r.3 == "Top").count();
    json!({"success": true, "output": format!("CPL exported: {} placements ({top} top, {} bottom) -> {out}", rows.len(), rows.len() - top),
           "data": {"outputPath": out, "totalPlacements": rows.len(), "topCount": top, "bottomCount": rows.len() - top, "sideFilter": side, "source": "board export (Electron.boardInfo is gone in this Fusion)"}})
}

fn pass_through_inner(cmd: &str, args: &Value, ctx: &Ctx) -> Value {
    let spoke = crate::palette::for_verb(cmd, args);
    if let Some((title, detail)) = &spoke { crate::palette::say(title, detail, None, crate::palette::eta_for_verb(&ctx.state_dir, cmd)); }
    let r = pass_through_verb(cmd, args, ctx);
    if spoke.is_some() { crate::palette::verb_finished(); }
    r
}

fn pass_through_verb(cmd: &str, args: &Value, ctx: &Ctx) -> Value {
    let sidecar = ctx.sidecar;
    let dialogs = ctx.dialogs;
    let t0 = Instant::now();
    let since = now_f();
    let window_raising = WINDOW_RAISING.contains(&cmd);
    *SENTINEL_CTX.lock().unwrap() = Some((ctx.state_dir.clone(), ctx.caller.clone(), ctx.pids.clone()));
    // a direct add-in hop for the etiquette's own probes (document_info, get_app_state)
    let direct = |verb: &str, a: Value, t: u64| call_addin(verb, &a, t, sidecar);
    let pre = crate::etiquette::pre(ctx.host, &ctx.state_dir, &ctx.caller, cmd, args, &ctx.pids, &direct);
    let t_pre = t0.elapsed().as_secs_f64();
    let t_call_done: std::cell::Cell<f64> = std::cell::Cell::new(0.0);
    let args_owned: Value = pre.args.clone();
    let args = &args_owned;
    let ran_bg = pre.ran_backgrounded;
    let res = (|| {
        if let Some(r) = &pre.refusal { return r.clone(); }
        if let Some(refusal) = crate::launch::gate(ctx, cmd, args) { return refusal; }
        if let Some(b) = busy_gate(cmd) { return b; }
        // Document guard (server.py issue #289, lost in the port until 1.10.12): a verb carrying
        // `expectDocument` runs only when that document is the active one. A Fusion tab switch
        // silently retargets app.activeDocument, so a write could land in the wrong file. Fails
        // OPEN when the active document cannot be read; blocks only a confirmed mismatch.
        if let Some(expect) = args["expectDocument"].as_str().filter(|s| !s.trim().is_empty()) {
            let st = call_addin("get_app_state", &json!({}), 8, sidecar);
            let actual = st["data"]["activeDocument"].as_str().or(st["activeDocument"].as_str()).map(|s| s.trim_end_matches('*').trim().to_string()).unwrap_or_default();
            if !actual.is_empty() && !actual.eq_ignore_ascii_case(expect.trim()) {
                return json!({"success": false, "errorCode": "wrong_document", "expected": expect, "actual": actual,
                    "error": format!("Active document is '{actual}', expected '{expect}'."),
                    "_hint": "The active document changed: a Fusion tab switch retargets the active document. Re-activate the intended document (fusion_activate_document), or re-issue with expectDocument set to the current name (a rename mid-session also changes it)."});
            }
        }
        // close_document {save:false} on a changed cloud document: Fusion asks to save even with
        // saveChanges=False, so the add-in used to save a version first, silently (it saved
        // BQ25792 v228 for a caller who asked to discard, 2026-10-28). Saving needs save:true or
        // saveToClose:true now; otherwise the close is refused and nothing is written.
        if cmd == "close_document" && args["save"].as_bool() != Some(true) && args["saveToClose"].as_bool() != Some(true) {
            let name = match args["name"].as_str() { Some(n) => serde_json::to_string(n).unwrap_or("None".into()), None => "None".into() };
            let probe = format!("import adsk.core\napp=adsk.core.Application.get()\nn={name}\nd=None\nfor i in range(app.documents.count):\n    x=app.documents.item(i)\n    if (n is None and x==app.activeDocument) or x.name==n:\n        d=x; break\nresult={{'found': d is not None, 'cloud': bool(d is not None and d.dataFile is not None), 'modified': bool(d is not None and d.isModified), 'name': d.name if d else None}}\n");
            let pr = call_addin("run_modeling_script", &json!({"script": probe}), 15, sidecar);
            let r = &pr["data"]["result"];
            if r["cloud"].as_bool() == Some(true) && r["modified"].as_bool() == Some(true) {
                return json!({"success": false, "errorCode": "unsaved_cloud_document", "document": r["name"],
                    "error": format!("'{}' is a saved cloud document with unsaved changes. Fusion asks to save it even when told not to, so closing it now would mean saving a new version. Nothing was closed or saved.", r["name"].as_str().unwrap_or("")),
                    "_hint": "Keep it open, or pass save:true to save a version and close (saveToClose:true does the same with the bridge's own version comment). Changes you do not want are best undone before saving."});
            }
        }
        // raw close_document gate: never close over an in-flight Hub upload (John, 2026-08-21)
        if cmd == "close_document" || cmd == "close_all_documents" {
            let deadline = Instant::now() + Duration::from_secs(180);
            let mut drained = false;
            while Instant::now() < deadline {
                let jr = call_addin("run_modeling_script", &json!({"script": "import adsk.core\nresult={'jobs': adsk.core.Application.get().hasActiveJobs}"}), 10, sidecar);
                if jr["data"]["result"]["jobs"].as_bool() != Some(true) { drained = true; break; }
                std::thread::sleep(Duration::from_secs(5));
            }
            if !drained {
                return json!({"success": false, "errorCode": "upload_in_flight_close_deferred",
                    "error": "Fusion is still uploading packages to the Hub - closing now would raise a lose-changes confirm and risk destroying the upload. The close was NOT issued.",
                    "_hint": "Re-call fusion_close_document in ~30s; the bridge closes the moment app.hasActiveJobs drains. Nothing was lost and no dialog was raised."});
            }
        }
        let timeout = timeout_secs(cmd);
        if LONG_RUNNING.contains(&cmd) {
            *LONG_COMMAND.lock().unwrap() = Some((cmd.to_string(), Instant::now()));
            let r = call_addin(cmd, args, timeout, sidecar);
            *LONG_COMMAND.lock().unwrap() = None;
            return r;
        }
        let r = call_addin(cmd, args, timeout, sidecar);
        t_call_done.set(t0.elapsed().as_secs_f64());
        if MUTATING.contains(&cmd) { return dialogs(r); }
        r
    })();
    let t_body = t0.elapsed().as_secs_f64();
    let mut res = res;
    if ran_bg && res.is_object() {
        if res.get("windowEtiquette").is_none() { res["windowEtiquette"] = json!("background"); }
        let note = "Window opened/changed in the BACKGROUND - the user's focus was NOT touched; do NOT foreground anything to compensate. Render results via fusion_state. If the USER asks to watch, re-call with {\"foreground\": true, \"foregroundReason\": \"<why, shown to the user as a 3s caption>\"}.";
        let h = res["_hint"].as_str().map(str::to_string);
        res["_hint"] = json!(match h { Some(h) if !h.contains(note) => format!("{h} {note}"), Some(h) => h, None => note.to_string() });
    }
    let _ = window_raising;
    let mut res = crate::etiquette::post(&ctx.state_dir, &ctx.caller, cmd, args, res, &pre, since, &ctx.pids, &direct);
    // where a slow verb spent its time: the etiquette envelope around the add-in call is
    // ab traffic (window facts, dialog sweeps), and it must never dwarf the work itself
    let total = t0.elapsed().as_secs_f64();
    if total >= 2.0 && res.is_object() {
        let call_end = if t_call_done.get() > 0.0 { t_call_done.get() } else { t_body };
        let r2 = |x: f64| (x * 100.0).round() / 100.0;
        res["_timing"] = json!({"totalSec": r2(total), "preSec": r2(t_pre), "addinSec": r2(call_end - t_pre), "dialogsSec": r2(t_body - call_end), "postSec": r2(total - t_body),
            "scope": "one exe pass-through of this verb only: excludes the relay hop, ab's queue, and any earlier pass-through a composite verb (routing, tour) made before this one, so the caller's wall clock can be longer"});
        eprintln!("[fusion-bridge] {cmd} took {:.1}s: pre {:.1}s, add-in {:.1}s, dialogs {:.1}s, post {:.1}s", total, t_pre, call_end - t_pre, t_body - call_end, total - t_body);
    }
    res
}

pub fn long_command_active() -> Option<Value> {
    LONG_COMMAND.lock().unwrap().clone().map(|(c, at)| json!({"command": c, "startedAt": now_f() - at.elapsed().as_secs_f64()}))
}

#[cfg(test)]
mod gerber_tests {
    use super::gerber_layer_count;
    use serde_json::json;
    #[test] fn the_count_comes_from_the_boards_stack() {
        assert_eq!(gerber_layer_count(&json!({"success": true, "copperLayers": ["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"]})), Some(4));
        assert_eq!(gerber_layer_count(&json!({"layers": [{}, {}]})), Some(2));
        assert_eq!(gerber_layer_count(&json!({"success": false, "copperLayers": ["F.Cu", "B.Cu"]})), None);
        assert_eq!(gerber_layer_count(&json!({})), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn api_error_classifier_matches_python_table() {
        let r = classify_api_error(json!({"success": false, "error": "3 : Part Design documents can only contain one component"}));
        assert_eq!(r["errorCode"], "part_template_single_component");
        let r = classify_api_error(json!({"success": false, "error": "x", "data": {"traceback": "RuntimeError: refers to a deleted object"}}));
        assert_eq!(r["errorCode"], "stale_api_handle");
        // an errorCode already present is never overwritten; a success is untouched
        let r = classify_api_error(json!({"success": false, "errorCode": "mine", "error": "refers to a deleted object"}));
        assert_eq!(r["errorCode"], "mine");
        let r = classify_api_error(json!({"success": true, "error": "refers to a deleted object"}));
        assert!(r.get("errorCode").is_none());
    }
    #[test]
    fn a_silent_addin_is_loaded_only_on_evidence() {
        // 2026-10-28: a 93 s script was in flight when the gate restarted Fusion
        let ev = loaded_evidence_from(Some(("run_modeling_script".into(), 31.04)), None, true).unwrap();
        assert_eq!(ev["evidence"], "request_in_flight");
        assert_eq!(ev["busyCommand"], "run_modeling_script");
        assert_eq!(ev["elapsedSeconds"], 31.0);
        // it answered earlier in this Fusion and its port still takes connections: busy
        assert_eq!(loaded_evidence_from(None, Some(42.4), false).unwrap()["evidence"], "answered_in_this_fusion");
        // its port refuses: the server is gone, a restart is the remedy
        assert!(loaded_evidence_from(None, Some(42.4), true).is_none());
        // never answered in this Fusion (installed while it ran): the restart path stays open
        assert!(loaded_evidence_from(None, None, false).is_none());
    }
    #[test]
    fn the_gate_waits_out_the_bridges_own_quick_reads() {
        // 2026-10-28: the post-verb document_info ran ~5 s with 94 documents open and the next
        // verb was refused; a quick read is waited out, real work still is not
        assert!(busy_with_short_read(&json!({"busy": true, "busyCommand": "document_info", "elapsedSeconds": 4.9})));
        assert!(busy_with_short_read(&json!({"busy": true, "busyCommand": "get_app_state", "elapsedSeconds": 2.5})));
        assert!(!busy_with_short_read(&json!({"busy": true, "busyCommand": "run_modeling_script", "elapsedSeconds": 4.9})));
        assert!(!busy_with_short_read(&json!({"busy": false, "busyCommand": "document_info"})));
    }
    #[test]
    fn requests_leave_the_in_flight_list_however_they_end() {
        assert!(IN_FLIGHT.lock().unwrap().iter().all(|(_, c, _)| c != "slow_one"));
        let a = InFlight::begin("slow_one");
        let b = InFlight::begin("second");
        assert!(IN_FLIGHT.lock().unwrap().iter().any(|(_, c, _)| c == "slow_one"));
        drop(a);
        assert!(IN_FLIGHT.lock().unwrap().iter().all(|(_, c, _)| c != "slow_one"));
        drop(b);
        assert!(IN_FLIGHT.lock().unwrap().iter().all(|(_, c, _)| c != "second"));
    }
    #[test]
    fn timeouts_and_ownership() {
        assert_eq!(timeout_secs("walk_cloud_tree"), 600);
        assert_eq!(timeout_secs("get_app_state"), 30);
        assert_eq!(addin_name("import_step"), "import_file");
        assert!(owns("get_app_state") && owns("open_by_urn") && !owns("open_lbr") && !owns("readiness"));
        assert!(WINDOW_RAISING.contains(&"save_to_cloud") && !WINDOW_RAISING.contains(&"document_info"));
    }
}

//! The bridge exe (spawn.kind: exe) - Phase 1, the plumbing shell.
//!
//! What it owns today: the HTTP surface ab talks to (GET /status + /health, POST /command
//! with X-Adom-Caller-* identity), the loopback ab client, the frozen verb catalog, and a
//! PROXY to the Python bridge running as a sidecar child for every verb not yet ported.
//! Users feel a faster, lighter bridge; nothing behavioural changes. Each later phase moves
//! verbs from the proxy table into native handlers until the sidecar is empty.
//!
//! Contract (proved by the Python bridge in production):
//!   - bind ADOM_BIND_HOST (always 127.0.0.1) on the port ab passes as --port; never 0.0.0.0
//!   - /status must answer 2xx whenever this process is up, with {status, busy, led, summary,
//!     tooltip, bridgeVersion, ...}: ab paints the chip from it verbatim
//!   - /command body {command, args}; the response is the verb's JSON as-is

mod chains;
mod writers;
mod body;
mod spice;
mod order;
mod pdf;
mod small_verbs;
mod addin;
mod mech;
mod mech_store;
mod fusion_prefs;
mod timings;
mod etiquette;
mod launch;
mod lifecycle;
mod readiness_exec;
mod seat;
mod orchestrations;
mod mint;
mod prove;
mod services;
mod maintenance;
mod testproject;
mod boardgen;
mod libraries;
mod partimport;
mod unsupported;
mod stdshapes;
mod replicate;
mod appverbs;
mod livefeed;
mod chainexec;
mod epg;
mod mcp;
mod managed;
mod signin;
mod aps_signin;
mod demo;
mod addin_install;
mod janitor;
mod electronics;
mod routing;
mod sch;
mod docs;
mod checks;
mod sheet;
mod bodies;
mod display;
mod savedoc;
mod fusionpath;
mod tour;
mod palette;
use fusion_core::ab::Caller;
use fusion_core::{HostPaths, HostProcess};
use serde_json::{json, Value};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Response, Server};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// EDA Bridge Standard names (adom/eda-bridge-standard verbs/standard.json) for jobs this bridge
/// already does under its own verb: the standard name is answered by that verb, reply marked aliasOf.
const STANDARD_ALIASES: &[(&str, &str)] = &[
    ("launch", "fusion_start"), ("export_gerber", "fusion_export_gerbers"), ("dismiss_dialogs", "fusion_dismiss_blocking_dialogs"),
    ("install", "fusion_install_fusion"),
    ("status", "fusion_readiness"), ("save", "fusion_save_document"), ("run_script", "fusion_run_modeling_script"),
    ("run_command", "fusion_execute_text_command"), ("list_commands", "fusion_list_text_commands"), ("dialog_log", "fusion_action_ledger"),
    ("signin_status", "fusion_account"), ("license_status", "fusion_account"), ("open_document", "fusion_open_design"),
    ("show_3d_chip", "fusion_show_3d_package"), ("edit_pours", "fusion_change_pours"),
];

/// The standard's verbs this bridge serves through its own, with the arguments renamed:
/// The standard's generate_body dimensions as Fusion's package generator (EPG Scripts3d) reads them.
/// `pins` is the part's total; each generator counts it its own way (read from its script, Fusion
/// 2706): qfn per side (DPins, EPins); qfp and plcc the two opposite sides together (it halves
/// them); soic, soj, son, dip and the oscillators the total. An exposed pad (D2/E2) turns on the
/// generator's thermal pad, which qfn names D1/E1. bga and headers take rows and columns, as given.
fn epg_params(family: &str, mut p: serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    let qfn = ["qfn", "vqfn", "wqfn"].iter().any(|q| family.starts_with(q));
    let quad_pairs = ["qfp", "lqfp", "tqfp", "pqfp", "plcc"].iter().any(|q| family.starts_with(q));
    let grid = family.starts_with("bga") || family.starts_with("header");
    if let Some(n) = p.remove("pins").and_then(|v| v.as_u64()) {
        if !p.contains_key("DPins") && !grid {
            if qfn { p.insert("DPins".into(), json!(n / 4)); p.insert("EPins".into(), json!(n / 4)); }
            else if quad_pairs { p.insert("DPins".into(), json!(n / 2)); p.insert("EPins".into(), json!(n / 2)); }
            else { p.insert("DPins".into(), json!(n)); }
        }
    }
    if p.contains_key("D2") || p.contains_key("E2") {
        if qfn { for (from, to) in [("D2", "D1"), ("E2", "E1")] { if let Some(v) = p.remove(from) { p.insert(to.into(), v); } } }
        p.entry("thermal").or_insert(json!(1));
    }
    p
}

/// generate_body -> fusion_generate_package (family -> type, dimensionsMm -> params with pins as
/// the generator's per-side DPins/EPins, marking -> etch, markingStyle -> etchStyle, outputPath ->
/// outputStep). (library_views is docs.rs: it opens the library first.)
fn standard_adapter(verb: &str, mut parsed: Value) -> (String, Value) {
    let bare = verb.strip_prefix("fusion_").unwrap_or(verb);
    let a = parsed["args"].clone();
    // Altium's own machinery: one refusal verb answers them all, with what Fusion does instead
    if unsupported::note(bare).is_some() { parsed["args"] = json!({"verb": bare}); parsed["command"] = json!("fusion_not_supported"); return ("fusion_not_supported".into(), parsed); }
    match bare {
        // read, or with set change: fusion_get_preferences and fusion_set_preference as one verb
        "preferences" => {
            match a["set"].as_object().filter(|m| !m.is_empty()).cloned() {
                Some(m) => { parsed["args"] = Value::Object(m); parsed["command"] = json!("fusion_set_preference"); ("fusion_set_preference".to_string(), parsed) }
                // key is applied to the reply, in the dispatch's post step (see preferences_key)
                None => { parsed["args"] = json!({"key": a["key"]}); parsed["command"] = json!("fusion_get_preferences"); ("fusion_get_preferences".to_string(), parsed) }
            }
        }
        // state's screenshots is the standard's name for this bridge's screenshot flag
        "state" if !a["screenshots"].is_null() => { parsed["args"]["screenshot"] = a["screenshots"].clone(); (verb.to_string(), parsed) }
        // the seat, by the standard's license actions: use_license takes it here, release gives it up, and
        // anything else (or nothing) reads the account and licence
        "license" => {
            let v = match a["action"].as_str().unwrap_or("") { "use_license" | "claim" => "fusion_claim_seat", "release" => "fusion_release_seat", _ => "fusion_account" };
            parsed["command"] = json!(v); (v.to_string(), parsed)
        }
        // the shared writer for the families it makes (no Fusion needed); Fusion's generator for the rest
        "generate_body" if body::engine_makes(&a) => ("fusion_generate_body_engine".into(), parsed),
        "generate_body" => {
            let family = a["family"].as_str().unwrap_or("").to_lowercase();
            let params = epg_params(&family, a["dimensionsMm"].as_object().cloned().unwrap_or_default());
            let mut na = json!({"type": family, "params": params});
            for (from, to) in [("marking", "etch"), ("markingStyle", "etchStyle"), ("outputPath", "outputStep")] { if !a[from].is_null() { na[to] = a[from].clone(); } }
            parsed["args"] = na; parsed["command"] = json!("fusion_generate_package");
            ("fusion_generate_package".into(), parsed)
        }
        "create_part_project" => {
            // the standard's part ("Lib:Name" or a deviceset) and libraryPath onto make_test_project;
            // Fusion names and places the new design itself, so projectName/outputDir do not apply
            let part = a["part"].as_str().unwrap_or("");
            let ds = part.rsplit(':').next().unwrap_or(part);
            let mut na = json!({"libraryPath": a["libraryPath"].clone()});
            if !ds.is_empty() { na["deviceset"] = json!(ds); }
            if !a["device"].is_null() { na["device"] = a["device"].clone(); }
            parsed["args"] = na; parsed["command"] = json!("fusion_make_test_project");
            ("fusion_make_test_project".into(), parsed)
        }
        _ => (verb.to_string(), parsed),
    }
}

fn standard_alias(asked: &str) -> Option<&'static str> {
    let bare = asked.strip_prefix("fusion_").unwrap_or(asked);
    STANDARD_ALIASES.iter().find(|(std_name, _)| *std_name == bare).map(|(_, own)| *own)
}

#[cfg(windows)]
fn host() -> Box<dyn HostBoth> { Box::new(fusion_os_win::WindowsHost) }
#[cfg(target_os = "macos")]
fn host() -> Box<dyn HostBoth> { Box::new(fusion_os_mac::MacHost) }
#[cfg(not(any(windows, target_os = "macos")))]
fn host() -> Box<dyn HostBoth> { Box::new(NoHost) }

trait HostBoth: HostProcess + HostPaths {}
/// A fresh host for code paths that cannot borrow the request's (the timeout diagnosis).
fn host_for_diagnosis() -> Box<dyn HostProcess> { let h: Box<dyn HostBoth> = host(); Box::new(HostShim(h)) }
fn host_for_diagnosis_paths() -> Option<std::path::PathBuf> { host().fusion_executable() }
struct HostShim(Box<dyn HostBoth>);
impl HostProcess for HostShim {
    fn fusion_processes(&self) -> Vec<fusion_core::host::FusionProcess> { self.0.fusion_processes() }
    fn launch_fusion(&self) -> Result<fusion_core::host::FusionProcess, fusion_core::host::HostError> { self.0.launch_fusion() }
    fn is_elevated(&self) -> bool { self.0.is_elevated() }
    fn image_running(&self, names: &[&str]) -> bool { self.0.image_running(names) }
    fn tie_child_lifetime(&self, pid: u32) -> bool { self.0.tie_child_lifetime(pid) }
    fn background_envelope(&self, seconds: f64, minimize: bool) { self.0.background_envelope(seconds, minimize) }
    fn user_watching(&self) -> bool { self.0.user_watching() }
    fn window_hung(&self, hwnd: i64) -> bool { self.0.window_hung(hwnd) }
    fn family_cpu_ticks(&self) -> u64 { self.0.family_cpu_ticks() }
}
impl<T: HostProcess + HostPaths> HostBoth for T {}

/// Linux is the CI/build host, never a Fusion host: a stub so the workspace builds here.
struct NoHost;
impl HostProcess for NoHost {
    fn fusion_processes(&self) -> Vec<fusion_core::host::FusionProcess> { vec![] }
    fn launch_fusion(&self) -> Result<fusion_core::host::FusionProcess, fusion_core::host::HostError> {
        Err(fusion_core::host::HostError::NotInstalled("this OS does not run Fusion".into()))
    }
    fn is_elevated(&self) -> bool { false }
}
impl HostPaths for NoHost {
    fn fusion_executable(&self) -> Option<std::path::PathBuf> { None }
    fn addin_dirs(&self) -> Vec<std::path::PathBuf> { vec![] }
    fn fusion_log_dir(&self) -> Option<std::path::PathBuf> { None }
    fn bridge_state_dir(&self) -> std::path::PathBuf { std::env::temp_dir() }
    fn scratch_dir(&self) -> std::path::PathBuf { std::env::temp_dir() }
    fn mechanical_state_dir(&self) -> std::path::PathBuf { std::env::temp_dir().join("adom-fusion-mechanical") }
}

/// The Python bridge as a child process, proxied until each verb is ported.
struct Sidecar {
    /// Held so the child is reaped with this process (dropping the handle would orphan it).
    #[allow(dead_code)]
    child: Option<Child>,
    port: u16,
    started: Instant,
}

impl Sidecar {
    /// ab provisions Python for kind:python bridges only; a kind:exe bridge that still needs
    /// the sidecar finds ab's portable copy itself (`~/.adom/adom-runtimes/python-*/`).
    fn python_exe() -> Option<std::path::PathBuf> {
        if let Ok(p) = std::env::var("ADOM_SIDECAR_PYTHON") { return Some(p.into()); }
        // ab knows which runtime it manages and which it quarantined: ask it (loopback verb
        // `runtimes`). The scan below is only for running without ab (a dev smoke test).
        if let Some(ab) = fusion_core::ab::Loopback::from_env() {
            use fusion_core::ab::AbClient;
            // ab 2.1.14+ (adom-bridge issue #134): {ensure:"python"} kicks the same no-UAC
            // provisioning a kind:python spawn gets and returns at once; poll until ready.
            // A first-time download can take minutes, so the wait is bounded, not brief.
            let t0 = Instant::now();
            let mut asked = false;
            while t0.elapsed() < Duration::from_secs(600) {
                let args = if asked { json!({}) } else { json!({"ensure": "python"}) };
                asked = true;
                match ab.call("runtimes", args, &Caller::default(), 15) {
                    Ok(v) => {
                        let py = &v["runtimes"]["python"];
                        if py["ready"].as_bool() == Some(true) || py["state"].as_str() == Some("ready") {
                            if let Some(exe) = py["exe"].as_str() { let pb = std::path::PathBuf::from(exe); if pb.exists() { return Some(pb); } }
                            break;
                        }
                        if py["state"].as_str().map(|s| s == "installing" || s == "downloading" || s == "provisioning").unwrap_or(false)
                            || v["ensureKicked"].as_bool() == Some(true) {
                            eprintln!("[fusion-bridge] waiting for ab to provision python ({} {}%)", py["phase"].as_str().unwrap_or("..."), py["pct"].as_u64().unwrap_or(0));
                            std::thread::sleep(Duration::from_secs(3));
                            continue;
                        }
                        break; // an ab without ensure (pre-2.1.14) answers the snapshot only
                    }
                    Err(_) => break,
                }
            }
        }
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).ok()?;
        let dir = std::path::Path::new(&home).join(".adom").join("adom-runtimes");
        let mut found: Vec<(String, std::path::PathBuf)> = Vec::new();
        for e in std::fs::read_dir(&dir).ok()?.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            // `python-3.12.13.broken-<stamp>` is a runtime ab quarantined (no stdlib): a
            // sidecar started from it dies with "No module named 'encodings'". Never pick it.
            if !n.starts_with("python-") || n.contains(".broken") { continue; }
            for cand in ["python.exe", "bin/python3", "python3"] {
                let p = e.path().join(cand);
                if p.exists() { found.push((n.clone(), p)); break; }
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found.pop().map(|f| f.1)
    }

    /// The sidecar runs under its OWN image name. ab's bundled-bridge collapse scans every
    /// loopback listener and adopts any `python.exe` whose /health body mentions Fusion as
    /// THE fusion bridge, re-pinning the route to it; that sent every verb straight to the
    /// sidecar and bypassed this exe for a whole session. python.exe is a tiny launcher that
    /// loads python3xx.dll from its own directory, so a copy named adom-fusion-sidecar.exe
    /// beside it runs identically and is invisible to that scan.
    fn sidecar_image(py: &std::path::Path) -> std::path::PathBuf {
        let Some(dir) = py.parent() else { return py.to_path_buf() };
        if !py.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case("python.exe")).unwrap_or(false) { return py.to_path_buf(); }
        let alias = dir.join("adom-fusion-sidecar.exe");
        let fresh = match (std::fs::metadata(&alias), std::fs::metadata(py)) { (Ok(a), Ok(b)) => a.len() == b.len(), _ => false };
        if fresh || std::fs::copy(py, &alias).is_ok() { alias } else { py.to_path_buf() }
    }

    fn spawn(bind: &str, log_dir: &std::path::Path) -> Option<Sidecar> {
        let py = Self::sidecar_image(&Self::python_exe()?);
        let here = std::env::current_exe().ok()?.parent()?.to_path_buf();
        // The sidecar's server.py sits beside this exe in the bridge dir during the port.
        let server_py = here.join("server.py");
        if !server_py.exists() { return None; }
        let port = pick_free_port();
        eprintln!("[fusion-bridge] sidecar python: {}", py.display());
        // The sidecar's own stdout/stderr land in <state>/sidecar.log so a startup failure is
        // readable (a null handle is how the first smoke test died silently).
        let _ = std::fs::create_dir_all(log_dir);
        let log = std::fs::OpenOptions::new().create(true).append(true).open(log_dir.join("sidecar.log")).ok();
        let (out, err): (Stdio, Stdio) = match log {
            Some(f) => (f.try_clone().map(Stdio::from).unwrap_or_else(|_| Stdio::null()), Stdio::from(f)),
            None => (Stdio::null(), Stdio::null()),
        };
        let child = Command::new(py)
            .arg(&server_py).arg("--port").arg(port.to_string())
            .env("ADOM_BIND_HOST", bind)
            .current_dir(&here)
            .stdin(Stdio::null()).stdout(out).stderr(err)
            .spawn().ok()?;
        Some(Sidecar { child: Some(child), port, started: Instant::now() })
    }

    fn url(&self, path: &str) -> String { format!("http://127.0.0.1:{}{}", self.port, path) }

    /// The sidecar's exit code if it has died (a dead sidecar must show in /status at once,
    /// not as a 45 s connection wait on the next verb).
    fn exited(&mut self) -> Option<i32> {
        self.child.as_mut().and_then(|c| c.try_wait().ok().flatten()).map(|st| st.code().unwrap_or(-1))
    }
}

/// Phase 3 layer 1: the readiness SNAPSHOT, pure and in-process, the same shape as the
/// Python bridge's `_build_status()` (installed / running / add-in probe / chip fields).
/// The probes run at most every 5 s (a health poll must never stall: ab reaps a bridge
/// whose /status hangs, and that killed two ten-part imports once).
struct Snapshot { at: Instant, body: Value }
static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);

fn addin_probe() -> Value {
    let v = ureq::get("http://127.0.0.1:8774/status").timeout(Duration::from_millis(1500)).call().ok()
        .and_then(|r| r.into_json::<Value>().ok()).unwrap_or(Value::Null);
    if v.is_object() { addin::note_answered(); }
    v
}

fn native_status(h: &dyn HostBoth) -> Value {
    if let Some(s) = SNAPSHOT.lock().unwrap().as_ref() { if s.at.elapsed() < Duration::from_secs(5) { return s.body.clone(); } }
    let exe = h.fusion_executable();
    let installed = exe.is_some();
    let dirs = h.addin_dirs();
    let addins_dir = dirs.iter().find(|d| d.exists()).cloned().or_else(|| dirs.first().cloned());
    let addin_installed = dirs.iter().any(|d| d.join("AdomBridge").join("AdomBridge.py").is_file());
    let running = !h.fusion_processes().is_empty();
    let addin = if running { addin_probe() } else { Value::Null };
    let addin_ok = addin.is_object();
    // layer 2: the window facts, from ab (never local Win32)
    let facts = if running {
        match fusion_core::ab::Loopback::from_env() { Some(ab) => fusion_core::windows_facts::facts_from_ab(&ab, &Caller::default()), None => fusion_core::windows_facts::WindowFacts { degraded: true, error: Some("ADOM_DIRECT_API_URL not set".into()), ..Default::default() } }
    } else { fusion_core::windows_facts::WindowFacts::default() };
    // The host gate: a platform the bridge has NO host implementation for. macOS is a
    // first-class host now (`os-mac`, `MacHost`), so `!cfg!(windows)` was wrong: it reported
    // macOS as unsupported even with Fusion installed, running and the add-in answering.
    // Only Linux and other targets still have no host (main.rs's `NoHost`).
    #[cfg(any(windows, target_os = "macos"))]
    const HOST_UNSUPPORTED: Option<(&str, &str)> = None;
    #[cfg(not(any(windows, target_os = "macos")))]
    const HOST_UNSUPPORTED: Option<(&str, &str)> = Some((
        "Unsupported OS",
        "Bridge running, but this host has no implementation. Fusion 360 verbs need Windows or macOS with Fusion 360 installed.",
    ));
    let (led, summary, tooltip) = if let Some((s, t)) = HOST_UNSUPPORTED {
        ("yellow", s, t)
    } else if !installed {
        ("yellow", "Fusion not installed", "Bridge running, but Fusion 360 is not installed. The AI can install it for the user (fusion-onboarding), then fusion_start.")
    } else if !running {
        ("yellow", "Fusion not running", "Fusion 360 is installed but not running. Call fusion_start to launch it.")
    } else if !addin_ok {
        ("yellow", "Add-in not connected", "Fusion 360 is running but the AdomBridge add-in isn't responding yet (still loading; if it persists the AI restarts Fusion via fusion_stop/start).")
    } else {
        ("green", "Fusion ready", "Fusion 360 running, AdomBridge add-in connected. All fusion_* verbs available.")
    };
    let long_op = LONG_OP.lock().unwrap().clone().map(|n| json!(n)).or_else(addin::long_command_active);
    let body = json!({
        "status": "ok", "busy": long_op.is_some(), "longOp": long_op,
        "led": led, "summary": summary, "tooltip": tooltip, "bridgeVersion": VERSION,
        "fusion": {"installed": installed, "exe_path": exe.map(|p| p.to_string_lossy().to_string()),
                   "addins_dir": addins_dir.map(|p| p.to_string_lossy().to_string()),
                   "addin_installed": addin_installed, "running": running},
        "addin": addin, "shell": "rust",
        "needsSignin": facts.needs_signin, "licensingDialog": facts.licensing_dialog.is_some(),
        "windowTitle": facts.main_title, "windowFactsDegraded": facts.degraded, "windowFactsError": facts.error,
    });
    *SNAPSHOT.lock().unwrap() = Some(Snapshot { at: Instant::now(), body: body.clone() });
    body
}

/// A port for the SIDECAR that ab can never mistake for the bridge's. ab allocates bridge
/// ports from the OS ephemeral range (49152+), persists the last one, and on a respawn probes
/// that port FIRST and routes to whatever answers /health there. An ephemeral pick for the
/// sidecar landed on exactly that persisted port once (Windows hands back the last freed
/// port), and ab bypassed the exe for the rest of the session. So the sidecar lives in
/// 20000..29999, which ab's allocator never touches.
fn pick_free_port() -> u16 {
    let seed = std::process::id() as u16 % 5000;
    for i in 0..5000u16 {
        let port = 20000 + (seed + i * 7) % 10000;
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() { return port; }
    }
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(8773)
}

fn caller_from(req: &tiny_http::Request) -> Caller {
    let mut c = Caller::default();
    for h in req.headers() {
        let k = h.field.as_str().to_ascii_lowercase();
        let v = h.value.as_str().to_string();
        match k.as_str() {
            "x-adom-caller-thread" => c.thread = v,
            "x-adom-caller-container" => c.container = v,
            "x-adom-caller-reason" => c.reason = v,
            _ => {}
        }
    }
    c
}

fn json_response(status: u16, v: &Value) -> Response<std::io::Cursor<Vec<u8>>> {
    let body = serde_json::to_vec(v).unwrap_or_default();
    Response::from_data(body).with_status_code(status)
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version") { println!("adom-fusion-bridge v{VERSION}"); return; }
    let port: u16 = args.iter().position(|a| a == "--port")
        .and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok()).unwrap_or(8773);
    let bind = std::env::var("ADOM_BIND_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let catalog = Arc::new(fusion_core::contract::load().expect("frozen verb catalog"));
    let h: Arc<Box<dyn HostBoth>> = Arc::new(host());
    let sidecar = Arc::new(Mutex::new(if sidecar_wanted(&**h) { Sidecar::spawn(&bind, &h.bridge_state_dir()) } else { eprintln!("[fusion-bridge] sidecar not spawned: every surface is owned by the exe (set an owner pref to \"sidecar\" or ADOM_FUSION_SIDECAR=1 to bring it back)"); None }));
    // ab stops this bridge with a hard kill (bridge_install, supervisor respawn): the sidecar
    // must go with it, or every respawn leaves one more Python on a random port.
    if let Some(sc) = sidecar.lock().unwrap().as_ref() {
        if let Some(pid) = sc.child.as_ref().map(|c| c.id()) {
            if !h.tie_child_lifetime(pid) { eprintln!("[fusion-bridge] could not tie the sidecar (pid {pid}) to this process's lifetime"); }
        }
    }
    let server = Server::http(format!("{bind}:{port}")).expect("bind loopback");
    *launch::SELF_URL.lock().unwrap() = Some(format!("http://127.0.0.1:{port}/command"));
    addin_install::startup(&h.bridge_state_dir(), &h.addin_dirs(), h.fusion_executable().is_some());
    {
        let (sc1, sc2) = (sidecar.clone(), sidecar.clone());
        let ready: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || owned_readiness(&*host(), &sc1, &Caller::default())["ready"].as_bool() == Some(true));
        let aps_signed: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || native_aps_status(&sc2, &Caller::default())["data"]["signedIn"].as_bool() == Some(true));
        signin::start_janitor(ready, aps_signed, Caller::default());
        janitor::start();
    }
    eprintln!("[fusion-bridge] v{VERSION} listening on {bind}:{port}; contract frozen {}; sidecar {}",
              catalog.frozen,
              sidecar.lock().unwrap().as_ref().map(|s| format!("python on :{}", s.port)).unwrap_or("absent".into()));

    // EVERY REQUEST ON ITS OWN THREAD. ab polls /status every few seconds and reaps a bridge
    // whose health poll stalls; a sequential loop let one long verb (fusion_start, a seven-
    // minute bind) block the poll, and ab respawned this exe twice during a single Fusion
    // launch, orphaning the run (ConfRoomROG, 2026-09-03). The Python bridge learned the
    // same lesson on 2026-08-29. tiny_http hands out requests concurrently; we take them.
    for req in server.incoming_requests() {
        let (h, catalog, sidecar) = (h.clone(), catalog.clone(), sidecar.clone());
        let port_now = port;
        std::thread::spawn(move || {
            let mut req = req;
            let port = port_now;
            let path = req.url().split('?').next().unwrap_or("/").to_string();
        // A panic in a handler must cost one request (tiny_http answers 500 for the dropped
        // request), never the process: ab paints the chip red the moment the port goes dark.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        match (req.method(), path.as_str()) {
            (Method::Get, "/status") | (Method::Get, "/health") => {
                // Prefer the sidecar's rich status (Fusion/add-in probe) while it exists;
                // this process is up regardless, so the answer is ALWAYS 2xx.
                // Without a sidecar this IS the answer; with one, the sidecar's rich status
                // wins and the native snapshot rides beside it for parity checks.
                let native = native_status(&**h);
                let mut body = native.clone();
                body["exePort"] = json!(port);
                if let Some(sc) = sidecar.lock().unwrap().as_mut() {
                    if let Some(code) = sc.exited() {
                        body["led"] = json!("red"); body["status"] = json!("error");
                        body["summary"] = json!(format!("Python sidecar exited (code {code})"));
                        body["tooltip"] = json!("The Rust shell is up but its Python sidecar died; read ~/.adom/fusion-bridge/sidecar.log. bridge_kill respawns both.");
                        body["sidecarExit"] = json!(code);
                    } else if let Ok(r) = ureq::get(&sc.url("/status")).timeout(Duration::from_secs(4)).call() {
                        if let Ok(mut v) = r.into_json::<Value>() {
                            v["shell"] = json!("rust"); v["sidecarPort"] = json!(sc.port); v["exePort"] = json!(port);
                            v["sidecarUptimeSec"] = json!(sc.started.elapsed().as_secs());
                            v["nativeSnapshot"] = json!({"led": native["led"], "summary": native["summary"], "fusion": native["fusion"], "addinOk": native["addin"].is_object(),
                                                         "needsSignin": native["needsSignin"], "licensingDialog": native["licensingDialog"], "windowTitle": native["windowTitle"], "windowFactsDegraded": native["windowFactsDegraded"], "windowFactsError": native["windowFactsError"]});
                            body = v;
                        }
                    }
                }
                let _ = req.respond(json_response(200, &body));
            }
            (Method::Post, "/demo/event") => {
                let mut raw = String::new(); let _ = req.as_reader().read_to_string(&mut raw);
                let ev: Value = serde_json::from_str(&raw).unwrap_or(json!({}));
                let reply = tour::on_panel_event(&ev);
                let _ = req.respond(json_response(200, &reply));
            }
            (Method::Post, "/command") => {
                let caller = caller_from(&req);
                let mut raw = String::new();
                let _ = req.as_reader().read_to_string(&mut raw);
                let parsed: Value = serde_json::from_str(&raw).unwrap_or(json!({}));
                let asked = parsed.get("command").and_then(|c| c.as_str()).unwrap_or("").to_string();
                // the EDA Bridge Standard's name for a job this bridge does under its own name
                let (verb, raw) = match standard_alias(&asked) {
                    Some(own) => { let mut p = parsed.clone(); p["command"] = json!(own); (own.to_string(), p.to_string()) }
                    None => (asked.clone(), raw),
                };
                // export_dxf with layers is the standard's board-layer DXF; without, Fusion's mechanical DXF
                let verb = if matches!(verb.as_str(), "fusion_export_dxf" | "export_dxf") && parsed["args"]["layers"].is_array() { "fusion_board_dxf".to_string() } else { verb };
                // the standard's generate_body and library_views, in this bridge's own verbs' terms
                let (verb, parsed) = standard_adapter(&verb, parsed);
                let raw = if verb.as_str() == asked.as_str() || standard_alias(&asked).is_some() { raw } else { parsed.to_string() };
                let reply = match verb.as_str() {
                    // Mechanical history is the exe's own journal: observational even when Fusion is
                    // cold, blocked or an owner preference selects the legacy sidecar.
                    v if mech::owns_status(v) => { let pids: Vec<u32> = h.fusion_processes().iter().map(|p| p.pid).collect(); let mut r = mech::status(&h.mechanical_state_dir(), &pids, &parsed["args"]); r["shell"] = json!("rust"); r },
                    v if mech::owns_capabilities(v) => {
                        let (sc, ca) = (sidecar.clone(), caller.clone());
                        let side = |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t);
                        let dlg = |r: Value| r;
                        let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: h.bridge_state_dir(), caller: caller.clone(), pids: h.fusion_processes().iter().map(|p| p.pid).collect(), sidecar: &side, dialogs: &dlg };
                        let mut r = mech::capabilities(&h.mechanical_state_dir(), &|| h.fusion_processes().iter().map(|p| p.pid).collect(), &|| addin::pass_through("get_app_state", &json!({}), &ctx));
                        r["shell"] = json!("rust"); r },
                    // Phase 3 shadow: the sidecar still answers; the exe computes its own readiness
                    // from real probes and rides beside it with a diff, until parity holds.
                    "fusion_readiness" | "readiness" if readiness_owner_is_exe(&**h) => owned_readiness(&**h, &sidecar, &caller),
                    "fusion_instances" | "instances" => appverbs::instances(&h.fusion_processes()),
                    "fusion_progress" | "progress" => appverbs::progress(&h.bridge_state_dir(), LONG_OP.lock().unwrap().clone(), *LONG_OP_SINCE.lock().unwrap(), parsed["args"]["recent"].as_u64().unwrap_or(10) as usize),
                    "fusion_diagnostics" | "diagnostics" => appverbs::diagnostics(&h.fusion_processes(), h.fusion_executable(), &h.bridge_state_dir(), &caller),
                    "fusion_foreground" | "foreground" => appverbs::foreground(&parsed["args"], &h.bridge_state_dir(), &caller),
                    "fusion_create_library" | "create_library" if chains_owner_is_exe(&**h) => chains::create_library(Arc::new(ExeChainHost { sidecar: sidecar.clone(), caller: caller.clone(), state_dir: h.bridge_state_dir() }), parsed["args"].clone()),
                    "fusion_install_library_bundle" | "install_library_bundle" if chains_owner_is_exe(&**h) => chains::install_library_bundle(Arc::new(ExeChainHost { sidecar: sidecar.clone(), caller: caller.clone(), state_dir: h.bridge_state_dir() }), parsed["args"].clone()),
                    "fusion_prove_result" | "prove_result" if chains_owner_is_exe(&**h) => { let mut r = chains::prove_result(&parsed["args"]); r["shell"] = json!("rust"); r },
                    // a schematic file read in the exe: no Fusion needed
                    "fusion_generate_body_engine" => { let mut r = body::generate(&parsed["args"]); r["bridgeVersion"] = json!(VERSION); r["shell"] = json!("rust"); r },
                    "fusion_read_sheet_file" | "read_sheet_file" => { let mut r = sheet::read_sheet_file(&parsed["args"]); r["bridgeVersion"] = json!(VERSION); r["shell"] = json!("rust"); r },
                    "fusion_netlist" | "netlist" if parsed["args"]["schPath"].is_string() => { let mut r = sheet::netlist_from_sch(&parsed["args"]); r["bridgeVersion"] = json!(VERSION); r["shell"] = json!("rust"); r },
                    v if routing::owns(v.strip_prefix("fusion_").unwrap_or(v)) || matches!(v.strip_prefix("fusion_").unwrap_or(v), "replicate_layout" | "silkscreen_prep" | "list_installed_libraries" | "update_footprints_from_library") || sch::owns(v.strip_prefix("fusion_").unwrap_or(v)) || docs::owns(v.strip_prefix("fusion_").unwrap_or(v)) || checks::owns(v.strip_prefix("fusion_").unwrap_or(v)) => {
                        let (sc, ca) = (sidecar.clone(), caller.clone());
                        let side = |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command":verb,"args":a}).to_string(), &ca, t);
                        let dlg = |r: Value| r;
                        let ctx = addin::Ctx { host: &**h, fusion_exe:h.fusion_executable(), short_path:|p|host().short_path(p), state_dir:h.bridge_state_dir(), caller:caller.clone(), pids:h.fusion_processes().iter().map(|p|p.pid).collect(), sidecar:&side, dialogs:&dlg };
                        let cmd=v.strip_prefix("fusion_").unwrap_or(v);
                        let adds_from_library = matches!(cmd, "schematic_run" | "place_parts" | "sch_place_part" | "sch_swap_part") && parsed["args"]["validateOnly"].is_null() && parsed["args"]["runId"].is_null();
                        let picker = if adds_from_library { small_verbs::watch_for_library_picker(&ctx.caller, &ctx.pids) } else { None };
                        let mut r=match cmd { "bodies" => bodies::bodies(&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), "replicate_layout" => replicate::replicate_layout(&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), "silkscreen_prep" => checks::silkscreen_prep(&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), "list_installed_libraries" => appverbs::list_installed_libraries(&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), "update_footprints_from_library" => boardgen::update_footprints_from_library(&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), "simulate" => spice::simulate(&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), "jlcpcb_order" => order::jlcpcb_order(&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), "export_bom" => routing::export_bom(&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), "save_document" => savedoc::save_document(&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), "save_to_cloud" => savedoc::save_to_cloud(&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), c if sch::owns(c) => sch::call(c,&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), c if docs::owns(c) => docs::call(c,&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), c if writers::owns(c) => writers::call(c,&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), c if checks::owns(c) => checks::call(c,&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), "schematic_run" => routing::schematic_run(&ctx.state_dir,&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)), _ => routing::call(cmd,&parsed["args"],&|v,a|addin::pass_through(v,&a,&ctx)) };
                        r=small_verbs::finish_picker_watch(picker, r);
                        if cmd=="netlist" { r=sheet::shape_board_netlist(r,&parsed["args"]); }
                        r["bridgeVersion"]=json!(VERSION); r["shell"]=json!("rust"); r
                    },
                    v @ ("fusion_walk_cloud_tree" | "walk_cloud_tree" | "fusion_search_cloud_files" | "search_cloud_files") => {
                        // HARD BLOCK, as the contract promises. The in-app walk holds Fusion's main thread
                        // for many minutes and has crashed Fusion (WinError 10054). The Python sidecar
                        // refused these; the exe let them through to the add-in from 1.10.0 until the
                        // arav-rog suite run caught a 6-minute walk (2026-09-20). Nothing reaches Fusion.
                        let st = native_aps_status(&sidecar, &caller);
                        let signed = st["data"]["signedIn"].as_bool().unwrap_or(false);
                        let configured = st["data"]["configured"].as_bool().unwrap_or(false);
                        let next = if signed { "APS is signed in on this box: fusion_aps_search {query} answers in seconds, fusion_aps_open {query} opens by name." }
                            else if configured { "APS is configured but signed out: fusion_aps_signin once, then fusion_aps_search {query}." }
                            else { "APS is not set up on this box yet: fusion_aps_status explains the one-time setup, then fusion_aps_signin. Until then fusion_list_cloud_projects / fusion_list_cloud_files read ONE folder at a time without the crash risk." };
                        json!({"success": false, "errorCode": "deprecated_cloud_search_disabled", "shell": "rust",
                            "error": format!("{v} is DISABLED: the in-app Fusion cloud search/walk takes 30+ min and CAN CRASH Fusion (WinError 10054). Use fusion_aps_search {{\"query\":\"...\"}} (or fusion_aps_open to open by name) instead."),
                            "output": "deprecated_cloud_search_disabled",
                            "data": {"disabled": true, "use": "fusion_aps_search", "apsConfigured": configured, "apsSignedIn": signed},
                            "_hint": next})
                    },
                    // import_step is the exe's (it places the STEP with the board; into:"new" passes to the add-in)
                    v if addin_owner_is_exe(&**h) && addin::owns(v.strip_prefix("fusion_").unwrap_or(v)) && !matches!(v, "fusion_import_step" | "import_step") => {
                        let cmd = v.strip_prefix("fusion_").unwrap_or(v).to_string();
                        let (sc, ca) = (sidecar.clone(), caller.clone());
                        let sd = h.bridge_state_dir();
                        let t0 = Instant::now();
                        let ind = timings::Indicator::start(&sd, &cmd, &caller);
                        let pids: Vec<u32> = h.fusion_processes().iter().map(|p| p.pid).collect();
                        let side = |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t);
                        let dlg = |res: Value| small_verbs::merge_dialog_array(res, &ca, &pids);
                        let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: sd.clone(), caller: caller.clone(), pids: pids.clone(), sidecar: &side, dialogs: &dlg };
                        let pt = |v: &str, a: Value| addin::pass_through(v, &a, &ctx);
                        let mut r = match cmd.as_str() {
                            "show_schematic" | "show_2d_board" | "show_3d_board" => orchestrations::show_with_document(&cmd, &parsed["args"], &pt),
                            "export_source" | "export_eagle_source" => orchestrations::export_in_right_view(&cmd, &parsed["args"], &pt),
                            "activate_document" => orchestrations::activate_with_same_name_note(&parsed["args"], &pt),
                            "mechanical" => mech::mechanical(&h.mechanical_state_dir(), &pids, &parsed["args"], &pt),
                            "take_screenshot" if parsed["args"]["outputPath"].as_str().map(|p| p.trim().is_empty()).unwrap_or(true) => {
                                // the contract says outputPath is optional; the add-in insisted on one
                                let mut a = if parsed["args"].is_object() { parsed["args"].clone() } else { json!({}) };
                                a["outputPath"] = json!(orchestrations::shots_dir().join(format!("viewport-{}.png", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0))).to_string_lossy().replace('\\', "/"));
                                addin::pass_through(&cmd, &a, &ctx)
                            }
                            _ => addin::pass_through(&cmd, &parsed["args"], &ctx),
                        };
                        small_verbs::add_image_delivery(&mut r);
                        ind.stop(r["success"].as_bool() == Some(false));
                        timings::log_run(&sd, &cmd, t0.elapsed().as_secs_f64(), r["success"].as_bool() != Some(false));
                        r["shell"] = json!("rust"); r },
                    "fusion_start" | "start" | "launch" if addin_owner_is_exe(&**h) => { let (sc, ca) = (sidecar.clone(), caller.clone()); let side = move |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t); let sd = h.bridge_state_dir(); let dlg = |r: Value| r; let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: sd.clone(), caller: caller.clone(), pids: h.fusion_processes().iter().map(|p| p.pid).collect(), sidecar: &side, dialogs: &dlg }; let t0 = Instant::now(); let ind = timings::Indicator::start(&sd, "start", &caller); let mut r = launch::start(&ctx, &parsed["args"]); ind.stop(r["success"].as_bool() == Some(false)); timings::log_run(&sd, "start", t0.elapsed().as_secs_f64(), r["success"].as_bool() != Some(false)); r["shell"] = json!("rust"); r },
                    "fusion_open_lbr" | "open_lbr" if addin_owner_is_exe(&**h) => { let (sc, ca, ca2) = (sidecar.clone(), caller.clone(), caller.clone()); let side = move |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t); let sd = h.bridge_state_dir(); let pids: Vec<u32> = h.fusion_processes().iter().map(|p| p.pid).collect(); let dlg = |r: Value| small_verbs::merge_dialog_array(r, &ca2, &pids); let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: sd.clone(), caller: caller.clone(), pids: pids.clone(), sidecar: &side, dialogs: &dlg }; let t0 = Instant::now(); let ind = timings::Indicator::start(&sd, "open_lbr", &caller); let mut r = orchestrations::open_lbr(&ctx, &parsed["args"], &|v, a| addin::pass_through(v, &a, &ctx)); ind.stop(r["success"].as_bool() == Some(false)); timings::log_run(&sd, "open_lbr", t0.elapsed().as_secs_f64(), r["success"].as_bool() != Some(false)); r["shell"] = json!("rust"); r },
                    v @ ("fusion_show_symbol" | "show_symbol" | "fusion_show_footprint" | "show_footprint" | "fusion_show_library" | "show_library" | "fusion_show_project" | "show_project" | "fusion_show_schematic" | "show_schematic" | "fusion_show_2d_board" | "show_2d_board" | "fusion_show_3d_board" | "show_3d_board" | "fusion_capture_library_views" | "capture_library_views" | "fusion_library_views" | "library_views" | "fusion_open_cloud_file" | "open_cloud_file" | "fusion_save_lbr" | "save_lbr" | "fusion_cleanup_cloud_files" | "cleanup_cloud_files" | "fusion_inspect_bodies" | "inspect_bodies" | "fusion_board_stackup" | "board_stackup" | "fusion_export_optimized_glb" | "export_optimized_glb" | "fusion_export_3d" | "export_3d" | "fusion_board_summary" | "board_summary" | "fusion_read_schematic" | "read_schematic" | "fusion_export_pos" | "export_pos" | "fusion_render_board_layers" | "render_board_layers" | "fusion_bom" | "bom" | "fusion_export_manufacturing" | "export_manufacturing" | "fusion_model_check" | "model_check" | "fusion_extensions" | "extensions" | "fusion_update_pcb" | "update_pcb" | "fusion_select_items" | "select_items" | "fusion_pcb_report" | "pcb_report" | "fusion_import_step" | "import_step" | "fusion_fetch_optimized_glb" | "fetch_optimized_glb" | "fusion_set_design_rules" | "set_design_rules" | "fusion_make_test_project" | "make_test_project" | "fusion_project_create" | "project_create" | "fusion_test_board" | "test_board" | "fusion_build_board" | "build_board" | "fusion_swap_footprint" | "swap_footprint" | "fusion_panelize" | "panelize" | "fusion_make_showcase_project" | "make_showcase_project" | "fusion_open_design" | "open_design" | "fusion_new_electronics_from_eagle" | "new_electronics_from_eagle" | "fusion_aps_open" | "aps_open" | "fusion_schematic" | "schematic" | "fusion_eagle_import_status" | "eagle_import_status" | "fusion_show_attributes" | "show_attributes") if addin_owner_is_exe(&**h) => { let (sc, ca, ca2) = (sidecar.clone(), caller.clone(), caller.clone()); let side = move |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t); let sd = h.bridge_state_dir(); let pids: Vec<u32> = h.fusion_processes().iter().map(|p| p.pid).collect(); let dlg = |r: Value| small_verbs::merge_dialog_array(r, &ca2, &pids); let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: sd.clone(), caller: caller.clone(), pids: pids.clone(), sidecar: &side, dialogs: &dlg }; let cmd = v.strip_prefix("fusion_").unwrap_or(v); let t0 = Instant::now(); let ind = timings::Indicator::start(&sd, cmd, &caller); let pass = |verb: &str, a: Value| addin::pass_through(verb, &a, &ctx); let r0 = match cmd { "show_symbol" => orchestrations::show_symbol(&ctx, &parsed["args"], &pass), "show_footprint" => orchestrations::show_footprint(&ctx, &parsed["args"], &pass), "show_library" => orchestrations::show_library(&ctx, &parsed["args"], &pass), "show_project" => match orchestrations::show_project(&ctx, &parsed["args"], &pass) { Some(r) => r, None => side("show_project", parsed["args"].clone(), 240) }, "capture_library_views" => small_verbs::apply_failure_dialogs(orchestrations::capture_library_views(&ctx, &parsed["args"], &pass), &caller, &pids), "library_views" => { let mut r = small_verbs::apply_failure_dialogs(orchestrations::capture_library_views(&ctx, &docs::library_views_args(&parsed["args"]), &pass), &caller, &pids); r["aliasOf"] = json!("fusion_capture_library_views"); r }, "open_cloud_file" => orchestrations::open_cloud_file(&ctx, &parsed["args"], &pass), "save_lbr" => small_verbs::merge_dialog_array(orchestrations::save_lbr(&ctx, &parsed["args"], &pass), &caller, &pids), "cleanup_cloud_files" => orchestrations::cleanup_cloud_files(&parsed["args"], &pass), "inspect_bodies" => orchestrations::inspect_bodies(&parsed["args"], &pass), "board_stackup" => services::board_stackup(&parsed["args"], &pass), "board_summary" => stdshapes::board_summary(&parsed["args"], &pass), "read_schematic" => stdshapes::read_schematic(&parsed["args"], &pass), "export_pos" => stdshapes::export_pos(&parsed["args"], &pass), "render_board_layers" => stdshapes::render_board_layers(&parsed["args"], &pass), "bom" => stdshapes::bom(&parsed["args"], &pass), "export_manufacturing" => stdshapes::export_manufacturing(&parsed["args"], &pass), "pcb_report" => stdshapes::pcb_report(&parsed["args"], &pass), "update_pcb" => stdshapes::update_pcb(&parsed["args"], &pass), "model_check" => stdshapes::model_check(&parsed["args"], &pass), "extensions" => stdshapes::extensions(&parsed["args"]), "select_items" => stdshapes::select_items(&parsed["args"], &pass), "export_3d" => services::export_3d(&parsed["args"], &pass), "import_step" => if parsed["args"]["into"].as_str() == Some("new") { pass("import_step", parsed["args"].clone()) } else { services::import_step_with_board(&parsed["args"], &pass) }, "export_optimized_glb" => if parsed["args"]["async"].as_bool() == Some(true) { services::start_glb_export_async(&ctx, &parsed["args"]) } else { services::export_optimized_glb(&parsed["args"], &pass) }, "fetch_optimized_glb" => if parsed["args"]["exportId"].is_string() { services::glb_export_async_status(&parsed["args"]) } else { services::fetch_optimized_glb(&parsed["args"]) }, "set_design_rules" => services::set_design_rules(&ctx, &parsed["args"], &pass), "make_test_project" => small_verbs::merge_dialog_array(testproject::make_test_project(&ctx, &parsed["args"], &pass), &caller, &pids), "project_create" => small_verbs::merge_dialog_array(testproject::project_create(&ctx, &parsed["args"], &pass), &caller, &pids), "test_board" => small_verbs::merge_dialog_array(boardgen::test_board(&ctx, &parsed["args"], &pass), &caller, &pids), "build_board" => small_verbs::merge_dialog_array(boardgen::build_board(&ctx, &parsed["args"], &pass), &caller, &pids), "swap_footprint" => small_verbs::merge_dialog_array(boardgen::swap_footprint(&ctx, &parsed["args"], &pass), &caller, &pids), "panelize" => small_verbs::merge_dialog_array(boardgen::panelize(&ctx, &parsed["args"], &pass), &caller, &pids), "make_showcase_project" => small_verbs::merge_dialog_array(testproject::make_showcase_project(&ctx, &parsed["args"], &pass), &caller, &pids), "open_design" => orchestrations::open_design(&ctx, &parsed["args"], &pass), "new_electronics_from_eagle" => orchestrations::new_electronics_from_eagle(&ctx, &parsed["args"], &pass), "schematic" => orchestrations::schematic(&ctx, &parsed["args"], &pass), "eagle_import_status" => orchestrations::eagle_import_status(&ctx, &parsed["args"]), "show_attributes" => orchestrations::show_attributes(&ctx, &parsed["args"], &pass), "aps_open" => orchestrations::aps_open(&ctx, &parsed["args"], &pass), other => orchestrations::open_with_screenshot(&ctx, other, &parsed["args"], &pass) }; let mut r = small_verbs::merge_dialog_array(r0, &caller, &pids); small_verbs::add_image_delivery(&mut r); ind.stop(r["success"].as_bool() == Some(false)); timings::log_run(&sd, cmd, t0.elapsed().as_secs_f64(), r["success"].as_bool() != Some(false)); r["shell"] = json!("rust"); r },
                    "fusion_state" | "state" if addin_owner_is_exe(&**h) => { let (sc, ca) = (sidecar.clone(), caller.clone()); let side = move |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t); let dlg = |r: Value| r; let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: h.bridge_state_dir(), caller: caller.clone(), pids: h.fusion_processes().iter().map(|p| p.pid).collect(), sidecar: &side, dialogs: &dlg }; let mut r = orchestrations::fusion_state(&ctx, &parsed["args"]); r["shell"] = json!("rust"); r },
                    v @ ("fusion_make_3d_package" | "make_3d_package" | "fusion_build_library_3d" | "build_library_3d" | "fusion_bind_library_3d" | "bind_library_3d" | "fusion_prove_library" | "prove_library" | "fusion_attach_3d_package" | "attach_3d_package" | "fusion_show_3d_package" | "show_3d_package" | "fusion_import_part" | "import_part") if addin_owner_is_exe(&**h) => { let (sc, ca, ca2) = (sidecar.clone(), caller.clone(), caller.clone()); let side = move |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t); let sd = h.bridge_state_dir(); let pids: Vec<u32> = h.fusion_processes().iter().map(|p| p.pid).collect(); let dlg = |r: Value| small_verbs::merge_dialog_array(r, &ca2, &pids); let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: sd.clone(), caller: caller.clone(), pids: pids.clone(), sidecar: &side, dialogs: &dlg }; let cmd = v.strip_prefix("fusion_").unwrap_or(v); let t0 = Instant::now(); *LONG_OP.lock().unwrap() = Some(cmd.to_string()); *LONG_OP_SINCE.lock().unwrap() = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0); let pass = |verb: &str, a: Value| addin::pass_through(verb, &a, &ctx); let r0 = match cmd { "make_3d_package" => mint::make_3d_package(&ctx, &parsed["args"], &pass), "bind_library_3d" => mint::bind_library_3d(&ctx, &parsed["args"], &pass), "prove_library" => prove::prove_library(&ctx, &parsed["args"], &pass), "attach_3d_package" => mint::attach_3d_package(&ctx, &parsed["args"], &pass), "show_3d_package" => mint::show_3d_package(&ctx, &parsed["args"], &pass), "import_part" => partimport::import_part(&ctx, &parsed["args"], &pass), _ => mint::build_library_3d(&ctx, &parsed["args"], &pass) }; *LONG_OP.lock().unwrap() = None; let mut r = small_verbs::apply_failure_dialogs(r0, &caller, &pids); timings::log_run(&sd, cmd, t0.elapsed().as_secs_f64(), r["success"].as_bool() != Some(false)); r["shell"] = json!("rust"); r },
                    v @ ("fusion_apply_fusion_update" | "apply_fusion_update" | "fusion_tool_update" | "tool_update" | "fusion_window_info" | "window_info" | "fusion_install_fusion" | "install_fusion" | "fusion_uninstall" | "uninstall" | "fusion_prefs_open" | "prefs_open" | "fusion_prefs_close" | "prefs_close" | "fusion_generate_package" | "generate_package" | "fusion_mcp_status" | "mcp_status" | "fusion_mcp_tools" | "mcp_tools" | "fusion_mcp_call" | "mcp_call" | "fusion_mcp_resources" | "mcp_resources" | "fusion_mcp_enable" | "mcp_enable" | "fusion_library_manager" | "library_manager" | "fusion_managed_library_create" | "managed_library_create" | "fusion_managed_library_link" | "managed_library_link" | "fusion_managed_library_pull" | "managed_library_pull" | "fusion_managed_library_push" | "managed_library_push" | "fusion_managed_library_unlink" | "managed_library_unlink" | "fusion_signin" | "signin" | "fusion_signin_2fa" | "signin_2fa" | "fusion_web_signin" | "web_signin" | "fusion_aps_signin" | "aps_signin" | "fusion_demo" | "demo" | "fusion_demo_progress" | "demo_progress" | "fusion_demo_control" | "demo_control" | "fusion_electronics_query" | "electronics_query" | "fusion_view" | "view" | "fusion_interactive" | "interactive" | "fusion_narrate" | "narrate") if addin_owner_is_exe(&**h) => { let (sc, ca) = (sidecar.clone(), caller.clone()); let side = move |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t); let dlg = |r: Value| r; let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: h.bridge_state_dir(), caller: caller.clone(), pids: h.fusion_processes().iter().map(|p| p.pid).collect(), sidecar: &side, dialogs: &dlg }; let pass = |verb: &str, a: Value| addin::pass_through(verb, &a, &ctx); let cmd = v.strip_prefix("fusion_").unwrap_or(v); let mut r = match cmd { "apply_fusion_update" => maintenance::apply_fusion_update(&ctx, &pass), "tool_update" => maintenance::tool_update(&ctx, &parsed["args"], &pass), "window_info" => stdshapes::window_info(&ctx), "install_fusion" => maintenance::install_fusion(&ctx), "prefs_open" => maintenance::prefs_open(&ctx, &parsed["args"], &pass), "prefs_close" => maintenance::prefs_close(&ctx, &parsed["args"]), "generate_package" => epg::generate_package(&parsed["args"], VERSION, &pass),
                            "signin" => { let readiness = || owned_readiness(&**h, &sidecar, &caller); let start = || launch::start(&ctx, &json!({})); let aps_state = || { let st = native_aps_status(&sidecar, &caller); (st["data"]["configured"].as_bool().unwrap_or(false), st["data"]["signedIn"].as_bool().unwrap_or(false)) }; let aps_signin = |p: &str| aps_signin::signin(&ctx, &json!({"profile": p})); signin::signin(&ctx, &parsed["args"], &signin::Env { readiness: &readiness, start: &start, aps_state: &aps_state, aps_signin: &aps_signin }) },
                            "signin_2fa" => signin::signin_2fa(&ctx, &parsed["args"]), "aps_signin" => aps_signin::signin(&ctx, &parsed["args"]),
                            "electronics_query" => electronics::query(&ctx, &parsed["args"], VERSION, &pass), "view" => tour::view(&pass, &parsed["args"]), "narrate" => tour::narrate(&parsed["args"]), "interactive" => tour::interactive(&pass, parsed["args"]["probeMainThread"].as_bool().unwrap_or(true)), "demo_progress" => if tour::owns(parsed["args"]["demoId"].as_str().unwrap_or("")) || parsed["args"]["demoId"].as_str().map(|s| s.is_empty()).unwrap_or(true) { tour::progress(&parsed["args"]) } else { demo::demo_progress(&ctx.state_dir, &parsed["args"]) },
                            "demo_control" => if tour::owns(parsed["args"]["demoId"].as_str().unwrap_or("")) || parsed["args"]["demoId"].as_str().map(|s| s.is_empty()).unwrap_or(true) { tour::control(&parsed["args"]) } else { demo::demo_control(&parsed["args"]) },
                            "demo" if parsed["args"]["mode"].as_str() == Some("tour") && parsed["args"]["legacy"].as_bool() != Some(true) => display::annotate(tour::start(&ctx, &parsed["args"], tour::Env { version: VERSION })),
                            "demo" => if parsed["args"]["mode"].as_str() == Some("tour") { let (sc1, sc2) = (sidecar.clone(), sidecar.clone()); let readiness: Arc<dyn Fn() -> Value + Send + Sync> = Arc::new(move || owned_readiness(&*host(), &sc1, &Caller::default())); let aps_state: Arc<dyn Fn() -> (bool, bool) + Send + Sync> = Arc::new(move || { let st = native_aps_status(&sc2, &Caller::default()); (st["data"]["configured"].as_bool().unwrap_or(false), st["data"]["signedIn"].as_bool().unwrap_or(false)) }); demo::demo_tour(&ctx, &parsed["args"], demo::TourEnv { readiness, aps_state, version: VERSION }, &pass) }
                                else { let readiness = || owned_readiness(&**h, &sidecar, &caller); let aps_state = || native_aps_status(&sidecar, &caller)["data"].clone(); let aps_search = |a: &Value| native_aps_search(a); let start = || launch::start(&ctx, &json!({})); let aps_pair = || { let st = native_aps_status(&sidecar, &caller); (st["data"]["configured"].as_bool().unwrap_or(false), st["data"]["signedIn"].as_bool().unwrap_or(false)) }; let aps_signin = |p: &str| aps_signin::signin(&ctx, &json!({"profile": p})); let signin_fn = |a: &Value| signin::signin(&ctx, a, &signin::Env { readiness: &readiness, start: &start, aps_state: &aps_pair, aps_signin: &aps_signin }); demo::demo(&ctx, &parsed["args"], &demo::DemoEnv { readiness: &readiness, signin: &signin_fn, aps_state: &aps_state, aps_search: &aps_search }, &pass) }, "web_signin" => signin::web_signin(&ctx, &parsed["args"]),
                            "library_manager" => managed::library_manager(&ctx, &parsed["args"], &pass), "managed_library_create" => managed::create(&ctx, &parsed["args"], &pass), "managed_library_link" => managed::link(&ctx, &parsed["args"], &pass), "managed_library_pull" => managed::pull(&ctx, &parsed["args"], &pass), "managed_library_push" => managed::push(&ctx, &parsed["args"], &pass), "managed_library_unlink" => managed::unlink(&ctx, &parsed["args"], &pass),
                            "mcp_status" | "mcp_tools" | "mcp_call" | "mcp_resources" | "mcp_enable" => { let st = native_aps_status(&sidecar, &caller); let aps = (st["data"]["configured"].as_bool().unwrap_or(false), st["data"]["signedIn"].as_bool().unwrap_or(false)); match cmd { "mcp_status" => mcp::status(&ctx, VERSION, aps), "mcp_tools" => mcp::tools(VERSION, aps), "mcp_call" => mcp::call(VERSION, &parsed["args"], aps), "mcp_resources" => mcp::resources(VERSION, &parsed["args"], aps), _ => mcp::enable(&ctx, VERSION, aps, &pass) } }, _ => maintenance::uninstall(&ctx, &h.addin_dirs(), &parsed["args"]) }; r["shell"] = json!("rust"); r },
                    "fusion_live_feed" | "live_feed" if addin_owner_is_exe(&**h) => { let pids: Vec<u32> = h.fusion_processes().iter().map(|p| p.pid).collect(); let mut r = livefeed::live_feed(&caller, pids, &parsed["args"]); r["shell"] = json!("rust"); r },
                    "fusion_check_3d_thumbnail" | "check_3d_thumbnail" if addin_owner_is_exe(&**h) => { let mut r = prove::check_3d_thumbnail(&parsed["args"]); r["shell"] = json!("rust"); r },
                    "fusion_resolve_seat_dialog" | "resolve_seat_dialog" if addin_owner_is_exe(&**h) => { let (sc, ca) = (sidecar.clone(), caller.clone()); let side = move |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t); let dlg = |r: Value| r; let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: h.bridge_state_dir(), caller: caller.clone(), pids: h.fusion_processes().iter().map(|p| p.pid).collect(), sidecar: &side, dialogs: &dlg }; let mut r = readiness_exec::resolve_seat_dialog_verb(&ctx, &parsed["args"]); r["shell"] = json!("rust"); r },
                    v @ ("fusion_account" | "account" | "fusion_claim_seat" | "claim_seat" | "fusion_release_seat" | "release_seat") if addin_owner_is_exe(&**h) => {
                        let (sc, ca) = (sidecar.clone(), caller.clone()); let side = move |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t);
                        let sd = h.bridge_state_dir(); let dlg = |r: Value| r;
                        let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: sd.clone(), caller: caller.clone(), pids: h.fusion_processes().iter().map(|p| p.pid).collect(), sidecar: &side, dialogs: &dlg };
                        let pt = |verb: &str, a: Value| addin::pass_through(verb, &a, &ctx);
                        let mut r = match v.strip_prefix("fusion_").unwrap_or(v) {
                            "account" => seat::account(&ctx, &pt),
                            "claim_seat" => seat::claim_seat(&ctx, &parsed["args"], &pt),
                            _ => seat::release_seat(&ctx, &parsed["args"], &pt),
                        };
                        r["shell"] = json!("rust"); r
                    },
                    "fusion_prewarm" | "prewarm" => {
                        // ab calls this once per installed version, in the background, after the first
                        // boot that follows an install (adom-bridge 2.1.153, page issue #99). It fetches
                        // what a first real call would wait for and NEVER starts Fusion: launching the
                        // user's CAD app unasked is not a prewarm. Idempotent.
                        let t0 = Instant::now(); let sd = h.bridge_state_dir();
                        let mut steps = Vec::new(); let mut ok = true;
                        let fusion_installed = h.fusion_executable().is_some();
                        let dirs = h.addin_dirs();
                        if fusion_installed || dirs.iter().any(|d| d.exists()) {
                            let r = addin_install::install(&dirs);
                            let refused = r["refused"].as_str().map(str::to_string);
                            if refused.is_some() { ok = false; }
                            steps.push(json!({"step": "add-in synced into Fusion's add-in folders", "ok": refused.is_none(), "installedTo": r["installedTo"], "refused": refused}));
                        } else { steps.push(json!({"step": "add-in sync", "ok": true, "skipped": "Fusion is not installed on this machine"})); }
                        match fusionpath::stage_root(&sd) { Ok(p) => steps.push(json!({"step": "space-free staging folder ready", "ok": true, "path": p.to_string_lossy()})), Err(e) => { ok = false; steps.push(json!({"step": "staging folder", "ok": false, "error": e})); } }
                        match services::design_rules_index(&sd) { Ok(i) => steps.push(json!({"step": "PCB design-rules catalog cached", "ok": true, "packs": i["vendors"].as_array().map(|a| a.len()).unwrap_or(0)})), Err(e) => steps.push(json!({"step": "PCB design-rules catalog", "ok": false, "error": e.chars().take(160).collect::<String>(), "note": "not fatal: fetched on first use"})) }
                        // the standard's prewarm: each step's done (ok kept), and ready
                        for s in steps.iter_mut() { s["done"] = json!(s["ok"].as_bool() == Some(true) && s["skipped"].is_null()); }
                        json!({"success": ok, "ok": ok, "ready": ok, "mutated": false, "shell": "rust", "fusionInstalled": fusion_installed, "fusionStarted": false, "steps": steps, "durationMs": t0.elapsed().as_millis() as u64,
                            "_hint": if fusion_installed { "Prewarmed without starting Fusion. The first fusion_start still pays Fusion's own launch; nothing a bridge can do ahead of time shortens that." } else { "Fusion is not installed here, so there is nothing to warm. fusion_readiness explains onboarding." }})
                    },
                    v @ ("fusion_check_dialogs" | "check_dialogs" | "fusion_dismiss_recovery" | "dismiss_recovery" | "fusion_relocate_recovery" | "relocate_recovery" | "fusion_close" | "close") if addin_owner_is_exe(&**h) => {
                        // Legacy window verbs the skills still name (the Python sidecar owned them until
                        // 1.10.0). Each maps onto code the exe already runs on its own at launch and in
                        // the dialog gate, so they answer instead of reaching the sidecar fallback.
                        let (sc, ca) = (sidecar.clone(), caller.clone()); let side = move |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t);
                        let sd = h.bridge_state_dir(); let dlg = |r: Value| r;
                        let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: sd.clone(), caller: caller.clone(), pids: h.fusion_processes().iter().map(|p| p.pid).collect(), sidecar: &side, dialogs: &dlg };
                        let mut r = match v.strip_prefix("fusion_").unwrap_or(v) {
                            "check_dialogs" => { let d = lifecycle::dialog_inventory(&ctx); json!({"success": true, "count": d.len(), "dialogs": d, "output": format!("{} dialog(s) open", d.len()), "_hint": "Each dialog carries its title, text and buttons, read through UI Automation (no screenshot needed). The bridge expires unknown dialogs by itself before the next add-in verb; fusion_state reports the same list plus the window capture. In-canvas alerts that have no window are in fusion_alerts."}) },
                            "dismiss_recovery" => { let (done, reloc) = readiness_exec::dismiss_recovery_dialog(&ctx, 5.0, true); json!({"success": true, "dismissed": done, "relocated": reloc, "output": if done { "Recovery prompt dismissed in the background; the recovery files were moved, not deleted." } else { "No recovery prompt was open." }}) },
                            "relocate_recovery" => { let reloc = readiness_exec::relocate_recovery_files(); json!({"success": true, "relocated": reloc, "_hint": "Files are moved to ~/.adom/recovery/fusion/<stamp>/, never deleted. fusion_start does this by itself before every launch."}) },
                            _ => { let mut r = lifecycle::stop(&ctx, &parsed["args"]); if r.is_object() { r["aliasOf"] = json!("fusion_stop"); } r },
                        };
                        r["shell"] = json!("rust"); r
                    },
                    v @ ("fusion_stop" | "stop" | "fusion_kill" | "kill" | "fusion_restart_for_addin" | "restart_for_addin") if addin_owner_is_exe(&**h) => {
                        let (sc, ca) = (sidecar.clone(), caller.clone()); let side = move |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t);
                        let sd = h.bridge_state_dir(); let dlg = |r: Value| r;
                        let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: sd.clone(), caller: caller.clone(), pids: h.fusion_processes().iter().map(|p| p.pid).collect(), sidecar: &side, dialogs: &dlg };
                        let cmd = v.strip_prefix("fusion_").unwrap_or(v);
                        let t0 = Instant::now();
                        let mut r = match cmd { "stop" => lifecycle::stop(&ctx, &parsed["args"]), "kill" => lifecycle::kill(&ctx, &parsed["args"]), _ => lifecycle::restart_for_addin(&ctx, &parsed["args"], &|verb, a| addin::pass_through(verb, &a, &ctx)) };
                        timings::log_run(&sd, cmd, t0.elapsed().as_secs_f64(), r["success"].as_bool() != Some(false));
                        r["shell"] = json!("rust"); r },
                    "fusion_get_preferences" | "get_preferences" if addin_owner_is_exe(&**h) => { let (sc, ca) = (sidecar.clone(), caller.clone()); let side = move |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t); let sd = h.bridge_state_dir(); let dlg = |r: Value| r; let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: sd.clone(), caller: caller.clone(), pids: h.fusion_processes().iter().map(|p| p.pid).collect(), sidecar: &side, dialogs: &dlg }; let _ = &ctx; let mut r = fusion_prefs::get_preference_key(&parsed["args"], &|v, a, t| addin::addin_http(v, &a, t.max(20))); r["shell"] = json!("rust"); r },
                    "fusion_set_preference" | "set_preference" if addin_owner_is_exe(&**h) => { let (sc, ca) = (sidecar.clone(), caller.clone()); let side = move |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t); let sd = h.bridge_state_dir(); let dlg = |r: Value| r; let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: sd.clone(), caller: caller.clone(), pids: h.fusion_processes().iter().map(|p| p.pid).collect(), sidecar: &side, dialogs: &dlg }; let mut r = fusion_prefs::set_preference(&parsed["args"], &|v, a, _t| addin::pass_through(v, &a, &ctx)); r["shell"] = json!("rust"); r },
                    "fusion_reset_workspace" | "reset_workspace" if addin_owner_is_exe(&**h) => { let (sc, ca) = (sidecar.clone(), caller.clone()); let side = move |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t); let sd = h.bridge_state_dir(); let dlg = |r: Value| r; let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: sd.clone(), caller: caller.clone(), pids: h.fusion_processes().iter().map(|p| p.pid).collect(), sidecar: &side, dialogs: &dlg }; let mut r = fusion_prefs::reset_workspace(&sd, &parsed["args"], &|v, a| addin::pass_through(v, &a, &ctx), &|v, a, _t| addin::pass_through(v, &a, &ctx)); r["shell"] = json!("rust"); r },
                    "fusion_describe" | "describe" => {
                        let c: Value = serde_json::from_str(fusion_core::contract::FROZEN).unwrap_or(json!({}));
                        let verbs = c["verbs"].clone(); let n = verbs.as_array().map(|a| a.len()).unwrap_or(0);
                        json!({"success": true, "bridge": "fusion360", "verbCount": n, "verbs": verbs, "data": {"bridge": "fusion360", "verbCount": n, "verbs": c["verbs"]}, "shell": "rust",
                            "contribute": {"source": "https://wiki.adom.inc/adom/fusion-bridge (the wiki page is the repo: adom-wiki repo clone adom/fusion-bridge)", "mirror": "a GitHub mirror under adom-inc, linked from the page; the wiki repo is canonical",
                                "issues": "adom-wiki issue create adom/fusion-bridge --category bug-report (include the reply JSON)", "pullRequests": "adom-wiki branch create / repo push --branch / pr create --from",
                                "skill": "fusion-contributing", "note": "A verb you had to work around is a bug report or a PR; both are welcome and merge fast with evidence."}}) },
                    "fusion_close_window" | "close_window" if addin_owner_is_exe(&**h) => { let mut r = small_verbs::close_window(&parsed["args"], &caller); r["shell"] = json!("rust"); r },
                    "fusion_screenshot_fusion" | "screenshot_fusion" if addin_owner_is_exe(&**h) => { let mut r = small_verbs::screenshot_fusion(&parsed["args"], &caller); r["shell"] = json!("rust"); r },
                    "fusion_dismiss_blocking_dialogs" | "dismiss_blocking_dialogs" if addin_owner_is_exe(&**h) => {
                        let pids: Vec<u32> = h.fusion_processes().iter().map(|p| p.pid).collect();
                        let (sc, ca) = (sidecar.clone(), caller.clone());
                        let mut r = small_verbs::dismiss_blocking_dialogs(&h.bridge_state_dir(), &parsed["args"], &caller, &pids, &|pct| timings::taskbar_pct(&ca, pct)); let _ = &sc;
                        r["shell"] = json!("rust"); r },
                    "fusion_notify_owner" | "notify_owner" => { let mut r = small_verbs::notify_owner(&h.bridge_state_dir(), &parsed["args"], &caller); r["shell"] = json!("rust"); r },
                    "fusion_cancel_run" | "cancel_run" if chains_owner_is_exe(&**h) => { let (sc, ca) = (sidecar.clone(), caller.clone()); let mut r = small_verbs::cancel_run(&parsed["args"], &|b| { let _ = addin::addin_http("activity_banner", &b, 20); }); let _ = (&sc, &ca); r["shell"] = json!("rust"); r },
                    "fusion_set_auto_update" | "set_auto_update" => { let mut r = small_verbs::set_auto_update(&h.bridge_state_dir(), &parsed["args"]); r["shell"] = json!("rust"); r },
                    "fusion_set_seat_takeover" | "set_seat_takeover" => { let mut r = small_verbs::set_seat_takeover(&h.bridge_state_dir(), &parsed["args"]); r["shell"] = json!("rust"); r },
                    "fusion_set_window_sizing" | "set_window_sizing" => { let mut r = small_verbs::set_window_sizing(&h.bridge_state_dir(), &parsed["args"]); r["shell"] = json!("rust"); r },
                    "fusion_set_driving_badge" | "set_driving_badge" => { let (sc, ca) = (sidecar.clone(), caller.clone()); let mut r = small_verbs::set_driving_badge(&h.bridge_state_dir(), &parsed["args"], &|| { if !etiquette::clear_badge(&ca) && !addin_owner_is_exe(&**h) { let _ = proxy_with_timeout(&sc, &json!({"command": "set_driving_badge", "args": {"enabled": false}}).to_string(), &ca, 20); } }); r["shell"] = json!("rust"); r },
                    "fusion_addin_status" | "addin_status" => { let mut r = small_verbs::addin_status(!h.fusion_processes().is_empty()); r["shell"] = json!("rust"); r },
                    "fusion_plan_library_import" | "plan_library_import" => {
                        let installed = h.fusion_executable().is_some(); let running = !h.fusion_processes().is_empty();
                        let addin_ok = running && ureq::post("http://127.0.0.1:8774/command").timeout(Duration::from_secs(6)).send_json(json!({"command": "get_app_state", "args": {}})).ok().and_then(|r| r.into_json::<Value>().ok()).map(|v| v["success"].as_bool().unwrap_or(false)).unwrap_or(false);
                        let mut r = small_verbs::plan_library_import(&h.bridge_state_dir(), installed, running, addin_ok, &parsed["args"]); r["shell"] = json!("rust"); r },
                    "fusion_readiness" | "readiness" => {
                        let mut r = proxy(&sidecar, &raw, &caller);
                        let native = shadow_readiness(&**h, &caller);
                        if let Some(obj) = r.as_object_mut() {
                            let keys = ["installed", "installing", "running", "ready", "needsSignin", "licensingDialog", "updating", "addinStale", "addinVersion", "lastReadyAt", "lastSignedInAt", "_hint", "statusVerb", "kicadIngest"];
                            let diff: Vec<String> = keys.iter().filter(|k| obj.get(**k) != native.get(**k)).map(|k| k.to_string()).collect();
                            obj.insert("nativeReadiness".into(), json!({"result": native, "shadowDiff": diff}));
                        }
                        r
                    }
                    // Native since Phase 2: Fusion's own alert toasts from its AppLogFile, the
                    // same answer the Python bridge gives (parser golden-tested against it).
                    v @ ("fusion_alerts" | "alerts" | "fusion_log_tail" | "log_tail") => {
                        let since = parsed["args"]["sinceSec"].as_f64().unwrap_or(300.0).max(1.0);
                        let mut a = parsed["args"].clone();
                        if v.ends_with("log_tail") && a["sinceOffset"].is_null() { a["sinceOffset"] = json!(0); }
                        native_alerts_args(&**h, since, &a)
                    }
                    // Native since Phase 2: the background-action ledger, a file the bridge appends.
                    "fusion_action_ledger" | "action_ledger" => native_action_ledger(&**h, &parsed["args"]),
                    // Native since Phase 2: the measured run-time report from the verb log.
                    "fusion_verb_times" | "verb_times" => native_verb_times(&**h, parsed["args"]["limit"].as_u64().unwrap_or(40) as usize),
                    // Native since Phase 2: offline library checks on the golden-tested .lbr engine.
                    "fusion_library_parts" | "library_parts" => native_library_parts(&parsed["args"]),
                    "fusion_export_library_part" | "export_library_part" => native_export_library_part(&parsed["args"]),
                    "fusion_edit_footprint" | "edit_footprint" => native_edit_footprint(&parsed["args"]),
                    v @ ("fusion_create_footprint" | "create_footprint" | "fusion_create_symbol" | "create_symbol" | "fusion_read_symbol" | "read_symbol" | "fusion_find_symbol" | "find_symbol"
                        | "fusion_copy_footprint" | "copy_footprint" | "fusion_rename_footprint" | "rename_footprint" | "fusion_remove_footprint" | "remove_footprint" | "fusion_xcopy_footprint" | "xcopy_footprint"
                        | "fusion_clone_symbol" | "clone_symbol" | "fusion_rename_symbol" | "rename_symbol" | "fusion_remove_symbol" | "remove_symbol" | "fusion_set_params" | "set_params" | "fusion_footprint_from_land_pattern" | "footprint_from_land_pattern" | "fusion_set_models" | "set_models" | "fusion_add_models" | "add_models" | "fusion_symbol_prims" | "symbol_prims" | "fusion_set_visibility" | "set_visibility" | "fusion_add_symbol_art" | "add_symbol_art" | "fusion_lint_library" | "lint_library" | "fusion_plate_vias" | "plate_vias" | "fusion_cleanup_libs" | "cleanup_libs") => native_lib_authoring(v.strip_prefix("fusion_").unwrap_or(v), &parsed["args"]),
                    "fusion_preflight_lbr" | "preflight_lbr" => native_preflight_lbr(&parsed["args"]),
                    "fusion_not_supported" | "not_supported" => unsupported::answer(&parsed["args"]),
                    "fusion_list_library" | "list_library" => { let mut r = stdshapes::list_library(&parsed["args"]); r["shell"] = json!("rust"); r },
                    "fusion_panel" | "panel" if addin_owner_is_exe(&**h) => { let (sc, ca) = (sidecar.clone(), caller.clone()); let side = move |verb: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": verb, "args": a}).to_string(), &ca, t); let dlg = |r: Value| r; let ctx = addin::Ctx { host: &**h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: h.bridge_state_dir(), caller: caller.clone(), pids: h.fusion_processes().iter().map(|p| p.pid).collect(), sidecar: &side, dialogs: &dlg }; let pass = |verb: &str, a: Value| addin::pass_through(verb, &a, &ctx); let mut r = appverbs::panel(&parsed["args"], &pass); r["shell"] = json!("rust"); r },
                    // Native since Phase 2: the raw authenticated APS GET (never-charge guarded).
                    "fusion_aps_get" | "aps_get" => native_aps_get(&parsed["args"]),
                    // Native since Phase 2: cloud discovery on the Data Management API.
                    "fusion_aps_browse" | "aps_browse" => native_aps_browse(&parsed["args"]),
                    "fusion_aps_search" | "aps_search" => native_aps_search(&parsed["args"]),
                    "fusion_aps_recent" | "aps_recent" => native_aps_recent(&parsed["args"]),
                    "fusion_aps_file_info" | "aps_file_info" => native_aps_file_info(&parsed["args"], false),
                    "fusion_aps_versions" | "aps_versions" => native_aps_file_info(&parsed["args"], true),
                    "fusion_aps_download" | "aps_download" => native_aps_download(&parsed["args"]),
                    "fusion_aps_create_folder" | "aps_create_folder" => native_aps_create_folder(&parsed["args"]),
                    "fusion_aps_upload" | "aps_upload" => native_aps_upload(&parsed["args"]),
                    "fusion_aps_status" | "aps_status" => native_aps_status(&sidecar, &caller),
                    "fusion_aps_set_client_id" | "aps_set_client_id" => { let mut r = aps_signin::set_client_id(&parsed["args"]); r["shell"] = json!("rust"); r },
                    "fusion_aps_set_browser" | "aps_set_browser" => { let mut r = aps_signin::set_browser(&parsed["args"]); r["shell"] = json!("rust"); r },
                    "fusion_aps_get_browser" | "aps_get_browser" => { let mut r = aps_signin::get_browser(); r["shell"] = json!("rust"); r },
                    "fusion_aps_forget_browser" | "aps_forget_browser" => { let mut r = aps_signin::forget_browser(); r["shell"] = json!("rust"); r },
                    "fusion_open_by_urn_note" | "open_by_urn_note" => json!({"success": true, "shell": "rust", "_hint": "See fusion_open_by_urn: a board is NOT a standalone file - open the electronics DESIGN document by URN, then fusion_show_2d_board / fusion_show_3d_board for its views."}),
                    "shell_info" => json!({"success": true, "shell": "rust", "bridgeVersion": VERSION,
                                          "contractFrozen": catalog.frozen,
                                          "_hint": "Rust plumbing shell; every catalogued verb is served, natively or via the Python sidecar."}),
                    _ => proxy(&sidecar, &raw, &caller),
                };
                // every failure points at the source: the bridge is open, the wiki page is the
                // repo, and a workaround an AI had to invent is a contribution (John, 2026-09-19)
                let mut reply = reply;
                if verb != asked && reply.is_object() { reply["aliasOf"] = json!(verb); }
                if reply.is_object() && reply["success"].as_bool() == Some(false) && reply["_contribute"].is_null() {
                    reply["_contribute"] = json!("If this is a bridge defect or a gap, the source is open: adom-wiki repo clone adom/fusion-bridge (the wiki page is the repo; a GitHub mirror is linked from the page). File it with this reply JSON (adom-wiki issue create adom/fusion-bridge) or fix it and send a PR (adom-wiki pr create). The fusion-contributing skill has the steps, including trying your build on your own box behind a dev pin.");
                }
                let _ = req.respond(json_response(200, &reply));
            }
            _ => { let _ = req.respond(json_response(404, &json!({"error": "not found"}))); }
        }
        }));
        if outcome.is_err() { eprintln!("[fusion-bridge] a handler panicked on {path}; the shell stays up"); }
        });
    }
}

/// Forward an unported verb to the Python sidecar, identity headers intact.
fn proxy(sidecar: &Arc<Mutex<Option<Sidecar>>>, raw: &str, caller: &Caller) -> Value { proxy_with_timeout(sidecar, raw, caller, 600) }

/// Verb names older skills and transcripts still use. The Python sidecar owned them until
/// 1.10.0; each answers with the living replacement so a caller is never left at a dead end.
fn retired_verb(verb: &str) -> Option<Value> {
    let v = verb.strip_prefix("fusion_").unwrap_or(verb);
    let (instead, how): (&str, &str) = match v {
        "send_key" => ("desktop_press_key (an ab verb)", "adom-bridge desktop_press_key {hwnd, key}. For a DIALOG you rarely need a key at all: the bridge reads dialogs through UI Automation (fusion_check_dialogs or fusion_state.dialogs[]) and expires unknown ones by itself before the next add-in verb; a named button is desktop_ui_click {hwnd, name, role:\"button\"}."),
        "click_fusion" | "click" => ("desktop_click (an ab verb)", "adom-bridge desktop_screenshot_window {hwnd} then desktop_click {hwnd, space:\"image\", shotId, x, y, cursor:false}: image-space coordinates from the shot, no math, no foreground. Prefer desktop_ui_click {hwnd, name} whenever the control has an accessible name. fusion_state gives the main hwnd."),
        "screenshot_all" => ("fusion_state", "fusion_state returns every dialog (title, text, buttons, hwnd) plus the main window capture in one read; fusion_screenshot_fusion is the main window alone."),
        "window_info" => ("fusion_state", "fusion_state carries hwnd, title, rect, dialogs[] and the focus events."),
        "detect" => ("fusion_readiness", "fusion_readiness reports installed / running / ready / needsSignin without launching anything."),
        "open_symbol" => ("fusion_show_symbol", "fusion_show_symbol {libraryPath, deviceset, capture:true}."),
        "schematic_info" => ("fusion_electronics_query", "fusion_electronics_query {entity:\"Part\"} (or Sheet, Net, Instance) reads the active schematic as typed rows; fusion_document_info says which document is active."),
        "dark_mode" => ("fusion_set_preference", "fusion_set_preference {theme:\"dark\"} applies live, no restart."),
        "aps_permissions" => ("fusion_aps_status", "fusion_aps_status reports sign-in, scopes and the hub it can see."),
        "mcp_execute" | "mcp_update" | "mcp_electronics_read" => ("fusion_mcp_call", "These are Autodesk MCP TOOL names, not bridge verbs: fusion_mcp_call {name:\"fusion_mcp_execute\", arguments:{...}} (fusion_mcp_tools lists them; fusion_electronics_query wraps electronics_read)."),
        "signout" | "export_molecule_bundle" => ("(no replacement)", "This verb was retired with the Python sidecar and has no exe equivalent yet. If you need it, the source is open: see the fusion-contributing skill, or file an issue on the fusion-bridge wiki page."),
        _ => return None,
    };
    Some(json!({"success": false, "errorCode": "verb_retired", "verb": verb, "useInstead": instead,
        "error": format!("'{verb}' was retired when the exe replaced the Python sidecar (1.10.0). Use {instead}."),
        "_hint": how}))
}

fn verb_of(raw: &str) -> String { serde_json::from_str::<Value>(raw).ok().and_then(|v| v["command"].as_str().or(v["verb"].as_str()).map(String::from)).unwrap_or_else(|| "?".into()) }

fn proxy_with_timeout(sidecar: &Arc<Mutex<Option<Sidecar>>>, raw: &str, caller: &Caller, timeout_secs: u64) -> Value {
    // ab routes the first verb the moment /status is 2xx, which is before the sidecar has
    // bound its port: wait out the sidecar's startup (bounded) instead of failing that verb.
    if let Some(code) = sidecar.lock().unwrap().as_mut().and_then(|s| s.exited()) {
        return json!({"success": false, "errorCode": "sidecar_exited", "error": format!("The Python sidecar exited with code {code}."),
                      "_hint": "Read ~/.adom/fusion-bridge/sidecar.log for the traceback, then bridge_kill {name:'fusion360'} so ab respawns the shell and its sidecar."});
    }
    let (url, young) = match sidecar.lock().unwrap().as_ref() { Some(s) => { *launch::SIDECAR_URL.lock().unwrap() = Some(s.url("/command")); (s.url("/command"), s.started.elapsed() < Duration::from_secs(45)) }, None => { if let Some(r) = retired_verb(&verb_of(raw)) { return r; } return json!({
        "success": false, "errorCode": "sidecar_absent", "verb": verb_of(raw),
        "error": format!("No handler in this exe owns the verb '{}', and there is no Python sidecar to fall back to (the exe ships alone since 1.10.0).", verb_of(raw)),
        "_hint": "This is a verb-name problem, not a Fusion or add-in problem: check the name against fusion_describe (the frozen contract) and do not restart Fusion or diagnose the add-in for it. If the name IS in the contract, file it on the fusion-bridge wiki page: a listed verb reached the fallback."})  } };
    let mut r = ureq::post(&url).timeout(Duration::from_secs(timeout_secs)).set("Content-Type", "application/json");
    if !caller.thread.is_empty() { r = r.set("X-Adom-Caller-Thread", &caller.thread); }
    if !caller.container.is_empty() { r = r.set("X-Adom-Caller-Container", &caller.container); }
    if !caller.reason.is_empty() { r = r.set("X-Adom-Caller-Reason", &caller.reason); }
    if young {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(45) {
            let base = url.trim_end_matches("/command").to_string();
            if ureq::get(&format!("{base}/status")).timeout(Duration::from_secs(2)).call().is_ok() { break; }
            std::thread::sleep(Duration::from_millis(400));
        }
    }
    match r.send_string(raw) {
        Ok(resp) => resp.into_json::<Value>().unwrap_or(json!({"success": false, "error": "sidecar returned non-JSON"})),
        Err(ureq::Error::Status(code, resp)) => resp.into_json::<Value>().unwrap_or(json!({"success": false, "error": format!("sidecar HTTP {code}")})),
        Err(e) => json!({"success": false, "errorCode": "sidecar_unreachable", "error": e.to_string(),
                         "_hint": "The Python sidecar beside adom-fusion-bridge.exe is not answering. Read ~/.adom/fusion-bridge/sidecar.log; if ab's portable python is missing (~/.adom/adom-runtimes/python-*), install any python bridge (kicad) once so ab provisions it."}),
    }
}


/// `fusion_alerts` served in-process. Fusion draws many errors as in-canvas CEF overlays with
/// no top-level window, so dialog enumeration cannot see them; its AppLogFile can. The log's
/// stamps are LOCAL time, so the cutoff is computed in the same convention (the Python bridge
/// compares `strptime(...).timestamp()` with `time.time()`, which is the same thing).
pub(crate) fn native_alerts(h: &dyn HostBoth, since_sec: f64) -> Value { native_alerts_args(h, since_sec, &Value::Null) }

/// The standard's log_tail on Fusion's AppLogFile: with sinceOffset, every line written since that
/// byte offset (errorsOnly: only error lines and alert toasts), at most maxBytes, and nextOffset to
/// pass next time. A log shorter than the offset was replaced by a new session's, so it reads from 0.
/// A log line's level as the standard's log_tail names it.
fn log_level(l: &str) -> &'static str {
    let low = l.to_lowercase();
    if l.contains("New Alert message") { "alert" } else if low.contains("error") || low.contains("exception") || low.contains("fail") { "error" } else if low.contains("warn") { "warning" } else { "info" }
}

fn log_tail_text(path: &std::path::Path, since: u64, max: u64, errors_only: bool) -> (Vec<String>, u64, bool) {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return (vec![], since, false) };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let rotated = since > size;
    let start = if rotated { 0 } else { since };
    let end = size.min(start + max);
    let mut buf = vec![0u8; (end - start) as usize];
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut buf).is_err() { return (vec![], start, rotated); }
    // stop at the last whole line so the next read starts on one
    let cut = if end < size { buf.iter().rposition(|b| *b == b'\n').map(|i| i + 1).unwrap_or(0) } else { buf.len() };
    let text = String::from_utf8_lossy(&buf[..cut]).to_string();
    let lines = text.lines().filter(|l| !l.trim().is_empty()).filter(|l| !errors_only || log_level(l) != "info").map(|l| l.chars().take(600).collect()).collect();
    (lines, start + cut as u64, rotated)
}

pub(crate) fn native_alerts_args(h: &dyn HostBoth, since_sec: f64, args: &Value) -> Value {
    let hint = "Fusion draws many errors as in-canvas CEF overlays with NO top-level window, so check_dialogs alone cannot see them; these come from Fusion's own AppLogFile 'New Alert message' entries. An empty list means Fusion raised no toast in the window - not that a command succeeded.";
    let Some(dir) = h.fusion_log_dir() else {
        return json!({"success": true, "alerts": [], "lines": [], "nextOffset": 0, "logFile": Value::Null, "shell": "rust",
                      "diagnostics": {"why": "no AppLogFile*.log found under any base"}, "_hint": hint});
    };
    // newest AppLogFile*.log in that dir
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("AppLogFile") && n.ends_with(".log") {
                let m = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                if best.as_ref().map(|b| m > b.0).unwrap_or(true) { best = Some((m, e.path())); }
            }
        }
    }
    let Some((_, path)) = best else {
        return json!({"success": true, "alerts": [], "logFile": Value::Null, "shell": "rust",
                      "diagnostics": {"baseFound": dir.to_string_lossy(), "why": "no AppLogFile*.log found under any base"}, "_hint": hint});
    };
    const TAIL: u64 = 400_000;
    let text = (|| -> std::io::Result<String> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(&path)?;
        let size = f.metadata()?.len();
        let mut buf = Vec::new();
        if size > TAIL { f.seek(SeekFrom::Start(size - TAIL))?; }
        f.read_to_end(&mut buf)?;
        let mut s = String::from_utf8_lossy(&buf).to_string();
        if size > TAIL { if let Some(i) = s.find('\n') { s = s[i + 1..].to_string(); } }
        Ok(s)
    })().unwrap_or_default();
    let now_local_as_utc = chrono::Utc::now().timestamp() + chrono::Local::now().offset().local_minus_utc() as i64;
    let cutoff = now_local_as_utc - since_sec as i64;
    let log_file = path.to_string_lossy().replace('\\', "/");
    if let Some(off) = args["sinceOffset"].as_u64() {
        let max = args["maxBytes"].as_u64().unwrap_or(200_000).clamp(1_000, 4_000_000);
        let (lines, next, rotated) = log_tail_text(&path, off, max, args["errorsOnly"].as_bool() == Some(true));
        let alerts = fusion_core::alerts::parse_alert_lines(lines.iter().map(|s| s.as_str()), i64::MIN);
        let truncated = std::fs::metadata(&path).map(|m| m.len() > next).unwrap_or(false);
        let entries: Vec<Value> = lines.iter().map(|l| json!({"level": log_level(l), "text": l})).collect();
        return json!({"success": true, "mutated": false, "entries": entries, "next": next, "truncated": truncated, "lines": lines, "nextOffset": next, "rotated": rotated, "alerts": alerts, "logFile": log_file, "shell": "rust",
                      "_hint": "Fusion's own log since sinceOffset (the standard's log_tail). Pass nextOffset next time; rotated:true means Fusion started a new log, so this read began at 0. alerts are the toasts among the lines."});
    }
    let alerts = fusion_core::alerts::parse_alert_lines(text.lines(), cutoff);
    let next = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    json!({"success": true, "alerts": alerts, "logFile": log_file, "nextOffset": next, "shell": "rust",
           "diagnostics": {"baseFound": dir.to_string_lossy()}, "_hint": hint})
}


#[cfg(test)]
mod log_tail_tests {
    #[test]
    fn a_cursor_reads_each_line_once_and_restarts_on_a_new_log() {
        let p = std::env::temp_dir().join(format!("adom-logtail-{}.log", std::process::id()));
        std::fs::write(&p, "20261030T120000 start\n20261030T120001 New Alert message. Short Message: Bad thing\n20261030T120002 error: x\n").unwrap();
        let (all, next, rot) = super::log_tail_text(&p, 0, 400_000, false);
        assert_eq!((all.len(), rot), (3, false));
        let (none, n2, _) = super::log_tail_text(&p, next, 400_000, false);
        assert!(none.is_empty() && n2 == next);
        let (errs, _, _) = super::log_tail_text(&p, 0, 400_000, true);
        assert_eq!(errs.len(), 2);
        assert_eq!((super::log_level("x New Alert message. y"), super::log_level("a warning"), super::log_level("ok")), ("alert", "warning", "info"));
        let (part, n3, _) = super::log_tail_text(&p, 0, 30, false);
        assert_eq!((part.len(), n3), (1, 22)); // only the whole first line
        let (again, _, rot) = super::log_tail_text(&p, 10_000, 400_000, false);
        assert!(rot && again.len() == 3);
        let _ = std::fs::remove_file(&p);
    }
}

/// `fusion_action_ledger`: READ-ONLY view of `<state>/actions.jsonl`, every background action
/// the bridge took on the host app (update clicks, seat takeovers, dialog dismissals, doc
/// closes, WM_CLOSEs), so a caller on a relay is not blind to it. Same filters and shape as
/// the Python handler: {limit (default 50, max 500), sinceSeconds, action (substring)}.
fn native_action_ledger(h: &dyn HostBoth, args: &Value) -> Value {
    let p = h.bridge_state_dir().join("actions.jsonl");
    let hint = "Every background action the bridge takes on the host app - update clicks and deferrals, seat takeovers, dialog dismissals, doc closes, hang-gate kills, WM_CLOSEs (with window class+size) - appended live, survives respawns. Filter: {action:'wm_close', sinceSeconds:3600}.";
    if !p.is_file() {
        return json!({"success": true, "entries": [], "count": 0, "shell": "rust",
                      "_hint": "Ledger empty: the bridge has taken no background actions on this box since 1.9.177 (or none since the file was last cleared)."});
    }
    let limit = args["limit"].as_u64().unwrap_or(50).clamp(1, 500) as usize;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let cutoff = args["sinceSeconds"].as_f64().map(|s| now - s);
    let want = args["action"].as_str().unwrap_or("").to_lowercase();
    let text = match std::fs::read_to_string(&p) { Ok(t) => t, Err(e) => return json!({"success": false, "error": format!("ledger unreadable: {e}")}) };
    let mut out: Vec<Value> = Vec::new();
    for line in text.lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        if let Some(c) = cutoff { if e["epoch"].as_f64().unwrap_or(0.0) < c { continue; } }
        if !want.is_empty() && !e["action"].as_str().unwrap_or("").to_lowercase().contains(&want) { continue; }
        out.push(e);
    }
    let count = out.len();
    let tail: Vec<Value> = out.into_iter().rev().take(limit).collect::<Vec<_>>().into_iter().rev().collect();
    json!({"success": true, "entries": tail, "count": count, "shell": "rust", "_hint": hint})
}


/// Python's `sorted(s)[int(round(q*(n-1)))]`: note `round` there is half-to-even.
fn pct(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() { return None; }
    let n = sorted.len() as f64;
    let i = (q * (n - 1.0)).round_ties_even() as i64;
    Some(sorted[i.clamp(0, sorted.len() as i64 - 1) as usize])
}

/// `fusion_verb_times`: per-verb count / p50 / p90 / max / failures aggregated from
/// `<state>/verb-times.jsonl` (every run with its outcome), sorted by p90 descending,
/// the same numbers `timings.verb_report` produces.
fn native_verb_times(h: &dyn HostBoth, limit: usize) -> Value {
    let p = h.bridge_state_dir().join("verb-times.jsonl");
    let hint = "Aggregated from ~/.adom/fusion-bridge/verb-times.jsonl (every run, with its outcome). p90 is the useful column when hunting a slow surface; `failures` next to a healthy p50 usually means a verb that fails FAST, which is a different bug from one that is slow.";
    let mut runs: Vec<(String, Vec<f64>, u64, f64)> = Vec::new(); // insertion order like a dict; last run's seconds
    let mut since: Option<i64> = None;
    if let Ok(text) = std::fs::read_to_string(&p) {
        for line in text.lines() {
            let Ok(r) = serde_json::from_str::<Value>(line) else { continue };
            if since.is_none() { since = r["at"].as_i64(); }
            let verb = r["verb"].as_str().unwrap_or("?").to_string();
            let secs = r["seconds"].as_f64().unwrap_or(0.0);
            let ok = r["ok"].as_bool().unwrap_or(false);
            match runs.iter_mut().find(|(v, _, _, _)| *v == verb) {
                Some(e) => { e.1.push(secs); if !ok { e.2 += 1; } e.3 = secs; }
                None => runs.push((verb, vec![secs], if ok { 0 } else { 1 }, secs)),
            }
        }
    }
    let mut rows: Vec<(String, Value, f64)> = runs.into_iter().map(|(verb, mut secs, fail, last)| {
        secs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p90 = pct(&secs, 0.9);
        let row = json!({"verb": verb, "count": secs.len(), "p50Sec": pct(&secs, 0.5), "p90Sec": p90, "maxSec": secs.last().copied(), "lastSec": last, "failures": fail});
        (verb, row, p90.unwrap_or(0.0))
    }).collect();
    rows.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal)); // stable, like Python's sort
    // the standard's verb_times: verbs is the list, slowest p90 first; byVerb keeps the older map
    let mut by = serde_json::Map::new();
    let mut list = Vec::new();
    for (verb, row, _) in rows.into_iter().take(limit) {
        by.insert(verb, json!({"count": row["count"], "p50": row["p50Sec"], "p90": row["p90Sec"], "max": row["maxSec"], "failures": row["failures"]}));
        list.push(row);
    }
    let count = list.len();
    let since = since.and_then(|s| chrono::DateTime::from_timestamp(s, 0)).map(|d| d.to_rfc3339());
    json!({"success": true, "mutated": false, "verbs": list, "byVerb": by, "since": since, "count": count, "shell": "rust", "_hint": hint})
}


fn lbr_path_arg(args: &Value, keys: &[&str]) -> String {
    keys.iter().find_map(|k| args[*k].as_str().filter(|v| !v.is_empty())).unwrap_or("").replace('\\', "/")
}

/// `fusion_library_parts`: every ORDERABLE variant in a .lbr with its part numbers, plus the
/// EE-grade warnings. Offline, no Fusion. Same shape as the Python handler.
fn native_library_parts(args: &Value) -> Value {
    let path = lbr_path_arg(args, &["libraryPath", "lbrPath", "filePath", "path"]);
    if path.is_empty() {
        return json!({"success": false, "errorCode": "no_path_argument", "error": "No library path given; the file was never examined.",
                      "acceptedArgs": ["libraryPath", "lbrPath", "filePath", "path"], "shell": "rust"});
    }
    let xml = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": e.to_string(), "libraryPath": path, "shell": "rust"}),
    };
    let md = match fusion_lbr::library_part_metadata(&xml) {
        Ok(m) => m,
        Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": e.to_string(), "libraryPath": path, "shell": "rust"}),
    };
    let lint = fusion_lbr::lint_part_metadata(&xml);
    let t = &md.totals;
    let tail = if lint.ok { "No metadata warnings.".to_string() } else {
        format!("{} warning(s) in `warnings` - these do NOT stop the library opening, but an EE cannot order from it as-is.", lint.warnings.len()) };
    json!({"success": true, "libraryPath": path, "devicesets": md.devicesets, "totals": t,
           "metadataOk": lint.ok, "warnings": lint.warnings,
           "attributeNames": ["MPN", "MANUFACTURER", "MOUSER_PN", "DIGIKEY_PN", "LCSC_PN"], "shell": "rust",
           "_hint": format!("{} deviceset(s), {} orderable variant(s); {} carry an MPN and {} a distributor PN. {} The full orderable identity is deviceset + device + technology (e.g. TJA1050 + T + -AUTO), because EAGLE puts package variants on <device> and grade/temperature variants on <technology> - which is how two variants share geometry and remain different parts.",
                            t.devicesets, t.variants, t.with_mpn, t.with_distributor, tail)})
}

/// The standard's export_library_part: one part of a .lbr into its own small library (its
/// symbols, its packages and its deviceset), offline. A Fusion library holds symbol and footprint
/// together, so both answers name the one file; 3D packages live in the cloud and are named by URN.
fn native_export_library_part(args: &Value) -> Value {
    let path = lbr_path_arg(args, &["libraryPath", "lbrPath", "filePath"]);
    let part = args["part"].as_str().unwrap_or("");
    let ds = part.rsplit(':').next().unwrap_or(part).to_string();
    if path.is_empty() || ds.is_empty() {
        return json!({"success": false, "errorCode": "bad_args", "error": "libraryPath and part (the deviceset's name, or Lib:Name) are required", "shell": "rust"});
    }
    let xml = match std::fs::read_to_string(&path) { Ok(t) => t, Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": e.to_string(), "libraryPath": path, "shell": "rust"}) };
    let tmp = std::env::temp_dir().join(format!("adom-export-part-{}", std::process::id()));
    let split = match fusion_lbr::split_per_part(&xml, &tmp) { Ok(m) => m, Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": e.to_string(), "shell": "rust"}) };
    let key = match args["footprint"].as_str().filter(|f| !f.is_empty()) { Some(f) => format!("{ds}::{f}"), None => ds.clone() };
    let Some(entry) = split.get(&key).or_else(|| split.iter().find(|(k, _)| k.eq_ignore_ascii_case(&key)).map(|(_, v)| v)) else {
        let names: Vec<&String> = split.keys().filter(|k| !k.contains("::")).collect();
        let _ = std::fs::remove_dir_all(&tmp);
        return json!({"success": false, "errorCode": "part_not_found", "error": format!("no part {key} in the library"), "parts": names, "shell": "rust"});
    };
    let out_dir = args["outputDir"].as_str().map(|s| s.replace('\\', "/")).unwrap_or_else(|| std::path::Path::new(&path).parent().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default());
    let _ = std::fs::create_dir_all(&out_dir);
    let file = std::path::Path::new(&entry.lbr_path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| format!("{ds}.lbr"));
    let dest = format!("{}/{}", out_dir.trim_end_matches('/'), file);
    let copied = std::fs::copy(&entry.lbr_path, &dest);
    let _ = std::fs::remove_dir_all(&tmp);
    if let Err(e) = copied { return json!({"success": false, "errorCode": "write_failed", "error": e.to_string(), "path": dest, "shell": "rust"}); }
    let models: Vec<Value> = entry.packages.iter().filter_map(|pk| fusion_lbr::read_footprint(&xml, None, pk).ok()).flat_map(|f| f["models"].as_array().cloned().unwrap_or_default()).collect();
    json!({"success": true, "shell": "rust", "exports": {"symbol": dest, "footprint": dest, "model": Value::Null, "library": dest},
           "symbols": entry.symbols, "packages": entry.packages, "models": models, "missing": ["model"],
           "_hint": "One .lbr with the part's symbols, packages and deviceset (a Fusion library keeps symbol and footprint together). 3D packages are cloud items, named in models by URN, so no model file is written: fusion_show_3d_package opens one to export it."})
}

/// The standard's edit_footprint on a .lbr, offline: the edit is made, the result is checked with
/// the same preflight as fusion_preflight_lbr, and only then written in place (the old file kept
/// beside it as .bak). A failing preflight writes nothing.
fn native_edit_footprint(args: &Value) -> Value {
    let path = lbr_path_arg(args, &["libraryPath", "lbrPath", "filePath"]);
    let name = args["name"].as_str().unwrap_or("");
    if path.is_empty() || name.is_empty() { return json!({"success": false, "errorCode": "bad_args", "error": "libraryPath and name (the footprint) are required", "shell": "rust"}); }
    let xml = match std::fs::read_to_string(&path) { Ok(t) => t, Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": e.to_string(), "libraryPath": path, "shell": "rust"}) };
    let (text, summary) = match fusion_lbr::edit_footprint(&xml, name, args) { Ok(r) => r, Err(e) => return json!({"success": false, "errorCode": "edit_refused", "error": e, "shell": "rust"}) };
    let after = fusion_lbr::read_footprint(&text, None, name).unwrap_or(Value::Null);
    let tmp = std::env::temp_dir().join(format!("adom-edit-fp-{}.lbr", std::process::id()));
    let _ = std::fs::write(&tmp, &text);
    // the authoring preflight, as create_footprint uses: a footprint-only library (no symbols or
    // components yet) is fine to write; opening a component in it is what needs them
    let (ok, errors) = fusion_lbr::authoring::preflight_text(&tmp);
    let _ = std::fs::remove_file(&tmp);
    if !ok { return json!({"success": false, "errorCode": "lbr_preflight_failed", "error": "the edited library would not open cleanly in Fusion; nothing was written", "preflight": errors, "summary": summary, "shell": "rust"}); }
    if args["dryRun"].as_bool() == Some(true) { return json!({"success": true, "mutated": false, "summary": summary, "footprint": after, "shell": "rust"}); }
    let bak = format!("{path}.bak");
    if let Err(e) = std::fs::copy(&path, &bak).and_then(|_| std::fs::write(&path, &text)) { return json!({"success": false, "errorCode": "write_failed", "error": e.to_string(), "shell": "rust"}); }
    json!({"success": true, "mutated": true, "libraryPath": path, "backup": bak, "summary": summary, "footprint": after, "shell": "rust",
           "_hint": "Edited in place (the old file is the .bak beside it) and preflighted. A library already open in Fusion does not see the change until it is reopened (fusion_open_lbr); a design's embedded copy updates when its parts are updated from the library."})
}

/// The standard's library-authoring verbs on .lbr files, offline (fusion_lbr::authoring). A change
/// is preflighted before it is written; an existing library keeps its old text as .bak; a library
/// that does not exist yet is created by create_footprint, create_symbol and xcopy_footprint.
fn native_lib_authoring(verb: &str, args: &Value) -> Value {
    use fusion_lbr::authoring as a;
    let s = |k: &str| args[k].as_str().filter(|v| !v.is_empty()).map(str::to_string);
    if verb == "find_symbol" {
        let query = s("query").unwrap_or_default();
        if query.is_empty() { return json!({"success": false, "errorCode": "bad_args", "error": "query is required", "shell": "rust"}); }
        let footprints = s("kind").as_deref() == Some("footprint");
        let mut paths: Vec<String> = args["libraries"].as_array().map(|l| l.iter().filter_map(|x| x.as_str().map(|p| p.replace('\\', "/"))).collect()).unwrap_or_default();
        let mut via = "explicit";

        // 1. The CATALOGUE first. Fusion keeps an index of every library it knows (the same
        // rows Library Manager shows) with an absolute path per row, so a hit needs no
        // scanning and no Fusion: ~934 libraries, answer in microseconds. This is what makes
        // the bridge behave like the user browsing their own Library Manager.
        let mut catalogue_hits = if paths.is_empty() {
            crate::libraries::search(&query, footprints)
        } else { Vec::new() };
        if !catalogue_hits.is_empty() { via = "catalogue"; }

        // 2. An explicit folder, searched RECURSIVELY: the built-in libraries live one level
        // down in adsk.eagle_library_*/<Name>.lbr, so a top-level scan found nothing and the
        // caller was told "no such part" while 54 libraries sat right there.
        if let Some(dir) = s("folder").or(s("libraryPath")) {
            let d = dir.replace('\\', "/");
            if std::path::Path::new(&d).is_dir() {
                let mut seen = std::collections::HashSet::new();
                let mut stack = vec![std::path::PathBuf::from(&d)];
                while let Some(cur) = stack.pop() {
                    let Ok(rd) = std::fs::read_dir(&cur) else { continue };
                    for e in rd.flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            // guard against symlink loops and absurd depth
                            if stack.len() < 64 { stack.push(p); }
                        } else if p.extension().map(|x| x.eq_ignore_ascii_case("lbr")).unwrap_or(false) {
                            let f = p.to_string_lossy().replace('\\', "/");
                            if seen.insert(f.clone()) { paths.push(f); }
                        }
                    }
                }
                if via == "explicit" { via = "folder"; }
            } else { paths.push(d); }
        }

        // 3. Nothing named: fall back to every library the catalogue knows, so a part is
        // found without the caller having to know where Fusion put it.
        if paths.is_empty() && catalogue_hits.is_empty() {
            paths = crate::libraries::all_library_paths();
            if !paths.is_empty() { via = "catalogue-all"; }
        }
        // Catalogue hits already carry their absolute path, so scanning them again is wasted
        // work: add only the ones the caller named.
        for h in catalogue_hits.drain(..) {
            if !paths.iter().any(|p| p == &h.path) { paths.push(h.path); }
        }

        if paths.is_empty() { return json!({"success": false, "errorCode": "bad_args", "error": "libraries [paths] or folder is required, and Fusion's library catalogue was not found on this machine. Pass `libraries: [\"/path/to/x.lbr\"]`, or `folder` pointing at a folder of .lbr files (searched recursively), or ask the user for the library path.", "shell": "rust"}); }
        let libs: Vec<(String, String)> = paths.iter().filter_map(|p| std::fs::read_to_string(p).ok().map(|t| (p.clone(), t))).collect();
        let mut r = a::find_symbol(&libs, &query, footprints);
        r["success"] = json!(true); r["searched"] = json!(libs.len()); r["via"] = json!(via); r["shell"] = json!("rust");
        if r["best"].is_null() {
            r["_hint"] = json!("No match. Do NOT invent a symbol name. Ask the user for a library path, or offer to create the part (fusion_create_footprint + fusion_create_symbol, both offline) - see skill://fusion-part-sourcing. Names are family-based: LED_RADIAL, LEDRD254*, RESAD*, 1X02.");
        }
        return r;
    }
    // the land pattern is laid out by the standard's engine; this writes it as an EAGLE package
    let mut land = Value::Null;
    if verb == "footprint_from_land_pattern" {
        land = adom_eda_checks::run("footprint_from_land_pattern", &json!({"args": args}));
        if land["success"].as_bool() != Some(true) {
            let mut r = land.clone(); r["shell"] = json!("rust");
            if r["_hint"].is_null() { r["_hint"] = json!("Give the datasheet's RECOMMENDED LAND PATTERN numbers (mm): chip {span, padLength, padWidth}; dual/sot {pins | leftPins+rightPins, pitch, span, padLength, padWidth}; quad {pins | pinsPerSideX+pinsPerSideY, pitch, span, spanY?, padLength, padWidth}; bga {rows, cols, pitch, ballPad, missing?}; plus exposedPad {w, h} and body {w, l, height?}."); }
            return r;
        }
        if args["dryRun"].as_bool() == Some(true) {
            return json!({"success": true, "mutated": false, "dryRun": true, "name": land["name"], "description": land["description"], "padTable": land["padTable"], "courtyardMm": land["courtyardMm"], "pads": land["pads"], "lines": land["lines"].as_array().map(|a| a.len()), "engine": land["engine"], "shell": "rust",
                "_hint": "Nothing written. Check padTable (name, x, y, w, h in mm, Y up, pin 1 top-left) against the datasheet, then call again without dryRun and with libraryPath."});
        }
    }
    let path = match s(if verb == "xcopy_footprint" { "toLib" } else { "libraryPath" }) { Some(p) => p.replace('\\', "/"), None => return json!({"success": false, "errorCode": "bad_args", "error": format!("{} is required", if verb == "xcopy_footprint" { "toLib (the destination .lbr)" } else { "libraryPath" }), "shell": "rust"}) };
    let existing = std::fs::read_to_string(&path).ok();
    let creates = matches!(verb, "create_footprint" | "create_symbol" | "xcopy_footprint" | "footprint_from_land_pattern");
    if existing.is_none() && !creates { return json!({"success": false, "errorCode": "library_unreadable", "error": format!("cannot read {path}"), "libraryPath": path, "shell": "rust"}); }
    if verb == "lint_library" {
        // the library read into the standard's shape, judged by the shared engine
        return match a::lint_input(existing.as_deref().unwrap()) {
            Ok(lib) => { let mut r = lint_judge(lib, args); r["libraryPath"] = json!(path); r["shell"] = json!("rust"); r }
            Err(e) => json!({"success": false, "errorCode": "library_unreadable", "error": e, "shell": "rust"}),
        };
    }
    if verb == "symbol_prims" {
        let name = s("symbolName").or(s("name")).unwrap_or_default();
        return match a::symbol_prims(existing.as_deref().unwrap(), &name) { Ok(mut v) => { v["success"] = json!(true); v["mutated"] = json!(false); v["libraryPath"] = json!(path); v["shell"] = json!("rust"); v }, Err(e) => json!({"success": false, "errorCode": "symbol_not_found", "error": e, "shell": "rust"}) };
    }
    if verb == "read_symbol" {
        let name = s("symbolName").or(s("name")).unwrap_or_default();
        return match a::read_symbol(existing.as_deref().unwrap(), &name) { Ok(mut v) => { v["success"] = json!(true); v["libraryPath"] = json!(path); v["shell"] = json!("rust"); v }, Err(e) => json!({"success": false, "errorCode": "symbol_not_found", "error": e, "shell": "rust"}) };
    }
    let text = existing.clone();
    let r: Result<(String, Value), String> = match verb {
        "create_footprint" => a::create_footprint(text.as_deref(), args),
        "create_symbol" => a::create_symbol(text.as_deref(), args),
        "footprint_from_land_pattern" => a::create_from_layout(text.as_deref(), land["name"].as_str().unwrap_or(""), land["description"].as_str().unwrap_or(""), &land).map(|(t, mut v)| {
            for k in ["padTable", "courtyardMm", "engine"] { v[k] = land[k].clone(); }
            if land["body"]["height"].is_number() { v["body3d"] = json!({"attached": false, "next": "fusion_generate_body {family, ...} writes the STEP; fusion_attach_3d_package binds it to this footprint"}); }
            (t, v)
        }),
        "copy_footprint" => a::copy_footprint(text.as_deref().unwrap(), None, &s("from").unwrap_or_default(), &s("to").unwrap_or_default()),
        "xcopy_footprint" => {
            let from_lib = s("fromLib").unwrap_or_default().replace('\\', "/");
            match std::fs::read_to_string(&from_lib) {
                Ok(src) => { let from = s("from").unwrap_or_default(); let to = s("to").unwrap_or(from.clone()); let dest = text.clone().unwrap_or_else(|| a::new_library("made by fusion-bridge")); a::copy_footprint(&dest, Some(&src), &from, &to) }
                Err(e) => Err(format!("fromLib {from_lib}: {e}")),
            }
        }
        "rename_footprint" => a::rename_footprint(text.as_deref().unwrap(), &s("from").unwrap_or_default(), &s("to").unwrap_or_default()),
        "remove_footprint" => a::remove_footprint(text.as_deref().unwrap(), &s("footprintName").or(s("name")).unwrap_or_default()),
        "clone_symbol" => a::clone_symbol(text.as_deref().unwrap(), &s("from").unwrap_or_default(), &s("to").unwrap_or_default()),
        "rename_symbol" => a::rename_symbol(text.as_deref().unwrap(), &s("from").unwrap_or_default(), &s("to").unwrap_or_default()),
        "remove_symbol" => a::remove_symbol(text.as_deref().unwrap(), &s("symbolName").or(s("name")).unwrap_or_default()),
        "add_symbol_art" => match s("kicadSym") {
            Some(k) => match std::fs::read_to_string(k.replace('\\', "/")) {
                Ok(t) => a::add_symbol_art(text.as_deref().unwrap(), &s("symbolName").or(s("name")).unwrap_or_default(), &t, args["clear"].as_bool() != Some(false)),
                Err(e) => Err(format!("cannot read kicadSym {k}: {e} (send_files the .kicad_sym to this machine first)")),
            },
            None => Err("kicadSym is required: the .kicad_sym (on the Fusion machine) carrying the baked outline".to_string()),
        },
        "set_visibility" => match s("parameter").or(s("paramName")) {
            Some(prm) => a::set_visibility(text.as_deref().unwrap(), &s("symbolName").or(s("name")).unwrap_or_default(), &prm, args["visible"].as_bool().unwrap_or(true)),
            None => Err("parameter is required (the attribute to show or hide, e.g. MPN)".to_string()),
        },
        "plate_vias" => a::plate_vias(text.as_deref().unwrap(), s("footprintName").as_deref()),
        "cleanup_libs" => a::cleanup_libs(text.as_deref().unwrap()),
        "set_models" => a::set_models(text.as_deref().unwrap(), &s("symbolName").unwrap_or_default(), &s("footprint").unwrap_or_default()),
        "add_models" => {
            // a model's footprint from another .lbr is copied in; its pcblib is that library
            let models: Vec<(String, Option<String>)> = args["models"].as_array().into_iter().flatten().filter_map(|m| {
                let n = m["name"].as_str()?.to_string();
                let src = m["pcblib"].as_str().or(m["libraryPath"].as_str()).map(|p| p.replace('\\', "/")).filter(|p| !p.eq_ignore_ascii_case(&path)).and_then(|p| std::fs::read_to_string(p).ok());
                Some((n, src))
            }).collect();
            if models.is_empty() { Err("models [{name, pcblib?}] is required".to_string()) }
            else { a::add_models(text.as_deref().unwrap(), &s("symbolName").unwrap_or_default(), &models, &args["params"], s("comment").as_deref(), s("description").as_deref()) }
        }
        "set_params" => a::set_params(text.as_deref().unwrap(), s("symbolName").as_deref(), &args["params"], s("comment").as_deref(), s("footprint").as_deref()),
        _ => Err(format!("unknown verb {verb}")),
    };
    let (new_text, summary) = match r { Ok(x) => x, Err(e) => return json!({"success": false, "errorCode": "edit_refused", "error": e, "shell": "rust"}) };
    let tmp = std::env::temp_dir().join(format!("adom-lib-{}-{}.lbr", verb, std::process::id()));
    let _ = std::fs::write(&tmp, &new_text);
    let (ok, errors) = a::preflight_text(&tmp);
    let _ = std::fs::remove_file(&tmp);
    if !ok { return json!({"success": false, "errorCode": "lbr_preflight_failed", "error": "the library would not open cleanly in Fusion; nothing was written", "preflight": errors, "summary": summary, "shell": "rust"}); }
    if args["dryRun"].as_bool() == Some(true) { return json!({"success": true, "mutated": false, "data": summary, "libraryPath": path, "shell": "rust"}); }
    if let Some(parent) = std::path::Path::new(&path).parent() { let _ = std::fs::create_dir_all(parent); }
    let backup = if existing.is_some() { let b = format!("{path}.bak"); let _ = std::fs::copy(&path, &b); Some(b) } else { None };
    if let Err(e) = std::fs::write(&path, &new_text) { return json!({"success": false, "errorCode": "write_failed", "error": e.to_string(), "shell": "rust"}); }
    json!({"success": true, "mutated": true, "data": summary, "libraryPath": path, "backup": backup, "shell": "rust",
           "_hint": "Written and preflighted. A Fusion library is opened with fusion_open_lbr (or fusion_show_footprint / fusion_show_symbol to see one item); a library already open in Fusion does not see the change until it is reopened."})
}

/// The shared engine's lint_library on a library read by fusion_lbr, with names passed through.
fn lint_judge(lib: Value, args: &Value) -> Value {
    adom_eda_checks::run("lint_library", &json!({"library": lib, "args": {"names": args["names"]}}))
}

/// `fusion_preflight_lbr`: will this .lbr open cleanly in Fusion? Offline, instant.
fn native_preflight_lbr(args: &Value) -> Value {
    let path = lbr_path_arg(args, &["libraryPath", "filePath"]);
    if path.is_empty() { return json!({"success": false, "error": "libraryPath required.", "shell": "rust"}); }
    let pf = fusion_lbr::preflight_lbr(std::path::Path::new(&path));
    let ok = pf.ok;
    json!({"success": ok, "preflight": pf, "libraryPath": path, "errorCode": if ok { Value::Null } else { json!("lbr_preflight_failed") }, "shell": "rust",
           "_hint": if ok { "Every check green - Fusion will open this library cleanly." } else {
               "Fix the named check(s) BEFORE any Fusion verb touches this file - each one maps to a real Fusion failure (blocking Error modal, silent No Components, Create-new prompt, or dead wiring). Re-stage and hash-compare if size-sane or xml-parses failed: transfer corruption is the usual cause." }})
}


/// `fusion_aps_get`: raw authenticated GET against APS on the shared token store, with the
/// free-endpoint allowlist and the monthly cap enforced. Non-GET methods stay on the sidecar.
fn native_aps_get(args: &Value) -> Value {
    use fusion_core::aps::{aps_get, ApsError, ApsStore};
    let Some(path) = args["path"].as_str().filter(|p| !p.is_empty()) else { return json!({"success": false, "error": "Missing path.", "shell": "rust"}) };
    let method = args["method"].as_str().unwrap_or("GET").to_uppercase();
    if method != "GET" { return proxy_unavailable("fusion_aps_get with a non-GET method is still served by the sidecar"); }
    let q: Vec<(String, String)> = args["query"].as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), match v { Value::String(s) => s.clone(), other => other.to_string() })).collect()).unwrap_or_default();
    let qref: Vec<(&str, &str)> = q.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let store = ApsStore::default_location();
    match aps_get(&store, path, &qref) {
        Ok((status, body)) => json!({"success": (200..300).contains(&status), "output": format!("HTTP {status}"), "data": {"httpStatus": status, "body": body}, "shell": "rust"}),
        Err(ApsError::NotSignedIn) => json!({"success": false, "output": "HTTP None", "data": {"httpStatus": Value::Null, "body": {"error": "not_signed_in"}}, "shell": "rust"}),
        Err(ApsError::BlockedNonFree(p)) => json!({"success": false, "output": "HTTP 0", "data": {"httpStatus": 0, "body": {"error": "blocked_non_free_endpoint", "path": p,
            "_hint": "Refused — the bridge only calls free Data Management endpoints (/project/v1/, /data/v1/) so you are never charged."}}, "shell": "rust"}),
        Err(ApsError::UsageCap) => json!({"success": false, "output": "HTTP 0", "data": {"httpStatus": 0, "body": {"error": "usage_cap_reached", "cap": fusion_core::aps::MONTHLY_CALL_CAP}}, "shell": "rust"}),
        Err(ApsError::Http(e)) => json!({"success": false, "output": "HTTP 0", "data": {"httpStatus": 0, "body": {"error": e}}, "shell": "rust"}),
    }
}

fn proxy_unavailable(why: &str) -> Value {
    json!({"success": false, "errorCode": "not_native_yet", "error": why, "shell": "rust"})
}


fn not_signed_in() -> Value {
    json!({"success": false, "error": "Not signed in to APS.", "errorCode": "aps_not_signed_in", "_hint": fusion_core::aps::APS_REQUIRED_HINT, "shell": "rust"})
}

/// `fusion_aps_browse`: hubs+projects → top folders → folder contents, no Fusion needed.
fn native_aps_browse(args: &Value) -> Value {
    use fusion_core::aps::*;
    let store = ApsStore::default_location();
    if store.valid_access_token().is_none() { return not_signed_in(); }
    let (hub_id, project_id, folder_id) = (args["hubId"].as_str(), args["projectId"].as_str(), args["folderId"].as_str());
    if let (Some(pid), Some(fid)) = (project_id, folder_id) {
        let (s, b) = aps_get(&store, &format!("/data/v1/projects/{pid}/folders/{fid}/contents"), &[("page[limit]", "200")]).unwrap_or((0, json!({})));
        let items: Vec<Value> = b["data"].as_array().cloned().unwrap_or_default().iter().map(|x| {
            let name = x["attributes"]["displayName"].as_str().or(x["attributes"]["name"].as_str());
            json!({"name": name, "type": x["type"], "id": x["id"]}) }).collect();
        return json!({"success": s == 200, "output": format!("{} item(s)", items.len()), "data": {"level": "folder", "projectId": pid, "folderId": fid, "items": items}, "shell": "rust"});
    }
    if let Some(pid) = project_id {
        let hid = match hub_id { Some(h) => Some(h.to_string()), None => list_hubs(&store).ok().and_then(|hs| hs.first().and_then(|h| h["id"].as_str().map(str::to_string))) };
        let folders = hid.as_deref().map(|h| top_folders(&store, h, pid)).unwrap_or_default();
        let items: Vec<Value> = folders.iter().map(|f| json!({"name": f["attributes"]["name"], "type": "folder", "id": f["id"]})).collect();
        return json!({"success": true, "output": format!("{} top folder(s)", items.len()), "data": {"level": "project", "hubId": hid, "projectId": pid, "items": items}, "shell": "rust"});
    }
    let hubs = match list_hubs(&store) { Ok(h) => h, Err((_, body)) => return json!({"success": false, "error": "APS hubs request failed.", "data": {"body": body}, "shell": "rust"}) };
    let mut out = Vec::new();
    for h in &hubs {
        let hid = h["id"].as_str().unwrap_or("");
        if let Some(f) = hub_id { if hid != f { continue; } }
        let projs: Vec<Value> = list_projects(&store, hid).iter().map(|p| json!({"name": p["attributes"]["name"], "id": p["id"]})).collect();
        out.push(json!({"hub": h["attributes"]["name"], "hubId": hid, "projects": projs}));
    }
    let nproj: usize = out.iter().map(|x| x["projects"].as_array().map(|a| a.len()).unwrap_or(0)).sum();
    json!({"success": true, "output": format!("{} hub(s), {} project(s)", out.len(), nproj), "data": {"level": "hubs", "hubs": out}, "shell": "rust"})
}

/// `fusion_aps_search`: the server-indexed name search across the hub, newest first.
/// The parent folder id of an item (Data API item record, never the index).
fn parent_folder_of(store: &fusion_core::aps::ApsStore, project_id: &str, item_id: &str) -> Option<String> {
    use fusion_core::aps::{aps_get, urlencode};
    // index hits are VERSIONS (urn:adsk.wipprod:fs.file:vf.X?version=N): resolve to the item first
    let path = if item_id.contains("fs.file:vf.") { format!("/data/v1/projects/{}/versions/{}/item", urlencode(project_id), urlencode(item_id)) } else { format!("/data/v1/projects/{}/items/{}", urlencode(project_id), urlencode(item_id)) };
    let (s, b) = aps_get(store, &path, &[]).ok()?;
    if s != 200 { return None; }
    if let Some(p) = b["data"]["relationships"]["parent"]["data"]["id"].as_str() { return Some(p.to_string()); }
    let item = b["data"]["id"].as_str()?;
    let (s2, b2) = aps_get(store, &format!("/data/v1/projects/{}/items/{}", urlencode(project_id), urlencode(item)), &[]).ok()?;
    if s2 != 200 { return None; }
    b2["data"]["relationships"]["parent"]["data"]["id"].as_str().map(str::to_string)
}

/// Non-index name search: list folder CONTENTS (Data API, never the search index) under the
/// scanned top folders, breadth first to `max_depth`, at most `max_folders` listings, 12 in
/// parallel. Returns items whose display name contains every token (issue #85).
fn contents_name_scan(store: &fusion_core::aps::ApsStore, tasks: &[(String, String, String)], toks: &[&str], max_depth: usize, max_folders: usize) -> Value {
    use std::sync::{Arc, Mutex};
    use fusion_core::aps::{aps_get, urlencode};
    let queue: Arc<Mutex<std::collections::VecDeque<(String, String, String, usize)>>> = Arc::new(Mutex::new(tasks.iter().map(|(p, pn, f)| (p.clone(), pn.clone(), f.clone(), 0usize)).collect()));
    let listed = Arc::new(Mutex::new(0usize));
    let hits: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let toks: Vec<String> = toks.iter().map(|t| t.to_string()).collect();
    let dir = store.dir.clone();
    let mut handles = Vec::new();
    for _ in 0..12 {
        let (queue, listed, hits, dir, toks) = (queue.clone(), listed.clone(), hits.clone(), dir.clone(), toks.clone());
        handles.push(std::thread::spawn(move || {
            let store = fusion_core::aps::ApsStore { dir };
            let mut idle = 0;
            loop {
                let next = queue.lock().unwrap().pop_front();
                let Some((pid, pname, fid, depth)) = next else { idle += 1; if idle > 20 { break; } std::thread::sleep(std::time::Duration::from_millis(100)); continue; };
                idle = 0;
                { let mut l = listed.lock().unwrap(); if *l >= max_folders { continue; } *l += 1; }
                let (s, b) = aps_get(&store, &format!("/data/v1/projects/{}/folders/{}/contents", urlencode(&pid), urlencode(&fid)), &[("page[limit]", "200")]).unwrap_or((0, json!({})));
                if s != 200 { continue; }
                for it in b["data"].as_array().cloned().unwrap_or_default() {
                    let ty = it["type"].as_str().unwrap_or("");
                    let name = it["attributes"]["displayName"].as_str().or(it["attributes"]["name"].as_str()).unwrap_or("").to_string();
                    if ty == "folders" {
                        if depth + 1 <= max_depth { queue.lock().unwrap().push_back((pid.clone(), pname.clone(), it["id"].as_str().unwrap_or("").to_string(), depth + 1)); }
                        continue;
                    }
                    let nl = name.to_lowercase();
                    if !toks.is_empty() && toks.iter().all(|t| nl.contains(t.as_str())) {
                        hits.lock().unwrap().push(json!({"name": name, "type": ty, "id": it["id"], "lineageUrn": it["id"], "projectId": pid, "projectName": pname, "folderId": fid,
                            "lastModified": it["attributes"]["lastModifiedTime"], "source": "folder-contents"}));
                    }
                }
            }
        }));
    }
    for h in handles { let _ = h.join(); }
    let mut out = hits.lock().unwrap().clone();
    out.sort_by(|a, b| b["lastModified"].as_str().unwrap_or("").cmp(a["lastModified"].as_str().unwrap_or("")));
    let mut seen = std::collections::HashSet::new();
    let out: Vec<Value> = out.into_iter().filter(|r| seen.insert(r["id"].as_str().unwrap_or("").to_string())).collect();
    json!(out)
}

pub(crate) fn native_aps_search(args: &Value) -> Value {
    crate::palette::say("Searching your hub", &format!("cloud search for {}", args["query"].as_str().unwrap_or("")), None, None);
    use fusion_core::aps::*;
    let query = args["query"].as_str().unwrap_or("").trim().to_string();
    if query.is_empty() { return json!({"success": false, "error": "Missing query.", "shell": "rust"}); }
    let store = ApsStore::default_location();
    if store.valid_access_token().is_none() { return not_signed_in(); }
    let limit = args["limit"].as_u64().unwrap_or(50) as usize;
    let hubs = match list_hubs(&store) { Ok(h) => h, Err((s, body)) => return json!({"success": false, "error": "APS hubs request failed.", "data": {"httpStatus": s, "body": body},
        "_hint": "Token may lack data:read scope or the app isn't Data-Management-enabled.", "shell": "rust"}) };
    let mut scanned = (0, 0, 0);
    let tasks = scan_tasks(&store, &hubs, args["hubId"].as_str(), args["projectId"].as_str(), &mut scanned);
    let tasks_snapshot: Vec<(String, String, String)> = tasks.iter().map(|t| (t.project_id.clone(), t.project_name.clone(), t.folder_id.clone())).collect();
    // Speculative sibling search, in parallel with the real one: the index scan costs ~30 s on a
    // 16-project hub, so the first shortened query ("Pincer Moving" for "Pincer Moving Large")
    // runs at the same time and is only read if the full query comes back empty (issue #85).
    let spec_toks: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_string).collect();
    let mut speculative: Vec<(usize, std::thread::JoinHandle<Vec<Value>>)> = Vec::new();
    if spec_toks.len() >= 2 {
        let mut variants = vec![(spec_toks.len() - 1, spec_toks[..spec_toks.len() - 1].join(" "))];
        if spec_toks.len() >= 3 { variants.push((1, spec_toks[0].clone())); }
        for (k, partial) in variants {
            let (dir, hubs2, hub_f, proj_f) = (store.dir.clone(), hubs.clone(), args["hubId"].as_str().map(str::to_string), args["projectId"].as_str().map(str::to_string));
            speculative.push((k, std::thread::spawn(move || {
                let store2 = ApsStore { dir };
                let t2 = scan_tasks(&store2, &hubs2, hub_f.as_deref(), proj_f.as_deref(), &mut (0, 0, 0));
                scan_parallel(&store2, t2, &partial, 10, false)
            })));
        }
    }
    let mut results = scan_parallel(&store, tasks, &query, limit, true);
    results.sort_by(|a, b| b["lastModified"].as_str().unwrap_or("").cmp(a["lastModified"].as_str().unwrap_or("")));
    let n = results.len();
    let shown: Vec<Value> = results.into_iter().take(limit).collect();
    if n == 0 {
        // Zero from the server index is not "the file does not exist" (issue #85: a model the
        // index missed sat in the same folder as its sibling hit). Fall back to a name scan of
        // the recently modified items the bridge can list without a full walk.
        let ql = query.to_lowercase();
        let toks: Vec<&str> = ql.split_whitespace().collect();
        // Fallback 1 (the issue's own suggestion, automated): find a RELATED file the index does
        // know by dropping query tokens from the end ("Pincer Moving Large" -> "Pincer Moving" ->
        // "Pincer"), take the distinct parent folders of those hits, and list each folder's
        // CONTENTS through the Data API (not the index), matching the full name directly.
        let mut fb: Vec<Value> = Vec::new();
        let mut related_folders: Vec<(String, String, String)> = Vec::new(); // (projectId, projectName, folderId)
        let mut k = toks.len();
        let mut spec_hits: std::collections::HashMap<usize, Vec<Value>> = speculative.into_iter().filter_map(|(k, h)| h.join().ok().map(|v| (k, v))).collect();
        while k > 0 && related_folders.is_empty() {
            k -= 1;
            if k == 0 { break; }
            let hits = match spec_hits.remove(&k) {
                Some(h) => h,
                None => { let partial = toks[..k].join(" "); let tasks2 = scan_tasks(&store, &hubs, args["hubId"].as_str(), args["projectId"].as_str(), &mut (0, 0, 0)); scan_parallel(&store, tasks2, &partial, 10, false) }
            };
            for h in hits.iter().take(12) {
                let (pid, pname, id) = (h["projectId"].as_str().unwrap_or("").to_string(), h["projectName"].as_str().unwrap_or("").to_string(), h["id"].as_str().unwrap_or("").to_string());
                if let Some(folder) = parent_folder_of(&store, &pid, &id) {
                    if !related_folders.iter().any(|(_, _, f)| *f == folder) { related_folders.push((pid, pname, folder)); }
                }
                if related_folders.len() >= 6 { break; }
            }
        }
        if !related_folders.is_empty() {
            fb = contents_name_scan(&store, &related_folders, &toks, 1, 12).as_array().cloned().unwrap_or_default();
        }
        // Fallback 2, only when scoped to one project (bounded, still no index): walk that
        // project's folders two levels deep.
        if fb.is_empty() && args["projectId"].as_str().is_some() {
            fb = contents_name_scan(&store, &tasks_snapshot, &toks, 2, 60).as_array().cloned().unwrap_or_default();
        }
        let related_note = if related_folders.is_empty() { String::new() } else { format!(" Related files the index does know sit in {} folder(s), listed directly through the Data API.", related_folders.len()) };
        let nf = fb.len();
        let folder_hint = fb.first().and_then(|f| f["folderId"].as_str()).map(|f| format!(" The first fallback hit lives in folder {f}; fusion_aps_browse {{folderId}} lists that folder in full.")).unwrap_or_default();
        return json!({"success": true, "output": format!("0 index match(es) for '{query}'; {nf} by name in recent items"),
            "data": {"query": query, "count": 0, "results": [], "scanned": {"hubs": scanned.0, "projects": scanned.1, "folders": scanned.2},
                     "fallback": {"source": "Data API folder contents (not the index) of the parent folders of related index hits (query shortened from the end), plus a two-level walk when projectId scopes the search; names matched directly", "relatedFolders": related_folders.iter().map(|(p, pn, f)| json!({"projectId": p, "projectName": pn, "folderId": f})).collect::<Vec<_>>(), "count": nf, "results": fb},
                     "completeness": "The APS server index can lag a new or renamed file by minutes to hours and tokenises names, so a zero here does not prove absence."},
            "shell": "rust",
            "_hint": format!("No index match for '{query}'.{}{} {}Next: fusion_aps_browse {{projectId, folderId}} on a known sibling's folder (scoped, no cloud walk), or pass projectId to add a bounded two-level walk. Do not conclude the file is missing from an index zero alone.",
                related_note, if nf > 0 { format!(" {nf} item(s) match by name in those folders (data.fallback, with projectId, folderId and lineage); open one with fusion_open_by_urn.") } else { " Nothing there matches the full name either.".to_string() }, folder_hint)});
    }
    // the shortened-query searches only matter when the full query finds nothing: do not make a
    // successful search wait for them (they finish in the background and are dropped)
    drop(speculative);
    json!({"success": true, "output": format!("{n} match(es) for '{query}'"),
           "data": {"query": query, "count": n, "results": shown, "scanned": {"hubs": scanned.0, "projects": scanned.1, "folders": scanned.2}}, "shell": "rust",
           "_hint": format!("Found {n} match(es) via the APS server index across {} project(s), newest first. Pass projectId to scope to one project (faster). Use fusion_aps_open to open a hit in Fusion.", scanned.1)})
}

/// `fusion_aps_recent`: most-recently-modified files across the hub (or one project).
fn native_aps_recent(args: &Value) -> Value {
    use fusion_core::aps::*;
    let store = ApsStore::default_location();
    if store.valid_access_token().is_none() { return not_signed_in(); }
    let limit = args["limit"].as_u64().unwrap_or(25) as usize;
    let hubs = match list_hubs(&store) { Ok(h) => h, Err((_, body)) => return json!({"success": false, "error": "APS hubs request failed.", "data": {"body": body}, "shell": "rust"}) };
    let mut scanned = (0, 0, 0);
    let tasks = scan_tasks(&store, &hubs, args["hubId"].as_str(), args["projectId"].as_str(), &mut scanned);
    let mut items: Vec<Value> = scan_parallel(&store, tasks, "", 200, false).into_iter().filter(|i| i["lastModified"].as_str().map(|s| !s.is_empty()).unwrap_or(false)).collect();
    items.sort_by(|a, b| b["lastModified"].as_str().unwrap_or("").cmp(a["lastModified"].as_str().unwrap_or("")));
    let items: Vec<Value> = items.into_iter().take(limit).collect();
    json!({"success": true, "output": format!("{} recent file(s)", items.len()), "data": {"count": items.len(), "results": items}, "shell": "rust",
           "_hint": "Most-recently-modified designs across the team hub, newest first."})
}


/// `fusion_aps_file_info` (tip + up to 20 versions) and `fusion_aps_versions` (every version):
/// who last changed a file, when, how many versions. `query` resolves through the native
/// search (newest match), else projectId + urn (a version urn is reduced to its lineage).
fn native_aps_file_info(args: &Value, full: bool) -> Value {
    use fusion_core::aps::*;
    let store = ApsStore::default_location();
    if store.valid_access_token().is_none() { return not_signed_in(); }
    let (mut project_id, mut urn, mut name) = (args["projectId"].as_str().map(str::to_string), args["urn"].as_str().map(str::to_string), args["name"].as_str().map(str::to_string));
    let query = args["query"].as_str().unwrap_or("").trim().to_string();
    if !query.is_empty() && !(project_id.is_some() && urn.is_some()) {
        let found = native_aps_search(&json!({"query": query, "limit": 5}));
        let first = found["data"]["results"].as_array().and_then(|a| a.first().cloned());
        let Some(m) = first else { return json!({"success": false, "error": format!("No cloud file matching '{query}'."), "shell": "rust"}) };
        project_id = m["projectId"].as_str().map(str::to_string); urn = m["id"].as_str().map(str::to_string); name = m["name"].as_str().map(str::to_string);
    }
    let (Some(pid), Some(urn)) = (project_id, urn) else { return json!({"success": false, "error": "Provide query, or projectId + urn.", "shell": "rust"}) };
    let raw = urn.split('?').next().unwrap_or("").to_string();
    let item_urn = match raw.find("fs.file:vf.") { Some(i) => format!("urn:adsk.wipprod:dm.lineage:{}", &raw[i + "fs.file:vf.".len()..]), None => urn.clone() };
    let enc = urlencode(&item_urn);
    let q: &[(&str, &str)] = if full { &[("page[limit]", "200")] } else { &[] };
    let (s, b) = aps_get(&store, &format!("/data/v1/projects/{pid}/items/{enc}/versions"), q).unwrap_or((0, json!({})));
    if s != 200 || !b.is_object() {
        return json!({"success": false, "error": "Could not fetch versions.", "data": {"httpStatus": s, "body": b, "itemUrn": item_urn}, "shell": "rust"});
    }
    let mut versions: Vec<Value> = b["data"].as_array().cloned().unwrap_or_default().iter().map(|v| {
        let a = &v["attributes"];
        let mut row = json!({"versionNumber": a["versionNumber"], "lastModifiedTime": a["lastModifiedTime"], "lastModifiedUserName": a["lastModifiedUserName"],
                             "createTime": a["createTime"], "createUserName": a["createUserName"], "storageSize": a["storageSize"]});
        if full { row["id"] = v["id"].clone(); }
        row }).collect();
    versions.sort_by(|x, y| y["versionNumber"].as_i64().unwrap_or(0).cmp(&x["versionNumber"].as_i64().unwrap_or(0)));
    let n = versions.len();
    let label = name.clone().unwrap_or_else(|| item_urn.clone());
    if full {
        json!({"success": true, "output": format!("{label}: {n} version(s)"),
               "data": {"name": name, "projectId": pid, "itemUrn": item_urn, "versionCount": n, "versions": versions}, "shell": "rust",
               "_hint": "Complete version history, newest-first."})
    } else {
        let tip = versions.first().cloned().unwrap_or(json!({}));
        let shown: Vec<Value> = versions.iter().take(20).cloned().collect();
        json!({"success": true, "output": format!("{label}: {n} version(s), last by {}", tip["lastModifiedUserName"].as_str().unwrap_or("None")),
               "data": {"name": name, "projectId": pid, "itemUrn": item_urn, "versionCount": n, "tip": tip, "versions": shown}, "shell": "rust",
               "_hint": "Version history newest-first; tip = current version (who/when/size)."})
    }
}


/// `fusion_aps_download`: a cloud file to the local machine, no Fusion needed. query or
/// projectId + urn; the tip version's storage object is fetched through a signed S3 URL.
fn native_aps_download(args: &Value) -> Value {
    use fusion_core::aps::*;
    let store = ApsStore::default_location();
    if store.valid_access_token().is_none() { return not_signed_in(); }
    let (mut project_id, mut urn, mut name) = (args["projectId"].as_str().map(str::to_string), args["urn"].as_str().map(str::to_string), args["name"].as_str().map(str::to_string));
    let query = args["query"].as_str().unwrap_or("").trim().to_string();
    if !query.is_empty() && !(project_id.is_some() && urn.is_some()) {
        let found = native_aps_search(&json!({"query": query, "limit": 5}));
        let Some(m) = found["data"]["results"].as_array().and_then(|a| a.first().cloned()) else { return json!({"success": false, "error": format!("No cloud file matching '{query}'."), "shell": "rust"}) };
        project_id = m["projectId"].as_str().map(str::to_string); urn = m["id"].as_str().map(str::to_string); name = m["name"].as_str().map(str::to_string);
    }
    let (Some(pid), Some(urn)) = (project_id, urn) else { return json!({"success": false, "error": "Provide query, or projectId + urn.", "shell": "rust"}) };
    let version_urn = match tip_version_urn(&store, &pid, &urn) { Ok(v) => v, Err(e) => return json!({"success": false, "error": "Could not resolve a version.", "data": {"detail": [e.0, e.1]}, "shell": "rust"}) };
    let (storage, vname) = match storage_of_version(&store, &pid, &version_urn) { Ok(x) => x, Err(e) => return json!({"success": false, "error": "Could not find storage for this version.", "data": {"detail": [e.0, e.1]}, "shell": "rust"}) };
    let (bucket, obj) = parse_oss(&storage);
    let (s, b) = aps_get(&store, &format!("/oss/v2/buckets/{bucket}/objects/{}/signeds3download", urlencode(&obj)), &[]).unwrap_or((0, json!({})));
    let Some(url) = b["url"].as_str().filter(|_| s == 200) else { return json!({"success": false, "error": "Could not get a download URL.", "data": {"httpStatus": s, "body": b}, "shell": "rust"}) };
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let save_dir = args["saveDir"].as_str().map(str::to_string).unwrap_or_else(|| format!("{home}/Downloads"));
    let fname = args["fileName"].as_str().map(str::to_string).or(vname).or(name).unwrap_or_else(|| "download.bin".into());
    let _ = std::fs::create_dir_all(&save_dir);
    let out_path = std::path::Path::new(&save_dir).join(&fname);
    let (st, data) = http_bytes(url);
    if st != 200 { return json!({"success": false, "error": format!("Download failed (HTTP {st})."), "data": {"bytes": data.len()}, "shell": "rust"}); }
    if let Err(e) = std::fs::write(&out_path, &data) { return json!({"success": false, "error": format!("Could not write {}: {e}", out_path.display()), "shell": "rust"}); }
    let n = data.len();
    let pretty = { let s = n.to_string(); let mut o = String::new(); for (i, c) in s.chars().enumerate() { if i > 0 && (s.len() - i) % 3 == 0 { o.push(','); } o.push(c); } o };
    json!({"success": true, "output": format!("Downloaded {fname} ({pretty} bytes) to {}", out_path.to_string_lossy()),
           "data": {"path": out_path.to_string_lossy(), "bytes": n, "name": fname, "version": version_urn}, "shell": "rust",
           "_hint": "Saved to the local machine; opening in Fusion is a separate step (fusion_aps_open)."})
}


fn aps_err(e: fusion_core::aps::ApsError) -> (u16, Value) { (0, json!({"error": e.to_string()})) }

/// `fusion_aps_create_folder {projectId, parentFolderId, name}`: a subfolder in a project.
fn native_aps_create_folder(args: &Value) -> Value {
    use fusion_core::aps::*;
    let store = ApsStore::default_location();
    if store.valid_access_token().is_none() { return not_signed_in(); }
    let pid = args["projectId"].as_str().unwrap_or("");
    let parent = args["parentFolderId"].as_str().or(args["folderId"].as_str()).unwrap_or("");
    let name = args["name"].as_str().unwrap_or("").trim().to_string();
    if pid.is_empty() || parent.is_empty() || name.is_empty() { return json!({"success": false, "error": "Provide projectId, parentFolderId, name.", "shell": "rust"}); }
    let body = json!({"jsonapi": {"version": "1.0"}, "data": {"type": "folders",
        "attributes": {"name": name, "extension": {"type": "folders:autodesk.core:Folder", "version": "1.0"}},
        "relationships": {"parent": {"data": {"type": "folders", "id": parent}}}}});
    let (s, b) = aps_call(&store, "POST", &format!("/data/v1/projects/{pid}/folders"), &[], Some(&body)).unwrap_or_else(aps_err);
    if !(s == 200 || s == 201) || !b.is_object() {
        return json!({"success": false, "error": "Create folder failed.", "data": {"httpStatus": s, "body": b}, "shell": "rust",
                      "_hint": "HTTP 403 = sign-in lacks data:write/data:create (fusion_aps_signin again). 409 = a folder with that name already exists here."});
    }
    json!({"success": true, "output": format!("Created folder '{name}'"), "data": {"folderId": b["data"]["id"], "name": name, "projectId": pid}, "shell": "rust"})
}

/// `fusion_aps_upload {projectId, folderId, localPath, fileName?}`: a local file as a NEW
/// cloud file: reserve storage, signed S3 PUT, complete, create the item (version 1).
fn native_aps_upload(args: &Value) -> Value {
    use fusion_core::aps::*;
    let store = ApsStore::default_location();
    if store.valid_access_token().is_none() { return not_signed_in(); }
    let (pid, fid, local) = (args["projectId"].as_str().unwrap_or(""), args["folderId"].as_str().unwrap_or(""), args["localPath"].as_str().unwrap_or(""));
    if pid.is_empty() || fid.is_empty() || local.is_empty() { return json!({"success": false, "error": "Provide projectId, folderId, localPath.", "shell": "rust"}); }
    // FOLDER HYGIENE: never dump loose files into a shared project ROOT (fusion-cloud-hygiene).
    if fid == "urn:adsk.wipprod:fs.folder:co.jyO4vxQXR6S9zFpTQWnZAg" {
        return json!({"success": false, "errorCode": "refused_root_upload", "shell": "rust",
            "error": "REFUSED: that folderId is a shared project ROOT - the bridge never writes loose files there (it clutters the team's cloud).",
            "_hint": "Create/choose a real subfolder first (fusion_aps_create_folder under an 'Adom AI Workspace' folder), or just call fusion_make_3d_package / fusion_build_library_3d, which now default to a non-root workspace folder. See the fusion-cloud-hygiene skill."});
    }
    let Ok(data) = std::fs::read(local) else { return json!({"success": false, "error": format!("Local file not found: {local}"), "shell": "rust"}) };
    let fname = args["fileName"].as_str().map(str::to_string).unwrap_or_else(|| std::path::Path::new(local).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default());
    let sbody = json!({"jsonapi": {"version": "1.0"}, "data": {"type": "objects", "attributes": {"name": fname}, "relationships": {"target": {"data": {"type": "folders", "id": fid}}}}});
    let (s, b) = aps_call(&store, "POST", &format!("/data/v1/projects/{pid}/storage"), &[], Some(&sbody)).unwrap_or_else(aps_err);
    if !(s == 200 || s == 201) || !b.is_object() {
        return json!({"success": false, "error": "Storage reservation failed.", "data": {"httpStatus": s, "body": b}, "shell": "rust",
                      "_hint": "HTTP 403 = sign-in lacks data:write/data:create - fusion_aps_signin again."});
    }
    let storage_urn = b["data"]["id"].as_str().unwrap_or("").to_string();
    let (bucket, obj) = parse_oss(&storage_urn);
    let objenc = urlencode(&obj);
    let (s, b) = aps_get(&store, &format!("/oss/v2/buckets/{bucket}/objects/{objenc}/signeds3upload"), &[]).unwrap_or_else(aps_err);
    let (Some(upload_key), Some(put_url)) = (b["uploadKey"].as_str(), b["urls"][0].as_str()) else {
        return json!({"success": false, "error": "Could not get an upload URL.", "data": {"httpStatus": s, "body": b}, "shell": "rust"}) };
    let st = http_put_bytes(put_url, &data);
    if !(st == 200 || st == 201) { return json!({"success": false, "error": format!("S3 upload PUT failed (HTTP {st})."), "shell": "rust"}); }
    let (s, b) = aps_call_ct(&store, "POST", &format!("/oss/v2/buckets/{bucket}/objects/{objenc}/signeds3upload"), &[], Some(&json!({"uploadKey": upload_key})), "application/json").unwrap_or_else(aps_err);
    if !(s == 200 || s == 201 || s == 202) { return json!({"success": false, "error": "Upload completion failed.", "data": {"httpStatus": s, "body": b}, "shell": "rust"}); }
    let ibody = json!({"jsonapi": {"version": "1.0"},
        "data": {"type": "items", "attributes": {"displayName": fname, "extension": {"type": "items:autodesk.core:File", "version": "1.0"}},
                 "relationships": {"tip": {"data": {"type": "versions", "id": "1"}}, "parent": {"data": {"type": "folders", "id": fid}}}},
        "included": [{"type": "versions", "id": "1", "attributes": {"name": fname, "extension": {"type": "versions:autodesk.core:File", "version": "1.0"}},
                      "relationships": {"storage": {"data": {"type": "objects", "id": storage_urn}}}}]});
    let (s, b) = aps_call(&store, "POST", &format!("/data/v1/projects/{pid}/items"), &[], Some(&ibody)).unwrap_or_else(aps_err);
    if !(s == 200 || s == 201) || !b.is_object() {
        return json!({"success": false, "error": "Create item failed (this verb uploads NEW files; the name may already exist).", "data": {"httpStatus": s, "body": b}, "shell": "rust"});
    }
    let n = data.len();
    let pretty = { let t = n.to_string(); let mut o = String::new(); for (i, c) in t.chars().enumerate() { if i > 0 && (t.len() - i) % 3 == 0 { o.push(','); } o.push(c); } o };
    json!({"success": true, "output": format!("Uploaded '{fname}' ({pretty} bytes) as a new cloud file."),
           "data": {"itemUrn": b["data"]["id"], "name": fname, "bytes": n, "projectId": pid, "folderId": fid}, "shell": "rust"})
}


/// `fusion_aps_status`: configured / signed in (usable NOW, refreshed if needed) / who /
/// the never-charge meter. `signinPending` is the sidecar's in-memory sign-in state while
/// it still owns the PKCE flow, so it is asked for that one field.
fn native_aps_status(sidecar: &Arc<Mutex<Option<Sidecar>>>, caller: &Caller) -> Value {
    use fusion_core::aps::*;
    let store = ApsStore::default_location();
    let client_id = store.client_id();
    let tok = store.tokens();
    let has_at = tok["access_token"].as_str().map(|s| !s.is_empty()).unwrap_or(false);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let live = has_at && tok["expires_at"].as_u64().unwrap_or(0) > now;
    let usable = if live { true } else if has_at { store.valid_access_token().is_some() } else { false };
    let refresh_failed = has_at && !usable;
    let pending = aps_signin::signin_pending() || if sidecar.lock().unwrap().is_some() {
        let r = proxy(sidecar, &json!({"command": "aps_status", "args": {}}).to_string(), caller);
        r["data"]["signinPending"].as_bool().unwrap_or(false)
    } else { false };
    let (month, calls) = store.usage();
    let data = json!({
        "configured": !client_id.is_empty(), "signedIn": usable,
        "signedInAs": if usable { user_info(&store) } else { Value::Null },
        "hasStoredToken": has_at, "refreshFailed": refresh_failed, "tokenLive": live,
        "tokenExpiresAt": tok["expires_at"], "scope": tok["scope"].as_str().unwrap_or(SCOPES),
        "signinPending": pending, "callbackUrls": all_registered_callbacks(),
        "usageMonth": month, "callsThisMonth": calls, "monthlyCap": MONTHLY_CALL_CAP, "capRemaining": store.cap_remaining(),
        "freeEndpointsOnly": FREE_PATH_PREFIXES, "neverCharge": true,
    });
    let (hint, summary) = if client_id.is_empty() {
        (format!("Not configured: this box has no APS app ID. FIRST get your org's shared one from the wiki (members-only, nothing secret): `adom-wiki whoami` lists your orgs (several? ask the user which), then `adom-wiki settings get <org> autodesk-aps` prints {{clientId}}. Got one: fusion_aps_set_client_id {{\"clientId\":\"<it>\"}} then fusion_aps_signin (the person signs in with their OWN Autodesk login). SETTINGS_NOT_FOUND: one admin registers a Desktop/PKCE app ONCE at https://aps.autodesk.com (enable Data Management API, set callbacks {}), sets it here, and shares it with `adom-wiki settings set <org> autodesk-aps clientId=<id> kind=public-pkce-client-id` so the rest of the org never repeats this (fusion-aps-org-setup skill). Never read another machine's config files for it.", all_registered_callbacks().join(", ")), "APS not configured")
    } else if !has_at { ("Configured but not signed in. Call fusion_aps_signin (opens browser).".into(), "configured, not signed in") }
    else if !live { ("Token expired and refresh failed. Call fusion_aps_signin again.".into(), "signed in, token expired") }
    else { ("Ready. Use fusion_aps_search {\"query\":\"...\"}.".into(), "ready") };
    json!({"success": true, "output": summary, "data": data, "_hint": hint, "shell": "rust"})
}


/// Gather every input the readiness computation needs from real probes (no side effects),
/// and compute the native answer. Shadow mode: nothing is written, nothing is acted on.
/// Who serves `fusion_readiness`: the sidecar (default, the exe shadows) or the exe
/// (`prefs.json` `readinessOwner: "exe"`). One owner at a time, never both acting.
/// Who owns a surface: the exe unless prefs.json names the sidecar explicitly (the rollback
/// switch while the Python bridge is still in the package). Defaults flipped 2026-09-04 once
/// every verb answered natively and the drill and the tour had run on the exe alone.
fn owner_is_exe(h: &dyn HostBoth, key: &str) -> bool { owner_pref(h, key).as_deref() != Some("sidecar") }
fn owner_pref(h: &dyn HostBoth, key: &str) -> Option<String> {
    std::fs::read_to_string(h.bridge_state_dir().join("prefs.json")).ok().and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).ok()).and_then(|p| p[key].as_str().map(str::to_string))
}
/// The sidecar is spawned only when some surface is handed back to it.
fn sidecar_wanted(h: &dyn HostBoth) -> bool { ["readinessOwner", "chainsOwner", "addinOwner"].iter().any(|k| owner_pref(h, k).as_deref() == Some("sidecar")) || std::env::var("ADOM_FUSION_SIDECAR").map(|v| v == "1").unwrap_or(false) }
fn readiness_owner_is_exe(h: &dyn HostBoth) -> bool { owner_is_exe(h, "readinessOwner") }

/// The signed-in Fusion user, fetched once through the add-in in the background and cached
/// until Fusion stops (the dashboard shows who is signed in; the auth matrix runs accounts).
static FUSION_USER: Mutex<Option<Value>> = Mutex::new(None);
static FUSION_USER_FETCHING: Mutex<bool> = Mutex::new(false);

fn ensure_fusion_user_async(sidecar: &Arc<Mutex<Option<Sidecar>>>, caller: &Caller) {
    if FUSION_USER.lock().unwrap().is_some() || *FUSION_USER_FETCHING.lock().unwrap() { return; }
    *FUSION_USER_FETCHING.lock().unwrap() = true;
    let (sidecar, caller) = (sidecar.clone(), caller.clone());
    std::thread::spawn(move || {
        let body = json!({"command": "fusion_run_modeling_script", "args": {"script": "import adsk.core\na=adsk.core.Application.get()\nresult={'user': a.userName, 'id': a.userId}"}}).to_string();
        let r = proxy(&sidecar, &body, &caller);
        let d = &r["data"]["result"];
        if let Some(u) = d["user"].as_str() { *FUSION_USER.lock().unwrap() = Some(json!({"user": u, "userId": d["id"]})); }
        *FUSION_USER_FETCHING.lock().unwrap() = false;
    });
}

/// The user's Default Modeling Orientation from Fusion's local options file (disk only).
fn modeling_orientation(user_id: Option<&str>) -> Option<String> {
    let uid = user_id?;
    let appdata = std::env::var("APPDATA").ok()?;
    let p = std::path::Path::new(&appdata).join("Autodesk").join("Neutron Platform").join("Options").join(uid).join("NGlobalOptions.xml");
    let t = std::fs::read_to_string(p).ok()?;
    let re = regex::Regex::new(r#"sceneUpOptionId[^>]*Value="(\d)""#).ok()?;
    match re.captures(&t)?.get(1)?.as_str() { "0" => Some("yup".into()), "1" => Some("zup".into()), _ => None }
}

/// `timings.record("launch", secs, "cold")`: a rolling ten samples per phase and start kind.
fn record_launch(h: &dyn HostBoth, secs: f64) {
    let p = h.bridge_state_dir().join("timings.json");
    let mut d: Value = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    let key = "launch|cold";
    let mut samples: Vec<Value> = d[key]["samples"].as_array().cloned().unwrap_or_default().into_iter().filter(|v| v.is_number()).collect();
    samples.push(json!((secs * 10.0).round() / 10.0));
    let keep: Vec<Value> = samples.iter().rev().take(10).rev().cloned().collect();
    d[key] = json!({"samples": keep, "lastAt": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|x| x.as_secs()).unwrap_or(0)});
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(&d).unwrap_or_default()).is_ok() { let _ = std::fs::rename(&tmp, &p); }
}

/// Merge the computation's pref writes into prefs.json (other keys untouched).
fn write_prefs(h: &dyn HostBoth, after: &serde_json::Map<String, Value>) {
    let p = h.bridge_state_dir().join("prefs.json");
    let mut d: Value = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    if !d.is_object() { d = json!({}); }
    for (k, v) in after { d[k] = v.clone(); }
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(&d).unwrap_or_default()).is_ok() { let _ = std::fs::rename(&tmp, &p); }
}

/// The exe OWNS readiness: gather, act where the Python handler would have acted (through
/// the sidecar's executor verbs and its own verbs), compute, persist, and answer.
fn owned_readiness(h: &dyn HostBoth, sidecar: &Arc<Mutex<Option<Sidecar>>>, caller: &Caller) -> Value {
    use fusion_core::readiness::compute;
    let mut inputs = gather_readiness_inputs(h, caller);
    let call = |verb: &str, args: Value| -> Value { proxy(sidecar, &json!({"command": verb, "args": args}).to_string(), caller) };
    let mut seat_resolved = false;
    // the seat / licensing modal: resolve now, then look again (like the Python handler)
    if inputs.running && inputs.licensing_dialog {
        let r = call("resolve_seat_dialog", json!({}));
        seat_resolved = r["clicks"].as_u64().unwrap_or(0) > 0;
        inputs = gather_readiness_inputs(h, caller);
    }
    // the crash-report janitor: clear crash dialogs while Fusion is up but not serving
    let provisional_ready = inputs.installed_raw && inputs.running && inputs.addin_version.is_some() && !inputs.needs_signin && !inputs.licensing_dialog;
    if inputs.running && !provisional_ready {
        let r = call("dismiss_blocking_dialogs", json!({"categories": ["crash_report"]}));
        inputs.crash_dialogs_cleared = r["count"].as_u64().unwrap_or(0) as usize;
    }
    // the silent update click happens BEFORE the hint is composed, like Python
    if provisional_ready {
        let r = call("apply_fusion_update", json!({}));
        inputs.update_offered = r["offered"].as_bool().unwrap_or(false);
        if r["applied"].as_bool() == Some(true) { inputs.update_applied_button = r["button"].as_str().map(str::to_string); }
    }
    // who is signed in, and their modeling orientation
    if provisional_ready { ensure_fusion_user_async(sidecar, caller); } else if !inputs.running { *FUSION_USER.lock().unwrap() = None; }
    inputs.fusion_user = FUSION_USER.lock().unwrap().clone();
    inputs.modeling_orientation = modeling_orientation(inputs.fusion_user.as_ref().and_then(|u| u["userId"].as_str()));
    let out = compute(&inputs);
    write_prefs(h, &out.prefs_after);
    if let Some(secs) = out.actions.record_launch_seconds { record_launch(h, secs); }
    if let Some(attempt) = out.actions.heal_launch {
        let (sidecar, caller) = (sidecar.clone(), caller.clone());
        std::thread::spawn(move || {
            let c = |verb: &str| proxy(&sidecar, &json!({"command": verb, "args": {}}).to_string(), &caller);
            if attempt >= 2 { c("kill"); } else { c("stop"); }
            std::thread::sleep(Duration::from_secs(8));
            c("start");
        });
        eprintln!("[fusion-bridge] add-in never came up; restarting Fusion (attempt {attempt})");
    }
    let mut r = out.result;
    if seat_resolved { r["seatDialogAutoResolved"] = json!(true); }
    r["readinessOwner"] = json!("exe");
    // a DisplayLink box crashes Fusion Electronics: say so on every readiness poll (page issue #121)
    if let Some(w) = display::warning() { r["displayWarning"] = w; }
    r
}

fn shadow_readiness(h: &dyn HostBoth, caller: &Caller) -> Value {
    use fusion_core::readiness::compute;
    let inputs = gather_readiness_inputs(h, caller);
    let facts_degraded = inputs.fusion_user.is_none() && false; // kept for the field below
    let out = compute(&inputs);
    let mut r = out.result;
    r["shadow"] = json!(true);
    r["actions"] = serde_json::to_value(&out.actions).unwrap_or(Value::Null);
    r["windowFactsDegraded"] = json!(WINDOW_FACTS_DEGRADED.lock().unwrap().0 || facts_degraded);
    r
}

/// The last window-facts fetch: (degraded, error) so both readiness paths can report it.
static WINDOW_FACTS_DEGRADED: Mutex<(bool, Option<String>)> = Mutex::new((false, None));

fn gather_readiness_inputs(h: &dyn HostBoth, caller: &Caller) -> fusion_core::readiness::Inputs {
    use fusion_core::readiness::{timings_stats, timings_summary, Inputs};
    let state = h.bridge_state_dir();
    let read_json = |p: std::path::PathBuf| -> Value { std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok()).unwrap_or(json!({})) };
    let prefs = read_json(state.join("prefs.json")).as_object().cloned().unwrap_or_default();
    let timings = read_json(state.join("timings.json"));
    let installed_raw = h.fusion_executable().is_some();
    let streaming = h.image_running(&["streamer.exe", "FusionDL.exe", "Fusion Client Downloader.exe"]) || h.fusion_install_streaming();
    let running = installed_raw && !h.fusion_processes().is_empty();
    let addin = if running { addin_probe() } else { Value::Null };
    let facts = if running { match fusion_core::ab::Loopback::from_env() { Some(ab) => fusion_core::windows_facts::facts_from_ab(&ab, caller), None => Default::default() } } else { Default::default() };
    let expected = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("addin").join("AdomBridge").join("AdomBridge.manifest")))
        .and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|m| m["version"].as_str().map(str::to_string)).unwrap_or_else(|| "unknown".into());
    let aps = fusion_core::aps::ApsStore::default_location();
    let aps_configured = !aps.client_id().is_empty();
    let aps_signed = aps.valid_access_token().is_some();
    let mcp = std::net::TcpStream::connect_timeout(&"127.0.0.1:27182".parse().unwrap(), Duration::from_millis(400)).is_ok();
    let addin_ok = addin.is_object();
    // a loaded add-in at work: its /status says busy, or it is starved silent while one of our
    // requests is in flight or after it answered in this Fusion (never a launch to heal)
    let pids: Vec<u32> = if running { h.fusion_processes().iter().map(|p| p.pid).collect() } else { vec![] };
    let addin_busy = if !running { None }
        else if addin["busy"].as_bool() == Some(true) { Some(json!({"evidence": "status_busy", "busyCommand": addin["busyCommand"], "elapsedSeconds": addin["elapsedSeconds"]})) }
        else if !addin_ok { addin::loaded_evidence(&pids) }
        else { None };
    let provisional_ready = installed_raw && running && addin_ok && !facts.needs_signin && facts.licensing_dialog.is_none();
    let roundtrip_ok = if provisional_ready {
        ureq::post("http://127.0.0.1:8774/command").timeout(Duration::from_secs(4)).send_json(json!({"command": "get_app_state", "args": {}})).ok()
            .and_then(|r| r.into_json::<Value>().ok()).map(|v| v["success"].as_bool().unwrap_or(false)).unwrap_or(false)
    } else { true };
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let kicad_ingest = std::env::var("PATH").unwrap_or_default().split(';').any(|d| std::path::Path::new(d).join("adom-lbr.exe").is_file() || std::path::Path::new(d).join("adom-lbr").is_file())
        || std::env::var("LOCALAPPDATA").map(|la| std::path::Path::new(&la).join("Programs").join("adom-lbr").join("adom-lbr.exe").is_file() || std::path::Path::new(&la).join("adom-lbr").join("adom-lbr.exe").is_file()).unwrap_or(false);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let now_iso = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
    let marker = std::path::Path::new(&std::env::var("TEMP").unwrap_or(home.clone())).join("adom-fusion").join("addin-start-failed.json");
    let addin_start_failed = std::fs::read_to_string(&marker).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).map(|mut m| {
        m["markerPath"] = json!(marker.to_string_lossy());
        m["_hint"] = json!(format!("Fusion loaded a BROKEN copy of the add-in from {}. The bridge re-syncs every AddIns dir on start; restart Fusion with fusion_restart_for_addin {{save:true}} so it loads the repaired copy. If it keeps failing, delete that folder and restart Fusion: the bridge reinstalls it.", m["addinDir"].as_str().unwrap_or("")));
        m });
    let inputs = Inputs {
        installed_raw, streaming, running, addin_version: addin["version"].as_str().map(str::to_string).or(if addin_ok { Some("unknown".into()) } else { None }),
        expected_addin_version: expected, needs_signin: facts.needs_signin, licensing_dialog: facts.licensing_dialog.is_some(),
        updating: h.fusion_update_in_progress(), aps_configured, aps_signed_in: aps_signed, mcp_port_open: mcp,
        auto_update_enabled: prefs.get("autoUpdateFusion").and_then(|v| v.as_bool()).unwrap_or(true), roundtrip_ok, kicad_ingest,
        fusion_user: None, modeling_orientation: None, bridge_version: VERSION.into(), prefs, now, now_iso,
        timings_launch: timings_summary(&timings)["launch"].clone(), launch_stats: timings_stats(&timings, "launch", "cold"),
        progress_override: None, addin_start_failed, crash_dialogs_cleared: 0, update_applied_button: None, update_offered: false,
        addin_busy,
    };
    *WINDOW_FACTS_DEGRADED.lock().unwrap() = (facts.degraded, facts.error.clone());
    inputs
}


/// The exe as the chains' host: sidecar verbs through the proxy, the add-in directly.
struct ExeChainHost { sidecar: Arc<Mutex<Option<Sidecar>>>, caller: Caller, state_dir: std::path::PathBuf }
impl chains::ChainHost for ExeChainHost {
    fn caller_thread(&self) -> String { self.caller.thread.clone() }
    fn call(&self, verb: &str, args: Value, timeout_secs: u64) -> Value {
        // the bind and prove leaves are native when the exe owns the add-in; everything else
        // the chain asks for is a sidecar executor
        const NATIVE: &[&str] = &["build_library_3d", "prove_library", "make_3d_package", "chain_docs_open", "chain_close_docs_since", "chain_close_bridge_doc", "chain_sweep_orphans", "chain_publish_library", "chain_addin_restart_info", "activate_document", "activity_banner", "start", "restart_for_addin", "plan_library_import"];
        if NATIVE.contains(&verb) && addin_owner_is_exe(&*host()) {
            let h = host();
            let (sc, ca) = (self.sidecar.clone(), self.caller.clone());
            let side = move |v: &str, a: Value, t: u64| proxy_with_timeout(&sc, &json!({"command": v, "args": a}).to_string(), &ca, t);
            let pids: Vec<u32> = h.fusion_processes().iter().map(|p| p.pid).collect();
            let dlg = |r: Value| r;
            let ctx = addin::Ctx { host: &*h, fusion_exe: h.fusion_executable(), short_path: |p| host().short_path(p), state_dir: self.state_dir.clone(), caller: self.caller.clone(), pids, sidecar: &side, dialogs: &dlg };
            let pass = |v: &str, a: Value| addin::pass_through(v, &a, &ctx);
            let mut a = args.clone(); if verb == "build_library_3d" { a["async"] = json!(false); }
            return match verb {
                "build_library_3d" => mint::build_library_3d(&ctx, &a, &pass), "make_3d_package" => mint::make_3d_package(&ctx, &a, &pass), "prove_library" => prove::prove_library(&ctx, &a, &pass),
                "chain_docs_open" => chainexec::docs_open(&pass), "chain_close_docs_since" => chainexec::close_docs_since(&ctx, &a, &pass), "chain_close_bridge_doc" => chainexec::close_bridge_doc(&ctx, &a, &pass),
                "plan_library_import" => { let running = !h.fusion_processes().is_empty(); small_verbs::plan_library_import(&self.state_dir, h.fusion_executable().is_some(), running, running && self.addin_round_trip_ready(), &a) },
                "start" => launch::start(&ctx, &a), "restart_for_addin" => lifecycle::restart_for_addin(&ctx, &a, &pass),
                "chain_sweep_orphans" => chainexec::sweep_orphans(&ctx, &pass), "chain_publish_library" => chainexec::publish_library(&ctx, &a, &pass), "chain_addin_restart_info" => chainexec::addin_restart_info(&ctx, &a, &pass),
                _ => pass(verb, a),
            };
        }
        proxy_with_timeout(&self.sidecar, &json!({"command": verb, "args": args}).to_string(), &self.caller, timeout_secs)
    }
    fn state_dir(&self) -> std::path::PathBuf { self.state_dir.clone() }
    fn addin_round_trip_ready(&self) -> bool {
        ureq::post("http://127.0.0.1:8774/command").timeout(Duration::from_secs(6)).send_json(json!({"command": "get_app_state", "args": {}})).ok()
            .and_then(|r| r.into_json::<Value>().ok()).map(|v| v["success"].as_bool().unwrap_or(false)).unwrap_or(false)
    }
    fn addin_status(&self) -> Value { addin_probe() }
    fn fusion_running(&self) -> bool { host().fusion_processes().len() > 0 }
    fn set_long_op(&self, name: Option<String>) { *LONG_OP_SINCE.lock().unwrap() = if name.is_some() { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) } else { 0.0 }; *LONG_OP.lock().unwrap() = name; }
    fn taskbar(&self, pct: Option<f64>) { timings::taskbar_pct(&self.caller, pct); }
}

/// The chain in flight (busy: true on /status, like the Python bridge's _LONG_OP).
static LONG_OP: Mutex<Option<String>> = Mutex::new(None);
static LONG_OP_SINCE: Mutex<f64> = Mutex::new(0.0);
/// (name, since) of the long op in flight: a chain, else a walk/search on the pass-through, else
/// any add-in request this exe has waited on for 5 s or more (a long script, an import, a close).
pub(crate) fn long_op_marker() -> Option<(String, f64)> {
    if let Some(n) = LONG_OP.lock().unwrap().clone() { return Some((n, *LONG_OP_SINCE.lock().unwrap())); }
    if let Some(m) = addin::long_command_active().and_then(|v| Some((v["command"].as_str()?.to_string(), v["startedAt"].as_f64()?))) { return Some(m); }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    addin::oldest_in_flight().filter(|(_, secs)| *secs >= 5.0).map(|(c, secs)| (c, now - secs))
}

fn chains_owner_is_exe(h: &dyn HostBoth) -> bool { owner_is_exe(h, "chainsOwner") }

fn addin_owner_is_exe(h: &dyn HostBoth) -> bool { owner_is_exe(h, "addinOwner") }

#[cfg(test)]
mod lint_library_tests {
    use super::*;
    #[test]
    fn a_library_read_by_fusion_lbr_is_judged_by_the_engine() {
        let sym = "META|LDO-X|reg|U\nMODE|0|n\nRECT|0|0|-300|600|0\nPIN|0|1|IN|0|-100|left|300||power_in\nPIN|0|2|OUT|600|-100|right|300||power_out\nPIN|0|3|EN|0|-200|left|300||input\n";
        let fp = "META|SOT-X|x\nPAD|1|-1|0|1|0.6|rect|0|top|0\nPAD|2|1|0|1|0.6|rect|0|top|0\nPAD|3|1|1|1|0.6|rect|0|top|0\nLINE|courtyard|-2|-1|2|1|0.05\n";
        let (xml, _) = fusion_lbr::import::import_part(None, Some(fp), Some(sym), None, &json!({})).unwrap();
        let r = lint_judge(fusion_lbr::authoring::lint_input(&xml).unwrap(), &json!({}));
        let rules: Vec<&str> = r["findings"].as_array().unwrap().iter().filter_map(|f| f["rule"].as_str()).collect();
        assert_eq!(r["errors"], 0, "{r}");
        assert!(rules.contains(&"no_silkscreen") && rules.contains(&"no_3d_body") && rules.contains(&"no_part_number"), "{rules:?}");
        assert!(!rules.contains(&"no_courtyard") && !rules.contains(&"pin_without_pad"), "{rules:?}");
    }
}

#[cfg(test)]
mod standard_alias_tests {
    #[test]
    fn standard_names_answer_through_the_bridges_own_verbs() {
        assert_eq!(super::standard_alias("fusion_launch"), Some("fusion_start"));
        assert_eq!(super::standard_alias("save"), Some("fusion_save_document"));
        assert_eq!(super::standard_alias("board_summary"), None); // answered in its own shape (stdshapes)
        assert_eq!(super::standard_alias("fusion_start"), None);
        assert_eq!(super::standard_alias("fusion_routing_state"), None);
        // every alias names a verb the frozen catalog serves
        let cat: serde_json::Value = serde_json::from_str(include_str!("../../../contracts/describe.json")).unwrap();
        let names: Vec<&str> = cat["verbs"].as_array().unwrap().iter().filter_map(|v| v["name"].as_str()).collect();
        for (std_name, own) in super::STANDARD_ALIASES {
            assert!(names.contains(own), "{own} is not in the catalog");
            assert!(names.contains(&format!("fusion_{std_name}").as_str()), "fusion_{std_name} is not described");
        }
    }
}

#[cfg(test)]
mod epg_param_tests {
    use super::*;
    fn dims(v: Value) -> serde_json::Map<String, Value> { v.as_object().cloned().unwrap() }
    #[test]
    fn each_generator_gets_pins_its_own_way() {
        // live 2026-10-28: SOIC-8 sent as 4 per side came out with 4 leads, QFP-48 as 12 per side with 24
        assert_eq!(epg_params("soic", dims(json!({"pins": 8})))["DPins"], 8);
        let q = epg_params("qfp", dims(json!({"pins": 48})));
        assert_eq!((q["DPins"].clone(), q["EPins"].clone()), (json!(24), json!(24)));
        let n = epg_params("qfn", dims(json!({"pins": 32, "D2": 3.3, "E2": 3.3})));
        assert_eq!((n["DPins"].clone(), n["EPins"].clone()), (json!(8), json!(8)));
        assert_eq!((n["D1"].clone(), n["E1"].clone(), n["thermal"].clone()), (json!(3.3), json!(3.3), json!(1)));
        assert!(n.get("D2").is_none());
        assert_eq!(epg_params("soic", dims(json!({"pins": 8, "D2": 2.3})))["thermal"], 1);
        assert_eq!(epg_params("soic", dims(json!({"pins": 8, "DPins": 6})))["DPins"], 6, "an explicit DPins wins");
        assert!(epg_params("bga", dims(json!({"pins": 64}))).get("DPins").is_none());
    }
}

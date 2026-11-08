//! The OS boundary: two traits, nothing else.

use std::path::PathBuf;

/// What a running Fusion looks like to the bridge.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FusionProcess {
    pub pid: u32,
    /// `Fusion360.exe` on Windows, `Autodesk Fusion` on macOS.
    pub image: String,
    pub parent_pid: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("Fusion is not installed on this machine: {0}")]
    NotInstalled(String),
    #[error("could not launch Fusion: {0}")]
    Launch(String),
    #[error("could not stop Fusion: {0}")]
    Stop(String),
    #[error("host operation failed: {0}")]
    Other(String),
}

/// Process-shaped things only the OS can answer. Keep it small: if a method could be an
/// Adom Bridge verb (`process_list`, `process_kill`, window enumeration, screenshots,
/// clicks, UIA), it does NOT belong here.
pub trait HostProcess: Send + Sync {
    /// The readiness POLL. Runs every few seconds for the life of the bridge, so it must be
    /// microseconds in-process (Toolhelp32 snapshot on Windows; `sysctl`/`proc_listpids`
    /// on macOS), never an HTTP round trip to ab.
    fn fusion_processes(&self) -> Vec<FusionProcess>;

    /// Launch Fusion detached from the bridge's own process tree so a bridge respawn never
    /// takes Fusion down with it (CREATE_BREAKAWAY_FROM_JOB on Windows; `open -a` on macOS).
    fn launch_fusion(&self) -> Result<FusionProcess, HostError>;

    /// Whether the bridge runs elevated (drives the installer's --globalinstall decision).
    fn is_elevated(&self) -> bool;

    /// Is any process with one of these image names running (case-insensitive)? The Fusion
    /// streamer / installer check on the readiness poll. In-process, like `fusion_processes`.
    fn image_running(&self, _names: &[&str]) -> bool { false }

    /// Tie a child (the Python sidecar during the port) to THIS process's lifetime, so a hard
    /// kill of the bridge by ab (every bridge_install, every supervisor respawn) takes the
    /// child with it instead of leaving an orphan on a random port. A Windows job object
    /// with KILL_ON_JOB_CLOSE (and BREAKAWAY_OK, so the child's own Fusion launch can still
    /// break away); a no-op elsewhere. Returns whether the tie took.
    fn tie_child_lifetime(&self, _pid: u32) -> bool { false }
    /// Guard the next `seconds` against Fusion-family focus theft (the background envelope:
    /// foreground-lock timeout raised, minimize animation off, a watcher that pushes any
    /// Fusion window that grabs the foreground back down unless the user is watching).
    /// Fire-and-forget; a no-op off Windows.
    fn background_envelope(&self, _seconds: f64, _minimize: bool) {}
    /// Is the user watching Fusion (they brought it forward themselves during an envelope)?
    fn user_watching(&self) -> bool { false }
    /// The OS's own hang verdict for a top-level window (IsHungAppWindow); false off Windows.
    fn window_hung(&self, _hwnd: i64) -> bool { false }
    /// Summed kernel+user CPU ticks (100ns units) of the Fusion family; 0 when unreadable.
    fn family_cpu_ticks(&self) -> u64 { 0 }
    /// Physical px per DIP for a window (GetDpiForWindow / 96 on Windows). The same ribbon
    /// control measured 296 DIP on a 4K box at 2.0 and a laptop at 1.5; rect fractions do not port.
    fn window_dpi_scale(&self, _hwnd: i64) -> f64 { 1.0 }
    /// Screen px of the window's CLIENT (0,0). Fusion draws its own title bar inside the client
    /// area and a maximized window's rect overhangs the screen, so rect.left/top is 13 px off.
    fn client_origin(&self, _hwnd: i64) -> (i32, i32) { (0, 0) }
    /// The work area (left, top, width, height) of the monitor a window sits on, and whether
    /// the window is maximized; None when unreadable. Used to grow a squint-sized Fusion window.
    fn window_work_area(&self, _hwnd: i64) -> Option<(i32, i32, i32, i32)> { None }
    fn window_is_maximized(&self, _hwnd: i64) -> bool { false }
}

/// Where things live on this OS. Every path the bridge ever touches comes from here.
pub trait HostPaths: Send + Sync {
    /// The Fusion launcher/executable, if installed (`webdeploy/production/<hash>/FusionLauncher.exe`
    /// on Windows; `/Applications/Autodesk Fusion.app` on macOS).
    fn fusion_executable(&self) -> Option<PathBuf>;
    /// Every add-in directory Fusion scans, newest convention first. Fusion has MOVED this
    /// across versions; the installer writes to all of them.
    fn addin_dirs(&self) -> Vec<PathBuf>;
    /// Fusion's own application log (`AppLogFile<stamp>.log`), the only place its CEF alert
    /// toasts are visible.
    fn fusion_log_dir(&self) -> Option<PathBuf>;
    /// The bridge's persistent state dir (`~/.adom/fusion-bridge`).
    fn bridge_state_dir(&self) -> PathBuf;
    /// A scratch dir for staged libraries, STEPs and screenshots.
    fn scratch_dir(&self) -> PathBuf;
    /// The mechanical operation journal and its dispatch handoff files, shared with the add-in
    /// (`%LOCALAPPDATA%/Adom/FusionBridge/mechanical` on Windows, where dev builds left history).
    fn mechanical_state_dir(&self) -> PathBuf;

    /// A fresh install is streaming: some webdeploy dir has the launcher but no complete
    /// Fusion360.exe exists anywhere yet (disk state, process-independent).
    fn fusion_install_streaming(&self) -> bool { false }
    /// Fusion is applying an auto-update: two or more launcher builds and one of them
    /// touched in the last ~20 minutes (stale leftover builds never trigger it).
    fn fusion_update_in_progress(&self) -> bool { false }
    /// A path that survives Fusion's space-delimited executeTextCommand (8.3 short form on
    /// Windows when the path has a space); the path itself elsewhere.
    fn short_path(&self, path: &str) -> String { path.replace('\\', "/") }
    /// `_clean_incomplete_webdeploy`: after a failed streamed launch, remove the partial
    /// launcher-bearing webdeploy dirs - and NOTHING when any complete install exists (the
    /// launcher-only stub beside a healthy app is legitimate). Returns the dir names removed.
    fn clean_incomplete_webdeploy(&self) -> Vec<String> { vec![] }
}

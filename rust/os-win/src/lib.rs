//! Windows host: the audited Win32 exceptions and nothing more.
//!  - Toolhelp32 snapshot for the readiness poll (in-process, microseconds, never a child
//!    process: a `tasklist` that stalled under a pegged CPU once launched a SECOND Fusion)
//!  - `cmd /c start "" /min <launcher>` with CREATE_BREAKAWAY_FROM_JOB for the Fusion launch,
//!    so a bridge respawn can never take Fusion down with it (issue #50: seven dead Fusions
//!    in one build matrix, every one a child of the bridge)
//!  - IsUserAnAdmin for the installer's --globalinstall decision
//!  - the webdeploy / FusionAddins / AppLogFile paths, ported 1:1 from fusion_detect.py,
//!    install_addin.py and server.py so the two shells agree during the overlap
//!
//! Everything here also compiles on Linux/macOS (the Win32 calls are cfg(windows)-gated and
//! answer "nothing running / not installed" elsewhere) so `tools/check-portable.sh` can
//! build and test the whole workspace on the dev container.
//!
//! AI hint: nothing in this crate may panic on a poll. A `todo!()` here once killed the
//! Rust shell on its very first /status request (tiny_http answered 500 for the request
//! dropped mid-unwind, then the port went dark). `tools/check-portable.sh` refuses `todo!`.

use fusion_core::host::{FusionProcess, HostError};
use fusion_core::{HostPaths, HostProcess};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct WindowsHost;

/// The ACTUAL app binary (~880 KB). Its presence at full size is the one reliable "installed"
/// signal: FusionLauncher.exe lands early mid-stream, and the launcher's .ini is absent from
/// the complete app dir, so neither of those may gate "installed".
const FUSION_APP_EXE: &str = "Fusion360.exe";
const MIN_APP_EXE_BYTES: u64 = 200_000;

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
}

fn home() -> PathBuf {
    env_dir("USERPROFILE").or_else(|| env_dir("HOME")).unwrap_or_else(|| PathBuf::from("C:\\Users\\Default"))
}

fn local_app_data() -> PathBuf {
    env_dir("LOCALAPPDATA").unwrap_or_else(|| home().join("AppData").join("Local"))
}

fn roaming_app_data() -> PathBuf {
    env_dir("APPDATA").unwrap_or_else(|| home().join("AppData").join("Roaming"))
}

/// Every profile under the users root (the parent of the home dir, normally `C:\Users`), so a
/// bridge running as one user still finds a Fusion installed or logging as another (issue #326).
fn user_profiles() -> Vec<PathBuf> {
    let root = home().parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("C:\\Users"));
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            if e.path().is_dir() { out.push(e.path()); }
        }
    }
    out
}

/// `<base>\Autodesk\webdeploy\production` for every profile, then the env-rooted bases.
fn webdeploy_bases() -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = user_profiles().into_iter()
        .map(|u| u.join("AppData").join("Local").join("Autodesk").join("webdeploy").join("production"))
        .collect();
    for var in ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(root) = env_dir(var) {
            let b = root.join("Autodesk").join("webdeploy").join("production");
            if !bases.contains(&b) { bases.push(b); }
        }
    }
    bases
}

fn app_dir_complete(sub: &Path) -> bool {
    std::fs::metadata(sub.join(FUSION_APP_EXE)).map(|m| m.is_file() && m.len() >= MIN_APP_EXE_BYTES).unwrap_or(false)
}

fn mtime_secs(p: &Path) -> u64 {
    std::fs::metadata(p).and_then(|m| m.modified())
        .ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs()).unwrap_or(0)
}

fn subdirs(base: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(base).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default()
}

/// Is Fusion installed at all: ANY hash dir holds a complete Fusion360.exe.
pub fn any_app_complete() -> bool {
    webdeploy_bases().iter().any(|b| subdirs(b).iter().any(|s| app_dir_complete(s)))
}

/// The FusionLauncher.exe to RUN, only once Fusion is actually installed. Two separate
/// questions (conflating them broke the launch once): "installed" is gated on Fusion360.exe;
/// "which launcher" prefers a FusionLauncher.exe WITH its .ini beside it (a bare launcher
/// throws Fusion's own "FusionLauncher.exe.ini is missing or incomplete"), then a bare
/// launcher, then the app exe itself. Newest mtime wins inside each pool.
fn find_fusion_launcher() -> Option<PathBuf> {
    if !any_app_complete() { return None; }
    let (mut runnable, mut bare, mut app_exes): (Vec<(u64, PathBuf)>, Vec<(u64, PathBuf)>, Vec<(u64, PathBuf)>) = (vec![], vec![], vec![]);
    for base in webdeploy_bases() {
        for sub in subdirs(&base) {
            let mt = mtime_secs(&sub);
            let launcher = sub.join("FusionLauncher.exe");
            if launcher.is_file() {
                if sub.join("FusionLauncher.exe.ini").is_file() { runnable.push((mt, launcher)); } else { bare.push((mt, launcher)); }
            }
            if app_dir_complete(&sub) { app_exes.push((mt, sub.join(FUSION_APP_EXE))); }
        }
    }
    for pool in [&mut runnable, &mut bare, &mut app_exes] {
        if !pool.is_empty() {
            pool.sort_by(|a, b| b.0.cmp(&a.0));
            return Some(pool[0].1.clone());
        }
    }
    None
}

#[cfg(windows)]
fn snapshot_images() -> Vec<(u32, u32, String)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    // SAFETY: same Toolhelp32 walk as snapshot_fusion, over a handle we own and close.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE || snap.is_null() { return out; }
        let mut pe: PROCESSENTRY32W = std::mem::zeroed();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut pe) != 0 {
            loop {
                let len = pe.szExeFile.iter().position(|&c| c == 0).unwrap_or(pe.szExeFile.len());
                out.push((pe.th32ProcessID, pe.th32ParentProcessID, String::from_utf16_lossy(&pe.szExeFile[..len])));
                if Process32NextW(snap, &mut pe) == 0 { break; }
            }
        }
        CloseHandle(snap);
    }
    out
}
#[cfg(not(windows))]
fn snapshot_images() -> Vec<(u32, u32, String)> { Vec::new() }

#[cfg(windows)]
fn snapshot_fusion() -> Vec<FusionProcess> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    // SAFETY: plain Toolhelp32 walk over a snapshot handle we own and close; the entry struct
    // is zeroed and sized before the first call, as the API requires.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE || snap.is_null() { return out; }
        let mut pe: PROCESSENTRY32W = std::mem::zeroed();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut pe) != 0 {
            loop {
                let len = pe.szExeFile.iter().position(|&c| c == 0).unwrap_or(pe.szExeFile.len());
                let name = String::from_utf16_lossy(&pe.szExeFile[..len]);
                if name.eq_ignore_ascii_case(FUSION_APP_EXE) {
                    out.push(FusionProcess { pid: pe.th32ProcessID, image: name, parent_pid: Some(pe.th32ParentProcessID) });
                }
                if Process32NextW(snap, &mut pe) == 0 { break; }
            }
        }
        CloseHandle(snap);
    }
    out
}

#[cfg(not(windows))]
fn snapshot_fusion() -> Vec<FusionProcess> { Vec::new() }

#[cfg(windows)]
fn start_shim(exe: &Path) -> Result<u32, HostError> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let base = CREATE_NO_WINDOW | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    // `start "" /min <exe>`: the empty "" is the window-title slot start expects first. cmd exits
    // at once, so the launcher's parent is gone and no tree walk from the bridge reaches Fusion.
    let spawn = |flags: u32| Command::new("cmd").args(["/c", "start", "", "/min"]).arg(exe)
        .creation_flags(flags).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    match spawn(base | CREATE_BREAKAWAY_FROM_JOB) {
        Ok(c) => Ok(c.id()),
        // The job forbids breakaway (ERROR_ACCESS_DENIED): still reparented via cmd.
        Err(_) => spawn(base).map(|c| c.id()).map_err(|e| HostError::Launch(e.to_string())),
    }
}

#[cfg(not(windows))]
fn start_shim(_exe: &Path) -> Result<u32, HostError> {
    Err(HostError::Launch("the Windows host only launches Fusion on Windows".into()))
}

impl HostProcess for WindowsHost {
    fn window_dpi_scale(&self, hwnd: i64) -> f64 { window_dpi_scale(hwnd) }
    fn client_origin(&self, hwnd: i64) -> (i32, i32) { client_origin(hwnd) }
    fn window_work_area(&self, hwnd: i64) -> Option<(i32, i32, i32, i32)> { window_work_area(hwnd) }
    fn window_is_maximized(&self, hwnd: i64) -> bool { window_is_maximized(hwnd) }
    fn fusion_processes(&self) -> Vec<FusionProcess> { snapshot_fusion() }

    fn image_running(&self, names: &[&str]) -> bool {
        snapshot_images().iter().any(|(_, _, img)| names.iter().any(|n| n.eq_ignore_ascii_case(img)))
    }

    fn launch_fusion(&self) -> Result<FusionProcess, HostError> {
        let exe = self.fusion_executable().ok_or_else(|| HostError::NotInstalled(
            "no webdeploy hash dir holds a complete Fusion360.exe".into()))?;
        let shim = start_shim(&exe)?;
        // Fusion360.exe appears a few seconds after the launcher; report the real process
        // when it shows, else the shim pid so the caller can still poll.
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(8) {
            if let Some(p) = snapshot_fusion().into_iter().next() { return Ok(p); }
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(FusionProcess { pid: shim, image: "cmd.exe (start shim; Fusion360.exe not visible yet)".into(), parent_pid: None })
    }

    fn background_envelope(&self, seconds: f64, minimize: bool) { envelope::begin(seconds, minimize) }
    fn window_hung(&self, hwnd: i64) -> bool { hang::window_hung(hwnd) }
    fn family_cpu_ticks(&self) -> u64 { hang::family_cpu_ticks() }
    fn user_watching(&self) -> bool { envelope::user_watching() }
    #[cfg(windows)]
    fn tie_child_lifetime(&self, pid: u32) -> bool {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};
        // SAFETY: a job handle we deliberately never close (its close is what kills the child
        // when THIS process dies), a zeroed limit struct sized for the call, a process handle
        // we close after the assignment.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() { return false; }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
            if SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _,
                                       std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32) == 0 { return false; }
            let ph = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if ph.is_null() { return false; }
            let ok = AssignProcessToJobObject(job, ph) != 0;
            CloseHandle(ph);
            ok
        }
    }

    #[cfg(windows)]
    fn is_elevated(&self) -> bool {
        // SAFETY: no arguments, no memory handed over; a plain BOOL answer.
        unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
    }
    #[cfg(not(windows))]
    fn is_elevated(&self) -> bool { false }
}

impl HostPaths for WindowsHost {
    fn fusion_executable(&self) -> Option<PathBuf> { find_fusion_launcher() }

    /// Newest convention first. Fusion MOVED this dir across versions and silently ignores the
    /// others (issue #63), so the installer writes to every one that exists.
    fn addin_dirs(&self) -> Vec<PathBuf> {
        let r = roaming_app_data().join("Autodesk");
        vec![
            r.join("FusionAddins"),
            r.join("Autodesk Fusion").join("API").join("AddIns"),
            r.join("Autodesk Fusion 360").join("API").join("AddIns"),
        ]
    }

    /// The `logs` dir holding the newest AppLogFile*.log across every profile's
    /// `Autodesk Fusion[ 360]` tree; None when Fusion has never logged here.
    fn fusion_log_dir(&self) -> Option<PathBuf> {
        let mut bases = vec![];
        for nm in ["Autodesk Fusion 360", "Autodesk Fusion"] {
            bases.push(local_app_data().join("Autodesk").join(nm));
        }
        for u in user_profiles() {
            for nm in ["Autodesk Fusion 360", "Autodesk Fusion"] {
                bases.push(u.join("AppData").join("Local").join("Autodesk").join(nm));
            }
        }
        let mut best: Option<(u64, PathBuf)> = None;
        for base in bases {
            for entry in subdirs(&base) {
                let logs = entry.join("logs");
                let Ok(rd) = std::fs::read_dir(&logs) else { continue };
                for f in rd.flatten() {
                    let name = f.file_name().to_string_lossy().to_string();
                    if name.starts_with("AppLogFile") && name.ends_with(".log") {
                        let m = mtime_secs(&f.path());
                        if best.as_ref().map(|b| m > b.0).unwrap_or(true) { best = Some((m, logs.clone())); }
                    }
                }
            }
        }
        best.map(|b| b.1)
    }

    fn bridge_state_dir(&self) -> PathBuf { home().join(".adom").join("fusion-bridge") }

    fn short_path(&self, path: &str) -> String { short_path(path) }
    fn clean_incomplete_webdeploy(&self) -> Vec<String> {
        if any_app_complete() { return vec![]; }
        let mut removed = Vec::new();
        for base in webdeploy_bases() {
            for sub in subdirs(&base) {
                if sub.join("FusionLauncher.exe").is_file() && !app_dir_complete(&sub) && std::fs::remove_dir_all(&sub).is_ok() { removed.push(sub.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()); }
            }
        }
        removed
    }
    fn fusion_install_streaming(&self) -> bool {
        if any_app_complete() { return false; }
        webdeploy_bases().iter().any(|b| subdirs(b).iter().any(|s| s.join("FusionLauncher.exe").is_file()))
    }

    fn fusion_update_in_progress(&self) -> bool {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut mtimes = Vec::new();
        for b in webdeploy_bases() { for s in subdirs(&b) { if s.join("FusionLauncher.exe").is_file() { mtimes.push(mtime_secs(&s)); } } }
        mtimes.len() >= 2 && mtimes.iter().any(|m| now.saturating_sub(*m) < 20 * 60)
    }

    fn scratch_dir(&self) -> PathBuf {
        env_dir("TEMP").unwrap_or_else(|| local_app_data().join("Temp")).join("adom-fusion")
    }

    fn mechanical_state_dir(&self) -> PathBuf { local_app_data().join("Adom").join("FusionBridge").join("mechanical") }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_never_panic_off_windows() {
        let h = WindowsHost;
        let _ = h.fusion_executable();
        assert_eq!(h.addin_dirs().len(), 3);
        let _ = h.fusion_log_dir();
        assert!(h.bridge_state_dir().ends_with("fusion-bridge"));
        assert!(h.scratch_dir().ends_with("adom-fusion"));
        assert!(h.fusion_processes().is_empty() || cfg!(windows));
    }
}


/// The background envelope, ported from fusion_detect.py (`begin_background_envelope` and
/// `_envelope_watch`): while a verb may raise a Fusion window, Windows is told to DENY foreground
/// changes (SPI_SETFOREGROUNDLOCKTIMEOUT 300000, LockSetForegroundWindow), the minimize animation
/// is off so a pushed-down window vanishes in one frame, and a watcher thread pushes any
/// Fusion-family window that reaches the foreground to the bottom (or minimizes it, for a launch)
/// unless the USER brought it forward (recent input) - then the foreground is theirs and the
/// watcher stands down until they leave it. Nested envelopes refcount; the last one out restores
/// the user's settings.
pub mod envelope {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    static USER_WATCHING: AtomicBool = AtomicBool::new(false);
    #[allow(dead_code)]
    static STATE: Mutex<(u32, Option<u32>, Option<i32>, bool)> = Mutex::new((0, None, None, false));
    pub fn user_watching() -> bool { USER_WATCHING.load(Ordering::Relaxed) }

    #[cfg(not(windows))]
    pub fn begin(_seconds: f64, _minimize: bool) {}

    #[cfg(windows)]
    pub fn begin(seconds: f64, minimize: bool) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{LockSetForegroundWindow, LSFW_LOCK};
        {
            let mut st = STATE.lock().unwrap();
            st.0 += 1;
            if st.0 == 1 {
                st.1 = win::get_fg_lock_timeout();
                st.2 = win::get_min_animate();
                win::set_fg_lock_timeout(300_000);
                win::set_min_animate(0);
                // SAFETY: plain user32 call with a documented flag.
                st.3 = unsafe { LockSetForegroundWindow(LSFW_LOCK) } != 0;
            }
        }
        std::thread::spawn(move || { watch(seconds, minimize); release(); });
    }

    #[cfg(windows)]
    fn release() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{LockSetForegroundWindow, LSFW_UNLOCK};
        let mut st = STATE.lock().unwrap();
        st.0 = st.0.saturating_sub(1);
        if st.0 == 0 {
            if let Some(fg) = st.1 { win::set_fg_lock_timeout(fg); }
            if let Some(a) = st.2 { win::set_min_animate(a); }
            // SAFETY: as above.
            if st.3 { unsafe { LockSetForegroundWindow(LSFW_UNLOCK); } }
        }
    }

    #[cfg(windows)]
    fn family_pids() -> std::collections::HashSet<u32> {
        super::snapshot_images().into_iter().filter(|(_, _, img)| fusion_core::windows_facts::FAMILY_IMAGES.iter().any(|f| f.eq_ignore_ascii_case(img))).map(|(pid, _, _)| pid).collect()
    }

    #[cfg(windows)]
    fn watch(seconds: f64, minimize: bool) {
        use std::time::{Duration, Instant};
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, SetWindowPos, ShowWindowAsync, HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOWMINNOACTIVE};
        let flags = SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE;
        let fg_pid = |hwnd: windows_sys::Win32::Foundation::HWND| -> u32 { let mut pid = 0u32; unsafe { GetWindowThreadProcessId(hwnd, &mut pid); } pid };
        let end = Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
        let mut pids = family_pids();
        let mut last_refresh = Instant::now();
        let mut prev_was_fusion = false;
        let mut away_since: Option<Instant> = None;
        let mut away_tick: Option<u32> = None;
        // SAFETY: GetForegroundWindow / GetWindowThreadProcessId / SetWindowPos / ShowWindowAsync
        // are plain user32 calls on handles we only read; nothing is dereferenced.
        unsafe {
            let fg0 = GetForegroundWindow();
            if !fg0.is_null() && !pids.is_empty() && pids.contains(&fg_pid(fg0)) { USER_WATCHING.store(true, Ordering::Relaxed); prev_was_fusion = true; }
            while Instant::now() < end {
                if last_refresh.elapsed() > Duration::from_secs(2) { pids = family_pids(); last_refresh = Instant::now(); }
                let fg = GetForegroundWindow();
                let fg_is_fusion = !fg.is_null() && !pids.is_empty() && pids.contains(&fg_pid(fg));
                if fg_is_fusion {
                    away_since = None;
                    if !prev_was_fusion && win::user_active_recently(5000) { USER_WATCHING.store(true, Ordering::Relaxed); }
                    if !USER_WATCHING.load(Ordering::Relaxed) {
                        if minimize { ShowWindowAsync(fg, SW_SHOWMINNOACTIVE); } else { SetWindowPos(fg, HWND_BOTTOM, 0, 0, 0, 0, flags); }
                    }
                } else if !fg.is_null() {
                    match away_since {
                        None => { away_since = Some(Instant::now()); away_tick = win::last_input_tick(); }
                        Some(t) if USER_WATCHING.load(Ordering::Relaxed) && t.elapsed() >= Duration::from_secs(3) => {
                            let now_tick = win::last_input_tick();
                            if let (Some(n), Some(a)) = (now_tick, away_tick) { if n != a { USER_WATCHING.store(false, Ordering::Relaxed); } }
                        }
                        _ => {}
                    }
                }
                prev_was_fusion = fg_is_fusion;
                std::thread::sleep(Duration::from_millis(50));
            }
            // a 20s grace: keep pushing a Fusion window down until the user acts or it stays away
            let grace_end = Instant::now() + Duration::from_secs(20);
            while Instant::now() < grace_end {
                if win::user_active_recently(5000) || USER_WATCHING.load(Ordering::Relaxed) { break; }
                let fg = GetForegroundWindow();
                if fg.is_null() || pids.is_empty() || !pids.contains(&fg_pid(fg)) { break; }
                SetWindowPos(fg, HWND_BOTTOM, 0, 0, 0, 0, flags);
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }

    #[cfg(windows)]
    mod win {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, ANIMATIONINFO, SPI_GETANIMATION, SPI_GETFOREGROUNDLOCKTIMEOUT, SPI_SETANIMATION, SPI_SETFOREGROUNDLOCKTIMEOUT};
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
        use windows_sys::Win32::System::SystemInformation::GetTickCount;
        // SAFETY (all four): SystemParametersInfoW with the documented parameter shapes; the
        // out-params are stack locals of the right size.
        pub fn get_fg_lock_timeout() -> Option<u32> { let mut v: u32 = 0; unsafe { if SystemParametersInfoW(SPI_GETFOREGROUNDLOCKTIMEOUT, 0, &mut v as *mut u32 as *mut _, 0) != 0 { Some(v) } else { None } } }
        pub fn set_fg_lock_timeout(ms: u32) { unsafe { SystemParametersInfoW(SPI_SETFOREGROUNDLOCKTIMEOUT, 0, ms as usize as *mut _, 0); } }
        pub fn get_min_animate() -> Option<i32> { let mut ai: ANIMATIONINFO = unsafe { std::mem::zeroed() }; ai.cbSize = std::mem::size_of::<ANIMATIONINFO>() as u32; unsafe { if SystemParametersInfoW(SPI_GETANIMATION, ai.cbSize, &mut ai as *mut _ as *mut _, 0) != 0 { Some(ai.iMinAnimate) } else { None } } }
        pub fn set_min_animate(v: i32) { let mut ai: ANIMATIONINFO = unsafe { std::mem::zeroed() }; ai.cbSize = std::mem::size_of::<ANIMATIONINFO>() as u32; ai.iMinAnimate = v; unsafe { SystemParametersInfoW(SPI_SETANIMATION, ai.cbSize, &mut ai as *mut _ as *mut _, 0); } }
        pub fn last_input_tick() -> Option<u32> { let mut lii: LASTINPUTINFO = unsafe { std::mem::zeroed() }; lii.cbSize = std::mem::size_of::<LASTINPUTINFO>() as u32; unsafe { if GetLastInputInfo(&mut lii) != 0 { Some(lii.dwTime) } else { None } } }
        pub fn user_active_recently(window_ms: u32) -> bool { match last_input_tick() { Some(t) => unsafe { GetTickCount() }.wrapping_sub(t) < window_ms, None => false } }
    }
}


/// Two audited Win32 reads for the hang gate (fusion_ui.is_fusion_hung, fusion_detect.fusion_using_cpu):
/// IsHungAppWindow, the shell's own verdict, and GetProcessTimes over the launch family so a
/// GIL-starved-but-working Fusion (CPU advancing) is never mistaken for a wedged one.
pub mod hang {
    #[cfg(not(windows))]
    pub fn window_hung(_hwnd: i64) -> bool { false }
    #[cfg(not(windows))]
    pub fn family_cpu_ticks() -> u64 { 0 }
    #[cfg(windows)]
    pub fn window_hung(hwnd: i64) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::IsHungAppWindow;
        // SAFETY: a plain user32 read on a handle we only pass through.
        unsafe { IsHungAppWindow(hwnd as *mut _) != 0 }
    }
    #[cfg(windows)]
    pub fn family_cpu_ticks() -> u64 {
        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
        use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let pids: Vec<u32> = super::snapshot_images().into_iter().filter(|(_, _, img)| fusion_core::windows_facts::FAMILY_IMAGES.iter().any(|f| f.eq_ignore_ascii_case(img))).map(|(p, _, _)| p).collect();
        let mut total = 0u64;
        for pid in pids {
            // SAFETY: a limited-information handle we close; four zeroed FILETIMEs of the right size.
            unsafe {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if h.is_null() { continue; }
                let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = (std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed());
                if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) != 0 {
                    total += (k.dwLowDateTime as u64) | ((k.dwHighDateTime as u64) << 32);
                    total += (u.dwLowDateTime as u64) | ((u.dwHighDateTime as u64) << 32);
                }
                CloseHandle(h);
            }
        }
        total
    }
}


/// `_fusion_text_path`: Fusion's executeTextCommand is space-delimited, and the quoted form
/// fails with error 3; the 8.3 short path with forward slashes opens first try.
#[cfg(windows)]
pub fn short_path(path: &str) -> String {
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let p = path.replace('\\', "/");
    if !p.contains(' ') { return p; }
    let wide: Vec<u16> = p.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = vec![0u16; 1024];
    // SAFETY: a NUL-terminated UTF-16 input and an output buffer of the stated length.
    let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() { return p; }
    let s = String::from_utf16_lossy(&buf[..n]).replace('\\', "/");
    if s.contains(' ') { p } else { s }
}
#[cfg(not(windows))]
pub fn short_path(path: &str) -> String { path.replace('\\', "/") }

/// `_window_dpi_scale`: GetDpiForWindow / 96, else the nearest monitor's effective DPI, else 1.0.
#[cfg(windows)]
pub fn window_dpi_scale(hwnd: i64) -> f64 {
    use windows_sys::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
    let h = hwnd as isize as windows_sys::Win32::Foundation::HWND;
    unsafe {
        let dpi = GetDpiForWindow(h);
        if dpi > 0 { return dpi as f64 / 96.0; }
        let mon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let (mut x, mut y) = (0u32, 0u32);
        if !mon.is_null() && GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut x, &mut y) == 0 && x > 0 { return x as f64 / 96.0; }
    }
    1.0
}
#[cfg(not(windows))]
pub fn window_dpi_scale(_hwnd: i64) -> f64 { 1.0 }

/// `_client_origin`: ClientToScreen of (0,0), the honest origin for ribbon DIP offsets.
#[cfg(windows)]
pub fn client_origin(hwnd: i64) -> (i32, i32) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    let mut pt = POINT { x: 0, y: 0 };
    if unsafe { ClientToScreen(hwnd as isize as windows_sys::Win32::Foundation::HWND, &mut pt) } != 0 { (pt.x, pt.y) } else { (0, 0) }
}
#[cfg(not(windows))]
pub fn client_origin(_hwnd: i64) -> (i32, i32) { (0, 0) }

#[cfg(windows)]
pub fn window_work_area(hwnd: i64) -> Option<(i32, i32, i32, i32)> {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    unsafe {
        let mon = MonitorFromWindow(hwnd as isize as windows_sys::Win32::Foundation::HWND, MONITOR_DEFAULTTONEAREST);
        if mon.is_null() { return None; }
        let mut mi: MONITORINFO = std::mem::zeroed(); mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 { return None; }
        let r = mi.rcWork; Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
    }
}
#[cfg(not(windows))]
pub fn window_work_area(_hwnd: i64) -> Option<(i32, i32, i32, i32)> { None }
#[cfg(windows)]
pub fn window_is_maximized(hwnd: i64) -> bool { unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsZoomed(hwnd as isize as windows_sys::Win32::Foundation::HWND) != 0 } }
#[cfg(not(windows))]
pub fn window_is_maximized(_hwnd: i64) -> bool { false }

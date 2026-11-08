//! macOS host: the same two traits as `os-win`, translated rather than rediscovered.
//!
//! Verified against a real Mac (2026-11-08) — these are measurements, not guesses:
//!   app       ~/Applications/Autodesk Fusion.app     <- FIRST; /Applications is the fallback
//!   add-ins   ~/Library/Application Support/Autodesk/{FusionAddins,
//!                                                    Autodesk Fusion/API/AddIns,
//!                                                    Autodesk Fusion 360/API/AddIns}
//!   logs      ~/Library/Application Support/Autodesk/Autodesk Fusion 360/<USER_ID>/logs
//!   state     ~/.adom/fusion-bridge
//!   scratch   $TMPDIR/adom-fusion
//!
//! ⛔ nothing in this crate may panic on a poll. `os-win`'s header explains why: a `todo!()`
//! reached from /status killed the Rust shell on its first request. Every method below always
//! answers; the "unknown" case is an empty Vec / None, never a panic.
//!
//! Two traps cost real time in the Python port (`handlers/close_fusion.py`,
//! `fusion_detect.py`) and are guarded here:
//!
//!  1. NEVER match Fusion by substring. After Fusion exits, its bundle leaves ORPHANED helpers
//!     behind (`cer_service`, `cer_dialog`, `ADPClientService`, `node`, `QtWebEngineProcess`)
//!     whose argv still contains ".../Autodesk Fusion.app/...". A substring match on
//!     "Autodesk Fusion" reported Fusion running with Fusion closed, so the bridge refused to
//!     launch it. Match the EXECUTABLE NAME.
//!  2. `ps` truncates its command column at 100 characters and the app lives under
//!     "Application Support" — a path with a SPACE in it. Never split a path on whitespace.

use fusion_core::host::{FusionProcess, HostError};
use fusion_core::{HostPaths, HostProcess};
use std::path::PathBuf;

pub struct MacHost;

/// CFBundleExecutable of the app. This is what `pgrep -x` matches and what `ps -o comm=`
/// reports for the real process; every helper in the bundle has a different one.
const FUSION_APP_BINARY: &str = "Autodesk Fusion";

/// The bundle directory name, used to tell a real Fusion bundle from any other app.
const FUSION_APP_BUNDLE: &str = "Autodesk Fusion.app";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
}

/// Every app bundle path Fusion can live at, most likely first. On the machine this was
/// measured on the app is under `~/Applications`, NOT `/Applications` — the Python detector
/// missed it for exactly that reason.
fn app_bundle_candidates() -> Vec<PathBuf> {
    let mut out = vec![
        home().join("Applications").join(FUSION_APP_BUNDLE),
        PathBuf::from("/Applications").join(FUSION_APP_BUNDLE),
    ];
    // Autodesk's own updater stream keeps a per-build copy here; used as a last resort.
    let webdeploy = home()
        .join("Library")
        .join("Application Support")
        .join("Autodesk")
        .join("webdeploy")
        .join("production");
    if let Ok(rd) = std::fs::read_dir(&webdeploy) {
        let mut builds: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        builds.sort();
        builds.reverse(); // newest build hash first
        for b in builds {
            let p = b.join(FUSION_APP_BUNDLE);
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// `~/Library/Application Support/Autodesk`
fn autodesk_support() -> PathBuf {
    home()
        .join("Library")
        .join("Application Support")
        .join("Autodesk")
}

/// Run `argv`, returning stdout on success. Never panics; any failure is an empty string.
fn run(argv: &[&str]) -> String {
    std::process::Command::new(argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

impl HostProcess for MacHost {
    /// The readiness POLL. In-process, microseconds: `pgrep -x` matches the executable name
    /// exactly, which no orphaned helper shares. Never a substring match (trap 1 above).
    fn fusion_processes(&self) -> Vec<FusionProcess> {
        let out = run(&["pgrep", "-x", FUSION_APP_BINARY]);
        let mut v = Vec::new();
        for tok in out.split_whitespace() {
            let Ok(pid) = tok.parse::<u32>() else { continue };
            // Confirm it really is the app and not something that shares the display name:
            // ask for the UNTRUNCATED command line (`ps -ww`) and require it to END with the
            // bundle binary path (trap 2: no whitespace splitting, "Application Support" has
            // a space).
            let cmd = run(&["ps", "-ww", "-p", &pid.to_string(), "-o", "command="]);
            let cmd = cmd.trim_end_matches(['\n', '\r']);
            let suffix = format!("/Contents/MacOS/{FUSION_APP_BINARY}");
            if !cmd.ends_with(&suffix) {
                continue;
            }
            v.push(FusionProcess {
                pid,
                image: FUSION_APP_BINARY.to_string(),
                parent_pid: None,
            });
        }
        v
    }

    /// `open -a` hands the launch to launchd, so the child is already outside the bridge's
    /// process tree — macOS has no job-object equivalent and needs none.
    fn launch_fusion(&self) -> Result<FusionProcess, HostError> {
        let Some(app) = self.fusion_executable() else {
            return Err(HostError::NotInstalled(
                "no Autodesk Fusion.app in ~/Applications, /Applications or the webdeploy tree".into(),
            ));
        };
        let status = std::process::Command::new("open")
            .arg("-a")
            .arg(&app)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| HostError::Launch(format!("could not run `open -a {}`: {e}", app.display())))?;
        if !status.success() {
            return Err(HostError::Launch(format!(
                "`open -a {}` exited {}",
                app.display(),
                status.code().map(|c| c.to_string()).unwrap_or_else(|| "by signal".into())
            )));
        }
        // `open` returns once launchd accepted the launch; the process may not exist yet, so
        // report the launch, not a pid we could not have.
        Ok(FusionProcess {
            pid: 0,
            image: FUSION_APP_BINARY.to_string(),
            parent_pid: None,
        })
    }

    /// `geteuid() == 0`.
    ///
    /// The first attempt stat'd `/` and read its owner uid — WRONG: `/` is always owned by
    /// uid 0, so that answers `true` for every user. It reported `is_elevated: true` for a
    /// normal uid-501 account on the reference Mac. `geteuid` is the only correct answer and
    /// needs `libc`, which `os-mac` alone may take (see Cargo.toml).
    fn is_elevated(&self) -> bool {
        // SAFETY: geteuid takes no arguments, touches no memory and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }
}

impl HostPaths for MacHost {
    /// A bundle, not an executable: `open -a` wants the `.app`. Every candidate must actually
    /// carry `Contents/MacOS/Autodesk Fusion` so a half-streamed install is not "installed".
    fn fusion_executable(&self) -> Option<PathBuf> {
        let suffix = format!("/Contents/MacOS/{FUSION_APP_BINARY}");
        for cand in app_bundle_candidates() {
            let bin = format!("{}{}", cand.display(), suffix);
            let p = PathBuf::from(&bin);
            if p.is_file() {
                return Some(cand);
            }
        }
        None
    }

    /// Every add-in convention Fusion scans, newest first. Written to ALL of them by the
    /// installer: an add-in in the wrong directory is silently ignored (upstream issue #63).
    fn addin_dirs(&self) -> Vec<PathBuf> {
        let base = autodesk_support();
        vec![
            base.join("FusionAddins"),
            base.join("Autodesk Fusion").join("API").join("AddIns"),
            base.join("Autodesk Fusion 360").join("API").join("AddIns"),
        ]
    }

    /// `~/Library/Application Support/Autodesk/Autodesk Fusion 360/<USER_ID>/logs`.
    /// `<USER_ID>` is an opaque per-user token Autodesk generates; find it by scanning, and
    /// prefer the one that actually has a logs directory.
    fn fusion_log_dir(&self) -> Option<PathBuf> {
        let root = autodesk_support().join("Autodesk Fusion 360");
        let rd = std::fs::read_dir(&root).ok()?;
        let mut with_logs: Vec<PathBuf> = Vec::new();
        let mut any: Vec<PathBuf> = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            // Skip the state files that sit beside the user dirs.
            if p.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(false) {
                continue;
            }
            any.push(p.clone());
            if p.join("logs").is_dir() {
                with_logs.push(p);
            }
        }
        with_logs.sort();
        if let Some(p) = with_logs.pop() {
            return Some(p.join("logs"));
        }
        any.sort();
        any.pop()
    }

    /// `~/.adom/fusion-bridge` — the same path the Python shell and the prefs use, so the two
    /// shells agree during the overlap.
    fn bridge_state_dir(&self) -> PathBuf {
        home().join(".adom").join("fusion-bridge")
    }

    /// `$TMPDIR/adom-fusion`, falling back to `/tmp`. On macOS `$TMPDIR` is a per-user,
    /// per-session sandbox path; the Python shell stages there too.
    fn scratch_dir(&self) -> PathBuf {
        env_dir("TMPDIR")
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("adom-fusion")
    }

    /// Not a `todo!()`: the mechanical verbs read this before anything else, so a panic here
    /// would take the bridge down on a Mac even for `fusion_mech_capabilities`. It is the
    /// add-in's journal root and must match `commands/mechanical/journal.py`.
    fn mechanical_state_dir(&self) -> PathBuf {
        home()
            .join("Library")
            .join("Application Support")
            .join("Adom")
            .join("FusionBridge")
            .join("mechanical")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mechanical_journal_lives_where_the_addin_writes_it() {
        let p = MacHost.mechanical_state_dir();
        assert!(
            p.ends_with("Library/Application Support/Adom/FusionBridge/mechanical"),
            "{}",
            p.display()
        );
    }

    #[test]
    fn state_and_scratch_have_no_todo_left() {
        // These two are read on the very first /status request; a panic here is a dead bridge.
        let state = MacHost.bridge_state_dir();
        assert!(state.ends_with(".adom/fusion-bridge"), "{}", state.display());
        let scratch = MacHost.scratch_dir();
        assert!(scratch.ends_with("adom-fusion"), "{}", scratch.display());
    }

    #[test]
    fn addin_dirs_are_the_three_verified_conventions_newest_first() {
        let d = MacHost.addin_dirs();
        assert_eq!(d.len(), 3, "{d:?}");
        assert!(d[0].ends_with("Autodesk/FusionAddins"), "{:?}", d[0]);
        assert!(d[1].ends_with("Autodesk Fusion/API/AddIns"), "{:?}", d[1]);
        assert!(d[2].ends_with("Autodesk Fusion 360/API/AddIns"), "{:?}", d[2]);
        for p in &d {
            assert!(p.is_absolute(), "{}", p.display());
        }
    }

    #[test]
    fn trait_1_the_app_under_home_applications_is_found_before_slash_applications() {
        // Measured: the app is in ~/Applications on the reference Mac. The candidate list must
        // ask there FIRST, otherwise a /Applications copy of another build shadows it.
        let c = app_bundle_candidates();
        assert!(c[0].starts_with(home()), "{:?}", c[0]);
        assert_eq!(c[0].file_name().unwrap(), "Autodesk Fusion.app");
    }

    #[test]
    fn trait_2_an_orphan_helper_is_never_reported_as_fusion() {
        // The bundle leaves helpers behind (cer_service, cer_dialog, ADPClientService, node)
        // whose argv contains "/Autodesk Fusion.app/". Matching on substring made the Python
        // detector answer "Fusion is running" with Fusion closed, so launch was refused.
        // Here: a process named like the app but with a DIFFERENT executable path must fail
        // the suffix test the poll applies.
        let helper = "/Users/x/Library/Application Support/Autodesk/webdeploy/production/h/\
                      Autodesk Fusion.app/Contents/Libraries/CER/service/cer_service";
        let suffix = format!("/Contents/MacOS/{FUSION_APP_BINARY}");
        assert!(!helper.ends_with(&suffix), "helper must not match the app suffix");

        let real = "/Users/x/Library/Application Support/Autodesk/webdeploy/production/h/\
                    Autodesk Fusion.app/Contents/MacOS/Autodesk Fusion";
        assert!(real.ends_with(&suffix), "the real app must match");
    }

    #[test]
    fn trait_2_a_path_with_a_space_is_not_split() {
        // "Application Support" has a space in it. The Python version once split the command
        // on whitespace and got "/Users/me/Library/Application", so every match failed.
        let cmd = "/Users/x/Library/Application Support/Autodesk/w/Autodesk Fusion.app/\
                   Contents/MacOS/Autodesk Fusion";
        assert!(cmd.contains(' '), "the fixture must exercise the space");
        // The suffix test works on the WHOLE string, no splitting.
        let suffix = format!("/Contents/MacOS/{FUSION_APP_BINARY}");
        assert!(cmd.ends_with(&suffix));
    }

    #[test]
    fn is_elevated_matches_the_real_euid() {
        // Regression: the first implementation stat'd `/` and compared its owner uid to 0.
        // `/` is ALWAYS owned by root, so that answered `true` for a normal account — and it
        // did, for uid 501 on the reference Mac. Compare against geteuid directly instead.
        let real = unsafe { libc::geteuid() == 0 };
        assert_eq!(MacHost.is_elevated(), real, "must agree with geteuid() == 0");
        if unsafe { libc::geteuid() } != 0 {
            assert!(!MacHost.is_elevated(), "a non-root account must answer false");
        }
    }

    #[test]
    fn a_poll_never_panics_and_always_answers() {
        // Whatever this machine has running, the poll must return a Vec and never throw.
        let v = MacHost.fusion_processes();
        for p in &v {
            assert!(p.pid > 0, "a real pid, never a placeholder");
            assert_eq!(p.image, FUSION_APP_BINARY);
        }
        // is_elevated must answer too (a normal user on a dev box).
        let _ = MacHost.is_elevated();
        // fusion_log_dir may legitimately be None on a machine without Fusion.
        let _ = MacHost.fusion_log_dir();
    }
}

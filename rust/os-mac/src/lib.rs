//! macOS host: the same two traits, for Kyle to implement. Every `todo!()` names what the
//! Windows crate does for that method, so the port is a translation, not a rediscovery.

use fusion_core::{HostPaths, HostProcess};
use fusion_core::host::{FusionProcess, HostError};
use std::path::PathBuf;

pub struct MacHost;

impl HostProcess for MacHost {
    fn fusion_processes(&self) -> Vec<FusionProcess> {
        todo!("proc_listpids / sysctl: find 'Autodesk Fusion' (Windows: Toolhelp32 snapshot)")
    }
    fn launch_fusion(&self) -> Result<FusionProcess, HostError> {
        todo!("`open -a 'Autodesk Fusion'` detached (Windows: CREATE_BREAKAWAY_FROM_JOB)")
    }
    fn is_elevated(&self) -> bool {
        todo!("geteuid() == 0 (Windows: IsUserAnAdmin)")
    }
}

impl HostPaths for MacHost {
    fn fusion_executable(&self) -> Option<PathBuf> { todo!("/Applications/Autodesk Fusion.app (Windows: webdeploy launcher)") }
    fn addin_dirs(&self) -> Vec<PathBuf> { todo!("~/Library/Application Support/Autodesk/... (Windows: FusionAddins)") }
    fn fusion_log_dir(&self) -> Option<PathBuf> { todo!("~/Library/Application Support/Autodesk/Autodesk Fusion 360/<userId>/logs") }
    fn bridge_state_dir(&self) -> PathBuf { todo!("~/.adom/fusion-bridge") }
    fn scratch_dir(&self) -> PathBuf { todo!("$TMPDIR/adom-fusion") }
    // not a todo!(): the mechanical verbs read this before anything else, so a panic here would take
    // the bridge down on a Mac even for fusion_mech_capabilities. It is the add-in's journal root.
    fn mechanical_state_dir(&self) -> PathBuf {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/Application Support/Adom/FusionBridge/mechanical")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_mechanical_journal_lives_where_the_addin_writes_it() {
        let p = MacHost.mechanical_state_dir();
        assert!(p.ends_with("Library/Application Support/Adom/FusionBridge/mechanical"), "{}", p.display());
    }
}

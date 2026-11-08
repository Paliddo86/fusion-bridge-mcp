//! Can this Fusion install start at all? Windows answers a launch of a stripped install with one "System
//! Error" popup per missing DLL, and the launcher points wherever its .ini says, whatever else is on disk.
//! An interrupted update left exactly that: a launcher pointing at a folder with Fusion360.exe and almost
//! nothing else (2026-11-04). So it's checked from the disk first, and the popups are never raised.
use std::path::{Path, PathBuf};

/// A real Fusion app folder has hundreds of DLLs; the stripped one had 2.
const MIN_APP_DLLS: usize = 20;
const APP_EXE: &str = "Fusion360.exe";

/// The folder a FusionLauncher.exe.ini starts, from its `cmd = ""C:\...\Fusion360.exe" ...` line. The file is UTF-16.
pub fn ini_target(bytes: &[u8]) -> Option<PathBuf> {
    let text = if bytes.starts_with(&[0xff, 0xfe]) {
        String::from_utf16_lossy(&bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    let line = text.lines().find(|l| l.trim_start().to_lowercase().starts_with("cmd"))?;
    let at = line.to_lowercase().find(&APP_EXE.to_lowercase())?;
    let start = line[..at].rfind('"')? + 1;
    Some(PathBuf::from(line[start..at].replace('\\', "/")))
}

fn dll_count(dir: &Path) -> usize {
    std::fs::read_dir(dir).map(|d| d.flatten().filter(|e| e.path().extension().map(|x| x.eq_ignore_ascii_case("dll")).unwrap_or(false)).count()).unwrap_or(0)
}

/// Why this launcher can't start Fusion, or None. Says None when it can't tell: this must never stop a good install.
pub fn launch_problem(launcher: &Path) -> Option<String> {
    let name = launcher.file_name()?.to_string_lossy().to_lowercase();
    let dir = if name == APP_EXE.to_lowercase() {
        launcher.parent()?.to_path_buf()
    } else {
        ini_target(&std::fs::read(launcher.with_file_name("FusionLauncher.exe.ini")).ok()?)?
    };
    if !dir.is_dir() { return Some(format!("its launcher points at {}, which isn't there", dir.display())); }
    let dlls = dll_count(&dir);
    (dlls < MIN_APP_DLLS).then(|| format!("{} has only {dlls} DLLs, so the install is incomplete (an update that didn't finish)", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp(tag: &str) -> PathBuf { let d = std::env::temp_dir().join(format!("adom-install-check-{tag}-{}", std::process::id())); let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).unwrap(); d }
    fn utf16(s: &str) -> Vec<u8> { let mut v = vec![0xff, 0xfe]; for u in s.encode_utf16() { v.extend(u.to_le_bytes()); } v }
    fn app(dir: &Path, dlls: usize) { std::fs::create_dir_all(dir).unwrap(); std::fs::write(dir.join(APP_EXE), b"x").unwrap(); for i in 0..dlls { std::fs::write(dir.join(format!("a{i}.dll")), b"x").unwrap(); } }
    fn launcher(root: &Path, target: &Path) -> PathBuf {
        let l = root.join("launcher"); std::fs::create_dir_all(&l).unwrap();
        std::fs::write(l.join("FusionLauncher.exe"), b"x").unwrap();
        std::fs::write(l.join("FusionLauncher.exe.ini"), utf16(&format!("[Launcher]\r\nstream = production\r\ncmd = \"\"{}/Fusion360.exe\" /arg\"\r\nglobal = False\r\n", target.display()))).unwrap();
        l.join("FusionLauncher.exe")
    }
    #[test] fn reads_the_folder_from_a_real_shaped_ini() {
        let b = utf16("[Launcher]\r\nstream = production\r\nauid = Autodesk\r\ncmd = \"\"C:\\Users\\me\\webdeploy\\production\\39e8\\Fusion360.exe\" /x\"\r\n");
        assert_eq!(ini_target(&b).unwrap(), PathBuf::from("C:/Users/me/webdeploy/production/39e8/"));
    }
    #[test] fn a_stripped_install_is_refused_and_a_full_one_is_not() {
        let r = temp("full");
        let (bad, good) = (r.join("bad"), r.join("good"));
        app(&bad, 2); app(&good, 300);
        assert!(launch_problem(&launcher(&r, &bad)).unwrap().contains("only 2 DLLs"));
        assert!(launch_problem(&launcher(&r, &good)).is_none());
        assert!(launch_problem(&bad.join(APP_EXE)).is_some());
        assert!(launch_problem(&good.join(APP_EXE)).is_none());
    }
    /// A real install, read-only: ADOM_REAL_INI=<FusionLauncher.exe.ini> ADOM_REAL_ROOT=<a path prefix that maps C:/ here>
    #[test] #[ignore] fn a_real_install_passes() {
        let ini = std::env::var("ADOM_REAL_INI").unwrap();
        let t = ini_target(&std::fs::read(&ini).unwrap()).expect("the ini names a folder");
        println!("target {}", t.display());
        let here = PathBuf::from(t.to_string_lossy().replace("C:/", &std::env::var("ADOM_REAL_ROOT").unwrap()));
        println!("dlls {}", dll_count(&here));
        assert!(dll_count(&here) >= MIN_APP_DLLS);
    }
    #[test] fn a_missing_target_is_a_problem_and_doubt_is_not() {
        let r = temp("doubt");
        assert!(launch_problem(&launcher(&r, &r.join("gone"))).unwrap().contains("isn't there"));
        let l = r.join("noini"); std::fs::create_dir_all(&l).unwrap();
        assert!(launch_problem(&l.join("FusionLauncher.exe")).is_none());
        std::fs::write(l.join("FusionLauncher.exe.ini"), b"[Launcher]\nstream = x\n").unwrap();
        assert!(launch_problem(&l.join("FusionLauncher.exe")).is_none());
    }
}

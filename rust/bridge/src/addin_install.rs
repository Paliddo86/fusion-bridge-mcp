//! What the Python bridge did at startup and the exe must do alone: sync the bundled
//! AdomBridge add-in into EVERY Fusion add-in location (Fusion moved the scan dir across
//! versions and ignores the others), refusing a payload whose relative imports do not resolve
//! inside it (issue #69: half a sync bricked the add-in), and fail the runs a previous bridge
//! process left mid-flight (a respawn is the only way a chain thread vanishes).
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn source_dir() -> Option<PathBuf> { std::env::current_exe().ok()?.parent().map(|d| d.join("addin").join("AdomBridge")) }
fn py_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(d: &Path, out: &mut Vec<PathBuf>) { if let Ok(rd) = std::fs::read_dir(d) { for e in rd.flatten() { let p = e.path(); if p.is_dir() { walk(&p, out); } else if p.extension().map(|x| x == "py").unwrap_or(false) { out.push(p); } } } }
    walk(root, &mut out); out
}
/// `_check_relative_imports`: "file imports .mod" problems; empty means the payload is whole.
pub fn check_relative_imports(root: &Path) -> Vec<String> {
    let files = py_files(root);
    let have: std::collections::HashSet<PathBuf> = files.iter().cloned().collect();
    let have_dirs: std::collections::HashSet<PathBuf> = files.iter().filter_map(|p| p.parent().map(|d| d.to_path_buf())).collect();
    let re = regex::Regex::new(r"(?m)^\s*from\s+(\.+)([\w.]*)\s+import").unwrap();
    let mut problems = Vec::new();
    for f in &files {
        let Ok(txt) = std::fs::read_to_string(f) else { continue };
        for m in re.captures_iter(&txt) {
            let (dots, module) = (&m[1], &m[2]);
            if module.is_empty() { continue; }
            let mut base = f.parent().map(|d| d.to_path_buf()).unwrap_or_default();
            for _ in 0..dots.len() - 1 { base = base.parent().map(|d| d.to_path_buf()).unwrap_or_default(); }
            let mut cand = base.clone(); for part in module.split('.') { cand = cand.join(part); }
            if !(have.contains(&cand.with_extension("py")) || have.contains(&cand.join("__init__.py")) || have_dirs.contains(&cand)) {
                problems.push(format!("{} imports {}{}", f.strip_prefix(root).unwrap_or(f).to_string_lossy().replace('\\', "/"), dots, module));
            }
        }
    }
    problems
}
/// `_sync_directory`: copy new or changed files, remove what the source no longer has (never
/// `__pycache__`), keep the target dir itself so Fusion's run-on-startup metadata survives.
fn sync_directory(source: &Path, target: &Path, log: &mut Vec<String>) -> std::io::Result<()> {
    let mut keep: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    fn walk(d: &Path, out: &mut Vec<PathBuf>) { if let Ok(rd) = std::fs::read_dir(d) { for e in rd.flatten() { let p = e.path(); out.push(p.clone()); if p.is_dir() { walk(&p, out); } } } }
    let mut src_all = Vec::new(); walk(source, &mut src_all);
    for src in src_all {
        let rel = src.strip_prefix(source).unwrap().to_path_buf(); keep.insert(rel.clone());
        let dst = target.join(&rel);
        if src.is_dir() { std::fs::create_dir_all(&dst)?; }
        else { if let Some(p) = dst.parent() { std::fs::create_dir_all(p)?; } let same = std::fs::read(&src).ok().zip(std::fs::read(&dst).ok()).map(|(a, b)| a == b).unwrap_or(false); if !same { std::fs::copy(&src, &dst)?; log.push(format!("Updated: {}", rel.to_string_lossy().replace('\\', "/"))); } }
    }
    let mut dst_all = Vec::new(); walk(target, &mut dst_all); dst_all.sort(); dst_all.reverse();
    for dst in dst_all {
        let rel = dst.strip_prefix(target).unwrap().to_path_buf();
        if rel.to_string_lossy().contains("__pycache__") || keep.contains(&rel) { continue; }
        if dst.is_file() { let _ = std::fs::remove_file(&dst); log.push(format!("Removed: {}", rel.to_string_lossy().replace('\\', "/"))); }
        else if dst.is_dir() && std::fs::read_dir(&dst).map(|mut r| r.next().is_none()).unwrap_or(false) { let _ = std::fs::remove_dir(&dst); }
    }
    Ok(())
}
fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for e in std::fs::read_dir(source)?.flatten() { let p = e.path(); let t = target.join(e.file_name()); if p.is_dir() { copy_tree(&p, &t)?; } else { std::fs::copy(&p, &t)?; } }
    Ok(())
}
/// `install_addin.install`: the add-in into every candidate dir; `{installedTo, refused, log}`.
pub fn install(addin_dirs: &[PathBuf]) -> Value {
    let Some(src) = source_dir().filter(|s| s.is_dir()) else { return json!({"installedTo": [], "refused": "Source add-in not found beside the exe", "log": []}) };
    let bad = check_relative_imports(&src);
    if !bad.is_empty() { return json!({"installedTo": [], "refused": format!("payload has unresolved relative imports - installing it would brick the add-in (issue #69): {}", bad.join("; ")), "log": []}); }
    let mut log = Vec::new(); let mut installed: Vec<String> = Vec::new();
    for dir in addin_dirs {
        let target = dir.join("AdomBridge");
        let r: std::io::Result<()> = (|| { std::fs::create_dir_all(dir)?; if target.exists() { sync_directory(&src, &target, &mut log)?; log.push(format!("Updated: {}", target.display())); } else { copy_tree(&src, &target)?; log.push(format!("Installed: {}", target.display())); } Ok(()) })();
        if let Err(e) = r { log.push(format!("skip {}: {e}", target.display())); continue; }
        let mut on_disk = check_relative_imports(&target);
        if !on_disk.is_empty() {
            log.push(format!("REPAIRING {}: copy on disk is incomplete ({})", target.display(), on_disk.join("; ").chars().take(200).collect::<String>()));
            let _ = std::fs::remove_dir_all(&target); let _ = copy_tree(&src, &target);
            on_disk = check_relative_imports(&target);
            if !on_disk.is_empty() { log.push(format!("STILL BROKEN after repair: {}: {:?}", target.display(), on_disk)); continue; }
        }
        installed.push(target.to_string_lossy().replace('\\', "/"));
    }
    json!({"installedTo": installed, "refused": Value::Null, "log": log})
}
/// `_reconcile_orphaned_runs`: fail every run the previous bridge process left mid-flight.
pub fn reconcile_orphaned_runs() -> usize {
    let mut runs = crate::chains::read_runs();
    let mut orphaned = 0;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    for (_, st) in runs.iter_mut() {
        if !matches!(st["stage"].as_str(), None | Some("done") | Some("failed")) {
            let stage = st["stage"].as_str().unwrap_or("").to_string();
            st["stage"] = json!("failed"); st["success"] = json!(false); st["verdict"] = json!("not_proven"); st["errorCode"] = json!("bridge_restarted_mid_run");
            st["error"] = json!(format!("The bridge process restarted (a bridge_install or crash) while this run was at '{stage}', which killed the run. Nothing after that stage happened. Start the import again."));
            st["orphanedAt"] = json!(now); orphaned += 1;
        }
    }
    if orphaned > 0 { crate::chains::write_runs_pub(&runs); if let Some(last) = runs.values().last() { let _ = std::fs::write(std::env::temp_dir().join("fusion-prove-last.json"), last.to_string()); } }
    orphaned
}
/// Everything the Python bridge did at startup, once, before the exe serves.
pub fn startup(state_dir: &Path, addin_dirs: &[PathBuf], fusion_installed: bool) {
    let orphaned = reconcile_orphaned_runs();
    if orphaned > 0 { eprintln!("[fusion-bridge] {orphaned} run(s) were mid-flight when the previous bridge died - marked failed (bridge_restarted_mid_run)"); }
    crate::fusion_prefs::action_ledger(state_dir, "bridge_started", json!({"orphanedRunsFailed": orphaned, "shell": "rust"}));
    if fusion_installed || addin_dirs.iter().any(|d| d.exists()) {
        let r = install(addin_dirs);
        match r["refused"].as_str() { Some(why) => eprintln!("[fusion-bridge] add-in sync REFUSED: {why}"), None => eprintln!("[fusion-bridge] add-in synced into {} location(s)", r["installedTo"].as_array().map(|a| a.len()).unwrap_or(0)) }
        for l in r["log"].as_array().cloned().unwrap_or_default() { if let Some(s) = l.as_str() { if s.starts_with("Updated: ") || s.starts_with("Removed") || s.starts_with("REPAIR") || s.starts_with("skip") { eprintln!("[fusion-bridge]   {s}"); } } }
    }
}

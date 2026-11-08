//! Paths Fusion's text commands can take.
//!
//! `executeTextCommand` is EAGLE's space-delimited command line and its quoted form fails, so a
//! file that lives under a path with a space (the bridge cache under "Adom Bridge", a profile
//! like "John Lauer", "Program Files") is STAGED: copied, with its same-stem siblings, into a
//! space-free folder, and Fusion is handed the copy. Never the DOS 8.3 alias: Fusion titles the
//! document after the path it was given, and "0603WA~1" broke every name check (issue #83).
use std::path::{Path, PathBuf};

/// Files that travel with the one being opened (a .sch wants its .brd beside it, a mint wants
/// the .step next to the .lbr).
pub const SIBLING_EXTS: &[&str] = &["sch", "brd", "lbr", "step", "stp", "f3d", "edru", "dru", "flbr", "fsch", "fbrd", "ulp"];

fn has_space(p: &Path) -> bool { p.to_string_lossy().contains(' ') }

/// The first space-free, writable stage root: the bridge state dir, then LOCALAPPDATA, then the
/// drive root. A stage under a path with a space would defeat the whole point.
pub fn stage_root(state_dir: &Path) -> Result<PathBuf, String> {
    let mut candidates: Vec<PathBuf> = vec![state_dir.join("stage")];
    if let Ok(la) = std::env::var("LOCALAPPDATA") { if !la.trim().is_empty() { candidates.push(PathBuf::from(la).join("adom-fusion-stage")); } }
    if let Ok(t) = std::env::var("TEMP") { if !t.trim().is_empty() { candidates.push(PathBuf::from(t).join("adom-fusion-stage")); } }
    candidates.push(PathBuf::from(if cfg!(windows) { "C:/adom-fusion-stage" } else { "/tmp/adom-fusion-stage" }));
    for c in candidates {
        if has_space(&c) { continue; }
        if std::fs::create_dir_all(&c).is_ok() { return Ok(c); }
    }
    Err("no space-free folder is writable for staging a file Fusion can open (state dir, LOCALAPPDATA, TEMP and the drive root all failed)".into())
}

fn bucket(parent: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parent.to_string_lossy().to_lowercase().replace('\\', "/").hash(&mut h);
    format!("{:08x}", h.finish() as u32)
}

fn fwd(p: &Path) -> String { p.to_string_lossy().replace('\\', "/") }

/// The path to hand Fusion for an INPUT file: the file itself when its path has no space,
/// otherwise a staged copy (siblings included) under a space-free folder. The document Fusion
/// opens keeps its real name either way.
pub fn for_fusion(state_dir: &Path, path: &str) -> Result<String, String> {
    let p = PathBuf::from(path.replace('\\', "/"));
    if !has_space(&p) { return Ok(fwd(&p)); }
    if !p.is_file() { return Err(format!("{} does not exist, so it cannot be staged for Fusion", fwd(&p))); }
    let parent = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let dir = stage_root(state_dir)?.join(bucket(&parent));
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create stage folder {}: {e}", fwd(&dir)))?;
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut copied = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&parent) {
        for e in rd.flatten() {
            let q = e.path();
            if !q.is_file() { continue; }
            let same_stem = q.file_stem().map(|s| s.to_string_lossy().eq_ignore_ascii_case(&stem)).unwrap_or(false);
            let ext_ok = q.extension().map(|x| SIBLING_EXTS.contains(&x.to_string_lossy().to_lowercase().as_str())).unwrap_or(false);
            if q == p || (same_stem && ext_ok) {
                let dst = dir.join(q.file_name().unwrap());
                let fresh = match (std::fs::metadata(&q), std::fs::metadata(&dst)) { (Ok(a), Ok(b)) => a.len() == b.len() && a.modified().ok() <= b.modified().ok(), _ => false };
                if !fresh { std::fs::copy(&q, &dst).map_err(|e| format!("cannot stage {} for Fusion: {e}", fwd(&q)))?; }
                copied.push(dst);
            }
        }
    }
    let staged = dir.join(p.file_name().unwrap());
    if !staged.is_file() { return Err(format!("staging {} produced no file", fwd(&p))); }
    Ok(fwd(&staged))
}

/// For an OUTPUT path Fusion will write: hand Fusion a space-free target when the wanted one
/// has a space, and return the staged path to move afterwards with `finish_output`.
pub fn output_for_fusion(state_dir: &Path, out: &str) -> Result<(String, Option<String>), String> {
    let p = PathBuf::from(out.replace('\\', "/"));
    if !has_space(&p) { return Ok((fwd(&p), None)); }
    let dir = stage_root(state_dir)?.join("out");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create stage folder {}: {e}", fwd(&dir)))?;
    let staged = dir.join(p.file_name().ok_or("output path has no file name")?);
    let _ = std::fs::remove_file(&staged);
    Ok((fwd(&staged), Some(fwd(&staged))))
}

/// Move a staged output to where the caller asked for it.
pub fn finish_output(staged: &str, out: &str) -> Result<(), String> {
    if let Some(parent) = Path::new(out).parent() { let _ = std::fs::create_dir_all(parent); }
    std::fs::copy(staged, out).map_err(|e| format!("cannot move the staged output to {out}: {e}"))?;
    let _ = std::fs::remove_file(staged);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spaced_inputs_are_staged_with_siblings_and_named_the_same() {
        let root = std::env::temp_dir().join(format!("fusionpath-{}", std::process::id()));
        let src = root.join("Adom Bridge").join("samples"); std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("0603WAF3300T5E.lbr"), b"<eagle/>").unwrap();
        std::fs::write(src.join("0603WAF3300T5E.step"), b"ISO-10303").unwrap();
        std::fs::write(src.join("other.step"), b"x").unwrap();
        let state = root.join("state");
        let got = for_fusion(&state, &src.join("0603WAF3300T5E.lbr").to_string_lossy()).unwrap();
        assert!(!got.contains(' '), "{got}");
        assert!(got.ends_with("/0603WAF3300T5E.lbr"), "{got}");
        let dir = Path::new(&got).parent().unwrap();
        assert!(dir.join("0603WAF3300T5E.step").is_file(), "sibling staged");
        assert!(!dir.join("other.step").exists(), "unrelated file not staged");
        let plain = root.join("plain"); std::fs::create_dir_all(&plain).unwrap(); std::fs::write(plain.join("a.brd"), b"y").unwrap();
        assert_eq!(for_fusion(&state, &plain.join("a.brd").to_string_lossy()).unwrap(), plain.join("a.brd").to_string_lossy().replace('\\', "/"));
        assert!(for_fusion(&state, &src.join("missing.lbr").to_string_lossy()).is_err());
        let (o, staged) = output_for_fusion(&state, &root.join("out dir").join("lib.flbr").to_string_lossy()).unwrap();
        assert!(!o.contains(' ') && staged.is_some());
        std::fs::write(&o, b"flbr").unwrap();
        finish_output(&o, &root.join("out dir").join("lib.flbr").to_string_lossy()).unwrap();
        assert!(root.join("out dir").join("lib.flbr").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}

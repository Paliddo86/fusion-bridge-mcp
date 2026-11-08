//! `timings.py` for the exe: the rolling window (`<state>/timings.json`, ten samples per
//! `phase|kind`), the append-only run log (`verb-times.jsonl`), and the verb estimate. Same
//! file, same keys, same rounding, so a run measured by either shell teaches the other.

use serde_json::{json, Value};
use std::path::Path;

const KEEP: usize = 10;

fn now_i() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }

pub fn load(state_dir: &Path) -> Value {
    std::fs::read_to_string(state_dir.join("timings.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).filter(|v| v.is_object()).unwrap_or(json!({}))
}

/// `timings.record(phase, seconds, start_kind)`.
pub fn record(state_dir: &Path, phase: &str, seconds: f64, start_kind: Option<&str>) {
    if !(0.0..=7200.0).contains(&seconds) { return; }
    let mut d = load(state_dir);
    let k = format!("{phase}|{}", start_kind.unwrap_or("any"));
    let mut samples: Vec<f64> = d[&k]["samples"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
    samples.push((seconds * 10.0).round() / 10.0);
    let keep: Vec<f64> = samples.iter().rev().take(KEEP).rev().cloned().collect();
    d[&k] = json!({"samples": keep, "lastAt": now_i()});
    let _ = std::fs::create_dir_all(state_dir);
    let p = state_dir.join("timings.json");
    let tmp = state_dir.join("timings.json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(&d).unwrap_or_default()).is_ok() { let _ = std::fs::rename(&tmp, &p); }
}

/// `timings.log_run`: the rolling window AND one line in the historical log.
pub fn log_run(state_dir: &Path, verb: &str, seconds: f64, ok: bool) {
    record(state_dir, &format!("verb:{verb}"), seconds, None);
    let p = state_dir.join("verb-times.jsonl");
    if let Ok(m) = std::fs::metadata(&p) { if m.len() > 4 * 1024 * 1024 { let _ = std::fs::rename(&p, state_dir.join("verb-times.jsonl.1")); } }
    use std::io::Write;
    let line = json!({"at": now_i(), "verb": verb, "seconds": (seconds * 100.0).round() / 100.0, "ok": ok, "note": ""}).to_string();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) { let _ = writeln!(f, "{line}"); }
}

/// `timings.verb_estimate`: (seconds, confidence) from the rolling window.
pub fn verb_estimate(state_dir: &Path, verb: &str) -> (f64, &'static str) {
    let st = fusion_core::readiness::timings_stats(&load(state_dir), &format!("verb:{verb}"), "any");
    let (count, p50) = (st["count"].as_u64().unwrap_or(0), st["p50"].as_f64().unwrap_or(0.0));
    if count >= 3 && p50 > 0.0 { (p50, "measured") } else if count >= 1 && p50 > 0.0 { (p50, "typical") } else { (0.0, "unknown") }
}

/// The activity indicator on Fusion's taskbar button through ab's `desktop_taskbar`: a bar
/// paced by the verb's own history (0 to 95 across the estimate, then hold), or the flash.
pub struct Indicator { stop: std::sync::Arc<std::sync::atomic::AtomicBool>, caller: fusion_core::ab::Caller }

fn fusion_hwnd(ab: &dyn fusion_core::ab::AbClient, caller: &fusion_core::ab::Caller) -> Option<i64> {
    fusion_core::windows_facts::facts_from_ab(ab, caller).main_hwnd
}

pub fn taskbar_progress(caller: &fusion_core::ab::Caller, state: &str, value: Option<f64>) -> bool {
    let Some(ab) = fusion_core::ab::Loopback::from_env() else { return false };
    use fusion_core::ab::AbClient;
    let Some(h) = fusion_hwnd(&ab, caller) else { return false };
    let mut prog = json!({"state": state});
    if let Some(v) = value { prog["value"] = json!(v.clamp(0.0, 100.0) as i64); }
    ab.call("desktop_taskbar", json!({"hwnd": h, "progress": prog}), caller, 15).is_ok()
}

/// `chain_taskbar_progress`: pct None clears the bar and the flash.
pub fn taskbar_pct(caller: &fusion_core::ab::Caller, pct: Option<f64>) {
    match pct {
        Some(p) => { taskbar_progress(caller, "normal", Some(p)); }
        None => {
            taskbar_progress(caller, "none", None);
            if let Some(ab) = fusion_core::ab::Loopback::from_env() { use fusion_core::ab::AbClient; if let Some(h) = fusion_hwnd(&ab, caller) { let _ = ab.call("desktop_taskbar", json!({"hwnd": h, "flash": "stop"}), caller, 15); } }
        }
    }
}

impl Indicator {
    /// `start_activity_progress` + the flash, per the sticky `activityIndicator` pref.
    pub fn start(state_dir: &Path, verb: &str, caller: &fusion_core::ab::Caller) -> Indicator {
        let mode = crate::small_verbs::get_pref(state_dir, "activityIndicator").and_then(|v| v.as_str().map(|s| s.to_lowercase())).filter(|m| ["flash", "progress", "both", "none"].contains(&m.as_str())).unwrap_or_else(|| "progress".into());
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (est, conf) = verb_estimate(state_dir, verb);
        if mode == "progress" || mode == "both" {
            let (stop2, caller2) = (stop.clone(), caller.clone());
            std::thread::spawn(move || {
                let started = std::time::Instant::now();
                if est <= 0.0 || conf == "unknown" {
                    taskbar_progress(&caller2, "indeterminate", None);
                    while !stop2.load(std::sync::atomic::Ordering::Relaxed) { std::thread::sleep(std::time::Duration::from_secs(1)); }
                    return;
                }
                let mut fails = 0;
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(700));
                    if stop2.load(std::sync::atomic::Ordering::Relaxed) { break; }
                    let pct = (95.0 * started.elapsed().as_secs_f64() / est).min(95.0);
                    if taskbar_progress(&caller2, "normal", Some(pct)) { fails = 0; } else { fails += 1; if fails > 120 { break; } }
                }
            });
        }
        if mode == "flash" || mode == "both" {
            if let Some(ab) = fusion_core::ab::Loopback::from_env() { use fusion_core::ab::AbClient; let _ = ab.call("desktop_taskbar", json!({"titleContains": "Autodesk Fusion", "flash": "until_focused"}), caller, 10); }
        }
        Indicator { stop, caller: caller.clone() }
    }
    /// `end_activity_progress`: a red flash on failure, then clear the bar and the flash.
    pub fn stop(self, failed: bool) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if failed { taskbar_progress(&self.caller, "error", Some(100.0)); std::thread::sleep(std::time::Duration::from_millis(1200)); }
        taskbar_pct(&self.caller, None);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn record_rolls_ten_and_rounds() {
        let d = std::env::temp_dir().join(format!("fb-timings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for i in 0..12 { super::record(&d, "verb:x", 1.0 + i as f64 * 0.333, None); }
        let s = super::load(&d)["verb:x|any"]["samples"].as_array().unwrap().clone();
        assert_eq!(s.len(), 10);
        assert_eq!(s[0], serde_json::json!(1.7));
        assert_eq!(super::verb_estimate(&d, "x").1, "measured");
        assert_eq!(super::verb_estimate(&d, "never").1, "unknown");
        let _ = std::fs::remove_dir_all(&d);
    }
}

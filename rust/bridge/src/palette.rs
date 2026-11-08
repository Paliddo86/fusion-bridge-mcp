//! The in-Fusion palette ("Adom is driving Fusion"): the VERBS' voice. John, 2026-09-05: the ab
//! captions describe the demo; the palette describes the verb running against Fusion right now
//! (opening a library, minting 3D part 2 of 5, switching to the 2D board), with the time it
//! usually takes on this machine.
//!
//! One worker thread owns the palette. `say` only records the latest message; the worker paints
//! it through the add-in's activity_banner (its own HTTP hop, busy-exempt) and keeps trying while
//! Fusion's main thread is tied up (a STEP import blocks it for a minute), so the newest message
//! always lands the moment Fusion breathes. Nothing new for 25 s and no import run in flight
//! takes the palette down.
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

static WORKER: Once = Once::new();
static DIRTY: AtomicBool = AtomicBool::new(false);
static HIDE: AtomicBool = AtomicBool::new(false);
/// While held (a tour in flight) the palette is never taken down by the idle timer: created once,
/// repainted in place, so no per-beat creation cost (the add-in retries delivery 3 s on creation).
static HOLD: AtomicBool = AtomicBool::new(false);
static SEQ: AtomicU64 = AtomicU64::new(0);
static FINISHED_SEQ: AtomicU64 = AtomicU64::new(0);
static PAINTED_SEQ: AtomicU64 = AtomicU64::new(0);
static LAST: Mutex<Option<(String, String, Option<f64>, Option<f64>, u64)>> = Mutex::new(None);
/// What the worker last did, for fusion_interactive: (action, title, success, error, seconds ago).
static LAST_RESULT: Mutex<Option<(String, String, bool, String, Instant)>> = Mutex::new(None);

fn send(action: &str, title: &str, detail: &str, percent: Option<f64>, eta: Option<f64>) -> Value {
    crate::addin::addin_http("activity_banner", &json!({"action": action, "title": title, "detail": detail, "percent": percent, "etaSec": eta.map(|e| e.max(0.0).round()), "tone": "working"}), 8)
}

fn worker() {
    let mut painted_seq: u64 = 0;
    let mut painted_at = Instant::now();
    let mut shown = false;
    loop {
        std::thread::sleep(Duration::from_millis(400));
        if HIDE.swap(false, Ordering::Relaxed) {
            for _ in 0..5 { if send("hide", "", "", None, None)["success"].as_bool() == Some(true) { break; } std::thread::sleep(Duration::from_secs(2)); }
            shown = false; DIRTY.store(false, Ordering::Relaxed); *LAST.lock().unwrap() = None; continue;
        }
        if DIRTY.load(Ordering::Relaxed) {
            let Some((t, d, p, e, seq)) = LAST.lock().unwrap().clone() else { DIRTY.store(false, Ordering::Relaxed); continue };
            if PAINTED_SEQ.load(Ordering::Relaxed) >= seq { DIRTY.store(false, Ordering::Relaxed); continue; }
            std::thread::sleep(Duration::from_secs(2));   // let the caller's verb go first
            let r = send("show", &t, &d, p, e);
            *LAST_RESULT.lock().unwrap() = Some(("show".into(), t.clone(), r["success"].as_bool() == Some(true), r["error"].as_str().unwrap_or("").chars().take(120).collect(), Instant::now()));
            if r["success"].as_bool() == Some(true) {
                shown = true; painted_seq = seq; painted_at = Instant::now(); PAINTED_SEQ.store(seq, Ordering::Relaxed);
                if SEQ.load(Ordering::Relaxed) == seq { DIRTY.store(false, Ordering::Relaxed); }
            } else { std::thread::sleep(Duration::from_secs(2)); }
            continue;
        }
        // every paint counts, also one made while the panel was already up: with `!shown &&` here, a second message
        // left painted_seq behind for good, so the test below could never pass and the panel stayed up forever (#143)
        if PAINTED_SEQ.load(Ordering::Relaxed) > painted_seq { shown = true; painted_seq = PAINTED_SEQ.load(Ordering::Relaxed); painted_at = Instant::now(); }
        let finished = FINISHED_SEQ.load(Ordering::Relaxed) == painted_seq;
        let idle_for = if finished { Duration::from_secs(3) } else { Duration::from_secs(25) };
        if shown && !HOLD.load(Ordering::Relaxed) && painted_at.elapsed() > idle_for && SEQ.load(Ordering::Relaxed) == painted_seq && !run_in_flight() {
            let r = send("hide", "", "", None, None);
            *LAST_RESULT.lock().unwrap() = Some(("hide".into(), String::new(), r["success"].as_bool() == Some(true), r["error"].as_str().unwrap_or("").chars().take(120).collect(), Instant::now()));
            if r["success"].as_bool() == Some(true) { shown = false; *LAST.lock().unwrap() = None; }
            else { painted_at = Instant::now(); }
        }
    }
}

/// Say what is being done to Fusion right now, with the time it usually takes (from the timing
/// store, so the estimate sharpens with every run). Fire-and-forget: the worker paints it.
pub fn say(title: &str, detail: &str, percent: Option<f64>, eta: Option<f64>) {
    WORKER.call_once(|| { std::thread::spawn(worker); });
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    *LAST.lock().unwrap() = Some((title.to_string(), detail.to_string(), percent, eta, seq));
    // Paint NOW, on the caller's thread, before the caller's own verb goes to the add-in: a
    // background paint raced the tour's Electron commands and each collision cost a 15 s retry
    // (ConfRoomROG, 2026-09-05). The worker only picks it up if this attempt did not land.
    let r = send("show", title, detail, percent, eta);
    *LAST_RESULT.lock().unwrap() = Some(("show".into(), title.to_string(), r["success"].as_bool() == Some(true), r["error"].as_str().unwrap_or("").chars().take(120).collect(), Instant::now()));
    if r["success"].as_bool() == Some(true) { PAINTED_SEQ.store(seq, Ordering::Relaxed); } else { DIRTY.store(true, Ordering::Relaxed); }
}
/// The verb that last spoke has answered: nothing is running behind that message any more, so the panel comes down in
/// moments rather than after the idle wait. A newer message or a run in flight keeps it up.
pub fn verb_finished() { FINISHED_SEQ.store(SEQ.load(Ordering::Relaxed), Ordering::Relaxed); }

/// Keep the palette up between messages (a tour in flight) or let the idle timer take it down.
pub fn hold(on: bool) { HOLD.store(on, Ordering::Relaxed); }

/// Take the palette down now (the tour's teardown, a cancelled run).
pub fn hide() {
    WORKER.call_once(|| { std::thread::spawn(worker); });
    HIDE.store(true, Ordering::Relaxed);
}

fn run_in_flight() -> bool {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    crate::chains::read_runs().values().any(|r| run_is_live(r, now))
}

/// A run holds the palette up only while it is genuinely moving: not finished, and its current
/// stage started within the last 20 minutes (a run interrupted by a restart never writes "done",
/// and used to pin a stale "0%" palette over Fusion forever).
fn run_is_live(r: &Value, now: f64) -> bool {
    let unfinished = !matches!(r["stage"].as_str(), Some("done") | Some("failed") | Some("cancelled") | None);
    let last = r["stageStartedAt"].as_f64().or(r["startedAt"].as_f64()).unwrap_or(0.0);
    unfinished && now - last < 1200.0
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    #[test]
    fn stale_runs_do_not_pin_the_palette() {
        let now = 10_000.0;
        assert!(super::run_is_live(&json!({"stage": "binding", "stageStartedAt": now - 60.0}), now));
        assert!(!super::run_is_live(&json!({"stage": "binding", "stageStartedAt": now - 3600.0}), now));
        assert!(!super::run_is_live(&json!({"stage": "starting", "startedAt": now - 7200.0}), now));
        assert!(!super::run_is_live(&json!({"stage": "done", "stageStartedAt": now}), now));
        assert!(!super::run_is_live(&json!({"stage": "binding"}), now));
    }
}

/// The plain add-in verbs worth a line on the palette (the orchestrations say their own).
pub fn for_verb(cmd: &str, args: &Value) -> Option<(String, String)> {
    let name = |k: &str| args[k].as_str().map(|s| s.replace('\\', "/").rsplit('/').next().unwrap_or("").to_string()).filter(|s| !s.is_empty());
    Some(match cmd {
        "open_by_urn" => ("Opening a cloud design".into(), format!("from your hub: {}", args["name"].as_str().or(args["urn"].as_str()).unwrap_or("").chars().rev().take(40).collect::<String>().chars().rev().collect::<String>())),
        "open_cloud_file" => ("Opening a cloud design".into(), format!("from your hub: {}", args["fileName"].as_str().or(args["name"].as_str()).unwrap_or(""))),
        // view switches are instant and callers repeat them (an AI flow asks for the board on every landing):
        // a palette per switch never got its 25 quiet seconds and sat over Fusion at 0% for a whole routing pass
        "show_schematic" | "show_2d_board" | "show_3d_board" => return None,
        "import_step" | "import_file" => ("Importing a model".into(), name("filePath").unwrap_or_default()),
        "import_electronics" | "new_electronics_from_eagle" => ("Importing an electronics design".into(), name("filePath").or(name("schPath")).unwrap_or_default()),
        "export_step" | "export_stl" | "export_f3d" | "export_glb" | "export_gerbers" | "export_bom" => (format!("Exporting {}", cmd.trim_start_matches("export_").to_uppercase()), name("outputPath").unwrap_or_default()),
        "save_to_cloud" => ("Saving to your hub".into(), args["name"].as_str().unwrap_or("").into()),
        _ => return None,
    })
}

/// The measured time a verb usually takes on this machine (p50 of past runs), for the palette.
pub fn eta_for_verb(state_dir: &std::path::Path, verb: &str) -> Option<f64> {
    let (secs, kind) = crate::timings::verb_estimate(state_dir, verb);
    if kind == "unknown" || secs <= 0.0 { None } else { Some(secs) }
}

/// The palette's last paint attempt, for fusion_interactive.
pub fn status() -> Value {
    match LAST_RESULT.lock().unwrap().as_ref() { Some((a, t, ok, e, at)) => json!({"lastAction": a, "lastTitle": t, "ok": ok, "error": if e.is_empty() { Value::Null } else { json!(e) }, "secondsAgo": at.elapsed().as_secs()}), None => json!({"lastAction": Value::Null}) }
}

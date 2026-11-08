//! `fusion_live_feed`: a ~1 fps look INTO Fusion as a box-side ring buffer (main window plus every
//! owned popup per tick, tagged), drained by a container-side presenter. Native, over ab's
//! desktop_screenshot_window; the health fields tell "live" from "spinning".

use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;

use fusion_core::ab::{AbClient, Caller, Loopback};

const MAX_FRAMES: usize = 300;

#[derive(Default)]
struct Feed { running: bool, stop: bool, tick: i64, frames: Vec<Value>, started_at: f64, captured: i64, no_hwnd: i64, last_error: Option<String>, last_error_at: Option<f64>, last_capture_at: Option<f64>, last_shot_keys: Option<Vec<String>> }
static FEED: Mutex<Feed> = Mutex::new(Feed { running: false, stop: false, tick: 0, frames: Vec::new(), started_at: 0.0, captured: 0, no_hwnd: 0, last_error: None, last_error_at: None, last_capture_at: None, last_shot_keys: None });

fn now_f() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

fn health(mut out: Value) -> Value {
    let f = FEED.lock().unwrap();
    out["captured"] = json!(f.captured); out["noHwnd"] = json!(f.no_hwnd); out["lastShotKeys"] = json!(f.last_shot_keys); out["lastShotDataKeys"] = Value::Null;
    out["lastError"] = json!(f.last_error); out["lastErrorAt"] = json!(f.last_error_at); out["lastCaptureAt"] = json!(f.last_capture_at);
    if f.tick >= 5 && f.captured == 0 { out["healthy"] = json!(false); out["_honesty"] = json!(format!("{} capture attempts, ZERO frames. This feed is spinning, not live. lastError={} noHwnd={}. Do not present it as a video.", f.tick, f.last_error.as_deref().unwrap_or("None"), f.no_hwnd)); }
    else if let (Some(t), Some(true)) = (f.last_capture_at, out["running"].as_bool()) { if now_f() - t > 15.0 { out["healthy"] = json!(false); out["_honesty"] = json!(format!("no frame for {:.0}s although running. lastError={}", now_f() - t, f.last_error.as_deref().unwrap_or("None"))); } else { out["healthy"] = json!(f.captured > 0); } }
    else { out["healthy"] = json!(f.captured > 0); }
    out
}

pub fn live_feed(caller: &Caller, pids: Vec<u32>, args: &Value) -> Value {
    let action = args["action"].as_str().unwrap_or("status").to_lowercase();
    let (alive, tick, buffered, started) = { let f = FEED.lock().unwrap(); (f.running, f.tick, f.frames.len(), f.started_at) };
    match action.as_str() {
        "status" => return health(json!({"success": true, "running": alive, "tick": tick, "buffered": buffered, "startedAt": if started > 0.0 { json!(started) } else { Value::Null }})),
        "frames" => {
            let since = args["since"].as_i64().unwrap_or(0);
            let fr: Vec<Value> = FEED.lock().unwrap().frames.iter().filter(|f| f["tick"].as_i64().unwrap_or(0) > since).cloned().collect();
            let n = fr.len();
            return health(json!({"success": true, "running": alive, "tick": tick, "frames": fr[n.saturating_sub(200)..].to_vec(), "count": n, "_hint": "Each frame is a BOX path (this machine); read_file it from the container and publish it. kind is main|popup; proof frames come from fusion_prove_result evidence[] with kind:'proof'. Pass since:<tick> to get only what you have not seen."}));
        }
        "stop" => { FEED.lock().unwrap().stop = true; return json!({"success": true, "stopped": true, "tick": tick}); }
        _ => {}
    }
    if alive { return json!({"success": true, "alreadyRunning": true, "tick": tick}); }
    let interval = args["intervalSec"].as_f64().unwrap_or(1.0).max(0.5);
    let max_sec = args["maxSec"].as_f64().unwrap_or(600.0).min(3600.0);
    { let mut f = FEED.lock().unwrap(); *f = Feed { running: true, started_at: now_f(), ..Default::default() }; }
    let caller = caller.clone();
    std::thread::spawn(move || {
        let t0 = now_f();
        let ab = Loopback::from_env();
        loop {
            { let f = FEED.lock().unwrap(); if f.stop || now_f() - t0 >= max_sec { break; } }
            let tick = { let mut f = FEED.lock().unwrap(); f.tick += 1; f.tick };
            let Some(ab) = ab.as_ref() else { let mut f = FEED.lock().unwrap(); f.last_error = Some("ab loopback unavailable".into()); f.last_error_at = Some(now_f()); std::thread::sleep(Duration::from_secs_f64(interval)); continue; };
            let facts = fusion_core::windows_facts::facts_from_ab(ab, &caller);
            let _ = &pids;
            let Some(hwnd) = facts.main_hwnd else { FEED.lock().unwrap().no_hwnd += 1; std::thread::sleep(Duration::from_secs_f64(interval)); continue; };
            let title = facts.main_title.clone().unwrap_or_else(|| "Fusion".into()).chars().take(60).collect::<String>();
            let rect = facts.family.iter().find(|w| w["hwnd"].as_i64() == Some(hwnd)).map(|w| fusion_core::dialogs::rect_of(&w["rect"])).unwrap_or(json!({}));
            match ab.call("desktop_screenshot_window", json!({"hwnd": hwnd}), &caller, 20) {
                Ok(shot) => {
                    let now = now_f();
                    let mut f = FEED.lock().unwrap();
                    f.last_shot_keys = Some(shot.as_object().map(|o| { let mut k: Vec<String> = o.keys().cloned().collect(); k.sort(); k.into_iter().take(12).collect() }).unwrap_or_default());
                    let main = shot["fullPath"].as_str().or(shot["fullPathHost"].as_str()).or(shot["path"].as_str()).map(|s| s.replace('\\', "/"));
                    let popups = shot["screenshots"].as_array().cloned().unwrap_or_default();
                    match main {
                        Some(p) => { f.captured += 1; f.last_capture_at = Some(now); f.frames.push(json!({"tick": tick, "kind": "main", "at": now, "path": p, "title": title, "rect": rect, "popups": popups.len()})); }
                        None => { f.last_error = Some(format!("screenshot returned no path: {}", shot["error"].as_str().or(shot["errorCode"].as_str()).unwrap_or("?")).chars().take(200).collect()); f.last_error_at = Some(now); }
                    }
                    for (k, sp) in popups.iter().enumerate() {
                        if let Some(pth) = sp["fullPathHost"].as_str().or(sp["fullPath"].as_str()).or(sp["path"].as_str()) {
                            f.frames.push(json!({"tick": tick, "kind": "popup", "at": now, "index": k + 1, "path": pth.replace('\\', "/"), "rect": if sp["rect"].is_null() { sp["bounds"].clone() } else { sp["rect"].clone() }, "title": sp["title"].as_str().unwrap_or("dialog").chars().take(50).collect::<String>()}));
                        }
                    }
                    if f.frames.len() > MAX_FRAMES { let cut = f.frames.len() - MAX_FRAMES; f.frames.drain(..cut); }
                }
                Err(e) => { let mut f = FEED.lock().unwrap(); f.last_error = Some(format!("ab: {e}").chars().take(200).collect()); f.last_error_at = Some(now_f()); }
            }
            std::thread::sleep(Duration::from_secs_f64(interval));
        }
        FEED.lock().unwrap().running = false;
    });
    json!({"success": true, "started": true, "intervalSec": interval, "maxSec": max_sec, "_hint": "Capturing into a box-side ring buffer. Drain it from the container with fusion_live_feed {action:'frames', since:<lastTick>}, read_file each path, and publish (e.g. adom-shotlog inject -c fusion-live -d '[main #n] ...'). tools/live_feed_drain.py does exactly that."})
}

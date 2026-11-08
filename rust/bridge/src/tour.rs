//! `fusion_demo {mode:"tour"}`: the guided show of the electronics bridge. Nine beats over the
//! bundled sample part: symbol, footprint, the 3D chip minted and bound into your hub, then a generated project in schematic, 2D and 3D
//! board, and a cloud search; ab's glass remote drives it (instant callback + poll backstop, one FIFO for every source), every beat
//! is measured before it moves (front, title, painted, stable), motion runs through Fusion's
//! own camera, and a progress record carries the evidence. The KiCad bridge's tour is the
//! reference for every rule here (wiki adom/kicad-bridge handlers/demo.py).
use crate::addin::Ctx;
use fusion_core::ab::{AbClient, Caller, Loopback};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const PANEL_ID: &str = "fusion-tour";
const CAPTION_ID: &str = "fusion-tour";
const ACK_ID: &str = "fusion-tour-ack";
/// The share of the primary work area Fusion gets for the show (John, 2026-09-05: 95%).
const WINDOW_SHARE: f64 = 0.95;
/// The board the schematic, 2D and 3D beats show: a real design from the user's hub (John,
/// 2026-09-04: the generated one-chip board looked lame; show Adonis's BQ25792 charger). The
/// generated solo board stays as the fallback for a hub without it. Override with args.board.
pub const DEMO_BOARD: &str = "BQ25792";
const DEMO_BOARD_PROJECT: &str = "Main";
const PANEL_REASON: &str = "Transport for the Fusion tour the user started: play, pause, next, back, mute, stop";

/// Beats: (key, panel stop label, estimated seconds). John, 2026-09-04: Adom is an electronics
/// prototyping company, so the tour demos symbol, footprint, 3D chip, schematic, 2D board, 3D
/// board and cloud search - never a mechanical part. Multipart libraries are MENTIONED, never
/// shown: a 3D bind is minutes per part, so a shown multipart beat is a nightmare, not a demo.
const BEATS: &[(&str, &str, u64)] = &[
    ("library", "Open the sample library", 30),
    ("symbol", "Symbol: the full pinout", 14),
    ("footprint", "Footprint: pads and layers", 14),
    ("chip", "3D chip: minted and bound", 170),
    ("schematic", "A real board: the schematic", 90),
    ("board2d", "The board in 2D", 24),
    ("board3d", "The board in 3D", 40),
    ("aps", "Cloud search over your hub", 8),
];

// ── state ─────────────────────────────────────────────────────────────────────
static TOUR: Mutex<Option<Value>> = Mutex::new(None);
static CTRL: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static EXIT: AtomicBool = AtomicBool::new(false);
static MUTED: AtomicBool = AtomicBool::new(false);
static SEEN_AT: Mutex<Vec<String>> = Mutex::new(Vec::new());
static PANEL_SHOWN: AtomicBool = AtomicBool::new(false);
static AUDIO: Mutex<Option<(std::process::Child, String)>> = Mutex::new(None);
static SAVED_BOUNDS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static CALLER: Mutex<Option<Caller>> = Mutex::new(None);
/// The narrator: the current beat's key and how many of its fillers have played.
static NARR: Mutex<Option<(String, usize)>> = Mutex::new(None);
/// True while a beat is still doing something on screen (action, verify, motion): the narrator
/// may start a filler. False once the beat only waits for audio, so the beat can end.
static BEAT_BUSY: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);
static GENERIC_USED: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The tour's state dir, for detached helpers (the in-Fusion palette) that need their own Ctx.
static STATE_DIR: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);
/// Set once this run's remote is stopped or hidden (by the tour, the user or an agent): nothing
/// shows the panel again until the next run (issue #121: it came back after a hide).
static PANEL_CLOSED: AtomicBool = AtomicBool::new(false);
/// When each transport command was last accepted or taken, for the debounce below.
static LAST_CMD: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());
/// A repeat of a transport command inside this window of the last one is dropped (issue #121:
/// eleven Back presses in 15 s replayed beats in a loop).
const DEBOUNCE: Duration = Duration::from_millis(3000);

/// One timestamped event in the record (post-production cuts on these).
fn event(name: &str, detail: Value) { let at = since_start(); rec(|s| push(s, "events", json!({"at": at, "event": name, "beat": s["step"].clone(), "detail": detail}))); }
fn since_start() -> f64 { let st = snap()["started"].as_f64().unwrap_or(0.0); if st > 0.0 { ((now() - st) * 100.0).round() / 100.0 } else { 0.0 } }
fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }
fn sleep_s(s: f64) { std::thread::sleep(Duration::from_secs_f64(s)); }
fn caller() -> Caller { CALLER.lock().unwrap().clone().unwrap_or_default() }
fn ab(verb: &str, args: Value, timeout: u64) -> Value {
    match Loopback::from_env() { Some(a) => match a.call(verb, args, &caller(), timeout) { Ok(v) => v, Err(e) => json!({"success": false, "_adCallError": format!("{e:?}")}) }, None => json!({"success": false, "_adCallError": "ab loopback unavailable"}) }
}
fn rec(f: impl FnOnce(&mut Value)) { if let Some(s) = TOUR.lock().unwrap().as_mut() { f(s); } }
/// Write to the record of tour `id` only: the live one if it still is, else its persisted file.
fn rec_for(sd: &std::path::Path, id: &str, f: impl FnOnce(&mut Value)) {
    { let mut g = TOUR.lock().unwrap(); if let Some(s) = g.as_mut() { if s["demoId"].as_str() == Some(id) { f(s); return; } } }
    if let Some(mut s) = crate::demo::load_pub(sd, id) { f(&mut s); crate::demo::persist_pub(sd, &s); }
}
fn snap() -> Value { TOUR.lock().unwrap().clone().unwrap_or(Value::Null) }
fn push(s: &mut Value, key: &str, v: Value) { let mut a = s[key].as_array().cloned().unwrap_or_default(); a.push(v); s[key] = json!(a); }
fn windows() -> Vec<Value> { crate::managed::find_key(&ab("desktop_list_windows", json!({}), 20), "windows", 0).and_then(|w| w.as_array().cloned()).unwrap_or_default() }
fn fusion_hwnd() -> Option<(i64, String)> { windows().into_iter().find(|w| w["title"].as_str().unwrap_or("").contains("Autodesk Fusion") && w["kind"].as_str() == Some("normal")).and_then(|w| Some((w["hwnd"].as_i64()?, w["title"].as_str().unwrap_or("").to_string()))) }
fn top_window() -> Option<Value> { let mut ws = windows(); ws.retain(|w| w["z"].is_number() && w["kind"].as_str() == Some("normal") && !w["title"].as_str().unwrap_or("").is_empty()); ws.sort_by_key(|w| w["z"].as_i64().unwrap_or(99)); ws.into_iter().next() }

// ── captions, acks, audio ────────────────────────────────────────────────────
fn caption(text: &str, ms: u64) {
    if EXIT.load(Ordering::Relaxed) { return; }
    // John, 2026-09-04: ab's "medium" is 48px since v2.0.84 and the captions overlapped the glass
    // remote; 32px (the old medium) and one short line keep clear of it. ab's own warnings
    // (too_wide_for_screen, too_tall_for_screen) go into the record so a wrap never passes silently.
    let at0 = since_start();
    let r = ab("desktop_caption", json!({"id": CAPTION_ID, "text": text, "duration": ms, "size": "medium", "position": "bottom", "reason": "Narrate the Fusion tour the user started, one line per beat"}), 6);
    let ok = r["success"].as_bool() == Some(true) && r["error"].as_str().map(|e| e.is_empty()).unwrap_or(true);
    let at = at0;
    let warns: Vec<String> = r["warnings"].as_array().map(|a| a.iter().filter_map(|w| w["check"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    rec(|s| push(s, "captions", json!({"text": text.chars().take(80).collect::<String>(), "ok": ok, "at": at, "chars": text.chars().count(), "warnings": warns, "error": r["error"].as_str().filter(|e| !e.is_empty()).or(r["_adCallError"].as_str())})));
}
fn caption_clear() { let _ = ab("desktop_caption", json!({"action": "hide", "id": CAPTION_ID, "reason": "The user stopped the Fusion tour; clear its caption"}), 6); }
fn ack(text: &str) {
    let r = ab("desktop_caption", json!({"id": ACK_ID, "text": text, "duration": 3500, "size": "medium", "position": "top", "reason": "Acknowledge a press on the Fusion tour remote the instant it arrives"}), 6);
    rec(|s| push(s, "captions", json!({"text": text, "ack": true, "ok": r["success"].as_bool() == Some(true), "error": r["error"].as_str().filter(|e| !e.is_empty())})));
}
fn audio_dir() -> std::path::PathBuf { std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("resources").join("demo-audio"))).unwrap_or_default() }
fn audio_stop() { if let Some((mut c, _)) = AUDIO.lock().unwrap().take() { let _ = c.kill(); } }
fn audio_playing() -> bool { let mut g = AUDIO.lock().unwrap(); match g.as_mut() { Some((c, _)) => c.try_wait().map(|s| s.is_none()).unwrap_or(false), None => false } }
/// Play the beat's pre-rendered line through a hidden WPF MediaPlayer (ab has no audio verb).
fn audio_play(beat: &str) {
    rec(|s| s["narrationBeat"] = json!(beat));
    *NARR.lock().unwrap() = Some((beat.to_string(), 0));
    // part A clip if the beat has one; else the main clip only when no part B clip exists
    let a = format!("{beat}-a"); let b = format!("{beat}-b");
    if audio_dir().join(format!("{a}.mp3")).is_file() { play_file(&a, "part-a"); }
    else if !audio_dir().join(format!("{b}.mp3")).is_file() { play_file(beat, "main"); }
}
/// Part B: the content is on screen and moving. Its clip replaces whatever filler is on air
/// (after letting a sentence end, briefly); falls back to the main clip when the beat has no B.
fn audio_play_b(beat: &str) {
    let b = format!("{beat}-b");
    let name = if audio_dir().join(format!("{b}.mp3")).is_file() { b } else { beat.to_string() };
    let t0 = Instant::now();
    while audio_playing() && t0.elapsed() < Duration::from_secs(3) { sleep_s(0.2); }
    play_file(&name, "part-b");
}
/// Play one clip from resources/demo-audio (kind = main | filler); records the stamp and the file.
fn play_file(name: &str, kind: &str) {
    let at = since_start();
    if MUTED.load(Ordering::Relaxed) { rec(|s| push(s, "audio", json!({"beat": name, "kind": kind, "muted": true, "at": at}))); return; }
    let f = audio_dir().join(format!("{name}.mp3"));
    if !f.is_file() { rec(|s| push(s, "audio", json!({"beat": name, "kind": kind, "missing": true, "at": at}))); return; }
    audio_stop();
    let ps = format!("Add-Type -AssemblyName presentationCore; $p = New-Object System.Windows.Media.MediaPlayer; $p.Open([uri]'{}'); $p.Volume = 0.9; $p.Play(); Start-Sleep -Milliseconds 500; $t0 = Get-Date; while (((Get-Date) - $t0).TotalSeconds -lt 40 -and (-not $p.NaturalDuration.HasTimeSpan -or $p.Position -lt $p.NaturalDuration.TimeSpan)) {{ Start-Sleep -Milliseconds 100 }}; $p.Close()", f.to_string_lossy().replace('\'', "''"));
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-Command", &ps]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)] { use std::os::windows::process::CommandExt; cmd.creation_flags(0x0800_0000); }
    match cmd.spawn() { Ok(c) => { *AUDIO.lock().unwrap() = Some((c, name.to_string())); rec(|s| push(s, "audio", json!({"beat": name, "kind": kind, "started": true, "at": at, "file": f.to_string_lossy().replace('\\', "/")}))); } Err(e) => rec(|s| push(s, "audio", json!({"beat": name, "kind": kind, "error": e.to_string()}))) }
}
/// The in-Fusion palette ("Adom is driving Fusion"): show / update / hide through the add-in's
/// activity_banner, fire-and-forget on a thread with its own Ctx. The palette carries the
/// caption's line, the beat and the percent, so Fusion itself shows the work getting done.
fn banner(action: &str, title: &str, detail: &str, percent: Option<u64>) {
    let Some(sd) = STATE_DIR.lock().unwrap().clone() else { return };
    let caller = caller();
    let args = json!({"action": action, "title": title, "detail": detail, "percent": percent, "tone": "working"});
    std::thread::spawn(move || {
        let host = crate::host_for_diagnosis();
        let side = |_v: &str, _a: Value, _t: u64| -> Value { json!({"success": false, "error": "no sidecar"}) };
        let pids: Vec<u32> = host.fusion_processes().iter().map(|p| p.pid).collect();
        let dlg = |r: Value| r;
        let ctx = Ctx { host: &*host, fusion_exe: crate::host_for_diagnosis_paths(), short_path: |p| crate::host().short_path(p), state_dir: sd, caller, pids, sidecar: &side, dialogs: &dlg };
        let r = crate::addin::pass_through("activity_banner", &args, &ctx);
        rec(|s| { s["palette"] = json!({"action": args["action"], "ok": r["success"].as_bool() == Some(true), "error": r["error"].as_str().filter(|e| !e.is_empty()), "at": 0}); });
    });
}
/// The narrator thread: whenever the current beat is still busy on screen and nothing is playing,
/// play the beat's next filler (<beat>-f<n>.mp3), then the generic pool, each once per tour. It
/// never starts a clip while paused or muted, and never once the beat only waits for audio, so a
/// beat ends at the end of a sentence. John, 2026-09-05: no dead air.
fn narrator_loop(pass: &dyn Fn(&str, Value) -> Value) {
    let mut quiet_since: Option<Instant> = None;
    let mut last_probe = Instant::now() - Duration::from_secs(10);
    let mut was_interactive: Option<bool> = None;
    loop {
        sleep_s(0.25);
        let st = snap();
        if st.is_null() || st["done"].as_bool() != Some(false) || EXIT.load(Ordering::Relaxed) { break; }
        // busy probe (cheap: the add-in's /status only; the main thread is the beat's to use)
        if last_probe.elapsed() >= Duration::from_millis(1500) {
            last_probe = Instant::now();
            let p = interactive(pass, false);
            let now_i = p["interactive"].as_bool().unwrap_or(true);
            if was_interactive != Some(now_i) { event(if now_i { "fusion-interactive" } else { "fusion-busy" }, json!({"addinBusy": p["addinBusy"], "addinUp": p["addinUp"]})); was_interactive = Some(now_i); }
        }
        if MUTED.load(Ordering::Relaxed) || PAUSED.load(Ordering::Relaxed) || !BEAT_BUSY.load(Ordering::Relaxed) { quiet_since = None; continue; }
        if audio_playing() { quiet_since = None; continue; }
        let q = *quiet_since.get_or_insert_with(Instant::now);
        if q.elapsed() < Duration::from_millis(900) { continue; }   // a breath between sentences
        let Some((beat, n)) = NARR.lock().unwrap().clone() else { continue };
        if !BEAT_BUSY.load(Ordering::Relaxed) { continue; }   // a failed beat cut the narration meanwhile
        let candidate = format!("{beat}-f{}", n + 1);
        if audio_dir().join(format!("{candidate}.mp3")).is_file() {
            *NARR.lock().unwrap() = Some((beat.clone(), n + 1));
            play_file(&candidate, "filler");
        } else {
            let mut used = GENERIC_USED.lock().unwrap();
            let next = (1..=2).map(|k| format!("generic-f{k}")).find(|g| !used.contains(g) && audio_dir().join(format!("{g}.mp3")).is_file());
            match next { Some(g) => { used.push(g.clone()); drop(used); play_file(&g, "filler"); } None => { BEAT_BUSY.store(false, Ordering::Relaxed); } }
        }
        quiet_since = None;
    }
}
fn apply_mute(m: bool, source: &str) {
    let was = MUTED.swap(m, Ordering::Relaxed);
    if m && !was { audio_stop(); } else if was && !m { if let Some(b) = snap()["narrationBeat"].as_str().map(str::to_string) { audio_play(&b); } }
    rec(|s| { s["muted"] = json!(m); s["muteSource"] = json!(source); });
}

// ── the remote: one FIFO for every source ────────────────────────────────────
fn queue(cmd: &str) { let mut q = CTRL.lock().unwrap(); if cmd == "exit" { q.clear(); q.push_back("exit".into()); } else if q.len() < 8 { q.push_back(cmd.to_string()); } }
/// A press repeats one being handled: the same command is still queued, or was accepted or
/// taken less than DEBOUNCE ago.
fn is_repeat(cmd: &str, queued: &VecDeque<String>, last: &[(String, Instant)], now: Instant) -> bool {
    queued.iter().any(|c| c == cmd) || last.iter().any(|(c, t)| c == cmd && now.saturating_duration_since(*t) < DEBOUNCE)
}
fn stamp_cmd(cmd: &str) { let mut l = LAST_CMD.lock().unwrap(); l.retain(|(c, _)| c != cmd); l.push((cmd.to_string(), Instant::now())); }
fn pending_interrupt() -> bool { EXIT.load(Ordering::Relaxed) || CTRL.lock().unwrap().iter().any(|c| matches!(c.as_str(), "prev" | "next" | "pause" | "exit")) }
/// One press from any source: record, ack, apply mute at once, queue the transport command.
pub fn on_press(button: &str, at: Value, via: &str) -> Value {
    let b = button.to_lowercase();
    let beat = snap();
    rec(|s| { push(s, "presses", json!({"button": b, "at": at, "via": via, "beat": beat["beatIndex"], "step": beat["step"]})); s["lastPress"] = json!({"button": b, "at": at, "via": via}); });
    let cmd = match b.as_str() { "back" | "prev" => "prev", "play" | "resume" => "play", "pause" => "pause", "next" => "next", "stop" | "exit" => "exit", _ => "" };
    // a press for a run that is over (or stopping) is dropped, and so is a repeat of one in hand
    if !cmd.is_empty() {
        let over = beat["done"].as_bool() != Some(false) || beat["stopping"].as_bool() == Some(true) || EXIT.load(Ordering::Relaxed);
        if over || is_repeat(cmd, &CTRL.lock().unwrap(), &LAST_CMD.lock().unwrap(), Instant::now()) {
            rec(|s| s["debouncedPresses"] = json!(s["debouncedPresses"].as_u64().unwrap_or(0) + 1));
            return json!({"debounced": cmd, "tourOver": over});
        }
    }
    let ack_text = match b.as_str() { "mute" => "Remote: muted", "unmute" => "Remote: narration back on", "pause" => "Remote: pausing", "play" | "resume" => "Remote: playing", "next" => "Remote: next", "back" | "prev" => "Remote: back", "stop" | "exit" => "Remote: stopping the tour", _ => "" };
    if !ack_text.is_empty() { ack(ack_text); }
    match b.as_str() {
        "mute" | "unmute" => { apply_mute(b == "mute", via); return json!({"applied": b}); }
        _ => {}
    }
    if cmd.is_empty() { return json!({"ignored": b}); }
    if matches!(cmd, "pause" | "exit" | "next" | "prev") { audio_stop(); }
    if cmd == "exit" { stop_now(); }
    stamp_cmd(cmd);
    queue(cmd);
    json!({"queued": cmd})
}
/// Exit ends the tour NOW (issue #121: eleven Exit presses over 28 s did not): the narration and
/// caption go, the remote comes down for good, and the record reads stopped at once. The tour
/// thread's `pass` refuses every verb from here, so the step in flight returns at its next call;
/// its teardown then closes the documents (tourDocsClosed "closing" holds a new tour off until then).
fn stop_now() {
    EXIT.store(true, Ordering::Relaxed);
    BEAT_BUSY.store(false, Ordering::Relaxed); *NARR.lock().unwrap() = None;
    audio_stop(); caption_clear();
    PANEL_CLOSED.store(true, Ordering::Relaxed);
    rec(|s| { s["stopping"] = json!(true); s["done"] = json!(true); s["exited"] = json!(true); s["step"] = json!("done"); s["stepLabel"] = json!("Tour stopped"); s["tourDocsClosed"] = json!("closing"); });
    std::thread::spawn(panel_hide);
}
/// ab's instant callback (POST /demo/event): {id, button, at, state, stage, muted}.
pub fn on_panel_event(ev: &Value) -> Value {
    if ev["id"].as_str().map(|i| i != PANEL_ID).unwrap_or(false) { return json!({"ignored": true}); }
    if let Some(at) = ev["at"].as_str().map(str::to_string).or_else(|| ev["at"].as_f64().map(|f| f.to_string())) { SEEN_AT.lock().unwrap().push(at); }
    let b = ev["button"].as_str().unwrap_or("");
    if let Some(m) = ev["muted"].as_bool() { if m != MUTED.load(Ordering::Relaxed) && !matches!(b, "mute" | "unmute") { apply_mute(m, "panel"); } }
    on_press(b, ev["at"].clone(), if ev["synthetic"].as_bool() == Some(true) { "callback-synthetic" } else { "callback" })
}
/// Only the live, running run `run_id` may show its remote, and never once it was stopped or hidden.
fn panel_may_show(run_id: &str, live: &Value, closed: bool) -> bool {
    !closed && !run_id.is_empty() && live["demoId"].as_str() == Some(run_id) && live["done"].as_bool() == Some(false) && live["stopping"].as_bool() != Some(true)
}
fn panel_show(run_id: &str, stage: usize, state: Option<&str>) -> bool {
    if !panel_may_show(run_id, &snap(), PANEL_CLOSED.load(Ordering::Relaxed) || EXIT.load(Ordering::Relaxed)) { return false; }
    let mut args = json!({"action": "show", "id": PANEL_ID, "stage": stage, "reason": PANEL_REASON});
    if !PANEL_SHOWN.load(Ordering::Relaxed) {
        args["title"] = json!("Fusion bridge tour"); args["stops"] = json!(BEATS.iter().map(|b| b.1).collect::<Vec<_>>()); args["state"] = json!(state.unwrap_or("playing")); args["position"] = json!("bottom-right"); args["muted"] = json!(MUTED.load(Ordering::Relaxed));
        if let Some(u) = crate::launch::SELF_URL.lock().unwrap().clone() { args["eventCallbackUrl"] = json!(u.replace("/command", "/demo/event")); }
    } else if let Some(st) = state { args["state"] = json!(st); }
    let r = ab("desktop_demo_panel", args.clone(), 8);
    let shown = r["shown"].as_bool() == Some(true) || (r["success"].as_bool() == Some(true) && r["error"].as_str().map(|e| e.is_empty()).unwrap_or(true) && r["_adCallError"].is_null());
    if shown { PANEL_SHOWN.store(true, Ordering::Relaxed); }
    rec(|s| s["panel"] = json!({"shown": shown, "stage": stage, "callbackUrl": args["eventCallbackUrl"].as_str().or(s["panel"]["callbackUrl"].as_str()), "callbackAccepted": r["callbackAccepted"], "error": r["error"].as_str().filter(|e| !e.is_empty()).or(r["_adCallError"].as_str())}));
    shown
}
/// Backstop drain of the panel's own queue; presses the callback already delivered are skipped by `at`.
fn panel_poll() {
    let r = ab("desktop_demo_panel", json!({"action": "poll", "id": PANEL_ID}), 6);
    if let Some(m) = r["muted"].as_bool() { if m != MUTED.load(Ordering::Relaxed) { apply_mute(m, "panel-poll"); } }
    for ev in r["events"].as_array().cloned().unwrap_or_default() {
        let at = ev["at"].as_str().map(str::to_string).or_else(|| ev["at"].as_f64().map(|f| f.to_string())).unwrap_or_default();
        if !at.is_empty() { let mut seen = SEEN_AT.lock().unwrap(); if seen.contains(&at) { continue; } seen.push(at.clone()); }
        on_press(ev["button"].as_str().unwrap_or(""), ev["at"].clone(), if ev["synthetic"].as_bool() == Some(true) { "poll-synthetic" } else { "poll" });
    }
    // gone or stopped after this run showed it means someone took the remote down: it stays
    // down for this run (issue #121). Before the first show, the state is the last run's.
    if matches!(r["state"].as_str(), Some("gone") | Some("stopped")) && PANEL_SHOWN.load(Ordering::Relaxed) { PANEL_SHOWN.store(false, Ordering::Relaxed); PANEL_CLOSED.store(true, Ordering::Relaxed); rec(|s| s["panel"]["closedBy"] = json!(r["state"])); }
}
fn drain_stale_presses() { for _ in 0..3 { let r = ab("desktop_demo_panel", json!({"action": "poll", "id": PANEL_ID}), 6); if r["events"].as_array().map(|a| a.is_empty()).unwrap_or(true) { break; } } }
/// Hide the remote and PROVE it: poll once (the tour is over, so the drain is harmless), hide again
/// if the panel still reports itself playing or paused, and record the replies. The laptop run of
/// 2026-09-04 left the panel on John's screen for nine minutes after the tour ended.
fn panel_hide() {
    let mut rep = json!({"attempts": 0});
    for attempt in 0..3 {
        rep["attempts"] = json!(attempt + 1);
        let h = ab("desktop_demo_panel", json!({"action": "hide", "id": PANEL_ID, "reason": "The Fusion tour ended; take its remote off the screen"}), 6);
        rep["hidden"] = json!(h["hidden"].as_bool() == Some(true) || h["success"].as_bool() == Some(true));
        rep["error"] = json!(h["error"].as_str().filter(|e| !e.is_empty()).or(h["_adCallError"].as_str()));
        sleep_s(0.5);
        let p = ab("desktop_demo_panel", json!({"action": "poll", "id": PANEL_ID}), 6);
        let st = p["state"].as_str().unwrap_or("").to_string();
        rep["stateAfter"] = json!(st);
        if st.is_empty() || st == "stopped" || st == "gone" { break; }
    }
    PANEL_SHOWN.store(false, Ordering::Relaxed); PANEL_CLOSED.store(true, Ordering::Relaxed);
    rec(|s| s["panelHidden"] = rep.clone());
}
fn take_cmd() -> Option<String> { panel_poll(); let c = CTRL.lock().unwrap().pop_front(); if let Some(c) = &c { stamp_cmd(c); } c }

// ── layout ────────────────────────────────────────────────────────────────────
fn primary_work_area() -> Option<Value> {
    let r = ab("desktop_list_monitors", json!({"reason": "Fusion tour: size the window to 80% of the primary screen"}), 6);
    let mons = crate::managed::find_key(&r, "monitors", 0).and_then(|m| m.as_array().cloned()).unwrap_or_default();
    let prim = mons.iter().find(|m| m["primary"].as_bool() == Some(true)).or(mons.first())?;
    Some(if prim["workArea"].is_object() { prim["workArea"].clone() } else { prim["bounds"].clone() })
}
fn leftovers_path(sd: &std::path::Path) -> std::path::PathBuf { sd.join("tours").join("window-bounds-to-restore.json") }
fn save_bounds(sd: &std::path::Path, hwnd: i64, title: &str) {
    if SAVED_BOUNDS.lock().unwrap().iter().any(|b| b["hwnd"].as_i64() == Some(hwnd)) { return; }
    if let Some(w) = windows().into_iter().find(|w| w["hwnd"].as_i64() == Some(hwnd)) {
        let rc = &w["rect"];
        let b = json!({"hwnd": hwnd, "title": title, "x": rc["left"], "y": rc["top"], "w": rc["width"], "h": rc["height"], "maximized": w["maximized"]});
        SAVED_BOUNDS.lock().unwrap().push(b);
        let _ = std::fs::create_dir_all(sd.join("tours"));
        let _ = std::fs::write(leftovers_path(sd), json!(SAVED_BOUNDS.lock().unwrap().clone()).to_string());
    }
}
/// The Fusion window at 80% of the primary work area, centred (Fusion is one window; its
/// documents are the beats), saved first so the end of the tour puts it back.
fn place_fusion(sd: &std::path::Path, hwnd: i64, title: &str) -> Value {
    let Some(wa) = primary_work_area() else { return json!({"placed": false, "why": "no monitor bounds"}) };
    let (mx, my, mw, mh) = (wa["x"].as_i64().unwrap_or(0), wa["y"].as_i64().unwrap_or(0), wa["width"].as_i64().unwrap_or(0), wa["height"].as_i64().unwrap_or(0));
    if mw == 0 || mh == 0 { return json!({"placed": false, "why": "empty work area"}); }
    let (w, h) = ((mw as f64 * WINDOW_SHARE) as i64, (mh as f64 * WINDOW_SHARE) as i64);
    let (x, y) = (mx + (mw - w) / 2, my + (mh - h) / 2);
    save_bounds(sd, hwnd, title);
    let r = ab("desktop_set_window_bounds", json!({"hwnd": hwnd, "x": x, "y": y, "w": w, "h": h, "reason": "Fusion tour: place Fusion at 95% of the screen, centred"}), 8);
    json!({"placed": r["success"].as_bool() == Some(true) && r["error"].as_str().map(|e| e.is_empty()).unwrap_or(true), "x": x, "y": y, "w": w, "h": h, "error": r["error"].as_str().filter(|e| !e.is_empty()).or(r["_adCallError"].as_str())})
}
/// The activity palette floats over the top right of the canvas in Fusion's electronics editors
/// (its docking request is ignored there), which hid the symbol's right pins in run 37. Park it
/// over the right-hand panel column of the placed Fusion window, off the drawing, at a size
/// that shows the whole card. Returns the rect it was given, or why not.
fn park_palette(fusion: &Value) -> Value {
    let rows = crate::managed::find_key(&ab("desktop_enum_windows", json!({}), 10), "windows", 0).and_then(|w| w.as_array().cloned()).unwrap_or_default();
    let Some(pal) = rows.iter().find(|w| w["title"].as_str().map(|t| t.trim().eq_ignore_ascii_case(fusion_core::dialogs::BRIDGE_PALETTE_TITLE)).unwrap_or(false) && w["visible"].as_bool() != Some(false)) else { return json!({"parked": false, "why": "no palette window"}) };
    let Some(h) = pal["hwnd"].as_i64() else { return json!({"parked": false, "why": "palette without hwnd"}) };
    let (fx, fy, fw) = (fusion["x"].as_i64().unwrap_or(0), fusion["y"].as_i64().unwrap_or(0), fusion["w"].as_i64().unwrap_or(0));
    if fw < 1200 { return json!({"parked": false, "why": "Fusion bounds unknown"}); }
    let (w, hh) = (640, 420);
    let (x, y) = (fx + fw - w - 24, fy + 250);
    let cur = pal["rect"].as_array().cloned().unwrap_or_default();
    if cur.len() == 4 && cur[0].as_i64() == Some(x) && cur[1].as_i64() == Some(y) { return json!({"parked": true, "x": x, "y": y, "w": w, "h": hh, "already": true}); }
    let r = ab("desktop_set_window_bounds", json!({"hwnd": h, "x": x, "y": y, "w": w, "h": hh, "reason": "Fusion tour: park the Adom activity palette over the right panel, off the drawing"}), 8);
    json!({"parked": r["success"].as_bool() == Some(true), "x": x, "y": y, "w": w, "h": hh, "error": r["error"].as_str().filter(|e| !e.is_empty()).or(r["_adCallError"].as_str())})
}
fn restore_bounds(sd: &std::path::Path, source: &str) -> usize {
    let mut saved = SAVED_BOUNDS.lock().unwrap().clone();
    if saved.is_empty() { saved = std::fs::read_to_string(leftovers_path(sd)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.as_array().cloned()).unwrap_or_default(); }
    let live: HashSet<i64> = windows().iter().filter_map(|w| w["hwnd"].as_i64()).collect();
    let mut n = 0;
    for b in saved { if let Some(h) = b["hwnd"].as_i64() { if live.contains(&h) && b["w"].as_i64().unwrap_or(0) > 0 {
        let _ = ab("desktop_set_window_bounds", json!({"hwnd": h, "x": b["x"], "y": b["y"], "w": b["w"], "h": b["h"], "reason": format!("Fusion tour {source}: restore Fusion to where the user had it")}), 8);
        if b["maximized"].as_bool() == Some(true) { let _ = ab("desktop_set_window_state", json!({"hwnd": h, "state": "maximize"}), 8); }
        n += 1;
    } } }
    SAVED_BOUNDS.lock().unwrap().clear();
    let _ = std::fs::remove_file(leftovers_path(sd));
    n
}

// ── measurement ───────────────────────────────────────────────────────────────
/// Decode a PNG the box wrote and count distinct colours in the central 60% (the canvas).
fn canvas_colours(path: &str) -> Option<usize> {
    let f = std::fs::File::open(path).ok()?;
    let dec = png::Decoder::new(std::io::BufReader::new(f));
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let bpp = info.color_type.samples() * (info.bit_depth as usize / 8).max(1);
    let (w, h) = (info.width as usize, info.height as usize);
    let (x0, x1, y0, y1) = (w / 5, w * 4 / 5, h / 5, h * 4 / 5);
    let mut set: HashSet<u32> = HashSet::new();
    let mut y = y0;
    while y < y1 { let mut x = x0; while x < x1 { let i = (y * w + x) * bpp; if i + 2 < buf.len() { set.insert(((buf[i] as u32 >> 3) << 10) | ((buf[i + 1] as u32 >> 3) << 5) | (buf[i + 2] as u32 >> 3)); } x += 4; } y += 4; }
    Some(set.len())
}
fn shot_window(hwnd: i64) -> Option<String> { let r = ab("desktop_screenshot_window", json!({"hwnd": hwnd, "resizeMax": 640}), 20); r["fullPath"].as_str().or(r["safePath"].as_str()).map(str::to_string) }
fn digest(path: &str) -> Option<u64> { use std::hash::{Hash, Hasher}; let b = std::fs::read(path).ok()?; let mut h = std::collections::hash_map::DefaultHasher::new(); b.hash(&mut h); Some(h.finish()) }
/// front · title · painted · stable, read from the desktop; one more raise attempt, then remeasured.
fn verify_beat(hwnd: i64, expected_doc: &str) -> Value {
    let mut v = json!({"hwnd": hwnd, "expected": expected_doc, "front": false, "title": false, "painted": null, "stable": null, "attempts": 0});
    for attempt in 0..2 {
        v["attempts"] = json!(attempt + 1);
        let top = top_window();
        v["frontTitle"] = top.as_ref().map(|t| t["title"].clone()).unwrap_or(Value::Null);
        v["front"] = json!(top.as_ref().and_then(|t| t["hwnd"].as_i64()) == Some(hwnd));
        let live = windows().into_iter().find(|w| w["hwnd"].as_i64() == Some(hwnd)).map(|w| w["title"].as_str().unwrap_or("").to_string()).unwrap_or_default();
        v["liveTitle"] = json!(live);
        v["title"] = json!(if expected_doc.is_empty() { Value::Null } else { json!(live.to_lowercase().contains(&expected_doc.to_lowercase())) });
        if v["front"].as_bool() == Some(true) { break; }
        let _ = ab("desktop_bring_to_front", json!({"hwnd": hwnd, "reason": format!("Fusion tour: bring Fusion forward for the {expected_doc} beat, once")}), 8);
        sleep_s(1.5);
    }
    if let Some(p) = shot_window(hwnd) { let n = canvas_colours(&p); v["painted"] = json!(n.map(|n| n >= 12)); v["canvasColours"] = json!(n); }
    let t0 = Instant::now();
    let mut prev = shot_window(hwnd).and_then(|p| digest(&p));
    v["stable"] = json!(false);
    while t0.elapsed() < Duration::from_secs(8) {
        if pending_interrupt() { v["interrupted"] = json!(true); break; }
        sleep_s(0.8);
        let cur = shot_window(hwnd).and_then(|p| digest(&p));
        if prev.is_some() && prev == cur { v["stable"] = json!(true); break; }
        prev = cur;
    }
    v["settleMs"] = json!(t0.elapsed().as_millis() as u64);
    let remote = v["frontTitle"].as_str().map(is_remote_viewer).unwrap_or(false);
    let own_popup = front_ok(hwnd) && v["front"].as_bool() != Some(true) && !remote;
    if v["front"].as_bool() != Some(true) && remote { v["frontNote"] = json!("a remote-desktop viewer is on top; front cannot be proven from here, layout and motion can"); }
    if own_popup { v["frontNote"] = json!("one of Fusion's own popups (the palette host) is on top; counted as front"); }
    v["ok"] = json!((v["front"].as_bool() == Some(true) || remote || own_popup) && v["title"].as_bool() != Some(false) && v["painted"].as_bool() != Some(false));
    v
}
/// RustDesk and friends sit on top of everything on a box driven over remote desktop.
fn is_remote_viewer(title: &str) -> bool { let t = title.to_lowercase(); ["rustdesk", "remote desktop", "anydesk", "teamviewer", "vnc", "parsec"].iter().any(|k| t.contains(k)) }
/// Windows that may sit above Fusion without meaning the user switched away: a remote-desktop
/// viewer, Fusion's own popups (the in-Fusion palette renders in a WebView popup host), and the
/// shell's popup host. Everything else in front is the user's business and stops motion.
fn front_ok(hwnd: i64) -> bool {
    match top_window() {
        Some(t) => t["hwnd"].as_i64() == Some(hwnd) || t["title"].as_str().map(is_remote_viewer).unwrap_or(false) || t["image"].as_str().map(|i| i.eq_ignore_ascii_case("Fusion360.exe")).unwrap_or(false) || matches!(t["title"].as_str(), Some("PopupHost")),
        None => false,
    }
}
fn is_front(hwnd: i64) -> bool { top_window().and_then(|t| t["hwnd"].as_i64()) == Some(hwnd) }
fn foreground_once(hwnd: i64, why: &str) -> bool {
    let _ = ab("desktop_bring_to_front", json!({"hwnd": hwnd, "reason": format!("Fusion tour: {why}, once")}), 8);
    for _ in 0..6 { sleep_s(0.25); if is_front(hwnd) { return true; } }
    // Windows keeps the foreground with the app the user last clicked (the dashboard in Hydrogen):
    // the user asked for the tour there, so that window steps behind Fusion, once
    if let Some(top) = top_window() { if let Some(th) = top["hwnd"].as_i64() { if th != hwnd && !top["title"].as_str().map(is_remote_viewer).unwrap_or(false) {
        let _ = ab("desktop_set_window_state", json!({"hwnd": th, "state": "bottom", "reason": format!("Fusion tour: {why}; the window in front steps behind Fusion once")}), 8);
        sleep_s(0.4);
        let _ = ab("desktop_bring_to_front", json!({"hwnd": hwnd, "reason": format!("Fusion tour: {why}, after lowering the window in front")}), 8);
        for _ in 0..6 { sleep_s(0.25); if is_front(hwnd) { return true; } }
    } } }
    false
}

pub struct Env { pub version: &'static str }

// ── Fusion-side scripts ───────────────────────────────────────────────────────
fn cam_script(body: &str) -> String { format!("import adsk.core, adsk.fusion, math\napp=adsk.core.Application.get()\nd=adsk.fusion.Design.cast(app.activeProduct)\nroot=d.rootComponent\nvp=app.activeViewport\ndef cam():\n    return vp.camera\ndef put(c, smooth=True):\n    c.isSmoothTransition=smooth\n    vp.camera=c\n    vp.refresh()\ndef orient(o, fit=True):\n    c=cam(); c.viewOrientation=o; c.isFitView=fit; put(c)\ndef orbit(deg):\n    c=cam(); e=c.eye; t=c.target; up=c.upVector\n    v=adsk.core.Vector3D.create(e.x-t.x,e.y-t.y,e.z-t.z)\n    m=adsk.core.Matrix3D.create(); m.setToRotation(math.radians(deg), up, t)\n    v.transformBy(m)\n    c.eye=adsk.core.Point3D.create(t.x+v.x,t.y+v.y,t.z+v.z); c.isFitView=False; put(c)\ndef tilt(deg):\n    c=cam(); e=c.eye; t=c.target; up=c.upVector\n    v=adsk.core.Vector3D.create(e.x-t.x,e.y-t.y,e.z-t.z)\n    right=v.crossProduct(up); right.normalize()\n    m=adsk.core.Matrix3D.create(); m.setToRotation(math.radians(deg), right, t)\n    v.transformBy(m)\n    c.eye=adsk.core.Point3D.create(t.x+v.x,t.y+v.y,t.z+v.z); c.isFitView=False; put(c)\ndef focus(x,y,z):\n    c=cam(); e=c.eye; t=c.target\n    dx,dy,dz=x-t.x,y-t.y,z-t.z\n    c.target=adsk.core.Point3D.create(x,y,z); c.eye=adsk.core.Point3D.create(e.x+dx,e.y+dy,e.z+dz); c.isFitView=False; put(c)\ndef zoom(f):\n    c=cam(); c.viewExtents=c.viewExtents*f; c.isFitView=False; put(c)\ndef pan(dx,dy):\n    c=cam(); e=c.eye; t=c.target; ext=c.viewExtents; s=ext*0.001\n    c.eye=adsk.core.Point3D.create(e.x+dx*s,e.y+dy*s,e.z); c.target=adsk.core.Point3D.create(t.x+dx*s,t.y+dy*s,t.z); c.isFitView=False; put(c)\n{body}\nresult=out if 'out' in dir() else {{'ok': True}}\n") }

/// `fusion_view`: first-class camera motion on the active Design viewport (3D) through Fusion's own
/// camera, or on the active Electron editor (2D) through the add-in's smooth WINDOW zoom. The tour's
/// motion uses this verb, so the tour tests it. John, 2026-09-05: motion verbs are first-class.
/// The last 2D box this exe framed (mm), so a glide can start from where the view is.
static LAST_BOX2D: Mutex<Option<(f64, f64, f64, f64)>> = Mutex::new(None);
/// One EAGLE command straight to the add-in, no envelope: a view frame is ~50 ms this way.
fn eagle(cmd: &str) -> Value { crate::addin::addin_http("execute_text_command", &json!({"command": format!("Electron.run {cmd}")}), 8) }
/// A 2D glide driven from HERE, one add-in call per frame. The electronics editor applies a
/// WINDOW box only when the main thread returns to its event loop, and presents it only when
/// the next command arrives (2026-09-05, proven on the package editor with screenshots): a
/// glide sent as one add-in call never shows, a glide sent as separate calls does, and a
/// bare WINDOW (EAGLE's redraw) presents the last frame.
fn glide_2d(from: Option<(f64, f64, f64, f64)>, to: (f64, f64, f64, f64), steps: u64, delay_ms: u64) -> Value {
    let start = from.or(*LAST_BOX2D.lock().unwrap());
    let mm = |v: f64| format!("{v:.3}mm");
    let frame = |b: (f64, f64, f64, f64)| eagle(&format!("WINDOW ({} {}) ({} {})", mm(b.0), mm(b.1), mm(b.2), mm(b.3)));
    let mut frames = 0u64;
    match start {
        None => { let r = frame(to); if r["success"].as_bool() != Some(true) { return r; } frames += 1; }
        Some(s) => {
            for i in 1..=steps.max(1) {
                let t = i as f64 / steps.max(1) as f64; let t = t * t * (3.0 - 2.0 * t);
                let b = (s.0 + (to.0 - s.0) * t, s.1 + (to.1 - s.1) * t, s.2 + (to.2 - s.2) * t, s.3 + (to.3 - s.3) * t);
                let r = frame(b); if r["success"].as_bool() != Some(true) { return r; } frames += 1;
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
        }
    }
    std::thread::sleep(Duration::from_millis(150));
    let _ = eagle("WINDOW");
    *LAST_BOX2D.lock().unwrap() = Some(to);
    json!({"success": true, "frames": frames, "box": [to.0, to.1, to.2, to.3], "from": start.map(|s| json!([s.0, s.1, s.2, s.3]))})
}
pub fn view(pass: &dyn Fn(&str, Value) -> Value, args: &Value) -> Value {
    let action = args["action"].as_str().unwrap_or("").to_lowercase();
    let smooth = args["smooth"].as_bool().unwrap_or(true);
    let space = args["space"].as_str().unwrap_or("3d").to_lowercase();
    if space == "2d" {
        let r = match action.as_str() {
            "fit" => { let r = pass("electron_zoom", json!({"fit": true})); *LAST_BOX2D.lock().unwrap() = None; std::thread::sleep(Duration::from_millis(150)); let _ = eagle("WINDOW"); r },
            "zoom" => { let r = pass("electron_zoom", json!({"factor": args["factor"].as_f64().unwrap_or(1.6), "steps": args["steps"].as_u64().unwrap_or(14)})); *LAST_BOX2D.lock().unwrap() = None; std::thread::sleep(Duration::from_millis(150)); let _ = eagle("WINDOW"); r },
            "pan" => {
                let f = |k: &str| args[k].as_f64();
                let (Some(x1), Some(y1), Some(x2), Some(y2)) = (f("x1"), f("y1"), f("x2"), f("y2")) else { return json!({"success": false, "error": "pan needs x1, y1, x2, y2 in mm"}) };
                let from = args["from"].as_array().filter(|a| a.len() == 4).and_then(|a| Some((a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?, a[3].as_f64()?)));
                glide_2d(from, (x1, y1, x2, y2), args["steps"].as_u64().unwrap_or(6), args["frameDelayMs"].as_u64().unwrap_or(120))
            },
            _ => return json!({"success": false, "error": "2D actions: fit | zoom {factor} | pan {x1,y1,x2,y2}", "_hint": "space:\"2d\" drives the active schematic, board or library editor through Fusion's WINDOW commands."}),
        };
        let mut r = r; r["action"] = json!(action); r["space"] = json!("2d"); return r;
    }
    let body = match action.as_str() {
        "fit" => "c=cam(); c.isFitView=True; put(c, SMOOTH)
out={'action':'fit'}".to_string(),
        "home" => "vp.goHome(SMOOTH)
out={'action':'home'}".to_string(),
        "iso" | "top" | "front" | "right" | "back" | "left" | "bottom" => { let o = match action.as_str() { "iso" => "IsoTopRightViewOrientation", "top" => "TopViewOrientation", "front" => "FrontViewOrientation", "right" => "RightViewOrientation", "back" => "BackViewOrientation", "left" => "LeftViewOrientation", _ => "BottomViewOrientation" }; format!("orient(adsk.core.ViewOrientations.{o}, {})
out={{'action':'{action}'}}", if args["fit"].as_bool().unwrap_or(true) { "True" } else { "False" }) }
        "orbit" => format!("orbit({})
out={{'action':'orbit'}}", args["degrees"].as_f64().unwrap_or(40.0)),
        "tilt" => format!("tilt({})
out={{'action':'tilt'}}", args["degrees"].as_f64().unwrap_or(-25.0)),
        "focus" => format!("focus({},{},{})
out={{'action':'focus'}}", args["x"].as_f64().unwrap_or(0.0), args["y"].as_f64().unwrap_or(0.0), args["z"].as_f64().unwrap_or(0.0)),
        "zoom" => format!("zoom({})
out={{'action':'zoom'}}", 1.0 / args["factor"].as_f64().unwrap_or(1.33).max(0.05)),
        "pan" => format!("pan({},{})
out={{'action':'pan'}}", args["dx"].as_f64().unwrap_or(0.0), args["dy"].as_f64().unwrap_or(0.0)),
        "style" => { let st = match args["style"].as_str().unwrap_or("shaded-edges") { "shaded" => "ShadedVisualStyle", "wireframe" => "WireframeVisualStyle", "hidden-edges" => "ShadedWithHiddenEdgesVisualStyle", "wireframe-hidden" => "WireframeWithHiddenEdgesVisualStyle", _ => "ShadedWithVisibleEdgesOnlyVisualStyle" }; format!("vp.visualStyle=adsk.core.VisualStyles.{st}; vp.refresh()
out={{'action':'style','style':'{st}'}}") }
        _ => return json!({"success": false, "error": "action must be fit | home | iso | top | front | right | back | left | bottom | orbit {degrees} | tilt {degrees} | zoom {factor} | pan {dx,dy} | focus {x,y,z in cm} | style {style}", "_hint": "3D actions move the active Design's camera; pass space:\"2d\" for a schematic, board or library editor."}),
    };
    let script = cam_script(&body).replace("put(c, SMOOTH)", if smooth { "put(c, True)" } else { "put(c, False)" }).replace("vp.goHome(SMOOTH)", if smooth { "vp.goHome(True)" } else { "vp.goHome(False)" });
    let mut r = pass("run_modeling_script", json!({"script": script}));
    r["action"] = json!(action); r["space"] = json!("3d");
    if r["success"].as_bool() == Some(true) { r["_hint"] = json!("Verify by capture: desktop_screenshot_window before and after; the tour records viewChanged this way."); }
    r
}
/// `fusion_narrate {clip}`: play one bundled narration clip on the box now (the dock bar and
/// dashboard lines a driver speaks before the tour starts). Returns the start epoch and the
/// clip length so a recording can be cut on it.
pub fn narrate(args: &Value) -> Value {
    let clip = args["clip"].as_str().unwrap_or("").trim().trim_end_matches(".mp3").to_string();
    if clip.is_empty() || clip.contains('/') || clip.contains('\\') { return json!({"success": false, "error": "clip required: a bundled narration name such as dock or dashboard"}); }
    let f = audio_dir().join(format!("{clip}.mp3"));
    if !f.is_file() { return json!({"success": false, "error": format!("no bundled clip {clip}.mp3"), "_hint": "resources/demo-audio in the bridge cache lists the clips"}); }
    let at = now();
    play_file(&clip, "prelude");
    json!({"success": true, "clip": clip, "startedAt": at, "file": f.to_string_lossy().replace('\\', "/"), "_hint": "Playing on the box now; the tour's own narration stops it when a beat starts."})
}
/// `fusion_interactive`: is Fusion ready for input right now? The add-in's /status busy flag (a
/// long command of ours in flight), Fusion's own active-jobs flag, and how long the main thread
/// took to answer a trivial script. The tour's narrator polls this to stamp busy stretches.
pub fn interactive(pass: &dyn Fn(&str, Value) -> Value, probe_main_thread: bool) -> Value {
    let st = crate::addin::addin_status_probe(Duration::from_secs(3));
    let addin_up = st.is_some();
    let addin_busy = st.as_ref().and_then(|s| s["busy"].as_bool()).unwrap_or(false);
    let mut jobs: Option<bool> = None; let mut latency_ms: Option<u64> = None;
    if probe_main_thread && addin_up && !addin_busy {
        let t0 = Instant::now();
        let r = pass("run_modeling_script", json!({"script": "import adsk.core
result={'jobs': adsk.core.Application.get().hasActiveJobs}"}));
        if r["success"].as_bool() == Some(true) { jobs = r["data"]["result"]["jobs"].as_bool(); latency_ms = Some(t0.elapsed().as_millis() as u64); }
    }
    let interactive = addin_up && !addin_busy && jobs != Some(true) && latency_ms.map(|l| l < 2500).unwrap_or(true);
    json!({"success": true, "interactive": interactive, "addinUp": addin_up, "addinBusy": addin_busy, "activeJobs": jobs, "mainThreadMs": latency_ms, "palette": crate::palette::status(),
        "_hint": if interactive { "Fusion is taking input: drive it. The tour's narrator stamps fusion-busy / fusion-interactive transitions in the record from this probe." } else { "Fusion is busy (a long command, active jobs, or a slow main thread); wait and probe again before driving." }})
}
struct Sample { lbr: String, step: Option<String>, deviceset: String, package: String, pins: usize, pads: usize, pin_pts: Vec<(f64, f64)>, pad_pts: Vec<(f64, f64)> }
fn count(re: &str, text: &str) -> usize { regex::Regex::new(re).map(|r| r.find_iter(text).count()).unwrap_or(0) }
fn first_cap(re: &str, text: &str) -> String { regex::Regex::new(re).ok().and_then(|r| r.captures(text).map(|c| c[1].to_string())).unwrap_or_default() }
/// The bundled single-part sample, staged as AdomDemoSample.lbr (+ .step) in the temp dir.
fn sample() -> Result<Sample, String> {
    let lbr = crate::demo::sample_lbr_pub().ok_or_else(|| "the bundled sample library is missing from this bridge install".to_string())?;
    let text = std::fs::read_to_string(&lbr).map_err(|e| e.to_string())?;
    let step = std::path::Path::new(&lbr).with_extension("step"); let step = step.is_file().then(|| step.to_string_lossy().replace('\\', "/"));
    let pts = |re: &str| -> Vec<(f64, f64)> { regex::Regex::new(re).map(|r| r.captures_iter(&text).filter_map(|c| Some((c[1].parse().ok()?, c[2].parse().ok()?))).collect()).unwrap_or_default() };
    let pin_pts = pts(r#"<pin [^>]*x="([-\d.]+)" y="([-\d.]+)""#);
    let pad_pts = pts(r#"<(?:smd|pad) [^>]*x="([-\d.]+)" y="([-\d.]+)""#);
    Ok(Sample { deviceset: first_cap(r#"<deviceset name="([^"]+)""#, &text), package: first_cap(r#"<package name="([^"]+)""#, &text), pins: count(r"<pin ", &text), pads: count(r"<(smd|pad) ", &text), lbr, step, pin_pts, pad_pts })
}
/// Clean slate: close documents a previous tour left behind (by name, unsaved changes dropped).
fn close_named_docs(pass: &dyn Fn(&str, Value) -> Value, needles: &[String]) -> usize {
    let needles: Vec<String> = needles.iter().filter(|n| !n.is_empty()).map(|n| n.to_lowercase()).collect();
    if needles.is_empty() { return 0; }
    let script = format!("import adsk.core\napp=adsk.core.Application.get()\nneedles={}\nclosed=0\nfor i in range(app.documents.count-1,-1,-1):\n    dd=app.documents.item(i)\n    nm=dd.name.lower()\n    if any(n in nm for n in needles):\n        try:\n            dd.close(False); closed+=1\n        except Exception: pass\nadsk.doEvents()\nresult={{'closed': closed}}", crate::fusion_prefs::py_str(&json!(needles)));
    pass("run_modeling_script", json!({"script": script}))["data"]["result"]["closed"].as_u64().unwrap_or(0) as usize
}
/// The newest hub item named exactly `name`, preferring `project`; None when APS is not signed in
/// or the hub has no such design.
fn find_hub_design(name: &str, project: &str) -> Option<(String, String)> {
    let r = crate::native_aps_search(&json!({"query": name, "limit": 12}));
    if r["success"].as_bool() != Some(true) { return None; }
    let hits: Vec<Value> = r["results"].as_array().or(r["items"].as_array()).or(r["data"]["results"].as_array()).cloned().unwrap_or_default();
    let exact: Vec<&Value> = hits.iter().filter(|h| h["name"].as_str().map(|n| n.trim().eq_ignore_ascii_case(name)).unwrap_or(false) && h["id"].as_str().map(|u| u.contains("fs.file")).unwrap_or(false)).collect();
    let pick = exact.iter().filter(|h| h["projectName"].as_str().map(|p| p.eq_ignore_ascii_case(project)).unwrap_or(false)).max_by_key(|h| h["lastModified"].as_str().unwrap_or("").to_string())
        .or_else(|| exact.iter().max_by_key(|h| h["lastModified"].as_str().unwrap_or("").to_string())).cloned();
    pick.map(|h| (h["id"].as_str().unwrap_or("").to_string(), h["projectName"].as_str().unwrap_or("").to_string()))
}
fn found_none_reason(r: &Value) -> bool { r["open"].is_null() }
fn active_doc(pass: &dyn Fn(&str, Value) -> Value) -> String { pass("get_app_state", json!({}))["data"]["activeDocument"].as_str().unwrap_or("").to_string() }
fn wait_design(pass: &dyn Fn(&str, Value) -> Value, secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline && !EXIT.load(Ordering::Relaxed) {
        let r = pass("run_modeling_script", json!({"script": "import adsk.core, adsk.fusion\napp=adsk.core.Application.get()\nd=adsk.fusion.Design.cast(app.activeProduct)\nresult={'design': d is not None, 'bodies': (d.rootComponent.bRepBodies.count + sum(o.bRepBodies.count for o in d.rootComponent.allOccurrences)) if d else 0}"}));
        if r["data"]["result"]["design"].as_bool() == Some(true) && r["data"]["result"]["bodies"].as_u64().unwrap_or(0) > 0 { return true; }
        sleep_s(2.0);
    }
    false
}
fn wait_active(pass: &dyn Fn(&str, Value) -> Value, pred: &dyn Fn(&str) -> bool, secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline && !EXIT.load(Ordering::Relaxed) { if pred(&active_doc(pass)) { return true; } sleep_s(2.0); }
    false
}

/// Retry a verb while the add-in reports its main thread busy with someone else's command (the
/// dashboard polls the bridge during a tour); bounded to ~12 s. ConfRoomROG 2026-09-05: the 2D board
/// beat failed on a 0.1 s get_app_state from the dashboard.
fn pass_patient(pass: &dyn Fn(&str, Value) -> Value, verb: &str, args: Value) -> Value {
    let t0 = Instant::now();
    loop {
        let r = pass(verb, args.clone());
        let busy = r["success"].as_bool() != Some(true) && r["error"].as_str().map(|e| e.contains("main thread busy") || e.contains("Fusion is busy") || e.contains("busy_")).unwrap_or(false);
        if !busy || t0.elapsed() > Duration::from_secs(20) || EXIT.load(Ordering::Relaxed) {
            let secs = t0.elapsed().as_secs_f64();
            if secs > 1.5 { event("verb", json!({"verb": verb, "secs": (secs * 10.0).round() / 10.0, "ok": r["success"]})); }
            return r;
        }
        sleep_s(0.4);
    }
}
fn run_show(ctx: &Ctx, pass_raw: &dyn Fn(&str, Value) -> Value, args: Value, env: &Env) {
    // once the user exits, every beat verb is refused so the step in flight unwinds at its next
    // call (issue #121); the teardown lifts this to close the tour's documents
    let teardown = AtomicBool::new(false);
    let pass_owned = |verb: &str, a: Value| if EXIT.load(Ordering::Relaxed) && !teardown.load(Ordering::Relaxed) { json!({"success": false, "error": "the user stopped the tour"}) } else { pass_patient(pass_raw, verb, a) };
    let pass: &dyn Fn(&str, Value) -> Value = &pass_owned;
    let run_id = snap()["demoId"].as_str().unwrap_or("").to_string();
    rec(|s| s["bridgeVersion"] = json!(env.version));
    let sd = ctx.state_dir.clone();
    let t0 = Instant::now();
    let total_est: u64 = BEATS.iter().map(|b| b.2).sum();
    let mut done_est: u64 = 0;
    // clean slate per tour
    EXIT.store(false, Ordering::Relaxed); PANEL_SHOWN.store(false, Ordering::Relaxed); PANEL_CLOSED.store(false, Ordering::Relaxed); SEEN_AT.lock().unwrap().clear(); CTRL.lock().unwrap().clear(); LAST_CMD.lock().unwrap().clear();
    MUTED.store(args["muted"].as_bool().unwrap_or(false), Ordering::Relaxed);
    BEAT_BUSY.store(false, Ordering::Relaxed); PAUSED.store(false, Ordering::Relaxed); *NARR.lock().unwrap() = None; GENERIC_USED.lock().unwrap().clear();
    drain_stale_presses();
    restore_bounds(&sd, "leftovers");
    let _ = crate::etiquette::clear_badge(&caller());
    if let Some((h, _)) = fusion_hwnd() { let _ = ab("desktop_taskbar", json!({"hwnd": h, "progress": {"state": "none"}}), 6); }
    let tp_base = crate::testproject::test_project(&sd).and_then(|t| t["docBase"].as_str().map(str::to_string)).unwrap_or_default();
    *STATE_DIR.lock().unwrap() = Some(sd.clone());
    crate::palette::hold(true);
    let board_for_clean = args["board"].as_str().unwrap_or(DEMO_BOARD).to_lowercase();
    let stale = close_named_docs(pass, &["adomdemosample".into(), tp_base.clone(), "ads8588sipm_3d".into(), "adom-test-".into(), board_for_clean, "untitled".into()]);
    // Fusion takes the screen the instant the tour starts: forward once, placed at 95%
    if let Some((h, t)) = fusion_hwnd() { let placed = place_fusion(&sd, h, &t); let fg = foreground_once(h, "the tour is starting"); rec(|s| { s["placed"] = json!(placed["placed"].as_bool() == Some(true)); s["placedAtStart"] = placed; s["foregroundedAtStart"] = json!(fg); }); }
    let docs_before = crate::orchestrations::open_doc_names(pass);
    rec(|s| { s["staleDocsClosed"] = json!(stale); s["docsBefore"] = json!(docs_before.len()); });
    // the hub lookup for the board beat runs now, beside the first beats (run 36: it cost the
    // schematic beat 30 s of dead air when it ran inline)
    let mut board_prefetch: Option<std::thread::JoinHandle<Option<(String, String)>>> = if args["boardUrn"].is_string() { None } else {
        let bn = args["board"].as_str().unwrap_or(DEMO_BOARD).to_string();
        Some(std::thread::spawn(move || find_hub_design(&bn, DEMO_BOARD_PROJECT)))
    };
    let mut results: Vec<Value> = Vec::new();
    let mut i = 0usize;
    let mut exited = false;
    let mut failed_early = false;
    let mut fusion: Option<(i64, String)> = None;
    let mut bound_urn = String::new();
    let mut bound = false;
    let mut chip_cache: Option<Value> = None;
    let mut project_base = String::new();
    let mut hub_board = false;
    let board_name = args["board"].as_str().unwrap_or(DEMO_BOARD).to_string();
    let sample = match sample() { Ok(s) => s, Err(e) => { rec(|s| { s["done"] = json!(true); s["failed"] = json!(true); s["error"] = json!(e); s["stepLabel"] = json!("Tour failed: no sample library"); }); return; } };
    rec(|s| s["sample"] = json!({"lbr": sample.lbr, "deviceset": sample.deviceset, "package": sample.package, "pins": sample.pins, "pads": sample.pads, "step": sample.step}));
    let update_pct = |done: u64, extra: u64| rec(|s| { s["percent"] = json!(((100 * (done + extra)) / total_est.max(1)).min(95)); s["elapsedSec"] = json!(t0.elapsed().as_secs()); s["etaSec"] = json!(total_est.saturating_sub(done + extra)); });
    let n = BEATS.len();
    while i < n {
        // transport between beats: pause holds, exit leaves, prev is TWO back (i is past the beat on screen)
        let mut cmd = take_cmd();
        if cmd.as_deref() == Some("pause") {
            audio_stop(); PAUSED.store(true, Ordering::Relaxed); event("paused", json!({})); panel_show(&run_id, i.max(1), Some("paused")); caption("Fusion tour paused", 300000); rec(|s| { s["paused"] = json!(true); s["stepLabel"] = json!("Paused"); });
            loop { sleep_s(1.0); cmd = take_cmd(); if matches!(cmd.as_deref(), Some("play") | Some("next") | Some("prev") | Some("exit")) { break; } }
            PAUSED.store(false, Ordering::Relaxed); event("resumed", json!({})); rec(|s| s["paused"] = json!(false));
        }
        if cmd.as_deref() == Some("exit") || EXIT.load(Ordering::Relaxed) { exited = true; break; }
        if cmd.as_deref() == Some("prev") { i = i.saturating_sub(2); results.truncate(i); }
        let (key, label, dur) = BEATS[i];
        rec(|s| { s["step"] = json!(key); s["stepLabel"] = json!(label); s["beatIndex"] = json!(i + 1); s["beatCount"] = json!(n); });
        update_pct(done_est, 0);
        panel_show(&run_id, i, None);
        BEAT_BUSY.store(true, Ordering::Relaxed);
        event("beat-start", json!({"index": i + 1, "key": key}));
        event("part-a", json!({"key": key}));
        let beat_t0 = Instant::now();
        let mut r = json!({"step": key, "label": label, "ok": false});
        let mut expected = String::new();
        let mut kind = "none";
        let mut idle_boxes: Vec<Value> = Vec::new();
        let mut idle_kind = String::new();
        let script = |body: &str| pass("run_modeling_script", json!({"script": cam_script(body)}));
        let view = |suffix: &str| crate::orchestrations::show_library_view(ctx, &sample.deviceset, suffix, Some(&sample.lbr), pass);
        // ---- the beat body ----
        match key {
            "library" => {
                caption(&format!("Fusion tour 1/{n}: the sample library - {}, {} pins, {} pads", sample.deviceset, sample.pins, sample.pads), 30000);
                audio_play("library");
                let o = crate::orchestrations::open_lbr(ctx, &json!({"filePath": sample.lbr}), pass);
                r["ok"] = json!(o["success"].as_bool() == Some(true)); r["error"] = o["error"].clone();
                // John, 2026-09-06: the library beat shows the library, not the symbol again:
                // the device view gangs the symbol, the footprint and the 3D preview in one screen
                if r["ok"].as_bool() == Some(true) { let d = view("dev"); r["deviceView"] = json!({"success": d["success"], "error": d["error"]}); }
                expected = "AdomDemoSample".into(); kind = "2d";
            }
            "symbol" => {
                caption(&format!("Fusion tour 2/{n}: the symbol - all {} pins, converted pin for pin", sample.pins), 30000);
                audio_play("symbol");
                let o = view("sym"); r["ok"] = json!(o["success"].as_bool() == Some(true)); r["error"] = o["error"].clone();
                expected = "AdomDemoSample".into(); kind = "2d";
            }
            "footprint" => {
                caption(&format!("Fusion tour 3/{n}: the footprint - {} pads on copper, mask and silk", sample.pads), 30000);
                audio_play("footprint");
                let o = view("pac"); r["ok"] = json!(o["success"].as_bool() == Some(true)); r["error"] = o["error"].clone();
                expected = "AdomDemoSample".into(); kind = "2d";
            }
            "chip" => {
                // a replay (Back) shows the chip this run already bound; the minting lines would narrate work not being done
                if chip_cache.is_some() { caption(&format!("Fusion tour 4/{n}: the 3D chip this tour already minted and bound"), 30000); *NARR.lock().unwrap() = None; }
                else { caption(&format!("Fusion tour 4/{n}: minting the 3D chip from its STEP and binding it (about two minutes)"), 60000); audio_play("chip"); }
                let was = active_doc(pass);
                match (&sample.step, sample.package.is_empty()) {
                    (Some(step), false) => {
                        let on_part = |_p: &Value, k: usize, total: usize| caption(&format!("3D chip: part {k} of {total} minted and bound"), 20000);
                        let (br, reused) = chip_bind_once(&mut chip_cache, || crate::mint::build_library_3d_core(ctx, &json!({"lbrPath": sample.lbr, "parts": [{"package": sample.package, "lbrPath": sample.lbr, "modelPath": step}], "outLbrPath": sample.lbr, "capture": false}), pass, &on_part));
                        bound = br["success"].as_bool() == Some(true) && br["partsBound"].as_u64().unwrap_or(0) >= 1;
                        bound_urn = br["parts"][0]["wip_urn"].as_str().unwrap_or("").to_string();
                        r["bind"] = json!({"success": bound, "partsBound": br["partsBound"], "error": br["error"], "urn": bound_urn, "dims_mm": br["parts"][0]["dims_mm"], "reused": reused});
                        rec(|s| s["bound3d"] = json!(bound));
                        if bound && !bound_urn.is_empty() && !EXIT.load(Ordering::Relaxed) {
                            let o = pass("open_by_urn", json!({"urn": bound_urn}));
                            r["open"] = json!({"success": o["success"], "error": o["error"]});
                            r["ok"] = json!(wait_active(pass, &|d| !d.is_empty() && d != was && !d.to_lowercase().contains("adomdemosample"), 90));
                            if r["ok"].as_bool() == Some(true) { caption(&format!("The minted {} - a real design in your hub, bound to the footprint", sample.package), 20000); let _ = script("vp.visualStyle=adsk.core.VisualStyles.ShadedWithVisibleEdgesOnlyVisualStyle\norient(adsk.core.ViewOrientations.IsoTopRightViewOrientation, True)\nout={'iso': True}"); }
                            else { r["error"] = json!("the bound f3d did not become the active document"); }
                        }
                        if r["ok"].as_bool() != Some(true) && !EXIT.load(Ordering::Relaxed) {
                            // graceful: show the chip from the STEP alone and say so
                            let why = br["error"].as_str().unwrap_or("cloud bind unavailable").chars().take(90).collect::<String>();
                            caption(&format!("3D chip from the STEP alone ({why})"), 20000);
                            let o = pass("import_step", json!({"filePath": step}));
                            r["fallback"] = json!({"importStep": o["success"], "error": o["error"], "why": why});
                            r["ok"] = json!(o["success"].as_bool() == Some(true) && wait_active(pass, &|d| !d.is_empty() && d != was, 60));
                            if r["ok"].as_bool() == Some(true) { let _ = script("orient(adsk.core.ViewOrientations.IsoTopRightViewOrientation, True)\nout={'iso': True}"); }
                        }
                    }
                    _ => { r["error"] = json!("no STEP staged beside the sample library"); }
                }
                kind = "3d";
            }
            "schematic" => {
                // a real board from the hub first; the generated solo board only when the hub has none
                let found = match args["boardUrn"].as_str() { Some(u) => Some((u.to_string(), String::new())), None => match board_prefetch.take() { Some(h) => h.join().ok().flatten(), None => find_hub_design(&board_name, DEMO_BOARD_PROJECT) } };
                if let Some((urn, proj)) = found.filter(|_| !EXIT.load(Ordering::Relaxed)) {
                    caption(&format!("Fusion tour 5/{n}: opening the {board_name} board from your hub (about a minute)"), 90000);
                    audio_play("schematic");
                    let o = pass("open_by_urn", json!({"urn": urn}));
                    r["open"] = json!({"urn": urn, "project": proj, "success": o["success"], "error": o["error"]});
                    let bn = board_name.to_lowercase();
                    let opened = o["success"].as_bool() == Some(true) && wait_active(pass, &|d| d.to_lowercase().starts_with(&bn), 150);
                    if opened {
                        let o2 = pass("show_schematic", json!({}));
                        r["ok"] = json!(o2["success"].as_bool() == Some(true)); r["error"] = o2["error"].clone();
                        if r["ok"].as_bool() == Some(true) { hub_board = true; project_base = board_name.clone(); caption(&format!("The {board_name} schematic - every net and part as the designer drew it"), 20000); }
                    } else { r["error"] = json!(format!("{board_name} did not become the active document")); }
                }
                if !hub_board && !EXIT.load(Ordering::Relaxed) {
                    let why = if found_none_reason(&r) { "your hub has no board by that name, or APS is not signed in" } else { "the hub board did not open" };
                    caption(&format!("Fusion tour 5/{n}: {why} - generating a board around {} instead", sample.deviceset), 60000);
                    audio_play("schematic-solo");
                    let m = crate::testproject::make_test_project(ctx, &json!({"libraryPath": sample.lbr}), pass);
                    r["makeProject"] = json!({"success": m["success"], "error": m["error"], "why": why});
                    if m["success"].as_bool() == Some(true) {
                        project_base = crate::testproject::test_project(&sd).and_then(|t| t["docBase"].as_str().map(str::to_string)).unwrap_or_default();
                        sleep_s(2.0);
                        let o = pass("show_schematic", json!({}));
                        r["ok"] = json!(o["success"].as_bool() == Some(true)); r["error"] = o["error"].clone();
                        if r["ok"].as_bool() == Some(true) { caption(&format!("The schematic: {} wired into a real sheet", sample.deviceset), 20000); }
                    } else { r["error"] = m["error"].clone(); }
                }
                r["hubBoard"] = json!(hub_board);
                expected = project_base.clone(); kind = "2d";
            }
            "board2d" => {
                caption(&format!("Fusion tour 6/{n}: the {} in 2D - real copper, real routing", if hub_board { board_name.clone() } else { "board".to_string() }), 30000);
                audio_play(if hub_board { "board2d" } else { "board2d-solo" });
                // the hub design carries schematic, board and 3D as one document set with one name:
                // never activate by substring (four documents share it); switch views inside it
                let act = if hub_board { true } else { crate::testproject::test_project_activate(ctx, "brd", pass) };
                let o = pass("show_2d_board", json!({}));
                r["activatedBoard"] = json!(act); r["ok"] = json!(o["success"].as_bool() == Some(true)); r["error"] = o["error"].clone();
                expected = project_base.clone(); kind = "2d";
            }
            "board3d" => {
                caption(&format!("Fusion tour 7/{n}: the {} in 3D{}", if hub_board { board_name.clone() } else { "board".to_string() }, if !hub_board && bound { ", carrying the chip minted a minute ago" } else { "" }), 30000);
                audio_play(if hub_board { "board3d" } else { "board3d-solo" });
                let act = if hub_board { pass("show_2d_board", json!({}))["success"].as_bool() == Some(true) } else { crate::testproject::test_project_activate(ctx, "brd", pass) };
                if act { sleep_s(1.5); }
                let was = active_doc(pass);
                let o = pass("show_3d_board", json!({}));
                r["activatedBoard"] = json!(act); r["ok"] = json!(o["success"].as_bool() == Some(true)); r["error"] = o["error"].clone();
                // the 3D board keeps the board document's NAME, so wait for a Design product, not a new name
                if r["ok"].as_bool() == Some(true) { let _ = was; r["designActive"] = json!(wait_design(pass, 90)); sleep_s(1.5); let _ = script("orient(adsk.core.ViewOrientations.IsoTopRightViewOrientation, True)\nout={'iso': True}"); }
                expected = project_base.clone(); kind = "3d";
            }
            "aps" => {
                let q: String = sample.deviceset.chars().take(8).collect();
                let q0 = Instant::now();
                let a = crate::native_aps_search(&json!({"query": q, "limit": 5}));
                let secs = q0.elapsed().as_secs_f64();
                if a["success"].as_bool() == Some(true) {
                    let hits = a["results"].as_array().or(a["items"].as_array()).or(a["data"]["results"].as_array()).map(|a| a.len()).unwrap_or(0);
                    caption(&format!("Fusion tour 8/{n}: cloud search - {hits} hits for {q} in {secs:.1} s, never charged"), 20000);
                    r["ok"] = json!(true); r["hits"] = json!(hits); r["seconds"] = json!(secs);
                } else {
                    caption(&format!("Fusion tour 8/{n}: cloud search skipped - not signed in to APS"), 20000);
                    r["ok"] = json!(true); r["skipped"] = json!(a["error"].as_str().unwrap_or("not signed in").chars().take(100).collect::<String>());
                }
                audio_play("aps");
            }
            _ => {}
        }
        // a failed action narrates nothing more of its beat: the clip and fillers stop now
        if r["ok"].as_bool() != Some(true) { cut_narration(); }
        if key == "library" && r["ok"].as_bool() != Some(true) {
            let why = r["error"].as_str().unwrap_or("the sample library did not open").to_string();
            caption(&format!("Fusion tour stopped: {why}"), 8000);
            r["seconds"] = json!(beat_t0.elapsed().as_secs_f64());
            results.push(r.clone()); rec(|s| { push(s, "steps", r.clone()); s["error"] = json!(why); });
            failed_early = true; break;
        }
        event("action-done", json!({"ok": r["ok"], "error": r["error"]}));
        // ---- measure, then move ----
        if r["ok"].as_bool() == Some(true) && !EXIT.load(Ordering::Relaxed) && key != "aps" {
            if fusion.is_none() { fusion = fusion_hwnd(); }
            if let Some((h, t)) = &fusion {
                if !r["placed"].is_object() && !snap()["placed"].as_bool().unwrap_or(false) { r["placed"] = place_fusion(&sd, *h, t); rec(|s| s["placed"] = json!(true)); }
                r["foregrounded"] = json!(front_ok(*h) || foreground_once(*h, &format!("the {label} beat")));
                { let pb = snap()["placedAtStart"].clone(); let pk = park_palette(&pb); if pk["parked"].as_bool() == Some(true) && pk["already"].as_bool() != Some(true) { rec(|s| s["palettePlaced"] = pk.clone()); } }
                let v = verify_beat(*h, &expected);
                r["verified"] = v.clone();
                rec(|s| s["verified"] = v.clone());
                event("verified", json!({"ok": v["ok"], "front": v["front"], "painted": v["painted"], "stable": v["stable"]}));
                if v["ok"].as_bool() == Some(true) && kind != "none" {
                    // the editor on screen must be the one the caption names (run 37 showed the
                    // board under the schematic sentence): probe, switch again, record the truth
                    match key { "schematic" => { r["editor"] = assert_editor(pass, "Schematic", "SwitchSchDocCmd"); } "board2d" => { r["editor"] = assert_editor(pass, "Board", "SwitchPcbDocCmd"); } _ => {} }
                    let mut parts = match key { "schematic" => part_positions(ctx, env, pass, "Instance"), "board2d" | "board3d" => part_positions(ctx, env, pass, "Element"), _ => Vec::new() };
                    if parts.is_empty() && matches!(key, "schematic" | "board2d" | "board3d") { sleep_s(3.0); parts = part_positions(ctx, env, pass, if key == "schematic" { "Instance" } else { "Element" }); }
                    let plan = plan_for(key, &sample, &parts, &board_name);
                    r["parts"] = json!(parts.len());
                    event("part-b", json!({"key": key}));
                    audio_play_b(key);
                    event("motion-start", json!({"kind": kind, "plan": plan.iter().map(|p| p.0).collect::<Vec<_>>(), "parts": parts.len()}));
                    idle_boxes = plan.iter().filter(|p| p.1["action"] == "pan").map(|p| p.1.clone()).collect();
                    idle_kind = kind.to_string();
                    r["motion"] = motion(*h, kind, pass, plan.clone());
                    event("motion-done", json!({"gestures": r["motion"]["gestures"], "viewChanged": r["motion"]["viewChanged"], "aborted": r["motion"]["aborted"]}));
                    // a motion that never moved the view gets one more pass (issue #121), then the beat fails below
                    if motion_needs_retry(&r["motion"]) && !EXIT.load(Ordering::Relaxed) {
                        let first = r["motion"].clone();
                        r["motion"] = motion(*h, kind, pass, plan);
                        r["motion"]["firstPass"] = first;
                        event("motion-retry", json!({"gestures": r["motion"]["gestures"], "viewChanged": r["motion"]["viewChanged"], "aborted": r["motion"]["aborted"]}));
                    }
                } else if kind != "none" {
                    r["motion"] = json!({"skipped": format!("not verified: front={} title={} painted={} (top window: {})", v["front"], v["title"], v["painted"], v["frontTitle"])});
                } else if v["ok"].as_bool() == Some(true) { event("part-b", json!({"key": key})); audio_play_b(key); }
            } else { r["verified"] = json!({"why": "no Fusion window"}); }
        }
        // issue #121: a beat that failed, was not on screen, or never moved says so briefly and
        // moves on, instead of narrating over a view that does not show it
        if !EXIT.load(Ordering::Relaxed) { if let Some(why) = beat_failure(&r) {
            if r["ok"].as_bool() == Some(true) { r["ok"] = json!(false); r["error"] = json!(why.clone()); }
            cut_narration();
            caption(&failed_caption(i + 1, n, label, &why), 6000);
            r["failedNarration"] = json!(true);
            event("beat-failed", json!({"key": key, "why": why}));
            let hold = Instant::now();
            while hold.elapsed() < Duration::from_secs(3) && !pending_interrupt() { sleep_s(0.3); }
        } }
        // evidence frame at the moment the beat completes
        if let Some((h, _)) = &fusion { if let Some(p) = shot_window(*h) { if let Ok(b) = std::fs::read(&p) { if b.len() <= 4 * 1024 * 1024 { let img = crate::etiquette::b64_pub(&b); rec(|s| { push(s, "shots", json!({"label": label, "title": label, "path": p, "image": img})); }); } } } }
        r["seconds"] = json!(beat_t0.elapsed().as_secs_f64());
        results.push(r.clone());
        rec(|s| push(s, "steps", r.clone()));
        done_est += dur;
        update_pct(done_est, 0);
        // advance at max(action done, audio done): the narrator stops starting fillers now, and the
        // beat ends when the sentence on air ends (bounded), honouring the remote
        BEAT_BUSY.store(false, Ordering::Relaxed);
        let wait_t0 = Instant::now();
        let mut tick: u64 = 0;
        let mut idle_gestures: u64 = 0;
        while audio_playing() && wait_t0.elapsed() < Duration::from_secs(40) {
            if EXIT.load(Ordering::Relaxed) { break; }
            if let Some(c) = take_cmd() { CTRL.lock().unwrap().push_front(c); break; }
            // keep the content moving while the narration finishes
            if !idle_kind.is_empty() && r["motion"]["gestures"].as_u64().unwrap_or(0) > 0 && fusion.as_ref().map(|(h, _)| front_ok(*h)).unwrap_or(false) {
                let o = idle_step(&idle_kind, tick, &idle_boxes, pass); tick += 1;
                if o["success"].as_bool() == Some(true) { idle_gestures += 1; }
                sleep_s(if idle_kind == "2d" { 2.2 } else { 1.0 });
            } else { sleep_s(0.4); }
        }
        if idle_gestures > 0 { r["motion"]["idleGestures"] = json!(idle_gestures); }
        event("beat-end", json!({"index": i + 1, "key": key, "seconds": beat_t0.elapsed().as_secs_f64()}));
        if EXIT.load(Ordering::Relaxed) { exited = true; break; }
        i += 1;
    }
    BEAT_BUSY.store(false, Ordering::Relaxed);
    teardown.store(true, Ordering::Relaxed);
    event("tour-end", json!({"exited": exited}));
    // ---- teardown: audio off, panel down, windows back, the tour marked done; THEN the tour's
    // documents close (closing takes a minute and must not read as dead air) ----
    audio_stop(); panel_hide();
    let ok_ct = results.iter().filter(|r| r["ok"].as_bool() == Some(true)).count();
    if exited { caption("Fusion tour stopped", 4000); } else if failed_early { caption("Fusion tour stopped: the sample library did not open", 6000); } else { audio_play("done"); caption(&format!("Fusion tour done: {ok_ct} of {n} beats, from symbol to 3D board to cloud search"), 8000); }
    rec(|s| { s["done"] = json!(true); s["exited"] = json!(exited); s["failed"] = json!(failed_early || (!exited && ok_ct < n)); s["percent"] = json!(100); s["windowsRestored"] = Value::Null; s["tourDocsClosed"] = json!("closing"); s["elapsedSec"] = json!(t0.elapsed().as_secs()); s["step"] = json!("done"); s["stepLabel"] = json!(if exited { "Tour stopped" } else if failed_early { "Tour failed: the sample library did not open" } else { "Tour complete" });
        s["result"] = json!({"openedBeats": ok_ct, "totalBeats": n, "exited": exited, "bound3d": bound, "failedSteps": results.iter().filter(|r| r["ok"].as_bool() != Some(true)).map(|r| r["step"].clone()).collect::<Vec<_>>()}); s["narrate"] = json!(format!("Fusion tour {}: {ok_ct} of {n} beats across the electronics library, the 3D chip and a generated board.", if exited { "stopped" } else { "complete" })); });
    let s = snap(); crate::demo::persist_pub(&sd, &s);
    let my_id = s["demoId"].as_str().unwrap_or("").to_string();
    let close_t0 = Instant::now();
    // John, 2026-09-05: the outro shows the documents closing (that is a feature too); the
    // windows go back to where they were only after the last tab is gone
    let closed = crate::mint::close_docs_opened_since(ctx, &docs_before, &[], pass);
    let restored = restore_bounds(&sd, if exited { "stop" } else { "end" });
    crate::palette::hold(false); crate::palette::hide();
    rec_for(&sd, &my_id, |s| { s["tourDocsClosed"] = json!(closed); s["closeSec"] = json!(close_t0.elapsed().as_secs()); s["windowsRestored"] = json!(restored); });
    if snap()["demoId"].as_str() == Some(my_id.as_str()) { let s = snap(); crate::demo::persist_pub(&sd, &s); }
}

fn bbox(pts: &[(f64, f64)]) -> Option<(f64, f64, f64, f64)> { if pts.is_empty() { return None; } Some((pts.iter().map(|p| p.0).fold(f64::MAX, f64::min), pts.iter().map(|p| p.1).fold(f64::MAX, f64::min), pts.iter().map(|p| p.0).fold(f64::MIN, f64::max), pts.iter().map(|p| p.1).fold(f64::MIN, f64::max))) }
fn pan_box(x1: f64, y1: f64, x2: f64, y2: f64) -> Value { json!({"action": "pan", "x1": x1, "y1": y1, "x2": x2, "y2": y2, "steps": 6, "frameDelayMs": 150}) }
/// A pause inside a plan: the package editor drops any WINDOW box that arrives within ~8 s of a
/// WINDOW FIT, so a plan that must fit first waits before its first glide.
fn settle(seconds: f64) -> Value { json!({"action": "wait", "seconds": seconds}) }
/// The first glide after WINDOW FIT starts from the fitted extent (the add-in cannot know it).
fn pan_from(fit: (f64, f64, f64, f64), x1: f64, y1: f64, x2: f64, y2: f64) -> Value { let mut b = pan_box(x1, y1, x2, y2); b["from"] = json!([fit.0, fit.1, fit.2, fit.3]); b }
/// The pointer walks the left pin column of a fitted symbol (window fractions, 4K box): pins
/// highlight under it, which is what a person does when they read a symbol.
fn pin_sweep() -> Value { json!({"action": "sweep", "path": [[0.40, 0.30], [0.40, 0.37], [0.40, 0.44], [0.40, 0.51], [0.40, 0.58], [0.40, 0.65], [0.62, 0.60], [0.62, 0.45], [0.62, 0.32]]}) }
/// Positions of the parts on the active board or schematic (mm), from the electronics read model.
fn part_positions(ctx: &Ctx, env: &Env, pass: &dyn Fn(&str, Value) -> Value, entity: &str) -> Vec<(String, String, f64, f64)> {
    let r = crate::electronics::query(ctx, &json!({"entity": entity, "fields": ["name", "value", "x", "y"], "limit": 600}), env.version, pass);
    r["items"].as_array().or(r["rows"].as_array()).map(|a| a.iter().filter_map(|it| Some((it["name"].as_str().unwrap_or("").to_string(), it["value"].as_str().unwrap_or("").to_string(), it["x"].as_f64()?, it["y"].as_f64()?))).collect()).unwrap_or_default()
}
/// Which electronics editor is on screen, from Fusion itself; re-issue the switch when it is not
/// the one wanted. `want` is a substring of DebugCommands.ActiveWorkspace ("Schematic", "Board").
fn assert_editor(pass: &dyn Fn(&str, Value) -> Value, want: &str, switch_cmd: &str) -> Value {
    let probe = |pass: &dyn Fn(&str, Value) -> Value| -> (String, String) {
        let r = pass("run_modeling_script", json!({"script": "import adsk.core\napp=adsk.core.Application.get()\nws=''\ntry:\n    ws=app.executeTextCommand('DebugCommands.ActiveWorkspace')\nexcept Exception:\n    pass\npt=''\ntry:\n    pt=app.activeProduct.productType\nexcept Exception:\n    pass\nresult={'ws': ws, 'pt': pt}"}));
        (r["data"]["result"]["ws"].as_str().unwrap_or("").trim().to_string(), r["data"]["result"]["pt"].as_str().unwrap_or("").to_string())
    };
    let (ws0, pt0) = probe(pass);
    let mut out = json!({"want": want, "workspace": ws0, "productType": pt0, "switched": false});
    // the workspace name or the product type may carry the word (run 38: the board editor
    // reports BoardProductType under a workspace not named "Board")
    let is_wanted = |ws: &str, pt: &str| ws.contains(want) || pt.contains(want);
    if !is_wanted(&ws0, &pt0) {
        let _ = pass("execute_text_command", json!({"command": format!("Commands.Start {switch_cmd}")}));
        sleep_s(2.5);
        let _ = pass("electron_run", json!({"command": "WINDOW FIT"}));
        let (ws1, pt1) = probe(pass);
        out["switched"] = json!(true); out["workspaceAfter"] = json!(ws1.clone()); out["productTypeAfter"] = json!(pt1.clone()); out["ok"] = json!(is_wanted(&ws1, &pt1));
        eprintln!("[fusion-bridge] tour: the {want} editor was not on screen ({ws0:?}); switched again -> {ws1:?}");
    } else { out["ok"] = json!(true); }
    out
}
fn find_ic(parts: &[(String, String, f64, f64)], board_name: &str) -> Option<(f64, f64)> {
    let bn = board_name.to_lowercase();
    parts.iter().find(|p| !bn.is_empty() && p.1.to_lowercase().contains(&bn)).or_else(|| parts.iter().find(|p| p.0.to_uppercase().starts_with("IC") || p.0.to_uppercase().starts_with("U1"))).map(|p| (p.2, p.3))
}
/// The motion plan for a beat: what an engineer would look at, from the data on screen. John,
/// 2026-09-05: "lots of smart motion as if the user is inspecting things in each window".
fn plan_for(key: &str, sample: &Sample, parts: &[(String, String, f64, f64)], board_name: &str) -> Vec<(&'static str, Value)> {
    // 2D motion is built from the two gestures Fusion's electronics editors render every time
    // (2026-09-05, ~40 screenshot trials on the package editor): a smooth WINDOW factor zoom
    // and the real pointer moving over the drawing (pads and pins highlight, tooltips name
    // them). A WINDOW box (pan) is applied only sometimes and presented later or never, so
    // no plan uses one.
    let fit2 = || json!({"action": "fit"});
    let z = |f: f64| json!({"action": "zoom", "factor": f, "steps": 12});
    let zoom_only = || vec![("zoom-in", z(2.2)), ("zoom-out", z(0.45)), ("zoom-in-2", z(1.6)), ("fit-2", fit2())];
    let _ = (sample, parts, board_name);
    match key {
        // the device view is a form (symbol, footprint, 3D preview side by side): the pointer
        // walks the three panes; no WINDOW commands there
        // measured on the 4K box (component view): symbol centred ~0.50/0.55, the package
        // table top right ~0.90/0.35, the footprint and 3D previews bottom right ~0.86/0.83
        "library" => vec![("the symbol", json!({"action": "sweep", "path": [[0.42, 0.35], [0.46, 0.45], [0.50, 0.55], [0.55, 0.62]]})), ("the package table", json!({"action": "sweep", "path": [[0.80, 0.30], [0.86, 0.34], [0.92, 0.35]]})), ("footprint and 3D previews", json!({"action": "sweep", "path": [[0.84, 0.78], [0.86, 0.83], [0.92, 0.83], [0.94, 0.86]]})), ("back to the symbol", json!({"action": "sweep", "path": [[0.70, 0.60], [0.55, 0.50], [0.45, 0.42]]})), ("zoom-in", z(1.6)), ("zoom-out", z(0.62))],
        "symbol" => vec![("pin sweep", pin_sweep()), ("zoom-in", z(2.0)), ("read the pins", pin_sweep()), ("zoom-out", z(0.5)), ("zoom-in-2", z(1.5)), ("fit-2", fit2())],
        "footprint" => {
            // after WINDOW FIT the footprint fills the canvas; the pointer sweeps the pad rows
            // (window fractions measured on the 4K box: canvas 0.25..0.77 x 0.19..0.87)
            let sweep = json!({"action": "sweep", "path": [[0.42, 0.32], [0.50, 0.32], [0.58, 0.32], [0.62, 0.45], [0.62, 0.60], [0.62, 0.72], [0.50, 0.77], [0.38, 0.70], [0.38, 0.50]]});
            let inner = json!({"action": "sweep", "path": [[0.44, 0.40], [0.56, 0.40], [0.58, 0.55], [0.56, 0.66], [0.44, 0.66], [0.42, 0.52]]});
            vec![("pad sweep", sweep), ("zoom-in", z(1.8)), ("pad sweep close", inner), ("zoom-out", z(0.55)), ("zoom-in-2", z(2.4)), ("fit-2", fit2())]
        }
        "schematic" => vec![("zoom-in", z(2.2)), ("net sweep", json!({"action": "sweep", "path": [[0.40, 0.40], [0.50, 0.42], [0.60, 0.45], [0.60, 0.58], [0.48, 0.62], [0.40, 0.56]]})), ("zoom-out", z(0.45)), ("zoom-in-2", z(1.6)), ("fit-2", fit2())],
        "board2d" => vec![("zoom-in", z(2.4)), ("copper sweep", json!({"action": "sweep", "path": [[0.42, 0.42], [0.50, 0.45], [0.58, 0.48], [0.58, 0.58], [0.48, 0.60], [0.42, 0.54]]})), ("zoom-out", z(0.42)), ("zoom-in-2", z(1.7)), ("fit-2", fit2())],
        "chip" => vec![("iso", json!({"action": "iso"})), ("orbit-1", json!({"action": "orbit", "degrees": 35})), ("tilt-down", json!({"action": "tilt", "degrees": -28})), ("zoom-leads", json!({"action": "zoom", "factor": 1.7})), ("orbit-2", json!({"action": "orbit", "degrees": 55})), ("orbit-3", json!({"action": "orbit", "degrees": 55})), ("tilt-up", json!({"action": "tilt", "degrees": 30})), ("zoom-out", json!({"action": "zoom", "factor": 0.6})), ("home", json!({"action": "iso"}))],
        "schematic" => {
            let Some((x1, y1, x2, y2)) = bbox(&parts.iter().map(|p| (p.2, p.3)).collect::<Vec<_>>()) else { return zoom_only() };
            let ic = find_ic(parts, board_name).unwrap_or(((x1 + x2) / 2.0, (y1 + y2) / 2.0));
            let left = parts.iter().min_by(|a, b| a.2.partial_cmp(&b.2).unwrap()).map(|p| (p.2, p.3)).unwrap_or((x1, y2));
            let right = parts.iter().max_by(|a, b| a.2.partial_cmp(&b.2).unwrap()).map(|p| (p.2, p.3)).unwrap_or((x2, y1));
            let r = 28.0;
            vec![("the controller", pan_box(ic.0 - r, ic.1 - r * 0.6, ic.0 + r, ic.1 + r * 0.6)), ("input side", pan_box(left.0 - r, left.1 - r * 0.6, left.0 + r, left.1 + r * 0.6)), ("output side", pan_box(right.0 - r, right.1 - r * 0.6, right.0 + r, right.1 + r * 0.6)), ("fit-2", fit2())]
        }
        "board2d" => {
            let Some((x1, y1, x2, y2)) = bbox(&parts.iter().map(|p| (p.2, p.3)).collect::<Vec<_>>()) else { return zoom_only() };
            let ic = find_ic(parts, board_name).unwrap_or(((x1 + x2) / 2.0, (y1 + y2) / 2.0));
            let r = 9.0;
            vec![("the controller", pan_box(ic.0 - r, ic.1 - r * 0.6, ic.0 + r, ic.1 + r * 0.6)), ("lower-left corner", pan_box(x1 - 4.0, y1 - 4.0, x1 + 2.0 * r, y1 + 1.4 * r)), ("upper-right corner", pan_box(x2 - 2.0 * r, y2 - 1.4 * r, x2 + 4.0, y2 + 4.0)), ("fit-2", fit2())]
        }
        "board3d" => {
            let ic = find_ic(parts, board_name).map(|(x, y)| (x / 10.0, y / 10.0));
            let mut v = vec![("iso", json!({"action": "iso"})), ("orbit-1", json!({"action": "orbit", "degrees": 40}))];
            match ic {
                Some((cx, cy)) => { v.push(("focus the controller", json!({"action": "focus", "x": cx, "y": cy, "z": 0.15}))); v.push(("zoom-in", json!({"action": "zoom", "factor": 2.2}))); v.push(("tilt-low", json!({"action": "tilt", "degrees": -30}))); v.push(("orbit-2", json!({"action": "orbit", "degrees": 70}))); v.push(("tilt-back", json!({"action": "tilt", "degrees": 25}))); v.push(("zoom-out", json!({"action": "zoom", "factor": 0.45}))); }
                None => { v.push(("zoom-in", json!({"action": "zoom", "factor": 1.5}))); v.push(("tilt-low", json!({"action": "tilt", "degrees": -30}))); v.push(("orbit-2", json!({"action": "orbit", "degrees": 70}))); v.push(("zoom-out", json!({"action": "zoom", "factor": 0.66}))); }
            }
            v.push(("whole board", json!({"action": "iso"}))); v
        }
        _ => vec![],
    }
}
/// One gentle idle gesture (John, 2026-09-05: the content never sits still): in 3D a slow orbit
/// with a breath of zoom every few ticks; in 2D a slow drift to the next inspection box.
fn idle_step(kind: &str, tick: u64, boxes: &[Value], pass: &dyn Fn(&str, Value) -> Value) -> Value {
    if kind == "3d" {
        let args = if tick % 7 == 6 { json!({"action": "zoom", "factor": if (tick / 7) % 2 == 0 { 1.12 } else { 0.9 }}) } else { json!({"action": "orbit", "degrees": 6}) };
        view(pass, &args)
    } else if kind == "2d" {
        let _ = boxes;
        view(pass, &json!({"space": "2d", "action": "zoom", "factor": if tick % 2 == 0 { 1.18 } else { 0.85 }, "steps": 10}))
    } else { json!({"success": true}) }
}
/// Motion on a verified window only; every gesture re-checks the front and the remote.
/// 2D editors move through the add-in's smooth WINDOW zoom; 3D through Fusion's own camera.
fn motion(hwnd: i64, kind: &str, pass: &dyn Fn(&str, Value) -> Value, plan: Vec<(&str, Value)>) -> Value {
    let mut rep = json!({"kind": kind, "gestures": 0, "aborted": null, "plan": plan.iter().map(|p| p.0).collect::<Vec<_>>()});
    let before = shot_window(hwnd).and_then(|p| digest(&p));
    // a plan may end where it began (iso, fit), so the view counts as changed once ANY frame
    // after a gesture differs from the first, not only the last one
    let mut changed = false;
    for (name, body) in plan {
        if pending_interrupt() { rep["aborted"] = json!(format!("stop requested before {name}")); break; }
        if !front_ok(hwnd) { rep["aborted"] = json!(format!("window not in front before {name} (user switched away)")); break; }
        let o = if body["action"] == "wait" {
            sleep_s(body["seconds"].as_f64().unwrap_or(2.0)); json!({"success": true})
        } else if body["action"] == "sweep" {
            // the real pointer, no overlay: pads highlight under it
            let mut ok = true;
            for pt in body["path"].as_array().cloned().unwrap_or_default() {
                if pending_interrupt() { break; }
                let r = ab("desktop_hover", json!({"hwnd": hwnd, "fx": pt[0], "fy": pt[1], "cursor": false, "reason": "Fusion tour: sweep the pointer across the pads so they highlight"}), 6);
                if r["success"].as_bool() != Some(true) { ok = false; break; }
                sleep_s(0.18);
            }
            json!({"success": ok, "error": if ok { Value::Null } else { json!("hover refused") }})
        } else if kind == "2d" { let mut b = body.clone(); b["space"] = json!("2d"); view(pass, &b) } else { view(pass, &body) };
        if o["success"].as_bool() != Some(true) { rep["aborted"] = json!(format!("{name}: {}", o["error"].as_str().unwrap_or("script failed"))); break; }
        rep["gestures"] = json!(rep["gestures"].as_u64().unwrap_or(0) + 1);
        // let the eye land before the next move; a 2D fit needs longer, the editor is still
        // laying it out and a box sent inside that window is dropped (2026-09-05)
        sleep_s(if kind == "2d" && body["action"] == "fit" { 3.0 } else if kind == "2d" { 1.2 } else { 1.0 });
        if !changed && before.is_some() { changed = shot_window(hwnd).and_then(|p| digest(&p)).map(|d| Some(d) != before).unwrap_or(false); }
    }
    let after = shot_window(hwnd).and_then(|p| digest(&p));
    rep["viewChanged"] = json!(changed || (before.is_some() && after.is_some() && before != after));
    rep["viewMeasured"] = json!(before.is_some());
    rep
}

// ── failed beats (issue #121) ─────────────────────────────────────────────────
/// Stop the beat's narration at once: no clip on air, no next filler.
fn cut_narration() { BEAT_BUSY.store(false, Ordering::Relaxed); *NARR.lock().unwrap() = None; audio_stop(); }
/// A motion that ran, was measured, was not cut short by the remote, and never moved the view.
fn motion_needs_retry(m: &Value) -> bool {
    m["viewChanged"].as_bool() == Some(false) && m["viewMeasured"].as_bool() != Some(false) && m["gestures"].as_u64().unwrap_or(0) > 0 && m["aborted"].is_null()
}
/// Why the beat must not be narrated as shown, or None when its view is on screen: the action
/// failed, the view could not be proven on screen, or its motion never moved the view (after the retry).
fn beat_failure(r: &Value) -> Option<String> {
    if r["ok"].as_bool() != Some(true) { return Some(short_why(r["error"].as_str().unwrap_or("the step did not complete"))); }
    if r["verified"]["ok"].as_bool() == Some(false) { return Some("its view could not be confirmed on screen".into()); }
    if motion_needs_retry(&r["motion"]) { return Some("the view did not move".into()); }
    None
}
/// The first sentence of an error, at most 80 characters, for a one-line caption.
fn short_why(e: &str) -> String {
    let e = e.trim();
    let first = e.split(". ").next().unwrap_or(e).trim_end_matches('.');
    if first.chars().count() <= 80 { first.to_string() } else { format!("{}...", first.chars().take(77).collect::<String>().trim_end()) }
}
fn failed_caption(index: usize, n: usize, label: &str, why: &str) -> String { format!("Fusion tour {index}/{n}: {label} - this step failed ({why}); moving on") }
/// The chip beat mints at most once per tour run (issue #121: Back over it minted a second STEP
/// binding with a new URN). A replay reuses the first successful bind; a failed mint is not kept.
fn chip_bind_once(cache: &mut Option<Value>, mint: impl FnOnce() -> Value) -> (Value, bool) {
    if let Some(br) = cache.as_ref() { return (br.clone(), true); }
    let br = mint();
    if br["success"].as_bool() == Some(true) && br["partsBound"].as_u64().unwrap_or(0) >= 1 && br["parts"][0]["wip_urn"].as_str().map(|u| !u.is_empty()).unwrap_or(false) { *cache = Some(br.clone()); }
    (br, false)
}

/// `fusion_demo {mode:"tour"}`: start the show detached; poll fusion_demo_progress {demoId}.
pub fn start(ctx: &Ctx, args: &Value, env: Env) -> Value {
    if snap()["done"].as_bool() == Some(false) { let s = snap(); return json!({"success": false, "error": "a tour is already running", "demoId": s["demoId"], "_hint": "Poll fusion_demo_progress with that demoId, or fusion_demo_control {action:'exit'} to stop it."}); }
    if snap()["tourDocsClosed"].as_str() == Some("closing") { let s = snap(); return json!({"success": false, "error": "the previous tour is still closing its documents", "demoId": s["demoId"], "_hint": "Wait a few seconds and start again; fusion_demo_progress {demoId} shows tourDocsClosed as a list once it is done."}); }
    let mut idb = [0u8; 4]; let _ = getrandom::getrandom(&mut idb);
    let id = format!("show-{:08x}", u32::from_le_bytes(idb));
    *CALLER.lock().unwrap() = Some(ctx.caller.clone());
    let est: u64 = BEATS.iter().map(|b| b.2).sum();
    *TOUR.lock().unwrap() = Some(json!({"demoId": id, "done": false, "failed": false, "step": "starting", "stepLabel": "Starting the tour", "beatIndex": 0, "beatCount": BEATS.len(), "percent": 0, "elapsedSec": 0, "estimatedSec": est, "steps": [], "shots": [], "presses": [], "captions": [], "audio": [], "events": [], "muted": args["muted"].as_bool().unwrap_or(false), "paused": false, "stopping": false, "panel": {"shown": false}, "started": now()}));
    let (sd, caller, args2, my_id) = (ctx.state_dir.clone(), ctx.caller.clone(), args.clone(), id.clone());
    std::thread::spawn(move || {
        let host = crate::host_for_diagnosis();
        let side = |_v: &str, _a: Value, _t: u64| -> Value { json!({"success": false, "error": "no sidecar"}) };
        let pids: Vec<u32> = host.fusion_processes().iter().map(|p| p.pid).collect();
        let dlg = |r: Value| r;
        let ctx2 = Ctx { host: &*host, fusion_exe: crate::host_for_diagnosis_paths(), short_path: |p| crate::host().short_path(p), state_dir: sd.clone(), caller: caller.clone(), pids, sidecar: &side, dialogs: &dlg };
        let pass2 = |verb: &str, a: Value| crate::addin::pass_through(verb, &a, &ctx2);
        let (sd_n, caller_n) = (sd.clone(), caller.clone());
        let narrator = std::thread::spawn(move || {
            let host = crate::host_for_diagnosis();
            let side = |_v: &str, _a: Value, _t: u64| -> Value { json!({"success": false, "error": "no sidecar"}) };
            let pids: Vec<u32> = host.fusion_processes().iter().map(|p| p.pid).collect();
            let dlg = |r: Value| r;
            let ctx3 = Ctx { host: &*host, fusion_exe: crate::host_for_diagnosis_paths(), short_path: |p| crate::host().short_path(p), state_dir: sd_n, caller: caller_n, pids, sidecar: &side, dialogs: &dlg };
            let pass3 = move |verb: &str, a: Value| crate::addin::pass_through(verb, &a, &ctx3);
            narrator_loop(&pass3);
        });
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_show(&ctx2, &pass2, args2, &env)));
        let _ = narrator.join();
        // only THIS tour's record: a later tour may own the live record by now
        let live = snap();
        if live["demoId"].as_str() == Some(my_id.as_str()) && live["done"].as_bool() != Some(true) { rec(|s| { s["done"] = json!(true); s["failed"] = json!(true); s["error"] = json!("the tour thread ended unexpectedly"); }); panel_hide(); }
        // an Exit marks the record closing before the teardown runs; a thread that died first must not hold the next tour off
        if snap()["demoId"].as_str() == Some(my_id.as_str()) && snap()["tourDocsClosed"].as_str() == Some("closing") { rec(|s| s["tourDocsClosed"] = json!([])); }
    });
    json!({"success": true, "demoId": id, "estimatedSec": est, "steps": BEATS.iter().map(|b| json!({"step": b.0, "label": b.1, "estimatedSec": b.2})).collect::<Vec<_>>(), "statusVerb": "fusion_demo_progress",
        "_hint": format!("Tour started in the background. Poll fusion_demo_progress {{\"demoId\": \"{id}\"}} every 2-3 s: percent, stepLabel, per-step verified/placed/motion, presses[], captions[], panel, and a per-beat evidence frame in shots[]. Drive it with fusion_demo_control {{demoId, action: play|pause|next|prev|exit|mute|unmute}} or the glass remote on screen.")})
}
pub fn owns(id: &str) -> bool { id.starts_with("show-") && snap()["demoId"].as_str() == Some(id) }
pub fn progress(args: &Value) -> Value {
    let s = snap();
    if s.is_null() { return json!({"success": false, "error": "no tour has been started", "_hint": "fusion_demo {mode:'tour'} starts one."}); }
    let mut out = json!({"success": true});
    for (k, v) in s.as_object().unwrap() { if k != "shots" { out[k] = v.clone(); } }
    let all = s["shots"].as_array().cloned().unwrap_or_default();
    out["shotCount"] = json!(all.len());
    let since = args["sinceShot"].as_u64().unwrap_or(0) as usize;
    let imgs = args["imagesAs"].as_str().unwrap_or("base64");
    let shots: Vec<Value> = all.into_iter().skip(since).map(|mut sh| { if imgs == "paths" { sh.as_object_mut().map(|o| o.remove("image")); } sh }).collect();
    out["shots"] = json!(shots);
    out["_hint"] = json!(if s["done"].as_bool() == Some(true) { "Tour finished: steps[] carries verified/placed/motion per beat, presses[] every remote press with its source, shots[] the evidence frames." } else { "Tour running. Poll every 2-3 s; pass sinceShot=shotCount to receive only new frames." });
    out
}
pub fn control(args: &Value) -> Value {
    let a = args["action"].as_str().unwrap_or("").to_lowercase();
    if !matches!(a.as_str(), "play" | "resume" | "pause" | "next" | "prev" | "back" | "exit" | "stop" | "mute" | "unmute") { return json!({"success": false, "error": "action must be play|pause|next|prev|exit|mute|unmute"}); }
    // a repeat Exit while the first one winds the tour down is already handled, not an error
    if matches!(a.as_str(), "exit" | "stop") && snap()["stopping"].as_bool() == Some(true) && snap()["tourDocsClosed"].as_str() == Some("closing") { return json!({"success": true, "action": a, "alreadyStopping": true, "_hint": "The tour is already stopped; its documents are closing."}); }
    if snap()["done"].as_bool() != Some(false) && !matches!(a.as_str(), "mute" | "unmute") { return json!({"success": false, "error": "no running tour", "_hint": "Controls only apply while a tour is running."}); }
    let r = on_press(&a, json!(now()), "dashboard");
    json!({"success": true, "action": a, "result": r, "muted": MUTED.load(Ordering::Relaxed), "paused": snap()["paused"], "_hint": "Honoured at the running beat's next gesture (a document mid-open finishes first); exit stops the tour, silences narration and clears the captions."})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_presses_are_debounced() {
        let t0 = Instant::now();
        let mut q = VecDeque::new();
        assert!(!is_repeat("prev", &q, &[], t0), "a first Back is taken");
        q.push_back("prev".to_string());
        assert!(is_repeat("prev", &q, &[], t0), "a Back while one is still queued is dropped");
        assert!(!is_repeat("next", &q, &[], t0), "another command is not a repeat");
        let q = VecDeque::new();
        let last = vec![("prev".to_string(), t0)];
        assert!(is_repeat("prev", &q, &last, t0 + Duration::from_millis(1500)), "a Back right after one was taken is dropped");
        assert!(!is_repeat("prev", &q, &last, t0 + DEBOUNCE + Duration::from_millis(1)), "a Back after the window is taken");
        assert!(!is_repeat("exit", &q, &last, t0 + Duration::from_millis(100)));
    }

    #[test]
    fn stopped_or_stale_runs_never_show_the_panel() {
        let live = json!({"demoId": "show-1", "done": false, "stopping": false});
        assert!(panel_may_show("show-1", &live, false));
        assert!(!panel_may_show("show-1", &live, true), "hidden or stopped: closed for this run");
        assert!(!panel_may_show("show-0", &live, false), "a late update from an older run is dropped");
        assert!(!panel_may_show("", &live, false));
        assert!(!panel_may_show("show-1", &json!({"demoId": "show-1", "done": true}), false), "a finished run");
        assert!(!panel_may_show("show-1", &json!({"demoId": "show-1", "done": false, "stopping": true}), false), "a run the user exited");
        assert!(!panel_may_show("show-1", &Value::Null, false));
    }

    #[test]
    fn chip_is_minted_once_per_run() {
        let mints = std::cell::Cell::new(0);
        let mint = || { mints.set(mints.get() + 1); json!({"success": true, "partsBound": 1, "parts": [{"wip_urn": format!("urn:vf.{}", mints.get())}]}) };
        let mut cache = None;
        let (a, reused_a) = chip_bind_once(&mut cache, mint);
        let (b, reused_b) = chip_bind_once(&mut cache, || panic!("a replay must not mint again"));
        assert_eq!(mints.get(), 1);
        assert!(!reused_a && reused_b);
        assert_eq!(a["parts"][0]["wip_urn"], b["parts"][0]["wip_urn"], "the replay reuses the first URN");
        // a failed mint is not kept, so the next pass may try again
        let mut cache = None;
        let (f, _) = chip_bind_once(&mut cache, || json!({"success": false, "partsBound": 0, "error": "no cloud"}));
        assert_eq!(f["success"], json!(false));
        assert!(cache.is_none());
    }

    #[test]
    fn failed_beats_are_not_narrated() {
        let ok = json!({"ok": true, "verified": {"ok": true}, "motion": {"gestures": 6, "viewChanged": true, "viewMeasured": true, "aborted": null}});
        assert_eq!(beat_failure(&ok), None);
        let failed = json!({"ok": false, "error": "Could not make AdomDemoSample.lbr the ACTIVE document (active: 'None'), so EDIT was not issued. Retry later."});
        let why = beat_failure(&failed).unwrap();
        assert!(why.starts_with("Could not make AdomDemoSample.lbr") && why.chars().count() <= 80, "{why}");
        assert!(failed_caption(2, 8, "Symbol: the full pinout", &why).contains("2/8: Symbol: the full pinout - this step failed"));
        assert_eq!(beat_failure(&json!({"ok": false})).as_deref(), Some("the step did not complete"));
        assert!(beat_failure(&json!({"ok": true, "verified": {"ok": false}, "motion": {"skipped": "not verified"}})).is_some(), "not on screen");
        // a still view after the motion: retried once, then failed
        let still = json!({"ok": true, "verified": {"ok": true}, "motion": {"gestures": 9, "viewChanged": false, "viewMeasured": true, "aborted": null}});
        assert!(motion_needs_retry(&still["motion"]));
        assert_eq!(beat_failure(&still).as_deref(), Some("the view did not move"));
        // the remote cutting the motion short, or no capture to measure with, is not a failure
        assert!(!motion_needs_retry(&json!({"gestures": 2, "viewChanged": false, "viewMeasured": true, "aborted": "stop requested before orbit-2"})));
        assert!(!motion_needs_retry(&json!({"gestures": 9, "viewChanged": false, "viewMeasured": false, "aborted": null})));
        assert!(!motion_needs_retry(&json!({"gestures": 0, "viewChanged": false, "viewMeasured": true, "aborted": null})));
        assert_eq!(beat_failure(&json!({"ok": true})), None, "the cloud-search beat has no view to check");
    }
}

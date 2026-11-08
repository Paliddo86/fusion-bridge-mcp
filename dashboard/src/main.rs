// fusion-dashboard - the fusion bridge's Launch Page (John's Dashboard spec, 2026-08-16).
// The FIRST container-side bridge binary: installed by the wiki pkg, serves the Dashboard
// web app, and aggregates status by shelling the adom-bridge CLI (the bridge stays the
// single source of truth; this is a thin, honest aggregator).
//
// House contract (adom-ui-design + app-creator), all of it:
//   - served favicon + <link rel=icon> (the REAL Fusion app icon, not an invented mark)
//   - SSE live channel at /events (state pushes, toasts, click directives)
//   - POST /ui/toast from day one - the AI narrates ON TOP of the running app
//   - CLI driving verbs: `ui toast|click|state|console` so the AI drives the real UI
//   - /state + /console observability, OK:/ERROR:/Hint: output, --ai-thread identity
//   - 24h idle reaper so containers never accumulate zombie servers
//
// The HTTP server is hand-rolled on TcpListener: tiny_http buffers a streamed body until
// EOF (verified live - zero bytes reached an SSE client), and SSE NEEDS per-event flushes.
// We control every route and run behind the container proxy, so ~120 lines of HTTP/1.1
// with explicit flush() beats a framework that can't stream.
//
// AI HINTS ARE THE PRODUCT as much as the UI: every JSON response carries _hint fields
// teaching a calling AI what it can automate next.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod demo_audio;
use demo_audio::demo_audio;

const UI_HTML: &str = include_str!("../ui/index.html");
// Web-control quick-actions demo (John, 2026-08-28: "whip up your own demo for this web
// control (quick actions) stuff"). Browser -> 127.0.0.1:2468x -> ab -> bridge, the wiki lane.
const QUICK_ACTIONS_HTML: &str = include_str!("../ui/quick-actions.html");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const IDLE_REAP_HOURS: u64 = 24;

// Tab identity: teal rounded square, two green LEDs - matches the dock icon family.
/// The favicon is the REAL Autodesk Fusion application icon, extracted from the user's own
/// installed Fusion360.exe (SHDefExtractIconW at 256px) - NOT a logo we invented. John, 2026-08-19:
/// "just go with the logo the user is familiar with and not try to reinvent the wheel." The tab,
/// the pinned taskbar tile, and the header brand mark all wear the icon they already know.
const FAVICON_PNG: &[u8] = include_bytes!("../ui/fusion-icon.png");
/// The app mark (icons contract, 2026-08-24): ONE line-drawing SVG for the tab favicon,
/// the header mark, the dock card and the wiki page. Never Autodesk's filled badge.
const FAVICON_SVG: &[u8] = include_bytes!("../../docs/img/dock-fusion.svg");
/// The SHARED newbie panel (adom-ui-design 2.0.0), vendored VERBATIM. Never reformat it:
/// it builds its noun-highlight with RegExp() rather than a literal precisely because a
/// page assembled as a template literal turns \b into a BACKSPACE and eats the word
/// boundaries silently - the regex still matches, just too greedily, so nothing looks
/// broken. Seven apps hand-rolled this panel and produced four paddings, two border
/// colours and three Copy drawings; this file is the fix for that class of drift.
const NEWBIE_PANEL_JS: &[u8] = include_bytes!("../ui/vendor/newbie-panel.js");

fn ok(msg: &str) {
    println!("OK: {}", msg);
}
fn err_hint(msg: &str, hint: &str) {
    eprintln!("ERROR: {}", msg);
    eprintln!("Hint: {}", hint);
}

/// Shell an adom-bridge verb and parse its JSON. --ai-thread attributes every call in the
/// user's Activity Log; forwarded from our own --ai-thread flag (app-creator identity rule).
fn bridge(ai_thread: &str, target: &str, verb: &str, args: &str) -> serde_json::Value {
    let out = Command::new("adom-bridge")
        .args(["--target", target, "--ai-thread", ai_thread, verb, args])
        .output();
    match out {
        Ok(o) => serde_json::from_slice(&o.stdout).unwrap_or_else(|_| {
            serde_json::json!({"error": String::from_utf8_lossy(&o.stdout).chars().take(200).collect::<String>()})
        }),
        Err(e) => serde_json::json!({"error": format!("adom-bridge CLI not runnable: {}", e),
            "_hint": "The container needs the adom-bridge CLI on PATH (it ships with every Adom container)."}),
    }
}

fn unwrap_data(v: &serde_json::Value) -> serde_json::Value {
    // ab envelopes drift across versions - look through data, tolerate flat.
    if let Some(d) = v.get("data") {
        if d.is_object() && d.as_object().map(|m| !m.is_empty()).unwrap_or(false) {
            return d.clone();
        }
    }
    v.clone()
}

fn default_target() -> String {
    // PREFER A BOX THAT IS ACTUALLY DOING SOMETHING (bug found 2026-08-22: the plain
    // dashboard defaulted to targets[0], which was a box with no APS token, so the
    // identity chips read empty while AdomLapper was signed in). Rank connected targets
    // by a cheap per-box readiness probe: ready > running > signed-into-APS > first seen.
    // Only ever a fallback - the frontend passes ?target= once it knows the roster.
    let out = match Command::new("adom-bridge").arg("targets").output() {
        Ok(o) => o.stdout,
        Err(_) => return String::new(),
    };
    let v: serde_json::Value = match serde_json::from_slice(&out) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    let targets: Vec<String> = v["targets"].as_array().map(|a| {
        a.iter().filter_map(|t| t["name"].as_str().map(String::from)).collect()
    }).unwrap_or_default();
    if targets.is_empty() { return String::new(); }
    if targets.len() == 1 { return targets[0].clone(); }
    let mut best: Option<(u8, String)> = None;   // (rank, name); higher rank wins
    for name in &targets {
        let rd = unwrap_data(&bridge("fusion-dashboard", name, "fusion_readiness", "{}"));
        // APS signed-in is what the identity chips read, so it breaks the common tie where
        // no box is running Fusion but ONE is signed into APS (the 2026-08-22 bug).
        let aps = unwrap_data(&bridge("fusion-dashboard", name, "fusion_aps_status", "{}"));
        let rank = if rd["ready"].as_bool().unwrap_or(false) { 4u8 }
                   else if rd["running"].as_bool().unwrap_or(false) { 3u8 }
                   else if aps["signedIn"].as_bool().unwrap_or(false) { 2u8 }
                   else { 1u8 };
        if best.as_ref().map(|(r, _)| rank > *r).unwrap_or(true) {
            best = Some((rank, name.clone()));
        }
    }
    best.map(|(_, n)| n).unwrap_or_else(|| targets[0].clone())
}

/// The LED payload: one call per facet, each with state + an AI hint on how to act.
fn gather_status(ai_thread: &str, target: &str) -> serde_json::Value {
    gather_status_ex(ai_thread, target, false)
}

fn gather_status_ex(ai_thread: &str, target: &str, launching: bool) -> serde_json::Value {
    // PARALLEL, not sequential: each verb is a relay round-trip, and six of them in series took
    // 36.7s against a remote box (measured live 2026-08-18) - the Dashboard sat blank that whole
    // time. Fan them out and the wall clock is the slowest single call.
    let verbs = ["fusion_readiness", "fusion_aps_status", "fusion_mcp_status",
                 "fusion_set_auto_update", "fusion_set_seat_takeover", "fusion_set_driving_badge",
                 "fusion_set_activity_indicator"];
    let handles: Vec<_> = verbs.iter().map(|v| {
        let (ai, t, verb) = (ai_thread.to_string(), target.to_string(), v.to_string());
        std::thread::spawn(move || unwrap_data(&bridge(&ai, &t, &verb, "{}")))
    }).collect();
    let mut res: Vec<serde_json::Value> = Vec::with_capacity(verbs.len());
    for h in handles {
        res.push(h.join().unwrap_or(serde_json::json!({})));
    }
    let (rd, aps, mcp) = (res[0].clone(), res[1].clone(), res[2].clone());
    let (auto_upd, seat, badge) = (res[3].clone(), res[4].clone(), res[5].clone());
    let activity = res.get(6).cloned().unwrap_or(serde_json::json!({}));

    let running = rd["fusionRunning"].as_bool().or(rd["running"].as_bool()).unwrap_or(false);
    let ready = rd["ready"].as_bool().unwrap_or(false);
    let needs_signin = rd["needsSignin"].as_bool().unwrap_or(false);
    let addin = rd["addinVersion"].as_str().unwrap_or("").to_string();
    // Configured-but-dormant beats broken-red (John, 2026-08-17): while Fusion is closed,
    // sign-in/add-in/MCP are UNKNOWABLE - paint them amber with the last verified time,
    // never red. Red is reserved for "actually needs setup".
    let last_signed = rd["lastSignedInAt"].as_str().unwrap_or("");
    let last_ready = rd["lastReadyAt"].as_str().unwrap_or("");
    // Is lastReadyAt older than 2h? Parsed leniently from "YYYY-MM-DDTHH:MM:SS..." vs now;
    // unparseable timestamps count as stale (never let garbage read as freshly busy).
    let last_ready_stale = !last_ready.is_empty() && {
        use std::time::{SystemTime, UNIX_EPOCH};
        parse_epoch_lenient(last_ready).map(|t| {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
            now - t > 15 * 60      // status-leds contract: busy past 15 min reads "stuck?"
        }).unwrap_or(true)
    };
    let mcp_last = mcp["lastEnabledAt"].as_str().unwrap_or("");
    // WHO is signed in, per facet (John, 2026-08-22): the auth test matrix runs
    // several accounts, and an anonymous green LED cannot distinguish them.
    let aps_who = aps["signedInAs"]["email"].as_str()
        .or(aps["signedInAs"]["name"].as_str()).unwrap_or("").to_string();
    let fusion_who = rd["fusionUser"]["user"].as_str().unwrap_or("").to_string();
    let aps_ok = aps["signedIn"].as_bool().or(aps["configured"].as_bool()).unwrap_or(false)
        || aps["ready"].as_bool().unwrap_or(false);
    let mcp_on = mcp["enabled"].as_bool().unwrap_or(false);
    let upd_offered = rd["fusionUpdateOffered"].as_bool().unwrap_or(false);
    let upd_handled = rd["fusionUpdateHandled"].as_bool().unwrap_or(false);

    // Did the bridge actually answer? A busy bridge returns an error envelope with no version
    // and no running flag - that is UNKNOWN, not "everything off".
    let answered = rd.get("bridgeVersion").map(|v| !v.is_null()).unwrap_or(false)
        || rd.get("running").is_some() || rd.get("installed").is_some();
    // TOP-LEVEL ROLLUP (dock_manifest 1.2.0, issue #37 checklist item 2): our own verdict
    // of which LEDs deserve to change the card, so the dock never falls back to its blunt
    // worst-facet rule. Fusion being CLOSED is a normal state for a control panel - the
    // card stays green. Only two things are worth a yellow: the bridge not answering
    // (transient - busy or relay hiccup), and Fusion up with the add-in neither serving
    // nor busy (self-healable, so warn, not fail). update/aps/mcp never move the card.
    let rollup = if !answered { "warn" }
                 else if running && !ready && last_ready.is_empty() { "warn" }
                 else { "ok" };
    serde_json::json!({
        "state": rollup,
        "target": target,
        "answered": answered,
        "busyReason": if answered { serde_json::Value::Null }
                      else { serde_json::json!(rd["error"].as_str().unwrap_or("the bridge did not answer (busy running a long verb?)")) },
        "dashboardVersion": VERSION,
        "bridgeVersion": rd["bridgeVersion"],
        "leds": {
            "fusionRunning": {"on": running, "label": "Fusion running",
                // A launch takes ~30-90s. Saying nothing during it (or showing red) leaves the
                // user wondering whether anything is happening at all - so say it plainly.
                "state": if running {"on"} else if launching {"starting"} else {"off"},
                "_hint": if running {"Fusion is up.".to_string()}
                         else if launching {"Opening Fusion right now - launching in the background and waiting for its bridge add-in. This usually takes under a minute.".to_string()}
                         else {"fusion_start launches it in the BACKGROUND (never steals focus).".to_string()}},
            "addinReady": {"on": ready, "label": "Bridge add-in", "version": addin,
                // BUSY is not BROKEN (ralph, 2026-08-18): while a long verb runs - a tour, a 3D
                // build - readiness answers not-ready because the add-in's main thread is
                // occupied. Painting that red said "not serving" while the tour was visibly
                // driving Fusion. Amber + an honest sentence instead.
                "state": if ready {"on"}
                         else if running && !last_ready.is_empty() && !last_ready_stale {"busy"}
                         else if running && !last_ready.is_empty() {"stale"}
                         else if !running && !last_ready.is_empty() {"dormant"}
                         else {"off"},
                "_hint": if ready {"Add-in serving - all fusion_* verbs live.".to_string()}
                         // STALENESS CUTOFF (project-a6 cold-verify flag, 2026-08-24): "busy with a
                         // long verb" citing a DAY-old lastReadyAt is a lie - no verb runs 2h+. Past
                         // the cutoff say what it actually is: the add-in has not answered since
                         // that timestamp and a Fusion restart reloads it (which also picks up a
                         // newer installed add-in version).
                         else if running && !last_ready.is_empty() && last_ready_stale {format!("Stuck? Not serving since {} - a long verb never runs this long. Bounce it: fusion_stop then fusion_start (that also loads any newer installed add-in).", last_ready)}
                         else if running && !last_ready.is_empty() {format!("Busy or still starting - Fusion is up and the add-in was serving at {}. A long verb (a tour, a 3D build) occupies its main thread, so readiness answers not-ready until it finishes.", last_ready)}
                         else if !running && !last_ready.is_empty() {format!("Dormant - resumes when Fusion starts (last verified {}).", last_ready)}
                         else {"Not serving. If needsSignin, run fusion_signin - it self-drives to the password step.".to_string()}},
            "signedIn": {"on": running && !needs_signin, "label": "Autodesk sign-in",
                "who": if fusion_who.is_empty() { serde_json::Value::Null } else { serde_json::json!(fusion_who) },
                "state": if running && !needs_signin {"on"} else if !running && !last_signed.is_empty() {"dormant"} else {"off"},
                "_hint": if needs_signin {"fusion_signin {email} self-drives the whole wall: launches, clicks, types the email, toasts the user ONLY at their password.".to_string()}
                         else if !running && !last_signed.is_empty() {format!("Signed in (verified {}). Unknowable while Fusion is closed - dormant, not broken.", last_signed)}
                         else if running {"Signed in.".to_string()} else {"Unknown until Fusion runs.".to_string()}},
            "aps": {"on": aps_ok, "label": "APS cloud search",
                "who": if aps_who.is_empty() { serde_json::Value::Null } else { serde_json::json!(aps_who) },
                "_hint": if aps_ok {"fusion_aps_search gives ~2s cloud lookups."} else {"Not configured: fusion_aps_set_client_id then fusion_aps_signin (silent when the browser session is warm)."}},
            "mcp": {"on": mcp_on, "label": "Fusion MCP server",
                "state": if mcp_on {"on"} else if !running && !mcp_last.is_empty() {"dormant"} else {"off"},
                "_hint": if mcp_on {"fusion_mcp_call drives Autodesk's own MCP tools.".to_string()}
                         else if !running && !mcp_last.is_empty() {format!("Enabled in Fusion's preferences (verified {}) - resumes when Fusion starts.", mcp_last)}
                         else {"fusion_mcp_enable turns it on (~10s, drives the Preferences dialog with a caption).".to_string()}},
            "datahub": {"on": aps_ok, "label": "Data Hub API",
                "who": if aps_who.is_empty() { serde_json::Value::Null } else { serde_json::json!(aps_who) },
                "_hint": "Data Hub rides the APS auth: file listings, versions, urn resolution via fusion_aps_get."},
            "update": {"on": !upd_offered || upd_handled, "label": "Fusion up to date",
                "pending": upd_offered && !upd_handled,
                "_hint": if upd_offered && !upd_handled {"An update is WAITING. Auto-update normally clicks it silently; offer to run it now."} else {"No pending update (auto-update clicks nags silently by default; sticky opt-out via fusion_set_auto_update)."}},
        },
        "settings": {
            "autoUpdateFusion": auto_upd["autoUpdateFusion"].as_bool().unwrap_or(true),
            "autoTakeOverSeat": seat["autoTakeOverSeat"].as_bool().unwrap_or(true),
            "autoGreenOnLaunch": read_settings()["autoGreenOnLaunch"].as_bool().unwrap_or(true),
            "showDrivingBadge": badge["showDrivingBadge"].as_bool().unwrap_or(true),
            "activityIndicator": activity["activityIndicator"].as_str().unwrap_or("progress")
        },
        "demoPart": read_settings()["demoPart"],
        "_hint": "This is the Dashboard status payload. Every LED carries its own _hint naming the verb that fixes it. POST /api/action {action, args} to act; GET /api/status to re-read; subscribe to /events (SSE) for live pushes; POST /ui/toast {message,kind} to narrate on top of the app."
    })
}

fn action(ai_thread: &str, target: &str, name: &str, args: &serde_json::Value) -> serde_json::Value {
    let a = args.to_string();
    match name {
        "start" => bridge(ai_thread, target, "fusion_start", "{}"),
        "stop" => {
            let r = unwrap_data(&bridge(ai_thread, target, "fusion_stop",
                                        "{\"reason\":\"dashboard: user clicked Close Fusion\"}"));
            if r["success"].as_bool() == Some(true) { return r; }
            // DOCTRINE: when the ONLY things blocking the close are the bridge's own throwaway
            // test docs, escalate for the user instead of handing them a failure to resolve.
            if r["bridgeDocsOnly"].as_bool() == Some(true) {
                let _k = unwrap_data(&bridge(ai_thread, target, "fusion_kill", "{}"));
                // VERIFY the outcome, do not trust the verb's flag: fusion_kill reports
                // success:false in shapes where Fusion did in fact close (ralph, 2026-08-18).
                std::thread::sleep(Duration::from_secs(4));
                let gone = !unwrap_data(&bridge(ai_thread, target, "fusion_readiness", "{}"))["running"]
                    .as_bool().unwrap_or(false);
                return serde_json::json!({"success": gone,
                    "escalated": true, "gracefulError": r["error"].clone(),
                    "_hint": "The graceful close was blocked only by the bridge's own generated test documents, so it was force-closed for you - nothing of yours was at risk."});
            }
            r
        }
        "signin" => bridge(ai_thread, target, "fusion_signin", &a),
        "tour" => {
            // THE REMOTE IS AB'S GLASS PANEL NOW (John, 2026-09-01: "ab's new demo control
            // panel that has play/pause/next/prev and is a floaty glass like interface").
            // remote:true makes the tour show desktop_demo_panel itself and drain its
            // presses onto the same transport flags this page's buttons set, so both work
            // mid-tour. The earlier pup floaty (a 320x420 window of this page's ?remote view)
            // is retired: two remotes on screen confused the story, and the glass panel
            // never steals focus. preset:electronics is the eight-stop showcase: library in,
            // symbol, footprint, 3D chip, ganged view, schematic, 2D board, 3D board.
            bridge(ai_thread, target, "fusion_demo",
                   "{\"mode\":\"tour\",\"preset\":\"electronics\",\"remote\":true}")
        }
        "tour_control" => bridge(ai_thread, target, "fusion_demo_control", &a),
        "tour_progress" => bridge(ai_thread, target, "fusion_demo_progress", &a),
        "set_auto_update" => bridge(ai_thread, target, "fusion_set_auto_update", &a),
        "set_seat_takeover" => bridge(ai_thread, target, "fusion_set_seat_takeover", &a),
        "set_driving_badge" => bridge(ai_thread, target, "fusion_set_driving_badge", &a),
        "mcp_enable" => bridge(ai_thread, target, "fusion_mcp_enable", "{}"),
        "aps_signin" => bridge(ai_thread, target, "fusion_aps_signin", "{}"),
        "import_basic_part" => import_basic_part(ai_thread, target, args),   // readiness handled by the caller
        _ => serde_json::json!({"error": format!("unknown action {}", name),
            "_hint": "Actions: start, stop, signin, tour, tour_progress, set_auto_update, set_seat_takeover, mcp_enable, aps_signin, import_basic_part."}),
    }
}

/// The wiki-to-Fusion demo, end to end from the container: clone the part page,
/// send its .lbr to the box, open it in Fusion's Library Manager. All background.
fn import_basic_part(ai_thread: &str, target: &str, args: &serde_json::Value) -> serde_json::Value {
    let page = args["page"].as_str().unwrap_or("aravk/in-s42ata");
    let tmp = format!("/tmp/dash-part-{}", std::process::id());
    let _ = std::fs::remove_dir_all(&tmp);
    let clone = Command::new("adom-wiki")
        .args(["repo", "clone", page, "--dir", &tmp]).output();
    if clone.is_err() {
        return serde_json::json!({"error": "adom-wiki CLI missing",
            "_hint": "The container ships adom-wiki; check PATH."});
    }
    let lbr = std::fs::read_dir(&tmp).ok().and_then(|rd| {
        rd.filter_map(|e| e.ok()).map(|e| e.path())
            .find(|p| p.extension().map(|x| x == "lbr").unwrap_or(false))
    });
    let Some(lbr) = lbr else {
        return serde_json::json!({"error": format!("{} has no .lbr file", page),
            "_hint": "Pick an adom basic part page (it publishes <PART>.lbr + <PART>.step)."});
    };
    let dest = "C:/Users/john/AppData/Local/Temp/fusion-dashboard";
    let send = bridge(ai_thread, target, "send_files",
        &serde_json::json!({"filePaths": [lbr.to_string_lossy()], "dest": dest}).to_string());
    let name = lbr.file_name().unwrap().to_string_lossy().to_string();
    // BUSY-RETRY (ralph, 2026-08-18): the add-in serializes on its main thread, so an open
    // issued while another session's long verb is running is REJECTED outright. Waiting is the
    // whole fix - never hand that back to the user as a failure.
    let mut open = bridge(ai_thread, target, "fusion_open_lbr",
        &serde_json::json!({"filePath": format!("{}/{}", dest, name)}).to_string());
    for _ in 0..10 {
        let o = unwrap_data(&open);
        let err = o["error"].as_str().unwrap_or("").to_lowercase();
        let busy = o["errorCode"].as_str() == Some("main_thread_busy")
            || err.contains("main thread busy") || err.contains("in progress from another");
        if !busy { break; }
        std::thread::sleep(Duration::from_secs(15));
        open = bridge(ai_thread, target, "fusion_open_lbr",
            &serde_json::json!({"filePath": format!("{}/{}", dest, name)}).to_string());
    }
    serde_json::json!({"page": page, "sent": unwrap_data(&send), "opened": unwrap_data(&open),
        "_hint": "Wiki part -> Fusion Library Manager, all background. Next steps an AI can run: fusion_show_symbol / fusion_show_footprint {name}, fusion_bind_library_3d for the 3D, fusion_make_test_project for a solo demo board. Render any of them with fusion_state."})
}

// ── SSE hub: the live channel every Adom app carries ─────────────────────────────────────
struct Hub {
    clients: Mutex<Vec<Sender<String>>>,
}
impl Hub {
    fn broadcast(&self, event: &str, data: &serde_json::Value) {
        let msg = format!("event: {}\ndata: {}\n\n", event, data);
        self.clients.lock().unwrap().retain(|c| c.send(msg.clone()).is_ok());
    }
    fn client_count(&self) -> usize {
        // Prune dead senders lazily so the count reflects live viewers.
        let mut g = self.clients.lock().unwrap();
        g.retain(|c| c.send(": ping\n\n".into()).is_ok());
        g.len()
    }
}

struct App {
    ai_thread: String,
    target: String,
    hub: Hub,
    console: Mutex<Vec<String>>,
    last_activity: Mutex<Instant>,
    // Auto-green: John's ruling 2026-08-18 - "the meaning of opening the dashboard is that you
    // open fusion and all the other services", with a sticky opt-out toggle.
    // PER TARGET (ralph, 2026-08-18): these were single globals, so an auto-green for one box
    // silently blocked a run for the other and the endpoint still answered "started" - exactly
    // wrong now that multi-box is first-class.
    auto_green_running: Mutex<std::collections::HashSet<String>>,
    auto_green_last: Mutex<std::collections::HashMap<String, Instant>>,
    // Surfaces demo staging, keyed BY TARGET: staging sends files to one specific box, so a
    // single global slot made a switch to another box reuse paths that box never received
    // (caught live 2026-08-18).
    // (libraryPath, deviceset, designReady) - designReady false means the generated test
    // project did NOT land, so the four design surfaces must regenerate it before they run.
    staged: Mutex<std::collections::HashMap<String, (String, String, bool)>>,
    // Last GOOD status per target. While the bridge is busy (a tour, a long verb), readiness
    // calls time out and every field reads false - which used to paint the whole board RED
    // while the tour was visibly running (ralph, 2026-08-18). Unknowable is not broken.
    last_good: Mutex<std::collections::HashMap<String, (Instant, serde_json::Value)>>,
    // Boxes we are actively launching Fusion on, so the LED can say "opening right now"
    // instead of leaving the user guessing (John, 2026-08-18).
    launching: Mutex<std::collections::HashMap<String, Instant>>,
    // Why the last launch attempt ended, so a failed launch explains itself on the LED instead
    // of silently reverting to a bare red "off" (ralph, 2026-08-18).
    last_launch_note: Mutex<std::collections::HashMap<String, String>>,
}

// ── Dashboard-own settings (container-side; bridge prefs stay in the bridge) ──────────────
fn settings_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let dir = std::path::Path::new(&home).join(".adom").join("fusion-dashboard");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("settings.json")
}

fn read_settings() -> serde_json::Value {
    let mut v = std::fs::read_to_string(settings_path()).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if v.get("autoGreenOnLaunch").is_none() {
        v["autoGreenOnLaunch"] = serde_json::Value::Bool(true);   // ON by default, per John
    }
    if v.get("demoPart").is_none() {
        // Deliberately NOT web control's IN-S42ATR: a different adom basic part (green 0603 vs
        // red 0402) proves the flow generalizes instead of re-proving one file.
        v["demoPart"] = serde_json::Value::String("aravk/in-s63btg".into());
    }
    v
}

fn write_settings(v: &serde_json::Value) {
    let _ = std::fs::write(settings_path(), serde_json::to_string_pretty(v).unwrap_or_default());
}
impl App {
    fn log(&self, m: String) {
        let mut g = self.console.lock().unwrap();
        g.push(m);
        let overflow = g.len().saturating_sub(500);
        if overflow > 0 {
            g.drain(0..overflow);
        }
    }
    fn tgt(&self) -> String {
        if self.target.is_empty() { default_target() } else { self.target.clone() }
    }
}


// ── AUTO-GREEN (John, 2026-08-18) ────────────────────────────────────────────────────────
// "when i launch the dashboard, the default behavior should be to get all those led's green,
//  meaning if fusion isn't launched, you literally launch it ... but i need a toggle to make
//  this not be the default, and if i turn it off you just show led status and don't act."
//
// So: opening the Dashboard = an INTENT to work, not just to look. The sequence does every
// step it can do WITHOUT the human, narrates each one as a toast on the live channel, and
// stops honestly at any wall that genuinely needs a person (a password, an unconfigured APS
// client id) rather than pretending. All of it background - never steals focus.
/// Is Fusion running on any OTHER bridge target than `here`? Returns that box's name.
/// Used by auto-green's seat courtesy - a page-load must never steal the one Autodesk
/// seat from a box that is actively using it (#50 regression: killed a build matrix).
fn fusion_running_elsewhere(ai: &str, here: &str) -> Option<String> {
    let out = Command::new("adom-bridge").arg("targets").output().ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    for t in v["targets"].as_array()? {
        let name = t["name"].as_str().unwrap_or("");
        if name.is_empty() || name.eq_ignore_ascii_case(here) { continue; }
        let rd = unwrap_data(&bridge(ai, name, "fusion_readiness", "{}"));
        if rd["running"].as_bool() == Some(true) {
            return Some(name.to_string());
        }
    }
    None
}

fn auto_green_forced(app: &Arc<App>, target: &str, force: bool) -> bool {
    {   // single-flight + 3-minute debounce, PER TARGET: several tabs/reloads must not stack
        // launches on the SAME box, but a run for one box must never block another.
        let mut running = app.auto_green_running.lock().unwrap();
        if running.contains(target) { return false; }      // a run is genuinely in flight
        if !force {
            if let Some(t) = app.auto_green_last.lock().unwrap().get(target) {
                if t.elapsed() < Duration::from_secs(180) { return false; }
            }
        }
        running.insert(target.to_string());
    }
    // Mark "launching" SYNCHRONOUSLY, before the worker thread starts: the endpoint returns
    // immediately and the page's very next status must already say "opening Fusion", not "off"
    // (ralph, 2026-08-18 - a 3-second window where the user saw a red LED after clicking).
    {
        let ai0 = app.ai_thread.clone();
        let t0 = target.to_string();
        let running_now = unwrap_data(&bridge(&ai0, &t0, "fusion_readiness", "{}"))["running"]
            .as_bool().unwrap_or(false);
        if !running_now {
            app.launching.lock().unwrap().insert(t0.clone(), Instant::now());
            app.last_launch_note.lock().unwrap().remove(&t0);
        }
    }
    let t = target.to_string();
    let ai = app.ai_thread.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        let toast = |msg: &str, kind: &str| {
            app2.hub.broadcast("toast", &serde_json::json!({"message": msg, "kind": kind}));
            app2.log(format!("auto-green: {}", msg));
        };
        let push_state = || {
            let st = gather_status(&ai, &t);
            app2.hub.broadcast("state", &st);
            st
        };
        let mut st = push_state();
        let led = |st: &serde_json::Value, k: &str| st["leds"][k]["on"].as_bool().unwrap_or(false);

        if led(&st, "fusionRunning") && led(&st, "addinReady") && led(&st, "aps") && led(&st, "mcp") {
            toast("Everything is already green on this box", "ok");
        } else {
            toast(&format!("Getting {} ready - launching Fusion and its services in the background", t), "");

            // 1. Fusion itself.
            // ⚠ SEAT COURTESY (#50 regression, 2026-08-23): one Autodesk seat, many boxes.
            // An UNATTENDED auto-green that launches Fusion while the SAME account's Fusion
            // is live on ANOTHER box makes Autodesk suspend that other Fusion - which killed
            // a 20-part build matrix mid-prove on AdomLapper when a dock test opened this
            // dashboard on the conference box. Auto-green never steals the seat: if Fusion
            // runs elsewhere, report it and stand down. A DELIBERATE Start Fusion click
            // still takes the seat (John's "the seat follows automatically" model is for
            // intentional starts, not page-loads).
            if !led(&st, "fusionRunning") {
                if let Some(other) = fusion_running_elsewhere(&ai, &t) {
                    let why = format!(
                        "Fusion is live on {} right now - starting it here would take the one \
                         Autodesk seat and suspend that box mid-work. Press Start Fusion to \
                         take the seat deliberately.", other);
                    toast(&why, "");
                    app2.last_launch_note.lock().unwrap().insert(t.clone(), why);
                    app2.auto_green_last.lock().unwrap().insert(t.clone(), Instant::now());
                    app2.auto_green_running.lock().unwrap().remove(&t);
                    push_state();
                    return;
                }
            }
            if !led(&st, "fusionRunning") {
                app2.launching.lock().unwrap().insert(t.clone(), Instant::now());
                app2.hub.broadcast("state", &gather_status_ex(&ai, &t, true));
                bridge(&ai, &t, "fusion_start", "{}");
                for _ in 0..30 {
                    std::thread::sleep(Duration::from_secs(10));
                    st = push_state();
                    if led(&st, "addinReady") { break; }
                    // A sign-in wall shows up as running-but-never-ready: handle it below.
                    if led(&st, "fusionRunning") && st["leds"]["signedIn"]["on"].as_bool() == Some(false)
                        && st["leds"]["signedIn"]["state"].as_str() == Some("off") { break; }
                }
                app2.launching.lock().unwrap().remove(&t);
                if led(&st, "addinReady") {
                    app2.last_launch_note.lock().unwrap().remove(&t);
                    toast("Fusion is up and the bridge add-in is serving", "ok");
                } else {
                    let why = if led(&st, "fusionRunning") {
                        "Fusion launched but its bridge add-in never came up - press Start Fusion to retry, or check for a sign-in wall."
                    } else {
                        "The launch attempt did not take (Autodesk seat busy elsewhere, or a licensing dialog). Press Start Fusion to try again."
                    };
                    app2.last_launch_note.lock().unwrap().insert(t.clone(), why.to_string());
                    toast(why, "err");
                }
            }

            // 2. Fusion is up but the add-in is not serving. TWO causes, both fixable without
            //    the human: a sign-in wall (drive it), or an add-in that simply did not load
            //    (restart Fusion - it only loads at launch). John's rule: detect and fix, then
            //    continue. Previously this branch only tried sign-in, so a stuck add-in sat red
            //    on the Dashboard forever (caught by the ralph loop, 2026-08-18).
            st = push_state();
            if led(&st, "fusionRunning") && !led(&st, "addinReady") {
                let si = unwrap_data(&bridge(&ai, &t, "fusion_signin", "{}"));
                if si["signinWall"].is_string() {
                    toast("Fusion needs your Autodesk password - I drove the sign-in to that step for you", "err");
                } else {
                    toast("Fusion is up but its add-in is not serving - healing that now", "");
                    wait_ready_with_heal(&app2, &t, 240);
                    st = push_state();
                    if led(&st, "addinReady") { toast("Bridge add-in is serving", "ok"); }
                }
            }

            // 3. Autodesk's MCP server (a Fusion preference; needs Fusion up).
            st = push_state();
            if led(&st, "addinReady") && !led(&st, "mcp") {
                toast("Turning on the Fusion MCP server", "");
                bridge(&ai, &t, "fusion_mcp_enable", "{}");
                st = push_state();
                if led(&st, "mcp") { toast("Fusion MCP server enabled", "ok"); }
            }

            // 4. APS cloud search. Signing in is only attempted when a client id is already
            //    configured (the warm case that can complete silently); an unconfigured APS is
            //    a real setup task for the human, so we say so instead of opening browsers.
            st = push_state();
            if !led(&st, "aps") {
                let aps = unwrap_data(&bridge(&ai, &t, "fusion_aps_status", "{}"));
                if aps["configured"].as_bool().unwrap_or(false) {
                    toast("Signing in to APS cloud search", "");
                    bridge(&ai, &t, "fusion_aps_signin", "{}");
                    st = push_state();
                    if led(&st, "aps") { toast("APS cloud search is live", "ok"); }
                } else {
                    toast("APS cloud search needs a one-time setup (client id) - use Setup when you want it", "err");
                }
            }
        }

        let st = push_state();
        let reds: Vec<String> = st["leds"].as_object().map(|o| o.iter()
            .filter(|(_, v)| !v["on"].as_bool().unwrap_or(false) && v["state"].as_str() != Some("dormant"))
            .map(|(k, _)| k.clone()).collect()).unwrap_or_default();
        if reds.is_empty() { toast("All green - ready to go", "ok"); }
        else { toast(&format!("Ready except: {} (see each card for the exact next step)", reds.join(", ")), ""); }

        app2.auto_green_last.lock().unwrap().insert(t.clone(), Instant::now());
        app2.auto_green_running.lock().unwrap().remove(&t);
    });
    true
}

/// Make the box ready, do not lecture the user about it (John, 2026-08-18: "when i load the
/// dashboard you're supposed to get everything ready for me ... launching the dashboard is like
/// opening fusion and making sure its fully setup and ready for the ai to control it").
/// Any action that needs Fusion calls this FIRST: it returns immediately when ready, otherwise
/// it kicks auto-green (single-flight, so an in-flight launch is joined rather than duplicated)
/// and waits, narrating progress, before the caller proceeds.
/// Wait for readiness while HEALING the fixable states. Does NOT call auto_green (auto_green
/// calls this, so putting the kick here would recurse).
fn wait_ready_with_heal(app: &Arc<App>, target: &str, max_wait_s: u64) -> bool {
    let ai = app.ai_thread.clone();
    let is_ready = || unwrap_data(&bridge(&ai, target, "fusion_readiness", "{}"))["ready"]
        .as_bool().unwrap_or(false);
    let t0 = Instant::now();
    let mut announced = false;
    let mut restarted = false;
    let mut last_start = Instant::now() - Duration::from_secs(120);   // allow an immediate launch
    while t0.elapsed() < Duration::from_secs(max_wait_s) {
        std::thread::sleep(Duration::from_secs(5));
        // SELF-HEAL rather than wait on a broken state (John, 2026-08-18: "even if something is
        // off, you should be detecting that and then auto launching all the needed pieces ...
        // never make the adom user do anything manually"). Two mechanically fixable states:
        //   a) a modal is blocking the add-in -> clear it;
        //   b) Fusion is up but the add-in never came up -> restart it ONCE (the add-in loads at
        //      launch, so stop+start is the actual remedy; more waiting never fixes it).
        // THE LAUNCH ITSELF: never assume someone else is starting Fusion. auto_green may be
        // debounced (a run finished in the last 3 minutes), in which case waiting here would
        // wait forever on a launch nobody performs - which is exactly how the surfaces started
        // failing with "installed but not running" (ralph, 2026-08-18). fusion_start is
        // idempotent, so issuing it directly is safe; throttled to one attempt per 75s.
        {
            let running = unwrap_data(&bridge(&ai, target, "fusion_readiness", "{}"))["running"]
                .as_bool().unwrap_or(false);
            if !running && last_start.elapsed() > Duration::from_secs(75) {
                last_start = Instant::now();
                app.launching.lock().unwrap().insert(target.to_string(), Instant::now());
                app.hub.broadcast("toast", &serde_json::json!({
                    "message": format!("Opening Fusion on {}", target), "kind": ""}));
                app.log(format!("wait_ready_with_heal: launching Fusion on {}", target));
                bridge(&ai, target, "fusion_start", "{}");
            }
        }
        if t0.elapsed() > Duration::from_secs(40) && !is_ready() {
            let st = unwrap_data(&bridge(&ai, target, "fusion_state", "{\"screenshot\":false}"));
            if st["blockingDialogs"].as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                app.log("ensure_ready: clearing blocking dialogs".into());
                bridge(&ai, target, "fusion_dismiss_blocking_dialogs", "{}");
            }
            if !restarted && t0.elapsed() > Duration::from_secs(90)
                && st["running"].as_bool().unwrap_or(false)
                && !st["addinResponding"].as_bool().unwrap_or(false)
                // a loaded add-in that is busy is not "did not start": never restart over it
                && st["addinBusy"].is_null() {
                restarted = true;
                app.hub.broadcast("toast", &serde_json::json!({
                    "message": "Fusion is up but its bridge add-in did not start - restarting Fusion for you", "kind": ""}));
                app.log("ensure_ready: add-in not serving, restarting Fusion".into());
                bridge(&ai, target, "fusion_stop", "{\"reason\":\"dashboard: add-in not serving, restarting to load it\"}");
                std::thread::sleep(Duration::from_secs(4));
                bridge(&ai, target, "fusion_kill", "{}");
                std::thread::sleep(Duration::from_secs(3));
                app.launching.lock().unwrap().insert(target.to_string(), Instant::now());
                bridge(&ai, target, "fusion_start", "{}");
            }
        }
        if is_ready() {
            app.launching.lock().unwrap().remove(target);
            app.hub.broadcast("toast", &serde_json::json!({"message": "Fusion is ready - continuing", "kind": "ok"}));
            app.hub.broadcast("state", &gather_status(&ai, target));
            return true;
        }
        // MEASURED, NEVER A SHRUG (page issue #32, John 2026-08-19): the bridge now returns a
        // `progress` block seeded from THIS machine's rolling launch history, so the wait is
        // reported with a real percent + ETA instead of "a minute or two". Pushed on the live
        // channel every tick so the LED and its caption narrow as the launch proceeds.
        let rd = unwrap_data(&bridge(&ai, target, "fusion_readiness", "{}"));
        if let Some(p) = rd.get("progress").filter(|p| !p.is_null()) {
            app.hub.broadcast("progress", &serde_json::json!({
                "target": target, "phase": p["phase"].clone(), "percent": p["percent"].clone(),
                "etaSec": p["etaSec"].clone(), "elapsedSec": p["elapsedSec"].clone(),
                "stepLabel": p["stepLabel"].clone(), "confidence": p["confidence"].clone(),
                "startKind": p["startKind"].clone()}));
            if !announced {
                announced = true;
                let eta = p["etaSec"].as_i64();
                app.hub.broadcast("toast", &serde_json::json!({
                    "message": match eta {
                        Some(s) => format!("{} - about {}s left ({})",
                                           p["stepLabel"].as_str().unwrap_or("Fusion is starting"), s,
                                           p["confidence"].as_str().unwrap_or("typical")),
                        None => format!("{}", p["stepLabel"].as_str().unwrap_or("Fusion is starting")),
                    }, "kind": ""}));
            }
        }
    }
    false
}

fn auto_green(app: &Arc<App>, target: &str) -> bool { auto_green_forced(app, target, false) }

/// The doctrine entry point: make the box ready, never tell the user to do it.
/// How long to wait for a launch on THIS box, from the bridge's own MEASURED history
/// (page issue #32) rather than a literal. p90 with generous headroom, because giving up
/// early on a genuinely-slow-but-healthy cold start hands the user a failure the machine
/// was about to resolve (ralph, 2026-08-19: a 4-minute cap failed a box that needed more).
/// Falls back to `floor` when this machine has no history yet.
fn ready_budget(ai: &str, target: &str, floor_s: u64) -> u64 {
    let st = unwrap_data(&bridge(ai, target, "fusion_state", "{\"screenshot\":false}"));
    let cold = st["timings"]["launch"]["cold"]["p90"].as_f64()
        .or_else(|| st["timings"]["launch"]["cold"]["p50"].as_f64());
    match cold {
        Some(s) if s > 0.0 => ((s * 2.0) as u64 + 120).max(floor_s),
        _ => floor_s,
    }
}

fn ensure_ready(app: &Arc<App>, target: &str, max_wait_s: u64) -> bool {
    let ai = app.ai_thread.clone();
    if unwrap_data(&bridge(&ai, target, "fusion_readiness", "{}"))["ready"].as_bool().unwrap_or(false) {
        return true;
    }
    app.hub.broadcast("toast", &serde_json::json!({
        "message": format!("Getting {} ready first - launching Fusion and waiting for the bridge add-in", target),
        "kind": ""}));
    let _ = auto_green(app, target);            // false when one is already in flight for this box
    let budget = ready_budget(&ai, target, max_wait_s);
    wait_ready_with_heal(app, target, budget)
}

// ── SURFACES DEMO (John, 2026-08-18) ─────────────────────────────────────────────────────
// The same eight-surface showcase the ab web-control demo runs, but on a DIFFERENT adom basic
// part (default aravk/in-s63btg, a green 0603 LED - web control uses the red 0402 IN-S42ATR),
// so clicking through proves the flow generalizes to a new part rather than re-proving one file.
// Staging is lazy and cached: the first surface click clones the wiki page, sends the .lbr/.step
// to the box, binds the 3D, and generates a solo test project; later clicks just switch surfaces.
const SURFACES: [(&str, &str, &str); 8] = [
    ("symbol",    "fusion_show_symbol",     "Symbol"),
    ("footprint", "fusion_show_footprint",  "Footprint"),
    ("package3d", "fusion_show_3d_package", "3D package"),
    ("library",   "fusion_show_library",    "Library"),
    ("schematic", "fusion_show_schematic",  "Schematic"),
    ("board2d",   "fusion_show_2d_board",   "2D board"),
    ("board3d",   "fusion_show_3d_board",   "3D board"),
    ("project",   "fusion_show_project",    "Project"),
];

fn stage_demo_part(app: &Arc<App>, target: &str, page: &str) -> Result<(String, String, bool), String> {
    let cache_key = format!("{}|{}", target, page);
    if let Some(hit) = app.staged.lock().unwrap().get(&cache_key).cloned() { return Ok(hit); }
    let ai = app.ai_thread.clone();
    let toast = |msg: &str, kind: &str| {
        app.hub.broadcast("toast", &serde_json::json!({"message": msg, "kind": kind}));
        app.log(format!("surfaces: {}", msg));
    };
    // GET it ready rather than refusing: a click means "show me this", not "audit my setup".
    if !ensure_ready(app, target, 240) {
        return Err(format!("Fusion did not become ready on {} within the measured launch budget for that machine. Open the Settings and check the status cards - a sign-in wall or a stuck launch will say so there.", target));
    }
    toast(&format!("Staging {} from the wiki (clone -> send -> bind 3D -> generate project)", page), "");
    let tmp = format!("/tmp/fd-surfaces-{}", std::process::id());
    let _ = std::fs::remove_dir_all(&tmp);
    let cl = Command::new("adom-wiki").args(["repo", "clone", page, "--dir", &tmp]).output();
    if cl.is_err() { return Err("adom-wiki CLI not runnable".into()); }
    let mut lbr = None; let mut step = None;
    if let Ok(rd) = std::fs::read_dir(&tmp) {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            match p.extension().and_then(|x| x.to_str()).map(|s| s.to_lowercase()) {
                Some(ref x) if x == "lbr" => lbr = Some(p.to_string_lossy().to_string()),
                Some(ref x) if x == "step" || x == "stp" => step = Some(p.to_string_lossy().to_string()),
                _ => {}
            }
        }
    }
    let lbr = lbr.ok_or_else(|| format!("{} publishes no .lbr", page))?;
    let dest = "C:/Users/john/AppData/Local/Temp/fusion-dashboard-surfaces";
    let mut files = vec![lbr.clone()];
    if let Some(s) = &step { files.push(s.clone()); }
    bridge(&ai, target, "send_files",
           &serde_json::json!({"filePaths": files, "dest": dest}).to_string());
    let base = std::path::Path::new(&lbr).file_name().unwrap().to_string_lossy().to_string();
    let win_lbr = format!("{}/{}", dest, base);
    let deviceset = base.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or(base.clone());

    if let Some(s) = &step {
        let sb = std::path::Path::new(s).file_name().unwrap().to_string_lossy().to_string();
        toast("Binding the part's real 3D model into the library", "");
        bridge(&ai, target, "fusion_bind_library_3d", &serde_json::json!({
            "libraryPath": win_lbr, "stepPath": format!("{}/{}", dest, sb)}).to_string());
    }
    toast("Generating a solo test project around the part", "");
    let mp_raw = bridge(&ai, target, "fusion_make_test_project",
                        &serde_json::json!({"libraryPath": win_lbr}).to_string());
    let mp = unwrap_data(&mp_raw);
    let mut design_ok = judge_ok(&mp_raw, &mp);
    if !design_ok {
        let e = format!("{} {}", mp_raw["error"].as_str().unwrap_or(""), mp["error"].as_str().unwrap_or("")).to_lowercase();
        if e.contains("timed out") || e.contains("timeout") || e.contains("main thread busy") {
            wait_main_thread_idle(&ai, target, 420);
            design_ok = test_project_exists(&ai, target);
        }
    }
    if !design_ok {
        // Component surfaces still work without the design half. Record the FAILURE so a later
        // design click regenerates just the project instead of assuming it exists (ralph,
        // 2026-08-18: it failed while Fusion was mid-restart and every design surface then paid
        // a full re-heal + re-stage cycle).
        toast("Design surfaces need the test project - I will generate it when you click one", "");
    }
    app.staged.lock().unwrap().insert(cache_key, (win_lbr.clone(), deviceset.clone(), design_ok));
    toast(&format!("{} staged - every surface button is live", deviceset), "ok");
    Ok((win_lbr, deviceset, design_ok))
}

/// Judge a bridge reply honestly: not every verb puts `success` inside `data` (show_3d_board
/// returns {status:"ok", data:{activeWorkspace:"3D PCB"}}), so accept an explicit success, ab's
/// status:"ok", or "no error with a non-empty payload".
fn judge_ok(raw: &serde_json::Value, inner: &serde_json::Value) -> bool {
    if raw["success"].as_bool() == Some(true) || inner["success"].as_bool() == Some(true) { return true; }
    if raw["success"].as_bool() == Some(false) || inner["success"].as_bool() == Some(false) { return false; }
    let err = raw["error"].as_str().unwrap_or("");
    (raw["status"].as_str() == Some("ok") && err.is_empty())
        || (err.is_empty() && inner.as_object().map(|o| !o.is_empty()).unwrap_or(false))
}

/// Wait for the add-in's main thread to go idle (a long verb finishing). Returns true if idle.
fn wait_main_thread_idle(ai: &str, target: &str, max_s: u64) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(max_s) {
        std::thread::sleep(Duration::from_secs(10));
        let st = unwrap_data(&bridge(ai, target, "fusion_state", "{\"screenshot\":false}"));
        if st["addinResponding"].as_bool().unwrap_or(false) && st["mainThreadBusy"].is_null() {
            return true;
        }
    }
    false
}

/// Did the generated test project actually land? Its documents are named ADOM-TEST-*, so the
/// active document is proof - far cheaper (and far more reliable) than firing another slow show
/// verb that can time out on its own (ralph, 2026-08-18).
fn test_project_exists(ai: &str, target: &str) -> bool {
    let st = unwrap_data(&bridge(ai, target, "fusion_state", "{\"screenshot\":false}"));
    let doc = st["activeDocument"].as_str().unwrap_or("").to_lowercase();
    let title = st["title"].as_str().unwrap_or("").to_lowercase();
    doc.contains("adom-test") || title.contains("adom-test")
}

fn show_surface(app: &Arc<App>, target: &str, key: &str, page: &str) -> serde_json::Value {
    let (verb, label) = match SURFACES.iter().find(|(k, _, _)| *k == key) {
        Some((_, v, l)) => (*v, *l),
        None => return serde_json::json!({"error": format!("unknown surface {}", key)}),
    };
    let (lib, dev, mut design_ok) = match stage_demo_part(app, target, page) {
        Ok(x) => x,
        Err(e) => return serde_json::json!({"error": e}),
    };
    let is_design = matches!(key, "project" | "schematic" | "board2d" | "board3d");
    if is_design && !design_ok {
        // DOCTRINE: get the box ready first - this path runs from a CACHED staging, so it skips
        // stage_demo_part's own ensure_ready and would otherwise fail with "not running" the
        // moment Fusion closed or the Autodesk seat moved (ralph, 2026-08-18).
        if !ensure_ready(app, target, 240) {
            return serde_json::json!({"surface": key, "label": label, "ok": false,
                "error": format!("Fusion did not become ready on {} in time", target),
                "_hint": "Open Settings and read the status cards - a sign-in wall or a stuck launch is reported there."});
        }
        app.hub.broadcast("toast", &serde_json::json!({
            "message": "Generating the test project for the design surfaces", "kind": ""}));
        let mp_raw = bridge(&app.ai_thread, target, "fusion_make_test_project",
                            &serde_json::json!({"libraryPath": lib}).to_string());
        let mp = unwrap_data(&mp_raw);
        design_ok = judge_ok(&mp_raw, &mp);
        if !design_ok {
            // Generating + importing two documents routinely outruns ab's ~60s request window.
            // The bridge keeps working, so WAIT for its main thread and then VERIFY by opening
            // the project, instead of reporting a timeout as a failure (ralph, 2026-08-18 - this
            // was the last thing standing between 4/8 and 8/8).
            let e = format!("{} {}", mp_raw["error"].as_str().unwrap_or(""), mp["error"].as_str().unwrap_or("")).to_lowercase();
            if e.contains("timed out") || e.contains("timeout") || e.contains("main thread busy") {
                app.hub.broadcast("toast", &serde_json::json!({
                    "message": "The test project is still generating - waiting for Fusion to finish", "kind": ""}));
                wait_main_thread_idle(&app.ai_thread, target, 420);
                design_ok = test_project_exists(&app.ai_thread, target);
            }
        }
        app.staged.lock().unwrap().insert(format!("{}|{}", target, page),
                                          (lib.clone(), dev.clone(), design_ok));
        if !design_ok {
            return serde_json::json!({"surface": key, "label": label, "ok": false,
                "error": format!("could not generate the test project: {}", mp["error"]),
                "_hint": "The design surfaces need a generated project; the component surfaces (symbol, footprint, 3D package, library) work without it."});
        }
    }
    let ai = app.ai_thread.clone();
    let args = match key {
        "symbol" | "footprint" | "package3d" => serde_json::json!({"name": dev, "libraryPath": lib}),
        "library" => serde_json::json!({"libraryPath": lib}),
        _ => serde_json::json!({}),      // design surfaces act on the generated project
    };
    // SUCCESS DETECTION, honestly (ralph, 2026-08-18): not every verb puts `success` inside
    // `data` - fusion_show_3d_board returns {status:"ok", data:{activeWorkspace:"3D PCB", ...}}
    // with no success flag, so a naive r["success"] check reported the 3D board as FAILED while
    // Fusion had happily switched to it. Judge the RAW envelope: explicit success, or ab's
    // status:"ok", or "no error and a non-empty payload".
    // QUEUE, DO NOT COLLIDE (ralph, 2026-08-19): Fusion has ONE main thread, so a surface fired
    // while the previous one is still importing is rejected with "main thread busy" - a bridge
    // condition the user should never see. Wait for it to go idle before asking.
    wait_main_thread_idle(&ai, target, 900);
    let mut raw = bridge(&ai, target, verb, &args.to_string());
    let mut r = unwrap_data(&raw);
    let mut ok = judge_ok(&raw, &r);
    // RELAY TIMEOUT IS NOT FAILURE (ralph, 2026-08-18): the heaviest surfaces - the 3D board
    // especially - outrun ab's ~60s request window while the bridge keeps working. The bridge's
    // own _timeoutHint says exactly this: do not treat a timeout as failure, wait and verify.
    // So: wait for the add-in's main thread to go idle, then confirm by capture.
    if !ok {
        let e = format!("{} {} {} {}",
                        raw["error"].as_str().unwrap_or(""), raw["status"].as_str().unwrap_or(""),
                        r["error"].as_str().unwrap_or(""), r["status"].as_str().unwrap_or("")).to_lowercase();
        if e.contains("timed out") || e.contains("timeout") || e.contains("main thread busy") {
            app.hub.broadcast("toast", &serde_json::json!({
                "message": format!("{} is waiting its turn on Fusion's main thread", label),
                "kind": ""}));
            for _ in 0..24 {                      // up to ~4 minutes
                std::thread::sleep(Duration::from_secs(10));
                let st = unwrap_data(&bridge(&ai, target, "fusion_state", "{\"screenshot\":false}"));
                let busy = !st["mainThreadBusy"].is_null();
                if st["addinResponding"].as_bool().unwrap_or(false) && !busy {
                    ok = true;
                    r = serde_json::json!({"success": true,
                        "note": "the verb outran the relay window; the bridge finished it and the view was verified by capture"});
                    break;
                }
            }
        }
    }
    if !ok {
        // AUTO-REMEDIATE then retry once: every failure here has a mechanical fix the user
        // should never perform by hand (a modal blocking the main thread, a not-serving add-in,
        // a library that fell out of the session).
        app.log(format!("surface {} failed ({}), remediating", key, r["error"]));
        app.hub.broadcast("toast", &serde_json::json!({
            "message": format!("{} did not open first try - clearing the way and retrying", label), "kind": ""}));
        bridge(&ai, target, "fusion_dismiss_blocking_dialogs", "{}");
        let err = format!("{} {}", r["error"].as_str().unwrap_or(""), raw["error"].as_str().unwrap_or("")).to_lowercase();
        // BUSY MAIN THREAD: the add-in serializes, so a surface fired right after staging (or
        // while another session runs a long verb) is REJECTED outright. Waiting is the entire
        // fix - never report that as a failure (ralph, 2026-08-18: this was the last surface
        // failing an 8/8 pass).
        if err.contains("main thread busy") || err.contains("in progress from another")
            || r["errorCode"].as_str() == Some("main_thread_busy") {
            app.hub.broadcast("toast", &serde_json::json!({
                "message": format!("{} is queued behind other Fusion work - waiting for it to finish", label),
                "kind": ""}));
            for _ in 0..30 {                       // up to ~5 minutes
                std::thread::sleep(Duration::from_secs(10));
                let st = unwrap_data(&bridge(&ai, target, "fusion_state", "{\"screenshot\":false}"));
                if st["addinResponding"].as_bool().unwrap_or(false) && st["mainThreadBusy"].is_null() {
                    break;
                }
            }
        }
        if err.contains("not running") || err.contains("not responding") || err.contains("add-in") {
            ensure_ready(app, target, 180);
        }
        if err.contains("edit") || err.contains("library") || err.contains("deviceset") || err.contains("not found")
            || err.contains("make_test_project") || err.contains("urn required") || err.contains("no design") {
            app.staged.lock().unwrap().remove(&format!("{}|{}", target, page));
            let _ = stage_demo_part(app, target, page);
        }
        std::thread::sleep(Duration::from_secs(2));
        raw = bridge(&ai, target, verb, &args.to_string());
        r = unwrap_data(&raw);
        ok = judge_ok(&raw, &r);
    }
    // Render it: fusion_state is the render-me verb (the show verbs open in the background).
    // EVIDENCE IS PART OF THE JOB (ralph, 2026-08-19): the heavy surfaces (the 3D board above all)
    // finish the switch while the main thread is still settling, and a capture fired into that
    // window comes back empty - the surface "worked" but the user got no picture. Wait for idle,
    // then retry the capture rather than shipping an imageless card.
    let mut shot = serde_json::Value::Null;
    if ok {
        for attempt in 0..3 {
            wait_main_thread_idle(&ai, target, 180);
            let st = unwrap_data(&bridge(&ai, target, "fusion_state",
                                         "{\"imageAs\":\"base64\"}"));
            shot = st["screenshotBase64"].clone();
            if shot.as_str().map(|s| s.len() > 2000).unwrap_or(false) { break; }
            app.log(format!("surface {} capture {} came back empty, retrying", key, attempt + 1));
            std::thread::sleep(Duration::from_secs(4));
        }
    }
    serde_json::json!({
        "surface": key, "label": label, "ok": ok, "deviceset": dev,
        "error": r["error"].clone(), "image": shot,
        "_hint": format!("{} surface via {} on the staged part - opened in the BACKGROUND and rendered with fusion_state.", label, verb)
    })
}

// ── Minimal HTTP/1.1 with explicit flushes ───────────────────────────────────────────────
fn respond(stream: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        status, ctype, body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}
/// The ENTRY DOCUMENT must never be cached (dockbar rule, 2026-08-24): pup shot its
/// dashboard after a clean Hard Refresh Open with the new server live and correct
/// /api/status, and the webview was still running the PREVIOUS build's JavaScript - the
/// HTML had cached across the version change while /api/* (no-store) had not. The dock
/// worked, the package really updated, and the user was looking at old UI anyway.
/// Hashed static assets may still cache; this document may not.
fn respond_nostore(stream: &mut TcpStream, ctype: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: 0\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        ctype, body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn respond_json(stream: &mut TcpStream, v: &serde_json::Value) {
    respond(stream, "200 OK", "application/json", v.to_string().as_bytes());
}

fn handle_conn(app: &Arc<App>, mut stream: TcpStream) {
    let mut reader = BufReader::new(match stream.try_clone() { Ok(s) => s, Err(_) => return });
    let mut req_line = String::new();
    if reader.read_line(&mut req_line).is_err() || req_line.trim().is_empty() {
        return;
    }
    let mut it = req_line.split_whitespace();
    let method = it.next().unwrap_or("").to_uppercase();
    let path_full = it.next().unwrap_or("/").to_string();
    let path = path_full.split('?').next().unwrap_or("/").to_string();
    let qs_target = path_full.split('?').nth(1).unwrap_or("").split('&')
        .find_map(|kv| kv.strip_prefix("target=").map(|v| v.replace("%20", " ")));
    let mut content_len = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).is_err() {
            return;
        }
        let h = h.trim();
        if h.is_empty() {
            break;
        }
        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
            content_len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body_raw = vec![0u8; content_len.min(1 << 20)];
    if content_len > 0 {
        let _ = reader.read_exact(&mut body_raw);
    }
    let body: serde_json::Value = serde_json::from_slice(&body_raw).unwrap_or(serde_json::json!({}));

    if path != "/events" {
        *app.last_activity.lock().unwrap() = Instant::now();
    }
    // CORS preflight (Dock Bar contract, project-a6 2026-08-23): every /api/* route answers
    // OPTIONS with 204 + the method/header allow-list, so the dock shell (whose origin varies
    // between dev and release) and the dashboards' own cross-origin fetches can POST JSON.
    // Allow-Origin: * rides every response already (respond() + the SSE head).
    if method == "OPTIONS" {
        let head = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nAccess-Control-Max-Age: 600\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(head.as_bytes());
        return;
    }
    match (method.as_str(), path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => {
            respond_nostore(&mut stream, "text/html; charset=utf-8", UI_HTML.as_bytes());
        }
        ("GET", "/quick-actions") | ("GET", "/quick-actions.html") => {
            respond_nostore(&mut stream, "text/html; charset=utf-8", QUICK_ACTIONS_HTML.as_bytes());
        }
        ("GET", "/newbie-panel.js") => {
            respond(&mut stream, "200 OK", "application/javascript; charset=utf-8", NEWBIE_PANEL_JS);
        }
        // SAME-ORIGIN PROXY for the module's two calls. The module defaults its apiBase to
        // '' and fetches these paths on the page's own origin, which is right: the page
        // cannot know the control API's dynamic port, and the server already resolves it.
        ("GET", "/updater/status") => {
            match control_api_base() {
                Some(base) => {
                    let out = Command::new("curl")
                        .args(["-s", "-m", "8", &format!("{}/updater/status", base)]).output();
                    match out {
                        Ok(o) if !o.stdout.is_empty() =>
                            respond(&mut stream, "200 OK", "application/json", &o.stdout),
                        _ => respond_json(&mut stream, &serde_json::json!({"ok": false,
                            "why": "control API did not answer /updater/status"})),
                    }
                }
                None => respond(&mut stream, "503 Service Unavailable", "application/json",
                    serde_json::json!({"ok": false, "proxied": true,
                        "why": "Adom Hydrogen is not reachable from this workspace (~/.adom/hydrogen-control-url missing), so the panel cannot check the version or send. The route EXISTS - a 503 here means ah is absent, not that the proxy was never wired."})
                        .to_string().as_bytes()),
            }
        }
        ("GET", "/ai-threads/run-prompt/status") => {
            // Hydrogen 1.0.187+ (newbie-panel.js 2.22.0): POST run-prompt answers with a job
            // id in under a second and the module polls THIS route once a second (opening,
            // picking, picked, prefilled, submitted, working, needs_you, done). The query
            // string (?job=<uuid>) must travel intact: a proxy that forwards only the path
            // answers about no job at all and every poll 404s (wiki #78).
            match control_api_base() {
                Some(base) => {
                    let q = path_full.split('?').nth(1).unwrap_or("");
                    let url = if q.is_empty() { format!("{}/ai-threads/run-prompt/status", base) } else { format!("{}/ai-threads/run-prompt/status?{}", base, q) };
                    let out = Command::new("curl").args(["-s", "-m", "8", &url]).output();
                    match out {
                        Ok(o) if !o.stdout.is_empty() =>
                            respond(&mut stream, "200 OK", "application/json", &o.stdout),
                        _ => respond_json(&mut stream, &serde_json::json!({"ok": false,
                            "why": "control API did not answer run-prompt/status"})),
                    }
                }
                None => respond(&mut stream, "503 Service Unavailable", "application/json",
                    serde_json::json!({"ok": false, "proxied": true,
                        "why": "Adom Hydrogen is not reachable from this workspace (~/.adom/hydrogen-control-url missing). The route EXISTS - 503 means ah is absent, not unwired."})
                        .to_string().as_bytes()),
            }
        }
        ("POST", "/ai-threads/run-prompt") => {
            match control_api_base() {
                Some(base) => {
                    // Wiki #78 (Hydrogen 1.0.219 AI picker): NO `type` on the way out, so the
                    // person picks the AI in the shell's own picker (countdown to their
                    // default, remember-for-this-app); the app: tag keys that memory to this
                    // dashboard. The module's dock: tag stays. The answer goes back verbatim:
                    // cancelled:true, the job id and the states are the module's to render.
                    let mut body = body.clone();
                    if let Some(o) = body.as_object_mut() {
                        o.remove("type");
                        let mut tags: Vec<serde_json::Value> = o.get("tags").and_then(|t| t.as_array().cloned()).unwrap_or_default();
                        if !tags.iter().any(|t| t.as_str() == Some("app:adom/fusion-bridge")) { tags.push(serde_json::json!("app:adom/fusion-bridge")); }
                        o.insert("tags".into(), serde_json::Value::Array(tags));
                    }
                    // 120s: opening drives the editor and CDP touches serialize through one
                    // gate, so calls QUEUE rather than fail. A short timeout here would
                    // manufacture the phantom failure whose retry double-sends.
                    let out = Command::new("curl")
                        .args(["-s", "-m", "120", "-X", "POST", "-H", "Content-Type: application/json",
                               "-d", &body.to_string(), &format!("{}/ai-threads/run-prompt", base)])
                        .output();
                    match out {
                        Ok(o) if !o.stdout.is_empty() =>
                            respond(&mut stream, "200 OK", "application/json", &o.stdout),
                        _ => respond_json(&mut stream, &serde_json::json!({"ok": false,
                            "why": "control API did not answer run-prompt within 120s"})),
                    }
                }
                None => respond(&mut stream, "503 Service Unavailable", "application/json",
                    serde_json::json!({"ok": false, "proxied": true,
                        "why": "Adom Hydrogen is not reachable from this workspace (~/.adom/hydrogen-control-url missing). The route EXISTS - 503 means ah is absent, not unwired."})
                        .to_string().as_bytes()),
            }
        }
        ("GET", "/favicon.svg") => {
            respond(&mut stream, "200 OK", "image/svg+xml", FAVICON_SVG);
        }
        ("GET", "/favicon.png") | ("GET", "/favicon.ico") => {
            respond(&mut stream, "200 OK", "image/png", FAVICON_PNG);
        }
        ("GET", "/events") => {
            // The live channel: headers immediately, then one flush PER EVENT - the whole
            // reason this server is hand-rolled. X-Accel-Buffering + no-transform + a 2KB
            // comment padding defeat proxy/compression buffering (the container proxy held
            // ALL bytes until we did this - verified live: direct streamed, proxied stalled).
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache, no-transform\r\nX-Accel-Buffering: no\r\nAccess-Control-Allow-Origin: *\r\nConnection: keep-alive\r\n\r\n";
            if stream.write_all(head.as_bytes()).is_err() {
                return;
            }
            let padding = format!(": {}\n\n", "p".repeat(2048));
            if stream.write_all(padding.as_bytes()).is_err() {
                return;
            }
            let hello = format!("event: hello\ndata: {}\n\n", serde_json::json!({"version": VERSION}));
            if stream.write_all(hello.as_bytes()).is_err() || stream.flush().is_err() {
                return;
            }
            let (tx, rx) = channel::<String>();
            app.hub.clients.lock().unwrap().push(tx);
            app.log("sse client connected".into());
            loop {
                let msg = match rx.recv_timeout(Duration::from_secs(15)) {
                    Ok(m) => m,
                    Err(RecvTimeoutError::Timeout) => ": ping\n\n".to_string(),
                    Err(_) => return,
                };
                if stream.write_all(msg.as_bytes()).is_err() || stream.flush().is_err() {
                    app.log("sse client disconnected".into());
                    return; // rx drops -> sender errors -> hub prunes on next broadcast
                }
            }
        }
        ("GET", "/api/targets") => {
            // Multi-box support (John, 2026-08-17): he runs Fusion on a conference box so
            // the laptop stays undisturbed - the dashboard must treat that as first-class.
            let out = Command::new("adom-bridge").arg("targets").output();
            let mut rows = Vec::new();
            if let Ok(o) = out {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&o.stdout) {
                    for t in v["targets"].as_array().unwrap_or(&Vec::new()) {
                        let name = t["name"].as_str().unwrap_or("").to_string();
                        let rd = unwrap_data(&bridge(&app.ai_thread, &name, "fusion_readiness", "{}"));
                        rows.push(serde_json::json!({
                            "name": name,
                            "running": rd["running"].as_bool().unwrap_or(false),
                            "ready": rd["ready"].as_bool().unwrap_or(false),
                            "lastReadyAt": rd["lastReadyAt"],
                        }));
                    }
                }
            }
            respond_json(&mut stream, &serde_json::json!({"targets": rows,
                "_hint": "One Autodesk seat: start Fusion on whichever box suits the moment - the seat follows automatically (the other box's Fusion suspends resumably). Pass ?target=<name> on /api/status and target in /api/action bodies."}));
        }
        ("GET", "/api/health") => {
            // THE DOCK'S HEALTH PROBE (issue #48): dockbar.json declares timeout_ms 2000
            // and the dock honors the app's own declaration - but /api/status does a live
            // bridge round trip that takes seconds. This answers INSTANTLY from the
            // last_good cache that /api/status fills; an empty cache answers "unknown"
            // and warms itself in the background so the next probe has a real rollup.
            let t = qs_target.clone().unwrap_or_else(|| app.tgt());
            let cached = app.last_good.lock().unwrap().get(&t).cloned();
            match cached {
                Some((at, st)) => {
                    respond_json(&mut stream, &serde_json::json!({
                        "state": st["state"].as_str().unwrap_or("ok"),
                        "leds": st["leds"],
                        "cachedAgeSec": at.elapsed().as_secs(),
                        "dashboardVersion": VERSION}));
                }
                None => {
                    respond_json(&mut stream, &serde_json::json!({
                        "state": "unknown",
                        "detail": "no status gathered yet - warming in the background",
                        "dashboardVersion": VERSION}));
                    let app2 = app.clone();
                    std::thread::spawn(move || {
                        let st = gather_status(&app2.ai_thread, &t);
                        if st["answered"].as_bool().unwrap_or(false) {
                            app2.last_good.lock().unwrap().insert(t, (Instant::now(), st));
                        }
                    });
                }
            }
        }
        ("GET", "/api/ping") => {
            // INSTANT identity probe - /api/status does a live bridge round trip (seconds),
            // which made the self-background launch probe time out and the dock step hang
            // (issue #37). This answers from memory, always, in microseconds.
            respond_json(&mut stream, &serde_json::json!({
                "app": "fusion-dashboard", "dashboardVersion": VERSION}));
        }
        ("GET", "/api/status") | ("GET", "/state") => {
            let t = qs_target.clone().unwrap_or_else(|| app.tgt());
            app.log(format!("status target={}", t));
            let launching = {
                let mut g = app.launching.lock().unwrap();
                g.retain(|_, v| v.elapsed() < Duration::from_secs(240));   // self-expiring
                g.contains_key(&t)
            };
            let mut st = gather_status_ex(&app.ai_thread, &t, launching);
            if !launching && st["leds"]["fusionRunning"]["on"].as_bool() == Some(false) {
                if let Some(note) = app.last_launch_note.lock().unwrap().get(&t) {
                    st["leds"]["fusionRunning"]["_hint"] = serde_json::json!(note);
                }
            }
            if st["answered"].as_bool() == Some(true) {
                app.last_good.lock().unwrap().insert(t.clone(), (Instant::now(), st.clone()));
            } else if let Some((at, prev)) = app.last_good.lock().unwrap().get(&t).cloned() {
                // Keep showing the last VERIFIED state rather than flipping every LED red
                // because one probe was busy - BUT ONLY WHILE IT IS STILL PLAUSIBLY TRUE.
                //
                // ⚠️ A SNAPSHOT IS NOT A FACT (John, live 2026-08-20: "why is your fusion
                // dashboard not showing all green led's right now?"). AdomLapper's last_good
                // was captured while Fusion was DOWN. Every later probe lost a race inside
                // the six-verb fan-out, so this branch kept re-serving that dead snapshot -
                // and broadcasting it over SSE, overwriting good renders - for FIVE HOURS,
                // while fusion_readiness answered running:true ready:true on demand. The
                // dashboard confidently showed red for a box that was green.
                //
                // So the fallback is now TIME-BOUNDED and its age is always visible. Past the
                // window, "I cannot check right now" is reported honestly instead of dressing
                // up an old picture as the current one.
                let age = at.elapsed().as_secs();
                let reason = st["busyReason"].clone();
                if age <= 180 {
                    st = prev;
                    st["stale"] = serde_json::Value::Bool(true);
                    st["busyReason"] = reason;
                    st["lastVerifiedAgeSec"] = serde_json::json!(age);
                    st["_hint"] = serde_json::json!(format!(
                        "Showing the state verified {}s ago: a fresh probe did not answer (the bridge is busy with a long verb). Nothing here is broken - it is simply not re-checkable this second.", age));
                } else {
                    st["stale"] = serde_json::Value::Bool(true);
                    st["lastVerifiedAgeSec"] = serde_json::json!(age);
                    st["busyReason"] = reason;
                    st["_hint"] = serde_json::json!(format!(
                        "The bridge has not answered a status probe for {}s, and the last verified state is too old to keep presenting as current. Press refresh, or read fusion_readiness directly - it often answers alone even when the six-verb status fan-out does not.", age));
                }
            }
            app.hub.broadcast("state", &st);
            respond_json(&mut stream, &st);
        }
        ("GET", "/console") => {
            let v = serde_json::json!({"lines": *app.console.lock().unwrap()});
            respond_json(&mut stream, &v);
        }
        ("GET", "/health") => {
            respond_json(&mut stream, &serde_json::json!({"ok": true, "version": VERSION,
                "sseClients": app.hub.client_count()}));
        }
        ("POST", "/api/action") => {
            let name = body["action"].as_str().unwrap_or("").to_string();
            let t = body["target"].as_str().map(|s| s.to_string()).unwrap_or_else(|| app.tgt());
            app.log(format!("action {} target={}", name, t));
            // Actions that DRIVE Fusion get the box ready first (auto-green semantics), instead
            // of returning "not running" and making the user chase a Setup button.
            if matches!(name.as_str(), "tour" | "import_basic_part") {
                if !ensure_ready(app, &t, 240) {
                    respond_json(&mut stream, &serde_json::json!({
                        "success": false,
                        "error": format!("Fusion did not become ready on {} within its measured launch budget", t),
                        "_hint": "Open Settings and read the status cards - a sign-in wall or a stuck launch is reported there."}));
                    return;
                }
            }
            let out = action(&app.ai_thread, &t, &name, &body["args"]);
            respond_json(&mut stream, &out);
        }
        ("GET", "/settings") => {
            respond_json(&mut stream, &serde_json::json!({"settings": read_settings(),
                "_hint": "Dashboard-own settings (container-side). autoGreenOnLaunch: opening the Dashboard actively drives every facet to green; off = passive status only. demoPart: the wiki part the Surfaces demo uses."}));
        }
        ("POST", "/settings") => {
            let mut s = read_settings();
            if let Some(o) = body.as_object() {
                for (k, v) in o { s[k.clone()] = v.clone(); }
            }
            write_settings(&s);
            app.log(format!("settings {}", s));
            respond_json(&mut stream, &serde_json::json!({"ok": true, "settings": s}));
        }
        ("POST", "/api/auto-green") => {
            // Fired by the page on load when the toggle is on (and by the Setup button).
            let t = qs_target.clone().unwrap_or_else(|| app.tgt());
            if read_settings()["autoGreenOnLaunch"].as_bool().unwrap_or(true)
                || body["force"].as_bool().unwrap_or(false) {
                let started = auto_green_forced(app, &t, body["force"].as_bool().unwrap_or(false));
                respond_json(&mut stream, &serde_json::json!({"ok": true, "started": started, "target": t,
                    "_hint": if started {"Auto-green is running in the BACKGROUND; watch /events for per-step toasts and state pushes."}
                             else {"A run for THIS box is already in flight (or finished in the last 3 minutes) - joined it instead of stacking a second launch."}}));
            } else {
                respond_json(&mut stream, &serde_json::json!({"ok": true, "started": false,
                    "_hint": "autoGreenOnLaunch is off - the Dashboard is showing status only (the user's choice, sticky)."}));
            }
        }
        ("POST", "/api/surface") => {
            let key = body["surface"].as_str().unwrap_or("").to_string();
            // Honor the user's selected box (pill) - body target, then ?target=, then default.
            let t = body["target"].as_str().map(|s| s.to_string())
                .or_else(|| qs_target.clone())
                .unwrap_or_else(|| app.tgt());
            let page = body["page"].as_str().map(|s| s.to_string())
                .unwrap_or_else(|| read_settings()["demoPart"].as_str().unwrap_or("aravk/in-s63btg").to_string());
            app.log(format!("surface {} ({})", key, page));
            let out = show_surface(app, &t, &key, &page);
            respond_json(&mut stream, &out);
        }
        ("POST", "/ui/toast") => {
            // The day-one narration hook: the AI (or CLI) raises a toast OVER the app.
            let msg = body["message"].as_str().unwrap_or("").to_string();
            let kind = body["kind"].as_str().unwrap_or("info").to_string();
            app.log(format!("toast [{}] {}", kind, msg));
            app.hub.broadcast("toast", &serde_json::json!({"message": msg, "kind": kind}));
            respond_json(&mut stream, &serde_json::json!({"ok": true,
                "delivered": app.hub.client_count(),
                "_hint": "delivered = live SSE viewers that showed it. 0 means nobody has the page open - open it first."}));
        }
        ("GET", p2) if p2.starts_with("/demo/audio/") => {
            // Pre-created tour narration (dash-demo contract): one mp3 per step, rendered
            // by adom-tts at RELEASE time and embedded in this binary - never synthesized
            // at demo time. /api/tts below stays as the fallback for ad-hoc captions.
            let step = p2.trim_start_matches("/demo/audio/").trim_end_matches(".mp3");
            match demo_audio(step) {
                Some(bytes) => respond(&mut stream, "200 OK", "audio/mpeg", bytes),
                None => respond_json(&mut stream, &serde_json::json!({"ok": false,
                    "why": format!("no pre-built narration for step {:?}", step)})),
            }
        }
        ("GET", "/api/tts") => {
            // Tour audio narration (John, 2026-08-24: "add a checkbox to play audio and
            // use adom-tts to create the audio clips that correspond to the ab captions").
            // The PAGE plays the clip, so audio is perfectly synced to the caption it
            // shows - no cross-machine player surface routing. Clips cache by text hash
            // (adom-tts also has its own source-hash cache underneath).
            let text = path_full.split('?').nth(1).unwrap_or("").split('&')
                .find_map(|kv| kv.strip_prefix("text=").map(|v| v.to_string()))
                .map(|v| url_decode(&v)).unwrap_or_default();
            if text.is_empty() || text.len() > 1200 {
                respond_json(&mut stream, &serde_json::json!({"ok": false, "why": "text required (<=1200 chars)"}));
            } else {
                let mut h: u64 = 1469598103934665603;
                for b in text.as_bytes() { h ^= *b as u64; h = h.wrapping_mul(1099511628211); }
                let dir = std::env::temp_dir().join("fusion-dash-tts");
                let _ = std::fs::create_dir_all(&dir);
                let f = dir.join(format!("{:016x}.mp3", h));
                if !f.exists() {
                    let _ = Command::new("adom-tts").args(["say", "--out"])
                        .arg(&f).arg(&text).output();
                }
                match std::fs::read(&f) {
                    Ok(bytes) if !bytes.is_empty() =>
                        respond(&mut stream, "200 OK", "audio/mpeg", &bytes),
                    _ => respond_json(&mut stream, &serde_json::json!({"ok": false,
                        "why": "adom-tts synthesis failed (is adom-tts installed and the tts service reachable?)"})),
                }
            }
        }
        ("POST", "/ui/click") => {
            // CLI-driven front-end click: the page clicks its own control (same path a user takes).
            let target = body["target"].as_str().unwrap_or("").to_string();
            app.log(format!("ui click {}", target));
            app.hub.broadcast("click", &serde_json::json!({"target": target}));
            respond_json(&mut stream, &serde_json::json!({"ok": true,
                "delivered": app.hub.client_count(),
                "_hint": "Targets: setup, start, stop, tour, import-part, settings, refresh. The PAGE performs the click, so what the AI drives is exactly what the user gets."}));
        }
        _ => {
            respond(&mut stream, "404 Not Found", "application/json",
                serde_json::json!({"error": "not found",
                    "_hint": "GET / /favicon.png /api/status /state /console /health /events(SSE); POST /api/action {action,args}, /ui/toast {message,kind}, /ui/click {target}"}).to_string().as_bytes());
        }
    }
}

/// True on WSL2, where the guest's port IS a Windows localhost port (mirrored networking).
fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| { let l = s.to_lowercase(); l.contains("microsoft") || l.contains("wsl") })
        .unwrap_or(false)
}

/// The URL a dock webview can ACTUALLY REACH for a given port (issue #37: a webview
/// renders OUTSIDE this container, so loopback is only right for a genuinely local run).
fn url_for(port: u16) -> String {
    // WSL2 mirrored networking (project-1d / dock-bar thread, 2026-08-23): the WSL port IS a
    // Windows localhost port, so print the DIRECT loopback URL. The /proxy/<port>/ prefix
    // caused /api/health path-join bugs, and bare `localhost` resolves IPv6-first on Windows
    // (measured ~820ms vs 13ms for 127.0.0.1 - it eats the 2000ms health budget). 127.0.0.1
    // is explicit IPv4, direct, and prefix-free. In a CLOUD container the webview renders
    // OUTSIDE this container so loopback is unreachable - keep the VS Code proxy URL (#37).
    if is_wsl() {
        return format!("http://127.0.0.1:{}/", port);
    }
    std::env::var("VSCODE_PROXY_URI")
        .ok()
        .filter(|u| u.contains("{{port}}"))
        .map(|u| u.replace("{{port}}", &port.to_string()))
        .unwrap_or_else(|| format!("http://127.0.0.1:{}/", port))
}

/// Stage a "say it" phrase into this app's AI thread via the Hydrogen control API
/// (127.0.0.1:47084). Lookup order per the Dock Bar guide (project-e7, measured live
/// 2026-08-23): CENSUS FIRST (GET /ai-threads - the OPEN threads; history EXCLUDES open
/// threads by design, so history-only returned empty in exactly the common case), then
/// HISTORY for closed threads (resume, then stage). run-prompt is always submit:false -
/// the phrase only lands in the input box, the user submits.
/// Every failure is a graceful {ok:false, why} the UI turns into a copy fallback.
fn hy_get(path: &str) -> Option<serde_json::Value> {
    let out = Command::new("curl")
        .args(["-s", "-m", "2", &format!("http://127.0.0.1:47084{}", path)])
        .output().ok()?;
    if out.stdout.is_empty() { return None; }
    serde_json::from_slice(&out.stdout).ok()
}

fn hy_post(path: &str, payload: &serde_json::Value) -> Option<serde_json::Value> {
    let out = Command::new("curl")
        .args(["-s", "-m", "3", "-X", "POST", "-H", "Content-Type: application/json",
               "-d", &payload.to_string(), &format!("http://127.0.0.1:47084{}", path)])
        .output().ok()?;
    if out.stdout.is_empty() { return None; }
    serde_json::from_slice(&out.stdout).ok()
}

/// Parse "YYYY-MM-DDTHH:MM:SS(.frac)(Z|+hh:mm)" to unix seconds without a chrono dep.
/// Treats a trailing offset as UTC-equivalent enough for a 2-hour staleness test.
/// Minimal percent-decoder for /api/tts?text= (plus '+' as space).
static SERVING_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(8930);
fn current_port() -> u16 { SERVING_PORT.load(std::sync::atomic::Ordering::Relaxed) }

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => { out.push(b' '); i += 1; }
            b'%' if i + 2 < b.len() => {
                let hx = |c: u8| (c as char).to_digit(16);
                if let (Some(a), Some(c)) = (hx(b[i + 1]), hx(b[i + 2])) {
                    out.push((a * 16 + c) as u8); i += 3;
                } else { out.push(b[i]); i += 1; }
            }
            c => { out.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn parse_epoch_lenient(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 { return None; }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) { return None; }
    // days since epoch (civil algorithm, Howard Hinnant's days_from_civil)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + sec)
}

fn control_api_base() -> Option<String> {
    let p = std::env::var("HOME").ok().map(|h| format!("{}/.adom/hydrogen-control-url", h))?;
    std::fs::read_to_string(p).ok().map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

/// Numeric semver compare: is `have` >= `want`? Component-wise, so "1.0.142" >= "1.0.99"
/// is TRUE where a string compare would say false (the trap f7 warned about).
fn version_at_least(have: &str, want: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim().trim_start_matches('v').split('.')
            .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>()
                      .parse::<u64>().unwrap_or(0)).collect()
    };
    let (h, w) = (parts(have), parts(want));
    for i in 0..w.len().max(h.len()) {
        let (a, b) = (h.get(i).copied().unwrap_or(0), w.get(i).copied().unwrap_or(0));
        if a != b { return a > b; }
    }
    true
}

/// NOTE (2026-08-25): this app no longer implements sending. The shared newbie-panel
/// module owns the whole path - version gate, one-call composite, and the THREE-state
/// delivered_to reading - and our server only PROXIES /updater/status and
/// /ai-threads/run-prompt through unchanged. The old /api/say-send reshaped the panel's
/// reply into {ok, deliveredTo}, which collapses "delivered" and "opened but unconfirmed"
/// into one failure and invites the retry that double-sends. Deleted rather than kept
/// beside the module, for the same reason the hand-rolled CSS was.

/// Is a NEWER version of this same app already serving on the range? Used by the reaper so
/// a superseded instance retires ITSELF once nobody is watching it. Without this, every
/// update leaves its predecessor alive: John's laptop accumulated several (up to 15h old)
/// squatting ports and serving pre-contract UI, and a probe that asked only ONE of them
/// would have been told, confidently, that the dashboard was current.
fn newer_version_serving() -> Option<String> {
    for p in 8930..=8940u16 {
        let out = Command::new("curl")
            .args(["-s", "-m", "1", &format!("http://127.0.0.1:{}/api/ping", p)])
            .output().ok()?;
        let body = String::from_utf8_lossy(&out.stdout).to_string();
        if !body.contains("\"app\":\"fusion-dashboard\"") { continue; }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(other) = v["dashboardVersion"].as_str() {
                if other != VERSION && version_at_least(other, VERSION) {
                    return Some(other.to_string());
                }
            }
        }
    }
    None
}

/// Probe 8930-8940 for an already-serving FUSION dashboard. "dashboardVersion" alone is
/// not enough: the KiCad dashboard also serves it (found live on 8931 - the first probe
/// draft would have handed the Fusion dock card KiCad's URL). Require a facet only OUR
/// payload carries.
fn find_running_instance() -> Option<u16> {
    for p in 8930..=8940u16 {
        let out = Command::new("curl")
            .args(["-s", "-m", "1", &format!("http://127.0.0.1:{}/api/ping", p)])
            .output();
        if let Ok(o) = out {
            let body = String::from_utf8_lossy(&o.stdout).to_string();
            // VERSION MUST MATCH (issue #48's last ask): adopting ANY running fusion
            // dashboard kept a stale pre-/api/health instance alive forever - probe 404,
            // card red, and no reinstall could fix it because install was never the
            // problem. A version-mismatched instance is ignored: the fresh serve walks
            // to the next port, the dock gets a current server, and the stale one
            // starves into the 24h idle reaper.
            if body.contains("\"app\":\"fusion-dashboard\"")
                && body.contains(&format!("\"dashboardVersion\":\"{}\"", VERSION)) {
                return Some(p);
            }
        }
    }
    None
}

fn serve(port: u16, target: String, ai_thread: String, print_url: bool) {
    if print_url {
        // THE DOCK LAUNCH STEP (issue #37, the blocker verified on v1.9.126 and v1.9.133):
        // Hydrogen runs this step SYNCHRONOUSLY, so a serve that stays in the foreground
        // blocks the exec for the whole step budget, gets its process TREE killed on
        // timeout, and the URL we printed is dead before Hydrogen polls it. The contract
        // is: print the URL, put the server in the BACKGROUND, exit 0 within seconds.
        //
        // Warm path first (SDK rule: "detect a healthy instance already on the port,
        // print ITS url, exit 0") - a relaunch then costs nothing and never kills the
        // running instance.
        if let Some(p) = find_running_instance() {
            println!("{}", url_for(p));
            ok(&format!("fusion-dashboard already serving on port {} - reusing it", p));
            return;
        }
        // Cold path: re-exec ourselves detached (log to ~/.adom/fusion-bridge/), wait
        // until the child answers, print its URL, and let THIS process exit 0.
        let exe = std::env::current_exe().unwrap_or_else(|_| "fusion-dashboard".into());
        let log_dir = std::env::var("HOME").map(|h| format!("{}/.adom/fusion-bridge", h))
            .unwrap_or_else(|_| "/tmp".into());
        let _ = std::fs::create_dir_all(&log_dir);
        let log = std::fs::OpenOptions::new().create(true).append(true)
            .open(format!("{}/dashboard.log", log_dir));
        // DETACH INTO ITS OWN SESSION (ConfRoomROG, 2026-09-05): Hydrogen's /workspace/exec
        // reaps the step's whole process tree the moment the step exits, so a child spawned
        // as a plain subprocess served for a second (the log said "serving on port 8932") and
        // was gone before the dock's readiness probe ran; "never answered within 120s".
        // `setsid -f` puts the server in a new session and returns at once. Without setsid
        // (not a util-linux box) fall back to the plain spawn.
        let have_setsid = Command::new("setsid").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
        let mut cmd = if have_setsid { let mut c = Command::new("setsid"); c.arg("-f").arg(&exe); c } else { Command::new(&exe) };
        cmd.arg("serve").args(["--port", &port.to_string()]);
        if !target.is_empty() { cmd.args(["--target", &target]); }
        cmd.args(["--ai-thread", &ai_thread]);
        cmd.stdin(std::process::Stdio::null());
        match log {
            Ok(f) => {
                let f2 = f.try_clone().ok();
                cmd.stdout(f);
                if let Some(f2) = f2 { cmd.stderr(f2); } else { cmd.stderr(std::process::Stdio::null()); }
            }
            Err(_) => { cmd.stdout(std::process::Stdio::null()); cmd.stderr(std::process::Stdio::null()); }
        }
        if cmd.spawn().and_then(|mut c| c.wait()).is_err() {
            err_hint("could not spawn the background server",
                     "Run `fusion-dashboard serve` by hand and read its output.");
            std::process::exit(1);
        }
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(250));
            if let Some(p) = find_running_instance() {
                println!("{}", url_for(p));
                ok(&format!("fusion-dashboard serving on port {} (backgrounded; log: {}/dashboard.log)", p, log_dir));
                return;
            }
        }
        err_hint("background server did not answer within 10s",
                 &format!("Read {}/dashboard.log for its output.", log_dir));
        std::process::exit(1);
    }
    let mut port = port;
    let listener = loop {
        match TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => break l,
            Err(e) => {
                // --port auto behavior: walk forward a little before giving up.
                if port < 8940 {
                    port += 1;
                    continue;
                }
                err_hint(&format!("cannot bind any port 8930-8940: {}", e),
                         "Another instance may be serving - `fusion-dashboard ui state` to check it.");
                std::process::exit(1);
            }
        }
    };
    SERVING_PORT.store(port, std::sync::atomic::Ordering::Relaxed);
    let app = Arc::new(App {
        ai_thread,
        target,
        hub: Hub { clients: Mutex::new(Vec::new()) },
        console: Mutex::new(Vec::new()),
        last_activity: Mutex::new(Instant::now()),
        auto_green_running: Mutex::new(std::collections::HashSet::new()),
        auto_green_last: Mutex::new(std::collections::HashMap::new()),
        staged: Mutex::new(std::collections::HashMap::new()),
        last_good: Mutex::new(std::collections::HashMap::new()),
        launching: Mutex::new(std::collections::HashMap::new()),
        last_launch_note: Mutex::new(std::collections::HashMap::new()),
    });
    ok(&format!("fusion-dashboard v{} serving on port {} (target {})", VERSION, port,
        if app.target.is_empty() { "auto" } else { &app.target }));

    // DOCK WEBVIEW OWNERSHIP (project-1d dock-bar contract, John-blessed model, 2026-08-23):
    // the dock pre-opens a named tab and hands us its name in ADOM_DOCK_WEBVIEW. Once WE are
    // serving, the tab is OURS: take it over and re-navigate it to our URL on EVERY (re)start,
    // so the dashboard follows our server with no dock involvement (restart the server, the
    // same tab renavigates). Env-gated (skip on a hand-run serve with the var unset), uses the
    // same env-aware URL we print (direct 127.0.0.1 on WSL, VS Code proxy in a cloud container),
    // and best-effort: a missing adom-cli or a closed tab never blocks or crashes serving.
    if let Ok(tab) = std::env::var("ADOM_DOCK_WEBVIEW") {
        if !tab.trim().is_empty() {
            let url = url_for(port);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(800)); // let accept() come up first
                let _ = Command::new("adom-cli")
                    .args(["hydrogen", "webview", "open-or-refresh",
                           "--name", &tab, "--url", &url])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            });
        }
    }

    // 24h idle reaper (app-creator lifecycle): a dock click respawns us, so exiting when
    // nobody has used the app for a day keeps containers free of zombie servers.
    {
        let app = app.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(300));
            let idle = app.last_activity.lock().unwrap().elapsed();
            let unwatched = app.hub.client_count() == 0;
            // SUPERSESSION RETIREMENT: a newer build is serving and nobody is watching me,
            // so step aside instead of squatting a port and answering probes with stale UI.
            // Only ever exits when unwatched AND idle, so a live viewer is never dropped.
            if unwatched && idle > Duration::from_secs(600) {
                if let Some(newer) = newer_version_serving() {
                    println!("OK: v{} is serving and I am v{} with no viewers for {}m - retiring",
                             newer, VERSION, idle.as_secs() / 60);
                    std::process::exit(0);
                }
            }
            if idle > Duration::from_secs(IDLE_REAP_HOURS * 3600) && unwatched {
                println!("OK: idle {}h with no viewers - reaping (dock click restarts me)", IDLE_REAP_HOURS);
                std::process::exit(0);
            }
        });
    }

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let app = app.clone();
        // Thread-per-connection so an open SSE stream never blocks the accept loop.
        std::thread::spawn(move || handle_conn(&app, stream));
    }
}

/// CLI `ui` verbs drive the RUNNING server over HTTP (find it on 8930-8940).
fn ui_call(port_flag: Option<u16>, path: &str, body: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    let ports: Vec<u16> = port_flag.map(|p| vec![p]).unwrap_or((8930..=8940).collect());
    for p in ports {
        let url = format!("http://127.0.0.1:{}{}", p, path);
        let mut cmd = Command::new("curl");
        cmd.args(["-s", "-m", "60"]);
        if let Some(b) = &body {
            cmd.args(["-X", "POST", "-H", "Content-Type: application/json", "-d", &b.to_string()]);
        }
        cmd.arg(&url);
        if let Ok(o) = cmd.output() {
            if o.status.success() && !o.stdout.is_empty() {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&o.stdout) {
                    return Ok(v);
                }
            }
        }
    }
    Err("no fusion-dashboard server answering on 8930-8940".into())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("help");
    let get_flag = |name: &str| -> Option<String> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
    };
    let ai_thread = get_flag("--ai-thread").unwrap_or_else(|| "fusion-dashboard".into());
    let port_flag: Option<u16> = get_flag("--port").and_then(|p| p.parse().ok());
    match cmd {
        "serve" => {
            let target = get_flag("--target").unwrap_or_default();
            let print_url = args.iter().any(|a| a == "--print-url");
            serve(port_flag.unwrap_or(8930), target, ai_thread, print_url);
        }
        "status" => {
            let target = get_flag("--target").unwrap_or_else(default_target);
            println!("{}", serde_json::to_string_pretty(&gather_status(&ai_thread, &target)).unwrap());
        }
        "ui" => {
            let sub = args.get(2).map(|s| s.as_str()).unwrap_or("");
            let res = match sub {
                "toast" => {
                    let msg = args.get(3).cloned().unwrap_or_default();
                    if msg.is_empty() {
                        err_hint("ui toast needs a message", "fusion-dashboard ui toast \"running test 3 of 8\"");
                        std::process::exit(1);
                    }
                    let kind = get_flag("--kind").unwrap_or_else(|| "info".into());
                    ui_call(port_flag, "/ui/toast", Some(serde_json::json!({"message": msg, "kind": kind})))
                }
                "click" => {
                    let target = args.get(3).cloned().unwrap_or_default();
                    if target.is_empty() {
                        err_hint("ui click needs a target", "fusion-dashboard ui click tour   (targets: setup, start, stop, tour, import-part, settings, refresh)");
                        std::process::exit(1);
                    }
                    ui_call(port_flag, "/ui/click", Some(serde_json::json!({"target": target})))
                }
                "state" => ui_call(port_flag, "/state", None),
                "console" => ui_call(port_flag, "/console", None),
                _ => {
                    err_hint("unknown ui verb", "ui toast \"msg\" [--kind ok|err|info] | ui click <target> | ui state | ui console");
                    std::process::exit(1);
                }
            };
            match res {
                Ok(v) => println!("OK: {}", v),
                Err(e) => {
                    err_hint(&e, "Start it first: `fusion-dashboard serve &`");
                    std::process::exit(1);
                }
            }
        }
        "open" => {
            // The founding contract's third verb: put the Dashboard on a surface. --pup (default)
            // opens a BACKGROUND pup window on the user's desktop via ab (never steals focus);
            // --webview prints the URL + the exact instruction for the AI, because Hydrogen
            // panel-id resolution is the webview skill's job, not a CLI guess.
            let mut port = 0u16;
            for p in 8930..=8940u16 {
                if let Ok(o) = Command::new("curl").args(["-s", "-m", "2", &format!("http://127.0.0.1:{}/health", p)]).output() {
                    if o.status.success() && String::from_utf8_lossy(&o.stdout).contains("\"ok\":true") { port = p; break; }
                }
            }
            if port == 0 {
                err_hint("no fusion-dashboard server is running", "Start it first: `fusion-dashboard serve &`, then re-run open.");
                std::process::exit(1);
            }
            let url = match std::env::var("VSCODE_PROXY_URI") {
                Ok(t) if t.contains("{{port}}") => t.replace("{{port}}", &port.to_string()),
                _ => format!("http://127.0.0.1:{}/", port),
            };
            let webview = args.iter().any(|a| a == "--webview");
            if webview {
                ok(&format!("Dashboard is serving at {}", url));
                println!("Hint: to place it in a Hydrogen webview tab, load the `webview` skill (adom-workspace-control) and add a tab for that URL - panel selection is the AI's call, never this CLI's.");
            } else {
                let target = get_flag("--target");
                let mut cmd = Command::new("adom-bridge");
                if let Some(t) = &target { cmd.args(["--target", t]); }
                cmd.args(["--ai-thread", &ai_thread, "pup_open_window",
                          &serde_json::json!({"url": url, "background": true}).to_string()]);
                match cmd.output() {
                    Ok(o) => {
                        let out = String::from_utf8_lossy(&o.stdout);
                        if out.contains("\"status\": \"ok\"") || out.contains("\"status\":\"ok\"") {
                            ok(&format!("Dashboard opened in a BACKGROUND pup window at {} (taskbar flashes; the user brings it up when ready - never foregrounded)", url));
                        } else {
                            err_hint(&format!("pup_open_window did not confirm: {}", out.chars().take(200).collect::<String>()),
                                     "Is a desktop connected? `adom-bridge targets`. The URL above still works in any browser.");
                        }
                    }
                    Err(e) => err_hint(&format!("adom-bridge not runnable: {}", e), "The container ships adom-bridge on PATH."),
                }
            }
        }
        "health" => ok(&format!("fusion-dashboard v{}", VERSION)),
        _ => {
            println!("fusion-dashboard v{} - the fusion bridge's Launch Page", VERSION);
            println!("  serve [--port 8930] [--target <box>] [--print-url]      serve the Dashboard (SSE live channel included)");
            println!("  status [--target <box>]                                 one-shot LED JSON");
            println!("  ui toast \"msg\" [--kind ok|err|info] [--port]            narrate a toast over the running app");
            println!("  ui click <setup|start|stop|tour|import-part|settings|refresh>   drive the real UI");
            println!("  ui state | ui console                                   observe the running app");
            println!("  open [--pup (default) | --webview] [--target <box>]     put the Dashboard on a surface (pup = background window)");
            println!("  health                                                  liveness");
            println!("  (global: --ai-thread <name> attributes bridge calls in the Activity Log)");
            println!("Hint: `fusion-dashboard serve --print-url` is the Hydrogen dock launch step.");
        }
    }
}

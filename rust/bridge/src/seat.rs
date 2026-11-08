//! The Autodesk SEAT (licence) and who Fusion is signed in as.
//!
//! One Autodesk subscription runs ONE active Fusion session at a time. Signing in on a second
//! machine raises "Active Sessions Exceeded"; answering Continue moves the seat there and
//! suspends the first machine RESUMABLY. John moves the seat between ConfRoomROG, arav-rog and
//! AdomLapper constantly while testing, so that move is a first-class verb instead of a recipe:
//! `fusion_account` says who is signed in and whether this box holds the seat, `fusion_claim_seat`
//! moves it here, `fusion_release_seat` gives it up so another box can take it without a dialog.
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Duration, Instant};

fn now_f() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

/// A seat move the AI asked for, so the passive sweep does not toast the user about a surprise.
fn note_intentional_move(state_dir: &Path, what: &str) {
    let _ = std::fs::create_dir_all(state_dir);
    let _ = std::fs::write(state_dir.join("seat-move.json"), json!({"at": now_f(), "what": what}).to_string());
}
/// True when this box claimed or released the seat within `secs`.
pub fn recent_intentional_move(state_dir: &Path, secs: f64) -> bool {
    std::fs::read_to_string(state_dir.join("seat-move.json")).ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["at"].as_f64()).map(|at| now_f() - at < secs).unwrap_or(false)
}

/// Autodesk's licence blob, read through Fusion's own `Application.LicenseInformation`.
fn license_info(pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let r = pass("execute_text_command", json!({"command": "Application.LicenseInformation"}));
    if r["success"].as_bool() != Some(true) { return Value::Null; }
    let raw = r["output"].as_str().map(str::to_string).or_else(|| r["data"].as_str().map(str::to_string)).unwrap_or_default();
    let parsed: Value = serde_json::from_str(raw.trim()).unwrap_or(Value::Null);
    let svc = parsed.as_object().and_then(|m| m.values().next().cloned()).unwrap_or(Value::Null);
    if !svc.is_object() { return Value::Null; }
    let b = |k: &str| svc[k].as_str().map(|s| s == "true").or(svc[k].as_bool());
    json!({"authorized": svc["auth"].as_bool(), "userId": svc["o2id"], "issuedAt": svc["iat"], "expiresAt": svc["exp"],
           "reauthorizeAt": svc["reauthz"], "trial": b(".isTrialLicense"), "temporary": b(".isTempLicense"), "gracePeriod": b(".isGracePeriod")})
}

/// `fusion_account`: who Fusion is signed in as on THIS box, and whether it holds the seat.
pub fn account(ctx: &crate::addin::Ctx, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let running = !ctx.host.fusion_processes().is_empty();
    if !running {
        return json!({"success": true, "running": false, "signedIn": Value::Null, "holdsSeat": false,
            "_hint": "Fusion is not running here, so this box holds no seat. fusion_claim_seat starts it and takes the seat (the other machine suspends resumably)."});
    }
    let ab = fusion_core::ab::Loopback::from_env();
    let facts = ab.as_ref().map(|a| fusion_core::windows_facts::facts_from_ab(a, &ctx.caller)).unwrap_or_default();
    if facts.needs_signin {
        return json!({"success": true, "running": true, "signedIn": false, "needsSignin": true, "holdsSeat": false, "windowTitle": facts.main_title,
            "_hint": "Fusion is at the Autodesk sign-in screen: nobody is signed in on this box. fusion_signin {auto:true, profile:\"<browser>:<email>\"} drives it; a password or 2FA step comes back as signinWall for the user to finish."});
    }
    let who = pass("run_modeling_script", json!({"script": "import adsk.core\napp = adsk.core.Application.get()\nu = app.currentUser\nresult = {\"email\": u.email if u else None, \"name\": u.displayName if u else None, \"userId\": u.userId if u else None, \"hub\": app.data.activeHub.name if app.data.activeHub else None, \"offline\": app.isOffLine}"}));
    let r = if who["data"]["result"].is_object() { who["data"]["result"].clone() } else { who["result"].clone() };
    let lic = license_info(pass);
    // "Holds the seat" is only ever a LOCAL reading: Autodesk hands each machine a licence token
    // that stays valid until its own reauthorize time, so two boxes can both answer authorized
    // for a while after a move (seen live, ConfRoomROG and arav-rog, 2026-09-23). Signed in +
    // authorized + no seat modal is the strongest signal one box has; the box that LOST the seat
    // shows the modal (seatDialogOpen) or stops being authorized at its next reauthorize.
    let holds = r["email"].is_string() && lic["authorized"].as_bool() != Some(false) && facts.licensing_dialog.is_none();
    json!({"success": true, "running": true, "signedIn": r["email"].is_string(), "account": r, "license": lic,
        "holdsSeat": holds, "seatDialogOpen": facts.licensing_dialog.is_some(), "windowTitle": facts.main_title,
        "_hint": if holds { "This box is signed in and licensed right now (the strongest local reading of 'holds the seat'; Autodesk lets a just-moved box stay authorized until its next reauthorize, so ask both boxes before concluding). Another box's fusion_claim_seat takes the seat from here." }
                 else { "Signed in, but this box does not hold the seat right now (another machine has it, or the seat modal is up). fusion_claim_seat takes it." }})
}

/// `fusion_claim_seat`: make THIS box the active Fusion seat.
pub fn claim_seat(ctx: &crate::addin::Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let t0 = Instant::now();
    let mut steps = Vec::new();
    note_intentional_move(&ctx.state_dir, "claim");
    if ctx.host.fusion_processes().is_empty() {
        let r = crate::launch::start(ctx, &json!({}));
        steps.push(json!({"step": "start Fusion", "ok": r["success"].as_bool() != Some(false)}));
        if r["success"].as_bool() == Some(false) { return json!({"success": false, "errorCode": "start_failed", "steps": steps, "error": r["error"]}); }
    } else { steps.push(json!({"step": "Fusion already running", "ok": true})); }
    // the seat modal is the transfer itself: Continue takes the seat here and suspends the other box
    let mut resolved = json!({"clicks": 0});
    for _ in 0..3 {
        let ab = fusion_core::ab::Loopback::from_env();
        let facts = ab.as_ref().map(|a| fusion_core::windows_facts::facts_from_ab(a, &ctx.caller)).unwrap_or_default();
        if facts.licensing_dialog.is_none() { break; }
        resolved = crate::readiness_exec::resolve_seat_dialog_verb(ctx, &json!({}));
        if resolved["errorCode"] == "needs_approval" {
            return json!({"success": false, "errorCode": "needs_approval", "steps": steps, "holdsSeat": false, "error": resolved["error"], "tellUser": resolved["tellUser"],
                "_hint": "The approval prompt is up. When the user has clicked Approve, call fusion_claim_seat again."});
        }
        steps.push(json!({"step": "answered the Autodesk seat modal (Continue: this box takes the seat)", "clicks": resolved["clicks"]}));
        std::thread::sleep(Duration::from_secs(2));
    }
    // after the seat modal Fusion resumes for a few seconds, and a read in that window sees no user
    // yet (ConfRoomROG, 2026-09-23: the claim had worked, the reply said it had not)
    let mut acct = account(ctx, pass);
    for _ in 0..6 {
        if acct["signedIn"].as_bool() == Some(true) || acct["needsSignin"].as_bool() == Some(true) { break; }
        std::thread::sleep(Duration::from_secs(5));
        acct = account(ctx, pass);
    }
    if acct["needsSignin"].as_bool() == Some(true) {
        let mut a = acct.clone();
        a["success"] = json!(false); a["errorCode"] = json!("needs_signin"); a["steps"] = json!(steps);
        a["error"] = json!("Fusion on this box is at the Autodesk sign-in screen, so there is no seat to claim yet.");
        a["_hint"] = json!(format!("Sign this box in first: fusion_signin {{auto:true{}}}. Then fusion_claim_seat. A password or 2FA step comes back as signinWall for the user to finish; the bridge never types a credential.", args["profile"].as_str().map(|p| format!(", profile:\"{p}\"")).unwrap_or_default()));
        return a;
    }
    json!({"success": acct["holdsSeat"].as_bool() == Some(true), "steps": steps, "seatDialogAnswered": resolved["clicks"],
        "account": acct["account"], "license": acct["license"], "holdsSeat": acct["holdsSeat"], "durationMs": t0.elapsed().as_millis() as u64,
        "seatSweepWarning": "The box you took it FROM keeps its own passive takeover sweep: if its Fusion is still open it can grab the seat straight back. fusion_release_seat there (or fusion_set_seat_takeover {enabled:false}) before a long test run.",
        "_hint": "This box now holds the Autodesk seat; any other machine signed into the same account suspended RESUMABLY (its documents are kept, it resumes when it takes the seat back). fusion_account on the other box confirms it lost the seat."})
}

/// `fusion_release_seat`: give up the seat from THIS box so another can take it cleanly.
pub fn release_seat(ctx: &crate::addin::Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    note_intentional_move(&ctx.state_dir, "release");
    if ctx.host.fusion_processes().is_empty() {
        return json!({"success": true, "released": true, "wasRunning": false, "_hint": "Fusion was not running here, so this box held no seat. Claim it on the box you want with fusion_claim_seat."});
    }
    let before = account(ctx, pass);
    let stop = crate::lifecycle::stop(ctx, &json!({"save": args["save"].as_bool().unwrap_or(false)}));
    if stop["success"].as_bool() == Some(false) {
        let mut s = stop.clone();
        s["_hint"] = json!("Fusion would not close cleanly, so the seat is still held here. Read the error: an unsaved document usually needs fusion_save_to_cloud or an explicit close first. Nothing was discarded.");
        s["heldBy"] = before["account"].clone();
        return s;
    }
    json!({"success": true, "released": true, "wasRunning": true, "wasHeldBy": before["account"], "stopped": stop["steps"],
        "_hint": "Fusion is closed here, so the seat is free: the next box's fusion_claim_seat takes it with no seat modal at all. Nothing was signed out, so this box claims it straight back later."})
}

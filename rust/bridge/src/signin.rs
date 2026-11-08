//! `fusion_signin`, `fusion_signin_2fa`, `fusion_web_signin`: sign Fusion in through the RIGHT
//! browser profile. Fusion fires its OAuth at the OS-default browser; the full authorize URL
//! lands in that browser's History, and completing it in ANY profile hands the token back to
//! the running Fusion (idmgr callback + autodesk:// handoff matched by request_id).
use crate::addin::Ctx;
use fusion_core::ab::{AbClient, Loopback};
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const AUTODESK_SIGNIN_HOST: &str = "signin.autodesk.com";
const SSO_BUTTONS: &[(&str, &str)] = &[("google", "Continue with Google"), ("apple", "Continue with Apple"), ("microsoft", "Continue with Microsoft"), ("facebook", "Continue with Facebook")];
const CONSUMER: &[&str] = &["gmail.com", "outlook.com", "hotmail.com", "yahoo.com", "icloud.com", "live.com"];
/// Sign-in windows this bridge opened: {sessionId, profile, kind, openedAt}.
pub static SIGNIN_WINDOWS: Mutex<Vec<Value>> = Mutex::new(Vec::new());

/// Everything the sign-in needs from the bridge around it.
pub struct Env<'a> { pub readiness: &'a dyn Fn() -> Value, pub start: &'a dyn Fn() -> Value, pub aps_state: &'a dyn Fn() -> (bool, bool), pub aps_signin: &'a dyn Fn(&str) -> Value }

fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }
fn sleep_s(s: f64) { std::thread::sleep(Duration::from_secs_f64(s)); }
fn ab_call(ctx: &Ctx, verb: &str, args: Value, timeout: u64) -> Value {
    match Loopback::from_env() { Some(ab) => match ab.call(verb, args, &ctx.caller, timeout) { Ok(v) => v, Err(e) => json!({"_adCallError": format!("{e:?}")}) }, None => json!({"_adCallError": "ab loopback unavailable"}) }
}
/// `_unwrap`: ab responses come back flat or nested under `data`; look through both.
pub fn unwrap(r: &Value) -> Value { match r["data"].as_object() { Some(o) if !o.is_empty() => r["data"].clone(), _ => if r.is_object() { r.clone() } else { json!({}) } } }
/// `_find_coordmap`: a coordMap (with shotId) anywhere in a verb response.
pub fn find_coordmap(v: &Value, depth: u8) -> Value {
    if depth > 5 { return json!({}); }
    if let Some(s) = v.as_str() { if s.starts_with('{') || s.starts_with('[') { if let Ok(p) = serde_json::from_str::<Value>(s) { return find_coordmap(&p, depth + 1); } } return json!({}); }
    if let Some(o) = v.as_object() {
        if o.get("coordMap").map(|c| c.is_object() && !c["shotId"].is_null()).unwrap_or(false) { return o["coordMap"].clone(); }
        if o.get("shotId").map(|s| !s.is_null()).unwrap_or(false) { return v.clone(); }
        for k in ["data", "output", "result", "response"] { if let Some(x) = o.get(k) { let got = find_coordmap(x, depth + 1); if got.as_object().map(|m| !m.is_empty()).unwrap_or(false) { return got; } } }
    }
    json!({})
}
fn windows(ctx: &Ctx) -> Vec<Value> { unwrap(&ab_call(ctx, "desktop_list_windows", json!({}), 40))["windows"].as_array().cloned().unwrap_or_default() }
fn truthy(v: &Value) -> bool { v.as_bool() == Some(true) || v.as_str() == Some("ok") }
/// `_announce_foreground`: tell the user WHY, right before stealing their foreground (a caption).
pub fn announce_foreground(ctx: &Ctx, reason: &str) -> bool { ab_call(ctx, "desktop_caption", json!({"text": format!("Adom: {reason}"), "id": "fusion-bridge-foreground", "expiresInMs": 2500}), 15)["_adCallError"].is_null() }

/// `_detect_signin_wall`: what the Autodesk sign-in is waiting on, from BROWSER WINDOW TITLES.
/// `ignore` = windows that existed before we opened this sign-in (stale tabs lie).
pub fn detect_signin_wall(ctx: &Ctx, ignore: &[i64]) -> Value {
    let mut best = json!({});
    for w in windows(ctx) {
        if w["hwnd"].as_i64().map(|h| ignore.contains(&h)).unwrap_or(false) { continue; }
        let t = w["title"].as_str().unwrap_or("").to_string(); let tl = t.to_lowercase();
        if tl.contains("2-step verification") || tl.contains("confirm sign-in") { return json!({"wall": "twofactor_email_code", "hwnd": w["hwnd"], "title": t}); }
        if tl.contains("identity manager") { best = json!({"wall": "protocol_handoff", "hwnd": w["hwnd"], "title": t}); }
        else if tl.contains("sign in - autodesk") && best["wall"].is_null() { best = json!({"wall": "credentials", "hwnd": w["hwnd"], "title": t}); }
    }
    best
}
/// `_click_background_first`: UIA Invoke (no focus steal) first; SendInput only with an announcement.
pub fn click_background_first(ctx: &Ctx, hwnd: i64, name: Option<&str>, shot: Option<(&Value, i64, i64)>, reason: Option<&str>) -> Value {
    if let Some(n) = name {
        let r = unwrap(&ab_call(ctx, "desktop_ui_click", json!({"hwnd": hwnd, "name": n}), 30));
        if truthy(&r["ok"]) || truthy(&r["clicked"]) || truthy(&r["invoked"]) { return json!({"clicked": true, "via": "uia_background", "announced": false, "why": format!("UIA Invoke on {} (background, no focus steal)", fusion_lbr::py_repr_str(n))}); }
    }
    if let Some((sid, x, y)) = shot {
        let announced = announce_foreground(ctx, reason.map(str::to_string).unwrap_or_else(|| format!("clicking {} for you", name.unwrap_or("a button"))).as_str());
        let _ = ab_call(ctx, "desktop_click", json!({"space": "image", "shotId": sid, "x": x, "y": y, "hwnd": hwnd}), 30);
        return json!({"clicked": true, "via": "foreground_click", "announced": announced, "why": "no UIA node; used SendInput (DOES foreground - user was told why)"});
    }
    json!({"clicked": false, "via": null, "announced": false, "why": "no UIA node and no shot coords given"})
}
/// The sign-in METHODS Autodesk offers. John signs into Fusion with Google; other people use a
/// password or another provider, so the method is an input, remembered per user, never a guess.
pub const METHODS: &[&str] = &["google", "apple", "microsoft", "facebook", "password"];

fn method_pref_path(state_dir: &std::path::Path) -> std::path::PathBuf { state_dir.join("signin-method.json") }
pub fn remembered_method(state_dir: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(method_pref_path(state_dir)).ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["method"].as_str().map(str::to_string))
}
fn remember_method(state_dir: &std::path::Path, method: &str, account: Option<&str>) {
    let _ = std::fs::create_dir_all(state_dir);
    let _ = std::fs::write(method_pref_path(state_dir), json!({"method": method, "account": account}).to_string());
}

/// nbrowser_eval answers {data:{result}} or a JSON string in `output` ({ok, result}); read either.
fn eval_result(r: &Value) -> Value {
    for c in [&r["data"]["result"], &r["result"], &r["data"]["value"], &r["value"]] { if !c.is_null() { return c.clone(); } }
    if let Some(o) = r["output"].as_str() { if let Ok(v) = serde_json::from_str::<Value>(o) { if !v["result"].is_null() { return v["result"].clone(); } } }
    Value::Null
}

/// Read the sign-in page through the browser extension (a window the BRIDGE opened, so it is
/// drivable): what stage it is at, which methods it offers, which accounts a chooser lists.
fn probe_page(ctx: &Ctx, session: &str) -> Value {
    let js = r#"(() => { const t = e => (e.innerText || e.textContent || '').trim();
      const els = [...document.querySelectorAll('button,a,[role=button]')];
      const labels = els.map(t).filter(Boolean);
      const methods = labels.filter(x => /^continue with /i.test(x)).map(x => x.replace(/^continue with /i, '').toLowerCase());
      const accounts = [...document.querySelectorAll('[data-identifier],li,div')].map(t).filter(x => /@/.test(x) && x.length < 120).slice(0, 10);
      return { url: location.href, title: document.title, methods, accounts,
               hasPassword: !!document.querySelector('input[type=password]'),
               hasEmail: !!document.querySelector('input[type=email],input[name=username],input[name=UserName]'),
               openProduct: labels.some(x => /open product/i.test(x)),
               signedIn: /you're signed in|youre signed in/i.test(document.body.innerText || '') };
    })()"#;
    let r = ab_call(ctx, "nbrowser_eval", json!({"sessionId": session, "expression": js}), 30);
    let v = eval_result(&r);
    if v.is_object() { v } else { Value::Null }
}

trait TakeIfObject { fn take_if_object(self) -> Option<Value>; }
impl TakeIfObject for Value { fn take_if_object(self) -> Option<Value> { if self.is_object() { Some(self) } else { None } } }

/// Click an element by its visible TEXT in the extension-driven window (no coordinates, no OCR).
fn click_text(ctx: &Ctx, session: &str, needle: &str) -> bool {
    // Actionable elements only (a matched wrapper <div> swallows the click: the 1.11.1 loop clicked
    // one ten times on AdomLapper and the page never moved). Innermost match wins, then the click
    // goes to its closest button/link/row, and the answer says whether the page actually changed.
    let js = format!(r#"(() => {{ const n = {needle:?}.toLowerCase();
      const vis = e => e.offsetParent !== null || e.getClientRects().length > 0;
      const txt = e => (e.innerText || e.textContent || '').trim().toLowerCase();
      const exact = n.includes('@') ? document.querySelector('[data-identifier="' + n + '"]') : null;
      const cands = exact ? [exact] : [...document.querySelectorAll('button,a,[role=button],[role=link],[data-identifier],li')].filter(e => vis(e) && txt(e).includes(n));
      if (!cands.length) return {{ clicked: false }};
      cands.sort((a, b) => txt(a).length - txt(b).length);
      const el = cands[0].closest('button,a,[role=button],[role=link],li') || cands[0];
      el.click();
      return {{ clicked: true, tag: el.tagName }}; }})()"#);
    let r = ab_call(ctx, "nbrowser_eval", json!({"sessionId": session, "expression": js}), 30);
    eval_result(&r)["clicked"].as_bool().unwrap_or(false)
}

/// Drive the Autodesk sign-in page with the METHOD the user signs in with. Returns a typed
/// outcome; it never types a password and never guesses which method or which account.
pub fn drive_with_method(ctx: &Ctx, session: &str, method: Option<&str>, account: Option<&str>) -> Value {
    let mut steps: Vec<String> = Vec::new();
    for _ in 0..10 {
        let p = probe_page(ctx, session);
        if !p.is_object() { return json!({"stage": "unreadable", "steps": steps, "_hint": "The sign-in page could not be read through the browser extension. Fall back to the window on screen."}); }
        let methods: Vec<String> = p["methods"].as_array().cloned().unwrap_or_default().iter().filter_map(|m| m.as_str().map(str::to_string)).collect();
        if p["signedIn"].as_bool() == Some(true) || p["openProduct"].as_bool() == Some(true) {
            let clicked = click_text(ctx, session, "open product");
            steps.push(format!("signed in; clicked Open Product: {clicked}"));
            return json!({"stage": "signed_in", "steps": steps, "openProductClicked": clicked,
                "_hint": "Autodesk says signed in and the handoff to Fusion is running. Chrome may ask 'Open Autodesk Identity Manager?': tick Always allow and click Open (desktop_ui_click, or desktop_click on the dialog). Then poll fusion_account until it reports the account."});
        }
        if p["hasPassword"].as_bool() == Some(true) {
            return json!({"stage": "password_required", "steps": steps, "method": "password",
                "_hint": "The page is on the PASSWORD step. The bridge never types a password. Either the user types it in the window, or a stored credential fills it (pup_login / nbrowser_autologin on a profile that has it). If the user signs in with Google instead, call fusion_signin {method:\"google\"} and the bridge takes the Google route."});
        }
        // a Google/Apple/Microsoft account chooser
        if p["url"].as_str().map(|u| u.contains("accounts.google.com") || u.contains("login.microsoftonline") || u.contains("appleid.apple.com")).unwrap_or(false) {
            let accounts: Vec<String> = p["accounts"].as_array().cloned().unwrap_or_default().iter().filter_map(|a| a.as_str().map(str::to_string)).collect();
            let Some(want) = account else {
                return json!({"stage": "account_choice_required", "accounts": accounts, "steps": steps,
                    "_hint": "The provider is asking WHICH account. Ask the user, then call fusion_signin {method, account:\"<email>\"}; the bridge remembers both."});
            };
            if click_text(ctx, session, want) { steps.push(format!("picked account {want}")); std::thread::sleep(std::time::Duration::from_secs(5)); continue; }
            return json!({"stage": "account_not_listed", "accounts": accounts, "wanted": want, "steps": steps,
                "_hint": "That account is not signed into this browser profile, so the provider cannot offer it. Either the user signs into it in that browser once, or pick one of accounts[]."});
        }
        // the Autodesk method page
        let Some(m) = method else {
            return json!({"stage": "method_required", "methods": if methods.is_empty() { json!(METHODS) } else { json!(methods) }, "steps": steps,
                "_hint": "ASK THE USER which sign-in method this Autodesk account uses (Google, Apple, Microsoft, Facebook or a password), then call fusion_signin {method:\"<it>\", account:\"<email>\"}. The bridge remembers it, so every later box just works."});
        };
        if m == "password" {
            if let Some(a) = account { let _ = click_text(ctx, session, "email"); steps.push(format!("password route for {a}")); }
            let _ = click_text(ctx, session, "next");
            std::thread::sleep(std::time::Duration::from_secs(4));
            continue;
        }
        let label = format!("continue with {m}");
        if steps.iter().filter(|x| x.as_str() == format!("clicked '{label}'")).count() >= 2 {
            return json!({"stage": "click_did_not_advance", "methods": methods, "wanted": m, "steps": steps,
                "_hint": "The provider button was clicked twice and the page did not move. Read the window (fusion_state / a screenshot); the browser may be showing its own prompt."});
        }
        if click_text(ctx, session, &label) { steps.push(format!("clicked '{label}'")); std::thread::sleep(std::time::Duration::from_secs(4)); continue; }
        return json!({"stage": "method_not_on_page", "methods": methods, "wanted": m, "steps": steps,
            "_hint": "That method is not offered on this page. methods[] is what Autodesk shows; ask the user which one to use."});
    }
    json!({"stage": "gave_up", "steps": steps, "_hint": "The sign-in page did not reach a signed-in state within the bridge's attempts. Read fusion_state and the window on screen."})
}

/// `_advance_credentials_page`: drive the Autodesk credentials page as far as code can go
/// WITHOUT secrets: set the email, invoke Next, classify what the page became. Never a password.
fn advance_credentials_page(ctx: &Ctx, hwnd: i64, email: Option<&str>) -> Value {
    let mut out = json!({"advanced": false, "wall": "credentials", "detail": ""});
    let found = crate::etiquette::find_controls_any(&ab_call(ctx, "desktop_find_control", json!({"hwnd": hwnd, "role": "edit"}), 40), 0);
    let mut ctrls = found["controls"].as_array().cloned().unwrap_or_default();
    if ctrls.is_empty() && found["best"].is_object() { ctrls = vec![found["best"].clone()]; }
    let nm = |c: &Value| format!("{}{}", c["name"].as_str().unwrap_or(""), c["automationId"].as_str().unwrap_or("")).to_lowercase();
    if ctrls.iter().any(|c| nm(c).contains("password")) { out["wall"] = json!("password"); out["detail"] = json!("password field present - the human step"); return out; }
    let emailish: Vec<Value> = { let e: Vec<Value> = ctrls.iter().filter(|c| ["email", "username", "user id", "userid"].iter().any(|k| nm(c).contains(k))).cloned().collect(); if e.is_empty() { ctrls.iter().take(1).cloned().collect() } else { e } };
    if let (Some(email), Some(c)) = (email, emailish.first()) {
        let r = unwrap(&ab_call(ctx, "desktop_ui_set", json!({"hwnd": hwnd, "automationId": c["automationId"], "name": c["name"], "text": email, "value": email}), 30));
        if truthy(&r["success"]) || truthy(&r["ok"]) {
            let mut detail = format!("typed email {} into {} (background UIA)", fusion_lbr::py_repr_str(email), fusion_lbr::py_repr_str(c["name"].as_str().or(c["automationId"].as_str()).unwrap_or("None")));
            for btn in ["Next", "NEXT", "Sign in", "Sign In", "Continue"] {
                let rb = unwrap(&ab_call(ctx, "desktop_ui_click", json!({"hwnd": hwnd, "name": btn}), 25));
                if truthy(&rb["success"]) || truthy(&rb["ok"]) || truthy(&rb["status"]) { out["advanced"] = json!(true); detail += &format!("; invoked {}", fusion_lbr::py_repr_str(btn)); break; }
            }
            if out["advanced"].as_bool() == Some(true) {
                sleep_s(4.0);
                let f2 = unwrap(&ab_call(ctx, "desktop_find_control", json!({"hwnd": hwnd, "role": "edit"}), 40));
                let mut c2 = f2["controls"].as_array().or(f2["matches"].as_array()).cloned().unwrap_or_default();
                if c2.is_empty() && f2["best"].is_object() { c2 = vec![f2["best"].clone()]; }
                if c2.iter().any(|c| nm(c).contains("password")) { out["wall"] = json!("password"); detail += "; page advanced to the PASSWORD step"; }
            }
            out["detail"] = json!(detail);
        }
    }
    out
}
fn resolve_and_advance_wall(ctx: &Ctx, mut wall: Value, email: Option<&str>) -> Value {
    if wall["wall"].as_str() == Some("credentials") { if let Some(h) = wall["hwnd"].as_i64() {
        let adv = advance_credentials_page(ctx, h, email);
        if adv["wall"].is_string() { wall["wall"] = adv["wall"].clone(); }
        wall["advance"] = adv["detail"].clone(); wall["advanced"] = adv["advanced"].clone();
    } }
    wall
}
fn wall_copy(wall: &str) -> (&'static str, &'static str) {
    match wall {
        "twofactor_email_code" => ("Autodesk needs your 6-digit code", "Autodesk emailed a verification code to finish signing Fusion in. Enter it in the Chrome window, or tell your AI to fetch it for you."),
        "credentials" => ("Autodesk sign-in needs you", "The Autodesk sign-in is asking for credentials. I never type your password or 2FA - please complete it in the Chrome window."),
        "password" => ("Just your password, everything else is done", "Adom drove the Fusion sign-in all the way: email entered, page advanced. The ONLY step left is your password (and I never type secrets). Type it in the browser window and everything continues automatically."),
        "protocol_handoff" => ("Allow Fusion to finish signing in", "Chrome is asking permission to open Autodesk Identity Manager. Click Open to hand the login back to Fusion."),
        _ => ("Fusion sign-in needs you", "Please check the sign-in window."),
    }
}
/// `_notify_signin_wall`: toast the user's real desktop about a human wall; true when delivered.
fn notify_signin_wall(ctx: &Ctx, wall: &str) -> bool {
    let (t, b) = wall_copy(wall);
    crate::small_verbs::notify_owner(&ctx.state_dir, &json!({"title": t, "body": b, "level": "warning"}), &ctx.caller)["success"].as_bool() == Some(true)
}
/// `_wall_hint`: the AI-facing instruction for a wall (do as much as possible IN CODE).
pub fn wall_hint(wall: &str, notified: bool) -> String {
    let common = format!("I ALREADY TOASTED THE USER about this (notifyDelivered={}) - you do not need to ask me to, and do not nag them again. ", if notified { "True" } else { "False" });
    let rest = match wall {
        "twofactor_email_code" => "WALL: Autodesk emailed a 6-DIGIT CODE. THE USER WANTS YOU TO DO THIS FOR THEM if you can, so TRY THIS FIRST, do not just hand it back to them: if `adom-google` is available in YOUR container, read the code out of their Gmail and submit it -   adom-google gmail search 'from:autodesk verification code' --limit 1 \nthen pull the 6 digits and call  fusion_signin_2fa {\"code\":\"123456\"}  which types it in the BACKGROUND (no foreground steal) and submits. If `adom-google` is NOT installed, TELL THE USER that installing adom-google would let you finish logins like this automatically next time, then let them type the code. NEVER ask them for the code if you can read it yourself.",
        "password" => "WALL: PASSWORD ONLY. The bridge already typed the user's email and advanced the page (see steps[]) - the single remaining human act is typing their password in the browser window. Do NOT ask them for the password, do NOT try to type it, and do NOT re-drive the page. After they type it, the flow continues automatically - poll fusion_readiness until ready:true (a 2FA wall may follow; if it does you may be able to finish it yourself via fusion_signin_2fa + adom-google Gmail).",
        "protocol_handoff" => "WALL: Chrome's native 'Open Autodesk Identity Manager?' dialog. Try to clear it YOURSELF in the background: desktop_find_control on that dialog then desktop_ui_click {name:'Open'} (UIA, no focus steal). Only if UIA finds no node, fall back to a foregrounded desktop_click. Then poll fusion_readiness.",
        _ => "WALL: the sign-in wants credentials. NEVER type their password or 2FA. If the browser autofills, you may submit it. Otherwise let the user finish in the window I opened.",
    };
    common + rest
}
/// `_describe_profile`: a human, UNAMBIGUOUS name for a browser profile, never "your browser".
pub fn describe_profile(p: &Value) -> String {
    let b = p["browser"].as_str().unwrap_or("browser").trim().to_string();
    let browser = match b.to_lowercase().as_str() { "chrome" => "Chrome".to_string(), "edge" => "Edge".to_string(), "brave" => "Brave".to_string(), _ => { let mut c = b.chars(); match c.next() { Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(), None => b.clone() } } };
    let name = p["displayName"].as_str().unwrap_or("").trim(); let email = p["email"].as_str().unwrap_or("").trim();
    if !name.is_empty() && !email.is_empty() && !email.to_lowercase().contains(&name.to_lowercase()) { format!("{browser} - {name} ({email})") }
    else if !email.is_empty() { format!("{browser} - {email}") } else if !name.is_empty() { format!("{browser} - {name}") }
    else if let Some(d) = p["profileDir"].as_str() { format!("{browser} - {d}") } else { browser }
}
fn signin_cfg_path() -> std::path::PathBuf { let d = dirs_home().join(".adom").join("fusion-signin"); let _ = std::fs::create_dir_all(&d); d.join("profile.json") }
fn dirs_home() -> std::path::PathBuf { std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from(".")) }
fn remember_signin_profile(p: &str) { let _ = std::fs::write(signin_cfg_path(), json!({"profile": p}).to_string()); }
pub fn remembered_signin_profile_pub() -> Option<String> { remembered_signin_profile() }
fn remembered_signin_profile() -> Option<String> { std::fs::read_to_string(signin_cfg_path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["profile"].as_str().filter(|s| !s.is_empty()).map(str::to_string)) }
pub fn track_signin_window(session: &str, profile: &str, kind: &str) { if !session.is_empty() { SIGNIN_WINDOWS.lock().unwrap().push(json!({"sessionId": session, "profile": profile, "kind": kind, "openedAt": now()})); } }

/// `_chrome_authorize_urls`: recent Autodesk-desktop OAuth authorize URLs from every Chrome and
/// Edge profile's History (copied first; Chrome keeps it locked). Newest first.
pub fn chrome_authorize_urls(max_age_min: u64) -> Vec<Value> {
    let Some(la) = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from) else { return vec![] };
    let roots = [("chrome", la.join("Google").join("Chrome").join("User Data")), ("edge", la.join("Microsoft").join("Edge").join("User Data"))];
    let t = now();
    let mut found = Vec::new();
    for (browser, root) in roots {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let hist = e.path().join("History");
            if !hist.is_file() { continue; }
            let prof = e.file_name().to_string_lossy().to_string();
            let tmp = std::env::temp_dir().join(format!("adom_hist_{browser}_{prof}.db"));
            if std::fs::copy(&hist, &tmp).is_err() { continue; }
            for ext in ["-wal", "-shm"] { let side = hist.with_file_name(format!("History{ext}")); if side.exists() { let _ = std::fs::copy(&side, tmp.with_file_name(format!("adom_hist_{browser}_{prof}.db{ext}"))); } }
            let Ok(con) = rusqlite::Connection::open(&tmp) else { continue };
            let _ = con.execute_batch("PRAGMA journal_mode=WAL");
            let rows: Vec<(String, i64)> = match con.prepare("SELECT url, last_visit_time FROM urls WHERE url LIKE '%developer.api.autodesk.com/authentication/v2/authorize%' OR url LIKE '%idp.auth.autodesk.com/as/authorize%' ORDER BY last_visit_time DESC LIMIT 6") {
                Ok(mut st) => st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).map(|it| it.flatten().collect()).unwrap_or_default(),
                Err(_) => continue,
            };
            for (url, cts) in rows {
                if !url.contains("idsdk") && !url.contains("idmgr") { continue; }
                let epoch = (cts as f64 / 1_000_000.0) - 11644473600.0;
                let age = t - epoch;
                if age <= (max_age_min * 60) as f64 { found.push(json!({"url": url, "sourceProfileDir": prof, "browser": browser, "ageSec": age as i64})); }
            }
        }
    }
    found.sort_by_key(|x| x["ageSec"].as_i64().unwrap_or(0));
    found
}
fn capture_fresh_authorize(wait_sec: u64, max_age: i64) -> Option<Value> {
    let deadline = Instant::now() + Duration::from_secs(wait_sec.max(1));
    loop {
        let urls = chrome_authorize_urls(15);
        if let Some(u) = urls.first() { if u["ageSec"].as_i64().unwrap_or(0) <= max_age { return Some(u.clone()); } }
        if Instant::now() >= deadline { return urls.first().cloned(); }
        sleep_s(2.0);
    }
}
/// `_fusion_window`: the main Fusion window row (prefers the sign-in/welcome window).
fn fusion_window(ctx: &Ctx) -> Value {
    let mut cand = json!({});
    for w in windows(ctx) {
        let tl = w["title"].as_str().unwrap_or("").to_lowercase();
        if tl.contains("autodesk fusion") || tl.starts_with("signing in") {
            if tl.contains("signing in") || tl.contains("welcome") { return w; }
            if cand.as_object().map(|o| o.is_empty()).unwrap_or(true) { cand = w; }
        }
    }
    cand
}
/// `_fusion_click_signin`: click Fusion's Welcome -> Sign In button ON THE USER'S BEHALF, always.
fn fusion_click_signin(ctx: &Ctx, env: &Env) -> Value {
    let win = fusion_window(ctx);
    let hwnd = win["hwnd"].as_i64().or_else(|| unwrap(&(env.readiness)())["hwnd"].as_i64());
    let Some(hwnd) = hwnd else { return json!({"clicked": false, "why": "could not find the Fusion window"}) };
    let rect = &win["rect"];
    let (wd, ht) = (rect["width"].as_i64().unwrap_or(0), rect["height"].as_i64().unwrap_or(0));
    if rect["left"].as_i64().map(|l| l <= -30000).unwrap_or(false) || (0 < wd && wd < 600) || (0 < ht && ht < 400) {
        let _ = ab_call(ctx, "desktop_set_window_state", json!({"hwnd": hwnd, "state": "restore"}), 30);
        sleep_s(1.5);
    }
    let raw = ab_call(ctx, "desktop_screenshot_window", json!({"hwnd": hwnd}), 45);
    let shot = unwrap(&raw);
    let cm = find_coordmap(&raw, 0);
    if cm["shotId"].is_null() {
        if let (Some(l), Some(t)) = (rect["left"].as_i64(), rect["top"].as_i64()) { if wd > 400 && ht > 300 {
            let (sx, sy) = (l + wd / 2, (t as f64 + ht as f64 * 0.57) as i64);
            let rr = unwrap(&ab_call(ctx, "desktop_click", json!({"x": sx, "y": sy, "hwnd": hwnd, "reason": "signing Fusion in for you (clicking Sign In, screen-space fallback)"}), 30));
            let okc = truthy(&rr["success"]) || truthy(&rr["ok"]) || truthy(&rr["status"]);
            return json!({"clicked": okc, "why": format!("screen-space fallback click at ({sx},{sy}) rect {wd}x{ht}: {}", if okc { "ok".to_string() } else { rr["error"].to_string().chars().take(80).collect() }), "via": "screen-fallback"});
        } }
        return json!({"clicked": false, "why": format!("screenshot returned no shotId (hwnd={hwnd} err={})", raw["error"].to_string().chars().take(100).collect::<String>())});
    }
    let img = &cm["image"];
    let w = shot["width"].as_f64().or(img["w"].as_f64()).or(img["width"].as_f64()).or(cm["imageWidth"].as_f64()).unwrap_or(1400.0);
    let h = shot["height"].as_f64().or(img["h"].as_f64()).or(img["height"].as_f64()).or(cm["imageHeight"].as_f64()).unwrap_or(860.0);
    let r = click_background_first(ctx, hwnd, Some("Sign In"), Some((&cm["shotId"], (w * 0.5) as i64, (h * 0.57) as i64)), Some("signing Fusion in for you (clicking Sign In)"));
    json!({"clicked": r["clicked"], "why": format!("{} (image {}x{})", r["why"].as_str().unwrap_or(""), w as i64, h as i64), "via": r["via"]})
}
/// `_drive_sso_signin`: finish the Autodesk sign-in over CDP using SSO - no password, no 2FA,
/// no foreground. Must be a FRESH flow, and the provider is clicked BEFORE any email is typed.
fn drive_sso_signin(ctx: &Ctx, profile: &str, tab: &Value, email: Option<&str>, provider: &str) -> Value {
    let mut steps: Vec<String> = Vec::new();
    let click = |text: &str| -> bool { let r = unwrap(&ab_call(ctx, "nbrowser_click", json!({"profile": profile, "tabId": tab, "text": text}), 45)); truthy(&r["ok"]) || truthy(&r["success"]) || truthy(&r["changed"]) };
    let btn = SSO_BUTTONS.iter().find(|b| b.0 == provider.to_lowercase()).map(|b| b.1).unwrap_or("Continue with Google");
    if !click(btn) { return json!({"done": false, "steps": steps, "why": format!("{} not on the page - this is probably NOT a fresh flow (a resumed flowId goes straight to the password screen).", fusion_lbr::py_repr_str(btn))}); }
    steps.push(format!("clicked {}", fusion_lbr::py_repr_str(btn)));
    sleep_s(7.0);
    if let Some(e) = email { if click(e) { steps.push(format!("picked the {e} account")); sleep_s(8.0); } }
    let ev = unwrap(&ab_call(ctx, "nbrowser_eval", json!({"profile": profile, "tabId": tab, "expression": "JSON.stringify({u:location.href.slice(0,120),b:document.body.innerText.slice(0,200)})"}), 45));
    let state = ev["result"].as_str().or(ev["value"].as_str()).map(str::to_string).unwrap_or_else(|| ev["result"].to_string());
    if state.contains("You're signed in") || state.to_lowercase().contains("signed in") {
        steps.push("provider returned 'You're signed in'".into());
        if click("Open Product") { steps.push("clicked 'Open Product' to hand the token to Fusion".into()); }
        return json!({"done": true, "steps": steps, "state": state.chars().take(200).collect::<String>()});
    }
    json!({"done": false, "steps": steps, "state": state.chars().take(200).collect::<String>(), "why": "did not reach the signed-in page; a provider password/2FA may be required"})
}

/// `fusion_signin`.
pub fn signin(ctx: &Ctx, args: &Value, env: &Env) -> Value {
    // The sign-in METHOD is an input, remembered per user (John signs in with Google). With it,
    // the bridge drives the page itself; without it, the page answers method_required and the AI
    // asks the user once. No toast until a HUMAN step is actually reached (John, 2026-09-23).
    let method: Option<String> = args["method"].as_str().map(|m| m.trim().to_lowercase()).filter(|m| METHODS.contains(&m.as_str())).or_else(|| remembered_method(&ctx.state_dir));
    let account_arg: Option<String> = args["account"].as_str().map(|a| a.trim().to_string()).filter(|a| a.contains('@'));
    if let Some(m) = args["method"].as_str().map(|m| m.trim().to_lowercase()).filter(|m| METHODS.contains(&m.as_str())) { remember_method(&ctx.state_dir, &m, account_arg.as_deref()); }
    let mut steps: Vec<String> = Vec::new();
    let auto = args["auto"].as_bool().unwrap_or(true);
    let wait_sec = args["waitSec"].as_u64().filter(|w| *w > 0).unwrap_or(32);
    let mut target: Option<String> = args["profile"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    if let Some(t) = &target { remember_signin_profile(t); }
    if target.is_none() { target = remembered_signin_profile(); }
    let mut rd = (env.readiness)();
    if rd["running"].as_bool() != Some(true) {
        steps.push("Fusion was not running - launching it (background) as part of sign-in".into());
        let s = (env.start)();
        if s["success"].as_bool() == Some(false) { steps.push(format!("launch failed: {}", s["error"].to_string().chars().take(80).collect::<String>())); }
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline { sleep_s(8.0); rd = (env.readiness)(); if rd["running"].as_bool() == Some(true) { break; } }
        if rd["running"].as_bool() != Some(true) {
            return json!({"success": false, "stage": "launch_failed", "done": false, "steps": steps, "error": "Fusion did not come up within 120s of launch.", "_hint": "Check fusion_readiness; the box may be mid-update. Retry fusion_signin.", "statusVerb": "fusion_readiness"});
        }
        sleep_s(5.0);
    }
    if rd["needsSignin"].as_bool() != Some(true) {
        return json!({"success": true, "stage": "already", "done": true, "steps": steps, "narrate": "Fusion is already signed in.", "_hint": format!("Already signed in (ready:{}). Nothing to do.", if rd["ready"].as_bool() == Some(true) { "True" } else { "False" }), "statusVerb": "fusion_signin"});
    }
    let profs = ab_call(ctx, "nbrowser_profiles", json!({}), 40);
    let pdata = if profs["data"].is_object() { profs["data"].clone() } else { profs.clone() };
    let mut choices: Vec<Value> = pdata["profiles"].as_array().cloned().unwrap_or_default().into_iter().filter(|p| p.is_object() && p["profile"].as_str().map(|s| !s.is_empty()).unwrap_or(false)).collect();
    let mut urls = chrome_authorize_urls(15);
    let fresh = urls.first().filter(|u| u["ageSec"].as_i64().unwrap_or(999) <= 150).cloned();
    if auto && fresh.is_none() {
        let ck = fusion_click_signin(ctx, env);
        steps.push(if ck["clicked"].as_bool() == Some(true) { "clicked Fusion 'Sign In' for the user".to_string() } else { format!("could not click Fusion 'Sign In' ({})", ck["why"].as_str().unwrap_or("")) });
        urls = capture_fresh_authorize(wait_sec, 140).map(|g| vec![g]).unwrap_or_default();
    }
    let Some(auth) = urls.first().cloned() else {
        return json!({"success": true, "stage": "click_signin", "done": false, "steps": steps, "narrate": "I clicked Fusion's Sign In for you, but Fusion has not written its sign-in URL to a browser yet. Give it a few seconds.",
            "_hint": "THIS VERB CLICKS FUSION'S 'Sign In' FOR THE USER - never tell them to go click it themselves, and do not click it yourself. It just did (see steps[]), but no Fusion OAuth URL has landed in any browser's history yet. Simply CALL fusion_signin AGAIN in a few seconds; it will click if needed, wait on-box for the URL, and re-open it in the right profile. If steps[] says the click FAILED, that is a bridge bug worth reporting - the usual causes are a minimized Fusion window (this verb restores it) or the sign-in wall not being up yet (check fusion_readiness needsSignin:true).", "statusVerb": "fusion_signin"});
    };
    let age = auth["ageSec"].as_i64().unwrap_or(0);
    let src_dir = auth["sourceProfileDir"].as_str().unwrap_or("").to_string();
    steps.push(format!("captured Fusion OAuth URL from {} profile '{}' ({}s old)", auth["browser"].as_str().unwrap_or(""), src_dir, age));
    if age > 150 && method.is_some() && args["_freshRetry"].is_null() {
        // A stale request cannot be finished. At the sign-in screen Fusion holds no documents, so
        // restarting it for a fresh request loses nothing: do it once instead of handing the user
        // a recipe (AdomLapper, 2026-09-23: a 580 s old request after an earlier attempt).
        let signed_out = crate::lifecycle::dialog_inventory(ctx).is_empty() && Loopback::from_env().map(|a| fusion_core::windows_facts::facts_from_ab(&a, &ctx.caller).needs_signin).unwrap_or(false);
        if signed_out {
            let st = crate::lifecycle::stop(ctx, &json!({}));
            // nothing is open at the sign-in screen, so a force close loses nothing when the clean close is refused
            if st["success"].as_bool() == Some(false) { let _ = crate::lifecycle::kill(ctx, &json!({})); }
            std::thread::sleep(Duration::from_secs(3));
            let _ = (env.start)();
            let mut a2 = args.clone(); if !a2.is_object() { a2 = json!({}); }
            a2["_freshRetry"] = json!(true);
            let mut r = signin(ctx, &a2, env);
            if let Some(arr) = r["stepsBefore"].as_array_mut() { arr.insert(0, json!(format!("the pending sign-in request was {age}s old (expired); restarted Fusion at its sign-in screen for a fresh one"))); }
            return r;
        }
    }
    if age > 150 {
        return json!({"success": true, "stage": "stale", "done": false, "steps": steps, "signinAuthUrl": auth["url"], "signinAgeSec": age,
            "_hint": format!("The newest Fusion sign-in URL is {age}s old - Fusion expires the request in ~2 min, so completing it yields 'Sign-in request expired' and NO handoff. Get a FRESH request: fusion_stop then fusion_start (resets Fusion to a clean 'Welcome to Fusion'), click Fusion's Sign In, and call fusion_signin again PROMPTLY. Completion is near-instant when the target profile is already signed into Autodesk (auto-consents). Skill: fusion-multiprofile-signin."), "statusVerb": "fusion_signin"});
    }
    let mut reason = String::new();
    // The account the user asked for decides the browser profile: the profile SIGNED INTO that
    // identity already holds its Google/Microsoft session, so the provider step needs no password.
    // Picking "a profile with an Autodesk session" instead chose John's personal Chrome on
    // AdomLapper, where the work account's Google session had lapsed (2026-09-23).
    if target.is_none() {
        if let Some(a) = &account_arg {
            if let Some(c) = choices.iter().find(|c| c["email"].as_str().map(|e| e.eq_ignore_ascii_case(a)).unwrap_or(false)) {
                target = c["profile"].as_str().map(str::to_string); reason = format!("its browser profile is signed into {a}");
            }
        }
    }
    if target.is_none() {
        for c in choices.iter_mut() {
            if c["live"].as_bool() != Some(true) { continue; }
            let ls = ab_call(ctx, "nbrowser_login_state", json!({"profile": c["profile"], "url": "https://accounts.autodesk.com/"}), 40);
            let lsd = if ls["data"].is_object() { ls["data"].clone() } else { ls.clone() };
            c["_ad"] = json!(if lsd["loggedIn"].as_bool() == Some(true) { "signed-in" } else if !lsd["ok"].is_null() { "signed-out" } else { "unprobed" });
            c["_conf"] = lsd["confidence"].clone(); c["_cookies"] = lsd["cookieCount"].clone();
        }
        let mut signed: Vec<Value> = choices.iter().filter(|c| c["_ad"].as_str() == Some("signed-in")).cloned().collect();
        if !signed.is_empty() {
            signed.sort_by_key(|c| std::cmp::Reverse((match c["_conf"].as_str() { Some("high") => 3, Some("medium") => 2, Some("low") => 1, _ => 0 }, c["_cookies"].as_i64().unwrap_or(0))));
            target = signed[0]["profile"].as_str().map(str::to_string); reason = "it holds a live Autodesk session".into();
        } else if let Some(w) = choices.iter().find(|c| c["live"].as_bool() == Some(true) && c["email"].as_str().map(|e| !e.is_empty() && !CONSUMER.iter().any(|d| e.to_lowercase().ends_with(&format!("@{d}")))).unwrap_or(false)) {
            target = w["profile"].as_str().map(str::to_string); reason = "it is a work/corporate profile (no live Autodesk session detected - CONFIRM)".into();
        }
    }
    if let Some(t) = &target { remember_signin_profile(t); if reason.is_empty() { reason = "you told me to use it".into(); } }
    let Some(target) = target else {
        return json!({"success": true, "stage": "ask_profile", "done": false, "steps": steps, "signinAuthUrl": auth["url"], "profileChoices": choices,
            "_hint": "Captured Fusion's OAuth URL but cannot tell which profile is your Autodesk account. ASK the user which profile their Autodesk login belongs to, then call fusion_signin {profile:'chrome:<their-account>'} - I will remember it. profileChoices lists every profile.", "statusVerb": "fusion_signin"});
    };
    let chosen = choices.iter().find(|c| c["profile"].as_str() == Some(&target)).cloned();
    let tdesc = chosen.as_ref().map(describe_profile).unwrap_or_else(|| target.clone());
    let sess = format!("fusion-signin-{}", now() as i64);
    let target_dir = chosen.as_ref().and_then(|c| c["profileDir"].as_str().or(c["dir"].as_str()).map(str::to_string));
    let email = args["email"].as_str().map(str::to_string).or_else(|| chosen.as_ref().and_then(|c| c["email"].as_str().filter(|e| !e.is_empty()).map(str::to_string)));
    if let Some(td) = &target_dir { if method.is_none() && !src_dir.is_empty() && td.trim().to_lowercase() == src_dir.trim().to_lowercase() {
        steps.push(format!("Fusion already opened the sign-in in {} (profile dir {}) - no re-open needed", target, fusion_lbr::py_repr_str(td)));
        let mut wall0 = detect_signin_wall(ctx, &[]);
        if wall0["wall"].is_string() { wall0 = resolve_and_advance_wall(ctx, wall0, email.as_deref()); }
        let notified0 = wall0["wall"].as_str().map(|w| notify_signin_wall(ctx, w)).unwrap_or(false);
        let wh = wall0["wall"].as_str().map(|w| wall_hint(w, notified0));
        return json!({"success": true, "stage": "already_right_profile", "done": false, "steps": steps, "signinProfile": target, "signinProfileDir": td, "signinWhere": target, "signinWall": wall0["wall"], "signinWallHwnd": wall0["hwnd"], "notifyDelivered": notified0, "signinWallHint": wh, "signinAuthUrl": auth["url"],
            "narrate": format!("Fusion opened its sign-in in the RIGHT profile already ({target}), so I did not open a second window."),
            "_hint": format!("NO re-open was needed - Fusion's OS-default browser IS the user's Autodesk profile here (dir {}). Do NOT open another window; that just litters their screen with a duplicate sign-in. The existing window is the one to finish. {}", fusion_lbr::py_repr_str(td), wh.unwrap_or_else(|| "Poll fusion_readiness until ready:true.".into())), "statusVerb": "fusion_readiness"});
    } }
    let pre: Vec<i64> = windows(ctx).iter().filter(|w| { let t = w["title"].as_str().unwrap_or("").to_lowercase(); t.contains("autodesk") && !t.contains("fusion") }).filter_map(|w| w["hwnd"].as_i64()).collect();
    let open_args = json!({"sessionId": sess, "url": auth["url"], "background": true, "profile": target, "thread": "fusion-signin", "purpose": "Fusion Autodesk sign-in (correct profile)"});
    let mut r = ab_call(ctx, "nbrowser_open_window", open_args.clone(), 60);
    let rd_open = if r["data"].is_object() { r["data"].clone() } else { r.clone() };
    let mut opened = !rd_open["sessionId"].is_null() || truthy(&r["ok"]) || truthy(&r["success"]) || truthy(&rd_open["opened"]) || r["_hint"].as_str().unwrap_or("").contains("opened in the BACKGROUND");
    let mut open_via: Option<&str> = if opened { Some("extension") } else { None };
    if !opened {
        let r2 = ab_call(ctx, "nbrowser_open_os_window", open_args, 60);
        if r2["_adCallError"].is_null() && (truthy(&r2["ok"]) || !r2["hwnd"].is_null() || r2["_hint"].as_str().unwrap_or("").contains("Launch fired")) { opened = true; open_via = Some("extension_free"); r = r2; }
    }
    steps.push(if opened { format!("re-opened it in {}{}", tdesc, if open_via == Some("extension_free") { " (extension-free)" } else { "" }) } else { format!("could not open {} (raw: {})", tdesc, r.to_string().chars().take(160).collect::<String>()) });
    if opened { track_signin_window(&sess, &target, "fusion"); }
    if opened && open_via == Some("extension") {
        std::thread::sleep(std::time::Duration::from_secs(3));
        let acct = account_arg.clone().or_else(|| email.clone());
        let out = drive_with_method(ctx, &sess, method.as_deref(), acct.as_deref());
        let stage = out["stage"].as_str().unwrap_or("").to_string();
        // AI-answerable stages come back WITHOUT a toast; only a real human step toasts
        let human = stage == "password_required";
        let notified = if human { notify_signin_wall(ctx, if stage == "password_required" { "password" } else { "credentials" }) } else { false };
        if stage != "unreadable" {
            let mut r = out.clone();
            r["success"] = json!(stage == "signed_in" || !human);
            r["done"] = json!(stage == "signed_in");
            r["method"] = json!(method); r["account"] = json!(acct);
            r["signinProfile"] = json!(target); r["signinSession"] = json!(sess); r["notifyDelivered"] = json!(notified);
            r["stepsBefore"] = json!(steps);
            return r;
        }
        steps.push("the extension could not read the page; falling back to the window on screen".into());
    }
    if opened && open_via == Some("extension") && args["driveSso"].as_bool().unwrap_or(true) {
        let tabs = unwrap(&ab_call(ctx, "nbrowser_list_tabs", json!({"profile": target}), 45))["tabs"].as_array().cloned().unwrap_or_default();
        let my_tab = tabs.iter().filter(|t| t["url"].as_str().unwrap_or("").to_lowercase().contains("autodesk") && t["owner"].as_str() == Some("agent-opened")).last().map(|t| if t["tabId"].is_null() { t["id"].clone() } else { t["tabId"].clone() });
        if let Some(tab) = my_tab {
            let sso = drive_sso_signin(ctx, &target, &tab, email.as_deref(), args["ssoProvider"].as_str().unwrap_or("google"));
            for s in sso["steps"].as_array().cloned().unwrap_or_default() { if let Some(x) = s.as_str() { steps.push(x.to_string()); } }
            if sso["done"].as_bool() == Some(true) {
                for _ in 0..6 {
                    sleep_s(5.0);
                    if unwrap(&(env.readiness)())["ready"].as_bool() == Some(true) {
                        steps.push("Fusion reports ready:true - SIGNED IN".into());
                        let (conf, signed) = (env.aps_state)();
                        if conf && !signed {
                            let _ = (env.aps_signin)(&target);
                            steps.push("APS not signed in - started its consent on the SAME warm SSO session (no second login for the user)".into());
                            for _ in 0..5 { sleep_s(4.0); if (env.aps_state)().1 { steps.push("APS signed in too - one login covered both".into()); break; } }
                        }
                        let (conf, signed) = (env.aps_state)();
                        return json!({"success": true, "stage": "signed_in", "done": true, "steps": steps, "signinProfile": target, "signinWhere": tdesc, "signinOpenedVia": open_via, "aps": {"configured": conf, "signedIn": signed},
                            "narrate": format!("Fusion is signed in as {}. No password or 2FA was needed - I used the browser profile's existing SSO session.{}", email.clone().unwrap_or_else(|| target.clone()), if signed { " APS cloud search is set up too, from the same login." } else { "" }),
                            "_hint": format!("DONE - Fusion is signed in (ready:true). Nothing else to do; do not tell the user to sign in. {} Drive Fusion normally now.", if signed { "APS is signed in as well, so fusion_aps_search is live." } else { "APS is NOT signed in - run fusion_aps_signin NOW while the browser SSO session is still warm so it consents silently." }), "statusVerb": "fusion_readiness"});
                    }
                }
            }
        }
    }
    let mut wall = if opened { detect_signin_wall(ctx, &pre) } else { json!({}) };
    if wall["wall"].is_string() { wall = resolve_and_advance_wall(ctx, wall, email.as_deref()); }
    let wall_kind = wall["wall"].as_str().map(str::to_string);
    let mut notified = false;
    if let Some(wk) = &wall_kind { notified = notify_signin_wall(ctx, wk); steps.push(format!("hit {wk} wall; toasted the user (delivered={})", if notified { "True" } else { "False" })); }
    if !opened {
        let mut live = detect_signin_wall(ctx, &[]);
        if live["wall"].is_string() {
            live = resolve_and_advance_wall(ctx, live, args["email"].as_str());
            let wk = live["wall"].as_str().unwrap_or("").to_string();
            let notified_fb = if wk.is_empty() { false } else { notify_signin_wall(ctx, &wk) };
            let adv = live["advance"].as_str().map(str::to_string).unwrap_or_else(|| format!("wall {}", fusion_lbr::py_repr_str(&wk)));
            steps.push(format!("no extension on this box - drove the EXISTING browser window instead: {adv}"));
            return json!({"success": true, "stage": "driving_existing_window", "done": false, "steps": steps, "signinWall": wk, "signinWallHwnd": live["hwnd"], "advanced": live["advanced"], "notifyDelivered": notified_fb, "signinWallHint": wall_hint(&wk, notified_fb), "signinAuthUrl": auth["url"],
                "narrate": format!("This machine has no Adom browser extension, so I drove the sign-in window Fusion already opened: {adv}."),
                "_hint": format!("No ABE here, so the EXISTING browser window was driven directly (email typed via background UIA where possible). {}", wall_hint(&wk, notified_fb)), "statusVerb": "fusion_readiness"});
        }
    }
    json!({"success": true, "stage": if opened { "opened" } else { "open_failed" }, "done": false, "steps": steps, "signinWhere": tdesc, "signinProfile": target, "signinOpenedVia": open_via, "signinWall": wall_kind, "signinWallHwnd": wall["hwnd"], "notifyDelivered": notified,
        "signinWallHint": wall_kind.as_ref().map(|w| wall_hint(w, notified)), "signinProfileReason": reason, "signinSession": if opened { json!(sess) } else { Value::Null }, "signinWrongBrowser": format!("{} / {}", auth["browser"].as_str().unwrap_or(""), src_dir), "signinAuthUrl": auth["url"],
        "narrate": if opened { format!("Fusion opened its sign-in in the WRONG browser ({src_dir}), so I moved the real Autodesk sign-in URL into {tdesc} - I picked that because {reason}. Finish it there, or I can drive it, and Fusion will pick up the login automatically.") } else { "I captured Fusion's sign-in URL but could not open the target profile.".to_string() },
        "_hint": if opened { format!("The REAL Fusion OAuth URL is now open in {tdesc} (session 'fusion-signin'). It completes back to the running Fusion via the idmgr/callback + autodesk:// protocol handoff (request_id match), so the browser profile no longer has to be the OS default - THIS is the fix for Fusion signing into the wrong account. TELL the user the exact profile (never 'your browser'), say WHY it was chosen (signinProfileReason), and offer: they finish it, you foreground it, or (with their OK) you drive 'Continue with Google/Apple/Microsoft'. NEVER type their password/2FA. Then poll fusion_readiness until ready:true. The wrong-browser tab ({src_dir}) can be closed. If the wrong ACCOUNT completes anyway, sign Fusion out and re-run. Skill: fusion-multiprofile-signin.") } else { format!("Open failed: {}. Retry, or open auth url yourself with nbrowser_open_window {{profile:'{}', url:<signinAuthUrl>}}.", r["_adCallError"].as_str().unwrap_or("None"), target) },
        "statusVerb": "fusion_signin"})
}

/// `fusion_signin_2fa`: submit Autodesk's emailed code IN THE BACKGROUND (UIA SetValue + Invoke).
pub fn signin_2fa(ctx: &Ctx, args: &Value) -> Value {
    let code = args["code"].as_str().map(str::to_string).or_else(|| args["code"].as_i64().map(|c| c.to_string())).unwrap_or_default();
    let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 4 {
        return json!({"success": false, "errorCode": "bad_code", "error": "Pass the emailed code, e.g. fusion_signin_2fa {\"code\":\"123456\"}.", "_hint": "Read it from the user's Gmail if you can: adom-google gmail search 'from:autodesk verification code' --limit 1 -> take the 6 digits -> call this verb again. If adom-google is not installed, tell the user it would let you automate this.", "statusVerb": "fusion_signin_2fa"});
    }
    let wall = detect_signin_wall(ctx, &[]);
    let Some(hwnd) = args["hwnd"].as_i64().or_else(|| wall["hwnd"].as_i64()) else {
        return json!({"success": false, "errorCode": "no_2fa_window", "error": "No Autodesk 2-step-verification window found.", "_hint": "Nothing is waiting on a code right now. Check fusion_readiness - if needsSignin is still true, run fusion_signin again to restart the flow.", "statusVerb": "fusion_readiness"});
    };
    let found = unwrap(&ab_call(ctx, "desktop_find_control", json!({"hwnd": hwnd, "role": "edit"}), 40));
    let ctrls = found["controls"].as_array().or(found["matches"].as_array()).cloned().unwrap_or_default();
    let mut steps: Vec<String> = Vec::new();
    let mut filled = false;
    if ctrls.len() >= digits.len() && ctrls.len() >= 4 {
        for (i, ch) in digits.chars().take(ctrls.len()).enumerate() { let _ = ab_call(ctx, "desktop_ui_set", json!({"hwnd": hwnd, "automationId": ctrls[i]["automationId"], "name": ctrls[i]["name"], "text": ch.to_string(), "value": ch.to_string()}), 20); }
        filled = true; steps.push(format!("typed {} digits into {} segmented boxes (background UIA)", digits.len(), ctrls.len()));
    } else if let Some(c) = ctrls.first() {
        let r = unwrap(&ab_call(ctx, "desktop_ui_set", json!({"hwnd": hwnd, "automationId": c["automationId"], "name": c["name"], "text": digits, "value": digits}), 20));
        filled = truthy(&r["ok"]) || truthy(&r["set"]) || r.is_object();
        steps.push(format!("typed the code into {} (background UIA)", fusion_lbr::py_repr_str(c["name"].as_str().or(c["automationId"].as_str()).unwrap_or("None"))));
    }
    if !filled {
        return json!({"success": false, "errorCode": "code_field_not_found", "error": "Could not find the code field via UIA.", "controlsSeen": ctrls.iter().take(8).collect::<Vec<_>>(),
            "_hint": format!("The code input was not in the UIA tree (shadow DOM). Run desktop_find_control {{hwnd:{hwnd}}} to see what IS exposed. Last resort is a FOREGROUNDED desktop_click on the field then desktop_type - avoid that if you can, foregrounding is disruptive to the user."), "statusVerb": "fusion_signin_2fa"});
    }
    let mut sub = click_background_first(ctx, hwnd, Some("Next"), None, None);
    if sub["clicked"].as_bool() != Some(true) { sub = click_background_first(ctx, hwnd, Some("Verify"), None, None); }
    steps.push(format!("submitted via {}", sub["via"].as_str().unwrap_or("no submit control found")));
    json!({"success": true, "submitted": true, "steps": steps, "narrate": "I read the code in and submitted it for you, without touching your screen.",
        "_hint": "Code submitted in the BACKGROUND. Now poll fusion_readiness until ready:true. If a Chrome 'Open Autodesk Identity Manager?' dialog appears, clear it with desktop_ui_click {name:'Open'} (background) - fusion_signin reports that wall too. If it says the code was wrong, the email may have a NEWER code.", "statusVerb": "fusion_readiness"})
}

/// `fusion_web_signin`: sign a pup window into an Autodesk web surface (Google SSO by default);
/// never guesses a credential.
pub fn web_signin(ctx: &Ctx, args: &Value) -> Value {
    let url = args["url"].as_str().unwrap_or("").trim().to_string();
    if url.is_empty() { return json!({"success": false, "error": "url is required (e.g. https://library.io/ or https://<hub>.autodesk360.com/)"}); }
    let sess = args["sessionId"].as_str().filter(|s| !s.is_empty()).unwrap_or("autodesk-web").to_string();
    let provider = args["provider"].as_str().filter(|s| !s.is_empty()).unwrap_or("google").to_lowercase();
    let user = args["username"].as_str().unwrap_or("").to_string();
    let mut steps: Vec<Value> = Vec::new();
    let pup = |verb: &str, a: Value, t: u64| ab_call(ctx, &format!("pup_{verb}"), a, t);
    let state = || { let r = pup("eval", json!({"sessionId": sess, "expression": "({url:location.href,body:document.body.innerText.replace(/\\s+/g,' ').slice(0,400)})"}), 60); if r["result"].is_object() { r["result"].clone() } else { json!({}) } };
    let _ = pup("open_window", json!({"sessionId": sess, "url": url, "reason": args["reason"].as_str().unwrap_or("sign in to an Autodesk web surface")}), 120);
    sleep_s(8.0);
    let st = state(); steps.push(json!({"step": "open", "url": st["url"].as_str().unwrap_or("").chars().take(120).collect::<String>()}));
    let body = st["body"].as_str().unwrap_or("").to_string();
    if !st["url"].as_str().unwrap_or("").contains(AUTODESK_SIGNIN_HOST) && !body.contains("Sign in to Autodesk") {
        return json!({"success": true, "alreadySignedIn": true, "sessionId": sess, "steps": steps, "_hint": "That surface was already authenticated in this pup profile."});
    }
    if provider == "google" {
        let r = pup("eval", json!({"sessionId": sess, "expression": "(()=>{const b=[...document.querySelectorAll('button,a,div[role=button]')].find(e=>/Continue with Google/i.test(e.textContent));if(!b)return {found:false};b.click();return {clicked:true};})()"}), 60);
        steps.push(json!({"step": "google-sso", "result": r["result"]}));
        sleep_s(9.0);
        let st = state();
        if !st["url"].as_str().unwrap_or("").contains("accounts.google.com") {
            return json!({"success": false, "errorCode": "google_sso_unavailable", "steps": steps, "state": st["body"].as_str().unwrap_or("").chars().take(200).collect::<String>(), "_hint": "The page did not hand off to Google. Retry with provider:'password'."});
        }
    }
    let lg = pup("login", json!({"sessionId": sess, "url": if provider == "google" { "https://accounts.google.com/".to_string() } else { url.clone() }, "username": if user.is_empty() { Value::Null } else { json!(user) }, "reason": "sign the user in so Adom can drive their Autodesk web surfaces"}), 120);
    steps.push(json!({"step": "vault-login", "used": lg["used"], "ok": lg["success"]}));
    sleep_s(8.0);
    let st = state();
    let at = st["url"].as_str().unwrap_or("").to_string(); let body = st["body"].as_str().unwrap_or("").to_string();
    if at.contains("accounts.google.com") || at.contains(AUTODESK_SIGNIN_HOST) {
        let bl = body.to_lowercase();
        let needs_2fa = ["verify it", "2-step", "verification", "authenticator", "code was sent"].iter().any(|k| bl.contains(k));
        return json!({"success": false, "errorCode": if needs_2fa { "second_factor_required" } else { "human_credential_required" }, "sessionId": sess, "steps": steps, "at": at.chars().take(120).collect::<String>(), "page": body.chars().take(200).collect::<String>(),
            "error": if needs_2fa { "Google is asking for the second factor, which only the user can supply." } else { "Stopped at the sign-in page: pup has no stored credential for this user" },
            "_hint": "ONE-TIME setup, run by the USER so no password ever reaches the AI:\n  adom-bridge credential_set '{\"host\":\"accounts.google.com\",\"username\":\"<you>@<domain>\",\"password\":\"<your Google password>\"}'\nthen re-run this verb. For 2FA, approve the prompt on your phone; the session then persists in pup's profile and later runs are silent."});
    }
    json!({"success": true, "signedIn": true, "sessionId": sess, "steps": steps, "at": at.chars().take(120).collect::<String>(), "_hint": "Signed in. The session persists in pup's profile, so library.io, the hub portal and Autodesk Account are now drivable for this user."})
}

const JANITOR_MAX_AGE: f64 = 900.0;
/// `_signin_janitor_loop` (the sign-in half): poll until each tracked sign-in window can be
/// closed, then close it. `ready` = Fusion ready:true; `aps_signed` = APS signedIn.
pub fn start_janitor(ready: std::sync::Arc<dyn Fn() -> bool + Send + Sync>, aps_signed: std::sync::Arc<dyn Fn() -> bool + Send + Sync>, caller: fusion_core::ab::Caller) {
    std::thread::spawn(move || loop {
        sleep_s(20.0);
        let pending = SIGNIN_WINDOWS.lock().unwrap().clone();
        if pending.is_empty() { continue; }
        let mut goals: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
        for w in pending {
            let kind = w["kind"].as_str().unwrap_or("fusion").to_string();
            let met = *goals.entry(kind.clone()).or_insert_with(|| if kind == "aps" { aps_signed() } else { ready() });
            let aged = now() - w["openedAt"].as_f64().unwrap_or(0.0) > JANITOR_MAX_AGE;
            if met || aged {
                let closed = Loopback::from_env().map(|ab| ab.call("nbrowser_close_window", json!({"sessionId": w["sessionId"], "profile": w["profile"]}), &caller, 30).is_ok()).unwrap_or(false);
                if closed { eprintln!("[fusion-bridge] janitor: closed {} sign-in window {} ({})", kind, w["sessionId"], if met { "signed in" } else { "expired" }); }
                SIGNIN_WINDOWS.lock().unwrap().retain(|x| x["sessionId"] != w["sessionId"]);
            }
        }
    });
}

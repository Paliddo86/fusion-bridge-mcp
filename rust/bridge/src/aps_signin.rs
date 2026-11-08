//! `fusion_aps_signin`, `fusion_aps_set_client_id`, `fusion_aps_set/get/forget_browser`: the
//! bridge's own 3-legged PKCE OAuth against APS (public client, no secret). A localhost
//! listener on one of the registered callback ports captures the code and exchanges it.
use crate::addin::Ctx;
use fusion_core::ab::{AbClient, Loopback};
use fusion_core::aps::{ApsStore, APS_BASE, REDIRECT_PATH, REDIRECT_PORTS, SCOPES, TOKEN_URL, all_registered_callbacks};
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;

const ABE_UPSELL: &str = "OPTIONAL, offer it: installing the Adom browser extension lets me DRIVE this sign-in instead of asking the user to click (fill, read the result, screenshot, clean up). ABE can install it for them automatically - it is just slow, roughly a 5-minute AI-driven process. See the 'installing-the-extension' flow. Everything here works WITHOUT it; the extension only buys automation.";

/// The sign-in in flight: {verifier, state, redirectUri, done, error, startedAt}.
pub static SIGNIN_STATE: Mutex<Option<Value>> = Mutex::new(None);
/// The listener's life: ten minutes (the Python bridge gave three, which a human at a
/// password prompt on another machine kept missing; the ports are only bound meanwhile).
pub const LISTENER_SECONDS: f64 = 600.0;
pub fn signin_pending() -> bool { SIGNIN_STATE.lock().unwrap().as_ref().map(|s| s["done"].as_bool() != Some(true) && now() - s["startedAt"].as_f64().unwrap_or(0.0) < LISTENER_SECONDS).unwrap_or(false) }
fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }
fn redirect_uri(port: u16) -> String { format!("http://localhost:{port}{REDIRECT_PATH}") }
fn ab_call(ctx: &Ctx, verb: &str, args: Value, timeout: u64) -> Value {
    match Loopback::from_env() { Some(ab) => match ab.call(verb, args, &ctx.caller, timeout) { Ok(v) => v, Err(e) => json!({"_adCallError": format!("{e:?}")}) }, None => json!({"_adCallError": "ab loopback unavailable"}) }
}
fn b64url(b: &[u8]) -> String { use base64::Engine; base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b) }
fn pkce_pair() -> (String, String) {
    use sha2::Digest;
    let mut raw = [0u8; 48]; let _ = getrandom::getrandom(&mut raw);
    let verifier = b64url(&raw);
    let challenge = b64url(&sha2::Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}
fn urlencode_form(pairs: &[(&str, &str)]) -> String { pairs.iter().map(|(k, v)| format!("{}={}", fusion_core::aps::urlencode(k), fusion_core::aps::urlencode(v))).collect::<Vec<_>>().join("&") }

/// `_exchange_code`: the authorization code for tokens, saved with obtained_at/expires_at.
fn exchange_code(store: &ApsStore, code: &str, verifier: &str, redirect: &str) -> Value {
    let body = urlencode_form(&[("grant_type", "authorization_code"), ("code", code), ("client_id", &store.client_id()), ("redirect_uri", redirect), ("code_verifier", verifier)]);
    let (status, text) = match ureq::post(TOKEN_URL).set("Content-Type", "application/x-www-form-urlencoded").timeout(Duration::from_secs(30)).send_string(&body) {
        Ok(r) => { let s = r.status(); (s, r.into_string().unwrap_or_default()) }
        Err(ureq::Error::Status(c, r)) => (c, r.into_string().unwrap_or_default()),
        Err(e) => (0, format!("{{\"error\": {}}}", Value::String(e.to_string()))),
    };
    let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
    if status == 200 && parsed["access_token"].as_str().map(|s| !s.is_empty()).unwrap_or(false) {
        let mut t = parsed; let n = now() as u64;
        t["obtained_at"] = json!(n); t["expires_at"] = json!(n + t["expires_in"].as_u64().unwrap_or(3600) - 60);
        store.save_tokens(&t);
        return json!({"ok": true});
    }
    json!({"ok": false, "status": status, "body": parsed})
}

/// `_probe_authorize_auth001`: does /authorize answer AUTH-001 before any login? A healthy app
/// redirects to login; a broken one returns the JSON. False on any ambiguity.
fn probe_auth001(auth_url: &str) -> bool {
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(8)).build();
    let body = match agent.get(auth_url).call() {
        Ok(r) => { if (300..400).contains(&r.status()) { return false; } r.into_string().unwrap_or_default() }
        Err(ureq::Error::Status(c, r)) => { if matches!(c, 301 | 302 | 303 | 307 | 308) { return false; } r.into_string().unwrap_or_default() }
        Err(_) => return false,
    };
    body.contains("AUTH-001") || body.to_lowercase().contains("does not have access to the api product")
}
pub fn auth001_recovery_hint(client_id: &str) -> String {
    format!("⚠ APS AUTH-001 - the APS app (Client ID {}...) LOST its Data Management API product, so authorize fails BEFORE any login. This is fixable; here is the exact recovery (this one genuinely needs ABE/nbrowser: recovery CLICKS and EVALs inside the portal, which the extension-free nbrowser_open_os_window cannot do - it only opens a URL. If the extension is not installed, install adom-browser-extension first; ABE can do it automatically, roughly a 5-minute AI-driven process. Alternatively walk the USER through steps 1-3 by hand): (1) nbrowser_open_window {{sessionId, thread, purpose, profile:<the warm Edge/Chrome profile>, url:'https://aps.autodesk.com/myapps/'}} and open the app whose Client ID is this one. (2) On the app's App-settings page, API Access section: confirm 'Data Management API' is checked, then FORCE a re-commit - nbrowser_eval a click on the VISIBLE 'Save changes' button (filter buttons by /save changes/i AND offsetParent!==null; the bare text matches ~9 hidden duplicates so text-click is ambiguous). If Save is disabled, toggle any one unneeded API off first to dirty the form. (3) WAIT ~1-3 min for Autodesk to propagate. (4) Retry fusion_aps_signin; authorize now reaches the CONSENT screen ('<app> is requesting permission ... View/Manage/Write your data ... Allow') - nbrowser_navigate the session to the new authUrl, then nbrowser_click 'Allow'; the URL flips to localhost:<port>/fusion-aps/callback?code=... and the bridge captures the token. (5) Poll fusion_aps_status until signedIn:true. Verified live 2026-07-14. ⚠ FIRST CHECK before driving anything: open myapps AS THIS USER - if it shows 'It seems you don't have a hub yet', this Autodesk account is NOT in the APS developer hub that owns the app, the app is invisible to it, and NO portal driving from this account can fix AUTH-001 (dead-ended live 2026-08-21, adom-bridge#93). Route the fix to the app's owner (the developer-hub admin), and have them add this user to the developer hub so the next AUTH-001 is self-servable. Meanwhile the library mint works WITHOUT APS since 1.9.133 (the add-in saves the f3d straight to the cloud); APS stays needed only for cloud search and thumbnail verification.", client_id.chars().take(8).collect::<String>())
}

fn browser_file(store: &ApsStore) -> std::path::PathBuf { store.dir.join("signin-browser.json") }
pub fn load_signin_browser(store: &ApsStore) -> Option<Value> { std::fs::read_to_string(browser_file(store)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).filter(|v| v.is_object()) }
pub fn save_signin_browser(store: &ApsStore, pref: &Value) -> bool {
    let _ = std::fs::create_dir_all(&store.dir);
    let mut keep = json!({});
    for k in ["name", "exe", "profileDir", "newWindow", "how"] { if !pref[k].is_null() { keep[k] = pref[k].clone(); } }
    keep["updatedAt"] = json!(chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string());
    std::fs::write(browser_file(store), serde_json::to_string_pretty(&keep).unwrap_or_default()).is_ok()
}
pub fn forget_signin_browser(store: &ApsStore) -> bool { match std::fs::remove_file(browser_file(store)) { Ok(()) => true, Err(e) => e.kind() == std::io::ErrorKind::NotFound } }
/// `_launch_url_in_browser`: a SPECIFIC browser+profile (never the OS default) at url.
fn launch_url_in_browser(pref: &Value, url: &str) -> bool {
    let Some(exe) = pref["exe"].as_str().filter(|e| !e.is_empty() && std::path::Path::new(e).exists()) else { return false };
    let mut cmd = std::process::Command::new(exe);
    if let Some(p) = pref["profileDir"].as_str().filter(|p| !p.is_empty()) { cmd.arg(format!("--profile-directory={p}")); }
    if pref["newWindow"].as_bool().unwrap_or(true) { cmd.arg("--new-window"); }
    cmd.arg(url).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)] { use std::os::windows::process::CommandExt; cmd.creation_flags(0x0800_0000); }
    cmd.spawn().is_ok()
}
/// A consent page only a HUMAN can finish must be where a human can see it: on the primary
/// monitor and in front. One sentence of `reason` is what ab's permission gate asks for.
fn surface_consent_window(ctx: &Ctx, hwnd: i64) {
    let why = "the Autodesk sign-in page needs the person at this machine to sign in, so it has to be visible";
    let _ = ab_call(ctx, "desktop_set_window_bounds", json!({"hwnd": hwnd, "x": 80, "y": 60, "w": 1200, "h": 900, "reason": why}), 15);
    let _ = ab_call(ctx, "desktop_bring_to_front", json!({"hwnd": hwnd, "reason": why}), 15);
}
/// `_open_os_window_extension_free`: the user's REAL browser profile, no extension required.
fn open_os_window_extension_free(ctx: &Ctx, url: &str, profile: Option<&str>) -> Value {
    // An EMPTY profile name is no profile: ab refuses "no local browser profile matches ''" and no
    // browser opens at all (arav-rog, 2026-09-29, cloud search stayed signed out). Omit it instead.
    let profile = profile.filter(|p| !p.trim().is_empty()).map(str::to_string)
        .or_else(crate::signin::remembered_signin_profile_pub).filter(|p| !p.trim().is_empty());
    let sess = format!("aps-signin-{}", now() as i64);
    let mut a = json!({"sessionId": sess, "thread": "fusion-aps-signin", "purpose": "Autodesk APS consent", "url": url, "background": true});
    if let Some(p) = &profile { a["profile"] = json!(p); }
    let mut r = ab_call(ctx, "nbrowser_open_window", a.clone(), 60);
    let owned = r["_adCallError"].is_null() && (r["ok"].as_bool() == Some(true) || !r["sessionId"].is_null());
    if !owned {
        // Nobody can drive this window (no extension), so a HUMAN has to click in it: it must be
        // on screen. Opened "background" it was parked at -32000,-32000 and John could not find it
        // on either monitor (arav-rog, 2026-09-20). Open it visible and pull it onto the primary.
        a["background"] = json!(false);
        r = ab_call(ctx, "nbrowser_open_os_window", a.clone(), 60);
        // Last resort: the machine's default browser, no profile, no extension. The consent is
        // the same page; the listener on localhost catches the callback whichever browser runs it.
        if !r["_adCallError"].is_null() || r["ok"].as_bool() == Some(false) || r["success"].as_bool() == Some(false) {
            let o = ab_call(ctx, "desktop_open_url", json!({"url": url, "reason": "open the Autodesk consent so cloud search can be signed in"}), 30);
            if o["_adCallError"].is_null() && o["success"].as_bool() != Some(false) { r = json!({"ok": true, "_hint": "Launch fired in the default browser (desktop_open_url)"}); }
        }
    }
    // Whichever path opened it, ab parks the window off-screen (-32000) and may return no hwnd
    // (1.10.32 proved both). Find it and decide: a page titled "Sign in" wants CREDENTIALS, which
    // only the person at the machine may type, so it must be visible. With no extension nobody can
    // drive even the Allow click, so that window is surfaced too. A warm session under the
    // extension stays in the background, where the AI finishes it.
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(1000));
        let lw = ab_call(ctx, "desktop_list_windows", json!({}), 15);
        let rows = lw["windows"].as_array().or_else(|| lw["data"]["windows"].as_array()).cloned().unwrap_or_default();
        let hit = rows.iter().find(|w| {
            let img = w["image"].as_str().unwrap_or("").to_ascii_lowercase();
            let t = w["title"].as_str().unwrap_or("");
            (img == "chrome.exe" || img == "msedge.exe" || img == "firefox.exe" || img == "brave.exe")
                && t.contains("Autodesk") && (!owned || t.contains("Sign in"))
                && w["rect"]["left"].as_i64().unwrap_or(0) <= -30000
        }).and_then(|w| w["hwnd"].as_i64());
        if let Some(h) = hit { surface_consent_window(ctx, h); r["hwnd"] = json!(h); r["surfaced"] = json!(true); break; }
    }
    if !r["_adCallError"].is_null() { return json!({"launched": false, "profile": profile, "error": r["_adCallError"]}); }
    let launched = r["ok"].as_bool() == Some(true) || !r["hwnd"].is_null() || !r["sessionId"].is_null() || r["_hint"].as_str().unwrap_or("").contains("Launch fired");
    if launched { crate::signin::track_signin_window(&sess, profile.as_deref().unwrap_or(""), "aps"); }
    json!({"launched": launched, "agentOwned": owned, "surfaced": r["surfaced"].as_bool().unwrap_or(false), "where": r["displayName"].as_str().or(r["profile"].as_str()).map(str::to_string).or(profile.clone()), "profile": r["profile"].as_str().map(str::to_string).or(profile), "hwnd": r["hwnd"], "extensionInstalled": r["extensionInstalled"],
        "error": if launched { Value::Null } else { json!(r["error"].as_str().or(r["_hint"].as_str()).unwrap_or("").chars().take(200).collect::<String>()) }})
}
fn abe_state(ctx: &Ctx) -> Value {
    let r = ab_call(ctx, "nbrowser_readiness", json!({}), 30);
    if !r["_adCallError"].is_null() || r["ready"].is_null() { return json!({"known": false, "note": "could not probe ABE (verb missing or CLI too old); the extension-free path does not need it"}); }
    json!({"known": true, "ready": r["ready"], "extensionConnected": r["extensionConnected"], "profiles": r["profiles"].as_array().cloned().unwrap_or_default(), "chromeInstalled": r["chromeInstalled"], "extensionStale": r["extensionStale"], "staleProfiles": r["staleProfiles"].as_array().cloned().unwrap_or_default()})
}
fn setup_chain(opened: bool, abe: &Value) -> Value {
    if opened { return json!([{"step": "user_approves", "do": "Tell the user the exact profile it opened in and ask them to click Approve."}, {"step": "poll", "verb": "fusion_aps_status", "until": "signedIn:true"}]); }
    let mut chain = vec![
        json!({"step": "open_consent", "verb": "nbrowser_open_os_window", "args": {"sessionId": "<task id>", "thread": "<your thread>", "purpose": "Autodesk APS consent", "profile": "chrome:<their-autodesk-email>", "url": "<authUrl from this response>"}, "note": "EXTENSION-FREE - works with no Adom extension installed. If this verb is unknown, the adom-bridge CLI on this box is too old: update it (or have the user paste authUrl into the right profile by hand)."}),
        json!({"step": "user_approves", "do": "Ask the user to approve in that profile; without the extension we cannot click for them."}),
        json!({"step": "poll", "verb": "fusion_aps_status", "until": "signedIn:true"})];
    if abe["known"].as_bool() == Some(true) && abe["extensionConnected"].as_bool() != Some(true) { chain.push(json!({"step": "optional_extension", "flow": "installing-the-extension", "why": "lets the assistant DRIVE the sign-in instead of asking for clicks", "cost": "~5 minutes, ABE automates it"})); }
    json!(chain)
}

/// The localhost callback listener for one sign-in: answers the redirect, exchanges the code.
fn serve_callback(server: tiny_http::Server, store: ApsStore) {
    let deadline = std::time::Instant::now() + Duration::from_secs(LISTENER_SECONDS as u64);
    while std::time::Instant::now() < deadline {
        if SIGNIN_STATE.lock().unwrap().as_ref().map(|s| s["done"].as_bool() == Some(true)).unwrap_or(true) { break; }
        let Ok(Some(req)) = server.recv_timeout(Duration::from_secs(1)) else { continue };
        let url = req.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        if path != REDIRECT_PATH { let _ = req.respond(tiny_http::Response::empty(404)); continue; }
        let q: std::collections::HashMap<String, String> = query.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), urldecode(v))).collect();
        let st = SIGNIN_STATE.lock().unwrap().clone().unwrap_or(json!({}));
        let mut ok = false;
        if let Some(code) = q.get("code") { if q.get("state").map(String::as_str) == st["state"].as_str() {
            let res = exchange_code(&store, code, st["verifier"].as_str().unwrap_or(""), st["redirectUri"].as_str().unwrap_or(""));
            ok = res["ok"].as_bool() == Some(true);
            if let Some(s) = SIGNIN_STATE.lock().unwrap().as_mut() { s["done"] = json!(true); s["error"] = if ok { Value::Null } else { res }; }
        } }
        let html = format!("<html><body style='font-family:sans-serif;background:#111;color:#eee;text-align:center;padding-top:80px'><h2>Fusion APS: {}</h2><p>You can close this tab and return to Adom.</p></body></html>", if ok { "signed in &check;" } else { "sign-in failed" });
        let _ = req.respond(tiny_http::Response::from_string(html).with_header(tiny_http::Header::from_bytes("Content-Type", "text/html").unwrap()));
    }
}
fn urldecode(s: &str) -> String {
    let b = s.as_bytes(); let mut out = Vec::with_capacity(b.len()); let mut i = 0;
    while i < b.len() { match b[i] { b'%' if i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 => { if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) { out.push(v); i += 3; continue; } out.push(b'%'); i += 1; } b'+' => { out.push(b' '); i += 1; } c => { out.push(c); i += 1; } } }
    String::from_utf8_lossy(&out).to_string()
}

/// `fusion_aps_signin` (`start_signin` + `handle_signin`).
pub fn signin(ctx: &Ctx, args: &Value) -> Value {
    let store = ApsStore::default_location();
    let client_id = store.client_id();
    if client_id.is_empty() {
        return json!({"success": false, "error": "No APS Client ID configured.", "errorCode": "aps_not_configured", "orgSettingsCommand": "adom-wiki settings get <org> autodesk-aps",
            "_hint": format!("No APS app ID on this box. FIRST get your org's shared one from the wiki (members-only, nothing secret): `adom-wiki whoami` lists your orgs (belong to several? ask the user which), then `adom-wiki settings get <org> autodesk-aps` prints {{clientId}}. Got one: fusion_aps_set_client_id {{\"clientId\":\"<it>\"}} then fusion_aps_signin, and the person signs in with THEIR OWN Autodesk login. SETTINGS_NOT_FOUND (the org has none yet, or you work solo): one admin registers a Desktop/PKCE app ONCE at https://aps.autodesk.com (Data Management API, the callbacks below), sets it here, then shares it with `adom-wiki settings set <org> autodesk-aps clientId=<id> kind=public-pkce-client-id` so nobody in the org does this again (the fusion-aps-org-setup skill has every step). Never read another machine's config files for it. Callbacks: {}.", all_registered_callbacks().join(", "))});
    }
    let (verifier, challenge) = pkce_pair();
    let mut sb = [0u8; 12]; let _ = getrandom::getrandom(&mut sb); let state = b64url(&sb);
    let mut server = None; let mut bound = 0u16;
    for port in REDIRECT_PORTS { if let Ok(s) = tiny_http::Server::http(("127.0.0.1", port)) { server = Some(s); bound = port; break; } }
    let Some(server) = server else { return json!({"success": false, "error": format!("All callback ports busy on this machine: {:?}", REDIRECT_PORTS), "_hint": "Close whatever is holding these ports, or a sign-in is already in progress. These are only bound for ~10 min during sign-in."}) };
    let redirect = redirect_uri(bound);
    *SIGNIN_STATE.lock().unwrap() = Some(json!({"verifier": verifier, "state": state, "redirectUri": redirect, "done": false, "error": null, "startedAt": now()}));
    let st2 = ApsStore { dir: store.dir.clone() };
    std::thread::spawn(move || serve_callback(server, st2));
    let auth_url = format!("{APS_BASE}/authentication/v2/authorize?{}", urlencode_form(&[("response_type", "code"), ("client_id", &client_id), ("redirect_uri", &redirect), ("scope", SCOPES), ("code_challenge", &challenge), ("code_challenge_method", "S256"), ("state", &state)]));
    if probe_auth001(&auth_url) {
        *SIGNIN_STATE.lock().unwrap() = None;
        return json!({"success": false, "error": "APS AUTH-001: the app's Client ID does not have access to the Data Management API product.", "errorCode": "aps_app_missing_product", "authUrl": auth_url, "appManagementUrl": "https://aps.autodesk.com/myapps/", "clientId": client_id, "recoverable": true, "_hint": auth001_recovery_hint(&client_id)});
    }
    let passed = args["exe"].as_str().filter(|e| !e.is_empty()).map(|exe| json!({"name": args["browser"].as_str().or(args["name"].as_str()), "exe": exe, "profileDir": args["profileDir"], "newWindow": args["newWindow"].as_bool().unwrap_or(true)}));
    let pref = passed.clone().or_else(|| load_signin_browser(&store));
    let mut opened_via = "none_use_extension"; let mut opened_pref = Value::Null; let mut ext_free = Value::Null;
    let abe = abe_state(ctx);
    if let Some(p) = pref.filter(|p| launch_url_in_browser(p, &auth_url)) {
        opened_via = if passed.is_some() { "specified_browser" } else { "remembered_browser" };
        opened_pref = json!({"name": p["name"], "exe": p["exe"], "profileDir": p["profileDir"]});
        save_signin_browser(&store, &p);
    } else if args["allowDefaultBrowser"].as_bool() == Some(true) && load_signin_browser(&store).is_none() {
        let ok = { #[cfg(windows)] { std::process::Command::new("cmd").args(["/C", "start", "", &auth_url]).spawn().is_ok() } #[cfg(not(windows))] { std::process::Command::new("xdg-open").arg(&auth_url).spawn().is_ok() } };
        if ok { opened_via = "os_default_browser"; }
    } else {
        ext_free = open_os_window_extension_free(ctx, &auth_url, args["profile"].as_str());
        if ext_free["launched"].as_bool() == Some(true) { opened_via = "os_window_extension_free"; }
    }
    let hint = match opened_via {
        "remembered_browser" | "specified_browser" => format!("Opened the Autodesk consent in the REMEMBERED browser ({}{}) — the one that authed last time. A background listener on port {bound} captures the token once you approve. Poll fusion_aps_status until signedIn:true.", opened_pref["name"].as_str().unwrap_or("cached"), opened_pref["profileDir"].as_str().map(|d| format!("/{d}")).unwrap_or_default()),
        "os_default_browser" => "Opened in the OS DEFAULT browser (fallback). On a multi-browser machine this may be the wrong profile; prefer driving authUrl in the user's real browser via nbrowser_open_window (new window), then call fusion_aps_set_browser to remember it. Poll fusion_aps_status.".to_string(),
        "os_window_extension_free" => format!("Opened the Autodesk consent in {} using the EXTENSION-FREE path (nbrowser_open_os_window) - no Adom browser extension required. TELL THE USER which profile it opened in (never just 'your browser') and that THEY need to click Approve, because without the extension I cannot see or drive that window. A listener on port {bound} captures the token once they approve. Poll fusion_aps_status until signedIn:true. {ABE_UPSELL}", ext_free["where"].as_str().unwrap_or("the user's browser profile")),
        _ => format!("Could not open a browser for you. No REMEMBERED Autodesk browser, and the EXTENSION-FREE open (nbrowser_open_os_window) did not fire{}. DO THIS: run the setupChain in this response in order. Simplest manual fallback - have the USER paste authUrl into the browser profile their Autodesk account lives in. A listener on port {bound} captures the token once they approve; poll fusion_aps_status until signedIn:true. {ABE_UPSELL}", ext_free["error"].as_str().filter(|e| !e.is_empty()).map(|e| format!(": {}", e.chars().take(120).collect::<String>())).unwrap_or_default()),
    };
    let ok_open = opened_via != "none_use_extension";
    json!({"success": true, "output": format!("Autodesk sign-in started. {}", if ok_open { "Approve access in the opened browser, then call fusion_aps_status." } else { "Open authUrl in the user's real browser (new window) to approve." }),
        "data": {"authUrl": auth_url, "redirectUri": redirect, "openedVia": opened_via, "openedBrowser": opened_pref, "needsExtensionOpen": !ok_open,
            "_surfaceOptions": {"nb": "BEST if installed: nbrowser_eval (CDP) fills/clicks in the user's REAL browser - their real Google/MS logins, Autodesk accepts it, no focus steal, DOM is live so no stale-screenshot problem. Cost: the nbe install is a heavy ask.",
                "pup": "No install, fully background. BUT (1) pup's profile only has accounts YOU signed into it - it will NOT have an arbitrary account the user 'has'; (2) Autodesk gates pup at the PASSWORD page ('browser not supported', survives a clean UA - it fingerprints the CDP launch), so prefer the email-first PASSWORDLESS email-OTP path (fetch the code via adom-google) and avoid the password page; (3) screenshots are stale unless pup_configure {highFps:on}.",
                "foreground": "desktop_click on the real browser - real logins, vendor-accepted, but STEALS FOCUS (rude). Last resort for one gated step; narrate it.",
                "_pathFacts": "Not every email has an Autodesk account (media@adom.inc did NOT - email-first said 'couldn't find an account'); such users MUST use SSO or Create Account. 'Continue with Google' only lists accounts signed into THE BROWSER PROFILE you are driving."}},
        "extensionFree": ext_free, "errorCode": if ok_open { Value::Null } else { json!("browser_open_failed") }, "abe": abe, "setupChain": setup_chain(ok_open, &abe), "_hint": hint})
}

pub fn set_client_id(args: &Value) -> Value {
    let cid = args["clientId"].as_str().or(args["client_id"].as_str()).unwrap_or("").trim().to_string();
    if cid.is_empty() { return json!({"success": false, "error": "Missing clientId."}); }
    let store = ApsStore::default_location();
    let mut cfg = store.read("config.json"); if !cfg.is_object() { cfg = json!({}); }
    cfg["client_id"] = json!(cid); store.write("config.json", &cfg);
    json!({"success": true, "output": format!("Saved APS Client ID ({}...). Now call fusion_aps_signin.", cid.chars().take(6).collect::<String>()), "data": {"configured": true},
        "orgSettingsFile": {"path": "integrations/autodesk-aps.json", "content": {"schema": "adom.org-integration/1", "integration": "autodesk-aps", "owner": "<org>", "clientId": cid, "kind": "public-pkce-client-id", "secret": false}},
        "_hint": format!("Saved. NEXT: fusion_aps_signin. THEN share it so no other box in your org needs this step: if `adom-wiki settings get <org> autodesk-aps` answers SETTINGS_NOT_FOUND, run `adom-wiki settings set <org> autodesk-aps clientId={cid} kind=public-pkce-client-id` (it refuses anything secret-shaped, and when <org>/org-settings does not exist yet it prints the one `page create` command to run first). It is a public PKCE identifier, never a secret. The fusion-aps-org-setup skill walks through it.")})
}
pub fn set_browser(args: &Value) -> Value {
    let store = ApsStore::default_location();
    let exe = args["exe"].as_str().unwrap_or("").trim().to_string();
    let how = args["how"].as_str().map(str::to_string).unwrap_or_else(|| if exe.is_empty() { "extension".into() } else { "launch".into() });
    let pref = json!({"name": args["name"].as_str().or(args["browser"].as_str()), "exe": if exe.is_empty() { Value::Null } else { json!(exe) }, "profileDir": args["profileDir"], "newWindow": args.get("newWindow").cloned().unwrap_or(json!(true)), "how": how});
    if exe.is_empty() && how != "extension" { return json!({"success": false, "error": "Provide exe (browser binary path) or how:'extension'.", "_hint": "e.g. {name:'Edge', exe:'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', profileDir:'Default'}"}); }
    let ok = save_signin_browser(&store, &pref);
    json!({"success": ok, "output": format!("Remembered Autodesk sign-in browser: {}.", pref["name"].as_str().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| if exe.is_empty() { how.clone() } else { exe.clone() })), "data": {"saved": ok, "browser": load_signin_browser(&store)}})
}
pub fn get_browser() -> Value {
    let pref = load_signin_browser(&ApsStore::default_location());
    json!({"success": true, "output": match &pref { Some(p) => format!("Remembered: {}", p["name"].as_str().or(p["exe"].as_str()).or(p["how"].as_str()).unwrap_or("")), None => "No Autodesk sign-in browser remembered yet.".to_string() }, "data": {"browser": pref}})
}
pub fn forget_browser() -> Value {
    let ok = forget_signin_browser(&ApsStore::default_location());
    json!({"success": ok, "output": if ok { "Forgot the remembered Autodesk sign-in browser." } else { "Could not clear it." }, "data": {"cleared": ok}})
}

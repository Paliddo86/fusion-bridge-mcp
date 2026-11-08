//! The APS (Autodesk Platform Services) client: the bridge's ONLY cloud discovery path.
//! Never a folder walk inside Fusion (that crashed Fusion and was banned for the AI).
//!
//! Ported from `aps.py` with its two guarantees intact:
//!   1. only free Data Management / OSS endpoints are ever called (`FREE_PATH_PREFIXES`);
//!   2. a monthly hard cap on calls, fail-closed, so a user is never charged.
//! The Manufacturing Data Model API (member and permission administration) is NOT on the
//! allowlist: from 2026-08-17 it is metered past a subscription's included quota, so a verb
//! that needs it must ask for explicit opt-in rather than ride these helpers.
//!
//! Token state lives where the Python bridge keeps it (`~/.adom/fusion-aps/`), so the two
//! shells share one sign-in during the port: `config.json` {client_id}, `tokens.json`
//! {access_token, refresh_token, expires_at}, `usage.json` {month, calls}.

use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const APS_BASE: &str = "https://developer.api.autodesk.com";
pub const TOKEN_URL: &str = "https://developer.api.autodesk.com/authentication/v2/token";
pub const FREE_PATH_PREFIXES: [&str; 4] = ["/project/v1/", "/data/v1/", "/oss/v2/", "/123D-Circuits/"];
/// Conservative; well under the Data Management free quota; fail-closed.
pub const MONTHLY_CALL_CAP: u64 = 50_000;

#[derive(Debug, thiserror::Error)]
pub enum ApsError {
    #[error("not_signed_in")]
    NotSignedIn,
    #[error("blocked_non_free_endpoint: {0}")]
    BlockedNonFree(String),
    #[error("usage_cap_reached")]
    UsageCap,
    #[error("http: {0}")]
    Http(String),
}

fn now_secs() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

/// `YYYY-MM` in UTC, the usage bucket (auto-resets each month).
pub fn usage_month_at(secs: u64) -> String {
    // days from civil, inverted (Howard Hinnant)
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}")
}

/// Where the Python bridge keeps APS state; the Rust shell reads and writes the same files.
pub struct ApsStore { pub dir: PathBuf }

impl ApsStore {
    pub fn default_location() -> ApsStore {
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
        ApsStore { dir: PathBuf::from(home).join(".adom").join("fusion-aps") }
    }
    pub fn read(&self, name: &str) -> Value {
        std::fs::read_to_string(self.dir.join(name)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}))
    }
    /// Atomic: a crash or a concurrent reader never sees half a tokens.json (a torn write of a
    /// single-use refresh token loses the sign-in for good).
    pub fn write(&self, name: &str, v: &Value) {
        let _ = std::fs::create_dir_all(&self.dir);
        let tmp = self.dir.join(format!("{name}.tmp{}", std::process::id()));
        if std::fs::write(&tmp, serde_json::to_string_pretty(v).unwrap_or_default()).is_ok() {
            if std::fs::rename(&tmp, self.dir.join(name)).is_err() { let _ = std::fs::remove_file(&tmp); }
        }
    }
    pub fn client_id(&self) -> String { self.read("config.json")["client_id"].as_str().unwrap_or("").to_string() }
    pub fn tokens(&self) -> Value { self.read("tokens.json") }
    pub fn save_tokens(&self, t: &Value) { self.write("tokens.json", t) }

    /// {month, calls} for THIS month (a stale month reads as zero, like the Python side).
    pub fn usage(&self) -> (String, u64) {
        let u = self.read("usage.json");
        let month = usage_month_at(now_secs());
        if u["month"].as_str() == Some(month.as_str()) { (month, u["calls"].as_u64().unwrap_or(0)) } else { (month, 0) }
    }
    pub fn bump_usage(&self, n: u64) -> u64 {
        let (month, calls) = self.usage();
        let calls = calls + n;
        self.write("usage.json", &json!({"month": month, "calls": calls}));
        calls
    }
    pub fn cap_remaining(&self) -> u64 { MONTHLY_CALL_CAP.saturating_sub(self.usage().1) }

    /// A live access token, refreshed through the refresh token when expired. Refresh sends
    /// NO scope on purpose: a refresh may only narrow scope, so passing today's wider scopes
    /// would make Autodesk reject a token originally granted read-only.
    pub fn valid_access_token(&self) -> Option<String> {
        let mut t = self.tokens();
        if t["access_token"].as_str().map(|s| s.is_empty()).unwrap_or(true) { return None; }
        if t["expires_at"].as_u64().unwrap_or(0) <= now_secs() {
            if !self.refresh() { return None; }
            t = self.tokens();
        }
        t["access_token"].as_str().map(str::to_string)
    }
    /// Refresh ONCE, whoever asks. Autodesk refresh tokens are single use and rotate; cloud search
    /// used to refresh from up to 12 parallel workers (and the janitor, and status calls, from
    /// other processes) with the SAME token, and replaying a rotated token can revoke the whole
    /// family. ConfRoomROG then stayed signed out and the daily run was red for five days
    /// (2026-09-24 to 09-28, page issues #100-#104). So: one refresher at a time across threads
    /// and processes, re-read the tokens once the lock is held (someone else may already have
    /// refreshed), and keep Autodesk's answer when it refuses.
    fn refresh(&self) -> bool {
        static IN_PROCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _held = IN_PROCESS.lock().unwrap_or_else(|p| p.into_inner());
        let _ = std::fs::create_dir_all(&self.dir);
        let lock = self.dir.join("refresh.lock");
        let t0 = std::time::Instant::now();
        loop {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
                Ok(_) => break,
                Err(_) => {
                    // a lock older than 90 s belongs to a process that died mid-refresh
                    let stale = std::fs::metadata(&lock).ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).map(|e| e.as_secs() > 90).unwrap_or(true);
                    if stale { let _ = std::fs::remove_file(&lock); continue; }
                    if t0.elapsed() > Duration::from_secs(45) { return self.tokens()["expires_at"].as_u64().unwrap_or(0) > now_secs(); }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
        let ok = self.refresh_locked();
        let _ = std::fs::remove_file(&lock);
        ok
    }
    fn refresh_locked(&self) -> bool {
        let t = self.tokens();
        // another thread or process refreshed while we waited for the lock: use its token
        if t["expires_at"].as_u64().unwrap_or(0) > now_secs() && t["access_token"].as_str().map(|s| !s.is_empty()).unwrap_or(false) { return true; }
        let Some(rt) = t["refresh_token"].as_str().filter(|s| !s.is_empty()).map(str::to_string) else { return false };
        let form = format!("grant_type=refresh_token&refresh_token={}&client_id={}", urlencode(&rt), urlencode(&self.client_id()));
        let resp = ureq::post(TOKEN_URL).timeout(Duration::from_secs(30))
            .set("Content-Type", "application/x-www-form-urlencoded").send_string(&form);
        let resp = match resp {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let body: Value = r.into_json().unwrap_or(json!({}));
                self.write("last_refresh_error.json", &json!({"at": now_secs(), "status": code, "error": body["error"], "description": body["error_description"].as_str().or(body["developerMessage"].as_str())}));
                return false;
            }
            Err(e) => { self.write("last_refresh_error.json", &json!({"at": now_secs(), "status": null, "error": format!("{e}")})); return false; }
        };
        let Ok(mut body) = resp.into_json::<Value>() else { return false };
        if body["access_token"].as_str().map(|s| !s.is_empty()).unwrap_or(false) {
            let now = now_secs();
            body["obtained_at"] = json!(now);
            body["expires_at"] = json!(now + body["expires_in"].as_u64().unwrap_or(3600) - 60);
            if body.get("refresh_token").is_none() { body["refresh_token"] = json!(rt); }
            self.save_tokens(&body);
            return true;
        }
        false
    }
}

/// Percent-encode a form value (RFC 3986 unreserved set kept).
pub fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// The path part of an absolute-from-host path or a full URL.
pub fn only_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("https://").or_else(|| path.strip_prefix("http://")) {
        match rest.find('/') { Some(i) => rest[i..].split('?').next().unwrap_or("/").to_string(), None => "/".into() }
    } else { path.split('?').next().unwrap_or("/").to_string() }
}

/// Guarantee 1: only ever hit free endpoints.
pub fn is_free_endpoint(path: &str) -> bool { let p = only_path(path); FREE_PATH_PREFIXES.iter().any(|x| p.starts_with(x)) }

/// Authenticated GET with both guarantees. Returns (status, body) and never raises on an
/// HTTP error status; the body is the JSON or `{"text": ...}` when not JSON.
pub fn aps_get(store: &ApsStore, path: &str, query: &[(&str, &str)]) -> Result<(u16, Value), ApsError> {
    aps_call(store, "GET", path, query, None)
}

pub fn aps_call(store: &ApsStore, method: &str, path: &str, query: &[(&str, &str)], body: Option<&Value>) -> Result<(u16, Value), ApsError> {
    aps_call_ct(store, method, path, query, body, "application/vnd.api+json")
}

/// `aps_call` with an explicit Content-Type (OSS upload completion wants plain JSON).
pub fn aps_call_ct(store: &ApsStore, method: &str, path: &str, query: &[(&str, &str)], body: Option<&Value>, content_type: &str) -> Result<(u16, Value), ApsError> {
    let at = store.valid_access_token().ok_or(ApsError::NotSignedIn)?;
    if !is_free_endpoint(path) { return Err(ApsError::BlockedNonFree(only_path(path))); }
    if store.cap_remaining() == 0 { return Err(ApsError::UsageCap); }
    let mut url = if path.starts_with("http") { path.to_string() } else { format!("{APS_BASE}{path}") };
    if !query.is_empty() {
        let qs: Vec<String> = query.iter().map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v))).collect();
        url.push(if url.contains('?') { '&' } else { '?' }); url.push_str(&qs.join("&"));
    }
    store.bump_usage(1);
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(120)).build();
    let req = agent.request(method, &url).set("Authorization", &format!("Bearer {at}"));
    let resp = match body {
        Some(b) => req.set("Content-Type", content_type).send_string(&b.to_string()),
        None => req.call(),
    };
    let (status, text) = match resp {
        Ok(r) => { let s = r.status(); (s, r.into_string().unwrap_or_default()) }
        Err(ureq::Error::Status(s, r)) => (s, r.into_string().unwrap_or_default()),
        Err(e) => return Err(ApsError::Http(e.to_string())),
    };
    let parsed = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({"text": text}));
    Ok((status, parsed))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn free_endpoint_guard_matches_python() {
        assert!(is_free_endpoint("/project/v1/hubs"));
        assert!(is_free_endpoint("https://developer.api.autodesk.com/data/v1/projects/x/folders/y/contents?page=2"));
        assert!(!is_free_endpoint("/mfg/graphql"));
        assert!(!is_free_endpoint("https://developer.api.autodesk.com/construction/admin/v1/projects"));
    }
    #[test]
    fn usage_month_is_utc_year_month() {
        assert_eq!(usage_month_at(0), "1970-01");
        assert_eq!(usage_month_at(20_699 * 86_400 + 3600), "2026-09");
        assert_eq!(usage_month_at(951_868_800), "2000-03");
    }
    #[test]
    fn usage_counter_resets_on_a_new_month_and_caps() {
        let dir = std::env::temp_dir().join(format!("fusion-aps-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = ApsStore { dir: dir.clone() };
        s.write("usage.json", &json!({"month": "1999-01", "calls": 49_999}));
        assert_eq!(s.usage().1, 0, "a stale month reads as zero");
        assert_eq!(s.bump_usage(1), 1);
        assert_eq!(s.cap_remaining(), MONTHLY_CALL_CAP - 1);
        s.write("usage.json", &json!({"month": usage_month_at(now_secs()), "calls": MONTHLY_CALL_CAP}));
        assert_eq!(s.cap_remaining(), 0);
        assert!(matches!(aps_get(&s, "/project/v1/hubs", &[]), Err(ApsError::NotSignedIn)), "no token: refused before any network");
        s.write("tokens.json", &json!({"access_token": "x", "expires_at": now_secs() + 3600}));
        assert!(matches!(aps_get(&s, "/mfg/graphql", &[]), Err(ApsError::BlockedNonFree(_))));
        assert!(matches!(aps_get(&s, "/project/v1/hubs", &[]), Err(ApsError::UsageCap)));
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn urlencode_is_form_safe() { assert_eq!(urlencode("a b/c?d=e&f"), "a%20b%2Fc%3Fd%3De%26f"); assert_eq!(urlencode("abc-_.~"), "abc-_.~"); }
}


// ───────────────────────── cloud discovery on the Data Management API ─────────────────────────

/// The Python bridge's APS_REQUIRED_HINT, verbatim.
pub const APS_REQUIRED_HINT: &str = "APS IS MANDATORY for cloud file search/browse/recent - SET IT UP, do not give up, do not fall back. ⛔ NEVER enumerate cloud files via app.data (dataProjects/dataFiles) in fusion_run_modeling_script: the DEPRECATED method holds Fusion's main thread for minutes, times out, and locks the app. AUTO-RECOVER APS instead: (1) NOT configured? The org Client ID is a PUBLIC PKCE id - pull it from a peer that has it: read ~/.adom/fusion-aps/config.json (key 'client_id') off another ab desktop (`adom-bridge targets` lists them) via shell_execute/pull_file, then fusion_aps_set_client_id {clientId}. (2) fusion_aps_signin -> open the returned authUrl in the user's REAL browser with a warm Autodesk session; INSTALL + USE the adom-browser-extension (nbrowser/ABE, https://wiki.adom.inc/adom/adom-browser-extension) to drive it reliably (recommend it - coordinate-clicking a browser is fragile). The localhost listener captures the token; poll fusion_aps_status until signedIn:true. (3) retry. If authorize returns AUTH-001 'client_id does not have access to the api product': the APS app at aps.autodesk.com/myapps lost its Data Management API product - re-commit its API Access + Save, wait a few minutes to propagate, retry.";

fn data_array(v: &Value) -> Vec<Value> { v["data"].as_array().cloned().unwrap_or_default() }
fn attr<'a>(it: &'a Value, k: &str) -> &'a str { it["attributes"][k].as_str().unwrap_or("") }

/// GET /project/v1/hubs → (hubs, error) exactly like `_list_hubs`.
pub fn list_hubs(store: &ApsStore) -> Result<Vec<Value>, (u16, Value)> {
    match aps_get(store, "/project/v1/hubs", &[]) {
        Ok((200, body)) if body.is_object() => Ok(data_array(&body)),
        Ok((s, body)) => Err((s, body)),
        Err(e) => Err((0, json!({"error": e.to_string()}))),
    }
}
pub fn list_projects(store: &ApsStore, hub_id: &str) -> Vec<Value> {
    match aps_get(store, &format!("/project/v1/hubs/{hub_id}/projects"), &[]) { Ok((200, b)) => data_array(&b), _ => vec![] }
}
/// A project's top folders almost never change, and fetching them was a sequential round trip
/// per project before any search began (16 on Adom's hub). Cached on disk for 6 hours.
pub fn top_folders(store: &ApsStore, hub_id: &str, project_id: &str) -> Vec<Value> {
    let safe: String = project_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '_').collect();
    let cache = store.dir.join("topfolders").join(format!("{safe}.json"));
    if let Ok(m) = std::fs::metadata(&cache) {
        if m.modified().ok().and_then(|t| t.elapsed().ok()).map(|e| e.as_secs() < 6 * 3600).unwrap_or(false) {
            if let Some(v) = std::fs::read_to_string(&cache).ok().and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok()) { if !v.is_empty() { return v; } }
        }
    }
    match aps_get(store, &format!("/project/v1/hubs/{hub_id}/projects/{project_id}/topFolders"), &[]) {
        Ok((200, b)) => {
            let v = data_array(&b);
            if !v.is_empty() { let _ = std::fs::create_dir_all(cache.parent().unwrap()); let _ = std::fs::write(&cache, serde_json::to_string(&v).unwrap_or_default()); }
            v
        }
        _ => vec![],
    }
}

/// Recursive name search within a folder: the folder's DIRECT contents first (`:search`
/// only recurses subfolders), then APS's server-side recursive index, paged, filtered
/// client-side by case-insensitive substring (the server-side displayName filter proved
/// unreliable). Stops at `limit` matches. An empty query matches everything.
pub fn folder_search(store: &ApsStore, project_id: &str, folder_id: &str, query: &str, limit: usize) -> (Vec<Value>, Option<(u16, Value)>) {
    let ql = query.to_lowercase();
    let mut matches: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut take = |it: &Value, matches: &mut Vec<Value>| -> bool {
        let id = it["id"].as_str().unwrap_or("").to_string();
        if seen.contains(&id) { return false; }
        if attr(it, "displayName").to_lowercase().contains(&ql) { seen.insert(id); matches.push(it.clone()); }
        matches.len() >= limit
    };
    if let Ok((200, body)) = aps_get(store, &format!("/data/v1/projects/{project_id}/folders/{folder_id}/contents"), &[("page[limit]", "200")]) {
        for it in data_array(&body) { if it["type"].as_str() == Some("items") && take(&it, &mut matches) { return (matches, None); } }
    }
    let mut url = format!("/data/v1/projects/{project_id}/folders/{folder_id}/search");
    let mut first = true;
    for _ in 0..25 {
        let q: &[(&str, &str)] = if first { &[("page[limit]", "200")] } else { &[] };
        first = false;
        match aps_get(store, &url, q) {
            Ok((200, body)) if body.is_object() => {
                for it in data_array(&body) { if take(&it, &mut matches) { return (matches, None); } }
                let nxt = &body["links"]["next"];
                let href = nxt["href"].as_str().or_else(|| nxt.as_str()).map(str::to_string);
                match href { Some(h) if !h.is_empty() => url = h, _ => break }
            }
            Ok((s, body)) => return (matches.clone(), if matches.is_empty() { Some((s, body)) } else { None }),
            Err(e) => return (matches.clone(), if matches.is_empty() { Some((0, json!({"error": e.to_string()}))) } else { None }),
        }
    }
    (matches, None)
}

/// One (hub, project, folder) to scan; what `handle_search` / `handle_recent` build.
#[derive(Clone)]
pub struct ScanTask { pub hub_id: String, pub project_id: String, pub project_name: String, pub folder_id: String }

/// The (hub, project, top folder) task list, honouring hub/project filters like Python.
pub fn scan_tasks(store: &ApsStore, hubs: &[Value], hub_filter: Option<&str>, project_filter: Option<&str>, scanned: &mut (usize, usize, usize)) -> Vec<ScanTask> {
    let mut tasks = Vec::new();
    for hub in hubs {
        let hid = hub["id"].as_str().unwrap_or("").to_string();
        if let Some(f) = hub_filter { if hid != f { continue; } }
        scanned.0 += 1;
        let mut projects = list_projects(store, &hid);
        if let Some(pf) = project_filter {
            let kept: Vec<Value> = projects.iter().filter(|p| p["id"].as_str() == Some(pf)).cloned().collect();
            projects = if kept.is_empty() { vec![json!({"id": pf, "attributes": {"name": pf}})] } else { kept };
        }
        // top folders for every project IN PARALLEL (cached ones answer from disk)
        let handles: Vec<_> = projects.into_iter().map(|p| {
            let (dir, hid2) = (store.dir.clone(), hid.clone());
            std::thread::spawn(move || {
                let pid = p["id"].as_str().unwrap_or("").to_string();
                let pname = attr(&p, "name").to_string();
                let folders = top_folders(&ApsStore { dir }, &hid2, &pid);
                (pid, pname, folders)
            })
        }).collect();
        for h in handles {
            let Ok((pid, pname, folders)) = h.join() else { continue };
            scanned.1 += 1;
            for f in folders {
                scanned.2 += 1;
                tasks.push(ScanTask { hub_id: hid.clone(), project_id: pid.clone(), project_name: pname.clone(), folder_id: f["id"].as_str().unwrap_or("").to_string() });
            }
        }
    }
    tasks
}

/// Scan every task in parallel (12 workers, like the Python pool), dedupe by id, newest first.
pub fn scan_parallel(store: &ApsStore, tasks: Vec<ScanTask>, query: &str, per_folder_limit: usize, with_hub: bool) -> Vec<Value> {
    use std::sync::{Arc, Mutex};
    let queue = Arc::new(Mutex::new(tasks));
    let out = Arc::new(Mutex::new(Vec::<(usize, Vec<Value>)>::new()));
    let dir = store.dir.clone();
    let query = query.to_string();
    let mut handles = Vec::new();
    let total = queue.lock().unwrap().len();
    let mut idx = 0usize;
    let indexed: Vec<(usize, ScanTask)> = queue.lock().unwrap().drain(..).map(|t| { idx += 1; (idx - 1, t) }).collect();
    let queue = Arc::new(Mutex::new(indexed));
    for _ in 0..12.min(total.max(1)) {
        let (queue, out, dir, query) = (queue.clone(), out.clone(), dir.clone(), query.clone());
        handles.push(std::thread::spawn(move || {
            let store = ApsStore { dir };
            loop {
                let next = queue.lock().unwrap().pop();
                let Some((i, t)) = next else { break };
                let (hits, _) = folder_search(&store, &t.project_id, &t.folder_id, &query, per_folder_limit);
                let rows: Vec<Value> = hits.iter().map(|h| {
                    let mut r = json!({"name": attr(h, "displayName"), "type": h["type"], "id": h["id"],
                                       "projectId": t.project_id, "projectName": t.project_name, "lastModified": h["attributes"]["lastModifiedTime"]});
                    if with_hub { r["hubId"] = json!(t.hub_id); }
                    r
                }).collect();
                out.lock().unwrap().push((i, rows));
            }
        }));
    }
    for h in handles { let _ = h.join(); }
    let mut chunks = out.lock().unwrap().clone();
    chunks.sort_by_key(|c| c.0); // pool.map order, so the first-seen dedupe matches Python
    let mut seen = std::collections::HashSet::new();
    let mut results = Vec::new();
    for (_, rows) in chunks { for r in rows { let id = r["id"].as_str().unwrap_or("").to_string(); if seen.insert(id) { results.push(r); } } }
    results
}


/// A version urn stays; a lineage urn resolves to its tip version (like `_tip_version_urn`).
pub fn tip_version_urn(store: &ApsStore, project_id: &str, urn: &str) -> Result<String, (u16, Value)> {
    let raw = urn.split('?').next().unwrap_or("");
    if urn.contains("?version=") || raw.contains("fs.file:vf.") { return Ok(urn.to_string()); }
    let lineage = match raw.find("fs.file:vf.") { Some(i) => format!("urn:adsk.wipprod:dm.lineage:{}", &raw[i + 11..]), None => urn.to_string() };
    match aps_get(store, &format!("/data/v1/projects/{project_id}/items/{}", urlencode(&lineage)), &[]) {
        Ok((200, b)) if b.is_object() => b["data"]["relationships"]["tip"]["data"]["id"].as_str().map(str::to_string).ok_or((200, b.clone())),
        Ok((s, b)) => Err((s, b)),
        Err(e) => Err((0, json!({"error": e.to_string()}))),
    }
}

/// (storage urn, file name) of a concrete version.
pub fn storage_of_version(store: &ApsStore, project_id: &str, version_urn: &str) -> Result<(String, Option<String>), (u16, Value)> {
    match aps_get(store, &format!("/data/v1/projects/{project_id}/versions/{}", urlencode(version_urn)), &[]) {
        Ok((200, b)) if b.is_object() => {
            let d = &b["data"];
            let storage = d["relationships"]["storage"]["data"]["id"].as_str().map(str::to_string).ok_or((200, b.clone()))?;
            Ok((storage, d["attributes"]["name"].as_str().map(str::to_string)))
        }
        Ok((s, b)) => Err((s, b)),
        Err(e) => Err((0, json!({"error": e.to_string()}))),
    }
}

/// urn:adsk.objects:os.object:<bucketKey>/<objectKey> → (bucket, object)
pub fn parse_oss(storage_urn: &str) -> (String, String) {
    let tail = storage_urn.rsplit("os.object:").next().unwrap_or("").to_string();
    match tail.find('/') { Some(i) => (tail[..i].to_string(), tail[i + 1..].to_string()), None => (tail, String::new()) }
}

/// Raw binary GET of a presigned S3 URL (the transfer itself is outside APS, so it is not
/// gated or counted; obtaining the URL was). Returns (status, bytes).
pub fn http_bytes(url: &str) -> (u16, Vec<u8>) {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(300)).build();
    let mut buf = Vec::new();
    match agent.get(url).call() {
        Ok(r) => { let s = r.status(); let _ = std::io::Read::read_to_end(&mut r.into_reader(), &mut buf); (s, buf) }
        Err(ureq::Error::Status(s, r)) => { let _ = std::io::Read::read_to_end(&mut r.into_reader(), &mut buf); (s, buf) }
        Err(e) => (0, e.to_string().into_bytes()),
    }
}


/// Raw binary PUT to a presigned S3 URL (outside APS: not gated, not counted).
pub fn http_put_bytes(url: &str, data: &[u8]) -> u16 {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(600)).build();
    match agent.put(url).set("Content-Type", "application/octet-stream").send_bytes(data) {
        Ok(r) => r.status(),
        Err(ureq::Error::Status(s, _)) => s,
        Err(_) => 0,
    }
}


pub const REDIRECT_PORTS: [u16; 3] = [8917, 8918, 8920];
pub const REDIRECT_PATH: &str = "/fusion-aps/callback";
pub const SCOPES: &str = "data:read data:write data:create";

/// Every callback URL that must be registered on the APS app (localhost + 127.0.0.1).
pub fn all_registered_callbacks() -> Vec<String> {
    let mut out = Vec::new();
    for p in REDIRECT_PORTS { out.push(format!("http://localhost:{p}{REDIRECT_PATH}")); out.push(format!("http://127.0.0.1:{p}{REDIRECT_PATH}")); }
    out
}

/// WHO the token belongs to (Autodesk's OIDC userinfo, a free endpoint), cached per token in
/// the store dir so the dashboard can show the account without a call per poll.
pub fn user_info(store: &ApsStore) -> Value {
    let Some(at) = store.valid_access_token() else { return Value::Null };
    let key = { use std::hash::{Hash, Hasher}; let mut h = std::collections::hash_map::DefaultHasher::new(); at.hash(&mut h); format!("{:x}", h.finish()) };
    let cache = store.read("userinfo-cache.json");
    if cache["key"].as_str() == Some(key.as_str()) { return cache["info"].clone(); }
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
    let Ok(r) = agent.get("https://api.userprofile.autodesk.com/userinfo").set("Authorization", &format!("Bearer {at}")).call() else { return json!({}) };
    let Ok(info) = r.into_json::<Value>() else { return json!({}) };
    let out = json!({"email": info["email"], "name": info["name"], "userId": info["sub"]});
    store.write("userinfo-cache.json", &json!({"key": key, "info": out}));
    out
}

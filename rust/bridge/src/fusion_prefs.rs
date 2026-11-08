//! Fusion's own preferences (theme, orbit scheme, units...) and the workspace reset, served by
//! the exe over the add-in pass-through. The preference script is byte-identical to server.py's
//! `_PREF_SCRIPT` (a test pins that), so both shells drive Fusion the same way.

use serde_json::{json, Map, Value};

pub const PREF_SCRIPT: &str = include_str!("pref_script.py");

fn py_json_string_literal(v: &Value) -> String {
    // json.dumps(json.dumps(reqs)): the request dict as a JSON string, then that string as a
    // JSON literal, which Python's json.loads(__REQS__) unwraps
    let inner = py_dumps(v);
    serde_json::to_string(&inner).unwrap_or_else(|_| "\"{}\"".into())
}

/// `_run_pref_script`: one retry on a transient add-in connection error.
fn run_pref_script(script: String, timeout: u64, call: &dyn Fn(&str, Value, u64) -> Value) -> Value {
    let res = call("run_modeling_script", json!({"script": script}), timeout);
    if res["success"].as_bool() == Some(true) { return res; }
    let err = res["error"].as_str().unwrap_or("").to_lowercase();
    if ["not running", "not responding", "add-in", "connection", "timed out", "refused"].iter().any(|s| err.contains(s)) {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let res2 = call("run_modeling_script", json!({"script": script}), timeout);
        if !res2.is_null() { return res2; }
    }
    res
}

fn shape(res: &Value, wrote: bool) -> Value {
    if res["success"].as_bool() != Some(true) {
        return json!({"success": false, "error": res["error"].as_str().unwrap_or("preferences script did not run"),
            "_hint": "fusion_set_preference/get_preferences need Fusion running + signed in (fusion_readiness -> ready:true). If needsSignin, drive the sign-in first."});
    }
    let data = &res["data"]["result"];
    let applied = data["applied"].as_object().cloned().unwrap_or_default();
    let current = data["current"].clone();
    let mut out = json!({"success": true, "applied": applied, "current": if current.is_null() { json!({}) } else { current }});
    if wrote {
        let oks: Vec<&str> = applied.iter().filter(|(_, v)| v["ok"].as_bool() == Some(true)).map(|(k, _)| k.as_str()).collect();
        let fails: Vec<(String, String)> = applied.iter().filter(|(_, v)| v["ok"].as_bool() != Some(true)).map(|(k, v)| (k.clone(), py_str(&v["error"]))).collect();
        if !fails.is_empty() {
            out["partial"] = json!(true);
            out["_hint"] = json!(format!("Applied live: {}. FAILED: {}. Note: some themes (Dark Gray/Classic) aren't shipped in every Fusion build - use 'darkblue' or 'device'. See `current` for the resulting state.",
                if oks.is_empty() { "(none)".to_string() } else { oks.join(", ") }, fails.iter().map(|(k, e)| format!("{k}={e}")).collect::<Vec<_>>().join("; ")));
        } else {
            out["_hint"] = json!(format!("Applied LIVE (no restart needed): {}. See `current` for resulting state.", oks.join(", ")));
        }
    } else {
        out["_hint"] = json!("Current Fusion preferences. Change any with fusion_set_preference, keys: theme (light/darkblue/darkgray/classic/device/dark), invertScrollZoom (bool), orbitScheme (fusion360/alias/inventor/solidworks/tinkercad/powermill), modelingOrientation (yup/zup), gestureNav (bool), cameraPivot (bool), lengthUnit (mm/cm/m/in/ft - applies to the active design).");
    }
    out
}

pub fn get_preferences(call: &dyn Fn(&str, Value, u64) -> Value) -> Value {
    let script = PREF_SCRIPT.replace("__REQS__", &py_json_string_literal(&json!({})));
    shape(&run_pref_script(script, 20, call), false)
}

/// The standard's preferences read: all of them, or one by key (the key names current lists).
pub fn get_preference_key(args: &Value, call: &dyn Fn(&str, Value, u64) -> Value) -> Value {
    let mut r = get_preferences(call);
    let Some(key) = args["key"].as_str().filter(|k| !k.is_empty()) else { return r };
    let Some(cur) = r["current"].as_object().cloned() else { return r };
    match cur.get(key) {
        Some(v) => { r["current"] = json!({key: v}); r }
        None => json!({"success": false, "errorCode": "unknown_key", "error": format!("Fusion has no setting {key}"), "keys": cur.keys().collect::<Vec<_>>()}),
    }
}

pub fn set_preference(args: &Value, call: &dyn Fn(&str, Value, u64) -> Value) -> Value {
    let reqs: Map<String, Value> = args.as_object().map(|o| o.iter().filter(|(k, _)| k.as_str() != "settle").map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
    if reqs.is_empty() {
        return json!({"success": false, "error": "No preferences given.",
            "_hint": "Pass e.g. {\"theme\":\"dark\"} or {\"invertScrollZoom\":true,\"orbitScheme\":\"solidworks\"}. Call fusion_get_preferences to see current values + keys."});
    }
    let script = PREF_SCRIPT.replace("__REQS__", &py_json_string_literal(&Value::Object(reqs)));
    shape(&run_pref_script(script, 30, call), true)
}

/// `_bridge_action_ledger`: one append-only record in `<state>/actions.jsonl`.
pub fn action_ledger(state_dir: &std::path::Path, action: &str, detail: Value) {
    let epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let mut rec = json!({"at": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(), "epoch": (epoch * 10.0).round() / 10.0, "action": action, "pid": std::process::id()});
    if let (Some(r), Some(d)) = (rec.as_object_mut(), detail.as_object()) { for (k, v) in d { r.insert(k.clone(), v.clone()); } }
    let _ = std::fs::create_dir_all(state_dir);
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(state_dir.join("actions.jsonl")) { let _ = writeln!(f, "{}", rec); }
}

/// `fusion_reset_workspace`: close everything, then sweep the unmodified Untitled scratch.
pub fn reset_workspace(state_dir: &std::path::Path, args: &Value, pass: &dyn Fn(&str, Value) -> Value, call: &dyn Fn(&str, Value, u64) -> Value) -> Value {
    let force = args["force"].as_bool().unwrap_or(false);
    let r = pass("close_all_documents", json!({"save": false, "closeUntitled": true, "force": force}));
    let mut closed: Vec<String> = Vec::new();
    let mut prev_total: Option<usize> = None;
    for _ in 0..24 {
        let st = call("document_info", json!({}), 8);
        let docs = st["data"]["openDocuments"].as_array().cloned().unwrap_or_default();
        let untitled: Vec<&Value> = docs.iter().filter(|d| d["name"].as_str().unwrap_or("").to_lowercase().starts_with("untitled") && !(d["isModified"].as_bool().unwrap_or(false) || d["modified"].as_bool().unwrap_or(false))).collect();
        if (untitled.len() <= 1 && docs.len() == untitled.len()) || untitled.is_empty() { break; }
        if let Some(p) = prev_total { if docs.len() >= p { break; } }
        prev_total = Some(docs.len());
        let victim = untitled[0]["name"].clone();
        let c = call("close_document", json!({"name": victim, "save": false, "saveToClose": true}), 20);
        if c["success"].as_bool() != Some(true) { break; }
        closed.push(py_str(&victim));
    }
    if !closed.is_empty() {
        eprintln!("[fusion-bridge] swept {} Untitled scratch doc(s): {}", closed.len(), closed.iter().take(6).cloned().collect::<Vec<_>>().join(", "));
        action_ledger(state_dir, "untitled_scratch_swept", json!({"count": closed.len()}));
    }
    let st = call("document_info", json!({}), 8);
    let left: Vec<Value> = st["data"]["openDocuments"].as_array().map(|a| a.iter().map(|d| d["name"].clone()).collect()).unwrap_or_default();
    json!({"success": true, "closedByAddin": r["data"]["closed"], "skipped": r["data"]["skipped"], "untitledScratchClosed": closed.len(), "remaining": left,
        "_hint": "Fusion always keeps ONE blank document alive, so a single Untitled remaining is correct, not a failure. A modified Untitled is left untouched unless you pass force:true - it may hold unsaved work."})
}

#[cfg(test)]
mod tests {
    #[test]
    fn pref_script_is_byte_identical_to_server_py() {
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../server.py")).expect("server.py beside the workspace");
        let i = src.find("_PREF_SCRIPT = r'''").unwrap() + "_PREF_SCRIPT = r'''".len();
        let j = src[i..].find("'''").unwrap() + i;
        assert_eq!(&src[i..j], super::PREF_SCRIPT, "rust/bridge/src/pref_script.py drifted from server.py's _PREF_SCRIPT");
    }
    #[test]
    fn py_dumps_matches_python() {
        assert_eq!(super::py_dumps(&serde_json::json!({"theme": "dark", "invertScrollZoom": true, "n": null})), r#"{"theme": "dark", "invertScrollZoom": true, "n": null}"#);
        assert_eq!(super::py_dumps(&serde_json::json!("é\"x")), r#""\u00e9\"x""#);
    }

}

/// Python's default `json.dumps` (separators ", " / ": ", ensure_ascii, insertion order).
pub fn py_dumps(v: &Value) -> String {
    fn esc(s: &str, out: &mut String) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""), '\\' => out.push_str("\\\\"), '\n' => out.push_str("\\n"), '\r' => out.push_str("\\r"), '\t' => out.push_str("\\t"),
                '\u{08}' => out.push_str("\\b"), '\u{0c}' => out.push_str("\\f"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c if (c as u32) > 0x7e => { let mut b = [0u16; 2]; for u in c.encode_utf16(&mut b) { out.push_str(&format!("\\u{:04x}", u)); } }
                c => out.push(c),
            }
        }
        out.push('"');
    }
    fn walk(v: &Value, out: &mut String) {
        match v {
            Value::Null => out.push_str("null"), Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => out.push_str(&n.to_string()), Value::String(s) => esc(s, out),
            Value::Array(a) => { out.push('['); for (i, x) in a.iter().enumerate() { if i > 0 { out.push_str(", "); } walk(x, out); } out.push(']'); }
            Value::Object(o) => { out.push('{'); for (i, (k, x)) in o.iter().enumerate() { if i > 0 { out.push_str(", "); } esc(k, out); out.push_str(": "); walk(x, out); } out.push('}'); }
        }
    }
    let mut out = String::new(); walk(v, &mut out); out
}

/// Python's `str()` of a JSON value.
pub fn py_str(v: &Value) -> String {
    match v { Value::Null => "None".into(), Value::Bool(b) => if *b { "True".into() } else { "False".into() }, Value::String(s) => s.clone(), other => py_dumps(other) }
}

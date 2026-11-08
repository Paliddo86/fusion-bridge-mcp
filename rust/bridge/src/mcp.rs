//! `fusion_mcp_*`: Autodesk's own Fusion MCP server on 127.0.0.1:27182 (Streamable HTTP).
//! The bridge is a plain JSON-RPC client of it; `mcp_enable` flips the Preferences toggle
//! Autodesk exposes no API for.
use crate::addin::Ctx;
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;

pub const MCP_URL: &str = "http://127.0.0.1:27182/mcp";
static SESSION_ID: Mutex<Option<String>> = Mutex::new(None);
static RPC_LOCK: Mutex<()> = Mutex::new(());

const OFF_STATIC: &str = "Fusion's MCP server is not reachable on 127.0.0.1:27182. It is OFF by default. The bridge can turn it on FOR the user: call fusion_mcp_enable, which opens Preferences, expands General, selects API, ticks 'Fusion MCP Server (runs locally on this device)' and clicks Apply/OK. It needs Fusion running and briefly uses the foreground (announced with an on-screen caption). Manual path if you prefer: profile icon (top right) -> Preferences -> General -> API -> tick the box. NOTE the setting does NOT survive if Fusion is killed before Apply is clicked.";

/// `{status, headers, body}` / `{httpError, body}` / `{error}` for one POST.
fn post(body: &Value, sid: Option<&str>, timeout: u64) -> Value {
    let mut req = ureq::post(MCP_URL).timeout(Duration::from_secs(timeout)).set("Content-Type", "application/json").set("Accept", "application/json, text/event-stream");
    if let Some(s) = sid { req = req.set("MCP-Session-Id", s); }
    match req.send_string(&body.to_string()) {
        Ok(r) => { let sid = r.header("mcp-session-id").map(str::to_string); let status = r.status(); let text = r.into_string().unwrap_or_default(); json!({"status": status, "sessionId": sid, "body": text}) }
        Err(ureq::Error::Status(code, r)) => { let text = r.into_string().unwrap_or_default(); json!({"httpError": code, "body": text.chars().take(1000).collect::<String>()}) }
        Err(e) => json!({"error": format!("{}", e).chars().take(220).collect::<String>()}),
    }
}

/// initialize + notifications/initialized: `{sessionId, serverInfo}` or `{error}`.
pub fn new_session(version: &str) -> Value {
    let r = post(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "fusion-bridge", "version": version}}}), None, 30);
    if !r["error"].is_null() || !r["httpError"].is_null() {
        return json!({"error": if r["error"].is_null() { format!("HTTP {}", r["httpError"]) } else { r["error"].as_str().unwrap_or("").to_string() }, "raw": r["body"]});
    }
    let sid = r["sessionId"].as_str().map(str::to_string);
    let info = serde_json::from_str::<Value>(r["body"].as_str().unwrap_or("{}")).map(|v| v["result"]["serverInfo"].clone()).unwrap_or(json!({}));
    if let Some(s) = &sid { let _ = post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), Some(s), 30); *SESSION_ID.lock().unwrap() = Some(s.clone()); }
    json!({"sessionId": sid, "serverInfo": if info.is_null() { json!({}) } else { info }})
}

/// One JSON-RPC call with automatic session (re)establishment; `{_mcpError}` on failure.
pub fn rpc(version: &str, method: &str, params: Option<Value>, timeout: u64) -> Value {
    let sid = {
        let _g = RPC_LOCK.lock().unwrap();
        if SESSION_ID.lock().unwrap().is_none() { let s = new_session(version); if !s["error"].is_null() { return json!({"_mcpError": s["error"]}); } }
        SESSION_ID.lock().unwrap().clone()
    };
    let mut body = json!({"jsonrpc": "2.0", "id": 7, "method": method});
    if let Some(p) = params { body["params"] = p; }
    let mut r = post(&body, sid.as_deref(), timeout);
    if matches!(r["httpError"].as_u64(), Some(400) | Some(404)) && r["body"].as_str().unwrap_or("").contains("ession") {
        let sid2 = { let _g = RPC_LOCK.lock().unwrap(); *SESSION_ID.lock().unwrap() = None; let s = new_session(version); if !s["error"].is_null() { return json!({"_mcpError": s["error"]}); } SESSION_ID.lock().unwrap().clone() };
        r = post(&body, sid2.as_deref(), timeout);
    }
    if !r["error"].is_null() || !r["httpError"].is_null() {
        return json!({"_mcpError": if r["error"].is_null() { format!("HTTP {}: {}", r["httpError"], r["body"].as_str().unwrap_or("").chars().take(200).collect::<String>()) } else { r["error"].as_str().unwrap_or("").to_string() }});
    }
    serde_json::from_str::<Value>(r["body"].as_str().unwrap_or("{}")).unwrap_or_else(|_| json!({"_mcpError": "unparseable response", "raw": r["body"].as_str().unwrap_or("").chars().take(400).collect::<String>()}))
}

/// The MCP-unreachable hint, state-aware: never send the AI to enable MCP when APS is signed
/// in and could serve a file search right now. `aps` = (configured, signedIn).
pub fn off_hint(aps: (bool, bool)) -> String {
    let lead = if aps.1 { "NOTE FIRST: APS is signed in, so for FILE SEARCH you do not need MCP at all - fusion_aps_search works right now. Only enable MCP if you need its other surface (script text-to-CAD, screenshots, electronics object-model reads). " }
        else if aps.0 { "NOTE: for FILE SEARCH, APS is configured and only needs fusion_aps_signin (silent on a warm SSO session) - that may be less disruptive than enabling MCP. " } else { "" };
    format!("{lead}{OFF_STATIC}")
}

pub fn status(ctx: &Ctx, version: &str, aps: (bool, bool)) -> Value {
    let s = new_session(version);
    if !s["error"].is_null() {
        return json!({"success": true, "enabled": false, "url": MCP_URL, "error": s["error"], "lastEnabledAt": crate::small_verbs::get_pref(&ctx.state_dir, "mcpLastEnabledAt").unwrap_or(Value::Null),
            "fusionRunning": !ctx.host.fusion_processes().is_empty(), "_hint": off_hint(aps), "statusVerb": "fusion_mcp_status"});
    }
    let tl = rpc(version, "tools/list", None, 60);
    let tools: Vec<Value> = if tl["_mcpError"].is_null() { tl["result"]["tools"].as_array().cloned().unwrap_or_default().iter().map(|t| json!({"name": t["name"], "description": t["description"].as_str().unwrap_or("").split('\n').next().unwrap_or("").chars().take(160).collect::<String>()})).collect() } else { vec![] };
    crate::small_verbs::set_pref(&ctx.state_dir, "mcpLastEnabledAt", json!(chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()));
    json!({"success": true, "enabled": true, "url": MCP_URL, "sessionId": s["sessionId"], "serverInfo": s["serverInfo"], "toolCount": tools.len(), "tools": tools,
        "_hint": "Autodesk's Fusion MCP server is LIVE. Its tools are Autodesk's own, not this bridge's: call them with fusion_mcp_call {tool, arguments}. Use fusion_mcp_tools for full input schemas and fusion_mcp_resources for the Electronics entity schemas. These COMPLEMENT the fusion_* verbs: prefer a native verb when one exists (it is tested and returns richer hints), and reach for MCP for Autodesk's text-to-CAD surface.", "statusVerb": "fusion_mcp_status"})
}

pub fn tools(version: &str, aps: (bool, bool)) -> Value {
    let tl = rpc(version, "tools/list", None, 60);
    if !tl["_mcpError"].is_null() { return json!({"success": false, "enabled": false, "error": tl["_mcpError"], "_hint": off_hint(aps)}); }
    let tools = tl["result"]["tools"].as_array().cloned().unwrap_or_default();
    json!({"success": true, "count": tools.len(), "tools": tools, "_hint": "Call one with fusion_mcp_call {\"tool\":\"<name>\",\"arguments\":{...}}.", "statusVerb": "fusion_mcp_status"})
}

pub fn call(version: &str, args: &Value, aps: (bool, bool)) -> Value {
    let Some(tool) = args["tool"].as_str().filter(|s| !s.is_empty()) else { return json!({"success": false, "error": "Pass {tool, arguments}.", "_hint": "fusion_mcp_tools lists the available tools and their input schemas."}) };
    let arguments = if args["arguments"].is_object() { args["arguments"].clone() } else { json!({}) };
    let r = rpc(version, "tools/call", Some(json!({"name": tool, "arguments": arguments})), args["timeout"].as_u64().filter(|t| *t > 0).unwrap_or(120));
    if !r["_mcpError"].is_null() { return json!({"success": false, "error": r["_mcpError"], "_hint": off_hint(aps)}); }
    let res = if r["result"].is_object() { r["result"].clone() } else { json!({}) };
    json!({"success": res["isError"].as_bool() != Some(true), "tool": tool, "result": res,
        "_hint": "Autodesk MCP tool result. Content is usually a list of {type,text} blocks. If it complains about no active document, open one first (fusion_aps_open) - MCP acts on the ACTIVE Fusion document.", "statusVerb": "fusion_mcp_status"})
}

pub fn resources(version: &str, args: &Value, aps: (bool, bool)) -> Value {
    if let Some(uri) = args["uri"].as_str().filter(|s| !s.is_empty()) {
        let r = rpc(version, "resources/read", Some(json!({"uri": uri})), 60);
        if !r["_mcpError"].is_null() { return json!({"success": false, "error": r["_mcpError"], "_hint": off_hint(aps)}); }
        return json!({"success": true, "uri": uri, "result": r["result"], "statusVerb": "fusion_mcp_status"});
    }
    let r = rpc(version, "resources/list", None, 60);
    if !r["_mcpError"].is_null() { return json!({"success": false, "error": r["_mcpError"], "_hint": off_hint(aps)}); }
    let res = r["result"]["resources"].as_array().cloned().unwrap_or_default();
    json!({"success": true, "count": res.len(), "resources": res.iter().map(|x| x["uri"].clone()).collect::<Vec<_>>(), "_hint": "Read one with fusion_mcp_resources {\"uri\":\"resource://...\"}.", "statusVerb": "fusion_mcp_status"})
}

/// Turn Autodesk's MCP server ON by driving Fusion's Preferences dialog (no API exists for the
/// toggle). The section click is image-space and takes the foreground, announced.
pub fn enable(ctx: &Ctx, version: &str, aps: (bool, bool), pass: &dyn Fn(&str, Value) -> Value) -> Value {
    if ctx.host.fusion_processes().is_empty() { return json!({"success": false, "error": "Fusion is not running.", "_hint": "fusion_start first, then fusion_mcp_enable."}); }
    if new_session(version)["error"].is_null() { return json!({"success": true, "enabled": true, "alreadyOn": true, "_hint": "Already enabled; nothing to do. fusion_mcp_status shows the tools."}); }
    let opened = crate::maintenance::prefs_open(ctx, &json!({"section": "api"}), pass);
    let mut steps: Vec<Value> = opened["steps"].as_array().cloned().unwrap_or_default();
    if opened["success"].as_bool() != Some(true) { return json!({"success": false, "error": opened["error"], "steps": steps, "_hint": off_hint(aps)}); }
    let Some(ab) = fusion_core::ab::Loopback::from_env() else { return json!({"success": false, "error": "ab loopback unavailable", "steps": steps}) };
    use fusion_core::ab::AbClient;
    let (hwnd, w, h) = (opened["hwnd"].as_i64().unwrap_or(0), opened["imageWidth"].as_f64().unwrap_or(1400.0), opened["imageHeight"].as_f64().unwrap_or(836.0));
    let _ = ab.call("desktop_click", json!({"space": "image", "shotId": opened["shotId"], "hwnd": hwnd, "x": (w * 0.493) as i64, "y": (h * 0.569) as i64}), &ctx.caller, 30);
    steps.push(json!("ticked 'Fusion MCP Server'"));
    std::thread::sleep(Duration::from_secs(1));
    let closed = crate::maintenance::prefs_close(ctx, &json!({"save": true}));
    steps.push(json!("clicked Apply then OK"));
    if closed["closed"].as_bool() != Some(true) { steps.push(json!("Preferences still open after Apply/OK")); }
    std::thread::sleep(Duration::from_secs(4));
    let s = new_session(version);
    let ok = s["error"].is_null();
    json!({"success": ok, "enabled": ok, "steps": steps, "sessionId": s["sessionId"], "serverInfo": s["serverInfo"],
        "narrate": if ok { "Fusion's MCP server is on. I enabled it for you in Preferences." } else { "I drove the Preferences dialog but the MCP port is still closed." },
        "_hint": if ok { "MCP server ENABLED - fusion_mcp_status lists its tools. Tell the user it is handled; they do not need to touch Preferences." } else { "The toggle did not take. Re-call fusion_mcp_enable - driving Preferences is racy on a busy main thread and a second pass usually lands it. Do NOT hand the user a Preferences walkthrough (describe.py says NEVER ask them to do this by hand); if a second pass also fails, report what the steps[] show and that the setting is discarded when Fusion restarts before Apply." },
        "statusVerb": "fusion_mcp_status"})
}

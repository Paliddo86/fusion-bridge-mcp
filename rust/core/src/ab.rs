//! Calling Adom Bridge ("ab") verbs from inside the bridge. ab is the host app; every
//! desktop-shaped thing (windows, clicks, UIA, screenshots, processes, captions, the demo
//! panel) is one of its verbs, identical on Windows and macOS. This module is the ONLY
//! way the core touches the desktop.
//!
//! Naming: the host app is Adom Bridge, "ab". None of its retired names appear anywhere
//! in this workspace, not even in comments; `tools/check-naming.sh` fails the build if
//! one does (which is why this comment does not spell them out).
//!
//! Wire contract (mirrors what the Python bridge proved in production): POST
//! `<ADOM_DIRECT_API_URL>/command` with `{"command", "args"}`, `X-Adom-Bridge-Token` from
//! `ADOM_BRIDGE_TOKEN`, the caller's `X-Adom-Caller-Thread/Container/Reason` forwarded so
//! ab records the delegation chain, and `X-Adom-Caller-Delegate: fusion`.

use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum AbError {
    #[error("ab loopback API unreachable: {0}")]
    Unreachable(String),
    #[error("verb {verb} refused: {code}: {message}")]
    Refused { verb: String, code: String, message: String },
    #[error("verb {0} failed: {1}")]
    Failed(String, String),
}

/// The caller this bridge is currently acting FOR (ab stamps it on every inbound verb).
#[derive(Debug, Clone, Default)]
pub struct Caller {
    pub thread: String,
    pub container: String,
    pub reason: String,
}

pub trait AbClient: Send + Sync {
    fn call(&self, verb: &str, args: Value, caller: &Caller, timeout_secs: u64) -> Result<Value, AbError>;
}

/// The loopback direct-API client. Reads its base URL and token from the environment ab
/// sets on every bridge it spawns.
pub struct Loopback {
    pub base: String,
    pub token: String,
}

impl Loopback {
    pub fn from_env() -> Option<Loopback> {
        let mut base = std::env::var("ADOM_DIRECT_API_URL").unwrap_or_default().trim().trim_end_matches('/').to_string();
        if base.is_empty() {
            // ab's fallback for a bridge spawned without the env var: ~/.adom/direct-api-port
            let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
            if let Ok(port) = std::fs::read_to_string(std::path::Path::new(&home).join(".adom").join("direct-api-port")) {
                let port = port.trim();
                if !port.is_empty() { base = format!("http://127.0.0.1:{port}"); }
            }
        }
        if base.is_empty() { return None; }
        // ab exempts a bridge's own calls from its caller-identity gate through this token; it
        // is injected at spawn and is the reason a hand-launched exe reports degraded window facts.
        Some(Loopback { base, token: std::env::var("ADOM_BRIDGE_TOKEN").unwrap_or_default() })
    }
}

impl AbClient for Loopback {
    fn call(&self, verb: &str, args: Value, caller: &Caller, timeout_secs: u64) -> Result<Value, AbError> {
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(timeout_secs.max(1))).build();
        let mut req = agent.post(&format!("{}/command", self.base))
            .set("Content-Type", "application/json")
            .set("X-Adom-Caller-Delegate", "fusion");
        if !self.token.is_empty() { req = req.set("X-Adom-Bridge-Token", &self.token); }
        if !caller.thread.is_empty() { req = req.set("X-Adom-Caller-Thread", &caller.thread); }
        if !caller.container.is_empty() { req = req.set("X-Adom-Caller-Container", &caller.container); }
        if !caller.reason.is_empty() { req = req.set("X-Adom-Caller-Reason", &caller.reason); }
        let resp = req.send_json(json!({"command": verb, "args": args}))
            .map_err(|e| AbError::Unreachable(e.to_string()))?;
        let mut v: Value = resp.into_json().map_err(|e| AbError::Failed(verb.into(), e.to_string()))?;
        // ab wraps a bridge verb's JSON in an envelope whose `output` is a JSON STRING; unwrap it.
        // The envelope's own top-level keys (fullPath, safePath, hwnd, ok...) are kept: the
        // inner JSON wins on a clash, the rest is merged in. desktop_screenshot_window's paths
        // live ONLY on the envelope, and dropping them once cost a native screenshot verb.
        if let Some(inner) = v.get("output").and_then(|o| o.as_str()) {
            if inner.trim_start().starts_with('{') {
                if let Ok(mut parsed) = serde_json::from_str::<Value>(inner) {
                    if let (Some(po), Some(eo)) = (parsed.as_object_mut(), v.as_object()) {
                        for (k, val) in eo { if k != "output" { po.entry(k.clone()).or_insert(val.clone()); } }
                    }
                    v = parsed;
                }
            }
        }
        if v.get("success") == Some(&Value::Bool(false)) || v.get("status") == Some(&Value::String("error".into())) {
            let code = v.get("errorCode").and_then(|c| c.as_str()).unwrap_or("error").to_string();
            let message = v.get("error").and_then(|c| c.as_str()).unwrap_or("").to_string();
            return Err(AbError::Refused { verb: verb.into(), code, message });
        }
        Ok(v)
    }
}

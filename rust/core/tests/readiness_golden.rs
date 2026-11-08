//! Every scenario in tests/golden/readiness.json was produced by the Python handler with its
//! helpers stubbed; the Rust computation must return the same result and the same prefs.
use fusion_core::readiness::{compute, Inputs};
use serde_json::{json, Map, Value};
use std::fs;

fn inputs_from(sc: &Value) -> Inputs {
    let i = &sc["inputs"];
    let prefs: Map<String, Value> = i["prefs"].as_object().cloned().unwrap_or_default();
    Inputs {
        installed_raw: i["installed"].as_bool().unwrap(), streaming: i["streaming"].as_bool().unwrap(), running: i["running"].as_bool().unwrap(),
        addin_version: i["addin"].as_str().map(str::to_string), expected_addin_version: "1.0.19".into(),
        needs_signin: { let t = i["title"].as_str().unwrap_or("").to_lowercase(); t.contains("signing in") || t.contains("welcome to fusion") || (t.contains("sign in - autodesk") && t.contains("fusion")) },
        licensing_dialog: i["licensing"].as_bool().unwrap(), updating: i["updating"].as_bool().unwrap(),
        aps_configured: i["apsConfigured"].as_bool().unwrap(), aps_signed_in: i["apsSignedIn"].as_bool().unwrap(),
        mcp_port_open: i["mcp"].as_bool().unwrap(), auto_update_enabled: i["autoUpdate"].as_bool().unwrap(), roundtrip_ok: i["roundtrip"].as_bool().unwrap(),
        kicad_ingest: false, fusion_user: None, modeling_orientation: None, bridge_version: "TEST".into(),
        prefs, now: i["now"].as_f64().unwrap(), now_iso: "2026-09-03T14:00:00".into(),
        timings_launch: json!({"cold": {"p50": 45.0, "p90": 53.0, "count": 4, "lastAt": "2026-09-01T10:00:00"}}),
        launch_stats: json!({"p50": 45.0, "p90": 53.0, "count": 4}),
        progress_override: Some(json!({"phase": "launch", "stepLabel": "", "percent": 40, "elapsedSec": 30, "estimatedSec": 45, "etaSec": 15, "confidence": "measured", "startKind": "cold", "overdue": false})),
        addin_start_failed: None, crash_dialogs_cleared: 0, update_applied_button: None, update_offered: false, addin_busy: None,
    }
}

#[test]
fn readiness_matches_the_python_handler_in_every_scenario() {
    let g: Value = serde_json::from_str(&fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/readiness.json")).unwrap()).unwrap();
    let mut n = 0;
    for (name, sc) in g.as_object().unwrap() {
        let out = compute(&inputs_from(sc));
        let want = &sc["result"];
        let mut got = out.result.clone();
        // fusionUser is a live cache in Python; both are null in the goldens
        if got != *want {
            let (a, b) = (got.as_object_mut().unwrap(), want.as_object().unwrap());
            for k in a.keys().cloned().collect::<Vec<_>>().iter().chain(b.keys()) {
                if a.get(k) != b.get(k) { panic!("{name}: key {k}\n  rust:   {}\n  python: {}", a.get(k).map(|v| v.to_string()).unwrap_or("<absent>".into()), b.get(k).map(|v| v.to_string()).unwrap_or("<absent>".into())); }
            }
        }
        let want_prefs: Map<String, Value> = sc["prefsAfter"].as_object().cloned().unwrap_or_default();
        assert_eq!(Value::Object(out.prefs_after.clone()), Value::Object(want_prefs), "{name}: prefs after");
        n += 1;
    }
    assert!(n >= 16, "{n} scenarios");
}

/// 2026-10-28: a 93 s script starved the add-in's probes, readiness read it as a launch, and the
/// launch observer would have restarted Fusion after 180 s. A busy, loaded add-in is not a launch.
#[test]
fn a_busy_addin_is_neither_a_launch_nor_a_restart() {
    let base = |busy: Option<Value>, prefs: Map<String, Value>, now: f64| Inputs {
        installed_raw: true, running: true, addin_version: None, expected_addin_version: "1.0.19".into(),
        auto_update_enabled: true, bridge_version: "TEST".into(), prefs, now, now_iso: "2026-10-28T00:09:00".into(),
        timings_launch: json!({}), launch_stats: json!({"p50": 45.0, "p90": 53.0, "count": 4}), addin_busy: busy, ..Default::default()
    };
    let ev = json!({"evidence": "request_in_flight", "busyCommand": "run_modeling_script", "elapsedSeconds": 31.0});
    // busy: no launch timer, a busy hint, the evidence in the reply
    let out = compute(&base(Some(ev.clone()), Map::new(), 1000.0));
    assert_eq!(out.result["ready"], json!(false));
    assert_eq!(out.result["addinBusy"], ev);
    assert!(out.result["_hint"].as_str().unwrap().contains("LOADED but busy"));
    assert!(out.prefs_after.get("launchStartedAt").map(|v| v.is_null()).unwrap_or(true));
    // a timer left from earlier silence is cleared, and 10 minutes later nothing heals
    let mut prefs = Map::new();
    prefs.insert("launchStartedAt".into(), json!(400.0));
    let out = compute(&base(Some(ev.clone()), prefs, 1000.0));
    assert!(out.actions.heal_launch.is_none());
    assert!(out.result.get("addinRestartTriggered").is_none());
    assert_eq!(out.prefs_after["launchStartedAt"], Value::Null);
    // without evidence the observer still heals a launch that never served (unchanged)
    let mut prefs = Map::new();
    prefs.insert("launchStartedAt".into(), json!(400.0));
    let out = compute(&base(None, prefs, 1000.0));
    assert_eq!(out.actions.heal_launch, Some(1));
}

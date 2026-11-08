//! Phase 3 layer 2: the window facts readiness needs, from ab's `desktop_list_windows`
//! (owner, pid, image, rect per row), never local Win32. Ported rule-for-rule from
//! `fusion_detect.family_windows` / `find_licensing_dialog` and the sign-in title check
//! in `_handle_fusion_readiness`, with their hard-won comments kept short here:
//!
//! - The launch family is Fusion360.exe / FusionLauncher.exe / AdskIdentityManager.exe.
//! - `needsSignin`: the MAIN Fusion window's title says "Signing in" / "Welcome to Fusion"
//!   / "Sign in - Autodesk ... Fusion". Title of the PROCESS window, never a browser tab.
//! - The seat / licensing modal ("Active Sessions Exceeded", "Suspend Remote Session") is
//!   an OWNED popup of the main window, DIALOG-SHAPED: width >= 480, height >= 180,
//!   landscape (w >= 1.3 h), not a docked rail (h <= 0.60 of the app's height), and never
//!   larger than 0.85 of the app's area. Never the largest family window. Every threshold
//!   is a ratio on purpose: two absolute pixel gates each hid this dialog once.

use crate::ab::{AbClient, Caller};
use serde_json::{json, Value};

pub const FAMILY_IMAGES: [&str; 3] = ["Fusion360.exe", "FusionLauncher.exe", "AdskIdentityManager.exe"];

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct WindowFacts {
    /// The launch family's visible top-level rows (ab's shape, untouched).
    pub family: Vec<Value>,
    #[serde(rename = "mainTitle")] pub main_title: Option<String>,
    #[serde(rename = "mainHwnd")] pub main_hwnd: Option<i64>,
    #[serde(rename = "needsSignin")] pub needs_signin: bool,
    /// The seat / licensing modal row when one is up.
    #[serde(rename = "licensingDialog")] pub licensing_dialog: Option<Value>,
    /// ab could not be asked (loopback missing or refused): facts unknown, not "none".
    pub degraded: bool,
    /// Why, when degraded (the ab error text), so a smoke test can tell 403 from absent.
    #[serde(skip_serializing_if = "Option::is_none")] pub error: Option<String>,
}

fn area(w: &Value) -> i64 { w["rect"]["width"].as_i64().unwrap_or(0) * w["rect"]["height"].as_i64().unwrap_or(0) }

/// The pure rule over ab's rows; `facts_from_ab` is the same after one verb call.
pub fn facts_from_rows(rows: &[Value]) -> WindowFacts {
    let family: Vec<Value> = rows.iter().filter(|w| {
        let img = w["image"].as_str().unwrap_or("");
        FAMILY_IMAGES.iter().any(|f| f.eq_ignore_ascii_case(img)) && !w["minimized"].as_bool().unwrap_or(false) || FAMILY_IMAGES.iter().any(|f| f.eq_ignore_ascii_case(img)) && w["kind"].as_str() == Some("normal")
    }).cloned().collect();
    let mut out = WindowFacts { family: family.clone(), ..Default::default() };
    // The main window is the one Fusion titles "... - Autodesk Fusion". Largest-area alone picked
    // the bridge's own activity palette ("Adom", 570x615) whenever the real window was minimized
    // (its rect collapses to a sliver), and every capture then photographed the palette
    // (AdomLapper gate, 2026-09-20).
    let is_palette = |w: &Value| w["title"].as_str().map(|t| t.trim().eq_ignore_ascii_case(crate::dialogs::BRIDGE_PALETTE_TITLE)).unwrap_or(false);
    let titled: Vec<&Value> = family.iter().filter(|w| w["title"].as_str().map(|t| t.contains("Autodesk Fusion")).unwrap_or(false)).collect();
    let main_opt = if !titled.is_empty() { titled.into_iter().max_by_key(|w| area(w)) } else { family.iter().filter(|w| !is_palette(w)).max_by_key(|w| area(w)) };
    let Some(main) = main_opt else { return out };
    out.main_hwnd = main["hwnd"].as_i64();
    out.main_title = main["title"].as_str().map(str::to_string);
    let t = out.main_title.clone().unwrap_or_default().to_lowercase();
    out.needs_signin = t.contains("signing in") || t.contains("welcome to fusion") || (t.contains("sign in - autodesk") && t.contains("fusion"));
    let main_area = area(main).max(1);
    let main_h = main["rect"]["height"].as_i64().unwrap_or(0).max(1) as f64;
    let dialog_shaped = |w: &Value| -> bool {
        // a tool window (tray, palette, popup: Fusion's 640x196 message tray is one) is never a modal
        if w["kind"].as_str() == Some("toolwindow") || w["className"].as_str().map(|c| c.ends_with("ToolSaveBits")).unwrap_or(false) { return false; }
        let (ww, hh) = (w["rect"]["width"].as_i64().unwrap_or(0) as f64, w["rect"]["height"].as_i64().unwrap_or(0) as f64);
        if ww < 480.0 || hh < 180.0 { return false; }
        if ww < hh * 1.3 { return false; }
        if hh > 0.60 * main_h { return false; }
        (ww * hh) <= 0.85 * main_area as f64
    };
    let owned = family.iter().find(|w| w["hwnd"] != main["hwnd"] && w["owner"].as_i64().unwrap_or(0) != 0 && dialog_shaped(w));
    let fallback = || family.iter().find(|w| w["hwnd"] != main["hwnd"] && dialog_shaped(w)).cloned();
    out.licensing_dialog = owned.cloned().or_else(|| if family.iter().all(|w| w["owner"].as_i64().unwrap_or(0) == 0) { fallback() } else { None });
    out
}

/// One `desktop_list_windows` call, then the rule.
pub fn facts_from_ab(ab: &dyn AbClient, caller: &Caller) -> WindowFacts {
    match ab.call("desktop_list_windows", json!({}), caller, 10) {
        Ok(v) => {
            let rows = v["data"]["windows"].as_array().or_else(|| v["windows"].as_array()).cloned().unwrap_or_default();
            facts_from_rows(&rows)
        }
        Err(e) => WindowFacts { degraded: true, error: Some(e.to_string()), ..Default::default() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(hwnd: i64, owner: i64, title: &str, w: i64, h: i64, kind: &str) -> Value {
        json!({"hwnd": hwnd, "owner": owner, "pid": 1, "image": "Fusion360.exe", "title": title, "kind": kind, "className": if kind == "toolwindow" { "Qt683QWindowToolSaveBits" } else { "Qt683QWindow" }, "minimized": false, "rect": {"width": w, "height": h}})
    }
    #[test]
    fn seat_dialog_shapes_from_the_field_notes() {
        let main = row(1, 0, "Untitled - Autodesk Fusion", 3866, 2090, "normal");
        for (w, h) in [(823, 262), (1235, 527), (1646, 702)] {
            let f = facts_from_rows(&[main.clone(), row(2, 1, "Fusion360", w, h, "normal")]);
            assert!(f.licensing_dialog.is_some(), "{w}x{h} is a seat dialog");
        }
        // the docked rail that false-fired twice: portrait, 78% of the app height
        let f = facts_from_rows(&[main.clone(), row(2, 1, "Fusion360", 600, 1620, "normal")]);
        assert!(f.licensing_dialog.is_none(), "portrait rail");
        // the message tray that false-fired for hours on ConfRoomROG (2026-09-03)
        let f = facts_from_rows(&[main.clone(), row(2, 1, "Fusion360", 640, 196, "toolwindow")]);
        assert!(f.licensing_dialog.is_none(), "tool window");
        // narrow side panels and toolbar strips
        let f = facts_from_rows(&[main.clone(), row(2, 1, "BROWSER", 440, 224, "toolwindow"), row(3, 1, "Fusion360", 640, 196, "toolwindow")]);
        assert!(f.licensing_dialog.is_none());
        assert!(!f.needs_signin);
    }
    #[test]
    fn sign_in_titles() {
        for t in ["Signing in - Autodesk Fusion", "Welcome to Fusion", "Sign in - Autodesk Fusion"] {
            assert!(facts_from_rows(&[row(1, 0, t, 1300, 820, "normal")]).needs_signin, "{t}");
        }
        assert!(!facts_from_rows(&[row(1, 0, "Home - Autodesk Fusion", 1300, 820, "normal")]).needs_signin);
    }
    #[test]
    fn a_non_maximized_main_is_never_the_dialog() {
        let f = facts_from_rows(&[row(1, 0, "Home - Autodesk Fusion", 1300, 820, "normal")]);
        assert!(f.licensing_dialog.is_none());
    }

    #[test]
    fn palette_is_never_the_main_window() {
        let rows = vec![
            serde_json::json!({"hwnd": 1, "image": "Fusion360.exe", "title": "gate-tof-brd* - Autodesk Fusion", "kind": "normal", "minimized": true, "owner": 0, "rect": {"left": -32000, "top": -32000, "width": 237, "height": 39}}),
            serde_json::json!({"hwnd": 2, "image": "Fusion360.exe", "title": "ADOM", "kind": "normal", "minimized": false, "owner": 1, "rect": {"left": 100, "top": 100, "width": 570, "height": 615}}),
        ];
        let f = facts_from_rows(&rows);
        assert_eq!(f.main_hwnd, Some(1));
    }
}

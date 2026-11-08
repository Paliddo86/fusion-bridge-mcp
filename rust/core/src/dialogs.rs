//! Fusion's blocking dialogs, classified by title (or body text). The rule table is
//! `dialog_rules.json`, the only one the exe reads: it was first generated from the retired Python
//! bridge's `handlers/dialog_classify.py` and is edited directly now. The goldens in
//! `tests/golden/dialog_titles.json` pin the result for a spread of real titles.

use serde_json::Value;

pub const RULES_JSON: &str = include_str!("dialog_rules.json");

fn rules() -> &'static Value {
    static CELL: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    CELL.get_or_init(|| serde_json::from_str(RULES_JSON).unwrap_or(Value::Null))
}

/// `_category_for_title`: (category, resolution), or None for an empty title.
pub fn category_for_title(title: &str) -> Option<(String, String)> {
    let t = title.trim().to_lowercase();
    if t.is_empty() { return None; }
    let r = rules();
    for rule in r["rules"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        if rule["needles"].as_array().map(|n| n.iter().any(|s| s.as_str().map(|s| t.contains(s)).unwrap_or(false))).unwrap_or(false) {
            return Some((rule["category"].as_str().unwrap_or("").to_string(), rule["resolution"].as_str().unwrap_or("").to_string()));
        }
    }
    if ["fusion", "fusion360", "autodesk fusion 360", "fusion 360"].contains(&t.as_str()) {
        return Some((r["generic"]["category"].as_str().unwrap_or("").to_string(), r["generic"]["resolution"].as_str().unwrap_or("").to_string()));
    }
    Some((r["unknown"]["category"].as_str().unwrap_or("").to_string(), r["unknown"]["resolution"].as_str().unwrap_or("").to_string()))
}

pub fn category_set(name: &str) -> Vec<String> {
    rules()["sets"][name].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// `_find_qt_dialog_windows` over ab's window rows: visible Qt-class windows that are not the
/// main window, minus small / tall docked tool panels.
/// the title Fusion gives the bridge's activity palette (`ui.palettes.add(PALETTE_ID, "Adom", ...)`)
pub const BRIDGE_PALETTE_TITLE: &str = "Adom";

pub fn qt_dialog_rows(rows: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for w in rows {
        if w["visible"].as_bool() == Some(false) { continue; }
        let class = w["className"].as_str().or(w["class"].as_str()).unwrap_or("");
        let title = w["title"].as_str().unwrap_or("");
        if !class.contains("Qt") || title.contains("Autodesk Fusion") { continue; }
        // the bridge's own activity palette is a 760x820 Qt tool window titled "Adom": a UIA
        // control search over its HTML costs seconds, and it is never a dialog (2026-09-05:
        // every doc-switching verb paid ~13 s of post-envelope for exactly that search)
        if title.trim().eq_ignore_ascii_case(BRIDGE_PALETTE_TITLE) { continue; }
        // Fusion's dockable panels (BROWSER, message tray, out-of-sync banner) are Qt tool windows
        // named with a QTView marker, sometimes in the title and sometimes only in their UIA text
        // (a floating BROWSER is titled just "Fusion360"). They are never dialogs (2026-09-22, #97).
        if title.contains("QTView") { continue; }
        let rect = rect_of(&w["rect"]);
        let (ww, hh) = (rect["width"].as_i64().unwrap_or(0), rect["height"].as_i64().unwrap_or(0));
        // tool windows shaped like a PANEL (taller than wide: a floating BROWSER is 600x800 and titled
        // just "Fusion360") are docked-panel frames, never modal dialogs. Skipping them here also saves
        // the seconds a UI Automation read of their contents costs on every verb (2026-09-22, #97).
        if class.contains("Tool") && (ww < 400 || hh < 300 || hh > ww) { continue; }
        out.push(serde_json::json!({"hwnd": w["hwnd"], "title": title, "className": class, "rect": rect}));
    }
    out
}

/// ab answers a rect as `[left, top, right, bottom]` (desktop_enum_windows) or as the
/// `{left, top, width, height}` object (desktop_list_windows); normalise to Python's dict.
pub fn rect_of(r: &Value) -> Value {
    if let Some(a) = r.as_array() {
        if a.len() == 4 {
            let (l, t, rr, b) = (a[0].as_i64().unwrap_or(0), a[1].as_i64().unwrap_or(0), a[2].as_i64().unwrap_or(0), a[3].as_i64().unwrap_or(0));
            return serde_json::json!({"left": l, "top": t, "right": rr, "bottom": b, "width": rr - l, "height": b - t});
        }
    }
    let (l, t) = (r["left"].as_i64().or(r["x"].as_i64()).unwrap_or(0), r["top"].as_i64().or(r["y"].as_i64()).unwrap_or(0));
    let (w, h) = (r["width"].as_i64().or_else(|| r["right"].as_i64().map(|x| x - l)).unwrap_or(0), r["height"].as_i64().or_else(|| r["bottom"].as_i64().map(|y| y - t)).unwrap_or(0));
    serde_json::json!({"left": l, "top": t, "right": l + w, "bottom": t + h, "width": w, "height": h})
}

#[cfg(test)]
mod tests {
    #[test]
    fn assertion_body_classifies() {
        let c = super::category_for_title("Fusion Electronics has detected an unexpected condition. Please report this error ... File: R:\\Electron\\EAGLE\\src\\pusher.cpp, Line 492. Condition: inLocator").unwrap();
        assert_eq!(c.0, "fusion_unexpected_condition");
        assert_eq!(super::category_for_title("Fusion").unwrap().0, "generic_modal_read_screenshot");
    }
    #[test]
    fn own_palette_is_never_a_dialog() {
        let rows = vec![
            serde_json::json!({"className": "Qt683QWindowToolSaveBits", "hwnd": 1379664, "pid": 16004, "rect": [1900, 300, 2660, 1120], "title": "ADOM", "visible": true}),
            serde_json::json!({"className": "Qt683QWindow", "hwnd": 42, "pid": 16004, "rect": [100, 100, 700, 400], "title": "Save changes?", "visible": true}),
        ];
        let out = super::qt_dialog_rows(&rows);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0]["hwnd"], 42);
    }
}

#[cfg(test)]
mod panel_tests {
    use super::*;
    #[test]
    fn fusion_panels_are_not_dialogs() {
        let rows = vec![
            serde_json::json!({"hwnd": 1, "title": "QTView:(102)Pcb2d3dOutOfSyncBanner", "className": "Qt683QWindowToolSaveBits", "visible": true, "rect": [824, 288, 1602, 336]}),
            serde_json::json!({"hwnd": 2, "title": "Gate check", "className": "Qt683QWindowIcon", "visible": true, "rect": [800, 400, 1320, 640]}),
        ];
        let d = qt_dialog_rows(&rows);
        assert_eq!(d.len(), 1); assert_eq!(d[0]["hwnd"], 2);
    }
}

#[cfg(test)]
mod gpu_tests {
    #[test]
    fn d3d12_assertion_is_the_gpu_not_the_router() {
        let c = super::category_for_title("Fusion Electronics has detected an unexpected condition. File: R:\\Electron\\EAGLE\\SkiaBridge\\SkiaWindowContextDx12.cpp, Line 1022. Condition: Unrecoverable d3d12 error!").unwrap();
        assert_eq!(c.0, "fusion_gpu_device_lost");
        let r = super::category_for_title("Fusion Electronics has detected an unexpected condition. File: R:\\Electron\\EAGLE\\src\\pusher.cpp, Line 492. Condition: inLocator").unwrap();
        assert_eq!(r.0, "fusion_unexpected_condition");
    }
}

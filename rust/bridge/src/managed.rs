//! `fusion_library_manager` and `fusion_managed_library_*`: Fusion's library.io choreography,
//! driven by measurement (ribbon DIPs scaled by the window's real DPI, the LIBRARY.IO popup
//! located as a window, CEF dialogs answered by rect fraction) and judged by Fusion's own
//! alert log, never by hope.
use crate::addin::Ctx;
use fusion_core::ab::{AbClient, Loopback};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// LIBRARY.IO ribbon targets in DIPs from the client origin. Re-measured 2026-09-04 on
/// ConfRoomROG after Fusion's morning update moved the ribbon tab row down ~11 DIP (the
/// 2026-09-01 numbers were 42 / 103 / 80); `group_alt` is the older group row, tried second.
const LIBIO_DIP: &[(&str, f64, f64)] = &[("manage_tab", 296.0, 53.0), ("group", 455.0, 113.0), ("group_alt", 455.0, 103.0), ("open_icon", 352.0, 86.0)];
/// The dropdown's four items, in order; the order does not change with link state.
const LIBIO_MENU_ITEMS: &[&str] = &["push", "pull", "link", "unlink"];
const PICKER_COLUMNS: &[&str] = &["name", "type", "version", "managedFolder", "description"];

fn sleep_s(s: f64) { std::thread::sleep(Duration::from_secs_f64(s)); }
fn ab_call(ctx: &Ctx, verb: &str, args: Value, timeout: u64) -> Value {
    Loopback::from_env().and_then(|ab| ab.call(verb, args, &ctx.caller, timeout).ok()).unwrap_or(json!({}))
}
/// `_find_key`: the first value for `key` anywhere in a nested ab response.
pub fn find_key(v: &Value, key: &str, depth: u8) -> Option<Value> {
    if depth > 6 { return None; }
    match v {
        Value::Object(o) => { if let Some(x) = o.get(key) { return Some(x.clone()); } o.values().find_map(|x| find_key(x, key, depth + 1)) }
        Value::Array(a) => a.iter().take(20).find_map(|x| find_key(x, key, depth + 1)),
        Value::String(s) if s.trim_start().starts_with('{') && s.contains(key) => serde_json::from_str::<Value>(s).ok().and_then(|p| find_key(&p, key, depth + 1)),
        _ => None,
    }
}
fn windows(ctx: &Ctx) -> Vec<Value> { find_key(&ab_call(ctx, "desktop_list_windows", json!({}), 25), "windows", 0).and_then(|w| w.as_array().cloned()).unwrap_or_default() }

/// `_fusion_main_window` + `_window_dpi_scale`: {hwnd, rect, dpiScale, title} or None.
fn fusion_main_window(ctx: &Ctx) -> Option<Value> {
    let w = windows(ctx).into_iter().find(|w| w["title"].as_str().unwrap_or("").contains("Autodesk Fusion") && w["kind"].as_str() == Some("normal"))?;
    let hwnd = w["hwnd"].as_i64()?;
    Some(json!({"hwnd": hwnd, "rect": w["rect"], "dpiScale": ctx.host.window_dpi_scale(hwnd), "title": w["title"]}))
}
fn alerts(_ctx: &Ctx, since: f64) -> Vec<Value> { crate::native_alerts(&*crate::host(), since)["alerts"].as_array().cloned().unwrap_or_default() }
/// `_alert_watermark`: the stamp of Fusion's most recent alert right now, so a verb only ever
/// judges itself by its OWN alerts.
fn alert_watermark(ctx: &Ctx) -> String { alerts(ctx, 3600.0).last().and_then(|a| a["at"].as_str()).unwrap_or("").to_string() }
fn alerts_since(ctx: &Ctx, mark: &str) -> Vec<Value> { let rows = alerts(ctx, 300.0); if mark.is_empty() { rows } else { rows.into_iter().filter(|r| r["at"].as_str().unwrap_or("") > mark).collect() } }

/// `_library_in_front`: (ok, doc, workspace) for the document Fusion actually has in front.
fn library_in_front(pass: &dyn Fn(&str, Value) -> Value) -> (bool, String, String) {
    let st = pass("get_app_state", json!({}));
    let doc = st["data"]["activeDocument"].as_str().unwrap_or("").to_string();
    let ws = st["data"]["activeWorkspace"].as_str().unwrap_or("").to_string();
    (ws.to_lowercase().contains("librar"), doc, ws)
}
fn not_a_library(doc: &str, ws: &str) -> Value {
    json!({"success": false, "errorCode": "not_a_library", "error": format!("The document in front is {} in the {} workspace, not a library.", fusion_lbr::py_repr_str(doc), if ws.is_empty() { "unknown" } else { ws }),
        "_hint": "fusion_open_lbr {lbrPath} first - it lands the library editor in front and verifies it, which activate_document alone does NOT."})
}
fn hub_check(pass: &dyn Fn(&str, Value) -> Value, doc: &str, with_open: bool) -> Value {
    let want = doc.replace('*', "").trim().to_string();
    let extra = if with_open { " 'id': (df.id if df else None), 'open': [app.documents.item(i).name for i in range(app.documents.count)][:8]" } else { "" };
    let script = format!("import adsk.core\napp=adsk.core.Application.get()\nwant={}\nfound=None\nfor i in range(app.documents.count):\n    dd=app.documents.item(i)\n    if (dd.name or '').replace('*','').strip()==want:\n        found=dd; break\ndf=None\nif found is not None:\n    try: df=found.dataFile\n    except Exception: df=None\nresult={{'name': (found.name if found else None), 'matched': found is not None, 'inHub': df is not None,{}}}\n", fusion_lbr::py_repr_str(&want), extra);
    pass("run_modeling_script", json!({"script": script}))["data"]["result"].clone()
}
/// `_managed_precheck`: a LIBRARY is in front, and (when the action needs it) it lives in a Hub.
fn precheck(ctx: &Ctx, pass: &dyn Fn(&str, Value) -> Value, require_hub: bool) -> Result<(String, String, Value), Value> {
    let (ok, doc, ws) = library_in_front(pass);
    if !ok { return Err(not_a_library(&doc, &ws)); }
    let Some(win) = fusion_main_window(ctx) else { return Err(json!({"success": false, "errorCode": "no_fusion_window", "error": "Could not locate Fusion's main window."})) };
    if require_hub {
        let res = hub_check(pass, &doc, false);
        if res["inHub"].as_bool() != Some(true) {
            return Err(json!({"success": false, "errorCode": "not_in_hub", "error": format!("Library {} is not saved in a Fusion Hub.", fusion_lbr::py_repr_str(res["name"].as_str().unwrap_or(&doc))), "remedyVerb": "fusion_save_to_cloud",
                "_hint": "fusion_save_to_cloud {name, projectName, folderPath} first; a real library save returns a fileId ending in .flbr."}));
        }
    }
    Ok((doc, ws, win))
}

/// `_ribbon_click`: a LIBRARY.IO ribbon target, re-foregrounding Fusion in the same call
/// (the user is constantly alt-tabbing away), except a popup item, where refocusing would
/// dismiss the menu.
fn ribbon_click(ctx: &Ctx, win: &Value, key: &str, why: &str) -> Value {
    let scale = win["dpiScale"].as_f64().unwrap_or(1.0);
    let hwnd = win["hwnd"].as_i64().unwrap_or(0);
    let (ox, oy) = ctx.host.client_origin(hwnd);
    let (dx, dy) = LIBIO_DIP.iter().find(|k| k.0 == key).map(|k| (k.1, k.2)).unwrap_or((0.0, 0.0));
    let (x, y) = ((ox as f64 + dx * scale) as i64, (oy as f64 + dy * scale) as i64);
    let _ = ab_call(ctx, "desktop_click", json!({"hwnd": hwnd, "x": x, "y": y, "cursor": false, "reason": why}), 25);
    json!({"step": key, "at": [x, y], "scale": scale})
}
/// `_popup_menu_rect`: the open LIBRARY.IO dropdown, measured as a window (Qt683QWindow titled
/// "Fusion360", menu-sized, topmost), never read.
fn popup_menu_rect(ctx: &Ctx, timeout: f64) -> Option<Value> {
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    while Instant::now() < deadline {
        let mut cands: Vec<(i64, Value)> = windows(ctx).into_iter().filter(|w| {
            let r = &w["rect"]; let (wd, ht) = (r["width"].as_i64().unwrap_or(0), r["height"].as_i64().unwrap_or(0));
            w["className"].as_str() == Some("Qt683QWindow") && w["title"].as_str() == Some("Fusion360") && (200..=1400).contains(&wd) && (60..=700).contains(&ht)
        }).map(|w| (w["z"].as_i64().unwrap_or(99), w)).collect();
        if !cands.is_empty() {
            cands.sort_by_key(|c| c.0);
            let w = &cands[0].1; let r = &w["rect"];
            return Some(json!({"hwnd": w["hwnd"], "left": r["left"].as_i64().unwrap_or(0), "top": r["top"].as_i64().unwrap_or(0), "width": r["width"].as_i64().unwrap_or(0), "height": r["height"].as_i64().unwrap_or(0)}));
        }
        sleep_s(0.4);
    }
    None
}
/// `_libio_menu`: MANAGE tab -> LIBRARY.IO group -> one item by NAME. None on success.
fn libio_menu(ctx: &Ctx, win: &Value, item: &str, why: &str, steps: &mut Vec<Value>) -> Option<Value> {
    let Some(idx) = LIBIO_MENU_ITEMS.iter().position(|i| *i == item) else { return Some(json!({"success": false, "errorCode": "bad_menu_item", "error": format!("{} is not a LIBRARY.IO menu item; expected one of {}", fusion_lbr::py_repr_str(item), LIBIO_MENU_ITEMS.join(", "))})) };
    steps.push(ribbon_click(ctx, win, "manage_tab", "open the library editor's MANAGE tab"));
    sleep_s(1.4);
    let mut rect = None;
    for attempt in 1..=2 {
        steps.push(ribbon_click(ctx, win, if attempt == 1 { "group" } else { "group_alt" }, "open the LIBRARY.IO command group"));
        sleep_s(1.0);
        rect = popup_menu_rect(ctx, 6.0);
        if rect.is_some() { break; }
        steps.push(json!({"step": "group_retry", "attempt": attempt}));
    }
    let Some(rect) = rect else { return Some(json!({"success": false, "errorCode": "libio_menu_missing", "steps": steps, "error": "The LIBRARY.IO dropdown did not open.",
        "_hint": "Almost always means a LIBRARY is not really in front - Fusion's API can report a library as active while an empty Untitled design holds the visible tab. Re-open the library, confirm the ribbon reads COMPONENT/DOCUMENT/MANAGE (not SOLID/SURFACE), then retry."})) };
    let n = LIBIO_MENU_ITEMS.len() as f64;
    let x = rect["left"].as_i64().unwrap_or(0) + rect["width"].as_i64().unwrap_or(0) / 2;
    let y = (rect["top"].as_i64().unwrap_or(0) as f64 + (idx as f64 + 0.5) * rect["height"].as_i64().unwrap_or(0) as f64 / n) as i64;
    let _ = ab_call(ctx, "desktop_mouse_click", json!({"x": x, "y": y, "reason": why}), 25);
    steps.push(json!({"step": item, "at": [x, y], "menuRect": rect}));
    sleep_s(2.0);
    None
}
/// `_wait_for_top_window`: a top-level window whose title contains `want` (Fusion's own dialogs
/// are real windows, unlike its ribbon and menus).
fn wait_for_top_window(ctx: &Ctx, want: &str, timeout: f64) -> Option<Value> {
    let w = want.to_lowercase();
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    while Instant::now() < deadline {
        if let Some(x) = windows(ctx).into_iter().find(|x| x["title"].as_str().unwrap_or("").to_lowercase().contains(&w)) { return Some(x); }
        sleep_s(0.8);
    }
    None
}
/// True once no window matching `want` remains (the list lags a beat behind a dismissal).
fn wait_for_top_window_gone(ctx: &Ctx, want: &str, timeout: f64) -> bool {
    let w = want.to_lowercase();
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    while Instant::now() < deadline {
        if let Some(list) = find_key(&ab_call(ctx, "desktop_list_windows", json!({}), 25), "windows", 0).and_then(|v| v.as_array().cloned()) {
            if !list.iter().any(|x| x["title"].as_str().unwrap_or("").to_lowercase().contains(&w)) { return true; }
        }
        sleep_s(1.0);
    }
    false
}
/// `_wait_for_dialog`: a REAL top-level Fusion dialog (dialog array) whose title contains `want`.
fn wait_for_dialog(ctx: &Ctx, want: &str, timeout: f64) -> Option<Value> {
    let w = want.to_lowercase();
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    while Instant::now() < deadline {
        if let Some(d) = crate::small_verbs::capture_dialog_array(&ctx.caller, &ctx.pids, 200) {
            if let Some(x) = d["dialogs"].as_array().and_then(|a| a.iter().find(|x| x["title"].as_str().unwrap_or("").to_lowercase().contains(&w))) { return Some(x.clone()); }
        }
        // Library Manager is a top-level Qt window of Fusion's own (not a dialog row): the
        // window list sees it when the dialog array does not (ConfRoomROG, 2026-09-04)
        if let Some(x) = windows(ctx).into_iter().find(|x| x["title"].as_str().unwrap_or("").to_lowercase().contains(&w)) { return Some(x); }
        sleep_s(1.0);
    }
    None
}
/// `_click_cef_button`: a button in a CEF-drawn Fusion dialog by rect fraction (UIA sees nothing).
fn click_cef_button(ctx: &Ctx, win: &Value, xf: f64, yf: f64, why: &str) -> bool {
    let r = &win["rect"];
    let (Some(left), Some(top)) = (r["left"].as_f64(), r["top"].as_f64()) else { return false };
    let (w, h) = (r["width"].as_f64().unwrap_or(0.0), r["height"].as_f64().unwrap_or(0.0));
    if w <= 0.0 || h <= 0.0 { return false; }
    let _ = ab_call(ctx, "desktop_click", json!({"x": (left + w * xf) as i64, "y": (top + h * yf) as i64, "hwnd": win["hwnd"], "cursor": false, "reason": why}), 25);
    true
}
/// `_handle_update_prompt`: "force update your library?" -> [Merge] [Replace] [Cancel]; Cancel by
/// default because Replace DESTROYS the local library.
fn handle_update_prompt(ctx: &Ctx, choice: &str, timeout: f64) -> Option<Value> {
    let label = match choice { "merge" => "Merge Latest Version", "replace" => "Replace Local Library", _ => "Cancel" };
    let win = wait_for_top_window(ctx, "Update", timeout)?;
    let frac = match label { "Merge Latest Version" => 0.396, "Replace Local Library" => 0.689, _ => 0.903 };
    if !click_cef_button(ctx, &win, frac, 0.824, &format!("answer Fusion's library Update prompt with {label}")) {
        return Some(json!({"prompt": "Update", "answered": false, "choice": label, "error": "Could not measure the Update dialog to answer it."}));
    }
    sleep_s(2.0);
    let gone = wait_for_top_window_gone(ctx, "Update", 20.0);
    Some(json!({"prompt": "Update", "choice": label, "answered": gone, "dialogClosed": gone}))
}
/// `_dismiss_ok_dialog`: a single-OK CEF result dialog ("Up to Date"), rightmost button.
fn dismiss_ok_dialog(ctx: &Ctx, title: &str, timeout: f64) -> Option<Value> {
    let win = wait_for_top_window(ctx, title, timeout)?;
    let clicked = click_cef_button(ctx, &win, 0.903, 0.824, &format!("dismiss Fusion's {title} dialog"));
    sleep_s(1.5);
    let gone = wait_for_top_window_gone(ctx, title, 20.0);
    Some(json!({"dialog": title, "clicked": clicked, "dismissed": gone}))
}
/// `_confirm_create_version`: "Create New Version" [Create] [Cancel], which is what performs a push.
fn confirm_create_version(ctx: &Ctx, choice: &str, timeout: f64) -> Option<Value> {
    let win = wait_for_top_window(ctx, "Create New Version", timeout)?;
    let frac = if choice == "create" { 0.565 } else { 0.818 };
    let clicked = click_cef_button(ctx, &win, frac, 0.824, &format!("answer Fusion's Create New Version prompt with {choice}"));
    sleep_s(2.0);
    let gone = wait_for_top_window_gone(ctx, "Create New Version", 20.0);
    Some(json!({"prompt": "Create New Version", "choice": choice, "clicked": clicked, "answered": gone, "dialogClosed": gone}))
}
/// `_picker_rows`: the Select Managed Library table (UIA TreeItems, five cells per row, only
/// the realized rows).
fn picker_rows(ctx: &Ctx, hwnd: &Value) -> (Vec<Value>, Option<String>) {
    let resp = ab_call(ctx, "desktop_ui_tree", json!({"hwnd": hwnd, "maxDepth": 14}), 60);
    let mut cells: Vec<(String, Value, Value)> = Vec::new();
    fn walk(n: &Value, cells: &mut Vec<(String, Value, Value)>) {
        match n {
            Value::Object(o) => {
                if o.get("role").and_then(|r| r.as_str()) == Some("TreeItem") { if let Some(name) = o.get("name").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) { let r = o.get("rect").cloned().unwrap_or(json!({})); cells.push((name.to_string(), r["centerX"].clone(), r["centerY"].clone())); } }
                for v in o.values() { if v.is_object() || v.is_array() { walk(v, cells); } }
            }
            Value::Array(a) => { for v in a { walk(v, cells); } }
            _ => {}
        }
    }
    walk(&resp, &mut cells);
    let n = PICKER_COLUMNS.len();
    let mut rows = Vec::new();
    let mut i = 0;
    while i + n <= cells.len() {
        let chunk = &cells[i..i + n];
        let mut row = json!({});
        for (k, c) in PICKER_COLUMNS.iter().zip(chunk) { row[*k] = json!(c.0); }
        row["_at"] = json!([chunk[0].1, chunk[0].2]);
        rows.push(row);
        i += n;
    }
    (rows, None)
}

/// `fusion_library_manager`: open Library Manager and read one of its three tabs.
pub fn library_manager(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let (ok, doc, ws) = library_in_front(pass);
    if !ok { return json!({"success": false, "errorCode": "not_a_library", "error": format!("A library must be in front; the front document is {} ({}).", fusion_lbr::py_repr_str(&doc), ws), "_hint": "fusion_open_lbr {lbrPath} first."}); }
    let Some(win) = fusion_main_window(ctx) else { return json!({"success": false, "errorCode": "no_fusion_window", "error": "Could not locate Fusion's main window."}) };
    let mut steps = vec![ribbon_click(ctx, &win, "manage_tab", "open the MANAGE tab")];
    sleep_s(1.2);
    steps.push(ribbon_click(ctx, &win, "open_icon", "open Library Manager (the OPEN icon's default action)"));
    sleep_s(9.0);
    let Some(dlg) = wait_for_dialog(ctx, "Library Manager", 30.0) else { return json!({"success": false, "errorCode": "library_manager_missing", "steps": steps, "alerts": alerts(ctx, 120.0)}) };
    let tab = args["tab"].as_str().unwrap_or("private").to_lowercase();
    let label = match tab.as_str() { "hub" => "Hub Libraries", "public" => "Public Libraries", _ => "Private Libraries" };
    let click = ab_call(ctx, "desktop_ui_click", json!({"hwnd": dlg["hwnd"], "name": label, "reason": format!("switch Library Manager to {label}")}), 40);
    steps.push(json!({"step": "tab", "label": label, "ok": click["success"].as_bool() == Some(true)}));
    sleep_s(12.0);
    let tree = ab_call(ctx, "desktop_ui_tree", json!({"hwnd": dlg["hwnd"], "maxDepth": 10, "maxNodes": 600}), 120);
    let mut rows: Vec<String> = Vec::new();
    fn walk(n: &Value, d: u8, rows: &mut Vec<String>) {
        if d > 10 { return; }
        if let Some(o) = n.as_object() {
            let nm = o.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            if nm.len() > 1 && nm.len() < 80 { rows.push(nm); }
            for c in o.get("children").and_then(|c| c.as_array()).cloned().unwrap_or_default() { walk(&c, d + 1, rows); }
        }
    }
    walk(&tree["root"], 0, &mut rows);
    const CHROME: &[&str] = &["Library Manager", "System", "Close", "Electronics Desktop Widget", "Hub Libraries", "Public Libraries", "Private Libraries", "Select an item to see details here"];
    let entries: Vec<&String> = rows.iter().filter(|r| !CHROME.contains(&r.as_str())).collect();
    json!({"success": true, "tab": label, "hwnd": dlg["hwnd"], "steps": steps, "rows": rows.iter().take(200).collect::<Vec<_>>(), "libraries": entries.iter().take(200).collect::<Vec<_>>(), "tableReadable": !entries.is_empty(), "nodeCount": tree["nodeCount"],
        "screenshotVerb": format!("desktop_screenshot_window {{hwnd: {}}}", dlg["hwnd"]),
        "_hint": "Hub Libraries = libraries in the Fusion hub (what our bind chain produces, and what Autodesk recommends for sharing). Private Libraries = library.io managed libraries in folders THIS account belongs to. Public Libraries = library.io's public catalogue. NOTE: the library TABLE is CEF-drawn and invisible to UIA, so libraries[] is normally EMPTY and that is not a failure - screenshot the hwnd to read the rows. Close it with fusion_close_window when done."})
}

/// `fusion_managed_library_create`: publish the library in front as a library.io managed library.
pub fn create(ctx: &Ctx, _args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let (ok, doc, ws) = library_in_front(pass);
    if !ok { return not_a_library(&doc, &ws); }
    let hub = hub_check(pass, &doc, true);
    if hub["inHub"].as_bool() != Some(true) {
        return json!({"success": false, "errorCode": "not_in_hub", "openDocuments": hub["open"], "error": format!("Library {} is not saved in a Fusion Hub, so Fusion cannot make it a managed library.", fusion_lbr::py_repr_str(hub["name"].as_str().unwrap_or(&doc))), "remedyVerb": "fusion_save_to_cloud",
            "_hint": "fusion_save_to_cloud {name, projectName, folderPath} on this library first; a real library save returns a fileId ending in .flbr (a .f3d means the WRONG document was in front)."});
    }
    let Some(win) = fusion_main_window(ctx) else { return json!({"success": false, "errorCode": "no_fusion_window", "error": "Could not locate Fusion's main window to drive the ribbon."}) };
    let mut steps = Vec::new();
    if let Some(e) = libio_menu(ctx, &win, "push", "Push changes to library.io", &mut steps) { return e; }
    let Some(dlg) = wait_for_top_window(ctx, "Create Managed Library", 25.0) else {
        return json!({"success": false, "errorCode": "create_dialog_missing", "steps": steps, "alerts": alerts(ctx, 120.0), "error": "The LIBRARY.IO menu opened but Create Managed Library never appeared.", "windowRect": win["rect"],
            "_hint": "Push offers Create only for a library that is NOT yet linked. If this library is already linked, fusion_managed_library_push pushes a new version instead - check fusion_alerts."});
    };
    let ok = ab_call(ctx, "desktop_ui_click", json!({"hwnd": dlg["hwnd"], "name": "Create", "reason": "create the managed library on library.io"}), 30);
    steps.push(json!({"step": "create", "ok": ok["success"].as_bool() == Some(true)}));
    if let Some(conf) = wait_for_dialog(ctx, "Create Managed Library", 40.0) {
        let _ = ab_call(ctx, "desktop_ui_click", json!({"hwnd": conf["hwnd"], "name": "OK", "reason": "acknowledge the managed-library confirmation"}), 30);
        steps.push(json!({"step": "ok"}));
    }
    let al = alerts(ctx, 120.0);
    json!({"success": al.is_empty(), "library": hub["name"].as_str().unwrap_or(&doc), "steps": steps, "alerts": al,
        "_hint": if al.is_empty() { "Managed library created; it lives under the account's managed FOLDER on library.io (not public - a public search will not find it). Share it by managing that folder on library.io." } else { "Fusion raised an alert during the push - read alerts[] and treat this as failed." }})
}

/// `fusion_managed_library_link`: link to an EXISTING managed library; without `name`, list the
/// picker and cancel.
pub fn link(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let (doc, _ws, win) = match precheck(ctx, pass, false) { Ok(c) => c, Err(e) => return e };
    let mut steps = Vec::new();
    let mark = alert_watermark(ctx);
    if let Some(e) = libio_menu(ctx, &win, "link", "Link to a managed library on library.io", &mut steps) { return e; }
    let Some(dlg) = wait_for_top_window(ctx, "Select Managed Library", 25.0) else {
        return json!({"success": false, "errorCode": "picker_missing", "steps": steps, "alerts": alerts(ctx, 120.0), "error": "The LIBRARY.IO menu opened but Select Managed Library never appeared.",
            "_hint": "Fusion greys out Link on a library that is already linked - call fusion_managed_library_unlink first, or check alerts[]."});
    };
    let hwnd = dlg["hwnd"].clone();
    let (rows, read_err) = picker_rows(ctx, &hwnd);
    let want = args["name"].as_str().unwrap_or("").trim().to_string();
    let close = |why: &str| { let _ = ab_call(ctx, "desktop_ui_click", json!({"hwnd": hwnd, "name": "Cancel", "reason": why}), 30); };
    if want.is_empty() {
        close("close the Select Managed Library picker after listing it read-only");
        return json!({"success": true, "listedOnly": true, "steps": steps, "managedLibraries": rows, "count": rows.len(), "readError": read_err,
            "_hint": "Pass name:<one of these> to link. This is what THIS Autodesk account can reach: library.io's public managed folders plus any managed folder this account belongs to. A managed library owned by another account does NOT appear until its managed folder is shared with this one. Qt virtualizes the list, so only realized rows are returned - scroll or filter in the dialog if the one you want is missing."});
    }
    let hit = rows.iter().find(|r| r["name"].as_str().unwrap_or("").to_lowercase() == want.to_lowercase()).cloned();
    let at = hit.as_ref().map(|h| h["_at"].clone()).unwrap_or(Value::Null);
    let (Some(hit), Some(ax), Some(ay)) = (hit, at[0].as_f64(), at[1].as_f64()) else {
        close("close the picker; the requested managed library was not listed");
        return json!({"success": false, "errorCode": "not_listed", "steps": steps, "managedLibraries": rows, "count": rows.len(), "error": format!("{} is not in this account's managed-library list.", fusion_lbr::py_repr_str(&want)),
            "_hint": "Names are exact. If you expect a library shared from another Autodesk account, it only appears once its MANAGED FOLDER is shared with this account on library.io - being in the same Fusion hub is not enough."});
    };
    let _ = ab_call(ctx, "desktop_click", json!({"x": ax as i64, "y": ay as i64, "cursor": false, "reason": format!("select {want} in the Select Managed Library picker")}), 25);
    sleep_s(1.0);
    let ok = ab_call(ctx, "desktop_ui_click", json!({"hwnd": hwnd, "name": "OK", "reason": "confirm the managed-library link"}), 30);
    steps.push(json!({"step": "confirm", "row": hit["name"], "clicked": find_key(&ok, "success", 0).and_then(|v| v.as_bool()).unwrap_or(false)}));
    sleep_s(2.0);
    let gone = wait_for_top_window_gone(ctx, "Select Managed Library", 15.0);
    let prompt = handle_update_prompt(ctx, args["onUpdatePrompt"].as_str().unwrap_or("cancel"), 12.0);
    if let Some(p) = &prompt { steps.push(p.clone()); }
    let al = alerts_since(ctx, &mark);
    let choice = prompt.as_ref().and_then(|p| p["choice"].as_str()).map(|c| fusion_lbr::py_repr_str(c)).unwrap_or("'not shown'".into());
    json!({"success": gone, "linkedTo": hit["name"], "library": doc, "managedFolder": hit["managedFolder"], "version": hit["version"], "steps": steps, "alerts": al, "dialogClosed": gone, "updatePrompt": prompt,
        "_hint": format!("Linked. Fusion offered to force-update this library from the managed one and that prompt was answered {choice} - pass onUpdatePrompt:'merge' to take the managed library's parts, or 'replace' to overwrite yours entirely.")})
}

/// `fusion_managed_library_pull`: the newest version FROM library.io into this linked library.
pub fn pull(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let (doc, _ws, win) = match precheck(ctx, pass, false) { Ok(c) => c, Err(e) => return e };
    let mut steps = Vec::new();
    let mark = alert_watermark(ctx);
    if let Some(e) = libio_menu(ctx, &win, "pull", "Pull changes from the managed library on library.io", &mut steps) { return e; }
    let prompt = handle_update_prompt(ctx, args["onUpdatePrompt"].as_str().unwrap_or("merge"), 12.0);
    if let Some(p) = &prompt { steps.push(p.clone()); }
    let up_to_date = if prompt.is_none() { let u = dismiss_ok_dialog(ctx, "Up to Date", 10.0); if let Some(x) = &u { steps.push(x.clone()); } u } else { None };
    let al = alerts_since(ctx, &mark);
    let answered = prompt.is_some() || up_to_date.is_some();
    let hint = if answered && al.is_empty() { if up_to_date.is_some() { "Already current - Fusion answered with Up to Date.".to_string() } else { format!("Pulled; the force-update prompt was answered {}.", prompt.as_ref().and_then(|p| p["choice"].as_str()).map(|c| fusion_lbr::py_repr_str(c)).unwrap_or("None".into())) } }
        else if !al.is_empty() { "Fusion raised an alert - read alerts[].".to_string() } else { "Neither the update prompt nor Up to Date appeared, so NOTHING was pulled. Fusion greys Pull out on a library that was never linked - link it first.".to_string() };
    json!({"success": answered && al.is_empty(), "library": doc, "steps": steps, "alerts": al, "upToDate": up_to_date.is_some(), "updatePrompt": prompt, "_hint": hint})
}

/// `fusion_managed_library_push`: a NEW VERSION of an already-linked managed library.
pub fn push(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let (doc, _ws, win) = match precheck(ctx, pass, true) { Ok(c) => c, Err(e) => return e };
    let mut steps = Vec::new();
    let mark = alert_watermark(ctx);
    if let Some(e) = libio_menu(ctx, &win, "push", "Push changes to library.io", &mut steps) { return e; }
    if wait_for_top_window(ctx, "Create Managed Library", 6.0).is_some() {
        return json!({"success": false, "errorCode": "not_linked_yet", "steps": steps, "dialog": "Create Managed Library",
            "_hint": "This library is not linked yet - Fusion offered Create Managed Library. Call fusion_managed_library_create instead (it completes that dialog), or fusion_managed_library_link to attach it to an existing one. The dialog is still open; answer or close it before the next verb."});
    }
    let confirm = confirm_create_version(ctx, args["onConfirm"].as_str().unwrap_or("create"), 20.0);
    if let Some(c) = &confirm { steps.push(c.clone()); }
    let al = alerts_since(ctx, &mark);
    let Some(confirm) = confirm else {
        return json!({"success": false, "errorCode": "confirm_missing", "steps": steps, "alerts": al, "library": doc, "error": "Push did not raise the Create New Version confirmation.",
            "_hint": "Nothing was pushed. Check that this library is linked to a managed library (fusion_managed_library_link) and read alerts[]."});
    };
    let ok = confirm["dialogClosed"].as_bool() == Some(true) && al.is_empty();
    json!({"success": ok, "library": doc, "steps": steps, "alerts": al, "confirmed": confirm,
        "_hint": if ok { "A new version was pushed to the managed library. Library Manager still shows the IMPORTED version number until you pull, so the row gains an update-available badge rather than incrementing." } else { "The confirmation did not close or Fusion alerted - treat this as NOT pushed and read alerts[]." }})
}

/// `fusion_managed_library_unlink`: unlink this library from its managed library.
pub fn unlink(ctx: &Ctx, _args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let (doc, _ws, win) = match precheck(ctx, pass, false) { Ok(c) => c, Err(e) => return e };
    let mut steps = Vec::new();
    let mark = alert_watermark(ctx);
    if let Some(e) = libio_menu(ctx, &win, "unlink", "Unlink from the managed library on library.io", &mut steps) { return e; }
    let al = alerts_since(ctx, &mark);
    json!({"success": al.is_empty(), "library": doc, "steps": steps, "alerts": al,
        "_hint": if al.is_empty() { "Unlink requested. Fusion greys this item out on a library that was never linked, so a silent no-op there is expected." } else { "Fusion raised an alert - read alerts[] and treat this as failed." }})
}

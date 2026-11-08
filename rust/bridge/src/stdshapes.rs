//! Standard verbs answered in the standard's own shape from what the bridge already reads: list_library
//! (a .lbr offline), board_summary (read_board), read_schematic (schematic_data), export_pos (the board's
//! parts and pads) and window_info (Fusion's windows and dialogs).

use serde_json::{json, Value};

use crate::addin::Ctx;

fn ms(t0: std::time::Instant) -> u64 { t0.elapsed().as_millis() as u64 }

/// `fusion_list_library`: the names in a .lbr. An EAGLE library holds both kinds, so `kind` picks:
/// sch (the components, default) or pcb (the footprints).
pub fn list_library(args: &Value) -> Value {
    let t0 = std::time::Instant::now();
    let Some(path) = args["libraryPath"].as_str().or(args["filePath"].as_str()).map(|p| p.replace('\\', "/")) else {
        return json!({"success": false, "errorCode": "bad_args", "error": "libraryPath is required: the .lbr on the Fusion machine (Fusion has no bundled default library)"});
    };
    let kind = match args["kind"].as_str().unwrap_or("sch") { "pcb" | "footprint" | "footprints" | "package" => "pcb", _ => "sch" };
    let limit = args["limit"].as_u64().unwrap_or(500) as usize;
    let text = match std::fs::read_to_string(&path) { Ok(t) => t, Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": format!("cannot read {path}: {e}")}) };
    let root = match fusion_lbr::Element::parse(&text) { Ok(r) => r, Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": e.to_string()}) };
    let (sec, tag) = if kind == "pcb" { ("packages", "package") } else { ("devicesets", "deviceset") };
    let mut names = Vec::new();
    fn walk<'a>(e: &'a fusion_lbr::Element, sec: &str, tag: &str, out: &mut Vec<String>) {
        if e.tag == sec { for c in e.children.iter().filter(|c| c.tag == tag) { if let Some(n) = c.attr("name") { out.push(n.to_string()); } } return; }
        for c in &e.children { walk(c, sec, tag, out); }
    }
    walk(&root, sec, tag, &mut names);
    let count = names.len();
    names.truncate(limit);
    json!({"success": true, "mutated": false, "data": {"kind": kind, "count": count, "names": names}, "kind": kind, "path": path, "roundtripMs": ms(t0),
           "_hint": if kind == "sch" { "Components (EAGLE devicesets). kind:'pcb' lists the footprints of the same library." } else { "Footprints (EAGLE packages). kind:'sch' lists the components." }})
}

/// `fusion_board_summary`: the standard's one-screen summary, shaped from one read_board round trip.
pub fn board_summary(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let t0 = std::time::Instant::now();
    // a read names no document: the board in front is the one asked about
    let mut read_args = args.clone();
    if read_args["expectDocument"].is_null() && read_args.is_object() {
        if let Some(n) = pass("get_app_state", json!({}))["data"]["activeDocument"].as_str() { read_args["expectDocument"] = json!(n); }
    }
    let rb = crate::routing::call("read_board", &read_args, pass);
    if rb["success"].as_bool() != Some(true) { return rb; }
    if args["raw"].as_bool() == Some(true) { return rb; }
    let d = &rb["data"];
    let comps = d["components"]["components"].as_array().cloned().unwrap_or_default();
    let limit = args["limit"].as_u64().unwrap_or(2000) as usize;
    let side = |s: &str| comps.iter().filter(|c| c["side"] == s).count();
    let in_use = d["occupancy"]["copperLayersInUse"].as_array().cloned().unwrap_or_default();
    let rules = d["rules"]["rules"].as_array().cloned().unwrap_or_default();
    let enabled = rules.iter().filter(|r| r["enabled"].as_bool() != Some(false) && r["attrs"]["enabled"].as_str() != Some("no")).count();
    let summary = json!({
        "board": {"name": rb["documentName"], "boardId": rb["boardId"], "outline": d["outline"]["outline"], "outlineClosed": d["outline"]["outlineClosed"], "revision": rb["revision"]},
        "components": comps.iter().take(limit).map(|c| json!({"designator": c["ref"], "footprint": c["pattern"], "comment": c["value"], "x": c["x"], "y": c["y"], "rotation": c["rotation"], "side": c["side"], "locked": c["locked"]})).collect::<Vec<_>>(),
        "componentsTop": side("top"), "componentsBottom": side("bottom"),
        "nets": d["nets"]["nets"].as_array().map(|a| a.len()).unwrap_or(0),
        "copperLayersInUse": in_use.len(),
        "copperLayersInUseNames": in_use.iter().filter_map(|l| l["name"].as_str()).collect::<Vec<_>>(),
        "layersWithPrimitives": d["occupancy"]["layersWithPrimitives"],
        "copperLayersAddressable": d["layers"]["copperLayers"],
        "rules": rules.len(), "rulesEnabled": enabled,
    });
    json!({"success": true, "mutated": false, "summary": summary, "documentName": rb["documentName"], "revision": rb["revision"], "roundtripMs": ms(t0)})
}

/// `fusion_read_schematic`: the standard's components of the schematic, from schematic_data.
pub fn read_schematic(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let t0 = std::time::Instant::now();
    let mut a = args.clone();
    if args["params"].as_bool() != Some(false) { a["params"] = json!(true); }
    let sd = crate::sch::call("schematic_data", &a, pass);
    if sd["success"].as_bool() == Some(false) { return sd; }
    let comps: Vec<Value> = sd["components"].as_array().cloned().unwrap_or_default().into_iter().map(|c| {
        let mut row = json!({"designator": c["designator"], "libReference": c["libReference"], "comment": c["comment"], "device": c["device"], "library": c["library"], "sheet": c["sheet"], "at": c["at"]});
        if c["params"].is_object() { row["parameters"] = c["params"].clone(); }
        row
    }).collect();
    let des: Vec<Value> = comps.iter().map(|c| c["designator"].clone()).collect();
    json!({"success": true, "mutated": false, "components": comps, "count": des.len(), "designators": des, "document": sd["documentName"], "truncated": sd["truncated"], "roundtripMs": ms(t0)})
}

/// One placed part as the pick-and-place file writes it.
struct Placed { refdes: String, value: String, footprint: String, x: f64, y: f64, rot: f64, top: bool, smd: bool }

/// `fusion_export_pos`: the pick-and-place file from the board's own parts and pads: csv or ascii, mm
/// or in, front/back/both, smdOnly, DNP parts left out (excludeDnp, default true), and fab:'jlcpcb'
/// for JLCPCB's CPL columns too. Fusion writes no Gerber position file, so that format is refused.
pub fn export_pos(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let format = args["format"].as_str().unwrap_or("csv").to_lowercase();
    if format == "gerber" { return json!({"success": false, "errorCode": "not_supported", "error": "Fusion writes no Gerber position (X3) file: format csv or ascii"}); }
    if format != "csv" && format != "ascii" { return json!({"success": false, "errorCode": "bad_args", "error": format!("format is csv, ascii or gerber, not {format}")}); }
    let inch = args["units"].as_str() == Some("in");
    let side = args["side"].as_str().unwrap_or("both");
    let rs = crate::routing::call("routing_state", args, pass);
    if rs["success"].as_bool() != Some(true) { return rs; }
    // EAGLE's populate="no" (the current variant's not-fitted parts) from the board source
    let src = crate::routing::call("board_source", args, pass);
    // and the parts marked on the board: edit_parts' DNP and EXCLUDE_FROM_POS attributes
    let dnp: Vec<String> = src["source"].as_str().or(src["data"]["source"].as_str()).map(|t| {
        let mut v: Vec<String> = regex::Regex::new(r#"<element name="([^"]+)"[^>]*populate="no""#).unwrap().captures_iter(t).map(|c| c[1].to_string()).collect();
        let block = regex::Regex::new(r#"(?s)<element name="([^"]+)"[^>]*[^/]>(.*?)</element>"#).unwrap();
        let flag = regex::Regex::new(r#"<attribute name="(?:DNP|EXCLUDE_FROM_POS)" value="(?i:yes|true|1)""#).unwrap();
        v.extend(block.captures_iter(t).filter(|c| flag.is_match(&c[2])).map(|c| c[1].to_string()));
        v
    }).unwrap_or_default();
    let pads = rs["pads"].as_array().cloned().unwrap_or_default();
    let mut parts: Vec<Placed> = Vec::new();
    for c in rs["components"].as_array().into_iter().flatten() {
        let r = c["ref"].as_str().unwrap_or("").to_string();
        let mine: Vec<&Value> = pads.iter().filter(|p| p["owner"].as_str() == Some(r.as_str()) || p["ref"].as_str() == Some(r.as_str())).collect();
        if mine.is_empty() { continue; } // a part with no pads (a frame, a logo) is not placed by a machine
        // SMD when any pad is a surface pad: a power part's footprint may carry drilled thermal vias too
        let smd = mine.iter().any(|p| p["drill"].as_f64().unwrap_or(0.0) == 0.0);
        parts.push(Placed { refdes: r, value: c["value"].as_str().unwrap_or("").into(), footprint: c["pattern"].as_str().unwrap_or("").into(),
            x: c["x"].as_f64().unwrap_or(0.0), y: c["y"].as_f64().unwrap_or(0.0), rot: c["rotation"].as_f64().unwrap_or(0.0), top: c["side"] != "bottom", smd });
    }
    let exclude_dnp = args["excludeDnp"].as_bool() != Some(false);
    let smd_only = args["smdOnly"].as_bool() == Some(true);
    parts.retain(|p| !(exclude_dnp && dnp.contains(&p.refdes)) && !(smd_only && !p.smd));
    parts.sort_by(|a, b| a.refdes.cmp(&b.refdes));
    let dir = args["outputDir"].as_str().map(|s| s.replace('\\', "/")).unwrap_or_else(|| format!("{}/adom-pos", std::env::temp_dir().to_string_lossy().replace('\\', "/")));
    if let Err(e) = std::fs::create_dir_all(&dir) { return json!({"success": false, "errorCode": "write_failed", "error": e.to_string()}); }
    let stem = rs["documentName"].as_str().unwrap_or("board").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect::<String>();
    let k = if inch { 1.0 / 25.4 } else { 1.0 };
    let unit = if inch { "in" } else { "mm" };
    let mut files = Vec::new();
    let mut counts = json!({});
    let sides: Vec<(&str, bool)> = match side { "front" | "top" => vec![("front", true)], "back" | "bottom" => vec![("back", false)], _ => vec![("front", true), ("back", false)] };
    for (name, top) in &sides {
        let rows: Vec<&Placed> = parts.iter().filter(|p| p.top == *top).collect();
        counts[*name] = json!(rows.len());
        let path = format!("{dir}/{stem}-pos-{name}.{}", if format == "csv" { "csv" } else { "txt" });
        let mut s = String::new();
        if format == "csv" {
            s.push_str(&format!("Ref,Val,Package,PosX({unit}),PosY({unit}),Rot,Side\n"));
            for p in &rows { s.push_str(&format!("\"{}\",\"{}\",\"{}\",{:.4},{:.4},{:.2},{}\n", p.refdes, p.value, p.footprint, p.x * k, p.y * k, p.rot, if p.top { "top" } else { "bottom" })); }
        } else {
            s.push_str(&format!("### Footprint positions, {unit}, {name}\n# Ref     Val              Package                PosX       PosY       Rot  Side\n"));
            for p in &rows { s.push_str(&format!("{:<9} {:<16} {:<22} {:>10.4} {:>10.4} {:>6.2}  {}\n", p.refdes, p.value, p.footprint, p.x * k, p.y * k, p.rot, if p.top { "top" } else { "bottom" })); }
            s.push_str("## End\n");
        }
        if let Err(e) = std::fs::write(&path, s) { return json!({"success": false, "errorCode": "write_failed", "error": e.to_string()}); }
        files.push(path);
    }
    if args["fab"].as_str().map(|f| f.eq_ignore_ascii_case("jlcpcb")).unwrap_or(false) {
        let path = format!("{dir}/{stem}-cpl-jlcpcb.csv");
        let mut s = String::from("Designator,Mid X,Mid Y,Layer,Rotation\n");
        for p in &parts { s.push_str(&format!("{},{:.4}mm,{:.4}mm,{},{:.0}\n", p.refdes, p.x, p.y, if p.top { "Top" } else { "Bottom" }, p.rot)); }
        if let Err(e) = std::fs::write(&path, s) { return json!({"success": false, "errorCode": "write_failed", "error": e.to_string()}); }
        files.push(path);
    }
    json!({"success": true, "mutated": false, "files": files, "parts": counts, "units": unit, "format": format, "dnpLeftOut": if exclude_dnp { json!(dnp) } else { json!([]) },
           "_hint": "Positions are each part's origin in the board frame, the way Fusion places it; JLCPCB checks rotations against its own library on upload."})
}

/// `fusion_window_info`: Fusion's top-level windows and its open dialogs, without screenshots.
pub fn window_info(ctx: &Ctx) -> Value {
    let Some(ab) = fusion_core::ab::Loopback::from_env() else { return json!({"success": false, "errorCode": "ab_unreachable", "error": "Adom Bridge did not answer on this machine"}) };
    let facts = fusion_core::windows_facts::facts_from_ab(&ab, &ctx.caller);
    let windows: Vec<Value> = facts.family.iter().map(|w| json!({"hwnd": w["hwnd"], "title": w["title"], "kind": w["kind"], "className": w["className"], "pid": w["pid"], "rect": w["rect"]})).collect();
    let main = facts.main_hwnd;
    let dialogs: Vec<Value> = crate::lifecycle::dialog_inventory(ctx).into_iter().map(|d| json!({"hwnd": d["hwnd"], "title": d["title"], "body": d["text"], "buttons": d["buttons"],
        "category": d["category"], "safeAction": d["safeButton"].as_str().or(d["resolution"].as_str()), "ownedByMainInstance": true})).collect();
    let modal = !dialogs.is_empty();
    json!({"success": true, "mutated": false, "windows": windows, "dialogs": dialogs, "hasModalDialogs": modal, "mainHwnd": main, "mainTitle": facts.main_title})
}

/// EAGLE layer numbers for the standard's render_board_layers presets the add-in's export does not name.
fn preset_layers(preset: &str) -> Option<Vec<i64>> {
    Some(match preset {
        "drill" => vec![20, 44, 45],
        "assembly_top" => vec![20, 21, 25, 27, 51],
        "assembly_bottom" => vec![20, 22, 26, 28, 52],
        "fabrication" => vec![20, 44, 45, 46, 47, 51, 52],
        _ => return None,
    })
}

/// A layer as the standard names it (copper F.Cu, In1.Cu .. In14.Cu, B.Cu) or Fusion's own number.
fn layer_number(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() { return Some(n); }
    let s = v.as_str()?.trim();
    if let Ok(n) = s.parse::<i64>() { return Some(n); }
    match s { "F.Cu" => Some(1), "B.Cu" => Some(16), _ => s.strip_prefix("In").and_then(|r| r.strip_suffix(".Cu")).and_then(|k| k.parse::<i64>().ok()).filter(|k| (1..=14).contains(k)).map(|k| k + 1) }
}

fn png_size(path: &str) -> Option<(u32, u32)> {
    let b = std::fs::read(path).ok()?;
    if b.len() < 24 || &b[1..4] != b"PNG" { return None; }
    Some((u32::from_be_bytes([b[16], b[17], b[18], b[19]]), u32::from_be_bytes([b[20], b[21], b[22], b[23]])))
}

/// `fusion_render_board_layers`: the standard's render_board_layers over the add-in's EXPORT IMAGE:
/// the standard's presets and layer names, png only (Fusion's EXPORT IMAGE writes no svg).
pub fn render_board_layers(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut a = args.clone();
    if args["listPresets"].as_bool() == Some(true) {
        let mut r = pass("export_board_image", json!({"listPresets": true}));
        for p in ["drill", "assembly_top", "assembly_bottom", "fabrication", "outline"] { r["data"]["presets"][p] = json!(format!("layers {:?}", preset_layers(p).unwrap_or(vec![20]))); }
        return r;
    }
    let format = args["format"].as_str().unwrap_or("png").to_lowercase();
    if format != "png" { return json!({"success": false, "errorCode": "not_supported", "error": format!("Fusion's EXPORT IMAGE writes no {format}: format png")}); }
    let preset = args["preset"].as_str().unwrap_or("all").to_string();
    if let Some(ls) = args["layers"].as_array() {
        let nums: Vec<i64> = ls.iter().filter_map(layer_number).collect();
        if nums.len() != ls.len() { return json!({"success": false, "errorCode": "bad_args", "error": "layers are copper names (F.Cu, In1.Cu .. In14.Cu, B.Cu) or Fusion layer numbers"}); }
        a["layers"] = json!(nums);
    } else if preset == "outline" { a["preset"] = json!("board_outline"); }
    else if let Some(l) = preset_layers(&preset) { a["layers"] = json!(l); }
    if a["outputPath"].is_null() {
        a["outputPath"] = json!(format!("{}/adom-board-layers/{}.png", std::env::temp_dir().to_string_lossy().replace('\\', "/"), preset));
    }
    let _ = pass("show_2d_board", json!({"expectDocument": args["expectDocument"]}));
    let mut r = pass("export_board_image", a.clone());
    if r["success"].as_bool() == Some(false) { return r; }
    let path = a["outputPath"].as_str().unwrap_or("").to_string();
    let (w, h) = png_size(&path).map(|(w, h)| (json!(w), json!(h))).unwrap_or((Value::Null, Value::Null));
    r["path"] = json!(path); r["format"] = json!("png"); r["widthPx"] = w; r["heightPx"] = h;
    r["preset"] = json!(if args["layers"].is_array() { "custom".to_string() } else { preset });
    r
}

/// `fusion_bom`: the standard's bom through the shared engine (adom-eda-checks bom): the schematic's
/// parts with their attributes (sheetParts), the live board, and the variant (`variant` names one;
/// default the current). The engine groups, checks and writes the order files as text; this writes
/// them to outputDir (bom.csv, bom-jlcpcb.csv, cpl-jlcpcb.csv, bom-mouser.csv, bom-digikey.csv).
pub fn bom(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let inputs = crate::routing::call("check_inputs", args, pass);
    if inputs["success"].as_bool() != Some(true) { return inputs; }
    let sd = crate::sch::call("schematic_data", &json!({"expectDocument": args["expectDocument"], "params": true, "limit": 5000}), pass);
    if sd["success"].as_bool() == Some(false) { return sd; }
    let vr = crate::sch::call("variants", &json!({"expectDocument": args["expectDocument"]}), pass);
    let variants = vr["variants"].as_array().cloned().unwrap_or_default();
    let chosen = match args["variant"].as_str() {
        Some(n) => match variants.iter().find(|v| v["name"].as_str() == Some(n)) { Some(v) => Some(v.clone()), None => return json!({"success": false, "errorCode": "unknown_variant", "error": format!("No variant {n}"), "variants": variants.iter().map(|v| v["name"].clone()).collect::<Vec<_>>()}) },
        None => variants.iter().find(|v| v["current"] == true).cloned(),
    };
    let mut eargs = json!({"boards": args["boards"].as_u64().unwrap_or(1)});
    if let Some(v) = &chosen {
        eargs["variant"] = json!({"name": v["name"], "notFitted": v["notFitted"],
            "alternates": v["alternates"].as_array().cloned().unwrap_or_default().iter().map(|a| json!({"designator": a["designator"], "part": a["value"]})).collect::<Vec<_>>()});
    }
    // edit_parts' DNP and EXCLUDE_FROM_BOM attributes count as not fitted, with or without a variant. They live
    // on the board's parts, so board_bom says which (excluded)
    let bb = crate::routing::call("board_bom", &json!({"expectDocument": args["expectDocument"]}), pass);
    let marked = bb["excluded"].as_array().cloned().unwrap_or_default();
    if !marked.is_empty() {
        if eargs["variant"].is_null() { eargs["variant"] = json!({"name": "", "notFitted": [], "alternates": []}); }
        let mut nf = eargs["variant"]["notFitted"].as_array().cloned().unwrap_or_default();
        nf.extend(marked);
        eargs["variant"]["notFitted"] = json!(nf);
    }
    let bundle = json!({"sheetParts": crate::checks::sheet_parts(&sd), "board": inputs["board"], "args": eargs});
    let mut r = adom_eda_checks::run("bom", &bundle);
    if r["success"].as_bool() != Some(true) { return r; }
    if args["write"].as_bool() != Some(false) {
        let stem = sd["documentName"].as_str().or(inputs["documentName"].as_str()).unwrap_or("board").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect::<String>();
        let dir = args["outputDir"].as_str().map(|s| s.replace('\\', "/")).unwrap_or_else(|| format!("{}/adom-bom/{stem}", std::env::temp_dir().to_string_lossy().replace('\\', "/")));
        if let Err(e) = std::fs::create_dir_all(&dir) { return json!({"success": false, "errorCode": "write_failed", "error": e.to_string()}); }
        let mut paths = Vec::new();
        for f in r["files"].as_array().cloned().unwrap_or_default() {
            let p = format!("{dir}/{}", f["name"].as_str().unwrap_or("bom.csv"));
            if let Err(e) = std::fs::write(&p, f["text"].as_str().unwrap_or("")) { return json!({"success": false, "errorCode": "write_failed", "error": e.to_string()}); }
            paths.push(json!(p));
        }
        let _ = r["files"].take();
        r["files"] = json!(paths);
        r["outputDir"] = json!(dir);
    }
    r["variants"] = json!(variants.iter().map(|v| v["name"].clone()).collect::<Vec<_>>());
    r["mutated"] = json!(false);
    r["shell"] = json!("rust");
    r
}

/// `fusion_export_manufacturing`: the manufacturing set in one call, in one folder: silkscreen prep
/// first (silkPrep, default true: designators off the pads), the Gerbers and NC drill from the
/// bundled JLCPCB CAM job (zipped), pick and place (CSV, mm), the BOM and its order files, and with
/// `formats` ODB++ and IPC-2581 beside them. Every step that fails is named in `issues`; nothing
/// opens a dialog.
pub fn export_manufacturing(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let t0 = std::time::Instant::now();
    let doc = args["expectDocument"].clone();
    let base = json!({"expectDocument": doc});
    let stem = doc.as_str().unwrap_or("board").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect::<String>();
    let dir = args["outputDir"].as_str().map(|s| s.replace('\\', "/")).unwrap_or_else(|| format!("{}/adom-manufacturing/{stem}", std::env::temp_dir().to_string_lossy().replace('\\', "/")));
    if args["clean"].as_bool() != Some(false) { let _ = std::fs::remove_dir_all(&dir); }
    if let Err(e) = std::fs::create_dir_all(&dir) { return json!({"success": false, "errorCode": "write_failed", "error": e.to_string()}); }
    let mut issues: Vec<Value> = Vec::new();
    let mut steps = serde_json::Map::new();
    if args["silkPrep"].as_bool() != Some(false) {
        let sp = crate::checks::silkscreen_prep(&base, pass);
        if sp["success"].as_bool() == Some(false) { issues.push(json!({"from": "silkscreen_prep", "what": sp["error"]})); }
        steps.insert("silkPrep".into(), json!({"moved": sp["moved"], "before": sp["before"], "after": sp["after"], "stillOnPads": sp["stillOnPads"]}));
    }
    let g = pass("export_gerbers", json!({"outputDir": format!("{dir}/gerber")}));
    let gd = if g["data"].is_object() { g["data"].clone() } else { g.clone() };
    if gd["zipPath"].is_null() { issues.push(json!({"from": "export_gerbers", "what": gd["error"].as_str().or(g["error"].as_str()).unwrap_or("the CAM job wrote no zip")})); }
    steps.insert("gerbers".into(), json!({"zip": gd["zipPath"], "files": gd["fileCount"]}));
    let pos = export_pos(&json!({"expectDocument": doc, "outputDir": format!("{dir}/assembly"), "fab": "jlcpcb"}), pass);
    if pos["success"].as_bool() != Some(true) { issues.push(json!({"from": "export_pos", "what": pos["error"]})); }
    let bm = bom(&json!({"expectDocument": doc, "outputDir": format!("{dir}/bom")}), pass);
    if bm["success"].as_bool() != Some(true) { issues.push(json!({"from": "bom", "what": bm["error"]})); }
    steps.insert("bom".into(), json!({"lines": bm["lineCount"], "fittedParts": bm["fittedParts"]}));
    for f in args["formats"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
        let r = match f {
            "odb" => crate::routing::call("export_odb", &json!({"expectDocument": doc, "outputDir": format!("{dir}/odb"), "compression": "tgz"}), pass),
            "ipc2581" => crate::writers::call("export_ipc2581", &json!({"expectDocument": doc, "outputDir": format!("{dir}/ipc2581")}), pass),
            other => json!({"success": false, "error": format!("{other} is not a format here: odb or ipc2581")}),
        };
        if r["success"].as_bool() == Some(false) { issues.push(json!({"from": format!("export_{f}"), "what": r["error"]})); }
        steps.insert(f.into(), json!({"path": r["path"]}));
    }
    // every file written, relative to the folder
    fn walk(d: &std::path::Path, root: &std::path::Path, out: &mut Vec<String>) {
        if let Ok(rd) = std::fs::read_dir(d) { for e in rd.flatten() { let p = e.path(); if p.is_dir() { walk(&p, root, out); } else if let Ok(r) = p.strip_prefix(root) { out.push(r.to_string_lossy().replace('\\', "/")); } } }
    }
    let mut files = Vec::new();
    walk(std::path::Path::new(&dir), std::path::Path::new(&dir), &mut files);
    files.sort();
    json!({"success": issues.is_empty(), "mutated": steps.contains_key("silkPrep") && steps["silkPrep"]["moved"].as_u64().unwrap_or(0) > 0, "outputDir": dir, "files": files, "steps": steps,
           "issues": issues, "dialogs": [], "elapsedSec": (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0,
           "errorCode": if issues.is_empty() { Value::Null } else { json!("manufacturing_incomplete") },
           "_hint": "Gerbers and drill are the zip under gerber/; assembly/ has pick and place (and JLCPCB's CPL); bom/ has the order files. silkPrep:false leaves the silkscreen as drawn."})
}

/// `fusion_pcb_report`: numbers instead of pixels. For a .lbr (path), every footprint: its pads, the
/// pads' bounding box in mil, a census of its primitives, and its 3D packages; for the open board
/// (no path, or want:'board'), the outline, each part with its pads, nets and bounding box, the census
/// and the tracks.
pub fn pcb_report(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let t0 = std::time::Instant::now();
    const MIL: f64 = 0.0254;
    let mil = |v: f64| (v / MIL * 10.0).round() / 10.0;
    let path = args["path"].as_str().or(args["libraryPath"].as_str()).map(|p| p.replace('\\', "/"));
    if let Some(path) = path.filter(|p| p.to_lowercase().ends_with(".lbr") && args["want"].as_str() != Some("board")) {
        let text = match std::fs::read_to_string(&path) { Ok(t) => t, Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": format!("cannot read {path}: {e}")}) };
        let root = match fusion_lbr::Element::parse(&text) { Ok(r) => r, Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": e.to_string()}) };
        fn find<'a>(e: &'a fusion_lbr::Element, tag: &str) -> Option<&'a fusion_lbr::Element> { if e.tag == tag { return Some(e); } e.children.iter().find_map(|c| find(c, tag)) }
        let want = args["name"].as_str().map(str::to_lowercase);
        // 3D packages: <packages3d><package3d><packageinstances><packageinstance name="PKG"/>
        let mut bodies: std::collections::HashMap<String, Vec<String>> = Default::default();
        if let Some(p3) = find(&root, "packages3d") {
            for p in p3.children.iter().filter(|c| c.tag == "package3d") {
                if let Some(pi) = p.children.iter().find(|c| c.tag == "packageinstances") {
                    for i in pi.children.iter().filter_map(|c| c.attr("name")) { bodies.entry(i.to_string()).or_default().push(p.attr("name").unwrap_or("").to_string()); }
                }
            }
        }
        let num = |e: &fusion_lbr::Element, k: &str| e.attr(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        let mut comps = Vec::new();
        for pk in find(&root, "packages").map(|s| s.children.iter().filter(|c| c.tag == "package").collect::<Vec<_>>()).unwrap_or_default() {
            let name = pk.attr("name").unwrap_or("").to_string();
            if let Some(w) = &want { if !name.to_lowercase().contains(w) { continue; } }
            let mut census: std::collections::BTreeMap<String, u64> = Default::default();
            for c in &pk.children { if c.tag != "description" { *census.entry(c.tag.clone()).or_default() += 1; } }
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            let mut pads = 0;
            for c in pk.children.iter().filter(|c| c.tag == "smd" || c.tag == "pad") {
                pads += 1;
                let (x, y) = (num(c, "x"), num(c, "y"));
                let (hw, hh) = if c.tag == "smd" { (num(c, "dx") / 2.0, num(c, "dy") / 2.0) } else { let d = num(c, "diameter").max(num(c, "drill") * 1.5) / 2.0; (d, d) };
                let r = c.attr("rot").map(|r| r.trim_start_matches(|ch: char| ch.is_ascii_alphabetic()).parse::<f64>().unwrap_or(0.0)).unwrap_or(0.0).to_radians();
                let (ex, ey) = (hw * r.cos().abs() + hh * r.sin().abs(), hw * r.sin().abs() + hh * r.cos().abs());
                x0 = x0.min(x - ex); y0 = y0.min(y - ey); x1 = x1.max(x + ex); y1 = y1.max(y + ey);
            }
            let bbox = if pads > 0 { json!([mil(x0), mil(y0), mil(x1), mil(y1)]) } else { Value::Null };
            comps.push(json!({"name": name, "pads": pads, "padsBboxMil": bbox, "census": census, "bodies": bodies.get(&name).cloned().unwrap_or_default()}));
        }
        return json!({"success": true, "mutated": false, "data": {"kind": "library", "components": comps, "bodyCols": ["package3d"]}, "path": path, "roundtripMs": t0.elapsed().as_millis() as u64});
    }
    let rb = crate::routing::call("read_board", args, pass);
    if rb["success"].as_bool() != Some(true) { return rb; }
    let d = &rb["data"];
    let pads = d["copper"]["pads"].as_array().cloned().unwrap_or_default();
    let comps: Vec<Value> = d["components"]["components"].as_array().cloned().unwrap_or_default().iter().map(|c| {
        let r = c["ref"].as_str().unwrap_or("");
        let mine: Vec<&Value> = pads.iter().filter(|p| p["key"].as_str().map(|k| k.split('.').next() == Some(r)).unwrap_or(false)).collect();
        let xs: Vec<f64> = mine.iter().filter_map(|p| p["at"][0].as_f64()).collect();
        let ys: Vec<f64> = mine.iter().filter_map(|p| p["at"][1].as_f64()).collect();
        let bbox = if xs.is_empty() { Value::Null } else { json!([mil(xs.iter().cloned().fold(f64::MAX, f64::min)), mil(ys.iter().cloned().fold(f64::MAX, f64::min)), mil(xs.iter().cloned().fold(f64::MIN, f64::max)), mil(ys.iter().cloned().fold(f64::MIN, f64::max))]) };
        let mut nets: Vec<String> = mine.iter().filter_map(|p| p["net"].as_str().map(str::to_string)).filter(|n| !n.is_empty()).collect(); nets.sort(); nets.dedup();
        json!({"designator": r, "footprint": c["pattern"], "x": c["x"], "y": c["y"], "rotation": c["rotation"], "side": c["side"], "pads": mine.len(), "padsBboxMil": bbox, "nets": nets, "bodies": if c["has3d"] == true { json!(["package3d"]) } else { json!([]) }})
    }).collect();
    let census = json!({"tracks": d["copper"]["tracks"].as_array().map(|a| a.len()), "arcs": d["copper"]["arcs"].as_array().map(|a| a.len()), "vias": d["copper"]["vias"].as_array().map(|a| a.len()),
                        "pads": pads.len(), "polygons": d["copper"]["polygons"].as_array().map(|a| a.len()), "components": comps.len()});
    let tracks: Vec<Value> = d["copper"]["tracks"].as_array().cloned().unwrap_or_default().into_iter().take(args["limit"].as_u64().unwrap_or(2000) as usize).collect();
    let outline: Vec<Value> = d["outline"]["outline"].as_array().cloned().unwrap_or_default().iter().map(|p| json!([mil(p[0].as_f64().unwrap_or(0.0)), mil(p[1].as_f64().unwrap_or(0.0))])).collect();
    json!({"success": true, "mutated": false, "data": {"kind": "board", "outlineMil": outline, "components": comps, "census": census, "tracks": tracks, "bodyCols": ["package3d"]},
           "documentName": rb["documentName"], "revision": rb["revision"], "roundtripMs": t0.elapsed().as_millis() as u64})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_layer_names_become_eagle_numbers() {
        assert_eq!([layer_number(&json!("F.Cu")), layer_number(&json!("In2.Cu")), layer_number(&json!("B.Cu")), layer_number(&json!(21)), layer_number(&json!("In15.Cu"))], [Some(1), Some(3), Some(16), Some(21), None]);
        assert_eq!(preset_layers("drill"), Some(vec![20, 44, 45]));
    }
    #[test]
    fn a_library_report_counts_pads_primitives_and_bodies() {
        let p = std::env::temp_dir().join(format!("adom-pr-{}.lbr", std::process::id()));
        std::fs::write(&p, r#"<eagle><drawing><library><packages><package name="R0603"><description>r</description><smd name="1" x="-0.8" y="0" dx="0.9" dy="1" layer="1"/><smd name="2" x="0.8" y="0" dx="0.9" dy="1" layer="1"/><wire x1="0" y1="0" x2="1" y2="0" layer="21"/></package></packages><packages3d><package3d name="R0603-3D"><packageinstances><packageinstance name="R0603"/></packageinstances></package3d></packages3d></library></drawing></eagle>"#).unwrap();
        let r = pcb_report(&json!({"path": p.to_string_lossy()}), &|_: &str, _: Value| json!({}));
        let c = &r["data"]["components"][0];
        assert_eq!((c["pads"].as_u64(), c["census"]["smd"].as_u64(), c["census"]["wire"].as_u64(), c["bodies"].clone()), (Some(2), Some(2), Some(1), json!(["R0603-3D"])));
        assert_eq!(c["padsBboxMil"], json!([-49.2, -19.7, 49.2, 19.7]));
        let _ = std::fs::remove_file(&p);
    }
    #[test]
    fn a_library_lists_its_components_or_its_footprints() {
        let p = std::env::temp_dir().join(format!("adom-ll-{}.lbr", std::process::id()));
        std::fs::write(&p, r#"<eagle><drawing><library><packages><package name="R0603"/><package name="SOT23"/></packages><symbols/><devicesets><deviceset name="R"/></devicesets></library></drawing></eagle>"#).unwrap();
        let path = p.to_string_lossy().to_string();
        let s = list_library(&json!({"libraryPath": path}));
        assert_eq!((s["kind"].as_str(), s["data"]["names"].clone()), (Some("sch"), json!(["R"])));
        let f = list_library(&json!({"libraryPath": path, "kind": "pcb", "limit": 1}));
        assert_eq!((f["data"]["count"].as_u64(), f["data"]["names"].clone()), (Some(2), json!(["R0603"])));
        let _ = std::fs::remove_file(&p);
    }
}

/// `fusion_update_pcb` (the standard's update_pcb): Fusion forward-annotates as you edit, so there is no
/// change order to run. The preview compares the sheet and the board; execute has nothing to carry over,
/// and it refuses removals: a part only on the board cannot be deleted from a command while the board is
/// linked to its schematic (DELETE does nothing there, live on 2706).
pub fn update_pcb(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let cc = crate::sch::call("cross_check", args, pass);
    if cc["success"].as_bool() != Some(true) { return cc; }
    let (add, remove) = (cc["onSheetNotOnBoard"].clone(), cc["onBoardNotOnSheet"].clone());
    let execute = args["execute"].as_bool() == Some(true);
    if execute && args["allowRemovals"].as_bool() == Some(true) && remove.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
        return json!({"success": false, "errorCode": "not_supported", "preview": {"addParts": add, "removeParts": remove}, "error": "Fusion cannot delete a part from a linked board by command: delete it in Fusion, or leave it (a part only on the board is usually a logo or a mounting hole).", "executed": false});
    }
    // nothing to run when the two agree. Parts only on the sheet are either package-less (supply symbols,
    // frames) or a design whose board is not linked to its sheet: never call that done
    let behind = add.as_array().map(|a| !a.is_empty()).unwrap_or(false);
    json!({"success": true, "executed": execute && !behind, "changeOrder": [],
        "preview": {"addParts": add, "removeParts": remove, "sheetParts": cc["schematicComponents"], "boardParts": cc["boardComponents"]},
        "sheet": cc["sheet"], "board": cc["board"],
        "_hint": if behind { "addParts are on the sheet and not on the board. A supply symbol or frame has no package and stays that way; a part with a footprint means this design's board is not linked to its sheet (opened from loose EAGLE files, or a fusion_project_create that answered linked:false): fusion_project_create, or fusion_board_seed with fusion_new_electronics_from_eagle, make a linked one." } else { "Fusion keeps the board in step with the sheet as you edit, so there is nothing to run. removeParts are only on the board and stay there." }})
}

/// `fusion_select_items` (the standard's select_items): select parts by designator, or clear the selection.
/// Fusion does not report its electronics selection back (see selection), so nothing can read it back.
pub fn select_items(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    // where: board or sheet. Fusion selects in the editor in front, and nothing here moves the user to another
    // editor, so the other one is refused by name
    if let Some(w) = args["where"].as_str() {
        if !matches!(w, "board" | "sheet") { return json!({"success": false, "errorCode": "bad_args", "error": "where is board or sheet"}); }
        let st = pass("get_app_state", json!({}));
        let ws = st["data"]["activeWorkspace"].as_str().unwrap_or("").to_lowercase();
        let front = if ws.contains("schematic") { "sheet" } else if ws.contains("pcb") || ws.contains("board") { "board" } else { "" };
        if !front.is_empty() && front != w {
            return json!({"success": false, "errorCode": "wrong_editor", "front": front, "error": format!("The {front} editor is in front, and Fusion selects only there: show the {w} first (fusion_show_{}).", if w == "board" { "2d_board" } else { "schematic" })});
        }
    }
    if args["clear"].as_bool() == Some(true) {
        let r = pass("run_modeling_script", json!({"script": "app.userInterface.activeSelections.clear()\nresult={'cleared':True}"}));
        return json!({"success": r["success"].as_bool() != Some(false), "cleared": true, "verified": false, "error": r["error"]});
    }
    let refs: Vec<String> = args["refs"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if refs.is_empty() { return json!({"success": false, "errorCode": "bad_args", "error": "refs is [designator], or clear:true"}); }
    // Fusion answers "Selected X" for any name at all, so the names are checked against the editor in front
    let script = "p=app.activeProduct\nitems=getattr(p,'parts',None) or getattr(p,'elements',None)\nresult={'names':[items.item(i).name for i in range(items.count)] if items else None}";
    let known: Option<Vec<String>> = pass("run_modeling_script", json!({"script": script}))["data"]["result"]["names"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_uppercase)).collect());
    let (mut selected, mut failed) = (Vec::new(), Vec::new());
    for r in &refs {
        if let Some(k) = &known { if !k.contains(&r.to_uppercase()) { failed.push(json!({"ref": r, "error": "no such part in the editor in front"})); continue; } }
        let x = pass("electron_select", json!({"name": r, "properties": false}));
        if x["success"].as_bool() == Some(false) { failed.push(json!({"ref": r, "error": x["error"]})); } else { selected.push(json!(r)); }
    }
    json!({"success": failed.is_empty(), "selected": selected, "failed": failed, "verified": false,
        "_hint": "Fusion does not report its electronics selection back, so this cannot be read back: look at the view (fusion_take_screenshot)."})
}

/// `fusion_extensions` (the standard's extensions): the Fusion add-ins and scripts installed for this user. Fusion
/// has no API to install or remove one, so those are refused by name.
pub fn extensions(args: &Value) -> Value {
    match args["action"].as_str().unwrap_or("list") {
        "list" => {}
        a @ ("install" | "remove") => return json!({"success": false, "errorCode": "not_supported", "error": format!("Fusion has no way to {a} an add-in from code: use its Scripts and Add-Ins dialog. The bridge deploys its own add-in itself.")}),
        o => return json!({"success": false, "errorCode": "bad_args", "error": format!("action is list, install or remove, not {o}")}),
    }
    let roaming = std::env::var("APPDATA").unwrap_or_default();
    let mut out = Vec::new();
    for (rel, kind) in [("Autodesk/FusionAddins", "addin"), ("Autodesk/Autodesk Fusion 360/API/AddIns", "addin"), ("Autodesk/Autodesk Fusion 360/API/Scripts", "script")] {
        for e in std::fs::read_dir(format!("{roaming}/{rel}")).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !e.path().is_dir() || out.iter().any(|x: &Value| x["id"] == json!(name)) { continue; }
            let manifest = std::fs::read_to_string(e.path().join(format!("{name}.manifest"))).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(json!({}));
            out.push(json!({"id": name, "name": name, "version": manifest["version"], "kind": kind}));
        }
    }
    json!({"success": true, "extensions": out, "count": out.len(), "_hint": "Fusion publishes no update feed for add-ins, so updateAvailable is not reported."})
}

/// `fusion_model_check` (the standard's model_check): per part, is there a 3D model, and does it resolve. Fusion's
/// models are cloud 3D packages, so "found" means the URN opens as a file for this account. Fusion can't say
/// whether one holds geometry without opening it, so hasGeometry is null. Read only.
pub fn model_check(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let (mut rows, limit): (Vec<(String, Vec<String>)>, usize) = (Vec::new(), args["limit"].as_u64().unwrap_or(50) as usize);
    let grab = |text: &str, tag: &str| -> Vec<(String, String)> {
        // (footprint name, wip urn or urn) for each <package3d>
        let re = regex::Regex::new(&format!(r#"(?s)<{tag}\b([^>]*)>(.*?)</{tag}>"#)).unwrap();
        let attr = |a: &str, k: &str| regex::Regex::new(&format!(r#"\b{k}="([^"]*)""#)).unwrap().captures(a).map(|c| c[1].to_string()).unwrap_or_default();
        let mut v = Vec::new();
        for c in re.captures_iter(text) {
            let u = { let w = attr(&c[1], "wip_urn"); if w.is_empty() { attr(&c[1], "urn") } else { w } };
            for i in regex::Regex::new(r#"<packageinstance name="([^"]+)""#).unwrap().captures_iter(&c[2]) { v.push((i[1].to_string(), u.clone())); }
        }
        v
    };
    if let Some(lib) = args["library"].as_str() {
        let Ok(t) = std::fs::read_to_string(lib) else { return json!({"success": false, "errorCode": "library_unreadable", "error": format!("cannot read {lib}")}) };
        let want = args["footprint"].as_str().unwrap_or("");
        let models = grab(&t, "package3d");
        for c in regex::Regex::new(r#"<package name="([^"]+)""#).unwrap().captures_iter(&t) {
            if !want.is_empty() && !c[1].eq_ignore_ascii_case(want) { continue; }
            rows.push((c[1].to_string(), models.iter().filter(|(n, _)| n == &c[1]).map(|(_, u)| u.clone()).collect()));
        }
        if rows.is_empty() && !want.is_empty() { return json!({"success": false, "errorCode": "unknown_footprint", "error": format!("no footprint {want} in {lib}")}); }
    } else {
        let src = crate::routing::call("board_source", args, pass);
        let Some(t) = src["source"].as_str().or(src["data"]["source"].as_str()).map(str::to_string) else { return src };
        let models = grab(&t, "package3d");
        for c in regex::Regex::new(r#"<element name="([^"]+)"[^>]*?package="([^"]+)""#).unwrap().captures_iter(&t) {
            rows.push((c[1].to_string(), models.iter().filter(|(n, _)| n == &c[2]).map(|(_, u)| u.clone()).collect()));
        }
    }
    let checked = rows.len();
    rows.truncate(limit);
    let urns: Vec<&String> = rows.iter().flat_map(|(_, u)| u.iter()).collect();
    let script = format!("import adsk.core,json\nout={{}}\nfor u in json.loads({}):\n    try:\n        lin=u.split('?')[0].replace('fs.file:vf.','dm.lineage:')\n        out[u]=app.data.findFileById(lin) is not None\n    except Exception:\n        out[u]=False\nresult={{'found':out}}", serde_json::to_string(&serde_json::to_string(&urns).unwrap()).unwrap());
    let found = pass("run_modeling_script", json!({"script": script}));
    let f = &found["data"]["result"]["found"];
    let mut all_ok = true;
    let parts: Vec<Value> = rows.iter().map(|(name, us)| {
        let models: Vec<Value> = us.iter().map(|u| json!({"path": u, "location": "cloud", "found": f[u.as_str()].as_bool().unwrap_or(false), "hasGeometry": Value::Null})).collect();
        let mut issues = Vec::new();
        if us.is_empty() { issues.push(json!("no 3D model")); }
        if models.iter().any(|m| m["found"] != json!(true)) { issues.push(json!("a 3D model's cloud file does not open for this account")); }
        if !issues.is_empty() { all_ok = false; }
        json!({"name": name, "models": models, "issues": issues})
    }).collect();
    json!({"success": true, "parts": parts, "checked": checked, "ok": all_ok,
        "_hint": "found means the cloud file opens for this account. hasGeometry is null: Fusion can't tell without opening each model. Whether a body sits in the right place is fusion_bodies."})
}

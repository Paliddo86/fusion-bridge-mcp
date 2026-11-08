//! The EDA Bridge Standard's checks tier through the shared engine (adom-eda-checks, vendored in
//! rust/vendor): the board model and its companions come from one export of the live board
//! (routing's check_inputs, plus fusion_bodies for the 3D checks), the engine computes, and the
//! reply names this bridge's verbs. thermal_vias and via_fence are engine plans that this bridge
//! places (place_planned_vias: read back, and undone on a mismatch) unless dryRun.
use serde_json::{json, Value};

// footprint_from_land_pattern needs no board: it writes a .lbr offline (main.rs native_lib_authoring)
// panelize builds a new board from the plan and opens it: boardgen::panelize, which has the Fusion context
// bom is stdshapes::bom (the engine with the schematic's parts, the variant and the files written);
// cross_check stays Fusion's own sheet-to-board compare (sch_script), which reads both documents live
pub fn owns(cmd: &str) -> bool { !matches!(cmd, "footprint_from_land_pattern" | "panelize" | "lint_library" | "bom" | "cross_check") && adom_eda_checks::CHECKS.contains(&cmd) }

/// A pcb-design-rules record for a fab (adom/pcb-design-rules rules/records/<vendor>.json): the
/// record for the board's copper layer count, or the asked process.
fn fab_record(vendor: &str, layers: u64, process: Option<&str>) -> Result<Value, String> {
    let slug: String = vendor.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    let url = format!("https://wiki.adom.inc/api/v1/pages/adom/pcb-design-rules/files/rules/records/{slug}.json");
    let (code, body) = fusion_core::aps::http_bytes(&url);
    if code != 200 { return Err(format!("no fab record for {vendor} (HTTP {code} from adom/pcb-design-rules)")); }
    let file: Value = serde_json::from_slice(&body).map_err(|e| format!("{url}: {e}"))?;
    let recs = file["records"].as_array().cloned().unwrap_or_default();
    let pick = recs.iter().find(|r| r["layerCount"].as_u64() == Some(layers) && process.map_or(true, |p| r["process"].as_str() == Some(p)))
        .or_else(|| recs.iter().find(|r| process.map_or(true, |p| r["process"].as_str() == Some(p))));
    match pick {
        Some(r) => { let mut r = r.clone(); r["vendor"] = file["vendor"].clone(); Ok(r) }
        None => Err(format!("{vendor} has no record for a {layers}-layer board{}", process.map(|p| format!(" ({p})")).unwrap_or_default())),
    }
}

pub fn call(cmd: &str, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let inputs = crate::routing::call("check_inputs", args, pass);
    if inputs["success"].as_bool() != Some(true) { return inputs; }
    let mut bundle = json!({"board": inputs["board"], "stack": inputs["stack"], "pours": inputs["pours"], "rules": inputs["rules"], "silk": inputs["silk"], "args": args});
    if cmd == "fab_check" {
        if args["fab"].is_object() { bundle["fab"] = args["fab"].clone(); }
        else if let Some(v) = args["vendor"].as_str() {
            let layers = inputs["stack"]["copperLayers"].as_u64().unwrap_or(2);
            match fab_record(v, layers, args["process"].as_str()) {
                Ok(r) => bundle["fab"] = r,
                Err(e) => return json!({"success": false, "errorCode": "no_fab_record", "error": e, "_hint": "Name a vendor adom/pcb-design-rules has (jlcpcb, pcbway, oshpark ...), or pass the record as fab, or limits:{...} in mm."}),
            }
        }
    }
    if matches!(cmd, "plan_route" | "plan_fanout" | "route_pair" | "length_tune") {
        // route_net's preflight holds the route to the board's strictest copper clearance (the
        // engine takes the smallest rule) and its copper-to-edge rule: plan to the same, so the
        // plan is one route_net accepts. A caller's larger value still wins.
        for k in ["clearanceMm", "edgeClearanceMm"] {
            let lim = inputs["routeLimits"][k].as_f64().unwrap_or(0.0);
            if args[k].as_f64().map_or(true, |v| v < lim) && lim > 0.0 { bundle["args"][k] = json!(lim); }
        }
    }
    if matches!(cmd, "assembly_check" | "board_report") && args["lcscMissing"].is_null() {
        // the BOM lines with no LCSC number, from the board's own part attributes (fusion_board_bom)
        let bom = crate::routing::call("board_bom", args, pass);
        if let Some(lines) = bom["lines"].as_array() {
            let missing: Vec<Value> = lines.iter().filter(|l| l["lcsc"].as_str().map(|s| s.trim().is_empty()).unwrap_or(true))
                .map(|l| json!({"designators": l["designators"], "value": l["value"], "package": l["package"], "mpn": l["mpn"]})).collect();
            bundle["args"]["lcscMissing"] = json!(missing);
        }
    }
    // EAGLE's DRC always checks shorts: there is no rule to find for it
    if matches!(cmd, "review_board" | "board_report") && args["alwaysChecked"].is_null() { bundle["args"]["alwaysChecked"] = json!(["short circuit"]); }
    if cmd == "silk_designators" { silk_boxes(&mut bundle["silk"]); }
    if cmd == "board_report" {
        // every companion this bridge has: the 3D bodies, a fab record (JLCPCB unless vendor names another),
        // Fusion's own DRC (it stores its result in the board: drc:false skips it) and a picture of the board
        let bodies = crate::bodies::bodies(args, pass);
        if bodies["success"].as_bool() == Some(true) { bundle["bodies"] = bodies; }
        let layers = inputs["stack"]["copperLayers"].as_u64().unwrap_or(2);
        if let Ok(r) = fab_record(args["vendor"].as_str().unwrap_or("jlcpcb"), layers, args["process"].as_str()) { bundle["fab"] = r; }
        if args["drc"].as_bool() != Some(false) {
            let d = crate::routing::call("run_drc", args, pass);
            if d["success"].as_bool() != Some(false) { bundle["drc"] = d; }
        }
        let png = std::env::temp_dir().join("adom-board-report").join("board.png").to_string_lossy().replace('\\', "/");
        let _ = std::fs::create_dir_all(std::path::Path::new(&png).parent().unwrap());
        let _ = pass("export_board_image", json!({"outputPath": png, "dpi": 150, "preset": "all"}));
        if let Ok(bytes) = std::fs::read(&png) { use base64::Engine; bundle["picture"] = json!(base64::engine::general_purpose::STANDARD.encode(bytes)); }
        if bundle["args"]["title"].is_null() { bundle["args"]["title"] = inputs["documentName"].clone(); }
    }
    if matches!(cmd, "pin_pad_check" | "schematic_review" | "review_board" | "board_report") {
        // the sheets' parts with their pins (number, name, electrical type) from Fusion's schematic,
        // and the netlist (Designator.Pad); Fusion keeps board and schematic in step
        // params: each part's attributes (MPN, MOUSER, DIGIKEY, LCSC, DATASHEET ...) for review_board's vendor checks
        let mut sa = args.clone(); sa["params"] = json!(true);
        // the sheets' wires, labels, pins and symbol boxes (sheetDrawing) for the drawing checks
        if matches!(cmd, "schematic_review" | "board_report") { sa["drawing"] = json!(true); }
        let sd = crate::sch::call("schematic_data", &sa, pass);
        if sd["success"].as_bool() == Some(false) { return sd; }
        bundle["sheetParts"] = json!(sheet_parts(&sd));
        if sd["sheetDrawing"].is_array() { bundle["sheetDrawing"] = sd["sheetDrawing"].clone(); }
        if matches!(cmd, "schematic_review" | "board_report") {
            let nl = crate::routing::call("netlist", args, pass);
            if nl["success"].as_bool() == Some(false) { return nl; }
            bundle["netlist"] = json!({"nets": nl["nets"]});
        }
    }
    if matches!(cmd, "height_check" | "placement_check" | "check_clearance" | "place_decoupling" | "find_space") {
        let bodies = crate::bodies::bodies(args, pass);
        if bodies["success"].as_bool() == Some(true) { bundle["bodies"] = bodies; }
        else if cmd == "height_check" { return bodies; }
    }
    let mut r = adom_eda_checks::run(cmd, &bundle);
    // the engine names verbs without a prefix: this bridge answers them as fusion_<verb>
    for key in ["apply", "plan", "pourPlan"] {
        if let Some(v) = r[key]["verb"].as_str().map(str::to_string) { r[key]["verb"] = json!(format!("fusion_{v}")); }
    }
    for k in ["boardId", "documentName", "revision"] { if !inputs[k].is_null() { r[k] = inputs[k].clone(); } }
    if matches!(cmd, "thermal_vias" | "via_fence") { return place(r, args, pass); }
    if cmd == "teardrops" { return place_teardrops(r, args, pass); }
    if cmd == "plan_fanout" {
        // each call is one fusion_route_net; every route changes the revision, so the next call
        // takes the revision the previous reply answered
        if let Some(calls) = r["calls"].as_array_mut() {
            for c in calls.iter_mut() {
                for (k, v) in [("boardId", inputs["boardId"].clone()), ("expectDocument", args["expectDocument"].clone())] { if !v.is_null() { c[k] = v; } }
            }
        }
        r["mutated"] = json!(false);
        r["_hint"] = json!("A plan: send each call to fusion_route_net in turn, with expectedRevision set to revision for the first and to the previous reply's revision after that. Drop or shorten whatever conflicts lists first. fusion_route_net undoes a call that splits another net's pour, or whose copper does not land as planned.");
    }
    if cmd == "silk_designators" && r["success"].as_bool() == Some(true) {
        r["mutated"] = json!(false);
        if args["dryRun"].as_bool() != Some(true) && r["applyArgs"]["names"].as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            let mut a = json!({"expectDocument": args["expectDocument"], "names": r["applyArgs"]["names"]});
            if !inputs["revision"].is_null() { a["expectedRevision"] = inputs["revision"].clone(); }
            let applied = crate::routing::call("apply_silkscreen", &a, pass);
            if applied["success"].as_bool() != Some(true) { let mut e = applied; e["planned"] = r["planned"].clone(); return e; }
            for k in ["applied", "failed", "results", "smashed", "revision"] { r[k] = applied[k].clone(); }
            r["mutated"] = json!(true);
        }
    }
    if cmd == "auto_classes" && r["success"].as_bool() == Some(true) && args["apply"].as_bool() == Some(true) {
        // Fusion's scoped rules are its net classes, and a class holds a width: a class the proposal gives
        // a width becomes one (add_rule); the others carry no rule to write, and Fusion pairs nets by their names
        let mut written = Vec::new(); let mut skipped = Vec::new();
        for c in r["applyArgs"]["net_classes"].as_array().cloned().unwrap_or_default() {
            match c["width"].as_f64() {
                Some(w) => {
                    let a = json!({"expectDocument": args["expectDocument"], "name": c["name"], "kind": "width", "min": w, "nets": c["nets"]});
                    let out = crate::routing::call("add_rule", &a, pass);
                    written.push(json!({"name": c["name"], "success": out["success"], "class": out["class"], "error": out["error"], "errorCode": out["errorCode"]}));
                }
                None => skipped.push(json!({"name": c["name"], "why": "no width, clearance or drill to hold: Fusion classes carry rules, not tags"})),
            }
        }
        r["applied"] = json!(written.iter().any(|w| w["success"] == true));
        r["mutated"] = r["applied"].clone();
        r["written"] = json!(written); r["skipped"] = json!(skipped);
        r["pairsNote"] = json!("Fusion pairs nets by their names (NAME_P/NAME_N): the proposed pairs are already pairs there (fusion_diff_pairs lists them)");
    }
    if cmd == "place_decoupling" && r["moveArgs"].is_object() {
        r["mutated"] = json!(false);
        r["_hint"] = json!("A plan: pass moveArgs to fusion_move_parts unchanged (it carries boardId and expectedRevision); validation is placement_check on the planned spots. Then route the caps short to the power pins.");
    }
    // the follow-up verb's guards, so its arguments pass on unchanged
    // (route_pair answers routeNet as a list, one per net; length_tune answers remove beside it)
    for key in ["routeNet", "moveArgs", "remove"] {
        let guards = [("boardId", inputs["boardId"].clone()), ("expectedRevision", inputs["revision"].clone()), ("expectDocument", args["expectDocument"].clone())];
        // look without writing: indexing a missing key mutably would add it as null
        if r.get(key).map(|v| v.is_object()).unwrap_or(false) {
            for (k, v) in guards.iter() { if !v.is_null() { r[key][*k] = v.clone(); } }
        } else if let Some(list) = r.get_mut(key).and_then(|v| v.as_array_mut()) {
            for item in list.iter_mut() { for (k, v) in guards.iter() { if *k != "expectedRevision" && !v.is_null() { item[*k] = v.clone(); } } }
        }
    }
    if cmd == "board_report" && r["text"].is_string() {
        // the page is written beside the board's other outputs; the reply keeps the path, not the page
        let text = r.as_object_mut().and_then(|o| o.remove("text")).and_then(|t| t.as_str().map(str::to_string)).unwrap_or_default();
        let dir = args["outputDir"].as_str().map(|s| s.replace('\\', "/")).unwrap_or_else(|| std::env::temp_dir().join("adom-board-report").to_string_lossy().replace('\\', "/"));
        let _ = std::fs::create_dir_all(&dir);
        let name: String = inputs["documentName"].as_str().unwrap_or("board").chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
        let path = format!("{dir}/{name}-report.html");
        if let Err(e) = std::fs::write(&path, text.as_bytes()) { return json!({"success": false, "errorCode": "write_failed", "error": format!("{path}: {e}")}); }
        r["path"] = json!(path); r["sizeBytes"] = json!(text.len());
        r["drcStoredInBoard"] = json!(bundle.get("drc").is_some());
    }
    if matches!(cmd, "route_pair" | "length_tune") {
        r["mutated"] = json!(false);
        r["_hint"] = json!(if cmd == "route_pair" {
            "A plan: send each routeNet entry to fusion_route_net in turn (expectedRevision: revision for the first, the previous reply's after), then widthRule through fusion_add_rule if you want the pair's width held. problems lists crossings and copper too close: while there are any it is not a route to place."
        } else {
            "A plan: fusion_remove_items with remove (the one track), then fusion_route_net with routeNet (expectedRevision: the revision fusion_remove_items answered). If the route is refused, put the removed track back with fusion_route_net."
        });
    }
    if cmd == "plan_route" && r["routeNet"].is_object() {
        r["mutated"] = json!(false);
        r["_hint"] = json!("Pass routeNet to fusion_route_net unchanged (it carries boardId and expectedRevision), then fusion_repour: pours are not obstacles to the plan, and Fusion repours them round the new copper. fusion_run_drc is still the proof. Fine-pitch pads need gridMm 0.1.");
    }
    r
}

/// Place an engine via plan unless dryRun: Fusion draws the vias, reads them back and undoes them
/// all on a mismatch, and the reply is the standard's applied / failed / revision.
fn place(mut r: Value, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    r["mutated"] = json!(false);
    if r["success"].as_bool() != Some(true) || args["dryRun"].as_bool() == Some(true) { return r; }
    let mut a = args.clone();
    a["vias"] = r["apply"]["args"]["vias"].clone();
    if !r["revision"].is_null() && a["expectedRevision"].is_null() { a["expectedRevision"] = r["revision"].clone(); }
    let placed = crate::routing::call("place_planned_vias", &a, pass);
    if placed["success"].as_bool() != Some(true) {
        let mut e = placed; e["planned"] = r["planned"].clone(); e["engine"] = r["engine"].clone(); return e;
    }
    for k in ["applied", "failed", "revision", "boardId", "documentName"] { if !placed[k].is_null() { r[k] = placed[k].clone(); } }
    r["mutated"] = json!(true);
    if let Some(o) = r.as_object_mut() { o.remove("apply"); }
    r["_hint"] = json!(if r["planned"]["pad"].is_null() { "Then fusion_repour so the pours join the vias, and fusion_routing_drc." }
        else { "Open vias in a pad wick solder away during reflow: tent or plug them, or keep drills at 0.3 mm or less. Pour the pad's net on the far side (fusion_apply_pours) so the heat has somewhere to go." });
    r
}

/// `fusion_silkscreen_prep`: the standard's silkscreen_prep in Fusion's terms. Designators that sit
/// on copper are moved off it where they fit (the engine's silk_designators plan, applied), counted
/// before and after; silk drawn outside the board is listed. A footprint's own silk lines belong to
/// its library package in EAGLE and cannot be clipped on one board: the fab's CAM (and JLCPCB) clip
/// silkscreen at the mask openings.
pub fn silkscreen_prep(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut dry = args.clone(); dry["dryRun"] = json!(true);
    let before = call("silk_designators", &dry, pass);
    if before["success"].as_bool() == Some(false) { return before; }
    let n = |r: &Value| r["onPadsBefore"].as_array().map(|a| a.len()).unwrap_or(0);
    let mut out = json!({"success": true, "mutated": false, "before": {"designatorsOnPads": n(&before)}, "engine": before["engine"]});
    if args["moveText"].as_bool() != Some(false) && args["dryRun"].as_bool() != Some(true) && before["planned"].as_array().map(|a| !a.is_empty()).unwrap_or(false) {
        let mut a = args.clone(); if let Some(o) = a.as_object_mut() { o.remove("dryRun"); }
        let applied = call("silk_designators", &a, pass);
        if applied["success"].as_bool() == Some(false) { let mut e = applied; e["before"] = out["before"].clone(); return e; }
        out["mutated"] = json!(true);
        out["moved"] = applied["applied"].clone().as_array().map(|x| json!(x.len())).unwrap_or(applied["moved"].clone());
    } else {
        out["planned"] = json!(before["planned"].as_array().map(|a| a.len()).unwrap_or(0));
    }
    let after = call("silk_designators", &dry, pass);
    out["after"] = json!({"designatorsOnPads": n(&after)});
    out["stillOnPads"] = after["onPadsBefore"].clone();
    out["unplaced"] = json!(after["unplaced"].as_array().map(|a| a.len()).unwrap_or(0));
    out["silkOffCopper"] = json!(n(&after) == 0);
    // loose silk on the board's own layers wholly outside its outline: listed for fusion_remove_graphics
    let st = crate::routing::call("routing_state", args, pass);
    let ol: Vec<(f64, f64)> = st["outline"].as_array().into_iter().flatten().filter_map(|p| Some((p[0].as_f64()?, p[1].as_f64()?))).collect();
    if !ol.is_empty() {
        let (x0, y0, x1, y1) = ol.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |a, p| (a.0.min(p.0), a.1.min(p.1), a.2.max(p.0), a.3.max(p.1)));
        let src = crate::routing::call("board_source", args, pass);
        let text = src["source"].as_str().or(src["data"]["source"].as_str()).unwrap_or("");
        let re = regex::Regex::new(r#"<(wire|text|circle|rectangle) [^>]*layer="(21|22)"[^>]*>"#).unwrap();
        let num = |t: &str, k: &str| regex::Regex::new(&format!(r#"\b{k}="(-?[0-9.]+)""#)).unwrap().captures(t).and_then(|c| c[1].parse::<f64>().ok());
        // plain silk only: the board's <plain> section, not the footprints in its libraries
        let plain = text.split("<plain>").nth(1).and_then(|t| t.split("</plain>").next()).unwrap_or("");
        let outside: Vec<Value> = re.find_iter(plain).filter_map(|m| {
            let t = m.as_str();
            let xs: Vec<f64> = ["x", "x1", "x2"].iter().filter_map(|k| num(t, k)).collect();
            let ys: Vec<f64> = ["y", "y1", "y2"].iter().filter_map(|k| num(t, k)).collect();
            if xs.is_empty() || ys.is_empty() { return None; }
            let all_out = xs.iter().all(|x| *x < x0 || *x > x1) || ys.iter().all(|y| *y < y0 || *y > y1);
            all_out.then(|| json!({"kind": m.as_str().split_whitespace().next().unwrap_or("").trim_start_matches('<'), "x": xs[0], "y": ys[0]}))
        }).collect();
        out["silkOutsideBoard"] = json!(outside);
    }
    out["_hint"] = json!("Designators are moved off copper where they fit (unplaced: no clear spot beside the part; fusion_apply_silkscreen moves one by hand). A footprint's own silk lines are its library package's: Fusion cannot clip them on one board, and the fab's CAM clips silkscreen at mask openings (JLCPCB does). silkOutsideBoard lists loose silk off the board for fusion_remove_graphics.");
    out
}

/// The engine's teardrops drawn as named solid polygons (routing_script place_teardrops), read back.
fn place_teardrops(mut r: Value, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    r["mutated"] = json!(false);
    let n = r["teardrops"].as_array().map(|a| a.len()).unwrap_or(0);
    if r["success"].as_bool() == Some(false) || args["dryRun"].as_bool() == Some(true) || n == 0 { return r; }
    let mut a = args.clone();
    a["teardrops"] = r["teardrops"].clone();
    if !r["revision"].is_null() && a["expectedRevision"].is_null() { a["expectedRevision"] = r["revision"].clone(); }
    let placed = crate::routing::call("place_teardrops", &a, pass);
    if placed["success"].as_bool() != Some(true) { let mut e = placed; e["planned"] = json!(n); e["engine"] = r["engine"].clone(); return e; }
    for k in ["applied", "revision", "boardId", "documentName"] { if !placed[k].is_null() { r[k] = placed[k].clone(); } }
    r["mutated"] = json!(true);
    r["_hint"] = json!("Teardrops are named solid polygons (rank 1, no thermals) of each track's net: fusion_repour, then fusion_routing_drc. Remove them with fusion_remove_pours on their nets, or UNDO.");
    r
}

/// Fusion's silk_state gives each designator's origin, size and turn; the shared engine's silk
/// planner reads a box. The box of EAGLE's vector text from its origin (lower left before the turn):
/// characters about 0.8 of the size wide, mirrored on the bottom side.
fn silk_boxes(silk: &mut Value) {
    let Some(comps) = silk["components"].as_array_mut() else { return };
    for c in comps.iter_mut() {
        let n = &c["name"];
        let (Some(x), Some(y)) = (n["x"].as_f64(), n["y"].as_f64()) else { continue };
        let h = n["size"].as_f64().unwrap_or(1.27);
        let w = n["text"].as_str().map(|t| t.chars().count()).unwrap_or(2).max(1) as f64 * h * 0.8;
        let rot = ((n["rotation"].as_f64().unwrap_or(0.0) % 360.0) + 360.0) % 360.0;
        let (mut x1, y1, mut x2, y2) = match rot.round() as i64 {
            90 => (x - h, y, x, y + w),
            180 => (x - w, y - h, x, y),
            270 => (x, y - w, x + h, y),
            _ => (x, y, x + w, y + h),
        };
        if n["side"].as_str() == Some("bottom") || c["side"].as_str() == Some("bottom") { let (a, b) = (2.0 * x - x2, 2.0 * x - x1); x1 = a; x2 = b; }
        let r = |v: f64| (v * 1000.0).round() / 1000.0;
        c["name"]["box"] = json!([r(x1), r(y1), r(x2), r(y2)]);
    }
}

#[cfg(test)]
mod silk_tests {
    use super::*;
    #[test]
    fn designator_boxes_follow_turn_and_side() {
        let mut s = json!({"components": [
            {"ref": "R1", "side": "top", "name": {"x": 10.0, "y": 5.0, "size": 1.0, "rotation": 0.0, "text": "R1"}},
            {"ref": "R2", "side": "top", "name": {"x": 10.0, "y": 5.0, "size": 1.0, "rotation": 90.0, "text": "R2"}},
            {"ref": "R3", "side": "bottom", "name": {"x": 10.0, "y": 5.0, "size": 1.0, "rotation": 0.0, "text": "R3"}}]});
        silk_boxes(&mut s);
        assert_eq!(s["components"][0]["name"]["box"], json!([10.0, 5.0, 11.6, 6.0]));
        assert_eq!(s["components"][1]["name"]["box"], json!([9.0, 5.0, 10.0, 6.6]));
        assert_eq!(s["components"][2]["name"]["box"], json!([8.4, 5.0, 10.0, 6.0]));
    }
}

/// fusion_schematic_data's components as the standard's sheetParts: {designator, pins: [{number,
/// name, electrical}]}, a pin that lands on several pads listed once per pad.
pub(crate) fn sheet_parts(sd: &Value) -> Vec<Value> {
    // a symbol with no package (EAGLE's supply symbols: GND, VCC) is no part: it has no pads, and it
    // joins its net by the pin's name
    sd["components"].as_array().cloned().unwrap_or_default().iter()
        .filter(|c| c["pins"].as_array().map(|ps| ps.iter().any(|p| !p["number"].as_str().unwrap_or("").trim().is_empty())).unwrap_or(false))
        .map(|c| {
        let pins: Vec<Value> = c["pins"].as_array().cloned().unwrap_or_default().iter().flat_map(|p| {
            let nums: Vec<String> = p["number"].as_str().unwrap_or("").split_whitespace().map(str::to_string).collect();
            let nums = if nums.is_empty() { vec![String::new()] } else { nums };
            nums.into_iter().map(move |n| json!({"number": n, "name": p["name"].clone(), "electrical": p["electrical"].clone(), "gate": p["gate"].clone()}))
        }).collect();
        // the standard's params [{name, value}], empty values left out (EAGLE libraries carry many blank fields)
        let params: Vec<Value> = c["params"].as_object().into_iter().flatten()
            .filter_map(|(k, v)| v.as_str().map(str::trim).filter(|v| !v.is_empty()).map(|v| json!({"name": k, "value": v}))).collect();
        // value and libRef (the deviceset: EAGLE's device name) feed the engine's design checks
        // EAGLE libraries keep a part's description in a DESCRIPTION or DESC attribute
        let description = ["DESCRIPTION", "DESC", "Description"].iter().find_map(|k| c["params"][*k].as_str().map(str::trim).filter(|s| !s.is_empty())).unwrap_or("");
        json!({"designator": c["designator"], "value": c["comment"], "libRef": c["libReference"], "description": description, "pins": pins, "params": params})
    }).collect()
}

#[cfg(test)]
mod sheet_tests {
    use super::*;
    #[test]
    fn a_pin_on_two_pads_is_listed_per_pad() {
        let sd = json!({"components": [{"designator": "U1", "pins": [{"number": "3 4", "name": "GND", "electrical": "pwr", "gate": "G$1"}, {"number": "1", "name": "IN", "electrical": 1}]},
                                       {"designator": "SUPPLY1", "pins": [{"number": "", "name": "GND", "electrical": "sup"}]}]});
        let p = sheet_parts(&sd);
        assert_eq!(p.len(), 1, "a supply symbol is no part");
        assert_eq!(p[0]["pins"].as_array().unwrap().len(), 3);
        assert_eq!(p[0]["pins"][1]["number"], "4");
        let sd = json!({"components": [{"designator": "R1", "pins": [{"number": "1", "name": "1"}], "params": {"MPN": "RC0603", "LCSC": " ", "DATASHEET": "https://x"}}]});
        assert_eq!(sheet_parts(&sd)[0]["params"], json!([{"name": "MPN", "value": "RC0603"}, {"name": "DATASHEET", "value": "https://x"}]), "blank LCSC left out, the part's order kept");
    }
}

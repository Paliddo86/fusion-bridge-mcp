//! `fusion_jlcpcb_order`: the standard's jlcpcb_order in one call. The board against JLCPCB's fab
//! limits (fab_check vendor jlcpcb), the Gerbers and drill zipped by the bundled JLCPCB CAM job, the
//! JLCPCB BOM and pick-and-place, the assembly-tier check and the order-form values, in one folder,
//! with every issue in one list. Fusion has no Silkscreen Preparation: the silk goes out as drawn.
use serde_json::{json, Value};

fn data(r: &Value) -> Value { if r["data"].is_object() { r["data"].clone() } else { r.clone() } }

pub fn jlcpcb_order(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let st = crate::routing::call("routing_state", args, pass);
    if st["success"].as_bool() == Some(false) { return st; }
    let doc = st["documentName"].as_str().unwrap_or("board").to_string();
    let safe: String = doc.chars().map(|c| if c.is_ascii_alphanumeric() || "-_. ".contains(c) { c } else { '_' }).collect();
    let folder = args["outputDir"].as_str().map(|s| s.replace('\\', "/"))
        .unwrap_or_else(|| std::env::temp_dir().join(format!("JLCPCB order {safe}")).to_string_lossy().replace('\\', "/"));
    // written fresh each time
    let _ = std::fs::remove_dir_all(&folder);
    if let Err(e) = std::fs::create_dir_all(&folder) { return json!({"success": false, "errorCode": "write_failed", "error": format!("{folder}: {e}")}); }
    let mut issues: Vec<Value> = Vec::new();
    let mut a = args.clone(); a["vendor"] = json!("jlcpcb");
    let fab = crate::checks::call("fab_check", &a, pass);
    if fab["success"].as_bool() == Some(false) { issues.push(json!({"from": "fab_check", "what": fab["error"]})); }
    for o in fab["outsideLimits"].as_array().cloned().unwrap_or_default() {
        issues.push(json!({"from": "fab_check", "what": format!("{} {} mm against JLCPCB's {} mm", o["limit"].as_str().unwrap_or("?"), o["boardMm"], o["limitMm"]), "at": o["at"]}));
    }
    let g = data(&pass("export_gerbers", json!({"outputDir": folder})));
    let gerber_zip = json!({"path": g["zipPath"], "files": g["fileCount"]});
    if g["zipPath"].is_null() { issues.push(json!({"from": "export_gerbers", "what": g["error"].as_str().unwrap_or("the CAM job wrote no zip")})); }
    let bom = crate::routing::call("board_bom", &json!({"expectDocument": args["expectDocument"], "outputDir": folder}), pass);
    let mut bom_files: Vec<Value> = bom["files"].as_object().map(|o| o.values().cloned().collect()).unwrap_or_default();
    if bom["success"].as_bool() == Some(false) { issues.push(json!({"from": "board_bom", "what": bom["error"]})); }
    let cpl_path = format!("{folder}/cpl-jlcpcb.csv");
    let cpl = data(&pass("export_cpl", json!({"outputPath": cpl_path, "side": "all"})));
    if std::path::Path::new(&cpl_path).is_file() { bom_files.push(json!(cpl_path)); } else { issues.push(json!({"from": "export_cpl", "what": cpl["error"].as_str().unwrap_or("no pick-and-place file was written")})); }
    let asm = crate::checks::call("assembly_check", args, pass);
    // no tier passes: the cheaper tier's failing rules are what stand between the board and assembly
    if asm["service"].is_null() {
        for r in asm["economic"]["rules"].as_array().cloned().unwrap_or_default().iter().filter(|r| r["ok"] == false) {
            issues.push(json!({"from": "assembly_check", "what": format!("{}: {}", r["rule"].as_str().unwrap_or("?"), r["detail"].as_str().unwrap_or(""))}));
        }
    }
    // the order form: what JLCPCB's page asks for
    let pts: Vec<(f64, f64)> = st["outline"].as_array().into_iter().flatten().filter_map(|p| Some((p[0].as_f64()?, p[1].as_f64()?))).collect();
    let dims = if pts.is_empty() { Value::Null } else {
        let (x0, x1) = pts.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
        let (y0, y1) = pts.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
        json!([((x1 - x0) * 100.0).round() / 100.0, ((y1 - y0) * 100.0).round() / 100.0])
    };
    let stack = crate::routing::call("layer_stack", args, pass);
    // outer copper in oz from the stack-up's first layer (35 um = 1 oz), to the half ounce
    let top_cu = stack["layers"].as_array().and_then(|l| l.first()).and_then(|x| x["copperUm"].as_f64()).map(|um| ((um / 35.0) * 2.0).round() / 2.0);
    let m = &fab["measured"];
    let mm = |k: &str| m[k]["mm"].as_f64().or_else(|| m[k].as_f64());
    let hole = [mm("padHole"), mm("viaDrill")].into_iter().flatten().fold(None::<f64>, |a, v| Some(a.map_or(v, |a| a.min(v))));
    let order_form = json!({"layers": st["copperLayers"].as_array().map(|l| l.len()), "dimensionsMm": dims, "outerCopperOz": top_cu,
        "minTrackSpaceMm": [mm("track"), mm("clearanceRule")], "minHoleMm": hole,
        "_note": "minTrackSpaceMm is the narrowest track and the board's copper clearance rule"});
    let fab_ok = fab["success"].as_bool() != Some(false) && fab["outsideLimits"].as_array().map(|a| a.is_empty()).unwrap_or(false);
    let service = asm["service"].clone();
    let ready = fab_ok && !service.is_null() && !g["zipPath"].is_null() && issues.is_empty();
    json!({"success": true, "mutated": false, "documentName": doc, "folder": folder, "ready": ready, "issues": issues,
           "gerberZip": gerber_zip, "bomFiles": bom_files, "assemblyService": service, "orderForm": order_form,
           "fab": {"ok": fab_ok, "record": fab["fab"]["name"], "engine": fab["engine"]}, "assembly": {"economic": asm["economic"]["ok"], "standard": asm["standard"]["ok"]},
           "_hint": "Fix what issues lists and run it again; when ready, upload the zip, then the BOM and CPL for assembly and check the rotations in JLCPCB's preview. Fusion has no Silkscreen Preparation: silk goes out as drawn (fusion_apply_silkscreen tidies it)."})
}

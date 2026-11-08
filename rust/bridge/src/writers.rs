//! fusion_export_gencad and fusion_export_dxf {layers} through the standard's shared writers
//! (adom-eda-checks, standard #16): the board model and Fusion's poured copper in, the file
//! text out, written here. DXF layers the engine does not draw with texts (silkscreen, fab,
//! documentation) stay on this bridge's own writer (routing_script board_dxf), so nothing is lost.
use serde_json::{json, Value};

pub fn owns(cmd: &str) -> bool { matches!(cmd, "export_gencad" | "board_dxf" | "export_ipc2581") }

fn safe(name: &str) -> String { name.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect() }

fn out_dir(args: &Value, default: &str) -> String {
    args["outputDir"].as_str().map(|s| s.replace('\\', "/")).unwrap_or_else(|| std::env::temp_dir().join(default).to_string_lossy().replace('\\', "/"))
}

/// Copper and the outline go to the engine; everything else stays on the bridge writer.
pub fn engine_layer(l: &str) -> bool { l == "Edge.Cuts" || l.ends_with(".Cu") }

pub fn call(cmd: &str, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let board = crate::routing::call("routing_state", args, pass);
    if board["success"].as_bool() == Some(false) { return board; }
    let doc = board["documentName"].as_str().unwrap_or("board").to_string();
    match cmd {
        "export_ipc2581" => {
            // the stack-up for the layer thicknesses and Fusion's poured copper island by island
            let stack = crate::routing::call("layer_stack", args, pass);
            let pours = crate::routing::call("pour_state", args, pass);
            let mut a = args.clone();
            if a["title"].is_null() { a["title"] = json!(doc); }
            let r = adom_eda_checks::run("export_ipc2581", &json!({"board": board, "stack": stack, "pours": pours, "args": a}));
            let Some(text) = r["text"].as_str() else { return r };
            let dir = out_dir(args, "adom-ipc2581");
            let _ = std::fs::create_dir_all(&dir);
            let path = format!("{dir}/{}.xml", safe(&doc));
            if let Err(e) = std::fs::write(&path, text) { return json!({"success": false, "errorCode": "write_failed", "error": format!("{path}: {e}")}); }
            let mut out = json!({"success": true, "mutated": false, "documentName": doc, "path": path, "format": r["format"], "mode": r["mode"], "units": r["units"],
                                 "counts": r["counts"], "writtenFrom": r["writtenFrom"], "engine": r["engine"], "_hint": r["_hint"]});
            if let Some(c) = r["counts"].as_object() { for (k, v) in c { out[k] = v.clone(); } }
            out
        }
        "export_gencad" => {
            let r = adom_eda_checks::run("export_gencad", &json!({"board": board, "args": args}));
            let Some(text) = r["text"].as_str() else { return r };
            let dir = out_dir(args, "adom-gencad");
            let _ = std::fs::create_dir_all(&dir);
            let path = format!("{dir}/{}.cad", safe(&doc));
            if let Err(e) = std::fs::write(&path, text) { return json!({"success": false, "errorCode": "write_failed", "error": format!("{path}: {e}")}); }
            json!({"success": true, "mutated": false, "documentName": doc, "path": path, "format": "GenCAD 1.4", "units": r["units"], "counts": r["counts"],
                   "components": r["counts"]["components"], "signals": r["counts"]["signals"], "shapes": r["counts"]["shapes"], "pads": r["counts"]["pads"],
                   "writtenFrom": r["writtenFrom"], "engine": r["engine"],
                   "_hint": "Written by the EDA Bridge Standard's shared writer (adom-eda-checks) from Fusion's board model, in inches; a bottom part is MIRRORX FLIP, as KiCad writes it."})
        }
        _ => {
            let want: Vec<String> = args["layers"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
            let (eng, own): (Vec<String>, Vec<String>) = want.iter().cloned().partition(|l| engine_layer(l));
            let dir = out_dir(args, "adom-board-dxf");
            let _ = std::fs::create_dir_all(&dir);
            let mut files: Vec<Value> = Vec::new();
            let mut notes: Vec<Value> = Vec::new();
            let mut as_drawn = Value::Null;
            let mut engine = Value::Null;
            if !eng.is_empty() {
                let pours = crate::routing::call("pour_state", args, pass);
                let mut a = args.clone(); a["layers"] = json!(eng);
                let r = adom_eda_checks::run("export_dxf", &json!({"board": board, "pours": pours, "args": a}));
                for f in r["files"].as_array().cloned().unwrap_or_default() {
                    let path = format!("{dir}/{}-{}", safe(&doc), f["name"].as_str().unwrap_or("layer.dxf"));
                    if let Err(e) = std::fs::write(&path, f["text"].as_str().unwrap_or("")) { return json!({"success": false, "errorCode": "write_failed", "error": format!("{path}: {e}")}); }
                    files.push(json!({"layer": f["layer"], "path": path, "entities": f["entities"], "writer": "adom-eda-checks"}));
                }
                if let Some(u) = r["unknownLayers"].as_array() { if !u.is_empty() { notes.push(json!({"unknownLayers": u})); } }
                as_drawn = r["poursAsDrawn"].clone();
                engine = r["engine"].clone();
            }
            if !own.is_empty() {
                let mut a = args.clone(); a["layers"] = json!(own); a["outputDir"] = json!(dir);
                let r = crate::routing::call("board_dxf", &a, pass);
                if r["success"].as_bool() == Some(false) { return r; }
                for f in r["files"].as_array().cloned().unwrap_or_default() { let mut f = f; f["writer"] = json!("fusion-bridge"); files.push(f); }
            }
            json!({"success": !files.is_empty(), "mutated": false, "documentName": doc, "files": files, "poursAsDrawn": as_drawn, "notes": notes, "units": "mm",
                   "engine": engine,
                   "_hint": "Copper and Edge.Cuts are written by the standard's shared writer from the board model with Fusion's poured copper; silkscreen, fab and documentation layers by this bridge's writer, which also draws their texts."})
        }
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn copper_and_outline_go_to_the_engine() {
        assert!(engine_layer("F.Cu") && engine_layer("In2.Cu") && engine_layer("Edge.Cuts"));
        assert!(!engine_layer("F.SilkS") && !engine_layer("F.Fab") && !engine_layer("Dwgs.User"));
    }
}

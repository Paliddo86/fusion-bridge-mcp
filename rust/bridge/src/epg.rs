//! `fusion_generate_package`: an IPC package from Fusion's built-in ElectronicsPackageGenerator,
//! optionally marked and exported, in one headless add-in script. The script is the Python
//! bridge's f-string, byte for byte (tests/golden/epg_script.json).
use serde_json::{json, Value};

const TEMPLATE: &str = include_str!("epg_template.py");
const TYPES_JSON: &str = include_str!("epg_types.json");

fn types() -> (Vec<String>, Vec<String>) {
    let v: Value = serde_json::from_str(TYPES_JSON).unwrap_or(json!({}));
    let s = |k: &str| v[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    (s("types"), s("nonDimension"))
}

/// Python `repr(float)`: shortest round-trip digits, exponent form outside 1e-4..1e16.
pub fn py_float_repr(f: f64) -> String {
    if f.is_nan() { return "nan".into(); }
    if f.is_infinite() { return if f > 0.0 { "inf".into() } else { "-inf".into() }; }
    if f == 0.0 { return if f.is_sign_negative() { "-0.0".into() } else { "0.0".into() }; }
    let e = format!("{:e}", f);
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if (-4..16).contains(&exp) {
        let s = format!("{}", f);
        if s.contains('.') { s } else { format!("{s}.0") }
    } else {
        format!("{}e{}{:02}", mant, if exp < 0 { "-" } else { "+" }, exp.abs())
    }
}

/// Python `repr()` of a JSON value (dicts keep insertion order, like Python's).
pub fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True".into() } else { "False".into() },
        Value::Number(n) => if let Some(i) = n.as_i64() { i.to_string() } else if let Some(u) = n.as_u64() { u.to_string() } else { py_float_repr(n.as_f64().unwrap_or(0.0)) },
        Value::String(s) => fusion_lbr::py_repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!("{{{}}}", o.iter().map(|(k, v)| format!("{}: {}", fusion_lbr::py_repr_str(k), py_repr(v))).collect::<Vec<_>>().join(", ")),
    }
}

/// The add-in script for one request, or the refusal for an unknown type.
pub fn script_for(args: &Value, bridge_version: &str) -> Result<String, Value> {
    let (all, non_dim) = types();
    let pkg_type = args["type"].as_str().unwrap_or("").trim().to_lowercase();
    if !all.contains(&pkg_type) {
        return Err(json!({"success": false, "error": format!("Unknown package type: {}", fusion_lbr::py_repr_str(&pkg_type)), "errorCode": "unknown_package_type", "supportedTypes": all,
            "_hint": "Pass type as one of supportedTypes (EPG Scripts3d module names). Common: chip (0402/0603/0805 passives), soic, qfn, qfp, bga, sot23, dfn2, melf, ecap, crystal, header_straight."}));
    }
    let units_cm = args["unitsCm"].as_bool().unwrap_or(false);
    let mut params = serde_json::Map::new();
    if let Some(o) = args["params"].as_object() {
        for (k, v) in o {
            let conv = !units_cm && v.is_number() && !v.is_boolean() && !non_dim.contains(k);
            params.insert(k.clone(), if conv { json!(v.as_f64().unwrap_or(0.0) / 10.0) } else { v.clone() });
        }
    }
    let etch = args["etch"].as_str().unwrap_or("").to_string();
    let etch_style = args["etchStyle"].as_str().filter(|s| !s.is_empty()).unwrap_or("raised").trim().to_lowercase();
    let etch_depth_cm = args["etchDepthMm"].as_f64().filter(|d| *d != 0.0).unwrap_or(0.03) / 10.0;
    let etch_height = args["etchHeightMm"].clone();
    let output_step = args["outputStep"].as_str().unwrap_or("").to_string();
    Ok(TEMPLATE.replace("@@RAW_PKG_TYPE@@", &pkg_type).replace("@@PKG_TYPE@@", &fusion_lbr::py_repr_str(&pkg_type))
        .replace("@@PARAMS@@", &py_repr(&Value::Object(params))).replace("@@ETCH_HEIGHT_MM@@", &py_repr(&etch_height))
        .replace("@@ETCH_DEPTH_CM@@", &py_float_repr(etch_depth_cm)).replace("@@ETCH_STYLE@@", &fusion_lbr::py_repr_str(&etch_style))
        .replace("@@ETCH@@", &fusion_lbr::py_repr_str(&etch)).replace("@@OUTPUT_STEP@@", &fusion_lbr::py_repr_str(&output_step))
        .replace("@@BRIDGE_VERSION@@", &fusion_lbr::py_repr_str(bridge_version)))
}

pub fn generate_package(args: &Value, bridge_version: &str, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let script = match script_for(args, bridge_version) { Ok(s) => s, Err(e) => return e };
    let mut res = pass("run_modeling_script", json!({"script": script}));
    if res["success"].as_bool() != Some(true) {
        if res["error"].as_str().unwrap_or("").contains("EPG_NOT_FOUND") {
            res["errorCode"] = json!("epg_not_found");
            res["_hint"] = json!("Fusion's built-in ElectronicsPackageGenerator add-in was not found in this install (ships with 2024+ Fusion under webdeploy .../Api/InternalAddins). Update Fusion, or build the part with fusion_make_3d_package from a STEP instead.");
        }
        return res;
    }
    let inner = res["data"]["result"].clone();
    let etch = args["etch"].as_str().unwrap_or("");
    let etch_style = args["etchStyle"].as_str().filter(|s| !s.is_empty()).unwrap_or("raised").trim().to_lowercase();
    let output_step = args["outputStep"].as_str().unwrap_or("");
    let mut out = json!({"success": true, "output": inner.to_string()});
    if let Some(o) = inner.as_object() { for (k, v) in o { out[k] = v.clone(); } }
    out["_hint"] = json!(format!("Generated an IPC-compliant parametric package via Fusion's built-in EPG{}{}. NEXT: fusion_screenshot_fusion for a visual check; pull_file the STEP for KiCad. HOW THE MARKING WORKS: the top surface is found from the WHOLE-chip bounding box - only upward planar faces in the top 5% height band count, largest area wins - and the text is laid out with a 10% margin inside that face, auto-fit width-aware (override with etchHeightMm). Default style is raised+white for CONTRAST (humans can't read a dark-on-dark emboss); pass etchStyle:'engraved' for a sunken laser cut. The text ALWAYS runs along the LONGEST axis of the top face (rotated 90deg when needed) for maximum room; the 10% margin is ENFORCED by measuring the placed text bbox and shrink-to-fitting (see fitIterations in the manifest). Multi-line markings work - join lines with a newline (e.g. 'LM358' + newline + 'ADOM-A' for MPN + variant; per-line auto-fit). SIDECAR MANIFEST: when outputStep is set, <step>.manifest.json records every setting picked + WHY + the calculated bboxes (chip bbox, top band, face extents, text box, height, rotation) - to refresh a marking (add a variant line), re-call this verb with the manifest's type + paramsCm (unitsCm:true) + the new etch text; generation is deterministic so placement reproduces exactly. The manifest is also returned inline as 'manifest'. PITFALLS: params are mm by default (unitsCm:true for EPG-native cm); dimension names follow each EPG generator (chip: D/E/A/L/L1; soic: A/A1/b/D/E/E1/e/L/DPins); raised markings are separate 'Marking' bodies (white in Fusion + colored STEP).",
        if etch.is_empty() { String::new() } else { format!(", marked '{}' on the chip top as {}", etch, if etch_style == "engraved" { "an engraved cut" } else { "RAISED WHITE text (silkscreen-style - high contrast on the dark body)" }) },
        if output_step.is_empty() { String::new() } else { format!(", exported STEP to {output_step}") }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn epg_script_matches_python() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../tests/golden/epg_script.json")).unwrap();
        for c in cases {
            match script_for(&c["args"], "TESTVER") {
                Ok(s) => { let want = c["script"].as_str().unwrap(); if s != want { let i = s.chars().zip(want.chars()).position(|(a, b)| a != b).unwrap_or(0); panic!("case {} differs at {}: ours {:?} vs python {:?}", c["args"]["type"], i, &s[i.saturating_sub(40)..(i + 80).min(s.len())], &want[i.saturating_sub(40)..(i + 80).min(want.len())]); } }
                Err(e) => assert_eq!(e["errorCode"], c["errorCode"], "case {}", c["args"]["type"]),
            }
        }
    }
    #[test]
    fn float_repr() {
        for (f, s) in [(0.3, "0.3"), (3.0, "3.0"), (1e-5 / 10.0, "1.0000000000000002e-06"), (1e16, "1e+16"), (0.0001, "0.0001"), (123.456, "123.456")] { assert_eq!(py_float_repr(f), s); }
    }
}

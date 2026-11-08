//! `fusion_generate_body` from the EDA Bridge Standard's shared writer (adom-eda-checks writers::generate_body,
//! standard #20): the package's body as STEP from its family and JEDEC dimensions, written here on the Fusion
//! machine. No Fusion needed. A family the engine does not make, or `generator: "fusion"`, goes to Fusion's own
//! package generator (fusion_generate_package) instead, which has about 50 families and etches markings as text.
use serde_json::{json, Value};

/// Does the shared writer answer this request? (false: Fusion's generator does)
pub fn engine_makes(args: &Value) -> bool {
    if args["generator"].as_str() == Some("fusion") { return false; }
    // no family: the engine's refusal names the families it makes
    if args["family"].as_str().map(str::trim).unwrap_or("").is_empty() { return true; }
    adom_eda_checks::run("generate_body", &json!({"args": {"family": args["family"]}}))["errorCode"] != "unknown_family"
}

pub fn generate(args: &Value) -> Value {
    let mut r = adom_eda_checks::run("generate_body", &json!({"args": args}));
    if r["success"].as_bool() != Some(true) { return r; }
    let text = r.as_object_mut().and_then(|o| o.remove("text")).and_then(|t| t.as_str().map(str::to_string)).unwrap_or_default();
    let path = match args["outputPath"].as_str() {
        Some(p) => p.replace('\\', "/"),
        None => std::env::temp_dir().join("adom-bodies").join(format!("{}.step", r["family"].as_str().unwrap_or("body"))).to_string_lossy().replace('\\', "/"),
    };
    if let Some(dir) = std::path::Path::new(&path).parent() { let _ = std::fs::create_dir_all(dir); }
    if let Err(e) = std::fs::write(&path, text.as_bytes()) {
        return json!({"success": false, "errorCode": "write_failed", "error": format!("{path}: {e}")});
    }
    let b = r["bodies"][0].clone();
    let span = |a: &str, z: &str| ((b[z].as_f64().unwrap_or(0.0) - b[a].as_f64().unwrap_or(0.0)) * 1000.0).round() / 1000.0;
    r["stepPath"] = json!(path);
    r["bboxMm"] = json!({"x": span("x0", "x1"), "y": span("y0", "y1"), "z": span("z0", "z1")});
    r["generator"] = json!("engine");
    r["mutated"] = json!(false);
    r["_hint"] = json!("Written by the EDA Bridge Standard's shared writer: the package's own frame, seated on z = 0. Compare bboxMm with the datasheet's body size before attaching it. generator:\"fusion\" uses Fusion's package generator instead (more families, real etched text).");
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn engine_families_stay_here_others_go_to_fusion() {
        assert!(engine_makes(&json!({"family": "soic"})));
        assert!(engine_makes(&json!({"family": "TQFP"})));
        assert!(!engine_makes(&json!({"family": "dpak"})));
        assert!(!engine_makes(&json!({"family": "soic", "generator": "fusion"})));
        assert!(engine_makes(&json!({})));
        assert_eq!(generate(&json!({}))["errorCode"], "unknown_family");
    }
    #[test]
    fn writes_the_step_and_reports_its_box() {
        let out = std::env::temp_dir().join("fusion-body-test").join("SOIC8.step");
        let r = generate(&json!({"family": "soic", "dimensionsMm": {"pins": 8, "D": 4.9, "E": 6.0, "E1": 3.9, "A": 1.75}, "outputPath": out.to_string_lossy()}));
        assert_eq!(r["success"], true, "{r}");
        assert!(r.get("text").is_none());
        let step = std::fs::read_to_string(&out).unwrap();
        assert!(step.starts_with("ISO-10303-21;"));
        assert_eq!(r["bboxMm"]["z"], 1.75);
        assert!(r["engine"].as_str().unwrap().starts_with("adom-eda-checks"));
    }
}

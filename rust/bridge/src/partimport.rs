//! `fusion_import_part`: the standard's import_part. The footprint and symbol specs (the same files
//! Altium's bridge takes) go into one .lbr offline (fusion_lbr::import), preflighted before it is
//! written; a STEP is then minted and bound to the component by bind_library_3d, dialog-free.

use serde_json::{json, Value};

use crate::addin::Ctx;

/// A spec given as a path on this machine, or as the spec text itself.
fn spec(args: &Value, key: &str) -> Result<Option<String>, Value> {
    let Some(v) = args[key].as_str().filter(|s| !s.trim().is_empty()) else { return Ok(None) };
    if v.contains('|') && v.contains('\n') { return Ok(Some(v.to_string())); }
    std::fs::read_to_string(v.replace('\\', "/")).map(Some).map_err(|e| json!({"success": false, "errorCode": "spec_unreadable", "error": format!("{key} {v}: {e}"), "_hint": "A spec file on the Fusion machine, or the spec text itself (META|... lines)."}))
}

pub fn import_part(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let s = |k: &str| args[k].as_str().filter(|v| !v.is_empty()).map(|v| v.replace('\\', "/"));
    // a Fusion library holds the footprint and the symbol together: one file
    let path = match (s("libraryPath"), s("pcbLib"), s("schLib")) {
        (Some(p), _, _) => p,
        (None, Some(a), Some(b)) if !a.eq_ignore_ascii_case(&b) => return json!({"success": false, "errorCode": "bad_args", "error": "pcbLib and schLib differ: a Fusion library (.lbr) holds the footprint and the symbol together, so pass one libraryPath"}),
        (None, Some(a), _) | (None, None, Some(a)) => a,
        _ => return json!({"success": false, "errorCode": "bad_args", "error": "libraryPath (a .lbr on the Fusion machine) is required"}),
    };
    let (fp, sym) = match (spec(args, "footprintSpec"), spec(args, "symbolSpec")) { (Ok(a), Ok(b)) => (a, b), (Err(e), _) | (_, Err(e)) => return e };
    if fp.is_none() && sym.is_none() && !args["params"].is_object() {
        return json!({"success": false, "errorCode": "nothing_to_import", "error": "import_part was given nothing to import: pass footprintSpec and/or symbolSpec, optionally step and params"});
    }
    let existing = std::fs::read_to_string(&path).ok();
    let (text, mut summary) = match fusion_lbr::import::import_part(existing.as_deref(), fp.as_deref(), sym.as_deref(), s("footprint").as_deref(), args) {
        Ok(x) => x, Err(e) => return json!({"success": false, "errorCode": "import_refused", "error": e, "libraryPath": path}),
    };
    let tmp = std::env::temp_dir().join(format!("adom-import-part-{}.lbr", std::process::id()));
    let _ = std::fs::write(&tmp, &text);
    let (ok, errors) = fusion_lbr::authoring::preflight_text(&tmp);
    let _ = std::fs::remove_file(&tmp);
    if !ok { return json!({"success": false, "errorCode": "lbr_preflight_failed", "error": "the library would not open cleanly in Fusion; nothing was written", "preflight": errors, "summary": summary}); }
    if args["dryRun"].as_bool() == Some(true) { return json!({"success": true, "mutated": false, "dryRun": true, "libraryPath": path, "footprint": summary["footprint"], "symbol": summary["symbol"], "steps": summary["steps"]}); }
    if let Some(d) = std::path::Path::new(&path).parent() { let _ = std::fs::create_dir_all(d); }
    let backup = existing.as_ref().map(|_| { let b = format!("{path}.bak"); let _ = std::fs::copy(&path, &b); b });
    if let Err(e) = std::fs::write(&path, &text) { return json!({"success": false, "errorCode": "write_failed", "error": format!("{path}: {e}")}); }
    let mut out = json!({"success": true, "mutated": true, "libraryPath": path, "backup": backup, "footprint": summary["footprint"], "symbol": summary["symbol"]});
    if let Some(step) = s("step") {
        let r = match summary["symbol"].as_str() {
            // the 3D package binds to a component's footprint: a footprint alone has none to bind to.
            // force: importing again replaces the body, the way the footprint and symbol are replaced
            None => json!({"success": false, "error": "no component in this import: the 3D package binds through a component (pass symbolSpec)"}),
            Some(ds) => crate::mint::bind_library_3d(ctx, &json!({"libraryPath": path, "stepPath": step, "deviceset": ds, "force": true}), pass),
        };
        let okb = r["success"].as_bool() == Some(true);
        summary["steps"].as_array_mut().unwrap().push(json!({"add_body": okb, "error": r["error"], "wipUrn": r["wip_urn"]}));
        if !okb { out["success"] = json!(false); out["errorCode"] = json!("add_body_failed"); out["failedAt"] = json!("add_body"); out["error"] = json!(format!("the library is written, but the 3D body did not bind: {}", r["error"].as_str().unwrap_or("?"))); }
    }
    out["steps"] = summary["steps"].clone();
    if out["success"] == true {
        out["_hint"] = json!("Written and preflighted. Importing again replaces the footprint and the component of these names. Place it with fusion_sch_place_part from a design holding the library, or see it with fusion_show_symbol / fusion_show_footprint.");
    }
    out
}

//! Saving an electronics design (page issue #125): `fusion_save_document` saves the open design in
//! place as a new version, and `fusion_save_to_cloud` on an electronics design saves the DESIGN (not
//! the board in front) and refuses a Save As of a design already saved. The logic is
//! savedoc_script.py, run through the add-in's script transport so it needs no add-in update. A
//! mechanical document goes to the add-in's own save.
use serde_json::{json, Value};

const SOURCE: &str = include_str!("savedoc_script.py");

fn run(action: &str, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let script = format!("import json\nexec({})\nresult=sd_handle(app,json.loads({}),{})\n",
        serde_json::to_string(SOURCE).unwrap(), serde_json::to_string(&args.to_string()).unwrap(), serde_json::to_string(action).unwrap());
    let r = pass("run_modeling_script", json!({"script": script}));
    match r["data"]["result"].as_object() {
        Some(_) => r["data"]["result"].clone(),
        None if r["success"].as_bool() == Some(false) => r,
        None => json!({"success": false, "errorCode": "invalid_save_response", "error": "Fusion did not return a structured save result", "mutationMayHaveOccurred": true,
                       "_hint": "Read the design's files (fusion_list_cloud_files) before saving again."}),
    }
}

/// `fusion_save_document {documentName?, comment?}`
pub fn save_document(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value { run("save_document", args, pass) }

/// `fusion_save_to_cloud {name, projectName?, folderPath?, description?, asCopy?, overwrite?}`
pub fn save_to_cloud(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let r = run("save_to_cloud", args, pass);
    if r["notElectronics"].as_bool() == Some(true) { pass("save_to_cloud", args.clone()) } else { r }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mechanical_documents_go_to_the_addins_own_save() {
        let seen = std::cell::RefCell::new(vec![]);
        let pass = |v: &str, _a: Value| { seen.borrow_mut().push(v.to_string()); if v == "run_modeling_script" { json!({"success": true, "data": {"result": {"notElectronics": true}}}) } else { json!({"success": true, "via": "addin"}) } };
        assert_eq!(save_to_cloud(&json!({"name": "x"}), &pass)["via"], "addin");
        assert_eq!(*seen.borrow(), vec!["run_modeling_script", "save_to_cloud"]);
    }
    #[test]
    fn electronics_answers_come_back_as_they_are() {
        let pass = |_v: &str, _a: Value| json!({"success": true, "data": {"result": {"success": false, "errorCode": "already_saved_use_save_document"}}});
        assert_eq!(save_to_cloud(&json!({"name": "x"}), &pass)["errorCode"], "already_saved_use_save_document");
        assert_eq!(save_document(&json!({}), &|_v: &str, _a: Value| json!({"success": false, "errorCode": "timeout"}))["errorCode"], "timeout");
    }
}

//! Routing operations execute together on Fusion's main thread through the existing
//! script transport. Embedding the implementation avoids replacing a loaded add-in.
use serde_json::{json, Value};
const SOURCE: &str = concat!(include_str!("routing_script.py"), "\n", include_str!("routing_planner.py"), "\n", include_str!("board_model.py"), "\n", include_str!("lbr_use.py"),
    "\nFUSION_DEFAULT_RULES = r'''", include_str!("fusion_default_designrules.xml"), "'''\n");
pub fn owns(cmd: &str) -> bool { matches!(cmd, "change_vias" | "edit_tracks" | "place_parts" | "routing_state" | "route_net" | "routing_drc" | "move_parts" | "place_vias" | "apply_pours" | "pour_state" | "board_source" | "ripup_nets" | "schematic_run" | "board_bom" | "change_pours" | "schematic_plan" | "board_seed" | "remove_pours" | "layer_stack" | "netlist" | "bodies" | "component_data" | "component_pins" | "list_nets" | "net_connections" | "list_designators" | "layers" | "copper_layers" | "layers_in_use" | "read_rules" | "rules_by_kind" | "silk_state" | "net_classes" | "diff_pairs" | "run_drc" | "repour" | "zoom_to" | "export_bom" | "save_document" | "save_to_cloud" | "apply_silkscreen" | "set_layer_visibility" | "select_net" | "set_track_width" | "remove_items" | "set_outline" | "board_shape" | "board_outline" | "run_erc" | "autoroute" | "update_board_from_eagle" | "board_dxf" | "edit_parts" | "align_parts" | "remove_graphics" | "remove_parts" | "check_inputs" | "export_odb" | "export_ipc356" | "export_gencad" | "export_ipc2581" | "import_svg" | "copy_routing" | "set_layer_count" | "board_info_compat" | "footprint_primitives" | "place_planned_vias" | "place_teardrops" | "board_features" | "add_rule" | "read_board") }
/// How many schematic commands one call into Fusion runs. Fusion's main thread is held for the whole
/// call, so a long list goes in batches, which lets it answer between them (#137).
const SCH_BATCH: usize = 300;

/// fusion_schematic_run with more commands than one batch: the whole list is checked first, so a bad
/// command still means nothing ran; then each batch in turn. The reply reads like one call's. A failure
/// stops there, with failedAt counted in the caller's list and what already ran in batchesDone.
fn schematic_run_batched(args: &Value, list: &[Value], run: &dyn Fn(&Value) -> Value) -> Value {
    let mut check = args.clone();
    check["validateOnly"] = json!(true);
    let checked = run(&check);
    if checked["success"].as_bool() != Some(true) { return checked; }
    let batches: Vec<&[Value]> = list.chunks(SCH_BATCH).collect();
    let (mut added, mut removed, mut ran, mut reformatted, mut assertion) = (Vec::new(), Vec::new(), 0u64, false, Value::Null);
    let mut last = Value::Null;
    for (i, chunk) in batches.iter().enumerate() {
        let mut a = args.clone();
        a["commands"] = json!(chunk);
        let mut r = run(&a);
        added.extend(r["added"].as_array().cloned().unwrap_or_default());
        removed.extend(r["library3dEntriesRemoved"].as_array().cloned().unwrap_or_default());
        reformatted = reformatted || r["libraryReformatted"].as_bool() == Some(true);
        if assertion.is_null() { assertion = r["fusionUnexpectedCondition"].clone(); }
        let this = r["commands"].as_u64().unwrap_or(0);
        if r["success"].as_bool() != Some(true) {
            if let Some(at) = r["failedAt"].as_u64() { r["failedAt"] = json!(at + (i * SCH_BATCH) as u64); }
            r["commands"] = json!(ran + this);
            r["commandsAsked"] = json!(list.len());
            r["batches"] = json!(batches.len());
            r["batchesDone"] = json!(i);
            if i > 0 { r["mutated"] = json!(true); }
            r["added"] = json!(added);
            return r;
        }
        ran += this;
        last = r;
    }
    last["commands"] = json!(ran);
    last["batches"] = json!(batches.len());
    last["added"] = json!(added);
    last["library3dEntriesRemoved"] = json!(removed);
    last["libraryReformatted"] = json!(reformatted);
    if !assertion.is_null() { last["fusionUnexpectedCondition"] = assertion; }
    last
}

/// How long the call itself waits for a long list. Adom Bridge gives a call to another machine 120 s,
/// after which the caller is told timed_out with Fusion still working, so this answers before that.
const SCH_FOREGROUND_SECS: u64 = 100;

fn sch_run_path(state_dir: &std::path::Path, id: &str) -> std::path::PathBuf {
    let safe: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    state_dir.join("schematic-runs").join(format!("{safe}.json"))
}

fn sch_run_write(state_dir: &std::path::Path, id: &str, v: &Value) {
    let p = sch_run_path(state_dir, id);
    if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() { let _ = std::fs::rename(&tmp, &p); }
}

fn sch_run_read(state_dir: &std::path::Path, id: &str) -> Option<Value> {
    std::fs::read_to_string(sch_run_path(state_dir, id)).ok().and_then(|t| serde_json::from_str(&t).ok())
}

/// What a poll answers: the run's reply once it has ended, or that it is still going.
fn sch_run_answer(state_dir: &std::path::Path, id: &str) -> Value {
    match sch_run_read(state_dir, id) {
        None => json!({"success": false, "errorCode": "unknown_run_id", "error": format!("No schematic run {id} on this machine")}),
        Some(rec) if rec["stage"] == "running" => json!({"success": true, "stillRunning": true, "runId": id, "commandsAsked": rec["commandsAsked"], "startedAt": rec["startedAt"],
            "_hint": "Fusion is still working through the list. Poll fusion_schematic_run {runId} again; don't send the list again."}),
        Some(mut rec) => { let mut r = rec["result"].take(); r["runId"] = json!(id); r["stillRunning"] = json!(false); r }
    }
}

/// The verb's front door. A list of up to one batch is one call. A longer one is checked, then runs on
/// the bridge's own thread: the call waits for it as long as it safely can and answers with the whole
/// reply, or else with a runId to poll, so the caller never reads a false timed_out (#137).
pub fn schematic_run(state_dir: &std::path::Path, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    if let Some(id) = args["runId"].as_str().filter(|_| args["commands"].is_null()) { return sch_run_answer(state_dir, id); }
    let long = args["commands"].as_array().map(|l| l.len()).filter(|n| *n > SCH_BATCH);
    let Some(asked) = long.filter(|_| args["_onThread"].is_null() && args["validateOnly"].is_null()) else { return call("schematic_run", args, pass) };
    let mut check = args.clone();
    check["validateOnly"] = json!(true);
    let checked = call("schematic_run", &check, pass);
    if checked["success"].as_bool() != Some(true) { return checked; }
    let id = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) as u64 ^ (std::process::id() as u64) << 40);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    sch_run_write(state_dir, &id, &json!({"stage": "running", "startedAt": now, "commandsAsked": asked}));
    let url = crate::launch::SELF_URL.lock().unwrap().clone();
    let mut a2 = args.clone();
    a2["_onThread"] = json!(true);
    let (sd, rid) = (state_dir.to_path_buf(), id.clone());
    std::thread::spawn(move || {
        let r = url.and_then(|u| ureq::post(&u).timeout(std::time::Duration::from_secs(3600)).send_json(json!({"command": "fusion_schematic_run", "args": a2})).ok())
            .and_then(|r| r.into_json::<Value>().ok())
            .unwrap_or(json!({"success": false, "errorCode": "run_lost", "error": "The bridge lost this schematic run before it answered: read the sheet (fusion_schematic_data) to see what landed before running it again"}));
        sch_run_write(&sd, &rid, &json!({"stage": "ended", "result": r}));
    });
    sch_wait_or_hand_over(state_dir, &id, asked, SCH_FOREGROUND_SECS)
}

fn sch_wait_or_hand_over(state_dir: &std::path::Path, id: &str, asked: usize, secs: u64) -> Value {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        if sch_run_read(state_dir, id).map(|r| r["stage"] != "running").unwrap_or(false) { return sch_run_answer(state_dir, id); }
        if std::time::Instant::now() >= until { break; }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    json!({"success": true, "mutated": true, "started": true, "stillRunning": true, "runId": id, "statusVerb": "fusion_schematic_run", "commandsAsked": asked,
        "_hint": format!("The list passed its checks and is running in Fusion. Poll fusion_schematic_run {{runId:\"{id}\"}} until stillRunning is false: that answer is the run's whole reply. Don't send the list again.")})
}

pub fn call(cmd: &str, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    if cmd == "schematic_run" && args["validateOnly"].is_null() {
        if let Some(list) = args["commands"].as_array().filter(|l| l.len() > SCH_BATCH) {
            return schematic_run_batched(args, list, &|a: &Value| call_one(cmd, a, pass));
        }
    }
    call_one(cmd, args, pass)
}

fn call_one(cmd: &str, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut args_v = args.clone();
    if args_v.is_object() { args_v["_bridgeVersion"] = json!(env!("CARGO_PKG_VERSION")); }
    let script = format!("import json\nexec({})\nresult=handle(app,json.loads({}),{})\n",
        serde_json::to_string(SOURCE).unwrap(),
        serde_json::to_string(&args_v.to_string()).unwrap(),
        serde_json::to_string(cmd).unwrap());
    let r = pass("run_modeling_script", json!({"script":script,"expectDocument":args["expectDocument"]}));
    if r["success"].as_bool() == Some(false) { return r; }
    match r["data"]["result"].as_object() {
        Some(_) => {
            // the envelope's findings ride along: Fusion's own assertion (dismissed) and the
            // document hygiene state, neither of which the routing script can see
            let mut out = r["data"]["result"].clone();
            for k in ["fusionUnexpectedCondition", "documentHygiene", "_hintDocumentHygiene"] { if !r[k].is_null() { out[k] = r[k].clone(); } }
            if !r["fusionUnexpectedCondition"].is_null() { out["_hint"] = json!(format!("{} Fusion Electronics raised its 'unexpected condition' assertion after this edit and the bridge clicked OK; the copper read-back above is the truth. Checkpoint now (fusion_export_eagle_source).", out["_hint"].as_str().unwrap_or("")).trim().to_string()); }
            // the board from a file is saved only when asked and only after an exact read back:
            // a new version of the same design, never a new design (page issue #125, item 4)
            if cmd == "update_board_from_eagle" && out["mutated"].as_bool() == Some(true) {
                out["saved"] = json!(false);
                if args["save"].as_bool() == Some(true) && out["verified"].as_bool() == Some(true) {
                    let sv = crate::savedoc::save_document(&json!({"documentName": out["documentName"], "comment": args["comment"].as_str().unwrap_or("fusion-bridge: board updated from an EAGLE file")}), pass);
                    out["saved"] = json!(sv["success"].as_bool() == Some(true) && sv["saved"].as_bool() != Some(false));
                    out["save"] = sv;
                }
            }
            out
        }
        None => json!({"success":false,"errorCode":"invalid_routing_response","error":"Fusion did not return a structured routing result", "mutationMayHaveOccurred":matches!(cmd, "change_vias" | "route_net" | "move_parts" | "place_vias" | "apply_pours" | "apply_silkscreen" | "set_track_width" | "remove_items" | "set_outline" | "board_shape" | "autoroute" | "update_board_from_eagle" | "edit_parts" | "align_parts" | "remove_graphics" | "remove_parts" | "import_svg" | "copy_routing" | "set_layer_count" | "place_planned_vias" | "place_teardrops" | "board_features" | "add_rule")}),
    }
}

/// `fusion_export_bom {outputPath?, grouped?}`: the board export carries every part's attributes, so the
/// BOM is written from it. The caller names no document: the active one is read first, and when a
/// schematic or 3D view is in front the design is switched to its board.
pub fn export_bom(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let probe = "result={'name':app.activeDocument.name if app.activeDocument else None,'product':app.activeProduct.productType if app.activeProduct else None}\n";
    let mut active = pass("run_modeling_script", json!({"script": probe}))["data"]["result"].clone();
    if active["product"] != "BoardProductType" {
        let _ = pass("show_2d_board", json!({}));
        active = pass("run_modeling_script", json!({"script": probe}))["data"]["result"].clone();
    }
    if active["product"] != "BoardProductType" {
        return json!({"success": false, "errorCode": "no_board", "error": format!("no board is open (the active document is {} {})", active["name"], active["product"]),
                      "_hint": "Open the electronics design (fusion_schematic {action:'open'} or fusion_open_cloud_file), then call fusion_export_bom again; it switches to the board itself."});
    }
    let mut a = args.clone();
    a["expectDocument"] = active["name"].clone();
    call("export_bom", &a, pass)
}

#[cfg(test)] mod tests {
    use super::*;
    fn commands(n: usize) -> Vec<Value> { (0..n).map(|i| json!(format!("c{i}"))).collect() }
    fn run_dir(tag: &str) -> std::path::PathBuf { let d = std::env::temp_dir().join(format!("adom-sch-run-{tag}-{}", std::process::id())); let _ = std::fs::remove_dir_all(&d); d }
    #[test] fn a_run_still_going_hands_over_a_run_id_and_a_poll_answers() {
        let d = run_dir("hand");
        sch_run_write(&d, "abc", &json!({"stage": "running", "startedAt": 1.0, "commandsAsked": 560}));
        let r = sch_wait_or_hand_over(&d, "abc", 560, 0);
        assert_eq!(r["stillRunning"], true);
        assert_eq!(r["runId"], "abc");
        assert_eq!(r["success"], true);
        let noop = |_: &str, _: Value| panic!("a poll must not touch Fusion");
        let p = schematic_run(&d, &json!({"runId": "abc"}), &noop);
        assert_eq!(p["stillRunning"], true);
        assert_eq!(p["commandsAsked"], 560);
        sch_run_write(&d, "abc", &json!({"stage": "ended", "result": {"success": true, "commands": 560, "batches": 2}}));
        let p = schematic_run(&d, &json!({"runId": "abc"}), &noop);
        assert_eq!(p["stillRunning"], false);
        assert_eq!(p["commands"], 560);
        let p = schematic_run(&d, &json!({"runId": "nope"}), &noop);
        assert_eq!(p["errorCode"], "unknown_run_id");
    }
    #[test] fn a_run_that_ended_in_time_answers_as_one_call() {
        let d = run_dir("done");
        sch_run_write(&d, "abc", &json!({"stage": "ended", "result": {"success": false, "errorCode": "add_failed", "failedAt": 305}}));
        let r = sch_wait_or_hand_over(&d, "abc", 560, 5);
        assert_eq!(r["errorCode"], "add_failed");
        assert_eq!(r["failedAt"], 305);
    }
    #[test] fn a_bad_long_list_is_refused_in_the_call_and_starts_nothing() {
        let d = run_dir("bad");
        let list = commands(560);
        let pass = |_: &str, _: Value| json!({"success": true, "data": {"result": {"success": false, "errorCode": "off_the_sheet"}}});
        let r = schematic_run(&d, &json!({"commands": list}), &pass);
        assert_eq!(r["errorCode"], "off_the_sheet");
        assert!(!d.join("schematic-runs").exists());
    }
    #[test] fn a_long_schematic_list_runs_in_batches_after_one_check() {
        let list = commands(700);
        let seen = std::cell::RefCell::new(Vec::new());
        let r = schematic_run_batched(&json!({"libraryPath": "l.lbr", "commands": list}), &list, &|a: &Value| {
            seen.borrow_mut().push((a["validateOnly"].as_bool() == Some(true), a["commands"].as_array().unwrap().len()));
            if a["validateOnly"].as_bool() == Some(true) { return json!({"success": true, "commands": 700}); }
            let n = a["commands"].as_array().unwrap().len();
            json!({"success": true, "commands": n, "added": [format!("P{n}")], "parts": ["all"], "libraryReformatted": n == 300 && seen.borrow().len() == 2})
        });
        assert_eq!(*seen.borrow(), vec![(true, 700), (false, 300), (false, 300), (false, 100)]);
        assert_eq!(r["success"], true);
        assert_eq!(r["commands"], 700);
        assert_eq!(r["batches"], 3);
        assert_eq!(r["added"], json!(["P300", "P300", "P100"]));
        assert_eq!(r["libraryReformatted"], true);
    }
    #[test] fn a_bad_command_anywhere_means_no_batch_ran() {
        let list = commands(700);
        let runs = std::cell::Cell::new(0);
        let r = schematic_run_batched(&json!({"commands": list}), &list, &|_: &Value| { runs.set(runs.get() + 1); json!({"success": false, "errorCode": "off_the_sheet"}) });
        assert_eq!(r["errorCode"], "off_the_sheet");
        assert_eq!(runs.get(), 1);
    }
    #[test] fn a_failure_in_a_later_batch_counts_from_the_callers_list() {
        let list = commands(700);
        let n = std::cell::Cell::new(0);
        let r = schematic_run_batched(&json!({"commands": list}), &list, &|a: &Value| {
            if a["validateOnly"].as_bool() == Some(true) { return json!({"success": true}); }
            n.set(n.get() + 1);
            if n.get() == 2 { json!({"success": false, "errorCode": "add_failed", "failedAt": 5, "commands": 6}) }
            else { json!({"success": true, "commands": 300, "added": ["R1"]}) }
        });
        assert_eq!(r["errorCode"], "add_failed");
        assert_eq!(r["failedAt"], 305);
        assert_eq!(r["commands"], 306);
        assert_eq!(r["batchesDone"], 1);
        assert_eq!(r["commandsAsked"], 700);
        assert_eq!(r["mutated"], true);
        assert_eq!(r["added"], json!(["R1"]));
        assert_eq!(n.get(), 2);
    }
    #[test] fn unwraps_semantic_failure() {
        let r=call("route_net",&json!({}),&|_,_|json!({"success":true,"data":{"result":{"success":false,"errorCode":"stale_board"}}}));
        assert_eq!(r["errorCode"],"stale_board");
    }
    #[test] fn transport_failure_is_preserved() {
        let r=call("route_net",&json!({}),&|_,_|json!({"success":false,"errorCode":"timeout"}));
        assert_eq!(r["errorCode"],"timeout");
    }
}

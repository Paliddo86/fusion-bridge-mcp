//! Standard verbs that belong to Altium's own machinery (its cloud, its script agent, its .NET
//! extension, integrated libraries, OutJobs, display modes). Fusion answers each with not_supported
//! and what Fusion does instead, through one verb, fusion_not_supported, so the catalog stays the
//! verbs that do work.

use serde_json::{json, Value};

const A365: &str = "Altium 365 is Altium's cloud; Fusion's cloud is the Autodesk hub (fusion_aps_* search and browse, fusion_open_cloud_file, fusion_save_to_cloud)";

pub const TABLE: &[(&str, &str)] = &[
    ("a365_configure", A365), ("a365_files_upload", A365), ("a365_library_overview", A365), ("a365_library_schema", A365),
    ("a365_logout", A365), ("a365_oauth_begin", A365), ("a365_oauth_cancel", A365), ("a365_oauth_status", A365),
    ("a365_query", A365), ("a365_refresh", A365), ("a365_status", A365),
    ("workspace_search", "the Altium 365 workspace library; Fusion's shared libraries are library.io (fusion_managed_library_*) and hub files (fusion_aps_search)"),
    ("workspace_place", "the Altium 365 workspace library; place a part from a library the design holds with fusion_sch_place_part"),
    ("compile_intlib", "a Fusion library is one .lbr holding symbols, footprints and components together: there is nothing to compile"),
    ("extract_intlib", "a Fusion library is one .lbr holding symbols, footprints and components together: there is nothing to extract"),
    ("outjob_info", "Fusion has no OutJob: each output is its own verb (fusion_export_gerbers with the JLCPCB CAM job, fusion_export_bom, fusion_export_pdf, fusion_export_odb ...)"),
    ("run_outjob", "Fusion has no OutJob: each output is its own verb (fusion_export_gerbers with the JLCPCB CAM job, fusion_export_bom, fusion_export_pdf, fusion_export_odb ...)"),
    ("sch_no_connect", "Fusion has no no-connect marker: its text commands have none (checked live on Fusion 2706), and an EAGLE pin is marked unused by its direction in the library (nc). Leave the pin open and approve its ERC message (fusion_run_erc lists it)"),
    ("agent_start", "Fusion has no script agent to start: the bridge's add-in loads with Fusion (fusion_start), and fusion_restart_for_addin reloads it"),
    ("agent_stop", "Fusion has no script agent: the add-in lives as long as Fusion does"),
    ("agent_recover", "Fusion has no script engine to halt: a blocking dialog is cleared with fusion_dismiss_blocking_dialogs, and fusion_addin_status says whether the add-in answers"),
    ("install_extension", "the bridge deploys its Fusion add-in itself on every start"),
    ("install_receipt", "no installer receipt: fusion_install_library_bundle answers with what it installed"),
    ("connect_workspace", "Altium's workspace picker; Fusion works in the signed-in Autodesk account's hub (fusion_account)"),
    ("selftest_script_error", "a test of Altium's DelphiScript agent; Fusion runs none"),
    ("symbol_mode", "an EAGLE symbol has one look: there are no display modes to show"),
    ("selection", "Fusion does not show the user's board selection to the bridge, checked live on Fusion 2706 (2026-10-30) three ways: a ULP's ingroup() reads no part even right after a GROUP, ui.activeSelections reads 0 right after a SELECT, and the electron Board's selectionSets raises InternalValidationError. Name the parts instead; fusion_electron_select shows one to the user"),
    ("uninstall_library", "a library Fusion offers is a file in the Autodesk hub, so taking it out means deleting that file for everyone in the hub, which the standard's uninstall never does (it leaves files alone); a hub file the AI made itself is deleted with fusion_cleanup_cloud_files"),
    ("normalize_bodies", "a Fusion footprint's 3D model is a cloud 3D package placed when it is made (fusion_make_3d_package and fusion_bind_library_3d orient it on the pads; fusion_show_3d_package places one by hand): a library holds no body offsets to move"),
    ("sch_sheet_symbol", "Fusion's hierarchy is modules, and a module cannot be made from a text command (EDIT 'name.m' answers nothing and the design gets no module, checked live on Fusion 2706); module instances are placed in Fusion's module dialog. fusion_sch_hierarchy reads an existing hierarchy and fusion_sch_sheet_pins its ports"),
    ("component_classes", "an EAGLE board has net classes (fusion_net_classes, fusion_add_rule) but no component classes or rooms: group a channel by its designators (fusion_replicate_layout takes them as lists)"),
];

pub fn note(name: &str) -> Option<&'static str> { TABLE.iter().find(|(n, _)| *n == name).map(|(_, v)| *v) }

/// `fusion_not_supported {verb?}`: the refusal for one of these verbs, or the list.
pub fn answer(args: &Value) -> Value {
    let verb = args["verb"].as_str().unwrap_or("").trim_start_matches("fusion_").to_string();
    match note(&verb) {
        Some(n) => json!({"success": false, "errorCode": "not_supported", "verb": verb, "error": format!("{verb} is not supported in Fusion: {n}"), "_hint": n}),
        None if verb.is_empty() => json!({"success": true, "mutated": false, "verbs": TABLE.iter().map(|(n, v)| json!({"verb": n, "why": v})).collect::<Vec<_>>(),
            "_hint": "Standard verbs that belong to Altium's own machinery. Calling any of them (fusion_<verb> or the bare name) answers not_supported with what Fusion does instead."}),
        None => json!({"success": false, "errorCode": "bad_args", "error": format!("{verb} is not one of the verbs Fusion answers not_supported; call fusion_not_supported with no verb for the list")}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_altium_only_verb_is_refused_with_what_fusion_does_instead() {
        let r = answer(&json!({"verb": "fusion_run_outjob"}));
        assert_eq!((r["success"].as_bool(), r["errorCode"].as_str()), (Some(false), Some("not_supported")));
        assert!(r["error"].as_str().unwrap().contains("fusion_export_gerbers"));
        assert_eq!(answer(&json!({}))["verbs"].as_array().unwrap().len(), TABLE.len());
        assert_eq!(answer(&json!({"verb": "route_net"}))["errorCode"], "bad_args");
    }
}

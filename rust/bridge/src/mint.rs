//! The 3D mint and the multi-part bind, native. `make_3d_package` runs the same Fusion-side
//! script server.py builds (the template is derived from a Python-generated golden and a test
//! pins it byte for byte), through the add-in pass-through; the cloud folder discovery and the
//! APS upload fallback use the exe's own APS client. `build_library_3d` loops the mint per part,
//! injects every binding in one pass (the lbr crate) and opens the bound library once.

use serde_json::{json, Value};
use std::time::{Duration, Instant};

use crate::addin::Ctx;

const TEMPLATE: &str = include_str!("make3d_template.py");
const ORIENT_BLOCK: &str = include_str!("make3d_orient_block.py");
/// Explicit source placement (issue #83): replaces every heuristic (orient, centering, pin-1).
const PLACEMENT_BLOCK: &str = include_str!("make3d_placement_block.py");

/// An authored model placement: translation in mm, rotation in degrees about X, then Y, then Z.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement { pub translation_mm: [f64; 3], pub rotation_deg: [f64; 3] }

/// `placement` argument -> Placement, or a refusal. Absent/null is Ok(None). Scale other than
/// 1, a non-finite number or a wrong shape is rejected before Fusion is touched.
pub fn parse_placement(v: &Value) -> Result<Option<Placement>, Value> {
    if v.is_null() { return Ok(None); }
    let Some(o) = v.as_object() else { return Err(json!({"success": false, "errorCode": "invalid_placement", "error": "placement must be an object {translation:[x,y,z] mm, rotation:[rx,ry,rz] deg, scale:1}"})); };
    let triple = |k: &str| -> Result<[f64; 3], Value> {
        match o.get(k) {
            None | Some(Value::Null) => Ok([0.0, 0.0, 0.0]),
            Some(Value::Array(a)) if a.len() == 3 => {
                let mut out = [0.0; 3];
                for (i, x) in a.iter().enumerate() {
                    let f = x.as_f64().filter(|f| f.is_finite()).ok_or_else(|| json!({"success": false, "errorCode": "invalid_placement", "error": format!("placement.{k}[{i}] must be a finite number")}))?;
                    out[i] = f;
                }
                Ok(out)
            }
            _ => Err(json!({"success": false, "errorCode": "invalid_placement", "error": format!("placement.{k} must be an array of three finite numbers")})),
        }
    };
    let t = triple("translation")?; let r = triple("rotation")?;
    match o.get("scale") {
        None | Some(Value::Null) => {}
        Some(s) => {
            let ok = match s { Value::Number(n) => n.as_f64() == Some(1.0), Value::Array(a) => a.len() == 3 && a.iter().all(|x| x.as_f64() == Some(1.0)), _ => false };
            if !ok { return Err(json!({"success": false, "errorCode": "unsupported_scale", "error": "placement.scale must be 1: the STEP is imported at its authored size and never rescaled (a scaled model would no longer match the manufacturer's geometry)"})); }
        }
    }
    if let Some(u) = o.get("units").and_then(|u| u.as_str()) { if u != "mm" { return Err(json!({"success": false, "errorCode": "invalid_placement", "error": format!("placement.units must be mm (got {u})")})); } }
    if let Some(u) = o.get("rotationUnits").and_then(|u| u.as_str()) { if u != "deg" && u != "degrees" { return Err(json!({"success": false, "errorCode": "invalid_placement", "error": format!("placement.rotationUnits must be deg (got {u})")})); } }
    Ok(Some(Placement { translation_mm: t, rotation_deg: r }))
}

/// sha256 + size of the source model, so the reply carries provenance of the bytes Fusion read.
pub fn source_provenance(path: &str) -> Value {
    use sha2::Digest;
    match std::fs::read(path) {
        Ok(b) => json!({"path": path, "bytes": b.len(), "sha256": format!("{:x}", sha2::Sha256::digest(&b)), "preserved": true, "note": "the STEP is imported as-is; the bridge never rewrites it"}),
        Err(e) => json!({"path": path, "error": e.to_string()}),
    }
}
pub const DEFAULT_UPLOAD_PROJECT: &str = "a.YnVzaW5lc3M6YWRvbTMjMjAyMzExMjk3MDM5NjAzMzE";
pub const KNOWN_ROOT_FOLDER: &str = "urn:adsk.wipprod:fs.folder:co.jyO4vxQXR6S9zFpTQWnZAg";
pub const AI_WORKSPACE_NAME: &str = "Adom AI Workspace";

fn now_f() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

/// Python's float repr for the values this script carries (shortest round trip, ".0" on integers).
fn py_float(v: f64) -> String { let s = format!("{v}"); if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") { s } else { format!("{s}.0") } }

/// The exact script `_orchestrate_make_3d_package` sends to the add-in.
pub fn make_3d_script(model_path: &str, orient: bool, pin1: Option<(f64, f64)>, layers_off: &[String], task: &str, name: &str) -> String {
    make_3d_script_placed(model_path, orient, pin1, layers_off, task, name, None)
}

/// The mint script with the placement policy applied: an explicit placement replaces every
/// heuristic (orient block, pin-1 rotation); orient:false means NO reorientation at all (the
/// pin-1 rotation used to survive it, issue #83); the default keeps both heuristics.
pub fn make_3d_script_placed(model_path: &str, orient: bool, pin1: Option<(f64, f64)>, layers_off: &[String], task: &str, name: &str, placement: Option<&Placement>) -> String {
    let (block, pin1, skip): (String, Option<(f64, f64)>, &str) = match placement {
        Some(pl) => (PLACEMENT_BLOCK.replace("{TX}", &py_float(pl.translation_mm[0])).replace("{TY}", &py_float(pl.translation_mm[1])).replace("{TZ}", &py_float(pl.translation_mm[2]))
                        .replace("{RX}", &py_float(pl.rotation_deg[0])).replace("{RY}", &py_float(pl.rotation_deg[1])).replace("{RZ}", &py_float(pl.rotation_deg[2])), None, "'skipped_explicit_placement'"),
        None if !orient => (String::new(), None, "'skipped_orient_false'"),
        None => (ORIENT_BLOCK.to_string(), pin1, "None"),
    };
    make_3d_script_raw(model_path, &block, pin1, skip, layers_off, task, name)
}

fn make_3d_script_raw(model_path: &str, orient_block: &str, pin1: Option<(f64, f64)>, pin1_skip: &str, layers_off: &[String], task: &str, name: &str) -> String {
    let mut layers: Vec<String> = layers_off.iter().cloned().collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    layers.sort();
    let layers_repr = format!("[{}]", layers.iter().map(|l| fusion_lbr::py_repr_str(l)).collect::<Vec<_>>().join(", "));
    let (pin1_repr, pad1_repr) = match pin1 { Some((x, y)) => (format!("({}, {})", py_float(x / 10.0), py_float(y / 10.0)), format!("[{},{}]", py_float(x), py_float(y))), None => ("None".to_string(), "None".to_string()) };
    TEMPLATE.replace("{MODEL_PATH}", model_path).replace("{DEFAULT_PROJECT}", DEFAULT_UPLOAD_PROJECT).replace("{TASK}", task).replace("{NAME}", name)
        .replace("{PIN1}", &pin1_repr).replace("{PAD1}", &pad1_repr).replace("{LAYERS_OFF}", &layers_repr).replace("{ORIENT_BLOCK}", orient_block).replace("{PIN1_SKIP}", pin1_skip)
}

/// Point a rendered mint script at a named project and folder (page issue #126): the template
/// saves into Fusion's ACTIVE project (else "Main") under "Adom AI Workspace/<task>", which put a
/// run's models in an unrelated project. With projectName the project is found by name in the hub
/// (never the active one; a missing name is refused, not substituted) and the .f3d goes to
/// <project>/<folderPath or "Adom AI Workspace">/<task>, folders created as needed. The template
/// itself is unchanged, so a call without projectName renders exactly as before.
pub fn with_destination(script: &str, project: &str, folder_path: &str, task: &str) -> Option<String> {
    let start = script.find("proj=None\n")?;
    let marker = format!("wsf=_sub(_sub(rf,'Adom AI Workspace'),'{task}')\n");
    let end = script.find(&marker)? + marker.len();
    let lit = |s: &str| serde_json::to_string(s).unwrap();
    let block = format!(r#"proj=None
_PN={pn}; _FP={fp}; _TK={tk}
try:
 hubs=app.data.dataHubs
 for hi in range(hubs.count):
  ps=hubs.item(hi).dataProjects
  for pj2 in range(ps.count):
   p2=ps.item(pj2)
   if str(p2.name).lower()==_PN.lower(): proj=p2; break
  if proj: break
except Exception as e: res['projErr']=str(e)[:60]
if not proj: res['errorCode']='project_not_found'; res['projectName']=_PN
try:
 if proj is None: raise RuntimeError('no project named %s in this hub' % _PN)
 rf=proj.rootFolder
 wsf=rf
 for _part in ([x for x in _FP.replace('\\','/').split('/') if x.strip()] or ['Adom AI Workspace']): wsf=_sub(wsf,_part)
 wsf=_sub(wsf,_TK)
 res['savedTo']={{'project':proj.name,'folder':'/'.join(([x for x in _FP.replace('\\','/').split('/') if x.strip()] or ['Adom AI Workspace'])+[_TK])}}
"#, pn = lit(project), fp = lit(folder_path), tk = lit(task));
    Some(format!("{}{}{}", &script[..start], block, &script[end..]))
}

/// `_pad1_xy_mm`: (x, y) of pad or smd "1" in the FIRST package of the .lbr.
pub fn pad1_xy_mm(lbr_path: &str) -> Option<(f64, f64)> {
    let t = std::fs::read_to_string(lbr_path).ok()?;
    let body = regex::Regex::new(r#"(?s)<package name="[^"]*">(.*?)</package>"#).unwrap().captures(&t).map(|c| c[1].to_string()).unwrap_or(t.clone());
    let c = regex::Regex::new(r#"<(?:smd|pad) name="1" x="([-0-9.]+)" y="([-0-9.]+)""#).unwrap().captures(&body)?;
    Some((c[1].parse().ok()?, c[2].parse().ok()?))
}

fn safe_folder_name(n: &str) -> String { let s: String = regex::Regex::new(r#"[<>:"/\\|?*]"#).unwrap().replace_all(n, "").trim().chars().take(60).collect(); if s.is_empty() { "task".into() } else { s } }

fn find_child_folder(project_id: &str, parent_id: &str, name: &str) -> Option<String> {
    let r = crate::native_aps_browse(&json!({"projectId": project_id, "folderId": parent_id}));
    r["data"]["items"].as_array()?.iter().find(|it| it["type"].as_str() == Some("folders") && it["name"].as_str().map(|n| n.trim() == name).unwrap_or(false)).and_then(|it| it["id"].as_str().map(str::to_string))
}

/// `_ensure_workspace_folder`: a folder inside "Adom AI Workspace", never a project root.
fn ensure_workspace_folder(project_id: &str, task: Option<&str>) -> Option<String> {
    let task = task.map(safe_folder_name);
    let ws = find_child_folder(project_id, KNOWN_ROOT_FOLDER, AI_WORKSPACE_NAME).or_else(|| { let cr = crate::native_aps_create_folder(&json!({"projectId": project_id, "parentFolderId": KNOWN_ROOT_FOLDER, "name": AI_WORKSPACE_NAME})); if cr["success"].as_bool() == Some(true) { cr["data"]["folderId"].as_str().map(str::to_string) } else { None } });
    let mut folder = ws.clone();
    if let (Some(t), Some(w)) = (task, ws) {
        let sub = find_child_folder(project_id, &w, &t).or_else(|| { let cr = crate::native_aps_create_folder(&json!({"projectId": project_id, "parentFolderId": w, "name": t})); if cr["success"].as_bool() == Some(true) { cr["data"]["folderId"].as_str().map(str::to_string) } else { None } });
        folder = sub.or(Some(w));
    }
    folder
}

/// `_discover_upload_target`: (projectId, folderId) - always an AI-owned non-root folder.
pub fn discover_upload_target_pub(args: &Value) -> (String, Option<String>) { discover_upload_target(args) }
fn discover_upload_target(args: &Value) -> (String, Option<String>) {
    let project_id = args["projectId"].as_str().filter(|s| !s.is_empty()).unwrap_or(DEFAULT_UPLOAD_PROJECT).to_string();
    let mut fid = args["folderId"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    if fid.as_deref() == Some(KNOWN_ROOT_FOLDER) { fid = None; }
    if fid.is_none() { fid = ensure_workspace_folder(&project_id, args["task"].as_str()); }
    (project_id, fid)
}

fn step_story_from_file(model_path: &str) -> String {
    let Ok(bytes) = std::fs::read(model_path) else { return String::new() };
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4 * 1024 * 1024)]);
    let words = [("Adom.HeatsinkVias", "the heatsink via barrels under the exposed pad"), ("Adom.SolderBalls", "solder-ball hemispheres on every pad"), ("Adom.LaserEtch", "the laser-etched part number"), ("Adom.Board", "board stock"), ("Adom.Pads", "rendered pads")];
    let got: Vec<&str> = words.iter().filter(|(g, _)| head.contains(g)).map(|(_, w)| *w).collect();
    if got.is_empty() { "chip body only (the baseline model)".into() } else { format!("unlike the chip-only model this STEP carries {}, kept visible", got.join(" and ")) }
}

pub fn step_layer_story(bodies: &Value) -> String {
    let mut groups: std::collections::BTreeMap<String, usize> = Default::default();
    for b in bodies.as_array().cloned().unwrap_or_default() { let g = b.as_str().unwrap_or("").split('/').next().unwrap_or("").to_string(); if g.starts_with("Adom.") && g != "Adom.Chip" { *groups.entry(g).or_insert(0) += 1; } }
    if groups.is_empty() { return "chip body only (the baseline model)".into(); }
    let words = |g: &str| match g { "Adom.HeatsinkVias" => "heatsink via barrels under the exposed pad".to_string(), "Adom.SolderBalls" => "solder-ball hemispheres sitting on the pads".into(), "Adom.LaserEtch" => "the laser-etched part number".into(), "Adom.Board" => "board stock".into(), "Adom.Pads" => "rendered pads".into(), o => o[5..].to_string() };
    format!("unlike the chip-only model, this one carries {}, all left visible", groups.iter().map(|(g, n)| format!("{n} {}", words(g))).collect::<Vec<_>>().join(" and "))
}

/// `fusion_make_3d_package`: a RENDERING footprint+chip 3D package, no GUI dialogs.
pub fn make_3d_package(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let lbr_path = args["lbrPath"].as_str().or(args["filePath"].as_str()).unwrap_or("").replace('\\', "/");
    let model_path = args["modelPath"].as_str().unwrap_or("").replace('\\', "/");
    let (project_id, folder_id) = discover_upload_target(args);
    if lbr_path.is_empty() || model_path.is_empty() { return json!({"success": false, "error": "lbrPath (.lbr with the footprint) and modelPath (.step) are required (Windows paths)."}); }
    let cap_label = args["captureLabel"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    let task = safe_folder_name(args["task"].as_str().unwrap_or("AI 3D packages"));
    let orient = args["orient"].as_bool().unwrap_or(true);
    let placement = match parse_placement(&args["placement"]) { Ok(p) => p, Err(e) => return e };
    let provenance = source_provenance(&model_path);
    let mut warnings: Vec<String> = Vec::new();
    if placement.is_none() && orient { warnings.push("No placement given: the chip's orientation comes from the bounding-box heuristic and the pin-1 mark heuristic (a circular feature near pad 1 can rotate it in 90-degree steps). Pass placement:{translation,rotation} from the manufacturer's authored transform to bypass both.".into()); }
    if placement.is_none() && !orient { warnings.push("orient:false: the model is imported exactly as authored, no reorientation and no pin-1 rotation (since 1.10.8).".into()); }
    let base = model_path.rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_default();
    let name = args["fileName"].as_str().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| format!("{base}_3d")).replace('\'', "").replace(".f3d", "");
    crate::chains::note_bridge_doc_pub(&ctx.state_dir, &name);
    let f3d_name = format!("{name}.f3d");
    let op = crate::orchestrations::open_lbr(ctx, &json!({"filePath": lbr_path}), pass);
    if op["success"].as_bool() != Some(true) { return json!({"success": false, "error": format!("open_lbr failed: {}", op["error"].as_str().unwrap_or("None"))}); }
    std::thread::sleep(Duration::from_secs(1));
    let cp = crate::orchestrations::create_3d_package_pub(ctx, &lbr_path, pass);
    if cp["success"].as_bool() != Some(true) { return json!({"success": false, "error": format!("Create3DPackage failed: {}", cp["error"].as_str().unwrap_or("None")), "_hint": "The .lbr must be adom-lbr-generated; its FIRST package's footprint is loaded into the generator."}); }
    std::thread::sleep(Duration::from_secs(2));
    let expected_pkg = std::fs::read_to_string(&lbr_path).ok().and_then(|x| fusion_lbr::artifact_names(&x).ok()).and_then(|n| n.pac.first().cloned()).unwrap_or_default();
    let di = pass("document_info", json!({}));
    let gen_doc = match &di["data"]["activeDocument"] { Value::Object(o) => o.get("name").and_then(|v| v.as_str()).unwrap_or(""), Value::String(s) => s.as_str(), _ => "" }.trim_end_matches('*').trim().to_string();
    if !expected_pkg.is_empty() && !gen_doc.is_empty() && !crate::orchestrations::doc_matches(&gen_doc, &expected_pkg) && !gen_doc.to_lowercase().starts_with("untitled") {
        return json!({"success": false, "errorCode": "wrong_footprint_loaded", "error": format!("The Package3D generator loaded footprint {} but this mint is for package {}; the chip would have landed on the wrong footprint.", fusion_lbr::py_repr_str(&gen_doc), fusion_lbr::py_repr_str(&expected_pkg)),
            "_hint": "Mint each part on a library whose FIRST package is that part's footprint (fusion_install_library_bundle splits a merged library per part for this reason)."});
    }
    let mut before_shot = Value::Null;
    if let Some(l) = &cap_label { let _ = pass("run_modeling_script", json!({"script": "app.activeViewport.fit()\nresult={}"})); std::thread::sleep(Duration::from_millis(400)); before_shot = json!(crate::orchestrations::capture_labeled(ctx, &format!("{l}_before"))); }
    let layers: Vec<String> = args["layersOff"].as_array().map(|a| a.iter().map(|v| match v { Value::String(s) => s.clone(), o => o.to_string() }).collect()).unwrap_or_default();
    let mut script = make_3d_script_placed(&model_path, orient, pad1_xy_mm(&lbr_path), &layers, &task, &name, placement.as_ref());
    let dest_project = args["projectName"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    if let Some(pn) = &dest_project {
        match with_destination(&script, pn, args["folderPath"].as_str().unwrap_or(""), &task) {
            Some(s2) => script = s2,
            None => return json!({"success": false, "errorCode": "destination_unsupported", "error": "this mint script has no destination block to point at projectName (a bridge defect: the template changed)"}),
        }
    }
    let r = pass("run_modeling_script", json!({"script": script}));
    let res = r["data"]["result"].clone();
    let f3d_path = res["path"].as_str().map(str::to_string);
    let after_shot = cap_label.as_ref().map(|l| json!(crate::orchestrations::capture_labeled(ctx, &format!("{l}_after")))).unwrap_or(Value::Null);
    if let Some(p) = &f3d_path {
        if p.contains("dm.lineage:") {
            let lineage = p.rsplit("dm.lineage:").next().unwrap_or("").to_string();
            let wip = format!("urn:adsk.wipprod:fs.file:vf.{lineage}?version=1");
            if lbr_path.contains("0603WAF3300T5E") { let _ = std::fs::write(ctx.state_dir.join("r3d-urn.json"), json!({"0603WAF3300T5E": wip}).to_string()); }
            let placement_out = if res["placement"].is_object() { res["placement"].clone() } else { json!({"requested": null, "heuristics": {"orient": orient, "centering": false, "pin1": res["pin1"]["status"]}, "verification": "heuristic (no explicit placement to verify against)"}) };
            return json!({"success": true, "wip_urn": wip, "lineageUrn": p, "dims": res["dims"], "pin1": res["pin1"], "placement": placement_out, "source": provenance, "warnings": warnings, "bodies": res["bodies"], "renamedTo": res["renamedTo"], "fileName": f3d_name, "savedTo": if res["savedTo"].is_object() { res["savedTo"].clone() } else { json!("cloud (Adom AI Workspace)") }, "before": before_shot, "after": after_shot,
                "_hint": "Saved DIRECTLY to the cloud workspace - no upload step needed. Bind this wip_urn into the library's <packages3d>."});
        }
    }
    if let Some(pn) = &dest_project {
        // a named destination is never swapped for the fixed APS upload folder (another project)
        let code = res["errorCode"].as_str().unwrap_or("cloud_save_failed").to_string();
        return json!({"success": false, "errorCode": code, "projectName": pn, "folderPath": args["folderPath"], "detail": res,
            "error": if code == "project_not_found" { format!("No project named {pn} in this Autodesk hub, so nothing was saved (the bridge never falls back to another project)") } else { format!("The model could not be saved into {pn}: {}", res["err"].as_str().or(res["projErr"].as_str()).unwrap_or("Fusion's in-app cloud save failed")) },
            "_hint": "Pass the exact project name as Fusion's data panel shows it (fusion_list_cloud_projects); folderPath is created if missing. Without projectName the models go to the active project, as before."});
    }
    if res["errorCode"].as_str() == Some("no_cloud_project") {
        let detail: serde_json::Map<String, Value> = ["projErr", "projErr2", "createErr"].iter().filter_map(|k| res.get(*k).filter(|v| !v.is_null()).map(|v| (k.to_string(), v.clone()))).collect();
        return json!({"success": false, "errorCode": "no_cloud_project", "error": format!("3D minting uploads an .f3d to Autodesk cloud storage, and this account has no usable cloud project (typical of a brand-new Autodesk account). The bridge searched every hub for any healthy project and also tried to CREATE an 'Adom' project; all failed: {}", if detail.is_empty() { "(no per-step detail)".to_string() } else { crate::fusion_prefs::py_str(&Value::Object(detail)) }),
            "before": before_shot, "after": after_shot, "_hint": "This is account setup, not a broken mint. Open Fusion's Data Panel once (a fresh account provisions its default project on first touch), or create any project in the hub, then re-run. fusion_list_cloud_projects shows what the account can see."});
    }
    let Some(f3d_path) = f3d_path.filter(|p| p.ends_with(".f3d")) else {
        return json!({"success": false, "error": format!("f3d saveAs did not produce a .f3d path: {}{}", crate::fusion_prefs::py_str(&res), r["error"].as_str().filter(|e| !e.is_empty()).map(|e| format!(" | addin: {}", e.chars().take(200).collect::<String>())).unwrap_or_default()), "before": before_shot, "after": after_shot, "_hint": "Confirm modelPath is a valid Windows .step path and Fusion is running."});
    };
    let Some(folder_id) = folder_id else {
        return json!({"success": false, "errorCode": "no_work_folder", "error": format!("In-app cloud save failed ({}) and APS has no 'Adom AI Workspace' folder to upload the local f3d to.", crate::fusion_prefs::py_str(&res).chars().take(120).collect::<String>()), "before": before_shot, "after": after_shot,
            "_hint": "Two independent paths can mint the 3D package: the add-in's direct cloud save (needs Fusion running + signed in; the res/projErr above says why it failed) or the APS upload fallback (needs fusion_aps_signin). Fix either. The bridge never writes to a shared project ROOT."});
    };
    let up = crate::native_aps_upload(&json!({"projectId": project_id, "folderId": folder_id, "localPath": f3d_path, "fileName": f3d_name}));
    let up_d = if up["data"].is_object() { up["data"].clone() } else { up.clone() };
    let mut item_urn = up_d["itemUrn"].as_str().unwrap_or("").to_string();
    let _ = pass("run_modeling_script", json!({"script": "app.activeDocument.close(False)\nresult={}"}));
    if !item_urn.contains("dm.lineage:") {
        if up_d["httpStatus"].as_i64() == Some(409) {
            let store = fusion_core::aps::ApsStore::default_location();
            if let Ok((200, listing)) = fusion_core::aps::aps_get(&store, &format!("/data/v1/projects/{project_id}/folders/{folder_id}/contents"), &[]) {
                let stem = f3d_name.to_lowercase().rsplit_once(".f3d").map(|(s, _)| s.to_string()).unwrap_or_default();
                for it in listing["data"].as_array().cloned().unwrap_or_default() {
                    let nm = it["attributes"]["displayName"].as_str().unwrap_or("").to_string(); let iid = it["id"].as_str().unwrap_or("").to_string();
                    if iid.contains("dm.lineage:") && nm.to_lowercase().starts_with(&stem) { eprintln!("[fusion-bridge] upload 409: reusing existing cloud f3d {nm:?} -> {}", &iid[iid.len().saturating_sub(40)..]); item_urn = iid; break; }
                }
            }
        }
        if !item_urn.contains("dm.lineage:") { return json!({"success": false, "error": format!("f3d upload did not return a lineage urn: {}", crate::fusion_prefs::py_str(&up_d))}); }
    }
    let lid = item_urn.rsplit("dm.lineage:").next().unwrap_or("").to_string();
    let wip = format!("urn:adsk.wipprod:fs.file:vf.{lid}?version=1");
    json!({"success": true, "wip_urn": wip, "dims_mm": res["dims"], "f3d": f3d_path, "before": before_shot, "after": after_shot,
        "_hint": format!("DONE - a PROPER component 3D package (FOOTPRINT + chip, aligned) created with NO GUI dialogs. NEXT: hand-write this wip_urn into the library's EAGLE XML - a <package3d name=\"PKG\" urn=\"\" wip_urn=\"{wip}\" locally_modified=\"yes\" type=\"model\"> (with <packageinstances><packageinstance name=\"PKG\"/></packageinstances>) inside <packages3d> (between </packages> and <symbols>), PLUS <package3dinstances><package3dinstance package3d_urn=\"{wip}\"/></package3dinstances> inside the device after </connects>. Then fusion_open_lbr the merged .lbr. EXPECT: fusion_check_dialogs == 0 (no broken-ref), the device 3D preview shows footprint + chip (NOT 'Thumbnail download failed'), and opening the f3d shows the chip's pads landing on the footprint pads. SPEEDUP: for a many-part library, call this verb once per part, collect the urns, hand-merge ALL bindings in ONE pass, then fusion_open_lbr ONCE (don't open/save per part). PITFALLS: needs Fusion running (after a bridge_install respawn it can transiently report 'not running' - fusion_start, then retry); a RAW STEP upload binds but renders NOTHING ('Thumbnail download failed') so always go through this verb (it makes an f3d); NEVER FINISH the Package3D generator (Package3DStop) - its Save dialog + two opaque CEF modals are unbeatable, this verb saveAs-es instead. Full recipe: the fusion-multipart-libraries skill.")})
}

fn bare(n: &str) -> String { regex::Regex::new(r"\s*\([^)]*\)\s*$").unwrap().replace(&n.replace('*', ""), "").trim().to_lowercase() }

/// `_close_docs_opened_since`: close every document that appeared since `before` (plus bridge scratch by name).
pub fn close_docs_opened_since(ctx: &Ctx, before: &[String], keep: &[String], pass: &dyn Fn(&str, Value) -> Value) -> Vec<String> {
    let keep: Vec<String> = keep.iter().map(|k| k.to_lowercase()).collect();
    let open_names = |pass: &dyn Fn(&str, Value) -> Value| -> Vec<String> { pass("document_info", json!({}))["data"]["openDocuments"].as_array().map(|a| a.iter().filter_map(|d| d["name"].as_str().map(str::to_string)).filter(|n| !n.is_empty()).collect()).unwrap_or_default() };
    let bridge_owned = |n: &str| { let b = bare(n); b.starts_with("adom-test-") || b.ends_with("_3d") || b == "adomdemosample" || b.starts_with("untitled") };
    let now_open = open_names(pass);
    let mut targets: std::collections::BTreeSet<String> = now_open.iter().filter(|n| !before.contains(n)).cloned().collect();
    for n in &now_open { if bridge_owned(n) { targets.insert(n.clone()); } }
    let deadline = Instant::now() + Duration::from_secs(240);
    while Instant::now() < deadline {
        let jr = pass("run_modeling_script", json!({"script": "import adsk.core\nresult={'jobs': adsk.core.Application.get().hasActiveJobs}"}));
        if jr["success"].as_bool() != Some(true) || jr["data"]["result"]["jobs"].as_bool() != Some(true) { break; }
        std::thread::sleep(Duration::from_secs(5));
    }
    let mut closed = Vec::new();
    let count_named = |pass: &dyn Fn(&str, Value) -> Value, name: &str| -> (usize, usize) { let docs = open_names(pass); (docs.len(), docs.iter().filter(|d| bare(d) == bare(name)).count()) };
    for n in targets {
        if keep.contains(&bare(&n)) { continue; }
        crate::chains::note_bridge_doc_pub(&ctx.state_dir, &n);
        for _ in 0..12 {
            let (total, mine) = count_named(pass, &n);
            if total <= 1 || mine == 0 { break; }
            let r = crate::orchestrations::close_bridge_doc_verified(ctx, &n, 30, pass);
            if r["deferred"].as_bool() == Some(true) { break; }
            let (_, mine2) = count_named(pass, &n);
            if mine2 < mine { closed.push(n.clone()); continue; }
            break;
        }
    }
    let mut last: Option<usize> = None;
    for _ in 0..12 {
        let st = pass("document_info", json!({}));
        let docs = st["data"]["openDocuments"].as_array().cloned().unwrap_or_default();
        if docs.len() <= 1 { break; }
        if let Some(l) = last { if docs.len() >= l { break; } }
        last = Some(docs.len());
        let Some(u) = docs.iter().find(|d| d["name"].as_str().unwrap_or("").starts_with("Untitled") && !(d["isModified"].as_bool().unwrap_or(false) || d["modified"].as_bool().unwrap_or(false))) else { break };
        let nm = u["name"].as_str().unwrap_or("").to_string();
        if pass("close_document", json!({"name": nm, "save": false, "saveToClose": true}))["success"].as_bool() == Some(true) { closed.push(nm); } else { break; }
    }
    closed
}

/// `_build_library_3d_core`: mint every part, inject all bindings in one pass, open once.
/// Bumped when make_3d_package's recipe changes what it produces, so older mints are made again.
const MINT_RECIPE: u32 = 1;
const MINT_CACHE_MAX: usize = 5000;

fn mint_cache_path(state_dir: &std::path::Path) -> std::path::PathBuf { state_dir.join("mint-cache.json") }

/// The `<package>` element a part's footprint comes from, as written: a wiki re-export rewrites the rest of the file.
fn package_xml(lbr_text: &str, pkg: &str) -> Option<String> {
    let doc = roxmltree::Document::parse_with_options(lbr_text, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).ok()?;
    let found = doc.descendants().find(|n| n.has_tag_name("package") && n.attribute("name").map(|a| a.eq_ignore_ascii_case(pkg)).unwrap_or(false))?;
    Some(lbr_text[found.range()].to_string())
}

/// Everything that changes what a mint makes, and nothing else: the model's bytes, the footprint, how the model
/// is placed, whose account it lands in, and the destination the caller named (one is never swapped for another).
fn mint_cache_key(step: &[u8], footprint: &str, pkg: &str, orient: bool, placement: &Value, account: &str, project: &Value, folder: &Value) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(json!({"recipe": MINT_RECIPE, "package": pkg, "orient": orient, "placement": placement, "account": account, "project": project, "folder": folder}).to_string().as_bytes());
    h.update(Sha256::digest(step));
    h.update(footprint.as_bytes());
    format!("{:x}", h.finalize())
}

fn mint_cache_read(state_dir: &std::path::Path) -> Value {
    std::fs::read_to_string(mint_cache_path(state_dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}))
}

fn mint_cache_store(state_dir: &std::path::Path, key: &str, record: Value) {
    let mut c = mint_cache_read(state_dir);
    if !c.is_object() { c = json!({}); }
    let m = c.as_object_mut().unwrap();
    m.remove(key);
    m.insert(key.to_string(), record);
    while m.len() > MINT_CACHE_MAX {
        let oldest = m.iter().min_by(|a, b| a.1["at"].as_f64().unwrap_or(0.0).total_cmp(&b.1["at"].as_f64().unwrap_or(0.0))).map(|(k, _)| k.clone());
        match oldest { Some(k) => { m.remove(&k); } None => break }
    }
    let p = mint_cache_path(state_dir);
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, c.to_string()).is_ok() { let _ = std::fs::rename(&tmp, &p); }
}

/// The signed-in Autodesk account, or None: with no account known nothing is reused.
fn mint_account(pass: &dyn Fn(&str, Value) -> Value) -> Option<String> {
    pass("run_modeling_script", json!({"script": "import adsk.core\nresult={'user': adsk.core.Application.get().userId}"}))["data"]["result"]["user"].as_str().filter(|u| !u.is_empty()).map(str::to_string)
}

/// A cached mint whose cloud file still exists. A deleted one, or any doubt, means None and a fresh mint:
/// a dead urn would put a broken package in the library.
fn cached_mint(state_dir: &std::path::Path, key: &str, pass: &dyn Fn(&str, Value) -> Value) -> Option<Value> {
    let rec = mint_cache_read(state_dir)[key].clone();
    let wip = rec["wipUrn"].as_str()?.to_string();
    let lineage = wip.split("vf.").nth(1)?.split('?').next()?.to_string();
    let script = format!("import adsk.core\ntry:\n    f=adsk.core.Application.get().data.findFileById('urn:adsk.wipprod:dm.lineage:{lineage}')\n    result={{'exists': f is not None and bool(f.name)}}\nexcept Exception:\n    result={{'exists': False}}");
    (pass("run_modeling_script", json!({"script": script}))["data"]["result"]["exists"].as_bool() == Some(true)).then_some(rec)
}

pub fn build_library_3d_core(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value, on_part: &dyn Fn(&Value, usize, usize)) -> Value {
    let combined = args["lbrPath"].as_str().unwrap_or("").replace('\\', "/");
    let parts = args["parts"].as_array().cloned().unwrap_or_default();
    if combined.is_empty() || parts.is_empty() { return json!({"success": false, "error": "lbrPath (combined .lbr) and parts[] are required.", "_hint": "parts: [{package, lbrPath (per-part footprint .lbr), modelPath (.step)}]. projectId/folderId default to the MAIN-project upload folder."}); }
    let out_path = args["outLbrPath"].as_str().filter(|s| !s.is_empty()).map(|s| s.replace('\\', "/")).unwrap_or_else(|| combined.clone());
    let capture = args["capture"].as_bool().unwrap_or(true);
    let task = args["task"].as_str().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| combined.rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_default());
    let mut targs = args.clone(); targs["task"] = json!(task);
    let (project_id, folder_id) = discover_upload_target(&targs);
    let (mut results, mut bindings, mut shots): (Vec<Value>, Vec<fusion_lbr::Binding>, Vec<Value>) = (vec![], vec![], vec![]);
    let mut cancelled = false;
    let total = parts.len();
    let fresh_all = args["freshMint"].as_bool() == Some(true);
    let account = if fresh_all { None } else { mint_account(pass) };
    let mut reused_count = 0usize;
    for p in &parts {
        let pkg = p["package"].as_str().unwrap_or("").to_string();
        let per_lbr = p["lbrPath"].as_str().unwrap_or("").replace('\\', "/");
        let step = p["modelPath"].as_str().unwrap_or("").replace('\\', "/");
        if let Some(key) = args["_cancelKey"].as_str() { if crate::chains::cancel_requested_pub(key) { cancelled = true; break; } }
        // this part minted before, same model, footprint and placement: its 3D package is reused, about a minute saved each
        let cache_key = account.as_ref().filter(|_| p["freshMint"].as_bool() != Some(true)).and_then(|acct| {
            let (bytes, text) = (std::fs::read(&step).ok()?, std::fs::read_to_string(&per_lbr).ok()?);
            Some(mint_cache_key(&bytes, &package_xml(&text, &pkg)?, &pkg, p["orient"].as_bool().unwrap_or(true), &p["placement"], acct, &args["projectName"], &args["folderPath"]))
        });
        if let Some(rec) = cache_key.as_ref().and_then(|k| cached_mint(&ctx.state_dir, k, pass)) {
            let wip = rec["wipUrn"].as_str().unwrap_or("").to_string();
            bindings.push(fusion_lbr::Binding { package: pkg.clone(), wip_urn: wip.clone() });
            results.push(json!({"package": pkg, "success": true, "reused": true, "wip_urn": wip, "dims_mm": rec["dims"], "pin1": rec["pin1"], "stepStory": rec["stepStory"]}));
            reused_count += 1;
            on_part(results.last().unwrap(), results.len(), total);
            continue;
        }
        let docs_before: Vec<String> = pass("document_info", json!({}))["data"]["openDocuments"].as_array().map(|a| a.iter().filter_map(|d| d["name"].as_str().map(str::to_string)).collect()).unwrap_or_default();
        if pkg.is_empty() || per_lbr.is_empty() || step.is_empty() { results.push(json!({"package": pkg, "success": false, "error": "package, lbrPath (per-part .lbr) and modelPath (.step) all required"})); on_part(results.last().unwrap(), results.len(), total); continue; }
        let mk_args = json!({"lbrPath": per_lbr, "modelPath": step, "projectId": project_id, "folderId": folder_id, "projectName": args["projectName"], "folderPath": args["folderPath"], "fileName": format!("{pkg}_3d"), "task": task, "placement": p["placement"].clone(), "orient": p["orient"].clone(), "captureLabel": if capture { json!(pkg) } else { Value::Null }, "orient": p["orient"].as_bool().unwrap_or(true)});
        let mut mk = make_3d_package(ctx, &mk_args, pass);
        if mk["success"].as_bool() != Some(true) { std::thread::sleep(Duration::from_secs(2)); mk = make_3d_package(ctx, &mk_args, pass); }
        let mut entry = json!({"package": pkg, "success": mk["success"].as_bool().unwrap_or(false)});
        if mk["success"].as_bool() == Some(true) {
            let dims = if mk["dims_mm"].is_null() { mk["dims"].clone() } else { mk["dims_mm"].clone() };
            let story = json!(step_layer_story(&mk["bodies"]));
            entry["wip_urn"] = mk["wip_urn"].clone(); entry["dims_mm"] = dims.clone(); entry["pin1"] = mk["pin1"].clone(); entry["stepStory"] = story.clone();
            if let Some(k) = &cache_key {
                let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
                mint_cache_store(&ctx.state_dir, k, json!({"wipUrn": mk["wip_urn"], "dims": dims, "pin1": mk["pin1"], "stepStory": story, "package": pkg, "at": at}));
            }
            bindings.push(fusion_lbr::Binding { package: pkg.clone(), wip_urn: mk["wip_urn"].as_str().unwrap_or("").to_string() });
        } else { entry["error"] = mk["error"].clone(); }
        if !mk["before"].is_null() { shots.push(json!({"package": pkg, "stage": "before", "path": mk["before"]})); }
        if !mk["after"].is_null() { shots.push(json!({"package": pkg, "stage": "after", "path": mk["after"]})); }
        entry["scratchClosed"] = json!(close_docs_opened_since(ctx, &docs_before, &[], pass));
        results.push(entry);
        on_part(results.last().unwrap(), results.len(), total);
    }
    let mut bound: Option<String> = None;
    if !bindings.is_empty() {
        let src = if std::path::Path::new(&out_path).exists() { out_path.clone() } else { combined.clone() };
        match std::fs::read_to_string(&src).map(|t| fusion_lbr::inject_package3d_bindings(&t, &bindings)).and_then(|t| std::fs::write(&out_path, t)) {
            Ok(_) => bound = Some(out_path.clone()),
            Err(e) => return json!({"success": false, "error": format!("binding injection failed: {e}"), "parts": results, "bindings": bindings, "screenshots": shots}),
        }
    }
    let mut opened = Value::Null;
    let mut open_error = Value::Null;
    let mut scratch_after_open = Value::Null;
    if let Some(b) = &bound { if args["openWhenDone"].as_bool().unwrap_or(true) {
        let op = crate::orchestrations::open_lbr(ctx, &json!({"filePath": b}), pass);
        let ok = op["success"].as_bool() == Some(true);
        opened = json!(ok);
        if !ok { open_error = op["error"].clone(); }
        // each part's close leaves the last open document alone, so the last _3d survives until the library is
        // open; now it's safe to close, and nothing else is touched (bridge-owned scratch only)
        if ok {
            let keep = [std::path::Path::new(b).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()];
            let open_now: Vec<String> = pass("document_info", json!({}))["data"]["openDocuments"].as_array().map(|a| a.iter().filter_map(|d| d["name"].as_str().map(str::to_string)).collect()).unwrap_or_default();
            scratch_after_open = json!(close_docs_opened_since(ctx, &open_now, &keep, pass));
        }
    } }
    let n_ok = results.iter().filter(|r| r["success"].as_bool() == Some(true)).count();
    // Parts minted but a library Fusion will not open is a failed build, whatever each part did.
    json!({"success": n_ok > 0 && open_error.is_null(), "error": open_error, "cancelled": cancelled, "boundLbr": bound, "partsBound": n_ok, "partsTotal": total, "partsReused": reused_count, "partsMinted": n_ok - reused_count, "scratchClosedAfterOpen": scratch_after_open, "parts": results, "screenshots": shots, "opened": opened,
        "_hint": if !open_error.is_null() { format!("{n_ok}/{total} parts minted, but the finished library would not open in Fusion: read error, and fix it before using {}.", bound.as_deref().unwrap_or("the library")) } else { format!("{n_ok}/{total} parts got a RENDERING footprint+chip 3D package; all bindings injected into {} and the library opened once. NEXT: fusion_check_dialogs should be 0 (no broken-ref). For clean device previews + screenshots, save to the cloud and REOPEN from the cloud (fusion_aps_open) - the in-session view often gets an empty 'Untitled' shoved in front. Each device's lower-right 3D preview should show footprint + chip (NOT 'Thumbnail download failed'). The BEFORE (footprint) / AFTER (chip placed) PNGs are in screenshots[] (on the box in %TEMP%/adom-fusion-shots; ab pulls them to you as localFullPath) - pull them with desktop_pull_file (or re-capture via desktop_screenshot_window) for the demo video. AFTER (recommended): call fusion_capture_library_views per showcase part to grab the symbol / footprint / component (pin<->pad mapped) views - those are what make EEs trust the library, and the 3D-only shots miss them. PITFALLS: needs Fusion running. ⚠️ RELAY TIMEOUT: a many-part run takes minutes but the Adom Bridge relay times out the REQUEST at ~60s - you may get 'Request timed out' even though the build KEEPS RUNNING server-side and finishes. Do NOT assume it failed: wait, then verify by reading the boundLbr (count <package3d name=) + the before/after PNGs (%TEMP%/adom-fusion-shots on the box, pulled to you as localFullPath). If a part is missing, just re-run build_library_3d for the missing parts pointing outLbrPath at the SAME file - binding injection is now MERGE-AWARE and idempotent, so it accumulates onto the existing bindings (it no longer wipes the parts that already succeeded). Each part also self-retries once on a transient first-part failure. Full recipe: the fusion-multipart-libraries skill.", bound.as_deref().unwrap_or("None")) }})
}

#[cfg(test)]
mod mint_cache_tests {
    use super::*;
    fn dir(t: &str) -> std::path::PathBuf { let d = std::env::temp_dir().join(format!("adom-mint-cache-{t}-{}", std::process::id())); let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).unwrap(); d }
    fn key(step: &[u8], fp: &str, pkg: &str, orient: bool, pl: Value, acct: &str, proj: Value) -> String { mint_cache_key(step, fp, pkg, orient, &pl, acct, &proj, &Value::Null) }
    #[test] fn the_key_changes_with_what_changes_the_result_and_nothing_else() {
        let base = key(b"s", "<package/>", "P", true, Value::Null, "me", Value::Null);
        assert_eq!(base, key(b"s", "<package/>", "P", true, Value::Null, "me", Value::Null));
        assert_ne!(base, key(b"t", "<package/>", "P", true, Value::Null, "me", Value::Null));
        assert_ne!(base, key(b"s", "<package a=\"1\"/>", "P", true, Value::Null, "me", Value::Null));
        assert_ne!(base, key(b"s", "<package/>", "Q", true, Value::Null, "me", Value::Null));
        assert_ne!(base, key(b"s", "<package/>", "P", false, Value::Null, "me", Value::Null));
        assert_ne!(base, key(b"s", "<package/>", "P", true, json!({"translation": [1, 0, 0]}), "me", Value::Null));
        assert_ne!(base, key(b"s", "<package/>", "P", true, Value::Null, "you", Value::Null));
        assert_ne!(base, key(b"s", "<package/>", "P", true, Value::Null, "me", json!("Other project")));
    }
    #[test] fn the_footprint_is_only_that_package_so_a_rewrite_of_the_file_keeps_it() {
        let a = r#"<eagle><library><packages><package name="R1"><smd name="1"/></package><package name="R2"/></packages></library></eagle>"#;
        let b = r#"<?xml version="1.0"?><!DOCTYPE eagle SYSTEM "eagle.dtd"><eagle attr="new"><library><description>x</description><packages><package name="R0"/><package name="R1"><smd name="1"/></package></packages></library></eagle>"#;
        assert_eq!(package_xml(a, "R1"), package_xml(b, "R1"));
        assert!(package_xml(a, "R1").unwrap().contains("smd"));
        assert!(package_xml(a, "NOPE").is_none());
    }
    #[test] fn a_stored_mint_is_reused_only_while_its_file_exists() {
        let d = dir("exists");
        mint_cache_store(&d, "k", json!({"wipUrn": "urn:adsk.wipprod:fs.file:vf.ABC?version=1", "dims": [1, 2, 3], "at": 1.0}));
        let asked = std::cell::RefCell::new(String::new());
        let yes = |_: &str, a: Value| { *asked.borrow_mut() = a["script"].as_str().unwrap().to_string(); json!({"data": {"result": {"exists": true}}}) };
        assert_eq!(cached_mint(&d, "k", &yes).unwrap()["dims"], json!([1, 2, 3]));
        assert!(asked.borrow().contains("dm.lineage:ABC'"), "{}", asked.borrow());
        assert!(cached_mint(&d, "k", &|_, _| json!({"data": {"result": {"exists": false}}})).is_none());
        assert!(cached_mint(&d, "k", &|_, _| json!({"success": false, "error": "busy"})).is_none());
        assert!(cached_mint(&d, "other", &yes).is_none());
    }
    #[test] fn no_account_means_no_reuse() {
        assert!(mint_account(&|_, _| json!({"data": {"result": {"user": ""}}})).is_none());
        assert!(mint_account(&|_, _| json!({"success": false})).is_none());
        assert_eq!(mint_account(&|_, _| json!({"data": {"result": {"user": "me"}}})).as_deref(), Some("me"));
    }
    #[test] fn the_cache_keeps_its_newest_entries() {
        let d = dir("evict");
        for i in 0..3 { mint_cache_store(&d, &format!("k{i}"), json!({"at": i as f64})); }
        assert_eq!(mint_cache_read(&d).as_object().unwrap().len(), 3);
        let mut big = serde_json::Map::new();
        for i in 0..MINT_CACHE_MAX { big.insert(format!("o{i}"), json!({"at": i as f64 + 10.0})); }
        std::fs::write(mint_cache_path(&d), Value::Object(big).to_string()).unwrap();
        mint_cache_store(&d, "new", json!({"at": 1e9}));
        let c = mint_cache_read(&d); let m = c.as_object().unwrap();
        assert_eq!(m.len(), MINT_CACHE_MAX); assert!(m.contains_key("new")); assert!(!m.contains_key("o0"));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn attach_script_matches_python_golden() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!("../tests/golden/attach_script.json")).unwrap();
        for c in cases {
            let layers: Vec<String> = c["layersOff"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
            let got = super::attach_script(c["modelPath"].as_str().unwrap(), c["orient"].as_bool().unwrap(), &layers);
            assert_eq!(got, c["script"].as_str().unwrap(), "attach script drift");
        }
    }
    #[test]
    fn placement_parses_and_refuses() {
        use serde_json::json;
        let ok = super::parse_placement(&json!({"translation": [-0.00008128, 1.39191746, 0.18269966], "rotation": [0, 0, 90], "scale": 1})).unwrap().unwrap();
        assert_eq!(ok.rotation_deg, [0.0, 0.0, 90.0]);
        assert!(super::parse_placement(&json!(null)).unwrap().is_none());
        assert_eq!(super::parse_placement(&json!({"translation": [1, 2], "rotation": [0, 0, 0]})).unwrap_err()["errorCode"], "invalid_placement");
        assert_eq!(super::parse_placement(&json!({"translation": [1, 2, "x"]})).unwrap_err()["errorCode"], "invalid_placement");
        assert_eq!(super::parse_placement(&json!({"translation": [0, 0, 0], "scale": 2})).unwrap_err()["errorCode"], "unsupported_scale");
        assert_eq!(super::parse_placement(&json!({"translation": [0, 0, 0], "units": "in"})).unwrap_err()["errorCode"], "invalid_placement");
        // an explicit placement skips both heuristics; orient:false skips both too; default keeps both
        let pl = super::Placement { translation_mm: [-0.00008128, 1.39191746, 0.18269966], rotation_deg: [0.0, 0.0, 90.0] };
        let placed = super::make_3d_script_placed("C:/m.step", true, Some((1.0, 2.0)), &[], "t", "n", Some(&pl));
        assert!(placed.contains("oc.transform2=_pmat") && placed.contains("PIN1=None") && placed.contains("PIN1_SKIP='skipped_explicit_placement'") && !placed.contains("is_flat"));
        assert!(placed.contains("_pt=(-8.128e-5,1.39191746,0.18269966)") || placed.contains("_pt=(-0.00008128,1.39191746,0.18269966)"), "{}", &placed[..400]);
        let off = super::make_3d_script_placed("C:/m.step", false, Some((1.0, 2.0)), &[], "t", "n", None);
        assert!(off.contains("PIN1=None") && off.contains("PIN1_SKIP='skipped_orient_false'") && !off.contains("is_flat") && !off.contains("_pmat"));
        let dflt = super::make_3d_script_placed("C:/m.step", true, Some((1.0, 2.0)), &[], "t", "n", None);
        assert!(dflt.contains("is_flat") && dflt.contains("PIN1=(0.1, 0.2)") && dflt.contains("PIN1_SKIP=None"));
    }
    #[test]
    fn mint_script_matches_python_golden() {
        if std::env::var("UPDATE_GOLDEN").is_ok() {
            let mut cases: Vec<serde_json::Value> = serde_json::from_str(include_str!("../tests/golden/make3d_script.json")).unwrap();
            for c in cases.iter_mut() {
                let pin1 = c["pin1"].as_array().map(|a| (a[0].as_f64().unwrap(), a[1].as_f64().unwrap()));
                let layers: Vec<String> = c["layersOff"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
                c["script"] = serde_json::Value::String(super::make_3d_script(c["modelPath"].as_str().unwrap(), c["orient"].as_bool().unwrap(), pin1, &layers, c["task"].as_str().unwrap(), c["name"].as_str().unwrap()));
            }
            std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/make3d_script.json"), serde_json::to_string_pretty(&cases).unwrap()).unwrap();
            return;
        }
        let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!("../tests/golden/make3d_script.json")).unwrap();
        for c in cases {
            let pin1 = c["pin1"].as_array().map(|a| (a[0].as_f64().unwrap(), a[1].as_f64().unwrap()));
            let layers: Vec<String> = c["layersOff"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
            let got = super::make_3d_script(c["modelPath"].as_str().unwrap(), c["orient"].as_bool().unwrap(), pin1, &layers, c["task"].as_str().unwrap(), c["name"].as_str().unwrap());
            let want = c["script"].as_str().unwrap();
            if got != want { let i = got.chars().zip(want.chars()).position(|(a, b)| a != b).unwrap_or(0); panic!("script drift at byte {i}: got {:?} want {:?}", &got[i.saturating_sub(40)..(i + 80).min(got.len())], &want[i.saturating_sub(40)..(i + 80).min(want.len())]); }
        }
    }
}

/// `fusion_build_library_3d`: detached by default (a many-part build outlives ab's relay cap);
/// progress lands in the shared run store and the palette, and the bound file is the proof.
pub fn build_library_3d(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let combined = args["lbrPath"].as_str().unwrap_or("").replace('\\', "/");
    let out_path = args["outLbrPath"].as_str().filter(|s| !s.is_empty()).map(|s| s.replace('\\', "/")).unwrap_or_else(|| combined.clone());
    let parts = args["parts"].as_array().cloned().unwrap_or_default();
    if args["async"].as_bool().unwrap_or(true) && !parts.is_empty() {
        // record the build in flight on disk (the capture guard reads it, respawns included)
        let flight_p = ctx.state_dir.join("builds_in_flight.json");
        let mut flight: Value = std::fs::read_to_string(&flight_p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
        flight[&out_path] = json!(now_f());
        let _ = std::fs::write(&flight_p, flight.to_string());
        let run_id = format!("{:x}", { use std::hash::{Hash, Hasher}; let mut h = std::collections::hash_map::DefaultHasher::new(); format!("{}{}", now_f(), std::process::id()).hash(&mut h); h.finish() })[..12].to_string();
        let total = parts.len();
        let mut bstate = json!({"runId": run_id, "verb": "fusion_build_library_3d", "stage": "starting", "boundLbr": out_path, "partsTotal": total, "partsDone": 0, "parts": [], "owner": "exe"});
        crate::chains::save_state_pub(&run_id, &bstate);
        // the build outlives this request: a fresh context on a thread (the borrowed one cannot cross)
        let (sd, caller, args2, rid) = (ctx.state_dir.clone(), ctx.caller.clone(), args.clone(), run_id.clone());
        let lib_label = crate::chains::lib_label_pub(&combined);
        std::thread::spawn(move || {
            let host = crate::host_for_diagnosis();
            let side_url = crate::launch::SIDECAR_URL.lock().unwrap().clone();
            let side = move |verb: &str, a: Value, t: u64| -> Value { match &side_url { Some(u) => ureq::post(u).timeout(Duration::from_secs(t)).send_json(json!({"command": verb, "args": a})).ok().and_then(|r| r.into_json().ok()).unwrap_or(json!({"success": false, "error": "sidecar unreachable"})), None => json!({"success": false, "error": "sidecar absent"}) } };
            let pids: Vec<u32> = host.fusion_processes().iter().map(|p| p.pid).collect();
            let dlg = |r: Value| r;
            let ctx2 = Ctx { host: &*host, fusion_exe: crate::host_for_diagnosis_paths(), short_path: |p| crate::host().short_path(p), state_dir: sd.clone(), caller: caller.clone(), pids, sidecar: &side, dialogs: &dlg };
            let pass2 = |verb: &str, a: Value| crate::addin::pass_through(verb, &a, &ctx2);
            let st = std::cell::RefCell::new(bstate.clone());
            let part_clock = std::cell::Cell::new(Instant::now());
            let on_part = |entry: &Value, i: usize, total: usize| {
                if i > 1 { crate::timings::record(&sd, "mint.part", part_clock.get().elapsed().as_secs_f64(), None); }
                part_clock.set(Instant::now());
                let per_part = crate::chains::estimate(&sd, "mint.part", 90.0).0;
                let eta = Some(per_part * (total.saturating_sub(i) + 1) as f64);
                let mut s = st.borrow_mut();
                s["stage"] = json!(format!("minting {} ({i}/{total})", entry["package"].as_str().unwrap_or("part")));
                s["partsDone"] = json!(i);
                let mut ps = s["parts"].as_array().cloned().unwrap_or_default(); ps.push(entry.clone()); s["parts"] = json!(ps);
                crate::chains::save_state_pub(&rid, &s);
                crate::palette::say(&format!("{lib_label}: binding 3D, part {i} of {total}"), &format!("{}. Importing its STEP and saving its 3D package", entry["package"].as_str().unwrap_or("part")), Some(100.0 * i as f64 / total.max(1) as f64 * 0.4), eta);
            };
            let res = build_library_3d_core(&ctx2, &args2, &pass2, &on_part);
            { let mut s = st.borrow_mut(); s["stage"] = json!("done"); s["success"] = json!(res["success"].as_bool().unwrap_or(false)); s["result"] = res; let n = s["parts"].as_array().map(|a| a.len()).unwrap_or(0); s["partsDone"] = json!(n); bstate = s.clone(); }
            crate::chains::save_state_pub(&rid, &bstate);
            let fp = sd.join("builds_in_flight.json");
            if let Some(mut f) = std::fs::read_to_string(&fp).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) { if let Some(o) = f.as_object_mut() { o.remove(bstate["boundLbr"].as_str().unwrap_or("")); } let _ = std::fs::write(&fp, f.to_string()); }
        });
        return json!({"success": true, "status": "started", "async": true, "runId": run_id, "statusVerb": "fusion_prove_result", "boundLbr": out_path, "partsTotal": total, "owner": "exe",
            "_hint": "⏳ 3D bind runs DETACHED (survives ab's 60s relay cap, which used to kill it + lose every wip_urn). POLL the boundLbr via read_file every ~30s until it has one <package3d name=... wip_urn=urn:...> PER PART (count == partsTotal). Do NOT re-fire while running (check fusion_addin_status.busy). When done, embed those <packages3d> + a per-<element> package3d_urn in your .brd, then fusion_open_board + fusion_show_3d_board for real 3D bodies (all background). If a urn stays unresolved in the 3D view, re-run for just that part - its f3d upload failed."});
    }
    let lib_label = crate::chains::lib_label_pub(&combined);
    let part_clock = std::cell::Cell::new(Instant::now());
    let on_part = |entry: &Value, i: usize, total: usize| {
        if i > 1 { crate::timings::record(&ctx.state_dir, "mint.part", part_clock.get().elapsed().as_secs_f64(), None); }
        part_clock.set(Instant::now());
        let per_part = crate::chains::estimate(&ctx.state_dir, "mint.part", 90.0).0;
        let story = step_story_from_file(entry["modelPath"].as_str().unwrap_or(""));
        crate::palette::say(&format!("{lib_label}: binding 3D, part {i} of {total}"), &format!("{}. {story}. Importing its STEP and saving its 3D package", entry["package"].as_str().unwrap_or("part")), Some(100.0 * i as f64 / total.max(1) as f64 * 0.4), Some(per_part * (total.saturating_sub(i) + 1) as f64));
    };
    build_library_3d_core(ctx, args, pass, &on_part)
}

/// `fusion_bind_library_3d`: mint one part's 3D, write the binding into the .lbr on disk, reopen.
pub fn bind_library_3d(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let lbr_path = args["libraryPath"].as_str().unwrap_or("").replace('\\', "/");
    let step_path = args["stepPath"].as_str().unwrap_or("").replace('\\', "/");
    if lbr_path.is_empty() || step_path.is_empty() { return json!({"success": false, "error": "libraryPath (.lbr) and stepPath (.step) required (Windows paths)."}); }
    let mut txt = match std::fs::read(&lbr_path) { Ok(b) => String::from_utf8_lossy(&b).to_string(), Err(e) => return json!({"success": false, "error": format!("cannot read {lbr_path}: {e}")}) };
    let deviceset = match args["deviceset"].as_str().filter(|s| !s.is_empty()) { Some(d) => d.to_string(), None => match regex::Regex::new(r#"<deviceset name="([^"]+)""#).unwrap().captures(&txt) { Some(c) => c[1].to_string(), None => return json!({"success": false, "error": "no deviceset in the library"}) } };
    let pkg_name = regex::Regex::new(r#"<package name="([^"]+)""#).unwrap().captures(&txt).map(|c| c[1].to_string()).unwrap_or_else(|| deviceset.clone());
    if let Some(c) = regex::Regex::new(r#"<package3d[^>]*urn="([^"]+)""#).unwrap().captures(&txt) {
        if args["force"].as_bool() != Some(true) {
            return json!({"success": true, "alreadyBound": true, "wip_urn": &c[1], "deviceset": deviceset, "libraryPath": lbr_path, "_hint": "This library already carries a package3d urn - nothing to mint. Pass force:true to re-bind (e.g. after replacing the STEP). fusion_check_3d_thumbnail verifies the cloud thumbnail rendered."});
        }
    }
    let safe_ds: String = regex::Regex::new(r"[^A-Za-z0-9_-]").unwrap().replace_all(&deviceset, "_").chars().take(40).collect();
    let mint = crate::small_verbs::apply_failure_dialogs(make_3d_package(ctx, &json!({"lbrPath": lbr_path, "modelPath": step_path, "fileName": format!("ADOM-{safe_ds}-3D")}), pass), &ctx.caller, &ctx.pids);
    let urn = mint["wip_urn"].as_str().unwrap_or("").to_string();
    if urn.is_empty() { return json!({"success": false, "error": format!("mint failed: {}", mint["error"].as_str().unwrap_or("None").chars().take(150).collect::<String>()), "_hint": mint["_hint"].as_str().unwrap_or("").chars().take(150).collect::<String>()}); }
    let bak = format!("{lbr_path}.prebind");
    if !std::path::Path::new(&bak).exists() { let _ = std::fs::copy(&lbr_path, &bak); }
    txt = regex::Regex::new(r"(?s)<packages3d>.*?</packages3d>\n?").unwrap().replace_all(&txt, "").to_string();
    let binding = format!(r#"<packages3d><package3d name="{deviceset}" urn="" wip_urn="{urn}" locally_modified="yes" type="model"><packageinstances><packageinstance name="{pkg_name}"/></packageinstances></package3d></packages3d>"#);
    txt = txt.replacen("</packages>", &format!("</packages>\n{binding}"), 1);
    txt = regex::Regex::new(r"(?s)<package3dinstances>.*?</package3dinstances>").unwrap().replace_all(&txt, "").to_string();
    txt = txt.replacen("<technologies>", &format!(r#"<package3dinstances><package3dinstance package3d_urn="{urn}"/></package3dinstances><technologies>"#), 1);
    if let Err(e) = std::fs::write(&lbr_path, &txt) { return json!({"success": false, "error": format!("binding write failed: {e}"), "wip_urn": urn}); }
    std::thread::sleep(Duration::from_secs(1));
    let reopened = crate::orchestrations::open_lbr(ctx, &json!({"filePath": lbr_path}), pass);
    if reopened["success"].as_bool() != Some(true) {
        let why: String = reopened["error"].as_str().or(reopened["_hint"].as_str()).unwrap_or("library re-open returned success:false with no detail (open_lbr)").chars().take(220).collect();
        return json!({"success": true, "wip_urn": urn, "libraryPath": lbr_path, "deviceset": deviceset, "bound": true, "reopened": false, "mutatedInput": true, "prebindBackup": bak, "reopenError": why,
            "_hint": format!("Binding minted ({urn}) and WRITTEN to the .lbr - the bind is real and persisted. Re-opening the library into Fusion failed (reopenError above); the prove stage opens the library itself, so the chain continues. If prove's 3D frame is missing, the library just needs a clean re-open once Fusion is idle.")});
    }
    let swept = sweep_untitled_scratch(pass);
    json!({"success": true, "wip_urn": urn, "mutatedInput": true, "untitledScratchClosed": swept.len(), "prebindBackup": bak, "libraryPath": lbr_path, "deviceset": deviceset, "bound": true, "reopened": true,
        "_hint": format!("NOTE: the input .lbr was REWRITTEN with the minted package3d binding (account-scoped; pristine copy kept at <libraryPath>.prebind). Library re-opened WITH the 3D binding. The Content Manager's 3D preview resolves after Autodesk's thumbnail service catches up (~2-3 min for a fresh mint) - EDIT {deviceset}.dev then screenshot to verify the render.")})
}

/// `_sweep_untitled_scratch`: close UNMODIFIED Untitled documents (Fusion keeps one alive).
pub fn sweep_untitled_scratch(pass: &dyn Fn(&str, Value) -> Value) -> Vec<String> {
    let mut closed = Vec::new();
    let mut prev_total: Option<usize> = None;
    for _ in 0..24 {
        let st = pass("document_info", json!({}));
        let docs = st["data"]["openDocuments"].as_array().cloned().unwrap_or_default();
        let untitled: Vec<&Value> = docs.iter().filter(|d| d["name"].as_str().unwrap_or("").to_lowercase().starts_with("untitled") && !(d["isModified"].as_bool().unwrap_or(false) || d["modified"].as_bool().unwrap_or(false))).collect();
        if (untitled.len() <= 1 && docs.len() == untitled.len()) || untitled.is_empty() { break; }
        if let Some(p) = prev_total { if docs.len() >= p { break; } }
        prev_total = Some(docs.len());
        let victim = untitled[0]["name"].as_str().unwrap_or("").to_string();
        if pass("close_document", json!({"name": victim, "save": false, "saveToClose": true}))["success"].as_bool() != Some(true) { break; }
        closed.push(victim);
    }
    closed
}

const ATTACH_TEMPLATE: &str = include_str!("attach_template.py");
const ATTACH_ORIENT_BLOCK: &str = include_str!("attach_orient_block.py");

/// The exact import/orient script `_orchestrate_attach_3d_package` sends.
pub fn attach_script(model_path: &str, orient: bool, layers_off: &[String]) -> String {
    let mut layers: Vec<String> = layers_off.iter().cloned().collect::<std::collections::BTreeSet<_>>().into_iter().collect(); layers.sort();
    let layers_repr = format!("[{}]", layers.iter().map(|l| fusion_lbr::py_repr_str(l)).collect::<Vec<_>>().join(", "));
    ATTACH_TEMPLATE.replace("{MODEL_PATH}", model_path).replace("{LAYERS_OFF}", &layers_repr).replace("{ORIENT_BLOCK}", if orient { ATTACH_ORIENT_BLOCK } else { "axis=None\n" })
}

/// `fusion_attach_3d_package`: enter the Package3D environment, import and orient the STEP, FINISH.
pub fn attach_3d_package(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let file_path = args["filePath"].as_str().unwrap_or("").replace('\\', "/");
    let model_path = args["modelPath"].as_str().unwrap_or("").replace('\\', "/");
    let package = args["packageName"].as_str().unwrap_or("").to_string();
    let orient = args["orient"].as_bool().unwrap_or(true);
    if file_path.is_empty() || model_path.is_empty() { return json!({"success": false, "error": "filePath (.lbr) and modelPath (.step) are required (Windows paths, e.g. C:/...).", "_hint": "Stage both onto Windows first (no container->Windows push verb). See the fusion-libraries skill."}); }
    let mut steps: Vec<String> = Vec::new();
    if args["foreground"].as_bool() != Some(true) { ctx.host.background_envelope(600.0, false); }
    let op = crate::orchestrations::open_lbr(ctx, &json!({"filePath": file_path}), pass);
    if op["success"].as_bool() != Some(true) { return json!({"success": false, "error": format!("open_lbr failed: {}", op["error"].as_str().unwrap_or("None")), "data": {"steps": steps}}); }
    steps.push("open_lbr: ok (library active)".into());
    std::thread::sleep(Duration::from_secs(1));
    let cp = crate::orchestrations::create_3d_package_pub(ctx, &file_path, pass);
    if cp["success"].as_bool() != Some(true) { return json!({"success": false, "error": format!("Create3DPackage failed: {}", cp["error"].as_str().unwrap_or("None")), "_hint": "The library document must be ACTIVE, and the .lbr must be adom-lbr-generated (a raw vendor EAGLE .lbr fails Fusion's XML parser).", "data": {"steps": steps}}); }
    steps.push("Create3DPackage: ok (Package3DEnvironment)".into());
    std::thread::sleep(Duration::from_secs(2));
    let layers: Vec<String> = args["layersOff"].as_array().map(|a| a.iter().map(|v| match v { Value::String(s) => s.clone(), o => o.to_string() }).collect()).unwrap_or_default();
    let imp = pass("run_modeling_script", json!({"script": attach_script(&model_path, orient, &layers)}));
    if imp["success"].as_bool() != Some(true) { return json!({"success": false, "error": format!("import/orient failed: {}", imp["error"].as_str().unwrap_or("None")), "data": {"steps": steps}}); }
    let dims = imp["data"]["result"].clone();
    steps.push(format!("import+orient: ok ({})", if dims.is_object() { crate::fusion_prefs::py_str(&dims["dims_mm"]) } else { crate::fusion_prefs::py_str(&dims) }));
    if dims.is_object() && !dims["bodies"].is_null() { steps.push(format!("bodies: {}; layers hidden: {}", crate::fusion_prefs::py_str(&dims["bodies"]), crate::fusion_prefs::py_str(&dims["layersHidden"]))); }
    std::thread::sleep(Duration::from_secs(1));
    let _ = pass("run_modeling_script", json!({"script": "import adsk.core\ncd=ui.commandDefinitions.itemById('Package3DStop')\nresult={'finished': bool(cd) and cd.execute()}\n"}));
    steps.push("FINISH (Package3DStop): executed".into());
    json!({"success": true, "output": format!("3D model placed + oriented on the footprint and FINISH executed for '{}'. A Save dialog is now up.", if package.is_empty() { &file_path } else { &package }), "data": {"steps": steps, "package": package}, "savePending": true,
        "_hint": "FINAL STEP (desktop-side, one click): a Fusion 'Save' dialog is now up to save the 3D package. Click it: desktop_find_window {titleContains:'Save'} -> desktop_ui_click {hwnd, automationId:'QTApplication.QTFrameWindow.standardActions.SaveButton'}. THEN VERIFY with desktop_screenshot_window on the Fusion hwnd: check ownedPopupCount (errors are owned popups), and the deviceset's Package column should flip Placeholder->part-name + the 3D preview becomes the real chip (the preview LAGS a beat - re-grab). Full flow: fusion-libraries skill / LIBRARY_FINDINGS.md sect 11."})
}

const SHOW_3D_FALLBACK_PACKAGE: &str = r#"<package name="{N}"><description>Adom auto footprint (3D carrier)</description><smd name="1" x="-1.27" y="0" dx="1.2" dy="1.2" layer="1"/><smd name="2" x="1.27" y="0" dx="1.2" dy="1.2" layer="1"/><wire x1="-2.2" y1="1.6" x2="2.2" y2="1.6" width="0.127" layer="21"/><wire x1="-2.2" y1="-1.6" x2="2.2" y2="-1.6" width="0.127" layer="21"/><text x="-2.2" y="2" size="1" layer="25">&gt;NAME</text></package>"#;

fn p3d_names(lbr: &str) -> Vec<String> { std::fs::read_to_string(lbr).ok().and_then(|x| fusion_lbr::artifact_names(&x).ok()).map(|n| n.p3d).unwrap_or_default() }

/// `fusion_show_3d_package`: the 3D chip for a library package, attaching a STEP first when asked.
pub fn show_3d_package(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut package_xml = args["packageXml"].as_str().map(str::to_string);
    let step_path = args["stepPath"].as_str().unwrap_or("").replace('\\', "/");
    let mut name = crate::orchestrations::artifact_name_from(args, &["packageXml"]);
    if name.is_empty() && !step_path.is_empty() { name = regex::Regex::new(r"[^A-Za-z0-9_-]").unwrap().replace_all(step_path.rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(s, _)| s).unwrap_or(""), "_").chars().take(60).collect(); }
    if name.is_empty() { return json!({"success": false, "error": "name required (or derivable from packageXml/stepPath)."}); }
    let mut library_path = args["libraryPath"].as_str().unwrap_or("").replace('\\', "/");
    let mut wrapped: Option<String> = None;
    if library_path.is_empty() {
        if package_xml.is_none() && !step_path.is_empty() { package_xml = Some(SHOW_3D_FALLBACK_PACKAGE.replace("{N}", &name)); }
        let Some(px) = package_xml.clone() else { return json!({"success": false, "error": "Need a footprint to host the 3D: pass libraryPath, packageXml, or stepPath.", "_hint": "A 3D package lives ON a footprint. stepPath alone works: a minimal 2-pad carrier footprint is generated."}) };
        match crate::orchestrations::lbr_wrap_write(&ctx.state_dir, &name, None, Some(&px)) { (Some(p), _) => { wrapped = Some(p.clone()); library_path = p; }, (_, Some(e)) => return json!({"success": false, "error": e}), _ => unreachable!() }
    }
    if !step_path.is_empty() {
        let mut res = crate::small_verbs::merge_dialog_array(attach_3d_package(ctx, &json!({"filePath": library_path, "modelPath": step_path, "packageName": name}), pass), &ctx.caller, &ctx.pids);
        if p3d_names(&library_path).is_empty() {
            let up_err = res["error"].as_str().or(res["attachError"].as_str()).unwrap_or("attach reported no error but bound nothing").to_string();
            return json!({"success": false, "errorCode": "3d_attach_failed", "error": "Attaching the STEP failed, so the library still has no 3D package - refusing to show Fusion's Placeholder as if it were the part.", "attachError": up_err.chars().take(400).collect::<String>(), "libraryPath": library_path,
                "_hint": "The upstream failure is in attachError (a 409 here usually means the f3d already exists in the cloud - the bind path reuses it as of 1.9.11x, so retry fusion_bind_library_3d and read ITS message). Nothing was rendered because a Placeholder screenshot photographs exactly like a real chip and must never count as one."});
        }
        if let Some(w) = &wrapped { res["wrappedLibrary"] = json!(w); }
        res["view"] = json!("3d-package");
        return res;
    }
    let opened = crate::orchestrations::open_lbr(ctx, &json!({"filePath": library_path}), pass);
    if opened["success"].as_bool() != Some(true) { return json!({"success": false, "error": format!("open_lbr failed: {}", opened["error"].as_str().unwrap_or("None"))}); }
    if p3d_names(&library_path).is_empty() {
        return json!({"success": false, "error": "That library has no 3D package bound, so the only thing to show is Fusion's grey Placeholder - refusing to report that as a 3D package.", "errorCode": "no_3d_package_bound", "libraryPath": library_path,
            "_hint": "Bind a real model first: re-call fusion_show_3d_package with stepPath pointing at the STEP file and the bridge attaches it onto the footprint, then shows it. A Placeholder rendering LOOKS like a successful 3D view in a screenshot, which is why this refuses rather than letting a caller count it as proof."});
    }
    let cp = crate::orchestrations::create_3d_package_pub(ctx, &library_path, pass);
    if cp["success"].as_bool() != Some(true) { return json!({"success": false, "error": format!("Create3DPackage failed: {}", cp["error"].as_str().unwrap_or("None")), "_hint": "The library must be Fusion-parseable (adom-lbr style); raw vendor .lbr files fail Fusion's parser."}); }
    let mut out = json!({"success": true, "view": "3d-package", "name": name, "libraryPath": library_path});
    let shot = crate::small_verbs::screenshot_fusion(&json!({}), &ctx.caller);
    if shot["success"].as_bool() == Some(true) { if let Some(p) = shot["savedTo"].as_str() { out["screenshot"] = json!(p); } }
    if let Some(w) = &wrapped { out["wrappedLibrary"] = json!(w); }
    out
}

#[cfg(test)]
mod destination_tests {
    #[test]
    fn named_project_replaces_the_active_project_block() {
        let plain = super::make_3d_script("C:/m.step", true, None, &[], "Run 7", "U1_3d");
        assert!(plain.contains("proj=app.data.activeProject"));
        let named = super::with_destination(&plain, "Claude AI", "Adom AI Flow", "Run 7").expect("block found");
        assert!(!named.contains("proj=app.data.activeProject"), "the active project is never consulted");
        assert!(named.contains("_PN=\"Claude AI\"; _FP=\"Adom AI Flow\"; _TK=\"Run 7\""));
        assert!(named.contains("res['errorCode']='project_not_found'"));
        assert!(named.contains("doc.saveAs('U1_3d', wsf"), "the save itself is untouched");
        // the rest of the script is unchanged
        let (a, b) = (plain.split("proj=None").next().unwrap(), named.split("proj=None").next().unwrap());
        assert_eq!(a, b);
        // quotes in names cannot break out of the string
        let q = super::with_destination(&plain, "Bob's \"project\"", "", "Run 7").unwrap();
        assert!(q.contains(r#"_PN="Bob's \"project\"""#));
    }
}

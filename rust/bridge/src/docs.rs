//! document_info and close_all_documents in the EDA Bridge Standard's shape (docs_script.py on
//! Fusion's main thread). document_info keeps the add-in's reply and adds the standard's fields.
use serde_json::{json, Value};
// render_process.py (the mechanical layer's render cancel) is embedded as text and loaded as its own module
// inside the script, so render_board_3d can end a timed-out render the same way render_image does
const SOURCE: &str = concat!(include_str!("docs_script.py"), "\nRENDER_PROCESS_SOURCE = r'''",
    include_str!("../../../addin/AdomBridge/commands/mechanical/render_process.py"), "'''\n");
pub fn owns(cmd: &str) -> bool { matches!(cmd, "document_info" | "close_all_documents" | "render_board_3d" | "read_footprint" | "project_info" | "project_snapshot" | "project_rename" | "export_pdf") }
fn script(cmd: &str, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let s = format!("import json\nexec({})\nresult=handle(app,json.loads({}),{})\n",
        serde_json::to_string(SOURCE).unwrap(), serde_json::to_string(&args.to_string()).unwrap(), serde_json::to_string(cmd).unwrap());
    let r = pass("run_modeling_script", json!({"script": s}));
    if r["success"].as_bool() == Some(false) { return r; }
    r["data"]["result"].clone()
}
pub fn call(cmd: &str, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    match cmd {
        "read_footprint" => read_footprint(args, pass),
        "project_info" => project_info(args, pass),
        "project_snapshot" => project_snapshot(args, pass),
        "project_rename" => project_rename(args, pass),
        "export_pdf" => export_pdf(args, pass),
        "document_info" => {
            let mut old = pass("document_info", args.clone());
            let std_ = script(cmd, args, pass);
            if std_["success"].as_bool() == Some(true) {
                for k in ["name", "kind", "saved", "modified", "path", "version", "project"] { old[k] = std_[k].clone(); }
            }
            old
        }
        // no 3D PCB open: show it (the bridge does it, not the caller), render, then put the user's
        // tab back, since showing the 3D PCB brings it to the front
        "render_board_3d" => {
            let r = script(cmd, args, pass);
            if r["errorCode"] != "no_3d_board" { return r; }
            let front = script("front_tab", &json!({}), pass);
            let shown = crate::orchestrations::show_with_document("show_3d_board", &json!({"documentName": args["expectDocument"]}), pass);
            let mut r2 = script(cmd, args, pass);
            if let Some(i) = front["index"].as_i64() { let _ = script("switch_tab", &json!({"index": i}), pass); }
            r2["shown3d"] = json!(shown["success"].as_bool() == Some(true));
            r2
        }
        _ => script(cmd, args, pass),
    }
}

/// The standard's library_views in fusion_capture_library_views' terms: parts -> packages, view
/// device -> component (the capture opens the library itself when it is not open).
pub fn library_views_args(args: &Value) -> Value {
    let mut a = args.clone();
    if let Some(p) = args["parts"].as_array() { a["packages"] = json!(p); }
    if let Some(v) = args["views"].as_array() { a["views"] = json!(v.iter().map(|x| if x == "device" { json!("component") } else { x.clone() }).collect::<Vec<_>>()); }
    a
}

/// The standard's read_footprint: from a .lbr (`libraryPath` + `name`), or by `libId` "Lib:Name"
/// from the libraries the open board embeds (`expectDocument` names the board; Fusion keeps no
/// global library table, so a design's own copy is the one its parts use).
fn read_footprint(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let path = args["libraryPath"].as_str().or(args["filePath"].as_str()).unwrap_or("").replace('\\', "/");
    let lib_id = args["libId"].as_str().unwrap_or("");
    let (lib, name) = match lib_id.split_once(':') { Some((l, n)) => (Some(l.to_string()), n.to_string()), None => (None, args["name"].as_str().unwrap_or(lib_id).to_string()) };
    if name.is_empty() {
        return json!({"success": false, "errorCode": "bad_args", "error": "name (with libraryPath) or libId \"Lib:Name\" is required",
                      "_hint": "fusion_read_footprint {libraryPath:'C:/libs/my.lbr', name:'SOIC-8'} reads a .lbr; {libId:'Lib:Name', expectDocument:'<board>'} reads the library the open board embeds."});
    }
    let (xml, source) = if !path.is_empty() {
        match std::fs::read_to_string(&path) { Ok(t) => (t, json!({"libraryPath": path})), Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": e.to_string(), "libraryPath": path}) }
    } else {
        if args["expectDocument"].as_str().unwrap_or("").is_empty() {
            return json!({"success": false, "errorCode": "bad_args", "error": "libId without libraryPath reads the open board's libraries: expectDocument (the board's name) is required"});
        }
        let b = crate::routing::call("board_source", &json!({"expectDocument": args["expectDocument"]}), pass);
        match b["source"].as_str() { Some(t) => (t.to_string(), json!({"board": b["documentName"]})), None => return b }
    };
    match fusion_lbr::read_footprint(&xml, lib.as_deref(), &name) {
        Ok(mut v) => {
            let n = v["pads"].as_array().map(|a| a.len()).unwrap_or(0);
            v["success"] = json!(true);
            v["source"] = source;
            v["_hint"] = json!(format!("{n} pad(s). Pad numbers are the package's pad names; a device's connects map them to the symbol's pins (fusion_library_parts lists the devices). 3D models are named by their Fusion package3d and URN: Fusion places them itself, so no offset, scale or turn is stored in the library."));
            v
        }
        Err(e) => json!({"success": false, "errorCode": "footprint_not_found", "error": e.to_string()}),
    }
}

/// The standard's project_info for a Fusion electronics design: its documents (open ones and the
/// cloud files of the same name beside them, with version and date), the sheet tree, the board
/// (copper layers, parts, nets, size) and the net classes. Read only; the design must be open.
fn project_info(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut a = args.clone();
    if a["expectDocument"].is_null() && a["project"].is_string() { a["expectDocument"] = a["project"].clone(); }
    let files = script("project_files", &a, pass);
    if files["success"].as_bool() != Some(true) {
        let mut f = files;
        f["_hint"] = json!("Open the design first (fusion_open_cloud_file {fileName, projectName}); project_info reads what Fusion has open and the cloud files beside it.");
        return f;
    }
    let ea = json!({"expectDocument": files["name"]});
    let sheets = crate::sch::call("sch_hierarchy", &ea, pass);
    let st = crate::routing::call("routing_state", &ea, pass);
    let classes = crate::routing::call("net_classes", &ea, pass);
    let size = st["outline"].as_array().filter(|o| !o.is_empty()).map(|o| {
        let xs: Vec<f64> = o.iter().filter_map(|p| p[0].as_f64()).collect();
        let ys: Vec<f64> = o.iter().filter_map(|p| p[1].as_f64()).collect();
        let span = |v: &Vec<f64>| (v.iter().cloned().fold(f64::MIN, f64::max) - v.iter().cloned().fold(f64::MAX, f64::min));
        json!([(span(&xs) * 1000.0).round() / 1000.0, (span(&ys) * 1000.0).round() / 1000.0])
    });
    let board = if st["success"].as_bool() == Some(false) { json!({"error": st["error"]}) } else {
        json!({"layers": st["copperLayers"], "parts": st["components"].as_array().map(|c| c.len()), "nets": st["nets"].as_array().map(|n| n.len()), "sizeMm": size})
    };
    let names: Vec<Value> = classes["classes"].as_array().or(classes["netClasses"].as_array()).map(|c| c.iter().map(|x| x["name"].clone()).collect()).unwrap_or_default();
    json!({"success": true, "name": files["name"], "project": files["project"], "files": files["files"], "open": true, "modifiedAt": files["modifiedAt"],
           "sheets": if sheets["success"].as_bool() == Some(false) { json!({"error": sheets["error"]}) } else { sheets["tree"].clone() },
           "board": board, "netClasses": names, "textVariables": {},
           "_hint": "Fusion keeps a design as cloud files (project, schematic, board, 3D PCB) of one name; versions are cloud versions and dates are epoch seconds. Fusion has no text variables."})
}

/// The standard's project_snapshot for a Fusion design: a dated folder on the Fusion machine with
/// the design's schematic and board as EAGLE XML, a PNG of every sheet and of the board, and PDFs
/// made from them. The cloud keeps Fusion's own versions; this is the plain-file copy beside them.
fn project_snapshot(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut a = args.clone();
    if a["expectDocument"].is_null() && a["project"].is_string() { a["expectDocument"] = a["project"].clone(); }
    let files = script("project_files", &a, pass);
    if files["success"].as_bool() != Some(true) { return files; }
    let name = files["name"].as_str().unwrap_or("design").to_string();
    let safe = |t: &str| t.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect::<String>();
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H%M%S").to_string();
    let label = a["label"].as_str().filter(|l| !l.is_empty()).map(|l| format!("-{}", safe(l))).unwrap_or_default();
    let base = a["outputDir"].as_str().map(|s| s.replace('\\', "/")).unwrap_or_else(|| {
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".into()).replace('\\', "/");
        format!("{home}/Documents/Adom Snapshots/{}", safe(&name))
    });
    let folder = format!("{}/{stamp}{label}", base.trim_end_matches('/'));
    if let Err(e) = std::fs::create_dir_all(&folder) { return json!({"success": false, "errorCode": "write_failed", "error": format!("{folder}: {e}")}); }
    let front = script("front_tab", &json!({}), pass)["index"].clone();
    let mut written: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut board_png: Option<(String, f64)> = None;
    let tab_of = |kind: &str| files["files"].as_array().and_then(|f| f.iter().find(|r| r["kind"] == kind && r["open"] == true)).map(|r| r["tab"].clone());
    for (kind, ext) in [("schematic", "sch"), ("board", "brd")] {
        let Some(tab) = tab_of(kind) else { notes.push(format!("no {kind} open, so no .{ext}")); continue };
        let _ = script("switch_tab", &json!({"index": tab}), pass);
        let out = format!("{folder}/{}.{ext}", safe(&name));
        let r = pass("export_eagle_source", json!({"outputPath": out}));
        if r["success"].as_bool() == Some(true) { written.push(out); } else { notes.push(format!("{ext}: {}", r["error"].as_str().unwrap_or("export failed"))); }
        if kind == "board" {
            let png = format!("{folder}/{}-board.png", safe(&name));
            let r = pass("export_board_image", json!({"outputPath": png, "preset": "all", "dpi": 300}));
            if r["success"].as_bool() == Some(true) { written.push(png.clone()); board_png = Some((png, r["dpi"].as_f64().unwrap_or(300.0))); } else { notes.push(format!("board image: {}", r["error"].as_str().unwrap_or("failed"))); }
        }
    }
    if !front.is_null() { let _ = script("switch_tab", &json!({"index": front}), pass); }
    let mut sheet_pngs: Vec<(String, f64)> = Vec::new();
    if tab_of("schematic").is_some() {
        let tree = crate::sch::call("sch_hierarchy", &json!({"expectDocument": name}), pass);
        let n = tree["tree"]["sheets"].as_array().map(|s| s.len()).unwrap_or(1).max(1);
        for k in 1..=n {
            let r = crate::sch::call("render_schematic", &json!({"expectDocument": name, "sheet": k, "outputDir": folder, "name": format!("{}-sheet{k}", safe(&name)), "width": 3000}), pass);
            match r["png"].as_str() { Some(p) => { written.push(p.to_string()); sheet_pngs.push((p.to_string(), r["dpi"].as_f64().unwrap_or(150.0))); }, None => notes.push(format!("sheet {k}: {}", r["error"].as_str().unwrap_or("render failed"))) }
        }
    }
    let mut made: Vec<String> = Vec::new();
    if a["pdfs"].as_bool() != Some(false) {
        if !sheet_pngs.is_empty() {
            let p = format!("{folder}/{}-schematic.pdf", safe(&name));
            match crate::pdf::images_to_pdf(&sheet_pngs, &p) { Ok(_) => { made.push(p.clone()); written.push(p); }, Err(e) => notes.push(format!("schematic pdf: {e}")) }
        }
        if let Some(bp) = &board_png {
            let p = format!("{folder}/{}-board.pdf", safe(&name));
            match crate::pdf::images_to_pdf(std::slice::from_ref(bp), &p) { Ok(_) => { made.push(p.clone()); written.push(p); }, Err(e) => notes.push(format!("board pdf: {e}")) }
        }
    }
    json!({"success": !written.is_empty(), "folder": folder, "files": written, "pdfs": {"made": made, "skipped": if a["pdfs"].as_bool() == Some(false) { json!("pdfs:false") } else { json!(Value::Null) }},
           "notes": notes, "design": name,
           "_hint": "A plain-file copy of the design beside Fusion's own cloud versions: EAGLE XML of the schematic and board, a PNG per sheet and of the board, and PDFs of them. Save a cloud version with fusion_save_document."})
}

/// The standard's project_rename for a design kept as EAGLE files: <folder>/<name>.sch and .brd
/// (and EAGLE's numbered backups) become <newName>.*, and a folder named after the design is
/// renamed with them. EAGLE files do not name each other, so nothing inside changes. Refused while
/// Fusion has the design open. A Fusion cloud design is renamed in Fusion's data panel, not here.
fn project_rename(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let project = args["project"].as_str().unwrap_or("").replace('\\', "/");
    let new_name = args["newName"].as_str().unwrap_or("").trim().to_string();
    if project.is_empty() || new_name.is_empty() { return json!({"success": false, "errorCode": "bad_args", "error": "project (a .sch, .brd or the folder holding them) and newName are required"}); }
    if new_name.chars().any(|c| "/\\:*?\"<>|".contains(c)) || new_name.len() > 64 { return json!({"success": false, "errorCode": "bad_args", "error": "newName: no path characters, 64 at most"}); }
    let p = std::path::Path::new(&project);
    if !p.exists() {
        return json!({"success": false, "errorCode": "not_supported", "error": format!("{project} is not a file or folder on the Fusion machine"),
                      "_hint": "A Fusion cloud design is renamed in Fusion's data panel (its project, schematic, board and 3D PCB together). project_rename renames a design kept as EAGLE files."});
    }
    let (dir, old) = if p.is_dir() {
        let stem = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        (p.to_path_buf(), stem)
    } else {
        (p.parent().map(|d| d.to_path_buf()).unwrap_or_default(), p.file_stem().map(|f| f.to_string_lossy().to_string()).unwrap_or_default())
    };
    let open = script("project_files", &json!({"expectDocument": old}), pass);
    if open["success"].as_bool() == Some(true) {
        return json!({"success": false, "errorCode": "open_in_fusion", "error": format!("Fusion has the design {old} open; close it first (fusion_close_document)")});
    }
    let mut renamed: Vec<Value> = Vec::new();
    let mut plan: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().to_string();
            let Some(rest) = fname.strip_prefix(&old) else { continue };
            // name.sch, name.brd, name.lbr and EAGLE's backups name.s#1 / name.b#2
            let ok = rest.starts_with('.') && regex::Regex::new(r"^\.(sch|brd|lbr|s#\d+|b#\d+|l#\d+|pro|epf)$").unwrap().is_match(&rest.to_lowercase());
            if ok { plan.push((e.path(), dir.join(format!("{new_name}{rest}")))); }
        }
    }
    if plan.is_empty() { return json!({"success": false, "errorCode": "nothing_to_rename", "error": format!("no {old}.sch or {old}.brd in {}", dir.display())}); }
    if let Some((_, to)) = plan.iter().find(|(_, to)| to.exists()) { return json!({"success": false, "errorCode": "exists", "error": format!("{} already exists", to.display())}); }
    let folder_too = dir.file_name().map(|f| f.to_string_lossy() == old).unwrap_or(false);
    let new_dir = dir.parent().map(|d| d.join(&new_name)).unwrap_or_else(|| dir.clone());
    if folder_too && new_dir.exists() { return json!({"success": false, "errorCode": "exists", "error": format!("{} already exists", new_dir.display())}); }
    for (a, b) in &plan { renamed.push(json!({"from": a.to_string_lossy().replace('\\', "/"), "to": b.to_string_lossy().replace('\\', "/")})); }
    if folder_too { renamed.push(json!({"from": dir.to_string_lossy().replace('\\', "/"), "to": new_dir.to_string_lossy().replace('\\', "/")})); }
    if args["dryRun"].as_bool() == Some(true) { return json!({"success": true, "mutated": false, "renamed": renamed, "changed": []}); }
    for (a, b) in &plan { if let Err(e) = std::fs::rename(a, b) { return json!({"success": false, "errorCode": "rename_failed", "error": format!("{}: {e}", a.display()), "renamed": renamed}); } }
    if folder_too { if let Err(e) = std::fs::rename(&dir, &new_dir) { return json!({"success": false, "errorCode": "rename_failed", "error": format!("{}: {e}", dir.display())}); } }
    json!({"success": true, "mutated": true, "renamed": renamed, "changed": [],
           "_hint": "EAGLE's schematic and board find each other by file name in one folder, so nothing inside the files needed changing. Open it again with fusion_new_electronics_from_eagle."})
}

/// The standard's export_pdf: every sheet of the design as one PDF (rendered with Fusion's own
/// EXPORT IMAGE, one page per sheet), and with board:true the board as a second PDF. Fusion has no
/// assembly or fabrication print outputs, so those are refused by name.
fn export_pdf(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    if args["assembly"].as_bool() == Some(true) || args["fabrication"].as_bool() == Some(true) {
        return json!({"success": false, "errorCode": "not_supported", "error": "Fusion has no assembly-drawing or fabrication-print output; ask for sheets and board (the composite) only"});
    }
    let mut a = args.clone();
    if a["expectDocument"].is_null() && a["projectPath"].is_string() { a["expectDocument"] = a["projectPath"].clone(); }
    let files = script("project_files", &a, pass);
    if files["success"].as_bool() != Some(true) { return files; }
    let name = files["name"].as_str().unwrap_or("design").to_string();
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    let out = a["output"].as_str().map(|s| s.replace('\\', "/")).unwrap_or_else(|| {
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".into()).replace('\\', "/");
        format!("{home}/Documents/{safe}.pdf")
    });
    let dir = std::path::Path::new(&out).parent().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|| ".".into());
    let _ = std::fs::create_dir_all(&dir);
    let tree = crate::sch::call("sch_hierarchy", &json!({"expectDocument": name}), pass);
    if tree["success"].as_bool() == Some(false) { return tree; }
    let n = tree["tree"]["sheets"].as_array().map(|s| s.len()).unwrap_or(1).max(1);
    let mut pages: Vec<(String, f64)> = Vec::new();
    for k in 1..=n {
        let r = crate::sch::call("render_schematic", &json!({"expectDocument": name, "sheet": k, "outputDir": dir, "name": format!("{safe}-pdf-sheet{k}"), "width": 3000}), pass);
        match r["png"].as_str() { Some(p) => pages.push((p.to_string(), r["dpi"].as_f64().unwrap_or(150.0))), None => return json!({"success": false, "error": format!("sheet {k}: {}", r["error"].as_str().unwrap_or("render failed"))}) }
    }
    if let Err(e) = crate::pdf::images_to_pdf(&pages, &out) { return json!({"success": false, "errorCode": "write_failed", "error": e}); }
    let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    for (p, _) in &pages { let _ = std::fs::remove_file(p); }
    let mut board_pdf = Value::Null;
    if a["board"].as_bool() == Some(true) {
        let front = script("front_tab", &json!({}), pass)["index"].clone();
        let tab = files["files"].as_array().and_then(|f| f.iter().find(|r| r["kind"] == "board" && r["open"] == true)).map(|r| r["tab"].clone());
        if let Some(tab) = tab {
            let _ = script("switch_tab", &json!({"index": tab}), pass);
            let png = format!("{dir}/{safe}-board.png");
            let r = pass("export_board_image", json!({"outputPath": png, "preset": "all", "dpi": 300}));
            if !front.is_null() { let _ = script("switch_tab", &json!({"index": front}), pass); }
            if r["success"].as_bool() == Some(true) {
                let bp = out.trim_end_matches(".pdf").to_string() + "-board.pdf";
                if crate::pdf::images_to_pdf(&[(png.clone(), r["dpi"].as_f64().unwrap_or(300.0))], &bp).is_ok() { board_pdf = json!(bp); }
                let _ = std::fs::remove_file(&png);
            }
        }
    }
    json!({"success": true, "pdfPath": out, "sheets": n, "outputBytes": bytes, "board": !board_pdf.is_null(), "boardPdf": board_pdf, "assembly": false, "fabrication": false,
           "_hint": "One page per sheet, rendered by Fusion's own EXPORT IMAGE at about 3000 px wide; board:true adds the board (all layers) as a second PDF."})
}

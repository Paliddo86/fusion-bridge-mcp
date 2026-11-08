//! The service-backed orchestrations, native: the board stackup read, the optimized-GLB export
//! through service-step2glb (sync, fetch, and the detached form), and the design-rule packs
//! from the wiki page adom/pcb-design-rules. Every Fusion hop goes through the pass-through.

use serde_json::{json, Value};
use std::time::{Duration, Instant};

use crate::addin::Ctx;

fn now_f() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }
fn step2glb_url() -> String { std::env::var("ADOM_STEP2GLB_URL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "https://step2glb-gmdoncpxdwx0.adom.cloud".into()).trim_end_matches('/').to_string() }
fn service_ua() -> String { format!("adom-fusion-bridge/{}", crate::VERSION) }
const DESIGN_RULES_PAGE: &str = "https://wiki.adom.inc/api/v1/pages/adom/pcb-design-rules/files";

/// `fusion_board_stackup`: copper count and thicknesses from the .brd design rules, FR4 from the 3D body.
pub fn board_stackup(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let _ = args;
    let _ = pass("show_2d_board", json!({}));
    let brd = std::env::temp_dir().join("adom_stackup.brd").to_string_lossy().replace('\\', "/");
    let r = pass("export_eagle_source", json!({"outputPath": brd}));
    if r["success"].as_bool() != Some(true) { return json!({"success": false, "error": format!("could not export .brd for stackup: {}", r["error"].as_str().or(r["message"].as_str()).unwrap_or("None")), "_hint": "Open the PROJECT and switch to the board (fusion_show_2d_board) first."}); }
    let txt = match std::fs::read(&brd) { Ok(b) => b.iter().map(|&c| c as char).collect::<String>(), Err(e) => return json!({"success": false, "error": format!("could not read .brd: {e}")}) };
    let p = |name: &str| regex::Regex::new(&format!(r#"<param name="{name}" value="([^"]*)""#)).unwrap().captures(&txt).map(|c| c[1].to_string());
    let layer_setup = p("layerSetup").unwrap_or_default();
    let mt_copper: Vec<String> = p("mtCopper").unwrap_or_default().split_whitespace().map(str::to_string).collect();
    let mt_isolate: Vec<String> = p("mtIsolate").unwrap_or_default().split_whitespace().map(str::to_string).collect();
    let copper_layers: Vec<String> = regex::Regex::new(r"\d+").unwrap().find_iter(&layer_setup).map(|m| m.as_str().to_string()).collect();
    let ncu = copper_layers.len();
    let bonds: Vec<&str> = layer_setup.chars().filter(|c| *c == '+' || *c == '*').map(|c| if c == '*' { "core" } else { "prepreg" }).collect();
    let _ = pass("show_3d_board", json!({}));
    let mm = pass("run_modeling_script", json!({"script": "import adsk.core, adsk.fusion\napp=adsk.core.Application.get(); des=adsk.fusion.Design.cast(app.activeProduct)\nbest=None; ba=-1.0\nfor occ in des.rootComponent.allOccurrences:\n for b in occ.component.bRepBodies:\n  if b.name.lower()=='board':\n   bb=b.boundingBox; mn=bb.minPoint; mx=bb.maxPoint\n   a=(mx.x-mn.x)*(mx.y-mn.y)\n   if a>ba: ba=a; best=((mx.x-mn.x)*10,(mx.y-mn.y)*10,(mx.z-mn.z)*10)\nprint('FR4 %.4f %.4f %.4f'%best if best else 'FR4 none')"}));
    let msg: String = match mm["output"].as_str() { Some(o) if o.starts_with('{') => serde_json::from_str::<Value>(o).ok().and_then(|v| v["message"].as_str().map(str::to_string)).unwrap_or_default(), Some(o) => mm["message"].as_str().unwrap_or(o).to_string(), None => mm["message"].as_str().unwrap_or("").to_string() };
    let fr4 = regex::Regex::new(r"FR4\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)").unwrap().captures(&msg).map(|c| json!({"x_mm": c[1].parse::<f64>().unwrap_or(0.0), "y_mm": c[2].parse::<f64>().unwrap_or(0.0), "dielectric_mm": c[3].parse::<f64>().unwrap_or(0.0)}));
    let num = |s: &str| s.replace("mm", "").parse::<f64>().ok();
    json!({"success": true, "layerSetup": layer_setup, "copperLayers": copper_layers, "copperCount": ncu,
        "copperThickness_mm": if mt_copper.is_empty() { Value::Null } else { json!(mt_copper.iter().take(ncu).map(|t| num(t)).collect::<Vec<_>>()) },
        "dielectricBonds": bonds, "dielectricThickness_raw": mt_isolate, "fr4": fr4, "brdPath": brd,
        "_hint": "Physical stack (see pcb-stackup skill): the FR4 is NOT one slab - it is prepreg/core/prepreg with copper between. Build the table + exploded view per that skill. If mtIsolate looks like 2-layer defaults, fit a symmetric split to fr4.dielectric_mm."})
}

fn multipart(files: &[(&str, String, Vec<u8>, &str)]) -> (Vec<u8>, String) {
    let boundary = format!("----AdomBridgeGLB{}{}", (now_f() * 1000.0) as u64, std::process::id());
    let mut out: Vec<u8> = Vec::new();
    for (field, filename, data, ctype) in files {
        out.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{filename}\"\r\nContent-Type: {ctype}\r\n\r\n").as_bytes());
        out.extend_from_slice(data); out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (out, boundary)
}

/// The add-in's silk capture is a square 2048 px render with margin around the board, and the board body stays
/// visible as a backdrop (Fusion draws nothing on a canvas without it). The optimizer maps the WHOLE image onto
/// the board, so an unprepared capture bakes in offset, shrunken labels on an opaque board-coloured sheet
/// (fusion-bridge#113). Crop to the board (the opaque part of the transparent-background capture) and keep only
/// the near-white silk strokes, transparent everywhere else. Returns None when the image cannot be read.
pub(crate) fn clean_silk_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut dec = png::Decoder::new(bytes);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut rd = dec.read_info().ok()?;
    let mut buf = vec![0u8; rd.output_buffer_size()];
    let info = rd.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = match info.color_type { png::ColorType::Rgba => 4, png::ColorType::Rgb => 3, png::ColorType::GrayscaleAlpha => 2, png::ColorType::Grayscale => 1, _ => return None };
    let px = |x: usize, y: usize| -> (f32, f32, f32, f32) {
        let i = (y * w + x) * ch;
        match ch { 4 => (buf[i] as f32, buf[i + 1] as f32, buf[i + 2] as f32, buf[i + 3] as f32), 3 => (buf[i] as f32, buf[i + 1] as f32, buf[i + 2] as f32, 255.0),
                   2 => (buf[i] as f32, buf[i] as f32, buf[i] as f32, buf[i + 1] as f32), _ => (buf[i] as f32, buf[i] as f32, buf[i] as f32, 255.0) }
    };
    // the board: bounding box of the opaque pixels (an opaque capture has no margin to crop, so it stays whole)
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for y in 0..h { for x in 0..w { if px(x, y).3 > 8.0 { x0 = x0.min(x); y0 = y0.min(y); x1 = x1.max(x); y1 = y1.max(y); } } }
    if x0 > x1 || y0 > y1 { return None; }
    let (cw, chh) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut out = Vec::with_capacity(cw * chh * 4);
    for y in y0..=y1 { for x in x0..=x1 {
        let (r, g, b, a) = px(x, y);
        let lum = 0.299 * r + 0.587 * g + 0.114 * b;               // silk is near white; board fill, pads and holes are dark
        let alpha = ((lum - 150.0) / 70.0).clamp(0.0, 1.0) * (a / 255.0);
        out.extend_from_slice(&[245, 245, 245, (alpha * 255.0).round() as u8]);
    } }
    let mut png_out = Vec::new();
    { let mut enc = png::Encoder::new(&mut png_out, cw as u32, chh as u32); enc.set_color(png::ColorType::Rgba); enc.set_depth(png::BitDepth::Eight);
      let mut wr = enc.write_header().ok()?; wr.write_image_data(&out).ok()?; }
    Some(png_out)
}

fn glb_submit(step_path: &str, board: Option<&str>, silk_top: Option<&str>, silk_bottom: Option<&str>, pin: &str, compress: Option<&str>, job_name: &str) -> Value {
    let Ok(step) = std::fs::read(step_path) else { return json!({"ok": false, "error": format!("could not read STEP: {step_path}")}) };
    let mut files: Vec<(&str, String, Vec<u8>, &str)> = vec![("step", std::path::Path::new(step_path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(), step, "application/step")];
    // The board file (EAGLE .brd) is what lets the service generate the footprint + symbol JSON with per-pad
    // wire-attach Z; the service sniffs .brd vs .kicad_pcb from the content (same contract as `step2glb --board`).
    if let Some(p) = board { if let Ok(b) = std::fs::read(p) { files.push(("board", std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(), b, "application/octet-stream")); } }
    for (field, p) in [("silk_top", silk_top), ("silk_bottom", silk_bottom)] { if let Some(p) = p { if let Ok(raw) = std::fs::read(p) { let b = clean_silk_png(&raw).unwrap_or(raw); files.push((field, std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(), b, "image/png")); } } }
    let (body, boundary) = multipart(&files);
    let mut url = format!("{}/convert?molecule=true&pin={}", step2glb_url(), fusion_core::aps::urlencode(pin));
    // the service's override (quantize | draco | none); omitted, a molecule is Draco-compressed
    if let Some(c) = compress { url.push_str(&format!("&compress={}", fusion_core::aps::urlencode(c))); }
    let resp = ureq::post(&url).timeout(Duration::from_secs(90)).set("Content-Type", &format!("multipart/form-data; boundary={boundary}")).set("X-Client", "fusion-bridge/adom").set("X-Job-Name", job_name).set("User-Agent", &service_ua()).send_bytes(&body);
    let queued: Value = match resp { Ok(r) => r.into_json().unwrap_or(json!({})), Err(e) => return json!({"ok": false, "error": format!("service POST failed: {e}"), "_hint": format!("Is {} reachable from this box? Override with ADOM_STEP2GLB_URL.", step2glb_url())}) };
    match queued["job_id"].as_str() { Some(j) => json!({"ok": true, "jobId": j}), None => json!({"ok": false, "error": "service did not return a job_id", "raw": queued}) }
}

fn glb_fetch(job_id: &str, out_path: Option<&str>, poll_s: u64) -> Value {
    let ua = service_ua();
    let deadline = Instant::now() + Duration::from_secs(poll_s);
    let mut stats = json!({});
    let mut first = true;
    while first || Instant::now() < deadline {
        first = false;
        match ureq::get(&format!("{}/jobs/{job_id}", step2glb_url())).timeout(Duration::from_secs(20)).set("User-Agent", &ua).call().ok().and_then(|r| r.into_json::<Value>().ok()) { Some(s) => stats = s, None => { std::thread::sleep(Duration::from_secs(4)); continue; } }
        match stats["status"].as_str() { Some("complete") => break, Some("error") => return json!({"ok": false, "status": "error", "error": if stats["error"].is_null() { stats.clone() } else { stats["error"].clone() }, "stats": stats}), _ => {} }
        if Instant::now() >= deadline { return json!({"ok": true, "status": stats["status"].as_str().unwrap_or("processing"), "stats": stats}); }
        std::thread::sleep(Duration::from_secs(6));
    }
    if stats["status"].as_str() != Some("complete") { return json!({"ok": true, "status": stats["status"].as_str().unwrap_or("processing"), "stats": stats}); }
    let (code, glb) = fusion_core::aps::http_bytes(&format!("{}/jobs/{job_id}/result", step2glb_url()));
    if code != 200 { return json!({"ok": false, "status": "complete", "error": format!("download failed: HTTP {code}"), "stats": stats}); }
    let mut wrote = Value::Null;
    let mut side = json!({});
    if let Some(p) = out_path {
        match std::fs::write(p, &glb) { Ok(_) => wrote = json!(p), Err(e) => return json!({"ok": false, "status": "complete", "error": format!("could not write GLB: {e}"), "stats": stats}) }
        side = fetch_side_artifacts(job_id, p);
    }
    json!({"ok": true, "status": "complete", "stats": stats, "glbBytes": glb.len(), "wrote": wrote, "side": side})
}

/// GET /jobs/{id}/footprint, /symbol (JSON, only when a board was attached) and /weblod (a decimated
/// `<stem>.web.glb` twin, only when the model is over the wiki's inline budget), written beside the GLB the
/// same way the step2glb CLI does. A 404 means the job produced none, which is not an error.
fn fetch_side_artifacts(job_id: &str, glb_path: &str) -> Value {
    let stem = &glb_path[..glb_path.len().saturating_sub(4)];
    let mut out = json!({"footprintPath": null, "symbolPath": null, "webGlbPath": null});
    for (kind, key, path) in [("footprint", "footprintPath", format!("{stem}_footprint.json")), ("symbol", "symbolPath", format!("{stem}_symbol.json")), ("weblod", "webGlbPath", format!("{stem}.web.glb"))] {
        let (code, bytes) = fusion_core::aps::http_bytes(&format!("{}/jobs/{job_id}/{kind}", step2glb_url()));
        if code == 200 && !bytes.is_empty() && std::fs::write(&path, &bytes).is_ok() { out[key] = json!(path); }
    }
    out
}

/// The molecule-publish gate stats from the job status, under the names the rest of the reply uses. A gate
/// that is not met is a stop sign for publishing (see the adom-step2glb skill), so the hint says which.
fn molecule_gates(stats: &Value, board_sent: bool) -> (Value, Option<String>) {
    let g = json!({"pinSize": stats["pin_size"], "pinSizeSource": stats["pin_size_source"], "footprintApplied": stats["footprint_applied"],
        "footprintPins": stats["footprint_pins"], "footprintRebase": stats["footprint_rebase"], "contactZ": stats["contact_z"], "warnings": stats["warnings"]});
    let mut stop: Vec<String> = Vec::new();
    if stats["molecule_anchored"].as_bool() != Some(true) { stop.push("molecule_anchored is not true: the MP1-MP4 markers were not found in the STEP".into()); }
    if board_sent && stats["footprint_applied"].as_bool() != Some(true) { stop.push("footprint_applied is not true: the service could not use the .brd (read warnings)".into()); }
    if !board_sent { stop.push("no board file was sent, so there is no footprint/symbol JSON; this GLB is not publishable as a molecule".into()); }
    if stats["warnings"].as_array().map(|w| !w.is_empty()).unwrap_or(false) { stop.push("the service returned warnings; each one is actionable".into()); }
    (g, if stop.is_empty() { None } else { Some(format!("Publish gates NOT met: {}.", stop.join("; "))) })
}


fn sha256_hex(bytes: &[u8]) -> String { use sha2::{Digest, Sha256}; Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect() }

/// The role of one CAM output, from the extension the bridge's bundled JLCPCB CAM jobs write. Fusion's CAM
/// processor writes RS-274X without X2 file attributes and its .gbrjob carries no FilesAttributes, so the
/// extension is the only reliable signal. Fusion cannot split plated and unplated holes, so the one Excellon
/// file is `drill_mixed` and normalize decides plating from the copper around each hole.
fn fab_role(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_uppercase();
    match ext.as_str() {
        "GTL" => "copper_top", "GBL" => "copper_bottom", "G1" => "copper_inner_1", "G2" => "copper_inner_2",
        "GTS" => "mask_top", "GBS" => "mask_bottom", "GTO" => "silk_top", "GBO" => "silk_bottom",
        "GTP" => "paste_top", "GBP" => "paste_bottom", "GKO" => "outline", "XLN" | "DRL" => "drill_mixed",
        "GBRJOB" => "gerber_job", _ => "other",
    }
}

/// Every entry of the fab zip with its sha256 and role, plus the parsed Gerber job file.
fn read_fab_zip(path: &str) -> Result<(Vec<Value>, Value), String> {
    let f = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("read zip {path}: {e}"))?;
    let (mut entries, mut job) = (Vec::new(), Value::Null);
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| e.to_string())?;
        if e.is_dir() { continue; }
        let name = e.name().to_string();
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut e, &mut buf).map_err(|e| e.to_string())?;
        let role = fab_role(&name);
        if role == "gerber_job" { job = serde_json::from_slice(&buf).unwrap_or(Value::Null); }
        let format = match role { "drill_mixed" => "excellon", "gerber_job" => "gerber_job_json", "other" => "unknown", _ => "gerber_rs274x" };
        entries.push(json!({"path": name, "role": role, "format": format, "bytes": buf.len(), "sha256": sha256_hex(&buf)}));
    }
    Ok((entries, job))
}

fn file_entry(path: &str, role: &str) -> Option<Value> {
    let b = std::fs::read(path).ok()?;
    let name = std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    Some(json!({"path": name, "role": role, "bytes": b.len(), "sha256": sha256_hex(&b)}))
}

/// `export.json`, the export record the board-look pipeline reads (board-look EXPORTERS.md, Aditya 2026-10-08):
/// tool and version, exporter version, every source file written with its sha256 and role (each Gerber and drill
/// file inside the fab zip too), and the stackup the tool knows. Nothing in the STEP or the Gerbers is renamed,
/// so the hashes describe the design as Fusion wrote it.
fn write_export_record(dir: &std::path::Path, design: &str, sources: &[Option<Value>], fab: Option<(&str, &[Value], &Value)>, gerbers_note: Option<&str>) -> Result<String, String> {
    let mut files: Vec<Value> = sources.iter().flatten().cloned().collect();
    let (mut tool_version, mut stackup) = (Value::Null, Value::Null);
    if let Some((zip_path, entries, job)) = fab {
        if let Some(mut z) = file_entry(zip_path, "fab_zip") { z["entries"] = json!(entries); files.push(z); }
        let gs = &job["Header"]["GenerationSoftware"];
        if gs.is_object() { tool_version = json!(format!("{} {}", gs["Application"].as_str().unwrap_or(""), gs["Version"].as_str().unwrap_or("")).trim().to_string()); }
        stackup = json!({"thicknessMm": job["Overall"]["BoardThickness"], "copperLayers": job["Overall"]["LayerNumber"], "sizeMm": job["Overall"]["Size"],
            "maskColour": {"top": null, "bottom": null}, "silkColour": {"top": null, "bottom": null}, "finish": null,
            "source": "gerber_job.gbrjob (Fusion does not export mask, silk colour or finish)"});
    }
    let rec = json!({"schema": "adom.export/1", "tool": "Autodesk Fusion", "toolVersion": tool_version,
        "exporter": "adom/fusion-bridge fusion_export_optimized_glb", "exporterVersion": crate::VERSION,
        "exportedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true), "design": design,
        "drill": {"plating": "mixed", "note": "Fusion's CAM processor writes one Excellon file for plated and unplated holes."},
        "gerbers": gerbers_note.map(|n| json!({"included": false, "reason": n})).unwrap_or(json!({"included": true, "format": "RS-274X (no X2 attributes)", "camJob": "bundled jlcpcb-2layers/4layers.cam"})),
        "stackup": stackup, "files": files});
    let p = dir.join("export.json");
    std::fs::write(&p, serde_json::to_string_pretty(&rec).unwrap_or_default()).map_err(|e| format!("write export.json: {e}"))?;
    Ok(p.to_string_lossy().replace('\\', "/"))
}

/// `fusion_export_optimized_glb` (the synchronous form).
pub fn export_optimized_glb(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let Some(mut out) = args["outputPath"].as_str().or(args["output_path"].as_str()).map(str::to_string).filter(|s| !s.is_empty()) else { return json!({"success": false, "error": "No outputPath specified.", "_hint": "Usage: fusion_export_optimized_glb {\"outputPath\":\"C:/tmp/board.glb\", \"silkscreen\":true, \"pin\":\"medium\"}"}) };
    if !out.to_lowercase().ends_with(".glb") { out.push_str(".glb"); }
    let pin = args["pin"].as_str().unwrap_or("medium").to_string();
    let want_silk = args["silkscreen"].as_bool().unwrap_or(true);
    let want_board = args["board"].as_bool().unwrap_or(true);
    let want_gerbers = args["gerbers"].as_bool().unwrap_or(true);
    let compress = args["compress"].as_str().filter(|c| !c.is_empty()).map(str::to_lowercase);
    if let Some(c) = &compress { if !matches!(c.as_str(), "draco" | "quantize" | "none") { return json!({"success": false, "errorCode": "bad_args", "error": format!("compress is draco, quantize or none, not {c}"), "_hint": "compress:\"none\" gives a plain GLB that wiki README and widget frames can load (they cannot reach a Draco decoder)."}); } }
    let _ = pass("show_3d_board", json!({}));
    let step_path = format!("{}.step", &out[..out.len() - 4]);
    let sr = pass("export_step", json!({"outputPath": step_path}));
    if sr["success"].as_bool() != Some(true) { return json!({"success": false, "error": format!("STEP export failed: {}", sr["error"].as_str().or(sr["message"].as_str()).unwrap_or("None")), "step": sr, "_hint": "The active design must be a 3D Design product (fusion_show_3d_board first)."}); }
    let (mut silk_top, mut silk_bottom, mut silk_note): (Option<String>, Option<String>, Option<&str>) = (None, None, None);
    if want_silk {
        let st = format!("{}_silk_top.png", &out[..out.len() - 4]); let sb = format!("{}_silk_bottom.png", &out[..out.len() - 4]);
        if pass("take_silkscreen_screenshot", json!({"outputPath": st, "layer": "top"}))["success"].as_bool() == Some(true) { silk_top = Some(st); }
        if pass("take_silkscreen_screenshot", json!({"outputPath": sb, "layer": "bottom"}))["success"].as_bool() == Some(true) { silk_bottom = Some(sb); }
        if silk_top.is_none() && silk_bottom.is_none() { silk_note = Some("silkscreen capture unavailable on this design; GLB built without a baked silk texture."); }
    }
    // The EAGLE .brd from the 2D board, then back to the 3D board so the caller's view is unchanged.
    let (mut board_path, mut board_note): (Option<String>, Option<String>) = (None, None);
    if want_board {
        let bp = format!("{}.brd", &out[..out.len() - 4]);
        let _ = pass("show_2d_board", json!({}));
        let br = pass("export_eagle_source", json!({"outputPath": bp}));
        if br["success"].as_bool() == Some(true) && std::path::Path::new(&bp).exists() { board_path = Some(bp); } else { board_note = Some(format!("board (.brd) export failed, so no footprint/symbol JSON: {}", br["error"].as_str().or(br["message"].as_str()).unwrap_or("None"))); }
    }
    // Gerber + drill from the CAM job shipped with the bridge (not the user's), as <stem>-fab.zip, on every
    // export by default (Decision 6, board-look 3D_PIPELINE.md). Needs the 2D board, like the .brd.
    let (mut fab_path, mut fab_entries, mut fab_job, mut gerbers_note): (Option<String>, Vec<Value>, Value, Option<String>) = (None, Vec::new(), Value::Null, None);
    if want_gerbers {
        if !want_board { let _ = pass("show_2d_board", json!({})); }
        let tmp = std::env::temp_dir().join(format!("adom-fab-{}", (now_f() * 1000.0) as u64)).to_string_lossy().replace('\\', "/");
        let gr = pass("export_gerbers", json!({"outputDir": tmp}));
        let src = gr["data"]["zipPath"].as_str().or(gr["zipPath"].as_str()).map(str::to_string);
        let dst = format!("{}-fab.zip", &out[..out.len() - 4]);
        match src.filter(|_| gr["success"].as_bool() == Some(true)).map(|s| std::fs::copy(&s, &dst).map(|_| s)) {
            Some(Ok(s)) => {
                let _ = std::fs::remove_file(&s); let _ = std::fs::remove_dir(&tmp);
                match read_fab_zip(&dst) { Ok((e, j)) => { fab_entries = e; fab_job = j; fab_path = Some(dst); } Err(e) => gerbers_note = Some(format!("fab zip written but unreadable: {e}")) }
            }
            Some(Err(e)) => gerbers_note = Some(format!("could not copy the fab zip: {e}")),
            None => gerbers_note = Some(format!("Gerber export failed: {}", gr["error"].as_str().or(gr["message"].as_str()).unwrap_or("None"))),
        }
    } else { gerbers_note = Some("gerbers:false was passed".into()); }
    if want_board || want_gerbers { let _ = pass("show_3d_board", json!({})); }
    // The export record covers the SOURCE files this verb wrote (the GLB and footprint JSON are derived).
    let out_dir = std::path::Path::new(&out).parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let design = std::path::Path::new(&out).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let sources = [file_entry(&step_path, "step"), board_path.as_deref().and_then(|p| file_entry(p, "board_eagle_brd")),
        silk_top.as_deref().and_then(|p| file_entry(p, "silk_png_top")), silk_bottom.as_deref().and_then(|p| file_entry(p, "silk_png_bottom"))];
    let (export_record, record_note) = match write_export_record(&out_dir, &design, &sources, fab_path.as_deref().map(|p| (p, fab_entries.as_slice(), &fab_job)), gerbers_note.as_deref()) { Ok(p) => (Some(p), None), Err(e) => (None, Some(e)) };
    let job_name = std::path::Path::new(&out).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let sub = glb_submit(&step_path, board_path.as_deref(), silk_top.as_deref(), silk_bottom.as_deref(), &pin, compress.as_deref(), &job_name);
    if sub["ok"].as_bool() != Some(true) { return json!({"success": false, "error": sub["error"], "_hint": sub["_hint"], "stepPath": step_path, "note": "STEP exported OK; optimize submit failed."}); }
    let job_id = sub["jobId"].as_str().unwrap_or("").to_string();
    let wait_s = args["wait"].as_u64().unwrap_or(15);
    let fetched = if wait_s > 0 { glb_fetch(&job_id, Some(&out), wait_s) } else { json!({"ok": true, "status": "processing"}) };
    let (status_url, result_url) = (format!("{}/jobs/{job_id}", step2glb_url()), format!("{}/jobs/{job_id}/result", step2glb_url()));
    if fetched["status"].as_str() == Some("complete") && !fetched["wrote"].is_null() {
        let stats = &fetched["stats"]; let size = fetched["glbBytes"].as_u64().unwrap_or(0);
        let (gates, stop) = molecule_gates(stats, board_path.is_some());
        let mut r = json!({"success": true, "status": "complete", "glbPath": out, "stepPath": step_path, "boardPath": board_path, "jobId": job_id, "silkTop": silk_top, "silkBottom": silk_bottom, "meshesBefore": stats["meshes_before"], "meshesAfter": stats["meshes_after"], "sizeBytes": size, "moleculeAnchored": stats["molecule_anchored"], "silkscreenApplied": stats["silkscreen_applied"], "gates": gates, "note": silk_note, "boardNote": board_note, "fabZipPath": fab_path, "gerbersNote": gerbers_note, "exportRecordPath": export_record, "exportRecordNote": record_note,
            "footprintPath": fetched["side"]["footprintPath"], "symbolPath": fetched["side"]["symbolPath"], "webGlbPath": fetched["side"]["webGlbPath"],
            "message": format!("Optimized GLB written ({} KB, meshes {}->{}, anchored={}, silk={}, footprint={}) to {out}", size / 1024, crate::fusion_prefs::py_str(&stats["meshes_before"]), crate::fusion_prefs::py_str(&stats["meshes_after"]), crate::fusion_prefs::py_str(&stats["molecule_anchored"]), crate::fusion_prefs::py_str(&stats["silkscreen_applied"]), crate::fusion_prefs::py_str(&stats["footprint_applied"])),
            "_hint": "Pull the GLB and its _footprint.json / _symbol.json with pull_file, then publish through molecule-publish. Same optimizer as the molecule GLBs (Colby's pipeline)."});
        if let Some(s) = stop { r["_hint"] = json!(format!("{s} Fix that before publishing. {}", r["_hint"].as_str().unwrap_or(""))); }
        if let Some(g) = gerbers_note.as_deref() { if want_gerbers { r["_hint"] = json!(format!("No fab zip ({g}); every export should carry the Gerbers. {}", r["_hint"].as_str().unwrap_or(""))); } }
        return r;
    }
    if fetched["ok"].as_bool() != Some(true) { return json!({"success": false, "error": fetched["error"], "jobId": job_id, "stepPath": step_path, "statusUrl": status_url, "resultUrl": result_url}); }
    json!({"success": true, "status": fetched["status"].as_str().unwrap_or("processing"), "pending": true, "jobId": job_id, "stepPath": step_path, "boardPath": board_path, "boardNote": board_note, "fabZipPath": fab_path, "gerbersNote": gerbers_note, "exportRecordPath": export_record, "exportRecordNote": record_note, "glbPath": out, "silkTop": silk_top, "silkBottom": silk_bottom, "note": silk_note, "statusUrl": status_url, "resultUrl": result_url,
        "message": format!("Optimize job {job_id} submitted; still processing after {wait_s}s. Finish it with fusion_fetch_optimized_glb {{\"jobId\":\"{job_id}\",\"outputPath\":\"{out}\"}} (re-call until complete)."),
        "_hint": "Big boards tessellate for a few minutes. Either re-call fusion_fetch_optimized_glb with this jobId (writes the GLB on the box when ready), or GET the resultUrl directly."})
}

/// `fusion_fetch_optimized_glb` (by jobId).
pub fn fetch_optimized_glb(args: &Value) -> Value {
    let Some(job_id) = args["jobId"].as_str().filter(|s| !s.is_empty()) else { return json!({"success": false, "error": "No jobId.", "_hint": "Usage: fusion_fetch_optimized_glb {\"jobId\":\"...\",\"outputPath\":\"C:/tmp/board.glb\"}"}) };
    let mut out = args["outputPath"].as_str().or(args["output_path"].as_str()).map(str::to_string);
    if let Some(o) = &mut out { if !o.to_lowercase().ends_with(".glb") { o.push_str(".glb"); } }
    let wait_s = args["wait"].as_u64().unwrap_or(15);
    let res = glb_fetch(job_id, out.as_deref(), wait_s);
    let (status_url, result_url) = (format!("{}/jobs/{job_id}", step2glb_url()), format!("{}/jobs/{job_id}/result", step2glb_url()));
    if res["status"].as_str() == Some("complete") && !res["wrote"].is_null() {
        let stats = &res["stats"]; let size = res["glbBytes"].as_u64().unwrap_or(0);
        // A fetch by jobId cannot know whether a board was sent; footprint JSON on disk means it was.
        let board_sent = args["boardSent"].as_bool().unwrap_or(!res["side"]["footprintPath"].is_null() || stats["footprint_applied"].is_boolean());
        let (gates, stop) = molecule_gates(stats, board_sent);
        let mut r = json!({"success": true, "status": "complete", "glbPath": out, "jobId": job_id, "sizeBytes": size, "meshesAfter": stats["meshes_after"], "moleculeAnchored": stats["molecule_anchored"], "silkscreenApplied": stats["silkscreen_applied"], "gates": gates,
            "footprintPath": res["side"]["footprintPath"], "symbolPath": res["side"]["symbolPath"], "webGlbPath": res["side"]["webGlbPath"], "message": format!("Optimized GLB written ({} KB) to {}", size / 1024, out.as_deref().unwrap_or("None"))});
        if let Some(s) = stop { r["_hint"] = json!(format!("{s} Fix that before publishing.")); }
        return r;
    }
    if res["ok"].as_bool() != Some(true) { return json!({"success": false, "error": res["error"], "jobId": job_id, "statusUrl": status_url, "resultUrl": result_url}); }
    json!({"success": true, "status": res["status"].as_str().unwrap_or("processing"), "pending": true, "jobId": job_id, "statusUrl": status_url, "resultUrl": result_url, "message": format!("Job {job_id} still {}; re-call fusion_fetch_optimized_glb to finish.", res["status"].as_str().unwrap_or("processing"))})
}

static GLB_ASYNC: std::sync::Mutex<Vec<(String, Value)>> = std::sync::Mutex::new(Vec::new());
fn glb_state_set(id: &str, v: Value) { let mut g = GLB_ASYNC.lock().unwrap(); if let Some(e) = g.iter_mut().find(|(k, _)| k == id) { e.1 = v; } else { g.push((id.to_string(), v)); } }
fn glb_state_get(id: &str) -> Option<Value> { GLB_ASYNC.lock().unwrap().iter().find(|(k, _)| k == id).map(|(_, v)| v.clone()) }

/// `_start_glb_export_async`: the detached form, polled by fetch_optimized_glb {exportId}.
pub fn start_glb_export_async(ctx: &Ctx, args: &Value) -> Value {
    let export_id = format!("glbx-{:08x}", (now_f() * 1000.0) as u64 & 0xffff_ffff);
    let started = now_f();
    glb_state_set(&export_id, json!({"exportId": export_id, "phase": "exporting-step", "done": false, "failed": false, "started": started}));
    let mut inner = args.clone(); if let Some(o) = inner.as_object_mut() { o.remove("async"); if !o.contains_key("wait") { o.insert("wait".into(), json!(20)); } }
    let (sd, caller, eid) = (ctx.state_dir.clone(), ctx.caller.clone(), export_id.clone());
    std::thread::spawn(move || {
        let host = crate::host_for_diagnosis();
        host.background_envelope(400.0, false);
        let side_url = crate::launch::SIDECAR_URL.lock().unwrap().clone();
        let side = move |verb: &str, a: Value, t: u64| -> Value { match &side_url { Some(u) => ureq::post(u).timeout(Duration::from_secs(t)).send_json(json!({"command": verb, "args": a})).ok().and_then(|r| r.into_json().ok()).unwrap_or(json!({"success": false, "error": "sidecar unreachable"})), None => json!({"success": false, "error": "sidecar absent"}) } };
        let pids: Vec<u32> = host.fusion_processes().iter().map(|p| p.pid).collect();
        let dlg = |r: Value| r;
        let ctx2 = Ctx { host: &*host, fusion_exe: crate::host_for_diagnosis_paths(), short_path: |p| crate::host().short_path(p), state_dir: sd, caller, pids, sidecar: &side, dialogs: &dlg };
        let pass2 = |verb: &str, a: Value| crate::addin::pass_through(verb, &a, &ctx2);
        let mut st = glb_state_get(&eid).unwrap_or(json!({}));
        let res = export_optimized_glb(&inner, &pass2);
        st["result"] = res.clone();
        if res["glbPath"].is_string() && res["success"].as_bool() == Some(true) && res["pending"].as_bool() != Some(true) { st["phase"] = json!("complete"); st["done"] = json!(true); glb_state_set(&eid, st); return; }
        let Some(job) = res["jobId"].as_str().filter(|_| res["success"].as_bool() == Some(true)).map(str::to_string) else { st["phase"] = json!("failed"); st["failed"] = json!(true); st["done"] = json!(true); st["error"] = json!(res["error"].as_str().unwrap_or("None").chars().take(200).collect::<String>()); glb_state_set(&eid, st); return; };
        st["phase"] = json!("optimizing"); st["jobId"] = json!(job); glb_state_set(&eid, st.clone());
        let deadline = Instant::now() + Duration::from_secs(600);
        while Instant::now() < deadline {
            let f = fetch_optimized_glb(&json!({"jobId": job, "outputPath": inner["outputPath"], "wait": 20, "boardSent": res["boardPath"].is_string()}));
            let mut f = f;
            for k in ["boardPath", "boardNote", "stepPath", "fabZipPath", "gerbersNote", "exportRecordPath", "exportRecordNote"] { if !res[k].is_null() { f[k] = res[k].clone(); } }
            st["result"] = f.clone();
            if f["glbPath"].is_string() && f["pending"].as_bool() != Some(true) { st["phase"] = json!("complete"); st["done"] = json!(true); glb_state_set(&eid, st); return; }
            if f["success"].as_bool() != Some(true) && f["pending"].as_bool() != Some(true) { st["phase"] = json!("failed"); st["failed"] = json!(true); st["done"] = json!(true); st["error"] = json!(f["error"].as_str().unwrap_or("None").chars().take(200).collect::<String>()); glb_state_set(&eid, st); return; }
            glb_state_set(&eid, st.clone());
            std::thread::sleep(Duration::from_secs(5));
        }
        st["phase"] = json!("failed"); st["failed"] = json!(true); st["done"] = json!(true); st["error"] = json!("optimizer did not finish within 10 min"); glb_state_set(&eid, st);
    });
    json!({"success": true, "pending": true, "exportId": export_id, "statusVerb": "fusion_fetch_optimized_glb", "_hint": format!("Export running ASYNC (the sync form can outlive the relay window on a cold Fusion session). Poll fusion_fetch_optimized_glb {{\"exportId\": \"{export_id}\"}} every few seconds: phase walks exporting-step -> optimizing -> complete, and the final poll carries glbPath/sizeBytes.")})
}

/// `_glb_export_async_status`.
pub fn glb_export_async_status(args: &Value) -> Value {
    let id = args["exportId"].as_str().unwrap_or("");
    let Some(st) = glb_state_get(id) else { return json!({"success": false, "error": format!("unknown exportId {id}"), "_hint": "Start one with fusion_export_optimized_glb {async: true}; its response carries the exportId."}) };
    let mut out = json!({"success": st["failed"].as_bool() != Some(true), "exportId": id, "phase": st["phase"], "done": st["done"], "elapsedSec": (now_f() - st["started"].as_f64().unwrap_or(now_f())) as i64});
    if st["jobId"].is_string() { out["jobId"] = st["jobId"].clone(); }
    if st["error"].is_string() { out["error"] = st["error"].clone(); }
    if st["done"].as_bool() == Some(true) && st["result"].is_object() { for k in ["glbPath", "sizeBytes", "meshesAfter", "moleculeAnchored", "silkscreenApplied", "gates", "boardPath", "boardNote", "fabZipPath", "gerbersNote", "exportRecordPath", "exportRecordNote", "footprintPath", "symbolPath", "webGlbPath", "_hint"] { if !st["result"][k].is_null() { out[k] = st["result"][k].clone(); } } }
    out
}

pub fn design_rules_index(state_dir: &std::path::Path) -> Result<Value, String> {
    let cache = state_dir.join("design-rules-index.json");
    if let Ok(m) = std::fs::metadata(&cache) { if m.modified().ok().and_then(|t| t.elapsed().ok()).map(|e| e.as_secs() < 3600).unwrap_or(false) { if let Some(v) = std::fs::read_to_string(&cache).ok().and_then(|t| serde_json::from_str(&t).ok()) { return Ok(v); } } }
    let idx: Value = ureq::get(&format!("{DESIGN_RULES_PAGE}/rules/index.json")).timeout(Duration::from_secs(20)).set("User-Agent", &service_ua()).call().map_err(|e| e.to_string())?.into_json().map_err(|e| e.to_string())?;
    let _ = std::fs::create_dir_all(state_dir); let _ = std::fs::write(&cache, idx.to_string());
    Ok(idx)
}

fn slug(s: &str) -> String { s.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect() }

fn design_rules_pick(idx: &Value, fab: &str, process: &str, layers: i64) -> Result<Value, Value> {
    let want = slug(fab);
    let vendors = idx["vendors"].as_array().cloned().unwrap_or_default();
    let rows: Vec<&Value> = vendors.iter().filter(|v| { let s = slug(v["vendorSlug"].as_str().unwrap_or("")); s == want || (!want.is_empty() && s.contains(&want)) }).collect();
    if rows.is_empty() { let mut all: Vec<String> = vendors.iter().filter_map(|v| v["vendorSlug"].as_str().map(str::to_string)).collect::<std::collections::BTreeSet<_>>().into_iter().collect(); all.sort(); return Err(json!(all)); }
    let mut procs: Vec<String> = rows.iter().filter_map(|v| v["process"].as_str().map(str::to_string)).collect::<std::collections::BTreeSet<_>>().into_iter().collect(); procs.sort();
    let proc_ = if !process.is_empty() { process.to_lowercase() } else if procs.iter().any(|p| p == "standard") { "standard".into() } else { procs.first().cloned().unwrap_or_default() };
    let hit = rows.iter().find(|v| v["process"].as_str() == Some(&proc_) && v["layerCount"].as_i64().unwrap_or(0) == layers);
    match hit { Some(h) => Ok((*h).clone()), None => { let mut lc: Vec<i64> = rows.iter().filter(|v| v["process"].as_str() == Some(&proc_)).map(|v| v["layerCount"].as_i64().unwrap_or(0)).collect::<std::collections::BTreeSet<_>>().into_iter().collect(); lc.sort(); Err(json!({"processes": procs, "layerCounts": lc})) } }
}

/// Rules in a generated profile that native DRC turns into noise: a Copper Clearance with
/// samesignal="yes" over an all/all scope flags every joint where a trace meets its own pad,
/// via or next segment as an Overlap warning (31 on the six-net fixture, page issue #82).
fn profile_rule_warnings(edru: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for line in edru.lines() {
        let l = line.trim();
        if !l.starts_with("<rule ") || !l.contains("samesignal=\"yes\"") || l.contains("enabled=\"no\"") { continue; }
        let attr = |k: &str| l.split(&format!("{k}=\"")).nth(1).and_then(|r| r.split('"').next()).unwrap_or("").to_string();
        let (one, other) = (attr("onescope"), attr("otherscope"));
        let broad = |s: &str| s.is_empty() || s == "all" || s.contains("wire");
        if broad(&one) && broad(&other) {
            out.push(json!({"rule": attr("name"), "type": attr("type"), "value": attr("value"), "onescope": one, "otherscope": other,
                "why": "same-signal clearance over traces: every trace joint, pad entry and via joint is at zero distance, so native DRC reports each one as an Overlap warning",
                "fix": "disable or rescope this rule in the profile on adom/pcb-design-rules (an smd-to-pad same-signal check is fine; all/all is not)"}));
        }
    }
    out
}

/// `fusion_set_design_rules`: a fab's rule pack from the wiki into the open board.
pub fn set_design_rules(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut a = args.clone(); if !a.is_object() { a = json!({}); }
    let action = a["action"].as_str().unwrap_or("apply").to_lowercase();
    let fab = a["fab"].as_str().unwrap_or("").trim().to_string();
    let mut dru_path = a["druPath"].as_str().unwrap_or("").replace('\\', "/");
    // Rules that exist only where the CALLER is (a container AI wrote a custom .edru): the bridge
    // cannot read the caller's disk, so it takes the file as content, writes it to the space-free
    // stage folder, and loads that. Replaces ab's CLI-only fusion_load_design_rules, which ab
    // removed in 2.1.145 (page issue #96).
    let inline: Option<Vec<u8>> = if let Some(t) = a["druText"].as_str().filter(|t| !t.trim().is_empty()) { Some(t.as_bytes().to_vec()) }
        else if let Some(b) = a["druBase64"].as_str().filter(|b| !b.trim().is_empty()) {
            use base64::Engine;
            match base64::engine::general_purpose::STANDARD.decode(b.trim()) { Ok(v) => Some(v), Err(e) => return json!({"success": false, "errorCode": "dru_base64_invalid", "error": format!("druBase64 is not valid base64: {e}")}) }
        } else { None };
    if let Some(bytes) = inline {
        if !dru_path.is_empty() || !fab.is_empty() { return json!({"success": false, "errorCode": "dru_source_ambiguous", "error": "Pass ONE rules source: druText/druBase64, druPath, or fab.", "_hint": "druText or druBase64 carries a file from your side; druPath names one already on the Fusion machine; fab picks a pack from adom/pcb-design-rules."}); }
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(400)]).to_string();
        if !(head.contains("<") || head.contains("=")) { return json!({"success": false, "errorCode": "dru_not_rules", "error": "The content does not look like a Fusion .edru (XML) or EAGLE .dru (key = value) rules file."}); }
        let ext = if head.trim_start().starts_with('<') { "edru" } else { "dru" };
        let dir = match crate::fusionpath::stage_root(&ctx.state_dir) { Ok(d) => d.join("rules"), Err(e) => return json!({"success": false, "errorCode": "stage_unavailable", "error": e}) };
        let _ = std::fs::create_dir_all(&dir);
        let name = a["druName"].as_str().map(|n| n.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect::<String>()).filter(|n| !n.is_empty()).unwrap_or_else(|| "caller-rules".into());
        let dst = dir.join(format!("{name}.{ext}"));
        if let Err(e) = std::fs::write(&dst, &bytes) { return json!({"success": false, "errorCode": "stage_write_failed", "error": format!("could not stage the rules file: {e}")}); }
        dru_path = dst.to_string_lossy().replace('\\', "/");
    }
    let mut rule_warnings: Vec<Value> = Vec::new();
    if action == "list" {
        let idx = match design_rules_index(&ctx.state_dir) { Ok(i) => i, Err(e) => return json!({"success": false, "errorCode": "design_rules_page_unreachable", "error": e.chars().take(200).collect::<String>(), "_hint": "The wiki page adom/pcb-design-rules did not answer; the bundled 3rd party fab rules still apply with no fab argument."}) };
        let rows: Vec<Value> = idx["vendors"].as_array().cloned().unwrap_or_default().iter().map(|v| json!({"fab": v["vendorSlug"], "vendor": v["vendor"], "process": v["process"], "layers": v["layerCount"], "available": v["filesGenerated"].as_bool().unwrap_or(false)})).collect();
        return json!({"success": true, "rules": rows, "count": rows.len(), "page": "https://wiki.adom.inc/adom/pcb-design-rules", "_hint": "Pick a row and call fusion_set_design_rules {fab, process, layers}. Layers auto-detects when omitted (2 or 4 from the open board)."});
    }
    if action == "show" || action == "export" || (fab.is_empty() && dru_path.is_empty()) { return pass("set_design_rules", a); }
    let mut pack = Value::Null;
    if dru_path.is_empty() {
        let mut layers = match &a["layers"] { Value::Null => "auto".to_string(), v => crate::fusion_prefs::py_str(v) };
        if layers == "auto" { let det = pass("detect_layers", json!({})); layers = det["data"]["layerCount"].as_i64().or(det["layerCount"].as_i64()).unwrap_or(2).to_string(); }
        let layers_n: i64 = layers.parse().unwrap_or(2);
        let idx = match design_rules_index(&ctx.state_dir) { Ok(i) => i, Err(e) => return json!({"success": false, "errorCode": "design_rules_page_unreachable", "error": e.chars().take(200).collect::<String>(), "_hint": "Could not fetch the rule packs from wiki.adom.inc; pass druPath to a local .edru, or omit fab for the bundled 3rd party fab rules."}) };
        let row = match design_rules_pick(&idx, &fab, a["process"].as_str().unwrap_or(""), layers_n) { Ok(r) => r, Err(alt) => return json!({"success": false, "errorCode": "design_rules_not_found", "fab": fab, "layers": layers_n, "available": alt, "_hint": "No pack for that fab/process/layer count; `available` lists what the page has. fusion_set_design_rules {action:'list'} shows every row."}) };
        if row["filesGenerated"].as_bool() != Some(true) { return json!({"success": false, "errorCode": "design_rules_not_generated", "fab": fab, "process": row["process"], "layers": layers_n, "_hint": "The page has this vendor's record but no generated .edru yet (the vendor publishes no usable limits for it). Pick another process or fab."}); }
        let rel = row["files"]["fusionEdru"].as_str().unwrap_or("").to_string();
        let (code, edru) = fusion_core::aps::http_bytes(&format!("{DESIGN_RULES_PAGE}/{rel}"));
        if code != 200 { return json!({"success": false, "errorCode": "design_rules_download_failed", "error": format!("HTTP {code}"), "file": rel}); }
        if !edru.windows(12).any(|w| w == b"<designrules") { return json!({"success": false, "errorCode": "design_rules_invalid", "file": rel, "error": "The downloaded file is not an EAGLE design rules document."}); }
        rule_warnings = profile_rule_warnings(&String::from_utf8_lossy(&edru));
        let stage = std::env::var("TEMP").ok().filter(|s| !s.is_empty()).map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("adom-fusion").join("design-rules");
        let _ = std::fs::create_dir_all(&stage);
        dru_path = stage.join(rel.rsplit('/').next().unwrap_or(&rel)).to_string_lossy().replace('\\', "/");
        if let Err(e) = std::fs::write(&dru_path, &edru) { return json!({"success": false, "errorCode": "design_rules_stage_failed", "error": e.to_string()}); }
        pack = json!({"fab": row["vendorSlug"], "vendor": row["vendor"], "process": row["process"], "layers": row["layerCount"], "file": rel, "record": row["files"]["record"]});
        a["_pack"] = pack.clone();
    }
    a["druPath"] = json!(dru_path); a["action"] = json!("apply");
    let mut res = pass("set_design_rules", a);
    if res.is_object() && !rule_warnings.is_empty() {
        res["ruleWarnings"] = json!(rule_warnings);
        res["_hintRuleWarnings"] = json!("The loaded profile carries at least one rule that native DRC will report at every ordinary trace joint (see ruleWarnings). Expect Overlap WARNINGS (code 19, type 2) at pad entries, via joints and segment joints until the profile is corrected on the adom/pcb-design-rules page; they are not defects in the copper. Different-signal overlaps stay ERRORS and still count.");
    }
    if res.is_object() {
        if pack.is_object() { if !res["data"].is_object() { res["data"] = json!({}); } res["data"]["pack"] = pack; res["data"]["source"] = json!("https://wiki.adom.inc/adom/pcb-design-rules"); }
        res["_hint"] = json!(format!("{} Confirm with fusion_alerts {{sinceSec:60}}: an empty list means Fusion accepted the rules; a toast means it refused the file.", res["_hint"].as_str().unwrap_or("")).trim().to_string());
    }
    res
}

#[cfg(test)]
mod fab_tests {
    use super::*;
    #[test]
    fn fab_roles_follow_the_bundled_cam_jobs() {
        for (n, r) in [("b.GTL", "copper_top"), ("b.GBL", "copper_bottom"), ("b_x.G1", "copper_inner_1"), ("b_x.G2", "copper_inner_2"),
            ("b.GTS", "mask_top"), ("b.GBS", "mask_bottom"), ("b.GTO", "silk_top"), ("b.GBO", "silk_bottom"), ("b.GTP", "paste_top"),
            ("b.GBP", "paste_bottom"), ("b.GKO", "outline"), ("b.XLN", "drill_mixed"), ("b_drill.XLN", "drill_mixed"),
            ("Adom/Main/GerberJob/gerber_job.gbrjob", "gerber_job"), ("readme.txt", "other"), ("x.pdf", "other")] {
            assert_eq!(fab_role(n), r, "{n}");
        }
    }
    #[test]
    fn fab_zip_entries_carry_sha256_and_the_job_file() {
        let p = std::env::temp_dir().join(format!("fabtest-{}.zip", std::process::id()));
        {
            let f = std::fs::File::create(&p).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("b.GTL", o).unwrap(); std::io::Write::write_all(&mut z, b"G04*").unwrap();
            z.start_file("job/gerber_job.gbrjob", o).unwrap();
            std::io::Write::write_all(&mut z, br#"{"Header":{"GenerationSoftware":{"Application":"Fusion Electronics","Version":"9.7.0"}},"Overall":{"BoardThickness":1.67,"LayerNumber":2}}"#).unwrap();
            z.finish().unwrap();
        }
        let (e, job) = read_fab_zip(p.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&p);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0]["role"], "copper_top");
        assert_eq!(e[0]["sha256"], sha256_hex(b"G04*"));
        assert_eq!(job["Overall"]["BoardThickness"], 1.67);
    }
}

#[cfg(test)]
mod silk_tests {
    use super::clean_silk_png;
    fn enc(w: u32, h: u32, px: &[u8]) -> Vec<u8> { let mut v = Vec::new(); { let mut e = png::Encoder::new(&mut v, w, h); e.set_color(png::ColorType::Rgba); e.set_depth(png::BitDepth::Eight); e.write_header().unwrap().write_image_data(px).unwrap(); } v }
    fn dec(b: &[u8]) -> (u32, u32, Vec<u8>) { let mut r = png::Decoder::new(b).read_info().unwrap(); let mut buf = vec![0; r.output_buffer_size()]; let i = r.next_frame(&mut buf).unwrap(); (i.width, i.height, buf) }
    #[test]
    fn crops_to_board_and_keeps_only_silk() {
        // 10x10 transparent canvas, a 6x4 opaque brown board at (2,3), one white silk pixel at (4,5)
        let mut px = vec![0u8; 10 * 10 * 4];
        for y in 3..7 { for x in 2..8 { let i = (y * 10 + x) * 4; px[i..i + 4].copy_from_slice(&[110, 100, 65, 255]); } }
        let i = (5 * 10 + 4) * 4; px[i..i + 4].copy_from_slice(&[250, 250, 250, 255]);
        let (w, h, out) = dec(&clean_silk_png(&enc(10, 10, &px)).unwrap());
        assert_eq!((w, h), (6, 4));
        let a = |x: usize, y: usize| out[(y * 6 + x) * 4 + 3];
        assert_eq!(a(2, 2), 255, "the silk pixel stays opaque");
        assert_eq!(a(0, 0), 0, "the board fill turns transparent");
    }
    #[test]
    fn empty_capture_is_left_alone() { assert!(clean_silk_png(&enc(4, 4, &[0u8; 64])).is_none()); }
}

/// `fusion_export_3d {format}`: the standard's export_3d, the board's 3D model in one format. The
/// design's 3D PCB is brought forward, then Fusion's own exporter for that format writes the file;
/// glb goes through the same optimizer as fusion_export_optimized_glb (web:false leaves it
/// uncompressed). A format Fusion has no exporter for is refused by name.
pub fn export_3d(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let format = args["format"].as_str().unwrap_or("").trim().trim_start_matches('.').to_lowercase();
    let Some(out) = args["outputPath"].as_str().map(str::to_string) else { return json!({"success": false, "errorCode": "bad_args", "error": "outputPath is required (the file on the Fusion machine)"}) };
    if args["components"].as_bool() == Some(false) {
        return json!({"success": false, "errorCode": "not_supported", "error": "components:false is not supported: Fusion's 3D PCB holds the board and its parts as one design, and its exporters write all of it"});
    }
    let verb = match format.as_str() {
        "step" | "stp" => "export_step", "stl" => "export_stl", "obj" => "export_obj", "3mf" => "export_3mf", "fbx" => "export_fbx",
        "iges" | "igs" => "export_iges", "sat" => "export_sat", "usdz" => "export_usdz", "f3d" => "export_f3d", "skp" => "export_skp", "glb" => "glb",
        "" => return json!({"success": false, "errorCode": "bad_args", "error": "format is required: stl, obj, glb, 3mf, fbx, iges, sat, usdz, f3d, skp or step"}),
        f => return json!({"success": false, "errorCode": "not_supported", "format": f,
            "error": format!("Fusion has no {f} exporter. It writes stl, obj, glb, 3mf, fbx, iges, sat, usdz, f3d, skp and step"),
            "_hint": "Pick one of those, or export step and convert it elsewhere."}),
    };
    let shown = pass("show_3d_board", json!({"expectDocument": args["expectDocument"]}));
    if shown["success"].as_bool() == Some(false) { return shown; }
    if verb == "glb" {
        let mut a = json!({"outputPath": out, "board": false, "gerbers": false});
        if args["web"].as_bool() == Some(false) { a["compress"] = json!("none"); }
        let mut r = export_optimized_glb(&a, pass);
        r["format"] = json!("glb");
        r["aliasOf"] = json!("fusion_export_optimized_glb");
        return r;
    }
    let mut r = pass(verb, json!({"outputPath": out}));
    let ok = r["success"].as_bool() != Some(false);
    let bytes = std::fs::metadata(&out).map(|m| m.len()).ok();
    r["format"] = json!(format);
    r["path"] = json!(out);
    r["components"] = json!(true);
    if let Some(b) = bytes { r["bytes"] = json!(b); }
    if ok && bytes.is_none() { r["success"] = json!(false); r["errorCode"] = json!("export_missing"); r["error"] = json!(format!("Fusion answered but {out} was not written")); }
    r
}

/// `fusion_import_step`'s board form: the STEP brought into the design's 3D PCB as a new component
/// (an enclosure, a bracket, a mating part), at `at` mm and `rotation` degrees from the board origin,
/// so height_check and the bodies companion can measure the board against it.
const IMPORT_STEP_SCRIPT: &str = r#"
import adsk.core, adsk.fusion, math, os
path = __PATH__
d = adsk.fusion.Design.cast(app.activeProduct)
if d is None:
    result = {'success': False, 'errorCode': 'no_3d_board', 'error': "The design's 3D PCB is not in front"}
elif not os.path.isfile(path):
    result = {'success': False, 'errorCode': 'file_not_found', 'error': 'No STEP at %s on the Fusion machine' % path}
else:
    at, rot = __AT__, __ROT__
    m = adsk.core.Matrix3D.create()
    for axis, deg in zip((adsk.core.Vector3D.create(1, 0, 0), adsk.core.Vector3D.create(0, 1, 0), adsk.core.Vector3D.create(0, 0, 1)), rot):
        if deg:
            r = adsk.core.Matrix3D.create(); r.setToRotation(math.radians(deg), axis, adsk.core.Point3D.create(0, 0, 0)); m.transformBy(r)
    m.translation = adsk.core.Vector3D.create(at[0] / 10.0, at[1] / 10.0, at[2] / 10.0)
    before = app.documents.count
    occ = d.rootComponent.occurrences.addNewComponent(m)
    occ.component.name = __NAME__
    opts = app.importManager.createSTEPImportOptions(path)
    opts.isViewFit = False
    got = app.importManager.importToTarget2(opts, occ.component)
    bodies = occ.component.bRepBodies.count + sum(o.component.bRepBodies.count for o in occ.component.allOccurrences)
    if got is None or got.count == 0 or bodies == 0:
        occ.deleteMe()
        result = {'success': False, 'errorCode': 'import_failed', 'error': 'Fusion imported nothing from %s into the 3D PCB (the component was removed again)' % path}
    else:
        occ.component.name = __NAME__   # the import renames the component it fills: name it after
        b = occ.boundingBox
        result = {'success': True, 'mutated': True, 'id': occ.fullPathName, 'name': occ.component.name, 'bodies': bodies, 'documentsOpened': app.documents.count - before,
                  'bboxMm': [round(v * 10, 3) for v in (b.minPoint.x, b.minPoint.y, b.minPoint.z, b.maxPoint.x, b.maxPoint.y, b.maxPoint.z)]}
"#;

pub fn import_step_with_board(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let Some(path) = args["filePath"].as_str().or(args["path"].as_str()).map(|p| p.replace('\\', "/")) else { return json!({"success": false, "errorCode": "bad_args", "error": "filePath is required: the STEP on the Fusion machine"}) };
    let v3 = |k: &str| -> Vec<f64> { let a: Vec<f64> = args[k].as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect()).unwrap_or_default(); if a.len() == 3 { a } else { vec![0.0; 3] } };
    let (at, rot) = (v3("at"), v3("rotation"));
    let name = args["name"].as_str().map(str::to_string).unwrap_or_else(|| std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("STEP".into()));
    let shown = pass("show_3d_board", json!({"expectDocument": args["expectDocument"]}));
    if shown["success"].as_bool() == Some(false) { return shown; }
    let script = IMPORT_STEP_SCRIPT.replace("__PATH__", &fusion_lbr::py_repr_str(&path)).replace("__NAME__", &fusion_lbr::py_repr_str(&name))
        .replace("__AT__", &format!("({}, {}, {})", at[0], at[1], at[2])).replace("__ROT__", &format!("({}, {}, {})", rot[0], rot[1], rot[2]));
    let r = pass("run_modeling_script", json!({"script": script}));
    if r["success"].as_bool() != Some(true) { return r; }
    let mut out = r["data"]["result"].clone();
    if !out.is_object() { return json!({"success": false, "errorCode": "import_failed", "error": "Fusion returned no structured result", "data": r["data"]}); }
    if out["success"] == json!(true) {
        out["_hint"] = json!("Placed with the board in its 3D PCB. fusion_bodies and fusion_height_check now measure the board against it; save the design (fusion_save_document) to keep it.");
    }
    out
}

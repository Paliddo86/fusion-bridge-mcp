//! The schematic verbs of the EDA Bridge Standard (sch_*): sch_script.py runs on Fusion's main
//! thread through the script transport, as the routing verbs do.
use serde_json::{json, Value};
const SOURCE: &str = concat!(include_str!("sch_script.py"), "\n", include_str!("lbr_use.py"));
pub fn owns(cmd: &str) -> bool { matches!(cmd, "sch_label" | "sch_wire" | "sch_batch" | "sch_update_from_library" | "sch_items" | "sch_pins" | "sch_trace_point" | "sch_page" | "sch_hierarchy" | "sch_text" | "sch_junction" | "sch_delete_wire" | "sch_connect_pins" | "sch_split_wire" | "sch_bus" | "sch_edit_label" | "sch_move_region" | "sch_fix_connectivity" | "sch_bus_entry" | "sch_connect_to_bus" | "render_schematic" | "sch_reset_fields" | "sch_create" | "sch_sheet_pins" | "sch_update_from_library" | "cross_probe" | "cross_check" | "schematic_data" | "part_parameters" | "doc_params" | "sheet_params" | "sch_edit" | "annotate" | "sch_connect" | "sch_place_part" | "sch_swap_part" | "export_spice" | "simulate" | "sch_zoom" | "jlcpcb_order" | "variants") }
pub fn call(cmd: &str, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    if cmd == "render_schematic" { return render_schematic(args, pass); }
    if cmd == "sch_batch" && args["dryRun"].as_bool() != Some(true) { return sch_batch(args, pass); }
    let script = format!("import json\nexec({})\nresult=handle(app,json.loads({}),{})\n",
        serde_json::to_string(SOURCE).unwrap(), serde_json::to_string(&args.to_string()).unwrap(), serde_json::to_string(cmd).unwrap());
    let r = pass("run_modeling_script", json!({"script": script}));
    if r["success"].as_bool() == Some(false) { return r; }
    let mut out = match r["data"]["result"].as_object() {
        Some(_) => r["data"]["result"].clone(),
        None => return json!({"success": false, "errorCode": "invalid_schematic_response", "error": "Fusion did not return a structured schematic result"}),
    };
    // save defaults to false: each Fusion save is a new cloud version. save:true saves the design
    // in place after a successful edit, as fusion_save_document does.
    if args["save"].as_bool() == Some(true) && out["success"].as_bool() == Some(true) && out["mutated"].as_bool() == Some(true) {
        let sv = crate::savedoc::save_document(&json!({"documentName": out["documentName"], "comment": args["saveComment"].as_str().unwrap_or("fusion-bridge schematic edit")}), pass);
        out["saved"] = json!(sv["success"].as_bool() == Some(true) && sv["saved"].as_bool() != Some(false));
        if out["saved"] != json!(true) { out["saveResult"] = sv; }
    }
    out
}

/// `render_schematic`: the sheet as a PNG (Fusion's EXPORT IMAGE, in sch_script.py), and visual
/// baselines kept beside it: `baseline:"set"` stores the render, `baseline:"compare"` renders at the
/// baseline's width and reports the pixel drift, the changed region in mm and a diff image.
pub fn render_schematic(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let dir = args["outputDir"].as_str().map(str::to_string).unwrap_or_else(|| {
        let t = std::env::var("TEMP").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string());
        format!("{}/adom-schematic-renders", t.replace('\\', "/"))
    });
    let _ = std::fs::create_dir_all(&dir);
    let sheet = args["sheet"].as_u64().unwrap_or(1);
    let stem: String = args["name"].as_str().unwrap_or("schematic").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let base = format!("{dir}/{stem}-s{sheet}");
    let (png, bpng, bjson, dpng) = (format!("{base}.png"), format!("{base}.baseline.png"), format!("{base}.baseline.json"), format!("{base}.diff.png"));
    let mode = args["baseline"].as_str().unwrap_or("");
    let mut a = args.clone();
    let mut prior = Value::Null;
    if mode == "compare" {
        prior = match std::fs::read_to_string(&bjson).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            Some(v) => v,
            None => return json!({"success": false, "errorCode": "no_baseline", "error": format!("No baseline at {bpng}"), "_hint": "Set one first: render_schematic {baseline:\"set\"} (same name, sheet and outputDir)."}),
        };
        a["width"] = prior["requestedWidth"].clone();
    }
    a["png"] = json!(png);
    let mut r = call("sch_render", &a, pass);
    if r["success"].as_bool() != Some(true) { return r; }
    match mode {
        "set" => {
            if let Err(e) = std::fs::copy(&png, &bpng) { return json!({"success": false, "errorCode": "write_failed", "error": format!("could not store the baseline: {e}")}); }
            let meta = json!({"requestedWidth": a["width"].as_u64().unwrap_or(2000), "width": r["width"], "height": r["height"], "dpi": r["dpi"], "extentMm": r["extentMm"], "fusionVersion": r["fusionVersion"], "revision": r["revision"]});
            let _ = std::fs::write(&bjson, serde_json::to_string_pretty(&meta).unwrap());
            r["baseline"] = json!(bpng);
        }
        "compare" => {
            let threshold = args["threshold"].as_f64().unwrap_or(0.5);
            let cmp = compare_png(&bpng, &png, &dpng);
            match cmp {
                Err(e) => return json!({"success": false, "errorCode": "compare_failed", "error": e}),
                Ok((drift, bbox, w, h)) => {
                    let dpi = r["dpi"].as_f64().unwrap_or(100.0);
                    let x0 = r["extentMm"][0].as_f64().unwrap_or(0.0);
                    let y1 = r["extentMm"][3].as_f64().unwrap_or(0.0);
                    let px = 25.4 / dpi;
                    r["driftPercent"] = json!((drift * 1000.0).round() / 1000.0);
                    r["pass"] = json!(drift <= threshold);
                    r["changedRegionMm"] = match bbox {
                        Some((ax, ay, bx, by)) => json!([round4(x0 + ax as f64 * px), round4(y1 - by as f64 * px), round4(x0 + bx as f64 * px), round4(y1 - ay as f64 * px)]),
                        None => Value::Null,
                    };
                    r["diffPng"] = if bbox.is_some() { json!(dpng) } else { Value::Null };
                    r["rendererChanged"] = json!(prior["fusionVersion"] != r["fusionVersion"]);
                    r["sizeChanged"] = json!(prior["width"].as_u64() != Some(w as u64) || prior["height"].as_u64() != Some(h as u64));
                    r["baseline"] = json!(bpng);
                }
            }
        }
        _ => {}
    }
    r["_hint"] = json!("changedRegionMm maps the changed pixels back to the sheet through the drawn extent (mm, y up), to within a text's overhang. A paper or extent change reads as a size change; rendererChanged means Fusion's version changed since the baseline.");
    r
}

fn round4(v: f64) -> f64 { (v * 10000.0).round() / 10000.0 }

fn decode_rgb(path: &str) -> Result<(usize, usize, Vec<u8>), String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut dec = png::Decoder::new(f);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut rd = dec.read_info().map_err(|e| format!("{path}: {e}"))?;
    let mut buf = vec![0u8; rd.output_buffer_size()];
    let info = rd.next_frame(&mut buf).map_err(|e| format!("{path}: {e}"))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = match info.color_type { png::ColorType::Rgba => 4, png::ColorType::Rgb => 3, png::ColorType::GrayscaleAlpha => 2, _ => 1 };
    let mut rgb = Vec::with_capacity(w * h * 3);
    for p in buf[..w * h * ch].chunks(ch) {
        match ch { 4 | 3 => rgb.extend_from_slice(&p[..3]), _ => rgb.extend_from_slice(&[p[0], p[0], p[0]]) }
    }
    Ok((w, h, rgb))
}

/// The share of pixels that differ (percent), their bounding box in pixels, and a diff image (the
/// new render with the changed pixels in red). Different sizes count as 100 percent drift.
fn compare_png(old: &str, new: &str, diff: &str) -> Result<(f64, Option<(usize, usize, usize, usize)>, usize, usize), String> {
    let (w0, h0, a) = decode_rgb(old)?;
    let (w, h, b) = decode_rgb(new)?;
    if (w0, h0) != (w, h) { return Ok((100.0, Some((0, 0, w.saturating_sub(1), h.saturating_sub(1))), w, h)); }
    let mut out = b.clone();
    let (mut n, mut bb) = (0usize, None::<(usize, usize, usize, usize)>);
    for i in 0..w * h {
        let d = (0..3).map(|k| (a[i * 3 + k] as i32 - b[i * 3 + k] as i32).abs()).max().unwrap_or(0);
        if d > 32 {
            n += 1;
            let (x, y) = (i % w, i / w);
            bb = Some(match bb { None => (x, y, x, y), Some((ax, ay, bx, by)) => (ax.min(x), ay.min(y), bx.max(x), by.max(y)) });
            out[i * 3..i * 3 + 3].copy_from_slice(&[255, 0, 0]);
        }
    }
    if bb.is_some() {
        let f = std::fs::File::create(diff).map_err(|e| format!("{diff}: {e}"))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().and_then(|mut wr| wr.write_image_data(&out)).map_err(|e| format!("{diff}: {e}"))?;
    }
    Ok((n as f64 * 100.0 / (w * h) as f64, bb, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn script_failure_passes_through() {
        let r = call("sch_items", &json!({}), &|_, _| json!({"success": false, "errorCode": "timeout"}));
        assert_eq!(r["errorCode"], "timeout");
        assert!(owns("sch_pins") && !owns("routing_state"));
    }
    #[test]
    fn png_compare_counts_changed_pixels() {
        let dir = std::env::temp_dir().join(format!("sch-png-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, red: bool| {
            let p = dir.join(name);
            let mut img = vec![255u8; 10 * 10 * 3];
            if red { img[(5 * 10 + 5) * 3..(5 * 10 + 5) * 3 + 3].copy_from_slice(&[0, 0, 0]); }
            let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&p).unwrap()), 10, 10);
            enc.set_color(png::ColorType::Rgb); enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&img).unwrap();
            p.to_string_lossy().to_string()
        };
        let (a, b) = (write("a.png", false), write("b.png", true));
        let d = dir.join("d.png").to_string_lossy().to_string();
        let (drift, bb, _, _) = compare_png(&a, &b, &d).unwrap();
        assert_eq!(1.0, drift);
        assert_eq!(Some((5, 5, 5, 5)), bb);
        assert_eq!(0.0, compare_png(&a, &a, &d).unwrap().0);
    }
}

/// The ERC counts of the open design: Fusion's own ERC (fusion_run_erc), or why it could not run.
fn erc_counts(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let r = crate::routing::call("run_erc", &json!({"expectDocument": args["ercDocument"].as_str().or(args["expectDocument"].as_str())}), pass);
    if r["success"].as_bool() != Some(true) { return json!({"ran": false, "error": r["error"]}); }
    json!({"errors": r["errorCount"], "warnings": r["warningCount"], "messages": r["messageCount"]})
}

/// `sch_batch`: the batch with an ERC before and after (the standard's ercBefore, ercAfter).
fn sch_batch(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let before = erc_counts(args, pass);
    let mut a = args.clone();
    a["dryRun"] = json!(false);
    let mut r = call("sch_batch_run", &a, pass);
    r["ercBefore"] = before;
    if r["mutated"].as_bool() == Some(true) { r["ercAfter"] = erc_counts(args, pass); } else { r["ercAfter"] = r["ercBefore"].clone(); }
    r
}

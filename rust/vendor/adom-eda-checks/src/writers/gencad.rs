//! GenCAD 1.4 written from the board model (mm, y up), for a bridge whose tool has no GenCAD exporter (Altium has none).
//! Outline, one PAD and PADSTACK per pad shape and drill, one SHAPE per footprint with pins in its own frame (the part's
//! rotation removed, a bottom part mirrored back to the top view), COMPONENT (bottom parts MIRRORX FLIP), DEVICE,
//! SIGNAL/NODE, TRACK codes by width and ROUTE with LINE/ARC/VIA, in inches. The syntax follows a file KiCad 10 wrote.
//! No I/O: the bridge writes the text.
use serde_json::{json, Value};

use super::dxf::{bulge, f, is_copper};

/// GenCAD's name for a copper layer of the board model.
fn gencad_layer(l: &str) -> String {
    match l {
        "F.Cu" => "TOP".into(),
        "B.Cu" => "BOTTOM".into(),
        _ => l.strip_prefix("In").and_then(|n| n.strip_suffix(".Cu")).map(|n| format!("INNER{n}")).unwrap_or_else(|| "TOP".into()),
    }
}

fn q(s: &str) -> String { format!("\"{}\"", s.replace('"', "'")) }

/// The board as GenCAD 1.4 text, in inches as fixture and assembly tools expect: outline, pads and padstacks, one
/// shape per footprint (pins in the footprint's own frame), components, devices, signals, and the routes with their
/// vias. Returns the text and counts.
pub fn gencad_text(state: &Value, title: &str, unique_pins: bool) -> (String, Value) {
    let inch = |mm: f64| format!("{:.6}", mm / 25.4);
    let arr = |k: &str| state[k].as_array().cloned().unwrap_or_default();
    let mut o = String::new();
    o += &format!("$HEADER\nGENCAD 1.4\nUSER {}\nDRAWING {}\nREVISION {}\nUNITS INCH\nORIGIN 0 0\nINTERTRACK 0\n$ENDHEADER\n",
        q(&format!("Adom EDA engine {}", env!("CARGO_PKG_VERSION"))), q(title), q("1"));
    // the outline
    o += "$BOARD\n";
    for e in arr("outlineEdges") {
        let (s, t) = ((f(&e["start"][0]), f(&e["start"][1])), (f(&e["end"][0]), f(&e["end"][1])));
        if e["kind"] == "arc" {
            // GenCAD arcs run counter-clockwise from the first point to the second
            let (a, b) = if bulge(&e) >= 0.0 { (s, t) } else { (t, s) };
            o += &format!("ARC {} {} {} {} {} {}\n", inch(a.0), inch(a.1), inch(b.0), inch(b.1), inch(f(&e["center"][0])), inch(f(&e["center"][1])));
        } else {
            o += &format!("LINE {} {} {} {}\n", inch(s.0), inch(s.1), inch(t.0), inch(t.1));
        }
    }
    o += "$ENDBOARD\n";
    // pads: one per shape, size and drill, in the footprint's frame
    let comps = arr("components");
    let pads = arr("pads");
    let mut pad_defs: Vec<(String, f64, f64, f64)> = Vec::new(); // (shape, w, h, drill)
    let mut pad_name = |shape: &str, w: f64, h: f64, drill: f64| -> usize {
        let k = (shape.to_string(), (w * 1000.0).round() / 1000.0, (h * 1000.0).round() / 1000.0, (drill * 1000.0).round() / 1000.0);
        match pad_defs.iter().position(|p| *p == k) { Some(i) => i, None => { pad_defs.push(k); pad_defs.len() - 1 } }
    };
    // shapes: one per footprint pattern and pad geometry. Two parts share a shape only when their pins match in the
    // footprint's frame: two different footprints of the same name (from two libraries) get NAME and NAME_2 (#17)
    struct Pin { name: String, pad: usize, x: f64, y: f64, rot: f64, through: bool }
    let mut shapes: Vec<(String, Vec<Pin>, bool)> = Vec::new();
    let mut shape_of: Vec<(String, String)> = Vec::new(); // ref -> shape name
    // the same pins within 5 um: the board model keeps positions to 1 um, so a rotated part's pins come back a micron off
    // (BQ25792's R3 against R5: 2.319 against 2.320 mm), and float noise turns 0 into -1e-16
    let same_pins = |a: &[Pin], b: &[Pin]| a.len() == b.len() && a.iter().zip(b).all(|(p, q)| p.name == q.name && p.pad == q.pad
        && (p.x - q.x).abs() < 0.005 && (p.y - q.y).abs() < 0.005 && (p.rot - q.rot).abs() < 0.05);
    for c in &comps {
        let rf = c["ref"].as_str().unwrap_or("").to_string();
        let pattern = c["pattern"].as_str().unwrap_or("").to_string();
        let base = if pattern.is_empty() { format!("SHAPE_{rf}") } else { pattern.clone() };
        let (cx, cy, th) = (f(&c["x"]), f(&c["y"]), f(&c["rotation"]).to_radians());
        let bottom = c["side"] == "bottom";
        let mut pins = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for p in pads.iter().filter(|p| p["ref"].as_str() == Some(&rf)) {
            let (dx, dy) = (f(&p["x"]) - cx, f(&p["y"]) - cy);
            let (mut lx, ly) = (dx * th.cos() + dy * th.sin(), -dx * th.sin() + dy * th.cos());
            if bottom { lx = -lx; }
            let mut rot = (f(&p["rotation"]) - f(&c["rotation"])).rem_euclid(360.0);
            if bottom { rot = (360.0 - rot).rem_euclid(360.0); }
            let (mut w, mut h) = (f(&p["sizeX"]), f(&p["sizeY"]));
            // the pad's own size along the footprint's axes
            if (rot - 90.0).abs() < 0.5 || (rot - 270.0).abs() < 0.5 { std::mem::swap(&mut w, &mut h); rot = 0.0; }
            if rot.abs() < 0.5 || (rot - 180.0).abs() < 0.5 { rot = 0.0; }
            let through = f(&p["drill"]) > 0.0;
            let mut nm = p["name"].as_str().unwrap_or("").to_string();
            if unique_pins && seen.contains(&nm) {
                let mut k = 2;
                while seen.contains(&format!("{nm}_{k}")) { k += 1; }
                nm = format!("{nm}_{k}");
            }
            seen.push(nm.clone());
            let id = pad_name(p["shape"].as_str().unwrap_or("rect"), w, h, f(&p["drill"]));
            pins.push(Pin { name: nm, pad: id, x: lx, y: ly, rot, through });
        }
        // the same pattern with the same pins: reuse that shape; the same name with other pins: a new name
        if let Some(existing) = shapes.iter().find(|(n, ps, _)| (*n == base || n.starts_with(&format!("{base}_"))) && same_pins(ps, &pins)) {
            shape_of.push((rf, existing.0.clone()));
            continue;
        }
        let mut name = base.clone();
        let mut k = 2;
        while shapes.iter().any(|x| x.0 == name) { name = format!("{base}_{k}"); k += 1; }
        shape_of.push((rf, name.clone()));
        let th_part = pins.iter().any(|p| p.through);
        shapes.push((name, pins, th_part));
    }
    // vias: one pad per size and drill
    let vias = arr("vias");
    let via_pad: Vec<usize> = vias.iter().map(|v| pad_name("round", f(&v["size"]), f(&v["size"]), f(&v["drill"]))).collect();
    o += "$PADS\n";
    for (i, (shape, w, h, drill)) in pad_defs.iter().enumerate() {
        let d = if *drill > 0.0 { inch(*drill) } else { "0".into() };
        if shape == "round" && (w - h).abs() < 1e-6 {
            o += &format!("PAD P{i} ROUND {d}\nCIRCLE 0 0 {}\n", inch(w / 2.0));
        } else if shape == "round" {
            o += &format!("PAD P{i} FINGER {d}\nRECTANGLE {} {} {} {}\n", inch(-w / 2.0), inch(-h / 2.0), inch(*w), inch(*h));
        } else {
            o += &format!("PAD P{i} RECTANGULAR {d}\nRECTANGLE {} {} {} {}\n", inch(-w / 2.0), inch(-h / 2.0), inch(*w), inch(*h));
        }
    }
    o += "$ENDPADS\n$PADSTACKS\n";
    let coppers: Vec<String> = {
        let mut ls: Vec<String> = arr("tracks").iter().chain(pads.iter()).filter_map(|x| x["layer"].as_str()).filter(|l| is_copper(l)).map(str::to_string).collect();
        ls.push("F.Cu".into()); ls.push("B.Cu".into());
        ls.sort_by_key(|l| match l.as_str() { "F.Cu" => 0, "B.Cu" => 999, x => x.trim_start_matches("In").trim_end_matches(".Cu").parse::<i32>().unwrap_or(500) });
        ls.dedup();
        ls
    };
    for (i, (_, _, _, drill)) in pad_defs.iter().enumerate() {
        let d = if *drill > 0.0 { inch(*drill) } else { "0".into() };
        o += &format!("PADSTACK PS{i} {d}\n");
        if *drill > 0.0 {
            for l in &coppers { o += &format!("PAD P{i} {} 0 0\n", gencad_layer(l)); }
        } else {
            o += &format!("PAD P{i} TOP 0 0\n");
        }
    }
    o += "$ENDPADSTACKS\n$SHAPES\n";
    for (name, pins, th) in &shapes {
        o += &format!("SHAPE {}\nINSERT {}\n", q(name), if *th { "TH" } else { "SMD" });
        for p in pins {
            o += &format!("PIN {} PS{} {} {} TOP {} 0\n", q(&p.name), p.pad, inch(p.x), inch(p.y), p.rot);
        }
    }
    o += "$ENDSHAPES\n$COMPONENTS\n";
    let mut devices: Vec<String> = Vec::new();
    for c in &comps {
        let rf = c["ref"].as_str().unwrap_or("");
        let shape = shape_of.iter().find(|s| s.0 == rf).map(|s| s.1.clone()).unwrap_or_default();
        let bottom = c["side"] == "bottom";
        let dev = format!("DEV_{shape}");
        if !devices.contains(&dev) { devices.push(dev.clone()); }
        o += &format!("COMPONENT {}\nDEVICE {}\nPLACE {} {}\nLAYER {}\nROTATION {}\nSHAPE {} {} {}\n",
            q(rf), q(&dev), inch(f(&c["x"])), inch(f(&c["y"])), if bottom { "BOTTOM" } else { "TOP" }, f(&c["rotation"]), q(&shape),
            if bottom { "MIRRORX" } else { "0" }, if bottom { "FLIP" } else { "0" });
    }
    o += "$ENDCOMPONENTS\n$DEVICES\n";
    for d in &devices { o += &format!("DEVICE {}\nPACKAGE {}\n", q(d), q(d.trim_start_matches("DEV_"))); }
    o += "$ENDDEVICES\n$SIGNALS\n";
    let mut nets: Vec<String> = pads.iter().filter_map(|p| p["net"].as_str()).filter(|n| !n.is_empty()).map(str::to_string).collect();
    nets.sort(); nets.dedup();
    for n in &nets {
        o += &format!("SIGNAL {}\n", q(n));
        for p in pads.iter().filter(|p| p["net"].as_str() == Some(n) && !p["ref"].as_str().unwrap_or("").is_empty()) {
            o += &format!("NODE {} {}\n", q(p["ref"].as_str().unwrap_or("")), q(p["name"].as_str().unwrap_or("")));
        }
    }
    o += "$ENDSIGNALS\n";
    // tracks by width, then each net's copper
    let tracks: Vec<Value> = arr("tracks").into_iter().filter(|t| t["layer"].as_str().map(is_copper).unwrap_or(false)).collect();
    let arcs: Vec<Value> = arr("arcs").into_iter().filter(|t| t["layer"].as_str().map(is_copper).unwrap_or(false)).collect();
    let mut widths: Vec<f64> = tracks.iter().chain(arcs.iter()).map(|t| (f(&t["width"]) * 1000.0).round() / 1000.0).collect();
    widths.sort_by(|a, b| a.partial_cmp(b).unwrap()); widths.dedup();
    o += "$TRACKS\n";
    for (i, w) in widths.iter().enumerate() { o += &format!("TRACK T{i} {}\n", inch(*w)); }
    o += "$ENDTRACKS\n$ROUTES\n";
    let code = |w: f64| widths.iter().position(|x| (*x - (w * 1000.0).round() / 1000.0).abs() < 1e-9).unwrap_or(0);
    let mut route_nets: Vec<String> = tracks.iter().chain(arcs.iter()).filter_map(|t| t["net"].as_str()).chain(vias.iter().filter_map(|v| v["net"].as_str())).filter(|n| !n.is_empty()).map(str::to_string).collect();
    route_nets.sort(); route_nets.dedup();
    let mut unrouted_copper = 0;
    for t in tracks.iter().chain(arcs.iter()) { if t["net"].as_str().unwrap_or("").is_empty() { unrouted_copper += 1; } }
    for n in &route_nets {
        o += &format!("ROUTE {}\n", q(n));
        for t in tracks.iter().filter(|t| t["net"].as_str() == Some(n)) {
            o += &format!("TRACK T{}\nLAYER {}\nLINE {} {} {} {}\n", code(f(&t["width"])), gencad_layer(t["layer"].as_str().unwrap_or("F.Cu")),
                inch(f(&t["start"]["x"])), inch(f(&t["start"]["y"])), inch(f(&t["end"]["x"])), inch(f(&t["end"]["y"])));
        }
        for a in arcs.iter().filter(|a| a["net"].as_str() == Some(n)) {
            let (c, r) = ((f(&a["center"]["x"]), f(&a["center"]["y"])), f(&a["radius"]));
            let p = |deg: f64| (c.0 + r * deg.to_radians().cos(), c.1 + r * deg.to_radians().sin());
            let (s, e) = (p(f(&a["startAngle"])), p(f(&a["endAngle"])));
            o += &format!("TRACK T{}\nLAYER {}\nARC {} {} {} {} {} {}\n", code(f(&a["width"])), gencad_layer(a["layer"].as_str().unwrap_or("F.Cu")),
                inch(s.0), inch(s.1), inch(e.0), inch(e.1), inch(c.0), inch(c.1));
        }
        for (i, v) in vias.iter().enumerate().filter(|(_, v)| v["net"].as_str() == Some(n)) {
            // as KiCad writes it: the padstack, the position, the layers and the drill
            o += &format!("VIA PS{} {} {} ALL {} VIA{i}\n", via_pad[i], inch(f(&v["position"]["x"])), inch(f(&v["position"]["y"])), inch(f(&v["drill"])));
        }
    }
    o += "$ENDROUTES\n";
    let counts = json!({"components": comps.len(), "shapes": shapes.len(), "pads": pad_defs.len(), "signals": nets.len(), "routes": route_nets.len(),
        "vias": vias.len(), "copperWithoutNet": unrouted_copper});
    (o, counts)
}

/// writers::gencad: `{board, args: {title?, uniquePins?}}` -> the GenCAD text and what it holds.
pub fn gencad(input: &Value) -> Value {
    let board = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = input.get("args").cloned().unwrap_or(json!({}));
    let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("board");
    let (text, counts) = gencad_text(&board, title, args.get("uniquePins").and_then(|v| v.as_bool()) == Some(true));
    json!({"success": true, "text": text, "counts": counts, "units": "in", "writtenFrom": "board model",
        "_hint": "GenCAD 1.4 written from the board model, not the tool's own exporter. copperWithoutNet counts tracks on no net, which no route can carry."})
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::dxf::tests::lane;

    #[test]
    fn gencad_has_every_section_and_the_lane_parts() {
        let Some(st) = lane() else { return };
        let (t, c) = gencad_text(&st, "lane", false);
        for s in ["HEADER", "BOARD", "PADS", "PADSTACKS", "SHAPES", "COMPONENTS", "DEVICES", "SIGNALS", "TRACKS", "ROUTES"] {
            assert_eq!(t.matches(&format!("${s}\n")).count(), 1, "{s}");
            assert_eq!(t.matches(&format!("$END{s}\n")).count(), 1, "{s}");
        }
        assert_eq!(c["components"], json!(st["components"].as_array().unwrap().len()));
        assert_eq!(t.matches("\nCOMPONENT ").count(), st["components"].as_array().unwrap().len());
        // the outline: four lines and four corner arcs, a 40 x 20 mm board in inches
        assert_eq!(t.split("$ENDBOARD").next().unwrap().matches("\nARC ").count(), 4);
        assert!(t.contains(&format!("{:.6}", 40.0 / 25.4)));
        // every pin of a SOIC-8 sits in its own frame: x is +-2.7 mm whatever the part's rotation
        let shape = t.split("SHAPE \"TST_SOIC8\"\n").nth(1).unwrap().split("SHAPE ").next().unwrap();
        assert_eq!(shape.matches("PIN ").count(), 8);
        for l in shape.lines().filter(|l| l.starts_with("PIN ")) {
            let x: f64 = l.split_whitespace().nth(3).unwrap().parse().unwrap();
            assert!((x.abs() * 25.4 - 2.7).abs() < 0.01, "{l}");
        }
    }

    #[test]
    fn duplicate_pin_names_are_made_unique_on_request() {
        let st = json!({"components": [{"ref": "J1", "pattern": "TAB", "x": 0.0, "y": 0.0, "rotation": 0.0, "side": "top"}],
            "pads": [{"ref": "J1", "name": "1", "x": 0.0, "y": 0.0, "sizeX": 1.0, "sizeY": 1.0, "shape": "rect", "drill": 0.0, "rotation": 0.0, "layer": "F.Cu", "net": ""},
                     {"ref": "J1", "name": "1", "x": 2.0, "y": 0.0, "sizeX": 1.0, "sizeY": 1.0, "shape": "rect", "drill": 0.0, "rotation": 0.0, "layer": "F.Cu", "net": ""}]});
        assert_eq!(gencad_text(&st, "x", false).0.matches("PIN \"1\" ").count(), 2);
        let (t, _) = gencad_text(&st, "x", true);
        assert!(t.contains("PIN \"1\" ") && t.contains("PIN \"1_2\" "));
    }

    #[test]
    fn bottom_parts_and_vias_read_as_kicad_writes_them() {
        let st = json!({"components": [{"ref": "R1", "pattern": "R0603", "x": 10.0, "y": 5.0, "rotation": 180.0, "side": "bottom"}],
            "pads": [{"ref": "R1", "name": "1", "x": 10.825, "y": 5.0, "sizeX": 0.8, "sizeY": 0.95, "shape": "rect", "drill": 0.0, "rotation": 180.0, "layer": "B.Cu", "net": "A"}],
            "vias": [{"net": "A", "drill": 0.3, "size": 0.6, "layers": ["F.Cu", "B.Cu"], "position": {"x": 10.0, "y": 7.0}}]});
        let (t, _) = gencad_text(&st, "x", false);
        assert!(t.contains("LAYER BOTTOM\nROTATION 180\nSHAPE \"R0603\" MIRRORX FLIP\n"), "{t}");
        // pad 1 is 0.825 mm left of the part's centre in its own frame (rotated 180, then mirrored back to the top view)
        assert!(t.contains(&format!("PIN \"1\" PS0 {:.6} 0.000000 TOP", 0.825 / 25.4)) || t.contains(&format!("PIN \"1\" PS0 {:.6} -0.000000 TOP", 0.825 / 25.4)), "{t}");
        assert!(t.contains(&format!("ALL {:.6} VIA0", 0.3 / 25.4)), "{t}");
    }

    #[test]
    fn the_writer_entry_says_where_it_came_from() {
        let r = gencad(&json!({"board": {"components": [], "pads": [], "outlineEdges": []}, "args": {"title": "t"}}));
        assert_eq!(r["success"], true);
        assert_eq!(r["writtenFrom"], "board model");
        assert!(r["text"].as_str().unwrap().starts_with("$HEADER\nGENCAD 1.4\n"));
    }

    #[test]
    fn same_named_footprints_with_other_pads_are_two_shapes() {
        let pad = |r: &str, n: &str, x: f64, w: f64| json!({"ref": r, "name": n, "x": x, "y": 0.0, "sizeX": w, "sizeY": 1.0, "shape": "rect", "drill": 0.0, "rotation": 0.0, "layer": "F.Cu", "net": ""});
        // R1 and R2: the same "R0603" footprint (R2 rotated 180): one shape. R3: another library's "R0603", other pads: its own
        let st = json!({"components": [{"ref": "R1", "pattern": "R0603", "x": 0.0, "y": 0.0, "rotation": 0.0, "side": "top"},
                                       {"ref": "R2", "pattern": "R0603", "x": 10.0, "y": 0.0, "rotation": 180.0, "side": "top"},
                                       {"ref": "R3", "pattern": "R0603", "x": 20.0, "y": 0.0, "rotation": 0.0, "side": "top"}],
            "pads": [pad("R1", "1", -0.8, 0.9), pad("R1", "2", 0.8, 0.9), pad("R2", "1", 10.801, 0.9), pad("R2", "2", 9.2, 0.9),
                     pad("R3", "1", 19.0, 1.2), pad("R3", "2", 21.0, 1.2)]});
        let (t, _) = gencad_text(&st, "x", false);
        assert_eq!(t.matches("SHAPE \"R0603\"\nINSERT").count(), 1, "R1 and R2 share one shape");
        assert!(t.contains("SHAPE \"R0603_2\"\nINSERT"), "R3's footprint is its own shape");
        assert!(!t.contains("R0603_3"), "R2 reuses R1's shape");
        // each part points at its shape
        let comp = |r: &str| t.split(&format!("COMPONENT \"{r}\"\n")).nth(1).unwrap().lines().find(|l| l.starts_with("SHAPE ")).unwrap().to_string();
        assert!(comp("R1").starts_with("SHAPE \"R0603\"") && comp("R2").starts_with("SHAPE \"R0603\"") && comp("R3").starts_with("SHAPE \"R0603_2\""));
    }
}


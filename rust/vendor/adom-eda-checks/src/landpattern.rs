//! Footprints from a datasheet's land pattern: the AI gives the numbers the datasheet prints (pitch, pad size, row
//! span, body), the engine lays out a complete footprint: pads numbered the standard way, an exposed pad, the courtyard,
//! silkscreen kept clear of the copper with a pin-1 dot, and a fab outline with a pin-1 chamfer (mm, Y up, origin at the
//! centre). From the Altium bridge's footprint.rs; each bridge writes the layout in its own library format.
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Pad { pub name: String, pub x: f64, pub y: f64, pub w: f64, pub h: f64, pub shape: String, pub hole: f64 }

#[derive(Debug, Default)]
pub struct Fp { pub pads: Vec<Pad>, pub lines: Vec<(&'static str, f64, f64, f64, f64, f64)>, pub discs: Vec<(&'static str, f64, f64, f64)>, pub courtyard: (f64, f64, f64, f64) }

fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }
fn num(a: &Value, k: &str) -> Option<f64> { a.get(k).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))) }

/// BGA row letters: A..Z without I, O, Q, S, X, Z, then AA, AB, ...
pub fn bga_row(i: usize) -> String {
    const L: &[u8] = b"ABCDEFGHJKLMNPRTUVWY";
    if i < L.len() { (L[i] as char).to_string() } else { format!("{}{}", L[i / L.len() - 1] as char, L[i % L.len()] as char) }
}

/// The pads of a land pattern. Errors name the missing number in the words the caller used.
pub fn pads(a: &Value) -> Result<Vec<Pad>, String> {
    let family = a.get("family").and_then(|f| f.as_str()).unwrap_or("dual").to_lowercase();
    let need = |k: &str| num(a, k).ok_or_else(|| format!("{family} needs {k} (mm)"));
    let shape = a.get("padShape").and_then(|s| s.as_str()).unwrap_or(if family == "bga" { "round" } else { "roundrect" }).to_string();
    let mut out = Vec::new();
    let mut push = |name: String, x: f64, y: f64, w: f64, h: f64| out.push(Pad { name, x: r3(x), y: r3(y), w: r3(w), h: r3(h), shape: shape.clone(), hole: 0.0 });
    match family.as_str() {
        "chip" | "two" | "2pad" => {
            let (span, pl, pw) = (need("span")?, need("padLength")?, need("padWidth")?);
            push("1".into(), -span / 2.0, 0.0, pl, pw);
            push("2".into(), span / 2.0, 0.0, pl, pw);
        }
        "dual" | "sot" => {
            let (span, pitch, pl, pw) = (need("span")?, need("pitch")?, need("padLength")?, need("padWidth")?);
            let (left, right) = if family == "sot" {
                (num(a, "leftPins").ok_or("sot needs leftPins and rightPins")? as usize, num(a, "rightPins").ok_or("sot needs leftPins and rightPins")? as usize)
            } else {
                let n = need("pins")? as usize;
                if n % 2 != 0 { return Err(format!("dual needs an even pin count, got {n}")); }
                (n / 2, n / 2)
            };
            // left column top to bottom, then right column bottom to top (counter-clockwise from pin 1 top-left). A shorter
            // column spreads over the longer one's span, its middle left empty: SOT-23-5's pins 4 and 5 line up with 3 and
            // 1 (kicad-lane #25); a single pin sits in the middle (SOT-23)
            let n = left.max(right);
            let step = |count: usize| if count > 1 && count < n { (n as f64 - 1.0) * pitch / (count as f64 - 1.0) } else { pitch };
            let (ls, rs) = (step(left), step(right));
            for i in 0..left { push((i + 1).to_string(), -span / 2.0, ((left as f64 - 1.0) / 2.0 - i as f64) * ls, pl, pw); }
            for j in 0..right { push((left + j + 1).to_string(), span / 2.0, (j as f64 - (right as f64 - 1.0) / 2.0) * rs, pl, pw); }
        }
        "quad" => {
            let (pitch, pl, pw) = (need("pitch")?, need("padLength")?, need("padWidth")?);
            let span_x = need("span")?;
            let span_y = num(a, "spanY").unwrap_or(span_x);
            let (ny, nx) = match (num(a, "pinsPerSideY"), num(a, "pinsPerSideX")) {
                (Some(y), Some(x)) => (y as usize, x as usize),
                _ => {
                    let n = need("pins")? as usize;
                    if n % 4 != 0 { return Err(format!("quad needs pins divisible by 4, or pinsPerSideX and pinsPerSideY; got {n}")); }
                    (n / 4, n / 4)
                }
            };
            let mut k = 1;
            // left top->bottom, bottom left->right, right bottom->top, top right->left
            for i in 0..ny { push(k.to_string(), -span_x / 2.0, ((ny as f64 - 1.0) / 2.0 - i as f64) * pitch, pl, pw); k += 1; }
            for i in 0..nx { push(k.to_string(), (i as f64 - (nx as f64 - 1.0) / 2.0) * pitch, -span_y / 2.0, pw, pl); k += 1; }
            for i in 0..ny { push(k.to_string(), span_x / 2.0, (i as f64 - (ny as f64 - 1.0) / 2.0) * pitch, pl, pw); k += 1; }
            for i in 0..nx { push(k.to_string(), ((nx as f64 - 1.0) / 2.0 - i as f64) * pitch, span_y / 2.0, pw, pl); k += 1; }
        }
        "bga" => {
            let (pitch, d) = (need("pitch")?, need("ballPad")?);
            let (rows, cols) = (need("rows")? as usize, need("cols")? as usize);
            let missing: Vec<String> = a.get("missing").and_then(|m| m.as_array()).cloned().unwrap_or_default().iter().filter_map(|v| v.as_str().map(|s| s.to_uppercase())).collect();
            for r in 0..rows {
                for c in 0..cols {
                    let name = format!("{}{}", bga_row(r), c + 1);
                    if missing.contains(&name) { continue; }
                    push(name, (c as f64 - (cols as f64 - 1.0) / 2.0) * pitch, ((rows as f64 - 1.0) / 2.0 - r as f64) * pitch, d, d);
                }
            }
        }
        f => return Err(format!("family '{f}' is not one of chip, dual, sot, quad, bga")),
    }
    // the exposed (thermal) pad
    if let Some(ep) = a.get("exposedPad").filter(|e| e.is_object()) {
        let (w, h) = (num(ep, "w").ok_or("exposedPad needs w and h (mm)")?, num(ep, "h").ok_or("exposedPad needs w and h (mm)")?);
        let name = ep.get("number").map(|v| match v { Value::String(s) => s.clone(), o => o.to_string() }).unwrap_or_else(|| (out.len() + 1).to_string());
        out.push(Pad { name, x: r3(num(ep, "x").unwrap_or(0.0)), y: r3(num(ep, "y").unwrap_or(0.0)), w: r3(w), h: r3(h), shape: "rect".into(), hole: 0.0 });
    }
    Ok(out)
}

/// The parts of [a0, a1] that stay outside every blocked interval, kept when at least `min` long.
fn subtract(a0: f64, a1: f64, blocked: &mut [(f64, f64)], min: f64) -> Vec<(f64, f64)> {
    blocked.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());
    let mut out = Vec::new();
    let mut cur = a0;
    for &(b0, b1) in blocked.iter() {
        if b1 <= cur || b0 >= a1 { continue; }
        if b0 > cur && b0 - cur >= min { out.push((cur, b0.min(a1))); }
        cur = cur.max(b1);
        if cur >= a1 { break; }
    }
    if a1 - cur >= min { out.push((cur, a1)); }
    out
}

/// The whole footprint: pads, then silkscreen (the body outline minus `clear` mm around every pad, and a pin-1 dot),
/// the fab outline with a pin-1 chamfer, and the courtyard (pads and body grown by `margin`).
pub fn layout(a: &Value) -> Result<Fp, String> {
    let family = a.get("family").and_then(|f| f.as_str()).unwrap_or("dual").to_lowercase();
    let pads = pads(a)?;
    if pads.is_empty() { return Err("no pads".into()); }
    let body = a.get("body").cloned().unwrap_or(json!({}));
    let (bw, bh) = (num(&body, "w"), num(&body, "l").or_else(|| num(&body, "h")));
    let silk_w = num(a, "silkWidth").unwrap_or(0.15);
    let clear = num(a, "silkClearance").unwrap_or(0.2);
    let margin = num(a, "courtyard").unwrap_or(0.25);
    let ext = |p: &Pad| (p.x - p.w / 2.0, p.y - p.h / 2.0, p.x + p.w / 2.0, p.y + p.h / 2.0);
    let (mut x0, mut y0, mut x1, mut y1) = pads.iter().map(ext).fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |a, e| (a.0.min(e.0), a.1.min(e.1), a.2.max(e.2), a.3.max(e.3)));
    let mut fp = Fp { pads: pads.clone(), ..Default::default() };
    if let (Some(w), Some(h)) = (bw, bh) {
        let (bx0, by0, bx1, by1) = (-w / 2.0, -h / 2.0, w / 2.0, h / 2.0);
        x0 = x0.min(bx0); y0 = y0.min(by0); x1 = x1.max(bx1); y1 = y1.max(by1);
        // silkscreen: each body edge minus the stretches that come within `clear` of copper
        let keep = clear + silk_w / 2.0;
        let mut edge = |horizontal: bool, c: f64, a0: f64, a1: f64| {
            let mut blocked: Vec<(f64, f64)> = pads.iter().filter_map(|p| {
                let (px0, py0, px1, py1) = ext(p);
                if horizontal { (c > py0 - keep && c < py1 + keep).then_some((px0 - keep, px1 + keep)) } else { (c > px0 - keep && c < px1 + keep).then_some((py0 - keep, py1 + keep)) }
            }).collect();
            for (s0, s1) in subtract(a0, a1, &mut blocked, 0.2) {
                if horizontal { fp.lines.push(("overlay", r3(s0), r3(c), r3(s1), r3(c), silk_w)); } else { fp.lines.push(("overlay", r3(c), r3(s0), r3(c), r3(s1), silk_w)); }
            }
        };
        // the outline is drawn just outside the body so it is not hidden under it
        let o = silk_w / 2.0;
        edge(true, by1 + o, bx0 - o, bx1 + o);
        edge(true, by0 - o, bx0 - o, bx1 + o);
        edge(false, bx0 - o, by0 - o, by1 + o);
        edge(false, bx1 + o, by0 - o, by1 + o);
        // fab: the body with its pin-1 corner (top-left) cut
        let ch = (w.min(h) * 0.25).min(1.0);
        let fab = [(bx0 + ch, by1), (bx1, by1), (bx1, by0), (bx0, by0), (bx0, by1 - ch), (bx0 + ch, by1)];
        for k in 0..fab.len() - 1 { fp.lines.push(("fab", r3(fab[k].0), r3(fab[k].1), r3(fab[k + 1].0), r3(fab[k + 1].1), 0.1)); }
    }
    // pin-1 dot: beside pin 1 on the outside, clear of its pad; a two-pad part only when polarized
    let polar = a.get("polarized").and_then(|b| b.as_bool()).unwrap_or(false);
    if family != "chip" || polar {
        let p1 = &pads[0];
        let r = 0.15;
        let (dx, dy) = if family == "bga" { (x0 - clear - r, y1 + clear + r) } else { (p1.x - p1.w / 2.0 - clear - r - silk_w, p1.y) };
        fp.discs.push(("overlay", r3(dx), r3(dy), r));
    }
    // courtyard: the pads and the body grown by the margin, on a 0.01 mm grid. The silkscreen pin-1 dot may sit outside it
    // (IPC-7351; kicad-lane #25: holding the dot pushed a 7 mm BGA's courtyard 0.5 mm out on two sides)
    let g = |v: f64, up: bool| if up { (v * 100.0).ceil() / 100.0 } else { (v * 100.0).floor() / 100.0 };
    let c = (g(x0 - margin, false), g(y0 - margin, false), g(x1 + margin, true), g(y1 + margin, true));
    fp.courtyard = c;
    for (a0, b0, a1, b1) in [(c.0, c.1, c.2, c.1), (c.2, c.1, c.2, c.3), (c.2, c.3, c.0, c.3), (c.0, c.3, c.0, c.1)] {
        fp.lines.push(("courtyard", a0, b0, a1, b1, 0.05));
    }
    Ok(fp)
}

/// A plain description: "SOIC-8, 1.27 mm pitch, 8 pads".
fn describe(a: &Value, fp: &Fp) -> String {
    let fam = a.get("family").and_then(|f| f.as_str()).unwrap_or("dual");
    let pitch = num(a, "pitch").map(|p| format!(", {p} mm pitch")).unwrap_or_default();
    format!("{} {}{}, {} pads (from the datasheet land pattern)", a.get("name").and_then(|n| n.as_str()).unwrap_or(""), fam, pitch, fp.pads.len())
}

/// footprint_from_land_pattern: input {args: {name, family, ..., body?, exposedPad?, description?}}. The layout: pads,
/// lines and discs by layer (overlay, fab, courtyard), the courtyard box, a description and the pad table. A missing
/// number is refused in the caller's words.
pub fn footprint_from_land_pattern(input: &Value) -> Value {
    let args = &input["args"];
    let Some(name) = args["name"].as_str().filter(|n| !n.is_empty()) else { return crate::input::refuse("bad_args", "name is required") };
    let fp = match layout(args) {
        Ok(f) => f,
        Err(e) => return json!({"success": false, "errorCode": "bad_args", "error": e,
            "_hint": "Give the datasheet's RECOMMENDED LAND PATTERN numbers (mm): chip {span, padLength, padWidth}; dual/sot {pins | leftPins+rightPins, pitch, span (pad centre to pad centre across), padLength, padWidth}; quad {pins | pinsPerSideX+pinsPerSideY, pitch, span, spanY?, padLength, padWidth}; bga {rows, cols, pitch, ballPad, missing?}; plus exposedPad {w, h} and body {w, l, height?}."}),
    };
    let desc = args["description"].as_str().map(String::from).unwrap_or_else(|| describe(args, &fp));
    let c = fp.courtyard;
    json!({"success": true, "name": name, "description": desc, "units": "mm", "frame": "Y up, origin at the centre, pin 1 top-left",
        "pads": fp.pads.iter().map(|p| json!({"name": p.name, "x": p.x, "y": p.y, "w": p.w, "h": p.h, "shape": p.shape, "cornerRatio": if p.shape == "roundrect" { 0.25 } else { 0.0 }, "hole": p.hole})).collect::<Vec<_>>(),
        "lines": fp.lines.iter().map(|(l, x1, y1, x2, y2, w)| json!({"layer": l, "x1": x1, "y1": y1, "x2": x2, "y2": y2, "width": w})).collect::<Vec<_>>(),
        "discs": fp.discs.iter().map(|(l, x, y, r)| json!({"layer": l, "x": x, "y": y, "r": r})).collect::<Vec<_>>(),
        "courtyardMm": [c.0, c.1, c.2, c.3], "body": args["body"].clone(),
        "padTable": fp.pads.iter().map(|p| json!([p.name, p.x, p.y, p.w, p.h])).collect::<Vec<_>>(),
        "_hint": "A layout, not a library: write it in the tool's own footprint format (overlay is the silkscreen, fab the assembly outline, courtyard the keep-out), then check padTable against the datasheet. With body.height, generate_body makes the 3D body to attach."})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soic8_numbers_down_the_left_and_up_the_right() {
        let p = pads(&json!({"family": "dual", "pins": 8, "pitch": 1.27, "span": 5.4, "padLength": 1.55, "padWidth": 0.6})).unwrap();
        assert_eq!((p[0].name.as_str(), p[0].x, p[0].y), ("1", -2.7, 1.905));
        assert_eq!((p[3].name.as_str(), p[3].y), ("4", -1.905));
        assert_eq!((p[4].name.as_str(), p[4].x, p[4].y), ("5", 2.7, -1.905));
        assert_eq!((p[7].name.as_str(), p[7].y), ("8", 1.905));
        assert_eq!((p[0].w, p[0].h), (1.55, 0.6));
    }

    #[test]
    fn quad_goes_counter_clockwise_and_turns_the_top_and_bottom_pads() {
        let p = pads(&json!({"family": "quad", "pins": 16, "pitch": 0.5, "span": 3.0, "padLength": 0.8, "padWidth": 0.25, "exposedPad": {"w": 1.7, "h": 1.7}})).unwrap();
        assert_eq!(p.len(), 17);
        assert_eq!((p[0].x, p[0].y), (-1.5, 0.75));      // 1: left, top
        assert_eq!((p[4].x, p[4].y, p[4].w), (-0.75, -1.5, 0.25)); // 5: bottom, left end, turned
        assert_eq!((p[8].x, p[8].y), (1.5, -0.75));      // 9: right, bottom
        assert_eq!((p[12].x, p[12].y), (0.75, 1.5));     // 13: top, right end
        assert_eq!((p[16].name.as_str(), p[16].shape.as_str()), ("17", "rect"));
    }

    #[test]
    fn sot23_has_two_pins_left_and_one_right() {
        let p = pads(&json!({"family": "sot", "leftPins": 2, "rightPins": 1, "pitch": 1.9, "span": 2.2, "padLength": 1.0, "padWidth": 0.6})).unwrap();
        assert_eq!(p.iter().map(|q| (q.name.clone(), q.x, q.y)).collect::<Vec<_>>(), vec![("1".into(), -1.1, 0.95), ("2".into(), -1.1, -0.95), ("3".into(), 1.1, 0.0)]);
        // SOT-23-5: pins 4 and 5 line up with 3 and 1 (kicad-lane #25)
        let q = pads(&json!({"family": "sot", "leftPins": 3, "rightPins": 2, "pitch": 0.95, "span": 2.6, "padLength": 1.1, "padWidth": 0.6})).unwrap();
        assert_eq!(q.iter().map(|p| (p.name.clone(), p.y)).collect::<Vec<_>>(), vec![("1".into(), 0.95), ("2".into(), 0.0), ("3".into(), -0.95), ("4".into(), -0.95), ("5".into(), 0.95)]);
    }

    #[test]
    fn bga_rows_skip_confusable_letters() {
        assert_eq!((bga_row(0), bga_row(7), bga_row(8), bga_row(19), bga_row(20)), ("A".into(), "H".into(), "J".into(), "Y".into(), "AA".into()));
        let p = pads(&json!({"family": "bga", "rows": 3, "cols": 3, "pitch": 0.8, "ballPad": 0.4, "missing": ["b2"]})).unwrap();
        assert_eq!(p.len(), 8);
        assert_eq!((p[0].name.as_str(), p[0].x, p[0].y), ("A1", -0.8, 0.8));
    }

    #[test]
    fn silkscreen_keeps_clear_of_copper_and_the_courtyard_holds_everything() {
        let a = json!({"family": "dual", "pins": 8, "pitch": 1.27, "span": 5.4, "padLength": 1.55, "padWidth": 0.6, "body": {"w": 3.9, "l": 4.9}});
        let fp = layout(&a).unwrap();
        let ext = |p: &Pad| (p.x - p.w / 2.0, p.y - p.h / 2.0, p.x + p.w / 2.0, p.y + p.h / 2.0);
        for &(l, x0, y0, x1, y1, w) in fp.lines.iter().filter(|l| l.0 == "overlay") {
            let _ = l;
            for p in &fp.pads {
                let (px0, py0, px1, py1) = ext(p);
                // distance from the segment (axis-aligned) to the pad box, minus half the stroke
                let dx = (px0 - x0.max(x1)).max(x0.min(x1) - px1).max(0.0);
                let dy = (py0 - y0.max(y1)).max(y0.min(y1) - py1).max(0.0);
                assert!((dx * dx + dy * dy).sqrt() - w / 2.0 >= 0.199, "silk {x0},{y0}-{x1},{y1} too close to pad {}", p.name);
            }
        }
        // a SOIC's body sides sit under its pads: only the top and bottom edges survive
        assert_eq!(fp.lines.iter().filter(|l| l.0 == "overlay").count(), 2);
        let c = fp.courtyard;
        assert!(fp.pads.iter().all(|p| { let (a0, b0, a1, b1) = ext(p); a0 >= c.0 && b0 >= c.1 && a1 <= c.2 && b1 <= c.3 }));
        assert_eq!(fp.discs.len(), 1);
        // the courtyard holds the pads and the body, not the pin-1 dot (kicad-lane #25): a 7 mm BGA is symmetric
        let bga = layout(&json!({"family": "bga", "rows": 8, "cols": 8, "pitch": 0.8, "ballPad": 0.4, "body": {"w": 7, "l": 7}})).unwrap();
        assert_eq!(bga.courtyard, (-3.75, -3.75, 3.75, 3.75));
        assert!(fp.discs[0].1 < -2.7 - 0.775);

    }

    #[test]
    fn a_missing_number_is_named() {
        let e = pads(&json!({"family": "dual", "pins": 8, "pitch": 1.27, "span": 5.4, "padWidth": 0.6})).unwrap_err();
        assert!(e.contains("padLength"));
    }

    #[test]
    fn the_verb_answers_the_layout_in_the_standard_shape() {
        let r = footprint_from_land_pattern(&json!({"args": {"name": "SOIC-8", "family": "dual", "pins": 8, "pitch": 1.27, "span": 5.4, "padLength": 1.55, "padWidth": 0.6, "body": {"w": 3.9, "l": 4.9, "height": 1.75}}}));
        assert_eq!(r["success"], json!(true), "{r}");
        assert_eq!(r["pads"][0], json!({"name": "1", "x": -2.7, "y": 1.905, "w": 1.55, "h": 0.6, "shape": "roundrect", "cornerRatio": 0.25, "hole": 0.0}));
        assert!(r["lines"].as_array().unwrap().iter().any(|l| l["layer"] == "courtyard"));
        assert_eq!(r["discs"][0]["layer"], "overlay");
        assert_eq!(r["padTable"].as_array().unwrap().len(), 8);
        assert!(r["description"].as_str().unwrap().starts_with("SOIC-8 dual, 1.27 mm pitch, 8 pads"));
        let e = footprint_from_land_pattern(&json!({"args": {"name": "X", "family": "dual", "pins": 8, "pitch": 1.27, "span": 5.4, "padWidth": 0.6}}));
        assert_eq!(e["errorCode"], "bad_args");
        assert!(e["error"].as_str().unwrap().contains("padLength"));
        assert_eq!(footprint_from_land_pattern(&json!({"args": {"family": "chip"}}))["errorCode"], "bad_args");
    }
}

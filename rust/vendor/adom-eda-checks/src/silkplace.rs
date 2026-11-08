//! Reference designators placed the way assembly houses want them: one size for all, upright, beside their own part,
//! clear of every pad (JLCPCB: silkscreen 0.15 mm from pads), every other part's body and every designator already
//! placed, and inside the board. From the Altium bridge's silkplace.rs: a plan here, which the bridge applies with
//! apply_silkscreen and checks against the tool's own text boxes after.
use serde_json::{json, Value};

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }

type Rect = (f64, f64, f64, f64);

fn rect_of(v: &Value) -> Option<Rect> {
    let a = v.as_array()?;
    if a.len() != 4 { return None; }
    let n: Vec<f64> = a.iter().map(f).collect();
    Some((n[0].min(n[2]), n[1].min(n[3]), n[0].max(n[2]), n[1].max(n[3])))
}
fn grow(r: Rect, d: f64) -> Rect { (r.0 - d, r.1 - d, r.2 + d, r.3 + d) }
fn hit(a: Rect, b: Rect) -> bool { a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3 }
fn union(a: Rect, b: Rect) -> Rect { (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)) }

/// A pad's copper as a rectangle (its rotated extent).
fn pad_rect(p: &Value) -> Rect {
    let (c, s) = (f(&p["rotation"]).to_radians().cos().abs(), f(&p["rotation"]).to_radians().sin().abs());
    let (sx, sy) = (f(&p["sizeX"]), f(&p["sizeY"]));
    let (hx, hy) = ((sx * c + sy * s) / 2.0, (sx * s + sy * c) / 2.0);
    (f(&p["x"]) - hx, f(&p["y"]) - hy, f(&p["x"]) + hx, f(&p["y"]) + hy)
}

fn on_side(layer: &Value, bottom: bool) -> bool {
    match layer.as_str() { Some("*.Cu") => true, Some("B.Cu") => bottom, Some("F.Cu") => !bottom, _ => false }
}

/// Where each designator goes: [{ref, x, y, size, rotation, side, at}] (x, y the text's lower-left, its right on the
/// bottom side where Altium mirrors it) and the parts with no clear spot. Pure: silk_state's components, the board's
/// pads and outline bounding box, and the boxes of labels that stay put.
pub fn plan(comps: &[Value], pads: &[Value], board: Rect, size: f64, min_size: f64, gap: f64, pad_clear: f64, fixed: &[(bool, Rect)]) -> (Vec<Value>, Vec<Value>) {
    // labels that stay where they are (parts left out, or with no spot) are obstacles like the ones placed
    let mut placed: Vec<(bool, Rect)> = fixed.to_vec();
    let (mut out, mut unplaced) = (Vec::new(), Vec::new());
    // the part's own outline: its body box and its own pads
    let own = |c: &Value| -> Option<Rect> {
        let r = c["ref"].as_str().unwrap_or("");
        let mut b = rect_of(&c["body"]);
        for p in pads.iter().filter(|p| p["ref"].as_str() == Some(r)) { b = Some(b.map(|x| union(x, pad_rect(p))).unwrap_or(pad_rect(p))); }
        b
    };
    let bodies: Vec<(String, bool, Rect)> = comps.iter().filter_map(|c| own(c).map(|b| (c["ref"].as_str().unwrap_or("").to_string(), c["side"] == "bottom", b))).collect();
    // big parts first: they have the fewest places to put a label and the small ones fit round them
    let mut order: Vec<&Value> = comps.iter().filter(|c| c["name"]["shown"].as_bool() != Some(false)).collect();
    order.sort_by(|a, b| {
        let area = |c: &Value| own(c).map(|r| (r.2 - r.0) * (r.3 - r.1)).unwrap_or(0.0);
        area(b).partial_cmp(&area(a)).unwrap()
    });
    let inside = grow(board, -0.2);
    for c in order {
        let rf = c["ref"].as_str().unwrap_or("").to_string();
        let bottom = c["side"] == "bottom";
        let Some(b) = own(c) else { unplaced.push(json!({"ref": rf, "why": "no body or pads to place it by"})); continue };
        if b.2 < board.0 || b.0 > board.2 || b.3 < board.1 || b.1 > board.3 {
            unplaced.push(json!({"ref": rf, "why": "the part is off the board: place it first (move_parts)"}));
            continue;
        }
        // the label's size at the new height, from Altium's own box at the old one
        // Altium's box is the text plus half its stroke all round (a 1.0 mm designator measures 1.285 mm tall on the
        // Bluetooth Sentinel example): plan the text, check the box
        let old = f(&c["name"]["size"]);
        let turned = (f(&c["name"]["rotation"]).rem_euclid(180.0) - 90.0).abs() < 1.0;
        let (m, w) = match rect_of(&c["name"]["box"]) {
            Some(r) if old > 0.0 && r.2 > r.0 => {
                let (bw, bh) = if turned { (r.3 - r.1, r.2 - r.0) } else { (r.2 - r.0, r.3 - r.1) };
                let m = ((bh - old) / 2.0).clamp(0.0, 0.3);
                (m, (bw - 2.0 * m).max(old * 0.5) * size / old)
            }
            _ => (0.15, rf.len() as f64 * size * 0.8),
        };
        let free = |text: Rect| -> bool {
            let r = grow(text, m);
            r.0 >= inside.0 && r.1 >= inside.1 && r.2 <= inside.2 && r.3 <= inside.3
                && !pads.iter().any(|p| on_side(&p["layer"], bottom) && hit(r, grow(pad_rect(p), pad_clear)))
                && !bodies.iter().any(|(o, side, ob)| *o != rf && *side == bottom && hit(r, *ob))
                && !placed.iter().any(|(side, pr)| *side == bottom && hit(r, grow(*pr, 0.1)))
        };
        // the sizes to try: `size`, then down in 0.1 mm steps to `min_size`
        let mut found = None;
        let mut s = size;
        while found.is_none() && s >= min_size - 1e-9 {
            found = spots(b, w * s / size, s, gap, bottom).into_iter().find(|(r, _, _)| free(*r)).map(|(r, rot, at)| (r, rot, at, s));
            s -= 0.1;
        }
        match found {
            Some((r, rot, at, s)) => {
                placed.push((bottom, grow(r, m)));
                // the text's anchor: lower left upright; turned 90 its letters lie left of the anchor (lower right of the
                // box); mirrored on the bottom it is the text's right end
                let (ax, ay) = if rot == 90 { (r.2, r.1) } else if bottom { (r.2, r.1) } else { (r.0, r.1) };
                out.push(json!({"ref": rf, "x": r3(ax), "y": r3(ay), "size": r3(s), "rotation": rot, "side": if bottom { "bottom" } else { "top" }, "at": at}));
            }
            None => unplaced.push(json!({"ref": rf, "why": "no clear spot beside the part", "triedSizes": [size, min_size]})),
        }
    }
    (out, unplaced)
}

/// Places a w x h label could go round the part box b, nearest first: centred on each side, then slid along it, at
/// `gap` and then 3 x `gap`; turned 90 degrees beside the left and right sides (top side only: Altium mirrors bottom
/// text). Each is (box, rotation, where).
fn spots(b: Rect, w: f64, h: f64, gap: f64, bottom: bool) -> Vec<(Rect, i64, String)> {
    let mut v = Vec::new();
    let (cx, cy) = ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0);
    for g in [gap, 3.0 * gap] {
        // slide offsets along a side: centre first, then outwards both ways in 0.5 mm steps
        let slide = |len: f64| -> Vec<f64> {
            let mut o = vec![0.0];
            let mut k = 0.5;
            while k <= len / 2.0 + 0.5 { o.push(-k); o.push(k); k += 0.5; }
            o
        };
        for d in slide(b.2 - b.0) {
            v.push(((cx - w / 2.0 + d, b.3 + g, cx + w / 2.0 + d, b.3 + g + h), 0, "above".to_string()));
            v.push(((cx - w / 2.0 + d, b.1 - g - h, cx + w / 2.0 + d, b.1 - g), 0, "below".to_string()));
        }
        for d in slide(b.3 - b.1) {
            v.push(((b.2 + g, cy - h / 2.0 + d, b.2 + g + w, cy + h / 2.0 + d), 0, "right".to_string()));
            v.push(((b.0 - g - w, cy - h / 2.0 + d, b.0 - g, cy + h / 2.0 + d), 0, "left".to_string()));
            if !bottom {
                v.push(((b.2 + g, cy - w / 2.0 + d, b.2 + g + h, cy + w / 2.0 + d), 90, "right, turned".to_string()));
                v.push(((b.0 - g - h, cy - w / 2.0 + d, b.0 - g, cy + w / 2.0 + d), 90, "left, turned".to_string()));
            }
        }
    }
    v
}

/// Designators whose box (Altium's own) still touches a pad, closer than `pad_clear`. Pure.
pub fn on_pads(comps: &[Value], pads: &[Value], pad_clear: f64) -> Vec<Value> {
    comps.iter().filter(|c| c["name"]["shown"].as_bool() != Some(false)).filter_map(|c| {
        let r = rect_of(&c["name"]["box"])?;
        let bottom = c["side"] == "bottom";
        let p = pads.iter().find(|p| on_side(&p["layer"], bottom) && hit(r, grow(pad_rect(p), pad_clear - 0.01)))?;
        Some(json!({"ref": c["ref"], "pad": format!("{}.{}", p["ref"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or(""))}))
    }).collect()
}

/// silk_designators: input {board, silk, args: {size?, minSize?, gap?, padClearance?, refs?}}. The plan: `planned`,
/// and `applyArgs` ready for apply_silkscreen (x, y the text's lower-left; its right end on the bottom side, where the
/// text is mirrored), the parts with no clear spot, and the labels on pads before.
pub fn silk_designators(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let num = |k: &str, d: f64| args.get(k).and_then(|v| v.as_f64()).unwrap_or(d);
    let (size, gap, clear) = (num("size", 1.0), num("gap", 0.2), num("padClearance", 0.15));
    if size < 0.5 { return crate::input::refuse("bad_args", "size is the text height in mm, 0.5 or more (JLCPCB reads 1.0 mm and up reliably)"); }
    let Some(bb) = crate::geom::outline_bbox(&st) else { return crate::input::refuse("no_outline", "the board has no outline") };
    let Some(silk) = crate::input::part(input, "silk") else { return crate::input::refuse("bad_args", "pass silk_state as silk: each part's body box and designator") };
    let all = silk["components"].as_array().cloned().unwrap_or_default();
    let mut comps = all.clone();
    if let Some(want) = args.get("refs").and_then(|v| v.as_array()) {
        let w: Vec<&str> = want.iter().filter_map(|v| v.as_str()).collect();
        comps.retain(|c| c["ref"].as_str().map(|r| w.iter().any(|x| x.eq_ignore_ascii_case(r))).unwrap_or(false));
    }
    let pads = st["pads"].as_array().cloned().unwrap_or_default();
    let before = on_pads(&comps, &pads, clear);
    let min_size = num("minSize", size).min(size);
    // the labels of parts left out stay put; so do the labels of parts with no spot, so plan again round those
    let label = |c: &Value| -> Option<(bool, Rect)> { if c["name"]["shown"].as_bool() == Some(false) { None } else { rect_of(&c["name"]["box"]).map(|r| (c["side"] == "bottom", r)) } };
    let mut fixed: Vec<(bool, Rect)> = all.iter().filter(|c| !comps.iter().any(|k| k["ref"] == c["ref"])).filter_map(label).collect();
    let mut moving = comps.clone();
    let (mut names, mut unplaced) = (Vec::new(), Vec::new());
    // every round pins at least one more label, so this ends; the last round leaves nothing unpinned
    for _ in 0..=comps.len() {
        let (n, u) = plan(&moving, &pads, bb, size, min_size, gap, clear, &fixed);
        names = n;
        if u.is_empty() { break; }
        let stuck: Vec<Value> = u.iter().map(|x| x["ref"].clone()).collect();
        fixed.extend(moving.iter().filter(|c| stuck.contains(&c["ref"])).filter_map(label));
        moving.retain(|c| !stuck.contains(&c["ref"]));
        unplaced.extend(u);
    }
    let apply: Vec<Value> = names.iter().map(|n| json!({"ref": n["ref"], "x": n["x"], "y": n["y"], "size": n["size"], "rotation": n["rotation"]})).collect();
    json!({"success": true, "planned": names, "unplaced": unplaced, "onPadsBefore": before, "sizeMm": size, "applyArgs": {"names": apply},
        "_hint": "A plan: every label upright (or turned 90 beside the part), one size, beside its own part, clear of pads by padClearance and of other bodies and labels. Send applyArgs to apply_silkscreen, then read silk_state back and check no label touches a pad with the tool's own text boxes. unplaced parts had no clear spot: try minSize for them (refs), move the part, or place the label by hand."})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp(r: &str, body: [f64; 4]) -> Value {
        json!({"ref": r, "side": "top", "body": body, "name": {"text": r, "size": 1.0, "shown": true, "box": [0.0, 0.0, 2.0, 1.0]}})
    }

    #[test]
    fn labels_go_beside_their_part_clear_of_pads_and_each_other() {
        // C1 sits just above U1, where U1's label would go
        let comps = vec![comp("U1", [10.0, 10.0, 16.0, 16.0]), comp("C1", [12.0, 16.4, 14.0, 17.4])];
        let pads = vec![json!({"ref": "C1", "name": "1", "x": 12.5, "y": 16.9, "sizeX": 0.8, "sizeY": 0.8, "rotation": 0, "layer": "F.Cu"}),
                        json!({"ref": "C1", "name": "2", "x": 13.5, "y": 16.9, "sizeX": 0.8, "sizeY": 0.8, "rotation": 0, "layer": "F.Cu"})];
        let (out, un) = plan(&comps, &pads, (0.0, 0.0, 40.0, 40.0), 1.0, 1.0, 0.2, 0.15, &[]);
        assert!(un.is_empty());
        // U1 above would land on C1's body and pads: it goes below
        assert_eq!(out[0]["ref"], "U1");
        assert_eq!(out[0]["at"], "below");
        assert_eq!(out[1]["ref"], "C1");
        assert_eq!(out[1]["at"], "above");
        assert_eq!(out[1]["y"], 17.6);
        assert_eq!(out[0]["y"], 8.8);
    }

    #[test]
    fn a_part_boxed_in_is_reported_not_forced() {
        let comps = vec![comp("R1", [1.0, 1.0, 3.0, 2.0])];
        let (out, un) = plan(&comps, &[], (0.0, 0.0, 4.0, 3.0), 1.0, 1.0, 0.2, 0.15, &[]);
        assert!(out.is_empty());
        assert_eq!(un[0]["ref"], "R1");
        let on = on_pads(&[json!({"ref": "R1", "side": "top", "name": {"shown": true, "box": [0.0, 0.0, 2.0, 1.0]}})],
                         &[json!({"ref": "R1", "name": "1", "x": 1.0, "y": 0.5, "sizeX": 0.5, "sizeY": 0.5, "rotation": 0, "layer": "F.Cu"})], 0.15);
        assert_eq!(on.len(), 1);
    }
}

//! The board in 3D from its parts' bodies (the bodies companion): how tall each part stands on its side, which ones break
//! an enclosure's height limit (board-wide, or under a lid feature in a region), and which bodies collide or come closer
//! than a gap. Bodies are compared as boxes (each body's footprint rectangle and its standoff-to-top range); a part made
//! of several bodies is several boxes, so L-shapes stay honest; a bridge that gives one box per part (Fusion) is the
//! union. The tool's own component clearance DRC checks the real shapes. From the Altium bridge's bodies.rs.
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }
fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }

/// One body as a box in mm: its part, side (top/bottom), x and y extent, and z from the board surface it sits on; a
/// turned body also has its footprint as a rectangle's four corners (`turned`), and x/y extent is their bounding box.
#[derive(Debug, Clone)]
pub struct BodyBox { pub part: String, pub top: bool, pub x0: f64, pub y0: f64, pub x1: f64, pub y1: f64, pub z0: f64, pub z1: f64, pub turned: Option<Vec<(f64, f64)>> }

/// A body row's turned rectangle: `{cx, cy, w, h, rotation}` (degrees counterclockwise) or `corners` (four [x, y]).
fn turned(r: &Value) -> Option<Vec<(f64, f64)>> {
    if let Some(cs) = r["corners"].as_array().filter(|a| a.len() == 4) { return Some(cs.iter().map(|c| (f(&c[0]), f(&c[1]))).collect()); }
    if r["cx"].is_number() && r["w"].is_number() && r["h"].is_number() {
        let (cx, cy, hw, hh, a) = (f(&r["cx"]), f(&r["cy"]), f(&r["w"]) / 2.0, f(&r["h"]) / 2.0, f(&r["rotation"]).to_radians());
        let (c, s) = (a.cos(), a.sin());
        return Some([(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].iter().map(|(u, v)| (cx + u * c - v * s, cy + u * s + v * c)).collect());
    }
    None
}

/// The bodies companion (`bodies[{ref, side, x0, y0, x1, y1, z0, z1}]`, mm, and for a turned body `{cx, cy, w, h,
/// rotation}` or `corners`) as boxes. Pure.
pub fn boxes(rows: &[Value]) -> Vec<BodyBox> {
    rows.iter().filter(|r| r["ref"].as_str().map(|s| !s.is_empty()).unwrap_or(false)).map(|r| {
        let t = turned(r);
        let (x0, y0, x1, y1) = match &t {
            Some(cs) => cs.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, p| (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1))),
            None => (f(&r["x0"]).min(f(&r["x1"])), f(&r["y0"]).min(f(&r["y1"])), f(&r["x0"]).max(f(&r["x1"])), f(&r["y0"]).max(f(&r["y1"]))),
        };
        BodyBox { part: r["ref"].as_str().unwrap_or("").to_string(), top: r["side"].as_str() != Some("bottom"), x0, y0, x1, y1, z0: f(&r["z0"]), z1: f(&r["z1"]), turned: t }
    }).collect()
}

/// How two bodies' footprints meet in x and y: (true, how deep) when they overlap, else (false, the gap between them).
/// Axis boxes compare as boxes; when either is turned, both are compared as rectangles by separating axes, so a part at
/// 45 degrees no longer counts its box's empty corners (kicad-lane #26: 70 false collisions round a turned QFN). Pure.
pub fn xy_meet(a: &BodyBox, b: &BodyBox) -> (bool, f64) {
    if a.turned.is_none() && b.turned.is_none() {
        let dx = (b.x0 - a.x1).max(a.x0 - b.x1);
        let dy = (b.y0 - a.y1).max(a.y0 - b.y1);
        return if dx < 0.0 && dy < 0.0 { (true, -(dx.max(dy))) } else { (false, (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt()) };
    }
    let poly = |x: &BodyBox| x.turned.clone().unwrap_or_else(|| vec![(x.x0, x.y0), (x.x1, x.y0), (x.x1, x.y1), (x.x0, x.y1)]);
    let (pa, pb) = (poly(a), poly(b));
    // separating axes: each edge's normal of either rectangle
    let mut sep = f64::MIN;
    for p in [&pa, &pb] {
        for i in 0..p.len() {
            let (e0, e1) = (p[i], p[(i + 1) % p.len()]);
            let (nx, ny) = (e0.1 - e1.1, e1.0 - e0.0);
            let l = (nx * nx + ny * ny).sqrt();
            if l < 1e-12 { continue; }
            let proj = |q: &Vec<(f64, f64)>| q.iter().map(|v| (v.0 * nx + v.1 * ny) / l).fold((f64::MAX, f64::MIN), |m, d| (m.0.min(d), m.1.max(d)));
            let ((a0, a1), (b0, b1)) = (proj(&pa), proj(&pb));
            sep = sep.max((b0 - a1).max(a0 - b1));
        }
    }
    if sep < 0.0 { return (true, -sep); }
    // apart: the shortest distance between their edges
    let seg = |p: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l2 = dx * dx + dy * dy;
        let t = if l2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) };
        ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
    };
    let mut d = f64::MAX;
    for (p, q) in [(&pa, &pb), (&pb, &pa)] {
        for v in p.iter() { for i in 0..q.len() { d = d.min(seg(*v, q[i], q[(i + 1) % q.len()])); } }
    }
    (false, d)
}

/// The input's bodies: the companion's `bodies`, or the companion given as a bare list.
pub fn input_boxes(input: &Value) -> Option<Vec<BodyBox>> {
    let b = crate::input::part(input, "bodies")?;
    let rows = b.get("bodies").or(Some(&b)).and_then(|x| x.as_array()).cloned()?;
    Some(boxes(&rows))
}

/// Parts on the board with no body: the companion's `noBody`, and every board part the boxes leave out.
fn no_body(input: &Value, bs: &[BodyBox]) -> Vec<Value> {
    let with: std::collections::BTreeSet<&str> = bs.iter().map(|b| b.part.as_str()).collect();
    let mut out: Vec<String> = crate::input::part(input, "bodies").and_then(|b| b["noBody"].as_array().cloned()).unwrap_or_default()
        .iter().filter_map(|v| v.as_str().map(String::from)).collect();
    if let Ok(st) = crate::input::board(input) {
        for r in st["components"].as_array().cloned().unwrap_or_default().iter().filter_map(|c| c["ref"].as_str().map(String::from)) {
            if !with.contains(r.as_str()) && !out.contains(&r) { out.push(r); }
        }
    }
    out.into_iter().map(Value::from).collect()
}

/// Each part's height above its side and its footprint in mm, tallest first.
pub fn heights(bs: &[BodyBox]) -> Vec<Value> {
    let mut m: BTreeMap<&str, (bool, f64, f64, f64, f64, f64)> = BTreeMap::new();
    for b in bs {
        let e = m.entry(&b.part).or_insert((b.top, 0.0, f64::MAX, f64::MAX, f64::MIN, f64::MIN));
        e.1 = e.1.max(b.z1);
        e.2 = e.2.min(b.x0); e.3 = e.3.min(b.y0); e.4 = e.4.max(b.x1); e.5 = e.5.max(b.y1);
    }
    let mut v: Vec<Value> = m.into_iter().map(|(p, (top, h, x0, y0, x1, y1))| json!({"ref": p, "side": if top { "top" } else { "bottom" }, "heightMm": r3(h), "rect": [r3(x0), r3(y0), r3(x1), r3(y1)]})).collect();
    v.sort_by(|a, b| b["heightMm"].as_f64().partial_cmp(&a["heightMm"].as_f64()).unwrap());
    v
}

/// Body pairs of different parts on the same side that overlap in 3D (`collisions`), or whose boxes come within
/// `gap` mm of each other in x and y where their heights overlap (`tooClose`). Pure.
pub fn clashes(bs: &[BodyBox], gap: f64) -> (Vec<Value>, Vec<Value>) {
    let mut idx: Vec<usize> = (0..bs.len()).collect();
    idx.sort_by(|a, b| bs[*a].x0.partial_cmp(&bs[*b].x0).unwrap());
    let mut seen: BTreeMap<(String, String), (bool, f64, Value)> = BTreeMap::new();
    for (k, &i) in idx.iter().enumerate() {
        let a = &bs[i];
        for &j in &idx[k + 1..] {
            let b = &bs[j];
            if b.x0 > a.x1 + gap { break; }
            if a.part == b.part || a.top != b.top { continue; }
            let dz = a.z1.min(b.z1) - a.z0.max(b.z0);
            if dz <= 0.0 { continue; } // one sits above the other (a part under a connector's overhang)
            let (hit, d) = xy_meet(a, b);
            if !hit && d >= gap { continue; }
            let key = if a.part < b.part { (a.part.clone(), b.part.clone()) } else { (b.part.clone(), a.part.clone()) };
            let at = json!([r3((a.x0.max(b.x0) + a.x1.min(b.x1)) / 2.0), r3((a.y0.max(b.y0) + a.y1.min(b.y1)) / 2.0)]);
            // per pair keep the worst: a collision over a near miss, then the deepest or the closest
            let worse = match seen.get(&key) { None => true, Some((h, v, _)) => (hit && !h) || (hit == *h && if hit { d > *v } else { d < *v }) };
            if worse { seen.insert(key, (hit, d, at)); }
        }
    }
    let (mut col, mut near) = (Vec::new(), Vec::new());
    for ((p, q), (hit, d, at)) in seen {
        if hit { col.push(json!({"parts": [p, q], "overlapMm": r3(d), "at": at})); } else { near.push(json!({"parts": [p, q], "gapMm": r3(d), "at": at})); }
    }
    (col, near)
}

/// height_check: input {bodies, board?, args: {maxTopMm?, maxBottomMm?, regions?: [{rect, maxMm, side?, name?}], minGapMm?, limit?}}.
/// `board` names the parts with no body (they have no height, and the reply says the check is not complete).
pub fn height_check(input: &Value) -> Value {
    let args = &input["args"];
    let Some(bs) = input_boxes(input) else { return crate::input::refuse("no_bodies", "the input has no bodies: pass the bodies companion (<prefix>_bodies) as bodies") };
    let hs = heights(&bs);
    let num = |k: &str| args.get(k).and_then(|v| v.as_f64());
    let mut over = Vec::new();
    for (side, lim) in [("top", num("maxTopMm")), ("bottom", num("maxBottomMm"))] {
        let Some(lim) = lim else { continue };
        for h in hs.iter().filter(|h| h["side"] == side) {
            let mm = h["heightMm"].as_f64().unwrap_or(0.0);
            if mm > lim { over.push(json!({"ref": h["ref"], "side": side, "heightMm": mm, "limitMm": lim, "overByMm": r3(mm - lim)})); }
        }
    }
    // a region is a lid feature over part of the board (a boss, a rib, a display): parts under it must stay lower
    let mut region_hits = Vec::new();
    for (i, rg) in args.get("regions").and_then(|v| v.as_array()).cloned().unwrap_or_default().iter().enumerate() {
        let rc: Vec<f64> = rg["rect"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect()).unwrap_or_default();
        let Some(max) = rg["maxMm"].as_f64() else { return json!({"success": false, "errorCode": "bad_args", "error": "each region is {rect:[x1,y1,x2,y2], maxMm, side?, name?}"}) };
        if rc.len() != 4 { return json!({"success": false, "errorCode": "bad_args", "error": "region.rect is [x1, y1, x2, y2] in mm"}); }
        let (rx0, ry0, rx1, ry1) = (rc[0].min(rc[2]), rc[1].min(rc[3]), rc[0].max(rc[2]), rc[1].max(rc[3]));
        let top = rg["side"].as_str() != Some("bottom");
        let name = rg["name"].as_str().map(|s| s.to_string()).unwrap_or(format!("region {}", i + 1));
        let mut worst: BTreeMap<&str, f64> = BTreeMap::new();
        for b in bs.iter().filter(|b| b.top == top && b.x1 > rx0 && b.x0 < rx1 && b.y1 > ry0 && b.y0 < ry1 && b.z1 > max) {
            let e = worst.entry(&b.part).or_insert(0.0);
            *e = e.max(b.z1);
        }
        for (p, h) in worst { region_hits.push(json!({"region": name, "ref": p, "heightMm": r3(h), "maxMm": max, "overByMm": r3(h - max)})); }
    }
    let gap = num("minGapMm").unwrap_or(0.0);
    let (collisions, too_close) = clashes(&bs, gap);
    // parts with no body have no height: say so, a height check that skips them is not a pass
    let no_body = no_body(input, &bs);
    let tallest = |side: &str| hs.iter().find(|h| h["side"] == side).cloned().unwrap_or(Value::Null);
    let ok = over.is_empty() && region_hits.is_empty() && collisions.is_empty() && too_close.is_empty();
    json!({"success": true, "ok": ok, "complete": no_body.is_empty(), "bodies": bs.len(), "parts": hs.len(), "tallest": {"top": tallest("top"), "bottom": tallest("bottom")},
        "overLimit": over, "regionHits": region_hits, "collisions": collisions, "tooClose": too_close, "minGapMm": gap,
        "noBody": no_body, "heights": hs.iter().take(args.get("limit").and_then(|v| v.as_u64()).unwrap_or(25) as usize).collect::<Vec<_>>(),
        "_hint": "Heights are each part's tallest body above the surface it sits on (bottom-side parts below the bottom). collisions and tooClose compare body boxes; the tool's component clearance DRC (run_drc) checks the real shapes. ok covers the parts that have bodies; complete is false while noBody lists parts with no 3D body and so no height: give them one (add_body) before trusting the check."})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(p: &str, top: bool, x0: f64, y0: f64, x1: f64, y1: f64, z0: f64, z1: f64) -> BodyBox {
        BodyBox { part: p.into(), top, x0, y0, x1, y1, z0, z1, turned: None }
    }

    #[test]
    fn the_companion_becomes_boxes() {
        let b = boxes(&[json!({"ref": "J1", "side": "bottom", "x0": 10.0, "y0": 5.0, "x1": 0.0, "y1": 0.0, "z0": 0.0, "z1": 4.1}), json!({"ref": "", "x0": 1})]);
        assert_eq!(b.len(), 1);
        assert!(!b[0].top && (b[0].z1 - 4.1).abs() < 1e-9 && (b[0].x1 - 10.0).abs() < 1e-9 && b[0].x0 == 0.0);
    }

    #[test]
    fn heights_take_each_parts_tallest_body() {
        let bs = vec![bx("J1", true, 0.0, 0.0, 10.0, 5.0, 0.0, 3.2), bx("J1", true, 0.0, 0.0, 10.0, 5.0, 4.0, 4.1), bx("R1", true, 20.0, 0.0, 21.0, 0.5, 0.0, 0.4)];
        let h = heights(&bs);
        assert_eq!(h[0]["ref"], "J1");
        assert_eq!(h[0]["heightMm"], 4.1);
        assert_eq!(h[1]["rect"], json!([20.0, 0.0, 21.0, 0.5]));
    }

    #[test]
    fn clashes_find_overlaps_near_misses_and_ignore_stacked_and_opposite_sides() {
        let bs = vec![
            bx("U1", true, 0.0, 0.0, 5.0, 5.0, 0.0, 1.0),
            bx("C1", true, 4.0, 1.0, 6.0, 2.0, 0.0, 0.5),   // overlaps U1 by 1 mm in x
            bx("C2", true, 5.2, 3.0, 6.0, 4.0, 0.0, 0.5),   // 0.2 mm from U1
            bx("C3", false, 1.0, 1.0, 2.0, 2.0, 0.0, 0.5),  // under U1, other side
            bx("D1", true, 1.0, 1.0, 2.0, 2.0, 1.5, 2.0),   // above U1's top
        ];
        let (col, near) = clashes(&bs, 0.3);
        assert_eq!(col.len(), 1);
        assert_eq!(col[0]["parts"], json!(["C1", "U1"]));
        assert_eq!(col[0]["overlapMm"], 1.0);
        assert_eq!(near.len(), 1);
        assert_eq!(near[0]["parts"], json!(["C2", "U1"]));
        assert_eq!(near[0]["gapMm"], 0.2);
        assert!(clashes(&bs, 0.0).1.is_empty());
    }

    #[test]
    fn height_check_reads_the_companion_and_names_parts_without_bodies() {
        let input = json!({"board": {"components": [{"ref": "U1"}, {"ref": "R7"}]},
                           "bodies": {"bodies": [{"ref": "U1", "side": "top", "x0": 0.0, "y0": 0.0, "x1": 5.0, "y1": 5.0, "z0": 0.0, "z1": 2.0}], "noBody": []},
                           "args": {"maxTopMm": 1.5}});
        let r = height_check(&input);
        assert_eq!(r["overLimit"][0]["ref"], "U1");
        assert_eq!(r["noBody"], json!(["R7"]));
        assert_eq!(r["complete"], json!(false));
        assert_eq!(height_check(&json!({"args": {}}))["errorCode"], "no_bodies");
    }

    #[test]
    fn turned_bodies_meet_by_their_rectangles_not_their_boxes() {
        // a 4 mm QFN at 45 degrees and a cap in its box's empty corner: apart, not a collision (kicad-lane #26)
        let rows = vec![json!({"ref": "U1", "side": "top", "cx": 0.0, "cy": 0.0, "w": 4.0, "h": 4.0, "rotation": 45.0, "z0": 0.0, "z1": 0.9}),
                        json!({"ref": "C1", "side": "top", "x0": 2.0, "y0": 2.0, "x1": 2.6, "y1": 2.3, "z0": 0.0, "z1": 0.5})];
        let bs = boxes(&rows);
        assert!((bs[0].x1 - 2.0 * 2f64.sqrt()).abs() < 1e-9, "the box still covers the turned body");
        let (hit, d) = xy_meet(&bs[0], &bs[1]);
        assert!(!hit && d > 0.5, "{hit} {d}");
        let (col, _) = clashes(&bs, 0.2);
        assert!(col.is_empty());
        // the same cap moved onto the QFN's edge collides
        let near = boxes(&[rows[0].clone(), json!({"ref": "C1", "side": "top", "x0": 1.2, "y0": 1.2, "x1": 1.8, "y1": 1.5, "z0": 0.0, "z1": 0.5})]);
        assert!(xy_meet(&near[0], &near[1]).0);
        // corners work too
        let c = boxes(&[json!({"ref": "U2", "corners": [[0, 0], [2, 0], [2, 1], [0, 1]], "z0": 0, "z1": 1})]);
        assert_eq!((c[0].x1, c[0].y1), (2.0, 1.0));
    }
}


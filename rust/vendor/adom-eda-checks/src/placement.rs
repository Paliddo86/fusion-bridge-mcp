//! A placement review before routing: parts whose courtyards (pads and body, grown by a margin) overlap on the same
//! side, parts off the board or over its edge, pads too close to the edge for assembly and depanelling, and parts on a
//! mounting hole. From the Altium bridge's placement.rs. The score after it (connectors away from the edge, decoupling
//! capacitors far from their IC, the ratsnest's length, and a 0-100 grade) is ported from kicad-bridge's
//! score_placement (#29).
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }
type Rect = (f64, f64, f64, f64);

fn hit(a: Rect, b: Rect) -> bool { a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3 }
fn grow(r: Rect, d: f64) -> Rect { (r.0 - d, r.1 - d, r.2 + d, r.3 + d) }

/// Each part's box (its pads, and its 3D body's box when it has one) and side. Pure.
pub fn part_boxes(st: &Value, bodies: &BTreeMap<String, Rect>) -> BTreeMap<String, (bool, Rect)> {
    let mut m: BTreeMap<String, (bool, Rect)> = BTreeMap::new();
    let side: BTreeMap<String, bool> = st["components"].as_array().cloned().unwrap_or_default().iter()
        .filter_map(|c| Some((c["ref"].as_str()?.to_string(), c["side"] == "bottom"))).collect();
    for p in st["pads"].as_array().cloned().unwrap_or_default() {
        let Some(r) = p["ref"].as_str().filter(|r| !r.is_empty()) else { continue };
        let (c, s) = (f(&p["rotation"]).to_radians().cos().abs(), f(&p["rotation"]).to_radians().sin().abs());
        let (hx, hy) = ((f(&p["sizeX"]) * c + f(&p["sizeY"]) * s) / 2.0, (f(&p["sizeX"]) * s + f(&p["sizeY"]) * c) / 2.0);
        let b = (f(&p["x"]) - hx, f(&p["y"]) - hy, f(&p["x"]) + hx, f(&p["y"]) + hy);
        let e = m.entry(r.to_string()).or_insert((side.get(r).copied().unwrap_or(false), b));
        e.1 = (e.1 .0.min(b.0), e.1 .1.min(b.1), e.1 .2.max(b.2), e.1 .3.max(b.3));
    }
    for (r, b) in bodies {
        if let Some(e) = m.get_mut(r) { e.1 = (e.1 .0.min(b.0), e.1 .1.min(b.1), e.1 .2.max(b.2), e.1 .3.max(b.3)); }
    }
    m
}

/// The review. `ring` is the outline (mm); holes are (x, y, radius) of the board's own drilled, part-less pads. Pure.
pub fn review(boxes: &BTreeMap<String, (bool, Rect)>, pads: &[Value], ring: &[(f64, f64)], holes: &[(f64, f64, f64)], margin: f64, edge: f64) -> Value {
    let (bx0, bx1) = ring.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
    let (by0, by1) = ring.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
    let inside = |x: f64, y: f64| {
        let mut c = false;
        for k in 0..ring.len() {
            let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
            if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 { c = !c; }
        }
        c
    };
    let dist = |x: f64, y: f64| (0..ring.len()).map(|k| {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l2 = dx * dx + dy * dy;
        let t = if l2 > 0.0 { (((x - a.0) * dx + (y - a.1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
        ((a.0 + t * dx - x).powi(2) + (a.1 + t * dy - y).powi(2)).sqrt()
    }).fold(f64::MAX, f64::min);
    let (mut overlaps, mut off, mut near, mut on_holes) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let list: Vec<(&String, &(bool, Rect))> = boxes.iter().collect();
    for i in 0..list.len() {
        for j in i + 1..list.len() {
            let ((ra, (sa, a)), (rb, (sb, b))) = (list[i], list[j]);
            if sa == sb && hit(grow(*a, margin), grow(*b, margin)) {
                let ov = (a.2.min(b.2) - a.0.max(b.0)).min(a.3.min(b.3) - a.1.max(b.1));
                overlaps.push(json!({"parts": [ra, rb], "side": if *sa { "bottom" } else { "top" }, "overlapMm": r3(ov + 2.0 * margin)}));
            }
        }
    }
    // on or off the board by its pads' centres: a box's corners fall outside a round board's edge while the part sits
    // well inside it (the Bluetooth Sentinel's J1 read as off the board)
    let mut pad_in: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for p in pads {
        let Some(r) = p["ref"].as_str().filter(|r| !r.is_empty()) else { continue };
        let e = pad_in.entry(r).or_insert((0, 0));
        e.0 += 1;
        if inside(f(&p["x"]), f(&p["y"])) { e.1 += 1; }
    }
    let _ = (bx0, bx1, by0, by1);
    for (r, (_, b)) in boxes {
        let (n, n_in) = pad_in.get(r.as_str()).copied().unwrap_or((0, 0));
        if n > 0 && n_in == 0 { off.push(json!({"ref": r, "where": "off the board"})); continue; }
        if n_in < n { off.push(json!({"ref": r, "where": "over the board edge"})); continue; }
        for (x, y, hr) in holes { if hit(*b, (x - hr, y - hr, x + hr, y + hr)) { on_holes.push(json!({"ref": r, "hole": [r3(*x), r3(*y)]})); } }
    }
    // pads near the edge: the closest pad of each part
    let mut closest: BTreeMap<String, f64> = BTreeMap::new();
    for p in pads {
        let Some(r) = p["ref"].as_str().filter(|r| !r.is_empty()) else { continue };
        // the pad's four corners, turned with it: inside the outline their distance counts, outside it is negative
        let (c, s) = (f(&p["rotation"]).to_radians().cos(), f(&p["rotation"]).to_radians().sin());
        let (hx, hy) = (f(&p["sizeX"]) / 2.0, f(&p["sizeY"]) / 2.0);
        let d = [(-hx, -hy), (hx, -hy), (hx, hy), (-hx, hy)].iter().map(|(u, v)| {
            let (x, y) = (f(&p["x"]) + u * c - v * s, f(&p["y"]) + u * s + v * c);
            if inside(x, y) { dist(x, y) } else { -dist(x, y) }
        }).fold(f64::MAX, f64::min);
        let e = closest.entry(r.to_string()).or_insert(f64::MAX);
        if d < *e { *e = d; }
    }
    for (r, d) in &closest {
        if *d < edge && !off.iter().any(|o| o["ref"] == json!(r)) { near.push(json!({"ref": r, "padToEdgeMm": r3(*d)})); }
    }
    let ok = overlaps.is_empty() && off.is_empty() && on_holes.is_empty();
    json!({"parts": boxes.len(), "ok": ok, "courtyardOverlaps": overlaps, "offOrOverEdge": off, "onMountingHoles": on_holes, "nearEdge": near, "marginMm": margin, "edgeMm": edge})
}

/// Whether two parts that overlap in plan view are a stack (a part under a raised one: an Adom molecule on pins, a
/// connector's overhang): their pads stay `margin` apart and no body of one meets a body of the other in 3D. A part with
/// no body cannot be shown stacked. Pure.
pub fn is_stacked(pads: &[Value], bs: &[crate::bodies::BodyBox], a: &str, b: &str, margin: f64) -> bool {
    // each pad's own box, turned with it: a molecule's pins span the part under it, so the union of its pads would too
    let pad_rects = |r: &str| -> Vec<Rect> { pads.iter().filter(|p| p["ref"].as_str() == Some(r)).map(|p| {
        let (c, s) = (f(&p["rotation"]).to_radians().cos().abs(), f(&p["rotation"]).to_radians().sin().abs());
        let (hx, hy) = ((f(&p["sizeX"]) * c + f(&p["sizeY"]) * s) / 2.0, (f(&p["sizeX"]) * s + f(&p["sizeY"]) * c) / 2.0);
        (f(&p["x"]) - hx, f(&p["y"]) - hy, f(&p["x"]) + hx, f(&p["y"]) + hy)
    }).collect() };
    let (pa, pb) = (pad_rects(a), pad_rects(b));
    if pa.is_empty() || pb.is_empty() || pa.iter().any(|x| pb.iter().any(|y| hit(grow(*x, margin), grow(*y, margin)))) { return false; }
    // a body-less SMD test point is flat copper: it stands 0 mm tall (fusion-lane, BQ25792's TP1.. under a module)
    let flat = |r: &str| -> Option<crate::bodies::BodyBox> {
        let ps: Vec<&Value> = pads.iter().filter(|p| p["ref"].as_str() == Some(r)).collect();
        if prefix(r) != "TP" || ps.is_empty() || ps.iter().any(|p| f(&p["drill"]) > 0.0) { return None; }
        let b = pad_rects(r).into_iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |a, x| (a.0.min(x.0), a.1.min(x.1), a.2.max(x.2), a.3.max(x.3)));
        Some(crate::bodies::BodyBox { part: r.to_string(), top: true, x0: b.0, y0: b.1, x1: b.2, y1: b.3, z0: 0.0, z1: 0.0, turned: None })
    };
    let (mut ba, mut bb): (Vec<crate::bodies::BodyBox>, Vec<crate::bodies::BodyBox>) = (bs.iter().filter(|x| x.part == a).cloned().collect(), bs.iter().filter(|x| x.part == b).cloned().collect());
    if ba.is_empty() { ba.extend(flat(a)); }
    if bb.is_empty() { bb.extend(flat(b)); }
    if ba.is_empty() || bb.is_empty() { return false; }
    // bodies meet when their footprints come within twice the margin (turned ones by their rectangles) and their heights overlap
    // a flat test point (z 0..0) meets a body that reaches the board (z0 at or below 0)
    !ba.iter().any(|p| bb.iter().any(|q| { let (h, d) = crate::bodies::xy_meet(p, q); (h || d < 2.0 * margin) && (p.z0.max(q.z0) < p.z1.min(q.z1) || (p.z1 == p.z0 && q.z0 <= p.z0) || (q.z1 == q.z0 && p.z0 <= q.z0)) }))
}

fn prefix(r: &str) -> String { r.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_uppercase() }

/// A connector by designator or footprint name. X is not enough on its own: Altium designs name crystals X1 (the
/// Sentinel's 16 MHz and 32.768 kHz read as connectors far from the edge).
fn is_connector(r: &str, pattern: &str) -> bool {
    let lib = pattern.to_ascii_lowercase();
    matches!(prefix(r).as_str(), "J" | "P" | "CN" | "USB" | "CON") || ["connector", "usb", "pinheader", "pin_header", "terminalblock", "jst", "molex", "barrel"].iter().any(|k| lib.contains(k))
}

/// The score: deductions per rule (per item, capped), the ratsnest's share of the board diagonal, 0-100 and a grade.
/// `overlaps` and `off` are counts from the review. Pure.
pub fn score(st: &Value, boxes: &BTreeMap<String, (bool, Rect)>, ring: &[(f64, f64)], overlaps: usize, off: usize, connector_edge: f64, decoupling: f64) -> Value {
    let pads = st["pads"].as_array().cloned().unwrap_or_default();
    let (bx0, bx1) = ring.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
    let (by0, by1) = ring.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
    let pattern: BTreeMap<String, String> = st["components"].as_array().cloned().unwrap_or_default().iter()
        .filter_map(|c| Some((c["ref"].as_str()?.to_string(), c["pattern"].as_str().unwrap_or("").to_string()))).collect();
    // connectors farther than connectorEdgeMm from the board's edge (its box)
    let mut far_conn = Vec::new();
    for (r, (_, b)) in boxes {
        if !is_connector(r, pattern.get(r).map(|s| s.as_str()).unwrap_or("")) { continue; }
        let d = (b.0 - bx0).min(bx1 - b.2).min(b.1 - by0).min(by1 - b.3);
        if d < 0.0 { continue; } // off or over the edge: the review has it
        if d > connector_edge { far_conn.push(json!({"ref": r, "edgeDistanceMm": r3(d)})); }
    }
    // decoupling capacitors (two pads: one on a ground, one on a supply rail an IC also has) farther than decouplingMm
    // from that IC's nearest pin on the same net. The net must look like a rail: a crystal's load capacitor (the
    // Sentinel's C1 on 16M_IN) is a signal to ground, not decoupling.
    let net = |p: &Value| p["net"].as_str().unwrap_or("").to_string();
    let mut by_ref: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for p in &pads { if let Some(r) = p["ref"].as_str().filter(|r| !r.is_empty()) { by_ref.entry(r.to_string()).or_default().push(p); } }
    let is_ic = |r: &str| matches!(prefix(r).as_str(), "U" | "IC") && by_ref.get(r).map(|v| v.len() >= 3).unwrap_or(false);
    let mut far_caps = Vec::new();
    for (r, ps) in &by_ref {
        if prefix(r) != "C" || ps.len() != 2 { continue; }
        let g = ps.iter().filter(|p| crate::schreview::is_ground(&net(p))).count();
        let Some(pw) = ps.iter().find(|p| !net(p).is_empty() && !crate::schreview::is_ground(&net(p))) else { continue };
        if g != 1 || !crate::schreview::looks_like_rail(&net(pw)) { continue; }
        let n = net(pw);
        let best = pads.iter().filter(|q| net(q) == n && q["ref"].as_str().map(|x| x != r && is_ic(x)).unwrap_or(false))
            .map(|q| (((f(&q["x"]) - f(&pw["x"])).powi(2) + (f(&q["y"]) - f(&pw["y"])).powi(2)).sqrt(), q)).min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((d, q)) = best { if d > decoupling { far_caps.push(json!({"ref": r, "net": n, "nearestPin": format!("{}.{}", q["ref"].as_str().unwrap_or(""), q["name"].as_str().unwrap_or("")), "distanceMm": r3(d)})); } }
    }
    // the ratsnest: each net's minimum spanning tree over its pads' centres
    let mut nets: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    for p in &pads { let n = net(p); if !n.is_empty() { nets.entry(n).or_default().push((f(&p["x"]), f(&p["y"]))); } }
    let (mut mst, mut edges) = (0.0, 0usize);
    for pts in nets.values().filter(|v| v.len() > 1) {
        let mut best = vec![f64::MAX; pts.len()];
        let mut done = vec![false; pts.len()];
        best[0] = 0.0;
        for _ in 0..pts.len() {
            let Some(i) = (0..pts.len()).filter(|&i| !done[i]).min_by(|&a, &b| best[a].total_cmp(&best[b])) else { break };
            done[i] = true;
            mst += best[i];
            for j in 0..pts.len() { if !done[j] { let d = ((pts[i].0 - pts[j].0).powi(2) + (pts[i].1 - pts[j].1).powi(2)).sqrt(); if d < best[j] { best[j] = d; } } }
        }
        edges += pts.len() - 1;
    }
    let diag = ((bx1 - bx0).powi(2) + (by1 - by0).powi(2)).sqrt();
    let frac = if edges > 0 && diag > 0.0 { mst / edges as f64 / diag } else { 0.0 };
    let mut deductions = Vec::new();
    let mut total = 0.0;
    let mut push = |rule: &str, per: f64, cap: f64, items: Vec<Value>, detail: &str| {
        let pts = (per * items.len() as f64).min(cap);
        total += pts;
        deductions.push(json!({"rule": rule, "count": items.len(), "points": pts, "perItem": per, "cap": cap, "items": items, "detail": detail}));
    };
    push("courtyard_overlap", 10.0, 40.0, (0..overlaps).map(|_| json!(null)).collect(), "same-side parts that overlap (courtyardOverlaps)");
    push("outside_board", 10.0, 30.0, (0..off).map(|_| json!(null)).collect(), "parts off or over the board edge (offOrOverEdge)");
    push("connector_off_edge", 5.0, 15.0, far_conn.clone(), "connectors farther than connectorEdgeMm from the board edge");
    push("decoupling_far", 3.0, 15.0, far_caps.clone(), "decoupling capacitors farther than decouplingMm from their IC's nearest pin on the same supply");
    let rpts = (((frac - 0.1) / 0.3).clamp(0.0, 1.0) * 200.0).round() / 10.0;
    total += rpts;
    let mean = if edges > 0 { mst / edges as f64 } else { 0.0 };
    deductions.push(json!({"rule": "ratsnest_length", "count": edges, "points": rpts, "cap": 20.0, "items": [],
        "detail": format!("mean ratsnest edge {:.2} mm is {:.1} % of the {:.1} mm board diagonal (0 points at 10 %, 20 at 40 %)", mean, frac * 100.0, diag)}));
    // the overlap and outside items are listed in the review itself
    for d in deductions.iter_mut() { if d["items"].as_array().map(|a| a.iter().all(|x| x.is_null())).unwrap_or(false) { d["items"] = json!([]); } }
    deductions.sort_by(|a, b| b["points"].as_f64().unwrap_or(0.0).total_cmp(&a["points"].as_f64().unwrap_or(0.0)));
    let score = (100.0 - total).max(0.0).round();
    let grade = match score as i64 { 90..=100 => "A", 80..=89 => "B", 70..=79 => "C", 50..=69 => "D", _ => "F" };
    json!({"score": score, "grade": grade, "deductions": deductions, "connectorsOffEdge": far_conn, "decouplingFar": far_caps,
        "ratsnest": {"totalMm": r3(mst), "edges": edges, "meanMm": r3(mean), "boardDiagonalMm": r3(diag), "share": r3(frac)}})
}

/// placement_check: input {board, bodies?, args: {marginMm?, edgeMm?, connectorEdgeMm? (3), decouplingMm? (3)}}. Without bodies the parts' boxes are their pads
/// only, and the reply says so (`bodiesUsed: false`).
pub fn placement_check(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let ring = crate::coppermap::outline_ring(&st);
    if ring.len() < 3 { return crate::input::refuse("no_outline", "the board has no outline"); }
    // the parts' 3D bodies (silk_state's 'body' box takes in the designator text and made every neighbour overlap)
    let mut bodies: BTreeMap<String, Rect> = BTreeMap::new();
    let given = crate::bodies::input_boxes(input);
    for b in given.clone().unwrap_or_default() {
        let e = bodies.entry(b.part.clone()).or_insert((b.x0, b.y0, b.x1, b.y1));
        *e = (e.0.min(b.x0), e.1.min(b.y0), e.2.max(b.x1), e.3.max(b.y1));
    }
    let boxes = part_boxes(&st, &bodies);
    let pads = st["pads"].as_array().cloned().unwrap_or_default();
    let holes: Vec<(f64, f64, f64)> = pads.iter().filter(|p| f(&p["drill"]) > 0.0 && p["ref"].as_str().map(|r| r.is_empty()).unwrap_or(true))
        .map(|p| (f(&p["x"]), f(&p["y"]), f(&p["sizeX"]).max(f(&p["drill"])) / 2.0)).collect();
    let num = |k: &str, d: f64| args.get(k).and_then(|v| v.as_f64()).unwrap_or(d);
    let margin = num("marginMm", 0.1);
    let mut out = review(&boxes, &pads, &ring, &holes, margin, num("edgeMm", 0.5));
    // a part under a raised one (an Adom molecule on pins, a connector's overhang) overlaps it in plan view only: when
    // their pads stay clear and no body of one meets a body of the other in 3D, it is a stack, listed apart, not an
    // overlap (fusion-lane #19). A part with no body cannot be shown stacked, so its overlaps stand.
    if let Some(bs) = &given {
        let (mut real, mut stacked) = (Vec::new(), Vec::new());
        for o in out["courtyardOverlaps"].as_array().cloned().unwrap_or_default() {
            let (a, b) = (o["parts"][0].as_str().unwrap_or(""), o["parts"][1].as_str().unwrap_or(""));
            if is_stacked(&pads, bs, a, b, margin) { stacked.push(o) } else { real.push(o) }
        }
        if !stacked.is_empty() {
            out["ok"] = json!(real.is_empty() && out["offOrOverEdge"].as_array().map(|a| a.is_empty()).unwrap_or(true) && out["onMountingHoles"].as_array().map(|a| a.is_empty()).unwrap_or(true));
        }
        out["courtyardOverlaps"] = json!(real);
        out["stacked"] = json!(stacked);
    }
    let sc = score(&st, &boxes, &ring, out["courtyardOverlaps"].as_array().map(|a| a.len()).unwrap_or(0), out["offOrOverEdge"].as_array().map(|a| a.len()).unwrap_or(0), num("connectorEdgeMm", 3.0), num("decouplingMm", 3.0));
    for k in ["score", "grade", "deductions", "connectorsOffEdge", "decouplingFar", "ratsnest"] { out[k] = sc[k].clone(); }
    out["success"] = json!(true);
    out["bodiesUsed"] = json!(given.is_some());
    out["_hint"] = json!("courtyardOverlaps: parts closer than twice marginMm (their pads and bodies): move them apart (move_parts, find_space). offOrOverEdge parts must come onto the board; nearEdge pads risk the router or the V-cut (JLCPCB: 0.4 mm from a V-cut, 0.2 from a routed edge). onMountingHoles: a screw head lands on the part. Without bodies (bodiesUsed false) a part's box is its pads only: pass the bodies companion to count overhanging bodies. score (0-100, grade A-F) weighs these with connectorsOffEdge (a connector deep inside the board), decouplingFar (a capacitor far from its IC's supply pin: place_decoupling) and the ratsnest's length; ok counts only overlaps, off-board parts and parts on holes.");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_review_finds_each_problem() {
        let ring = vec![(0.0, 0.0), (40.0, 0.0), (40.0, 20.0), (0.0, 20.0)];
        let mut boxes = BTreeMap::new();
        boxes.insert("U1".to_string(), (false, (5.0, 5.0, 10.0, 10.0)));
        boxes.insert("C1".to_string(), (false, (10.2, 5.0, 11.0, 6.0)));   // 0.2 mm from U1: inside 2 x 0.25
        boxes.insert("C2".to_string(), (true, (10.2, 5.0, 11.0, 6.0)));    // the same spot on the bottom: fine
        boxes.insert("J1".to_string(), (false, (38.0, 8.0, 42.0, 12.0)));  // over the edge
        boxes.insert("R9".to_string(), (false, (50.0, 5.0, 51.0, 6.0)));   // off the board
        boxes.insert("R2".to_string(), (false, (1.5, 15.5, 2.5, 16.5)));   // on the hole at (2, 16)
        let pad = |r: &str, x: f64, y: f64| json!({"ref": r, "x": x, "y": y, "sizeX": 0.6, "sizeY": 0.6});
        let pads = vec![pad("C1", 10.6, 5.5), pad("U1", 5.2, 0.6), pad("J1", 39.0, 10.0), pad("J1", 41.0, 10.0), pad("R9", 50.5, 5.5)];
        let r = review(&boxes, &pads, &ring, &[(2.0, 16.0, 1.6)], 0.25, 0.5);
        assert_eq!(r["courtyardOverlaps"].as_array().unwrap().len(), 1);
        assert_eq!(r["courtyardOverlaps"][0]["parts"], json!(["C1", "U1"]));
        let off: Vec<String> = r["offOrOverEdge"].as_array().unwrap().iter().map(|o| format!("{} {}", o["ref"].as_str().unwrap(), o["where"].as_str().unwrap())).collect();
        assert!(off.contains(&"J1 over the board edge".to_string()) && off.contains(&"R9 off the board".to_string()));
        assert_eq!(r["onMountingHoles"][0]["ref"], "R2");
        assert_eq!(r["nearEdge"][0]["ref"], "U1");
        assert_eq!(r["ok"], json!(false));
    }

    #[test]
    fn the_score_finds_a_deep_connector_and_a_far_capacitor() {
        let pad = |r: &str, n: &str, x: f64, y: f64, net: &str| json!({"ref": r, "name": n, "x": x, "y": y, "sizeX": 1.0, "sizeY": 1.0, "rotation": 0.0, "net": net});
        // a 50 x 30 board: J1 in the middle, C1 10 mm from U1's VDD pin, C2 beside it
        let st = json!({"outline": [[0, 0], [50, 0], [50, 30], [0, 30]],
            "outlineEdges": [{"kind": "line", "start": [0.0, 0.0]}, {"kind": "line", "start": [50.0, 0.0]}, {"kind": "line", "start": [50.0, 30.0]}, {"kind": "line", "start": [0.0, 30.0]}],
            "components": [{"ref": "J1", "side": "top", "pattern": "PinHeader_1x02"}, {"ref": "U1", "side": "top"}, {"ref": "C1", "side": "top"}, {"ref": "C2", "side": "top"}],
            "pads": [pad("J1", "1", 25.0, 15.0, "SIG"), pad("J1", "2", 27.0, 15.0, "GND"),
                     pad("U1", "VDD", 10.0, 10.0, "+3V3"), pad("U1", "GND", 10.0, 12.0, "GND"), pad("U1", "IO", 12.0, 10.0, "SIG"),
                     pad("C1", "1", 20.0, 10.0, "+3V3"), pad("C1", "2", 21.0, 10.0, "GND"),
                     pad("C2", "1", 10.0, 8.0, "+3V3"), pad("C2", "2", 11.0, 8.0, "GND")]});
        let r = placement_check(&json!({"board": st, "args": {}}));
        assert_eq!(r["connectorsOffEdge"][0]["ref"], "J1", "{r}");
        let far: Vec<&str> = r["decouplingFar"].as_array().unwrap().iter().map(|c| c["ref"].as_str().unwrap()).collect();
        assert_eq!(far, vec!["C1"]);
        assert_eq!(r["decouplingFar"][0]["nearestPin"], "U1.VDD");
        assert!(r["score"].as_f64().unwrap() < 100.0 && r["grade"].is_string());
        assert_eq!(r["ok"], json!(true), "a score is advice: ok counts only overlaps, off-board parts and holes");
        assert!(r["ratsnest"]["edges"].as_u64().unwrap() > 0);
    }

    #[test]
    fn a_part_under_a_raised_one_is_stacked_not_overlapping() {
        let pad = |r: &str, x: f64, y: f64| json!({"ref": r, "x": x, "y": y, "sizeX": 0.6, "sizeY": 0.6, "rotation": 0.0});
        // M1: a molecule on two pins at x 0 and 10, its board raised 3 to 4 mm; C1 sits under it, between the pins
        let st = json!({"outline": [[-5, -5], [15, -5], [15, 5], [-5, 5]],
            "outlineEdges": [{"kind": "line", "start": [-5.0, -5.0]}, {"kind": "line", "start": [15.0, -5.0]}, {"kind": "line", "start": [15.0, 5.0]}, {"kind": "line", "start": [-5.0, 5.0]}],
            "components": [{"ref": "M1", "side": "top"}, {"ref": "C1", "side": "top"}],
            "pads": [pad("M1", 0.0, 0.0), pad("M1", 10.0, 0.0), pad("C1", 5.0, 0.0), pad("C1", 6.0, 0.0)]});
        let body = |r: &str, x0: f64, x1: f64, z0: f64, z1: f64| json!({"ref": r, "side": "top", "x0": x0, "y0": -2.0, "x1": x1, "y1": 2.0, "z0": z0, "z1": z1});
        let stacked = placement_check(&json!({"board": st, "bodies": {"bodies": [body("M1", -1.0, 11.0, 3.0, 4.0), body("C1", 4.5, 6.5, 0.0, 1.0)]}, "args": {}}));
        assert_eq!(stacked["courtyardOverlaps"].as_array().unwrap().len(), 0, "{stacked}");
        assert_eq!(stacked["stacked"][0]["parts"], json!(["C1", "M1"]));
        assert_eq!(stacked["ok"], json!(true));
        // the same plan, but C1 tall enough to reach the raised board: a real overlap
        let real = placement_check(&json!({"board": st, "bodies": {"bodies": [body("M1", -1.0, 11.0, 3.0, 4.0), body("C1", 4.5, 6.5, 0.0, 3.5)]}, "args": {}}));
        assert_eq!(real["courtyardOverlaps"].as_array().unwrap().len(), 1);
        assert_eq!(real["ok"], json!(false));
        // a body-less SMD test point under the raised board is flat: stacked; under a body that sits on the board: not
        let st2 = json!({"outline": st["outline"], "outlineEdges": st["outlineEdges"], "components": [{"ref": "M1", "side": "top"}, {"ref": "TP1", "side": "top"}],
            "pads": [pad("M1", 0.0, 0.0), pad("M1", 10.0, 0.0), pad("TP1", 5.0, 0.0)]});
        let tp = placement_check(&json!({"board": st2, "bodies": {"bodies": [body("M1", -1.0, 11.0, 3.0, 4.0)]}, "args": {}}));
        assert_eq!(tp["stacked"][0]["parts"], json!(["M1", "TP1"]), "{tp}");
        let tp_low = placement_check(&json!({"board": st2, "bodies": {"bodies": [body("M1", -1.0, 11.0, -0.05, 1.7)]}, "args": {}}));
        assert_eq!(tp_low["courtyardOverlaps"].as_array().unwrap().len(), 1, "{tp_low}");
        // no bodies: pads only, no stack possible to show, nothing hidden
        let none = placement_check(&json!({"board": st, "args": {}}));
        assert_eq!(none["bodiesUsed"], json!(false));
        assert!(none.get("stacked").is_none());
    }
}


//! Room on the board: free spots for a part near another (clear of every other part on that side, of that side's
//! copper, and inside the outline), and two-pad parts a quarter turn would shorten the airlines of. From the Altium
//! bridge's design.rs; a part is the box of its pads and, with the bodies companion, of its 3D body.
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }
type Rect = (f64, f64, f64, f64);
fn grow(r: Rect, g: f64) -> Rect { (r.0 - g, r.1 - g, r.2 + g, r.3 + g) }
fn hits(a: Rect, b: Rect) -> bool { a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3 }
fn dist(a: (f64, f64), b: (f64, f64)) -> f64 { ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt() }

/// The parts' boxes and sides, with their bodies when the companion came.
fn part_boxes(st: &Value, input: &Value) -> BTreeMap<String, (bool, Rect)> {
    let mut bodies: BTreeMap<String, Rect> = BTreeMap::new();
    for b in crate::bodies::input_boxes(input).unwrap_or_default() {
        let e = bodies.entry(b.part.clone()).or_insert((b.x0, b.y0, b.x1, b.y1));
        *e = (e.0.min(b.x0), e.1.min(b.y0), e.2.max(b.x1), e.3.max(b.y1));
    }
    crate::placement::part_boxes(st, &bodies)
}

/// find_space: input {board, bodies?, args: {near? | at?: {x, y}, part? | sizeMm?: [w, h], side?, clearanceMm?, edgeMm?,
/// count?, radiusMm?, copper?, check?}}. Free spots nearest first, as where the part's origin goes; with check: true,
/// the parts that already overlap or leave the outline.
pub fn find_space(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let s = |k: &str| args[k].as_str().map(String::from);
    let ring = crate::coppermap::outline_ring(&st);
    let boxes = part_boxes(&st, input);
    let parts: Vec<(String, Rect, bool)> = boxes.iter().map(|(r, (b, x))| (r.clone(), *x, *b)).collect();
    let clr = args["clearanceMm"].as_f64().unwrap_or(0.25);
    let edge = args["edgeMm"].as_f64().unwrap_or(0.3);
    let seg = |p: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l2 = dx * dx + dy * dy;
        let t = if l2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) };
        dist(p, (a.0 + t * dx, a.1 + t * dy))
    };
    let in_outline = |r: Rect| -> bool {
        if ring.len() < 3 { return true; }
        [(r.0, r.1), (r.2, r.1), (r.2, r.3), (r.0, r.3)].iter().all(|&c| crate::route::inside(c, &ring) && (0..ring.len()).all(|i| seg(c, ring[i], ring[(i + 1) % ring.len()]) >= edge))
    };
    if args["check"].as_bool() == Some(true) {
        // the same tests placement_check makes (fusion-lane): a part under a raised one (an Adom molecule on pins) whose
        // pads stay apart and whose bodies do not meet in 3D is stacked, not overlapping; a part is off the outline by
        // its pads' centres, not its box (a round board's edge cuts a box corner off a part well inside it)
        let pads = st["pads"].as_array().cloned().unwrap_or_default();
        let bodies = crate::bodies::input_boxes(input);
        let (mut overlaps, mut stacked) = (Vec::new(), Vec::new());
        for i in 0..parts.len() { for j in i + 1..parts.len() {
            if parts[i].2 != parts[j].2 || !hits(parts[i].1, parts[j].1) { continue; }
            let pair = json!([parts[i].0, parts[j].0]);
            match &bodies { Some(bs) if crate::placement::is_stacked(&pads, bs, &parts[i].0, &parts[j].0, 0.0) => stacked.push(pair), _ => overlaps.push(pair) }
        } }
        let outside: Vec<&String> = parts.iter().filter(|p| {
            let own: Vec<&Value> = pads.iter().filter(|q| q["ref"].as_str() == Some(p.0.as_str())).collect();
            ring.len() >= 3 && !own.is_empty() && own.iter().any(|q| !crate::route::inside((f(&q["x"]), f(&q["y"])), &ring))
        }).map(|p| &p.0).collect();
        return json!({"success": true, "overlaps": overlaps, "stacked": stacked, "outsideOutline": outside, "parts": parts.len(), "bodiesUsed": bodies.is_some(),
            "_hint": "Overlaps compare part boxes (pads, and 3D bodies when given) on the same side; placement_check adds margins, stacked parts and mounting holes. find_space {near, part} finds a free spot; move_parts moves it."});
    }
    let comp = |d: &str| st["components"].as_array().and_then(|a| a.iter().find(|c| c["ref"].as_str().map(|r| r.eq_ignore_ascii_case(d)).unwrap_or(false)).cloned());
    // what to place: an existing part (its extent and origin) or a size
    let me = s("part").and_then(|p| parts.iter().find(|x| x.0.eq_ignore_ascii_case(&p)).cloned());
    let (w, h, off, my_bottom) = match (&me, args["sizeMm"].as_array()) {
        (Some((d, r, b)), _) => {
            let c = comp(d).unwrap_or(Value::Null);
            let (ox, oy) = (c["x"].as_f64().unwrap_or((r.0 + r.2) / 2.0), c["y"].as_f64().unwrap_or((r.1 + r.3) / 2.0));
            (r.2 - r.0, r.3 - r.1, (ox - (r.0 + r.2) / 2.0, oy - (r.1 + r.3) / 2.0), *b)
        }
        (None, Some(sz)) if sz.len() == 2 => (f(&sz[0]).max(0.01), f(&sz[1]).max(0.01), (0.0, 0.0), false),
        _ => return crate::input::refuse("bad_args", "part (a designator on the board) or sizeMm [w, h] is required"),
    };
    let bottom = match s("side").as_deref() { Some("bottom") => true, Some("top") => false, _ => my_bottom };
    let (cx, cy) = if let Some(n) = s("near") {
        match parts.iter().find(|x| x.0.eq_ignore_ascii_case(&n)) { Some((_, r, _)) => ((r.0 + r.2) / 2.0, (r.1 + r.3) / 2.0), None => return crate::input::refuse("no_such_part", &format!("{n} is not on the board")) }
    } else if args["at"].is_object() {
        (f(&args["at"]["x"]), f(&args["at"]["y"]))
    } else { return crate::input::refuse("bad_args", "near (a designator) or at {x, y} is required") };
    let mine = me.as_ref().map(|m| m.0.clone()).unwrap_or_default();
    let mut blocks: Vec<Rect> = parts.iter().filter(|p| p.2 == bottom && !p.0.eq_ignore_ascii_case(&mine)).map(|p| grow(p.1, clr)).collect();
    if args["copper"].as_bool() != Some(false) {
        let layer = if bottom { "B.Cu" } else { "F.Cu" };
        // net-less tracks are a hatched pour's pieces, which re-pour round a new part (6473 of them on the Bluetooth
        // Sentinel left no spot anywhere)
        for t in st["tracks"].as_array().into_iter().flatten().filter(|t| t["layer"] == layer && t["net"].as_str().map(|n| !n.is_empty()).unwrap_or(false)) {
            let (a, b, hw) = ((f(&t["start"]["x"]), f(&t["start"]["y"])), (f(&t["end"]["x"]), f(&t["end"]["y"])), f(&t["width"]) / 2.0);
            blocks.push(grow((a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)), hw + clr));
        }
        for v in st["vias"].as_array().into_iter().flatten() {
            let (x, y, r) = (f(&v["position"]["x"]), f(&v["position"]["y"]), f(&v["size"]) / 2.0);
            blocks.push(grow((x - r, y - r, x + r, y + r), clr));
        }
    }
    let count = args["count"].as_u64().unwrap_or(5) as usize;
    let radius = args["radiusMm"].as_f64().unwrap_or(10.0);
    let step = 0.25;
    let n = (radius / step).ceil() as i64;
    let mut cands: Vec<(f64, f64, f64)> = Vec::new();
    for i in -n..=n { for j in -n..=n {
        let (x, y) = (cx + i as f64 * step, cy + j as f64 * step);
        let d = dist((x, y), (cx, cy));
        if d <= radius { cands.push((d, x, y)); }
    } }
    cands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let (mut spots, mut taken): (Vec<Value>, Vec<Rect>) = (Vec::new(), Vec::new());
    for (d, x, y) in cands {
        let r = (x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0);
        if blocks.iter().any(|b| hits(r, *b)) || taken.iter().any(|t| hits(r, *t)) || !in_outline(r) { continue; }
        spots.push(json!({"x": r3(x + off.0), "y": r3(y + off.1), "distanceMm": r3(d), "extent": [r3(r.0), r3(r.1), r3(r.2), r3(r.3)]}));
        taken.push(grow(r, clr));
        if spots.len() >= count { break; }
    }
    if spots.is_empty() { return json!({"success": false, "errorCode": "no_space", "error": format!("no free {} x {} mm spot within {radius} mm", r3(w), r3(h)), "spots": []}); }
    json!({"success": true, "spots": spots, "part": if mine.is_empty() { Value::Null } else { json!(mine) }, "sizeMm": [r3(w), r3(h)], "side": if bottom { "bottom" } else { "top" },
        "_hint": "spots[].x/y are where the part's origin goes: move_parts {parts:[{ref, x, y, rotation}]}, then repour (pours are not avoided: they clear round the new pads). Each spot keeps clearanceMm from other part boxes on that side, from that side's tracks and vias, and edgeMm from the outline; nearest first. find_space {check: true} lists parts that already overlap."})
}

/// Minimum spanning tree length of points (Prim's, O(n^2): nets are small).
fn mst(pts: &[(f64, f64)]) -> f64 {
    if pts.len() < 2 { return 0.0; }
    let (mut inside, mut best) = (vec![false; pts.len()], vec![f64::MAX; pts.len()]);
    best[0] = 0.0;
    let mut total = 0.0;
    for _ in 0..pts.len() {
        let mut u = usize::MAX;
        for i in 0..pts.len() { if !inside[i] && (u == usize::MAX || best[i] < best[u]) { u = i; } }
        inside[u] = true;
        total += best[u];
        for v in 0..pts.len() { if !inside[v] { let d = dist(pts[u], pts[v]); if d < best[v] { best[v] = d; } } }
    }
    total
}

/// check_orientation: input {board, args: {parts?, minImprovementMm?, planeNetPads?}}. Advisory: each two-pad part's
/// quarter turns scored in place, a routable net by its airline (MST) length with the pad there, a plane-like net
/// (more than planeNetPads pads, GND) by the distance to its nearest same-net pad. Turns that save minImprovementMm or
/// more are reported; nothing moves.
pub fn check_orientation(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let min_gain = args["minImprovementMm"].as_f64().unwrap_or(0.6);
    let plane_pads = args["planeNetPads"].as_u64().unwrap_or(12) as usize;
    let want: Vec<String> = args["parts"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_ascii_uppercase())).collect()).unwrap_or_default();
    let pads = st["pads"].as_array().cloned().unwrap_or_default();
    let mut net_pads: HashMap<String, Vec<(String, (f64, f64))>> = HashMap::new();
    for p in &pads {
        let n = p["net"].as_str().unwrap_or("");
        if !n.is_empty() { net_pads.entry(n.to_string()).or_default().push((p["ref"].as_str().unwrap_or("").to_string(), (f(&p["x"]), f(&p["y"])))); }
    }
    let (mut findings, mut checked) = (Vec::new(), 0);
    for c in st["components"].as_array().into_iter().flatten() {
        let r = c["ref"].as_str().unwrap_or("").to_string();
        if !want.is_empty() && !want.contains(&r.to_ascii_uppercase()) { continue; }
        let mine: Vec<&Value> = pads.iter().filter(|p| p["ref"].as_str() == Some(&r)).collect();
        if mine.len() != 2 { continue; }
        checked += 1;
        let (cx, cy, rot) = (f(&c["x"]), f(&c["y"]), f(&c["rotation"]));
        // the airlines with the part turned by delta degrees (counterclockwise, y up)
        let score = |delta: f64| -> f64 {
            let a = delta.to_radians();
            let mut total = 0.0;
            for p in &mine {
                let net = p["net"].as_str().unwrap_or("");
                if net.is_empty() { continue; }
                let (px, py) = (f(&p["x"]) - cx, f(&p["y"]) - cy);
                let at = (cx + px * a.cos() - py * a.sin(), cy + px * a.sin() + py * a.cos());
                let others: Vec<(f64, f64)> = net_pads.get(net).map(|v| v.iter().filter(|(o, _)| o != &r).map(|(_, q)| *q).collect()).unwrap_or_default();
                if others.is_empty() { continue; }
                total += if others.len() + 1 > plane_pads { others.iter().map(|q| dist(at, *q)).fold(f64::MAX, f64::min) } else { let mut pts = others.clone(); pts.push(at); mst(&pts) };
            }
            total
        };
        let now = score(0.0);
        let mut best = (0.0, now);
        for d in [90.0, 180.0, 270.0] { let sc = score(d); if sc < best.1 - 1e-9 { best = (d, sc); } }
        if now - best.1 >= min_gain {
            findings.push(json!({"ref": r, "rotation": rot, "suggested": (rot + best.0).rem_euclid(360.0), "airlineNowMm": r3(now), "airlineThenMm": r3(best.1), "savesMm": r3(now - best.1)}));
        }
    }
    findings.sort_by(|a, b| f(&b["savesMm"]).partial_cmp(&f(&a["savesMm"])).unwrap_or(std::cmp::Ordering::Equal));
    json!({"success": true, "checked": checked, "suggestions": findings,
        "_hint": "Advisory only: each two-pad part's quarter turns scored by airline length (MST of a routable net with the pad there; distance to the nearest pad of a plane-like net such as GND). Turn with move_parts {parts:[{ref, rotation}]} (position kept), then reroute those nets. Routed copper is not considered: on a routed board, treat these as hints."})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(r: &str, n: &str, x: f64, y: f64, net: &str) -> Value {
        json!({"ref": r, "name": n, "x": x, "y": y, "sizeX": 0.6, "sizeY": 0.6, "rotation": 0, "layer": "F.Cu", "drill": 0, "net": net, "shape": "rect"})
    }

    fn board() -> Value {
        // R1 lies along x, but its nets go up and down: a quarter turn shortens both airlines
        json!({"outline": [[0, 0], [20, 0], [20, 20], [0, 20]],
            "components": [{"ref": "R1", "x": 10.0, "y": 10.0, "rotation": 0.0, "side": "top"}, {"ref": "U1", "x": 10.0, "y": 15.0, "rotation": 0.0, "side": "top"}, {"ref": "U2", "x": 10.0, "y": 5.0, "rotation": 0.0, "side": "top"}],
            "pads": [pad("R1", "1", 9.0, 10.0, "A"), pad("R1", "2", 11.0, 10.0, "B"), pad("U1", "1", 10.0, 15.0, "A"), pad("U1", "2", 11.0, 15.0, "X"), pad("U2", "1", 10.0, 5.0, "B"), pad("U2", "2", 11.0, 5.0, "Y")]})
    }

    #[test]
    fn a_quarter_turn_that_shortens_the_airlines_is_suggested() {
        let r = check_orientation(&json!({"board": board(), "args": {}}));
        // U1 and U2 have two pads too, but turning them about the pad at their origin gains nothing
        assert_eq!((r["checked"].clone(), r["suggestions"].as_array().unwrap().len()), (json!(3), 1), "{r}");
        let s = &r["suggestions"][0];
        // pad 1 (net A) has to come up to U1: a quarter turn clockwise, 270 counterclockwise
        assert_eq!((s["ref"].as_str(), s["suggested"].as_f64()), (Some("R1"), Some(270.0)), "{r}");
        assert!(f(&s["savesMm"]) > 0.6);
    }

    #[test]
    fn free_spots_avoid_parts_copper_and_the_edge() {
        let mut b = board();
        b["tracks"] = json!([{"layer": "F.Cu", "net": "A", "width": 0.25, "start": {"x": 12.0, "y": 0.0}, "end": {"x": 12.0, "y": 20.0}}]);
        let r = find_space(&json!({"board": b, "args": {"near": "R1", "sizeMm": [2.0, 1.0], "count": 3}}));
        assert_eq!(r["success"], json!(true), "{r}");
        for s in r["spots"].as_array().unwrap() {
            let e: Vec<f64> = s["extent"].as_array().unwrap().iter().map(f).collect();
            // clear of R1's box (8.7..11.3 x 9.7..10.3) by 0.25, of the track at x 12 by 0.125 + 0.25
            assert!(e[2] <= 8.7 - 0.25 + 1e-9 || e[0] >= 11.3 + 0.25 - 1e-9 || e[3] <= 9.7 - 0.25 + 1e-9 || e[1] >= 10.3 + 0.25 - 1e-9, "{s}");
            assert!(e[2] <= 12.0 - 0.375 + 1e-9 || e[0] >= 12.0 + 0.375 - 1e-9, "{s}");
        }
        let chk = find_space(&json!({"board": board(), "args": {"check": true}}));
        assert_eq!((chk["overlaps"].as_array().unwrap().len(), chk["outsideOutline"].as_array().unwrap().len()), (0, 0));
        // a part under a raised molecule: pads apart, bodies apart in height, so stacked, not an overlap (fusion-lane)
        let mut m = board();
        m["components"].as_array_mut().unwrap().push(json!({"ref": "M1", "x": 10.0, "y": 10.0, "rotation": 0.0, "side": "top"}));
        m["pads"].as_array_mut().unwrap().extend([pad("M1", "1", 6.0, 10.0, "P"), pad("M1", "2", 14.0, 10.0, "Q")]);
        let bodies = json!({"bodies": [{"ref": "M1", "side": "top", "x0": 5.5, "y0": 8.0, "x1": 14.5, "y1": 12.0, "z0": 3.0, "z1": 4.0},
                                       {"ref": "R1", "side": "top", "x0": 8.7, "y0": 9.7, "x1": 11.3, "y1": 10.3, "z0": 0.0, "z1": 0.5}]});
        let s = find_space(&json!({"board": m, "bodies": bodies, "args": {"check": true}}));
        assert_eq!((s["overlaps"].as_array().unwrap().len(), s["stacked"].clone(), s["bodiesUsed"].clone()), (0, json!([["M1", "R1"]]), json!(true)), "{s}");
        assert_eq!(find_space(&json!({"board": board(), "args": {"near": "Q9", "sizeMm": [1, 1]}}))["errorCode"], "no_such_part");
    }
}

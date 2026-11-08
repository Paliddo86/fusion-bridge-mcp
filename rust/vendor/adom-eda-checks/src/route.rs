//! A route between two pads of a net, planned on the board model and answered as route_net's arguments: a bounded
//! orthogonal A* over a grid on any copper layers the caller lists, with through vias between them. Every step keeps
//! `width/2 + clearance` from copper of other nets on its layer (pads by their real outline, tracks, arcs, vias), stays
//! inside the outline by the edge clearance, and no via lands in a pad. From the Fusion bridge's routing_planner.py
//! (fusion-lane, #12), which searched two layers of an EAGLE board; this one reads the standard board model. A plan
//! only: nothing changes until the bridge runs route_net with `routeNet`.
use serde_json::{json, Value};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

type P = (f64, f64);

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }

fn seg_point(p: P, a: P, b: P) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    ((a.0 + t * dx - p.0).powi(2) + (a.1 + t * dy - p.1).powi(2)).sqrt()
}

/// The distance between two segments (0 when they cross).
pub(crate) fn seg_seg(a: P, b: P, c: P, d: P) -> f64 {
    let cross = |p: P, q: P, r: P| (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0);
    if cross(a, b, c) * cross(a, b, d) < 0.0 && cross(c, d, a) * cross(c, d, b) < 0.0 { return 0.0; }
    seg_point(a, c, d).min(seg_point(b, c, d)).min(seg_point(c, a, b)).min(seg_point(d, a, b))
}

pub(crate) fn inside(p: P, ring: &[P]) -> bool {
    let mut c = false;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 { c = !c; }
    }
    c
}

/// Copper as a shape: a capsule (a segment with a half-width: tracks, round pads, vias) or a polygon (rect pads, a
/// custom pad's copperPoints).
#[derive(Clone)]
pub(crate) enum Shape { Cap(P, P, f64), Poly(Vec<P>) }

impl Shape {
    /// Edge-to-edge distance from the segment a..b (its centreline) to this copper.
    pub(crate) fn dist(&self, a: P, b: P) -> f64 {
        match self {
            Shape::Cap(c, d, r) => seg_seg(a, b, *c, *d) - r,
            Shape::Poly(ring) => {
                if inside(a, ring) || inside(b, ring) { return 0.0; }
                (0..ring.len()).map(|i| seg_seg(a, b, ring[i], ring[(i + 1) % ring.len()])).fold(f64::MAX, f64::min)
            }
        }
    }
    /// Edge-to-edge distance between two pieces of copper, 0 when they touch or one holds the other.
    pub(crate) fn gap(&self, o: &Shape) -> f64 {
        match o {
            Shape::Cap(c, d, r) => (self.dist(*c, *d) - r).max(0.0),
            Shape::Poly(ring) => {
                let held = match self { Shape::Cap(c, _, _) => inside(*c, ring), Shape::Poly(own) => own.first().map(|p| inside(*p, ring)).unwrap_or(false) };
                if held { return 0.0; }
                (0..ring.len()).map(|i| self.dist(ring[i], ring[(i + 1) % ring.len()])).fold(f64::MAX, f64::min).max(0.0)
            }
        }
    }
    pub(crate) fn bounds(&self) -> (f64, f64, f64, f64) {
        match self {
            Shape::Cap(c, d, r) => (c.0.min(d.0) - r, c.1.min(d.1) - r, c.0.max(d.0) + r, c.1.max(d.1) + r),
            Shape::Poly(ring) => ring.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, p| (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1))),
        }
    }
}

/// A pad's copper: copperPoints when the bridge sent them, else its shape turned with it.
pub(crate) fn pad_shape(p: &Value) -> Shape {
    let (x, y, sx, sy, rot) = (f(&p["x"]), f(&p["y"]), f(&p["sizeX"]), f(&p["sizeY"]), f(&p["rotation"]).to_radians());
    let pts: Vec<P> = p["copperPoints"].as_array().cloned().unwrap_or_default().iter().map(|q| (f(&q[0]), f(&q[1]))).collect();
    if pts.len() >= 3 { return Shape::Poly(pts); }
    let (c, s) = (rot.cos(), rot.sin());
    match p["shape"].as_str() {
        Some("round") | Some("circle") => {
            let (l, w) = ((sx - sy).abs() / 2.0, sx.min(sy) / 2.0);
            let (ux, uy) = if sx >= sy { (c * l, s * l) } else { (-s * l, c * l) };
            Shape::Cap((x - ux, y - uy), (x + ux, y + uy), w)
        }
        _ => Shape::Poly([(-sx, -sy), (sx, -sy), (sx, sy), (-sx, sy)].iter().map(|(u, v)| (x + u / 2.0 * c - v / 2.0 * s, y + u / 2.0 * s + v / 2.0 * c)).collect()),
    }
}

/// The copper layers a pad is on: every one for a through-hole pad (`*.Cu` or a drill), else its own.
pub(crate) fn pad_layers(p: &Value, all: &[String]) -> Vec<String> {
    let l = p["layer"].as_str().unwrap_or("");
    if l == "*.Cu" || f(&p["drill"]) > 0.0 { all.to_vec() } else { vec![l.to_string()] }
}

/// plan_route: input {board, rules?, args: {net, from, to, widthMm?, layers?, allowVias?, viaSizeMm?, viaDrillMm?, gridMm?,
/// viaCost?, clearanceMm?, edgeClearanceMm?, maxNodes?}}. `from` and `to` are pads as Ref.Pad. The width and via come from
/// the args, else the board's width and via rules (`rules`), else 0.25 mm and 0.6/0.3; `planner` says which.
pub fn plan_route(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let num = |k: &str, d: f64| args.get(k).and_then(|v| v.as_f64()).unwrap_or(d);
    let Some(net) = args["net"].as_str() else { return crate::input::refuse("bad_args", "net is required") };
    if !st["nets"].as_array().map(|a| a.iter().any(|n| n.as_str().map(|s| s.eq_ignore_ascii_case(net)).unwrap_or(false))).unwrap_or(false) {
        return crate::input::refuse("no_such_net", &format!("{net} is not a net on this board"));
    }
    let pads = st["pads"].as_array().cloned().unwrap_or_default();
    let key = |p: &Value| format!("{}.{}", p["ref"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or(""));
    let mut ends = Vec::new();
    for k in ["from", "to"] {
        let Some(want) = args[k].as_str() else { return crate::input::refuse("bad_args", &format!("{k} is a pad as Ref.Pad (U1.3)")) };
        let Some(p) = pads.iter().find(|p| key(p).eq_ignore_ascii_case(want)) else { return crate::input::refuse("no_such_pad", &format!("{want} is not a pad on this board")) };
        if !p["net"].as_str().map(|n| n.eq_ignore_ascii_case(net)).unwrap_or(false) {
            return json!({"success": true, "found": false, "reason": format!("{want} is on {}, not {net}", p["net"].as_str().unwrap_or("no net"))});
        }
        ends.push(p.clone());
    }
    if key(&ends[0]).eq_ignore_ascii_case(&key(&ends[1])) { return crate::input::refuse("bad_args", "from and to are the same pad"); }
    let layers: Vec<String> = args["layers"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_else(|| vec!["F.Cu".into(), "B.Cu".into()]);
    if layers.is_empty() || layers.iter().any(|l| !l.ends_with(".Cu") || l == "*.Cu") { return crate::input::refuse("bad_args", "layers are copper layers by name: [\"F.Cu\", \"In1.Cu\", \"B.Cu\"]"); }
    // the board's own width and via rules, so the route passes its DRC (a 0.25 mm default broke a 10 mil Width rule on
    // Altium's lane board, 2026-10-28); args still win
    let rules = crate::input::rules(input);
    let (width_rule, via_rule) = (crate::features::width_style(&rules), crate::features::via_style(&rules));
    let width = num("widthMm", width_rule.unwrap_or(0.25));
    let width_from = if args.get("widthMm").is_some() { "widthMm" } else if width_rule.is_some() { "the board's width rule" } else { "default 0.25" };
    let (via_size, via_drill) = (num("viaSizeMm", via_rule.map(|v| v.0).unwrap_or(0.6)), num("viaDrillMm", via_rule.map(|v| v.1).unwrap_or(0.3)));
    let via_from = if args.get("viaSizeMm").is_some() { "viaSizeMm/viaDrillMm" } else if via_rule.is_some() { "the board's via rule" } else { "default 0.6/0.3" };
    if via_drill >= via_size { return crate::input::refuse("bad_args", "viaDrillMm must be smaller than viaSizeMm"); }
    let grid = num("gridMm", 0.5);
    if !(0.05..=5.0).contains(&grid) { return crate::input::refuse("bad_args", "gridMm is 0.05 to 5"); }
    let allow_vias = args["allowVias"].as_bool().unwrap_or(true) && layers.len() > 1;
    let via_cost = num("viaCost", 5.0);
    let max_nodes = args["maxNodes"].as_u64().unwrap_or(50_000).clamp(1, 500_000) as usize;
    // the board's strictest board-wide copper rule decides (fusion-lane #21: a via planned at the 0.1 wire rule landed
    // 0.109 mm from a pad where 0.15 applies); a caller's clearanceMm can only raise it
    let rule = crate::fab::routing_clearance(&rules);
    let clearance = num("clearanceMm", 0.0).max(rule.unwrap_or(0.2));
    let edge = num("edgeClearanceMm", 0.3);
    let ring = crate::coppermap::outline_ring(&st);
    if ring.len() < 3 { return crate::input::refuse("no_outline", "the board has no outline"); }

    // a through via passes every copper layer of the board, not only the ones the route may use (fusion-lane #21: a via
    // planned on F.Cu and In2.Cu landed 0.094 mm from a B.Cu track)
    let mut copper = crate::placetools::copper_layers(&st, input);
    for l in &layers { if !copper.contains(l) { copper.push(l.clone()); } }
    // other nets' pours: not obstacles (they re-pour round the route), but a route across one can cut it in two, so the
    // reply names each one it crosses, and avoidPours keeps the route out of them
    // avoidPours: true keeps out of every other net's pour, a list of nets out of theirs only; pourCost makes each mm run
    // inside a pour cost 1 + pourCost mm instead, a preference rather than a wall (fusion-lane #21: on BQ25792 almost
    // every spot is under some pour, and a hard block found no path)
    let avoid_nets: Option<Vec<String>> = match &args["avoidPours"] {
        Value::Bool(true) => Some(Vec::new()),
        Value::Array(a) => Some(a.iter().filter_map(|v| v.as_str().map(|s| s.to_uppercase())).collect()),
        _ => None,
    };
    let avoids = |n: &str| avoid_nets.as_ref().map(|v| v.is_empty() || v.contains(&n.to_uppercase())).unwrap_or(false);
    let pour_cost = num("pourCost", 0.0).max(0.0);
    let pours: Vec<(String, String, Shape)> = st["polygons"].as_array().cloned().unwrap_or_default().iter()
        .filter(|p| p["net"].as_str().map(|n| !n.is_empty() && !n.eq_ignore_ascii_case(net)).unwrap_or(false))
        .filter_map(|p| {
            let ring: Vec<P> = p["points"].as_array()?.iter().map(|q| (f(&q[0]), f(&q[1]))).collect();
            (ring.len() >= 3).then(|| (p["net"].as_str().unwrap_or("").to_string(), p["layer"].as_str().unwrap_or("").to_string(), Shape::Poly(ring)))
        }).collect();
    // a pour that holds the route's own end pad cannot be kept out of (fusion-lane #21: SW1.2 sits inside VBAT's F.Cu
    // pour, so avoidPours: ["VBAT"] found no path): it is exempt, and the reply names it
    let held: Vec<usize> = (0..pours.len()).filter(|&i| {
        let (_, l, s) = &pours[i];
        let Shape::Poly(ring) = s else { return false };
        ends.iter().any(|p| pad_layers(p, &copper).contains(l) && inside((f(&p["x"]), f(&p["y"])), ring))
    }).collect();
    let blocks = |i: usize| avoids(&pours[i].0) && !held.contains(&i);
    let exempt: Vec<Value> = held.iter().filter(|&&i| avoids(&pours[i].0)).map(|&i| json!({"net": pours[i].0, "layer": pours[i].1, "why": "it holds an end pad of the route"})).collect();
    // copper of other nets, by layer; every pad for the via-in-pad rule. Net-less tracks and arcs are a hatched pour's
    // pieces (Altium lists 8777 of the Bluetooth Sentinel's 9378 tracks with no net) and pours re-pour round new copper,
    // so neither blocks a route: repour after placing it (the reply says so)
    let same = |v: &Value| v["net"].as_str().map(|n| n.eq_ignore_ascii_case(net) || (n.is_empty() && v.get("start").or(v.get("center")).is_some())).unwrap_or(false);
    let mut ob: Vec<(Shape, Vec<String>, (f64, f64, f64, f64))> = Vec::new();
    let mut push = |s: Shape, ls: Vec<String>| { let b = s.bounds(); ob.push((s, ls, b)); };
    for p in pads.iter().filter(|p| !same(p)) { push(pad_shape(p), pad_layers(p, &copper)); }
    for t in st["tracks"].as_array().cloned().unwrap_or_default().iter().filter(|t| !same(t)) {
        push(Shape::Cap((f(&t["start"]["x"]), f(&t["start"]["y"])), (f(&t["end"]["x"]), f(&t["end"]["y"])), f(&t["width"]) / 2.0), vec![t["layer"].as_str().unwrap_or("").to_string()]);
    }
    for a in st["arcs"].as_array().cloned().unwrap_or_default().iter().filter(|a| !same(a)) {
        let (cx, cy, r) = (f(&a["center"]["x"]), f(&a["center"]["y"]), f(&a["radius"]));
        let (a1, mut a2) = (f(&a["startAngle"]), f(&a["endAngle"]));
        if a2 <= a1 { a2 += 360.0; }
        let n = ((a2 - a1) / 10.0).ceil().max(1.0) as usize;
        let p = |deg: f64| (cx + r * deg.to_radians().cos(), cy + r * deg.to_radians().sin());
        for i in 0..n { push(Shape::Cap(p(a1 + (a2 - a1) * i as f64 / n as f64), p(a1 + (a2 - a1) * (i + 1) as f64 / n as f64), f(&a["width"]) / 2.0), vec![a["layer"].as_str().unwrap_or("").to_string()]); }
    }
    for v in st["vias"].as_array().cloned().unwrap_or_default().iter().filter(|v| !same(v)) {
        let c = (f(&v["position"]["x"]), f(&v["position"]["y"]));
        push(Shape::Cap(c, c, f(&v["size"]) / 2.0), copper.clone());
    }
    let all_pads: Vec<Shape> = pads.iter().map(pad_shape).collect();

    // the grid: lines every gridMm over the outline, plus lines through both pad centres
    let (x0, x1) = ring.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
    let (y0, y1) = ring.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
    let cells = ((x1 - x0) / grid + 3.0) * ((y1 - y0) / grid + 3.0) * layers.len() as f64;
    if cells > 2_000_000.0 { return crate::input::refuse("search_budget", "the grid has too many cells: use a coarser gridMm"); }
    let axis = |lo: f64, hi: f64, extra: [f64; 2]| -> Vec<f64> {
        let mut v: Vec<f64> = (0..=((hi - lo) / grid).floor() as usize).map(|i| r3(lo + i as f64 * grid)).collect();
        v.extend(extra.iter().map(|e| r3(*e)));
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        v
    };
    let (sp, gp) = ((f(&ends[0]["x"]), f(&ends[0]["y"])), (f(&ends[1]["x"]), f(&ends[1]["y"])));
    let xs = axis(x0, x1, [sp.0, gp.0]);
    let ys = axis(y0, y1, [sp.1, gp.1]);
    let idx = |v: &[f64], t: f64| v.iter().position(|x| (x - r3(t)).abs() < 1e-9).unwrap();
    let (start, goal) = ((idx(&xs, sp.0), idx(&ys, sp.1)), (idx(&xs, gp.0), idx(&ys, gp.1)));
    let start_layers: Vec<usize> = pad_layers(&ends[0], &layers).iter().filter_map(|l| layers.iter().position(|x| x == l)).collect();
    let goal_layers: Vec<usize> = pad_layers(&ends[1], &layers).iter().filter_map(|l| layers.iter().position(|x| x == l)).collect();
    if start_layers.is_empty() || goal_layers.is_empty() {
        return json!({"success": true, "found": false, "reason": "a pad is on no layer the route may use: add its layer to layers"});
    }
    if !allow_vias && !start_layers.iter().any(|l| goal_layers.contains(l)) {
        return json!({"success": true, "found": false, "reason": "the pads share no layer and allowVias is false"});
    }

    let edge_ok = |a: P, b: P, r: f64| inside(a, &ring) && inside(b, &ring)
        && (0..ring.len()).all(|k| seg_seg(a, b, ring[k], ring[(k + 1) % ring.len()]) >= edge + r - 1e-9);
    let clear_on = |a: P, b: P, r: f64, layer: &str| {
        let (bx0, by0, bx1, by1) = (a.0.min(b.0) - r - clearance, a.1.min(b.1) - r - clearance, a.0.max(b.0) + r + clearance, a.1.max(b.1) + r + clearance);
        ob.iter().filter(|(_, ls, bb)| ls.iter().any(|l| l == layer) && !(bb.2 < bx0 || bb.0 > bx1 || bb.3 < by0 || bb.1 > by1))
            .all(|(s, _, _)| s.dist(a, b) >= r + clearance - 1e-9)
    };
    let mut cache: HashMap<(usize, usize, usize, usize, usize, usize), bool> = HashMap::new();
    let mut step_ok = |u: (usize, usize, usize), v: (usize, usize, usize)| -> bool {
        let k = if u <= v { (u.0, u.1, u.2, v.0, v.1, v.2) } else { (v.0, v.1, v.2, u.0, u.1, u.2) };
        if let Some(b) = cache.get(&k) { return *b; }
        let (a, b) = ((xs[u.0], ys[u.1]), (xs[v.0], ys[v.1]));
        let ok = if u.2 != v.2 {
            // a through via: clear on every layer, inside the outline, in no pad
            let r = via_size / 2.0;
            edge_ok(a, a, r) && copper.iter().all(|l| clear_on(a, a, r, l)) && all_pads.iter().all(|s| s.dist(a, a) >= r + clearance - 1e-9)
        } else {
            edge_ok(a, b, width / 2.0) && clear_on(a, b, width / 2.0, &layers[u.2])
                && !(0..pours.len()).any(|i| blocks(i) && pours[i].1 == layers[u.2] && pours[i].2.dist(a, b) < width / 2.0)
        };
        cache.insert(k, ok);
        ok
    };
    let h = |u: (usize, usize, usize)| (xs[u.0] - gp.0).abs() + (ys[u.1] - gp.1).abs() + if goal_layers.contains(&u.2) { 0.0 } else { via_cost };
    let q = |v: f64| (v * 1e6).round() as i64;
    let mut heap: BinaryHeap<Reverse<(i64, i64, (usize, usize, usize))>> = BinaryHeap::new();
    let mut cost: HashMap<(usize, usize, usize), f64> = HashMap::new();
    let mut parent: HashMap<(usize, usize, usize), (usize, usize, usize)> = HashMap::new();
    for &l in &start_layers { let u = (start.0, start.1, l); cost.insert(u, 0.0); heap.push(Reverse((q(h(u)), 0, u))); }
    let mut seen: std::collections::HashSet<(usize, usize, usize)> = Default::default();
    let mut found = None;
    while let Some(Reverse((_, _, u))) = heap.pop() {
        if !seen.insert(u) { continue; }
        if (u.0, u.1) == goal && goal_layers.contains(&u.2) { found = Some(u); break; }
        if seen.len() >= max_nodes { break; }
        let g = cost[&u];
        let mut nb = Vec::new();
        if u.0 + 1 < xs.len() { nb.push((u.0 + 1, u.1, u.2)); }
        if u.0 > 0 { nb.push((u.0 - 1, u.1, u.2)); }
        if u.1 + 1 < ys.len() { nb.push((u.0, u.1 + 1, u.2)); }
        if u.1 > 0 { nb.push((u.0, u.1 - 1, u.2)); }
        if allow_vias { for l in 0..layers.len() { if l != u.2 { nb.push((u.0, u.1, l)); } } }
        for v in nb {
            if seen.contains(&v) || !step_ok(u, v) { continue; }
            let step = if v.2 != u.2 { via_cost } else {
                let (a, b) = ((xs[u.0], ys[u.1]), (xs[v.0], ys[v.1]));
                let len = (b.0 - a.0).abs() + (b.1 - a.1).abs();
                if pour_cost > 0.0 && pours.iter().any(|(_, l, s)| *l == layers[u.2] && s.dist(a, b) < width / 2.0) { len * (1.0 + pour_cost) } else { len }
            };
            let ng = g + step;
            if ng >= cost.get(&v).copied().unwrap_or(f64::MAX) - 1e-9 { continue; }
            cost.insert(v, ng);
            parent.insert(v, u);
            heap.push(Reverse((q(ng + h(v)), q(ng), v)));
        }
    }
    let revision = st["revision"].clone();
    let Some(goal_node) = found else {
        let why = if seen.len() >= max_nodes { "no path within the search limit (maxNodes): try a coarser gridMm or a higher maxNodes" } else { "no legal path on this grid: other copper, the outline or the clearance blocks every way. A fine-pitch pad (0.5 mm pitch) usually needs gridMm 0.1 to escape; more layers give more room" };
        // what an avoided pour does at each end: a pour the end pad sits in is exempt; one that only touches the pad's
        // first step out still walls it in (fusion-lane #21: avoidPours ["VBAT"] found no path from SW1.2)
        let near_ends: Vec<Value> = ends.iter().map(|p| {
            let c = (f(&p["x"]), f(&p["y"]));
            let reach = grid + width / 2.0 + clearance;
            let touching: Vec<Value> = (0..pours.len()).filter(|&i| avoids(&pours[i].0) && pad_layers(p, &copper).contains(&pours[i].1) && pours[i].2.dist(c, c) <= reach)
                .map(|i| json!({"net": pours[i].0, "layer": pours[i].1, "exempt": held.contains(&i)})).collect();
            json!({"pad": key(p), "avoidedPoursHere": touching})
        }).collect();
        let walled = near_ends.iter().any(|e| e["avoidedPoursHere"].as_array().map(|a| a.iter().any(|x| x["exempt"] == json!(false))).unwrap_or(false));
        // when keeping out of the pours is what fails, say so: plan again through them (fusion-lane #21: VBAT's pour wraps
        // SW1.2 in a notch it never touches)
        let mut through = Value::Null;
        if avoid_nets.is_some() && seen.len() < max_nodes {
            let mut again = input.clone();
            again["args"].as_object_mut().map(|o| o.remove("avoidPours"));
            let r = plan_route(&again);
            if r["found"] == json!(true) { through = json!({"found": true, "lengthMm": r["lengthMm"], "vias": r["vias"], "crossesPours": r["crossesPours"]}); }
        }
        let why = if avoid_nets.is_some() && walled { "no legal path: an avoided pour touches an end pad without holding it (avoidedPoursHere), so the route cannot leave the pad without entering it. Try pourCost instead of avoidPours, or drop that net from avoidPours".to_string() }
            else if !through.is_null() { format!("no path keeps out of the avoided pours: every way between the pads crosses one (a path through them exists: withoutAvoid). Use pourCost to go round them where it can, or drop a net from avoidPours") }
            else { why.to_string() };
        return json!({"success": true, "found": false, "reason": why, "expandedNodes": seen.len(), "revision": revision, "ends": near_ends, "withoutAvoid": through,
            "planner": {"algorithm": "bounded orthogonal A*", "widthFrom": width_from, "viaFrom": via_from, "gridMm": grid, "layers": layers, "viasCheckedOn": copper, "clearanceMm": clearance, "edgeClearanceMm": edge,
                        "avoidPours": args["avoidPours"].clone(), "avoidExempt": exempt, "pourCost": pour_cost, "expandedNodes": seen.len()}});
    };
    let mut path = vec![goal_node];
    while let Some(p) = parent.get(path.last().unwrap()) { path.push(*p); }
    path.reverse();
    // smooth the grid path: from each point, the farthest later point on the same layer that a straight or 45-degree
    // segment reaches with the same clearance, inside the outline and into no pour the grid path kept out of. Staircases
    // become diagonals (fusion-lane #21: Fusion's DRC flags every vertex of a staircase). smooth: false keeps the grid path.
    let at = |u: (usize, usize, usize)| (xs[u.0], ys[u.1]);
    let pours_hit = |a: P, b: P, l: usize| -> Vec<usize> { (0..pours.len()).filter(|&i| pours[i].1 == layers[l] && pours[i].2.dist(a, b) < width / 2.0).collect() };
    let octilinear = |a: P, b: P| { let (dx, dy) = ((b.0 - a.0).abs(), (b.1 - a.1).abs()); dx < 1e-9 || dy < 1e-9 || (dx - dy).abs() < 1e-9 };
    let smooth = args["smooth"].as_bool().unwrap_or(true);
    let mut pts = vec![path[0]];
    let mut i = 0;
    while i + 1 < path.len() {
        if path[i + 1].2 != path[i].2 || !smooth { pts.push(path[i + 1]); i += 1; continue; }
        let (mut best, mut j) = (i + 1, i + 2);
        let mut grid_pours: Vec<usize> = pours_hit(at(path[i]), at(path[i + 1]), path[i].2);
        while j < path.len() && path[j].2 == path[i].2 {
            for p in pours_hit(at(path[j - 1]), at(path[j]), path[i].2) { if !grid_pours.contains(&p) { grid_pours.push(p); } }
            let (a, b) = (at(path[i]), at(path[j]));
            if octilinear(a, b) && edge_ok(a, b, width / 2.0) && clear_on(a, b, width / 2.0, &layers[path[i].2])
                && pours_hit(a, b, path[i].2).iter().all(|p| grid_pours.contains(p) && !blocks(*p)) { best = j; }
            j += 1;
        }
        pts.push(path[best]);
        i = best;
    }
    // keep the corners and the layer changes
    let mut keep = vec![pts[0]];
    for w in pts.windows(3) {
        let (a, b, c) = (w[0], w[1], w[2]);
        let (pa, pb, pc) = (at(a), at(b), at(c));
        let straight = ((pb.0 - pa.0) * (pc.1 - pb.1) - (pb.1 - pa.1) * (pc.0 - pb.0)).abs() < 1e-9 && (pb.0 - pa.0) * (pc.0 - pb.0) + (pb.1 - pa.1) * (pc.1 - pb.1) >= 0.0;
        if a.2 == b.2 && b.2 == c.2 && straight { continue; }
        keep.push(b);
    }
    if pts.len() > 1 { keep.push(*pts.last().unwrap()); }
    let mut way = vec![json!({"pad": key(&ends[0])})];
    let mut vias = Vec::new();
    let mut length = 0.0;
    let mut crosses: Vec<Value> = Vec::new();
    let last = keep.len() - 1;
    for (i, w) in keep.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        let (pa, pb) = ((xs[a.0], ys[a.1]), (xs[b.0], ys[b.1]));
        if a.2 != b.2 { vias.push(json!([pa.0, pa.1])); } else {
            length += ((pb.0 - pa.0).powi(2) + (pb.1 - pa.1).powi(2)).sqrt();
            for (n, l, s) in &pours {
                if *l == layers[a.2] && s.dist(pa, pb) < width / 2.0 && !crosses.iter().any(|c: &Value| c["net"] == json!(n) && c["layer"] == json!(l)) { crosses.push(json!({"net": n, "layer": l})); }
            }
        }
        // every corner and layer change; the goal itself is the closing {pad} waypoint
        if i + 1 < last { way.push(json!({"x": pb.0, "y": pb.1, "layer": layers[b.2]})); }
    }
    way.push(json!({"pad": key(&ends[1])}));
    json!({"success": true, "found": true, "lengthMm": r3(length), "vias": vias.len(), "viaAt": vias,
        "routeNet": {"net": net, "layer": layers[path[0].2], "width": width, "viaSize": via_size, "viaDrill": via_drill, "paths": [way]},
        "planner": {"algorithm": if smooth { "bounded orthogonal A*, smoothed to straight and 45-degree segments" } else { "bounded orthogonal A*" }, "widthFrom": width_from, "viaFrom": via_from, "gridMm": grid, "layers": layers, "viasCheckedOn": copper, "clearanceMm": clearance, "edgeClearanceMm": edge,
            "avoidPours": args["avoidPours"].clone(), "avoidExempt": exempt, "pourCost": pour_cost, "smooth": smooth, "expandedNodes": seen.len()},
        "crossesPours": crosses, "avoidPours": args["avoidPours"].clone(), "pourCost": pour_cost, "revision": revision,
        "_hint": if crosses.is_empty() { "A plan only: pass routeNet to route_net unchanged (with expectedRevision: revision). A layer change is two waypoints at the same point, where route_net places a via. Pours (and a hatched pour's net-less pieces) are not obstacles: repour after placing the route. The path is orthogonal on the grid: a finer gridMm finds tighter paths and searches longer." }
                 else { "A plan only: pass routeNet to route_net unchanged (with expectedRevision: revision). The route runs through the pours in crossesPours: after the repour, check that none of those nets split in two (routing_state's splitNets), or plan again keeping out of them: avoidPours: true (or a list of the nets whose pours to keep out of), or pourCost: 2 to prefer going round them where a way exists. A layer change is two waypoints at the same point, where route_net places a via." }})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(extra_tracks: Value) -> Value {
        json!({"nets": ["SIG", "GND"], "revision": "r1", "outline": [[0, 0], [20, 0], [20, 10], [0, 10]],
               "outlineEdges": [{"kind": "line", "start": [0.0, 0.0]}, {"kind": "line", "start": [20.0, 0.0]}, {"kind": "line", "start": [20.0, 10.0]}, {"kind": "line", "start": [0.0, 10.0]}],
               "pads": [{"ref": "U1", "name": "1", "x": 2.0, "y": 5.0, "sizeX": 1.0, "sizeY": 1.0, "rotation": 0.0, "shape": "rect", "layer": "F.Cu", "drill": 0.0, "net": "SIG"},
                        {"ref": "J1", "name": "1", "x": 18.0, "y": 5.0, "sizeX": 1.0, "sizeY": 1.0, "rotation": 0.0, "shape": "rect", "layer": "F.Cu", "drill": 0.0, "net": "SIG"},
                        {"ref": "J1", "name": "2", "x": 18.0, "y": 2.0, "sizeX": 1.0, "sizeY": 1.0, "rotation": 0.0, "shape": "rect", "layer": "B.Cu", "drill": 0.0, "net": "SIG"}],
               "tracks": extra_tracks, "arcs": [], "vias": []})
    }

    #[test]
    fn a_clear_board_is_one_straight_track() {
        let r = plan_route(&json!({"board": board(json!([])), "args": {"net": "SIG", "from": "U1.1", "to": "J1.1"}}));
        assert_eq!(r["found"], json!(true), "{r}");
        assert_eq!(r["vias"], 0);
        assert!((r["lengthMm"].as_f64().unwrap() - 16.0).abs() < 1e-9);
        assert_eq!(r["routeNet"]["paths"][0][0], json!({"pad": "U1.1"}));
        assert_eq!(r["routeNet"]["paths"][0].as_array().unwrap().last().unwrap(), &json!({"pad": "J1.1"}));
        assert_eq!(r["routeNet"]["layer"], "F.Cu");
    }

    #[test]
    fn a_wall_on_top_is_crossed_on_the_bottom_or_not_at_all() {
        // a GND track across the whole board on F.Cu
        let wall = json!([{"net": "GND", "layer": "F.Cu", "width": 0.5, "start": {"x": 10.0, "y": 0.0}, "end": {"x": 10.0, "y": 10.0}}]);
        let r = plan_route(&json!({"board": board(wall.clone()), "args": {"net": "SIG", "from": "U1.1", "to": "J1.1"}}));
        assert_eq!(r["found"], json!(true), "{r}");
        assert_eq!(r["vias"], 2, "down to B.Cu and back up: {r}");
        // every point of the path clears the wall on F.Cu
        for w in r["routeNet"]["paths"][0].as_array().unwrap() { if w["layer"] == "F.Cu" { assert!((f(&w["x"]) - 10.0).abs() > 0.25 + 0.2 + 0.125 - 1e-9, "{w}"); } }
        let none = plan_route(&json!({"board": board(wall), "args": {"net": "SIG", "from": "U1.1", "to": "J1.1", "allowVias": false}}));
        assert_eq!(none["found"], json!(false));
    }

    #[test]
    fn pads_on_different_layers_take_a_via_and_errors_are_named() {
        let r = plan_route(&json!({"board": board(json!([])), "args": {"net": "SIG", "from": "U1.1", "to": "J1.2"}}));
        assert_eq!(r["found"], json!(true), "{r}");
        assert_eq!(r["vias"], 1);
        // the via is clear of every pad
        let v = &r["viaAt"][0];
        for (px, py) in [(2.0, 5.0), (18.0, 5.0), (18.0, 2.0)] { assert!(((f(&v[0]) - px).powi(2) + (f(&v[1]) - py).powi(2)).sqrt() > 0.3 + 0.5, "{v}"); }
        assert_eq!(plan_route(&json!({"board": board(json!([])), "args": {"net": "SIG", "from": "U9.1", "to": "J1.1"}}))["errorCode"], "no_such_pad");
        assert_eq!(plan_route(&json!({"board": board(json!([])), "args": {"net": "NOPE", "from": "U1.1", "to": "J1.1"}}))["errorCode"], "no_such_net");
        let mut b = board(json!([]));
        b["pads"][1]["net"] = json!("GND");
        assert_eq!(plan_route(&json!({"board": b, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1"}}))["found"], json!(false));
    }

    #[test]
    fn three_layers_when_asked() {
        // walls on F.Cu and B.Cu: only In1.Cu crosses
        let walls = json!([{"net": "GND", "layer": "F.Cu", "width": 0.5, "start": {"x": 10.0, "y": 0.0}, "end": {"x": 10.0, "y": 10.0}},
                           {"net": "GND", "layer": "B.Cu", "width": 0.5, "start": {"x": 10.0, "y": 0.0}, "end": {"x": 10.0, "y": 10.0}}]);
        let two = plan_route(&json!({"board": board(walls.clone()), "args": {"net": "SIG", "from": "U1.1", "to": "J1.1"}}));
        assert_eq!(two["found"], json!(false));
        let three = plan_route(&json!({"board": board(walls), "args": {"net": "SIG", "from": "U1.1", "to": "J1.1", "layers": ["F.Cu", "In1.Cu", "B.Cu"]}}));
        assert_eq!(three["found"], json!(true), "{three}");
        assert!(three["routeNet"]["paths"][0].as_array().unwrap().iter().any(|w| w["layer"] == "In1.Cu"));
    }
    #[test]
    fn the_track_and_via_follow_the_boards_rules() {
        // Altium's lane board: a 10 mil Width rule and a 0.5/0.3 Routing Via rule
        let rules = json!([{"enabled": true, "scope1": "All", "priority": 1, "descriptor": "Width Constraint (Min=10mil) (Max=10mil) (Preferred=10mil) (All)"},
                           {"enabled": true, "scope1": "All", "priority": 1, "descriptor": "Routing Via (PreferredHoleWidth=0.3mm) (PreferedWidth=0.5mm) (All)"}]);
        let r = plan_route(&json!({"board": board(json!([])), "rules": rules, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1"}}));
        assert_eq!((r["routeNet"]["width"].clone(), r["routeNet"]["viaSize"].clone()), (json!(0.254), json!(0.5)), "{r}");
        assert_eq!(r["planner"]["widthFrom"], "the board's width rule");
        let given = plan_route(&json!({"board": board(json!([])), "rules": rules, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1", "widthMm": 0.4}}));
        assert_eq!(given["routeNet"]["width"], json!(0.4));
        assert_eq!(plan_route(&json!({"board": board(json!([])), "args": {"net": "SIG", "from": "U1.1", "to": "J1.1"}}))["planner"]["widthFrom"], "default 0.25");
    }
    #[test]
    fn a_through_via_clears_every_copper_layer_not_only_the_planned_ones() {
        // a wall on F.Cu forces two vias to In1.Cu; a GND track on B.Cu along y 5 is on no planned layer, but every via
        // passes through B.Cu (fusion-lane #21)
        let t = json!([{"net": "GND", "layer": "F.Cu", "width": 0.5, "start": {"x": 10.0, "y": 0.0}, "end": {"x": 10.0, "y": 10.0}},
                       {"net": "GND", "layer": "B.Cu", "width": 0.5, "start": {"x": 1.0, "y": 5.0}, "end": {"x": 19.0, "y": 5.0}}]);
        let r = plan_route(&json!({"board": board(t), "args": {"net": "SIG", "from": "U1.1", "to": "J1.1", "layers": ["F.Cu", "In1.Cu"]}}));
        assert_eq!(r["found"], json!(true), "{r}");
        assert_eq!(r["vias"], 2);
        for v in r["viaAt"].as_array().unwrap() { assert!((f(&v[1]) - 5.0).abs() >= 0.25 + 0.3 + 0.2 - 1e-9, "{v}"); }
        assert!(r["planner"]["viasCheckedOn"].as_array().unwrap().contains(&json!("B.Cu")));
    }

    #[test]
    fn the_strictest_board_wide_clearance_rule_decides() {
        let rules = json!([{"kind": "clearance", "name": "wire-wire", "clearanceMm": 0.1}, {"kind": "clearance", "name": "pad-via", "clearanceMm": 0.15},
                           {"name": "Clearance HV", "scope1": "InNet('HV')", "scope2": "All", "descriptor": "Clearance Constraint (Gap=0.5mm) (InNet('HV')),(All)"}]);
        let r = plan_route(&json!({"board": board(json!([])), "rules": rules, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1"}}));
        assert_eq!(r["planner"]["clearanceMm"], json!(0.15), "{r}");
        // fab_check still compares the smallest with a fab's limits
        assert_eq!(crate::fab::min_clearance(rules.as_array().unwrap()), Some(0.1));
    }

    #[test]
    fn a_route_names_the_pours_it_crosses_or_keeps_out_of_them() {
        let mut b = board(json!([]));
        b["polygons"] = json!([{"net": "GND", "layer": "F.Cu", "points": [[8.0, 0.0], [12.0, 0.0], [12.0, 10.0], [8.0, 10.0]]}]);
        let r = plan_route(&json!({"board": b, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1"}}));
        assert_eq!((r["vias"].as_u64(), r["crossesPours"].clone()), (Some(0), json!([{"net": "GND", "layer": "F.Cu"}])), "{r}");
        let around = plan_route(&json!({"board": b, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1", "avoidPours": true}}));
        assert_eq!((around["vias"].as_u64(), around["crossesPours"].clone()), (Some(2), json!([])), "{around}");
        // by net: avoiding another net's pours only leaves this one crossable
        assert_eq!(plan_route(&json!({"board": b, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1", "avoidPours": ["VBAT"]}}))["vias"], json!(0));
        assert_eq!(plan_route(&json!({"board": b, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1", "avoidPours": ["gnd"]}}))["vias"], json!(2));
        // a wall of pours across both layers: a hard block finds no path, a cost still routes (across it)
        let mut wall = board(json!([]));
        wall["polygons"] = json!([{"net": "GND", "layer": "F.Cu", "points": [[8.0, 0.0], [12.0, 0.0], [12.0, 10.0], [8.0, 10.0]]}, {"net": "GND", "layer": "B.Cu", "points": [[8.0, 0.0], [12.0, 0.0], [12.0, 10.0], [8.0, 10.0]]}]);
        assert_eq!(plan_route(&json!({"board": wall, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1", "avoidPours": true}}))["found"], json!(false));
        assert_eq!(plan_route(&json!({"board": wall, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1", "pourCost": 2.0}}))["found"], json!(true));
        // a pour that holds the route's own end pad cannot be kept out of: it is exempt and named (fusion-lane #21)
        let mut all = board(json!([]));
        all["polygons"] = json!([{"net": "GND", "layer": "F.Cu", "points": [[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [0.0, 10.0]]}]);
        let held = plan_route(&json!({"board": all, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1", "avoidPours": ["GND"]}}));
        assert_eq!(held["found"], json!(true), "{held}");
        assert_eq!(held["planner"]["avoidExempt"][0]["net"], "GND");
        assert_eq!(held["planner"]["pourCost"], json!(0.0));
        // a pour that touches U1.1 without holding it walls the pad in: the no-path reply says so, with its planner block
        let mut touch = board(json!([]));
        touch["polygons"] = json!([{"net": "GND", "layer": "F.Cu", "points": [[2.6, 0.0], [12.0, 0.0], [12.0, 10.0], [2.6, 10.0]]}, {"net": "GND", "layer": "B.Cu", "points": [[2.6, 0.0], [12.0, 0.0], [12.0, 10.0], [2.6, 10.0]]}]);
        let walled = plan_route(&json!({"board": touch, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1", "avoidPours": ["GND"]}}));
        assert_eq!(walled["found"], json!(false), "{walled}");
        assert_eq!(walled["ends"][0]["avoidedPoursHere"][0], json!({"net": "GND", "layer": "F.Cu", "exempt": false}), "{walled}");
        assert!(walled["reason"].as_str().unwrap().contains("avoidedPoursHere"));
        assert_eq!(walled["planner"]["avoidPours"], json!(["GND"]));
        // a wall it never touches: the reply says a path exists through the pours
        let wall_only = plan_route(&json!({"board": wall, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1", "avoidPours": true}}));
        assert_eq!(wall_only["withoutAvoid"]["found"], json!(true), "{wall_only}");
        assert!(wall_only["reason"].as_str().unwrap().contains("every way between the pads crosses one"));
        // and with the cost a short way round a narrow pour beats crossing it
        let mut strip = board(json!([]));
        strip["polygons"] = json!([{"net": "GND", "layer": "F.Cu", "points": [[9.0, 2.0], [11.0, 2.0], [11.0, 10.0], [9.0, 10.0]]}]);
        let soft = plan_route(&json!({"board": strip, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1", "pourCost": 10.0, "allowVias": false}}));
        assert_eq!(soft["crossesPours"], json!([]), "{soft}");
    }
    #[test]
    fn a_staircase_becomes_one_45_degree_segment() {
        // U1.1 at (2, 5) to a pad at (8, 9): the grid path is a staircase of corners; smoothed, the plan is straight and
        // 45-degree runs only, the diagonal clear of everything
        let mut b = board(json!([]));
        b["pads"][1]["x"] = json!(8.0); b["pads"][1]["y"] = json!(9.0);
        let r = plan_route(&json!({"board": b, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1"}}));
        let way = r["routeNet"]["paths"][0].as_array().unwrap().clone();
        let mut pts = vec![(2.0, 5.0)];
        pts.extend(way[1..way.len() - 1].iter().map(|w| (f(&w["x"]), f(&w["y"]))));
        pts.push((8.0, 9.0));
        assert!(pts.len() <= 3, "at most one corner: {r}");
        for w in pts.windows(2) {
            let (dx, dy) = ((w[1].0 - w[0].0).abs(), (w[1].1 - w[0].1).abs());
            assert!(dx < 1e-9 || dy < 1e-9 || (dx - dy).abs() < 1e-9, "{w:?}");
        }
        assert!(pts.windows(2).any(|w| (w[1].0 - w[0].0).abs() > 1e-9 && (w[1].1 - w[0].1).abs() > 1e-9), "one diagonal: {r}");
        // shorter than the 10 mm staircase: 4 * sqrt(2) + 2
        assert!((f(&r["lengthMm"]) - (4.0 * 2f64.sqrt() + 2.0)).abs() < 1e-3, "{r}");
        let grid = plan_route(&json!({"board": b, "args": {"net": "SIG", "from": "U1.1", "to": "J1.1", "smooth": false}}));
        assert!((f(&grid["lengthMm"]) - 10.0).abs() < 1e-9);
    }
}

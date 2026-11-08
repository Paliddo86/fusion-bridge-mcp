//! The copper of each layer as a raster (0.1 mm by default) inside the board outline, from the live board: tracks,
//! arcs, pads, vias and the copper Altium actually poured (its regions minus their holes); an internal plane counts as
//! copper everywhere. From it: how much of each layer is copper and how evenly the stack is balanced (bow and twist),
//! and where signal tracks run over a gap in the copper of the layer next to them (the return current's path).
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r1(v: f64) -> f64 { (v * 10.0).round() / 10.0 }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }

/// A layer's copper as cells of `res` mm from (x0, y0).
#[derive(Clone)]
pub struct Raster { pub x0: f64, pub y0: f64, pub res: f64, pub w: usize, pub h: usize, pub cells: Vec<bool> }

impl Raster {
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64, res: f64) -> Raster {
        let w = (((x1 - x0) / res).ceil() as usize).max(1);
        let h = (((y1 - y0) / res).ceil() as usize).max(1);
        Raster { x0, y0, res, w, h, cells: vec![false; w * h] }
    }
    fn centre(&self, i: usize, j: usize) -> (f64, f64) { (self.x0 + (i as f64 + 0.5) * self.res, self.y0 + (j as f64 + 0.5) * self.res) }
    /// cells whose centre is in [x0, x1] x [y0, y1], clipped
    fn span(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> (usize, usize, usize, usize) {
        let c = |v: f64, o: f64, n: usize| (((v - o) / self.res - 0.5).max(0.0) as usize).min(n);
        let (i0, j0) = (c(x0, self.x0, self.w), c(y0, self.y0, self.h));
        let (i1, j1) = ((((x1 - self.x0) / self.res + 0.5).max(0.0) as usize).min(self.w), (((y1 - self.y0) / self.res + 0.5).max(0.0) as usize).min(self.h));
        (i0, j0, i1, j1)
    }
    pub fn at(&self, x: f64, y: f64) -> bool {
        let (i, j) = ((x - self.x0) / self.res, (y - self.y0) / self.res);
        if i < 0.0 || j < 0.0 || i >= self.w as f64 || j >= self.h as f64 { return false; }
        self.cells[j as usize * self.w + i as usize]
    }
    /// a segment of width w with round ends (a track, an arc's piece, an oval pad)
    pub fn capsule(&mut self, ax: f64, ay: f64, bx: f64, by: f64, w: f64) {
        let r = w / 2.0;
        let (i0, j0, i1, j1) = self.span(ax.min(bx) - r, ay.min(by) - r, ax.max(bx) + r, ay.max(by) + r);
        let (dx, dy) = (bx - ax, by - ay);
        let l2 = dx * dx + dy * dy;
        for j in j0..j1 { for i in i0..i1 {
            let (x, y) = self.centre(i, j);
            let t = if l2 > 0.0 { (((x - ax) * dx + (y - ay) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let (px, py) = (ax + t * dx - x, ay + t * dy - y);
            if px * px + py * py <= r * r { self.cells[j * self.w + i] = true; }
        } }
    }
    /// a rectangle sx by sy centred at (cx, cy), turned `rot` degrees
    pub fn rect(&mut self, cx: f64, cy: f64, sx: f64, sy: f64, rot: f64) {
        let (c, s) = (rot.to_radians().cos(), rot.to_radians().sin());
        let e = (sx * sx + sy * sy).sqrt() / 2.0;
        let (i0, j0, i1, j1) = self.span(cx - e, cy - e, cx + e, cy + e);
        for j in j0..j1 { for i in i0..i1 {
            let (x, y) = self.centre(i, j);
            let (u, v) = ((x - cx) * c + (y - cy) * s, -(x - cx) * s + (y - cy) * c);
            if u.abs() <= sx / 2.0 && v.abs() <= sy / 2.0 { self.cells[j * self.w + i] = true; }
        } }
    }
    /// polygons filled by the even-odd rule (an outline with its holes), set to `on`
    pub fn polys(&mut self, rings: &[Vec<(f64, f64)>], on: bool) {
        let ys: Vec<f64> = rings.iter().flatten().map(|p| p.1).collect();
        if ys.is_empty() { return; }
        let (lo, hi) = (ys.iter().cloned().fold(f64::MAX, f64::min), ys.iter().cloned().fold(f64::MIN, f64::max));
        let (_, j0, _, j1) = self.span(self.x0, lo, self.x0, hi);
        for j in j0..j1 {
            let y = self.y0 + (j as f64 + 0.5) * self.res;
            let mut xs: Vec<f64> = Vec::new();
            for ring in rings {
                let n = ring.len();
                for k in 0..n {
                    let (a, b) = (ring[k], ring[(k + 1) % n]);
                    if (a.1 <= y) != (b.1 <= y) { xs.push(a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0)); }
                }
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            for p in xs.chunks(2) {
                if p.len() < 2 { continue; }
                let (i0, _, i1, _) = self.span(p[0], y, p[1], y);
                for i in i0..i1 { self.cells[j * self.w + i] = on; }
            }
        }
    }
    pub fn and(&mut self, mask: &Raster) { for (c, m) in self.cells.iter_mut().zip(&mask.cells) { *c = *c && *m; } }
    pub fn count(&self) -> usize { self.cells.iter().filter(|c| **c).count() }
}

/// The stack's copper layers in order as (board model name, plane?, plane net). The name is the stack entry's `layer`
/// when the bridge gives it, else its place in the copper stack (F.Cu, In<n>.Cu, B.Cu): a plane is named like any other
/// inner layer (standard 0.3.1), never by a tool's id.
pub fn stack_layers(stack: &Value) -> Vec<(String, bool, String)> {
    let ls: Vec<Value> = stack["layers"].as_array().cloned().unwrap_or_default();
    let n = ls.len();
    ls.iter().enumerate().map(|(i, l)| {
        let plane = l["kind"] == "plane";
        let name = l["layer"].as_str().map(str::to_string).unwrap_or_else(|| if i == 0 { "F.Cu".to_string() } else if i + 1 == n { "B.Cu".to_string() } else { format!("In{i}.Cu") });
        (name, plane, if plane { l["net"].as_str().unwrap_or("").to_string() } else { String::new() })
    }).collect()
}

/// The outline as a ring in mm (arc edges sampled).
pub(crate) fn outline_ring(st: &Value) -> Vec<(f64, f64)> {
    let mut ring = Vec::new();
    for e in st["outlineEdges"].as_array().cloned().unwrap_or_default() {
        if e["kind"] == "arc" {
            let (cx, cy, r) = (f(&e["center"][0]), f(&e["center"][1]), f(&e["radius"]));
            let (a1, mut a2) = (f(&e["angle1"]), f(&e["angle2"]));
            if a2 <= a1 { a2 += 360.0; }
            // every 2 degrees: a chord cuts inside the arc by r(1 - cos 1deg), 3 um on a 20 mm radius (16 points per
            // arc put a hatched pour's ends 0.04 mm outside the Bluetooth Sentinel example's edge)
            let k = (((a2 - a1) / 2.0).ceil() as usize).max(4);
            for q in 0..=k { let t = (a1 + (a2 - a1) * q as f64 / k as f64).to_radians(); ring.push((cx + r * t.cos(), cy + r * t.sin())); }
        } else { ring.push((f(&e["start"][0]), f(&e["start"][1]))); }
    }
    if ring.is_empty() { ring = st["outline"].as_array().cloned().unwrap_or_default().iter().map(|p| (f(&p[0]), f(&p[1]))).collect(); }
    ring
}

/// Each layer's copper raster inside the outline. `refs` None: all copper. Some(nets): only the copper a return current
/// can use: pours, planes, those nets' own copper, and net-less copper (Altium lists a hatched pour's tracks with no
/// net: 8777 of the Bluetooth Sentinel example's 9378 tracks). Pure: routing_state, pour_state's zones, the stack.
pub fn rasters(st: &Value, zones: &[Value], layers: &[(String, bool, String)], res: f64, refs: Option<&[String]>) -> (Raster, BTreeMap<String, Raster>) {
    let ring = outline_ring(st);
    let (x0, x1) = ring.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
    let (y0, y1) = ring.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
    let mut board = Raster::new(x0, y0, x1, y1, res);
    board.polys(&[ring], true);
    let mut out = BTreeMap::new();
    for (name, plane, _) in layers {
        let mut r = Raster::new(x0, y0, x1, y1, res);
        if *plane { r.cells = board.cells.clone(); out.insert(name.clone(), r); continue; }
        {
            let keep = |n: &Value| match refs { None => true, Some(rs) => { let n = n.as_str().unwrap_or(""); n.is_empty() || rs.iter().any(|r| r.eq_ignore_ascii_case(n)) } };
            let on_layer = |l: &Value| l.as_str().map(|s| s == name || s == "*.Cu").unwrap_or(false);
            let on = |p: &Value| on_layer(&p["layer"]) && keep(&p["net"]);
            for t in st["tracks"].as_array().cloned().unwrap_or_default().iter().filter(|t| on(t)) {
                r.capsule(f(&t["start"]["x"]), f(&t["start"]["y"]), f(&t["end"]["x"]), f(&t["end"]["y"]), f(&t["width"]));
            }
            for a in st["arcs"].as_array().cloned().unwrap_or_default().iter().filter(|a| on(a)) {
                let (cx, cy, rad) = (f(&a["center"]["x"]), f(&a["center"]["y"]), f(&a["radius"]));
                let (s, mut e) = (f(&a["startAngle"]), f(&a["endAngle"]));
                if e <= s { e += 360.0; }
                let k = ((e - s) / 10.0).ceil().max(1.0) as usize;
                for q in 0..k {
                    let (t0, t1) = ((s + (e - s) * q as f64 / k as f64).to_radians(), (s + (e - s) * (q + 1) as f64 / k as f64).to_radians());
                    r.capsule(cx + rad * t0.cos(), cy + rad * t0.sin(), cx + rad * t1.cos(), cy + rad * t1.sin(), f(&a["width"]));
                }
            }
            for p in st["pads"].as_array().cloned().unwrap_or_default().iter().filter(|p| on(p)) {
                let (sx, sy) = (f(&p["sizeX"]), f(&p["sizeY"]));
                // a custom pad's real copper when the bridge sends it (Fusion's copperPoints); its bounding rectangle otherwise
                let outline: Vec<(f64, f64)> = p["copperPoints"].as_array().cloned().unwrap_or_default().iter().map(|q| (f(&q[0]), f(&q[1]))).collect();
                if outline.len() >= 3 { r.polys(&[outline], true); continue; }
                match p["shape"].as_str() {
                    Some("round") | Some("circle") if (sx - sy).abs() > 1e-6 => {
                        let (c, s) = (f(&p["rotation"]).to_radians().cos(), f(&p["rotation"]).to_radians().sin());
                        let (l, w) = ((sx - sy).abs() / 2.0, sx.min(sy));
                        let (ux, uy) = if sx > sy { (c * l, s * l) } else { (-s * l, c * l) };
                        r.capsule(f(&p["x"]) - ux, f(&p["y"]) - uy, f(&p["x"]) + ux, f(&p["y"]) + uy, w);
                    }
                    Some("round") | Some("circle") => r.capsule(f(&p["x"]), f(&p["y"]), f(&p["x"]), f(&p["y"]), sx),
                    _ => r.rect(f(&p["x"]), f(&p["y"]), sx, sy, f(&p["rotation"])),
                }
            }
            for v in st["vias"].as_array().cloned().unwrap_or_default().iter().filter(|v| keep(&v["net"])) {
                r.capsule(f(&v["position"]["x"]), f(&v["position"]["y"]), f(&v["position"]["x"]), f(&v["position"]["y"]), f(&v["size"]));
            }
        }
        for z in zones.iter().filter(|z| z["layer"].as_str() == Some(name)) {
            for fill in z["fills"].as_array().cloned().unwrap_or_default() {
                let pts = |v: &Value| v.as_array().cloned().unwrap_or_default().iter().map(|p| (f(&p[0]), f(&p[1]))).collect::<Vec<_>>();
                let mut rings = vec![pts(&fill["outline"])];
                rings.extend(fill["holes"].as_array().cloned().unwrap_or_default().iter().map(pts));
                r.polys(&rings, true);
            }
        }
        r.and(&board);
        out.insert(name.clone(), r);
    }
    (board, out)
}

/// Per layer: copper area and share of the board; per mirrored pair (1 with n, 2 with n-1...): the difference.
pub fn balance(board: &Raster, maps: &BTreeMap<String, Raster>, layers: &[(String, bool, String)]) -> (Vec<Value>, Vec<Value>) {
    let cell = board.res * board.res;
    let total = board.count() as f64;
    let pct = |name: &str| maps.get(name).map(|r| 100.0 * r.count() as f64 / total.max(1.0)).unwrap_or(0.0);
    let rows: Vec<Value> = layers.iter().map(|(n, plane, net)| {
        let mut o = json!({"layer": n, "copperMm2": r1(maps.get(n).map(|r| r.count()).unwrap_or(0) as f64 * cell), "percent": r1(pct(n))});
        if *plane { o["plane"] = json!(net); }
        o
    }).collect();
    let k = layers.len();
    let pairs = (0..k / 2).map(|i| {
        let (a, b) = (&layers[i].0, &layers[k - 1 - i].0);
        let d = (pct(a) - pct(b)).abs();
        json!({"layers": [a, b], "percent": [r1(pct(a)), r1(pct(b))], "differencePoints": r1(d), "ok": d <= 20.0})
    }).collect();
    (rows, pairs)
}

/// Per signal net: track length on each layer, and the length that runs over no reference copper on either
/// neighbouring layer, with where the first gap starts. `skip` nets are the references themselves. Pure.
pub fn return_paths(st: &Value, refs: &BTreeMap<String, Raster>, layers: &[(String, bool, String)], skip: &[String], step: f64) -> Vec<Value> {
    let idx: BTreeMap<&str, usize> = layers.iter().enumerate().map(|(i, l)| (l.0.as_str(), i)).collect();
    let mut acc: BTreeMap<(String, String), (f64, f64, Option<(f64, f64)>, Vec<(f64, f64)>)> = BTreeMap::new();
    for t in st["tracks"].as_array().cloned().unwrap_or_default() {
        let (net, layer) = (t["net"].as_str().unwrap_or(""), t["layer"].as_str().unwrap_or(""));
        if net.is_empty() || skip.iter().any(|s| s.eq_ignore_ascii_case(net)) { continue; }
        let Some(&i) = idx.get(layer) else { continue };
        let nb: Vec<&Raster> = [i.checked_sub(1), Some(i + 1)].iter().flatten().filter_map(|j| layers.get(*j)).filter_map(|l| refs.get(&l.0)).collect();
        let (ax, ay, bx, by) = (f(&t["start"]["x"]), f(&t["start"]["y"]), f(&t["end"]["x"]), f(&t["end"]["y"]));
        let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        let n = (len / step).ceil().max(1.0) as usize;
        let e = acc.entry((net.to_string(), layer.to_string())).or_insert((0.0, 0.0, None, Vec::new()));
        e.0 += len;
        for k in 0..n {
            let s = (k as f64 + 0.5) / n as f64;
            let (x, y) = (ax + s * (bx - ax), ay + s * (by - ay));
            if !nb.iter().any(|r| r.at(x, y)) {
                e.1 += len / n as f64;
                if e.2.is_none() { e.2 = Some((r3(x), r3(y))); }
                e.3.push((x, y));
            }
        }
    }
    let mut out: Vec<Value> = acc.into_iter().filter(|(_, v)| v.1 > 0.0).map(|((net, layer), (len, gap, at, pts))| json!({
        "net": net, "layer": layer, "lengthMm": r3(len), "unreferencedMm": r3(gap), "percent": r1(100.0 * gap / len.max(1e-9)), "firstGapAt": at.map(|p| json!([p.0, p.1])),
        "gapRuns": gap_runs(&pts, 1.0).iter().map(|r| json!([r3(r.0), r3(r.1), r3(r.2), r3(r.3)])).collect::<Vec<_>>()})).collect();
    out.sort_by(|a, b| b["unreferencedMm"].as_f64().partial_cmp(&a["unreferencedMm"].as_f64()).unwrap());
    out
}

/// The gap sample points grouped into runs: boxes (x0, y0, x1, y1) of points within `join` mm of each other. Pure.
pub fn gap_runs(pts: &[(f64, f64)], join: f64) -> Vec<(f64, f64, f64, f64)> {
    let mut runs: Vec<(f64, f64, f64, f64)> = Vec::new();
    for &(x, y) in pts {
        match runs.iter_mut().find(|r| x >= r.0 - join && x <= r.2 + join && y >= r.1 - join && y <= r.3 + join) {
            Some(r) => { r.0 = r.0.min(x); r.1 = r.1.min(y); r.2 = r.2.max(x); r.3 = r.3.max(y); }
            None => runs.push((x, y, x, y)),
        }
    }
    // runs that grew into each other merge
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..runs.len() {
            for j in i + 1..runs.len() {
                let (a, b) = (runs[i], runs[j]);
                if a.0 <= b.2 + join && b.0 <= a.2 + join && a.1 <= b.3 + join && b.1 <= a.3 + join {
                    runs[i] = (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3));
                    runs.remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    runs
}

/// A plan that closes each gap: a pour patch `margin` mm round every gap run, on the neighbouring layer that already
/// carries the most reference copper, on that layer's reference net; as apply_pours arguments. Pure.
pub fn fix_plan(rows: &[Value], layers: &[(String, bool, String)], refs: &BTreeMap<String, Raster>, zones: &[Value], skip: &[String], board: (f64, f64, f64, f64), margin: f64) -> Vec<Value> {
    let idx: BTreeMap<&str, usize> = layers.iter().enumerate().map(|(i, l)| (l.0.as_str(), i)).collect();
    let mut pours = Vec::new();
    for row in rows {
        let Some(&i) = row["layer"].as_str().and_then(|l| idx.get(l)) else { continue };
        // a plane needs no patch (it is copper everywhere but its splits); pick the signal neighbour with most copper
        let best = [i.checked_sub(1), Some(i + 1)].iter().flatten().filter_map(|j| layers.get(*j)).filter(|l| !l.1)
            .max_by_key(|l| refs.get(&l.0).map(|r| r.count()).unwrap_or(0));
        let Some(nb) = best else { continue };
        let net = zones.iter().find(|z| z["layer"].as_str() == Some(nb.0.as_str())).and_then(|z| z["net"].as_str()).map(String::from)
            .or_else(|| skip.first().cloned());
        let Some(net) = net else { continue };
        for (k, r) in row["gapRuns"].as_array().cloned().unwrap_or_default().iter().enumerate() {
            let (x0, y0) = ((f(&r[0]) - margin).max(board.0 + 0.3), (f(&r[1]) - margin).max(board.1 + 0.3));
            let (x1, y1) = ((f(&r[2]) + margin).min(board.2 - 0.3), (f(&r[3]) + margin).min(board.3 - 0.3));
            if x1 <= x0 || y1 <= y0 { continue; }
            // does the patch overlap the reference copper already on that layer? (one that does not floats)
            let joins = refs.get(&nb.0).map(|ras| {
                let mut hit = false;
                let mut y = y0;
                while y <= y1 && !hit { let mut x = x0; while x <= x1 && !hit { hit = ras.at(x, y); x += ras.res; } y += ras.res; }
                hit
            }).unwrap_or(false);
            // removeDead off: a patch that only overlaps the pour it extends touches no pad or track of its net, and
            // Altium's dead-copper removal poured it as nothing (0 mm2, measured 2026-10-26)
            pours.push(json!({"net": net, "layer": nb.0, "name": format!("Return {} {}", row["net"].as_str().unwrap_or(""), k + 1), "removeDead": false,
                "points": [[r3(x0), r3(y0)], [r3(x1), r3(y0)], [r3(x1), r3(y1)], [r3(x0), r3(y1)]], "closes": {"net": row["net"], "layer": row["layer"]}, "joinsExistingCopper": joins}));
        }
    }
    pours
}

/// The board, its poured copper and its stack from the input bundle, or the refusal to answer.
fn gather(input: &Value) -> Result<(Value, Vec<Value>, Vec<(String, bool, String)>), Value> {
    let st = crate::input::board(input)?;
    if st["outline"].as_array().map(|a| a.len() < 3).unwrap_or(true) { return Err(crate::input::refuse("no_outline", "the board has no outline")); }
    let zones = crate::input::pours(input);
    let layers = crate::input::stack(input).map(|s| stack_layers(&s)).unwrap_or_default();
    if layers.is_empty() { return Err(crate::input::refuse("no_stack", "the input has no stack: pass the layer_stack companion as stack")); }
    Ok((st, zones, layers))
}

fn res_of(args: &Value) -> f64 { args.get("resolutionMm").and_then(|v| v.as_f64()).unwrap_or(0.1).clamp(0.02, 1.0) }

/// copper_balance: input {board, stack, pours, args: {resolutionMm?}}
pub fn copper_balance(input: &Value) -> Value {
    let (st, zones, layers) = match gather(input) { Ok(g) => g, Err(e) => return e };
    let args = &input["args"];
    let res = res_of(args);
    let (board, maps) = rasters(&st, &zones, &layers, res, None);
    let (rows, pairs) = balance(&board, &maps, &layers);
    let unpoured: Vec<Value> = st["polygons"].as_array().cloned().unwrap_or_default().iter()
        .filter(|g| !zones.iter().any(|z| z["layer"] == g["layer"] && z["net"] == g["net"] && z["filled"] == json!(true)))
        .map(|g| json!({"net": g["net"], "layer": g["layer"]})).collect();
    let ok = pairs.iter().all(|p| p["ok"] == json!(true));
    json!({"success": true, "boardMm2": r1(board.count() as f64 * res * res), "resolutionMm": res, "layers": rows, "pairs": pairs, "balanced": ok,
        "poursWithoutRegions": unpoured,
        "_hint": if ok { "Mirrored layers carry similar copper: the stack should stay flat through reflow." } else { "A mirrored pair differs by more than 20 points of copper: the board can bow or twist through reflow. Pour the lighter layer (apply_pours, a GND pour or copper thieving), or ask the fab to add thieving on its side." }})
}

/// return_path: input {board, stack, pours, args: {nets?, referenceNets?, resolutionMm?, fixMarginMm?}}
pub fn return_path(input: &Value) -> Value {
    let (st, zones, layers) = match gather(input) { Ok(g) => g, Err(e) => return e };
    let args = &input["args"];
    let res = res_of(args);
    // the reference nets: the ones that own pours or planes, unless named
    let mut skip: Vec<String> = args.get("referenceNets").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    if skip.is_empty() {
        skip = st["polygons"].as_array().cloned().unwrap_or_default().iter().filter_map(|g| g["net"].as_str().map(String::from))
            .chain(zones.iter().filter_map(|z| z["net"].as_str().map(String::from))).chain(layers.iter().filter(|l| l.1).map(|l| l.2.clone()))
            .filter(|n| !n.is_empty()).collect();
        skip.sort(); skip.dedup();
    }
    let (_, refs) = rasters(&st, &zones, &layers, res, Some(&skip));
    let mut rows = return_paths(&st, &refs, &layers, &skip, res);
    if let Some(want) = args.get("nets").and_then(|v| v.as_array()) {
        let w: Vec<String> = want.iter().filter_map(|v| v.as_str()).map(|s| s.to_ascii_lowercase()).collect();
        rows.retain(|r| r["net"].as_str().map(|n| w.contains(&n.to_ascii_lowercase())).unwrap_or(false));
    }
    let no_ref: Vec<&String> = layers.iter().map(|l| &l.0).filter(|n| refs.get(*n).map(|r| r.count() == 0).unwrap_or(true)).collect();
    // a plan to close the gaps, for the caller to read and run: nothing is changed here
    let bb = crate::geom::outline_bbox(&st).unwrap_or((0.0, 0.0, 0.0, 0.0));
    let margin = args.get("fixMarginMm").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let patches = fix_plan(&rows, &layers, &refs, &zones, &skip, bb, margin);
    let fix = if patches.is_empty() { Value::Null } else {
        let pours: Vec<Value> = patches.iter().map(|p| { let mut q = p.clone(); if let Some(o) = q.as_object_mut() { o.remove("closes"); o.remove("joinsExistingCopper"); } q }).collect();
        let floating: Vec<Value> = patches.iter().filter(|p| p["joinsExistingCopper"] == json!(false)).map(|p| json!({"name": p["name"], "layer": p["layer"]})).collect();
        json!({"verb": "apply_pours", "args": {"pours": pours}, "patches": patches.len(), "closes": patches.iter().map(|p| p["closes"].clone()).collect::<Vec<_>>(),
               "floating": floating,
               "note": "A plan only: nothing changed. Each patch is a pour of the reference net round one gap run, on the neighbouring layer with the most reference copper. Read it (a patch can cut through other routing's space: the tool pours round it, keeping clearance), then run the verb with args, repour, and run return_path again. floating patches overlap no copper of their net yet: tie each to it with a via (place_vias) or it is an island."})
    };
    let note = if skip.is_empty() { json!("No net owns a pour or a plane on this board: there is no reference copper, so every signal is reported. Pour GND (apply_pours) or name referenceNets.") } else { Value::Null };
    json!({"success": true, "referenceNets": skip, "note": note, "layersWithNoReferenceCopper": no_ref, "nets": rows, "ok": rows.is_empty(), "resolutionMm": res, "fix": fix,
        "_hint": if rows.is_empty() { "Every signal track runs over pour or plane copper on a neighbouring layer." } else { "Each row is a net whose tracks on that layer run over no pour or plane on either neighbouring layer for unreferencedMm (from firstGapAt). Its return current detours around the gap: more loop area, more emission and crosstalk. Close the gap (extend the pour: apply_pours), route the net over solid copper, or add stitching capacitors or vias where it changes reference." }})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_shapes_have_their_area() {
        let mut r = Raster::new(0.0, 0.0, 20.0, 20.0, 0.05);
        r.rect(5.0, 5.0, 4.0, 2.0, 0.0);
        assert!((r.count() as f64 * 0.0025 - 8.0).abs() < 0.3);
        let mut c = Raster::new(0.0, 0.0, 20.0, 20.0, 0.05);
        c.capsule(10.0, 10.0, 10.0, 10.0, 4.0);
        assert!((c.count() as f64 * 0.0025 - std::f64::consts::PI * 4.0).abs() < 0.3);
        let mut p = Raster::new(0.0, 0.0, 20.0, 20.0, 0.05);
        p.polys(&[vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], vec![(2.0, 2.0), (4.0, 2.0), (4.0, 4.0), (2.0, 4.0)]], true);
        assert!((p.count() as f64 * 0.0025 - 96.0).abs() < 0.5);
        assert!(!p.at(3.0, 3.0) && p.at(6.0, 6.0));
        // KiCad's fractured fill: the hole joined to the outline by a zero-width cut, holes empty. Same copper.
        let mut k = Raster::new(0.0, 0.0, 20.0, 20.0, 0.05);
        k.polys(&[vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 2.0), (2.0, 2.0), (2.0, 4.0), (4.0, 4.0), (4.0, 2.0), (2.0, 2.0), (0.0, 2.0)]], true);
        assert_eq!(k.count(), p.count());
        assert!(!k.at(3.0, 3.0) && k.at(6.0, 6.0));
    }

    #[test]
    fn a_custom_pad_is_its_copper_points_when_sent() {
        let pad = |extra: Value| { let mut p = json!({"ref": "Q1", "name": "5", "x": 5.0, "y": 5.0, "sizeX": 4.0, "sizeY": 4.0, "rotation": 0, "layer": "F.Cu", "net": "D", "shape": "rect"});
            if let Value::Object(m) = extra { for (k, v) in m { p[k] = v; } } p };
        let area = |p: Value| { let st = json!({"outline": [[0, 0], [10, 0], [10, 10], [0, 10]], "pads": [p]});
            let (_, m) = rasters(&st, &[], &[("F.Cu".into(), false, "".into())], 0.05, None); m["F.Cu"].count() as f64 * 0.0025 };
        assert!((area(pad(json!({}))) - 16.0).abs() < 0.5);
        // an L of copper inside that rectangle: 4x2 + 2x2 = 12 mm2
        assert!((area(pad(json!({"copperPoints": [[3, 3], [7, 3], [7, 5], [5, 5], [5, 7], [3, 7]]}))) - 12.0).abs() < 0.5);
    }

    fn st() -> Value {
        json!({"outline": [[0, 0], [20, 0], [20, 10], [0, 10]],
               "outlineEdges": [{"kind": "line", "start": [0.0, 0.0]}, {"kind": "line", "start": [20.0, 0.0]}, {"kind": "line", "start": [20.0, 10.0]}, {"kind": "line", "start": [0.0, 10.0]}],
               "tracks": [{"net": "SIG", "layer": "F.Cu", "width": 0.2, "start": {"x": 1.0, "y": 5.0}, "end": {"x": 19.0, "y": 5.0}}]})
    }

    #[test]
    fn balance_and_return_path_on_a_split_ground() {
        let layers = vec![("F.Cu".to_string(), false, String::new()), ("B.Cu".to_string(), false, String::new())];
        // GND poured on B.Cu over x 0..8 and 12..20: a 4 mm split under the signal
        let zones = vec![json!({"layer": "B.Cu", "net": "GND", "filled": true, "fills": [
            {"outline": [[0, 0], [8, 0], [8, 10], [0, 10]], "holes": []}, {"outline": [[12, 0], [20, 0], [20, 10], [12, 10]], "holes": []}]})];
        let (board, maps) = rasters(&st(), &zones, &layers, 0.1, None);
        let (rows, pairs) = balance(&board, &maps, &layers);
        assert!((rows[1]["percent"].as_f64().unwrap() - 80.0).abs() < 1.0);
        assert!((rows[0]["percent"].as_f64().unwrap() - 1.8).abs() < 0.5);
        assert_eq!(pairs[0]["ok"], json!(false));
        let (_, refs) = rasters(&st(), &zones, &layers, 0.1, Some(&["GND".to_string()]));
        // the signal itself is not reference copper
        assert!(!refs["F.Cu"].at(10.0, 5.0));
        let rp = return_paths(&st(), &refs, &layers, &["GND".to_string()], 0.1);
        assert_eq!(rp.len(), 1);
        assert!((rp[0]["unreferencedMm"].as_f64().unwrap() - 4.0).abs() < 0.2);
        assert_eq!(rp[0]["firstGapAt"][0].as_f64().unwrap().floor(), 8.0);
    }

    #[test]
    fn stack_names_match_routing_state() {
        let s = json!({"layers": [{"name": "Top"}, {"kind": "plane", "plane": 1, "net": "GND"}, {"kind": "signal", "mid": 1}, {"name": "Bottom"}]});
        let l = stack_layers(&s);
        assert_eq!(l.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), vec!["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"]);
        assert!(l[1].1 && l[1].2 == "GND");
        // the bridge's own name wins when it gives one
        let named = json!({"layers": [{"layer": "F.Cu"}, {"layer": "In1.Cu", "kind": "plane", "net": "GND", "layerId": 39}, {"layer": "B.Cu"}]});
        assert_eq!(stack_layers(&named)[1], ("In1.Cu".to_string(), true, "GND".to_string()));
    }
    #[test]
    fn gap_runs_group_and_the_fix_patches_the_neighbour() {
        let pts: Vec<(f64, f64)> = (0..40).map(|k| (8.05 + k as f64 * 0.1, 5.0)).chain((0..5).map(|k| (15.0 + k as f64 * 0.1, 5.0))).collect();
        let runs = gap_runs(&pts, 1.0);
        assert_eq!(runs.len(), 2);
        let layers = vec![("F.Cu".to_string(), false, String::new()), ("B.Cu".to_string(), false, String::new())];
        let zones = vec![json!({"layer": "B.Cu", "net": "GND", "filled": true, "fills": [{"outline": [[0, 0], [8, 0], [8, 10], [0, 10]], "holes": []}]})];
        let (_, refs) = rasters(&st(), &zones, &layers, 0.1, Some(&["GND".to_string()]));
        let rows = vec![json!({"net": "SIG", "layer": "F.Cu", "gapRuns": [[8.05, 5.0, 11.95, 5.0]]})];
        let p = fix_plan(&rows, &layers, &refs, &zones, &["GND".to_string()], (0.0, 0.0, 20.0, 10.0), 1.0);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["layer"], "B.Cu");
        assert_eq!(p[0]["net"], "GND");
        assert_eq!(p[0]["points"][0], json!([7.05, 4.0]));
        assert_eq!(p[0]["joinsExistingCopper"], json!(true));
        assert_eq!(p[0]["removeDead"], json!(false));
    }
}

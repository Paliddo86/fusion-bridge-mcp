//! Via plans computed from the board model: thermal vias in a part's exposed pad and a via fence along RF tracks or the
//! board edge (and the stitching grid behind board_features). From the Altium bridge's features.rs. Plans only: each
//! answers the vias as place_vias arguments, and the bridge places them.
use serde_json::{json, Value};

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn pt(v: &Value) -> (f64, f64) { (f(&v[0]), f(&v[1])) }

fn inside(poly: &[(f64, f64)], x: f64, y: f64) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 { c = !c; }
    }
    c
}

fn seg_dist(px: f64, py: f64, a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 { 0.0 } else { (((px - a.0) * dx + (py - a.1) * dy) / l2).clamp(0.0, 1.0) };
    ((px - a.0 - t * dx).powi(2) + (py - a.1 - t * dy).powi(2)).sqrt()
}

/// Copper that a stitching via on `net` must keep clear of: (kind, geometry, half-width) as segments and discs.
enum Ob { Seg((f64, f64), (f64, f64), f64), Disc(f64, f64, f64) }

fn obstacles(state: &Value, net: &str) -> (Vec<Ob>, Vec<(f64, f64)>) {
    // copper on another net. Lines on mechanical and overlay layers are not copper, and net-less copper tracks are a
    // hatched pour's pieces (Altium lists them with no net) that re-pour round a new via
    let other = |v: &Value| {
        let n = v["net"].as_str().unwrap_or("");
        let copper = v.get("layer").map(|l| l.as_str().map(|s| s.ends_with(".Cu")).unwrap_or(false)).unwrap_or(true);
        let piece = n.is_empty() && v.get("start").or(v.get("center")).is_some();
        copper && !piece && !n.eq_ignore_ascii_case(net)
    };
    let mut ob = Vec::new();
    let mut same_vias = Vec::new();
    for t in state["tracks"].as_array().cloned().unwrap_or_default() {
        if other(&t) { ob.push(Ob::Seg((f(&t["start"]["x"]), f(&t["start"]["y"])), (f(&t["end"]["x"]), f(&t["end"]["y"])), f(&t["width"]) / 2.0)); }
    }
    for a in state["arcs"].as_array().cloned().unwrap_or_default() {
        if !other(&a) { continue; }
        // an arc as short chords
        let (cx, cy, r) = (f(&a["center"]["x"]), f(&a["center"]["y"]), f(&a["radius"]));
        let (mut a1, a2) = (f(&a["startAngle"]), f(&a["endAngle"]));
        let mut span = a2 - a1;
        if span <= 0.0 { span += 360.0; }
        let n = ((span / 10.0).ceil() as usize).max(1);
        let p = |deg: f64| (cx + r * deg.to_radians().cos(), cy + r * deg.to_radians().sin());
        for _ in 0..n { let b = a1 + span / n as f64; ob.push(Ob::Seg(p(a1), p(b), f(&a["width"]) / 2.0)); a1 = b; }
    }
    for v in state["vias"].as_array().cloned().unwrap_or_default() {
        let (x, y) = (f(&v["position"]["x"]), f(&v["position"]["y"]));
        if other(&v) { ob.push(Ob::Disc(x, y, f(&v["size"]) / 2.0)); } else { same_vias.push((x, y)); }
    }
    for p in state["pads"].as_array().cloned().unwrap_or_default() {
        // a pad on the net still gets clearance: a via in a pad is not stitching. Its circumscribed circle keeps it
        // conservative at any rotation.
        let r = (f(&p["sizeX"]).powi(2) + f(&p["sizeY"]).powi(2)).sqrt() / 2.0;
        ob.push(Ob::Disc(f(&p["x"]), f(&p["y"]), r.max(f(&p["drill"]) / 2.0)));
    }
    (ob, same_vias)
}

/// Where stitching vias for `net` go: a grid at `pitch` inside the outline (or `region`), `edge` mm clear of the
/// board edge, `clearance` mm clear of copper on other nets and of every pad, half a pitch from vias the net already
/// has, at most `max`. Pure, for the tests.
#[allow(clippy::too_many_arguments)]
pub fn plan_stitching(state: &Value, net: &str, pitch: f64, size: f64, clearance: f64, edge: f64, region: Option<&[(f64, f64)]>, max: usize) -> Vec<(f64, f64)> {
    plan_stitching_with(state, net, pitch, size, clearance, edge, region, max, &[], true, &[])
}

/// plan_stitching, also clear of `extra` discs (x, y, radius: holes and fiducials placed in the same call) and, with
/// `avoid_parts`, out from under every part (the box around its pads, grown by the clearance).
#[allow(clippy::too_many_arguments)]
pub fn plan_stitching_with(state: &Value, net: &str, pitch: f64, size: f64, clearance: f64, edge: f64, region: Option<&[(f64, f64)]>, max: usize,
                           extra: &[(f64, f64, f64)], avoid_parts: bool, keepouts: &[(f64, f64, f64, f64)]) -> Vec<(f64, f64)> {
    let outline: Vec<(f64, f64)> = state["outline"].as_array().cloned().unwrap_or_default().iter().map(pt).collect();
    if outline.len() < 3 || pitch <= 0.0 { return vec![]; }
    let area: Vec<(f64, f64)> = region.map(|r| r.to_vec()).unwrap_or_else(|| outline.clone());
    let (mut ob, same) = obstacles(state, net);
    for (x, y, r) in extra { ob.push(Ob::Disc(*x, *y, *r)); }
    // each part's pad box: a via between a chip's pad rows sits under its body
    let mut boxes: std::collections::BTreeMap<String, (f64, f64, f64, f64)> = Default::default();
    if avoid_parts {
        for p in state["pads"].as_array().cloned().unwrap_or_default() {
            let Some(r) = p["ref"].as_str().filter(|r| !r.is_empty()) else { continue };
            let h = (f(&p["sizeX"]).powi(2) + f(&p["sizeY"]).powi(2)).sqrt() / 2.0;
            let (x, y) = (f(&p["x"]), f(&p["y"]));
            let b = boxes.entry(r.to_string()).or_insert((f64::MAX, f64::MAX, f64::MIN, f64::MIN));
            *b = (b.0.min(x - h), b.1.min(y - h), b.2.max(x + h), b.3.max(y + h));
        }
    }
    let (x0, x1) = area.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
    let (y0, y1) = area.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
    let mut cands = Vec::new();
    let mut y = y0 + pitch / 2.0;
    while y < y1 {
        let mut x = x0 + pitch / 2.0;
        while x < x1 { if inside(&area, x, y) { cands.push((x, y)); } x += pitch; }
        y += pitch;
    }
    keep_clear(state, &outline, &cands, pitch / 2.0, size, clearance, edge, max, &ob, &same, &boxes, keepouts)
}

/// The candidate via points that fit: inside the outline, `edge` from it, `clearance` from other copper (`ob`), out of
/// part boxes and keepouts, and `sep` from the net's vias and from each other. Pure.
#[allow(clippy::too_many_arguments)]
fn keep_clear(_state: &Value, outline: &[(f64, f64)], cands: &[(f64, f64)], sep: f64, size: f64, clearance: f64, edge: f64, max: usize, ob: &[Ob],
              same: &[(f64, f64)], boxes: &std::collections::BTreeMap<String, (f64, f64, f64, f64)>, keepouts: &[(f64, f64, f64, f64)]) -> Vec<(f64, f64)> {
    let r = size / 2.0;
    let mut out: Vec<(f64, f64)> = Vec::new();
    for &(x, y) in cands {
        if out.len() >= max { break; }
        let ok = inside(outline, x, y)
            && (0..outline.len()).all(|k| seg_dist(x, y, outline[k], outline[(k + 1) % outline.len()]) >= edge + r)
            && ob.iter().all(|o| match o {
                Ob::Seg(a, b, hw) => seg_dist(x, y, *a, *b) - hw - r >= clearance,
                Ob::Disc(cx, cy, cr) => ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - cr - r >= clearance,
            })
            && same.iter().chain(out.iter()).all(|(sx, sy)| ((x - sx).powi(2) + (y - sy).powi(2)).sqrt() >= sep)
            && boxes.values().all(|b| x + r + clearance <= b.0 || x - r - clearance >= b.2 || y + r + clearance <= b.1 || y - r - clearance >= b.3)
            && keepouts.iter().all(|k| x + r <= k.0 || x - r >= k.2 || y + r <= k.1 || y - r >= k.3);
        if ok { out.push(((x * 1000.0).round() / 1000.0, (y * 1000.0).round() / 1000.0)); }
    }
    out
}

/// Via fence candidates: both sides of every copper track of `nets`, `offset` from its centreline, every `pitch` along
/// it; and with `edge_inset`, along the outline that far inside it. Pure.
pub fn fence_points(state: &Value, nets: &[String], pitch: f64, offset: f64, edge_inset: Option<f64>) -> Vec<(f64, f64)> {
    let mut v = Vec::new();
    for t in state["tracks"].as_array().cloned().unwrap_or_default() {
        let n = t["net"].as_str().unwrap_or("");
        if !nets.iter().any(|x| x.eq_ignore_ascii_case(n)) || !t["layer"].as_str().map(|l| l.ends_with(".Cu")).unwrap_or(false) { continue; }
        let (a, b) = ((f(&t["start"]["x"]), f(&t["start"]["y"])), (f(&t["end"]["x"]), f(&t["end"]["y"])));
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        if len <= 0.0 { continue; }
        let (ux, uy) = ((b.0 - a.0) / len, (b.1 - a.1) / len);
        let k = (len / pitch).floor() as usize;
        for i in 0..=k {
            let s = if k == 0 { len / 2.0 } else { i as f64 * len / k as f64 };
            for side in [1.0, -1.0] { v.push((a.0 + ux * s - uy * offset * side, a.1 + uy * s + ux * offset * side)); }
        }
    }
    if let Some(inset) = edge_inset {
        let ring = crate::coppermap::outline_ring(state);
        let n = ring.len();
        // inward is left of each edge on a counterclockwise ring
        let area: f64 = (0..n).map(|k| ring[k].0 * ring[(k + 1) % n].1 - ring[(k + 1) % n].0 * ring[k].1).sum();
        let sgn = if area > 0.0 { 1.0 } else { -1.0 };
        let mut carry: f64 = 0.0;
        for k in 0..n {
            let (a, b) = (ring[k], ring[(k + 1) % n]);
            let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
            if len <= 0.0 { continue; }
            let (ux, uy) = ((b.0 - a.0) / len, (b.1 - a.1) / len);
            // clear of the corners by the inset, where an inward step from one edge lands on the next
            let mut s = carry.max(inset);
            while s <= len - inset { v.push((a.0 + ux * s - uy * inset * sgn, a.1 + uy * s + ux * inset * sgn)); s += pitch; }
            carry = (s - len).max(0.0);
        }
    }
    v
}

/// The board's preferred via (diameter, drill) in mm. A standard rule gives it as `{kind: "via", sizeMm, drillMm}`; Altium's
/// Routing Via descriptor is read too, e.g. "Routing Via (MinHoleWidth=0.3mm) ... (PreferredHoleWidth=0.3mm) ...
/// (PreferedWidth=0.5mm) (All)" (Altium's own spelling). Pure.
pub fn via_style(rules: &[Value]) -> Option<(f64, f64)> {
    if let Some(r) = rules.iter().filter(|r| r["enabled"] != json!(false))
        .find(|r| r["kind"].as_str().map(|k| k.eq_ignore_ascii_case("via")).unwrap_or(false) && r["sizeMm"].as_f64().is_some() && r["drillMm"].as_f64().is_some()) {
        return Some((f(&r["sizeMm"]), f(&r["drillMm"])));
    }
    let num = |d: &str, key: &str| -> Option<f64> {
        let i = d.find(&format!("({key}="))? + key.len() + 2;
        let s = &d[i..d[i..].find(')')? + i];
        let (v, unit) = s.trim_end_matches(char::is_alphabetic).parse::<f64>().ok().map(|v| (v, &s[s.trim_end_matches(char::is_alphabetic).len()..]))?;
        Some(if unit.eq_ignore_ascii_case("mil") { v * 0.0254 } else { v })
    };
    let mut rs: Vec<&Value> = rules.iter().filter(|r| r["enabled"] != json!(false) && r["descriptor"].as_str().map(|d| d.starts_with("Routing Via")).unwrap_or(false)).collect();
    // the board-wide rule (scope All), highest priority first
    rs.sort_by_key(|r| (r["scope1"].as_str() != Some("All"), r["priority"].as_i64().unwrap_or(99)));
    let d = rs.first()?["descriptor"].as_str()?;
    Some((num(d, "PreferedWidth").or(num(d, "PreferredWidth"))?, num(d, "PreferredHoleWidth")?))
}


/// The board's preferred track width in mm. A standard rule gives it as `{kind: "width", preferredMm}` (or `widthMm`);
/// Altium's Width rule descriptor is read too, e.g. "Width Constraint (Min=10mil) (Max=10mil) (Preferred=10mil) (All)":
/// the board-wide one (scope All), highest priority first, its Preferred else its Min. Pure.
pub fn width_style(rules: &[Value]) -> Option<f64> {
    if let Some(r) = rules.iter().filter(|r| r["enabled"] != json!(false))
        .find(|r| r["kind"].as_str().map(|k| k.eq_ignore_ascii_case("width")).unwrap_or(false)) {
        // its preferred width, else its minimum, as Altium's descriptor is read (Fusion sends EAGLE's msWidth as minMm)
        if let Some(v) = r["preferredMm"].as_f64().or_else(|| r["widthMm"].as_f64()).or_else(|| r["minMm"].as_f64()) { return Some(v); }
    }
    let num = |d: &str, key: &str| -> Option<f64> {
        let i = d.find(&format!("({key}="))? + key.len() + 2;
        let s = &d[i..d[i..].find(')')? + i];
        let v: f64 = s.trim_end_matches(char::is_alphabetic).parse().ok()?;
        Some(if s.ends_with("mil") { v * 0.0254 } else { v })
    };
    let mut rs: Vec<&Value> = rules.iter().filter(|r| r["enabled"] != json!(false) && r["descriptor"].as_str().map(|d| d.starts_with("Width")).unwrap_or(false)).collect();
    rs.sort_by_key(|r| (r["scope1"].as_str() != Some("All"), r["priority"].as_i64().unwrap_or(99)));
    let d = rs.first()?["descriptor"].as_str()?;
    num(d, "Preferred").or_else(|| num(d, "Min")).map(|v| (v * 10000.0).round() / 10000.0)
}

/// Thermal via grid points inside a pad (x, y, sx, sy, rotation mm): `pitch` apart, `inset` from the pad's edge plus
/// the via's radius, centred. Pure.
pub fn thermal_grid(x: f64, y: f64, sx: f64, sy: f64, rot: f64, pitch: f64, size: f64, inset: f64) -> Vec<(f64, f64)> {
    let (ux, uy) = (sx / 2.0 - inset - size / 2.0, sy / 2.0 - inset - size / 2.0);
    if ux < 0.0 || uy < 0.0 { return vec![]; }
    let (nx, ny) = ((2.0 * ux / pitch).floor() as i64 + 1, (2.0 * uy / pitch).floor() as i64 + 1);
    let (c, s) = (rot.to_radians().cos(), rot.to_radians().sin());
    let mut out = Vec::new();
    for i in 0..nx { for j in 0..ny {
        let (lx, ly) = ((i as f64 - (nx - 1) as f64 / 2.0) * pitch, (j as f64 - (ny - 1) as f64 / 2.0) * pitch);
        out.push((((x + lx * c - ly * s) * 1000.0).round() / 1000.0, ((y + lx * s + ly * c) * 1000.0).round() / 1000.0));
    } }
    out
}

/// The via each plan uses: `size`/`drill` from args, else the board's via rule (`rules`), else 0.6/0.3; with where it came from.
fn via_of(input: &Value) -> (f64, f64, &'static str) {
    let args = &input["args"];
    let style = via_style(&crate::input::rules(input));
    let from = if args.get("size").is_some() { "size/drill" } else if style.is_some() { "the board's via rule" } else { "default 0.6/0.3" };
    (num(args, "size", style.map(|s| s.0).unwrap_or(0.6)), num(args, "drill", style.map(|s| s.1).unwrap_or(0.3)), from)
}

/// The plan as place_vias arguments.
fn apply_of(pts: &[(f64, f64)], net: &str, size: f64, drill: f64) -> Value {
    json!({"verb": "place_vias", "args": {"vias": pts.iter().map(|(x, y)| json!({"net": net, "x": x, "y": y, "size": size, "drill": drill})).collect::<Vec<_>>()}})
}

/// thermal_vias: input {board, rules?, args: {ref, pad?, pitchMm?, size?, drill?, insetMm?}}. A grid of vias inside the part's
/// exposed pad (the one named, else its largest), clear of vias already there. Plans only: `apply` is place_vias' arguments.
pub fn thermal_vias(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let Some(rf) = args["ref"].as_str() else { return crate::input::refuse("bad_args", "ref names the part (e.g. U3)") };
    let pads: Vec<Value> = st["pads"].as_array().cloned().unwrap_or_default().into_iter().filter(|p| p["ref"].as_str().map(|r| r.eq_ignore_ascii_case(rf)).unwrap_or(false)).collect();
    if pads.is_empty() { return crate::input::refuse("part_not_found", &format!("{rf} has no pads on this board")); }
    let pad = match args["pad"].as_str() {
        Some(n) => pads.iter().find(|p| p["name"].as_str().map(|x| x.eq_ignore_ascii_case(n)).unwrap_or(false)).cloned(),
        None => pads.iter().max_by(|a, b| (f(&a["sizeX"]) * f(&a["sizeY"])).partial_cmp(&(f(&b["sizeX"]) * f(&b["sizeY"]))).unwrap()).cloned(),
    };
    let Some(pad) = pad else { return json!({"success": false, "errorCode": "no_such_pad", "error": "that pad is not on the part", "pads": pads.iter().map(|p| p["name"].clone()).collect::<Vec<_>>()}) };
    let net = pad["net"].as_str().unwrap_or("").to_string();
    if net.is_empty() { return crate::input::refuse("pad_has_no_net", &format!("{rf}.{} is on no net: thermal vias would float", pad["name"].as_str().unwrap_or(""))); }
    let (size, drill, from) = via_of(input);
    let pitch = num(args, "pitchMm", (size + 0.5).max(1.0));
    let inset = num(args, "insetMm", 0.1);
    let grid = thermal_grid(f(&pad["x"]), f(&pad["y"]), f(&pad["sizeX"]), f(&pad["sizeY"]), f(&pad["rotation"]), pitch, size, inset);
    // keep clear of vias already there: the Bluetooth Sentinel's U4 pad had 8, and a grid laid over them broke Altium's
    // hole-to-hole rule 8 times (2026-10-26)
    let vias: Vec<(f64, f64, f64)> = st["vias"].as_array().cloned().unwrap_or_default().iter().map(|v| (f(&v["position"]["x"]), f(&v["position"]["y"]), f(&v["size"]))).collect();
    let half = f(&pad["sizeX"]).max(f(&pad["sizeY"])) / 2.0;
    let existing = vias.iter().filter(|(x, y, _)| (x - f(&pad["x"])).abs() <= half && (y - f(&pad["y"])).abs() <= half).count();
    let pts: Vec<(f64, f64)> = grid.iter().cloned().filter(|(x, y)| vias.iter().all(|(vx, vy, vs)| ((x - vx).powi(2) + (y - vy).powi(2)).sqrt() >= size / 2.0 + vs / 2.0 + 0.2)).collect();
    let planned = json!({"part": rf, "pad": pad["name"], "net": net, "padMm": [pad["sizeX"], pad["sizeY"]], "vias": pts.len(), "existingVias": existing, "skippedNearExisting": grid.len() - pts.len(), "pitchMm": pitch,
                         "via": {"sizeMm": size, "drillMm": drill, "from": from}, "points": pts.iter().map(|(x, y)| json!([x, y])).collect::<Vec<_>>()});
    if pts.is_empty() {
        return json!({"success": existing > 0, "errorCode": if existing > 0 { "already_has_vias" } else { "pad_too_small" }, "error": if existing > 0 { "the pad's existing vias leave no room for more at this pitch" } else { "no via fits inside the pad at this size and inset" }, "planned": planned});
    }
    json!({"success": true, "planned": planned, "apply": apply_of(&pts, &net, size, drill),
        "_hint": "A plan only: run place_vias with apply.args. Vias inside the pad carry its heat to the other layers' copper: pour the net on the far side (apply_pours) so they have somewhere to go. Open vias in a pad wick solder: ask the fab to tent or plug them, or use 0.3 mm or smaller drills. run_drc afterwards."})
}

/// via_fence: input {board, rules?, args: {viaNet, nets?, edge?, pitchMm?, freqGHz?, offsetMm?, edgeInsetMm?, size?, drill?,
/// clearance?}}. Vias of viaNet both sides of the named nets' tracks and/or round the board edge, clear of other copper.
/// Plans only: `apply` is place_vias' arguments.
pub fn via_fence(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let Some(via_net) = args["viaNet"].as_str() else { return crate::input::refuse("bad_args", "viaNet is the net the fence ties to, usually GND") };
    if !st["nets"].as_array().map(|a| a.iter().any(|n| n.as_str().map(|s| s.eq_ignore_ascii_case(via_net)).unwrap_or(false))).unwrap_or(false) {
        return json!({"success": false, "errorCode": "no_such_net", "error": format!("{via_net} is not a net on this board"), "nets": st["nets"]});
    }
    let nets: Vec<String> = args.get("nets").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    let edge_on = args.get("edge").and_then(|b| b.as_bool()).unwrap_or(false);
    if nets.is_empty() && !edge_on { return crate::input::refuse("bad_args", "name nets to fence (e.g. [\"ANT_RF\"]) and/or edge:true for a fence round the board edge"); }
    // the via the board's own via rule prefers, so the fence passes its DRC (0.6/0.3 broke the Bluetooth Sentinel
    // example's 0.5/0.3 rule on every via)
    let (size, drill, from) = via_of(input);
    let clearance = num(args, "clearance", 0.2);
    // lambda/20 in FR-4 (effective er about 3.4) at the highest frequency, else 2 mm
    let pitch = match args.get("freqGHz").and_then(|v| v.as_f64()) { Some(g) if g > 0.0 => (299.79 / g / 3.4f64.sqrt() / 20.0).min(num(args, "pitchMm", f64::MAX)), _ => num(args, "pitchMm", 2.0) };
    let widest = st["tracks"].as_array().cloned().unwrap_or_default().iter().filter(|t| nets.iter().any(|n| t["net"].as_str().map(|s| s.eq_ignore_ascii_case(n)).unwrap_or(false))).map(|t| f(&t["width"])).fold(0.0, f64::max);
    let offset = num(args, "offsetMm", widest / 2.0 + clearance + size / 2.0 + 0.1);
    let inset = if edge_on { Some(num(args, "edgeInsetMm", size / 2.0 + 0.5)) } else { None };
    let cands = fence_points(&st, &nets, pitch, offset, inset);
    let outline: Vec<(f64, f64)> = crate::coppermap::outline_ring(&st);
    let (ob, same) = obstacles(&st, via_net);
    let pts = keep_clear(&st, &outline, &cands, pitch * 0.6, size, clearance, 0.3, 5000, &ob, &same, &Default::default(), &[]);
    let planned = json!({"vias": pts.len(), "candidates": cands.len(), "via": {"sizeMm": size, "drillMm": drill, "from": from}, "pitchMm": (pitch * 1000.0).round() / 1000.0,
                         "offsetMm": (offset * 1000.0).round() / 1000.0, "points": pts.iter().map(|(x, y)| json!([x, y])).collect::<Vec<_>>()});
    if pts.is_empty() {
        return json!({"success": false, "errorCode": "no_room", "error": "no fence via fits", "planned": planned, "_hint": "Lower clearance or size, change offsetMm, or check the nets have tracks."});
    }
    json!({"success": true, "planned": planned, "apply": apply_of(&pts, via_net, size, drill),
        "_hint": "A plan only: run place_vias with apply.args, then repour so pours join the vias, then run_drc. candidates - vias are the spots where other copper, a pad or the edge was too close."})
}

/// The four corner positions of the outline's bounding box, `inset` mm in.
pub fn corners(state: &Value, inset: f64) -> Vec<(f64, f64)> {
    let o: Vec<(f64, f64)> = state["outline"].as_array().cloned().unwrap_or_default().iter().map(pt).collect();
    if o.is_empty() { return vec![]; }
    let (x0, x1) = o.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
    let (y0, y1) = o.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
    vec![(x0 + inset, y0 + inset), (x1 - inset, y0 + inset), (x1 - inset, y1 - inset), (x0 + inset, y1 - inset)]
}

pub fn region_of(v: Option<&Value>) -> Option<Vec<(f64, f64)>> {
    let a = v?.as_array()?;
    let r: Vec<(f64, f64)> = a.iter().map(pt).collect();
    if r.len() >= 3 { Some(r) } else { None }
}

pub fn summary(points: &[(f64, f64)]) -> Value {
    json!(points.iter().map(|(x, y)| json!([x, y])).collect::<Vec<_>>())
}

fn num(v: &Value, k: &str, d: f64) -> f64 { v.get(k).and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))).unwrap_or(d) }

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Value {
        json!({"outline": [[0, 0], [20, 0], [20, 10], [0, 10]],
               "tracks": [{"start": {"x": 0.0, "y": 5.0}, "end": {"x": 20.0, "y": 5.0}, "width": 0.25, "net": "SIG"}],
               "arcs": [], "vias": [{"position": {"x": 2.5, "y": 2.5}, "size": 0.6, "net": "GND"}],
               "pads": [{"x": 17.5, "y": 7.5, "sizeX": 1.0, "sizeY": 1.0, "drill": 0.0, "net": "GND"}]})
    }

    #[test]
    fn stitching_keeps_clear_of_other_nets_pads_edges_and_existing_vias() {
        let p = plan_stitching(&board(), "GND", 5.0, 0.6, 0.3, 0.5, None, 100);
        // the 5 mm grid is 2.5/7.5/12.5/17.5 by 2.5/7.5; (2.5,2.5) has a GND via already, (17.5,7.5) is a pad
        assert_eq!(p, vec![(7.5, 2.5), (12.5, 2.5), (17.5, 2.5), (2.5, 7.5), (7.5, 7.5), (12.5, 7.5)]);
        // a track of another net through the grid row blocks it
        let mut b = board();
        b["tracks"][0]["start"]["y"] = json!(2.5);
        b["tracks"][0]["end"]["y"] = json!(2.5);
        assert!(plan_stitching(&b, "GND", 5.0, 0.6, 0.3, 0.5, None, 100).iter().all(|p| p.1 != 2.5));
        // a region limits it, max caps it
        let reg = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert_eq!(plan_stitching(&board(), "GND", 5.0, 0.6, 0.3, 0.5, Some(&reg), 100), vec![(7.5, 2.5), (2.5, 7.5), (7.5, 7.5)]);
        assert_eq!(plan_stitching(&board(), "GND", 5.0, 0.6, 0.3, 0.5, None, 2).len(), 2);
    }

    #[test]
    fn stitching_stays_off_holes_and_out_from_under_parts() {
        let mut b = board();
        // a part U1 whose two pads span x 6..9 at y 7.5: (7.5, 7.5) lies between them, under its body
        b["pads"] = json!([{"ref": "U1", "x": 6.0, "y": 7.5, "sizeX": 0.6, "sizeY": 0.6, "drill": 0.0, "net": "SIG"},
                           {"ref": "U1", "x": 9.0, "y": 7.5, "sizeX": 0.6, "sizeY": 0.6, "drill": 0.0, "net": "SIG"}]);
        let p = plan_stitching_with(&b, "GND", 5.0, 0.6, 0.3, 0.5, None, 100, &[(12.5, 2.5, 1.6)], true, &[(15.0, 0.0, 20.0, 10.0)]);
        assert!(!p.contains(&(7.5, 7.5)) && !p.contains(&(12.5, 2.5)));
        assert!(p.iter().all(|q| q.0 < 15.0), "the keepout at x 15..20 takes no via");
        let p = plan_stitching_with(&b, "GND", 5.0, 0.6, 0.3, 0.5, None, 100, &[], false, &[]);
        assert!(p.contains(&(7.5, 7.5)));
    }

    #[test]
    fn corner_holes_sit_inset_from_the_outline() {
        assert_eq!(corners(&board(), 3.0), vec![(3.0, 3.0), (17.0, 3.0), (17.0, 7.0), (3.0, 7.0)]);
    }
    #[test]
    fn fence_points_line_both_sides_of_a_track_and_the_edge() {
        let st = json!({"tracks": [{"net": "RF", "layer": "F.Cu", "width": 0.4, "start": {"x": 0.0, "y": 5.0}, "end": {"x": 10.0, "y": 5.0}}],
                        "outline": [[-5, 0], [15, 0], [15, 10], [-5, 10]],
                        "outlineEdges": [{"kind": "line", "start": [-5.0, 0.0]}, {"kind": "line", "start": [15.0, 0.0]}, {"kind": "line", "start": [15.0, 10.0]}, {"kind": "line", "start": [-5.0, 10.0]}]});
        let p = fence_points(&st, &["RF".to_string()], 2.0, 1.0, None);
        assert_eq!(p.len(), 12);
        assert!(p.contains(&(0.0, 6.0)) && p.contains(&(10.0, 4.0)));
        let e = fence_points(&st, &[], 5.0, 1.0, Some(0.5));
        assert!(e.iter().all(|(x, y)| *x >= -4.5 - 1e-9 && *x <= 14.5 + 1e-9 && *y >= 0.5 - 1e-9 && *y <= 9.5 + 1e-9), "{e:?}");
        assert!(e.len() >= 10, "{e:?}");
    }
    #[test]
    fn via_style_reads_altiums_descriptor() {
        let rules = vec![json!({"enabled": true, "scope1": "All", "priority": 1, "descriptor": "Routing Via (MinHoleWidth=0.3mm) (MaxHoleWidth=0.3mm) (PreferredHoleWidth=0.3mm) (MinWidth=0.5mm) (MaxWidth=0.5mm) (PreferedWidth=0.5mm) (All)"})];
        assert_eq!(via_style(&rules), Some((0.5, 0.3)));
        let mil = vec![json!({"enabled": true, "scope1": "All", "priority": 1, "descriptor": "Routing Via (PreferredHoleWidth=12mil) (PreferedWidth=24mil) (All)"})];
        let (s, d) = via_style(&mil).unwrap();
        assert!((s - 0.6096).abs() < 1e-9 && (d - 0.3048).abs() < 1e-9);
    }
    #[test]
    fn thermal_grid_fills_an_exposed_pad() {
        // a 3.3 mm QFN pad, 0.6 mm vias, 1.1 mm pitch: a 3 x 3 grid centred on the pad
        let g = thermal_grid(10.0, 10.0, 3.3, 3.3, 0.0, 1.1, 0.6, 0.1);
        assert_eq!(g.len(), 9);
        assert!(g.contains(&(10.0, 10.0)) && g.contains(&(8.9, 11.1)));
        assert!(thermal_grid(0.0, 0.0, 0.5, 0.5, 0.0, 1.0, 0.6, 0.1).is_empty());
        // turned 90 degrees, a long pad's grid turns with it
        let r = thermal_grid(0.0, 0.0, 4.0, 1.5, 90.0, 1.1, 0.6, 0.1);
        assert!(r.iter().all(|(x, _)| x.abs() < 0.5));
    }

    #[test]
    fn via_style_takes_the_standard_rule_first() {
        let rules = vec![json!({"kind": "via", "sizeMm": 0.45, "drillMm": 0.2}),
                         json!({"enabled": true, "scope1": "All", "priority": 1, "descriptor": "Routing Via (PreferredHoleWidth=0.3mm) (PreferedWidth=0.5mm) (All)"})];
        assert_eq!(via_style(&rules), Some((0.45, 0.2)));
    }

    #[test]
    fn the_verbs_answer_place_vias_plans() {
        let st = json!({"nets": ["GND", "RF"], "outline": [[0, 0], [30, 0], [30, 20], [0, 20]],
            "outlineEdges": [{"kind": "line", "start": [0.0, 0.0]}, {"kind": "line", "start": [30.0, 0.0]}, {"kind": "line", "start": [30.0, 20.0]}, {"kind": "line", "start": [0.0, 20.0]}],
            "tracks": [{"net": "RF", "layer": "F.Cu", "width": 0.4, "start": {"x": 5.0, "y": 10.0}, "end": {"x": 25.0, "y": 10.0}}], "arcs": [], "vias": [],
            "pads": [{"ref": "U1", "name": "EP", "x": 15.0, "y": 4.0, "sizeX": 3.3, "sizeY": 3.3, "rotation": 0.0, "drill": 0.0, "net": "GND", "layer": "F.Cu"}]});
        let t = thermal_vias(&json!({"board": st, "rules": [{"kind": "via", "sizeMm": 0.6, "drillMm": 0.3}], "args": {"ref": "U1"}}));
        assert_eq!(t["success"], json!(true), "{t}");
        assert_eq!(t["planned"]["vias"], 9);
        assert_eq!(t["apply"]["verb"], "place_vias");
        assert_eq!(t["apply"]["args"]["vias"][0]["net"], "GND");
        assert_eq!(t["planned"]["via"]["from"], "the board's via rule");
        let v = via_fence(&json!({"board": st, "args": {"viaNet": "GND", "nets": ["RF"], "pitchMm": 2.0}}));
        assert_eq!(v["success"], json!(true), "{v}");
        assert!(v["planned"]["vias"].as_u64().unwrap() >= 10);
        assert_eq!(v["apply"]["args"]["vias"].as_array().unwrap().len() as u64, v["planned"]["vias"].as_u64().unwrap());
        assert_eq!(via_fence(&json!({"board": st, "args": {"viaNet": "NOPE", "nets": ["RF"]}}))["errorCode"], "no_such_net");
        assert_eq!(thermal_vias(&json!({"board": st, "args": {"ref": "U9"}}))["errorCode"], "part_not_found");
    }
    #[test]
    fn width_style_reads_the_standard_rule_and_altiums_descriptor() {
        assert_eq!(width_style(&[json!({"kind": "width", "preferredMm": 0.2})]), Some(0.2));
        assert_eq!(width_style(&[json!({"kind": "width", "name": "minimum width (msWidth)", "minMm": 0.15})]), Some(0.15));
        let alt = vec![json!({"enabled": true, "scope1": "InNet('VIN')", "priority": 1, "descriptor": "Width Constraint (Min=20mil) (Max=40mil) (Preferred=30mil) (InNet('VIN'))"}),
                       json!({"enabled": true, "scope1": "All", "priority": 2, "descriptor": "Width Constraint (Min=10mil) (Max=10mil) (Preferred=10mil) (All)"})];
        assert_eq!(width_style(&alt), Some(0.254), "the board-wide rule, not a net's");
        assert_eq!(width_style(&[json!({"descriptor": "Width Constraint (Min=0.15mm) (Max=1mm) (All)", "scope1": "All"})]), Some(0.15));
        assert_eq!(width_style(&[]), None);
    }
}

//! Teardrops: where a track meets a via or a pad of its own net, a tapered fill from the pad out along the track, so a
//! drill wander or an etch nick does not cut the joint. Each is a polygon on the track's layer, planned clear of every
//! other net's copper; a joint with no room is skipped and named. The shape is the straight teardrop KiCad draws: as
//! wide as the pad at the pad's centre (`widthRatio`), narrowing to the track `lengthRatio` x the pad's size beyond its
//! edge.
use crate::route::{pad_shape, Shape};
use serde_json::{json, Value};

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }
type P = (f64, f64);

/// The teardrop polygon from a joint at `c` (pad or via of size `s` across) along a track toward `toward`, `w` wide.
/// None when the track is too short or already as wide as the pad. Pure.
pub fn shape(c: P, toward: P, s: f64, w: f64, length_ratio: f64, width_ratio: f64) -> Option<Vec<P>> {
    let (dx, dy) = (toward.0 - c.0, toward.1 - c.1);
    let len = (dx * dx + dy * dy).sqrt();
    let half = (s / 2.0 * width_ratio).min(s / 2.0);
    if len < 1e-6 || w >= 2.0 * half * 0.9 { return None; }
    let (u, n) = ((dx / len, dy / len), (-dy / len, dx / len));
    // the taper ends lengthRatio x the pad's size past its edge, or before the track's far end
    let reach = (s / 2.0 + length_ratio * s).min(len * 0.8);
    if reach <= s / 2.0 { return None; }
    let at = |along: f64, side: f64| (c.0 + u.0 * along + n.0 * side, c.1 + u.1 * along + n.1 * side);
    Some(vec![at(0.0, half), at(reach, w / 2.0), at(reach, -w / 2.0), at(0.0, -half)])
}

/// teardrops: input {board, rules?, args: {targets?: ["via", "tht", "smd"] (default via and tht), lengthRatio? (0.5),
/// widthRatio? (1.0), clearanceMm?}}. The teardrops as polygons on their track's layer, and the joints skipped with why.
pub fn teardrops(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let targets: Vec<String> = args["targets"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_lowercase())).collect()).unwrap_or_else(|| vec!["via".into(), "tht".into()]);
    let (lr, wr) = (args["lengthRatio"].as_f64().unwrap_or(0.5), args["widthRatio"].as_f64().unwrap_or(1.0));
    let clearance = args["clearanceMm"].as_f64().unwrap_or(0.0).max(crate::fab::routing_clearance(&crate::input::rules(input)).unwrap_or(0.2));
    let net = |v: &Value| v["net"].as_str().unwrap_or("").to_string();
    let tracks: Vec<Value> = st["tracks"].as_array().cloned().unwrap_or_default().into_iter().filter(|t| !net(t).is_empty() && t["layer"].as_str().map(|l| l.ends_with(".Cu")).unwrap_or(false)).collect();
    let vias = st["vias"].as_array().cloned().unwrap_or_default();
    let pads = st["pads"].as_array().cloned().unwrap_or_default();
    // the joints: (name, net, centre, size across, the layers it is on: empty for every layer)
    let mut joints: Vec<(String, String, P, f64, Option<String>)> = Vec::new();
    if targets.iter().any(|t| t == "via") {
        for v in &vias { joints.push(("via".into(), net(v), (f(&v["position"]["x"]), f(&v["position"]["y"])), f(&v["size"]), None)); }
    }
    for p in &pads {
        let through = f(&p["drill"]) > 0.0 || p["layer"] == "*.Cu";
        if (through && !targets.iter().any(|t| t == "tht")) || (!through && !targets.iter().any(|t| t == "smd")) { continue; }
        let s = f(&p["sizeX"]).min(f(&p["sizeY"]));
        let name = format!("{}.{}", p["ref"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or(""));
        joints.push((name, net(p), (f(&p["x"]), f(&p["y"])), s, (!through).then(|| p["layer"].as_str().unwrap_or("").to_string())));
    }
    // other nets' copper, by layer ("" every layer)
    let mut copper: Vec<(String, String, Shape)> = Vec::new();
    for t in &tracks { copper.push((net(t), t["layer"].as_str().unwrap_or("").into(), Shape::Cap((f(&t["start"]["x"]), f(&t["start"]["y"])), (f(&t["end"]["x"]), f(&t["end"]["y"])), f(&t["width"]) / 2.0))); }
    for v in &vias { let c = (f(&v["position"]["x"]), f(&v["position"]["y"])); copper.push((net(v), String::new(), Shape::Cap(c, c, f(&v["size"]) / 2.0))); }
    for p in &pads { let l = if f(&p["drill"]) > 0.0 || p["layer"] == "*.Cu" { String::new() } else { p["layer"].as_str().unwrap_or("").to_string() }; copper.push((net(p), l, pad_shape(p))); }
    let (mut out, mut skipped) = (Vec::new(), Vec::new());
    for t in &tracks {
        let layer = t["layer"].as_str().unwrap_or("").to_string();
        let (a, b) = ((f(&t["start"]["x"]), f(&t["start"]["y"])), (f(&t["end"]["x"]), f(&t["end"]["y"])));
        for (end, other) in [(a, b), (b, a)] {
            let Some(j) = joints.iter().find(|j| j.1 == net(t) && j.4.as_ref().map(|l| *l == layer).unwrap_or(true) && ((j.2 .0 - end.0).powi(2) + (j.2 .1 - end.1).powi(2)).sqrt() < 0.01) else { continue };
            let Some(poly) = shape(j.2, other, j.3, f(&t["width"]), lr, wr) else { continue };
            let s = Shape::Poly(poly.clone());
            if let Some((on, _, _)) = copper.iter().find(|(on, l, c)| *on != j.1 && (l.is_empty() || *l == layer) && s.gap(c) < clearance - 1e-9) {
                skipped.push(json!({"at": j.0, "net": j.1, "layer": layer, "why": format!("no room: it would come within {clearance} mm of {on}")}));
                continue;
            }
            out.push(json!({"at": j.0, "net": j.1, "layer": layer, "points": poly.iter().map(|p| json!([r3(p.0), r3(p.1)])).collect::<Vec<_>>()}));
        }
    }
    json!({"success": true, "teardrops": out, "count": out.len(), "skipped": skipped, "clearanceMm": clearance, "targets": targets, "lengthRatio": lr, "widthRatio": wr, "revision": st["revision"],
        "_hint": "A plan: draw each teardrop as a filled copper polygon on its layer and net (the tool's own teardrop command where it has one), with expectedRevision: revision, then repour. Each keeps clearanceMm from other nets' copper; skipped names the joints with no room."})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_teardrop_tapers_from_the_pad_to_the_track() {
        let p = shape((0.0, 0.0), (5.0, 0.0), 0.6, 0.2, 0.5, 1.0).unwrap();
        // as wide as the via at its centre, the track's width 0.3 + 0.3 = 0.6 mm out
        assert_eq!(p, vec![(0.0, 0.3), (0.6, 0.1), (0.6, -0.1), (0.0, -0.3)]);
        // a track already as wide as the pad, or a stub too short to taper, takes none
        assert!(shape((0.0, 0.0), (5.0, 0.0), 0.6, 0.6, 0.5, 1.0).is_none());
        assert!(shape((0.0, 0.0), (0.3, 0.0), 0.6, 0.2, 0.5, 1.0).is_none());
    }

    #[test]
    fn teardrops_go_on_vias_and_skip_a_joint_with_no_room() {
        let b = json!({"revision": "r9",
            "tracks": [{"net": "A", "layer": "F.Cu", "width": 0.2, "start": {"x": 0.0, "y": 0.0}, "end": {"x": 5.0, "y": 0.0}},
                       {"net": "B", "layer": "F.Cu", "width": 0.2, "start": {"x": 10.0, "y": 0.0}, "end": {"x": 15.0, "y": 0.0}},
                       {"net": "C", "layer": "F.Cu", "width": 0.2, "start": {"x": 10.4, "y": -3.0}, "end": {"x": 10.4, "y": -0.45}}],
            "vias": [{"net": "A", "size": 0.6, "drill": 0.3, "position": {"x": 0.0, "y": 0.0}}, {"net": "B", "size": 0.6, "drill": 0.3, "position": {"x": 10.0, "y": 0.0}}],
            "pads": []});
        let r = teardrops(&json!({"board": b, "args": {}}));
        assert_eq!(r["count"], json!(1), "{r}");
        assert_eq!(r["teardrops"][0]["at"], "via");
        assert_eq!(r["teardrops"][0]["net"], "A");
        // B's teardrop would come too near C's track
        assert_eq!(r["skipped"][0]["net"], "B");
        assert!(r["skipped"][0]["why"].as_str().unwrap().contains("C"));
    }
}

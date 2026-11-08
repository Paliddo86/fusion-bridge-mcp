//! Length tuning: a serpentine in the net's longest straight track that has room beside it, its bumps sized to add
//! exactly the length a target (or another net's length) asks for, every bump clear of other copper and inside the
//! board, or a refusal that says why. From the Altium bridge's lengthtune.rs; the plan is answered as the track to
//! remove and the route_net arguments that replace it, for the bridge to apply in one undo step.
use crate::route::{pad_shape, Shape};
use serde_json::{json, Value};

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }
type P = (f64, f64);

/// The serpentine's points replacing a track a->b: `n` rectangular bumps of height `amp` to the side `side` (+1 left,
/// -1 right), each `pitch` long along the track, centred on it. Pure.
pub fn meander(a: P, b: P, n: usize, amp: f64, pitch: f64, side: f64) -> Vec<P> {
    let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let (ux, uy) = ((b.0 - a.0) / len, (b.1 - a.1) / len);
    let (nx, ny) = (-uy * side, ux * side);
    let at = |s: f64, h: f64| (a.0 + ux * s + nx * h, a.1 + uy * s + ny * h);
    let start = (len - n as f64 * pitch) / 2.0;
    let mut pts = vec![a];
    for k in 0..n {
        let s = start + k as f64 * pitch;
        pts.push(at(s, 0.0));
        pts.push(at(s, amp));
        pts.push(at(s + pitch / 2.0, amp));
        pts.push(at(s + pitch / 2.0, 0.0));
    }
    pts.push(b);
    pts.dedup_by(|p, q| (p.0 - q.0).abs() < 1e-9 && (p.1 - q.1).abs() < 1e-9);
    pts
}

/// How many bumps and how tall, to add `extra` mm along a track `len` long: as few bumps as fit at `max_amp`, each
/// `pitch` long, leaving `margin` at both ends. None when it cannot fit. Pure.
pub fn sizing(extra: f64, len: f64, pitch: f64, max_amp: f64, margin: f64) -> Option<(usize, f64)> {
    if extra <= 0.0 { return None; }
    let room = ((len - 2.0 * margin) / pitch).floor() as usize;
    if room == 0 { return None; }
    let n = ((extra / (2.0 * max_amp)).ceil() as usize).max(1);
    if n > room { return None; }
    Some((n, extra / (2.0 * n as f64)))
}

/// length_tune: input {board, rules?, args: {net, targetMm? | matchTo?, maxAmplitudeMm?, pitchMm?, clearance?}}.
pub fn length_tune(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let Some(net) = args["net"].as_str().map(String::from) else { return crate::input::refuse("bad_args", "net is the net to lengthen") };
    let lens = crate::lengths::lengths(&st);
    let len_of = |n: &str| lens.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| f(&v["lengthMm"]));
    let Some(now) = len_of(&net) else { return crate::input::refuse("not_routed", &format!("{net} has no routed track")) };
    let target = match (args["targetMm"].as_f64(), args["matchTo"].as_str()) {
        (Some(t), _) => t,
        (None, Some(m)) => match len_of(m) { Some(l) => l, None => return crate::input::refuse("not_routed", &format!("{m} has no routed track to match")) },
        _ => return crate::input::refuse("bad_args", "targetMm (mm) or matchTo (a net) says how long it should be"),
    };
    let extra = target - now;
    if extra <= 0.01 { return json!({"success": true, "changed": false, "net": net, "lengthMm": r3(now), "targetMm": r3(target), "_hint": "Already at or past the target: tuning only lengthens. Shorten the other net, or route this one more directly."}); }
    let num = |k: &str, d: f64| args[k].as_f64().unwrap_or(d);
    let rule = crate::fab::routing_clearance(&crate::input::rules(input));
    let clearance = num("clearance", 0.0).max(rule.unwrap_or(0.2));
    let same = |v: &Value| v["net"].as_str().map(|n| n.eq_ignore_ascii_case(&net)).unwrap_or(false);
    // other nets' copper by layer ("*" every layer): routed tracks (a hatched pour's net-less pieces re-pour round the
    // bumps), vias, and pads by their real outline
    let mut other: Vec<(String, Shape)> = Vec::new();
    for t in st["tracks"].as_array().into_iter().flatten().filter(|t| !same(t) && t["net"].as_str().map(|n| !n.is_empty()).unwrap_or(false)) {
        other.push((t["layer"].as_str().unwrap_or("").into(), Shape::Cap((f(&t["start"]["x"]), f(&t["start"]["y"])), (f(&t["end"]["x"]), f(&t["end"]["y"])), f(&t["width"]) / 2.0)));
    }
    for v in st["vias"].as_array().into_iter().flatten().filter(|v| !same(v)) {
        let c = (f(&v["position"]["x"]), f(&v["position"]["y"]));
        other.push(("*".into(), Shape::Cap(c, c, f(&v["size"]) / 2.0)));
    }
    for p in st["pads"].as_array().into_iter().flatten().filter(|p| !same(p)) {
        let l = if p["layer"] == "*.Cu" || f(&p["drill"]) > 0.0 { "*".to_string() } else { p["layer"].as_str().unwrap_or("").to_string() };
        other.push((l, pad_shape(p)));
    }
    let ring = crate::coppermap::outline_ring(&st);
    // the net's tracks, longest first; for each, a side where the serpentine fits
    let mut tracks: Vec<Value> = st["tracks"].as_array().cloned().unwrap_or_default().into_iter().filter(same).collect();
    let tlen = |t: &Value| ((f(&t["end"]["x"]) - f(&t["start"]["x"])).powi(2) + (f(&t["end"]["y"]) - f(&t["start"]["y"])).powi(2)).sqrt();
    tracks.sort_by(|a, b| tlen(b).partial_cmp(&tlen(a)).unwrap());
    let mut why = Vec::new();
    for t in &tracks {
        let (a, b) = ((f(&t["start"]["x"]), f(&t["start"]["y"])), (f(&t["end"]["x"]), f(&t["end"]["y"])));
        let w = f(&t["width"]);
        let layer = t["layer"].as_str().unwrap_or("F.Cu").to_string();
        let pitch = num("pitchMm", (4.0 * w).max(4.0 * clearance + 2.0 * w));
        let Some((n, amp)) = sizing(extra, tlen(t), pitch, num("maxAmplitudeMm", 3.0), 2.0 * w) else { why.push(json!({"track": [[r3(a.0), r3(a.1)], [r3(b.0), r3(b.1)]], "why": "too short for the bumps this needs at this amplitude"})); continue };
        for side in [1.0, -1.0] {
            let pts = meander(a, b, n, amp, pitch, side);
            let clear = pts.windows(2).all(|s| other.iter().filter(|o| o.0 == layer || o.0 == "*").all(|o| o.1.dist(s[0], s[1]) >= w / 2.0 + clearance - 1e-9))
                && (ring.len() < 3 || pts.iter().all(|p| crate::route::inside(*p, &ring)));
            if !clear { continue; }
            let added: f64 = pts.windows(2).map(|s| ((s[1].0 - s[0].0).powi(2) + (s[1].1 - s[0].1).powi(2)).sqrt()).sum::<f64>() - tlen(t);
            let way: Vec<Value> = pts.iter().map(|p| json!({"x": r3(p.0), "y": r3(p.1), "layer": layer})).collect();
            return json!({"success": true, "changed": false, "net": net, "lengthMm": r3(now), "targetMm": r3(target), "expectedMm": r3(now + added), "clearanceMm": clearance,
                "plan": {"layer": layer, "replaces": [[r3(a.0), r3(a.1)], [r3(b.0), r3(b.1)]], "bumps": n, "amplitudeMm": r3(amp), "pitchMm": r3(pitch), "side": if side > 0.0 { "left" } else { "right" }, "addsMm": r3(added)},
                "remove": {"tracks": [{"net": net, "layer": layer, "start": {"x": a.0, "y": a.1}, "end": {"x": b.0, "y": b.1}}]},
                "routeNet": {"net": net, "layer": layer, "width": w, "paths": [way]}, "revision": st["revision"],
                "_hint": "A plan: remove the one track in `remove` and route `routeNet` in its place, in one undo step, with expectedRevision: revision. net_lengths then shows the new length; every bump keeps clearanceMm from other nets' pads, routed tracks and vias."});
        }
        why.push(json!({"track": [[r3(a.0), r3(a.1)], [r3(b.0), r3(b.1)]], "why": "no clear room on either side for the bumps"}));
    }
    json!({"success": false, "errorCode": "no_room", "error": format!("no track of {net} has room for {:.2} mm more", extra), "lengthMm": r3(now), "targetMm": r3(target), "tried": why,
           "_hint": "Lower maxAmplitudeMm for more, shorter bumps, raise it for fewer, or reroute the net with a longer straight run in open space."})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_meander_adds_exactly_twice_its_height_per_bump() {
        let (n, amp) = sizing(6.0, 30.0, 1.2, 1.0, 0.4).unwrap();
        assert_eq!(n, 3);
        assert!((amp - 1.0).abs() < 1e-9);
        let pts = meander((0.0, 0.0), (30.0, 0.0), n, amp, 1.2, 1.0);
        let len: f64 = pts.windows(2).map(|s| ((s[1].0 - s[0].0).powi(2) + (s[1].1 - s[0].1).powi(2)).sqrt()).sum();
        assert!((len - 36.0).abs() < 1e-9, "{len}");
        assert!(sizing(100.0, 5.0, 1.2, 1.0, 0.4).is_none());
        assert!(sizing(-1.0, 30.0, 1.2, 1.0, 0.4).is_none());
    }

    #[test]
    fn the_verb_plans_round_other_copper_to_match_a_net() {
        let t = |net: &str, y: f64, x1: f64| json!({"net": net, "layer": "F.Cu", "width": 0.2, "start": {"x": 2.0, "y": y}, "end": {"x": x1, "y": y}});
        // A is 30 mm, B 24 mm; another net runs just above A, so the bumps go below it
        let b = json!({"outline": [[0, -20], [40, -20], [40, 20], [0, 20]], "revision": "r3",
            "tracks": [t("A", 0.0, 32.0), t("B", 10.0, 26.0), json!({"net": "X", "layer": "F.Cu", "width": 0.2, "start": {"x": 2.0, "y": 0.6}, "end": {"x": 32.0, "y": 0.6}})]});
        let r = length_tune(&json!({"board": b, "args": {"net": "B", "matchTo": "A"}}));
        assert_eq!(r["success"], json!(true), "{r}");
        assert!((f(&r["expectedMm"]) - 30.0).abs() < 1e-6, "{r}");
        let a = length_tune(&json!({"board": b, "args": {"net": "A", "targetMm": 34.0}}));
        assert_eq!(a["plan"]["side"], "right", "{a}");
        assert_eq!(length_tune(&json!({"board": b, "args": {"net": "A", "targetMm": 20.0}}))["changed"], json!(false));
        assert_eq!(length_tune(&json!({"board": b, "args": {"net": "Q", "targetMm": 20.0}}))["errorCode"], "not_routed");
    }
}

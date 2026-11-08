//! A differential pair planned as a pair: one centreline from the midpoint of the source pads through waypoints to the
//! midpoint of the destination pads, each net offset half a pitch (width + gap) to its own side with mitred corners, a
//! short fan-out from each pad, both lengths and the skew, and what the plan would run into. From the Altium bridge's
//! pairroute.rs; the plan is answered as route_net arguments (and the pair's add_rule width rule), for the bridge to
//! apply.
use crate::route::{pad_shape, Shape};
use serde_json::{json, Value};

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }
type P = (f64, f64);

fn sub(a: P, b: P) -> P { (a.0 - b.0, a.1 - b.1) }
fn norm(a: P) -> P { let l = (a.0 * a.0 + a.1 * a.1).sqrt(); if l > 0.0 { (a.0 / l, a.1 / l) } else { (0.0, 0.0) } }
fn left(d: P) -> P { (-d.1, d.0) }

/// The polyline offset `d` to the left (negative: right), corners mitred. Pure.
pub fn offset(path: &[P], d: f64) -> Vec<P> {
    let n = path.len();
    (0..n).map(|i| {
        let din = if i > 0 { norm(sub(path[i], path[i - 1])) } else { norm(sub(path[1], path[0])) };
        let dout = if i + 1 < n { norm(sub(path[i + 1], path[i])) } else { din };
        let (n1, n2) = (left(din), left(dout));
        let m = norm((n1.0 + n2.0, n1.1 + n2.1));
        let k = (m.0 * n1.0 + m.1 * n1.1).max(0.2); // a hairpin would blow the mitre up: cap it at 5x
        (path[i].0 + m.0 * d / k, path[i].1 + m.1 * d / k)
    }).collect()
}

fn length(p: &[P]) -> f64 { p.windows(2).map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt()).sum() }

/// Both nets' points from pad to pad: the centreline from the pad midpoints through `via`, each net on the side its
/// source pad is on, `fan` mm of fan-out at each end. Pure.
pub fn plan(pos: (P, P), neg: (P, P), via: &[P], width: f64, gap: f64, fan: f64) -> (Vec<P>, Vec<P>) {
    let s = ((pos.0 .0 + neg.0 .0) / 2.0, (pos.0 .1 + neg.0 .1) / 2.0);
    let e = ((pos.1 .0 + neg.1 .0) / 2.0, (pos.1 .1 + neg.1 .1) / 2.0);
    let mut centre = vec![s];
    centre.extend_from_slice(via);
    centre.push(e);
    // pull the ends in by the fan-out so each pad has room to reach its own side
    let d0 = norm(sub(centre[1], centre[0]));
    let k = centre.len();
    let d1 = norm(sub(centre[k - 1], centre[k - 2]));
    centre[0] = (s.0 + d0.0 * fan, s.1 + d0.1 * fan);
    centre[k - 1] = (e.0 - d1.0 * fan, e.1 - d1.1 * fan);
    let half = (width + gap) / 2.0;
    // the positive net goes on the side its source pad is on
    let side = { let v = sub(pos.0, s); let n = left(d0); if v.0 * n.0 + v.1 * n.1 >= 0.0 { 1.0 } else { -1.0 } };
    let mut p = vec![pos.0]; p.extend(offset(&centre, half * side)); p.push(pos.1);
    let mut n = vec![neg.0]; n.extend(offset(&centre, -half * side)); n.push(neg.1);
    (p, n)
}

fn crosses(a: P, b: P, c: P, d: P) -> bool {
    let o = |p: P, q: P, r: P| (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0);
    let (d1, d2, d3, d4) = (o(c, d, a), o(c, d, b), o(a, b, c), o(a, b, d));
    (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0)
}

/// What is wrong with a planned pair before it touches the board: the two nets crossing each other (the pads swap sides
/// between the ends), or a segment closer than the clearance to another net's copper on the layer. `copper` is
/// (what, net, shape). Pure.
pub(crate) fn check(p: &[P], n: &[P], copper: &[(String, String, Shape)], nets: (&str, &str), width: f64, clearance: f64) -> Vec<Value> {
    let mut out = Vec::new();
    for i in 0..p.len().saturating_sub(1) {
        for j in 0..n.len().saturating_sub(1) {
            if crosses(p[i], p[i + 1], n[j], n[j + 1]) {
                out.push(json!({"problem": "pair_crosses", "at": [r3((p[i].0 + p[i + 1].0) / 2.0), r3((p[i].1 + p[i + 1].1) / 2.0)],
                                "fix": "the pads are on swapped sides at the two ends: route to a via pair and cross on another layer, or swap the pins"}));
                return out;
            }
        }
    }
    for (path, net) in [(p, nets.0), (n, nets.1)] {
        // the first and last segments leave their own pads: those pads are the net's own, so any copper they touch is checked too
        'segs: for w in path.windows(2) {
            for (what, on, s) in copper {
                if on.eq_ignore_ascii_case(net) { continue; }
                let d = s.dist(w[0], w[1]);
                if d < width / 2.0 + clearance - 1e-9 {
                    out.push(json!({"problem": if what.contains('.') { "over_a_pad" } else { "over_copper" }, "net": net, "with": what, "withNet": on, "gapMm": r3((d - width / 2.0).max(0.0)),
                                    "fix": "add waypoints (via) that lead the pair out of the part and round other copper"}));
                    break 'segs;
                }
            }
        }
    }
    out
}

/// route_pair: input {board, stack?, rules?, args: {pos, neg, from: {pos, neg}, to: {pos, neg}, via?: [[x, y]], layer?,
/// width?, gap?, diffOhms?, fanMm?, clearance?}}. The plan: `routeNet` for each net, the pair's `widthRule`, lengths,
/// skew, and `problems` (the plan answers success: false while there are any).
pub fn route_pair(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let s = |k: &str| args[k].as_str().map(String::from);
    let (Some(pn), Some(nn)) = (s("pos"), s("neg")) else { return crate::input::refuse("bad_args", "pos and neg are the pair's two nets") };
    let pad = |o: &str, k: &str| args[o][k].as_str().map(String::from);
    let (Some(pf), Some(nf), Some(pt), Some(nt)) = (pad("from", "pos"), pad("from", "neg"), pad("to", "pos"), pad("to", "neg")) else {
        return crate::input::refuse("bad_args", "from: {pos: \"U1.3\", neg: \"U1.4\"} and to: {pos, neg} name the pads at each end");
    };
    let pads = st["pads"].as_array().cloned().unwrap_or_default();
    let key = |p: &Value| format!("{}.{}", p["ref"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or(""));
    let at = |k: &str| pads.iter().find(|p| key(p).eq_ignore_ascii_case(k)).map(|p| (f(&p["x"]), f(&p["y"])));
    let (Some(a), Some(b), Some(c), Some(d)) = (at(&pf), at(&nf), at(&pt), at(&nt)) else {
        return crate::input::refuse("no_such_pad", &format!("one of {pf}, {nf}, {pt}, {nt} is not a pad on the board"));
    };
    let layer = args["layer"].as_str().unwrap_or("F.Cu").to_string();
    // width and gap: given, or solved from the stack-up for diffOhms on this layer
    let (mut width, mut gap, mut sized) = (args["width"].as_f64(), args["gap"].as_f64(), Value::Null);
    if let (Some(z), Some(stack)) = (args["diffOhms"].as_f64(), crate::input::stack(input)) {
        let geo = crate::impedance::geometry(&stack);
        let names = crate::coppermap::stack_layers(&stack);
        if let Some(i) = names.iter().position(|n| n.0 == layer) {
            if i < geo.len() {
                let rows = crate::impedance::widths(&geo[i..=i], None, Some(z), gap);
                if let (Some(w), Some(g)) = (rows[0]["diffWidth"].as_f64(), rows[0]["diffGap"].as_f64()) { width = width.or(Some(w)); gap = gap.or(Some(g)); sized = rows[0].clone(); }
            }
        }
    }
    let (Some(width), Some(gap)) = (width, gap) else { return crate::input::refuse("bad_args", "give width and gap (mm), or diffOhms with the stack to solve them") };
    let via: Vec<P> = args["via"].as_array().map(|a| a.iter().map(|p| (f(&p[0]), f(&p[1]))).collect()).unwrap_or_default();
    let fan = args["fanMm"].as_f64().unwrap_or(1.0);
    let (pp, np) = plan((a, c), (b, d), &via, width, gap, fan);
    // other nets' copper on the layer: pads by their outline, routed tracks and vias (a hatched pour's net-less pieces
    // re-pour round the pair)
    let rule = crate::fab::routing_clearance(&crate::input::rules(input));
    let clearance = args["clearance"].as_f64().unwrap_or(0.0).max(rule.unwrap_or(0.15));
    let mut copper: Vec<(String, String, Shape)> = pads.iter().filter(|p| p["layer"] == layer || p["layer"] == "*.Cu" || f(&p["drill"]) > 0.0)
        .map(|p| (key(p), p["net"].as_str().unwrap_or("").to_string(), pad_shape(p))).collect();
    for t in st["tracks"].as_array().into_iter().flatten().filter(|t| t["layer"] == json!(layer) && t["net"].as_str().map(|n| !n.is_empty()).unwrap_or(false)) {
        copper.push(("track".into(), t["net"].as_str().unwrap_or("").into(), Shape::Cap((f(&t["start"]["x"]), f(&t["start"]["y"])), (f(&t["end"]["x"]), f(&t["end"]["y"])), f(&t["width"]) / 2.0)));
    }
    for v in st["vias"].as_array().into_iter().flatten() {
        let c = (f(&v["position"]["x"]), f(&v["position"]["y"]));
        copper.push(("via".into(), v["net"].as_str().unwrap_or("").into(), Shape::Cap(c, c, f(&v["size"]) / 2.0)));
    }
    let problems = check(&pp, &np, &copper, (&pn, &nn), width, clearance);
    let path = |v: &[P], first: &str, last: &str| -> Value {
        let mut out = vec![json!({"pad": first})];
        for p in &v[1..v.len() - 1] { out.push(json!({"x": r3(p.0), "y": r3(p.1), "layer": layer})); }
        out.push(json!({"pad": last}));
        json!([out])
    };
    let (lp, ln) = (length(&pp), length(&np));
    let ok = problems.is_empty();
    json!({"success": ok, "errorCode": if ok { Value::Null } else { json!("pair_plan_blocked") }, "error": if ok { Value::Null } else { json!("the planned pair would run into copper or cross itself") },
        "problems": problems, "widthMm": r3(width), "gapMm": r3(gap), "sizedFor": sized, "layer": layer, "clearanceMm": clearance,
        "routeNet": [{"net": pn, "layer": layer, "width": r3(width), "paths": path(&pp, &pf, &pt)}, {"net": nn, "layer": layer, "width": r3(width), "paths": path(&np, &nf, &nt)}],
        "widthRule": {"kind": "width", "name": format!("Pair {pn} {nn}"), "scope1": format!("InNet('{pn}') Or InNet('{nn}')"), "min": r3(width * 0.9), "preferred": r3(width), "max": r3(width * 1.1)},
        "lengthsMm": {pn.clone(): r3(lp), nn.clone(): r3(ln)}, "skewMm": r3((lp - ln).abs()), "revision": st["revision"],
        "_hint": if ok { "A plan: send each routeNet entry to route_net (with expectedRevision: revision), then widthRule to add_rule so the board's default width rule does not flag the pair. skewMm is the length difference from the fan-outs and corners: net_lengths measures it routed, length_tune evens it." }
                 else { "Nothing to apply while problems is not empty: add waypoints (via) that lead the pair out of the parts and round other copper, or change layer where the pair must cross, and plan again." }})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_keeps_the_pitch_round_a_corner() {
        let l = offset(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], 0.5);
        assert!((l[0].1 - 0.5).abs() < 1e-9);
        assert!((l[1].0 - 9.5).abs() < 1e-9 && (l[1].1 - 0.5).abs() < 1e-9);
        assert!((l[2].0 - 9.5).abs() < 1e-9);
    }

    #[test]
    fn plan_puts_each_net_on_its_pads_side_and_check_catches_crossings_and_copper() {
        let (p, n) = plan(((0.0, 0.5), (20.0, 0.5)), ((0.0, -0.5), (20.0, -0.5)), &[], 0.2, 0.15, 1.0);
        assert!(p[1].1 > 0.0 && n[1].1 < 0.0);
        assert!((p[1].1 - n[1].1 - 0.35).abs() < 1e-9);
        assert!((length(&p) - length(&n)).abs() < 1e-9);
        let (x, y) = plan(((0.0, 0.5), (20.0, -0.5)), ((0.0, -0.5), (20.0, 0.5)), &[], 0.2, 0.15, 1.0);
        assert_eq!(check(&x, &y, &[], ("P", "N"), 0.2, 0.15)[0]["problem"], "pair_crosses");
        let pad = pad_shape(&json!({"x": 10.0, "y": 0.175, "sizeX": 0.6, "sizeY": 0.6, "shape": "round"}));
        assert_eq!(check(&p, &n, &[("U1.8".into(), "X".into(), pad)], ("P", "N"), 0.2, 0.15)[0]["problem"], "over_a_pad");
        let track = Shape::Cap((10.0, -3.0), (10.0, 3.0), 0.1);
        assert_eq!(check(&p, &n, &[("track".into(), "X".into(), track)], ("P", "N"), 0.2, 0.15)[0]["problem"], "over_copper");
        assert!(check(&p, &n, &[], ("P", "N"), 0.2, 0.15).is_empty());
    }

    #[test]
    fn the_verb_answers_route_net_arguments_for_both_nets() {
        let pad = |r: &str, n: &str, x: f64, y: f64, net: &str| json!({"ref": r, "name": n, "x": x, "y": y, "sizeX": 0.3, "sizeY": 0.3, "rotation": 0, "layer": "F.Cu", "drill": 0, "net": net, "shape": "rect"});
        let b = json!({"revision": "r7", "pads": [pad("U1", "1", 0.0, 0.5, "USB_P"), pad("U1", "2", 0.0, -0.5, "USB_N"), pad("J1", "1", 20.0, 0.5, "USB_P"), pad("J1", "2", 20.0, -0.5, "USB_N")]});
        let r = route_pair(&json!({"board": b, "args": {"pos": "USB_P", "neg": "USB_N", "from": {"pos": "U1.1", "neg": "U1.2"}, "to": {"pos": "J1.1", "neg": "J1.2"}, "width": 0.2, "gap": 0.15}}));
        assert_eq!(r["success"], json!(true), "{r}");
        assert_eq!(r["routeNet"][0]["paths"][0][0], json!({"pad": "U1.1"}));
        assert_eq!(r["skewMm"], json!(0.0));
        assert_eq!(r["widthRule"]["preferred"], json!(0.2));
        assert_eq!(route_pair(&json!({"board": b, "args": {"pos": "USB_P", "neg": "USB_N", "from": {"pos": "U9.1", "neg": "U1.2"}, "to": {"pos": "J1.1", "neg": "J1.2"}, "width": 0.2, "gap": 0.15}}))["errorCode"], "no_such_pad");
    }
}

//! Routed length per net from the board model (tracks and arcs, per layer, with the vias), the skew inside each
//! differential pair, and the spread of a matched-length group against its tolerance. From the Altium bridge's lengths.rs.
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }

/// Per net: total routed length (mm), length by layer, via count and segment count. Pure, for the tests.
pub fn lengths(state: &Value) -> BTreeMap<String, Value> {
    let mut acc: BTreeMap<String, (f64, BTreeMap<String, f64>, usize, usize)> = BTreeMap::new();
    let mut add = |net: &str, layer: &str, len: f64| {
        if net.is_empty() { return; }
        let e = acc.entry(net.to_string()).or_default();
        e.0 += len;
        *e.1.entry(layer.to_string()).or_insert(0.0) += len;
        e.3 += 1;
    };
    for t in state["tracks"].as_array().cloned().unwrap_or_default() {
        let len = ((f(&t["end"]["x"]) - f(&t["start"]["x"])).powi(2) + (f(&t["end"]["y"]) - f(&t["start"]["y"])).powi(2)).sqrt();
        add(t["net"].as_str().unwrap_or(""), t["layer"].as_str().unwrap_or("?"), len);
    }
    for a in state["arcs"].as_array().cloned().unwrap_or_default() {
        let mut span = f(&a["endAngle"]) - f(&a["startAngle"]);
        if span <= 0.0 { span += 360.0; }
        add(a["net"].as_str().unwrap_or(""), a["layer"].as_str().unwrap_or("?"), f(&a["radius"]) * span.to_radians());
    }
    for v in state["vias"].as_array().cloned().unwrap_or_default() {
        if let Some(n) = v["net"].as_str().filter(|n| !n.is_empty()) { acc.entry(n.to_string()).or_default().2 += 1; }
    }
    acc.into_iter().map(|(n, (len, by, vias, segs))| {
        let by: Map<String, Value> = by.into_iter().map(|(k, v)| (k, json!(r3(v)))).collect();
        (n.clone(), json!({"net": n, "lengthMm": r3(len), "byLayer": by, "vias": vias, "segments": segs}))
    }).collect()
}

fn len_of(l: &BTreeMap<String, Value>, net: &str) -> f64 {
    l.iter().find(|(k, _)| k.eq_ignore_ascii_case(net)).map(|(_, v)| f(&v["lengthMm"])).unwrap_or(0.0)
}

/// Skew of each pair (pos minus neg). Pure, for the tests.
pub fn pair_skew(l: &BTreeMap<String, Value>, pairs: &[Value]) -> Vec<Value> {
    pairs.iter().map(|p| {
        let (pos, neg) = (p["pos"].as_str().unwrap_or(""), p["neg"].as_str().unwrap_or(""));
        let (a, b) = (len_of(l, pos), len_of(l, neg));
        json!({"name": p.get("name").cloned().unwrap_or(json!(format!("{pos}/{neg}"))), "pos": pos, "neg": neg, "posMm": r3(a), "negMm": r3(b), "skewMm": r3(a - b)})
    }).collect()
}

/// A matched-length group: its shortest and longest net and whether the spread is within the tolerance. Pure.
pub fn group(l: &BTreeMap<String, Value>, nets: &[String], tol: f64) -> Value {
    let vals: Vec<(String, f64)> = nets.iter().map(|n| (n.clone(), len_of(l, n))).collect();
    let (min, max) = vals.iter().fold((f64::MAX, f64::MIN), |a, v| (a.0.min(v.1), a.1.max(v.1)));
    let spread = if vals.is_empty() { 0.0 } else { max - min };
    let longest = vals.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).map(|v| v.0.clone());
    json!({"nets": vals.iter().map(|(n, v)| json!({"net": n, "lengthMm": r3(*v), "shortByMm": r3(max - v)})).collect::<Vec<_>>(),
           "minMm": r3(if vals.is_empty() { 0.0 } else { min }), "maxMm": r3(if vals.is_empty() { 0.0 } else { max }), "spreadMm": r3(spread),
           "toleranceMm": tol, "withinTolerance": spread <= tol + 1e-9, "longest": longest})
}

/// Differential pairs by their net names: X_P/X_N, XP/XN (after a separator), X+/X-. Pure.
pub fn pairs_by_name(l: &BTreeMap<String, Value>) -> Vec<Value> {
    let mut out = Vec::new();
    for n in l.keys() {
        let u = n.to_uppercase();
        for (p, q) in [("_P", "_N"), ("+", "-"), ("_DP", "_DM"), ("_D+", "_D-")] {
            if let Some(base) = u.strip_suffix(p) {
                let want = format!("{base}{q}");
                if let Some(neg) = l.keys().find(|k| k.to_uppercase() == want) {
                    out.push(json!({"name": &n[..base.len()], "pos": n, "neg": neg}));
                }
            }
        }
    }
    out
}

/// net_lengths: input {board, args: {nets?, pairs?: [{name?, pos, neg}], groups?: [{nets, toleranceMm}]}}. Without
/// pairs, the pairs come from the net names (pairsFrom says which).
pub fn net_lengths(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let all = lengths(&st);
    let want: Vec<String> = args.get("nets").and_then(|n| n.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let nets: Vec<Value> = if want.is_empty() { all.values().cloned().collect() } else {
        want.iter().map(|n| all.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.clone()).unwrap_or(json!({"net": n, "lengthMm": 0.0, "unrouted": true}))).collect()
    };
    let (pairs, from): (Vec<Value>, &str) = match args.get("pairs").and_then(|p| p.as_array()) {
        Some(p) => (p.clone(), "args"),
        None => (pairs_by_name(&all), "net names (_P/_N, +/-): pass pairs for the board's own differential pairs"),
    };
    let groups: Vec<Value> = args.get("groups").and_then(|g| g.as_array()).cloned().unwrap_or_default().iter().map(|g| {
        let n: Vec<String> = g["nets"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
        group(&all, &n, g.get("toleranceMm").and_then(|t| t.as_f64()).unwrap_or(0.5))
    }).collect();
    let out_of_tol = groups.iter().filter(|g| g["withinTolerance"] == json!(false)).count();
    json!({"success": true, "units": "mm", "nets": nets, "pairs": pair_skew(&all, &pairs), "pairsFrom": from, "groups": groups, "groupsOutOfTolerance": out_of_tol,
        "_hint": "Lengths are the routed copper of each net (tracks and arcs; vias counted, not added). A net with branches sums all of them, so read a multi-pin net as total copper, not pin to pin. Lengthen the short nets of a group (length_tune, or route_net with a detour) until spreadMm is within toleranceMm; add a matched-length rule with add_rule."})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Value {
        json!({"tracks": [
            {"start": {"x": 0.0, "y": 0.0}, "end": {"x": 10.0, "y": 0.0}, "layer": "F.Cu", "net": "USB_P"},
            {"start": {"x": 10.0, "y": 0.0}, "end": {"x": 10.0, "y": 5.0}, "layer": "B.Cu", "net": "USB_P"},
            {"start": {"x": 0.0, "y": 1.0}, "end": {"x": 12.0, "y": 1.0}, "layer": "F.Cu", "net": "USB_N"}],
            "arcs": [{"center": {"x": 0.0, "y": 0.0}, "radius": 2.0, "startAngle": 0.0, "endAngle": 90.0, "layer": "F.Cu", "net": "CLK"}],
            "vias": [{"position": {"x": 10.0, "y": 0.0}, "net": "USB_P"}]})
    }

    #[test]
    fn lengths_add_tracks_and_arcs_per_layer() {
        let l = lengths(&board());
        assert_eq!(l["USB_P"]["lengthMm"], json!(15.0));
        assert_eq!(l["USB_P"]["byLayer"]["B.Cu"], json!(5.0));
        assert_eq!(l["USB_P"]["vias"], json!(1));
        assert_eq!(l["CLK"]["lengthMm"], json!(3.142));
    }

    #[test]
    fn pairs_report_skew_and_groups_their_spread() {
        let l = lengths(&board());
        let s = pair_skew(&l, &[json!({"name": "USB", "pos": "USB_P", "neg": "USB_N"})]);
        assert_eq!(s[0]["skewMm"], json!(3.0));
        let g = group(&l, &["USB_P".into(), "usb_n".into()], 2.0);
        assert_eq!((g["spreadMm"].clone(), g["withinTolerance"].clone(), g["longest"].clone()), (json!(3.0), json!(false), json!("USB_P")));
        assert_eq!(g["nets"][1]["shortByMm"], json!(3.0));
        // the verb finds USB_P/USB_N by name when no pairs are given
        let r = net_lengths(&json!({"board": board(), "args": {}}));
        assert_eq!((r["pairs"][0]["name"].clone(), r["pairs"][0]["skewMm"].clone()), (json!("USB"), json!(3.0)), "{r}");
    }
}

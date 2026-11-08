//! Current capacity of a net's copper (IPC-2221): the width each layer needs for a current at an allowed temperature
//! rise, against the narrowest track the net actually has there, and how many of its vias one layer change needs.
use serde_json::{json, Map, Value};

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }
fn r2(v: f64) -> f64 { (v * 100.0).round() / 100.0 }

const MIL: f64 = 0.0254;

/// IPC-2221's chart as a formula: the cross-section (mil^2) for `amps` at `rise` degrees C, outer or inner layer.
fn area_mil2(amps: f64, rise: f64, outer: bool) -> f64 {
    let k = if outer { 0.048 } else { 0.024 };
    (amps / (k * rise.powf(0.44))).powf(1.0 / 0.725)
}

/// The track width (mm) `amps` needs on copper `copper_um` thick.
pub fn width_for(amps: f64, rise: f64, copper_um: f64, outer: bool) -> f64 {
    let t_mil = copper_um / 1000.0 / MIL;
    area_mil2(amps, rise, outer) / t_mil * MIL
}

/// What one via carries: its barrel (plating `plating_um` around a `drill` mm hole) as an inner conductor.
pub fn via_amps(drill: f64, plating_um: f64, rise: f64) -> f64 {
    let t = plating_um / 1000.0;
    let a_mil2 = std::f64::consts::PI * (drill + t) * t / (MIL * MIL);
    0.024 * rise.powf(0.44) * a_mil2.powf(0.725)
}

/// routing_state's layer name to its index in the stack (F.Cu first, In<n>.Cu, B.Cu last).
fn stack_index(layer: &str, copper: usize) -> Option<usize> {
    match layer {
        "F.Cu" => Some(0),
        "B.Cu" => Some(copper.saturating_sub(1)),
        l => l.strip_prefix("In").and_then(|r| r.strip_suffix(".Cu")).and_then(|n| n.parse().ok()),
    }
}

/// Per net asked: each layer it is routed on with its narrowest track and the width the current needs. Pure.
pub fn check(state: &Value, copper_um: &[f64], nets: &Map<String, Value>, rise: f64, plating_um: f64) -> Vec<Value> {
    let tracks = state["tracks"].as_array().cloned().unwrap_or_default();
    let arcs = state["arcs"].as_array().cloned().unwrap_or_default();
    let vias = state["vias"].as_array().cloned().unwrap_or_default();
    let polys = state["polygons"].as_array().cloned().unwrap_or_default();
    let n = copper_um.len().max(2);
    nets.iter().map(|(net, amps)| {
        let amps = f(amps);
        let same = |v: &Value| v.as_str().map(|s| s.eq_ignore_ascii_case(net)).unwrap_or(false);
        // the narrowest piece of copper on each layer, tracks and arcs alike
        let mut narrow: std::collections::BTreeMap<String, (f64, Value)> = Default::default();
        let mut see = |layer: &str, w: f64, at: Value| {
            let e = narrow.entry(layer.to_string()).or_insert((f64::MAX, Value::Null));
            if w < e.0 { *e = (w, at); }
        };
        for t in tracks.iter().filter(|t| same(&t["net"])) { see(t["layer"].as_str().unwrap_or("?"), f(&t["width"]), json!([t["start"]["x"], t["start"]["y"]])); }
        for a in arcs.iter().filter(|a| same(&a["net"])) { see(a["layer"].as_str().unwrap_or("?"), f(&a["width"]), json!([a["center"]["x"], a["center"]["y"]])); }
        let mut short = Vec::new();
        let layers: Vec<Value> = narrow.into_iter().filter(|(l, _)| l.ends_with(".Cu")).map(|(layer, (w, at))| {
            let i = stack_index(&layer, n).unwrap_or(0);
            let outer = i == 0 || i + 1 == n;
            let cu = copper_um.get(i).copied().filter(|c| *c > 0.0).unwrap_or(35.0);
            let need = width_for(amps, rise, cu, outer);
            if w + 1e-9 < need { short.push(layer.clone()); }
            json!({"layer": layer, "copperUm": cu, "narrowestMm": r3(w), "at": at, "neededMm": r3(need), "ok": w + 1e-9 >= need})
        }).collect();
        let net_vias: Vec<&Value> = vias.iter().filter(|v| same(&v["net"])).collect();
        let smallest = net_vias.iter().map(|v| f(&v["drill"])).fold(f64::MAX, f64::min);
        let via = if net_vias.is_empty() { Value::Null } else {
            let each = via_amps(smallest, plating_um, rise);
            json!({"count": net_vias.len(), "smallestDrillMm": r3(smallest), "ampsEach": r2(each), "neededPerLayerChange": (amps / each).ceil() as i64})
        };
        // each layer once, however many pours of the net it holds (#15)
        let mut pours: Vec<Value> = Vec::new();
        for l in polys.iter().filter(|p| same(&p["net"])).map(|p| p["layer"].clone()) { if !pours.contains(&l) { pours.push(l); } }
        json!({"net": net, "amps": amps, "layers": layers, "shortOn": short, "ok": short.is_empty(), "vias": via, "poursOn": pours})
    }).collect()
}

/// current_capacity: input {board, stack?, args: {nets: {net: amps}, riseC?, platingUm?}}. Without a stack every layer
/// is taken as 35 um (1 oz) copper, and the reply says so.
pub fn current_capacity(input: &Value) -> Value {
    let args = &input["args"];
    let nets = match args.get("nets").and_then(|v| v.as_object()) {
        Some(m) if !m.is_empty() && m.values().all(|v| v.as_f64().map(|a| a > 0.0).unwrap_or(false)) => m.clone(),
        _ => return json!({"success": false, "errorCode": "bad_args", "error": "nets is {net: amps}, e.g. {\"VIN\": 3, \"GND\": 3}"}),
    };
    let rise = args.get("riseC").and_then(|v| v.as_f64()).unwrap_or(10.0);
    let plating = args.get("platingUm").and_then(|v| v.as_f64()).unwrap_or(25.0);
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let missing: Vec<&String> = nets.keys().filter(|k| !st["nets"].as_array().map(|a| a.iter().any(|n| n.as_str().map(|s| s.eq_ignore_ascii_case(k)).unwrap_or(false))).unwrap_or(false)).collect();
    if !missing.is_empty() { return json!({"success": false, "errorCode": "no_such_net", "error": format!("not nets on this board: {missing:?}"), "nets": st["nets"]}); }
    let stack = crate::input::stack(input);
    let copper: Vec<f64> = stack.as_ref().and_then(|s| s["layers"].as_array().cloned()).unwrap_or_default().iter().map(|l| f(&l["copperUm"])).collect();
    let rows = check(&st, &copper, &nets, rise, plating);
    let all_ok = rows.iter().all(|r| r["ok"].as_bool() == Some(true));
    json!({"success": true, "riseC": rise, "platingUm": plating, "nets": rows, "ok": all_ok,
        "assumed": if copper.is_empty() { json!("no stack in the input: every layer taken as 35 um (1 oz) copper") } else { Value::Null },
        "_hint": if all_ok { "Every routed layer of these nets carries its current within the temperature rise (IPC-2221). Vias: a layer change needs neededPerLayerChange of them side by side." }
                 else { "shortOn names the layers whose narrowest copper is under neededMm: widen it (set_track_width, or a width rule with add_rule) or pour the net (apply_pours). A pour on the same layer (poursOn) may already carry the current; the check reads tracks only." }})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_2221_matches_its_chart() {
        // 1 oz (35 um) outer copper, 10 C rise: 1 A needs about 0.30 mm (12 mil), 3 A about 1.4 mm
        let w1 = width_for(1.0, 10.0, 35.0, true);
        assert!((0.25..0.35).contains(&w1), "{w1}");
        let w3 = width_for(3.0, 10.0, 35.0, true);
        assert!((1.2..1.6).contains(&w3), "{w3}");
        // inner layers need about 2.6x the width
        assert!(width_for(1.0, 10.0, 35.0, false) / w1 > 2.4);
        // a 0.3 mm via with 25 um plating carries roughly 1 to 1.5 A at 10 C
        let v = via_amps(0.3, 25.0, 10.0);
        assert!((0.8..1.6).contains(&v), "{v}");
    }

    #[test]
    fn check_finds_the_narrow_layer() {
        let st = json!({"tracks": [{"net": "VIN", "layer": "F.Cu", "width": 1.5, "start": {"x": 0.0, "y": 0.0}}, {"net": "VIN", "layer": "B.Cu", "width": 0.3, "start": {"x": 4.0, "y": 1.0}}],
                        "vias": [{"net": "VIN", "drill": 0.3}, {"net": "VIN", "drill": 0.4}], "polygons": [{"net": "VIN", "layer": "B.Cu"}, {"net": "VIN", "layer": "B.Cu"}]});
        let nets = json!({"VIN": 3.0}).as_object().unwrap().clone();
        let r = &check(&st, &[35.0, 35.0], &nets, 10.0, 25.0)[0];
        assert_eq!(r["shortOn"], json!(["B.Cu"]));
        assert_eq!(r["vias"]["smallestDrillMm"], 0.3);
        assert!(r["vias"]["neededPerLayerChange"].as_i64().unwrap() >= 2);
        assert_eq!(r["poursOn"], json!(["B.Cu"]));
    }

    #[test]
    fn the_verb_says_what_it_assumed_and_refuses_unknown_nets() {
        let st = json!({"nets": ["VIN"], "tracks": [{"net": "VIN", "layer": "F.Cu", "width": 0.2, "start": {"x": 0.0, "y": 0.0}}]});
        let r = current_capacity(&json!({"board": st, "args": {"nets": {"VIN": 2}}}));
        assert_eq!(r["ok"], json!(false));
        assert!(r["assumed"].as_str().unwrap().contains("35 um"));
        assert_eq!(current_capacity(&json!({"board": st, "args": {"nets": {"VOUT": 1}}}))["errorCode"], "no_such_net");
        assert_eq!(current_capacity(&json!({"board": st, "args": {}}))["errorCode"], "bad_args");
    }
}


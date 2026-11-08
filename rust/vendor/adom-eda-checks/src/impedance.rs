//! Controlled impedance from the board's stack-up (the layer_stack companion): the track width (and pair gap) each copper layer needs for a
//! target impedance, by closed-form models (Hammerstad-Jensen microstrip, Wheeler stripline, IPC-2141 coupling). Within
//! a few percent of a field solver for ordinary FR-4 geometry; the fab's stack-up and its solver have the last word.
use serde_json::{json, Value};
use std::f64::consts::PI;

fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }
fn r1(v: f64) -> f64 { (v * 10.0).round() / 10.0 }

/// Free-space impedance of a microstrip of width ratio u (Hammerstad-Jensen).
fn z01(u: f64) -> f64 {
    let f = 6.0 + (2.0 * PI - 6.0) * (-(30.666 / u).powf(0.7528)).exp();
    60.0 * (f / u + (1.0 + 4.0 / (u * u)).sqrt()).ln()
}

fn eeff0(u: f64, er: f64) -> f64 {
    let a = 1.0 + ((u.powi(4) + (u / 52.0).powi(2)) / (u.powi(4) + 0.432)).ln() / 49.0 + (1.0 + (u / 18.1).powi(3)).ln() / 18.7;
    let b = 0.564 * ((er - 0.9) / (er + 3.0)).powf(0.053);
    (er + 1.0) / 2.0 + (er - 1.0) / 2.0 * (1.0 + 10.0 / u).powf(-a * b)
}

/// Uncoated microstrip: width w over a plane h below, copper t thick, dielectric er (any length unit).
pub fn microstrip(w: f64, h: f64, t: f64, er: f64) -> f64 {
    let u = w / h;
    let tn = t / h;
    let coth = |x: f64| x.cosh() / x.sinh();
    let du1 = if tn > 0.0 { tn / PI * (1.0 + 4.0 * std::f64::consts::E / (tn * coth((6.517 * u).sqrt()).powi(2))).ln() } else { 0.0 };
    let dur = 0.5 * (1.0 + 1.0 / (er - 1.0).sqrt().cosh()) * du1;
    let (u1, ur) = (u + du1, u + dur);
    let ee = eeff0(ur, er) * (z01(u1) / z01(ur)).powi(2);
    z01(ur) / ee.sqrt()
}

/// Centred stripline (Wheeler 1978, thickness corrected): width w between planes b apart, copper t.
fn stripline_sym(w: f64, b: f64, t: f64, er: f64) -> f64 {
    let x = t / b;
    let bt = b - t;
    let dw = if x > 0.0 {
        let n = 2.0 / (1.0 + 2.0 * x / (3.0 * (1.0 - x)));
        bt * (x / (PI * (1.0 - x))) * (1.0 - 0.5 * ((x / (2.0 - x)).powi(2) + (0.0796 * x / (w / b + 1.1 * x)).powf(n)).ln())
    } else { 0.0 };
    let m = (w + dw) / bt;
    let k = 8.0 / (PI * m);
    30.0 / er.sqrt() * (1.0 + 4.0 / (PI * m) * (k + (k * k + 6.27).sqrt())).ln()
}

/// Stripline h1 above and h2 below the track (planes h1 + t + h2 apart): Cohn's combination of the two centred cases.
pub fn stripline(w: f64, h1: f64, h2: f64, t: f64, er: f64) -> f64 {
    let za = stripline_sym(w, 2.0 * h1 + t, t, er);
    let zb = stripline_sym(w, 2.0 * h2 + t, t, er);
    2.0 * za * zb / (za + zb)
}

/// Edge-coupled differential impedance (IPC-2141): the single-ended z0, gap s, and h (microstrip) or plane spacing b
/// (stripline).
pub fn differential(z0: f64, s: f64, hb: f64, strip: bool) -> f64 {
    if strip { 2.0 * z0 * (1.0 - 0.374 * (-2.9 * s / hb).exp()) } else { 2.0 * z0 * (1.0 - 0.48 * (-0.96 * s / hb).exp()) }
}

/// The width (same unit as the geometry) where `z(w)` meets `target`; z falls as w grows. None when out of range.
fn solve(target: f64, lo: f64, hi: f64, z: impl Fn(f64) -> f64) -> Option<f64> {
    if z(lo) < target || z(hi) > target { return None; }
    let (mut a, mut b) = (lo, hi);
    for _ in 0..80 {
        let m = (a + b) / 2.0;
        if z(m) > target { a = m } else { b = m }
    }
    Some((a + b) / 2.0)
}

/// One copper layer's geometry from the stack (mm): the dielectric to the copper above and below, and their Dk.
#[derive(Debug, Clone)]
pub struct Geo { pub name: String, pub t: f64, pub above: Option<(f64, f64)>, pub below: Option<(f64, f64)> }

/// The stack summary (`layer_stack` data) as each copper layer's geometry.
pub fn geometry(stack: &Value) -> Vec<Geo> {
    let ls: Vec<Value> = stack["layers"].as_array().cloned().unwrap_or_default();
    let um = |v: &Value| v.as_f64().unwrap_or(0.0) / 1000.0;
    let diel = |l: &Value| -> Option<(f64, f64)> {
        let h = um(&l["dielectricUm"]);
        if h <= 0.0 { None } else { Some((h, l["dk1000"].as_f64().map(|d| d / 1000.0).unwrap_or(4.3))) }
    };
    (0..ls.len()).map(|i| Geo {
        name: ls[i]["name"].as_str().unwrap_or("").to_string(),
        t: um(&ls[i]["copperUm"]).max(0.0),
        above: if i > 0 { diel(&ls[i - 1]) } else { None },
        below: diel(&ls[i]),
    }).collect()
}

/// Width (and gap) for each layer, mm. `diff` is the differential target; `gap` fixes the pair gap (else gap = width).
pub fn widths(geo: &[Geo], z0: Option<f64>, diff: Option<f64>, gap: Option<f64>) -> Vec<Value> {
    geo.iter().map(|g| {
        let (strip, z): (bool, Box<dyn Fn(f64) -> f64>) = match (g.above, g.below) {
            (Some((h1, e1)), Some((h2, e2))) => {
                let er = (e1 * h1 + e2 * h2) / (h1 + h2);
                let t = g.t;
                (true, Box::new(move |w| stripline(w, h1, h2, t, er)))
            }
            (None, Some((h, er))) | (Some((h, er)), None) => { let t = g.t; (false, Box::new(move |w| microstrip(w, h, t, er))) }
            (None, None) => return json!({"layer": g.name, "error": "no dielectric next to this layer in the stack"}),
        };
        // the spacing the coupling term uses: h for microstrip, plane to plane for stripline
        let hb = match (g.above, g.below) { (Some((a, _)), Some((b, _))) => a + b, (Some((h, _)), None) | (None, Some((h, _))) => h, _ => 1.0 };
        let mut o = json!({"layer": g.name, "model": if strip { "stripline" } else { "microstrip" }, "copperUm": r1(g.t * 1000.0),
                           "dielectricMm": [g.above.map(|a| r3(a.0)), g.below.map(|b| r3(b.0))]});
        if let Some(t) = z0 {
            match solve(t, 0.01, 20.0, &z) { Some(w) => { o["width"] = json!(r3(w)); o["z0"] = json!(r1(z(w))); } None => { o["z0Error"] = json!(format!("{t} ohm is out of reach on this layer (0.01 to 20 mm)")); } }
        }
        if let Some(t) = diff {
            let zd = |w: f64| differential(z(w), gap.unwrap_or(w), hb, strip);
            match solve(t, 0.01, 20.0, zd) {
                Some(w) => { o["diffWidth"] = json!(r3(w)); o["diffGap"] = json!(r3(gap.unwrap_or(w))); o["zdiff"] = json!(r1(zd(w))); o["diffZ0"] = json!(r1(z(w))); }
                None => { o["diffError"] = json!(format!("{t} ohm differential is out of reach on this layer with that gap")); }
            }
        }
        o
    }).collect()
}

/// impedance: input {stack, args: {z0?, diff?, gapMm?, layers?, apply?: {scope, name?}}}. Needs only the stack-up.
/// `apply` answers a plan, never writes: one `add_rule` per layer (a width rule, or a diffpair rule for `diff`) scoped
/// by the caller's `scope` in the tool's own query language, at +/-10%, for the bridge to run.
pub fn impedance(input: &Value) -> Value {
    let args = &input["args"];
    let z0 = args.get("z0").and_then(|v| v.as_f64());
    let diff = args.get("diff").and_then(|v| v.as_f64());
    let z0 = if z0.is_none() && diff.is_none() { Some(50.0) } else { z0 };
    let gap = args.get("gapMm").and_then(|v| v.as_f64());
    let Some(stack) = crate::input::stack(input) else { return crate::input::refuse("no_stack", "the input has no stack: pass the layer_stack companion as stack") };
    if stack.get("ok") == Some(&json!(false)) || stack["layers"].as_array().map(|a| a.is_empty()).unwrap_or(true) {
        return crate::input::refuse("no_stack", "the stack has no layers");
    }
    let mut geo = geometry(&stack);
    // each copper layer's board model name beside the tool's own (F.Cu, In1.Cu, B.Cu)
    let board_names: Vec<String> = crate::coppermap::stack_layers(&stack).into_iter().map(|l| l.0).collect();
    let mut named: Vec<(Geo, String)> = geo.drain(..).zip(board_names).collect();
    if let Some(want) = args.get("layers").and_then(|v| v.as_array()) {
        let names: Vec<String> = want.iter().filter_map(|v| v.as_str()).map(|s| s.to_ascii_lowercase()).collect();
        named.retain(|(g, b)| names.contains(&g.name.to_ascii_lowercase()) || names.contains(&b.to_ascii_lowercase()));
    }
    let geo: Vec<Geo> = named.iter().map(|n| n.0.clone()).collect();
    let mut rows = widths(&geo, z0, diff, gap);
    for (row, (_, b)) in rows.iter_mut().zip(&named) { row["boardLayer"] = json!(b); }
    // a rigid-flex stack (the Bluetooth Sentinel example) has a coverlay a few microns thick next to a copper layer:
    // the numbers are right for that geometry and useless for a rigid board, so say which rows to doubt
    let mut warnings = Vec::new();
    for (g, row) in geo.iter().zip(&rows) {
        for d in [g.above, g.below].iter().flatten() {
            if d.0 < 0.05 { warnings.push(format!("{}: a {} mm dielectric next to it (a flex coverlay or a stack-up typo?): check the stack-up in the tool", g.name, r3(d.0))); break; }
        }
        for k in ["width", "diffWidth", "diffGap"] {
            if let Some(w) = row[k].as_f64() { if w < 0.075 { warnings.push(format!("{}: {k} {w} mm is below what most fabs make (0.075 to 0.1 mm)", g.name)); } }
        }
    }
    let mut plan = Value::Null;
    if let Some(ap) = args.get("apply").filter(|a| !a.is_null()) {
        let Some(scope) = ap.get("scope").and_then(|v| v.as_str()) else { return crate::input::refuse("bad_args", "apply.scope is the tool's query for what the rule covers (Altium: InNetClass('RF'); KiCad: A.NetClass == 'RF')") };
        let base = ap.get("name").and_then(|v| v.as_str()).unwrap_or("Impedance");
        let mut rules = Vec::new();
        for row in &rows {
            let layer = row["layer"].as_str().unwrap_or("");
            let common = json!({"name": format!("{base} {layer}"), "scope": scope, "layer": row["boardLayer"], "toolLayer": layer});
            let mut a = if diff.is_some() {
                let (Some(w), Some(s)) = (row["diffWidth"].as_f64(), row["diffGap"].as_f64()) else { continue };
                json!({"kind": "diffpair", "width": w, "gap": s, "minWidth": r3(w * 0.9), "maxWidth": r3(w * 1.1), "minGap": r3(s * 0.9), "maxGap": r3(s * 1.1)})
            } else {
                let Some(w) = row["width"].as_f64() else { continue };
                json!({"kind": "width", "min": r3(w * 0.9), "preferred": w, "max": r3(w * 1.1)})
            };
            for (k, v) in common.as_object().unwrap() { a[k] = v.clone(); }
            rules.push(a);
        }
        plan = json!({"verb": "add_rule", "rules": rules, "note": "A plan only: nothing changed. Run add_rule once per rule; each is scoped to its layer and to scope."});
    }
    json!({"success": true, "targets": {"z0": z0, "diff": diff, "gapMm": gap}, "layers": rows, "apply": plan, "warnings": warnings,
        "stack": {"copperLayers": stack["copperLayers"], "totalUm": stack["totalUm"]},
        "_hint": "Widths are for uncoated copper: solder mask lowers an outer layer's impedance by about 1 to 3 ohm. Inner layers assume planes on both neighbours. Within a few percent of a field solver; send the fab the target and let its stack-up tune it. apply:{scope} answers one width or diffpair rule per layer at +/-10% for add_rule."})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microstrip_matches_the_textbook_50_ohm_line() {
        // FR-4 er 4.4, 1.6 mm, 35 um: a 50 ohm line is about 3.0 mm wide (Pozar, field solvers 2.9 to 3.1)
        let w = solve(50.0, 0.01, 20.0, |w| microstrip(w, 1.6, 0.035, 4.4)).unwrap();
        assert!((2.8..3.2).contains(&w), "{w}");
    }

    #[test]
    fn stripline_matches_the_textbook_case() {
        // centred stripline, planes 1 mm apart, er 4.3, t 17 um: IPC-2141's formula puts 50 ohm near 0.40 mm
        let w = solve(50.0, 0.01, 20.0, |w| stripline(w, 0.4915, 0.4915, 0.017, 4.3)).unwrap();
        assert!((0.38..0.48).contains(&w), "{w}");
        // off centre, the nearer plane pulls the impedance down
        assert!(stripline(0.3, 0.2, 0.8, 0.017, 4.3) < stripline(0.3, 0.5, 0.5, 0.017, 4.3));
    }

    #[test]
    fn geometry_and_widths_from_a_two_layer_stack() {
        let st = json!({"layers": [{"name": "Top Layer", "copperUm": 36, "dielectricUm": 320, "dk1000": 4800}, {"name": "Bottom Layer", "copperUm": 36}]});
        let g = geometry(&st);
        assert_eq!(g.len(), 2);
        assert!(g[1].above.is_some() && g[1].below.is_none());
        let rows = widths(&g, Some(50.0), Some(90.0), None);
        let w = rows[0]["width"].as_f64().unwrap();
        assert!((0.45..0.65).contains(&w), "{w}");
        assert_eq!(rows[0]["model"], "microstrip");
        // gap = width: a 90 ohm pair's tracks come out near the 50 ohm width
        assert!((rows[0]["diffWidth"].as_f64().unwrap() - w).abs() < 0.1);
        assert!(widths(&g, None, Some(90.0), Some(0.2))[0]["diffWidth"].as_f64().unwrap() < rows[0]["diffWidth"].as_f64().unwrap());
        assert!((rows[0]["zdiff"].as_f64().unwrap() - 90.0).abs() < 0.5);
    }

    #[test]
    fn apply_is_a_plan_of_add_rule_args_on_standard_layers() {
        let input = json!({"stack": {"copperLayers": 2, "layers": [{"name": "Top Layer", "copperUm": 36, "dielectricUm": 320, "dk1000": 4800}, {"name": "Bottom Layer", "copperUm": 36}]},
                           "args": {"z0": 50, "apply": {"scope": "InNetClass('RF')"}}});
        let r = impedance(&input);
        assert_eq!(r["layers"][0]["boardLayer"], "F.Cu");
        assert_eq!(r["apply"]["verb"], "add_rule");
        let rule = &r["apply"]["rules"][1];
        assert_eq!((rule["kind"].as_str(), rule["layer"].as_str(), rule["toolLayer"].as_str()), (Some("width"), Some("B.Cu"), Some("Bottom Layer")));
        assert_eq!(rule["scope"], "InNetClass('RF')");
        // a filter by either name
        let only = impedance(&json!({"stack": input["stack"], "args": {"layers": ["B.Cu"]}}));
        assert_eq!(only["layers"].as_array().unwrap().len(), 1);
        assert_eq!(impedance(&json!({"args": {}}))["errorCode"], "no_stack");
        assert_eq!(impedance(&json!({"stack": input["stack"], "args": {"apply": {}}}))["errorCode"], "bad_args");
    }
}


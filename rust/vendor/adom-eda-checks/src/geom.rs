//! Board geometry shared by the checks: the outline's bounding box with its arc edges sampled.
use serde_json::Value;

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }

/// The outline's bounding box (x0, y0, x1, y1) in mm, arc edges included.
pub fn outline_bbox(state: &Value) -> Option<(f64, f64, f64, f64)> {
    let mut o: Vec<(f64, f64)> = state["outline"].as_array().cloned().unwrap_or_default().iter().map(|p| (f(&p[0]), f(&p[1]))).collect();
    // an arc edge bulges past its end points: sample it (the Bluetooth Sentinel example measured 80 x 4.5 mm by its
    // vertices alone)
    for e in state["outlineEdges"].as_array().cloned().unwrap_or_default() {
        if e["kind"].as_str() != Some("arc") { continue; }
        let (cx, cy, r) = (f(&e["center"][0]), f(&e["center"][1]), f(&e["radius"]));
        let (a1, mut a2) = (f(&e["angle1"]), f(&e["angle2"]));
        if a2 <= a1 { a2 += 360.0; }
        let mut a = a1;
        while a <= a2 + 1e-9 { o.push((cx + r * a.to_radians().cos(), cy + r * a.to_radians().sin())); a += 2.0; }
    }
    if o.is_empty() { return None; }
    let (x0, x1) = o.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
    let (y0, y1) = o.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
    Some((x0, y0, x1, y1))
}

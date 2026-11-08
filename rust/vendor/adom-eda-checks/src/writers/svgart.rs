//! SVG artwork (a logo, a label) as filled regions for a board layer. The SVG's filled shapes are flattened to polygons
//! (paths with arcs and curves, rects, circles, ellipses, polygons, transforms), a shape inside another becomes its
//! hole, and each shape with its holes is ONE contour that reaches every hole through a zero-width slit: Altium printed a
//! region's separate hole contour filled, the slit form prints open (Develop, 2026-10-28). Text, images and stroke-only
//! shapes are listed as skipped. No I/O: the bridge places the regions.
use serde_json::{json, Value};
use std::str::FromStr;

type Pt = (f64, f64);

#[derive(Clone, Copy)]
struct Tf { a: f64, b: f64, c: f64, d: f64, e: f64, f: f64 }

impl Tf {
    fn id() -> Self { Tf { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 } }
    /// self, then t applied inside it (SVG nesting: the parent's transform is outermost)
    fn then(&self, t: &Tf) -> Tf {
        Tf { a: self.a * t.a + self.c * t.b, b: self.b * t.a + self.d * t.b, c: self.a * t.c + self.c * t.d, d: self.b * t.c + self.d * t.d,
             e: self.a * t.e + self.c * t.f + self.e, f: self.b * t.e + self.d * t.f + self.f }
    }
    fn ap(&self, p: Pt) -> Pt { (self.a * p.0 + self.c * p.1 + self.e, self.b * p.0 + self.d * p.1 + self.f) }
    fn scale(&self) -> f64 { (self.a * self.d - self.b * self.c).abs().sqrt().max(1e-12) }
}

/// One element's style value, from its attribute or its style="" (the style wins).
fn style<'a>(n: &roxmltree::Node<'a, 'a>, key: &str) -> Option<String> {
    if let Some(s) = n.attribute("style") {
        for decl in s.split(';') {
            if let Some((k, v)) = decl.split_once(':') { if k.trim() == key { return Some(v.trim().to_string()); } }
        }
    }
    n.attribute(key).map(|v| v.trim().to_string())
}

/// The inherited value of a style property (fill, display, visibility...).
fn inherited(n: &roxmltree::Node, key: &str) -> Option<String> {
    let mut cur = Some(*n);
    while let Some(x) = cur {
        if x.is_element() { if let Some(v) = style(&x, key) { if v != "inherit" { return Some(v); } } }
        cur = x.parent();
    }
    None
}

fn num(n: &roxmltree::Node, k: &str) -> f64 {
    n.attribute(k).and_then(|v| svgtypes::Length::from_str(v).ok()).map(|l| l.number).unwrap_or(0.0)
}

/// Points along a cubic or quadratic curve, flattened to `tol` in the SVG's own units.
fn flatten_cubic(out: &mut Vec<Pt>, p0: Pt, p1: Pt, p2: Pt, p3: Pt, tol: f64) {
    let d = ((p1.0 - p0.0).hypot(p1.1 - p0.1) + (p2.0 - p1.0).hypot(p2.1 - p1.1) + (p3.0 - p2.0).hypot(p3.1 - p2.1)).max(1e-9);
    let n = ((d / tol).sqrt().ceil() as usize).clamp(2, 200);
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let u = 1.0 - t;
        out.push((u * u * u * p0.0 + 3.0 * u * u * t * p1.0 + 3.0 * u * t * t * p2.0 + t * t * t * p3.0,
                  u * u * u * p0.1 + 3.0 * u * u * t * p1.1 + 3.0 * u * t * t * p2.1 + t * t * t * p3.1));
    }
}

/// A path's closed subpaths, flattened. An open subpath is closed (SVG fills it as if it were).
fn path_rings(d: &str, tol: f64) -> Result<Vec<Vec<Pt>>, String> {
    let mut rings = Vec::new();
    let mut cur: Vec<Pt> = Vec::new();
    let mut last = (0.0, 0.0);
    for seg in svgtypes::SimplifyingPathParser::from(d) {
        match seg.map_err(|e| e.to_string())? {
            svgtypes::SimplePathSegment::MoveTo { x, y } => {
                if cur.len() > 2 { rings.push(std::mem::take(&mut cur)); } else { cur.clear(); }
                cur.push((x, y)); last = (x, y);
            }
            svgtypes::SimplePathSegment::LineTo { x, y } => { cur.push((x, y)); last = (x, y); }
            svgtypes::SimplePathSegment::CurveTo { x1, y1, x2, y2, x, y } => { flatten_cubic(&mut cur, last, (x1, y1), (x2, y2), (x, y), tol); last = (x, y); }
            svgtypes::SimplePathSegment::Quadratic { x1, y1, x, y } => {
                let c1 = (last.0 + 2.0 / 3.0 * (x1 - last.0), last.1 + 2.0 / 3.0 * (y1 - last.1));
                let c2 = (x + 2.0 / 3.0 * (x1 - x), y + 2.0 / 3.0 * (y1 - y));
                flatten_cubic(&mut cur, last, c1, c2, (x, y), tol); last = (x, y);
            }
            svgtypes::SimplePathSegment::ClosePath => {
                if cur.len() > 2 { last = cur[0]; rings.push(std::mem::take(&mut cur)); }
            }
        }
    }
    if cur.len() > 2 { rings.push(cur); }
    Ok(rings)
}

fn ellipse_ring(cx: f64, cy: f64, rx: f64, ry: f64, tol: f64) -> Vec<Pt> {
    let n = ((std::f64::consts::PI * (rx + ry) / (8.0 * tol).sqrt().max(1e-6)).ceil() as usize).clamp(16, 360);
    (0..n).map(|i| { let a = 2.0 * std::f64::consts::PI * i as f64 / n as f64; (cx + rx * a.cos(), cy + ry * a.sin()) }).collect()
}

fn rect_ring(x: f64, y: f64, w: f64, h: f64, rx: f64, ry: f64, tol: f64) -> Vec<Pt> {
    if rx <= 0.0 && ry <= 0.0 { return vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)]; }
    let (rx, ry) = (if rx > 0.0 { rx } else { ry }.min(w / 2.0), if ry > 0.0 { ry } else { rx }.min(h / 2.0));
    let mut out = Vec::new();
    let corners = [((x + w - rx, y + ry), -90.0), ((x + w - rx, y + h - ry), 0.0), ((x + rx, y + h - ry), 90.0), ((x + rx, y + ry), 180.0)];
    let n = ((rx.max(ry) / tol).sqrt().ceil() as usize).clamp(3, 24);
    for (c, a0) in corners {
        for i in 0..=n {
            let a = (a0 + 90.0 * i as f64 / n as f64).to_radians();
            out.push((c.0 + rx * a.cos(), c.1 + ry * a.sin()));
        }
    }
    out
}

fn point_list(s: &str) -> Vec<Pt> {
    let v: Vec<f64> = svgtypes::NumberListParser::from(s).filter_map(|n| n.ok()).collect();
    v.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).collect()
}

pub struct Art { pub rings: Vec<Vec<Pt>>, pub skipped: Vec<Value>, pub mm_per_unit: Option<f64> }

/// Every filled shape of the SVG as rings in the root's user units (y down), transforms applied.
pub fn parse(text: &str, tol_units: f64) -> Result<Art, String> {
    let doc = roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).map_err(|e| format!("not an SVG: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" { return Err("the document's root is not <svg>".into()); }
    // the size of one user unit in mm, when the SVG says it (width="40mm" viewBox="0 0 400 200")
    let mm_per_unit = root.attribute("width").and_then(|w| svgtypes::Length::from_str(w).ok()).and_then(|l| {
        let mm = match l.unit {
            svgtypes::LengthUnit::Mm => l.number, svgtypes::LengthUnit::Cm => l.number * 10.0, svgtypes::LengthUnit::In => l.number * 25.4,
            svgtypes::LengthUnit::Pt => l.number * 25.4 / 72.0, svgtypes::LengthUnit::Pc => l.number * 25.4 / 6.0,
            svgtypes::LengthUnit::Px | svgtypes::LengthUnit::None => l.number * 25.4 / 96.0,
            _ => return None,
        };
        let vbw = root.attribute("viewBox").map(|v| point_list(v)).filter(|v| v.len() == 2).map(|v| v[1].0).unwrap_or(l.number);
        (vbw > 0.0).then_some(mm / vbw)
    });
    let mut art = Art { rings: Vec::new(), skipped: Vec::new(), mm_per_unit };
    let mut stack: Vec<(roxmltree::Node, Tf)> = vec![(root, Tf::id())];
    while let Some((n, tf)) = stack.pop() {
        for c in n.children().filter(|c| c.is_element()) {
            let name = c.tag_name().name();
            let mut t = tf;
            if let Some(s) = c.attribute("transform") {
                match svgtypes::Transform::from_str(s) {
                    Ok(x) => t = tf.then(&Tf { a: x.a, b: x.b, c: x.c, d: x.d, e: x.e, f: x.f }),
                    Err(e) => { art.skipped.push(json!({"element": name, "why": format!("transform not read: {e}")})); continue; }
                }
            }
            if style(&c, "display").as_deref() == Some("none") { continue; }
            match name {
                "g" | "a" | "switch" => { stack.push((c, t)); continue; }
                "defs" | "clipPath" | "mask" | "pattern" | "symbol" | "marker" | "title" | "desc" | "metadata" | "style" | "linearGradient" | "radialGradient" | "filter" => continue,
                "text" | "image" | "use" | "foreignObject" => { art.skipped.push(json!({"element": name, "why": match name { "text" => "text: convert it to paths first", "image" => "a bitmap image", _ => "not imported" }})); continue; }
                "line" => { art.skipped.push(json!({"element": name, "why": "a line has no area"})); continue; }
                _ => {}
            }
            if inherited(&c, "visibility").as_deref() == Some("hidden") { continue; }
            if inherited(&c, "fill").as_deref() == Some("none") {
                art.skipped.push(json!({"element": name, "id": c.attribute("id"), "why": "no fill (stroke only): give the SVG filled shapes"}));
                continue;
            }
            // flatten in the element's own units, to the tolerance asked in the root's units
            let tol = (tol_units / t.scale()).max(1e-6);
            let local: Vec<Vec<Pt>> = match name {
                "path" => match path_rings(c.attribute("d").unwrap_or(""), tol) { Ok(r) => r, Err(e) => { art.skipped.push(json!({"element": name, "why": format!("path data not read: {e}")})); continue; } },
                "rect" => vec![rect_ring(num(&c, "x"), num(&c, "y"), num(&c, "width"), num(&c, "height"), num(&c, "rx"), num(&c, "ry"), tol)],
                "circle" => vec![ellipse_ring(num(&c, "cx"), num(&c, "cy"), num(&c, "r"), num(&c, "r"), tol)],
                "ellipse" => vec![ellipse_ring(num(&c, "cx"), num(&c, "cy"), num(&c, "rx"), num(&c, "ry"), tol)],
                "polygon" | "polyline" => vec![point_list(c.attribute("points").unwrap_or(""))],
                other => { art.skipped.push(json!({"element": other, "why": "not a shape this import reads"})); continue; }
            };
            for r in local.into_iter().filter(|r| r.len() > 2) {
                art.rings.push(r.into_iter().map(|p| t.ap(p)).collect());
            }
        }
    }
    Ok(art)
}

fn area(r: &[Pt]) -> f64 {
    let n = r.len();
    (0..n).map(|i| { let (a, b) = (r[i], r[(i + 1) % n]); a.0 * b.1 - b.0 * a.1 }).sum::<f64>() / 2.0
}

fn inside(p: Pt, r: &[Pt]) -> bool {
    let mut c = false;
    let n = r.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (r[i], r[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 { c = !c; }
        j = i;
    }
    c
}

fn cross(o: Pt, a: Pt, b: Pt) -> f64 { (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0) }

/// Do segments ab and cd cross (touching at an end does not count)?
fn crosses(a: Pt, b: Pt, c: Pt, d: Pt) -> bool {
    let (d1, d2, d3, d4) = (cross(c, d, a), cross(c, d, b), cross(a, b, c), cross(a, b, d));
    ((d1 > 1e-12 && d2 < -1e-12) || (d1 < -1e-12 && d2 > 1e-12)) && ((d3 > 1e-12 && d4 < -1e-12) || (d3 < -1e-12 && d4 > 1e-12))
}

/// Group rings into shapes with holes by nesting (a ring inside an odd number of others is a hole of its tightest
/// container), then make each shape one contour that reaches its holes through zero-width slits.
pub fn contours(rings: &[Vec<Pt>]) -> Vec<Vec<Pt>> {
    let n = rings.len();
    let depth: Vec<usize> = (0..n).map(|i| (0..n).filter(|&j| j != i && inside(rings[i][0], &rings[j])).count()).collect();
    let mut out = Vec::new();
    for o in (0..n).filter(|&i| depth[i] % 2 == 0) {
        // counter-clockwise outside, clockwise holes
        let mut c = rings[o].clone();
        if area(&c) < 0.0 { c.reverse(); }
        let mut holes: Vec<Vec<Pt>> = (0..n).filter(|&h| depth[h] == depth[o] + 1 && inside(rings[h][0], &rings[o])
            && (0..n).all(|k| k == o || k == h || depth[k] != depth[o] + 1 || !inside(rings[h][0], &rings[k]) || depth[k] != depth[h]))
            .map(|h| { let mut r = rings[h].clone(); if area(&r) > 0.0 { r.reverse(); } r }).collect();
        // the leftmost holes first, each joined to the nearest point of the contour so far that it can see
        holes.sort_by(|a, b| a.iter().map(|p| p.0).fold(f64::MAX, f64::min).partial_cmp(&b.iter().map(|p| p.0).fold(f64::MAX, f64::min)).unwrap());
        for (hi, h) in holes.iter().enumerate() {
            let (hv, _) = h.iter().enumerate().min_by(|a, b| a.1 .0.partial_cmp(&b.1 .0).unwrap()).unwrap();
            let hp = h[hv];
            let others: Vec<&Vec<Pt>> = holes.iter().enumerate().filter(|(k, _)| *k > hi).map(|(_, r)| r).collect();
            let visible = |q: Pt| {
                let segs = |r: &[Pt]| (0..r.len()).any(|i| crosses(hp, q, r[i], r[(i + 1) % r.len()]));
                !segs(&c) && !segs(h) && !others.iter().any(|r| segs(r))
            };
            let mut best: Option<(usize, f64)> = None;
            for (i, q) in c.iter().enumerate() {
                let d = (q.0 - hp.0).hypot(q.1 - hp.1);
                if best.map(|b| d < b.1).unwrap_or(true) && visible(*q) { best = Some((i, d)); }
            }
            let Some((ci, _)) = best else { continue };
            let mut merged: Vec<Pt> = c[..=ci].to_vec();
            merged.extend(h[hv..].iter().chain(h[..=hv].iter()).copied());
            merged.push(c[ci]);
            merged.extend_from_slice(&c[ci + 1..]);
            c = merged;
        }
        out.push(c);
    }
    out
}

/// writers::svg_to_regions: `{svg, args: {at: [x, y] mm, widthMm?, scale?, anchor?: topleft|center, layer?, mirror?,
/// toleranceMm?}}` -> the regions in board mm (y up), each one closed contour with its holes slit in.
pub fn svg_to_regions(input: &Value) -> Value {
    let bad = |m: &str| crate::input::refuse("bad_args", m);
    let args = input.get("args").cloned().unwrap_or(json!({}));
    let Some(text) = input.get("svg").and_then(|v| v.as_str()).or_else(|| args.get("svg").and_then(|v| v.as_str())) else { return bad("svg (the SVG text) is required") };
    let Some(at) = args.get("at").and_then(|v| v.as_array()).filter(|a| a.len() == 2).and_then(|a| Some((a[0].as_f64()?, a[1].as_f64()?))) else { return bad("args.at [x, y] in mm is required") };
    let layer = args.get("layer").and_then(|v| v.as_str()).unwrap_or("F.SilkS").to_string();
    let tol_mm = args.get("toleranceMm").and_then(|v| v.as_f64()).unwrap_or(0.02).max(0.001);
    // a first pass in the SVG's own units learns the size, then it is flattened again to the tolerance in mm
    let probe = match parse(text, 0.5) { Ok(a) => a, Err(e) => return crate::input::refuse("bad_svg", &e) };
    let pts: Vec<Pt> = probe.rings.iter().flatten().copied().collect();
    if pts.is_empty() {
        let mut r = crate::input::refuse("nothing_to_import", "the SVG has no filled shape");
        r["skipped"] = json!(probe.skipped);
        r["_hint"] = json!("Filled shapes import; stroke-only artwork and text do not (convert text and strokes to filled paths first).");
        return r;
    }
    let (x0, y0, x1, y1) = pts.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, p| (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1)));
    let (w, h) = ((x1 - x0).max(1e-9), (y1 - y0).max(1e-9));
    let scale = args.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let k = match args.get("widthMm").and_then(|v| v.as_f64()) {
        Some(wm) if wm > 0.0 => wm / w,
        _ => probe.mm_per_unit.unwrap_or(25.4 / 96.0) * scale,
    };
    let art = match parse(text, tol_mm / k) { Ok(a) => a, Err(e) => return crate::input::refuse("bad_svg", &e) };
    let mirror = args.get("mirror").and_then(|v| v.as_bool()).unwrap_or(layer.starts_with("B."));
    let center = args.get("anchor").and_then(|v| v.as_str()) == Some("center");
    let (wm, hm) = (w * k, h * k);
    // SVG y runs down, the board's up; the artwork's top-left (or centre) lands on at
    let to_board = |p: Pt| -> Pt {
        let u = if mirror { x1 - p.0 } else { p.0 - x0 } * k;
        let v = (p.1 - y0) * k;
        if center { (at.0 - wm / 2.0 + u, at.1 + hm / 2.0 - v) } else { (at.0 + u, at.1 - v) }
    };
    let rings: Vec<Vec<Pt>> = art.rings.iter().map(|r| r.iter().map(|p| to_board(*p)).collect()).collect();
    let shapes = contours(&rings);
    let r4 = |v: f64| (v * 10000.0).round() / 10000.0;
    json!({"success": true, "regions": shapes.iter().map(|s| json!({"layer": layer, "points": s.iter().map(|p| json!([r4(p.0), r4(p.1)])).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "polygons": shapes.len(), "holes": rings.len() - shapes.len(), "points": shapes.iter().map(|s| s.len()).sum::<usize>(),
        "sizeMm": [r4(wm), r4(hm)], "layer": layer, "mirror": mirror, "skipped": art.skipped, "writtenFrom": "svg",
        "_hint": "Each region is one closed contour in board mm; its holes are reached through zero-width slits, so a tool that fills a separate hole contour still prints them open."})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_with_a_square_hole_is_one_contour_with_a_slit() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="40mm" viewBox="0 0 40 40">
            <path d="M0 0 H40 V40 H0 Z M10 10 V30 H30 V10 Z"/></svg>"#;
        let art = parse(svg, 0.01).unwrap();
        assert_eq!(art.rings.len(), 2);
        assert_eq!(art.mm_per_unit, Some(1.0));
        let c = contours(&art.rings);
        assert_eq!(c.len(), 1);
        // 4 outside, 4 + 1 around the hole, back to the outside: 10 points; the area is 40^2 - 20^2
        assert_eq!(c[0].len(), 10);
        assert!((area(&c[0]).abs() - 1200.0).abs() < 1e-6, "{}", area(&c[0]));
    }

    #[test]
    fn stroke_only_and_text_are_skipped_with_a_reason() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><g fill="none" stroke="black"><path d="M0 0 L10 10"/></g>
            <text>Hi</text><circle cx="5" cy="5" r="2"/></svg>"#;
        let art = parse(svg, 0.01).unwrap();
        assert_eq!(art.rings.len(), 1);
        assert_eq!(art.skipped.len(), 2);
    }

    #[test]
    fn transforms_and_arcs_are_applied() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><g transform="translate(100 0) scale(2)">
            <path d="M0 0 A5 5 0 0 1 10 0 Z"/></g></svg>"#;
        let art = parse(svg, 0.001).unwrap();
        let xs: Vec<f64> = art.rings[0].iter().map(|p| p.0).collect();
        assert!(xs.iter().cloned().fold(f64::MAX, f64::min) >= 99.999 && xs.iter().cloned().fold(f64::MIN, f64::max) <= 120.001);
        // a half circle of radius 10 after the scale
        assert!((area(&art.rings[0]).abs() - std::f64::consts::PI * 50.0).abs() < 0.5, "{}", area(&art.rings[0]));
    }

    #[test]
    fn the_regions_land_on_the_board_with_y_up() {
        let r = svg_to_regions(&json!({"svg": r#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="10mm" viewBox="0 0 20 10"><rect x="0" y="0" width="20" height="10"/></svg>"#,
            "args": {"at": [5.0, 15.0]}}));
        assert_eq!(r["polygons"], 1);
        assert_eq!(r["sizeMm"], json!([20.0, 10.0]));
        let ys: Vec<f64> = r["regions"][0]["points"].as_array().unwrap().iter().map(|p| p[1].as_f64().unwrap()).collect();
        // the top-left at (5, 15): the artwork runs down to y = 5
        assert_eq!(ys.iter().cloned().fold(f64::MIN, f64::max), 15.0);
        assert_eq!(ys.iter().cloned().fold(f64::MAX, f64::min), 5.0);
        // a bottom layer mirrors it by default
        let b = svg_to_regions(&json!({"svg": "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"4\" height=\"2\"/></svg>", "args": {"at": [0, 0], "layer": "B.SilkS"}}));
        assert_eq!(b["mirror"], true);
        assert_eq!(svg_to_regions(&json!({"svg": "<svg xmlns=\"http://www.w3.org/2000/svg\"/>", "args": {"at": [0, 0]}}))["errorCode"], "nothing_to_import");
    }

    #[test]
    fn letter_o_holes_go_to_their_own_letter() {
        // two rings each with a hole: two contours, each reaching only its own hole
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <path d="M0 0 H10 V10 H0 Z M3 3 V7 H7 V3 Z"/><path d="M20 0 H30 V10 H20 Z M23 3 V7 H27 V3 Z"/></svg>"#;
        let c = contours(&parse(svg, 0.01).unwrap().rings);
        assert_eq!(c.len(), 2);
        for s in &c { assert!((area(s).abs() - 84.0).abs() < 1e-6); }
    }
}

//! generate_body (EDA Bridge Standard #20, from the Altium bridge's body3d.rs): a part's 3D body from its package family and datasheet dimensions, to IPC-7351 proportions, written
//! as a STEP file (AP214 boundary representation, mm, coloured body and leads). Pure Rust: no CAD tool is needed.
//! Bodies are made of prisms (a planar outline swept along an axis), which is all these packages need; a round can or
//! ball is a many-sided prism. The package sits in its own frame: IPC-7351 zero orientation, seated on z = 0. No I/O:
//! the reply carries the STEP text, and the bridge writes the file.

use serde_json::{json, Map, Value};

type P3 = [f64; 3];

/// A solid as its faces, each a planar polygon listed counter-clockwise seen from outside.
#[derive(Clone)]
pub struct Solid { pub faces: Vec<Vec<P3>>, pub lead: bool, pub mark: bool }

/// A prism: `outline` (counter-clockwise in its own u,v plane) swept from w0 to w1, placed by `map(u, v, w)`.
fn prism(outline: &[(f64, f64)], w0: f64, w1: f64, lead: bool, map: impl Fn(f64, f64, f64) -> P3) -> Solid {
    let n = outline.len();
    let bot: Vec<P3> = outline.iter().rev().map(|&(u, v)| map(u, v, w0)).collect();
    let top: Vec<P3> = outline.iter().map(|&(u, v)| map(u, v, w1)).collect();
    let mut faces = vec![bot, top];
    for i in 0..n {
        let (a, b) = (outline[i], outline[(i + 1) % n]);
        faces.push(vec![map(a.0, a.1, w0), map(b.0, b.1, w0), map(b.0, b.1, w1), map(a.0, a.1, w1)]);
    }
    let mut s = Solid { faces, lead, mark: false };
    // a map that mirrors (an odd number of axis flips) turns every face inside out: put them right
    if signed_volume(&s) < 0.0 { for f in &mut s.faces { f.reverse(); } }
    s
}

fn signed_volume(s: &Solid) -> f64 {
    s.faces.iter().map(|f| (1..f.len() - 1).map(|i| { let (a, b, c) = (f[0], f[i], f[i + 1]); a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0]) }).sum::<f64>()).sum::<f64>() / 6.0
}

fn rect(w: f64, h: f64) -> Vec<(f64, f64)> { vec![(-w / 2.0, -h / 2.0), (w / 2.0, -h / 2.0), (w / 2.0, h / 2.0), (-w / 2.0, h / 2.0)] }
fn ngon(r: f64, n: usize) -> Vec<(f64, f64)> { (0..n).map(|i| { let a = 2.0 * std::f64::consts::PI * (i as f64 + 0.5) / n as f64; (r * a.cos(), r * a.sin()) }).collect() }

/// A box from (x0, y0, z0) to (x1, y1, z1).
fn bx(x0: f64, y0: f64, z0: f64, x1: f64, y1: f64, z1: f64, lead: bool) -> Solid {
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    prism(&rect((x1 - x0).abs(), (y1 - y0).abs()), z0, z1, lead, |u, v, w| [cx + u, cy + v, w])
}

/// A gull-wing lead running outward along +x from the body edge at x = x0 to the tip at x = x1, centred on y, width b,
/// thickness t, shoulder at height zs, foot of length l on the seating plane.
fn gull(x0: f64, x1: f64, y: f64, b: f64, t: f64, zs: f64, l: f64, sign: f64) -> Solid {
    let knee = (x1 - l).max(x0 + t * 1.5);
    let mid = x0 + (knee - x0) * 0.5;
    // the profile in (x, z), counter-clockwise: under the shoulder, down the leg, along the foot, and back over
    let prof = vec![(x0, zs), (mid, zs), (knee, 0.0), (x1, 0.0), (x1, t), (knee + t * 0.4, t), (mid + t * 0.4, zs + t), (x0, zs + t)];
    prism(&prof, y - b / 2.0, y + b / 2.0, true, |u, v, w| [sign * u, w, v])
}

fn d(dims: &Map<String, Value>, k: &str, def: f64) -> f64 { dims.get(k).and_then(|v| v.as_f64()).unwrap_or(def) }

/// The families and their default dimensions (JEDEC letters, mm), from common parts. `pins` is the total pin count in
/// every family that has a choice; D2 and E2 are a QFN or DFN's exposed pad, drawn when both are given: an 0603 chip, a SOIC-8,
/// a QFN-16 4x4, a TQFP-32 7x7, a SOT-23, a DFN-8 3x3, a BGA-16 0.8 mm, a MiniMELF, a 6.3 mm can, a 3.2x2.5 crystal,
/// a 1x4 2.54 mm header.
pub fn families() -> Vec<(&'static str, Value)> {
    vec![
        ("chip", json!({"D": 1.6, "E": 0.8, "A": 0.45, "L": 0.3})),
        ("soic", json!({"pins": 8, "D": 4.9, "E": 6.0, "E1": 3.9, "A": 1.75, "A1": 0.15, "b": 0.42, "e": 1.27, "L": 0.6, "c": 0.2})),
        ("qfn", json!({"pins": 16, "D": 4.0, "E": 4.0, "A": 0.9, "b": 0.3, "e": 0.65, "L": 0.4, "D2": 2.1, "E2": 2.1})),
        ("qfp", json!({"pins": 32, "D": 9.0, "E": 9.0, "D1": 7.0, "E1": 7.0, "A": 1.2, "A1": 0.1, "b": 0.37, "e": 0.8, "L": 0.6, "c": 0.15})),
        ("sot23", json!({"pins": 3, "D": 2.9, "E": 2.4, "E1": 1.3, "A": 1.0, "A1": 0.05, "b": 0.4, "e": 0.95, "L": 0.4, "c": 0.12})),
        ("dfn", json!({"pins": 8, "D": 3.0, "E": 3.0, "A": 0.85, "b": 0.3, "e": 0.65, "L": 0.4, "D2": 2.4, "E2": 1.6})),
        ("bga", json!({"rows": 4, "cols": 4, "D": 4.0, "E": 4.0, "A": 1.2, "A1": 0.25, "b": 0.4, "e": 0.8})),
        ("melf", json!({"D": 3.5, "E": 1.4, "L": 0.45})),
        ("ecap", json!({"D": 6.3, "A": 5.4, "E": 6.6})),
        ("crystal", json!({"D": 3.2, "E": 2.5, "A": 0.8, "b": 1.2, "L": 1.0})),
        ("header", json!({"pins": 4, "rows": 1, "e": 2.54, "A": 2.5, "L": 3.0, "L1": 6.0, "b": 0.64})),
    ]
}

/// The solids of a package: its body (and leads) in the IPC-7351 zero orientation, pin 1 top-left.
pub fn package(family: &str, dims: &Map<String, Value>) -> Option<Vec<Solid>> {
    let mut s = Vec::new();
    let pins = |def: f64| d(dims, "pins", def).round().max(1.0) as usize;
    match family {
        "chip" => {
            let (len, w, h, l) = (d(dims, "D", 1.6), d(dims, "E", 0.8), d(dims, "A", 0.45), d(dims, "L", 0.3));
            s.push(bx(-len / 2.0 + l, -w / 2.0, 0.0, len / 2.0 - l, w / 2.0, h, false));
            s.push(bx(-len / 2.0, -w / 2.0, 0.0, -len / 2.0 + l, w / 2.0, h, true));
            s.push(bx(len / 2.0 - l, -w / 2.0, 0.0, len / 2.0, w / 2.0, h, true));
        }
        "soic" | "sot23" | "qfp" => {
            let n = pins(if family == "sot23" { 3.0 } else { 8.0 });
            let (a, a1, b, e, l, c) = (d(dims, "A", 1.75), d(dims, "A1", 0.1), d(dims, "b", 0.42), d(dims, "e", 1.27), d(dims, "L", 0.6), d(dims, "c", 0.2));
            let zs = a1 + (a - a1) * 0.4;
            if family == "qfp" {
                let (dd, ee, d1, e1) = (d(dims, "D", 9.0), d(dims, "E", 9.0), d(dims, "D1", 7.0), d(dims, "E1", 7.0));
                s.push(bx(-e1 / 2.0, -d1 / 2.0, a1, e1 / 2.0, d1 / 2.0, a, false));
                let per = (n / 4).max(1);
                let span = (per - 1) as f64 * e;
                for i in 0..per {
                    let off = span / 2.0 - i as f64 * e;
                    // left (pins 1..), bottom, right, top: counter-clockwise from pin 1 top-left
                    s.push(gull(e1 / 2.0, ee / 2.0, off, b, c, zs, l, -1.0));
                    let mut bt = gull(d1 / 2.0, dd / 2.0, -off, b, c, zs, l, 1.0);
                    for f in &mut bt.faces { for p in f.iter_mut() { *p = [-p[1], -p[0], p[2]]; } f.reverse(); }
                    s.push(bt);
                    s.push(gull(e1 / 2.0, ee / 2.0, -off, b, c, zs, l, 1.0));
                    let mut tp = gull(d1 / 2.0, dd / 2.0, off, b, c, zs, l, 1.0);
                    for f in &mut tp.faces { for p in f.iter_mut() { *p = [p[1], p[0], p[2]]; } f.reverse(); }
                    s.push(tp);
                }
            } else {
                let (dd, ee, e1) = (d(dims, "D", 4.9), d(dims, "E", 6.0), d(dims, "E1", 3.9));
                s.push(bx(-e1 / 2.0, -dd / 2.0, a1, e1 / 2.0, dd / 2.0, a, false));
                if family == "sot23" && n == 3 {
                    s.push(gull(e1 / 2.0, ee / 2.0, e, b, c, zs, l, -1.0));
                    s.push(gull(e1 / 2.0, ee / 2.0, -e, b, c, zs, l, -1.0));
                    s.push(gull(e1 / 2.0, ee / 2.0, 0.0, b, c, zs, l, 1.0));
                } else {
                    let per = n.div_ceil(2);
                    let span = (per - 1) as f64 * e;
                    for i in 0..per { s.push(gull(e1 / 2.0, ee / 2.0, span / 2.0 - i as f64 * e, b, c, zs, l, -1.0)); }
                    for i in 0..(n - per) { s.push(gull(e1 / 2.0, ee / 2.0, -span / 2.0 + i as f64 * e, b, c, zs, l, 1.0)); }
                }
            }
        }
        "qfn" | "dfn" => {
            let n = pins(if family == "dfn" { 8.0 } else { 16.0 });
            let (dd, ee, a, b, e, l) = (d(dims, "D", 4.0), d(dims, "E", 4.0), d(dims, "A", 0.9), d(dims, "b", 0.3), d(dims, "e", 0.65), d(dims, "L", 0.4));
            let t = 0.02;
            s.push(bx(-ee / 2.0, -dd / 2.0, t, ee / 2.0, dd / 2.0, a, false));
            let sides = if family == "dfn" { 2 } else { 4 };
            let per = (n / sides).max(1);
            let span = (per - 1) as f64 * e;
            for i in 0..per {
                let off = span / 2.0 - i as f64 * e;
                s.push(bx(-ee / 2.0, off - b / 2.0, 0.0, -ee / 2.0 + l, off + b / 2.0, t, true));
                s.push(bx(ee / 2.0 - l, -off - b / 2.0, 0.0, ee / 2.0, -off + b / 2.0, t, true));
                if sides == 4 {
                    s.push(bx(-off - b / 2.0, -dd / 2.0, 0.0, -off + b / 2.0, -dd / 2.0 + l, t, true));
                    s.push(bx(off - b / 2.0, dd / 2.0 - l, 0.0, off + b / 2.0, dd / 2.0, t, true));
                }
            }
            let (d2, e2) = (d(dims, "D2", 0.0), d(dims, "E2", 0.0));
            if d2 > 0.0 && e2 > 0.0 { s.push(bx(-e2 / 2.0, -d2 / 2.0, 0.0, e2 / 2.0, d2 / 2.0, t, true)); }
        }
        "bga" => {
            // rows x cols, or `pins` (the total, as every family takes it) as the smallest square grid that holds them
            let side = dims.get("pins").and_then(|v| v.as_f64()).map(|n| n.max(1.0).sqrt().ceil()).unwrap_or(4.0);
            let (rows, cols) = (d(dims, "rows", side).round().max(1.0) as usize, d(dims, "cols", side).round().max(1.0) as usize);
            let (dd, ee, a, a1, b, e) = (d(dims, "D", 4.0), d(dims, "E", 4.0), d(dims, "A", 1.2), d(dims, "A1", 0.25), d(dims, "b", 0.4), d(dims, "e", 0.8));
            s.push(bx(-ee / 2.0, -dd / 2.0, a1, ee / 2.0, dd / 2.0, a, false));
            for r in 0..rows {
                for c in 0..cols {
                    let (x, y) = ((c as f64 - (cols - 1) as f64 / 2.0) * e, ((rows - 1) as f64 / 2.0 - r as f64) * e);
                    s.push(prism(&ngon(b / 2.0, 12), 0.0, a1, true, |u, v, w| [x + u, y + v, w]));
                }
            }
        }
        "melf" => {
            let (len, dia, l) = (d(dims, "D", 3.5), d(dims, "E", 1.4), d(dims, "L", 0.45));
            let r = dia / 2.0;
            // a cylinder lying along x, resting on z = 0
            s.push(prism(&ngon(r * 0.97, 24), -len / 2.0 + l, len / 2.0 - l, false, |u, v, w| [w, u, r + v]));
            s.push(prism(&ngon(r, 24), -len / 2.0, -len / 2.0 + l, true, |u, v, w| [w, u, r + v]));
            s.push(prism(&ngon(r, 24), len / 2.0 - l, len / 2.0, true, |u, v, w| [w, u, r + v]));
        }
        "ecap" => {
            let (dia, h, base) = (d(dims, "D", 6.3), d(dims, "A", 5.4), d(dims, "E", 6.6));
            s.push(bx(-base / 2.0, -base / 2.0, 0.0, base / 2.0, base / 2.0, 0.6, false));
            s.push(prism(&ngon(dia / 2.0, 32), 0.6, h, true, |u, v, w| [u, v, w]));
        }
        "crystal" => {
            let (dd, ee, a, b, l) = (d(dims, "D", 3.2), d(dims, "E", 2.5), d(dims, "A", 0.8), d(dims, "b", 1.2), d(dims, "L", 1.0));
            s.push(bx(-dd / 2.0, -ee / 2.0, 0.05, dd / 2.0, ee / 2.0, a, false));
            for (sx, sy) in [(-1.0, 1.0), (1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
                let (x, y) = (sx * (dd / 2.0 - l / 2.0), sy * (ee / 2.0 - b / 2.0));
                s.push(bx(x - l / 2.0, y - b / 2.0, 0.0, x + l / 2.0, y + b / 2.0, 0.05, true));
            }
        }
        "header" => {
            let (n, rows, e, a, l, l1, b) = (pins(4.0), d(dims, "rows", 1.0).round().max(1.0) as usize, d(dims, "e", 2.54), d(dims, "A", 2.5), d(dims, "L", 3.0), d(dims, "L1", 6.0), d(dims, "b", 0.64));
            let per = n.div_ceil(rows);
            let (w, h) = (rows as f64 * e, per as f64 * e);
            // pin 1 at the origin, the rest down the column (and across the rows)
            s.push(bx(-e / 2.0, -h + e / 2.0, 0.0, w - e / 2.0, e / 2.0, a, false));
            for i in 0..n {
                let (x, y) = ((i / per) as f64 * e, -((i % per) as f64) * e);
                s.push(bx(x - b / 2.0, y - b / 2.0, -l, x + b / 2.0, y + b / 2.0, a + l1, true));
            }
        }
        _ => return None,
    }
    Some(s)
}

/// STEP AP214 text of the solids: one product, every solid a MANIFOLD_SOLID_BREP of planar faces, coloured.
pub fn step(solids: &[Solid], name: &str) -> String {
    let mut out = Vec::<String>::new();
    let mut id = 0usize;
    let mut add = |out: &mut Vec<String>, s: String| -> usize { id += 1; out.push(format!("#{id}={s};")); id };
    let r = |v: f64| { let s = format!("{:.6}", if v.abs() < 5e-7 { 0.0 } else { v }); s };
    let app = add(&mut out, "APPLICATION_CONTEXT('core data for automotive mechanical design processes')".into());
    add(&mut out, format!("APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#{app})"));
    let pc = add(&mut out, format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));
    let prod = add(&mut out, format!("PRODUCT('{name}','{name}','',(#{pc}))"));
    let pdf = add(&mut out, format!("PRODUCT_DEFINITION_FORMATION('','',#{prod})"));
    let pdc = add(&mut out, format!("PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"));
    let pd = add(&mut out, format!("PRODUCT_DEFINITION('design','',#{pdf},#{pdc})"));
    let pds = add(&mut out, format!("PRODUCT_DEFINITION_SHAPE('','',#{pd})"));
    add(&mut out, format!("PRODUCT_RELATED_PRODUCT_CATEGORY('part',$,(#{prod}))"));
    let lu = add(&mut out, "(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.))".into());
    let au = add(&mut out, "(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.))".into());
    let su = add(&mut out, "(NAMED_UNIT(*)SI_UNIT($,.STERADIAN.)SOLID_ANGLE_UNIT())".into());
    let unc = add(&mut out, format!("UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-06),#{lu},'distance_accuracy_value','confusion accuracy')"));
    let ctx = add(&mut out, format!("(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc}))GLOBAL_UNIT_ASSIGNED_CONTEXT((#{lu},#{au},#{su}))REPRESENTATION_CONTEXT('Context #1','3D Context with UNIT and UNCERTAINTY'))"));
    let o = add(&mut out, "CARTESIAN_POINT('',(0.,0.,0.))".into());
    let z = add(&mut out, "DIRECTION('',(0.,0.,1.))".into());
    let x = add(&mut out, "DIRECTION('',(1.,0.,0.))".into());
    let ax = add(&mut out, format!("AXIS2_PLACEMENT_3D('',#{o},#{z},#{x})"));
    let mut breps = Vec::new();
    let mut styled = Vec::new();
    // colours: a dark moulded body, bright leads
    let mut style = |out: &mut Vec<String>, rgb: (f64, f64, f64)| -> usize {
        let c = add(out, format!("COLOUR_RGB('',{},{},{})", r(rgb.0), r(rgb.1), r(rgb.2)));
        let fc = add(out, format!("FILL_AREA_STYLE_COLOUR('',#{c})"));
        let fa = add(out, format!("FILL_AREA_STYLE('',(#{fc}))"));
        let sf = add(out, format!("SURFACE_STYLE_FILL_AREA(#{fa})"));
        let ss = add(out, format!("SURFACE_SIDE_STYLE('',(#{sf}))"));
        let su = add(out, format!("SURFACE_STYLE_USAGE(.BOTH.,#{ss})"));
        add(out, format!("PRESENTATION_STYLE_ASSIGNMENT((#{su}))"))
    };
    let body_style = style(&mut out, (0.15, 0.15, 0.16));
    let lead_style = style(&mut out, (0.80, 0.80, 0.82));
    let mark_style = style(&mut out, (0.92, 0.92, 0.90));
    for sol in solids {
        // shared vertices and edges, so the shell is closed
        let mut verts: Vec<(P3, usize)> = Vec::new();
        let mut edges: Vec<((usize, usize), usize)> = Vec::new();
        let key = |p: &P3| [(p[0] * 1e5).round() as i64, (p[1] * 1e5).round() as i64, (p[2] * 1e5).round() as i64];
        let mut faces = Vec::new();
        for f in &sol.faces {
            let mut vids = Vec::new();
            for p in f {
                let k = key(p);
                let vid = match verts.iter().find(|(q, _)| key(q) == k) {
                    Some((_, v)) => *v,
                    None => {
                        let cp = add(&mut out, format!("CARTESIAN_POINT('',({},{},{}))", r(p[0]), r(p[1]), r(p[2])));
                        let v = add(&mut out, format!("VERTEX_POINT('',#{cp})"));
                        verts.push((*p, v));
                        v
                    }
                };
                vids.push((vid, *p));
            }
            let mut oes = Vec::new();
            for i in 0..vids.len() {
                let (a, b) = (vids[i], vids[(i + 1) % vids.len()]);
                let (lo, hi) = if a.0 < b.0 { (a.0, b.0) } else { (b.0, a.0) };
                let eid = match edges.iter().find(|(k, _)| *k == (lo, hi)) {
                    Some((_, e)) => *e,
                    None => {
                        // the edge runs from the lower vertex id to the higher
                        let (pa, pb) = if a.0 == lo { (a.1, b.1) } else { (b.1, a.1) };
                        let len = ((pb[0] - pa[0]).powi(2) + (pb[1] - pa[1]).powi(2) + (pb[2] - pa[2]).powi(2)).sqrt().max(1e-12);
                        let cp = add(&mut out, format!("CARTESIAN_POINT('',({},{},{}))", r(pa[0]), r(pa[1]), r(pa[2])));
                        let dir = add(&mut out, format!("DIRECTION('',({},{},{}))", r((pb[0] - pa[0]) / len), r((pb[1] - pa[1]) / len), r((pb[2] - pa[2]) / len)));
                        let vec = add(&mut out, format!("VECTOR('',#{dir},{})", r(len)));
                        let ln = add(&mut out, format!("LINE('',#{cp},#{vec})"));
                        let e = add(&mut out, format!("EDGE_CURVE('',#{lo},#{hi},#{ln},.T.)"));
                        edges.push(((lo, hi), e));
                        e
                    }
                };
                oes.push(add(&mut out, format!("ORIENTED_EDGE('',*,*,#{eid},{})", if a.0 == lo { ".T." } else { ".F." })));
            }
            let loop_ = add(&mut out, format!("EDGE_LOOP('',({}))", oes.iter().map(|e| format!("#{e}")).collect::<Vec<_>>().join(",")));
            let bound = add(&mut out, format!("FACE_OUTER_BOUND('',#{loop_},.T.)"));
            // the face's plane: Newell's normal, a reference direction along its first edge
            let mut nrm = [0.0f64; 3];
            for i in 0..f.len() { let (a, b) = (f[i], f[(i + 1) % f.len()]); nrm[0] += (a[1] - b[1]) * (a[2] + b[2]); nrm[1] += (a[2] - b[2]) * (a[0] + b[0]); nrm[2] += (a[0] - b[0]) * (a[1] + b[1]); }
            let nl = (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt().max(1e-12);
            let nn = [nrm[0] / nl, nrm[1] / nl, nrm[2] / nl];
            let e0 = [f[1][0] - f[0][0], f[1][1] - f[0][1], f[1][2] - f[0][2]];
            let el = (e0[0] * e0[0] + e0[1] * e0[1] + e0[2] * e0[2]).sqrt().max(1e-12);
            let cp = add(&mut out, format!("CARTESIAN_POINT('',({},{},{}))", r(f[0][0]), r(f[0][1]), r(f[0][2])));
            let dn = add(&mut out, format!("DIRECTION('',({},{},{}))", r(nn[0]), r(nn[1]), r(nn[2])));
            let dr = add(&mut out, format!("DIRECTION('',({},{},{}))", r(e0[0] / el), r(e0[1] / el), r(e0[2] / el)));
            let pl_ax = add(&mut out, format!("AXIS2_PLACEMENT_3D('',#{cp},#{dn},#{dr})"));
            let plane = add(&mut out, format!("PLANE('',#{pl_ax})"));
            faces.push(add(&mut out, format!("ADVANCED_FACE('',(#{bound}),#{plane},.T.)")));
        }
        let shell = add(&mut out, format!("CLOSED_SHELL('',({}))", faces.iter().map(|f| format!("#{f}")).collect::<Vec<_>>().join(",")));
        let brep = add(&mut out, format!("MANIFOLD_SOLID_BREP('{}',#{shell})", if sol.mark { "marking" } else if sol.lead { "lead" } else { "body" }));
        breps.push(brep);
        styled.push(add(&mut out, format!("STYLED_ITEM('color',(#{}),#{brep})", if sol.mark { mark_style } else if sol.lead { lead_style } else { body_style })));
    }
    let rep = add(&mut out, format!("ADVANCED_BREP_SHAPE_REPRESENTATION('{name}',(#{ax},{}),#{ctx})", breps.iter().map(|b| format!("#{b}")).collect::<Vec<_>>().join(",")));
    add(&mut out, format!("SHAPE_DEFINITION_REPRESENTATION(#{pds},#{rep})"));
    add(&mut out, format!("MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION('',({}),#{ctx})", styled.iter().map(|s| format!("#{s}")).collect::<Vec<_>>().join(",")));
    format!("ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('altium-bridge generate_body'),'2;1');\nFILE_NAME('{name}.step','2026-01-01T00:00:00',(''),(''),'altium-bridge','altium-bridge','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN {{ 1 0 10303 214 1 1 1 1 }}'));\nENDSEC;\nDATA;\n{}\nENDSEC;\nEND-ISO-10303-21;\n", out.join("\n"))
}

/// A segment font for markings: each character is strokes between the points of a 2 x 2 cell (x 0..1, y 0..2), as
/// a 16-segment display draws them plus two diagonals for V. Part markings use capitals, digits and a few signs.
fn glyph(c: char) -> Option<&'static [((f64, f64), (f64, f64))]> {
    const TL: (f64, f64) = (0.0, 2.0); const TM: (f64, f64) = (0.5, 2.0); const TR: (f64, f64) = (1.0, 2.0);
    const ML: (f64, f64) = (0.0, 1.0); const MM: (f64, f64) = (0.5, 1.0); const MR: (f64, f64) = (1.0, 1.0);
    const BL: (f64, f64) = (0.0, 0.0); const BM: (f64, f64) = (0.5, 0.0); const BR: (f64, f64) = (1.0, 0.0);
    const A: ((f64, f64), (f64, f64)) = (TL, TR); const B: ((f64, f64), (f64, f64)) = (TR, MR); const C: ((f64, f64), (f64, f64)) = (MR, BR);
    const D: ((f64, f64), (f64, f64)) = (BL, BR); const E: ((f64, f64), (f64, f64)) = (ML, BL); const F: ((f64, f64), (f64, f64)) = (TL, ML);
    const G1: ((f64, f64), (f64, f64)) = (ML, MM); const G2: ((f64, f64), (f64, f64)) = (MM, MR); const G: ((f64, f64), (f64, f64)) = (ML, MR);
    const VT: ((f64, f64), (f64, f64)) = (TM, MM); const VB: ((f64, f64), (f64, f64)) = (MM, BM); const V: ((f64, f64), (f64, f64)) = (TM, BM);
    const DTL: ((f64, f64), (f64, f64)) = (TL, MM); const DTR: ((f64, f64), (f64, f64)) = (TR, MM);
    const DBL: ((f64, f64), (f64, f64)) = (MM, BL); const DBR: ((f64, f64), (f64, f64)) = (MM, BR);
    const LB: ((f64, f64), (f64, f64)) = (ML, BM); const RB: ((f64, f64), (f64, f64)) = (MR, BM);
    const DOT: ((f64, f64), (f64, f64)) = ((0.4, 0.0), (0.6, 0.0));
    // D and B: the left side full and the right corners cut, so D does not read as I and O side by side (it was a
    // box with a centre bar: "ADOM" read "AIOM" on a QFN in Altium, 2026-10-28) and B is not an 8
    const KT: (f64, f64) = (0.75, 2.0); const KM: (f64, f64) = (0.75, 1.0); const KB: (f64, f64) = (0.75, 0.0);
    const RT: (f64, f64) = (1.0, 1.75); const RU: (f64, f64) = (1.0, 1.25); const RL: (f64, f64) = (1.0, 0.75); const RB0: (f64, f64) = (1.0, 0.25);
    const DG: &[((f64, f64), (f64, f64))] = &[F, E, (TL, KT), (KT, RT), (RT, RB0), (RB0, KB), (BL, KB)];
    const BG: &[((f64, f64), (f64, f64))] = &[F, E, (TL, KT), (KT, RT), (RT, RU), (RU, KM), (ML, KM), (KM, RL), (RL, RB0), (RB0, KB), (BL, KB)];
    Some(match c.to_ascii_uppercase() {
        '0' => &[A, B, C, D, E, F, DTR, DBL], '1' => &[V], '2' => &[A, B, G, E, D], '3' => &[A, B, G2, C, D],
        '4' => &[F, G, B, C], '5' => &[A, F, G, C, D], '6' => &[A, F, E, D, C, G], '7' => &[A, B, C], '8' => &[A, B, C, D, E, F, G],
        '9' => &[A, B, C, D, F, G], 'A' => &[E, F, A, B, C, G], 'B' => BG, 'C' => &[A, F, E, D], 'D' => DG,
        'E' => &[A, F, E, D, G1], 'F' => &[A, F, E, G1], 'G' => &[A, F, E, D, C, G2], 'H' => &[F, E, B, C, G], 'I' => &[A, V, D],
        'J' => &[B, C, D, E], 'K' => &[F, E, G1, DTR, DBR], 'L' => &[F, E, D], 'M' => &[E, F, B, C, DTL, DTR], 'N' => &[E, F, B, C, DTL, DBR],
        'O' => &[A, B, C, D, E, F], 'P' => &[A, B, F, E, G], 'Q' => &[A, B, C, D, E, F, DBR], 'R' => &[A, B, F, E, G, DBR],
        'S' => &[A, F, G, C, D], 'T' => &[A, V], 'U' => &[F, E, D, C, B], 'V' => &[F, B, LB, RB], 'W' => &[E, F, B, C, DBL, DBR],
        'X' => &[DTL, DTR, DBL, DBR], 'Y' => &[DTL, DTR, VB], 'Z' => &[A, DTR, DBL, D], '-' => &[G], '+' => &[G, VT, VB],
        '/' => &[DTR, DBL], '.' => &[DOT], ' ' => &[],
        _ => return None,
    })
}

/// The marking as thin bars on the top face of `top` (the body's top), centred and sized to fit it, lines stacked.
/// Raised stands 30 um proud; engraved is a 5 um layer flush on the top (the body is not cut). Returns the solids and
/// the characters the font does not draw.
fn marking(text: &str, top: &Solid, engraved: bool) -> (Vec<Solid>, Vec<char>) {
    let b = bbox(std::slice::from_ref(top));
    let (w, d, z) = (b[3] - b[0], b[4] - b[1], b[5]);
    let (cx, cy) = ((b[0] + b[3]) / 2.0, (b[1] + b[4]) / 2.0);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut skipped = Vec::new();
    if lines.is_empty() { return (Vec::new(), skipped); }
    // a character cell is 0.6 h wide with 0.25 h between characters; lines are 1.5 h apart
    let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1) as f64;
    let h = (0.85 * w / (longest * 0.85 - 0.25).max(0.6)).min(0.8 * d / (lines.len() as f64 * 1.5 - 0.5)).min(1.5);
    let stroke = (h * 0.12).max(0.03);
    let (z0, z1) = if engraved { (z, z + 0.005) } else { (z, z + 0.03) };
    let mut out = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        let n = line.chars().count() as f64;
        let x0 = cx - (n * 0.85 - 0.25) * h / 2.0;
        let y0 = cy + (lines.len() as f64 * 1.5 - 0.5) * h / 2.0 - h - li as f64 * 1.5 * h;
        for (ci, ch) in line.chars().enumerate() {
            let Some(strokes) = glyph(ch) else { skipped.push(ch); continue };
            let ox = x0 + ci as f64 * 0.85 * h;
            for ((u0, v0), (u1, v1)) in strokes {
                // a stroke of the 0..1 x 0..2 cell, scaled to 0.6 h x h, as a bar `stroke` wide
                let (ax, ay, bx2, by) = (ox + u0 * 0.6 * h, y0 + v0 * 0.5 * h, ox + u1 * 0.6 * h, y0 + v1 * 0.5 * h);
                let len = (bx2 - ax).hypot(by - ay).max(1e-9);
                let (nx, ny) = (-(by - ay) / len * stroke / 2.0, (bx2 - ax) / len * stroke / 2.0);
                let (ex, ey) = ((bx2 - ax) / len * stroke / 2.0, (by - ay) / len * stroke / 2.0);
                let quad = vec![(ax - ex - nx, ay - ey - ny), (bx2 + ex - nx, by + ey - ny), (bx2 + ex + nx, by + ey + ny), (ax - ex + nx, ay - ey + ny)];
                let mut s = prism(&quad, z0, z1, false, |u, v, w| [u, v, w]);
                s.mark = true;
                out.push(s);
            }
        }
    }
    (out, skipped)
}

fn bbox(solids: &[Solid]) -> [f64; 6] {
    solids.iter().flat_map(|s| s.faces.iter().flatten()).fold([f64::MAX, f64::MAX, f64::MAX, f64::MIN, f64::MIN, f64::MIN], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].min(p[2]), b[3].max(p[0]), b[4].max(p[1]), b[5].max(p[2])])
}

/// generate_body: input `{args: {family, dimensionsMm?, marking?, markingStyle?, name?}}`. The package's body as STEP
/// text (`text`) and its box in the bodies companion's form (`bodies`, one box for the whole body; a raised marking's
/// 30 um is not part of it). No I/O: the bridge writes the file and attaches it (add_body).
pub fn generate_body(input: &Value) -> Value {
    let args = &input["args"];
    let fams = families();
    let family = args.get("family").and_then(|v| v.as_str()).unwrap_or("").to_lowercase().replace(['-', '_', ' '], "");
    let family = match family.as_str() { "sot23" | "sot233" => "sot23".to_string(), "tqfp" | "lqfp" => "qfp".into(), "son" | "wson" => "dfn".into(), "minimelf" => "melf".into(), "electrolytic" | "can" => "ecap".into(), f => f.to_string() };
    let Some((name, defaults)) = fams.iter().find(|(n, _)| *n == family) else {
        return json!({"success": false, "errorCode": "unknown_family", "error": format!("family {:?} is not one the engine makes", args.get("family")),
            "families": fams.iter().map(|(n, _)| *n).collect::<Vec<_>>()});
    };
    let mut dims = defaults.as_object().cloned().unwrap_or_default();
    if let Some(given) = args.get("dimensionsMm").and_then(|v| v.as_object()) {
        for (k, v) in given { if v.is_number() { dims.insert(k.clone(), v.clone()); } }
    }
    let Some(mut solids) = package(name, &dims) else { return json!({"success": false, "errorCode": "unknown_family"}) };
    // the marking sits on the body's top (an electrolytic can's top for ecap)
    let marking_text = args.get("marking").and_then(|v| v.as_str()).map(|t| t.replace("\\n", "\n")).filter(|t| !t.trim().is_empty());
    let engraved = args.get("markingStyle").and_then(|v| v.as_str()) == Some("engraved");
    let mut unmarked: Vec<char> = Vec::new();
    if let Some(t) = &marking_text {
        let top = solids[if *name == "ecap" { 1 } else { 0 }].clone();
        let (marks, skipped) = marking(t, &top, engraved);
        unmarked = skipped;
        solids.extend(marks);
    }
    let stem = args.get("name").and_then(|v| v.as_str()).unwrap_or(name).replace('\'', "");
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
    // the box is the package's: a raised marking's 30 um is not part of the body's height
    let body: Vec<Solid> = solids.iter().filter(|s| !s.mark).cloned().collect();
    let b = bbox(&body);
    json!({"success": true, "text": step(&solids, &stem), "family": name, "dimensionsMm": dims, "solids": solids.len(),
        "bboxMm": {"x": r3(b[3] - b[0]), "y": r3(b[4] - b[1]), "z": r3(b[5] - b[2])},
        "bodies": [{"x0": r3(b[0]), "y0": r3(b[1]), "x1": r3(b[3]), "y1": r3(b[4]), "z0": r3(b[2]), "z1": r3(b[5])}],
        "marking": marking_text,
        "markingStyle": if marking_text.is_some() { json!(if engraved { "engraved (a 5 um layer flush on the top; the body is not cut)" } else { "raised 30 um" }) } else { Value::Null },
        "notApplied": if unmarked.is_empty() { json!([]) } else { json!([format!("marking characters the font does not draw: {}", unmarked.iter().collect::<String>())]) },
        "writtenFrom": "package dimensions",
        "_hint": "The body is in the package's own frame (IPC-7351 zero orientation, pin 1 top-left, seated on z = 0). Write text to a .step file and attach it (add_body). Compare bboxMm with the datasheet's body size before attaching it."})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed(s: &Solid) -> bool {
        // every edge is used once each way: a closed, consistently oriented shell
        let key = |p: &P3| [(p[0] * 1e5).round() as i64, (p[1] * 1e5).round() as i64, (p[2] * 1e5).round() as i64];
        let mut e: Vec<([i64; 3], [i64; 3])> = Vec::new();
        for f in &s.faces { for i in 0..f.len() { e.push((key(&f[i]), key(&f[(i + 1) % f.len()]))); } }
        e.iter().all(|(a, b)| e.iter().filter(|x| x == &&(*b, *a)).count() == 1)
    }

    #[test]
    fn every_family_is_closed_solids_with_outward_faces() {
        for (name, def) in families() {
            let solids = package(name, def.as_object().unwrap()).unwrap();
            for s in &solids {
                assert!(closed(s), "{name}: a shell is open");
                assert!(signed_volume(s) > 0.0, "{name}: a solid is inside out");
            }
        }
    }

    #[test]
    fn a_soic8_has_its_lead_span_and_height() {
        let dims = families().into_iter().find(|f| f.0 == "soic").unwrap().1;
        let s = package("soic", dims.as_object().unwrap()).unwrap();
        assert_eq!(s.len(), 9);
        let b = bbox(&s);
        assert!((b[3] - b[0] - 6.0).abs() < 1e-9 && (b[5] - 1.75).abs() < 1e-9 && b[2].abs() < 1e-9, "{b:?}");
        // pin 1 is the top-left lead
        let lead1 = bbox(&s[1..2]);
        assert!(lead1[0] < 0.0 && lead1[4] > 1.9, "{lead1:?}");
    }

    /// STEP_DUMP=<dir>: write every family's default body there, for a STEP reader to check
    #[test]
    fn dump_for_a_reader() {
        let Ok(dir) = std::env::var("STEP_DUMP") else { return };
        for (name, def) in families() {
            std::fs::write(format!("{dir}/{name}.step"), step(&package(name, def.as_object().unwrap()).unwrap(), name)).unwrap();
        }
        for (name, text, idx) in [("soic", "NE555\nTI 2X4", 0), ("qfn", "STM32\nG031", 0), ("ecap", "100UF\n25V", 1)] {
            let def = families().into_iter().find(|f| f.0 == name).unwrap().1;
            let mut s = package(name, def.as_object().unwrap()).unwrap();
            let top = s[idx].clone();
            s.extend(marking(text, &top, false).0);
            std::fs::write(format!("{dir}/{name}-marked.step"), step(&s, name)).unwrap();
        }
    }

    #[test]
    fn the_step_text_is_well_formed() {
        let s = package("chip", families()[0].1.as_object().unwrap()).unwrap();
        let t = step(&s, "R0603");
        assert!(t.starts_with("ISO-10303-21;") && t.trim_end().ends_with("END-ISO-10303-21;"));
        assert_eq!(t.matches("MANIFOLD_SOLID_BREP").count(), 3);
        assert_eq!(t.matches("ADVANCED_FACE").count(), 18);
        // every referenced entity exists
        let ids: Vec<usize> = t.lines().filter_map(|l| l.strip_prefix('#').and_then(|r| r.split('=').next()).and_then(|n| n.parse().ok())).collect();
        let max = *ids.iter().max().unwrap();
        for cap in t.split('#').skip(1) { let n: String = cap.chars().take_while(|c| c.is_ascii_digit()).collect(); if let Ok(n) = n.parse::<usize>() { assert!(n <= max); } }
    }

    #[test]
    fn a_marking_sits_on_the_body_top_and_fits_it() {
        let dims = families().into_iter().find(|f| f.0 == "soic").unwrap().1;
        let s = package("soic", dims.as_object().unwrap()).unwrap();
        let (m, skipped) = marking("NE555\nTI 2X", &s[0], false);
        assert!(skipped.is_empty());
        assert!(!m.is_empty());
        let (body, mb) = (bbox(&s[..1]), bbox(&m));
        assert!((mb[2] - 1.75).abs() < 1e-9 && (mb[5] - 1.78).abs() < 1e-9, "{mb:?}");
        assert!(mb[0] > body[0] && mb[3] < body[3] && mb[1] > body[1] && mb[4] < body[4], "inside the top: {mb:?} {body:?}");
        for x in &m { assert!(closed(x) && signed_volume(x) > 0.0); }
        assert_eq!(marking("A~B", &s[0], true).1, vec!['~']);
    }
    #[test]
    fn the_writer_answers_step_text_and_the_bodys_box() {
        let r = generate_body(&json!({"args": {"family": "SOIC", "marking": "NE555", "name": "U1"}}));
        assert_eq!(r["success"], json!(true));
        assert!(r["text"].as_str().unwrap().starts_with("ISO-10303-21;"));
        assert_eq!(r["bodies"][0]["z1"], json!(1.75), "the marking is not in the box");
        assert_eq!(r["bboxMm"]["x"], json!(6.0));
        assert_eq!(r["writtenFrom"], "package dimensions");
        assert_eq!(generate_body(&json!({"args": {"family": "nope"}}))["errorCode"], "unknown_family");
    }
    #[test]
    fn fusion_lanes_reference_parts_match() {
        // fusion_generate_package 1.13.3's 0603, SOIC-8, QFN-32 (3.3 mm exposed pad) and QFP-48 (#20): the same box and
        // the same number of solids
        let cases = [
            ("chip", json!({"D": 1.6, "E": 0.8, "A": 0.45, "L": 0.3}), [1.6, 0.8, 0.45], 3),
            ("soic", json!({"pins": 8, "D": 4.9, "E": 6.0, "E1": 3.9, "A": 1.75, "A1": 0.1, "b": 0.41, "e": 1.27, "L": 0.84}), [6.0, 4.9, 1.75], 9),
            ("qfn", json!({"pins": 32, "D": 5.0, "E": 5.0, "A": 0.9, "b": 0.25, "e": 0.5, "L": 0.4, "D2": 3.3, "E2": 3.3}), [5.0, 5.0, 0.9], 34),
            ("qfp", json!({"pins": 48, "D": 9.0, "E": 9.0, "D1": 7.0, "E1": 7.0, "A": 1.6, "A1": 0.1, "b": 0.22, "e": 0.5, "L": 0.6}), [9.0, 9.0, 1.6], 49),
        ];
        for (family, dims, size, solids) in cases {
            let r = generate_body(&json!({"args": {"family": family, "dimensionsMm": dims}}));
            assert_eq!(r["solids"], json!(solids), "{family}");
            assert_eq!([r["bboxMm"]["x"].as_f64().unwrap(), r["bboxMm"]["y"].as_f64().unwrap(), r["bboxMm"]["z"].as_f64().unwrap()], size, "{family}");
        }
        // the exposed pad is drawn only when given
        let mut no_ep = json!({"pins": 32, "D": 5.0, "E": 5.0});
        no_ep["D2"] = json!(0.0);
        assert_eq!(generate_body(&json!({"args": {"family": "qfn", "dimensionsMm": no_ep}}))["solids"], json!(33));
        // pins is the total in a BGA too: 64 balls are an 8 x 8 grid
        assert_eq!(package("bga", json!({"pins": 64}).as_object().unwrap()).unwrap().len(), 65);
    }

    #[test]
    fn d_and_b_are_not_i_o_or_8() {
        // D was a box with a centre bar and read as "IO"; B was an 8 with a bar
        assert_ne!(glyph('D'), glyph('O'));
        assert_ne!(glyph('B'), glyph('8'));
        for c in ['D', 'B'] {
            let g = glyph(c).unwrap();
            assert!(g.iter().all(|s| s.0 .0 != 0.5 || s.1 .0 != 0.5), "{c} has no centre bar");
            assert!(!g.iter().any(|s| (s.0 == (1.0, 2.0)) || (s.1 == (1.0, 2.0))), "{c}'s top-right corner is cut");
        }
    }
}

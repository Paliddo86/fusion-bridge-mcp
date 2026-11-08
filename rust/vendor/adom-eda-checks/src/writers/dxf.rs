//! DXF of one board layer, written from the board model (mm, y up) for a bridge whose tool has no headless DXF exporter
//! (Altium's runs only through a dialog). One file text per layer, R12 entities: Edge.Cuts as lines and arcs or one
//! closed polyline (`contours`), copper as tracks and arcs with their width, pads, vias and pours, any other layer's
//! lines and arcs by the standard's name or the tool's own id (L<id>). No I/O: the bridge writes the text to files.
use serde_json::{json, Value};

/// Altium layer ids for the standard's non-copper layer names (the copper ones come through the stack).
fn named_layer(name: &str) -> Option<i64> {
    Some(match name {
        "F.SilkS" | "F.Silkscreen" => 33,
        "B.SilkS" | "B.Silkscreen" => 34,
        "F.Paste" => 35,
        "B.Paste" => 36,
        "F.Mask" => 37,
        "B.Mask" => 38,
        _ => {
            // Mechanical<n> / M<n> / Altium's own L<id>
            if let Some(n) = name.strip_prefix("Mechanical").or_else(|| name.strip_prefix('M')).and_then(|n| n.parse::<i64>().ok()) {
                return (1..=32).contains(&n).then_some(56 + n);
            }
            return name.strip_prefix('L').and_then(|n| n.parse().ok());
        }
    })
}

pub(super) fn is_copper(name: &str) -> bool {
    name == "F.Cu" || name == "B.Cu" || (name.starts_with("In") && name.ends_with(".Cu"))
}

pub(super) fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }

/// DXF R12 text, one entity list. `scale` converts mm to the file's unit.
struct Dxf { body: String, n: usize, scale: f64 }

impl Dxf {
    fn new(scale: f64) -> Self { Dxf { body: String::new(), n: 0, scale } }
    fn num(&self, v: f64) -> String { format!("{:.6}", v * self.scale) }
    fn line(&mut self, layer: &str, a: (f64, f64), b: (f64, f64)) {
        self.body += &format!("0\nLINE\n8\n{layer}\n10\n{}\n20\n{}\n11\n{}\n21\n{}\n", self.num(a.0), self.num(a.1), self.num(b.0), self.num(b.1));
        self.n += 1;
    }
    fn circle(&mut self, layer: &str, c: (f64, f64), r: f64) {
        self.body += &format!("0\nCIRCLE\n8\n{layer}\n10\n{}\n20\n{}\n40\n{}\n", self.num(c.0), self.num(c.1), self.num(r));
        self.n += 1;
    }
    /// counter-clockwise from a1 to a2, degrees
    fn arc(&mut self, layer: &str, c: (f64, f64), r: f64, a1: f64, a2: f64) {
        self.body += &format!("0\nARC\n8\n{layer}\n10\n{}\n20\n{}\n40\n{}\n50\n{a1:.6}\n51\n{a2:.6}\n", self.num(c.0), self.num(c.1), self.num(r));
        self.n += 1;
    }
    /// A polyline of (x, y, bulge) vertices; width for a track's copper.
    fn polyline(&mut self, layer: &str, pts: &[(f64, f64, f64)], closed: bool, width: f64) {
        self.body += &format!("0\nPOLYLINE\n8\n{layer}\n66\n1\n70\n{}\n40\n{}\n41\n{}\n", if closed { 1 } else { 0 }, self.num(width), self.num(width));
        for (x, y, b) in pts {
            self.body += &format!("0\nVERTEX\n8\n{layer}\n10\n{}\n20\n{}\n", self.num(*x), self.num(*y));
            if *b != 0.0 { self.body += &format!("42\n{b:.6}\n"); }
        }
        self.body += "0\nSEQEND\n";
        self.n += 1;
    }
    fn text(&self, insunits: i64) -> String {
        format!("0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1009\n9\n$INSUNITS\n70\n{insunits}\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n{}0\nENDSEC\n0\nEOF\n", self.body)
    }
}

/// The bulge of an arc drawn from `start` (counter-clockwise from a1 to a2 when start is at a1, else clockwise).
pub(super) fn bulge(e: &Value) -> f64 {
    let (a1, a2) = (f(&e["angle1"]), f(&e["angle2"]));
    let mut sweep = (a2 - a1).rem_euclid(360.0);
    if sweep == 0.0 { sweep = 360.0; }
    let c = (f(&e["center"][0]), f(&e["center"][1]));
    let s = (f(&e["start"][0]), f(&e["start"][1]));
    let at_a1 = ((s.1 - c.1).atan2(s.0 - c.0).to_degrees() - a1).rem_euclid(360.0);
    let ccw = at_a1 < 1.0 || at_a1 > 359.0;
    let b = (sweep.to_radians() / 4.0).tan();
    if ccw { b } else { -b }
}

/// A pad's outline on its layer: round as a circle (or an obround), everything else as its rotated rectangle.
fn pad(d: &mut Dxf, layer: &str, p: &Value) {
    let (x, y, w, h) = (f(&p["x"]), f(&p["y"]), f(&p["sizeX"]), f(&p["sizeY"]));
    let rot = f(&p["rotation"]).to_radians();
    let shape = p["shape"].as_str().unwrap_or("rect");
    if shape == "round" && (w - h).abs() < 1e-6 {
        d.circle(layer, (x, y), w / 2.0);
        return;
    }
    let r = |dx: f64, dy: f64| (x + dx * rot.cos() - dy * rot.sin(), y + dx * rot.sin() + dy * rot.cos());
    if shape == "round" {
        // an obround: two straight sides and two half circles
        let (l, rad) = if w > h { ((w - h) / 2.0, h / 2.0) } else { ((h - w) / 2.0, w / 2.0) };
        let pts: Vec<(f64, f64, f64)> = if w > h {
            vec![(-l, -rad, 0.0), (l, -rad, 1.0), (l, rad, 0.0), (-l, rad, 1.0)]
        } else {
            vec![(rad, -l, 0.0), (rad, l, 1.0), (-rad, l, 0.0), (-rad, -l, 1.0)]
        }.into_iter().map(|(a, b, bu)| { let (px, py) = r(a, b); (px, py, bu) }).collect();
        d.polyline(layer, &pts, true, 0.0);
        return;
    }
    let pts: Vec<(f64, f64, f64)> = [(-w / 2.0, -h / 2.0), (w / 2.0, -h / 2.0), (w / 2.0, h / 2.0), (-w / 2.0, h / 2.0)]
        .iter().map(|(a, b)| { let (px, py) = r(*a, *b); (px, py, 0.0) }).collect();
    d.polyline(layer, &pts, true, 0.0);
}

/// One layer of the board as DXF text, with how many entities it holds. `layer` is a standard name (Edge.Cuts, F.Cu,
/// In1.Cu, B.Cu, F.SilkS, F.Mask, Mechanical1 ...) or Altium's own L<id>. None: a name this writer does not know.
/// Pours are drawn as drawn (`polygons`); `layer_dxf_with` draws the copper actually poured instead.
pub fn layer_dxf(state: &Value, layer: &str, contours: bool, inches: bool) -> Option<(String, usize)> {
    layer_dxf_with(state, &[], layer, contours, inches)
}

/// `layer_dxf` with pour_state's `zones`: on a copper layer that has poured fills, each island is drawn with its holes
/// (the real copper, clearances and thermal reliefs included) in place of the pour as drawn.
pub fn layer_dxf_with(state: &Value, zones: &[Value], layer: &str, contours: bool, inches: bool) -> Option<(String, usize)> {
    let mut d = Dxf::new(if inches { 1.0 / 25.4 } else { 1.0 });
    let name = layer.replace('.', "_");
    let arr = |k: &str| state[k].as_array().cloned().unwrap_or_default();
    if layer == "Edge.Cuts" {
        let edges = arr("outlineEdges");
        if contours && !edges.is_empty() {
            let pts: Vec<(f64, f64, f64)> = edges.iter().map(|e| (f(&e["start"][0]), f(&e["start"][1]), if e["kind"] == "arc" { bulge(e) } else { 0.0 })).collect();
            d.polyline(&name, &pts, true, 0.0);
        } else {
            for e in &edges {
                if e["kind"] == "arc" {
                    d.arc(&name, (f(&e["center"][0]), f(&e["center"][1])), f(&e["radius"]), f(&e["angle1"]), f(&e["angle2"]));
                } else {
                    d.line(&name, (f(&e["start"][0]), f(&e["start"][1])), (f(&e["end"][0]), f(&e["end"][1])));
                }
            }
        }
        return Some((d.text(if inches { 1 } else { 4 }), d.n));
    }
    let copper = is_copper(layer);
    let id = if copper { None } else { named_layer(layer) };
    if !copper && id.is_none() && !state["tracks"].as_array().into_iter().flatten().chain(state["arcs"].as_array().into_iter().flatten()).any(|t| t["layer"].as_str() == Some(layer)) {
        return None;
    }
    // a layer by its name, or (a tool that names non-copper layers by id) by the id the standard name maps to
    let on = |v: &Value| v["layer"].as_str() == Some(layer) || (!copper && id.is_some() && v["layerId"].as_i64() == id);
    for t in arr("tracks").iter().filter(|t| on(t)) {
        let w = f(&t["width"]);
        d.polyline(&name, &[(f(&t["start"]["x"]), f(&t["start"]["y"]), 0.0), (f(&t["end"]["x"]), f(&t["end"]["y"]), 0.0)], false, w);
    }
    for a in arr("arcs").iter().filter(|a| on(a)) {
        let (c, r) = ((f(&a["center"]["x"]), f(&a["center"]["y"])), f(&a["radius"]));
        let (a1, a2) = (f(&a["startAngle"]), f(&a["endAngle"]));
        let mut sweep = (a2 - a1).rem_euclid(360.0);
        if sweep == 0.0 { sweep = 360.0; }
        let p = |deg: f64| (c.0 + r * deg.to_radians().cos(), c.1 + r * deg.to_radians().sin());
        let (s, e) = (p(a1), p(a1 + sweep));
        d.polyline(&name, &[(s.0, s.1, (sweep.to_radians() / 4.0).tan()), (e.0, e.1, 0.0)], false, f(&a["width"]));
    }
    if copper {
        for p in arr("pads").iter().filter(|p| p["layer"].as_str() == Some(layer) || p["layer"].as_str() == Some("*.Cu")) {
            pad(&mut d, &name, p);
        }
        for v in arr("vias") {
            let ls: Vec<&str> = v["layers"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
            // a through via is on every copper layer; a blind or buried one on its ends (the model names only those)
            let through = ls.first() == Some(&"F.Cu") && ls.last() == Some(&"B.Cu");
            if through || ls.contains(&layer) {
                d.circle(&name, (f(&v["position"]["x"]), f(&v["position"]["y"])), f(&v["size"]) / 2.0);
            }
        }
        let fills: Vec<Value> = zones.iter().filter(|z| z["layer"].as_str() == Some(layer))
            .flat_map(|z| z["fills"].as_array().cloned().unwrap_or_default()).collect();
        let ring = |v: &Value| -> Vec<(f64, f64, f64)> { v.as_array().map(|ps| ps.iter().map(|p| (f(&p[0]), f(&p[1]), 0.0)).collect()).unwrap_or_default() };
        if !fills.is_empty() {
            // the copper actually poured: each island's outline and its holes (a fractured fill is one ring: same copper)
            for fl in &fills {
                for r in std::iter::once(ring(&fl["outline"])).chain(fl["holes"].as_array().cloned().unwrap_or_default().iter().map(ring)) {
                    if r.len() > 2 { d.polyline(&name, &r, true, 0.0); }
                }
            }
        } else {
            for g in arr("polygons").iter().filter(|g| g["layer"].as_str() == Some(layer)) {
                let pts = ring(&g["points"]);
                if pts.len() > 2 { d.polyline(&name, &pts, true, 0.0); }
            }
        }
    }
    Some((d.text(if inches { 1 } else { 4 }), d.n))
}

/// writers::dxf_layers: `{board, pours?, args: {layers, contours?, units?: mm|in}}` -> one DXF text per layer. With
/// `pours` (pour_state) a copper layer shows the copper actually poured; without, the pours as drawn.
pub fn dxf_layers(input: &Value) -> Value {
    let board = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let zones = crate::input::pours(input);
    let args = input.get("args").cloned().unwrap_or(json!({}));
    let layers: Vec<String> = args.get("layers").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if layers.is_empty() { return crate::input::refuse("bad_args", "args.layers is required: [\"Edge.Cuts\", \"F.Cu\", ...]"); }
    let inches = args.get("units").and_then(|v| v.as_str()) == Some("in");
    let contours = args.get("contours").and_then(|v| v.as_bool()) == Some(true);
    let (mut files, mut unknown) = (Vec::new(), Vec::new());
    for l in &layers {
        match layer_dxf_with(&board, &zones, l, contours, inches) {
            Some((text, n)) => files.push(json!({"layer": l, "name": format!("{}.dxf", l.replace('.', "_")), "entities": n, "text": text})),
            None => unknown.push(l.clone()),
        }
    }
    let poured_layers: Vec<&String> = layers.iter().filter(|l| zones.iter().any(|z| z["layer"].as_str() == Some(l.as_str()))).collect();
    let as_drawn: Vec<&String> = layers.iter().filter(|l| !poured_layers.contains(l) && board["polygons"].as_array().map(|a| a.iter().any(|g| g["layer"].as_str() == Some(l.as_str()))).unwrap_or(false)).collect();
    json!({"success": unknown.is_empty() && !files.is_empty(), "files": files, "unknownLayers": unknown, "units": if inches { "in" } else { "mm" },
        "writtenFrom": "board model", "poursAsDrawn": as_drawn,
        "_hint": "Written from the board model, not the tool's own exporter: outline, tracks, arcs, pads, vias and pours (texts are not drawn). Edge.Cuts with contours:true is one closed polyline, what CAM and laser tools want. poursAsDrawn lists layers whose pours are the outlines as drawn, not the copper poured: pass pours (pour_state) for the real copper."})
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The Altium lane board from ADOM_EDA_FIXTURES (altium-lane's fixtures), None when it is not set.
    pub(crate) fn lane() -> Option<Value> {
        let p = std::path::PathBuf::from(std::env::var_os("ADOM_EDA_FIXTURES")?).join("altium").join("lane.routing_state.json");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(p).ok()?).unwrap();
        crate::input::board(&json!({"board": v.get("reply").cloned().unwrap_or(v)})).ok()
    }

    #[test]
    fn the_outline_is_eight_edges_or_one_closed_polyline() {
        let Some(st) = lane() else { return };
        let (t, n) = layer_dxf(&st, "Edge.Cuts", false, false).unwrap();
        assert_eq!(n, 8);
        assert_eq!(t.matches("\nARC\n").count(), 4);
        let (t, n) = layer_dxf(&st, "Edge.Cuts", true, false).unwrap();
        assert_eq!(n, 1);
        assert_eq!(t.matches("\nVERTEX\n").count(), 8);
        // a corner arc turns a quarter: bulge tan(90/4)
        assert!(t.contains("42\n0.414214\n"), "{t}");
        assert!(t.ends_with("0\nEOF\n"));
    }

    #[test]
    fn copper_holds_its_tracks_pads_and_through_vias() {
        let Some(st) = lane() else { return };
        let (_, top) = layer_dxf(&st, "F.Cu", false, false).unwrap();
        let tracks = st["tracks"].as_array().unwrap().iter().filter(|t| t["layer"] == "F.Cu").count();
        let pads = st["pads"].as_array().unwrap().iter().filter(|p| p["layer"] == "F.Cu" || p["layer"] == "*.Cu").count();
        let vias = st["vias"].as_array().unwrap().len();
        let arcs = st["arcs"].as_array().unwrap().iter().filter(|a| a["layer"] == "F.Cu").count();
        assert_eq!(top, tracks + arcs + pads + vias);
        assert!(layer_dxf(&st, "Mechanical1", false, false).unwrap().1 > 0, "the L57 lines");
        assert!(layer_dxf(&st, "Dwgs.User", false, false).is_none());
    }

    #[test]
    fn inches_scale_every_coordinate() {
        let Some(st) = lane() else { return };
        let (t, _) = layer_dxf(&st, "Edge.Cuts", false, true).unwrap();
        assert!(t.contains("$INSUNITS\n70\n1\n"));
        assert!(t.contains(&format!("{:.6}", 40.0 / 25.4)));
    }

    #[test]
    fn a_layer_by_its_standard_name_and_the_writer_entry() {
        let board = json!({"outlineEdges": [{"kind": "line", "start": [0.0, 0.0], "end": [10.0, 0.0]}, {"kind": "line", "start": [10.0, 0.0], "end": [10.0, 5.0]},
                {"kind": "line", "start": [10.0, 5.0], "end": [0.0, 5.0]}, {"kind": "line", "start": [0.0, 5.0], "end": [0.0, 0.0]}],
            "tracks": [{"layer": "F.Cu", "net": "A", "width": 0.2, "start": {"x": 1.0, "y": 1.0}, "end": {"x": 9.0, "y": 1.0}},
                       {"layer": "F.SilkS", "net": "", "width": 0.15, "start": {"x": 1.0, "y": 4.0}, "end": {"x": 9.0, "y": 4.0}}],
            "arcs": [], "pads": [], "vias": [], "polygons": []});
        let r = dxf_layers(&json!({"board": board, "args": {"layers": ["Edge.Cuts", "F.Cu", "F.SilkS", "Dwgs.User"], "contours": true}}));
        assert_eq!(r["files"].as_array().unwrap().len(), 3);
        assert_eq!(r["unknownLayers"], json!(["Dwgs.User"]));
        assert_eq!(r["writtenFrom"], "board model");
        assert_eq!(r["files"][2]["entities"], 1, "the silkscreen line, found by its name");
        assert!(r["files"][0]["text"].as_str().unwrap().contains("\nPOLYLINE\n"));
        assert_eq!(dxf_layers(&json!({"board": {}, "args": {}}))["errorCode"], "bad_args");
    }

    #[test]
    fn poured_fills_replace_the_pour_as_drawn() {
        let st = json!({"outline": [[0, 0], [10, 0], [10, 10], [0, 10]], "tracks": [], "arcs": [], "pads": [], "vias": [],
                        "polygons": [{"net": "GND", "layer": "B.Cu", "points": [[0, 0], [10, 0], [10, 10], [0, 10]]}]});
        let zones = vec![json!({"net": "GND", "layer": "B.Cu", "fills": [{"outline": [[1, 1], [9, 1], [9, 9], [1, 9]], "holes": [[[4, 4], [6, 4], [6, 6], [4, 6]]]}]})];
        // as drawn: one outline; poured: the island and its hole
        assert_eq!(layer_dxf(&st, "B.Cu", false, false).unwrap().1, 1);
        assert_eq!(layer_dxf_with(&st, &zones, "B.Cu", false, false).unwrap().1, 2);
        let r = dxf_layers(&json!({"board": st, "args": {"layers": ["B.Cu"]}}));
        assert_eq!(r["poursAsDrawn"], json!(["B.Cu"]));
        let r = dxf_layers(&json!({"board": st, "pours": {"zones": zones}, "args": {"layers": ["B.Cu"]}}));
        assert_eq!(r["poursAsDrawn"], json!([]));
    }
}

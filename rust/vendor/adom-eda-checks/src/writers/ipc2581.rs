//! IPC-2581 revision C (mm) for assembly and test, from the board model: the layers and stack-up, the outline, one
//! package per footprint (pins in the footprint's own frame), the components and BOM, the nets with their pins, and on
//! each copper layer the pads (with their pin), traces, arcs, vias and, with pour_state, the poured copper island by
//! island, plus the drills. From the Fusion bridge's routing_script.py `ipc2581_xml`, which validates against the
//! IPC-2581C schema; the element order follows it.
use super::dxf::f;
use serde_json::{json, Value};

/// A number as IPC-2581 wants it: up to 4 decimals, no trailing zeros.
fn n(v: f64) -> String {
    let s = format!("{:.4}", if v.abs() < 5e-5 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" { "0".into() } else { s.to_string() }
}

/// Fab formats are read by ASCII tools: the usual unit signs by name (10kΩ -> 10kohm), '?' for the rest.
fn ascii(t: &str) -> String {
    t.chars().map(|c| match c {
        c if (c as u32) < 128 => c.to_string(),
        '\u{03A9}' | '\u{2126}' => "ohm".into(), '\u{00B5}' | '\u{03BC}' => "u".into(), '\u{00B1}' => "+/-".into(), '\u{00B0}' => "deg".into(), '\u{2013}' | '\u{2014}' => "-".into(),
        _ => "?".into(),
    }).collect()
}

/// An attribute value: ASCII, XML-escaped.
fn a(t: &str) -> String { ascii(t).replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;") }

/// The pad primitives used, by id, in the order first used.
#[derive(Default)]
struct Dict { ids: Vec<(String, String)> }

impl Dict {
    /// A pad's primitive: a circle, an oval (a stadium) or a centred rectangle.
    fn pad(&mut self, shape: &str, w: f64, h: f64) -> String {
        let round = matches!(shape, "round" | "circle");
        let (id, el) = if round && (w - h).abs() < 1e-6 {
            (format!("CIRCLE_{}", n(w)), format!("<Circle diameter=\"{}\"/>", n(w)))
        } else if round {
            (format!("OVAL_{}x{}", n(w), n(h)), format!("<Oval width=\"{}\" height=\"{}\"/>", n(w), n(h)))
        } else {
            (format!("RECT_{}x{}", n(w), n(h)), format!("<RectCenter width=\"{}\" height=\"{}\"/>", n(w), n(h)))
        };
        let id = id.replace('.', "_").replace('-', "m");
        if !self.ids.iter().any(|(i, _)| *i == id) { self.ids.push((id.clone(), el)); }
        id
    }
    fn xml(&self) -> String {
        let mut o = String::from("    <DictionaryStandard units=\"MILLIMETER\">\n");
        for (id, el) in &self.ids { o += &format!("      <EntryStandard id=\"{id}\">{el}</EntryStandard>\n"); }
        o + "    </DictionaryStandard>\n"
    }
}

/// A closed outline from the board model's outline edges (lines and arcs), or its point list.
fn profile_polygon(st: &Value) -> String {
    let edges = st["outlineEdges"].as_array().cloned().unwrap_or_default();
    let pt = |v: &Value| (f(&v[0]), f(&v[1]));
    let mut o = String::from("        <Polygon>\n");
    if !edges.is_empty() {
        let first = pt(&edges[0]["start"]);
        o += &format!("          <PolyBegin x=\"{}\" y=\"{}\"/>\n", n(first.0), n(first.1));
        for (i, e) in edges.iter().enumerate() {
            // an edge ends where the next begins (the last closes the ring)
            let end = if e["end"].is_array() { pt(&e["end"]) } else { pt(&edges[(i + 1) % edges.len()]["start"]) };
            if e["kind"] == "arc" {
                let cw = super::dxf::bulge(e) < 0.0;
                o += &format!("          <PolyStepCurve x=\"{}\" y=\"{}\" centerX=\"{}\" centerY=\"{}\" clockwise=\"{cw}\"/>\n", n(end.0), n(end.1), n(f(&e["center"][0])), n(f(&e["center"][1])));
            } else {
                o += &format!("          <PolyStepSegment x=\"{}\" y=\"{}\"/>\n", n(end.0), n(end.1));
            }
        }
    } else {
        let ring: Vec<(f64, f64)> = st["outline"].as_array().cloned().unwrap_or_default().iter().map(pt).collect();
        if let Some(first) = ring.first() {
            o += &format!("          <PolyBegin x=\"{}\" y=\"{}\"/>\n", n(first.0), n(first.1));
            for p in ring.iter().skip(1).chain(std::iter::once(first)) { o += &format!("          <PolyStepSegment x=\"{}\" y=\"{}\"/>\n", n(p.0), n(p.1)); }
        }
    }
    o + "        </Polygon>\n"
}

/// A ring of [x, y] as an IPC-2581 Polygon or Cutout (closed).
fn ring_xml(tag: &str, ring: &[(f64, f64)], ind: &str) -> String {
    let mut o = format!("{ind}<{tag}>\n{ind}  <PolyBegin x=\"{}\" y=\"{}\"/>\n", n(ring[0].0), n(ring[0].1));
    for p in ring.iter().skip(1).chain(std::iter::once(&ring[0])) { o += &format!("{ind}  <PolyStepSegment x=\"{}\" y=\"{}\"/>\n", n(p.0), n(p.1)); }
    o + &format!("{ind}</{tag}>\n")
}

/// The board as IPC-2581C text and what it holds.
pub fn ipc2581_text(st: &Value, input: &Value, title: &str, date: &str) -> (String, Value) {
    let arr = |v: &Value, k: &str| v[k].as_array().cloned().unwrap_or_default();
    let name = { let s: String = ascii(title).chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' }).collect(); if s.is_empty() { "board".into() } else { s } };
    let copper = crate::placetools::copper_layers(st, input);
    let lname = |i: usize| if i == 0 { "TOP".to_string() } else if i + 1 == copper.len() { "BOTTOM".to_string() } else { format!("INNER{i}") };
    let idx = |l: &str| copper.iter().position(|c| c == l);
    let mut dict = Dict::default();
    let comps = arr(st, "components");
    let pads = arr(st, "pads");
    let vias = arr(st, "vias");
    let stack = crate::input::stack(input);

    // the BOM: one item per pattern (and value, when the model carries one)
    let mut groups: Vec<((String, String), Vec<&Value>)> = Vec::new();
    for c in &comps {
        if !pads.iter().any(|p| p["ref"] == c["ref"]) { continue; }
        let k = (c["pattern"].as_str().unwrap_or("").to_string(), c["value"].as_str().unwrap_or("").to_string());
        match groups.iter_mut().find(|g| g.0 == k) { Some(g) => g.1.push(c), None => groups.push((k, vec![c])) }
    }
    groups.sort_by(|x, y| x.0.cmp(&y.0));
    let part_id = |p: &str, v: &str| { let s = a(&format!("{p}_{v}")); let s = s.trim_matches('_').to_string(); if s.is_empty() { "PART".to_string() } else { s } };
    let mut bom = format!("  <Bom name=\"{name}_BOM\">\n    <BomHeader assembly=\"{name}\" revision=\"1\">\n      <StepRef name=\"{name}\"/>\n    </BomHeader>\n");
    for ((pat, val), cs) in &groups {
        bom += &format!("    <BomItem OEMDesignNumberRef=\"{}\" quantity=\"{}\" category=\"ELECTRICAL\" description=\"{}\">\n", part_id(pat, val), cs.len(), a(val));
        for c in cs {
            bom += &format!("      <RefDes name=\"{}\" packageRef=\"{}\" populate=\"true\" layerRef=\"{}\"/>\n", a(c["ref"].as_str().unwrap_or("")), a(pat), if c["side"] == "bottom" { "BOTTOM" } else { "TOP" });
        }
        bom += "      <Characteristics category=\"ELECTRICAL\">\n";
        if !val.is_empty() { bom += &format!("        <Textual definitionSource=\"board model\" textualCharacteristicName=\"Value\" textualCharacteristicValue=\"{}\"/>\n", a(val)); }
        bom += "      </Characteristics>\n    </BomItem>\n";
    }
    bom += "  </Bom>\n";

    // layers and the stack-up
    let mut cad = String::from("    <CadData>\n");
    for (i, _) in copper.iter().enumerate() {
        let side = if i == 0 { "TOP" } else if i + 1 == copper.len() { "BOTTOM" } else { "INTERNAL" };
        cad += &format!("      <Layer name=\"{}\" layerFunction=\"SIGNAL\" side=\"{side}\" polarity=\"POSITIVE\"/>\n", lname(i));
    }
    cad += "      <Layer name=\"DRILL_TOP_BOTTOM\" layerFunction=\"DRILL\" side=\"ALL\" polarity=\"POSITIVE\"/>\n";
    let total = stack.as_ref().and_then(|s| s["totalUm"].as_f64()).filter(|t| *t > 0.0).unwrap_or(1600.0) / 1000.0;
    let cu: Vec<f64> = stack.as_ref().map(|s| arr(s, "layers").iter().map(|l| f(&l["copperUm"])).filter(|c| *c > 0.0).collect()).unwrap_or_default();
    cad += &format!("      <Stackup name=\"PRIMARY\" overallThickness=\"{}\" tolPlus=\"0\" tolMinus=\"0\" whereMeasured=\"METAL\" stackupStatus=\"PROPOSED\">\n        <StackupGroup name=\"PRIMARY\" thickness=\"{}\" tolPlus=\"0\" tolMinus=\"0\">\n", n(total), n(total));
    for i in 0..copper.len() {
        cad += &format!("          <StackupLayer layerOrGroupRef=\"{}\" thickness=\"{}\" tolPlus=\"0\" tolMinus=\"0\" sequence=\"{}\"/>\n", lname(i), n(cu.get(i).copied().unwrap_or(35.0) / 1000.0), i + 1);
    }
    cad += "        </StackupGroup>\n      </Stackup>\n";
    let mut step = format!("      <Step name=\"{name}\">\n");

    // via padstacks, one per size and drill
    let mut vstacks: Vec<((String, String), String)> = Vec::new();
    for v in &vias {
        let key = (n(f(&v["size"])), n(f(&v["drill"])));
        if vstacks.iter().any(|x| x.0 == key) { continue; }
        let vn = format!("VIA_{}_{}", key.0, key.1).replace('.', "_");
        let prim = dict.pad("round", f(&v["size"]), f(&v["size"]));
        step += &format!("        <PadStackDef name=\"{vn}\">\n          <PadstackHoleDef name=\"H{vn}\" diameter=\"{}\" platingStatus=\"VIA\" plusTol=\"0\" minusTol=\"0\" x=\"0\" y=\"0\"/>\n", key.1);
        for i in 0..copper.len() {
            step += &format!("          <PadstackPadDef layerRef=\"{}\" padUse=\"REGULAR\">\n            <Location x=\"0\" y=\"0\"/>\n            <StandardPrimitiveRef id=\"{prim}\"/>\n          </PadstackPadDef>\n", lname(i));
        }
        step += "        </PadStackDef>\n";
        vstacks.push((key, vn));
    }
    step += "        <Datum x=\"0\" y=\"0\"/>\n        <Profile>\n";
    step += &profile_polygon(st).replace("        <Polygon>", "          <Polygon>").replace("\n          <Poly", "\n            <Poly").replace("        </Polygon>", "          </Polygon>");
    step += "        </Profile>\n";

    // packages: one per pattern, pins in the footprint's own frame (mirrored back for a bottom part)
    let mut made: Vec<(String, bool)> = Vec::new();
    let mut pkg_xml = String::new();
    for c in &comps {
        let rf = c["ref"].as_str().unwrap_or("");
        let own: Vec<&Value> = pads.iter().filter(|p| p["ref"].as_str() == Some(rf)).collect();
        let pat = { let p = c["pattern"].as_str().unwrap_or(""); if p.is_empty() { format!("PKG_{rf}") } else { p.to_string() } };
        if own.is_empty() || made.iter().any(|m| m.0 == pat) { continue; }
        let (cx, cy, th) = (f(&c["x"]), f(&c["y"]), f(&c["rotation"]).to_radians());
        let bottom = c["side"] == "bottom";
        let th_part = own.iter().any(|p| f(&p["drill"]) > 0.0);
        let mut pins = String::new();
        let mut bb = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in &own {
            let (dx, dy) = (f(&p["x"]) - cx, f(&p["y"]) - cy);
            let (mut lx, ly) = (dx * th.cos() + dy * th.sin(), -dx * th.sin() + dy * th.cos());
            if bottom { lx = -lx; }
            let mut rot = (f(&p["rotation"]) - f(&c["rotation"])).rem_euclid(360.0);
            if bottom { rot = (360.0 - rot).rem_euclid(360.0); }
            let (w, h) = (f(&p["sizeX"]), f(&p["sizeY"]));
            let half = w.max(h).max(f(&p["drill"])) / 2.0;
            bb = (bb.0.min(lx - half), bb.1.min(ly - half), bb.2.max(lx + half), bb.3.max(ly + half));
            let prim = dict.pad(p["shape"].as_str().unwrap_or("rect"), w, h);
            pins += &format!("          <Pin number=\"{}\" type=\"{}\">\n", a(p["name"].as_str().unwrap_or("")), if f(&p["drill"]) > 0.0 { "THRU" } else { "SURFACE" });
            if rot.abs() > 1e-6 && (rot - 360.0).abs() > 1e-6 { pins += &format!("            <Xform rotation=\"{}\"/>\n", n(rot)); }
            pins += &format!("            <Location x=\"{}\" y=\"{}\"/>\n            <StandardPrimitiveRef id=\"{prim}\"/>\n          </Pin>\n", n(lx), n(ly));
        }
        pkg_xml += &format!("        <Package name=\"{}\" type=\"OTHER\" pinOneOrientation=\"OTHER\">\n          <Outline>\n{}            <LineDesc lineEnd=\"ROUND\" lineWidth=\"0.1\"/>\n          </Outline>\n{pins}        </Package>\n",
            a(&pat), ring_xml("Polygon", &[(bb.0, bb.1), (bb.2, bb.1), (bb.2, bb.3), (bb.0, bb.3)], "            "));
        made.push((pat, th_part));
    }
    step += &pkg_xml;
    for c in &comps {
        let rf = c["ref"].as_str().unwrap_or("");
        let pat = { let p = c["pattern"].as_str().unwrap_or(""); if p.is_empty() { format!("PKG_{rf}") } else { p.to_string() } };
        let Some((_, th_part)) = made.iter().find(|m| m.0 == pat) else { continue };
        if !pads.iter().any(|p| p["ref"].as_str() == Some(rf)) { continue; }
        let bottom = c["side"] == "bottom";
        step += &format!("        <Component refDes=\"{}\" packageRef=\"{}\" part=\"{}\" layerRef=\"{}\" mountType=\"{}\">\n", a(rf), a(&pat),
            part_id(c["pattern"].as_str().unwrap_or(""), c["value"].as_str().unwrap_or("")), if bottom { "BOTTOM" } else { "TOP" }, if *th_part { "THMT" } else { "SMT" });
        let rot = f(&c["rotation"]).rem_euclid(360.0);
        let has_rot = rot.abs() > 1e-6 && (rot - 360.0).abs() > 1e-6;
        if has_rot || bottom {
            step += &format!("          <Xform{}{}/>\n", if has_rot { format!(" rotation=\"{}\"", n(rot)) } else { String::new() }, if bottom { " mirror=\"true\"" } else { "" });
        }
        step += &format!("          <Location x=\"{}\" y=\"{}\"/>\n        </Component>\n", n(f(&c["x"])), n(f(&c["y"])));
    }
    // nets with their pins
    let mut nets: Vec<(String, Vec<&Value>)> = Vec::new();
    for p in &pads {
        let (Some(net), Some(r)) = (p["net"].as_str().filter(|s| !s.is_empty()), p["ref"].as_str().filter(|s| !s.is_empty())) else { continue };
        let _ = r;
        match nets.iter_mut().find(|x| x.0 == net) { Some(x) => x.1.push(p), None => nets.push((net.to_string(), vec![p])) }
    }
    nets.sort_by(|x, y| x.0.cmp(&y.0));
    for (net, ps) in &nets {
        step += &format!("        <LogicalNet name=\"{}\">\n", a(net));
        for p in ps { step += &format!("          <PinRef componentRef=\"{}\" pin=\"{}\"/>\n", a(p["ref"].as_str().unwrap_or("")), a(p["name"].as_str().unwrap_or(""))); }
        step += "        </LogicalNet>\n";
    }

    // copper, layer by layer: sets of features by net and use
    let zones = crate::input::pours(input);
    let (mut n_pads, mut n_traces, mut n_arcs, mut n_vias, mut n_islands) = (0, 0, 0, 0, 0);
    for (li, layer) in copper.iter().enumerate() {
        let mut sets: Vec<((String, &str), String)> = Vec::new();
        let mut put = |net: &str, kind: &'static str, body: String| {
            match sets.iter_mut().find(|s| s.0 .0 == net && s.0 .1 == kind) { Some(s) => s.1 += &body, None => sets.push(((net.to_string(), kind), body)) }
        };
        for p in &pads {
            let on = p["layer"].as_str() == Some(layer.as_str()) || p["layer"] == "*.Cu" || f(&p["drill"]) > 0.0;
            if !on { continue; }
            let rot = f(&p["rotation"]).rem_euclid(360.0);
            let prim = dict.pad(p["shape"].as_str().unwrap_or("rect"), f(&p["sizeX"]), f(&p["sizeY"]));
            let mut b = String::from("            <Pad>\n");
            if rot.abs() > 1e-6 && (rot - 360.0).abs() > 1e-6 { b += &format!("              <Xform rotation=\"{}\"/>\n", n(rot)); }
            b += &format!("              <Location x=\"{}\" y=\"{}\"/>\n              <StandardPrimitiveRef id=\"{prim}\"/>\n", n(f(&p["x"])), n(f(&p["y"])));
            if let Some(r) = p["ref"].as_str().filter(|s| !s.is_empty()) { b += &format!("              <PinRef componentRef=\"{}\" pin=\"{}\"/>\n", a(r), a(p["name"].as_str().unwrap_or(""))); }
            b += "            </Pad>\n";
            put(p["net"].as_str().unwrap_or(""), "pad", b);
            n_pads += 1;
        }
        for v in &vias {
            let ls: Vec<usize> = v["layers"].as_array().map(|x| x.iter().filter_map(|l| l.as_str().and_then(idx)).collect()).unwrap_or_default();
            let (lo, hi) = if ls.len() == 2 { (ls[0].min(ls[1]), ls[0].max(ls[1])) } else { (0, copper.len() - 1) };
            if li < lo || li > hi { continue; }
            let key = (n(f(&v["size"])), n(f(&v["drill"])));
            let vn = &vstacks.iter().find(|x| x.0 == key).unwrap().1;
            let prim = dict.pad("round", f(&v["size"]), f(&v["size"]));
            put(v["net"].as_str().unwrap_or(""), "via", format!("            <Pad padstackDefRef=\"{vn}\">\n              <Location x=\"{}\" y=\"{}\"/>\n              <StandardPrimitiveRef id=\"{prim}\"/>\n            </Pad>\n", n(f(&v["position"]["x"])), n(f(&v["position"]["y"]))));
            n_vias += 1;
        }
        for t in arr(st, "tracks").iter().filter(|t| t["layer"].as_str() == Some(layer.as_str())) {
            put(t["net"].as_str().unwrap_or(""), "trace", format!("            <Features>\n              <Line startX=\"{}\" startY=\"{}\" endX=\"{}\" endY=\"{}\">\n                <LineDesc lineEnd=\"ROUND\" lineWidth=\"{}\"/>\n              </Line>\n            </Features>\n",
                n(f(&t["start"]["x"])), n(f(&t["start"]["y"])), n(f(&t["end"]["x"])), n(f(&t["end"]["y"])), n(f(&t["width"]))));
            n_traces += 1;
        }
        for r in arr(st, "arcs").iter().filter(|r| r["layer"].as_str() == Some(layer.as_str())) {
            let (cx, cy, rad) = (f(&r["center"]["x"]), f(&r["center"]["y"]), f(&r["radius"]));
            let (s0, s1) = (f(&r["startAngle"]).to_radians(), f(&r["endAngle"]).to_radians());
            put(r["net"].as_str().unwrap_or(""), "trace", format!("            <Features>\n              <Arc startX=\"{}\" startY=\"{}\" endX=\"{}\" endY=\"{}\" centerX=\"{}\" centerY=\"{}\" clockwise=\"false\">\n                <LineDesc lineEnd=\"ROUND\" lineWidth=\"{}\"/>\n              </Arc>\n            </Features>\n",
                n(cx + rad * s0.cos()), n(cy + rad * s0.sin()), n(cx + rad * s1.cos()), n(cy + rad * s1.sin()), n(cx), n(cy), n(f(&r["width"]))));
            n_arcs += 1;
        }
        for z in zones.iter().filter(|z| z["layer"].as_str() == Some(layer.as_str())) {
            for fill in arr(z, "fills") {
                let ring = |v: &Value| -> Vec<(f64, f64)> { v.as_array().cloned().unwrap_or_default().iter().map(|q| (f(&q[0]), f(&q[1]))).collect() };
                let outer = ring(&fill["outline"]);
                if outer.len() < 3 { continue; }
                let mut b = String::from("            <Features>\n              <Contour>\n");
                b += &ring_xml("Polygon", &outer, "                ");
                for h in arr(&fill, "holes") { let h = ring(&h); if h.len() >= 3 { b += &ring_xml("Cutout", &h, "                "); } }
                b += "              </Contour>\n            </Features>\n";
                put(z["net"].as_str().unwrap_or(""), "pour", b);
                n_islands += 1;
            }
        }
        step += &format!("        <LayerFeature layerRef=\"{}\">\n", lname(li));
        for ((net, kind), body) in &sets {
            let mut attrs = if net.is_empty() { String::new() } else { format!(" net=\"{}\"", a(net)) };
            if *kind == "pad" { attrs += " padUsage=\"TERMINATION\""; } else if *kind == "via" { attrs += " padUsage=\"VIA\""; }
            step += &format!("          <Set{attrs}>\n{body}          </Set>\n");
        }
        step += "        </LayerFeature>\n";
    }
    // the drills: plated pad holes (a hole with no copper ring round it is non-plated), vias
    let mut holes: Vec<(String, f64, f64, f64, &str)> = pads.iter().filter(|p| f(&p["drill"]) > 0.0)
        .map(|p| (p["net"].as_str().unwrap_or("").to_string(), f(&p["x"]), f(&p["y"]), f(&p["drill"]), if f(&p["sizeX"]).min(f(&p["sizeY"])) <= f(&p["drill"]) + 1e-6 { "NONPLATED" } else { "PLATED" })).collect();
    holes.extend(vias.iter().map(|v| (v["net"].as_str().unwrap_or("").to_string(), f(&v["position"]["x"]), f(&v["position"]["y"]), f(&v["drill"]), "VIA")));
    step += "        <LayerFeature layerRef=\"DRILL_TOP_BOTTOM\">\n";
    let mut hnets: Vec<&String> = holes.iter().map(|h| &h.0).collect();
    hnets.sort();
    hnets.dedup();
    let mut k = 0;
    for net in hnets {
        step += &format!("          <Set{}>\n", if net.is_empty() { String::new() } else { format!(" net=\"{}\"", a(net)) });
        for h in holes.iter().filter(|h| &h.0 == net) {
            k += 1;
            step += &format!("            <Hole name=\"H{k}\" diameter=\"{}\" platingStatus=\"{}\" plusTol=\"0\" minusTol=\"0\" x=\"{}\" y=\"{}\"/>\n", n(h.3), h.4, n(h.1), n(h.2));
        }
        step += "          </Set>\n";
    }
    step += "        </LayerFeature>\n      </Step>\n";
    cad += &step;
    cad += "    </CadData>\n";

    let soft = format!("adom-eda-checks {}", env!("CARGO_PKG_VERSION"));
    let mut o = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<IPC-2581 revision=\"C\" xmlns=\"http://webstds.ipc.org/2581\">\n  <Content roleRef=\"Owner\">\n    <FunctionMode mode=\"ASSEMBLY\"/>\n");
    o += &format!("    <StepRef name=\"{name}\"/>\n");
    for i in 0..copper.len() { o += &format!("    <LayerRef name=\"{}\"/>\n", lname(i)); }
    o += "    <LayerRef name=\"DRILL_TOP_BOTTOM\"/>\n";
    o += &format!("    <BomRef name=\"{name}_BOM\"/>\n");
    o += &dict.xml();
    o += "  </Content>\n  <LogisticHeader>\n    <Role id=\"Owner\" roleFunction=\"SENDER\"/>\n    <Enterprise id=\"Adom\" code=\"NONE\"/>\n    <Person name=\"adom-eda-checks\" enterpriseRef=\"Adom\" roleRef=\"Owner\"/>\n  </LogisticHeader>\n";
    o += &format!("  <HistoryRecord number=\"1\" origination=\"{date}\" software=\"{soft}\" lastChange=\"{date}\">\n    <FileRevision fileRevisionId=\"1\" comment=\"from the board model\">\n      <SoftwarePackage name=\"adom-eda-checks\" revision=\"{}\" vendor=\"Adom\">\n        <Certification certificationStatus=\"SELFTEST\"/>\n      </SoftwarePackage>\n    </FileRevision>\n  </HistoryRecord>\n", env!("CARGO_PKG_VERSION"));
    o += &bom;
    o += "  <Ecad name=\"";
    o += &name;
    o += "\">\n    <CadHeader units=\"MILLIMETER\"/>\n";
    o += &cad;
    o += "  </Ecad>\n</IPC-2581>\n";
    let counts = json!({"layers": copper.len() + 1, "packages": made.len(), "components": comps.iter().filter(|c| pads.iter().any(|p| p["ref"] == c["ref"])).count(), "nets": nets.len(),
        "pads": n_pads, "traces": n_traces, "arcs": n_arcs, "vias": n_vias, "pourIslands": n_islands, "holes": k});
    (o, counts)
}

/// The current UTC time as an xsd:dateTime, for the history record when the caller gives no `date`.
pub(crate) fn now() -> String {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (s.div_euclid(86400), s.rem_euclid(86400));
    // civil from days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// writers::ipc2581: `{board, stack?, pours?, args: {title?, date?}}` -> the IPC-2581C text and what it holds.
pub fn export_ipc2581(input: &Value) -> Value {
    let board = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = input.get("args").cloned().unwrap_or(json!({}));
    let title = args["title"].as_str().unwrap_or("board");
    let date = args["date"].as_str().map(String::from).unwrap_or_else(now);
    let (text, counts) = ipc2581_text(&board, input, title, &date);
    let poured = !crate::input::pours(input).is_empty();
    json!({"success": true, "text": text, "counts": counts, "format": "IPC-2581 revision C", "mode": "ASSEMBLY", "units": "mm", "writtenFrom": "board model",
        "_hint": if poured { "IPC-2581C written from the board model (valid against the IPC-2581C schema): layers and stack-up, outline, packages and components, BOM, nets, and on each copper layer the pads, traces, arcs, vias and the poured copper island by island, plus the drills. Solder mask, paste and silkscreen are not in it: a fab order goes through the tool's Gerbers or ODB++." }
                 else { "IPC-2581C written from the board model, without pours: pass pour_state as pours to add the poured copper island by island. Solder mask, paste and silkscreen are not in it: a fab order goes through the tool's Gerbers or ODB++." }})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Value {
        json!({"outlineEdges": [{"kind": "line", "start": [0.0, 0.0]}, {"kind": "line", "start": [30.0, 0.0]}, {"kind": "arc", "start": [30.0, 20.0], "end": [0.0, 20.0], "center": [15.0, 20.0], "radius": 15.0, "angle1": 0.0, "angle2": 180.0}, {"kind": "line", "start": [0.0, 20.0]}],
            "components": [{"ref": "U1", "x": 10.0, "y": 10.0, "rotation": 90.0, "side": "top", "pattern": "SOT23", "value": "LDO 3.3V"}, {"ref": "R1", "x": 20.0, "y": 5.0, "rotation": 0.0, "side": "bottom", "pattern": "0603", "value": "10kΩ"},
                           {"ref": "LOGO", "x": 2.0, "y": 2.0, "rotation": 0.0, "side": "top", "pattern": "LOGO"}],
            "pads": [{"ref": "U1", "name": "1", "x": 9.05, "y": 8.9, "sizeX": 0.6, "sizeY": 0.7, "rotation": 90.0, "shape": "rect", "layer": "F.Cu", "drill": 0.0, "net": "VIN"},
                     {"ref": "U1", "name": "2", "x": 10.95, "y": 8.9, "sizeX": 0.6, "sizeY": 0.7, "rotation": 90.0, "shape": "rect", "layer": "F.Cu", "drill": 0.0, "net": "GND"},
                     {"ref": "U1", "name": "3", "x": 10.0, "y": 11.1, "sizeX": 0.6, "sizeY": 0.7, "rotation": 90.0, "shape": "rect", "layer": "F.Cu", "drill": 0.0, "net": "VOUT"},
                     {"ref": "R1", "name": "1", "x": 19.2, "y": 5.0, "sizeX": 0.8, "sizeY": 0.9, "rotation": 0.0, "shape": "rect", "layer": "B.Cu", "drill": 0.0, "net": "VOUT"},
                     {"ref": "R1", "name": "2", "x": 20.8, "y": 5.0, "sizeX": 0.8, "sizeY": 0.9, "rotation": 0.0, "shape": "rect", "layer": "B.Cu", "drill": 0.0, "net": "GND"},
                     {"ref": "", "name": "", "x": 3.0, "y": 17.0, "sizeX": 3.2, "sizeY": 3.2, "rotation": 0.0, "shape": "round", "layer": "*.Cu", "drill": 3.2, "net": ""}],
            "tracks": [{"layer": "F.Cu", "width": 0.25, "net": "VOUT", "start": {"x": 10.0, "y": 11.1}, "end": {"x": 15.0, "y": 11.1}}],
            "arcs": [{"layer": "B.Cu", "width": 0.25, "net": "VOUT", "center": {"x": 17.0, "y": 8.0}, "radius": 2.0, "startAngle": 0.0, "endAngle": 90.0}],
            "vias": [{"position": {"x": 15.0, "y": 11.1}, "size": 0.6, "drill": 0.3, "layers": ["F.Cu", "B.Cu"], "net": "VOUT"}]})
    }

    #[test]
    fn the_board_comes_out_as_ipc_2581c() {
        let pours = json!({"zones": [{"net": "GND", "layer": "B.Cu", "fills": [{"outline": [[1.0, 1.0], [29.0, 1.0], [29.0, 9.0], [1.0, 9.0]], "holes": [[[18.5, 4.0], [21.5, 4.0], [21.5, 6.0], [18.5, 6.0]]]}]}]});
        let r = export_ipc2581(&json!({"board": board(), "pours": pours, "args": {"title": "Test Board", "date": "2026-10-29T00:00:00Z"}}));
        let t = r["text"].as_str().unwrap();
        assert!(t.starts_with("<?xml") && t.contains("<IPC-2581 revision=\"C\""));
        assert_eq!(r["counts"], json!({"layers": 3, "packages": 2, "components": 2, "nets": 3, "pads": 7, "traces": 1, "arcs": 1, "vias": 2, "pourIslands": 1, "holes": 2}), "{}", r["counts"]);
        // U1's pin 1 back in the footprint's frame: (9.05, 8.9) less (10, 10), turned back 90 degrees -> (-1.1, 0.95)
        assert!(t.contains("<Pin number=\"1\" type=\"SURFACE\">\n            <Location x=\"-1.1\" y=\"0.95\"/>"), "{t}");
        // the bottom resistor is mirrored, the mounting hole has no ring so it is non-plated, the ohm sign is spelled out
        assert!(t.contains("refDes=\"R1\" packageRef=\"0603\" part=\"0603_10kohm\" layerRef=\"BOTTOM\" mountType=\"SMT\">\n          <Xform mirror=\"true\"/>"));
        assert!(t.contains("platingStatus=\"NONPLATED\"") && t.contains("platingStatus=\"VIA\""));
        assert!(t.contains("<PolyStepCurve x=\"0\" y=\"20\" centerX=\"15\" centerY=\"20\" clockwise=\"false\"/>"), "{t}");
        assert!(t.contains("<Cutout>") && t.contains("<Oval") == false && t.contains("<Circle diameter=\"3.2\"/>"));
        assert!(!t.contains("LOGO"), "a component with no pads is no part");
        assert_eq!(n(0.10000001), "0.1");
        assert_eq!(n(-0.00001), "0");
        assert_eq!(ascii("10kΩ ±5%"), "10kohm +/-5%");
    }

    #[test]
    fn the_history_date_is_an_xsd_date_time() {
        let d = now();
        assert_eq!(d.len(), 20);
        assert!(d.starts_with("20") && d.ends_with('Z') && &d[10..11] == "T", "{d}");
    }
}

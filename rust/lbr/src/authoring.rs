//! Library authoring on EAGLE .lbr text, offline: the standard's create_footprint, create_symbol,
//! read_symbol, find_symbol, copy/rename/remove/xcopy_footprint, clone/rename/remove_symbol and
//! set_params. In EAGLE a footprint is a <package>; the standard's "symbol" (a component you place)
//! is a <deviceset> with the <symbol>s its gates use. A symbol made here keeps its pin numbers on a
//! package-less device (attribute ADOM_PINMAP, "number:pin;..."), the way EAGLE's own supply
//! symbols carry no package, so set_params {footprint} can join pins to pads later.

use crate::Element;
use serde_json::{json, Value};

const TEMPLATE_HEAD: &str = include_str!("lbr_template_head.xml");
const MIL: f64 = 0.0254;

/// A new, empty library document (the layer table of a Fusion library).
pub fn new_library(description: &str) -> String {
    format!("{TEMPLATE_HEAD}<library>\n<description>{}</description>\n<packages>\n</packages>\n<symbols>\n</symbols>\n<devicesets>\n</devicesets>\n</library>\n</drawing>\n</eagle>\n",
            description.replace('&', "&amp;").replace('<', "&lt;"))
}

pub(crate) fn f(v: f64) -> String { let s = format!("{:.4}", v); let s = s.trim_end_matches('0').trim_end_matches('.').to_string(); if s == "-0" { "0".into() } else { s } }
pub(crate) fn el(tag: &str, attrs: &[(&str, String)]) -> Element { let mut e = Element::new(tag); e.attrs = attrs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(); e }
pub(crate) fn set(e: &mut Element, k: &str, v: &str) { match e.attrs.iter_mut().find(|(kk, _)| kk == k) { Some(a) => a.1 = v.to_string(), None => e.attrs.push((k.into(), v.into())) } }
pub(crate) fn lib_mut(root: &mut Element) -> Result<&mut Element, String> { root.find_deep_mut("library").ok_or_else(|| "not a library: no <library>".to_string()) }
pub(crate) fn section<'a>(lib: &'a mut Element, tag: &str) -> &'a mut Element {
    if lib.find_child(tag).is_none() {
        // EAGLE order inside <library>: description?, packages, packages3d?, symbols, devicesets
        let order = ["description", "packages", "packages3d", "symbols", "devicesets"];
        let rank = |t: &str| order.iter().position(|o| *o == t).unwrap_or(9);
        let at = lib.children.iter().position(|c| rank(&c.tag) > rank(tag)).unwrap_or(lib.children.len());
        lib.children.insert(at, Element::new(tag));
    }
    lib.find_child_mut(tag).unwrap()
}
pub(crate) fn names(sec: &Element, tag: &str) -> Vec<String> { sec.children.iter().filter(|c| c.tag == tag).filter_map(|c| c.attr("name").map(str::to_string)).collect() }
pub(crate) fn find_ci<'a>(list: &'a [String], want: &str) -> Option<&'a String> { list.iter().find(|n| *n == want).or_else(|| list.iter().find(|n| n.eq_ignore_ascii_case(want))) }

/// Pads as the standard writes them: "des,x_mil,y_mil,w_mil,h_mil,rect|round,hole_mil;..." (hole 0 = SMD),
/// or a JSON list of {number, at:[x,y] mm, sizeMm:[w,h], shape, drillMm}.
fn parse_pads(v: &Value) -> Result<Vec<Element>, String> {
    let mut out = Vec::new();
    let mk = |des: &str, x: f64, y: f64, w: f64, h: f64, round: bool, hole: f64| -> Element {
        if hole > 0.0 {
            let mut e = el("pad", &[("name", des.into()), ("x", f(x)), ("y", f(y)), ("drill", f(hole)), ("diameter", f(w.max(h)))]);
            if !round { e.attrs.push(("shape".into(), "square".into())); }
            if (w - h).abs() > 1e-6 { e.attrs.retain(|a| a.0 != "shape"); e.attrs.push(("shape".into(), "long".into())); if h > w { e.attrs.push(("rot".into(), "R90".into())); } set(&mut e, "diameter", &f(w.min(h))); }
            e
        } else {
            let mut e = el("smd", &[("name", des.into()), ("x", f(x)), ("y", f(y)), ("dx", f(w)), ("dy", f(h)), ("layer", "1".into())]);
            if round { e.attrs.push(("roundness".into(), "100".into())); }
            e
        }
    };
    if let Some(s) = v.as_str() {
        for (i, item) in s.split(';').map(str::trim).filter(|t| !t.is_empty()).enumerate() {
            let p: Vec<&str> = item.split(',').map(str::trim).collect();
            if p.len() < 6 { return Err(format!("pad {}: want des,x_mil,y_mil,w_mil,h_mil,rect|round[,hole_mil], got {item}", i + 1)); }
            let n = |k: usize| p.get(k).and_then(|t| t.parse::<f64>().ok()).unwrap_or(0.0);
            out.push(mk(p[0], n(1) * MIL, n(2) * MIL, n(3) * MIL, n(4) * MIL, p[5].eq_ignore_ascii_case("round"), n(6) * MIL));
        }
    } else if let Some(a) = v.as_array() {
        for (i, p) in a.iter().enumerate() {
            let des = p["number"].as_str().map(str::to_string).or(p["number"].as_u64().map(|n| n.to_string())).ok_or(format!("pad {}: number", i + 1))?;
            let at = p["at"].as_array().ok_or(format!("pad {des}: at [x, y] mm"))?;
            let sz = p["sizeMm"].as_array().ok_or(format!("pad {des}: sizeMm [w, h]"))?;
            let g = |a: &Vec<Value>, k: usize| a.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
            out.push(mk(&des, g(at, 0), g(at, 1), g(sz, 0), g(sz, 1), p["shape"].as_str() == Some("round"), p["drillMm"].as_f64().unwrap_or(0.0)));
        }
    } else { return Err("pads is required".into()); }
    if out.is_empty() { return Err("pads is empty".into()); }
    let mut seen = std::collections::HashSet::new();
    for e in &out { let n = e.attr("name").unwrap_or(""); if !seen.insert(n.to_string()) { return Err(format!("two pads are numbered {n}")); } }
    Ok(out)
}

/// The standard's create_footprint: a package from a pad list, with a documentation outline round
/// the pads and >NAME / >VALUE placeholders. Several at once with `footprints`.
pub fn create_footprint(xml: Option<&str>, args: &Value) -> Result<(String, Value), String> {
    let text = xml.map(str::to_string).unwrap_or_else(|| new_library("made by fusion-bridge"));
    let mut root = Element::parse(&text).map_err(|e| e.to_string())?;
    let specs: Vec<Value> = match args["footprints"].as_array() { Some(a) => a.iter().map(|x| { let mut m = args.clone(); for (k, v) in x.as_object().cloned().unwrap_or_default() { m[k] = v; } m }).collect(), None => vec![args.clone()] };
    let mut made = Vec::new();
    {
        let lib = lib_mut(&mut root)?;
        for spec in &specs {
            let name = spec["name"].as_str().filter(|s| !s.is_empty()).ok_or("name is required")?.to_string();
            let pads = parse_pads(&spec["pads"])?;
            let sec = section(lib, "packages");
            if find_ci(&names(sec, "package"), &name).is_some() { return Err(format!("footprint {name} already exists (rename_footprint, remove_footprint, or another name)")); }
            let mut pkg = el("package", &[("name", name.clone())]);
            if let Some(d) = spec["description"].as_str() { let mut de = Element::new("description"); de.text = d.into(); pkg.children.push(de); }
            let ext = |e: &Element| -> (f64, f64, f64, f64) {
                let g = |k: &str| e.attr(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
                let (x, y) = (g("x"), g("y"));
                let (hw, hh) = if e.tag == "smd" { (g("dx") / 2.0, g("dy") / 2.0) } else { let d = g("diameter").max(g("drill") * 1.6) / 2.0; (d, d) };
                (x - hw, y - hh, x + hw, y + hh)
            };
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for p in &pads { let (a, b, c, d) = ext(p); x0 = x0.min(a); y0 = y0.min(b); x1 = x1.max(c); y1 = y1.max(d); }
            let n = pads.len();
            pkg.children.extend(pads);
            let m = 0.25;
            for (a, b) in [((x0 - m, y0 - m), (x1 + m, y0 - m)), ((x1 + m, y0 - m), (x1 + m, y1 + m)), ((x1 + m, y1 + m), (x0 - m, y1 + m)), ((x0 - m, y1 + m), (x0 - m, y0 - m))] {
                pkg.children.push(el("wire", &[("x1", f(a.0)), ("y1", f(a.1)), ("x2", f(b.0)), ("y2", f(b.1)), ("width", "0.1".into()), ("layer", "51".into())]));
            }
            let mut tn = el("text", &[("x", f(x0 - m)), ("y", f(y1 + m + 0.3)), ("size", "1".into()), ("layer", "25".into())]); tn.text = ">NAME".into();
            let mut tv = el("text", &[("x", f(x0 - m)), ("y", f(y0 - m - 1.3)), ("size", "1".into()), ("layer", "27".into())]); tv.text = ">VALUE".into();
            pkg.children.push(tn); pkg.children.push(tv);
            section(lib, "packages").children.push(pkg);
            made.push(json!({"footprint": name, "pads": n}));
        }
    }
    let state = if xml.is_some() { "opened" } else { "created" };
    Ok((root.to_xml_document(), json!({"footprints": made, "libraryState": state})))
}

/// A package from a laid-out land pattern (the shape the standard's footprint_from_land_pattern
/// returns): pads {name, x, y, w, h, shape, hole} in mm, Y up; lines {layer: overlay|fab|courtyard,
/// x1, y1, x2, y2, width}; discs {layer, x, y, r}. Silk goes on tPlace (21), fab on tDocu (51), the
/// courtyard on tKeepout (39), which is where read_footprint looks for it.
pub fn create_from_layout(xml: Option<&str>, name: &str, description: &str, layout: &Value) -> Result<(String, Value), String> {
    let text = xml.map(str::to_string).unwrap_or_else(|| new_library("made by fusion-bridge"));
    let mut root = Element::parse(&text).map_err(|e| e.to_string())?;
    let layer = |l: &str| -> Result<&'static str, String> { match l { "overlay" | "silk" => Ok("21"), "fab" | "balls" => Ok("51"), "courtyard" => Ok("39"), "comment" | "document" => Ok("48"), "copper" | "top" => Ok("1"), o => Err(format!("unknown layer {o}")) } };
    let g = |v: &Value, k: &str| v[k].as_f64().unwrap_or(0.0);
    let pads = layout["pads"].as_array().filter(|a| !a.is_empty()).ok_or("the layout has no pads")?;
    let mut pkg = el("package", &[("name", name.to_string())]);
    if !description.is_empty() { let mut de = Element::new("description"); de.text = description.into(); pkg.children.push(de); }
    let mut seen: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for p in pads {
        let mut n = p["name"].as_str().map(str::to_string).or(p["name"].as_u64().map(|n| n.to_string())).ok_or("a pad has no name")?;
        // EAGLE pad names are unique; a spec that repeats one (the heatsink-via convention) gets
        // N@2, N@3 ..., which a device connects to the same pin
        let seen_n = seen.entry(n.clone()).or_default();
        if !seen_n.is_empty() {
            if layout["repeatedPads"].as_bool() != Some(true) { return Err(format!("two pads are numbered {n}")); }
            n = format!("{n}@{}", seen_n.len() + 1);
        }
        seen_n.push(n.clone());
        let (x, y, w, h, hole) = (g(p, "x"), g(p, "y"), g(p, "w"), g(p, "h"), g(p, "hole"));
        let shape = p["shape"].as_str().unwrap_or("roundrect");
        let e = if hole > 0.0 {
            let mut e = el("pad", &[("name", n), ("x", f(x)), ("y", f(y)), ("drill", f(hole)), ("diameter", f(w.min(h)))]);
            if (w - h).abs() > 1e-6 { e.attrs.push(("shape".into(), "long".into())); if h > w { e.attrs.push(("rot".into(), "R90".into())); } }
            else if shape != "round" { e.attrs.push(("shape".into(), "square".into())); }
            e
        } else {
            let mut e = el("smd", &[("name", n), ("x", f(x)), ("y", f(y)), ("dx", f(w)), ("dy", f(h)), ("layer", "1".into())]);
            // the engine's cornerRatio (0.25 for roundrect) is EAGLE's roundness in percent
            let round = match (shape, p["cornerRatio"].as_f64()) { ("round" | "oval", _) => 100, (_, Some(c)) if c > 0.0 => (c * 100.0).round() as i64, ("roundrect", _) => 25, _ => 0 };
            if round > 0 { e.attrs.push(("roundness".into(), round.to_string())); }
            e
        };
        pkg.children.push(e);
    }
    let mut per_layer: std::collections::BTreeMap<&str, usize> = Default::default();
    for l in layout["lines"].as_array().into_iter().flatten() {
        let ly = layer(l["layer"].as_str().unwrap_or(""))?;
        *per_layer.entry(ly).or_default() += 1;
        pkg.children.push(el("wire", &[("x1", f(g(l, "x1"))), ("y1", f(g(l, "y1"))), ("x2", f(g(l, "x2"))), ("y2", f(g(l, "y2"))), ("width", f(g(l, "width").max(0.01))), ("layer", ly.into())]));
    }
    for a in layout["arcs"].as_array().into_iter().flatten() {
        let ly = layer(a["layer"].as_str().unwrap_or(""))?;
        *per_layer.entry(ly).or_default() += 1;
        let (cx, cy, r, w, a0, a1) = (g(a, "cx"), g(a, "cy"), g(a, "r"), g(a, "width").max(0.01), g(a, "a0"), g(a, "a1"));
        let sweep = a1 - a0;
        if sweep.abs() >= 359.999 { pkg.children.push(el("circle", &[("x", f(cx)), ("y", f(cy)), ("radius", f(r)), ("width", f(w)), ("layer", ly.into())])); continue; }
        let pt = |deg: f64| (cx + r * deg.to_radians().cos(), cy + r * deg.to_radians().sin());
        let (p0, p1) = (pt(a0), pt(a1));
        pkg.children.push(el("wire", &[("x1", f(p0.0)), ("y1", f(p0.1)), ("x2", f(p1.0)), ("y2", f(p1.1)), ("width", f(w)), ("layer", ly.into()), ("curve", f(sweep))]));
    }
    for d in layout["discs"].as_array().into_iter().flatten() {
        let ly = layer(d["layer"].as_str().unwrap_or(""))?;
        // a filled dot: EAGLE draws a circle of width 0 filled
        pkg.children.push(el("circle", &[("x", f(g(d, "x"))), ("y", f(g(d, "y"))), ("radius", f(g(d, "r"))), ("width", "0".into()), ("layer", ly.into())]));
    }
    // >NAME above and >VALUE below whatever the package covers
    let (mut y0, mut y1, mut x0) = (f64::MAX, f64::MIN, f64::MAX);
    for p in pads { y0 = y0.min(g(p, "y") - g(p, "h") / 2.0); y1 = y1.max(g(p, "y") + g(p, "h") / 2.0); x0 = x0.min(g(p, "x") - g(p, "w") / 2.0); }
    for l in layout["lines"].as_array().into_iter().flatten() { for k in ["1", "2"] { y0 = y0.min(g(l, &format!("y{k}"))); y1 = y1.max(g(l, &format!("y{k}"))); x0 = x0.min(g(l, &format!("x{k}"))); } }
    let mut tn = el("text", &[("x", f(x0)), ("y", f(y1 + 0.3)), ("size", "1".into()), ("layer", "25".into())]); tn.text = ">NAME".into();
    let mut tv = el("text", &[("x", f(x0)), ("y", f(y0 - 1.3)), ("size", "1".into()), ("layer", "27".into())]); tv.text = ">VALUE".into();
    pkg.children.push(tn); pkg.children.push(tv);
    {
        let lib = lib_mut(&mut root)?;
        let sec = section(lib, "packages");
        if find_ci(&names(sec, "package"), name).is_some() { return Err(format!("footprint {name} already exists (rename_footprint, remove_footprint, or another name)")); }
        sec.children.push(pkg);
    }
    let lines = |k: &str| per_layer.get(k).copied().unwrap_or(0);
    let aliases: std::collections::BTreeMap<String, Vec<String>> = seen.into_iter().filter(|(_, v)| v.len() > 1).collect();
    Ok((root.to_xml_document(), json!({"footprint": name, "pads": pads.len(), "silkLines": lines("21"), "fabLines": lines("51"), "courtyardLines": lines("39"), "padAliases": aliases,
        "libraryState": if xml.is_some() { "opened" } else { "created" }})))
}

fn direction_for(name: &str, hidden: bool) -> &'static str {
    let u = name.to_uppercase();
    if hidden || ["GND", "VCC", "VDD", "VSS", "VEE", "V+", "V-", "AGND", "DGND", "PGND", "VIN", "VBAT", "3V3", "5V"].iter().any(|p| u == *p || u.starts_with(&format!("{p}@"))) { "pwr" } else { "pas" }
}

/// The standard's create_symbol: a component (deviceset) with one gate per part, each a
/// rectangular symbol with its pins on the sides asked; part 0 pins and hidden-net pins go on a
/// power gate placed on request. Pin numbers are kept for set_params {footprint}.
pub fn create_symbol(xml: Option<&str>, args: &Value) -> Result<(String, Value), String> {
    let text = xml.map(str::to_string).unwrap_or_else(|| new_library("made by fusion-bridge"));
    let mut root = Element::parse(&text).map_err(|e| e.to_string())?;
    let specs: Vec<Value> = match args["symbols"].as_array() { Some(a) => a.iter().map(|x| { let mut m = args.clone(); for (k, v) in x.as_object().cloned().unwrap_or_default() { m[k] = v; } m }).collect(), None => vec![args.clone()] };
    let mut made = Vec::new();
    {
        let lib = lib_mut(&mut root)?;
        for spec in &specs {
            let name = spec["name"].as_str().filter(|s| !s.is_empty()).ok_or("name is required")?.to_string();
            if find_ci(&names(section(lib, "devicesets"), "deviceset"), &name).is_some() { return Err(format!("symbol {name} already exists")); }
            let pins_s = spec["pins"].as_str().ok_or("pins is required: 'des,name,left|right|top|bottom,offset_mil[,part[,hiddenNet]];...'")?;
            let bw = spec["bodyWidthMil"].as_f64().unwrap_or(800.0) * MIL;
            let bh = spec["bodyHeightMil"].as_f64().unwrap_or(600.0) * MIL;
            // part -> [(des, name, side, offset_mm, hidden)]
            let mut parts: std::collections::BTreeMap<i64, Vec<(String, String, String, f64, bool)>> = Default::default();
            for (i, item) in pins_s.split(';').map(str::trim).filter(|t| !t.is_empty()).enumerate() {
                let p: Vec<&str> = item.split(',').map(str::trim).collect();
                if p.len() < 4 { return Err(format!("pin {}: want des,name,side,offset_mil, got {item}", i + 1)); }
                let side = p[2].to_lowercase();
                if !["left", "right", "top", "bottom"].contains(&side.as_str()) { return Err(format!("pin {}: side is left, right, top or bottom", p[0])); }
                let part: i64 = p.get(4).and_then(|v| v.parse().ok()).unwrap_or(1);
                let hidden = p.get(5).map(|v| !v.is_empty()).unwrap_or(false);
                let pname = if hidden { p[5].to_string() } else { p[1].to_string() };
                let key = if part == 0 || hidden { 0 } else { part };
                parts.entry(key).or_default().push((p[0].to_string(), pname, side, p[3].parse::<f64>().unwrap_or(0.0) * MIL, hidden));
            }
            if parts.is_empty() { return Err("pins is empty".into()); }
            let real: Vec<i64> = parts.keys().cloned().filter(|k| *k != 0).collect();
            let multi = real.len() > 1;
            let mut gates = Element::new("gates");
            let mut pinmap: Vec<String> = Vec::new();
            let mut sym_count = 0;
            for (k, pins) in &parts {
                let suffix = if *k == 0 { "_PWR".to_string() } else if multi { format!("_{}", (b'A' + (*k as u8 - 1)) as char) } else { String::new() };
                let sname = format!("{name}{suffix}");
                let mut sym = el("symbol", &[("name", sname.clone())]);
                let (hw, hh) = (bw / 2.0, bh / 2.0);
                for (a, b) in [((-hw, -hh), (hw, -hh)), ((hw, -hh), (hw, hh)), ((hw, hh), (-hw, hh)), ((-hw, hh), (-hw, -hh))] {
                    sym.children.push(el("wire", &[("x1", f(a.0)), ("y1", f(a.1)), ("x2", f(b.0)), ("y2", f(b.1)), ("width", "0.254".into()), ("layer", "94".into())]));
                }
                let mut tn = el("text", &[("x", f(-hw)), ("y", f(hh + 0.8)), ("size", "1.778".into()), ("layer", "95".into())]); tn.text = ">NAME".into();
                let mut tv = el("text", &[("x", f(-hw)), ("y", f(-hh - 2.5)), ("size", "1.778".into()), ("layer", "96".into())]); tv.text = ">VALUE".into();
                sym.children.push(tn); sym.children.push(tv);
                let mut used: std::collections::HashMap<String, usize> = Default::default();
                let len = 5.08;
                for (des, pname, side, off, hidden) in pins {
                    let c = used.entry(pname.to_uppercase()).or_insert(0); *c += 1;
                    let unique = if *c > 1 { format!("{pname}@{c}") } else { pname.clone() };
                    let (x, y, rot) = match side.as_str() {
                        "left" => (-hw - len, hh - off, "R0"), "right" => (hw + len, hh - off, "R180"),
                        "top" => (-hw + off, hh + len, "R270"), _ => (-hw + off, -hh - len, "R90"),
                    };
                    let mut pe = el("pin", &[("name", unique.clone()), ("x", f(x)), ("y", f(y)), ("length", "middle".into()), ("direction", direction_for(&unique, *hidden).into())]);
                    if rot != "R0" { pe.attrs.push(("rot".into(), rot.into())); }
                    if *hidden { pe.attrs.push(("visible".into(), "off".into())); }
                    sym.children.push(pe);
                    let gname = if *k == 0 { "P".to_string() } else if multi { ((b'A' + (*k as u8 - 1)) as char).to_string() } else { "G$1".into() };
                    pinmap.push(format!("{des}:{gname}.{unique}"));
                }
                section(lib, "symbols").children.push(sym); sym_count += 1;
                let gname = if *k == 0 { "P".to_string() } else if multi { ((b'A' + (*k as u8 - 1)) as char).to_string() } else { "G$1".into() };
                let mut g = el("gate", &[("name", gname), ("symbol", sname), ("x", f((gates.children.len() as f64) * (bw + 15.24))), ("y", "0".into())]);
                if *k == 0 { g.attrs.push(("addlevel".into(), "request".into())); }
                gates.children.push(g);
            }
            let mut ds = el("deviceset", &[("name", name.clone()), ("prefix", spec["designator"].as_str().unwrap_or("U").to_string())]);
            if let Some(d) = spec["description"].as_str() { let mut de = Element::new("description"); de.text = d.into(); ds.children.push(de); }
            ds.children.push(gates);
            let mut devs = Element::new("devices");
            let mut dev = el("device", &[("name", "".into())]);
            let mut techs = Element::new("technologies");
            let mut tech = el("technology", &[("name", "".into())]);
            tech.children.push(el("attribute", &[("name", "ADOM_PINMAP".into()), ("value", pinmap.join(";")), ("constant", "no".into())]));
            techs.children.push(tech); dev.children.push(techs); devs.children.push(dev); ds.children.push(devs);
            section(lib, "devicesets").children.push(ds);
            made.push(json!({"symbol": name, "pins": pinmap.len(), "parts": real.len().max(1), "symbols": sym_count}));
        }
    }
    let state = if xml.is_some() { "opened" } else { "created" };
    Ok((root.to_xml_document(), json!({"symbols": made, "libraryState": state})))
}

fn pin_number_map(ds: &Element) -> Vec<(String, String, String)> {
    // (number, gate, pin) from a device's connects, else from ADOM_PINMAP
    let mut out = Vec::new();
    if let Some(devs) = ds.find_child("devices") {
        for d in devs.children.iter().filter(|c| c.tag == "device") {
            if let Some(cs) = d.find_child("connects") {
                for c in cs.children.iter().filter(|c| c.tag == "connect") {
                    for pad in c.attr("pad").unwrap_or("").split(' ').filter(|p| !p.is_empty()) { out.push((pad.to_string(), c.attr("gate").unwrap_or("").to_string(), c.attr("pin").unwrap_or("").to_string())); }
                }
                if !out.is_empty() { return out; }
            }
        }
        for d in devs.children.iter().filter(|c| c.tag == "device") {
            fn walk(e: &Element, out: &mut Vec<(String, String, String)>) {
                if e.tag == "attribute" && e.attr("name") == Some("ADOM_PINMAP") {
                    for item in e.attr("value").unwrap_or("").split(';').filter(|s| !s.is_empty()) {
                        if let Some((num, gp)) = item.split_once(':') { if let Some((g, p)) = gp.split_once('.') { out.push((num.into(), g.into(), p.into())); } }
                    }
                }
                for c in &e.children { walk(c, out); }
            }
            walk(d, &mut out);
            if !out.is_empty() { return out; }
        }
    }
    out
}

fn electrical(dir: &str) -> u8 { match dir { "in" => 0, "io" => 1, "out" => 2, "oc" => 3, "hiz" => 5, "pwr" | "sup" => 7, _ => 4 } }

/// The standard's read_symbol for a deviceset.
pub fn read_symbol(xml: &str, name: &str) -> Result<Value, String> {
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let lib = lib_mut(&mut root)?;
    let dss = section(lib, "devicesets").children.clone();
    let ds = dss.iter().find(|d| d.tag == "deviceset" && d.attr("name") == Some(name)).or_else(|| dss.iter().find(|d| d.tag == "deviceset" && d.attr("name").map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false)))
        .ok_or_else(|| format!("no symbol {name}; the library has {}", dss.iter().filter_map(|d| d.attr("name")).collect::<Vec<_>>().join(", ")))?;
    let syms = section(lib, "symbols").children.clone();
    let map = pin_number_map(ds);
    let mut pins = Vec::new();
    let gates: Vec<&Element> = ds.find_child("gates").map(|g| g.children.iter().filter(|c| c.tag == "gate").collect()).unwrap_or_default();
    for g in &gates {
        if let Some(sym) = syms.iter().find(|s| s.attr("name") == g.attr("symbol")) {
            for p in sym.children.iter().filter(|c| c.tag == "pin") {
                let pname = p.attr("name").unwrap_or("");
                let des: Vec<&str> = map.iter().filter(|(_, gg, pp)| gg == g.attr("name").unwrap_or("") && pp == pname).map(|(n, _, _)| n.as_str()).collect();
                pins.push(json!({"des": des.join(" "), "name": pname.split('@').next().unwrap_or(pname), "pin": pname, "gate": g.attr("name"), "electrical": electrical(p.attr("direction").unwrap_or("io")), "hidden": p.attr("visible") == Some("off")}));
            }
        }
    }
    let mut params = Vec::new();
    let mut models = Vec::new();
    if let Some(devs) = ds.find_child("devices") {
        for d in devs.children.iter().filter(|c| c.tag == "device") {
            if let Some(pk) = d.attr("package").filter(|p| !p.is_empty()) { models.push(json!({"type": "PCBLIB", "name": pk, "device": d.attr("name"), "isCurrent": models.is_empty()})); }
            fn walk(e: &Element, out: &mut Vec<Value>) { if e.tag == "attribute" && e.attr("name") != Some("ADOM_PINMAP") { out.push(json!({"name": e.attr("name"), "value": e.attr("value").unwrap_or(""), "hidden": e.attr("display") == Some("off")})); } for c in &e.children { walk(c, out); } }
            walk(d, &mut params);
        }
    }
    params.dedup_by(|a, b| a["name"] == b["name"]);
    Ok(json!({"libref": ds.attr("name"), "description": ds.find_child("description").map(|d| d.text.trim().to_string()).unwrap_or_default(),
              "displayModeCount": 1, "parts": gates.len(), "pinsMode0": pins.len(), "pins": pins, "params": params, "models": models, "prefix": ds.attr("prefix")}))
}

/// The standard's find_symbol over library texts: (library label, text) pairs.
pub fn find_symbol(libs: &[(String, String)], query: &str, footprints: bool) -> Value {
    let q = query.to_lowercase();
    let mut matches = Vec::new();
    for (label, text) in libs {
        let Ok(mut root) = Element::parse(text) else { continue };
        let Ok(lib) = lib_mut(&mut root) else { continue };
        let (sec, tag) = if footprints { ("packages", "package") } else { ("devicesets", "deviceset") };
        for n in names(section(lib, sec), tag) { if n.to_lowercase().contains(&q) { matches.push(json!({"name": n, "library": label, "exact": n.eq_ignore_ascii_case(query)})); } }
    }
    let best = matches.iter().find(|m| m["exact"] == true).cloned().or_else(|| matches.iter().min_by_key(|m| m["name"].as_str().map(|s| s.len()).unwrap_or(usize::MAX)).cloned());
    json!({"matches": matches, "best": best})
}

fn primitives(e: &Element) -> usize { e.children.iter().filter(|c| c.tag != "description").count() }

/// copy_footprint (within a library) and xcopy_footprint (from another library's text).
pub fn copy_footprint(xml: &str, from_xml: Option<&str>, from: &str, to: &str) -> Result<(String, Value), String> {
    let src_text = from_xml.unwrap_or(xml);
    let mut src = Element::parse(src_text).map_err(|e| e.to_string())?;
    let pk = {
        let lib = lib_mut(&mut src)?;
        let sec = section(lib, "packages");
        let n = find_ci(&names(sec, "package"), from).cloned().ok_or_else(|| format!("no footprint {from}"))?;
        sec.children.iter().find(|p| p.tag == "package" && p.attr("name") == Some(n.as_str())).cloned().unwrap()
    };
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let lib = lib_mut(&mut root)?;
    let sec = section(lib, "packages");
    if find_ci(&names(sec, "package"), to).is_some() { return Err(format!("footprint {to} already exists in the destination")); }
    let mut c = pk; set(&mut c, "name", to); c.attrs.retain(|(k, _)| k != "urn" && k != "library_version");
    let n = primitives(&c);
    sec.children.push(c);
    Ok((root.to_xml_document(), json!({"from": from, "to": to, "primitives": n})))
}

/// rename_footprint: the package, every device that uses it, and its 3D package instances.
pub fn rename_footprint(xml: &str, from: &str, to: &str) -> Result<(String, Value), String> {
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let lib = lib_mut(&mut root)?;
    let sec = section(lib, "packages");
    let n = find_ci(&names(sec, "package"), from).cloned().ok_or_else(|| format!("no footprint {from}"))?;
    if find_ci(&names(sec, "package"), to).is_some() && !n.eq_ignore_ascii_case(to) { return Err(format!("footprint {to} already exists")); }
    for p in sec.children.iter_mut().filter(|p| p.tag == "package" && p.attr("name") == Some(n.as_str())) { set(p, "name", to); }
    let mut devices = 0;
    fn walk(e: &mut Element, old: &str, new: &str, devices: &mut usize) {
        if e.tag == "device" && e.attr("package") == Some(old) { set(e, "package", new); *devices += 1; }
        if e.tag == "packageinstance" && e.attr("name") == Some(old) { set(e, "name", new); }
        for c in e.children.iter_mut() { walk(c, old, new, devices); }
    }
    walk(lib, &n, to, &mut devices);
    Ok((root.to_xml_document(), json!({"renamed": to, "from": n, "devicesUpdated": devices})))
}

/// remove_footprint: refused while a device uses it, so no symbol is left pointing at nothing.
pub fn remove_footprint(xml: &str, name: &str) -> Result<(String, Value), String> {
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let lib = lib_mut(&mut root)?;
    let n = find_ci(&names(section(lib, "packages"), "package"), name).cloned().ok_or_else(|| format!("no footprint {name}"))?;
    let users: Vec<String> = section(lib, "devicesets").children.iter().filter(|d| d.find_child("devices").map(|ds| ds.children.iter().any(|x| x.attr("package") == Some(n.as_str()))).unwrap_or(false)).filter_map(|d| d.attr("name").map(str::to_string)).collect();
    if !users.is_empty() { return Err(format!("footprint {n} is used by {}: remove or relink those first", users.join(", "))); }
    section(lib, "packages").children.retain(|p| !(p.tag == "package" && p.attr("name") == Some(n.as_str())));
    Ok((root.to_xml_document(), json!({"removed": n})))
}

/// clone_symbol: the deviceset copied under a new name with copies of its symbols.
pub fn clone_symbol(xml: &str, from: &str, to: &str) -> Result<(String, Value), String> {
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let lib = lib_mut(&mut root)?;
    let dsn = find_ci(&names(section(lib, "devicesets"), "deviceset"), from).cloned().ok_or_else(|| format!("no symbol {from}"))?;
    if find_ci(&names(section(lib, "devicesets"), "deviceset"), to).is_some() { return Err(format!("symbol {to} already exists")); }
    let mut ds = section(lib, "devicesets").children.iter().find(|d| d.tag == "deviceset" && d.attr("name") == Some(dsn.as_str())).cloned().unwrap();
    set(&mut ds, "name", to); ds.attrs.retain(|(k, _)| k != "urn" && k != "library_version");
    let syms = section(lib, "symbols").children.clone();
    let mut new_syms = Vec::new();
    let mut prims = 0;
    if let Some(g) = ds.find_child_mut("gates") {
        for gate in g.children.iter_mut().filter(|c| c.tag == "gate") {
            let old = gate.attr("symbol").unwrap_or("").to_string();
            if let Some(s) = syms.iter().find(|s| s.attr("name") == Some(old.as_str())) {
                let nn = if old.starts_with(&dsn) { format!("{to}{}", &old[dsn.len()..]) } else { format!("{to}_{old}") };
                let mut c = s.clone(); set(&mut c, "name", &nn); c.attrs.retain(|(k, _)| k != "urn" && k != "library_version");
                prims += primitives(&c);
                if !syms.iter().any(|x| x.attr("name") == Some(nn.as_str())) && !new_syms.iter().any(|x: &Element| x.attr("name") == Some(nn.as_str())) { new_syms.push(c); }
                set(gate, "symbol", &nn);
            }
        }
    }
    section(lib, "symbols").children.extend(new_syms);
    section(lib, "devicesets").children.push(ds);
    Ok((root.to_xml_document(), json!({"from": dsn, "to": to, "primitives": prims, "displayModeCount": 1})))
}

pub fn rename_symbol(xml: &str, from: &str, to: &str) -> Result<(String, Value), String> {
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let lib = lib_mut(&mut root)?;
    let sec = section(lib, "devicesets");
    let n = find_ci(&names(sec, "deviceset"), from).cloned().ok_or_else(|| format!("no symbol {from}"))?;
    if find_ci(&names(sec, "deviceset"), to).is_some() && !n.eq_ignore_ascii_case(to) { return Err(format!("symbol {to} already exists")); }
    for d in sec.children.iter_mut().filter(|d| d.tag == "deviceset" && d.attr("name") == Some(n.as_str())) { set(d, "name", to); }
    Ok((root.to_xml_document(), json!({"renamed": to, "from": n})))
}

/// remove_symbol: the deviceset, and the symbols no other deviceset uses.
pub fn remove_symbol(xml: &str, name: &str) -> Result<(String, Value), String> {
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let lib = lib_mut(&mut root)?;
    let n = find_ci(&names(section(lib, "devicesets"), "deviceset"), name).cloned().ok_or_else(|| format!("no symbol {name}"))?;
    let used_by = |sec: &Element, skip: &str| -> std::collections::HashSet<String> {
        sec.children.iter().filter(|d| d.tag == "deviceset" && d.attr("name") != Some(skip)).flat_map(|d| d.find_child("gates").map(|g| g.children.iter().filter_map(|x| x.attr("symbol").map(str::to_string)).collect::<Vec<_>>()).unwrap_or_default()).collect()
    };
    let mine: Vec<String> = section(lib, "devicesets").children.iter().filter(|d| d.attr("name") == Some(n.as_str())).flat_map(|d| d.find_child("gates").map(|g| g.children.iter().filter_map(|x| x.attr("symbol").map(str::to_string)).collect::<Vec<_>>()).unwrap_or_default()).collect();
    let others = used_by(section(lib, "devicesets"), &n);
    section(lib, "devicesets").children.retain(|d| !(d.tag == "deviceset" && d.attr("name") == Some(n.as_str())));
    let gone: Vec<String> = mine.into_iter().filter(|s| !others.contains(s)).collect();
    section(lib, "symbols").children.retain(|s| !(s.tag == "symbol" && s.attr("name").map(|x| gone.iter().any(|g| g == x)).unwrap_or(false)));
    Ok((root.to_xml_document(), json!({"removed": n, "symbolsRemoved": gone})))
}

/// A device's connects for a footprint: each pin joined, by pad NUMBER, to every pad of that number
/// on the footprint (N and its repeats N@2, N@3 ...). The pin numbers come from the component's
/// existing connects (a repeat's @k dropped) or its ADOM_PINMAP.
fn connects_for(ds: &Element, comp: &str, fp: &str, pk_pads: &[String]) -> Result<Element, String> {
    let base = |p: &str| p.split('@').next().unwrap_or(p).to_string();
    let mut by_pin: std::collections::BTreeMap<(String, String), Vec<String>> = Default::default();
    for (num, g, p) in pin_number_map(ds) { let v = by_pin.entry((g, p)).or_default(); let b = base(&num); if !v.contains(&b) { v.push(b); } }
    if by_pin.is_empty() { return Err(format!("{comp} has no pin numbers to join to {fp}'s pads")); }
    let mut cs = Element::new("connects");
    let mut missing = Vec::new();
    for ((g, p), nums) in &by_pin {
        let pads: Vec<&String> = pk_pads.iter().filter(|q| nums.iter().any(|n| base(q) == *n)).collect();
        if pads.is_empty() { missing.extend(nums.iter().cloned()); continue; }
        cs.children.push(el("connect", &[("gate", g.clone()), ("pin", p.clone()), ("pad", pads.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" "))]));
    }
    if !missing.is_empty() { return Err(format!("{fp} has no pad {}", missing.join(", "))); }
    Ok(cs)
}

/// The standard's set_models: the component keeps ONE footprint, every other device removed.
pub fn set_models(xml: &str, symbol: &str, footprint: &str) -> Result<(String, Value), String> {
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let lib = lib_mut(&mut root)?;
    let pk_names = names(section(lib, "packages"), "package");
    let fp = find_ci(&pk_names, footprint).cloned().ok_or_else(|| format!("no footprint {footprint} in this library"))?;
    let pk_pads: Vec<String> = section(lib, "packages").children.iter().find(|p| p.attr("name") == Some(fp.as_str())).map(|p| p.children.iter().filter(|c| c.tag == "smd" || c.tag == "pad").filter_map(|c| c.attr("name").map(str::to_string)).collect()).unwrap_or_default();
    let sec = section(lib, "devicesets");
    let n = find_ci(&names(sec, "deviceset"), symbol).cloned().ok_or_else(|| format!("no symbol {symbol}"))?;
    let ds = sec.children.iter_mut().find(|d| d.tag == "deviceset" && d.attr("name") == Some(n.as_str())).unwrap();
    let cs = connects_for(ds, &n, &fp, &pk_pads)?;
    let devs = match ds.find_child_mut("devices") { Some(d) => d, None => { ds.children.push(Element::new("devices")); ds.find_child_mut("devices").unwrap() } };
    // the attributes ride along on the kept device
    let techs = devs.children.iter().find(|d| d.tag == "device").and_then(|d| d.find_child("technologies").cloned());
    let removed = devs.children.iter().filter(|d| d.tag == "device" && d.attr("package").map(|p| !p.is_empty()).unwrap_or(false)).count();
    devs.children.retain(|d| d.tag != "device");
    let mut dev = el("device", &[("name", "".into()), ("package", fp.clone())]);
    dev.children.push(cs);
    dev.children.push(techs.unwrap_or_else(|| { let mut t = Element::new("technologies"); t.children.push(el("technology", &[("name", "".into())])); t }));
    devs.children.push(dev);
    Ok((root.to_xml_document(), json!({"component": n, "footprint": fp, "removed": removed})))
}

/// The component (deviceset) and the symbols its gates draw, by name.
fn component_symbols(root: &mut Element, name: &str) -> Result<(String, Vec<String>), String> {
    let lib = lib_mut(root)?;
    let ds = section(lib, "devicesets").children.iter().find(|d| d.tag == "deviceset" && d.attr("name").map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false)).ok_or_else(|| format!("no symbol {name}"))?;
    let syms: Vec<String> = ds.find_child("gates").map(|g| g.children.iter().filter_map(|x| x.attr("symbol").map(str::to_string)).collect()).unwrap_or_default();
    Ok((ds.attr("name").unwrap_or(name).to_string(), syms))
}

/// The standard's symbol_prims: a component's drawing counted by kind, the body rectangle (the
/// symbol layer's lines and rectangles) and the polygons' box, in mil, and its footprints.
pub fn symbol_prims(xml: &str, name: &str) -> Result<Value, String> {
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let (comp, syms) = component_symbols(&mut root, name)?;
    let lib = lib_mut(&mut root)?;
    let mil = |v: f64| (v / MIL * 10.0).round() / 10.0;
    let num = |e: &Element, k: &str| e.attr(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let (mut lines, mut rects, mut polys, mut circles, mut arcs, mut texts, mut pins) = (0, 0, 0, 0, 0, 0, 0);
    let (mut body, mut poly) = ((f64::MAX, f64::MAX, f64::MIN, f64::MIN), (f64::MAX, f64::MAX, f64::MIN, f64::MIN));
    let grow = |b: &mut (f64, f64, f64, f64), x: f64, y: f64| { b.0 = b.0.min(x); b.1 = b.1.min(y); b.2 = b.2.max(x); b.3 = b.3.max(y); };
    for sn in &syms {
        let Some(sym) = section(lib, "symbols").children.iter().find(|x| x.tag == "symbol" && x.attr("name") == Some(sn.as_str())) else { continue };
        for c in &sym.children {
            match c.tag.as_str() {
                "wire" if c.attr("curve").is_some() => arcs += 1,
                "wire" => { lines += 1; if c.attr("layer") == Some("94") { grow(&mut body, num(c, "x1"), num(c, "y1")); grow(&mut body, num(c, "x2"), num(c, "y2")); } }
                "rectangle" => { rects += 1; if c.attr("layer") == Some("94") { grow(&mut body, num(c, "x1"), num(c, "y1")); grow(&mut body, num(c, "x2"), num(c, "y2")); } }
                "polygon" => { polys += 1; for v in c.children.iter().filter(|v| v.tag == "vertex") { grow(&mut poly, num(v, "x"), num(v, "y")); } }
                "circle" => circles += 1,
                "text" => texts += 1,
                "pin" => pins += 1,
                _ => {}
            }
        }
    }
    let bx = |b: (f64, f64, f64, f64)| if b.0 == f64::MAX { Value::Null } else { json!([mil(b.0), mil(b.1), mil(b.2), mil(b.3)]) };
    let ds = section(lib, "devicesets").children.iter().find(|d| d.attr("name") == Some(comp.as_str())).unwrap();
    let models: Vec<Value> = ds.find_child("devices").map(|d| d.children.iter().filter_map(|x| x.attr("package").filter(|p| !p.is_empty()).map(|p| json!({"type": "PCBLIB", "name": p, "device": x.attr("name").unwrap_or("")}))).collect()).unwrap_or_default();
    Ok(json!({"symbol": comp, "symbols": syms, "pins": pins, "lines": lines, "rectangles": rects, "polylines": polys, "circles": circles, "arcs": arcs, "texts": texts,
              "bodyRectMil": bx(body), "polyBboxMil": bx(poly), "displayModes": 1, "models": models}))
}

/// The standard's set_visibility: an EAGLE component shows a parameter on the sheet when its symbol
/// carries a >NAME text for it, so showing one adds that text (Info layer, below the body) to each
/// symbol the component's gates draw, and hiding one removes it.
pub fn set_visibility(xml: &str, name: &str, parameter: &str, visible: bool) -> Result<(String, Value), String> {
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let (comp, syms) = component_symbols(&mut root, name)?;
    let key = format!(">{}", parameter.trim_start_matches('>').to_uppercase());
    let lib = lib_mut(&mut root)?;
    let mut changed = Vec::new();
    for sn in &syms {
        let Some(sym) = section(lib, "symbols").children.iter_mut().find(|x| x.tag == "symbol" && x.attr("name") == Some(sn.as_str())) else { continue };
        let has = sym.children.iter().any(|c| c.tag == "text" && c.text.eq_ignore_ascii_case(&key));
        if visible && !has {
            let (mut x0, mut y0) = (f64::MAX, f64::MAX);
            for c in &sym.children { for (kx, ky) in [("x1", "y1"), ("x2", "y2"), ("x", "y")] { if let (Some(x), Some(y)) = (c.attr(kx).and_then(|v| v.parse::<f64>().ok()), c.attr(ky).and_then(|v| v.parse::<f64>().ok())) { x0 = x0.min(x); y0 = y0.min(y); } } }
            // below whatever the symbol already writes there (>VALUE and other shown parameters)
            let shown = sym.children.iter().filter(|c| c.tag == "text" && c.text.starts_with('>')).count() as f64;
            let mut t = el("text", &[("x", f(if x0 == f64::MAX { 0.0 } else { x0 })), ("y", f(if y0 == f64::MAX { 0.0 } else { y0 } - 2.54 * shown)), ("size", "1.778".into()), ("layer", "97".into())]);
            t.text = key.clone();
            sym.children.push(t);
            changed.push(sn.clone());
        } else if !visible && has {
            sym.children.retain(|c| !(c.tag == "text" && c.text.eq_ignore_ascii_case(&key)));
            changed.push(sn.clone());
        }
    }
    Ok((root.to_xml_document(), json!({"component": comp, "parameter": key.trim_start_matches('>'), "visible": visible, "symbolsChanged": changed})))
}

/// A KiCad symbol's body rectangle (mm, x0 y0 x1 y1) and polylines, from a .kicad_sym (the symbol
/// and its unit blocks, which nest inside it).
fn kicad_art(text: &str, name: &str) -> (Option<(f64, f64, f64, f64)>, Vec<Vec<(f64, f64)>>) {
    let head = format!("(symbol \"{name}\"");
    let Some(start) = text.find(&head) else { return (None, vec![]) };
    let b = text.as_bytes(); let (mut depth, mut end) = (0i32, text.len());
    for (i, &c) in b.iter().enumerate().skip(start) { if c == b'(' { depth += 1; } else if c == b')' { depth -= 1; if depth == 0 { end = i + 1; break; } } }
    let blk = &text[start..end];
    let num = |t: &str| t.trim().parse::<f64>().ok();
    let rect = regex::Regex::new(r"\(rectangle\s*\(start\s+([-\d.]+)\s+([-\d.]+)\)\s*\(end\s+([-\d.]+)\s+([-\d.]+)\)").ok()
        .and_then(|re| re.captures(blk)).and_then(|c| Some((num(&c[1])?, num(&c[2])?, num(&c[3])?, num(&c[4])?)))
        .map(|(a, b2, c, d)| (a.min(c), b2.min(d), a.max(c), b2.max(d)));
    let xy = regex::Regex::new(r"\(xy\s+([-\d.]+)\s+([-\d.]+)\)").unwrap();
    let mut polys = Vec::new();
    let mut rest = blk;
    while let Some(i) = rest.find("(polyline") {
        let sub = &rest[i..]; let sb = sub.as_bytes(); let (mut dep, mut e) = (0i32, sub.len());
        for (j, &c) in sb.iter().enumerate() { if c == b'(' { dep += 1; } else if c == b')' { dep -= 1; if dep == 0 { e = j + 1; break; } } }
        let pts: Vec<(f64, f64)> = xy.captures_iter(&sub[..e]).filter_map(|c| Some((num(&c[1])?, num(&c[2])?))).collect();
        if pts.len() >= 2 { polys.push(pts); }
        rest = &sub[e..];
    }
    (rect, polys)
}

/// The standard's add_symbol_art: the chip-outline artwork adom-symbol baked into a KiCad symbol,
/// drawn into the EAGLE component's symbol(s) on the symbol layer, the KiCad body rectangle mapped
/// onto the symbol's own body box. clear (default) removes earlier art first: every symbol-layer
/// wire and polygon except the body's own edges.
pub fn add_symbol_art(xml: &str, name: &str, kicad_sym: &str, clear: bool) -> Result<(String, Value), String> {
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let (comp, syms) = component_symbols(&mut root, name)?;
    // the KiCad symbol by the component's own name (as asked, when it differs in case)
    let (rect, polys) = { let a = kicad_art(kicad_sym, &comp); if a.0.is_some() { a } else { kicad_art(kicad_sym, name) } };
    let kr = rect.ok_or_else(|| format!("no (rectangle ...) body for symbol {comp} in the .kicad_sym"))?;
    if polys.is_empty() { return Err(format!("no polylines in symbol {comp}: bake the outline first (adom-symbol create --outline-svg <mpn>-3d-outline-heavy.svg --outline-name none)")); }
    let lib = lib_mut(&mut root)?;
    let num = |e: &Element, k: &str| e.attr(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let (mut drawn, mut removed) = (0usize, 0usize);
    let mut body_out = Value::Null;
    for sn in &syms {
        let Some(sym) = section(lib, "symbols").children.iter_mut().find(|x| x.tag == "symbol" && x.attr("name") == Some(sn.as_str())) else { continue };
        // the body box: the symbol layer's lines and rectangles
        let mut bb = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for c in sym.children.iter().filter(|c| (c.tag == "wire" || c.tag == "rectangle") && c.attr("layer") == Some("94")) {
            for (x, y) in [(num(c, "x1"), num(c, "y1")), (num(c, "x2"), num(c, "y2"))] { bb.0 = bb.0.min(x); bb.1 = bb.1.min(y); bb.2 = bb.2.max(x); bb.3 = bb.3.max(y); }
        }
        if bb.0 == f64::MAX { return Err(format!("symbol {sn} has no body (no lines on the symbol layer) to fit the art to")); }
        if clear {
            let on_edge = |c: &Element| {
                let (x1, y1, x2, y2) = (num(c, "x1"), num(c, "y1"), num(c, "x2"), num(c, "y2"));
                let e = 1e-6;
                ((x1 - x2).abs() < e && ((x1 - bb.0).abs() < e || (x1 - bb.2).abs() < e)) || ((y1 - y2).abs() < e && ((y1 - bb.1).abs() < e || (y1 - bb.3).abs() < e))
            };
            let before = sym.children.len();
            sym.children.retain(|c| !(c.attr("layer") == Some("94") && ((c.tag == "wire" && !on_edge(c)) || c.tag == "polygon")));
            removed += before - sym.children.len();
        }
        let (sx, sy) = ((bb.2 - bb.0) / (kr.2 - kr.0).max(1e-9), (bb.3 - bb.1) / (kr.3 - kr.1).max(1e-9));
        let map = |(x, y): (f64, f64)| (bb.0 + (x - kr.0) * sx, bb.1 + (y - kr.1) * sy);
        for pl in &polys {
            for w in pl.windows(2) {
                let (a, b) = (map(w[0]), map(w[1]));
                if (a.0 - b.0).abs() + (a.1 - b.1).abs() < 1e-9 { continue; }
                sym.children.push(el("wire", &[("x1", f(a.0)), ("y1", f(a.1)), ("x2", f(b.0)), ("y2", f(b.1)), ("width", "0.1524".into()), ("layer", "94".into())]));
                drawn += 1;
            }
        }
        let mm2mil = |v: f64| (v / MIL * 10.0).round() / 10.0;
        body_out = json!([mm2mil(bb.0), mm2mil(bb.1), mm2mil(bb.2), mm2mil(bb.3)]);
    }
    Ok((root.to_xml_document(), json!({"component": comp, "polylines": polys.len(), "points": polys.iter().map(|p| p.len()).sum::<usize>(), "lines": drawn, "removed": removed, "bodyRectMil": body_out})))
}

/// The standard's lint_library input, read from an EAGLE library: each package as a footprint
/// (pads in its own frame, mm; holes as mechanical pads; the courtyard from tKeepout, silk from
/// tPlace, its 3D packages, its description) and each component as a symbol per device (pin numbers
/// from the device's connects, its package, the MPN attribute). The engine judges them.
pub fn lint_input(xml: &str) -> Result<Value, String> {
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let lib = lib_mut(&mut root)?;
    let num = |e: &Element, k: &str| e.attr(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let turned = |e: &Element| e.attr("rot").map(|r| { let d: f64 = r.trim_start_matches(['M', 'S', 'R']).parse().unwrap_or(0.0); ((d / 90.0).round() as i64).rem_euclid(2) == 1 }).unwrap_or(false);
    // which packages a 3D package instances
    let mut bodies: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
    if let Some(p3) = lib.find_child("packages3d") {
        for d in p3.children.iter().filter(|c| c.tag == "package3d") {
            for pi in d.find_child("packageinstances").map(|x| x.children.clone()).unwrap_or_default() {
                if let Some(n) = pi.attr("name") { bodies.entry(n.to_string()).or_default().push(json!({"name": d.attr("name"), "urn": d.attr("urn").or(d.attr("wip_urn"))})); }
            }
        }
    }
    let mut fps = Vec::new();
    for pk in section(lib, "packages").children.iter().filter(|c| c.tag == "package") {
        let name = pk.attr("name").unwrap_or("").to_string();
        let mut pads = Vec::new();
        let (mut court, mut silk) = (Vec::new(), 0usize);
        for c in &pk.children {
            match c.tag.as_str() {
                "smd" => { let (w, h) = (num(c, "dx"), num(c, "dy")); let (w, h) = if turned(c) { (h, w) } else { (w, h) };
                           pads.push(json!({"number": c.attr("name").unwrap_or(""), "x": num(c, "x"), "y": num(c, "y"), "w": w, "h": h})); }
                "pad" => { let d = if c.attr("diameter").is_some() { num(c, "diameter") } else { num(c, "drill") * 1.6 };
                           pads.push(json!({"number": c.attr("name").unwrap_or(""), "x": num(c, "x"), "y": num(c, "y"), "w": d, "h": d, "hole": num(c, "drill")})); }
                "hole" => pads.push(json!({"number": "", "x": num(c, "x"), "y": num(c, "y"), "w": num(c, "drill"), "h": num(c, "drill"), "hole": num(c, "drill"), "mechanical": true, "plated": false})),
                _ => {}
            }
            match c.attr("layer") {
                Some("39") => {
                    for (kx, ky) in [("x1", "y1"), ("x2", "y2"), ("x", "y")] { if c.attr(kx).is_some() { court.push(json!([num(c, kx), num(c, ky)])); } }
                    for v in c.children.iter().filter(|v| v.tag == "vertex") { court.push(json!([num(v, "x"), num(v, "y")])); }
                }
                Some("21") => silk += 1,
                _ => {}
            }
        }
        fps.push(json!({"name": name, "pads": pads, "courtyard": court, "silk": if silk > 0 { json!([silk]) } else { json!([]) },
                        "bodies": bodies.get(&name).cloned().unwrap_or_default(), "description": pk.find_child("description").map(|d| d.text.trim().to_string()).unwrap_or_default()}));
    }
    let mut syms = Vec::new();
    let _ = section(lib, "symbols");
    let sets: Vec<Element> = section(lib, "devicesets").children.iter().filter(|c| c.tag == "deviceset").cloned().collect();
    let lib = &*lib;
    for ds in &sets {
        let dsn = ds.attr("name").unwrap_or("").to_string();
        let devs: Vec<&Element> = ds.find_child("devices").map(|d| d.children.iter().filter(|x| x.tag == "device").collect()).unwrap_or_default();
        let many = devs.len() > 1;
        for dev in devs {
            let pkg = dev.attr("package").unwrap_or("").to_string();
            let mut pins: Vec<Value> = Vec::new();
            for c in dev.find_child("connects").map(|x| x.children.iter().filter(|c| c.tag == "connect").collect::<Vec<_>>()).unwrap_or_default() {
                for pad in c.attr("pad").unwrap_or("").split_whitespace() { pins.push(json!({"number": pad.split('@').next().unwrap_or(pad), "name": c.attr("pin")})); }
            }
            if pins.is_empty() {
                // a device with no footprint has no connects: its pins carry their numbers in
                // ADOM_PINMAP ("1:G$1.IN;..."), else they are the gates' symbol pins, unnumbered
                let mut map = String::new();
                let mut w = vec![dev];
                while let Some(e) = w.pop() { for c in &e.children { if c.tag == "attribute" && c.attr("name") == Some("ADOM_PINMAP") { map = c.attr("value").unwrap_or("").to_string(); } w.push(c); } }
                if !map.is_empty() {
                    for item in map.split(';').filter(|x| !x.is_empty()) { if let Some((n, gp)) = item.split_once(':') { pins.push(json!({"number": n, "name": gp.split_once('.').map(|x| x.1).unwrap_or(gp)})); } }
                } else {
                    let gates: Vec<String> = ds.find_child("gates").map(|g| g.children.iter().filter_map(|x| x.attr("symbol").map(str::to_string)).collect()).unwrap_or_default();
                    for g in gates {
                        if let Some(sym) = lib.find_child("symbols").and_then(|sec| sec.children.iter().find(|x| x.attr("name") == Some(g.as_str()))) {
                            for p in sym.children.iter().filter(|c| c.tag == "pin") { pins.push(json!({"number": "", "name": p.attr("name")})); }
                        }
                    }
                }
            }
            let mut mpn = String::new();
            let mut walk = vec![dev];
            while let Some(e) = walk.pop() {
                for c in &e.children {
                    if c.tag == "attribute" && ["MPN", "MANUFACTURER_PART_NUMBER", "MANUFACTURER PART NUMBER", "PARTNUMBER", "PART_NUMBER"].contains(&c.attr("name").unwrap_or("").to_uppercase().as_str()) {
                        if let Some(v) = c.attr("value").filter(|v| !v.trim().is_empty()) { mpn = v.to_string(); }
                    }
                    walk.push(c);
                }
            }
            let name = if many { format!("{dsn}{}", dev.attr("name").unwrap_or("")) } else { dsn.clone() };
            syms.push(json!({"name": name, "pins": pins, "footprint": pkg, "partNumber": mpn}));
        }
    }
    Ok(json!({"footprints": fps, "symbols": syms}))
}

/// The standard's add_models: each footprint becomes another device of the component (EAGLE's
/// several-packages-per-component), pins joined by pad number; a footprint already attached is
/// skipped, since a device names it once. `sources` holds other .lbr texts a model's footprint is
/// copied from. Then params, comment and description.
pub fn add_models(xml: &str, symbol: &str, models: &[(String, Option<String>)], params: &Value, comment: Option<&str>, description: Option<&str>) -> Result<(String, Value), String> {
    let mut text = xml.to_string();
    let (mut added, mut skipped) = (Vec::new(), Vec::new());
    for (fp, source) in models {
        // a footprint from another library comes in first
        let have = { let mut r = Element::parse(&text).map_err(|e| e.to_string())?; let l = lib_mut(&mut r)?; find_ci(&names(section(l, "packages"), "package"), fp).is_some() };
        if !have {
            let src = source.as_deref().ok_or_else(|| format!("no footprint {fp} in this library (give the model's library)"))?;
            text = copy_footprint(&text, Some(src), fp, fp)?.0;
        }
        let attached = {
            let mut r = Element::parse(&text).map_err(|e| e.to_string())?; let l = lib_mut(&mut r)?;
            let sec = section(l, "devicesets");
            let ds = sec.children.iter().find(|d| d.tag == "deviceset" && d.attr("name").map(|n| n.eq_ignore_ascii_case(symbol)).unwrap_or(false)).ok_or_else(|| format!("no symbol {symbol}"))?;
            ds.find_child("devices").map(|d| d.children.iter().any(|x| x.attr("package").map(|p| p.eq_ignore_ascii_case(fp)).unwrap_or(false))).unwrap_or(false)
        };
        if attached { skipped.push(fp.clone()); continue; }
        text = set_params(&text, Some(symbol), &Value::Null, None, Some(fp))?.0;
        added.push(fp.clone());
    }
    let (mut t, s) = set_params(&text, Some(symbol), params, comment, None)?;
    if let Some(d) = description.filter(|d| !d.is_empty()) {
        let mut r = Element::parse(&t).map_err(|e| e.to_string())?;
        { let l = lib_mut(&mut r)?; let sec = section(l, "devicesets");
          if let Some(ds) = sec.children.iter_mut().find(|x| x.tag == "deviceset" && x.attr("name").map(|n| n.eq_ignore_ascii_case(symbol)).unwrap_or(false)) {
              ds.children.retain(|c| c.tag != "description"); let mut de = Element::new("description"); de.text = d.into(); ds.children.insert(0, de); } }
        t = r.to_xml_document();
    }
    let models_now = { let mut r = Element::parse(&t).map_err(|e| e.to_string())?; let l = lib_mut(&mut r)?; let sec = section(l, "devicesets");
        sec.children.iter().find(|d| d.attr("name").map(|n| n.eq_ignore_ascii_case(symbol)).unwrap_or(false)).and_then(|d| d.find_child("devices")).map(|d| d.children.iter().filter(|x| x.attr("package").map(|p| !p.is_empty()).unwrap_or(false)).count()).unwrap_or(0) };
    Ok((t, json!({"libref": s["component"], "models": models_now, "added": added, "alreadyAttached": skipped, "params": s["paramsSet"]})))
}

/// The standard's set_params: attributes on every device of the symbol (EAGLE keeps them per
/// technology), a COMMENT attribute, and with `footprint` a device on that package whose connects
/// come from the pin numbers (pad name = pin number).
pub fn set_params(xml: &str, symbol: Option<&str>, params: &Value, comment: Option<&str>, footprint: Option<&str>) -> Result<(String, Value), String> {
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let lib = lib_mut(&mut root)?;
    let pk_names = names(section(lib, "packages"), "package");
    let pk_pads: Vec<String> = match footprint {
        Some(fp) => {
            let n = find_ci(&pk_names, fp).cloned().ok_or_else(|| format!("no footprint {fp} in this library"))?;
            section(lib, "packages").children.iter().find(|p| p.attr("name") == Some(n.as_str())).map(|p| p.children.iter().filter(|c| c.tag == "smd" || c.tag == "pad").filter_map(|c| c.attr("name").map(str::to_string)).collect()).unwrap_or_default()
        }
        None => vec![],
    };
    let fp_name = footprint.and_then(|fp| find_ci(&pk_names, fp).cloned());
    let sec = section(lib, "devicesets");
    let all = names(sec, "deviceset");
    let n = match symbol { Some(s) => find_ci(&all, s).cloned().ok_or_else(|| format!("no symbol {s}"))?, None => all.first().cloned().ok_or("the library has no symbols")? };
    let ds = sec.children.iter_mut().find(|d| d.tag == "deviceset" && d.attr("name") == Some(n.as_str())).unwrap();
    let mut kv: Vec<(String, String)> = match params {
        Value::Object(m) => m.iter().map(|(k, v)| (k.to_uppercase(), v.as_str().map(str::to_string).unwrap_or(v.to_string()))).collect(),
        Value::String(s) => s.split(';').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.trim().to_uppercase(), v.trim().to_string())).collect(),
        _ => vec![],
    };
    if let Some(c) = comment { kv.push(("COMMENT".into(), c.to_string())); }
    let mut linked = Value::Null;
    if let (Some(fp), false) = (&fp_name, pk_pads.is_empty()) {
        let cs = connects_for(ds, &n, fp, &pk_pads)?;
        let devs = match ds.find_child_mut("devices") { Some(d) => d, None => { ds.children.push(Element::new("devices")); ds.find_child_mut("devices").unwrap() } };
        // the package-less device that held the pin numbers becomes the footprint's device
        let idx = devs.children.iter().position(|d| d.tag == "device" && d.attr("package").map(|p| p.is_empty()).unwrap_or(true));
        let mut dev = match idx { Some(i) => devs.children.remove(i), None => el("device", &[("name", if devs.children.is_empty() { "".into() } else { format!("-{}", fp) })]) };
        set(&mut dev, "package", fp);
        dev.children.retain(|c| c.tag != "connects");
        dev.children.insert(0, cs);
        devs.children.push(dev);
        linked = json!(fp);
    }
    let mut set_n = 0;
    if let Some(devs) = ds.find_child_mut("devices") {
        if devs.children.is_empty() { devs.children.push(el("device", &[("name", "".into())])); }
        for d in devs.children.iter_mut().filter(|c| c.tag == "device") {
            let techs = match d.find_child_mut("technologies") { Some(t) => t, None => { d.children.push(Element::new("technologies")); d.find_child_mut("technologies").unwrap() } };
            if techs.children.is_empty() { techs.children.push(el("technology", &[("name", "".into())])); }
            for t in techs.children.iter_mut().filter(|c| c.tag == "technology") {
                for (k, v) in &kv {
                    match t.children.iter_mut().find(|a| a.tag == "attribute" && a.attr("name") == Some(k.as_str())) {
                        Some(a) => set(a, "value", v),
                        None => t.children.push(el("attribute", &[("name", k.clone()), ("value", v.clone()), ("constant", "no".into())])),
                    }
                }
            }
        }
        set_n = kv.len();
    }
    Ok((root.to_xml_document(), json!({"component": n, "paramsSet": set_n, "footprint": linked})))
}
/// The standard's plate_vias: in EAGLE a pad is always plated, and an unplated drill is a <hole>.
/// Each hole in a footprint (or one footprint) becomes a plated pad. A hole inside an SMD land (a
/// heatsink via in an exposed pad) is named after that land, N@k, and joined to its connects in every
/// device, so it is on the land's net; any other hole becomes H<k>, unconnected.
pub fn plate_vias(xml: &str, footprint: Option<&str>) -> Result<(String, Value), String> {
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let lib = lib_mut(&mut root)?;
    let fp = match footprint { Some(w) => Some(find_ci(&names(section(lib, "packages"), "package"), w).cloned().ok_or_else(|| format!("no footprint {w}"))?), None => None };
    let num = |e: &Element, k: &str| e.attr(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let (mut holes, mut fixed, mut joins) = (0usize, Vec::new(), Vec::<(String, String, String)>::new());
    for pk in section(lib, "packages").children.iter_mut().filter(|p| p.tag == "package" && fp.as_deref().map(|n| p.attr("name") == Some(n)).unwrap_or(true)) {
        let pname = pk.attr("name").unwrap_or("").to_string();
        let mut taken: std::collections::HashSet<String> = pk.children.iter().filter(|c| c.tag == "pad" || c.tag == "smd").filter_map(|c| c.attr("name").map(str::to_string)).collect();
        // each SMD land as its half extents round its centre (a turned land by its bounding box)
        let lands: Vec<(String, f64, f64, f64, f64)> = pk.children.iter().filter(|c| c.tag == "smd").map(|s| {
            let deg = s.attr("rot").map(|r| r.trim_start_matches(|c: char| c.is_ascii_alphabetic()).parse::<f64>().unwrap_or(0.0)).unwrap_or(0.0).to_radians();
            let (hw, hh) = (num(s, "dx") / 2.0, num(s, "dy") / 2.0);
            (s.attr("name").unwrap_or("").to_string(), num(s, "x"), num(s, "y"), hw * deg.cos().abs() + hh * deg.sin().abs(), hw * deg.sin().abs() + hh * deg.cos().abs())
        }).collect();
        for c in pk.children.iter_mut().filter(|c| c.tag == "hole") {
            holes += 1;
            let (x, y) = (num(c, "x"), num(c, "y"));
            let land = lands.iter().find(|(_, lx, ly, hw, hh)| (x - lx).abs() <= *hw && (y - ly).abs() <= *hh).map(|l| l.0.clone());
            let stem = land.clone().map(|l| format!("{}@", l.split('@').next().unwrap_or(&l))).unwrap_or_else(|| "H".into());
            let name = (if land.is_some() { 2 } else { 1 }..).map(|k| format!("{stem}{k}")).find(|n| !taken.contains(n)).unwrap();
            taken.insert(name.clone());
            let drill = c.attr("drill").unwrap_or("0").to_string();
            // no diameter: the board's restring rules size the copper ring, as for any EAGLE via
            *c = el("pad", &[("name", name.clone()), ("x", f(x)), ("y", f(y)), ("drill", drill)]);
            fixed.push(format!("{pname}:{name}"));
            if let Some(l) = land { joins.push((pname.clone(), l, name)); }
        }
    }
    // a via named after its land joins the connect that land is on, in every device of that footprint
    for d in section(lib, "devicesets").children.iter_mut().filter(|d| d.tag == "deviceset") {
        for dev in d.find_child_mut("devices").map(|x| x.children.iter_mut().collect::<Vec<_>>()).unwrap_or_default() {
            let pk = dev.attr("package").unwrap_or("").to_string();
            for cn in dev.find_child_mut("connects").map(|x| x.children.iter_mut().collect::<Vec<_>>()).unwrap_or_default() {
                let mut pads: Vec<String> = cn.attr("pad").unwrap_or("").split_whitespace().map(str::to_string).collect();
                let add: Vec<String> = joins.iter().filter(|(p, l, _)| *p == pk && pads.contains(l)).map(|j| j.2.clone()).collect();
                if add.is_empty() { continue; }
                pads.extend(add);
                set(cn, "pad", &pads.join(" "));
            }
        }
    }
    let plated = fixed.len();
    Ok((root.to_xml_document(), json!({"holes": holes, "plated": plated, "fixed": fixed.join(";"), "joinedToLands": joins.len(),
        "note": if holes == 0 { "no unplated holes: every drilled pad in an EAGLE footprint is already plated" } else { "each hole is now a plated pad; one inside an SMD land is on that land's net" }})))
}

/// The standard's cleanup_libs: the empty leftovers a library collects removed: components with no
/// device, and footprints and symbols with nothing drawn that nothing uses. Anything with content stays.
pub fn cleanup_libs(xml: &str) -> Result<(String, Value), String> {
    let mut root = Element::parse(xml).map_err(|e| e.to_string())?;
    let lib = lib_mut(&mut root)?;
    let mut removed: Vec<String> = Vec::new();
    let ds = section(lib, "devicesets");
    ds.children.retain(|d| {
        let empty = d.tag == "deviceset" && d.find_child("devices").map(|x| x.children.iter().all(|c| c.tag != "device")).unwrap_or(true);
        if empty { removed.push(format!("component {}", d.attr("name").unwrap_or(""))); }
        !empty
    });
    let used_sym: std::collections::HashSet<String> = ds.children.iter().flat_map(|d| d.find_child("gates").map(|g| g.children.iter().filter_map(|x| x.attr("symbol").map(str::to_string)).collect::<Vec<_>>()).unwrap_or_default()).collect();
    let used_pk: std::collections::HashSet<String> = ds.children.iter().flat_map(|d| d.find_child("devices").map(|g| g.children.iter().filter_map(|x| x.attr("package").map(str::to_string)).collect::<Vec<_>>()).unwrap_or_default()).collect();
    let drawn = |e: &Element| e.children.iter().any(|c| c.tag != "description");
    for (sec, tag, used, what) in [("symbols", "symbol", &used_sym, "symbol"), ("packages", "package", &used_pk, "footprint")] {
        section(lib, sec).children.retain(|e| {
            let gone = e.tag == tag && !drawn(e) && !e.attr("name").map(|n| used.contains(n)).unwrap_or(false);
            if gone { removed.push(format!("{what} {}", e.attr("name").unwrap_or(""))); }
            !gone
        });
    }
    let n = removed.len();
    Ok((root.to_xml_document(), json!({"removed": n, "items": removed})))
}


/// The preflight a library under construction must pass: every check of fusion_preflight_lbr
/// except the ones only a finished library meets (has symbols, devicesets, packages, connects):
/// a footprint-only library is fine to write; opening a component in it is what needs them.
pub fn preflight_text(path: &std::path::Path) -> (bool, Vec<String>) {
    let pf = crate::preflight_lbr(path);
    // also fine mid-build: a symbol not yet joined to a footprint (no connects anywhere yet), and a
    // library whose last footprint was just removed
    let errs: Vec<String> = pf.errors.into_iter().filter(|e| !e.starts_with("symbols-present") && !e.starts_with("devicesets-present")
        && !e.starts_with("connects-resolve: no connects at all") && !e.starts_with("packages-nonempty")).collect();
    (errs.is_empty(), errs)
}

#[cfg(test)]
mod lint_tests {
    use super::*;

    #[test]
    fn a_library_is_read_for_the_engine_and_judged() {
        let sym = "META|LDO-X|reg|U\nMODE|0|n\nRECT|0|0|-300|600|0\nPIN|0|1|IN|0|-100|left|300||power_in\nPIN|0|2|OUT|600|-100|right|300||power_out\n";
        let fp = "META|SOT-X|x\nPAD|1|-1|0|1|0.6|rect|0|top|0\nPAD|2|1|0|1|0.6|rect|0|top|0\nLINE|courtyard|-2|-1|2|1|0.05\n";
        let (xml, _) = crate::import::import_part(None, Some(fp), Some(sym), None, &json!({"params": {"MPN": "LDO-X-1"}})).unwrap();
        // a footprint-less component keeps its pin numbers in ADOM_PINMAP
        let (bare, _) = create_symbol(None, &json!({"name": "BARE", "pins": "1,A,left,100;2,B,right,100"})).unwrap();
        let b = lint_input(&bare).unwrap();
        assert_eq!(b["symbols"][0]["pins"].as_array().unwrap().len(), 2, "{b}");
        assert_eq!((b["symbols"][0]["pins"][0]["number"].as_str(), b["symbols"][0]["footprint"].as_str()), (Some("1"), Some("")));
        let lib = lint_input(&xml).unwrap();
        assert_eq!(lib["footprints"][0]["pads"].as_array().unwrap().len(), 2);
        assert_eq!(lib["symbols"][0]["partNumber"], "LDO-X-1");
        assert_eq!(lib["symbols"][0]["footprint"], "SOT-X");
        assert_eq!(lib["footprints"][0]["silk"], json!([]), "no silk drawn: sent empty, so the engine reports it");
        assert_eq!(lib["footprints"][0]["bodies"], json!([]));
        assert_eq!(lib["footprints"][0]["courtyard"].as_array().unwrap().len(), 2);
        assert_eq!(lib["symbols"][0]["pins"].as_array().unwrap().len(), 2);
    }
}

#[cfg(test)]
mod art_tests {
    use super::*;

    #[test]
    fn kicad_art_is_fitted_to_the_eagle_body() {
        let sym = "META|IND-X|ind|L\nMODE|0|n\nRECT|0|0|-400|800|0\nPIN|0|1|1|0|-200|left|300||passive\nPIN|0|2|2|800|-200|right|300||passive\n";
        let fp = "META|IND-FP|x\nPAD|1|-1|0|1|1|rect|0|top|0\nPAD|2|1|0|1|1|rect|0|top|0\n";
        let (xml, _) = crate::import::import_part(None, Some(fp), Some(sym), None, &json!({})).unwrap();
        let ks = "(kicad_symbol_lib (symbol \"IND-X\" (symbol \"IND-X_0_1\" (rectangle (start -2 2) (end 2 -2)) (polyline (pts (xy -2 0) (xy 0 2) (xy 2 0)))) ))";
        let (out, r) = add_symbol_art(&xml, "ind-x", ks, true).unwrap();
        assert_eq!((r["polylines"].as_u64(), r["lines"].as_u64(), r["removed"].as_u64()), (Some(1), Some(2), Some(0)), "{r}");
        // KiCad (-2,0) -> the body's left edge at mid height: x 0, y -200 mil; (0,2) -> top middle
        assert!(out.contains(r#"<wire x1="0" y1="-5.08" x2="10.16" y2="0" width="0.1524" layer="94" />"#), "{out}");
        // again with clear: the old art goes, the body edges stay
        let (_, r2) = add_symbol_art(&out, "IND-X", ks, true).unwrap();
        assert_eq!(r2["removed"].as_u64(), Some(2));
        assert!(add_symbol_art(&xml, "IND-X", "(kicad_symbol_lib (symbol \"IND-X\" (rectangle (start 0 0) (end 1 1))))", true).is_err());
    }
}

#[cfg(test)]
mod prims_tests {
    use super::*;

    #[test]
    fn prims_count_the_drawing_and_visibility_toggles_a_text() {
        let sym = "META|LDO-X|reg|U\nMODE|0|n\nRECT|0|0|-300|600|0\nLINE|0|0|0|100|100\nPIN|0|1|IN|0|-100|left|300||power_in\nPIN|0|2|OUT|600|-100|right|300||power_out\n";
        let fp = "META|SOT-X|x\nPAD|1|-1|0|1|1|rect|0|top|0\nPAD|2|1|0|1|1|rect|0|top|0\n";
        let (xml, _) = crate::import::import_part(None, Some(fp), Some(sym), None, &json!({})).unwrap();
        let p = symbol_prims(&xml, "ldo-x").unwrap();
        assert_eq!((p["pins"].as_u64(), p["lines"].as_u64(), p["models"][0]["name"].as_str()), (Some(2), Some(5), Some("SOT-X")));
        assert_eq!(p["bodyRectMil"], json!([0.0, -300.0, 600.0, 100.0]));
        let (shown, r) = set_visibility(&xml, "LDO-X", "mpn", true).unwrap();
        assert_eq!(r["symbolsChanged"][0], "LDO-X");
        assert!(shown.contains(">MPN</text>") || shown.contains("&gt;MPN</text>"), "{shown}");
        let (again, r2) = set_visibility(&shown, "LDO-X", "MPN", true).unwrap();
        assert_eq!(r2["symbolsChanged"].as_array().unwrap().len(), 0, "already shown");
        let (hidden, _) = set_visibility(&again, "LDO-X", "MPN", false).unwrap();
        assert!(!hidden.contains("MPN</text>"));
    }
}

#[cfg(test)]
mod model_tests {
    use super::*;

    #[test]
    fn models_are_added_once_and_set_keeps_one() {
        let fp = "META|A2|two\nPAD|1|-1|0|1|1|rect|0|top|0\nPAD|2|1|0|1|1|rect|0|top|0\nPAD|2|1|1|0.5|0.5|rect|0|top|0\n";
        let fp_b = "META|B2|two\nPAD|1|-2|0|1|1|rect|0|top|0\nPAD|2|2|0|1|1|rect|0|top|0\n";
        let sym = "META|R-X|res|R\nMODE|0|n\nPIN|0|1|1|0|0|left|100||passive\nPIN|0|2|2|200|0|right|100||passive\n";
        let (xml, _) = crate::import::import_part(None, Some(fp), Some(sym), None, &json!({})).unwrap();
        let (other, _) = crate::import::import_part(None, Some(fp_b), None, None, &json!({})).unwrap();
        let (two, r) = add_models(&xml, "R-X", &[("B2".into(), Some(other.clone())), ("A2".into(), None)], &json!({"MPN": "X1"}), None, Some("a resistor")).unwrap();
        assert_eq!((r["models"].as_u64(), r["added"][0].as_str(), r["alreadyAttached"][0].as_str()), (Some(2), Some("B2"), Some("A2")), "{r}");
        assert!(two.contains(r#"<connect gate="G$1" pin="2" pad="2" />"#) && two.contains(r#"pad="2 2@2""#), "{two}");
        let (one, r) = set_models(&two, "r-x", "B2").unwrap();
        assert_eq!((r["removed"].as_u64(), r["footprint"].as_str()), (Some(2), Some("B2")));
        assert_eq!(one.matches("<device ").count(), 1);
        assert!(one.contains(r#"name="MPN" value="X1""#), "params ride on the kept device");
        assert!(set_models(&one, "R-X", "NOPE").is_err());
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn a_laid_out_land_pattern_becomes_a_package_that_preflights() {
        let layout = json!({
            "pads": [{"name": "1", "x": -2.7, "y": 0.635, "w": 1.55, "h": 0.6, "shape": "roundrect", "hole": 0},
                     {"name": "2", "x": 2.7, "y": 0.635, "w": 1.55, "h": 0.6, "shape": "rect", "hole": 0},
                     {"name": "3", "x": 0, "y": 0, "w": 1.7, "h": 1.7, "shape": "round", "hole": 0.8}],
            "lines": [{"layer": "overlay", "x1": -1.0, "y1": 2.5, "x2": 1.0, "y2": 2.5, "width": 0.15},
                      {"layer": "fab", "x1": -1.95, "y1": 2.45, "x2": 1.95, "y2": 2.45, "width": 0.1},
                      {"layer": "courtyard", "x1": -3.8, "y1": 2.8, "x2": 3.8, "y2": 2.8, "width": 0.05}],
            "discs": [{"layer": "overlay", "x": -3.9, "y": 0.635, "r": 0.15}]});
        let (xml, s) = create_from_layout(None, "SOIC-TEST", "test", &layout).unwrap();
        assert_eq!((s["pads"].as_u64(), s["silkLines"].as_u64(), s["fabLines"].as_u64(), s["courtyardLines"].as_u64()), (Some(3), Some(1), Some(1), Some(1)));
        assert!(xml.contains(r#"<smd name="1" x="-2.7" y="0.635" dx="1.55" dy="0.6" layer="1" roundness="25" />"#), "{xml}");
        assert!(xml.contains(r#"<smd name="2" x="2.7" y="0.635" dx="1.55" dy="0.6" layer="1" />"#));
        assert!(xml.contains(r#"<pad name="3" x="0" y="0" drill="0.8" diameter="1.7" />"#));
        assert!(xml.contains(r#"<circle x="-3.9" y="0.635" radius="0.15" width="0" layer="21" />"#));
        assert!(xml.contains(r#"layer="39" />"#) && xml.contains(r#"layer="51" />"#));
        let p = std::env::temp_dir().join(format!("adom-layout-test-{}.lbr", std::process::id()));
        std::fs::write(&p, &xml).unwrap();
        let (ok, errs) = preflight_text(&p);
        let _ = std::fs::remove_file(&p);
        assert!(ok, "{errs:?}");
        // the same name twice is refused
        assert!(create_from_layout(Some(&xml), "soic-test", "", &layout).is_err());
        assert!(create_from_layout(None, "X", "", &json!({"pads": [], "lines": []})).is_err());
    }
}

#[cfg(test)]
mod plate_cleanup_tests {
    use super::*;
    const LIB: &str = r#"<?xml version="1.0"?><eagle><drawing><library><packages>
<package name="QFN"><smd name="EP" x="0" y="0" dx="3" dy="3" layer="1"/><smd name="1" x="2" y="0" dx="0.3" dy="0.8" layer="1"/><hole x="0.5" y="0.5" drill="0.3"/><hole x="-0.5" y="-0.5" drill="0.3"/><hole x="5" y="5" drill="1"/></package>
<package name="EMPTY"></package></packages><symbols><symbol name="S"><pin name="GND" x="0" y="0"/><pin name="A" x="0" y="2"/></symbol><symbol name="BLANK"></symbol></symbols>
<devicesets><deviceset name="CHIP"><gates><gate name="G$1" symbol="S" x="0" y="0"/></gates><devices><device name="" package="QFN"><connects><connect gate="G$1" pin="GND" pad="EP"/><connect gate="G$1" pin="A" pad="1"/></connects></device></devices></deviceset>
<deviceset name="Component_1"><gates/><devices/></deviceset></devicesets></library></drawing></eagle>"#;

    #[test]
    fn holes_in_a_land_become_its_plated_vias_on_its_net() {
        let (x, s) = plate_vias(LIB, None).unwrap();
        assert_eq!((s["holes"].as_u64(), s["plated"].as_u64(), s["joinedToLands"].as_u64()), (Some(3), Some(3), Some(2)));
        assert_eq!(s["fixed"], "QFN:EP@2;QFN:EP@3;QFN:H1");
        assert!(x.contains(r#"pad="EP EP@2 EP@3""#) && !x.contains("<hole"));
        assert!(x.contains(r#"<pad name="H1" x="5" y="5" drill="1" />"#));
        assert_eq!(plate_vias(&x, Some("qfn")).unwrap().1["holes"], 0);
    }

    #[test]
    fn cleanup_takes_only_empty_unused_items() {
        let (x, s) = cleanup_libs(LIB).unwrap();
        assert_eq!(s["removed"], 3);
        assert!(x.contains(r#"name="CHIP""#) && !x.contains("Component_1") && !x.contains("BLANK") && !x.contains(r#"name="EMPTY""#));
        assert_eq!(cleanup_libs(&x).unwrap().1["removed"], 0);
    }
}

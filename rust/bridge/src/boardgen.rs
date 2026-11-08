//! The standard's test_board and build_board: a board written as EAGLE source from a .lbr's
//! footprints, offline, then opened in Fusion through the dialog-free Document.newDesignFromLocal
//! (as fusion_make_showcase_project does) and read back pad by pad from the live board. Fusion
//! keeps a board-only design for a .brd with no schematic beside it, so nothing is annotated.
//! Coordinates are mm, Y up, each footprint placed by its own origin.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;

use crate::addin::Ctx;
use fusion_lbr::Element;

fn f(v: f64) -> String { let s = format!("{:.4}", v); let s = s.trim_end_matches('0').trim_end_matches('.').to_string(); if s == "-0" { "0".into() } else { s } }
fn el(tag: &str, attrs: &[(&str, String)]) -> Element { let mut e = Element::new(tag); e.attrs = attrs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(); e }
fn num(e: &Element, k: &str) -> f64 { e.attr(k).and_then(|v| v.parse().ok()).unwrap_or(0.0) }
fn rot_deg(e: &Element) -> f64 { e.attr("rot").map(|r| r.trim_start_matches(['M', 'S']).trim_start_matches('R')).and_then(|r| r.parse().ok()).unwrap_or(0.0) }
fn deep<'a>(e: &'a Element, tag: &str) -> Option<&'a Element> { if e.tag == tag { return Some(e); } e.children.iter().find_map(|c| deep(c, tag)) }
fn child_named<'a>(e: &'a Element, tag: &str, name: &str) -> Option<&'a Element> { e.children.iter().find(|c| c.tag == tag && c.attr("name").map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false)) }

/// One footprint to place: designator, package, origin (mm) and rotation (degrees).
#[derive(Debug, Clone)]
pub struct Placement { pub r: String, pub pkg: String, pub x: f64, pub y: f64, pub rot: f64, pub value: String }

/// What compose builds: on `base` (an existing .brd, appended to) or a new board with `outline`.
pub struct Plan<'a> {
    pub base: Option<&'a str>,
    pub lbr: &'a str,
    pub lib_name: &'a str,
    pub outline: Vec<Element>,
    pub layer_setup: &'a str,
    pub parts: Vec<Placement>,
    /// net -> [(designator, pad)]
    pub nets: BTreeMap<String, Vec<(String, String)>>,
    /// (designator, pad, dx, dy): a 0.2 mm top-copper track from the pad to the part's origin + (dx, dy)
    pub stubs: Vec<(String, String, f64, f64)>,
}

/// The packages a .lbr holds, by name.
pub fn lbr_packages(lbr: &str) -> Result<Vec<Element>, String> {
    let root = Element::parse(lbr).map_err(|e| e.to_string())?;
    let pk = deep(&root, "packages").ok_or("the library has no <packages>")?;
    Ok(pk.children.iter().filter(|c| c.tag == "package").cloned().collect())
}

/// A package's pads: (name, x, y), in the package's own frame.
fn pads_of(pkg: &Element) -> Vec<(String, f64, f64)> {
    pkg.children.iter().filter(|c| c.tag == "smd" || c.tag == "pad").filter_map(|c| Some((c.attr("name")?.to_string(), num(c, "x"), num(c, "y")))).collect()
}

/// Everything a package draws (pads, wires, circles, rectangles), unrotated: x0, y0, x1, y1.
pub fn extent(pkg: &Element) -> (f64, f64, f64, f64) {
    let mut b = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let mut add = |x: f64, y: f64, hw: f64, hh: f64| { b.0 = b.0.min(x - hw); b.1 = b.1.min(y - hh); b.2 = b.2.max(x + hw); b.3 = b.3.max(y + hh); };
    for c in &pkg.children {
        match c.tag.as_str() {
            "smd" => { let (w, h) = (num(c, "dx") / 2.0, num(c, "dy") / 2.0); let turned = (rot_deg(c) / 90.0).round() as i64 % 2 != 0; if turned { add(num(c, "x"), num(c, "y"), h, w) } else { add(num(c, "x"), num(c, "y"), w, h) } }
            "pad" => { let d = num(c, "diameter").max(num(c, "drill") * 1.6) / 2.0; add(num(c, "x"), num(c, "y"), d, d) }
            "wire" => { let w = num(c, "width") / 2.0; add(num(c, "x1"), num(c, "y1"), w, w); add(num(c, "x2"), num(c, "y2"), w, w) }
            "circle" => { let r = num(c, "radius") + num(c, "width") / 2.0; add(num(c, "x"), num(c, "y"), r, r) }
            "rectangle" => { add(num(c, "x1"), num(c, "y1"), 0.0, 0.0); add(num(c, "x2"), num(c, "y2"), 0.0, 0.0) }
            _ => {}
        }
    }
    if b.0 == f64::MAX { (-0.5, -0.5, 0.5, 0.5) } else { b }
}

/// The extent of a package turned by `rot` degrees (its box's corners, turned).
pub fn turned_extent(pkg: &Element, rot: f64) -> (f64, f64, f64, f64) {
    let (x0, y0, x1, y1) = extent(pkg);
    let (s, c) = rot.to_radians().sin_cos();
    let pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| (x * c - y * s, x * s + y * c));
    pts.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, p| (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1)))
}

/// A rectangle (with rounded corners when `r` > 0) or a closed polygon, as layer-20 wires.
pub fn outline_wires(outline: &Value) -> Result<Vec<Element>, String> {
    let w = |x1: f64, y1: f64, x2: f64, y2: f64, curve: Option<f64>| { let mut e = el("wire", &[("x1", f(x1)), ("y1", f(y1)), ("x2", f(x2)), ("y2", f(y2)), ("width", "0.1".into()), ("layer", "20".into())]); if let Some(c) = curve { e.attrs.push(("curve".into(), f(c))); } e };
    if let Some(r) = outline["rect"].as_array() {
        let g = |k: usize| r.get(k).and_then(|v| v.as_f64()).ok_or("outline.rect is [x0, y0, w, h] in mm");
        let (x0, y0, wd, ht) = (g(0)?, g(1)?, g(2)?, g(3)?);
        if wd <= 0.0 || ht <= 0.0 { return Err("outline.rect: w and h must be above 0".into()); }
        let (x1, y1) = (x0 + wd, y0 + ht);
        let cr = outline["cornerRadius"].as_f64().unwrap_or(0.0).clamp(0.0, wd.min(ht) / 2.0);
        if cr <= 0.0 { return Ok(vec![w(x0, y0, x1, y0, None), w(x1, y0, x1, y1, None), w(x1, y1, x0, y1, None), w(x0, y1, x0, y0, None)]); }
        // counter-clockwise: each edge, then a 90-degree arc round the next corner
        let mut v = Vec::new();
        for (a, b, arc_to) in [((x0 + cr, y0), (x1 - cr, y0), (x1, y0 + cr)), ((x1, y0 + cr), (x1, y1 - cr), (x1 - cr, y1)), ((x1 - cr, y1), (x0 + cr, y1), (x0, y1 - cr)), ((x0, y1 - cr), (x0, y0 + cr), (x0 + cr, y0))] {
            if (a.0 - b.0).abs() + (a.1 - b.1).abs() > 1e-9 { v.push(w(a.0, a.1, b.0, b.1, None)); }
            v.push(w(b.0, b.1, arc_to.0, arc_to.1, Some(90.0)));
        }
        return Ok(v);
    }
    if let Some(p) = outline["points"].as_array() {
        let pts: Vec<(f64, f64)> = p.iter().filter_map(|q| Some((q.get(0)?.as_f64()?, q.get(1)?.as_f64()?))).collect();
        if pts.len() < 3 || pts.len() != p.len() { return Err("outline.points is [[x, y], ...] in mm, three at least".into()); }
        return Ok((0..pts.len()).map(|i| { let (a, b) = (pts[i], pts[(i + 1) % pts.len()]); w(a.0, a.1, b.0, b.1, None) }).collect());
    }
    Err("outline is {rect: [x0, y0, w, h], cornerRadius?} or {points: [[x, y], ...]} (mm)".into())
}

/// The board XML, and what could not be wired (pads the footprint lacks, pads already on another net).
pub fn compose(p: &Plan) -> Result<(String, Value), String> {
    let lib_pkgs = lbr_packages(p.lbr)?;
    let lib_root = Element::parse(p.lbr).map_err(|e| e.to_string())?;
    let lib_p3d: Vec<Element> = deep(&lib_root, "packages3d").map(|s| s.children.iter().filter(|c| c.tag == "package3d").cloned().collect()).unwrap_or_default();
    let skeleton;
    let text = match p.base { Some(t) => t, None => {
        skeleton = format!("{}{}<board><plain/><libraries/><attributes/><variantdefs/><classes><class number=\"0\" name=\"default\" width=\"0\" drill=\"0\"/></classes><designrules name=\"default\"><param name=\"layerSetup\" value=\"{}\"/></designrules><autorouter/><elements/><signals/></board></drawing></eagle>\n",
            crate::testproject::HEAD, crate::testproject::board_layers(), p.layer_setup);
        &skeleton
    } };
    let mut root = Element::parse(text).map_err(|e| e.to_string())?;
    let board = root.find_deep_mut("board").ok_or("not a board: no <board>")?;
    for t in ["plain", "libraries", "elements", "signals"] { if board.find_child(t).is_none() { board.children.push(Element::new(t)); } }
    // the library: the footprints the parts use (and the 3D packages that instance them)
    let mut urn: BTreeMap<String, String> = BTreeMap::new();
    let mut added = Vec::new();
    {
        let libs = board.find_child_mut("libraries").unwrap();
        if child_named(libs, "library", p.lib_name).is_none() { let mut l = el("library", &[("name", p.lib_name.to_string())]); l.children.push(Element::new("packages")); libs.children.push(l); }
        let lib = libs.children.iter_mut().find(|c| c.tag == "library" && c.attr("name").map(|n| n.eq_ignore_ascii_case(p.lib_name)).unwrap_or(false)).unwrap();
        if lib.find_child("packages").is_none() { lib.children.insert(0, Element::new("packages")); }
        for part in &p.parts {
            let src = lib_pkgs.iter().find(|k| k.attr("name").map(|n| n.eq_ignore_ascii_case(&part.pkg)).unwrap_or(false)).ok_or(format!("footprint {} is not in the library", part.pkg))?;
            let name = src.attr("name").unwrap_or("").to_string();
            let pk = lib.find_child_mut("packages").unwrap();
            if child_named(pk, "package", &name).is_none() { pk.children.push(src.clone()); added.push(name.clone()); }
            for d in &lib_p3d {
                let inst = d.find_child("packageinstances").map(|pi| pi.children.iter().any(|c| c.attr("name") == Some(name.as_str()))).unwrap_or(false);
                let u = d.attr("wip_urn").or(d.attr("urn")).unwrap_or("");
                if !inst || u.is_empty() { continue; }
                urn.insert(name.clone(), u.to_string());
                if lib.find_child("packages3d").is_none() { let at = lib.children.iter().position(|c| c.tag == "packages").map(|i| i + 1).unwrap_or(lib.children.len()); lib.children.insert(at, Element::new("packages3d")); }
                let s3 = lib.find_child_mut("packages3d").unwrap();
                if child_named(s3, "package3d", d.attr("name").unwrap_or("")).is_none() { s3.children.push(d.clone()); }
                break;
            }
        }
    }
    // every footprint on the board by designator, for pads and origins
    let mut on_board: BTreeMap<String, (Element, f64, f64, f64)> = BTreeMap::new();
    {
        let libs = board.find_child("libraries").unwrap();
        for e in board.find_child("elements").unwrap().children.iter().filter(|c| c.tag == "element") {
            let (Some(n), Some(l), Some(pk)) = (e.attr("name"), e.attr("library"), e.attr("package")) else { continue };
            if let Some(pkg) = child_named(libs, "library", l).and_then(|lb| lb.find_child("packages")).and_then(|s| child_named(s, "package", pk)) { on_board.insert(n.to_uppercase(), (pkg.clone(), num(e, "x"), num(e, "y"), rot_deg(e))); }
        }
    }
    {
        let elements = board.find_child_mut("elements").unwrap();
        for part in &p.parts {
            if on_board.contains_key(&part.r.to_uppercase()) { return Err(format!("{} is already on the board", part.r)); }
            let src = lib_pkgs.iter().find(|k| k.attr("name").map(|n| n.eq_ignore_ascii_case(&part.pkg)).unwrap_or(false)).unwrap();
            let name = src.attr("name").unwrap_or("").to_string();
            let mut e = el("element", &[("name", part.r.clone()), ("library", p.lib_name.to_string()), ("package", name.clone()), ("value", part.value.clone()), ("x", f(part.x)), ("y", f(part.y))]);
            let r = part.rot.rem_euclid(360.0);
            if r.abs() > 1e-9 { e.attrs.push(("rot".into(), format!("R{}", f(r)))); }
            if let Some(u) = urn.get(&name) { e.attrs.push(("package3d_urn".into(), u.clone())); }
            elements.children.push(e);
            on_board.insert(part.r.to_uppercase(), (src.clone(), part.x, part.y, r));
        }
    }
    let pad_at = |r: &str, pad: &str| -> Option<(f64, f64)> {
        let (pkg, x, y, rot) = on_board.get(&r.to_uppercase())?;
        let (_, px, py) = pads_of(pkg).into_iter().find(|q| q.0 == pad)?;
        let (s, c) = rot.to_radians().sin_cos();
        Some((x + px * c - py * s, y + px * s + py * c))
    };
    let mut unknown: Vec<Value> = Vec::new();
    let mut conflicts: Vec<Value> = Vec::new();
    {
        let signals = board.find_child_mut("signals").unwrap();
        // the pads each existing signal already holds
        let mut held: BTreeMap<(String, String), String> = BTreeMap::new();
        for s in signals.children.iter().filter(|c| c.tag == "signal") { for c in s.children.iter().filter(|c| c.tag == "contactref") { held.insert((c.attr("element").unwrap_or("").to_uppercase(), c.attr("pad").unwrap_or("").to_string()), s.attr("name").unwrap_or("").to_string()); } }
        for (net, pins) in &p.nets {
            for (r, pad) in pins {
                if pad_at(r, pad).is_none() { unknown.push(json!({"pin": format!("{r}.{pad}"), "net": net, "why": if on_board.contains_key(&r.to_uppercase()) { "the footprint has no pad of that name" } else { "no such part on the board" }})); continue; }
                let key = (r.to_uppercase(), pad.clone());
                match held.get(&key) {
                    Some(n) if n == net => continue,
                    Some(n) => { conflicts.push(json!({"pin": format!("{r}.{pad}"), "want": net, "already": n})); continue; }
                    None => {}
                }
                if !signals.children.iter().any(|c| c.tag == "signal" && c.attr("name") == Some(net.as_str())) { signals.children.push(el("signal", &[("name", net.clone())])); }
                let s = signals.children.iter_mut().find(|c| c.tag == "signal" && c.attr("name") == Some(net.as_str())).unwrap();
                let real = on_board.get_key_value(&r.to_uppercase()).map(|_| r.clone()).unwrap();
                s.children.push(el("contactref", &[("element", real), ("pad", pad.clone())]));
                held.insert(key, net.clone());
            }
        }
        for (r, pad, dx, dy) in &p.stubs {
            let Some((px, py)) = pad_at(r, pad) else { unknown.push(json!({"pin": format!("{r}.{pad}"), "why": "stub: the footprint has no pad of that name"})); continue };
            let (_, ox, oy, _) = on_board.get(&r.to_uppercase()).unwrap();
            let wire = el("wire", &[("x1", f(px)), ("y1", f(py)), ("x2", f(ox + dx)), ("y2", f(oy + dy)), ("width", "0.2".into()), ("layer", "1".into())]);
            match held.get(&(r.to_uppercase(), pad.clone())).cloned() {
                Some(net) => signals.children.iter_mut().find(|c| c.tag == "signal" && c.attr("name") == Some(net.as_str())).unwrap().children.push(wire),
                None => { let mut s = el("signal", &[("name", format!("STUB_{r}_{pad}"))]); s.children.push(el("contactref", &[("element", r.clone()), ("pad", pad.clone())])); s.children.push(wire); signals.children.push(s); }
            }
        }
    }
    let mut grown = Value::Null;
    let mut outside: Vec<String> = Vec::new();
    if p.base.is_none() { board.find_child_mut("plain").unwrap().children.extend(p.outline.iter().cloned()); }
    else if !p.parts.is_empty() {
        // appended parts: a plain rectangular outline grows to hold them (2 mm round them); any other
        // shape stays, and the parts that fall outside it are named
        let need = p.parts.iter().map(|q| { let (pkg, x, y, r) = on_board.get(&q.r.to_uppercase()).unwrap(); let e = turned_extent(pkg, *r); (x + e.0, y + e.1, x + e.2, y + e.3) })
            .fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |a, e| (a.0.min(e.0), a.1.min(e.1), a.2.max(e.2), a.3.max(e.3)));
        let plain = board.find_child_mut("plain").unwrap();
        let edges: Vec<usize> = plain.children.iter().enumerate().filter(|(_, c)| c.tag == "wire" && c.attr("layer") == Some("20")).map(|(i, _)| i).collect();
        let rect = edges.len() == 4 && edges.iter().all(|&i| { let c = &plain.children[i]; c.attr("curve").map(|v| v.parse::<f64>().unwrap_or(0.0) == 0.0).unwrap_or(true) && ((num(c, "x1") - num(c, "x2")).abs() < 1e-6 || (num(c, "y1") - num(c, "y2")).abs() < 1e-6) });
        let bb = edges.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |a, &i| { let c = &plain.children[i]; (a.0.min(num(c, "x1").min(num(c, "x2"))), a.1.min(num(c, "y1").min(num(c, "y2"))), a.2.max(num(c, "x1").max(num(c, "x2"))), a.3.max(num(c, "y1").max(num(c, "y2")))) });
        let fits = need.0 >= bb.0 && need.1 >= bb.1 && need.2 <= bb.2 && need.3 <= bb.3;
        if !fits && rect {
            let n = (bb.0.min(need.0 - 2.0), bb.1.min(need.1 - 2.0), bb.2.max(need.2 + 2.0), bb.3.max(need.3 + 2.0));
            let width = plain.children[edges[0]].attr("width").unwrap_or("0.1").to_string();
            let mut new_edges = outline_wires(&json!({"rect": [n.0, n.1, n.2 - n.0, n.3 - n.1]})).unwrap();
            for e in new_edges.iter_mut() { if let Some(a) = e.attrs.iter_mut().find(|a| a.0 == "width") { a.1 = width.clone(); } }
            for (k, &i) in edges.iter().enumerate() { plain.children[i] = new_edges[k].clone(); }
            grown = json!({"fromMm": [bb.0, bb.1, bb.2, bb.3], "toMm": [n.0, n.1, n.2, n.3]});
        } else if !fits && !edges.is_empty() {
            for q in &p.parts { let (pkg, x, y, r) = on_board.get(&q.r.to_uppercase()).unwrap(); let e = turned_extent(pkg, *r); if x + e.0 < bb.0 || y + e.1 < bb.1 || x + e.2 > bb.2 || y + e.3 > bb.3 { outside.push(q.r.clone()); } }
        }
    }
    Ok((root.to_xml_document(), json!({"unknownPins": unknown, "conflicts": conflicts, "packagesAdded": added, "outlineGrown": grown, "outsideOutline": outside})))
}

/// The standard's swap_footprint on board text: the part (`des`, else the first) takes `pkg` from the
/// .lbr at the same place and turn; its pads leave their nets and take `assigns` (pad number -> net,
/// every pad of that number: N and its repeats N@2 ...). Tracks stay.
pub fn swap(text: &str, lbr: &str, lib_name: &str, des: Option<&str>, pkg: &str, assigns: &[(String, String)]) -> Result<(String, Value), String> {
    let lib_pkgs = lbr_packages(lbr)?;
    let src = lib_pkgs.iter().find(|k| k.attr("name").map(|n| n.eq_ignore_ascii_case(pkg)).unwrap_or(false)).ok_or(format!("footprint {pkg} is not in the library"))?.clone();
    let name = src.attr("name").unwrap_or(pkg).to_string();
    let lib_root = Element::parse(lbr).map_err(|e| e.to_string())?;
    let urn = deep(&lib_root, "packages3d").and_then(|s| s.children.iter().find(|d| d.find_child("packageinstances").map(|pi| pi.children.iter().any(|c| c.attr("name") == Some(name.as_str()))).unwrap_or(false)))
        .and_then(|d| d.attr("wip_urn").or(d.attr("urn")).map(str::to_string)).filter(|u| !u.is_empty());
    let mut root = Element::parse(text).map_err(|e| e.to_string())?;
    let board = root.find_deep_mut("board").ok_or("not a board: no <board>")?;
    let r = {
        let els = board.find_child_mut("elements").ok_or("the board has no parts")?;
        let e = match des { Some(d) => els.children.iter_mut().find(|c| c.tag == "element" && c.attr("name").map(|n| n.eq_ignore_ascii_case(d)).unwrap_or(false)).ok_or(format!("no part {d} on the board"))?,
                            None => els.children.iter_mut().find(|c| c.tag == "element").ok_or("the board has no parts")? };
        let old = e.attr("package").unwrap_or("").to_string();
        for (k, v) in [("library", lib_name.to_string()), ("package", name.clone())] { match e.attrs.iter_mut().find(|a| a.0 == k) { Some(a) => a.1 = v, None => e.attrs.push((k.into(), v)) } }
        e.attrs.retain(|a| a.0 != "package3d_urn");
        if let Some(u) = &urn { e.attrs.push(("package3d_urn".into(), u.clone())); }
        (e.attr("name").unwrap_or("").to_string(), old)
    };
    {
        let libs = board.find_child_mut("libraries").ok_or("the board has no libraries")?;
        if child_named(libs, "library", lib_name).is_none() { let mut l = el("library", &[("name", lib_name.to_string())]); l.children.push(Element::new("packages")); libs.children.push(l); }
        let lib = libs.children.iter_mut().find(|c| c.tag == "library" && c.attr("name").map(|n| n.eq_ignore_ascii_case(lib_name)).unwrap_or(false)).unwrap();
        if lib.find_child("packages").is_none() { lib.children.insert(0, Element::new("packages")); }
        let pk = lib.find_child_mut("packages").unwrap();
        // a footprint of the same name already there is replaced: the swap brings the library's version
        pk.children.retain(|c| !(c.tag == "package" && c.attr("name").map(|n| n.eq_ignore_ascii_case(&name)).unwrap_or(false)));
        pk.children.push(src.clone());
    }
    let pads: Vec<String> = pads_of(&src).into_iter().map(|p| p.0).collect();
    let base = |p: &str| p.split('@').next().unwrap_or(p).to_string();
    let mut wired = 0; let mut unknown = Vec::new();
    {
        let signals = match board.find_child_mut("signals") { Some(s) => s, None => { board.children.push(Element::new("signals")); board.find_child_mut("signals").unwrap() } };
        for s in signals.children.iter_mut().filter(|c| c.tag == "signal") { s.children.retain(|c| !(c.tag == "contactref" && c.attr("element").map(|n| n.eq_ignore_ascii_case(&r.0)).unwrap_or(false))); }
        for (num, net) in assigns {
            let mine: Vec<&String> = pads.iter().filter(|p| base(p) == *num || *p == num).collect();
            if mine.is_empty() { unknown.push(json!({"pad": num, "net": net})); continue; }
            if !signals.children.iter().any(|c| c.tag == "signal" && c.attr("name") == Some(net.as_str())) { signals.children.push(el("signal", &[("name", net.clone())])); }
            let sg = signals.children.iter_mut().find(|c| c.tag == "signal" && c.attr("name") == Some(net.as_str())).unwrap();
            for p in mine { sg.children.push(el("contactref", &[("element", r.0.clone()), ("pad", p.clone())])); wired += 1; }
        }
    }
    Ok((root.to_xml_document(), json!({"designator": r.0, "from": r.1, "to": name, "padsWired": wired, "unknownPads": unknown, "has3d": urn.is_some()})))
}

/// `fusion_swap_footprint`: a part on a board kept as EAGLE source takes another footprint at the
/// same place; nets come from assigns; the board reopens in Fusion and is read back.
pub fn swap_footprint(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let (lbr_path, lbr) = match read_lbr(args) { Ok(x) => x, Err(e) => return e };
    let Some(pcb) = args["pcbPath"].as_str().filter(|s| s.to_lowercase().ends_with(".brd")).map(|s| s.replace('\\', "/")) else {
        return json!({"success": false, "errorCode": "bad_args", "error": "pcbPath (an EAGLE .brd, as fusion_test_board writes) is required",
            "_hint": "Fusion swaps a footprint on a board kept as EAGLE source. On an open cloud design, a part takes another of its component's footprints in Fusion's PACKAGE command, or another component with fusion_sch_swap_part."}) };
    let Some(fp) = args["footprintName"].as_str().filter(|s| !s.is_empty()) else { return json!({"success": false, "errorCode": "bad_args", "error": "footprintName is required"}) };
    let text = match std::fs::read_to_string(&pcb) { Ok(t) => t, Err(e) => return json!({"success": false, "errorCode": "board_unreadable", "error": format!("{pcb}: {e}")}) };
    let assigns: Vec<(String, String)> = list(&args["assigns"], ';').iter().filter_map(|a| a.split_once('=').map(|(p, n)| (p.trim().to_string(), n.trim().to_string()))).collect();
    let (xml, rep) = match swap(&text, &lbr, &lib_name_of(&lbr_path), args["designator"].as_str(), fp, &assigns) { Ok(x) => x, Err(e) => return json!({"success": false, "errorCode": "swap_refused", "error": e}) };
    if let Err(e) = write(&pcb, &xml) { return e; }
    let des = rep["designator"].as_str().unwrap_or("").to_string();
    let mut out = json!({"success": true, "mutated": true, "pcb": pcb, "swap": rep});
    if args["open"].as_bool() == Some(false) { return out; }
    match open_and_read(ctx, &pcb, pass) {
        Err(e) => e,
        Ok((doc, st)) => {
            let lc = st["components"].as_array().and_then(|a| a.iter().find(|c| c["ref"].as_str().map(|r| r.eq_ignore_ascii_case(&des)).unwrap_or(false)).cloned());
            out["documentName"] = json!(doc);
            out["data"] = json!({"placed": des, "padsOnComp": st["pads"].as_array().map(|a| a.iter().filter(|p| p["ref"].as_str().map(|r| r.eq_ignore_ascii_case(&des)).unwrap_or(false)).count()).unwrap_or(0),
                "bodiesOnComp": if lc.map(|c| c["has3d"] == true).unwrap_or(false) { 1 } else { 0 }});
            out["_hint"] = json!(format!("{des} now carries {fp}, reopened in Fusion as {doc}. Pads not in assigns have no net. fusion_show_3d_board to see it."));
            out
        }
    }
}

/// Every coordinate of an element and its children moved by (dx, dy).
fn shift(e: &mut Element, dx: f64, dy: f64) {
    for (k, v) in e.attrs.iter_mut() {
        let d = match k.as_str() { "x" | "x1" | "x2" | "x3" => dx, "y" | "y1" | "y2" | "y3" => dy, _ => continue };
        if let Ok(n) = v.parse::<f64>() { *v = f(n + d); }
    }
    for c in e.children.iter_mut() { shift(c, dx, dy); }
}

/// The panel: the board's copies at the plan's copyShift (copy 1 keeps its names; copy k renames
/// parts and nets with _k so the copies stay apart), each copy's own outline left out (layer 20),
/// and the panel's outline, cut lines and texts, holes, tooling holes and fiducials from the plan.
pub fn compose_panel(source: &str, plan: &Value) -> Result<(String, Value), String> {
    let shifts: Vec<(f64, f64)> = plan["copyShift"].as_array().ok_or("the plan has no copyShift")?.iter().filter_map(|s| Some((s.get(0)?.as_f64()?, s.get(1)?.as_f64()?))).collect();
    if shifts.is_empty() { return Err("the plan places no copies".into()); }
    let mut root = Element::parse(source).map_err(|e| e.to_string())?;
    let board = root.find_deep_mut("board").ok_or("not a board: no <board>")?;
    for t in ["plain", "elements", "signals"] { if board.find_child(t).is_none() { board.children.push(Element::new(t)); } }
    // Fusion's source carries the board's computed shape; the panel's comes from its own outline
    board.children.retain(|c| c.tag != "boardshape");
    let outline = |c: &Element| c.attr("layer") == Some("20");
    let plain0: Vec<Element> = board.find_child("plain").unwrap().children.iter().filter(|c| !outline(c)).cloned().collect();
    let els0: Vec<Element> = board.find_child("elements").unwrap().children.clone();
    let sigs0: Vec<Element> = board.find_child("signals").unwrap().children.clone();
    let suffix = |k: usize, n: &str| if k == 0 { n.to_string() } else { format!("{n}_{}", k + 1) };
    let (mut plain, mut els, mut sigs) = (Vec::new(), Vec::new(), Vec::new());
    for (k, (dx, dy)) in shifts.iter().enumerate() {
        for c in &plain0 { let mut c = c.clone(); shift(&mut c, *dx, *dy); plain.push(c); }
        for e in &els0 {
            let mut e = e.clone(); shift(&mut e, *dx, *dy);
            if let Some(a) = e.attrs.iter_mut().find(|a| a.0 == "name") { a.1 = suffix(k, &a.1); }
            els.push(e);
        }
        for sg in &sigs0 {
            let mut sg = sg.clone(); shift(&mut sg, *dx, *dy);
            if let Some(a) = sg.attrs.iter_mut().find(|a| a.0 == "name") { a.1 = suffix(k, &a.1); }
            for c in sg.children.iter_mut().filter(|c| c.tag == "contactref") { if let Some(a) = c.attrs.iter_mut().find(|a| a.0 == "element") { a.1 = suffix(k, &a.1); } }
            sigs.push(sg);
        }
    }
    let w = |x1: f64, y1: f64, x2: f64, y2: f64, wd: f64| el("wire", &[("x1", f(x1)), ("y1", f(y1)), ("x2", f(x2)), ("y2", f(y2)), ("width", f(wd)), ("layer", "20".into())]);
    let pm = plan["panelMm"].as_array().and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))).ok_or("the plan has no panelMm")?;
    for (a, b, c, d) in [(0.0, 0.0, pm.0, 0.0), (pm.0, 0.0, pm.0, pm.1), (pm.0, pm.1, 0.0, pm.1), (0.0, pm.1, 0.0, 0.0)] { plain.push(w(a, b, c, d, 0.1)); }
    let g = |v: &Value, k: &str| v[k].as_f64().unwrap_or(0.0);
    let ol = &plan["outlineLayer"];
    for l in ol["lines"].as_array().into_iter().flatten() { plain.push(w(g(l, "x1"), g(l, "y1"), g(l, "x2"), g(l, "y2"), l["width"].as_f64().unwrap_or(0.1))); }
    for t in ol["texts"].as_array().into_iter().flatten() {
        let mut e = el("text", &[("x", f(g(t, "x"))), ("y", f(g(t, "y"))), ("size", f(t["size"].as_f64().unwrap_or(1.0))), ("layer", "20".into())]);
        let r = g(t, "rotation"); if r.abs() > 1e-9 { e.attrs.push(("rot".into(), format!("R{}", f(r)))); }
        e.text = t["text"].as_str().unwrap_or("").into(); plain.push(e);
    }
    let holes: Vec<&Value> = plan["holes"].as_array().into_iter().flatten().chain(plan["toolingHoles"].as_array().into_iter().flatten()).collect();
    for h in &holes { plain.push(el("hole", &[("x", f(g(h, "x"))), ("y", f(g(h, "y"))), ("drill", f(h["drill"].as_f64().unwrap_or(0.5)))])); }
    // a fiducial: a 1 mm copper dot in a 2 mm mask opening, as fusion_board_features places them
    let fids = plan["fiducials"].as_array().cloned().unwrap_or_default();
    for fd in &fids {
        for (layer, r) in [("1", 0.5), ("29", 1.0)] { plain.push(el("circle", &[("x", f(g(fd, "x"))), ("y", f(g(fd, "y"))), ("radius", f(r)), ("width", "0".into()), ("layer", layer.into())])); }
    }
    let parts = els.len();
    board.find_child_mut("plain").unwrap().children = plain;
    board.find_child_mut("elements").unwrap().children = els;
    board.find_child_mut("signals").unwrap().children = sigs;
    Ok((root.to_xml_document(), json!({"copies": shifts.len(), "parts": parts, "holes": holes.len(), "fiducials": fids.len()})))
}

/// `fusion_panelize`: the engine's panel plan for the open board, built as a new EAGLE board (the
/// board's source with its copies placed) and opened in Fusion as a local design, then read back.
pub fn panelize(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let plan = crate::checks::call("panelize", args, pass);
    if plan["success"].as_bool() != Some(true) || args["dryRun"].as_bool() == Some(true) { return plan; }
    let src = crate::routing::call("board_source", &json!({"expectDocument": args["expectDocument"]}), pass);
    let Some(source) = src["source"].as_str().or(src["data"]["source"].as_str()) else { return json!({"success": false, "errorCode": "board_unreadable", "error": src["error"], "plan": plan}) };
    let doc = src["documentName"].as_str().or(args["expectDocument"].as_str()).unwrap_or("board").to_string();
    let safe: String = doc.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let out = args["outputPath"].as_str().map(|s| s.replace('\\', "/")).unwrap_or_else(|| std::env::temp_dir().join("adom-panels").join(format!("{safe}-panel-{}x{}.brd", plan["rows"], plan["cols"])).to_string_lossy().replace('\\', "/"));
    let (xml, rep) = match compose_panel(source, &plan) { Ok(x) => x, Err(e) => return json!({"success": false, "errorCode": "panel_refused", "error": e, "plan": plan}) };
    if let Err(e) = write(&out, &xml) { return e; }
    let mut r = plan.clone();
    r["mutated"] = json!(false); r["pcbPath"] = json!(out); r["built"] = rep.clone();
    if args["open"].as_bool() == Some(false) { r["_hint"] = json!("The panel is written (open:false); the board you panelized is untouched."); return r; }
    match open_and_read(ctx, &out, pass) {
        Err(e) => { let mut e = e; e["plan"] = plan; e }
        Ok((pdoc, st)) => {
            r["documentName"] = json!(pdoc);
            r["readBack"] = json!({"components": st["components"].as_array().map(|a| a.len()), "expected": rep["parts"], "unconnected": st["unconnected"], "outlineClosed": st["outlineClosed"]});
            r["_hint"] = json!(format!("The panel is open as {pdoc}, a new local design; the board you panelized is untouched. Copy k's parts and nets carry _k. Export it with fusion_export_gerbers (the outline layer carries the rout path, V-cut lines and texts); jlcpcb.rules says what JLCPCB will make of it."));
            r
        }
    }
}

/// A package's pads by name: (x, y, w, h, shape, drill, turn, layer).
fn pad_rows(pkg: &Element) -> BTreeMap<String, (f64, f64, f64, f64, String, f64, f64, String)> {
    pkg.children.iter().filter(|c| c.tag == "smd" || c.tag == "pad").filter_map(|c| {
        let n = c.attr("name")?.to_string();
        let row = if c.tag == "smd" { (num(c, "x"), num(c, "y"), num(c, "dx"), num(c, "dy"), format!("smd roundness {}", c.attr("roundness").unwrap_or("0")), 0.0, rot_deg(c), c.attr("layer").unwrap_or("1").to_string()) }
                  else { let d = num(c, "diameter"); (num(c, "x"), num(c, "y"), d, d, format!("pad {}", c.attr("shape").unwrap_or("round")), num(c, "drill"), rot_deg(c), "1-16".into()) };
        Some((n, row))
    }).collect()
}

/// What differs between a placed footprint (the board's embedded package) and the library's.
fn pad_diff(board: &Element, lib: &Element) -> Option<Value> {
    let (a, b) = (pad_rows(board), pad_rows(lib));
    let added: Vec<&String> = b.keys().filter(|k| !a.contains_key(*k)).collect();
    let removed: Vec<&String> = a.keys().filter(|k| !b.contains_key(*k)).collect();
    let mut changed = Vec::new();
    for (k, x) in &a {
        let Some(y) = b.get(k) else { continue };
        let mut c = Vec::new();
        let e = |p: f64, q: f64| (p - q).abs() > 1e-4;
        if e(x.0, y.0) || e(x.1, y.1) { c.push(format!("at ({}, {}) -> ({}, {})", f(x.0), f(x.1), f(y.0), f(y.1))); }
        if e(x.2, y.2) || e(x.3, y.3) { c.push(format!("size {} x {} -> {} x {}", f(x.2), f(x.3), f(y.2), f(y.3))); }
        if x.4 != y.4 { c.push(format!("shape {} -> {}", x.4, y.4)); }
        if e(x.5, y.5) { c.push(format!("drill {} -> {}", f(x.5), f(y.5))); }
        if e(x.6, y.6) { c.push(format!("turn {} -> {}", f(x.6), f(y.6))); }
        if x.7 != y.7 { c.push(format!("layer {} -> {}", x.7, y.7)); }
        if !c.is_empty() { changed.push(json!({"pad": k, "changes": c})); }
    }
    if added.is_empty() && removed.is_empty() && changed.is_empty() { None } else { Some(json!({"padsAdded": added, "padsRemoved": removed, "padsChanged": changed})) }
}

/// `fusion_update_footprints_from_library`: the standard's update_footprints_from_library, the plan.
/// Each placed footprint (the package the board embeds) is compared pad by pad with the same-named
/// package in the libraries given (a Fusion design's libraries are cloud items, so the .lbr files to
/// compare against are named). apply is refused: Fusion's UPDATE asks about each part in a dialog,
/// which the bridge never raises.
pub fn update_footprints_from_library(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let paths: Vec<String> = match (args["libraries"].as_array(), args["libraryPath"].as_str()) {
        (Some(a), _) => a.iter().filter_map(|x| x.as_str().map(|s| s.replace('\\', "/"))).collect(),
        (None, Some(p)) => vec![p.replace('\\', "/")],
        _ => vec![],
    };
    if paths.is_empty() { return json!({"success": false, "errorCode": "bad_args", "error": "libraryPath or libraries ([.lbr]) is required: a Fusion design's libraries are cloud items, so name the .lbr files to compare against",
        "_hint": "fusion_export_library_part or the .lbr a part came from; fusion_list_installed_libraries lists the hub's libraries."}); }
    let mut libs: Vec<(String, Vec<Element>)> = Vec::new();
    for p in &paths { match std::fs::read_to_string(p).map_err(|e| e.to_string()).and_then(|t| lbr_packages(&t)) { Ok(pk) => libs.push((p.clone(), pk)), Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": format!("{p}: {e}")}) } }
    let src = crate::routing::call("board_source", &json!({"expectDocument": args["expectDocument"]}), pass);
    let Some(text) = src["source"].as_str().or(src["data"]["source"].as_str()) else { return json!({"success": false, "errorCode": "board_unreadable", "error": src["error"]}) };
    let root = match Element::parse(text) { Ok(r) => r, Err(e) => return json!({"success": false, "errorCode": "board_unreadable", "error": e.to_string()}) };
    let Some(board) = deep(&root, "board") else { return json!({"success": false, "errorCode": "board_unreadable", "error": "no <board>"}) };
    let refs: Option<Vec<String>> = args["refs"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_uppercase)).collect());
    let blibs = board.find_child("libraries");
    let mut rows = Vec::new();
    let mut differs = 0;
    for e in board.find_child("elements").map(|x| x.children.iter().filter(|c| c.tag == "element").collect::<Vec<_>>()).unwrap_or_default() {
        let (r, l, pk) = (e.attr("name").unwrap_or(""), e.attr("library").unwrap_or(""), e.attr("package").unwrap_or(""));
        if let Some(want) = &refs { if !want.contains(&r.to_uppercase()) { continue; } }
        let placed = blibs.and_then(|b| child_named(b, "library", l)).and_then(|x| x.find_child("packages")).and_then(|x| child_named(x, "package", pk));
        let found = libs.iter().find_map(|(p, pks)| pks.iter().find(|k| k.attr("name").map(|n| n.eq_ignore_ascii_case(pk)).unwrap_or(false)).map(|k| (p.clone(), k)));
        let row = match (placed, found) {
            (Some(pl), Some((path, lk))) => match pad_diff(pl, lk) {
                Some(d) => { differs += 1; json!({"ref": r, "footprint": pk, "library": path, "status": "differs", "diff": d}) }
                None => json!({"ref": r, "footprint": pk, "library": path, "status": "up_to_date"}),
            },
            (_, None) => json!({"ref": r, "footprint": pk, "library": l, "status": "library_not_found", "why": "no package of that name in the libraries given"}),
            (None, Some((path, _))) => json!({"ref": r, "footprint": pk, "library": path, "status": "library_not_found", "why": "the board holds no copy of its package"}),
        };
        rows.push(row);
    }
    let plan = json!({"plan": true, "mutated": false, "revision": src["revision"], "differs": differs, "footprints": rows});
    if args["apply"].as_bool() == Some(true) {
        let mut out = json!({"success": false, "errorCode": "not_supported", "error": "apply is not supported in Fusion: its UPDATE asks about each part in a dialog, which the bridge never raises. The plan is below.",
            "_hint": "Update the parts in Fusion (Library Manager > update, or right-click the part > Update), or swap one footprint on a board kept as EAGLE source with fusion_swap_footprint."});
        for (k, v) in plan.as_object().unwrap() { out[k] = v.clone(); }
        return out;
    }
    let mut out = json!({"success": true});
    for (k, v) in plan.as_object().unwrap() { out[k] = v.clone(); }
    out["_hint"] = json!("A plan: each placed footprint against the same-named package in the libraries given, pad by pad. Fusion updates parts from their library itself (it asks about each one), so apply is refused.");
    out
}

/// "A;B;C" or a list.
fn list(v: &Value, sep: char) -> Vec<String> {
    match v { Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect(),
              Value::String(s) => s.split(sep).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(), _ => vec![] }
}

fn layer_setup(args: &Value) -> Result<&'static str, Value> {
    match args["layers"].as_u64().unwrap_or(2) { 2 => Ok("(1*16)"), 4 => Ok("(1+2*15+16)"), 6 => Ok("(1+2*3+14*15+16)"), 8 => Ok("(1+2*3+4*13+14*15+16)"),
        _ => Err(json!({"success": false, "errorCode": "bad_args", "error": "layers is 2, 4, 6 or 8 in Fusion"})) }
}

fn lib_name_of(path: &str) -> String {
    regex::Regex::new(r"[^A-Za-z0-9_-]").unwrap().replace_all(path.rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(s, _)| s).unwrap_or("lib"), "_").to_string()
}

fn read_lbr(args: &Value) -> Result<(String, String), Value> {
    let path = args["libraryPath"].as_str().unwrap_or("").replace('\\', "/");
    if path.is_empty() { return Err(json!({"success": false, "errorCode": "bad_args", "error": "libraryPath (.lbr on the Fusion machine) is required", "_hint": "A Fusion or EAGLE library: fusion_create_footprint and fusion_footprint_from_land_pattern write one."})); }
    match std::fs::read(&path) { Ok(b) => Ok((path, String::from_utf8_lossy(&b).to_string())), Err(e) => Err(json!({"success": false, "errorCode": "library_unreadable", "error": format!("{path}: {e}")})) }
}

/// Open the written board in Fusion (a new local design, dialog-free) and read it back.
fn open_and_read(ctx: &Ctx, path: &str, pass: &dyn Fn(&str, Value) -> Value) -> Result<(String, Value), Value> {
    let stem = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let fp = crate::fusionpath::for_fusion(&ctx.state_dir, path).unwrap_or_else(|_| path.to_string());
    let r = pass("execute_text_command", json!({"command": format!("Document.newDesignFromLocal {fp}")}));
    let mut doc = String::new();
    for _ in 0..12 {
        let a = pass("get_app_state", json!({}))["data"]["activeDocument"].as_str().unwrap_or("").to_string();
        if a.to_lowercase().contains(&stem.to_lowercase()) { doc = a; break; }
        std::thread::sleep(Duration::from_secs(3));
    }
    if doc.is_empty() { return Err(json!({"success": false, "errorCode": "open_failed", "error": format!("Fusion did not open {path}: {}", r["error"].as_str().unwrap_or("no document came up")), "pcbPath": path, "saved": true})); }
    crate::chains::note_bridge_doc_pub(&ctx.state_dir, &stem);
    let _ = pass("execute_text_command", json!({"command": "Electron.run WINDOW FIT;"}));
    let st = crate::routing::call("routing_state", &json!({"expectDocument": doc}), pass);
    if st["success"].as_bool() == Some(false) { return Err(json!({"success": false, "errorCode": "readback_failed", "error": st["error"], "documentName": doc, "pcbPath": path, "saved": true})); }
    Ok((doc, st))
}

fn write(path: &str, xml: &str) -> Result<(), Value> {
    if let Some(d) = std::path::Path::new(path).parent() { let _ = std::fs::create_dir_all(d); }
    std::fs::write(path, xml).map_err(|e| json!({"success": false, "errorCode": "not_saved", "error": format!("{path}: {e}")}))
}

/// Pins as the board reads them: "REF.PAD" -> net ("" for none).
fn board_pins(st: &Value) -> BTreeMap<String, String> {
    st["pads"].as_array().into_iter().flatten().filter_map(|p| Some((format!("{}.{}", p["ref"].as_str()?.to_uppercase(), p["name"].as_str()?), p["net"].as_str().unwrap_or("").to_string()))).collect()
}

/// `fusion_test_board`: footprints from a .lbr on one board, a new one (wMm x hMm) when pcbPath is
/// new or omitted, appended right of the current parts when pcbPath is an existing .brd.
pub fn test_board(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let (lbr_path, lbr) = match read_lbr(args) { Ok(x) => x, Err(e) => return e };
    let names = list(&args["footprintName"], ';');
    if names.is_empty() { return json!({"success": false, "errorCode": "bad_args", "error": "footprintName is required: one name or 'A;B;C'"}); }
    let pkgs = match lbr_packages(&lbr) { Ok(p) => p, Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": e}) };
    let find = |n: &str| pkgs.iter().find(|p| p.attr("name").map(|x| x.eq_ignore_ascii_case(n)).unwrap_or(false));
    let missing: Vec<&String> = names.iter().filter(|n| find(n).is_none()).collect();
    if !missing.is_empty() { return json!({"success": false, "errorCode": "footprint_not_found", "missing": missing, "available": pkgs.iter().filter_map(|p| p.attr("name")).collect::<Vec<_>>(), "error": "some footprints are not in the library"}); }
    let setup = match layer_setup(args) { Ok(s) => s, Err(e) => return e };
    let lib_name = lib_name_of(&lbr_path);
    let pcb = args["pcbPath"].as_str().filter(|s| !s.is_empty()).map(|s| s.replace('\\', "/"))
        .unwrap_or_else(|| std::env::temp_dir().join("adom-test-boards").join(format!("{}.brd", lib_name_of(&format!("{}.x", names[0])))).to_string_lossy().replace('\\', "/"));
    if !pcb.to_lowercase().ends_with(".brd") { return json!({"success": false, "errorCode": "bad_args", "error": "pcbPath is an EAGLE board (.brd)"}); }
    let base = if args["pcbPath"].is_string() { std::fs::read_to_string(&pcb).ok() } else { None };
    let appended = base.is_some();
    let prefix = args["prefix"].as_str().unwrap_or("U").to_string();
    let gap = 3.0;
    // where the row starts, and the next designator number
    let (mut cx, mut row_y, mut next, existing) = match &base {
        Some(t) => {
            let root = match Element::parse(t) { Ok(r) => r, Err(e) => return json!({"success": false, "errorCode": "board_unreadable", "error": e.to_string()}) };
            let board = deep(&root, "board");
            let libs = board.and_then(|b| b.find_child("libraries"));
            let els: Vec<&Element> = board.and_then(|b| b.find_child("elements")).map(|e| e.children.iter().filter(|c| c.tag == "element").collect()).unwrap_or_default();
            let mut right = f64::MIN; let mut y = 0.0;
            for e in &els {
                let ext = libs.and_then(|l| child_named(l, "library", e.attr("library").unwrap_or(""))).and_then(|l| l.find_child("packages")).and_then(|s| child_named(s, "package", e.attr("package").unwrap_or(""))).map(|p| turned_extent(p, rot_deg(e))).unwrap_or((0.0, 0.0, 0.0, 0.0));
                if num(e, "x") + ext.2 > right { right = num(e, "x") + ext.2; y = num(e, "y"); }
            }
            let re = regex::Regex::new(&format!(r"^{}(\d+)$", regex::escape(&prefix))).unwrap();
            let n = els.iter().filter_map(|e| re.captures(e.attr("name")?).and_then(|c| c[1].parse::<u32>().ok())).max().unwrap_or(0);
            (if right == f64::MIN { gap } else { right + gap }, y, n + 1, els.len())
        }
        None => (gap, 0.0, 1, 0),
    };
    let mut parts = Vec::new();
    let mut tallest = 0.0f64;
    for n in &names {
        let pkg = find(n).unwrap();
        let (x0, y0, x1, y1) = extent(pkg);
        tallest = tallest.max(y1 - y0);
        parts.push(Placement { r: format!("{prefix}{next}"), pkg: pkg.attr("name").unwrap_or(n).to_string(), x: cx - x0, y: 0.0, rot: 0.0, value: pkg.attr("name").unwrap_or(n).to_string() });
        parts.last_mut().unwrap().y = -(y0 + y1) / 2.0; // centred on the row, fixed below
        cx += (x1 - x0) + gap; next += 1;
    }
    // a new board: the given size, else one that fits the row
    let (bw, bh) = (args["wMm"].as_f64().unwrap_or(cx), args["hMm"].as_f64().unwrap_or(tallest + 2.0 * gap + 2.0));
    if !appended { row_y = bh / 2.0; }
    for p in parts.iter_mut() { p.y += row_y; }
    let last = parts.last().unwrap().r.clone();
    let mut nets: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for a in list(&args["assigns"], ';') { if let Some((pad, net)) = a.split_once('=') { nets.entry(net.trim().to_string()).or_default().push((last.clone(), pad.trim().to_string())); } }
    let mut stubs = Vec::new();
    for s in list(&args["stubs"], ';') {
        let Some((pad, at)) = s.split_once('=') else { continue };
        let v: Vec<f64> = at.split(',').filter_map(|t| t.trim().parse().ok()).collect();
        if v.len() == 2 { stubs.push((last.clone(), pad.trim().to_string(), v[0], v[1])); } else { return json!({"success": false, "errorCode": "bad_args", "error": format!("stubs: want pad=x,y (mm from the part's origin), got {s}")}); }
    }
    let outline = if appended { vec![] } else { match outline_wires(&json!({"rect": [0, 0, bw, bh]})) { Ok(o) => o, Err(e) => return json!({"success": false, "errorCode": "bad_args", "error": e}) } };
    let placed: Vec<String> = parts.iter().map(|p| p.r.clone()).collect();
    let plan = Plan { base: base.as_deref(), lbr: &lbr, lib_name: &lib_name, outline, layer_setup: setup, parts, nets, stubs };
    let (xml, report) = match compose(&plan) { Ok(x) => x, Err(e) => return json!({"success": false, "errorCode": "build_refused", "error": e}) };
    if let Err(e) = write(&pcb, &xml) { return e; }
    let saved = std::fs::read_to_string(&pcb).map(|t| placed.iter().all(|r| t.contains(&format!("name=\"{r}\"")))).unwrap_or(false);
    let mut out = json!({"success": true, "mutated": true, "appended": appended, "pcb": pcb, "placed": placed.join(";"), "existingParts": existing, "saved": saved,
        "unknownPins": report["unknownPins"], "conflicts": report["conflicts"], "outlineGrown": report["outlineGrown"], "outsideOutline": report["outsideOutline"], "boardMm": if appended { Value::Null } else { json!([bw, bh]) }});
    if args["open"].as_bool() == Some(false) { out["count"] = json!(existing + placed.len()); out["_hint"] = json!("Written, not opened (open:false). fusion_test_board again with this pcbPath appends."); return out; }
    match open_and_read(ctx, &pcb, pass) {
        Err(e) => { let mut e = e; e["pcb"] = json!(pcb); e }
        Ok((doc, st)) => {
            let comps = st["components"].as_array().cloned().unwrap_or_default();
            let lc = comps.iter().find(|c| c["ref"].as_str().map(|r| r.eq_ignore_ascii_case(&last)).unwrap_or(false));
            out["documentName"] = json!(doc); out["count"] = json!(comps.len()); out["revision"] = st["revision"].clone();
            out["padsOnComp"] = json!(st["pads"].as_array().map(|a| a.iter().filter(|p| p["ref"].as_str().map(|r| r.eq_ignore_ascii_case(&last)).unwrap_or(false)).count()).unwrap_or(0));
            out["bodiesOnComp"] = json!(if lc.map(|c| c["has3d"] == true).unwrap_or(false) { 1 } else { 0 });
            out["_hint"] = json!(format!("{} on the board, open in Fusion as {doc} (a local design; fusion_save_to_cloud keeps it). Run it again with this pcbPath to add parts: they land right of these, designators continue. fusion_show_3d_board for the bodies.", placed.join(", ")));
            out
        }
    }
}

/// `fusion_build_board`: a NEW board from a parts list and a netlist: outline, every footprint from
/// one .lbr placed by its origin, pad nets assigned, written, opened, then read back pin by pin.
pub fn build_board(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let (lbr_path, lbr) = match read_lbr(args) { Ok(x) => x, Err(e) => return e };
    let pcb = args["pcbPath"].as_str().unwrap_or("").replace('\\', "/");
    if !pcb.to_lowercase().ends_with(".brd") { return json!({"success": false, "errorCode": "bad_args", "error": "pcbPath (the .brd to write) is required"}); }
    let Some(list_) = args["parts"].as_array().filter(|a| !a.is_empty()) else { return json!({"success": false, "errorCode": "bad_args", "error": "parts [{ref, footprint, x, y, rotation, comment?}] is required"}) };
    let setup = match layer_setup(args) { Ok(s) => s, Err(e) => return e };
    let pkgs = match lbr_packages(&lbr) { Ok(p) => p, Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": e}) };
    let mut parts = Vec::new();
    let mut missing = Vec::new();
    for (i, p) in list_.iter().enumerate() {
        let (Some(r), Some(fp)) = (p["ref"].as_str(), p["footprint"].as_str()) else { return json!({"success": false, "errorCode": "bad_args", "error": format!("parts[{i}]: ref and footprint are required")}) };
        let Some(pkg) = pkgs.iter().find(|k| k.attr("name").map(|n| n.eq_ignore_ascii_case(fp)).unwrap_or(false)) else { missing.push(json!({"ref": r, "footprint": fp})); continue };
        parts.push(Placement { r: r.to_string(), pkg: pkg.attr("name").unwrap_or(fp).to_string(), x: p["x"].as_f64().unwrap_or(0.0), y: p["y"].as_f64().unwrap_or(0.0), rot: p["rotation"].as_f64().unwrap_or(0.0), value: p["comment"].as_str().unwrap_or(fp).to_string() });
    }
    if !missing.is_empty() { return json!({"success": false, "errorCode": "footprint_not_found", "missing": missing, "available": pkgs.iter().filter_map(|p| p.attr("name")).collect::<Vec<_>>(), "error": "a footprint is not in the library: nothing was built"}); }
    let mut seen = std::collections::HashSet::new();
    if let Some(d) = parts.iter().find(|p| !seen.insert(p.r.to_uppercase())) { return json!({"success": false, "errorCode": "bad_args", "error": format!("{} is in parts twice", d.r)}); }
    let mut nets: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (net, pins) in args["nets"].as_object().cloned().unwrap_or_default() {
        for pin in list(&pins, ',') {
            let Some((r, pad)) = pin.rsplit_once('.') else { return json!({"success": false, "errorCode": "bad_args", "error": format!("nets.{net}: pins are REF.PAD, got {pin}")}) };
            nets.entry(net.clone()).or_default().push((r.to_string(), pad.to_string()));
        }
    }
    // no outline given: the parts' box and 2 mm
    let outline_v = if args["outline"].is_object() { args["outline"].clone() } else {
        let b = parts.iter().map(|p| { let pk = pkgs.iter().find(|k| k.attr("name") == Some(p.pkg.as_str())).unwrap(); let e = turned_extent(pk, p.rot); (p.x + e.0, p.y + e.1, p.x + e.2, p.y + e.3) })
            .fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |a, e| (a.0.min(e.0), a.1.min(e.1), a.2.max(e.2), a.3.max(e.3)));
        json!({"rect": [b.0 - 2.0, b.1 - 2.0, b.2 - b.0 + 4.0, b.3 - b.1 + 4.0]})
    };
    let outline = match outline_wires(&outline_v) { Ok(o) => o, Err(e) => return json!({"success": false, "errorCode": "bad_args", "error": e}) };
    let want: Vec<(String, String)> = nets.iter().flat_map(|(n, pins)| pins.iter().map(move |(r, p)| (format!("{}.{p}", r.to_uppercase()), n.clone()))).collect();
    let refs: Vec<String> = parts.iter().map(|p| p.r.clone()).collect();
    let plan = Plan { base: None, lbr: &lbr, lib_name: &lib_name_of(&lbr_path), outline, layer_setup: setup, parts, nets, stubs: vec![] };
    let (xml, report) = match compose(&plan) { Ok(x) => x, Err(e) => return json!({"success": false, "errorCode": "build_refused", "error": e}) };
    if let Err(e) = write(&pcb, &xml) { return e; }
    let mut out = json!({"success": true, "mutated": true, "pcbPath": pcb, "outline": outline_v, "unknownPins": report["unknownPins"], "conflicts": report["conflicts"]});
    if args["open"].as_bool() == Some(false) { out["_hint"] = json!("Written, not opened (open:false): no read back."); return out; }
    let (doc, st) = match open_and_read(ctx, &pcb, pass) { Ok(x) => x, Err(e) => return e };
    let got = board_pins(&st);
    let on: std::collections::HashSet<String> = st["components"].as_array().into_iter().flatten().filter_map(|c| c["ref"].as_str().map(str::to_uppercase)).collect();
    let missing_parts: Vec<&String> = refs.iter().filter(|r| !on.contains(&r.to_uppercase())).collect();
    let mut wrong = Vec::new(); let mut wired = 0;
    for (pin, net) in &want {
        match got.get(pin) { Some(g) if g == net => wired += 1, g => wrong.push(json!({"pin": pin, "want": net, "got": g})) }
    }
    out["documentName"] = json!(doc); out["revision"] = st["revision"].clone(); out["counts"] = st["counts"].clone();
    out["placed"] = json!(refs.len() - missing_parts.len()); out["missingParts"] = json!(missing_parts); out["padsWired"] = json!(wired); out["wrongPins"] = json!(wrong);
    out["unconnected"] = st["unconnected"].clone();
    out["_hint"] = json!(if wrong.is_empty() && missing_parts.is_empty() { format!("Built and read back pin by pin: every pad on its net. {} connections to route (fusion_plan_route / fusion_route_net, or fusion_autoroute). The board is a local design: fusion_save_to_cloud keeps it.", st["unconnected"]) }
        else { "Read back differs: wrongPins lists each pad with the net asked for and the one the board has; a footprint's pad names must match the netlist's pin numbers (R1.1 = pad 1 of R1).".to_string() });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LBR: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE eagle SYSTEM "eagle.dtd">
<eagle version="9.6.2"><drawing><layers><layer number="1" name="Top" color="4" fill="1" visible="yes" active="yes"/></layers><library><packages>
<package name="R0603"><smd name="1" x="-0.8" y="0" dx="0.9" dy="0.9" layer="1"/><smd name="2" x="0.8" y="0" dx="0.9" dy="0.9" layer="1"/></package>
<package name="SOT23"><smd name="1" x="-1.1" y="0.95" dx="1" dy="0.6" layer="1"/><smd name="2" x="-1.1" y="-0.95" dx="1" dy="0.6" layer="1"/><smd name="3" x="1.1" y="0" dx="1" dy="0.6" layer="1"/></package>
</packages></library></drawing></eagle>"#;

    fn plan<'a>(base: Option<&'a str>, parts: Vec<Placement>, nets: &[(&str, &[(&str, &str)])]) -> Plan<'a> {
        Plan { base, lbr: LBR, lib_name: "demo", outline: outline_wires(&json!({"rect": [0, 0, 20, 10], "cornerRadius": 1})).unwrap(), layer_setup: "(1*16)", parts,
               nets: nets.iter().map(|(n, p)| (n.to_string(), p.iter().map(|(r, q)| (r.to_string(), q.to_string())).collect())).collect(), stubs: vec![] }
    }
    fn part(r: &str, pkg: &str, x: f64, y: f64, rot: f64) -> Placement { Placement { r: r.into(), pkg: pkg.into(), x, y, rot, value: pkg.into() } }

    #[test]
    fn a_board_from_a_netlist_carries_parts_nets_and_a_rounded_outline() {
        let (xml, rep) = compose(&plan(None, vec![part("R1", "R0603", 5.0, 5.0, 90.0), part("Q1", "sot23", 12.0, 5.0, 0.0)], &[("GND", &[("R1", "1"), ("Q1", "2")]), ("X", &[("R1", "9"), ("U9", "1")])])).unwrap();
        assert!(xml.contains(r#"<element name="R1" library="demo" package="R0603" value="R0603" x="5" y="5" rot="R90" />"#), "{xml}");
        assert!(xml.contains(r#"package="SOT23""#));
        assert!(xml.contains(r#"<signal name="GND"><contactref element="R1" pad="1" /><contactref element="Q1" pad="2" /></signal>"#), "{xml}");
        assert_eq!(rep["unknownPins"].as_array().unwrap().len(), 2);
        assert_eq!(xml.matches(r#"curve="90""#).count(), 4);
        assert_eq!(xml.matches(r#"layer="20""#).count(), 8);
        assert!(xml.contains(r#"<param name="layerSetup" value="(1*16)" />"#));
        // the footprint copies once into the board's library
        assert_eq!(xml.matches(r#"<package name="R0603">"#).count(), 1);
    }

    #[test]
    fn appending_keeps_the_board_and_refuses_a_pad_on_two_nets() {
        let (first, _) = compose(&plan(None, vec![part("U1", "R0603", 3.0, 5.0, 0.0)], &[("A", &[("U1", "1")])])).unwrap();
        let (second, rep) = compose(&plan(Some(&first), vec![part("U2", "SOT23", 9.0, 5.0, 0.0)], &[("B", &[("U1", "1"), ("U2", "3")])])).unwrap();
        assert!(second.contains(r#"<element name="U1""#) && second.contains(r#"<element name="U2""#));
        assert_eq!(second.matches(r#"curve="90""#).count(), 4, "the outline is not drawn twice");
        // a rounded outline is not a plain rectangle: it stays, and a part off it is named
        let (_, far) = compose(&plan(Some(&first), vec![part("U9", "R0603", 40.0, 5.0, 0.0)], &[])).unwrap();
        assert_eq!(far["outsideOutline"][0], "U9");
        let mut sq = plan(None, vec![part("U1", "R0603", 3.0, 5.0, 0.0)], &[]);
        sq.outline = outline_wires(&json!({"rect": [0, 0, 10, 10]})).unwrap();
        let (sqx, _) = compose(&sq).unwrap();
        let (grown, rep2) = compose(&plan(Some(&sqx), vec![part("U2", "SOT23", 12.0, 5.0, 0.0)], &[])).unwrap();
        assert_eq!(rep2["outlineGrown"]["toMm"][2].as_f64().unwrap(), 12.0 + 1.6 + 2.0, "{rep2}");
        assert_eq!(grown.matches(r#"layer="20""#).count(), 4);
        assert_eq!(rep["conflicts"][0]["already"], "A");
        assert!(second.contains(r#"<signal name="B"><contactref element="U2" pad="3" /></signal>"#));
        assert!(compose(&plan(Some(&second), vec![part("U2", "R0603", 0.0, 0.0, 0.0)], &[])).is_err());
    }

    #[test]
    fn a_stub_runs_from_the_turned_pad_on_its_net() {
        let mut p = plan(None, vec![part("U1", "R0603", 10.0, 5.0, 90.0)], &[("N", &[("U1", "2")])]);
        p.stubs = vec![("U1".into(), "2".into(), 0.0, 3.0)];
        let (xml, _) = compose(&p).unwrap();
        // pad 2 at (0.8, 0) turned 90 degrees lands at (10, 5.8)
        assert!(xml.contains(r#"<wire x1="10" y1="5.8" x2="10" y2="8" width="0.2" layer="1" />"#), "{xml}");
    }

    #[test]
    fn a_swap_keeps_the_place_and_rewires_by_pad_number() {
        let (first, _) = compose(&plan(None, vec![part("U1", "R0603", 3.0, 5.0, 90.0)], &[("A", &[("U1", "1")])])).unwrap();
        let (xml, r) = swap(&first, LBR, "demo", Some("u1"), "SOT23", &[("3".into(), "OUT".into()), ("9".into(), "X".into())]).unwrap();
        assert!(xml.contains(r#"<element name="U1" library="demo" package="SOT23" value="R0603" x="3" y="5" rot="R90" />"#), "{xml}");
        assert!(!xml.contains(r#"<contactref element="U1" pad="1" />"#), "old nets are gone");
        assert!(xml.contains(r#"<signal name="OUT"><contactref element="U1" pad="3" /></signal>"#));
        assert_eq!((r["padsWired"].as_u64(), r["unknownPads"][0]["pad"].as_str(), r["from"].as_str()), (Some(1), Some("9"), Some("R0603")));
        assert!(swap(&first, LBR, "demo", Some("Q7"), "SOT23", &[]).is_err());
    }

    #[test]
    fn a_panel_repeats_the_board_renamed_without_its_own_outline() {
        let (board, _) = compose(&plan(None, vec![part("R1", "R0603", 5.0, 5.0, 0.0)], &[("GND", &[("R1", "1")])])).unwrap();
        let board = board.replace("<elements>", "<boardshape><polygonoutlinesegments><vertex x=\"0\" y=\"1\" /></polygonoutlinesegments></boardshape><elements>");
        let p = json!({"copyShift": [[5, 5], [30, 5]], "panelMm": [55, 20], "outlineLayer": {"lines": [{"x1": 25, "y1": 0, "x2": 25, "y2": 20, "width": 0.1}], "texts": [{"text": "V-CUT", "x": 25, "y": 1, "size": 1, "rotation": 90}]},
                       "holes": [{"x": 25, "y": 10, "drill": 0.5}], "toolingHoles": [{"x": 2, "y": 2, "drill": 2.0}], "fiducials": [{"x": 2, "y": 18}]});
        let (xml, r) = compose_panel(&board, &p).unwrap();
        assert_eq!((r["copies"].as_u64(), r["parts"].as_u64(), r["holes"].as_u64()), (Some(2), Some(2), Some(2)));
        assert!(xml.contains(r#"<element name="R1" library="demo" package="R0603" value="R0603" x="10" y="10" />"#), "{xml}");
        assert!(xml.contains(r#"<element name="R1_2" library="demo" package="R0603" value="R0603" x="35" y="10" />"#));
        assert!(xml.contains(r#"<signal name="GND_2"><contactref element="R1_2" pad="1" /></signal>"#));
        // the board's rounded outline is not copied: the panel's 4 edges and one cut line are the outline layer
        assert_eq!(xml.matches(r#"curve="90""#).count(), 0);
        assert_eq!(xml.matches(r#"layer="20""#).count(), 6);
        assert!(xml.contains(r#"<text x="25" y="1" size="1" layer="20" rot="R90">V-CUT</text>"#));
        assert!(!xml.contains("boardshape"), "Fusion recomputes the panel's shape");
    }

    #[test]
    fn a_widened_pad_reads_as_a_difference() {
        let pk = lbr_packages(LBR).unwrap();
        let mut wide = pk[0].clone();
        for c in wide.children.iter_mut().filter(|c| c.attr("name") == Some("1")) { for a in c.attrs.iter_mut() { if a.0 == "dx" { a.1 = "2".into(); } } }
        let d = pad_diff(&pk[0], &wide).unwrap();
        assert_eq!(d["padsChanged"][0]["pad"], "1");
        assert!(d["padsChanged"][0]["changes"][0].as_str().unwrap().starts_with("size 0.9 x 0.9 -> 2 x 0.9"), "{d}");
        assert!(pad_diff(&pk[0], &pk[0]).is_none());
        assert_eq!(pad_diff(&pk[0], &pk[1]).unwrap()["padsAdded"], json!(["3"]));
    }

    #[test]
    fn outlines_refuse_bad_shapes() {
        assert!(outline_wires(&json!({"rect": [0, 0, 0, 5]})).is_err());
        assert!(outline_wires(&json!({"points": [[0, 0], [1, 0]]})).is_err());
        assert_eq!(outline_wires(&json!({"points": [[0, 0], [4, 0], [4, 3]]})).unwrap().len(), 3);
        assert_eq!(turned_extent(&lbr_packages(LBR).unwrap()[0], 90.0).3.round(), 1.0);
    }
}

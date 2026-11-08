//! The standard's import_part on EAGLE .lbr text, offline: a footprint spec and a symbol spec (the
//! files Altium's bridge takes, so one spec serves both) become a package, a symbol and a deviceset
//! whose device joins every pin to its pad by number, with the part's parameters as attributes.
//! Importing again replaces the footprint and the component of the same names.
//!
//! Footprint spec (mm, Y up): META|name|desc  PAD|des|x|y|w|h|shape|hole|layer|crpct
//! LINE|layer|x1|y1|x2|y2|w  ARC|layer|cx|cy|r|w|a0|a1  DISC|layer|cx|cy|r
//! Symbol spec (mils, Y up): META|libref|desc|designator  MODE|i|name  RECT|mode|x1|y1|x2|y2
//! PIN|mode|number|name|x|y|orient|len|hide|etype  LINE|mode|x1|y1|x2|y2  ARC|mode|cx|cy|r|a0|a1
//! A PIN's x,y is its body end and orient the way it runs out (left|right|up|down); EAGLE puts a
//! pin at its outer end. An EAGLE symbol has one look, so only one display mode is drawn.

use crate::authoring::{create_from_layout, el, f, find_ci, lib_mut, names, new_library, section};
use crate::Element;
use serde_json::{json, Value};

const MIL: f64 = 0.0254;

fn fields(line: &str) -> Vec<&str> { line.split('|').map(str::trim).collect() }
fn num(v: &[&str], k: usize) -> f64 { v.get(k).and_then(|t| t.parse().ok()).unwrap_or(0.0) }

/// A footprint spec as the layout create_from_layout writes: (name, description, layout).
pub fn parse_footprint_spec(text: &str) -> Result<(String, String, Value), String> {
    let (mut name, mut desc) = (String::new(), String::new());
    let (mut pads, mut lines, mut arcs, mut discs) = (vec![], vec![], vec![], vec![]);
    for (i, raw) in text.lines().enumerate() {
        let v = fields(raw);
        match v[0] {
            "META" => { name = v.get(1).unwrap_or(&"").to_string(); desc = v.get(2).unwrap_or(&"").to_string(); }
            "PAD" => {
                if v.len() < 9 { return Err(format!("footprint spec line {}: PAD|des|x|y|w|h|shape|hole|layer|crpct, got {raw}", i + 1)); }
                let crp = num(&v, 9);
                pads.push(json!({"name": v[1], "x": num(&v, 2), "y": num(&v, 3), "w": num(&v, 4), "h": num(&v, 5), "shape": v[6], "hole": num(&v, 7), "cornerRatio": crp / 100.0}));
            }
            "LINE" => lines.push(json!({"layer": v.get(1).unwrap_or(&""), "x1": num(&v, 2), "y1": num(&v, 3), "x2": num(&v, 4), "y2": num(&v, 5), "width": num(&v, 6)})),
            "ARC" => arcs.push(json!({"layer": v.get(1).unwrap_or(&""), "cx": num(&v, 2), "cy": num(&v, 3), "r": num(&v, 4), "width": num(&v, 5), "a0": num(&v, 6), "a1": num(&v, 7)})),
            "DISC" => discs.push(json!({"layer": v.get(1).unwrap_or(&""), "x": num(&v, 2), "y": num(&v, 3), "r": num(&v, 4)})),
            "" => {}
            o => return Err(format!("footprint spec line {}: unknown kind {o}", i + 1)),
        }
    }
    if name.is_empty() { return Err("footprint spec: no META|name line".into()); }
    if pads.is_empty() { return Err(format!("footprint spec {name}: no PAD lines")); }
    Ok((name, desc, json!({"pads": pads, "lines": lines, "arcs": arcs, "discs": discs, "repeatedPads": true})))
}

/// One symbol pin, in EAGLE's terms.
#[derive(Debug, Clone)]
pub struct SymPin { pub number: String, pub name: String, pub x: f64, pub y: f64, pub rot: &'static str, pub length: &'static str, pub direction: &'static str, pub hidden: bool }

/// The symbol spec's one display mode: (libref, description, designator, body graphics, pins, modes in the spec).
pub fn parse_symbol_spec(text: &str, mode: &str) -> Result<(String, String, String, Vec<Element>, Vec<SymPin>, usize), String> {
    let (mut libref, mut desc, mut des) = (String::new(), String::new(), "U".to_string());
    let mut graphics = Vec::new(); let mut pins = Vec::new(); let mut modes = 0usize;
    let wire = |x1: f64, y1: f64, x2: f64, y2: f64| el("wire", &[("x1", f(x1 * MIL)), ("y1", f(y1 * MIL)), ("x2", f(x2 * MIL)), ("y2", f(y2 * MIL)), ("width", "0.254".into()), ("layer", "94".into())]);
    for (i, raw) in text.lines().enumerate() {
        let v = fields(raw);
        let mine = v.get(1).map(|m| *m == mode).unwrap_or(false);
        match v[0] {
            "META" => { libref = v.get(1).unwrap_or(&"").to_string(); desc = v.get(2).unwrap_or(&"").to_string(); if let Some(d) = v.get(3).filter(|d| !d.is_empty()) { des = d.to_string(); } }
            "MODE" => modes += 1,
            "RECT" if mine => { let (a, b, c, d) = (num(&v, 2), num(&v, 3), num(&v, 4), num(&v, 5)); graphics.extend([wire(a, b, c, b), wire(c, b, c, d), wire(c, d, a, d), wire(a, d, a, b)]); }
            "LINE" if mine => graphics.push(wire(num(&v, 2), num(&v, 3), num(&v, 4), num(&v, 5))),
            "ARC" if mine => {
                let (cx, cy, r, a0, a1) = (num(&v, 2), num(&v, 3), num(&v, 4), num(&v, 5), num(&v, 6));
                if (a1 - a0).abs() >= 359.999 { graphics.push(el("circle", &[("x", f(cx * MIL)), ("y", f(cy * MIL)), ("radius", f(r * MIL)), ("width", "0.254".into()), ("layer", "94".into())])); }
                else {
                    let pt = |d: f64| ((cx + r * d.to_radians().cos()) * MIL, (cy + r * d.to_radians().sin()) * MIL);
                    let (p0, p1) = (pt(a0), pt(a1));
                    graphics.push(el("wire", &[("x1", f(p0.0)), ("y1", f(p0.1)), ("x2", f(p1.0)), ("y2", f(p1.1)), ("width", "0.254".into()), ("layer", "94".into()), ("curve", f(a1 - a0))]));
                }
            }
            "PIN" if mine => {
                if v.len() < 8 { return Err(format!("symbol spec line {}: PIN|mode|number|name|x|y|orient|len, got {raw}", i + 1)); }
                let len_mil = num(&v, 7);
                let (bx, by) = (num(&v, 4), num(&v, 5));
                // outward from the body end to the connection point, and EAGLE's turn for a pin there
                let (dx, dy, rot) = match v[6].to_lowercase().as_str() { "left" | "180" => (-1.0, 0.0, "R0"), "right" | "0" => (1.0, 0.0, "R180"), "up" | "90" => (0.0, 1.0, "R270"), "down" | "270" => (0.0, -1.0, "R90"),
                    o => return Err(format!("symbol spec line {}: orient {o} is left, right, up or down", i + 1)) };
                let length = if len_mil < 50.0 { "point" } else if len_mil < 150.0 { "short" } else if len_mil < 250.0 { "middle" } else { "long" };
                let el_len = match length { "point" => 0.0, "short" => 100.0, "middle" => 200.0, _ => 300.0 };
                let direction = match v.get(9).map(|t| t.to_lowercase()).unwrap_or_default().as_str() {
                    "input" => "in", "output" => "out", "bidirectional" => "io", "tri_state" => "hiz", "power_in" => "pwr", "power_out" => "sup", "open_collector" | "open_emitter" => "oc", "no_connect" => "nc", _ => "pas" };
                pins.push(SymPin { number: v[2].to_string(), name: v[3].to_string(), x: (bx + dx * el_len) * MIL, y: (by + dy * el_len) * MIL, rot, length, direction, hidden: v.get(8).map(|h| !h.is_empty()).unwrap_or(false) });
            }
            "" | "RECT" | "LINE" | "ARC" | "PIN" => {}
            o => return Err(format!("symbol spec line {}: unknown kind {o}", i + 1)),
        }
    }
    if libref.is_empty() { return Err("symbol spec: no META|libref line".into()); }
    if pins.is_empty() { return Err(format!("symbol spec {libref}: no PIN lines in mode {mode}")); }
    Ok((libref, desc, des, graphics, pins, modes.max(1)))
}

fn remove_named(sec: &mut Element, tag: &str, name: &str) -> bool {
    let before = sec.children.len();
    sec.children.retain(|c| !(c.tag == tag && c.attr("name").map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false)));
    before != sec.children.len()
}

/// The standard's import_part: footprint and/or symbol from their specs into one .lbr, the device
/// joining pins to pads, params as attributes. `footprint` names an existing package when no
/// footprint spec is given. Returns the new text and the steps.
pub fn import_part(xml: Option<&str>, fp_spec: Option<&str>, sym_spec: Option<&str>, footprint: Option<&str>, args: &Value) -> Result<(String, Value), String> {
    if fp_spec.is_none() && sym_spec.is_none() && !args["params"].is_object() { return Err("nothing to import: footprintSpec and/or symbolSpec (optionally params)".into()); }
    let mut text = xml.map(str::to_string).unwrap_or_else(|| new_library("made by fusion-bridge"));
    let mut steps = Vec::new();
    let mut fp_name = footprint.map(str::to_string);
    if let Some(spec) = fp_spec {
        let (name, desc, layout) = parse_footprint_spec(spec)?;
        // importing again replaces the footprint of that name (its devices keep pointing at it)
        let mut root = Element::parse(&text).map_err(|e| e.to_string())?;
        let replaced = remove_named(section(lib_mut(&mut root)?, "packages"), "package", &name);
        text = root.to_xml_document();
        let (t, s) = create_from_layout(Some(&text), &name, &desc, &layout)?;
        text = t;
        steps.push(json!({"build_footprint": name, "pads": s["pads"], "padAliases": s["padAliases"], "replaced": replaced}));
        fp_name = Some(name);
    }
    let mut sym_name: Option<String> = None;
    if let Some(spec) = sym_spec {
        let mode = args["mode"].as_u64().unwrap_or(0).to_string();
        let (libref, desc, des, graphics, pins, modes) = parse_symbol_spec(spec, &mode)?;
        let mut root = Element::parse(&text).map_err(|e| e.to_string())?;
        let lib = lib_mut(&mut root)?;
        // the package's pads, with repeated numbers' aliases (N, N@2 ...)
        let pads: Vec<String> = match &fp_name {
            Some(n) => {
                let sec = section(lib, "packages");
                let pk = sec.children.iter().find(|c| c.tag == "package" && c.attr("name").map(|x| x.eq_ignore_ascii_case(n)).unwrap_or(false)).ok_or(format!("footprint {n} is not in the library"))?;
                pk.children.iter().filter(|c| c.tag == "smd" || c.tag == "pad").filter_map(|c| c.attr("name").map(str::to_string)).collect()
            }
            None => vec![],
        };
        let replaced = remove_named(section(lib, "devicesets"), "deviceset", &libref);
        if replaced || find_ci(&names(section(lib, "symbols"), "symbol"), &libref).is_some() {
            let used_elsewhere = section(lib, "devicesets").children.iter().any(|d| d.find_child("gates").map(|g| g.children.iter().any(|x| x.attr("symbol").map(|s| s.eq_ignore_ascii_case(&libref)).unwrap_or(false))).unwrap_or(false));
            if used_elsewhere { return Err(format!("symbol {libref} is used by another component: import under another name")); }
            remove_named(section(lib, "symbols"), "symbol", &libref);
        }
        let mut sym = el("symbol", &[("name", libref.clone())]);
        sym.children.extend(graphics);
        let (mut x0, mut y0, mut y1) = (f64::MAX, f64::MAX, f64::MIN);
        for p in &pins { x0 = x0.min(p.x); y0 = y0.min(p.y); y1 = y1.max(p.y); }
        let mut tn = el("text", &[("x", f(x0)), ("y", f(y1 + 1.27)), ("size", "1.778".into()), ("layer", "95".into())]); tn.text = ">NAME".into();
        let mut tv = el("text", &[("x", f(x0)), ("y", f(y0 - 3.81)), ("size", "1.778".into()), ("layer", "96".into())]); tv.text = ">VALUE".into();
        sym.children.push(tn); sym.children.push(tv);
        let mut used: std::collections::HashMap<String, usize> = Default::default();
        let mut connects: Vec<(String, String)> = Vec::new();
        let mut no_pad: Vec<String> = Vec::new();
        for p in &pins {
            // EAGLE pin names are unique words: blanks take the number, repeats take @n
            let base: String = if p.name.is_empty() || p.name == "~" { p.number.clone() } else { p.name.replace(' ', "_") };
            let c = used.entry(base.to_uppercase()).or_insert(0); *c += 1;
            let pname = if *c > 1 { format!("{base}@{c}") } else { base };
            let mut pe = el("pin", &[("name", pname.clone()), ("x", f(p.x)), ("y", f(p.y)), ("length", p.length.into()), ("direction", p.direction.into())]);
            if p.rot != "R0" { pe.attrs.push(("rot".into(), p.rot.into())); }
            if p.hidden { pe.attrs.push(("visible".into(), "off".into())); }
            sym.children.push(pe);
            if fp_name.is_some() {
                let mine: Vec<&String> = pads.iter().filter(|q| *q == &p.number || q.starts_with(&format!("{}@", p.number))).collect();
                if mine.is_empty() { no_pad.push(format!("{} ({})", p.number, pname)); } else { connects.push((pname, mine.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" "))); }
            }
        }
        if !no_pad.is_empty() { return Err(format!("pins with no pad of that number on {}: {}", fp_name.clone().unwrap_or_default(), no_pad.join(", "))); }
        section(lib, "symbols").children.push(sym);
        let mut ds = el("deviceset", &[("name", libref.clone()), ("prefix", des.clone())]);
        let d = args["description"].as_str().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or(desc);
        if !d.is_empty() { let mut de = Element::new("description"); de.text = d; ds.children.push(de); }
        let mut gates = Element::new("gates"); gates.children.push(el("gate", &[("name", "G$1".into()), ("symbol", libref.clone()), ("x", "0".into()), ("y", "0".into())])); ds.children.push(gates);
        let mut devs = Element::new("devices");
        let mut dev = el("device", &[("name", "".into())]);
        if let Some(n) = &fp_name {
            dev.attrs.push(("package".into(), n.clone()));
            let mut cs = Element::new("connects");
            for (pin, pad) in &connects { cs.children.push(el("connect", &[("gate", "G$1".into()), ("pin", pin.clone()), ("pad", pad.clone())])); }
            dev.children.push(cs);
        }
        let mut techs = Element::new("technologies"); techs.children.push(el("technology", &[("name", "".into())])); dev.children.push(techs);
        devs.children.push(dev); ds.children.push(devs);
        section(lib, "devicesets").children.push(ds);
        text = root.to_xml_document();
        steps.push(json!({"build_symbol": libref, "pins": pins.len(), "mode": mode, "modesInSpec": modes, "replaced": replaced}));
        steps.push(json!({"add_models": connects.len(), "footprint": fp_name}));
        sym_name = Some(libref);
    }
    let comment = args["comment"].as_str().filter(|s| !s.is_empty());
    if let (Some(sym), true) = (&sym_name, args["params"].is_object() || comment.is_some()) {
        let (t, s) = crate::authoring::set_params(&text, Some(sym), &args["params"], comment, None)?;
        text = t;
        steps.push(json!({"params": s["paramsSet"]}));
    }
    Ok((text, json!({"footprint": fp_name, "symbol": sym_name, "steps": steps})))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP: &str = "META|SOT23-X|three pins\nPAD|1|-1.1|0.95|1|0.6|roundrect|0|top|25\nPAD|2|-1.1|-0.95|1|0.6|rect|0|top|0\nPAD|3|1.1|0|1|0.6|rect|0|top|0\nPAD|3|1.1|0.5|0.4|0.4|round|0.3|multi|0\nLINE|overlay|-0.5|1.6|0.5|1.6|0.15\nARC|fab|0|0|0.3|0.1|0|360\nDISC|overlay|-1.9|0.95|0.15\n";
    const SYM: &str = "META|LDO-X|a regulator|U\nMODE|0|std\nRECT|0|0|-300|600|0\nPIN|0|1|VIN|0|-100|left|300||power_in\nPIN|0|2|GND|0|-200|left|300||power_in\nPIN|0|3|VOUT|600|-100|right|300||power_out\nMODE|1|alt\nPIN|1|1|VIN|0|0|left|300||\n";

    #[test]
    fn a_part_from_its_specs_joins_every_pin_to_its_pads() {
        let (xml, r) = import_part(None, Some(FP), Some(SYM), None, &json!({"params": {"MPN": "LDO-X-123"}, "comment": "3V3 LDO"})).unwrap();
        assert_eq!(r["footprint"], "SOT23-X"); assert_eq!(r["symbol"], "LDO-X");
        assert_eq!(r["steps"][0]["padAliases"]["3"], json!(["3", "3@2"]));
        // a repeated pad number: both pads on pin 3
        assert!(xml.contains(r#"<connect gate="G$1" pin="VOUT" pad="3 3@2" />"#), "{xml}");
        assert!(xml.contains(r#"<connect gate="G$1" pin="VIN" pad="1" />"#));
        // body end (0, -100 mil) running left 300 mil: EAGLE's pin sits at the outer end, turned R0
        assert!(xml.contains(r#"<pin name="VIN" x="-7.62" y="-2.54" length="long" direction="pwr" />"#), "{xml}");
        assert!(xml.contains(r#"<pin name="VOUT" x="22.86" y="-2.54" length="long" direction="sup" rot="R180" />"#));
        assert!(xml.contains(r#"<pad name="3@2""#) && xml.contains(r#"<circle x="0" y="0" radius="0.3" width="0.1" layer="51" />"#));
        assert!(xml.contains(r#"name="MPN" value="LDO-X-123""#), "{xml}");
        assert_eq!(r["steps"][1]["modesInSpec"], 2);
        let p = std::env::temp_dir().join(format!("adom-import-test-{}.lbr", std::process::id()));
        std::fs::write(&p, &xml).unwrap();
        let (ok, errs) = crate::authoring::preflight_text(&p);
        let _ = std::fs::remove_file(&p);
        assert!(ok, "{errs:?}");
        // importing again replaces, not stacks
        let (again, r2) = import_part(Some(&xml), Some(FP), Some(SYM), None, &json!({})).unwrap();
        assert_eq!(again.matches(r#"<package name="SOT23-X">"#).count(), 1);
        assert_eq!(again.matches(r#"<deviceset name="LDO-X""#).count(), 1);
        assert_eq!(again.matches(r#"<symbol name="LDO-X">"#).count(), 1);
        assert_eq!((r2["steps"][0]["replaced"].as_bool(), r2["steps"][1]["replaced"].as_bool()), (Some(true), Some(true)));
    }

    #[test]
    fn a_pin_with_no_pad_and_an_empty_call_are_refused() {
        let sym = SYM.replace("PIN|0|3|VOUT", "PIN|0|9|VOUT");
        assert!(import_part(None, Some(FP), Some(&sym), None, &json!({})).unwrap_err().contains("9 (VOUT)"));
        assert!(import_part(None, None, None, None, &json!({})).is_err());
        assert!(parse_footprint_spec("PAD|1|0|0|1|1|rect|0|top|0").is_err());
    }
}

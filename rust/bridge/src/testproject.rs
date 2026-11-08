//! `fusion_make_test_project`, native: a minimal .sch/.brd pair generated from NOTHING but the
//! part's .lbr (no cloud, no sign-in), opened in Fusion through the dialog-free
//! Document.newDesignFromLocal path. The generated XML is pinned to a Python-generated golden.

use serde_json::{json, Value};
use std::time::Duration;

use crate::addin::Ctx;

fn py_float(v: f64) -> String { let s = format!("{v}"); if s.contains('.') || s.contains('e') { s } else { format!("{s}.0") } }
fn round_to(v: f64, d: i32) -> f64 { let m = 10f64.powi(d); (v * m).round() / m }

pub struct Extract { pub deviceset: String, pub symbol: String, pub package: String, pub connects: Vec<(String, String)>, pub sym_name: String, pub gate_name: String, pub pkg_name: String, pub device: String, pub prefix: String, pub layers: String }

/// `_mtp_extract`: the pieces the generator needs out of the .lbr.
pub fn mtp_extract(lbr: &str, deviceset: &str, device: Option<&str>) -> Result<Extract, String> {
    let block_of = |tag: &str, name: &str| -> Option<String> { regex::Regex::new(&format!(r#"(?s)<{tag} name="{}"[\s>].*?</{tag}>"#, regex::escape(name))).unwrap().find(lbr).map(|m| m.as_str().to_string()) };
    let Some(mut ds) = block_of("deviceset", deviceset) else { return Err(format!("deviceset {} not found in the library", fusion_lbr::py_repr_str(deviceset))) };
    let gate = regex::Regex::new(r#"<gate name="([^"]+)" symbol="([^"]+)""#).unwrap().captures(&ds).map(|c| (c[1].to_string(), c[2].to_string()));
    let prefix = regex::Regex::new(r#"<deviceset name="[^"]+"[^>]*prefix="([^"]+)""#).unwrap().captures(&ds).map(|c| c[1].to_string());
    let dev = match device {
        Some(d) => match regex::Regex::new(&format!(r#"<device name="{}" package="([^"]+)""#, regex::escape(d))).unwrap().captures(&ds) { Some(c) => Some(c[1].to_string()), None => return Err(format!("deviceset {} has no device named {}", fusion_lbr::py_repr_str(deviceset), fusion_lbr::py_repr_str(d))) },
        None => regex::Regex::new(r#"<device name="[^"]*" package="([^"]+)""#).unwrap().captures(&ds).map(|c| c[1].to_string()),
    };
    let (Some((gate_name, sym_name)), Some(pkg_name)) = (gate, dev) else { return Err(format!("deviceset {} has no gate/package device mapping", fusion_lbr::py_repr_str(deviceset))) };
    let connects: Vec<(String, String)> = regex::Regex::new(r#"<connect gate="[^"]+" pin="([^"]+)" pad="([^"]+)""#).unwrap().captures_iter(&ds).map(|c| (c[1].to_string(), c[2].to_string())).collect();
    ds = regex::Regex::new(r"(?s)<package3dinstances>.*?</package3dinstances>").unwrap().replace_all(&ds, "").to_string();
    let (sym, pkg) = (block_of("symbol", &sym_name), block_of("package", &pkg_name));
    let (Some(symbol), Some(package)) = (sym, pkg) else { return Err(format!("library is missing the symbol/package/connects for {}", fusion_lbr::py_repr_str(deviceset))) };
    if connects.is_empty() { return Err(format!("library is missing the symbol/package/connects for {}", fusion_lbr::py_repr_str(deviceset))); }
    let layers = regex::Regex::new(r"(?s)<layers>.*?</layers>").unwrap().find(lbr).map(|m| m.as_str().to_string()).unwrap_or_default();
    Ok(Extract { deviceset: ds, symbol, package, connects, sym_name, gate_name, pkg_name, device: device.unwrap_or("").to_string(), prefix: prefix.unwrap_or_else(|| "U".into()), layers })
}

const R_SYM: &str = r#"<symbol name="ADOMR"><wire x1="-2.54" y1="1.016" x2="2.54" y2="1.016" width="0.254" layer="94"/><wire x1="2.54" y1="1.016" x2="2.54" y2="-1.016" width="0.254" layer="94"/><wire x1="2.54" y1="-1.016" x2="-2.54" y2="-1.016" width="0.254" layer="94"/><wire x1="-2.54" y1="-1.016" x2="-2.54" y2="1.016" width="0.254" layer="94"/><pin name="1" x="-5.08" y="0" visible="off" length="short"/><pin name="2" x="5.08" y="0" visible="off" length="short" rot="R180"/><text x="-2.54" y="1.524" size="1.778" layer="95">&gt;NAME</text><text x="-2.54" y="-3.302" size="1.778" layer="96">&gt;VALUE</text></symbol>"#;
const R_PKG: &str = r#"<package name="ADOMR0603"><description>Resistor 0603</description><smd name="1" x="-0.85" y="0" dx="1" dy="1" layer="1"/><smd name="2" x="0.85" y="0" dx="1" dy="1" layer="1"/><wire x1="-0.4" y1="0.5" x2="0.4" y2="0.5" width="0.127" layer="21"/><wire x1="-0.4" y1="-0.5" x2="0.4" y2="-0.5" width="0.127" layer="21"/><text x="-1" y="0.8" size="0.8" layer="25">&gt;NAME</text></package>"#;
const R_DS: &str = r#"<deviceset name="ADOMR" prefix="R"><description>generated test resistor</description><gates><gate name="G$1" symbol="ADOMR" x="0" y="0"/></gates><devices><device name="" package="ADOMR0603"><connects><connect gate="G$1" pin="1" pad="1"/><connect gate="G$1" pin="2" pad="2"/></connects><technologies><technology name=""/></technologies></device></devices></deviceset>"#;
const SCH_LAYERS: &str = r#"<layers><layer number="91" name="Nets" color="2" fill="1" visible="yes" active="yes"/><layer number="92" name="Busses" color="1" fill="1" visible="yes" active="yes"/><layer number="93" name="Pins" color="2" fill="1" visible="no" active="yes"/><layer number="94" name="Symbols" color="4" fill="1" visible="yes" active="yes"/><layer number="95" name="Names" color="7" fill="1" visible="yes" active="yes"/><layer number="96" name="Values" color="7" fill="1" visible="yes" active="yes"/><layer number="97" name="Info" color="7" fill="1" visible="yes" active="yes"/><layer number="98" name="Guide" color="6" fill="1" visible="yes" active="yes"/></layers>"#;
pub(crate) const HEAD: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE eagle SYSTEM \"eagle.dtd\">\n<eagle version=\"9.6.2\">\n<drawing>\n<settings><setting alwaysvectorfont=\"no\"/></settings><grid distance=\"0.1\" unitdist=\"inch\" unit=\"inch\" style=\"lines\" multiple=\"1\" display=\"no\" altdistance=\"0.01\" altunitdist=\"inch\" altunit=\"inch\"/>";

/// `_pkg_extent_mm`: everything a package draws, on any layer.
pub fn pkg_extent_mm(p: &str) -> (f64, f64, f64, f64) {
    let (mut xs, mut ys): (Vec<f64>, Vec<f64>) = (vec![], vec![]);
    let f = |s: &str| s.parse::<f64>().unwrap_or(0.0);
    for c in regex::Regex::new(r#"<smd [^>]*x="([-0-9.]+)" y="([-0-9.]+)" dx="([-0-9.]+)" dy="([-0-9.]+)""#).unwrap().captures_iter(p) { let (x, y, dx, dy) = (f(&c[1]), f(&c[2]), f(&c[3]), f(&c[4])); let r = dx.max(dy) / 2.0; xs.extend([x - r, x + r]); ys.extend([y - r, y + r]); }
    for c in regex::Regex::new(r#"<pad [^>]*x="([-0-9.]+)" y="([-0-9.]+)"[^>]*?(?:diameter="([-0-9.]+)")?"#).unwrap().captures_iter(p) { let (x, y) = (f(&c[1]), f(&c[2])); let r = c.get(3).map(|m| f(m.as_str())).unwrap_or(1.0) / 2.0; xs.extend([x - r, x + r]); ys.extend([y - r, y + r]); }
    for c in regex::Regex::new(r#"<wire x1="([-0-9.]+)" y1="([-0-9.]+)" x2="([-0-9.]+)" y2="([-0-9.]+)" width="([-0-9.]+)""#).unwrap().captures_iter(p) { let (x1, y1, x2, y2, w) = (f(&c[1]), f(&c[2]), f(&c[3]), f(&c[4]), f(&c[5])); xs.extend([x1 - w, x1 + w, x2 - w, x2 + w]); ys.extend([y1 - w, y1 + w, y2 - w, y2 + w]); }
    for c in regex::Regex::new(r#"<circle x="([-0-9.]+)" y="([-0-9.]+)" radius="([-0-9.]+)" width="([-0-9.]+)""#).unwrap().captures_iter(p) { let (x, y, r0, w) = (f(&c[1]), f(&c[2]), f(&c[3]), f(&c[4])); let r = r0 + w; xs.extend([x - r, x + r]); ys.extend([y - r, y + r]); }
    for c in regex::Regex::new(r#"<rectangle x1="([-0-9.]+)" y1="([-0-9.]+)" x2="([-0-9.]+)" y2="([-0-9.]+)""#).unwrap().captures_iter(p) { xs.extend([f(&c[1]), f(&c[3])]); ys.extend([f(&c[2]), f(&c[4])]); }
    for c in regex::Regex::new(r#"<vertex x="([-0-9.]+)" y="([-0-9.]+)""#).unwrap().captures_iter(p) { xs.push(f(&c[1])); ys.push(f(&c[2])); }
    if xs.is_empty() || ys.is_empty() { return (-3.0, -3.0, 3.0, 3.0); }
    let mn = |v: &Vec<f64>| v.iter().cloned().fold(f64::INFINITY, f64::min); let mx = |v: &Vec<f64>| v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    (mn(&xs), mn(&ys), mx(&xs), mx(&ys))
}

pub struct Geo { pub w: f64, pub h: f64, pub px: f64, pub py: f64, pub rx: f64, pub ry: f64, pub wire: (f64, f64, f64, f64) }
pub fn board_geometry(package_xml: &str, margin: f64, with_resistor: bool) -> Geo {
    let (x0, y0, x1, y1) = pkg_extent_mm(package_xml);
    let (pw, ph) = (x1 - x0, y1 - y0);
    let bay = if with_resistor { 7.0 } else { 0.0 };
    let w = round_to(bay + pw + 2.0 * margin, 2);
    let h = round_to((ph + 2.0 * margin).max(if with_resistor { 9.0 } else { 0.0 }), 2);
    let px = round_to(bay + margin - x0, 3);
    let py = round_to(h / 2.0 - (y0 + y1) / 2.0, 3);
    let (rx, ry) = (3.5, round_to(h / 2.0, 3));
    Geo { w, h, px, py, rx, ry, wire: (rx + 0.85, ry, round_to(px + x0 - 0.3, 3), ry) }
}
fn outline(w: f64, h: f64) -> String { let (w, h) = (py_float(w), py_float(h)); format!(r#"<wire x1="0" y1="0" x2="{w}" y2="0" width="0.2" layer="20"/><wire x1="{w}" y1="0" x2="{w}" y2="{h}" width="0.2" layer="20"/><wire x1="{w}" y1="{h}" x2="0" y2="{h}" width="0.2" layer="20"/><wire x1="0" y1="{h}" x2="0" y2="0" width="0.2" layer="20"/>"#) }

fn p3d_block(part: &str, urn: &str, pkg: &str) -> String { if urn.is_empty() { String::new() } else { format!(r#"<packages3d><package3d name="{part}" urn="" wip_urn="{urn}" locally_modified="yes" type="model"><packageinstances><packageinstance name="{pkg}"/></packageinstances></package3d></packages3d>"#) } }

/// `_mtp_generate`: the .sch/.brd pair.
pub fn mtp_generate(lib_name: &str, ex: &Extract, part_name: &str, urn: &str, rex: Option<&Extract>, r_urn: &str, solo: bool) -> (String, String) {
    let pins: Vec<&str> = ex.connects.iter().map(|(p, _)| p.as_str()).collect();
    let pads: std::collections::HashMap<&str, &str> = ex.connects.iter().map(|(p, d)| (p.as_str(), d.as_str())).collect();
    let (p1, p2) = (pins[0], pins.get(1).cloned().unwrap_or(pins[0]));
    let lib = format!(r#"<library name="{lib_name}"><packages>{}</packages>{}<symbols>{}</symbols><devicesets>{}</devicesets></library>"#, ex.package, p3d_block(part_name, urn, &ex.pkg_name), ex.symbol, ex.deviceset);
    let (r_lib_name, r_ds, r_gate, r_pin1, r_pin2, r_pads, rlib, r_part, r_pkg_name): (&str, &str, String, String, String, std::collections::HashMap<String, String>, String, String, String) = match rex {
        Some(r) => { let rp: Vec<&str> = r.connects.iter().map(|(p, _)| p.as_str()).collect(); (
            "adom-resistor", "0603WAF3300T5E", r.gate_name.clone(), rp[0].to_string(), rp.get(1).cloned().unwrap_or(rp[0]).to_string(), r.connects.iter().map(|(p, d)| (p.clone(), d.clone())).collect(),
            format!(r#"<library name="adom-resistor"><packages>{}</packages>{}<symbols>{}</symbols><devicesets>{}</devicesets></library>"#, r.package, p3d_block("0603WAF3300T5E", r_urn, &r.pkg_name), r.symbol, r.deviceset),
            r#"<part name="R1" library="adom-resistor" deviceset="0603WAF3300T5E" device="" value="330"/>"#.to_string(), r.pkg_name.clone()) }
        None => ("adomdemo", "ADOMR", "G$1".to_string(), "1".to_string(), "2".to_string(), [("1", "1"), ("2", "2")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            format!(r#"<library name="adomdemo"><packages>{R_PKG}</packages><symbols>{R_SYM}</symbols><devicesets>{R_DS}</devicesets></library>"#), r#"<part name="R1" library="adomdemo" deviceset="ADOMR" device="" value="330"/>"#.to_string(), "ADOMR0603".to_string()),
    };
    let _ = r_ds;
    let g = &ex.gate_name;
    let pfx = { let p = ex.prefix.trim(); if p.is_empty() { "U" } else { p } };
    let part_ref = if pfx.to_uppercase() == "R" && !solo { format!("{pfx}2") } else { format!("{pfx}1") };
    let geo = board_geometry(&ex.package, 3.0, !solo);
    if solo {
        let sch = format!("{HEAD}{SCH_LAYERS}<schematic xreflabel=\"%F%N/%S.%C%R\" xrefpart=\"/%S.%C%R\"><libraries>{lib}</libraries><attributes/><variantdefs/><classes><class number=\"0\" name=\"default\" width=\"0\" drill=\"0\"/></classes><parts><part name=\"{part_ref}\" library=\"{lib_name}\" deviceset=\"{part_name}\" device=\"{}\"/></parts><sheets><sheet><plain><text x=\"0\" y=\"25.4\" size=\"2.54\" layer=\"97\">{part_name} - Adom component demo</text></plain><instances><instance part=\"{part_ref}\" gate=\"{}\" x=\"25.4\" y=\"12.7\"/></instances><busses/><nets/></sheet></sheets></schematic></drawing></eagle>\n", ex.device, ex.gate_name);
        let brd = format!("{HEAD}{}<board><plain>{}</plain><libraries><library name=\"{lib_name}\"><packages>{}</packages>{}</library></libraries><attributes/><variantdefs/><classes><class number=\"0\" name=\"default\" width=\"0\" drill=\"0\"/></classes><designrules name=\"default\"/><autorouter/><elements><element name=\"{part_ref}\" library=\"{lib_name}\" package=\"{}\" value=\"\" x=\"{}\" y=\"{}\"{}/></elements><signals/></board></drawing></eagle>\n", ex.layers, outline(geo.w, geo.h), ex.package, p3d_block(part_name, urn, &ex.pkg_name), ex.pkg_name, py_float(geo.px), py_float(geo.py), if urn.is_empty() { String::new() } else { format!(r#" package3d_urn="{urn}""#) });
        return (sch, brd);
    }
    let sch = format!("{HEAD}{SCH_LAYERS}<schematic xreflabel=\"%F%N/%S.%C%R\" xrefpart=\"/%S.%C%R\"><libraries>{lib}{rlib}</libraries><attributes/><variantdefs/><classes><class number=\"0\" name=\"default\" width=\"0\" drill=\"0\"/></classes><parts><part name=\"{part_ref}\" library=\"{lib_name}\" deviceset=\"{part_name}\" device=\"{}\"/>{r_part}</parts><sheets><sheet><plain><text x=\"0\" y=\"30.48\" size=\"2.54\" layer=\"97\">Adom generated test circuit: {part_name}</text></plain><instances><instance part=\"R1\" gate=\"{r_gate}\" x=\"12.7\" y=\"12.7\"/><instance part=\"{part_ref}\" gate=\"{g}\" x=\"38.1\" y=\"12.7\"/></instances><busses/><nets><net name=\"N1\" class=\"0\"><segment><wire x1=\"17.78\" y1=\"12.7\" x2=\"25.4\" y2=\"12.7\" width=\"0.1524\" layer=\"91\"/><pinref part=\"R1\" gate=\"{r_gate}\" pin=\"{r_pin2}\"/><pinref part=\"{part_ref}\" gate=\"{g}\" pin=\"{p1}\"/></segment></net><net name=\"VCC\" class=\"0\"><segment><wire x1=\"7.62\" y1=\"12.7\" x2=\"2.54\" y2=\"12.7\" width=\"0.1524\" layer=\"91\"/><pinref part=\"R1\" gate=\"{r_gate}\" pin=\"{r_pin1}\"/></segment></net><net name=\"GND\" class=\"0\"><segment><pinref part=\"{part_ref}\" gate=\"{g}\" pin=\"{p2}\"/></segment></net></nets></sheet></sheets></schematic></drawing></eagle>\n", ex.device);
    let lib_no_sym = lib.replace(&format!("<symbols>{}</symbols><devicesets>{}</devicesets>", ex.symbol, ex.deviceset), "");
    let rlib_no_sym = match rex { Some(r) => rlib.replace(&format!("<symbols>{}</symbols><devicesets>{}</devicesets>", r.symbol, r.deviceset), ""), None => rlib.replace(&format!("<symbols>{R_SYM}</symbols><devicesets>{R_DS}</devicesets>"), "") };
    let (wx1, wy1, wx2, wy2) = geo.wire;
    let brd = format!("{HEAD}{}<board><plain>{}</plain><libraries>{lib_no_sym}{rlib_no_sym}</libraries><attributes/><variantdefs/><classes><class number=\"0\" name=\"default\" width=\"0\" drill=\"0\"/></classes><designrules name=\"default\"/><autorouter/><elements><element name=\"R1\" library=\"{r_lib_name}\" package=\"{r_pkg_name}\" value=\"330\" x=\"{}\" y=\"{}\"{}/><element name=\"{part_ref}\" library=\"{lib_name}\" package=\"{}\" value=\"\" x=\"{}\" y=\"{}\"{}/></elements><signals><signal name=\"N1\"><contactref element=\"R1\" pad=\"{}\"/><contactref element=\"{part_ref}\" pad=\"{}\"/><wire x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" width=\"0.3\" layer=\"1\"/></signal><signal name=\"VCC\"><contactref element=\"R1\" pad=\"{}\"/></signal><signal name=\"GND\"><contactref element=\"{part_ref}\" pad=\"{}\"/></signal></signals></board></drawing></eagle>\n",
        ex.layers, outline(geo.w, geo.h), py_float(geo.rx), py_float(geo.ry), if rex.is_some() && !r_urn.is_empty() { format!(r#" package3d_urn="{r_urn}""#) } else { String::new() }, ex.pkg_name, py_float(geo.px), py_float(geo.py), if urn.is_empty() { String::new() } else { format!(r#" package3d_urn="{urn}""#) },
        r_pads.get(&r_pin2).cloned().unwrap_or_default(), pads.get(p1).cloned().unwrap_or(""), py_float(wx1), py_float(wy1), py_float(wx2), py_float(wy2), r_pads.get(&r_pin1).cloned().unwrap_or_default(), pads.get(p2).cloned().unwrap_or(""));
    (sch, brd)
}

fn resistor_extract() -> Option<Extract> {
    let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_default();
    let p = dir.join("resources").join("samples").join("0603WAF3300T5E.lbr");
    let txt = std::fs::read(&p).ok().map(|b| String::from_utf8_lossy(&b).to_string())?;
    mtp_extract(&txt, "0603WAF3300T5E", None).ok()
}

/// `fusion_make_test_project`.
/// `_test_project_activate`: activate the generated test project's sch or brd DOCUMENT
/// (a newDesignFromLocal re-open is the activation). True once it is confirmed active.
pub fn test_project_activate(ctx: &Ctx, kind: &str, pass: &dyn Fn(&str, Value) -> Value) -> bool {
    let Some(tp) = crate::small_verbs::get_pref(&ctx.state_dir, "testProject") else { return false };
    let Some(path) = tp[kind].as_str().filter(|p| !p.is_empty()) else { return false };
    let base = tp["docBase"].as_str().unwrap_or("").to_lowercase();
    let fp = crate::fusionpath::for_fusion(&ctx.state_dir, path).unwrap_or_else(|_| path.replace('\\', "/"));
    let _ = pass("execute_text_command", json!({"command": format!("Document.newDesignFromLocal {}", fp)}));
    for _ in 0..6 {
        std::thread::sleep(std::time::Duration::from_secs(2));
        if !base.is_empty() && pass("get_app_state", json!({}))["data"]["activeDocument"].as_str().unwrap_or("").to_lowercase().contains(&base) { return true; }
    }
    false
}
/// The generated test project on file, if any (`testProject` in prefs.json).
pub fn test_project(state_dir: &std::path::Path) -> Option<Value> { crate::small_verbs::get_pref(state_dir, "testProject").filter(|v| v["brd"].is_string()) }

pub fn make_test_project(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let lbr_path = args["libraryPath"].as_str().unwrap_or("").replace('\\', "/");
    if lbr_path.is_empty() { return json!({"success": false, "error": "libraryPath (.lbr, Windows path) required.", "_hint": "Stage the component page's .lbr onto the box first (send_files)."}); }
    let lbr_text = match std::fs::read(&lbr_path) { Ok(b) => String::from_utf8_lossy(&b).to_string(), Err(e) => return json!({"success": false, "error": format!("could not read {lbr_path}: {e}")}) };
    let deviceset = match args["deviceset"].as_str().filter(|s| !s.is_empty()) { Some(d) => d.to_string(), None => match regex::Regex::new(r#"<deviceset name="([^"]+)""#).unwrap().captures(&lbr_text) { Some(c) => c[1].to_string(), None => return json!({"success": false, "error": "no deviceset found in the library"}) } };
    crate::palette::say(&format!("Generating a test project around {deviceset}"), "a schematic and a board from EAGLE source, then Fusion imports both", None, crate::palette::eta_for_verb(&ctx.state_dir, "make_test_project"));
    let ex = match mtp_extract(&lbr_text, &deviceset, args["device"].as_str()) { Ok(e) => e, Err(e) => return json!({"success": false, "error": e}) };
    let lib_name: String = regex::Regex::new(r"[^A-Za-z0-9_-]").unwrap().replace_all(lbr_path.rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(s, _)| s).unwrap_or(""), "_").to_string();
    let mut urn = String::new();
    let pk = regex::escape(&ex.pkg_name);
    for c in regex::Regex::new(r#"(?s)<package3d [^>]*wip_urn="([^"]+)"[^>]*>(.*?)</package3d>"#).unwrap().captures_iter(&lbr_text) { if regex::Regex::new(&format!(r#"<packageinstance name="{pk}"/>"#)).unwrap().is_match(&c[2]) { urn = c[1].to_string(); break; } }
    if urn.is_empty() { if let Some(c) = regex::Regex::new(r#"wip_urn="([^"]+)""#).unwrap().captures(&lbr_text) { urn = c[1].to_string(); } }
    let solo = args["withResistor"].as_bool() != Some(true);
    let rex = if solo { None } else { resistor_extract() };
    let r_urn = if rex.is_some() { std::fs::read_to_string(ctx.state_dir.join("r3d-urn.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["0603WAF3300T5E"].as_str().map(str::to_string)).unwrap_or_default() } else { String::new() };
    let (sch, mut brd) = mtp_generate(&lib_name, &ex, &deviceset, &urn, rex.as_ref(), &r_urn, solo);
    if args["silkLabels"].as_bool() != Some(false) {
        let geo = board_geometry(&ex.package, 3.0, !solo);
        let (x0, y0, x1, _) = pkg_extent_mm(&ex.package);
        let label = format!(r#"<text x="{}" y="{}" size="1" layer="21" ratio="12" align="center">{}</text>"#, py_float(round_to(geo.px + (x0 + x1) / 2.0, 3)), py_float(round_to((geo.py + y0 - 1.6).max(0.8), 3)), deviceset);
        brd = brd.replacen("</plain>", &format!("{label}</plain>"), 1);
    }
    let d = std::env::temp_dir().join("adom-fusion-wrap"); let _ = std::fs::create_dir_all(&d);
    let base: String = format!("ADOM-TEST-{}", regex::Regex::new(r"[^A-Za-z0-9_-]").unwrap().replace_all(&deviceset, "_").chars().take(40).collect::<String>());
    let sch_path = d.join(format!("{base}.sch")).to_string_lossy().replace('\\', "/");
    let brd_path = d.join(format!("{base}.brd")).to_string_lossy().replace('\\', "/");
    let _ = std::fs::write(&sch_path, &sch); let _ = std::fs::write(&brd_path, &brd);
    let mut steps: Vec<String> = Vec::new();
    for (label, fpath) in [("board", &brd_path), ("schematic", &sch_path)] {
        let fp = crate::fusionpath::for_fusion(&ctx.state_dir, fpath).unwrap_or_else(|_| fpath.replace('\\', "/"));
        let r = pass("execute_text_command", json!({"command": format!("Document.newDesignFromLocal {}", fp)}));
        let mut okd = r["success"].as_bool() == Some(true);
        if !okd {
            let expected = fpath.rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(s, _)| s.to_lowercase()).unwrap_or_default();
            for _ in 0..10 { std::thread::sleep(Duration::from_secs(3)); if pass("get_app_state", json!({}))["data"]["activeDocument"].as_str().unwrap_or("").to_lowercase().contains(&expected) { okd = true; break; } }
        }
        steps.push(format!("{label} open: {}", if okd { "ok".to_string() } else { format!("FAILED: {}", r["error"].as_str().unwrap_or("None").chars().take(80).collect::<String>()) }));
        if !okd { return json!({"success": false, "schPath": sch_path, "brdPath": brd_path, "error": format!("{label} open failed: {}", r["error"].as_str().unwrap_or("None").chars().take(150).collect::<String>()), "data": {"steps": steps}}); }
        std::thread::sleep(Duration::from_secs(2));
    }
    crate::small_verbs::set_pref(&ctx.state_dir, "testProject", json!({"sch": sch_path, "brd": brd_path, "docBase": base.to_lowercase()}));
    crate::chains::note_bridge_doc_pub(&ctx.state_dir, &base);
    json!({"success": true, "schPath": sch_path, "brdPath": brd_path, "deviceset": deviceset, "steps": steps,
        "circuit": if solo { format!("JUST {deviceset} on a small board (solo demo layout)") } else { format!("R1 (330) in series with {deviceset}; VCC/GND stubs") },
        "_hint": "Test project opened as TWO documents (board instantiated with placed parts, schematic as its drawn sheet) via the dialog-free Document.newDesignFromLocal path. The BOARD document is active: fusion_show_2d_board / fusion_show_3d_board work now; for the schematic view activate its document (it opened last... the SCHEMATIC document is actually active) - board legs may need the board doc re-activated via Document.newDesignFromLocal or open_board. Generated from NOTHING but the component's .lbr - no cloud, no sign-in."})
}


/// `fusion_make_showcase_project`: every deviceset of a library (or the ones named) placed on a
/// fresh schematic and board, no nets, so a set of parts can be shown off (symbols on the
/// sheet, footprints on the board, the bound 3D bodies on the 3D PCB). Dialog-free: the pair is
/// generated as EAGLE source and opened through Document.newDesignFromLocal (John, 2026-09-20:
/// "create a new schematic and board to show off 3 new ToF sensors").
pub fn make_showcase_project(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let lbr_path = args["libraryPath"].as_str().unwrap_or("").replace('\\', "/");
    // Async by contract: two local opens plus fits outlive ab's relay budget, so the default
    // returns a runId at once and the work runs on this exe; poll fusion_eagle_import_status.
    if args["wait"].as_bool() != Some(true) && !lbr_path.is_empty() && std::path::Path::new(&lbr_path).is_file() {
        let run_id = format!("{:x}", ((crate::orchestrations::now_pub() * 1000.0) as u64) ^ ((std::process::id() as u64) << 20));
        crate::orchestrations::eagle_run_write_pub(&ctx.state_dir, &run_id, &json!({"stage": "started", "kind": "showcase", "startedAt": crate::orchestrations::now_pub(), "libraryPath": lbr_path}));
        let url = crate::launch::SELF_URL.lock().unwrap().clone();
        let mut a2 = args.clone(); a2["wait"] = json!(true); a2["_runId"] = json!(run_id);
        let (sd, rid) = (ctx.state_dir.clone(), run_id.clone());
        std::thread::spawn(move || {
            let r = url.and_then(|u| ureq::post(&u).timeout(Duration::from_secs(900)).send_json(json!({"command": "fusion_make_showcase_project", "args": a2})).ok()).and_then(|r| r.into_json::<Value>().ok()).unwrap_or(json!({"success": false, "error": "the exe could not post the showcase to itself"}));
            let mut v = json!({"kind": "showcase", "libraryPath": r["libraryPath"]});
            v["stage"] = json!(if r["success"].as_bool() == Some(true) { "done" } else { "failed" }); v["finishedAt"] = json!(crate::orchestrations::now_pub());
            for k in ["documentName", "schematicDocument", "boardDocument", "partsExpected", "partsPlaced", "parts", "schPath", "brdPath", "boardMm", "steps", "error", "errorCode", "_hint"] { if !r[k].is_null() { v[k] = r[k].clone(); } }
            v["result"] = r;
            crate::orchestrations::eagle_run_write_pub(&sd, &rid, &v);
        });
        return json!({"success": true, "started": true, "runId": run_id, "statusVerb": "fusion_eagle_import_status",
            "_hint": format!("Showcase generating and opening in the background (it outlives the relay window). Poll fusion_eagle_import_status {{runId:\"{run_id}\"}} until stage is done: it carries boardDocument, schematicDocument, partsPlaced and the hints. Meanwhile fusion_get_app_state keeps answering.")});
    }
    if lbr_path.is_empty() { return json!({"success": false, "errorCode": "library_path_required", "error": "libraryPath (.lbr, Windows path on the Fusion machine) is required.", "_hint": "Stage the library on the box first (send_files); a component page's .lbr or the bundle's merged library both work."}); }
    let lbr_text = match std::fs::read(&lbr_path) { Ok(b) => String::from_utf8_lossy(&b).to_string(), Err(e) => return json!({"success": false, "errorCode": "library_unreadable", "error": format!("could not read {lbr_path}: {e}")}) };
    let all: Vec<String> = regex::Regex::new(r#"<deviceset name="([^"]+)""#).unwrap().captures_iter(&lbr_text).map(|c| c[1].to_string()).collect();
    let wanted: Vec<String> = match args["devicesets"].as_array() { Some(a) if !a.is_empty() => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(), _ => all.clone() };
    if wanted.is_empty() { return json!({"success": false, "errorCode": "no_devicesets", "error": "the library has no devicesets to place."}); }
    let missing: Vec<&String> = wanted.iter().filter(|w| !all.iter().any(|a| a.eq_ignore_ascii_case(w))).collect();
    if !missing.is_empty() { return json!({"success": false, "errorCode": "deviceset_not_in_library", "missing": missing, "available": all, "error": "some requested devicesets are not in the library."}); }
    let mut extracts = Vec::new();
    for ds in &wanted { match mtp_extract(&lbr_text, ds, None) { Ok(e) => extracts.push(e), Err(e) => return json!({"success": false, "errorCode": "deviceset_unusable", "deviceset": ds, "error": e}) } }
    let lib_name: String = regex::Regex::new(r"[^A-Za-z0-9_-]").unwrap().replace_all(lbr_path.rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(s, _)| s).unwrap_or(""), "_").to_string();
    let name: String = args["name"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string()).unwrap_or_else(|| format!("{lib_name}-showcase")).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(48).collect();
    // The library goes in VERBATIM (whole <library> block, renamed), not re-assembled from
    // extracted pieces: a Fusion-exported library carries <atts>/<att_sets> and other structure
    // the piecewise rebuild lost, and Fusion then refused the whole schematic ("has errors and
    // cannot be opened") and instantiated nothing (ConfRoomROG, 2026-09-20). The board gets the
    // same block minus symbols, devicesets and attribute declarations.
    let lib_re = regex::Regex::new(r"(?s)<library(?:\s[^>]*)?>.*?</library>").unwrap();
    let Some(lib_block) = lib_re.find(&lbr_text).map(|m| m.as_str().to_string()) else { return json!({"success": false, "errorCode": "library_unreadable", "error": "no <library> block in the .lbr"}); };
    let lib_block = regex::Regex::new(r"^<library(?:\s[^>]*)?>").unwrap().replace(&lib_block, format!(r#"<library name="{lib_name}">"#).as_str()).to_string();
    let strip = |t: &str, tag: &str| regex::Regex::new(&format!(r"(?s)<{tag}>.*?</{tag}>")).unwrap().replace_all(t, "").to_string();
    let lib_block_brd = strip(&strip(&strip(&lib_block, "symbols"), "devicesets"), "atts");
    let p3ds: String = regex::Regex::new(r"(?s)<packages3d>.*?</packages3d>").unwrap().find(&lbr_text).map(|m| m.as_str().to_string()).unwrap_or_default();
    // layout: a row on the sheet (40 mm pitch) and a row on the board (each package's extent plus 6 mm)
    let mut parts = String::new(); let mut instances = String::new(); let mut elements = String::new();
    let (mut bx, margin) = (4.0f64, 4.0f64); let mut bh = 10.0f64;
    let mut sx = 20.0f64; // sheet x cursor: each symbol is pitched by its own measured width
    let mut labels = String::new();
    let mut refs: Vec<Value> = Vec::new();
    for (i, e) in extracts.iter().enumerate() {
        let pfx = { let p = e.prefix.trim(); if p.is_empty() { "U" } else { p } };
        let rname = format!("{pfx}{}", i + 1);
        parts.push_str(&format!(r#"<part name="{rname}" library="{lib_name}" deviceset="{}" device="{}"/>"#, wanted[i], e.device));
        let (sx0, _, sx1, _) = pkg_extent_mm(&e.symbol);
        let sw = (sx1 - sx0).max(10.0);
        let ix = round_to(sx - sx0, 2);
        instances.push_str(&format!(r#"<instance part="{rname}" gate="{}" x="{}" y="40"/>"#, e.gate_name, py_float(ix)));
        sx = round_to(sx + sw + 25.0, 2);
        let (x0, y0, x1, y1) = pkg_extent_mm(&e.package);
        let (pw, ph) = (x1 - x0, y1 - y0);
        let ex_ = round_to(bx - x0, 3);
        elements.push_str(&format!(r#"<element name="{rname}" library="{lib_name}" package="{}" value="{}" x="{}" y="{}"{}/>"#, e.pkg_name, wanted[i], py_float(ex_), py_float(round_to(margin + ph.max(6.0) / 2.0 - (y0 + y1) / 2.0 + 6.0, 3)), { let pk = regex::escape(&e.pkg_name); let u = regex::Regex::new(r#"(?s)<package3d [^>]*wip_urn="([^"]+)"[^>]*>(.*?)</package3d>"#).unwrap().captures_iter(&p3ds).find(|c| regex::Regex::new(&format!(r#"<packageinstance name="{pk}"/>"#)).unwrap().is_match(&c[2])).map(|c| c[1].to_string()).unwrap_or_default(); if u.is_empty() { String::new() } else { format!(r#" package3d_urn="{u}""#) } }));
        // the part name on the top silkscreen under its footprint (John's rule for every board
        // the tools make, page issue #91); silkLabels:false turns it off
        if args["silkLabels"].as_bool() != Some(false) {
            let ey = round_to(margin + ph.max(6.0) / 2.0 - (y0 + y1) / 2.0 + 6.0, 3);
            labels.push_str(&format!(r#"<text x="{}" y="{}" size="1" layer="21" ratio="12" align="center">{}</text>"#, py_float(round_to(ex_ + (x0 + x1) / 2.0, 3)), py_float(round_to(ey + y0 - 1.6, 3)), wanted[i]));
        }
        refs.push(json!({"ref": rname, "deviceset": wanted[i], "package": e.pkg_name, "symbol": e.sym_name}));
        bx = round_to(bx + pw.max(6.0) + 6.0, 2);
        bh = bh.max(ph + 2.0 * margin + 12.0);
    }
    let (bw, bh) = (round_to(bx + margin, 2), round_to(bh, 2));
    let sch = format!("{HEAD}{}<schematic xreflabel=\"%F%N/%S.%C%R\" xrefpart=\"/%S.%C%R\"><libraries>{lib_block}</libraries><attributes/><variantdefs/><classes><class number=\"0\" name=\"default\" width=\"0\" drill=\"0\"/></classes><parts>{parts}</parts><sheets><sheet><plain/><instances>{instances}</instances><busses/><nets/></sheet></sheets></schematic></drawing></eagle>\n", extracts[0].layers);
    let brd = format!("{HEAD}{}<board><plain>{}{labels}</plain><libraries>{lib_block_brd}</libraries><attributes/><variantdefs/><classes><class number=\"0\" name=\"default\" width=\"0\" drill=\"0\"/></classes><designrules name=\"default\"></designrules><autorouter></autorouter><elements>{elements}</elements><signals/></board></drawing></eagle>\n", extracts[0].layers, outline(bw, bh));
    let d = std::env::temp_dir().join("adom-fusion-wrap"); let _ = std::fs::create_dir_all(&d);
    // two documents with DISTINCT names: a local .sch and a local .brd open as separate designs,
    // so the caller must be able to activate the board one by name for the 2D/3D views
    let (sch_doc, brd_doc) = (format!("{name}-sch"), format!("{name}-brd"));
    let sch_path = d.join(format!("{sch_doc}.sch")).to_string_lossy().replace('\\', "/");
    let brd_path = d.join(format!("{brd_doc}.brd")).to_string_lossy().replace('\\', "/");
    if let Err(e) = std::fs::write(&sch_path, &sch).and_then(|_| std::fs::write(&brd_path, &brd)) { return json!({"success": false, "errorCode": "write_failed", "error": e.to_string()}); }
    crate::palette::say(&format!("Showcase: {} part(s) from {lib_name}", extracts.len()), "a schematic and a board from EAGLE source, opened dialog-free", None, crate::palette::eta_for_verb(&ctx.state_dir, "make_test_project"));
    let mut steps: Vec<String> = Vec::new();
    // schematic first, board last: the board document ends up ACTIVE, so show_2d_board /
    // show_3d_board work at once; each open is followed by WINDOW FIT so the capture shows the parts
    for (label, fpath, docn) in [("schematic", &sch_path, &sch_doc), ("board", &brd_path, &brd_doc)] {
        let fp = crate::fusionpath::for_fusion(&ctx.state_dir, fpath).unwrap_or_else(|_| fpath.replace('\\', "/"));
        let r = pass("execute_text_command", json!({"command": format!("Document.newDesignFromLocal {}", fp)}));
        let mut okd = r["success"].as_bool() == Some(true);
        for _ in 0..10 { if pass("get_app_state", json!({}))["data"]["activeDocument"].as_str().unwrap_or("").to_lowercase().contains(&docn.to_lowercase()) { okd = true; break; } std::thread::sleep(Duration::from_secs(3)); }
        steps.push(format!("{label} open: {}", if okd { "ok".to_string() } else { format!("FAILED: {}", r["error"].as_str().unwrap_or("None").chars().take(80).collect::<String>()) }));
        if !okd { return json!({"success": false, "errorCode": "open_failed", "schPath": sch_path, "brdPath": brd_path, "steps": steps, "error": format!("{label} open failed: {}", r["error"].as_str().unwrap_or("None").chars().take(150).collect::<String>())}); }
        let _ = pass("execute_text_command", json!({"command": "Electron.run WINDOW FIT;"}));
        std::thread::sleep(Duration::from_secs(1));
    }
    crate::chains::note_bridge_doc_pub(&ctx.state_dir, &sch_doc); crate::chains::note_bridge_doc_pub(&ctx.state_dir, &brd_doc);
    let placed = pass("board_info", json!({}));
    let bd = if placed["data"]["summary"].is_object() { placed["data"]["summary"].clone() } else if placed["data"].is_object() { placed["data"].clone() } else { placed.clone() };
    json!({"success": true, "documentName": brd_doc, "boardDocument": brd_doc, "schematicDocument": sch_doc, "libraryPath": lbr_path, "schPath": sch_path, "brdPath": brd_path, "parts": refs, "partsExpected": extracts.len(), "partsPlaced": bd["componentCount"], "boardMm": {"width": bw, "height": bh}, "steps": steps,
        "_hint": format!("Showcase opened as two local documents, dialog-free: {brd_doc} (the board with the placed parts, ACTIVE now) and {sch_doc} (the sheet with the symbols). NEXT: fusion_show_3d_board {{capture:true}} then fusion_view {{action:\"orbit\", degrees:45}} / {{action:\"iso\"}} to turn the board (the bound 3D bodies render where the library carries package3d urns); fusion_show_2d_board for copper; fusion_activate_document {{name:\"{sch_doc}\"}} then fusion_schematic {{action:\"show\", capture:true}} for the sheet; fusion_save_to_cloud {{name}} to keep either. No nets were drawn: this is a display, not a circuit.")})
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_project_xml_matches_python_golden() {
        let g: serde_json::Value = serde_json::from_str(include_str!("../tests/golden/mtp.json")).unwrap();
        let lbr = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../lbr/tests/golden/Demo_Board.bound5.lbrx")).unwrap();
        let rtxt = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../resources/samples/0603WAF3300T5E.lbr")).unwrap();
        let rex = super::mtp_extract(&rtxt, "0603WAF3300T5E", None).unwrap();
        for c in g["cases"].as_array().unwrap() {
            let ds = c["deviceset"].as_str().unwrap(); let solo = c["solo"].as_bool().unwrap();
            let ex = super::mtp_extract(&lbr, ds, None).unwrap();
            let (sch, brd) = super::mtp_generate("Demo_Board", &ex, ds, c["urn"].as_str().unwrap(), if solo { None } else { Some(&rex) }, c["rUrn"].as_str().unwrap(), solo);
            for (got, want, what) in [(sch, c["sch"].as_str().unwrap().to_string(), "sch"), (brd, c["brd"].as_str().unwrap().to_string(), "brd")] {
                if got != want { let i = got.chars().zip(want.chars()).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len())); panic!("{ds} solo={solo} {what} drift at {i}: got {:?} want {:?}", &got[i.saturating_sub(60)..(i + 80).min(got.len())], &want[i.saturating_sub(60)..(i + 80).min(want.len())]); }
            }
        }
    }
}


/// A board's <layers>: the sixteen copper layers, EAGLE's board layers and the schematic ones.
pub(crate) fn board_layers() -> String {
    let names = [(1, "Top"), (16, "Bottom"), (17, "Pads"), (18, "Vias"), (19, "Unrouted"), (20, "Dimension"), (21, "tPlace"), (22, "bPlace"), (23, "tOrigins"), (24, "bOrigins"), (25, "tNames"), (26, "bNames"), (27, "tValues"), (28, "bValues"), (29, "tStop"), (30, "bStop"), (31, "tCream"), (32, "bCream"), (35, "tGlue"), (36, "bGlue"), (39, "tKeepout"), (40, "bKeepout"), (41, "tRestrict"), (42, "bRestrict"), (43, "vRestrict"), (44, "Drills"), (45, "Holes"), (46, "Milling"), (47, "Measures"), (48, "Document"), (49, "Reference"), (51, "tDocu"), (52, "bDocu")];
    let mut brd_layers = String::from("<layers>");
    for n in 1..=16 { let nm = names.iter().find(|(k, _)| *k == n).map(|(_, v)| v.to_string()).unwrap_or(format!("Route{n}")); brd_layers.push_str(&format!(r#"<layer number="{n}" name="{nm}" color="{}" fill="1" visible="yes" active="yes"/>"#, if n == 1 { 4 } else if n == 16 { 1 } else { 6 })); }
    for (n, nm) in names.iter().filter(|(k, _)| *k > 16) { brd_layers.push_str(&format!(r#"<layer number="{n}" name="{nm}" color="7" fill="1" visible="yes" active="yes"/>"#)); }
    brd_layers.push_str(&SCH_LAYERS["<layers>".len()..SCH_LAYERS.len() - "</layers>".len()]);
    brd_layers.push_str("</layers>");
    brd_layers
}

/// The standard's project_create: an empty schematic and board as EAGLE source in
/// <parentDir>/<name>/ (the board's copper count in its layer setup; title and text variables as
/// the schematic's global attributes), imported into Fusion as a linked design, or opened as two unlinked
/// documents when the import can't run (linked:false). Saving it to the cloud is fusion_save_to_cloud.
pub fn project_create(ctx: &Ctx, args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let name = args["name"].as_str().unwrap_or("").trim().to_string();
    let parent = args["parentDir"].as_str().unwrap_or("").replace('\\', "/");
    if name.is_empty() || parent.is_empty() { return json!({"success": false, "errorCode": "bad_args", "error": "parentDir and name are required"}); }
    if !regex::Regex::new(r"^[A-Za-z0-9 _.\-]{1,64}$").unwrap().is_match(&name) { return json!({"success": false, "errorCode": "bad_args", "error": "name: letters, digits, space, _ . - (64 at most)"}); }
    let layers = args["layers"].as_u64().unwrap_or(2);
    let setup = match layers { 2 => "(1*16)", 4 => "(1+2*15+16)", 6 => "(1+2*3+14*15+16)", 8 => "(1+2*3+4*13+14*15+16)", _ => return json!({"success": false, "errorCode": "bad_args", "error": "layers is 2, 4, 6 or 8 in Fusion"}) };
    let dir = format!("{}/{}", parent.trim_end_matches('/'), name);
    let (sch_path, brd_path) = (format!("{dir}/{name}.sch"), format!("{dir}/{name}.brd"));
    let esc = |t: &str| t.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
    let mut attrs = String::new();
    if let Some(t) = args["title"].as_str() { attrs.push_str(&format!(r#"<attribute name="TITLE" value="{}"/>"#, esc(t))); }
    if let Some(vars) = args["textVariables"].as_object() { for (k, v) in vars { attrs.push_str(&format!(r#"<attribute name="{}" value="{}"/>"#, esc(&k.to_uppercase()), esc(v.as_str().unwrap_or(&v.to_string())))); } }
    let plan = json!({"project": dir, "files": [sch_path, brd_path], "layers": layers, "layerSetup": setup});
    if args["dryRun"].as_bool() == Some(true) { return json!({"success": true, "mutated": false, "plan": plan}); }
    if std::path::Path::new(&sch_path).exists() && args["overwrite"].as_bool() != Some(true) {
        return json!({"success": false, "errorCode": "exists", "error": format!("{sch_path} exists"), "_hint": "Pass overwrite:true, or another name."});
    }
    let brd_layers = board_layers();
    let sch = format!("{HEAD}{SCH_LAYERS}<schematic xreflabel=\"%F%N/%S.%C%R\" xrefpart=\"/%S.%C%R\"><libraries/><attributes>{attrs}</attributes><variantdefs/><classes><class number=\"0\" name=\"default\" width=\"0\" drill=\"0\"/></classes><parts/><sheets><sheet><plain/><instances/><busses/><nets/></sheet></sheets></schematic></drawing></eagle>\n");
    let brd = format!("{HEAD}{brd_layers}<board><plain/><libraries/><attributes/><variantdefs/><classes><class number=\"0\" name=\"default\" width=\"0\" drill=\"0\"/></classes><designrules name=\"default\"><param name=\"layerSetup\" value=\"{setup}\"/></designrules><autorouter/><elements/><signals/></board></drawing></eagle>\n");
    if let Err(e) = std::fs::create_dir_all(&dir) { return json!({"success": false, "errorCode": "write_failed", "error": format!("{dir}: {e}")}); }
    if let Err(e) = std::fs::write(&sch_path, &sch).and_then(|_| std::fs::write(&brd_path, &brd)) { return json!({"success": false, "errorCode": "write_failed", "error": e.to_string()}); }
    // Fusion's EAGLE import makes a design whose sheet and board are linked: a part placed on the sheet is on
    // the board (live 2026-11-03, 25 s on an empty project). Opening the two files with newDesignFromLocal
    // makes two unlinked documents, so that's only the fallback when the import can't run.
    let why_not_linked;
    if args["linked"].as_bool() != Some(false) {
        let started = crate::orchestrations::new_electronics_from_eagle(ctx, &json!({"schPath": sch_path, "brdPath": brd_path}), pass);
        if let Some(run) = started["runId"].as_str().filter(|_| started["success"].as_bool() == Some(true)) {
            let mut stage = Value::Null;
            let mut record = Value::Null;
            for _ in 0..30 {
                std::thread::sleep(Duration::from_secs(3));
                record = crate::orchestrations::eagle_import_status(ctx, &json!({"runId": run}));
                stage = record["stage"].clone();
                if stage == "done" || stage == "failed" { break; }
            }
            if stage == "done" {
                crate::chains::note_bridge_doc_pub(&ctx.state_dir, &name);
                let doc = pass("get_app_state", json!({}))["data"]["activeDocument"].clone();
                return json!({"success": true, "mutated": true, "linked": true, "documentName": doc, "project": dir, "files": [sch_path, brd_path], "layers": layers, "layerSetup": setup,
                    "fabRules": args["fab"].clone().as_str().or(args["fabRules"].as_str()).map(|f| json!({"recorded": f, "apply": "fusion_set_design_rules {fab} once the board has content"})),
                    "_hint": "An empty electronics design with its sheet and board linked: a part placed on the sheet goes on the board. Fusion names it by its own rules (documentName). Save it to the cloud with fusion_save_to_cloud {name, projectName, folderPath}; the EAGLE files stay in project as the source."});
            }
            if stage != "failed" {
                return json!({"success": true, "mutated": true, "stillRunning": true, "started": true, "runId": run, "statusVerb": "fusion_eagle_import_status", "project": dir, "files": [sch_path, brd_path], "layers": layers, "layerSetup": setup,
                    "needsApproval": started["needsApproval"], "tellUser": started["tellUser"],
                    "_hint": "Fusion's import is still running, likely waiting on Adom Bridge's approval to fill its Open dialog. Poll fusion_eagle_import_status {runId}."});
            }
            why_not_linked = json!(record["result"]["error"].as_str().unwrap_or("Fusion's EAGLE import failed"));
        } else {
            why_not_linked = json!(started["error"].as_str().unwrap_or("Fusion's EAGLE import could not start"));
        }
    } else {
        why_not_linked = json!("linked:false was asked for");
    }
    let mut steps: Vec<String> = Vec::new();
    for (label, fpath) in [("board", &brd_path), ("schematic", &sch_path)] {
        let fp = crate::fusionpath::for_fusion(&ctx.state_dir, fpath).unwrap_or_else(|_| fpath.clone());
        let r = pass("execute_text_command", json!({"command": format!("Document.newDesignFromLocal {}", fp)}));
        let mut okd = r["success"].as_bool() == Some(true);
        if !okd {
            for _ in 0..10 { std::thread::sleep(Duration::from_secs(3)); if pass("get_app_state", json!({}))["data"]["activeDocument"].as_str().unwrap_or("").to_lowercase().contains(&name.to_lowercase()) { okd = true; break; } }
        }
        steps.push(format!("{label}: {}", if okd { "opened" } else { "not opened" }));
        if !okd { return json!({"success": false, "errorCode": "open_failed", "error": format!("Fusion did not open the {label}: {}", r["error"].as_str().unwrap_or("")), "plan": plan, "steps": steps}); }
        std::thread::sleep(Duration::from_secs(2));
    }
    crate::chains::note_bridge_doc_pub(&ctx.state_dir, &name);
    json!({"success": true, "mutated": true, "linked": false, "whyNotLinked": why_not_linked, "project": dir, "files": [sch_path, brd_path], "layers": layers, "layerSetup": setup, "steps": steps,
           "fabRules": args["fab"].clone().as_str().or(args["fabRules"].as_str()).map(|f| json!({"recorded": f, "apply": "fusion_set_design_rules {fab} once the board has content"})),
           "_hint": "An empty schematic and board, open as two documents that are NOT linked (whyNotLinked): parts placed on the sheet stay off the board. Save it to the cloud with fusion_save_to_cloud {name, projectName, folderPath}; the EAGLE files stay in project as the source."})
}

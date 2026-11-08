//! A schematic read straight from its file, without Fusion: `fusion_read_sheet_file` (the parts of
//! an EAGLE .sch with their pins and each pin's electrical type, the EDA Bridge Standard's sheet
//! parts) and `fusion_netlist {schPath}` (the compiled netlist by pad: Designator.Pad, the same
//! form the board netlist uses). A Fusion .fsch is a zip with the .sch inside; both are read.
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::time::Instant;

/// The .sch XML from a path: the file itself, or the .sch inside a Fusion .fsch container.
fn sch_xml(path: &str) -> Result<String, Value> {
    let bytes = std::fs::read(path).map_err(|e| json!({"success": false, "errorCode": "file_not_found",
        "error": format!("cannot read {path}: {e}"), "_hint": "Pass path: an EAGLE .sch or a Fusion .fsch on this machine (fusion_export_eagle_source writes one from the open design)."}))?;
    if bytes.starts_with(b"PK") {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| json!({"success": false, "errorCode": "unsupported_document", "error": format!("{path} is a zip but not a readable Fusion container: {e}")}))?;
        let names: Vec<String> = (0..z.len()).filter_map(|i| z.by_index(i).ok().map(|f| f.name().to_string())).filter(|n| n.ends_with(".sch")).collect();
        let name = match names.as_slice() { [one] => one.clone(), _ => return Err(json!({"success": false, "errorCode": "unsupported_document", "error": format!("{path} holds {} schematic files, expected one", names.len())})) };
        let mut s = String::new();
        z.by_name(&name).map_err(|e| json!({"success": false, "errorCode": "unsupported_document", "error": e.to_string()}))?.read_to_string(&mut s).map_err(|e| json!({"success": false, "errorCode": "unsupported_document", "error": e.to_string()}))?;
        return Ok(s);
    }
    String::from_utf8(bytes).map_err(|_| json!({"success": false, "errorCode": "unsupported_document", "error": format!("{path} is not an EAGLE XML schematic")}))
}

/// EAGLE pin direction as the standard's electrical type (Altium's names; supply and nc have no
/// Altium code and keep EAGLE's meaning).
fn electrical(direction: &str) -> &'static str {
    match direction { "in" => "input", "out" => "output", "io" => "io", "oc" => "open_collector", "pwr" => "power",
                      "pas" => "passive", "hiz" => "hiz", "sup" => "supply", "nc" => "not_connected", _ => "io" }
}

struct Lib<'a> {
    symbols: HashMap<(String, String), roxmltree::Node<'a, 'a>>,
    devicesets: HashMap<(String, String), roxmltree::Node<'a, 'a>>,
}

fn child<'a>(n: roxmltree::Node<'a, 'a>, tag: &str) -> Option<roxmltree::Node<'a, 'a>> { n.children().find(|c| c.has_tag_name(tag)) }
fn kids<'a>(n: roxmltree::Node<'a, 'a>, tag: &'a str) -> impl Iterator<Item = roxmltree::Node<'a, 'a>> + 'a { n.children().filter(move |c| c.has_tag_name(tag)) }
fn attr(n: roxmltree::Node, k: &str) -> String { n.attribute(k).unwrap_or("").to_string() }

fn libraries<'a>(sch: roxmltree::Node<'a, 'a>) -> Lib<'a> {
    let mut lib = Lib { symbols: HashMap::new(), devicesets: HashMap::new() };
    for l in sch.descendants().filter(|n| n.has_tag_name("library")) {
        let ln = attr(l, "name");
        for s in l.descendants().filter(|n| n.has_tag_name("symbol")) { lib.symbols.insert((ln.clone(), attr(s, "name")), s); }
        for d in l.descendants().filter(|n| n.has_tag_name("deviceset")) { lib.devicesets.insert((ln.clone(), attr(d, "name")), d); }
    }
    lib
}

fn device<'a>(ds: roxmltree::Node<'a, 'a>, name: &str) -> Option<roxmltree::Node<'a, 'a>> {
    ds.descendants().find(|d| d.has_tag_name("device") && d.attribute("name").unwrap_or("") == name)
}

/// (gate, pin) -> pads, from a device's connects ("1 2 3" is a pin on three pads).
fn pin_pads(dev: Option<roxmltree::Node>) -> HashMap<(String, String), Vec<String>> {
    let mut m: HashMap<(String, String), Vec<String>> = HashMap::new();
    if let Some(d) = dev {
        for c in d.descendants().filter(|n| n.has_tag_name("connect")) {
            m.entry((attr(c, "gate"), attr(c, "pin"))).or_default().extend(attr(c, "pad").split_whitespace().map(str::to_string));
        }
    }
    m
}

const MIL: f64 = 1.0 / 0.0254;

/// Every EAGLE file opens with <!DOCTYPE eagle SYSTEM "eagle.dtd">, which roxmltree refuses unless asked.
fn parse(xml: &str) -> Result<roxmltree::Document<'_>, roxmltree::Error> {
    roxmltree::Document::parse_with_options(xml, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() })
}

/// `fusion_read_sheet_file {path, pins?, params?}`
pub fn read_sheet_file(args: &Value) -> Value {
    let t0 = Instant::now();
    let path = match args["path"].as_str().or(args["filePath"].as_str()).or(args["schPath"].as_str()) {
        Some(p) => p.to_string(),
        None => return json!({"success": false, "errorCode": "bad_args", "error": "path (an EAGLE .sch or Fusion .fsch) is required",
                               "_hint": "fusion_export_eagle_source {outputPath:'C:/.../design.sch'} with the schematic in front writes one from the open design."}),
    };
    let xml = match sch_xml(&path) { Ok(x) => x, Err(e) => return e };
    let doc = match parse(&xml) { Ok(d) => d, Err(e) => return json!({"success": false, "errorCode": "unsupported_document", "error": format!("{path}: not well-formed XML ({e})")}) };
    let Some(sch) = doc.descendants().find(|n| n.has_tag_name("schematic")) else {
        return json!({"success": false, "errorCode": "not_a_schematic", "error": format!("{path} has no <schematic>: is it a board (.brd) or a library (.lbr)?")});
    };
    let lib = libraries(sch);
    let want_pins = args["pins"].as_bool().unwrap_or(true);
    let want_params = args["params"].as_bool().unwrap_or(false);
    // where each part's gates sit: (sheet number, x, y, rot) of its first instance
    let mut placed: HashMap<String, (usize, f64, f64, String, usize)> = HashMap::new();
    let sheets: Vec<roxmltree::Node> = child(sch, "sheets").map(|s| kids(s, "sheet").collect()).unwrap_or_default();
    for (i, s) in sheets.iter().enumerate() {
        for inst in s.descendants().filter(|n| n.has_tag_name("instance")) {
            let e = placed.entry(attr(inst, "part")).or_insert((i + 1, inst.attribute("x").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                inst.attribute("y").and_then(|v| v.parse().ok()).unwrap_or(0.0), inst.attribute("rot").unwrap_or("R0").to_string(), 0));
            e.4 += 1;
        }
    }
    let mut parts = Vec::new();
    let mut unknown = Vec::new();
    for p in child(sch, "parts").map(|ps| kids(ps, "part").collect::<Vec<_>>()).unwrap_or_default() {
        let (name, ln, dsn, devn) = (attr(p, "name"), attr(p, "library"), attr(p, "deviceset"), attr(p, "device"));
        let ds = lib.devicesets.get(&(ln.clone(), dsn.clone())).copied();
        let mut row = Map::new();
        row.insert("designator".into(), json!(name));
        row.insert("libReference".into(), json!(format!("{dsn}{devn}")));
        row.insert("deviceset".into(), json!(dsn));
        row.insert("device".into(), json!(devn));
        row.insert("comment".into(), json!(attr(p, "value")));
        row.insert("description".into(), json!(ds.and_then(|d| child(d, "description")).and_then(|d| d.text()).unwrap_or("").trim().chars().take(240).collect::<String>()));
        row.insert("uniqueId".into(), json!(name));
        row.insert("sourceLibrary".into(), json!(ln));
        let gates: Vec<roxmltree::Node> = ds.and_then(|d| child(d, "gates")).map(|g| kids(g, "gate").collect()).unwrap_or_default();
        row.insert("partCount".into(), json!(gates.len()));
        if let Some((sheet, x, y, rot, n)) = placed.get(&name) {
            row.insert("sheet".into(), json!(sheet));
            row.insert("xMil".into(), json!((x * MIL).round()));
            row.insert("yMil".into(), json!((y * MIL).round()));
            row.insert("xMm".into(), json!(x));
            row.insert("yMm".into(), json!(y));
            row.insert("orientation".into(), json!(rot));
            row.insert("gatesPlaced".into(), json!(n));
        }
        let mpn = kids(p, "attribute").find(|a| a.attribute("name").map(|n| n.eq_ignore_ascii_case("MPN")).unwrap_or(false)).map(|a| attr(a, "value"));
        if let Some(m) = mpn { row.insert("partNumber".into(), json!(m)); }
        if want_params {
            let mut params = Map::new();
            for a in kids(p, "attribute") { params.insert(attr(a, "name"), json!(attr(a, "value"))); }
            row.insert("params".into(), Value::Object(params));
        }
        if ds.is_none() { unknown.push(name.clone()); }
        if want_pins {
            let pads = pin_pads(ds.and_then(|d| device(d, &devn)));
            let mut pins = Vec::new();
            for g in &gates {
                let Some(sym) = lib.symbols.get(&(ln.clone(), attr(*g, "symbol"))) else { continue };
                for pin in sym.descendants().filter(|n| n.has_tag_name("pin")) {
                    let dir = pin.attribute("direction").unwrap_or("io");
                    let pn = attr(pin, "name");
                    let numbers = pads.get(&(attr(*g, "name"), pn.clone())).cloned().unwrap_or_default();
                    // a pin on several pads is listed once per pad; a pin on none (a symbol-only pin) has number ""
                    for num in if numbers.is_empty() { vec![String::new()] } else { numbers } {
                        pins.push(json!({"number": num, "name": pn, "gate": attr(*g, "name"), "electrical": electrical(dir), "direction": dir}));
                    }
                }
            }
            row.insert("pins".into(), Value::Array(pins));
        }
        parts.push(Value::Object(row));
    }
    let modules = sch.descendants().filter(|n| n.has_tag_name("module")).count();
    let mut out = json!({"success": true, "path": path, "components": parts, "count": parts.len(), "sheets": sheets.len(),
        "elapsedMs": t0.elapsed().as_millis() as u64, "format": doc.root_element().attribute("version").map(|v| format!("EAGLE {v} XML")).unwrap_or_else(|| "EAGLE XML".into()),
        "complete": modules == 0 && unknown.is_empty(),
        "_hint": "The saved file, not the live sheet: unsaved edits in an open Fusion are not in it. Pins carry electrical (input, output, io, open_collector, passive, hiz, power, supply, not_connected) for a schematic review; nets come from fusion_netlist {schPath}."});
    if modules > 0 { out["notes"] = json!([format!("{modules} hierarchical module(s): their inner parts are not listed yet (module instances read as their top-level parts only)")]); }
    if !unknown.is_empty() { out["unknownDevicesets"] = json!(unknown); }
    out
}

/// `fusion_netlist {schPath, net?, expect?}`: the compiled netlist from a schematic file, by pad.
pub fn netlist_from_sch(args: &Value) -> Value {
    let t0 = Instant::now();
    let path = args["schPath"].as_str().unwrap_or("").to_string();
    let xml = match sch_xml(&path) { Ok(x) => x, Err(e) => return e };
    let doc = match parse(&xml) { Ok(d) => d, Err(e) => return json!({"success": false, "errorCode": "unsupported_document", "error": format!("{path}: {e}")}) };
    let Some(sch) = doc.descendants().find(|n| n.has_tag_name("schematic")) else {
        return json!({"success": false, "errorCode": "not_a_schematic", "error": format!("{path} has no <schematic>")});
    };
    let lib = libraries(sch);
    let mut part_pads: HashMap<String, HashMap<(String, String), Vec<String>>> = HashMap::new();
    for p in child(sch, "parts").map(|ps| kids(ps, "part").collect::<Vec<_>>()).unwrap_or_default() {
        let ds = lib.devicesets.get(&(attr(p, "library"), attr(p, "deviceset"))).copied();
        part_pads.insert(attr(p, "name"), pin_pads(ds.and_then(|d| device(d, &attr(p, "device")))));
    }
    let mut nets: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut padless = BTreeSet::new();
    for n in sch.descendants().filter(|n| n.has_tag_name("net")) {
        let e = nets.entry(attr(n, "name")).or_default();
        for r in n.descendants().filter(|x| x.has_tag_name("pinref")) {
            let (part, gate, pin) = (attr(r, "part"), attr(r, "gate"), attr(r, "pin"));
            match part_pads.get(&part).and_then(|m| m.get(&(gate.clone(), pin.clone()))) {
                Some(pads) if !pads.is_empty() => { for pad in pads { e.insert(format!("{part}.{pad}")); } }
                _ => { padless.insert(format!("{part}.{gate}.{pin}")); }
            }
        }
    }
    nets.retain(|_, v| !v.is_empty());
    finish(nets, args, json!({"source": format!("{path} (the schematic file, by pad)"), "pinsWithoutPad": padless, "elapsedMs": t0.elapsed().as_millis() as u64}))
}

/// The netlist reply with `net` (one net) and `expect` (compare pin by pin) applied: shared by
/// the schematic-file read and the live board read.
pub fn finish(nets: BTreeMap<String, BTreeSet<String>>, args: &Value, extra: Value) -> Value {
    let mut mismatches = Vec::new();
    if let Some(exp) = args["expect"].as_object() {
        let key = |s: &str| s.to_ascii_lowercase();
        let have: HashMap<String, (&String, &BTreeSet<String>)> = nets.iter().map(|(k, v)| (key(k), (k, v))).collect();
        for (n, pins) in exp {
            let want: BTreeSet<String> = pins.as_array().into_iter().flatten().filter_map(|p| p.as_str().map(str::to_string)).collect();
            match have.get(&key(n)) {
                None => mismatches.push(json!({"net": n, "missing": want, "extra": []})),
                Some((_, got)) => {
                    let missing: Vec<&String> = want.difference(got).collect();
                    let extra: Vec<&String> = got.difference(&want).collect();
                    if !missing.is_empty() || !extra.is_empty() { mismatches.push(json!({"net": n, "missing": missing, "extra": extra})); }
                }
            }
        }
        let expected: BTreeSet<String> = exp.keys().map(|k| key(k)).collect();
        for k in nets.keys() { if !expected.contains(&key(k)) { mismatches.push(json!({"net": k, "unexpected": nets[k]})); } }
    }
    let shown: BTreeMap<&String, &BTreeSet<String>> = match args["net"].as_str() {
        Some(one) => nets.iter().filter(|(k, _)| k.eq_ignore_ascii_case(one)).collect(),
        None => nets.iter().collect(),
    };
    if let Some(one) = args["net"].as_str() {
        if shown.is_empty() { return json!({"success": false, "errorCode": "no_such_net", "error": format!("no net named {one}"), "netCount": nets.len()}); }
    }
    let mut out = json!({"success": true, "mutated": false, "nets": shown, "netCount": nets.len()});
    if args["expect"].is_object() { out["mismatches"] = json!(mismatches); out["ok"] = json!(mismatches.is_empty()); }
    if let Value::Object(m) = extra { for (k, v) in m { out[k] = v; } }
    out["_hint"] = json!(if args["expect"].is_object() { "mismatches lists each net whose pads differ from what you expected (missing, extra) and nets you did not expect." } else { "Every net with its pads as Designator.Pad. Pass expect {net:[\"U1.3\",...]} to compare with the circuit you meant." });
    out
}

/// The board netlist reply (from the routing script) put through the same net / expect handling.
pub fn shape_board_netlist(r: Value, args: &Value) -> Value {
    if r["success"].as_bool() != Some(true) || !(args["net"].is_string() || args["expect"].is_object()) { return r; }
    let nets: BTreeMap<String, BTreeSet<String>> = r["nets"].as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_array().into_iter().flatten().filter_map(|p| p.as_str().map(str::to_string)).collect())).collect()).unwrap_or_default();
    let mut extra = r.clone();
    for k in ["nets", "netCount", "_hint", "success"] { extra.as_object_mut().map(|m| m.remove(k)); }
    finish(nets, args, extra)
}

#[cfg(test)]
mod tests {
    use super::*;
    const SCH: &str = r#"<?xml version="1.0"?>
<!DOCTYPE eagle SYSTEM "eagle.dtd">
<eagle version="9.6.2"><drawing><schematic><libraries><library name="L">
<symbols><symbol name="OPAMP"><pin name="+IN" x="0" y="0" direction="in"/><pin name="OUT" x="5" y="0" direction="out"/><pin name="V+" x="0" y="5" direction="pwr"/></symbol>
<symbol name="R"><pin name="1" x="0" y="0" direction="pas"/><pin name="2" x="5" y="0" direction="pas"/></symbol></symbols>
<devicesets><deviceset name="LM321"><description>Single op amp</description><gates><gate name="G$1" symbol="OPAMP" x="0" y="0"/></gates><devices><device name="-SOT" package="SOT23-5"><connects>
<connect gate="G$1" pin="+IN" pad="3"/><connect gate="G$1" pin="OUT" pad="1"/><connect gate="G$1" pin="V+" pad="5 6"/></connects></device></devices></deviceset>
<deviceset name="R"><gates><gate name="G$1" symbol="R" x="0" y="0"/></gates><devices><device name="0402" package="0402"><connects><connect gate="G$1" pin="1" pad="1"/><connect gate="G$1" pin="2" pad="2"/></connects></device></devices></deviceset>
</devicesets></library></libraries><parts><part name="U1" library="L" deviceset="LM321" device="-SOT" value="LM321"><attribute name="MPN" value="LM321MF"/></part>
<part name="R1" library="L" deviceset="R" device="0402" value="10k"/></parts>
<sheets><sheet><instances><instance part="U1" gate="G$1" x="25.4" y="50.8"/><instance part="R1" gate="G$1" x="0" y="0" rot="R90"/></instances>
<nets><net name="OUT"><segment><pinref part="U1" gate="G$1" pin="OUT"/><pinref part="R1" gate="G$1" pin="1"/></segment></net>
<net name="VCC"><segment><pinref part="U1" gate="G$1" pin="V+"/></segment></net></nets></sheet></sheets></schematic></drawing></eagle>"#;

    fn tmp(body: &str) -> String {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("sheet-test-{}-{n}.sch", std::process::id()));
        std::fs::write(&p, body).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn parts_with_pins_and_electrical_types() {
        let r = read_sheet_file(&json!({"path": tmp(SCH), "params": true}));
        assert_eq!(r["success"], true, "{r}");
        assert_eq!(r["count"], 2);
        let u1 = &r["components"][0];
        assert_eq!(u1["designator"], "U1");
        assert_eq!(u1["libReference"], "LM321-SOT");
        assert_eq!(u1["comment"], "LM321");
        assert_eq!(u1["partNumber"], "LM321MF");
        assert_eq!(u1["xMil"], 1000.0);
        let pins = u1["pins"].as_array().unwrap();
        assert!(pins.contains(&json!({"number": "3", "name": "+IN", "gate": "G$1", "electrical": "input", "direction": "in"})));
        assert!(pins.contains(&json!({"number": "1", "name": "OUT", "gate": "G$1", "electrical": "output", "direction": "out"})));
        // a power pin on two pads is listed for each
        assert_eq!(pins.iter().filter(|p| p["name"] == "V+").count(), 2);
        assert_eq!(r["components"][1]["orientation"], "R90");
        assert_eq!(r["complete"], true);
    }

    #[test]
    fn netlist_by_pad_with_expect() {
        let p = tmp(SCH);
        let r = netlist_from_sch(&json!({"schPath": p}));
        assert_eq!(r["nets"]["OUT"], json!(["R1.1", "U1.1"]));
        assert_eq!(r["nets"]["VCC"], json!(["U1.5", "U1.6"]));
        let r = netlist_from_sch(&json!({"schPath": p, "expect": {"out": ["U1.1", "R1.2"], "VCC": ["U1.5", "U1.6"]}}));
        assert_eq!(r["ok"], false);
        assert_eq!(r["mismatches"], json!([{"net": "out", "missing": ["R1.2"], "extra": ["R1.1"]}]));
        let r = netlist_from_sch(&json!({"schPath": p, "net": "vcc"}));
        assert_eq!(r["nets"], json!({"VCC": ["U1.5", "U1.6"]}));
        assert_eq!(netlist_from_sch(&json!({"schPath": p, "net": "NOPE"}))["errorCode"], "no_such_net");
    }

    #[test]
    fn refuses_what_is_not_a_schematic() {
        assert_eq!(read_sheet_file(&json!({}))["errorCode"], "bad_args");
        assert_eq!(read_sheet_file(&json!({"path": "/no/such.sch"}))["errorCode"], "file_not_found");
        assert_eq!(read_sheet_file(&json!({"path": tmp("<eagle><drawing><board/></drawing></eagle>")}))["errorCode"], "not_a_schematic");
    }
}


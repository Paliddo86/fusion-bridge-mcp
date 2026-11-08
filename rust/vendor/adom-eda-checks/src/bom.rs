//! The bill of materials, from the sheets' parts and the board: fitted parts grouped one line per manufacturer part
//! number, checked for what an order needs, and the order files as text (generic, JLCPCB BOM + CPL, Mouser and DigiKey
//! uploads). Ported from the Altium bridge's bom.rs so every bridge answers `bom` the same way; the bridge reads the
//! parts and writes the files. Also `cross_check`: the sheets' designators against the board's.
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Parameter names that mean the same thing across libraries and tools (Altium templates, the Adom part import, the
/// wiki's Cloud Library, Altium 365 supplier links, KiCad fields, EAGLE attributes).
const MPN: &[&str] = &["MPN", "Manufacturer Part Number", "Manufacturer Part Number 1", "Mfr Part #", "Mfr. Part Number", "MFR PN", "MFR_PN", "MANUFACTURER_PART_NUMBER", "MANUFACTURER_PN", "Part Number", "PartNumber", "Part_Number"];
const MFR: &[&str] = &["Manufacturer", "Manufacturer 1", "Manufacturer Name", "Mfr", "MANUFACTURER", "MANUFACTURER_NAME"];
const MOUSER: &[&str] = &["Mouser Part #", "Mouser Part Number", "Mouser PN", "MOUSER_PN", "Mouser"];
const DIGIKEY: &[&str] = &["DigiKey Part #", "Digi-Key Part Number", "DigiKey Part Number", "Digikey PN", "DIGIKEY_PN", "DigiKey", "Digi-Key_PN"];
const LCSC: &[&str] = &["LCSC Part #", "LCSC", "LCSC Part", "LCSC_PN", "JLCPCB Part #", "JLC Part #", "JLCPCB"];
const VALUE: &[&str] = &["Value", "VALUE"];
const PACKAGE: &[&str] = &["Package", "Case/Package", "PACKAGE"];
const DNP: &[&str] = &["DNP", "Do Not Populate", "DNF", "Do Not Fit"];
const DESCRIPTION: &[&str] = &["Description", "DESCRIPTION"];
const FIELDS: &[(&str, &[&str])] = &[("mpn", MPN), ("manufacturer", MFR), ("mouser", MOUSER), ("digikey", DIGIKEY), ("lcsc", LCSC)];

fn st(v: &Value) -> String { match v { Value::String(s) => s.clone(), Value::Null => String::new(), o => o.to_string() } }

/// R1 < R2 < R10 < R10A: the letters, then the number, then the rest.
pub fn natural_key(d: &str) -> (String, u64, String) {
    let pre: String = d.chars().take_while(|c| !c.is_ascii_digit()).collect();
    let rest = &d[pre.len()..];
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    (pre.to_ascii_uppercase(), num.parse().unwrap_or(0), rest[num.len()..].to_string())
}

fn truthy(v: &str) -> bool { matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "y" | "dnp" | "x") }

/// One part, merged from its sheet (parameters) and the board (footprint, placement).
#[derive(Debug, Clone, Default)]
pub struct Part {
    pub designator: String,
    pub comment: String,
    pub description: String,
    pub lib_ref: String,
    /// out of the BOM by its kind (Altium's graphical, net tie and standard-no-BOM kinds; KiCad's exclude from BOM)
    pub no_bom: bool,
    pub params: Vec<(String, String)>,
    pub footprint: String,
    pub placed: Option<(f64, f64, f64, String)>,
    pub dnp: bool,
    pub alternate: String,
    pub managed: bool,
    pub source_lib: String,
}

impl Part {
    fn get(&self, names: &[&str]) -> String {
        for n in names {
            if let Some((_, v)) = self.params.iter().find(|(k, v)| k.eq_ignore_ascii_case(n) && !v.trim().is_empty() && !v.trim().starts_with('=')) { return v.trim().to_string(); }
        }
        String::new()
    }
    fn mpn(&self) -> String { self.get(MPN) }
    fn value(&self) -> String { let v = self.get(VALUE); if v.is_empty() { self.comment.clone() } else { v } }
    /// Copper test pads, fiducials and the like carry an MPN or maker that says nothing is bought ("none (PCB copper)"):
    /// they stay in the BOM, marked, and out of the ordering and assembly files.
    fn not_purchased(&self) -> bool {
        [self.mpn(), self.get(MFR)].iter().any(|v| {
            let m = v.trim().to_ascii_lowercase();
            m.starts_with("none") || m == "n/a" || m == "na" || m == "-" || m.contains("pcb copper") || m.contains("not purchased") || m.contains("no part")
        })
    }
}

/// The parts from the sheetParts companion (units of one part merged by designator), with the board's footprint and
/// placement. Returns (parts, designators on the board but on no sheet).
pub fn parts(sheet: &[Value], board: Option<&Value>) -> (Vec<Part>, Vec<String>) {
    let mut by: BTreeMap<String, Part> = BTreeMap::new();
    for c in sheet {
        let d = st(&c["designator"]);
        if d.is_empty() || d.ends_with('?') { continue; }
        // params: [{name, value}] or {name: value}
        let params: Vec<(String, String)> = match &c["params"] {
            Value::Array(a) => a.iter().filter(|p| p.get("value").is_some()).map(|p| (st(&p["name"]), st(&p["value"]))).collect(),
            Value::Object(o) => o.iter().map(|(k, v)| (k.clone(), st(v))).collect(),
            _ => Vec::new(),
        };
        let kind = c["kind"].as_i64().unwrap_or(0);
        let no_bom = matches!(kind, 2 | 4 | 5) || c["inBom"] == false || c["excludeFromBom"] == true;
        let e = by.entry(d.to_ascii_uppercase()).or_insert_with(|| Part {
            designator: d.clone(), comment: [&c["comment"], &c["value"]].iter().map(|v| st(v)).find(|s| !s.is_empty()).unwrap_or_default(),
            description: st(&c["description"]), lib_ref: [&c["libRef"], &c["libReference"], &c["libId"]].iter().map(|v| st(v)).find(|s| !s.is_empty()).unwrap_or_default(),
            no_bom, managed: !st(&c["itemGuid"]).is_empty(), source_lib: st(&c["sourceLibrary"]), dnp: c["dnp"] == true, ..Default::default() });
        // a multi-unit part appears once per unit: keep the first, add what it lacks
        for (k, v) in params { if !e.params.iter().any(|(k2, _)| k2.eq_ignore_ascii_case(&k)) { e.params.push((k, v)); } }
        if c["dnp"] == true { e.dnp = true; }
        if e.footprint.is_empty() { e.footprint = [&c["footprint"], &c["pattern"]].iter().map(|v| st(v)).find(|s| !s.is_empty()).unwrap_or_default(); }
    }
    let mut board_only = Vec::new();
    if let Some(b) = board {
        for c in b["components"].as_array().cloned().unwrap_or_default() {
            let d = st(&c["ref"]);
            if d.is_empty() { continue; }
            let placed = Some((c["x"].as_f64().unwrap_or(0.0), c["y"].as_f64().unwrap_or(0.0), c["rotation"].as_f64().unwrap_or(0.0), if c["side"] == "bottom" { "bottom".to_string() } else { "top".to_string() }));
            match by.get_mut(&d.to_ascii_uppercase()) {
                Some(p) => { let pat = st(&c["pattern"]); if !pat.is_empty() { p.footprint = pat; } p.placed = placed; }
                None => board_only.push(d),
            }
        }
    }
    for p in by.values_mut() {
        if truthy(&p.get(DNP)) || p.get(&["Fitted"]).eq_ignore_ascii_case("false") { p.dnp = true; }
    }
    board_only.sort_by_key(|d| natural_key(d));
    (by.into_values().collect(), board_only)
}

/// Group the fitted parts: one line per manufacturer part number, else per value + footprint. Returns (lines, checks).
pub fn group(parts: &[Part]) -> (Vec<Value>, Value) {
    let mut groups: BTreeMap<String, Vec<&Part>> = BTreeMap::new();
    for p in parts.iter().filter(|p| !p.dnp && !p.no_bom) {
        let m = p.mpn();
        let key = if m.is_empty() { format!("~{}|{}", p.value().to_ascii_uppercase(), p.footprint.to_ascii_uppercase()) } else { format!("{}|{}", m.to_ascii_uppercase(), p.get(MFR).to_ascii_uppercase()) };
        groups.entry(key).or_default().push(p);
    }
    let mut lines = Vec::new();
    let mut inconsistent = Vec::new();
    for (_, mut g) in groups {
        g.sort_by_key(|p| natural_key(&p.designator));
        let refs: Vec<String> = g.iter().map(|p| p.designator.clone()).collect();
        let mut line = Map::new();
        line.insert("quantity".into(), json!(g.len()));
        line.insert("designators".into(), json!(refs));
        for (name, names) in FIELDS {
            let mut distinct: Vec<String> = g.iter().map(|p| p.get(names)).filter(|v| !v.is_empty()).collect();
            distinct.sort(); distinct.dedup();
            if distinct.len() > 1 { inconsistent.push(json!({"field": name, "designators": refs, "values": distinct})); }
            line.insert((*name).into(), json!(distinct.first().cloned().unwrap_or_default()));
        }
        let first = g[0];
        let mut fps: Vec<&str> = g.iter().map(|p| p.footprint.as_str()).filter(|f| !f.is_empty()).collect();
        fps.sort(); fps.dedup();
        if fps.len() > 1 { inconsistent.push(json!({"field": "footprint", "designators": refs, "values": fps})); }
        line.insert("value".into(), json!(first.value()));
        line.insert("comment".into(), json!(first.comment));
        line.insert("description".into(), json!(if first.description.is_empty() { first.get(DESCRIPTION) } else { first.description.clone() }));
        line.insert("sourceLibrary".into(), json!(first.source_lib));
        line.insert("managed".into(), json!(first.managed));
        line.insert("footprint".into(), json!(fps.first().copied().unwrap_or("")));
        line.insert("package".into(), json!(first.get(PACKAGE)));
        line.insert("libReference".into(), json!(first.lib_ref));
        line.insert("purchase".into(), json!(!first.not_purchased()));
        lines.push(Value::Object(line));
    }
    lines.sort_by_key(|l| natural_key(l["designators"][0].as_str().unwrap_or("")));
    let sorted = |mut v: Vec<String>| { v.sort_by_key(|d| natural_key(d)); v };
    let fitted: Vec<&Part> = parts.iter().filter(|p| !p.dnp && !p.no_bom && !p.not_purchased()).collect();
    let missing = |names: &[&str]| sorted(fitted.iter().filter(|p| p.get(names).is_empty()).map(|p| p.designator.clone()).collect());
    let no_lcsc: Vec<Value> = lines.iter().filter(|l| l["purchase"] == json!(true) && st(&l["lcsc"]).is_empty())
        .map(|l| json!({"designators": l["designators"], "mpn": l["mpn"], "value": l["value"]})).collect();
    let checks = json!({
        "missingMpn": missing(MPN), "missingManufacturer": missing(MFR),
        "missingPartNumbers": {"mouser": missing(MOUSER), "digikey": missing(DIGIKEY), "lcsc": missing(LCSC)},
        "inconsistent": inconsistent,
        "dnp": sorted(parts.iter().filter(|p| p.dnp).map(|p| p.designator.clone()).collect()),
        "excludedKinds": sorted(parts.iter().filter(|p| !p.dnp && p.no_bom).map(|p| p.designator.clone()).collect()),
        "notPurchased": sorted(parts.iter().filter(|p| !p.dnp && !p.no_bom && p.not_purchased()).map(|p| p.designator.clone()).collect()),
        "jlcpcb": {"ready": no_lcsc.is_empty(), "linesWithoutLcsc": no_lcsc},
    });
    (lines, checks)
}

fn csv(rows: &[Vec<String>]) -> String {
    let cell = |s: &str| if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
    rows.iter().map(|r| r.iter().map(|c| cell(c)).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join("\r\n") + "\r\n"
}

/// The order files as (name, text). `boards` multiplies the quantities of the distributor uploads.
pub fn files(lines: &[Value], parts: &[Part], boards: u64) -> Vec<(String, String)> {
    let refs = |l: &Value| l["designators"].as_array().map(|a| a.iter().map(st).collect::<Vec<_>>().join(",")).unwrap_or_default();
    let mut out = Vec::new();
    let mut rows = vec![["Line", "Quantity", "Designators", "Value", "Comment", "Description", "Footprint", "Package", "MPN", "Manufacturer", "Mouser Part #", "DigiKey Part #", "LCSC Part #"].map(String::from).to_vec()];
    for (i, l) in lines.iter().enumerate() {
        rows.push(vec![(i + 1).to_string(), st(&l["quantity"]), refs(l), st(&l["value"]), st(&l["comment"]), st(&l["description"]), st(&l["footprint"]), st(&l["package"]), st(&l["mpn"]), st(&l["manufacturer"]), st(&l["mouser"]), st(&l["digikey"]), st(&l["lcsc"])]);
    }
    out.push(("bom.csv".to_string(), csv(&rows)));
    // JLCPCB assembly: their BOM and CPL columns, mm, Top/Bottom
    let mut j = vec![["Comment", "Designator", "Footprint", "LCSC Part #"].map(String::from).to_vec()];
    for l in lines.iter().filter(|l| l["purchase"] != json!(false)) { j.push(vec![st(&l["value"]), refs(l), st(&l["footprint"]), st(&l["lcsc"])]); }
    out.push(("bom-jlcpcb.csv".to_string(), csv(&j)));
    let mut c = vec![["Designator", "Mid X", "Mid Y", "Layer", "Rotation"].map(String::from).to_vec()];
    let mut placed: Vec<&Part> = parts.iter().filter(|p| !p.dnp && !p.no_bom && !p.not_purchased() && p.placed.is_some()).collect();
    placed.sort_by_key(|p| natural_key(&p.designator));
    for p in placed {
        let (x, y, r, side) = p.placed.clone().unwrap();
        c.push(vec![p.designator.clone(), format!("{x:.4}mm"), format!("{y:.4}mm"), if side == "bottom" { "Bottom".into() } else { "Top".into() }, format!("{}", (r.rem_euclid(360.0) * 1000.0).round() / 1000.0)]);
    }
    out.push(("cpl-jlcpcb.csv".to_string(), csv(&c)));
    // the distributors' BOM tools (Mouser BOM Tool, DigiKey myLists) map these headers directly
    for (name, field, header) in [("bom-mouser.csv", "mouser", "Mouser Part Number"), ("bom-digikey.csv", "digikey", "Digi-Key Part Number")] {
        let mut m = vec![vec![header.to_string(), "Manufacturer Part Number".into(), "Manufacturer".into(), "Quantity".into(), "Customer Reference".into()]];
        for l in lines.iter().filter(|l| l["purchase"] != json!(false) && (!st(&l[field]).is_empty() || !st(&l["mpn"]).is_empty())) {
            m.push(vec![st(&l[field]), st(&l["mpn"]), st(&l["manufacturer"]), (l["quantity"].as_u64().unwrap_or(0) * boards.max(1)).to_string(), refs(l)]);
        }
        out.push((name.to_string(), csv(&m)));
    }
    out
}

fn sheet_list(input: &Value) -> Option<Vec<Value>> {
    let p = crate::input::part(input, "sheetParts")?;
    p.get("components").or(Some(&p)).and_then(|x| x.as_array()).cloned()
}

/// bom: input {sheetParts, board?, args: {variant? (the standard `variants` row: {name, notFitted: [ref], alternates:
/// [{designator, part | fields}]}), boards? (1)}}. Parts carry designator, comment or value, description, libRef,
/// footprint?, kind? (Altium) or inBom/excludeFromBom (KiCad), dnp?, params ([{name, value}] or {name: value}).
pub fn bom(input: &Value) -> Value {
    let Some(sheet) = sheet_list(input) else { return crate::input::refuse("bad_args", "pass the sheets' parts (every sheet, with their parameters) as sheetParts") };
    let board = crate::input::part(input, "board");
    let (mut list, board_only) = parts(&sheet, board.as_ref());
    let args = &input["args"];
    let variant = &args["variant"];
    let mut alternates = Vec::new();
    if variant.is_object() {
        let nf: BTreeSet<String> = variant["notFitted"].as_array().map(|a| a.iter().map(|x| st(x).to_ascii_uppercase()).collect()).unwrap_or_default();
        let alts: BTreeMap<String, String> = match &variant["alternates"] {
            Value::Array(a) => a.iter().map(|x| (st(&x["designator"]).to_ascii_uppercase(), if x["part"].is_null() { st(&x["fields"]) } else { st(&x["part"]) })).collect(),
            Value::Object(o) => o.iter().map(|(k, v)| (k.to_ascii_uppercase(), st(v))).collect(),
            _ => BTreeMap::new(),
        };
        for p in list.iter_mut() {
            let k = p.designator.to_ascii_uppercase();
            if nf.contains(&k) { p.dnp = true; }
            if let Some(a) = alts.get(&k) { p.alternate = a.clone(); alternates.push(json!({"designator": p.designator, "alternatePart": a})); }
        }
    }
    let (lines, mut checks) = group(&list);
    let mut not_on_board: Vec<String> = if board.is_some() { list.iter().filter(|p| p.placed.is_none() && !p.no_bom).map(|p| p.designator.clone()).collect() } else { Vec::new() };
    not_on_board.sort_by_key(|d| natural_key(d));
    checks["sheetNotOnBoard"] = json!(not_on_board);
    checks["boardNotOnSheet"] = json!(board_only);
    let boards = args["boards"].as_u64().unwrap_or(1);
    let fitted = list.iter().filter(|p| !p.dnp && !p.no_bom).count();
    let files: Vec<Value> = files(&lines, &list, boards).into_iter().map(|(n, t)| json!({"name": n, "text": t})).collect();
    json!({"success": true, "variant": variant["name"], "lines": lines, "lineCount": lines.len(), "fittedParts": fitted, "checks": checks, "alternates": alternates, "boards": boards, "files": files,
        "_hint": "One line per manufacturer part number. Write files[] (bom.csv, bom-jlcpcb.csv + cpl-jlcpcb.csv for JLCPCB assembly, bom-mouser.csv and bom-digikey.csv for those sites' BOM tools). checks: parts with no MPN or distributor number (find them with adom-parts-search and write them back with sheet_params), lines that mix values for one part number (checks.inconsistent), JLCPCB lines with no LCSC number, and parts on the sheets but not the board or the other way round."})
}

/// cross_check: input {sheetParts, board}. The sheets' designators against the board's.
pub fn cross_check(input: &Value) -> Value {
    let Some(sheet) = sheet_list(input) else { return crate::input::refuse("bad_args", "pass the sheets' parts as sheetParts") };
    let st_ = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let (list, board_only) = parts(&sheet, Some(&st_));
    let mut sheet_only: Vec<String> = list.iter().filter(|p| p.placed.is_none() && !p.no_bom).map(|p| p.designator.clone()).collect();
    sheet_only.sort_by_key(|d| natural_key(d));
    let matched = list.iter().filter(|p| p.placed.is_some()).count();
    json!({"success": true, "schematicComponents": list.len(), "boardComponents": matched + board_only.len(), "matched": matched,
        "onSheetNotOnBoard": sheet_only, "onBoardNotOnSheet": board_only,
        "_hint": "onSheetNotOnBoard: update the board from the schematic (update_pcb). onBoardNotOnSheet: parts placed by hand or left after a schematic change; remove them (remove_parts) or add them to a sheet. Parts kept out of the BOM by kind are not counted as missing from the board."})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bom_groups_by_part_number_and_checks_the_order() {
        let p = |d: &str, v: &str, params: Value| json!({"designator": d, "comment": v, "libRef": "R", "params": params});
        let sheet = vec![
            p("R10", "10k", json!({"MPN": "RC0603FR-0710KL", "Manufacturer": "Yageo", "LCSC": "C98220"})),
            p("R2", "10k", json!({"MPN": "RC0603FR-0710KL", "Manufacturer": "Yageo", "LCSC": "C98221"})),
            p("R1", "10k", json!({"MPN": "RC0603FR-0710KL", "Manufacturer": "Yageo"})),
            p("C1", "100n", json!({"Value": "100nF"})),
            p("TP1", "TP", json!({"MPN": "none (PCB copper)"})),
            p("U1", "MCU", json!([{"name": "MPN", "value": "RP2040"}, {"name": "DNP", "value": "yes"}])),
            json!({"designator": "R?", "comment": "x"}),
            json!({"designator": "H1", "comment": "hole", "excludeFromBom": true}),
        ];
        let board = json!({"components": [{"ref": "R1", "pattern": "R_0603", "x": 1.0, "y": 2.0, "rotation": 450.0, "side": "top"}, {"ref": "R2", "pattern": "R_0603", "x": 3.0, "y": 2.0, "rotation": 0.0, "side": "bottom"}, {"ref": "J9", "pattern": "X", "x": 0.0, "y": 0.0}]});
        let r = bom(&json!({"sheetParts": sheet, "board": board, "args": {"boards": 5, "variant": {"name": "lite", "notFitted": ["C1"]}}}));
        // R1, R2, R10 on one line in natural order; TP1 its own line, not purchased; U1 and C1 not fitted; H1 excluded
        assert_eq!(r["lines"][0]["designators"], json!(["R1", "R2", "R10"]), "{r}");
        assert_eq!(r["lines"][0]["quantity"], 3);
        assert_eq!(r["lineCount"], 2);
        assert_eq!(r["checks"]["inconsistent"][0]["field"], "lcsc");
        assert_eq!(r["checks"]["dnp"], json!(["C1", "U1"]));
        assert_eq!(r["checks"]["excludedKinds"], json!(["H1"]));
        assert_eq!(r["checks"]["notPurchased"], json!(["TP1"]));
        assert_eq!(r["checks"]["boardNotOnSheet"], json!(["J9"]));
        assert_eq!(r["checks"]["sheetNotOnBoard"], json!(["C1", "R10", "TP1", "U1"]));
        let file = |n: &str| r["files"].as_array().unwrap().iter().find(|f| f["name"] == n).unwrap()["text"].as_str().unwrap().to_string();
        assert!(file("cpl-jlcpcb.csv").contains("R1,1.0000mm,2.0000mm,Top,90\r\n") && file("cpl-jlcpcb.csv").contains("R2,3.0000mm,2.0000mm,Bottom,0\r\n"), "{}", file("cpl-jlcpcb.csv"));
        assert!(file("bom-mouser.csv").contains(",RC0603FR-0710KL,Yageo,15,\"R1,R2,R10\""), "{}", file("bom-mouser.csv"));
        assert!(!file("bom-jlcpcb.csv").contains("TP1"));
        let x = cross_check(&json!({"sheetParts": [p("R1", "", json!({})), p("R2", "", json!({}))], "board": {"components": [{"ref": "R1"}, {"ref": "J9"}], "pads": []}}));
        assert_eq!((x["matched"].clone(), x["onSheetNotOnBoard"].clone(), x["onBoardNotOnSheet"].clone()), (json!(1), json!(["R2"]), json!(["J9"])), "{x}");
    }
}

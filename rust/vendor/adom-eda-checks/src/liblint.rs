//! Library lint (EDA Bridge Standard #28): is a symbol or footprint library fit to ship? The content rules every bridge
//! can share, from the footprints and symbols it already reads; a bridge adds its tool's own (KiCad's
//! `tool_refuses_file`) to the same findings. Severity belongs to the rule, not the bridge. A rule is only evaluated
//! when the bridge sent what it needs: a key left out means "not read" and emits nothing, a key sent empty is a finding.
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The shared vocabulary: (kind, rule, severity).
pub const RULES: &[(&str, &str, &str)] = &[
    ("footprint", "no_pads", "error"),
    ("footprint", "zero_size_pad", "error"),
    ("footprint", "unnumbered_pad", "warning"),
    ("footprint", "duplicate_pad_number", "warning"),
    ("footprint", "no_courtyard", "warning"),
    ("footprint", "origin_off_part", "warning"),
    ("footprint", "no_silkscreen", "info"),
    ("footprint", "no_3d_body", "info"),
    ("footprint", "no_description", "info"),
    ("symbol", "no_pins", "error"),
    ("symbol", "footprint_not_found", "error"),
    ("symbol", "pin_without_pad", "error"),
    ("symbol", "unnumbered_pin", "warning"),
    ("symbol", "duplicate_pin_number", "warning"),
    ("symbol", "no_footprint", "warning"),
    ("symbol", "pad_without_pin", "warning"),
    ("symbol", "no_part_number", "info"),
    ("library", "tool_refuses_file", "error"),
];

/// How far past the pads and courtyard the origin may sit before it counts as off the part (mm).
pub const ORIGIN_MARGIN_MM: f64 = 1.0;

fn severity(rule: &str) -> &'static str { RULES.iter().find(|r| r.1 == rule).map(|r| r.2).unwrap_or("warning") }
fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn text(v: &Value) -> String { match v { Value::String(s) => s.trim().to_string(), Value::Number(n) => n.to_string(), _ => String::new() } }
/// Sent but empty: null, "", [] or {}.
fn empty(v: &Value) -> bool { match v { Value::Null => true, Value::String(s) => s.trim().is_empty(), Value::Array(a) => a.is_empty(), Value::Object(o) => o.is_empty(), _ => false } }
/// A pad no pin connects to: a mechanical hole, or unplated.
fn mechanical(p: &Value) -> bool { p["mechanical"] == true || p["plated"] == false }
/// "Lib:Name" and "Name" name the same footprint.
fn base(name: &str) -> &str { name.rsplit(':').next().unwrap_or(name) }
/// "pin 3 has" or "pins 2, 3 have".
fn list(word: &str, items: &[&String]) -> String {
    let names = items.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
    if items.len() == 1 { format!("{word} {names} has") } else { format!("{word}s {names} have") }
}

/// A courtyard's extent: [x0, y0, x1, y1], {x0, y0, x1, y1}, or a list of [x, y] points.
fn extent(c: &Value) -> Option<(f64, f64, f64, f64)> {
    if let Some(a) = c.as_array() {
        if a.len() == 4 && a.iter().all(|v| v.is_number()) { return Some((f(&a[0]).min(f(&a[2])), f(&a[1]).min(f(&a[3])), f(&a[0]).max(f(&a[2])), f(&a[1]).max(f(&a[3])))); }
        let pts: Vec<(f64, f64)> = a.iter().filter_map(|p| p.as_array().filter(|q| q.len() >= 2).map(|q| (f(&q[0]), f(&q[1])))).collect();
        if pts.is_empty() { return None; }
        return Some(pts.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, p| (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1))));
    }
    if c["x0"].is_number() { return Some((f(&c["x0"]).min(f(&c["x1"])), f(&c["y0"]).min(f(&c["y1"])), f(&c["x0"]).max(f(&c["x1"])), f(&c["y0"]).max(f(&c["y1"])))); }
    None
}

/// A pad's copper box (the hole's, if larger), in the footprint's frame.
fn pad_box(p: &Value) -> (f64, f64, f64, f64) {
    let (x, y) = (f(&p["x"]), f(&p["y"]));
    let (w, h) = (f(&p["w"]).max(f(&p["hole"])) / 2.0, f(&p["h"]).max(f(&p["hole"])) / 2.0);
    (x - w, y - h, x + w, y + h)
}

/// How many separate lands the boxes make: boxes that touch or overlap (within 0.01 mm) join.
fn lands(boxes: &[(f64, f64, f64, f64)]) -> usize {
    let mut group: Vec<usize> = (0..boxes.len()).collect();
    fn root(g: &mut Vec<usize>, i: usize) -> usize { let mut i = i; while g[i] != i { g[i] = g[g[i]]; i = g[i]; } i }
    let touch = |a: &(f64, f64, f64, f64), b: &(f64, f64, f64, f64)| a.0 <= b.2 + 0.01 && b.0 <= a.2 + 0.01 && a.1 <= b.3 + 0.01 && b.1 <= a.3 + 0.01;
    for i in 0..boxes.len() { for j in i + 1..boxes.len() { if touch(&boxes[i], &boxes[j]) { let (a, b) = (root(&mut group, i), root(&mut group, j)); group[a] = b; } } }
    (0..boxes.len()).filter(|&i| root(&mut group, i) == i).count()
}

fn finding(name: &str, kind: &str, rule: &str, detail: String) -> Value {
    json!({"name": name, "kind": kind, "rule": rule, "severity": severity(rule), "detail": detail})
}

/// The findings for one footprint (pads in its own frame, mm, origin at 0,0).
pub fn lint_footprint(fp: &Value) -> Vec<Value> {
    let name = text(&fp["name"]);
    let mut out = Vec::new();
    let pads = fp["pads"].as_array().cloned().unwrap_or_default();
    if pads.is_empty() { out.push(finding(&name, "footprint", "no_pads", "the footprint has no pads".into())); }
    let mut seen: BTreeMap<String, Vec<(f64, f64, f64, f64)>> = BTreeMap::new();
    for p in &pads {
        let (w, h, hole) = (f(&p["w"]), f(&p["h"]), f(&p["hole"]));
        if (w <= 0.0 || h <= 0.0) && hole <= 0.0 { out.push(finding(&name, "footprint", "zero_size_pad", format!("pad {} is {w} x {h} mm", text(&p["number"])))); }
        let n = text(&p["number"]);
        if n.is_empty() { if !mechanical(p) { out.push(finding(&name, "footprint", "unnumbered_pad", format!("a pad at ({}, {}) has no number", f(&p["x"]), f(&p["y"])))); } }
        else if !mechanical(p) { seen.entry(n).or_default().push(pad_box(p)); }
    }
    // pads sharing a number that touch or overlap are one land (a split exposed pad, thermal vias in it): a duplicate
    // is a number on lands apart from each other
    for (n, boxes) in &seen {
        let lands = lands(boxes);
        if lands > 1 { out.push(finding(&name, "footprint", "duplicate_pad_number", format!("pad number {n} is on {lands} separate lands ({} pads)", boxes.len()))); }
    }
    let court = fp.get("courtyard");
    if court.map(empty).unwrap_or(false) { out.push(finding(&name, "footprint", "no_courtyard", "no courtyard".into())); }
    // the origin should sit on the part: inside the pads and courtyard, grown by the margin
    let mut b: Option<(f64, f64, f64, f64)> = None;
    let mut grow = |e: (f64, f64, f64, f64)| b = Some(b.map_or(e, |o| (o.0.min(e.0), o.1.min(e.1), o.2.max(e.2), o.3.max(e.3))));
    for p in &pads { let (x, y, w, h) = (f(&p["x"]), f(&p["y"]), f(&p["w"]).max(f(&p["hole"])) / 2.0, f(&p["h"]).max(f(&p["hole"])) / 2.0); grow((x - w, y - h, x + w, y + h)); }
    if let Some(e) = court.and_then(extent) { grow(e); }
    if let Some((x0, y0, x1, y1)) = b {
        let m = ORIGIN_MARGIN_MM;
        if 0.0 < x0 - m || 0.0 > x1 + m || 0.0 < y0 - m || 0.0 > y1 + m {
            let (dx, dy) = ((x0 - m).max(0.0) + (0.0f64).max(-(x1 + m)), (y0 - m).max(0.0) + (0.0f64).max(-(y1 + m)));
            out.push(finding(&name, "footprint", "origin_off_part", format!("the origin is {:.3} mm outside the pads and courtyard (part spans x {x0:.3}..{x1:.3}, y {y0:.3}..{y1:.3})", (dx * dx + dy * dy).sqrt())));
        }
    }
    if fp.get("silk").map(empty).unwrap_or(false) { out.push(finding(&name, "footprint", "no_silkscreen", "no silkscreen".into())); }
    if fp.get("bodies").map(empty).unwrap_or(false) { out.push(finding(&name, "footprint", "no_3d_body", "no 3D body".into())); }
    if fp.get("description").map(empty).unwrap_or(false) { out.push(finding(&name, "footprint", "no_description", "no description".into())); }
    out
}

/// The findings for one symbol, against the footprints the bridge sent (None: footprints not read, so the
/// footprint rules that need them are not evaluated).
pub fn lint_symbol(sym: &Value, footprints: Option<&BTreeMap<String, Value>>) -> Vec<Value> {
    let name = text(&sym["name"]);
    let mut out = Vec::new();
    let pins = sym["pins"].as_array().cloned().unwrap_or_default();
    if pins.is_empty() { out.push(finding(&name, "symbol", "no_pins", "the symbol has no pins".into())); }
    let mut numbers: BTreeMap<String, usize> = BTreeMap::new();
    for p in &pins {
        let n = text(&p["number"]);
        if n.is_empty() { out.push(finding(&name, "symbol", "unnumbered_pin", format!("pin {} has no number", text(&p["name"])))); }
        else { *numbers.entry(n).or_default() += 1; }
    }
    for (n, k) in &numbers { if *k > 1 { out.push(finding(&name, "symbol", "duplicate_pin_number", format!("{k} pins are numbered {n}"))); } }
    let fps: Vec<String> = match sym.get("footprint") {
        Some(Value::Array(a)) => a.iter().map(text).filter(|s| !s.is_empty()).collect(),
        Some(v) => Some(text(v)).filter(|s| !s.is_empty()).into_iter().collect(),
        None => Vec::new(),
    };
    if sym.get("footprint").map(empty).unwrap_or(false) { out.push(finding(&name, "symbol", "no_footprint", "no footprint is linked".into())); }
    if let Some(lib) = footprints {
        for fpn in &fps {
            let Some(fp) = lib.get(fpn).or_else(|| lib.get(base(fpn))) else {
                out.push(finding(&name, "symbol", "footprint_not_found", format!("footprint {fpn} is not in the footprint libraries checked")));
                continue;
            };
            let pads: BTreeSet<String> = fp["pads"].as_array().map(|a| a.iter().filter(|p| !mechanical(p)).map(|p| text(&p["number"])).filter(|n| !n.is_empty()).collect()).unwrap_or_default();
            let missing: Vec<&String> = numbers.keys().filter(|n| !pads.contains(*n)).collect();
            if !missing.is_empty() { out.push(finding(&name, "symbol", "pin_without_pad", format!("{} no pad on {fpn}", list("pin", &missing)))); }
            let spare: Vec<&String> = pads.iter().filter(|n| !numbers.contains_key(*n)).collect();
            if !spare.is_empty() { out.push(finding(&name, "symbol", "pad_without_pin", format!("{} no pin on {fpn}", list("pad", &spare)))); }
        }
    }
    if sym.get("partNumber").map(empty).unwrap_or(false) { out.push(finding(&name, "symbol", "no_part_number", "no manufacturer part number".into())); }
    out
}

/// lint_library: input {library: {footprints?: [{name, pads: [{number, x, y, w, h, hole?, mechanical?, plated?}],
/// courtyard?, silk?, bodies?, description?}], symbols?: [{name, pins: [{number, name?}], footprint?, partNumber?}],
/// unread?: [{name, error}], findings?: [the bridge's own]}, args: {names?}}. Symbols are checked against the footprints
/// sent (the library's own, or those of footprintLibraries the bridge resolved).
pub fn lint_library(input: &Value) -> Value {
    let lib = &input["library"];
    if !lib.is_object() { return crate::input::refuse("bad_args", "library is required: {footprints?, symbols?, unread?, findings?}"); }
    let names: Option<BTreeSet<String>> = input["args"]["names"].as_array().map(|a| a.iter().map(text).collect());
    let wanted = |v: &Value| names.as_ref().map(|n| n.contains(&text(&v["name"]))).unwrap_or(true);
    let fps = lib["footprints"].as_array().cloned().unwrap_or_default();
    let syms = lib["symbols"].as_array().cloned().unwrap_or_default();
    let by_name: Option<BTreeMap<String, Value>> = lib.get("footprints").filter(|v| v.is_array()).map(|_| fps.iter().map(|fp| (text(&fp["name"]), fp.clone())).flat_map(|(n, fp)| [(base(&n).to_string(), fp.clone()), (n, fp)]).collect());
    let mut findings: Vec<Value> = lib["findings"].as_array().cloned().unwrap_or_default();
    let mut checked = 0;
    for fp in fps.iter().filter(|v| wanted(v)) { checked += 1; findings.extend(lint_footprint(fp)); }
    for s in syms.iter().filter(|v| wanted(v)) { checked += 1; findings.extend(lint_symbol(s, by_name.as_ref())); }
    let count = |sev: &str| findings.iter().filter(|x| x["severity"] == sev).count();
    let (errors, warnings, infos) = (count("error"), count("warning"), count("info"));
    let unread = lib["unread"].as_array().cloned().unwrap_or_default();
    json!({"success": true, "ok": errors == 0 && unread.is_empty(), "checked": checked, "errors": errors, "warnings": warnings, "infos": infos,
        "findings": findings, "unread": unread,
        "rules": RULES.iter().map(|r| json!({"kind": r.0, "rule": r.1, "severity": r.2})).collect::<Vec<_>>(),
        "_hint": "Errors stop a part working; warnings are usually mistakes; infos are gaps. A rule is only checked when the bridge read what it needs, so a missing finding is not a pass for a rule the bridge could not read."})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(n: &str, x: f64, y: f64) -> Value { json!({"number": n, "x": x, "y": y, "w": 0.6, "h": 1.5}) }

    #[test]
    fn a_clean_footprint_and_its_symbol_pass() {
        let r = lint_library(&json!({"library": {
            "footprints": [{"name": "SOT23", "pads": [pad("1", -0.95, -1.1), pad("2", 0.95, -1.1), pad("3", 0.0, 1.1)], "courtyard": [-1.7, -2.0, 1.7, 2.0], "silk": [[0, 0]], "bodies": [{"h": 1.1}], "description": "SOT-23"}],
            "symbols": [{"name": "BJT", "pins": [{"number": "1"}, {"number": "2"}, {"number": "3"}], "footprint": "Lib:SOT23", "partNumber": "MMBT3904"}]}}));
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["checked"], 2);
        assert_eq!(r["findings"], json!([]));
    }

    #[test]
    fn the_shared_rules_fire_with_their_own_severity() {
        // the xcopy bug of altium-bridge#29: a copy 1270 mm off its origin
        let far = json!({"name": "BAT_FP_X", "pads": [pad("1", 1270.0, 0.0), pad("1", 1272.0, 0.0), json!({"number": "", "x": 1271.0, "y": 0.0, "w": 0.0, "h": 0.0})], "courtyard": null, "silk": [], "description": ""});
        let r = lint_library(&json!({"library": {"footprints": [far],
            "symbols": [{"name": "BAT", "pins": [{"number": "1"}, {"number": "2"}], "footprint": "BAT_FP_X", "partNumber": ""}, {"name": "LOST", "pins": [{"number": "1"}], "footprint": "NOPE"}, {"name": "BARE", "pins": [], "footprint": ""}],
            "findings": [{"name": "X", "kind": "library", "rule": "tool_refuses_file", "severity": "error", "detail": "kicad-cli: bad token"}]}}));
        let rules: Vec<&str> = r["findings"].as_array().unwrap().iter().map(|x| x["rule"].as_str().unwrap()).collect();
        for want in ["tool_refuses_file", "zero_size_pad", "unnumbered_pad", "duplicate_pad_number", "no_courtyard", "origin_off_part", "no_silkscreen", "no_description", "pin_without_pad", "no_part_number", "footprint_not_found", "no_pins", "no_footprint"] {
            assert!(rules.contains(&want), "{want} missing from {rules:?}");
        }
        // bodies was not sent, so no_3d_body is not evaluated
        assert!(!rules.contains(&"no_3d_body"));
        assert_eq!(r["ok"], false);
        assert!(r["findings"].as_array().unwrap().iter().all(|x| x["severity"] == severity(x["rule"].as_str().unwrap())));
        let off = r["findings"].as_array().unwrap().iter().find(|x| x["rule"] == "origin_off_part").unwrap();
        assert!(off["detail"].as_str().unwrap().starts_with("the origin is 1268.700 mm"), "{off}");
    }

    #[test]
    fn a_split_exposed_pad_is_one_land() {
        // KiCad's Package_SO: an exposed pad 17 as four tiles and two thermal vias, plus two pins both numbered 1
        let ep: Vec<Value> = [(-0.6, -0.6), (0.6, -0.6), (-0.6, 0.6), (0.6, 0.6)].iter().map(|&(x, y)| json!({"number": "17", "x": x, "y": y, "w": 1.2, "h": 1.2})).chain([json!({"number": "17", "x": 0.0, "y": 0.0, "w": 0.6, "h": 0.6, "hole": 0.3}), json!({"number": "17", "x": 0.3, "y": 0.0, "w": 0.0, "h": 0.0, "mechanical": true})]).collect();
        let mut pads = ep.clone();
        pads.extend([pad("1", -2.7, 1.9), pad("1", 2.7, 1.9)]);
        let r = lint_library(&json!({"library": {"footprints": [{"name": "HTSSOP", "pads": pads}]}}));
        let d: Vec<&str> = r["findings"].as_array().unwrap().iter().filter(|x| x["rule"] == "duplicate_pad_number").map(|x| x["detail"].as_str().unwrap()).collect();
        assert_eq!(d, vec!["pad number 1 is on 2 separate lands (2 pads)"], "{r}");
    }

    #[test]
    fn a_spare_pad_warns_and_a_mechanical_one_does_not() {
        let fp = json!({"name": "USB", "pads": [pad("1", 0.0, 0.0), pad("2", 0.65, 0.0), pad("S1", 3.0, 0.0), json!({"number": "", "x": -2.0, "y": 1.0, "w": 0.0, "h": 0.0, "hole": 0.65, "plated": false})]});
        let r = lint_library(&json!({"library": {"footprints": [fp], "symbols": [{"name": "USB", "pins": [{"number": "1"}, {"number": "2"}], "footprint": "USB"}]}}));
        let f: Vec<(&str, &str)> = r["findings"].as_array().unwrap().iter().map(|x| (x["rule"].as_str().unwrap(), x["detail"].as_str().unwrap())).collect();
        assert_eq!(f, vec![("pad_without_pin", "pad S1 has no pin on USB")], "{r}");
        assert_eq!(r["ok"], true, "a warning does not fail the library");
        // a name filter, and unread items fail ok
        let r = lint_library(&json!({"library": {"footprints": [{"name": "A", "pads": []}, {"name": "B", "pads": [pad("1", 0.0, 0.0)]}], "unread": [{"name": "C", "error": "locked"}]}, "args": {"names": ["B"]}}));
        assert_eq!((r["checked"].clone(), r["errors"].clone(), r["ok"].clone()), (json!(1), json!(0), json!(false)));
    }
}

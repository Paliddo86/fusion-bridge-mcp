//! Design review: small checks, each a pure function over the board model, the sheet parts and the board's rules. From
//! the Altium bridge's review.rs, on the standard model: components off the board are judged against the outline
//! polygon (Altium's read carried only a bounding box), and rules are classed from a standard kind, a descriptor's
//! leading words or Altium's rule ordinal. What the model cannot answer is named in `notImplemented`.
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn s<'a>(v: &'a Value, k: &str) -> &'a str { v.get(k).and_then(|x| x.as_str()).unwrap_or("") }

const PLACEHOLDER_VALUES: [&str; 12] = ["comment", "value", "*", "?", "n/a", "na", "tbd", "xxx", "component", "designator", "part", "unknown"];

/// Adom house rule: Mouser is the preferred vendor, and every purchased part carries Mouser, DigiKey and LCSC numbers.
const VENDOR_PATTERNS: [(&str, &[&str]); 3] = [("Mouser", &["mouser"]), ("DigiKey", &["digikey", "digi-key", "digi key"]), ("LCSC", &["lcsc", "jlcpcb"])];

/// Designator prefixes that carry no vendor part number.
const NON_PURCHASED: [&str; 8] = ["TP", "FID", "MH", "H", "LOGO", "NET", "PWR", "GND"];

/// The rules a fab relies on: clearance, width, hole size, short circuit.
const CRITICAL: [&str; 4] = ["clearance", "width", "hole size", "short circuit"];

pub const NOT_IMPLEMENTED: [(&str, &str); 4] = [
    ("unused_mechanical_layers", "the board model carries no mechanical layers"),
    ("unrouted_connections", "needs the ratsnest: routing_drc or the tool's DRC"),
    ("silkscreen_over_pad", "silk_designators reports labels on pads (onPadsBefore)"),
    ("missing_decoupling", "schematic_review checks each IC supply for a capacitor to ground"),
];

fn finding(check: &str, severity: &str, key: (&str, &str), message: String) -> Value {
    let mut v = json!({"check": check, "severity": severity, "message": message});
    if !key.1.is_empty() { v[key.0] = json!(key.1); }
    v
}

fn purchased(designator: &str) -> bool {
    let prefix: String = designator.to_uppercase().chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    !NON_PURCHASED.contains(&prefix.as_str())
}

fn params_of(c: &Value) -> Vec<(String, String)> {
    c["params"].as_array().map(|a| a.iter().filter_map(|p| {
        let name = p["name"].as_str()?.trim().to_lowercase();
        (!name.is_empty()).then(|| (name, p["value"].as_str().unwrap_or("").trim().to_string()))
    }).collect()).unwrap_or_default()
}

/// A rule's class: a standard kind, the descriptor's leading words, or Altium's rule ordinal. Pure.
pub fn rule_class(r: &Value) -> String {
    if let Some(k) = r["kind"].as_str() {
        let k = k.to_lowercase();
        for c in CRITICAL { if k.replace(['_', '-'], " ").contains(c) || (c == "hole size" && k == "hole") || (c == "short circuit" && k == "short") { return c.into(); } }
        return k;
    }
    let d = s(r, "descriptor").to_lowercase().replace('-', " ");
    for (prefix, c) in [("clearance constraint", "clearance"), ("width constraint", "width"), ("hole size constraint", "hole size"), ("short circuit constraint", "short circuit")] {
        if d.starts_with(prefix) { return c.into(); }
    }
    match r["kind"].as_i64() {
        Some(0) => return "clearance".into(), Some(2) => return "width".into(), Some(42) => return "hole size".into(), Some(15) => return "short circuit".into(),
        // any other Altium ordinal is no critical rule: its descriptor names it ("Routing Topology Rule (Shortest)" is
        // no short-circuit rule, "Polygon Connect (Conductor Width ...)" no width rule)
        Some(_) => return d.split('(').next().unwrap_or("").trim().to_string(),
        None => {}
    }
    // a tool's own rule names (Fusion's EAGLE rules: wire-wire, pad-via, minimum width, minimum drill, ...)
    let t = format!("{} {} {}", s(r, "kindName"), s(r, "name"), d).to_lowercase().replace(['_', '-'], " ");
    if t.contains("short circuit") || t.contains("shortcircuit") { return "short circuit".into(); }
    if (t.contains("width") || t.contains("minimum wire")) && !t.contains("annular") && !t.contains("conductor") { return "width".into(); }
    if t.contains("drill") || t.contains("hole size") || t.contains("holesize") { return "hole size".into(); }
    let copper = !["silk", "mask", "paste", "edge", "board", "courtyard", "component", "hole to hole", "plane"].iter().any(|w| t.contains(w));
    if copper && (t.contains("clearance") || t.contains("wire wire") || t.contains("pad via") || t.contains("pad pad")) { return "clearance".into(); }
    d.split('(').next().unwrap_or("").trim().to_string()
}

/// The checks over the board model, the sheet parts (read_sheet_file's components, or none) and the rules. Returns the
/// findings (errors first), the checks run and those skipped with why. Pure.
pub fn run_checks(st: &Value, sheet: Option<&[Value]>, rules: &[Value], grid: f64, edge: f64, wanted: Option<&[String]>, always: &[String]) -> (Vec<Value>, Vec<String>, Map<String, Value>) {
    let comps = st["components"].as_array().cloned().unwrap_or_default();
    let pads = st["pads"].as_array().cloned().unwrap_or_default();
    let ring = crate::coppermap::outline_ring(st);
    let want = |n: &str| wanted.map(|w| w.iter().any(|x| x == n)).unwrap_or(true);
    let (mut out, mut ran, mut skipped) = (Vec::new(), Vec::new(), Map::new());
    let mut run = |name: &str, needs: Option<&str>, out: &mut Vec<Value>, f: &dyn Fn() -> Vec<Value>| {
        if !want(name) { return; }
        if let Some(why) = needs { skipped.insert(name.into(), json!(why)); return; }
        ran.push(name.to_string());
        out.extend(f());
    };
    let placed: Vec<&Value> = comps.iter().filter(|c| pads.iter().any(|p| p["ref"] == c["ref"])).collect();
    let own = |r: &str| -> Vec<(f64, f64)> { pads.iter().filter(|p| p["ref"].as_str() == Some(r)).map(|p| (f(&p["x"]), f(&p["y"]))).collect() };
    run("placeholder_values", None, &mut out, &|| placed.iter().filter_map(|c| {
        let v = c.get("value").or(c.get("comment")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        PLACEHOLDER_VALUES.contains(&v.to_lowercase().as_str()).then(|| finding("placeholder_values", "error", ("designator", s(c, "ref")), format!("{} has the placeholder value {v:?}: it will reach the BOM as is.", s(c, "ref"))))
    }).collect());
    run("single_pin_nets", None, &mut out, &|| {
        let mut n: BTreeMap<&str, usize> = BTreeMap::new();
        for p in &pads { if let Some(net) = p["net"].as_str().filter(|x| !x.is_empty()) { *n.entry(net).or_default() += 1; } }
        n.into_iter().filter(|(_, k)| *k == 1).map(|(net, _)| finding("single_pin_nets", "warning", ("net", net), format!("Net {net:?} has a single pad: it connects to nothing."))).collect()
    });
    run("off_grid_components", if grid > 0.0 { None } else { Some("no placement grid: pass gridMm (a 5 mil grid is 0.127)") }, &mut out, &|| placed.iter().filter_map(|c| {
        let off = |v: f64| { let k = v / grid; (k - k.round()).abs() * grid > 0.001 };
        (off(f(&c["x"])) || off(f(&c["y"]))).then(|| finding("off_grid_components", "info", ("designator", s(c, "ref")), format!("{} sits at ({}, {}) mm, off the {grid} mm grid.", s(c, "ref"), f(&c["x"]), f(&c["y"]))))
    }).collect());
    run("duplicate_designators", None, &mut out, &|| {
        let mut v = Vec::new();
        let mut count = |label: &str, names: Vec<String>| {
            let mut seen: BTreeMap<String, u32> = BTreeMap::new();
            for d in names.into_iter().filter(|d| !d.is_empty()) { *seen.entry(d).or_default() += 1; }
            for (d, n) in seen { if n > 1 { v.push(finding("duplicate_designators", "error", ("designator", &d), format!("Designator {d} occurs {n} times on the {label}."))); } }
        };
        count("board", comps.iter().map(|c| s(c, "ref").to_string()).collect());
        // a multi-part chip appears once per part on the sheets: count each designator once per part id
        if let Some(sh) = sheet {
            let mut uniq: BTreeSet<(String, String)> = BTreeSet::new();
            let names: Vec<String> = sh.iter().filter(|c| uniq.insert((s(c, "designator").to_string(), format!("{}{}", s(c, "uniqueId"), c["partId"])))).map(|c| s(c, "designator").to_string()).collect();
            let multi: BTreeSet<String> = sh.iter().filter(|c| c["partCount"].as_i64().unwrap_or(1) > 1).map(|c| s(c, "designator").to_string()).collect();
            count("sheets", names.into_iter().filter(|d| !multi.contains(d)).collect());
        }
        v
    });
    run("components_outside_board_outline", if ring.len() < 3 { Some("the board has no outline") } else { None }, &mut out, &|| placed.iter().filter_map(|c| {
        let ps = own(s(c, "ref"));
        let outside = ps.iter().filter(|p| !crate::route::inside(**p, &ring)).count();
        (outside > 0).then(|| finding("components_outside_board_outline", "error", ("designator", s(c, "ref")),
            format!("{} has {outside} of {} pad(s) outside the board outline.", s(c, "ref"), ps.len())))
    }).collect());
    run("components_near_board_edge", if ring.len() < 3 { Some("the board has no outline") } else { None }, &mut out, &|| placed.iter().filter_map(|c| {
        let ps = own(s(c, "ref"));
        if ps.iter().any(|p| !crate::route::inside(*p, &ring)) { return None; }
        let seg = |p: (f64, f64), a: (f64, f64), b: (f64, f64)| {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l2 = dx * dx + dy * dy;
            let t = if l2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) };
            ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
        };
        let d = ps.iter().map(|p| (0..ring.len()).map(|i| seg(*p, ring[i], ring[(i + 1) % ring.len()])).fold(f64::MAX, f64::min)).fold(f64::MAX, f64::min);
        (d < edge).then(|| finding("components_near_board_edge", "warning", ("designator", s(c, "ref")), format!("{} has a pad centre {:.3} mm from the board edge (under {edge} mm).", s(c, "ref"), d)))
    }).collect());
    let no_sheet = sheet.is_none().then_some("no sheet parts: pass sheetParts");
    let sheet_parts: Vec<Value> = sheet.map(|x| x.to_vec()).unwrap_or_default();
    run("vendor_part_numbers", no_sheet, &mut out, &|| sheet_parts.iter().filter(|c| purchased(s(c, "designator"))).filter_map(|c| {
        let blob = params_of(c).iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(" ").to_lowercase();
        let missing: Vec<&str> = VENDOR_PATTERNS.iter().filter(|(_, keys)| !keys.iter().any(|k| blob.contains(k))).map(|(v, _)| *v).collect();
        if missing.is_empty() { return None; }
        let mut x = finding("vendor_part_numbers", "warning", ("designator", s(c, "designator")), format!("{} has no {} part number (house rule: Mouser preferred, all three required).", s(c, "designator"), missing.join(" / ")));
        x["missing"] = json!(missing);
        Some(x)
    }).collect());
    run("missing_datasheets", no_sheet, &mut out, &|| sheet_parts.iter().filter(|c| purchased(s(c, "designator"))).filter_map(|c| {
        let has = params_of(c).iter().any(|(k, _)| k.contains("datasheet") || k == "helpurl");
        (!has).then(|| finding("missing_datasheets", "info", ("designator", s(c, "designator")), format!("{} has no Datasheet parameter.", s(c, "designator"))))
    }).collect());
    run("schematic_placeholder_values", no_sheet, &mut out, &|| sheet_parts.iter().filter_map(|c| {
        let v = s(c, "comment").trim().to_string();
        PLACEHOLDER_VALUES.contains(&v.to_lowercase().as_str()).then(|| finding("schematic_placeholder_values", "error", ("designator", s(c, "designator")), format!("{} on the sheet has the placeholder Comment {v:?}.", s(c, "designator"))))
    }).collect());
    let no_rules = rules.is_empty().then_some("no rules: pass read_rules as rules");
    run("disabled_design_rules", no_rules, &mut out, &|| rules.iter().filter(|r| r["enabled"] == json!(false)).map(|r| {
        let c = rule_class(r);
        let sev = if CRITICAL.contains(&c.as_str()) { "error" } else { "warning" };
        finding("disabled_design_rules", sev, ("rule", s(r, "name")), format!("Design rule {:?} ({c}) is disabled: DRC passes without ever applying it.", s(r, "name")))
    }).collect());
    run("missing_critical_rules", no_rules, &mut out, &|| {
        // a tool that always checks a class (EAGLE's DRC always checks shorts) passes it in alwaysChecked
        let mut have: BTreeSet<String> = rules.iter().filter(|r| r["enabled"] != json!(false)).map(rule_class).collect();
        have.extend(always.iter().map(|a| a.to_lowercase().replace(['_', '-'], " ")));
        CRITICAL.iter().filter(|c| !have.contains(**c)).map(|c| finding("missing_critical_rules", "error", ("rule", c), format!("No enabled {c} rule on this board."))).collect()
    });
    run("duplicate_rule_priority", no_rules, &mut out, &|| {
        let mut seen: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        for r in rules.iter().filter(|r| r["enabled"] != json!(false) && !r["priority"].is_null()) { seen.entry((rule_class(r), r["priority"].to_string())).or_default().push(s(r, "name").to_string()); }
        seen.into_iter().filter(|(_, n)| n.len() > 1).map(|((c, p), mut n)| { n.sort(); finding("duplicate_rule_priority", "info", ("rule", &n.join(", ")), format!("{} enabled {c} rules share priority {p} ({}): which one wins is not readable from the board.", n.len(), n.join(", "))) }).collect()
    });
    run("components_without_footprint", None, &mut out, &|| placed.iter().filter(|c| s(c, "pattern").trim().is_empty())
        .map(|c| finding("components_without_footprint", "warning", ("designator", s(c, "ref")), format!("{} has no footprint pattern name: it cannot be matched back to a library.", s(c, "ref")))).collect());
    let order = |x: &Value| match s(x, "severity") { "error" => 0, "warning" => 1, _ => 2 };
    out.sort_by(|a, b| order(a).cmp(&order(b)).then_with(|| s(a, "check").cmp(s(b, "check"))));
    (out, ran, skipped)
}

/// review_board: input {board, sheetParts?, rules?, args: {checks?, gridMm?, edgeMm?, alwaysChecked?: ["short circuit", ...]}}.
pub fn review_board(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let sheet: Option<Vec<Value>> = crate::input::part(input, "sheetParts").and_then(|p| p.get("components").or(Some(&p)).and_then(|x| x.as_array()).cloned());
    let wanted: Option<Vec<String>> = args["checks"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect());
    let always: Vec<String> = args["alwaysChecked"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let (findings, ran, skipped) = run_checks(&st, sheet.as_deref(), &crate::input::rules(input), args["gridMm"].as_f64().unwrap_or(0.0), args["edgeMm"].as_f64().unwrap_or(0.5), wanted.as_deref(), &always);
    let mut counts = json!({"error": 0, "warning": 0, "info": 0});
    for x in &findings { let k = s(x, "severity").to_string(); counts[&k] = json!(counts[&k].as_i64().unwrap_or(0) + 1); }
    json!({"success": true, "findings": findings, "counts": counts, "checks": ran, "skipped": skipped,
        "notImplemented": NOT_IMPLEMENTED.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<Map<String, Value>>(),
        "_hint": "Findings are errors first. skipped names the checks a missing companion stopped (pass sheetParts and rules for all of them); notImplemented names what this review does not judge, so silence there is not a clean board."})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(r: &str, n: &str, x: f64, y: f64, net: &str) -> Value { json!({"ref": r, "name": n, "x": x, "y": y, "net": net}) }

    #[test]
    fn each_check_finds_its_problem() {
        let st = json!({"outline": [[0, 0], [20, 0], [20, 10], [0, 10]],
            "components": [{"ref": "U1", "x": 5.0, "y": 5.0, "pattern": "SOIC8", "value": "TBD"}, {"ref": "R1", "x": 19.8, "y": 5.0, "pattern": ""}, {"ref": "R1", "x": 10.0, "y": 5.0, "pattern": "0603"}, {"ref": "J1", "x": 25.0, "y": 5.0, "pattern": "HDR"}],
            "pads": [pad("U1", "1", 4.0, 5.0, "A"), pad("U1", "2", 6.0, 5.0, "LONE"), pad("R1", "1", 19.8, 5.0, "A"), pad("J1", "1", 25.0, 5.0, "")]});
        let sheet = vec![json!({"designator": "U1", "comment": "LM358", "params": [{"name": "Mouser Part Number", "value": "595-LM358"}]}), json!({"designator": "TP1", "comment": "Comment"})];
        let rules = vec![json!({"name": "Clearance", "kind": 0, "enabled": true, "priority": 1, "descriptor": "Clearance Constraint (Gap=0.1mm)"}),
                         json!({"name": "Short", "kind": 15, "enabled": false, "priority": 1, "descriptor": "Short-Circuit Constraint (Allowed=No)"}),
                         json!({"name": "W1", "kind": "width", "enabled": true, "priority": 2}), json!({"name": "W2", "kind": "width", "enabled": true, "priority": 2})];
        let (f, ran, _) = run_checks(&st, Some(&sheet), &rules, 0.127, 0.5, None, &[]);
        let has = |c: &str, key: &str, v: &str| f.iter().any(|x| x["check"] == c && x[key] == v);
        assert!(has("placeholder_values", "designator", "U1"));
        assert!(has("single_pin_nets", "net", "LONE"));
        assert!(has("duplicate_designators", "designator", "R1"));
        assert!(has("components_outside_board_outline", "designator", "J1"));
        assert!(has("components_near_board_edge", "designator", "R1"));
        assert!(has("components_without_footprint", "designator", "R1"));
        assert!(f.iter().any(|x| x["check"] == "vendor_part_numbers" && x["designator"] == "U1" && x["missing"] == json!(["DigiKey", "LCSC"])));
        assert!(!f.iter().any(|x| x["check"] == "vendor_part_numbers" && x["designator"] == "TP1"), "a test point is not purchased");
        assert!(has("schematic_placeholder_values", "designator", "TP1"));
        assert!(f.iter().any(|x| x["check"] == "disabled_design_rules" && x["severity"] == "error"));
        assert!(has("missing_critical_rules", "rule", "hole size") && has("missing_critical_rules", "rule", "short circuit"));
        assert!(f.iter().any(|x| x["check"] == "duplicate_rule_priority"));
        assert_eq!(f[0]["severity"], "error");
        assert_eq!(ran.len(), 13);
        let (_, _, skipped) = run_checks(&st, None, &[], 0.127, 0.5, None, &[]);
        assert!(skipped.contains_key("vendor_part_numbers") && skipped.contains_key("missing_critical_rules"));
        // EAGLE always checks shorts: passed in alwaysChecked, a missing short-circuit rule is no finding
        let (f2, _, _) = run_checks(&st, Some(&sheet), &rules, 0.0, 0.5, Some(&["missing_critical_rules".to_string()]), &["short_circuit".to_string()]);
        assert!(!f2.iter().any(|x| x["rule"] == "short circuit"), "{f2:?}");
    }

    #[test]
    fn rules_are_classed_three_ways() {
        assert_eq!(rule_class(&json!({"kind": 42})), "hole size");
        assert_eq!(rule_class(&json!({"kind": 99, "descriptor": "Width Constraint (Min=0.1mm)"})), "width");
        assert_eq!(rule_class(&json!({"kind": "short_circuit"})), "short circuit");
        // a tool's own names (Fusion's EAGLE rules), and a silk clearance is no copper clearance
        assert_eq!(rule_class(&json!({"name": "Wire-Wire", "value": "0.1mm"})), "clearance");
        assert_eq!(rule_class(&json!({"name": "Minimum Drill"})), "hole size");
        assert_eq!(rule_class(&json!({"name": "Silk Clearance"})), "");
        // Altium's other ordinals stay what their descriptor says, whatever words they hold
        assert_eq!(rule_class(&json!({"kind": 7, "name": "RoutingTopology", "descriptor": "Routing Topology Rule(Topology=Shortest) (All)"})), "routing topology rule");
        assert_eq!(rule_class(&json!({"kind": 6, "name": "PlaneConnect", "descriptor": "Power Plane Connect Rule(Relief Connect )(Conductor Width=0.254mm)"})), "power plane connect rule");
    }
}

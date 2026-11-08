//! Ready for JLCPCB assembly? The board against both of JLCPCB's PCBA tiers as published
//! (jlcpcb.com/capabilities/pcb-assembly-capabilities, read 2026-10-26): size, thickness and layers, one or two sides,
//! the smallest package and the finest pin pitch, fiducials, and which parts have no LCSC number. It answers the tier
//! the board can be ordered in. From the Altium bridge's assembly.rs.
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }

/// What the board offers the assembly line, from the board model. Pure.
pub fn facts(st: &Value) -> Value {
    let pads = st["pads"].as_array().cloned().unwrap_or_default();
    let comps = st["components"].as_array().cloned().unwrap_or_default();
    let mut by: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    for p in pads.iter().filter(|p| f(&p["drill"]) == 0.0) {
        if let Some(r) = p["ref"].as_str().filter(|r| !r.is_empty()) { by.entry(r.to_string()).or_default().push((f(&p["x"]), f(&p["y"]))); }
    }
    // the finest pitch: the closest pad centres within one part with 3 or more SMD pads (an IC, a BGA, a connector)
    let mut pitch: Option<(f64, String)> = None;
    // the smallest two-pad part: its pad centres (an 0402 is about 1.0 mm apart, an 0201 0.5, an 01005 0.35)
    let mut chip: Option<(f64, String)> = None;
    for (r, ps) in &by {
        let mut d = f64::MAX;
        // pads with one centre are one land (a MOSFET's drain pads drawn as one polygon, fusion-lane #24), not a 0 mm pitch
        for i in 0..ps.len() { for j in i + 1..ps.len() { let e = ((ps[i].0 - ps[j].0).powi(2) + (ps[i].1 - ps[j].1).powi(2)).sqrt(); if e > 0.01 { d = d.min(e); } } }
        if d == f64::MAX { continue; }
        if ps.len() >= 3 && pitch.as_ref().map(|p| d < p.0).unwrap_or(true) { pitch = Some((d, r.clone())); }
        if ps.len() == 2 && chip.as_ref().map(|p| d < p.0).unwrap_or(true) { chip = Some((d, r.clone())); }
    }
    let named = |c: &Value, pat: &[&str]| { let p = c["pattern"].as_str().unwrap_or("").to_ascii_uppercase(); pat.iter().any(|x| p.contains(x)) };
    let tiny: Vec<Value> = comps.iter().filter(|c| named(c, &["0201", "01005", "0603M", "0402M"])).map(|c| json!({"ref": c["ref"], "footprint": c["pattern"]})).collect();
    let fid = |s: &str| s.to_ascii_uppercase().starts_with("FID");
    let fiducials = comps.iter().filter(|c| c["ref"].as_str().map(fid).unwrap_or(false) || named(c, &["FIDUCIAL"])).count()
        + pads.iter().filter(|p| p["ref"].as_str().map(|r| r.is_empty()).unwrap_or(true) && p["name"].as_str().map(fid).unwrap_or(false)).count();
    // a part is a component with pads: a logo or an icon has none and is never placed (fusion-lane #24)
    let placed = |c: &Value| pads.iter().any(|p| p["ref"] == c["ref"]);
    let bottom: Vec<Value> = comps.iter().filter(|c| c["side"] == "bottom" && placed(c)).map(|c| c["ref"].clone()).collect();
    let tht = pads.iter().filter(|p| f(&p["drill"]) > 0.0 && p["ref"].as_str().map(|r| !r.is_empty()).unwrap_or(false))
        .filter_map(|p| p["ref"].as_str().map(String::from)).collect::<std::collections::BTreeSet<_>>();
    json!({"parts": comps.iter().filter(|c| placed(c)).count(), "bottomSideParts": bottom, "finestPitch": pitch.map(|(d, r)| json!({"mm": r3(d), "ref": r})),
           "smallestChip": chip.map(|(d, r)| json!({"padCentresMm": r3(d), "ref": r})), "tinyFootprints": tiny, "fiducials": fiducials,
           "throughHoleParts": tht.into_iter().collect::<Vec<_>>()})
}

fn rule(name: &str, ok: Option<bool>, detail: String) -> Value { json!({"rule": name, "ok": ok, "detail": detail}) }

/// The board against one tier ("economic" or "standard"). `size` is [w, h] mm (the board, or the panel it ships in),
/// `lcsc_missing` the purchased lines with no LCSC number (None: not checked). Pure.
pub fn tier(name: &str, fa: &Value, size: (f64, f64), thick: Option<f64>, layers: Option<i64>, lcsc_missing: Option<usize>, panel: bool) -> Value {
    let eco = name == "economic";
    let mut rs = Vec::new();
    let (w, h) = (size.0.min(size.1), size.0.max(size.1));
    let (lo, hi): ((f64, f64), (f64, f64)) = match (eco, panel) { (true, false) => ((10.0, 10.0), (470.0, 500.0)), (true, true) => ((10.0, 10.0), (250.0, 250.0)), (false, false) => ((70.0, 70.0), (460.0, 500.0)), (false, true) => ((70.0, 70.0), (250.0, 250.0)) };
    let ok = w >= lo.0 && h >= lo.1 && w <= hi.0.min(hi.1) && h <= hi.0.max(hi.1);
    rs.push(rule(&format!("{} {} x {} to {} x {} mm", if panel { "panel" } else { "board" }, lo.0, lo.1, hi.0, hi.1), Some(ok), format!("{} x {} mm{}", r3(size.0), r3(size.1), if !ok && !eco && !panel { ": a panel (panelize) can bring it to 70 x 70" } else { "" })));
    if eco {
        rs.push(match thick { Some(t) => rule("board 0.8 to 1.6 mm thick", Some((0.8 - 1e-6..=1.6 + 1e-6).contains(&t)), format!("{} mm", r3(t))), None => rule("board 0.8 to 1.6 mm thick", None, "thickness not read: pass the stack".into()) });
        rs.push(match layers { Some(n) => rule("2, 4 or 6 layers", Some([2, 4, 6].contains(&n)), format!("{n} layers")), None => rule("2, 4 or 6 layers", None, "layers not read: pass the stack".into()) });
        let bot = fa["bottomSideParts"].as_array().map(|a| a.len()).unwrap_or(0);
        rs.push(rule("parts on one side only", Some(bot == 0), if bot == 0 { "all on top".into() } else { format!("{bot} part(s) on the bottom: {}", fa["bottomSideParts"]) }));
    }
    let (chip_min, pitch_min) = if eco { (0.8, 0.4) } else { (0.3, 0.35) };
    let tiny = fa["tinyFootprints"].as_array().map(|a| a.len()).unwrap_or(0);
    let chip = fa["smallestChip"]["padCentresMm"].as_f64();
    let chip_ok = (eco && tiny == 0 || !eco) && chip.map(|c| c >= chip_min).unwrap_or(true);
    rs.push(rule(if eco { "smallest package 0402" } else { "smallest package 01005" }, Some(chip_ok),
        format!("smallest two-pad part {} (pad centres {} mm){}", fa["smallestChip"]["ref"].as_str().unwrap_or("-"), chip.map(r3).unwrap_or(0.0), if tiny > 0 { format!("; footprints named under 0402: {}", fa["tinyFootprints"]) } else { String::new() })));
    let pitch = fa["finestPitch"]["mm"].as_f64();
    rs.push(rule(&format!("IC pin pitch {pitch_min} mm or more (BGA {} mm)", if eco { 0.5 } else { 0.3 }), Some(pitch.map(|p| p + 1e-6 >= pitch_min).unwrap_or(true)),
        format!("finest {} mm on {}", pitch.map(r3).unwrap_or(0.0), fa["finestPitch"]["ref"].as_str().unwrap_or("-"))));
    if !eco {
        let n = fa["fiducials"].as_u64().unwrap_or(0);
        rs.push(rule("fiducials and edge rails", Some(n >= 2 || panel), if panel { "on the panel rails (panelize adds three)".into() } else { format!("{n} fiducial(s) on the board: add 3 (board_features fiducials) or ship it in a panel with rails") }));
    }
    rs.push(match lcsc_missing { Some(0) => rule("every purchased part has an LCSC number", Some(true), "all lines".into()),
        Some(n) => rule("every purchased part has an LCSC number", Some(false), format!("{n} line(s) without one: find them (adom-parts-search, adom-jlcpcb) or consign them")),
        None => rule("every purchased part has an LCSC number", None, "not checked: pass lcscMissing from the bridge's bom".into()) });
    let ok = rs.iter().all(|r| r["ok"] != json!(false));
    json!({"tier": name, "ok": ok, "rules": rs})
}

/// assembly_check: input {board, stack?, args: {panelMm?: [w, h], lcscMissing?: count or the BOM lines without one}}.
pub fn assembly_check(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let Some((x0, y0, x1, y1)) = crate::geom::outline_bbox(&st) else { return crate::input::refuse("no_outline", "the board has no outline") };
    let panel = args["panelMm"].as_array().filter(|a| a.len() == 2).map(|a| (f(&a[0]), f(&a[1])));
    let size = panel.unwrap_or((x1 - x0, y1 - y0));
    let stack = crate::input::stack(input);
    let thick = stack.as_ref().and_then(|s| s["totalUm"].as_f64()).map(|u| u / 1000.0);
    let layers = stack.as_ref().and_then(|s| s["copperLayers"].as_i64());
    let (lcsc_missing, lcsc_lines) = match &args["lcscMissing"] {
        Value::Array(a) => (Some(a.len()), Value::Array(a.clone())),
        v if v.as_u64().is_some() => (v.as_u64().map(|n| n as usize), Value::Null),
        _ => (None, Value::Null),
    };
    let fa = facts(&st);
    let eco = tier("economic", &fa, size, thick, layers, lcsc_missing, panel.is_some());
    let std_ = tier("standard", &fa, size, thick, layers, lcsc_missing, panel.is_some());
    let service = if eco["ok"] == json!(true) { json!("economic") } else if std_["ok"] == json!(true) { json!("standard") } else { Value::Null };
    json!({"success": true, "fab": {"name": "JLCPCB", "source": "https://jlcpcb.com/capabilities/pcb-assembly-capabilities", "read": "2026-10-26"},
        "service": service, "economic": eco, "standard": std_, "board": fa, "linesWithoutLcsc": lcsc_lines,
        "_hint": "service is the cheapest JLCPCB assembly tier every rule allows (null: fix what the rules mark false). Rotation: JLCPCB's part orientation can differ from the tool's for some packages; check every part in JLCPCB's placement preview after uploading the BOM and pick-and-place."})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Value {
        json!({"components": [{"ref": "U1", "side": "top", "pattern": "QFN-20"}, {"ref": "C1", "side": "top", "pattern": "C0402"}, {"ref": "R9", "side": "bottom", "pattern": "R0603"}],
               "pads": [{"ref": "U1", "name": "1", "x": 0.0, "y": 0.0, "drill": 0.0}, {"ref": "U1", "name": "2", "x": 0.5, "y": 0.0, "drill": 0.0}, {"ref": "U1", "name": "3", "x": 1.0, "y": 0.0, "drill": 0.0},
                        {"ref": "C1", "name": "1", "x": 5.0, "y": 0.0, "drill": 0.0}, {"ref": "C1", "name": "2", "x": 6.0, "y": 0.0, "drill": 0.0},
                        {"ref": "R9", "name": "1", "x": 9.0, "y": 0.0, "drill": 0.0}, {"ref": "R9", "name": "2", "x": 10.5, "y": 0.0, "drill": 0.0}],
               "outline": [[0, 0], [40, 0], [40, 20], [0, 20]]})
    }

    #[test]
    fn facts_find_pitch_the_smallest_chip_and_the_bottom_side() {
        let fa = facts(&board());
        assert_eq!(fa["finestPitch"], json!({"mm": 0.5, "ref": "U1"}));
        assert_eq!(fa["smallestChip"], json!({"padCentresMm": 1.0, "ref": "C1"}));
        assert_eq!(fa["bottomSideParts"], json!(["R9"]));
        // fusion-lane #24: pads sharing one centre are one land, and a pad-less icon on the bottom is no part
        let mut b = board();
        b["components"].as_array_mut().unwrap().push(json!({"ref": "Q1", "side": "top", "pattern": "PowerPAK"}));
        b["components"].as_array_mut().unwrap().push(json!({"ref": "LOGO1", "side": "bottom", "pattern": "ICON"}));
        for k in 5..=8 { b["pads"].as_array_mut().unwrap().push(json!({"ref": "Q1", "name": k.to_string(), "x": 20.0, "y": 0.0, "drill": 0.0})); }
        b["pads"].as_array_mut().unwrap().push(json!({"ref": "Q1", "name": "1", "x": 20.0, "y": 1.27, "drill": 0.0}));
        let fa = facts(&b);
        assert_eq!(fa["finestPitch"], json!({"mm": 0.5, "ref": "U1"}));
        assert_eq!((fa["bottomSideParts"].clone(), fa["parts"].clone()), (json!(["R9"]), json!(4)));
    }

    #[test]
    fn a_small_two_sided_board_needs_standard_in_a_panel() {
        let fa = facts(&board());
        let eco = tier("economic", &fa, (40.0, 20.0), Some(1.6), Some(2), Some(0), false);
        assert_eq!(eco["ok"], json!(false)); // R9 on the bottom
        let std_ = tier("standard", &fa, (40.0, 20.0), Some(1.6), Some(2), Some(0), false);
        assert_eq!(std_["ok"], json!(false)); // under 70 x 70 and no fiducials
        let std_panel = tier("standard", &fa, (90.0, 80.0), Some(1.6), Some(2), Some(0), true);
        assert_eq!(std_panel["ok"], json!(true));
        assert_eq!(tier("economic", &fa, (40.0, 20.0), Some(0.4), Some(2), None, false)["rules"][1]["ok"], json!(false));
    }

    #[test]
    fn the_verb_reads_the_stack_and_the_bridges_lcsc_lines() {
        let r = assembly_check(&json!({"board": board(), "stack": {"totalUm": 1600, "copperLayers": 2}, "args": {"panelMm": [90, 80], "lcscMissing": [{"comment": "10k", "designators": "R9"}]}}));
        assert_eq!(r["service"], Value::Null, "{r}");
        assert_eq!(r["linesWithoutLcsc"][0]["designators"], "R9");
        let lcsc = r["standard"]["rules"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(lcsc["ok"], json!(false));
        assert_eq!(assembly_check(&json!({"board": board(), "stack": {"totalUm": 1600, "copperLayers": 2}, "args": {"panelMm": [90, 80], "lcscMissing": 0}}))["service"], "standard");
        assert_eq!(assembly_check(&json!({"board": {"pads": []}, "args": {}}))["errorCode"], "no_outline");
    }
}

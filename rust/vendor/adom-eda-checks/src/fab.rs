//! The board's fab-relevant minimums, measured from the board model, and how they compare with a fab's limits: the
//! numbers a fab's order form and capability page ask about, so the AI can check a board before ordering. Fab limits
//! are never kept here: they come in as a record from adom/pcb-design-rules (`fab`), or as explicit `limits`.
use serde_json::{json, Map, Value};

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }

/// The nearest copper (track, arc, pad, via) to the real outline, edge of copper to the outline's line, mm. Pour
/// boundaries are left out: they are drawn on the edge and the tool pulls the poured copper back by its rules.
pub fn edge_distance(state: &Value) -> Option<(f64, Value)> {
    let ring = crate::coppermap::outline_ring(state);
    if ring.len() < 3 { return None; }
    let seg = |x: f64, y: f64| -> f64 {
        (0..ring.len()).map(|k| {
            let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 0.0 { (((x - a.0) * dx + (y - a.1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            ((a.0 + t * dx - x).powi(2) + (a.1 + t * dy - y).powi(2)).sqrt()
        }).fold(f64::MAX, f64::min)
    };
    let copper = |l: &Value| l.as_str().map(|s| s.ends_with(".Cu")).unwrap_or(false);
    let mut pts: Vec<(f64, f64, f64, Value)> = Vec::new();
    for t in state["tracks"].as_array().cloned().unwrap_or_default().iter().filter(|t| copper(&t["layer"])) {
        for p in [&t["start"], &t["end"]] { pts.push((f(&p["x"]), f(&p["y"]), f(&t["width"]) / 2.0, json!({"track": t["net"], "layer": t["layer"]}))); }
    }
    for v in state["vias"].as_array().cloned().unwrap_or_default() { pts.push((f(&v["position"]["x"]), f(&v["position"]["y"]), f(&v["size"]) / 2.0, json!({"via": v["net"]}))); }
    for p in state["pads"].as_array().cloned().unwrap_or_default() {
        let (rf, nm) = (p["ref"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or(""));
        pts.push((f(&p["x"]), f(&p["y"]), f(&p["sizeX"]).max(f(&p["sizeY"])) / 2.0, json!({"pad": if rf.is_empty() { nm.to_string() } else { format!("{rf}.{nm}") }})));
    }
    pts.into_iter().map(|(x, y, half, mut what)| { what["at"] = json!([r3(x), r3(y)]); (r3(seg(x, y) - half), what) })
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
}

/// The smallest of each feature with where it is. Pure, for the tests.
pub fn measure(state: &Value) -> Map<String, Value> {
    let mut m = Map::new();
    let mut min_by = |key: &str, items: Vec<(f64, Value)>| {
        if let Some((v, at)) = items.into_iter().filter(|(v, _)| *v > 0.0).min_by(|a, b| a.0.partial_cmp(&b.0).unwrap()) {
            m.insert(key.into(), json!({"mm": r3(v), "at": at}));
        }
    };
    // copper only: outline and drawing lines on mechanical layers (L57...) are not tracks a fab etches
    let copper = |l: &Value| l.as_str().map(|s| s == "F.Cu" || s == "B.Cu" || (s.starts_with("In") && s.ends_with(".Cu"))).unwrap_or(false);
    min_by("track", state["tracks"].as_array().cloned().unwrap_or_default().iter().filter(|t| copper(&t["layer"])).map(|t| (f(&t["width"]), json!({"net": t["net"], "layer": t["layer"], "from": [t["start"]["x"], t["start"]["y"]]}))).collect());
    let vias = state["vias"].as_array().cloned().unwrap_or_default();
    min_by("viaDrill", vias.iter().map(|v| (f(&v["drill"]), json!({"net": v["net"], "at": [v["position"]["x"], v["position"]["y"]]}))).collect());
    min_by("viaDiameter", vias.iter().map(|v| (f(&v["size"]), json!({"net": v["net"], "at": [v["position"]["x"], v["position"]["y"]]}))).collect());
    min_by("viaAnnularRing", vias.iter().map(|v| ((f(&v["size"]) - f(&v["drill"])) / 2.0, json!({"net": v["net"], "at": [v["position"]["x"], v["position"]["y"]]}))).collect());
    let pads = state["pads"].as_array().cloned().unwrap_or_default();
    let drilled: Vec<&Value> = pads.iter().filter(|p| f(&p["drill"]) > 0.0).collect();
    let label = |p: &Value| json!({"pad": format!("{}.{}", p["ref"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or("")), "at": [p["x"], p["y"]]});
    min_by("padHole", drilled.iter().map(|p| (f(&p["drill"]), label(p))).collect());
    // a pad's ring on its narrow side (a round hole in an oblong pad)
    min_by("padAnnularRing", drilled.iter().filter(|p| f(&p["sizeX"]) > f(&p["drill"])).map(|p| ((f(&p["sizeX"]).min(f(&p["sizeY"])) - f(&p["drill"])) / 2.0, label(p))).collect());
    min_by("smdPad", pads.iter().filter(|p| f(&p["drill"]) == 0.0).map(|p| (f(&p["sizeX"]).min(f(&p["sizeY"])), label(p))).collect());
    if let Some((x0, y0, x1, y1)) = crate::geom::outline_bbox(state) {
        m.insert("boardSize".into(), json!({"mm": [r3(x1 - x0), r3(y1 - y0)]}));
        let _ = (x0, y0, x1, y1);
        if let Some((d, mut at)) = edge_distance(state) { at["mm"] = json!(d); m.insert("edgeClearance".into(), at); }
    }
    // drilled holes: the largest, and the closest pair edge to edge (via to via, and any pair with a pad hole)
    let mut holes: Vec<(f64, f64, f64, bool, Value)> = vias.iter().map(|v| (f(&v["position"]["x"]), f(&v["position"]["y"]), f(&v["drill"]), false, json!({"via": v["net"], "at": [v["position"]["x"], v["position"]["y"]]}))).collect();
    holes.extend(drilled.iter().map(|p| (f(&p["x"]), f(&p["y"]), f(&p["drill"]), true, label(p))));
    if let Some(h) = holes.iter().max_by(|a, b| a.2.partial_cmp(&b.2).unwrap()) { m.insert("largestDrill".into(), json!({"mm": r3(h.2), "at": h.4})); }
    holes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let (mut vv, mut pp): (Option<(f64, Value)>, Option<(f64, Value)>) = (None, None);
    for i in 0..holes.len() {
        for j in i + 1..holes.len() {
            let (a, b) = (&holes[i], &holes[j]);
            if b.0 - a.0 > 10.0 { break; }
            let d = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt() - (a.2 + b.2) / 2.0;
            if (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6 { continue; } // a via in a pad
            let slot = if a.3 || b.3 { &mut pp } else { &mut vv };
            if slot.as_ref().map(|s| d < s.0).unwrap_or(true) { *slot = Some((d, json!([a.4.clone(), b.4.clone()]))); }
        }
    }
    if let Some((d, at)) = vv { m.insert("holeToHoleVia".into(), json!({"mm": r3(d), "at": at})); }
    if let Some((d, at)) = pp { m.insert("holeToHolePad".into(), json!({"mm": r3(d), "at": at})); }
    m.insert("counts".into(), json!({"tracks": state["tracks"].as_array().map(|a| a.len()), "vias": vias.len(), "pads": pads.len(), "drilledPads": drilled.len()}));
    m
}

/// The measured values against the caller's limits; each limit names its feature (min*, or max for board and layers).
pub fn compare(meas: &Map<String, Value>, limits: &Map<String, Value>, clearance: Option<f64>, layers: Option<i64>) -> Vec<Value> {
    let mut out = Vec::new();
    let got = |k: &str| meas.get(k).and_then(|v| v["mm"].as_f64());
    for (limit, feature) in [("minTrack", "track"), ("minViaDrill", "viaDrill"), ("minViaDiameter", "viaDiameter"), ("minViaAnnularRing", "viaAnnularRing"),
                             ("minPadHole", "padHole"), ("minPadAnnularRing", "padAnnularRing"), ("minSmdPad", "smdPad"),
                             ("minEdgeClearance", "edgeClearance"), ("minHoleToHoleVia", "holeToHoleVia"), ("minHoleToHolePad", "holeToHolePad"),
                             ("minSilkLine", "silkLine"), ("minSilkText", "silkText")] {
        if let (Some(l), Some(v)) = (limits.get(limit).and_then(|x| x.as_f64()), got(feature)) {
            if v + 1e-9 < l { out.push(json!({"limit": limit, "limitMm": l, "boardMm": v, "at": meas[feature]["at"]})); }
        }
    }
    if let (Some(l), Some(c)) = (limits.get("minClearance").and_then(|x| x.as_f64()), clearance) {
        if c + 1e-9 < l { out.push(json!({"limit": "minClearance", "limitMm": l, "boardMm": c, "at": "the board's clearance rule"})); }
    }
    if let (Some(l), Some(n)) = (limits.get("maxLayers").and_then(|x| x.as_i64()), layers) {
        if n > l { out.push(json!({"limit": "maxLayers", "limit_": l, "board": n})); }
    }
    if let (Some(l), Some(v)) = (limits.get("maxDrill").and_then(|x| x.as_f64()), got("largestDrill")) {
        if v > l + 1e-9 { out.push(json!({"limit": "maxDrill", "limitMm": l, "boardMm": v, "at": meas["largestDrill"]["at"], "note": "larger holes are milled (routed) instead"})); }
    }
    if let (Some(lim), Some(sz)) = (limits.get("minBoardMm").and_then(|x| x.as_array()), meas.get("boardSize").and_then(|b| b["mm"].as_array())) {
        let (lw, lh) = (lim.first().and_then(|x| x.as_f64()).unwrap_or(0.0), lim.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0));
        let (w, h) = (sz[0].as_f64().unwrap_or(0.0), sz[1].as_f64().unwrap_or(0.0));
        if w.min(h) + 1e-9 < lw.min(lh) || w.max(h) + 1e-9 < lw.max(lh) { out.push(json!({"limit": "minBoardMm", "limitMm": [lw, lh], "boardMm": [w, h]})); }
    }
    if let (Some(lim), Some(sz)) = (limits.get("maxBoardMm").and_then(|x| x.as_array()), meas.get("boardSize").and_then(|b| b["mm"].as_array())) {
        let (lw, lh) = (lim.first().and_then(|x| x.as_f64()).unwrap_or(f64::MAX), lim.get(1).and_then(|x| x.as_f64()).unwrap_or(f64::MAX));
        let (w, h) = (sz[0].as_f64().unwrap_or(0.0), sz[1].as_f64().unwrap_or(0.0));
        // either orientation fits
        if !((w <= lw && h <= lh) || (w <= lh && h <= lw)) { out.push(json!({"limit": "maxBoardMm", "limitMm": [lw, lh], "boardMm": [w, h]})); }
    }
    out
}

/// A length written with its unit ("0.1mm", "4 mil", "0.004in", "100um") in mm; None without a known unit.
fn length_mm(text: &str) -> Option<f64> {
    let t = text.trim();
    let num: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let v: f64 = num.parse().ok()?;
    match t[num.len()..].trim().to_ascii_lowercase().as_str() {
        "mm" => Some(v),
        "mil" | "mils" | "th" => Some(v * 0.0254),
        "in" | "inch" => Some(v * 25.4),
        "um" | "\u{b5}m" => Some(v / 1000.0),
        _ => None,
    }
}

/// A clearance rule that names another object than copper of another net: the board edge, solder mask, silkscreen,
/// holes, keepouts, the courtyard, or the same net (Fusion's Solder-Mask Clearance is a mask expansion, often 0.05 mm).
const NOT_COPPER_CLEARANCE: &[&str] = &["dimension", "edge", "board", "mask", "silk", "paste", "hole", "restrict", "keepout", "keep-out",
                                        "courtyard", "component", "body", "plane", "same net", "samenet", "same-net"];

/// The smallest copper-to-copper clearance among the board's enabled rules, in mm. A rule gives it as a number
/// (`clearanceMm`, `gapMm` or `minMm`), as a `value` with its unit ("0.1mm": Fusion's form), or in a descriptor the tool
/// wrote ("Gap=10mil", "Gap=0.2mm": Altium's form). Clearance rules about anything but copper of another net are left out.
pub fn min_clearance(rules: &[Value]) -> Option<f64> {
    clearances(rules).into_iter().map(|c| c.0).reduce(f64::min).map(r3)
}

/// The copper clearance a route or a via must keep: the strictest board-wide copper rule (a pad-to-via 0.15 beside a
/// wire-to-wire 0.1 means 0.15 near a pad), so a plan passes the board's DRC. A rule scoped to some nets only (a high
/// voltage class) is left out unless no board-wide rule exists, then the smallest scoped one counts (fusion-lane #21).
pub fn routing_clearance(rules: &[Value]) -> Option<f64> {
    let all = clearances(rules);
    all.iter().filter(|c| c.1).map(|c| c.0).reduce(f64::max).or_else(|| all.iter().map(|c| c.0).reduce(f64::min)).map(r3)
}

/// Every enabled copper-to-copper clearance rule as (mm, board-wide).
fn clearances(rules: &[Value]) -> Vec<(f64, bool)> {
    let text = |r: &Value, k: &str| r[k].as_str().map(|s| s.to_lowercase()).unwrap_or_default();
    // board-wide: no scope, or Altium's All on both sides
    let wide = |r: &Value| ["scope1", "scope2", "scope"].iter().all(|k| r[*k].as_str().map(|s| s.is_empty() || s.eq_ignore_ascii_case("all")).unwrap_or(true));
    rules.iter().filter(|r| r["kind"].as_i64() == Some(0) || text(r, "kind").contains("clearance")
            || r["kindName"].as_str() == Some("Clearance") || text(r, "name").contains("clearance"))
        .filter(|r| !["kind", "name", "kindName"].iter().any(|k| { let t = text(r, k); NOT_COPPER_CLEARANCE.iter().any(|w| t.contains(w)) }))
        .filter(|r| r["enabled"].as_bool() != Some(false))
        .filter_map(|r| {
            let v = if let Some(v) = ["clearanceMm", "gapMm", "minMm"].iter().find_map(|k| r[*k].as_f64()) { v }
            else if let Some(v) = r["value"].as_str().and_then(length_mm) { v }
            else {
                let d = r["descriptor"].as_str()?;
                let i = d.find("Gap=")? + 4;
                let num: String = d[i..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                let v: f64 = num.parse().ok()?;
                let unit = &d[i + num.len()..];
                if unit.starts_with("mil") { v * 0.0254 } else { v }
            };
            Some((v, wide(r)))
        }).collect()
}

/// A pcb-design-rules record (one entry of `rules/records/<vendor>.json`'s `records`, with the file's `vendor`) as the
/// limits this check compares, plus what to say about where they came from. Limits the record does not give are left
/// out, never guessed. Pure.
pub fn limits_from_record(rec: &Value, vendor: &str) -> (Map<String, Value>, Value) {
    let mut l = Map::new();
    let mut put = |k: &str, v: Option<f64>| { if let Some(v) = v { l.insert(k.into(), json!(v)); } };
    put("minTrack", rec["minTraceWidth"]["mm"].as_f64());
    put("minClearance", rec["minCopperClearance"]["mm"].as_f64().or(rec["minSpacing"]["mm"].as_f64()));
    put("minViaDrill", rec["minVia"]["drillMm"].as_f64());
    put("minViaDiameter", rec["minVia"]["padMm"].as_f64());
    put("minPadHole", rec["minDrill"]["mm"].as_f64());
    // the record's annular ring is the plated-through-hole ring ("PTH ... absolute minimum")
    put("minPadAnnularRing", rec["annularRing"]["viaMm"].as_f64());
    put("minEdgeClearance", rec["copperToBoardEdge"]["mm"].as_f64());
    put("minHoleToHoleVia", rec["holeToHole"]["mm"].as_f64());
    put("minSilkLine", rec["silkLineWidth"]["mm"].as_f64());
    if let (Some(x), Some(y)) = (rec["maxBoardSize"]["xMm"].as_f64(), rec["maxBoardSize"]["yMm"].as_f64()) { l.insert("maxBoardMm".into(), json!([x, y])); }
    if let Some(n) = rec["layerCount"].as_i64() { l.insert("maxLayers".into(), json!(n)); }
    let src = rec["sources"].as_array().and_then(|s| s.first()).cloned().unwrap_or(Value::Null);
    let conditions: Map<String, Value> = rec.as_object().map(|o| o.iter().filter_map(|(k, v)| v.get("condition").map(|c| (k.clone(), c.clone()))).collect()).unwrap_or_default();
    let fab = json!({"name": vendor, "process": rec["process"], "layers": rec["layerCount"], "source": src["url"], "read": src["dateRead"],
                     "conditions": conditions, "notes": rec["notes"]});
    (l, fab)
}

/// fab_check: input {board, stack?, rules?, silk?, fab?, args: {limits?, copperOz?}}. `fab` is a pcb-design-rules
/// record (one of a vendor file's `records`, with `vendor` beside it or given as args.vendor); `limits` adds or
/// overrides any limit by name, in mm.
pub fn fab_check(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let mut meas = measure(&st);
    let rule_list = crate::input::rules(input);
    let clearance = min_clearance(&rule_list);
    if let Some(c) = clearance { meas.insert("clearanceRule".into(), json!({"mm": c})); }
    let stack = crate::input::stack(input);
    let layers = stack.as_ref().and_then(|s| s["copperLayers"].as_i64());
    if let Some(n) = layers { meas.insert("copperLayers".into(), json!(n)); }
    let mut limits = Map::new();
    let mut fab = Value::Null;
    if let Some(rec) = input.get("fab").filter(|r| !r.is_null()) {
        let vendor = rec["vendor"].as_str().or(args["vendor"].as_str()).unwrap_or("the fab").to_string();
        let (l, mut fv) = limits_from_record(rec, &vendor);
        if let (Some(want), Some(n)) = (rec["layerCount"].as_i64(), layers) {
            if want != n { fv["mismatch"] = json!(format!("the record is for {want} layers; the board has {n}: pass the {n}-layer record")); }
        }
        // the record's limits hold for its stated copper (1 oz outer as a rule); heavier copper needs wider tracks
        let outer_um = stack.as_ref().and_then(|s| s["layers"][0]["copperUm"].as_f64()).unwrap_or(35.0);
        let oz = args["copperOz"].as_f64().unwrap_or(outer_um / 35.0);
        fv["copperOz"] = json!(r3(oz));
        if oz >= 1.75 { fv["copperNote"] = json!("the board's outer copper is heavier than the record's 1 oz condition: its notes give the wider track and gap limits for heavier copper; pass them as limits"); }
        limits = l;
        fab = fv;
    }
    for (k, v) in args["limits"].as_object().cloned().unwrap_or_default() { limits.insert(k, v); }
    // silkscreen sizes from silk_state, only when a limit asks and the input has it
    if limits.contains_key("minSilkText") || limits.contains_key("minSilkLine") {
        if let Some(silk) = crate::input::part(input, "silk") {
            let mut texts: Vec<(f64, Value)> = silk["texts"].as_array().cloned().unwrap_or_default().iter().filter(|t| t["side"] != "other").map(|t| (f(&t["size"]), json!({"text": t["text"], "at": [t["x"], t["y"]]}))).collect();
            for c in silk["components"].as_array().cloned().unwrap_or_default() {
                for k in ["name", "comment"] { if c[k]["shown"].as_bool() == Some(true) { texts.push((f(&c[k]["size"]), json!({"ref": c["ref"], "text": c[k]["text"], "at": [c[k]["x"], c[k]["y"]]}))); } }
            }
            if let Some((v, at)) = texts.into_iter().filter(|t| t.0 > 0.0).min_by(|a, b| a.0.partial_cmp(&b.0).unwrap()) { meas.insert("silkText".into(), json!({"mm": r3(v), "at": at})); }
            if let Some(l) = silk["lines"].as_array().cloned().unwrap_or_default().iter().filter(|l| l["side"] != "other" && f(&l["width"]) > 0.0).min_by(|a, b| f(&a["width"]).partial_cmp(&f(&b["width"])).unwrap()) {
                meas.insert("silkLine".into(), json!({"mm": r3(f(&l["width"])), "at": [l["x1"], l["y1"]]}));
            }
        }
    }
    let mut fails = compare(&meas, &limits, clearance, layers);
    // each finding names where its limit came from
    if !fab.is_null() { for x in fails.iter_mut() { x["source"] = json!({"name": fab["name"], "url": fab["source"], "read": fab["read"]}); } }
    let unseen: Vec<&str> = [("minClearance", clearance.is_some()), ("minSilkText", meas.contains_key("silkText")), ("minSilkLine", meas.contains_key("silkLine")), ("maxLayers", layers.is_some())]
        .iter().filter(|(k, seen)| limits.contains_key(*k) && !seen).map(|(k, _)| *k).collect();
    json!({"success": true, "fab": fab, "measured": meas, "limitsChecked": limits.keys().collect::<Vec<_>>(), "outsideLimits": fails,
        "notChecked": unseen, "fits": if limits.is_empty() { Value::Null } else { json!(fails.is_empty()) },
        "costlierOrBelowRecommended": [],
        "_hint": if limits.is_empty() { "Measured only. Pass a fab's record from adom/pcb-design-rules as fab, or limits:{minTrack, minClearance, minViaDrill, minViaDiameter, minViaAnnularRing, minPadHole, minPadAnnularRing, minSmdPad, minEdgeClearance, minHoleToHoleVia, minHoleToHolePad, maxDrill, minSilkLine, minSilkText, maxLayers, minBoardMm:[w,h], maxBoardMm:[w,h]} in mm, to check them." }
                 else { "outsideLimits names each feature below the fab's limit, where it is, and the page the limit came from. notChecked lists limits the input gave nothing to measure against (pass rules, stack or silk). Read fab.notes: costlier options and recommended sizes are there. Fix the board (set_track_width, add_rule, a footprint change) or choose the fab's advanced service; then run run_drc." }})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Value {
        json!({"tracks": [{"width": 0.25, "net": "A", "layer": "F.Cu", "start": {"x": 0.0, "y": 0.0}}, {"width": 0.1, "net": "B", "layer": "B.Cu", "start": {"x": 5.0, "y": 5.0}}, {"width": 0.05, "net": "", "layer": "L57", "start": {"x": 0.0, "y": 0.0}}],
               "vias": [{"size": 0.6, "drill": 0.3, "net": "GND", "position": {"x": 1.0, "y": 1.0}}, {"size": 0.45, "drill": 0.25, "net": "B", "position": {"x": 2.0, "y": 2.0}}],
               "pads": [{"ref": "J1", "name": "1", "x": 0.0, "y": 0.0, "sizeX": 1.7, "sizeY": 1.7, "drill": 1.0}, {"ref": "U1", "name": "3", "x": 3.0, "y": 0.0, "sizeX": 0.3, "sizeY": 1.0, "drill": 0.0}],
               "outline": [[0, 0], [50, 0], [50, 30], [0, 30]]})
    }

    #[test]
    fn the_smallest_features_are_found_with_where_they_are() {
        let m = measure(&board());
        assert_eq!(m["track"]["mm"], json!(0.1));
        assert_eq!(m["track"]["at"]["net"], json!("B"));
        assert_eq!((m["viaDrill"]["mm"].clone(), m["viaDiameter"]["mm"].clone(), m["viaAnnularRing"]["mm"].clone()), (json!(0.25), json!(0.45), json!(0.1)));
        assert_eq!((m["padHole"]["mm"].clone(), m["padAnnularRing"]["mm"].clone(), m["smdPad"]["mm"].clone()), (json!(1.0), json!(0.35), json!(0.3)));
        assert_eq!(m["boardSize"]["mm"], json!([50.0, 30.0]));
    }

    #[test]
    fn arc_edges_count_toward_the_board_size() {
        let b = json!({"outline": [[0, 0], [10, 0]], "outlineEdges": [{"kind": "arc", "center": [5.0, 0.0], "radius": 5.0, "angle1": 0.0, "angle2": 180.0}]});
        assert_eq!(measure(&b)["boardSize"]["mm"], json!([10.0, 5.0]));
    }

    #[test]
    fn limits_name_what_falls_short_and_a_board_fits_either_way_round() {
        let m = measure(&board());
        let lim = json!({"minTrack": 0.127, "minViaDrill": 0.3, "maxBoardMm": [30, 60], "maxLayers": 2, "minClearance": 0.127});
        let f = compare(&m, lim.as_object().unwrap(), Some(0.1), Some(2));
        let names: Vec<&str> = f.iter().map(|x| x["limit"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["minTrack", "minViaDrill", "minClearance"]);
    }

    #[test]
    fn clearance_is_read_from_rule_descriptors_in_mil_or_mm() {
        let rules = vec![json!({"kind": 0, "descriptor": "Clearance Constraint (Gap=10mil) (All),(All)", "enabled": true}),
                         json!({"kind": 0, "descriptor": "Clearance Constraint (Gap=0.15mm) (InNet('SW')),(All)", "enabled": true}),
                         json!({"kind": 0, "descriptor": "Clearance Constraint (Gap=0.05mm) (All),(All)", "enabled": false})];
        assert_eq!(min_clearance(&rules), Some(0.15));
    }
    /// JLCPCB's 2-layer record as adom/pcb-design-rules gives it (rules/records/jlcpcb.json, read 2026-09-03), trimmed
    fn jlc_2l() -> Value {
        json!({"vendor": "JLCPCB", "process": "standard", "layerCount": 2, "minTraceWidth": {"mm": 0.1, "condition": "1 oz copper outer layer"},
               "minSpacing": {"mm": 0.1}, "minDrill": {"mm": 0.15}, "minVia": {"drillMm": 0.15, "padMm": 0.25}, "annularRing": {"viaMm": 0.18},
               "copperToBoardEdge": {"mm": 0.2}, "holeToHole": {"mm": 0.2}, "silkLineWidth": {"mm": 0.15}, "minCopperClearance": {"mm": 0.1},
               "maxBoardSize": {"xMm": 670.0, "yMm": 600.0}, "notes": "Min drill 0.15 mm costs more.",
               "sources": [{"url": "https://jlcpcb.com/capabilities/pcb-capabilities", "dateRead": "2026-09-03"}]})
    }

    #[test]
    fn a_pcb_design_rules_record_becomes_limits_with_its_source() {
        let (l, fab) = limits_from_record(&jlc_2l(), "JLCPCB");
        assert_eq!((l["minTrack"].clone(), l["minClearance"].clone(), l["minPadAnnularRing"].clone()), (json!(0.1), json!(0.1), json!(0.18)));
        assert_eq!(l["maxBoardMm"], json!([670.0, 600.0]));
        assert!(!l.contains_key("minSilkText"), "a limit the record does not give is not guessed");
        assert_eq!((fab["source"].as_str(), fab["read"].as_str()), (Some("https://jlcpcb.com/capabilities/pcb-capabilities"), Some("2026-09-03")));
        // a 0.08 mm track fails, holes 0.15 mm apart edge to edge fail the via rule; each finding names the page
        let st = json!({"tracks": [{"width": 0.08, "net": "A", "layer": "F.Cu", "start": {"x": 1.0, "y": 1.0}}],
                        "vias": [{"drill": 0.3, "size": 0.6, "net": "A", "position": {"x": 0.0, "y": 0.0}}, {"drill": 0.3, "size": 0.6, "net": "B", "position": {"x": 0.45, "y": 0.0}}],
                        "outline": [[-5, -5], [5, -5], [5, 5], [-5, 5]]});
        let r = fab_check(&json!({"board": st, "stack": {"copperLayers": 2, "layers": [{"copperUm": 35}, {"copperUm": 35}]}, "fab": jlc_2l()}));
        let fails: Vec<&str> = r["outsideLimits"].as_array().unwrap().iter().map(|f| f["limit"].as_str().unwrap()).collect();
        assert!(fails.contains(&"minTrack") && fails.contains(&"minHoleToHoleVia"), "{fails:?}");
        assert_eq!(r["outsideLimits"][0]["source"]["read"], "2026-09-03");
        assert_eq!(r["fits"], json!(false));
        // no rules in the input: the clearance limit is reported as not checked, never passed
        assert!(r["notChecked"].as_array().unwrap().contains(&json!("minClearance")));
    }

    #[test]
    fn clearance_from_a_standard_rule() {
        assert_eq!(min_clearance(&[json!({"kind": "clearance", "clearanceMm": 0.127}), json!({"kind": "clearance", "gapMm": 0.2})]), Some(0.127));
    }

    #[test]
    fn clearance_from_a_value_with_its_unit_and_never_same_net() {
        // Fusion writes rules as a kind name and a value with its unit
        let rules = vec![json!({"kind": "Copper Clearance", "value": "0.15mm"}), json!({"kind": "Copper Clearance", "value": "5mil"}),
                         json!({"kind": "Same Net Clearance", "value": "0.05mm"}), json!({"kind": "Copper Clearance", "value": "0.2"})];
        assert_eq!(min_clearance(&rules), Some(0.127));
        assert_eq!(length_mm("0.004in"), Some(0.1016));
        assert_eq!(length_mm("100um"), Some(0.1));
        assert_eq!(length_mm("0.2"), None, "a bare number has no unit: never guessed");
    }

    #[test]
    fn only_copper_to_copper_clearance_counts() {
        // Fusion's rule set (#13): only Copper Clearance is the clearance between nets
        let fusion = vec![json!({"kind": "Copper Clearance", "value": "0.1mm"}), json!({"kind": "Dimension Clearance", "value": "0.3mm"}),
                          json!({"kind": "Solder-Mask Clearance", "value": "0.05mm"}), json!({"kind": "Restrict Clearance", "value": "0.02mm"}),
                          json!({"kind": "Via Restrict Clearance", "value": "0.02mm"}), json!({"kind": "Keepout Clearance", "value": "0.01mm"})];
        assert_eq!(min_clearance(&fusion), Some(0.1));
        // Altium names its other clearances by object too
        let altium = vec![json!({"kind": 0, "name": "Clearance", "descriptor": "Clearance Constraint (Gap=0.2mm) (All),(All)"}),
                          json!({"kind": 17, "name": "HoleToHoleClearance", "descriptor": "Gap=0.1mm"}),
                          json!({"kind": 20, "name": "SilkToSolderMaskClearance", "descriptor": "Gap=0.05mm"})];
        assert_eq!(min_clearance(&altium), Some(0.2));
    }
}

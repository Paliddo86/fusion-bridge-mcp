//! The numbers a fab quote needs, from the board model (size, copper layers, thickness, smallest track, gap and drill,
//! holes, parts and joints), and a rough price table at the quantities asked. From the KiCad bridge's fab.rs, with one
//! change: the engine has no rates of its own. Prices come in as a record read from the fab's own page with its date
//! (`prices`), and without one the reply is the numbers only. Pure.
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r2(v: f64) -> f64 { (v * 100.0).round() / 100.0 }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }

/// The board's numbers. `layers` from the stack when given, else the copper layers the board uses; the thickness from
/// the stack only.
pub fn board_numbers(st: &Value, stack: Option<&Value>, rules: &[Value]) -> Value {
    let (w, h) = crate::geom::outline_bbox(st).map(|(x0, y0, x1, y1)| (x1 - x0, y1 - y0)).unwrap_or((0.0, 0.0));
    let mut used: BTreeSet<String> = BTreeSet::new();
    for k in ["tracks", "arcs", "pads", "polygons"] {
        for x in st[k].as_array().into_iter().flatten() {
            if let Some(l) = x["layer"].as_str().filter(|l| l.ends_with(".Cu") && *l != "*.Cu") { used.insert(l.to_string()); }
        }
    }
    let layers = stack.and_then(|s| s["copperLayers"].as_u64()).unwrap_or((used.len() as u64).max(2));
    let thickness = stack.and_then(|s| s["totalUm"].as_f64()).filter(|t| *t > 0.0).map(|t| r3(t / 1000.0));
    let m = crate::fab::measure(st);
    let mm = |k: &str| m.get(k).and_then(|v| v["mm"].as_f64());
    let min_drill = [mm("viaDrill"), mm("padHole")].into_iter().flatten().fold(None, |a: Option<f64>, b| Some(a.map(|x| x.min(b)).unwrap_or(b)));
    let pads = st["pads"].as_array().cloned().unwrap_or_default();
    let vias = st["vias"].as_array().map(|a| a.len()).unwrap_or(0);
    let drilled = pads.iter().filter(|p| f(&p["drill"]) > 0.0).count();
    // a part is a component with pads (a logo or a drawing has none); through-hole when any pad is drilled
    let (mut smd, mut tht, mut smd_j, mut tht_j, mut sides, mut unique) = (0, 0, 0, 0, BTreeSet::new(), BTreeSet::new());
    for c in st["components"].as_array().into_iter().flatten() {
        let r = c["ref"].as_str().unwrap_or("");
        let own: Vec<&Value> = pads.iter().filter(|p| p["ref"].as_str() == Some(r)).collect();
        if own.is_empty() { continue; }
        let holes = own.iter().filter(|p| f(&p["drill"]) > 0.0).count();
        if holes > 0 { tht += 1; } else { smd += 1; }
        smd_j += own.len() - holes;
        tht_j += holes;
        sides.insert(c["side"].as_str().unwrap_or("top").to_string());
        unique.insert(c["pattern"].as_str().unwrap_or(r).to_string());
    }
    json!({"widthMm": r3(w), "heightMm": r3(h), "areaMm2": r2(w * h), "layers": layers, "thicknessMm": thickness,
        "minTrackMm": mm("track"), "minGapMm": crate::fab::min_clearance(rules), "minDrillMm": min_drill,
        "holes": vias + drilled, "vias": vias, "drilledPads": drilled, "smdParts": smd, "thtParts": tht, "totalParts": smd + tht,
        "smdJoints": smd_j, "thtJoints": tht_j, "uniqueParts": unique.len(), "assemblySides": sides.into_iter().collect::<Vec<_>>()})
}

/// The price table from a price record: `{vendor, url, read, currency, pcb: [{layers, setup, perM2, promo?: {maxMm: [w,
/// h], prices: [[qty, price]]}}], thickness?: {standardMm, factor}, assembly?: {setupPerSide, stencilPerSide, perSmdJoint,
/// perThtJoint, perExtendedPart}, excludes?: [..]}`. The pcb row is the one for the fewest copper layers that still
/// covers the board.
pub fn price_table(n: &Value, prices: &Value, quantities: &[u64], assembly: bool, basic_parts: Option<u64>) -> Result<Value, String> {
    let layers = n["layers"].as_u64().unwrap_or(2);
    let rows = prices["pcb"].as_array().ok_or("the price record has no pcb rows")?;
    let row = rows.iter().filter(|r| r["layers"].as_u64().map(|l| l >= layers).unwrap_or(false)).min_by_key(|r| r["layers"].as_u64())
        .ok_or_else(|| format!("the price record has no row for {layers} copper layers"))?;
    let (setup, per_m2) = (f(&row["setup"]), f(&row["perM2"]));
    let (w, h) = (f(&n["widthMm"]), f(&n["heightMm"]));
    let area_m2 = w * h / 1_000_000.0;
    let promo = row.get("promo").filter(|p| {
        let max = (f(&p["maxMm"][0]), f(&p["maxMm"][1]));
        (w <= max.0 && h <= max.1) || (w <= max.1 && h <= max.0)
    });
    let factor = match (prices.get("thickness"), n["thicknessMm"].as_f64()) {
        (Some(t), Some(mm)) if (mm - f(&t["standardMm"])).abs() > 0.05 => f(&t["factor"]).max(1.0),
        _ => 1.0,
    };
    let a = &prices["assembly"];
    if assembly && a.is_null() { return Err("assembly was asked for, but the price record has no assembly rates".into()); }
    let sides = n["assemblySides"].as_array().map(|s| s.len()).unwrap_or(1).max(1) as f64;
    let unique = n["uniqueParts"].as_u64().unwrap_or(0);
    let extended = basic_parts.map(|b| unique.saturating_sub(b)).unwrap_or(unique);
    let mut out = Vec::new();
    for &q in quantities {
        let qf = q as f64;
        // the promotional price for the smallest quantity it covers
        let promo_price = promo.and_then(|p| p["prices"].as_array().and_then(|ps| ps.iter().filter(|e| e[0].as_u64().map(|x| x >= q).unwrap_or(false)).min_by_key(|e| e[0].as_u64()).map(|e| f(&e[1]))));
        let pcb = r2(promo_price.unwrap_or(setup + per_m2 * area_m2 * qf) * factor);
        let mut r = json!({"quantity": q, "board": pcb});
        let mut total = pcb;
        if assembly && n["totalParts"].as_u64().unwrap_or(0) > 0 {
            let asm = r2((f(&a["setupPerSide"]) + f(&a["stencilPerSide"])) * sides + (f(&n["smdJoints"]) * f(&a["perSmdJoint"]) + f(&n["thtJoints"]) * f(&a["perThtJoint"])) * qf + f(&a["perExtendedPart"]) * extended as f64);
            r["assembly"] = json!(asm);
            total += asm;
        }
        r["total"] = json!(r2(total));
        r["perBoard"] = json!(r2(total / qf.max(1.0)));
        r["currency"] = prices["currency"].clone();
        out.push(r);
    }
    let mut assumptions = vec![format!("Board: {setup} setup plus {per_m2} per square metre of boards, from the {}-layer row.", row["layers"])];
    if let Some(p) = promo { assumptions.push(format!("Boards up to {} x {} mm take the promotional prices {} where they cover the quantity.", p["maxMm"][0], p["maxMm"][1], p["prices"])); }
    if factor > 1.0 { assumptions.push(format!("A thickness other than {} mm multiplies the board price by {factor}.", prices["thickness"]["standardMm"])); }
    if assembly {
        assumptions.push(format!("Assembly: setup and stencil per assembled side ({sides} side(s)), per joint, and a loading fee per extended part ({extended} of {unique} unique parts counted as extended{}).", if basic_parts.is_some() { "" } else { "; pass basicParts to count the basic ones" }));
        assumptions.push("Component cost is not included: price the BOM with adom-parts-search.".into());
    }
    for e in prices["excludes"].as_array().into_iter().flatten() { if let Some(s) = e.as_str() { assumptions.push(format!("Not included: {s}.")); } }
    Ok(json!({"prices": out, "assumptions": assumptions}))
}

/// estimate_cost: input {board, stack?, rules?, prices?, args: {quantities?, assembly?, basicParts?}}.
pub fn estimate_cost(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let quantities: Vec<u64> = args["quantities"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64()).filter(|q| *q > 0).collect()).unwrap_or_else(|| vec![5, 10, 30, 100]);
    if quantities.is_empty() { return crate::input::refuse("bad_args", "quantities are whole numbers above 0, e.g. [5, 10, 30, 100]"); }
    let assembly = args["assembly"].as_bool().unwrap_or(false);
    let stack = crate::input::stack(input);
    let n = board_numbers(&st, stack.as_ref(), &crate::input::rules(input));
    let mut notes = Vec::new();
    if n["thicknessMm"].is_null() { notes.push("thicknessMm: no stack in the input, so the thickness is unknown and no thickness surcharge is applied"); }
    if n["minGapMm"].is_null() { notes.push("minGapMm: the board's clearance rule, and no rules came in the input"); }
    let Some(prices) = crate::input::part(input, "prices") else {
        return json!({"success": true, "board": n, "prices": Value::Null, "source": Value::Null, "notes": notes, "quantities": quantities,
            "_hint": "The board's numbers only: the engine carries no rates. Pass a price record as prices ({vendor, url, read, currency, pcb: [{layers, setup, perM2, promo?}], thickness?, assembly?, excludes?}), read from the fab's own page with the date, and the reply prices these quantities from it."});
    };
    match price_table(&n, &prices, &quantities, assembly, args["basicParts"].as_u64()) {
        Ok(t) => json!({"success": true, "board": n, "prices": t["prices"], "assumptions": t["assumptions"], "notes": notes, "rough": true,
            "source": {"name": prices["vendor"], "url": prices["url"], "read": prices["read"]},
            "_hint": "A rough estimate from the price record named in source, as of its read date: get a real quote from the fab before ordering."}),
        Err(e) => crate::input::refuse("bad_prices", &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Value {
        json!({"outline": [[0, 0], [50, 0], [50, 40], [0, 40]],
            "tracks": [{"layer": "F.Cu", "width": 0.2, "net": "A", "start": {"x": 1, "y": 1}, "end": {"x": 5, "y": 1}}, {"layer": "B.Cu", "width": 0.15, "net": "A", "start": {"x": 1, "y": 2}, "end": {"x": 5, "y": 2}}],
            "vias": [{"drill": 0.3, "size": 0.6, "net": "A", "position": {"x": 5, "y": 1.5}}],
            "components": [{"ref": "U1", "side": "top", "pattern": "SOIC8"}, {"ref": "J1", "side": "top", "pattern": "HDR2"}, {"ref": "C1", "side": "bottom", "pattern": "0603"}, {"ref": "LOGO", "side": "top", "pattern": "LOGO"}],
            "pads": [{"ref": "U1", "drill": 0, "layer": "F.Cu"}, {"ref": "U1", "drill": 0, "layer": "F.Cu"}, {"ref": "J1", "drill": 1.0, "layer": "*.Cu", "x": 10, "y": 10, "sizeX": 1.7, "sizeY": 1.7},
                     {"ref": "J1", "drill": 1.0, "layer": "*.Cu", "x": 12.54, "y": 10, "sizeX": 1.7, "sizeY": 1.7}, {"ref": "C1", "drill": 0, "layer": "B.Cu"}, {"ref": "C1", "drill": 0, "layer": "B.Cu"}]})
    }

    fn record() -> Value {
        json!({"vendor": "Example Fab", "url": "https://fab.example/prices", "read": "2026-10-29", "currency": "USD",
            "pcb": [{"layers": 2, "setup": 2.0, "perM2": 30.0, "promo": {"maxMm": [100, 100], "prices": [[5, 2.0], [10, 5.0]]}}, {"layers": 4, "setup": 8.0, "perM2": 90.0}],
            "thickness": {"standardMm": 1.6, "factor": 1.15},
            "assembly": {"setupPerSide": 8.0, "stencilPerSide": 7.0, "perSmdJoint": 0.0017, "perThtJoint": 0.0173, "perExtendedPart": 3.0},
            "excludes": ["shipping", "tax"]})
    }

    #[test]
    fn the_numbers_come_from_the_board() {
        let n = board_numbers(&board(), None, &[json!({"kind": "clearance", "clearanceMm": 0.15})]);
        assert_eq!((n["widthMm"].as_f64(), n["heightMm"].as_f64(), n["layers"].as_u64()), (Some(50.0), Some(40.0), Some(2)));
        assert_eq!((n["minTrackMm"].as_f64(), n["minGapMm"].as_f64(), n["minDrillMm"].as_f64()), (Some(0.15), Some(0.15), Some(0.3)));
        // the logo has no pads: three parts, J1 through-hole, two sides
        assert_eq!((n["smdParts"].as_u64(), n["thtParts"].as_u64(), n["smdJoints"].as_u64(), n["thtJoints"].as_u64(), n["holes"].as_u64()), (Some(2), Some(1), Some(4), Some(2), Some(3)));
        assert_eq!(n["assemblySides"], json!(["bottom", "top"]));
        assert!(n["thicknessMm"].is_null());
        assert_eq!(board_numbers(&board(), Some(&json!({"copperLayers": 4, "totalUm": 1600})), &[])["thicknessMm"], json!(1.6));
    }

    #[test]
    fn without_a_price_record_it_answers_the_numbers_only() {
        let r = estimate_cost(&json!({"board": board(), "args": {}}));
        assert_eq!(r["success"], json!(true));
        assert!(r["prices"].is_null() && r["source"].is_null());
        assert_eq!(r["board"]["layers"], json!(2));
    }

    #[test]
    fn prices_come_from_the_record_and_name_it() {
        let r = estimate_cost(&json!({"board": board(), "prices": record(), "args": {"quantities": [5, 30], "assembly": true}}));
        assert_eq!(r["source"], json!({"name": "Example Fab", "url": "https://fab.example/prices", "read": "2026-10-29"}));
        // 5 boards: the promotional 2.00; 30: 2 + 30 * 0.002 m2 * 30 = 3.80
        assert_eq!((r["prices"][0]["board"].as_f64(), r["prices"][1]["board"].as_f64()), (Some(2.0), Some(3.8)), "{r}");
        // assembly at 5: (8 + 7) * 2 sides + (4 * 0.0017 + 2 * 0.0173) * 5 + 3 * 3 unique = 39.21
        assert_eq!(r["prices"][0]["assembly"].as_f64(), Some(39.21), "{r}");
        assert!(r["assumptions"].as_array().unwrap().iter().any(|a| a == "Not included: shipping."));
        // a 4-layer stack takes the 4-layer row; 6 layers has none
        let four = estimate_cost(&json!({"board": board(), "stack": {"copperLayers": 4, "totalUm": 1200}, "prices": record(), "args": {"quantities": [10]}}));
        assert_eq!(four["prices"][0]["board"].as_f64(), Some(r2((8.0 + 90.0 * 0.002 * 10.0) * 1.15)), "{four}");
        assert_eq!(estimate_cost(&json!({"board": board(), "stack": {"copperLayers": 6}, "prices": record(), "args": {}}))["errorCode"], "bad_prices");
    }
}

//! Manufacturing panels: a step-and-repeat of a board with rails, tooling holes and fiducials, and how the boards come
//! apart: V-score lines or mouse-bite tabs drawn the way JLCPCB reads them, with JLCPCB's rules checked
//! (jlcpcb.com/blog/v-cut-panelization-standards, /blog/mouse-bite-panelization-guide, read 2026-10-23). From the Altium
//! bridge's panel.rs: the engine answers the plan in panel mm from the panel's lower-left corner, and each bridge builds
//! the panel in its own way (Altium an embedded board array, Fusion an EAGLE board with the copies placed).
use serde_json::{json, Value};

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }

/// A mouse-bite tab: five 0.6 mm holes 0.4 mm apart edge to edge (JLCPCB: 5 to 8 a set, 0.35 to 0.4 mm).
const BITE_HOLE: f64 = 0.6;
const BITE_PITCH: f64 = 1.0;
const BITES: i64 = 5;
const TAB: f64 = 5.0;

/// The panel in mm from its lower-left corner: its size, each board's outline corner, tooling holes and fiducials.
/// `edge` is the routed space between the rails and the boards (tabs); 0 for V-score.
pub fn panel_layout(w: f64, h: f64, rows: i64, cols: i64, gap: f64, rail: f64, edge: f64) -> Value {
    let m = rail + edge;
    let pw = 2.0 * m + cols as f64 * w + (cols - 1) as f64 * gap;
    let ph = 2.0 * m + rows as f64 * h + (rows - 1) as f64 * gap;
    let mut boards = Vec::new();
    for r in 0..rows { for c in 0..cols { boards.push(json!([r3(m + c as f64 * (w + gap)), r3(m + r as f64 * (h + gap))])); } }
    // rails 5 mm or wider carry 3 tooling holes (the fourth corner left bare so the panel only loads one way) and
    // 3 fiducials, each 5 mm in from its hole
    let (mut holes, mut fids) = (Vec::new(), Vec::new());
    if rail >= 5.0 {
        let k = rail / 2.0;
        for (x, y) in [(k, k), (pw - k, k), (k, ph - k)] { holes.push(json!([r3(x), r3(y)])); }
        for (x, y) in [(k + 5.0, k), (pw - k - 5.0, k), (k + 5.0, ph - k)] { fids.push(json!([r3(x), r3(y)])); }
    }
    json!({"size": [r3(pw), r3(ph)], "boards": boards, "toolingHoles": holes, "fiducials": fids})
}

/// V-score: the x of every vertical cut and the y of every horizontal one (boards edge to edge, gap 0). With rails the
/// panel's own edges are milled, not cut, but the rails' inner edges are cuts.
pub fn vcut_lines(w: f64, h: f64, rows: i64, cols: i64, rail: f64) -> (Vec<f64>, Vec<f64>) {
    let keep = |i: i64, n: i64| rail > 0.0 || (i != 0 && i != n);
    ((0..=cols).filter(|c| keep(*c, cols)).map(|c| r3(rail + c as f64 * w)).collect(),
     (0..=rows).filter(|r| keep(*r, rows)).map(|r| r3(rail + r as f64 * h)).collect())
}

/// Each cut as a line across the whole panel (JLCPCB: a V-cut runs edge to edge, no jumps), with "V-CUT" written at
/// both ends: in the rails (their inner half, clear of the tooling holes and fiducials at rail/2), or just outside the
/// panel without them. A label near the far edge goes on the other side of its line. Texts are (x, y, rotation).
pub fn vcut_marks(xs: &[f64], ys: &[f64], pw: f64, ph: f64, rail: f64) -> (Vec<[f64; 4]>, Vec<(f64, f64, i64)>) {
    const LEN: f64 = 5.0; // "V-CUT" at 1 mm high
    let (mut lines, mut texts) = (Vec::new(), Vec::new());
    let inside = rail >= 3.0;
    let beside = |p: f64, span: f64| if p + 0.4 + LEN > span - 0.5 { p - 0.4 - LEN } else { p + 0.4 };
    for &x in xs {
        lines.push([x, 0.0, x, ph]);
        let (lo, hi) = if inside { (rail - 1.3, ph - rail + 0.3) } else { (-2.0, ph + 1.0) };
        texts.push((r3(beside(x, pw)), r3(lo), 0));
        texts.push((r3(beside(x, pw)), r3(hi), 0));
    }
    for &y in ys {
        lines.push([0.0, y, pw, y]);
        // turned 90 degrees a text grows up from its anchor and its letters lie to the anchor's left
        let (lo, hi) = if inside { (rail - 0.3, pw - rail + 1.3) } else { (-1.0, pw + 2.0) };
        texts.push((r3(lo), r3(beside(y, ph)), 90));
        texts.push((r3(hi), r3(beside(y, ph)), 90));
    }
    (lines, texts)
}

fn copper(l: &Value) -> bool { l.as_str().map(|s| s.ends_with(".Cu")).unwrap_or(false) }

/// The nearest copper to the outline's bounding box (mm, with where), and copper whose centre lies outside it (the
/// panel would repeat that into the rails and the neighbours).
pub fn edge_clearance(state: &Value, bb: (f64, f64, f64, f64)) -> (Option<(f64, Value)>, Vec<Value>) {
    let (x0, y0, x1, y1) = bb;
    let mut pts: Vec<(f64, f64, f64, Value)> = Vec::new();
    let arr = |k: &str| state[k].as_array().cloned().unwrap_or_default();
    for t in arr("tracks").iter().filter(|t| copper(&t["layer"])) {
        for p in [&t["start"], &t["end"]] { pts.push((f(&p["x"]), f(&p["y"]), f(&t["width"]) / 2.0, json!({"track": t["net"], "layer": t["layer"]}))); }
    }
    for a in arr("arcs").iter().filter(|a| copper(&a["layer"])) {
        let (cx, cy, r) = (f(&a["center"]["x"]), f(&a["center"]["y"]), f(&a["radius"]));
        let (s, mut e) = (f(&a["startAngle"]), f(&a["endAngle"]));
        if e <= s { e += 360.0; }
        for k in 0..=8 {
            let t = (s + (e - s) * k as f64 / 8.0).to_radians();
            pts.push((cx + r * t.cos(), cy + r * t.sin(), f(&a["width"]) / 2.0, json!({"arc": a["net"], "layer": a["layer"]})));
        }
    }
    for v in arr("vias") { pts.push((f(&v["position"]["x"]), f(&v["position"]["y"]), f(&v["size"]) / 2.0, json!({"via": v["net"]}))); }
    for p in arr("pads") {
        let (rf, nm) = (p["ref"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or(""));
        pts.push((f(&p["x"]), f(&p["y"]), f(&p["sizeX"]).max(f(&p["sizeY"])) / 2.0, json!({"pad": if rf.is_empty() { nm.to_string() } else { format!("{rf}.{nm}") }})));
    }
    for g in arr("polygons").iter().filter(|g| copper(&g["layer"])) {
        for p in g["points"].as_array().cloned().unwrap_or_default() { pts.push((f(&p[0]), f(&p[1]), 0.0, json!({"pour": g["net"], "layer": g["layer"]}))); }
    }
    let mut near: Option<(f64, Value)> = None;
    let mut outside = Vec::new();
    for (x, y, half, what) in pts {
        let inside = x >= x0 && x <= x1 && y >= y0 && y <= y1;
        if !inside {
            if outside.len() < 10 { let mut o = what.clone(); o["at"] = json!([r3(x), r3(y)]); outside.push(o); }
            continue;
        }
        let d = (x - x0).min(x1 - x).min(y - y0).min(y1 - y) - half;
        if near.as_ref().map(|n| d < n.0).unwrap_or(true) { let mut o = what; o["at"] = json!([r3(x), r3(y)]); near = Some((r3(d), o)); }
    }
    (near, outside)
}

/// Does the board draw a line of its outline on a mechanical layer? Every copy in the array brings it along closed,
/// and a router follows it all the way round.
pub fn mech_outline(state: &Value, bb: (f64, f64, f64, f64)) -> bool {
    let (x0, y0, x1, y1) = bb;
    let on = |x: f64, y: f64| (x - x0).abs() < 0.05 || (x - x1).abs() < 0.05 || (y - y0).abs() < 0.05 || (y - y1).abs() < 0.05;
    let mech = |l: &Value| l.as_str().and_then(|s| s.strip_prefix('L')).and_then(|n| n.parse::<i64>().ok()).map(|n| (57..=72).contains(&n)).unwrap_or(false);
    state["tracks"].as_array().map(|ts| ts.iter().any(|t| mech(&t["layer"]) && on(f(&t["start"]["x"]), f(&t["start"]["y"])) && on(f(&t["end"]["x"]), f(&t["end"]["y"])))).unwrap_or(false)
}

fn rule(name: &str, ok: Option<bool>, detail: String) -> Value { json!({"rule": name, "ok": ok, "detail": detail}) }

/// JLCPCB's V-cut rules against this panel. `ok: null` is a note, not a pass or a fail.
#[allow(clippy::too_many_arguments)]
pub fn jlc_vscore(pw: f64, ph: f64, thick: Option<f64>, clear: Option<f64>, cuts: (usize, usize), rail: f64, w: f64, h: f64, rounded: bool) -> Vec<Value> {
    let mut out = Vec::new();
    let size_ok = pw >= 70.0 && ph >= 70.0 && pw <= 475.0 && ph <= 475.0;
    out.push(rule("panel 70 x 70 mm to 475 x 475 mm", Some(size_ok), if size_ok { format!("{} x {} mm", r3(pw), r3(ph)) } else {
        format!("{} x {} mm: add rows or columns, or widen the rails (railMm), to reach 70 mm each way", r3(pw), r3(ph)) }));
    out.push(match thick {
        Some(t) => rule("board 0.6 mm thick or more", Some(t >= 0.6), format!("{} mm (V-cut is not offered on 0.4 mm boards)", r3(t))),
        None => rule("board 0.6 mm thick or more", None, "the stack-up's thickness could not be read".into()),
    });
    out.push(match clear {
        Some(c) => rule("copper 0.4 mm or more from every cut (1.0 mm recommended, 2.0 mm by tall parts)", Some(c >= 0.4),
                        format!("nearest copper {c} mm from the board edge{}", if (0.4..1.0).contains(&c) { ": passes, under the 1.0 mm JLCPCB recommends" } else { "" })),
        None => rule("copper 0.4 mm or more from every cut", None, "no copper on the board".into()),
    });
    out.push(rule("at most 25 cuts each way", Some(cuts.0 <= 25 && cuts.1 <= 25), format!("{} vertical, {} horizontal", cuts.0, cuts.1)));
    let span = if rail > 0.0 { w.min(h).min(rail) } else { w.min(h) };
    out.push(rule("parallel cuts 2 mm apart or more", Some(span >= 2.0), format!("closest pair {} mm apart", r3(span))));
    let need = if thick.map(|t| t <= 0.8).unwrap_or(false) { 5.0 } else { 3.0 };
    out.push(if rail > 0.0 { rule("rails 3 mm wide or more (5 mm on 0.8 mm boards and thinner)", Some(rail >= need), format!("{rail} mm, {need} mm needed")) }
             else { rule("rails", None, "none: fine for bare boards, assembly needs rails (JLCPCB adds tooling holes and fiducials on them)".into()) });
    out.push(rule("straight board edges on the cuts (no arcs)", Some(!rounded), if rounded { "the outline has arcs (rounded corners): a V-cut is straight, so the corners come out square or need milling. Square the corners, or use tabs".into() } else { "rectangular".into() }));
    if w.min(h) < 15.0 { out.push(rule("pieces under 15 mm on a side cost extra", None, format!("{} x {} mm boards", r3(w), r3(h)))); }
    out.push(rule("V-cut is not offered with Economic Assembly", None, "order Standard Assembly for an assembled V-cut panel".into()));
    out
}

/// JLCPCB's mouse-bite rules against this panel.
pub fn jlc_tabs(gap: f64, pw: f64, ph: f64, clear: Option<f64>) -> Vec<Value> {
    let mut out = vec![
        rule("gap between boards 1.2 mm or more (1.6 or 2.0 typical)", Some(gap >= 1.2), format!("{gap} mm")),
        rule("0.6 mm holes, 5 to a set, 0.35 to 0.4 mm apart", Some(true), "five 0.6 mm holes at 1.0 mm pitch (0.4 mm between them), centred a third of a hole into the board".into()),
        rule("at least 2 sets an edge, one every 50 to 60 mm", Some(true), "2 tabs on edges of 15 mm and longer, one more each 55 mm; 1 on shorter edges".into()),
        rule("no mouse bites on the rail side", Some(true), "holes on the board side only".into()),
        rule("longest panel side under 250 mm (recommended)", Some(pw.max(ph) < 250.0), format!("{} mm", r3(pw.max(ph)))),
    ];
    if let Some(c) = clear { out.push(rule("copper clear of the bitten edge", Some(c >= 0.5), format!("nearest copper {c} mm from the board edge (mouse bites leave about 0.5 mm of rough edge)"))); }
    out
}

/// Where the tabs go along an edge of length `l`: their centres from the edge's start.
fn tab_centres(l: f64) -> Vec<f64> {
    if l < TAB + 1.0 { return vec![]; }
    let n = if l < 15.0 { 1 } else { ((l / 55.0).ceil() as usize).max(2) };
    (0..n).map(|k| l * (k as f64 + 0.5) / n as f64).collect()
}

/// The span [a, b] with the `cuts` spans taken out.
fn broken(a: f64, b: f64, cuts: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut s = a;
    let mut cs = cuts.to_vec();
    cs.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());
    for (c0, c1) in cs { if c0 > s { out.push((s, c0.min(b))); } s = s.max(c1); }
    if s < b { out.push((s, b)); }
    out
}

/// Mouse-bite tabs: the rout path on the outline layer (each board's edges broken at its tabs, the tab sides across
/// the gap, the rails' inner edge broken where tabs meet it) and the holes, all in panel mm. The rout path is drawn by
/// the bridge because the copies bring no outline of their own (the verb refuses a board that draws one).
pub fn tab_plan(w: f64, h: f64, rows: i64, cols: i64, gap: f64, rail: f64) -> (Vec<[f64; 4]>, Vec<[f64; 2]>, usize) {
    let lay = panel_layout(w, h, rows, cols, gap, rail, if rail > 0.0 { gap } else { 0.0 });
    let (pw, ph) = (f(&lay["size"][0]), f(&lay["size"][1]));
    let (mut lines, mut holes, mut tabs) = (Vec::new(), Vec::new(), 0usize);
    // the rails' inner edge, broken where outer tabs land: bottom, top, left, right
    let mut rail_cuts: [Vec<(f64, f64)>; 4] = Default::default();
    let off = BITE_HOLE / 2.0 - BITE_HOLE / 3.0;
    for r in 0..rows {
        for c in 0..cols {
            let b = &lay["boards"][(r * cols + c) as usize];
            let (bx, by) = (f(&b[0]), f(&b[1]));
            // (side, outer, along-x?, fixed coordinate, start, length, outward sign)
            for (side, outer, horiz, fixed, start, len, out) in [
                (0usize, r == 0, true, by, bx, w, -1.0), (1, r == rows - 1, true, by + h, bx, w, 1.0),
                (2, c == 0, false, bx, by, h, -1.0), (3, c == cols - 1, false, bx + w, by, h, 1.0)] {
                if outer && rail <= 0.0 { continue; } // the panel's own edge: the outline covers it
                let centres = tab_centres(len);
                let spans: Vec<(f64, f64)> = centres.iter().map(|t| (start + t - TAB / 2.0, start + t + TAB / 2.0)).collect();
                for (a, z) in broken(start, start + len, &spans) {
                    lines.push(if horiz { [a, fixed, z, fixed] } else { [fixed, a, fixed, z] });
                }
                let far = fixed + out * gap;
                for &(a, z) in &spans {
                    tabs += 1;
                    // the tab's two sides across the gap: drawn from the right and top edges (the neighbour's left and
                    // bottom edges are the same lines) and from every edge that faces a rail
                    if side == 1 || side == 3 || outer {
                        for p in [a, z] { lines.push(if horiz { [p, fixed, p, far] } else { [fixed, p, far, p] }); }
                    }
                    if outer { rail_cuts[side].push((a, z)); }
                    let mid = (a + z) / 2.0;
                    for k in 0..BITES {
                        let along = mid + (k - BITES / 2) as f64 * BITE_PITCH;
                        let across = fixed + out * off;
                        holes.push(if horiz { [r3(along), r3(across)] } else { [r3(across), r3(along)] });
                    }
                }
            }
        }
    }
    if rail > 0.0 {
        let (ix0, iy0, ix1, iy1) = (rail, rail, pw - rail, ph - rail);
        for (side, fixed, a, z, horiz) in [(0usize, iy0, ix0, ix1, true), (1, iy1, ix0, ix1, true), (2, ix0, iy0, iy1, false), (3, ix1, iy0, iy1, false)] {
            for (p, q) in broken(a, z, &rail_cuts[side]) { lines.push(if horiz { [p, fixed, q, fixed] } else { [fixed, p, fixed, q] }); }
        }
    }
    let lines = lines.into_iter().map(|l| [r3(l[0]), r3(l[1]), r3(l[2]), r3(l[3])]).collect();
    (lines, holes, tabs)
}

/// panelize: input {board, stack?, args: {rows, cols, separation?: none|vscore|tabs, gapMm?, railMm?, toolingHoles?}}. The
/// plan in panel mm from its lower-left corner: its size, where each copy's outline corner goes (and the shift that takes
/// the board's own coordinates there), the outline-layer lines (the V-CUT lines and their labels, or the tab rout path),
/// the mouse-bite and tooling holes, the fiducials, and JLCPCB's rules against it.
pub fn panelize(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let bad = |e: &str| crate::input::refuse("bad_args", e);
    let int = |k: &str, d: i64| args[k].as_i64().unwrap_or(d);
    let (rows, cols) = (int("rows", 1), int("cols", 2));
    if rows < 1 || cols < 1 || rows * cols > 400 { return bad("rows and cols are 1 or more, at most 400 boards"); }
    let sep = args["separation"].as_str().unwrap_or("none").to_ascii_lowercase();
    if !["none", "vscore", "tabs"].contains(&sep.as_str()) { return bad("separation is none, vscore (V-cut lines, boards edge to edge) or tabs (mouse bites)"); }
    let num = |k: &str| args[k].as_f64();
    let gap = match (sep.as_str(), num("gapMm")) {
        ("vscore", Some(g)) if g != 0.0 => return bad("V-score boards sit edge to edge: leave gapMm out or pass 0"),
        ("vscore", _) => 0.0,
        (_, Some(g)) => g,
        _ => 2.0,
    };
    let rail = num("railMm").unwrap_or(5.0);
    if gap < 0.0 || rail < 0.0 { return bad("gapMm and railMm are 0 or more"); }
    if sep == "tabs" && gap < 1.2 { return bad("mouse bites need a gap of 1.2 mm or more (JLCPCB: 1.6 or 2.0 typical)"); }
    let Some(bb) = crate::geom::outline_bbox(&st) else { return crate::input::refuse("no_outline", "the board has no outline to step and repeat") };
    let (x0, y0, x1, y1) = bb;
    let (w, h) = (x1 - x0, y1 - y0);
    if sep == "tabs" && mech_outline(&st, bb) {
        return json!({"success": false, "errorCode": "outline_on_mechanical_layer",
            "error": "the board draws its outline on a mechanical layer, and every copy in the panel brings that outline along closed: a router following it cuts each board free, so mouse-bite tabs cannot hold",
            "_hint": "Use separation: \"vscore\", or panelize without separation and order the fab's own panel with mouse bites, or move the outline to a layer the Gerbers leave out."});
    }
    let (clear, outside) = edge_clearance(&st, bb);
    let rounded = st["outlineEdges"].as_array().map(|es| es.iter().any(|e| e["kind"] == "arc")).unwrap_or(false);
    let thick = crate::input::stack(input).and_then(|s| s["totalUm"].as_f64()).map(|u| u / 1000.0);
    let edge = if sep == "tabs" && rail > 0.0 { gap } else { 0.0 };
    let lay = panel_layout(w, h, rows, cols, gap, rail, edge);
    let (pw, ph) = (f(&lay["size"][0]), f(&lay["size"][1]));
    let mut sep_out = json!({"kind": sep});
    let (mut lines, mut texts, mut holes, mut jlc): (Vec<[f64; 4]>, Vec<Value>, Vec<Value>, Vec<Value>) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    match sep.as_str() {
        "vscore" => {
            let (xs, ys) = vcut_lines(w, h, rows, cols, rail);
            let (ls, ts) = vcut_marks(&xs, &ys, pw, ph, rail);
            lines = ls;
            texts = ts.iter().map(|(x, y, rot)| json!({"text": "V-CUT", "x": x, "y": y, "size": 1.0, "rotation": rot})).collect();
            jlc = jlc_vscore(pw, ph, thick, clear.as_ref().map(|c| c.0), (xs.len(), ys.len()), rail, w, h, rounded);
            sep_out["cutsX"] = json!(xs);
            sep_out["cutsY"] = json!(ys);
        }
        "tabs" => {
            let (ls, hs, tabs) = tab_plan(w, h, rows, cols, gap, rail);
            lines = ls;
            holes = hs.iter().map(|p| json!({"x": p[0], "y": p[1], "drill": BITE_HOLE, "plated": false})).collect();
            jlc = jlc_tabs(gap, pw, ph, clear.as_ref().map(|c| c.0));
            sep_out["tabs"] = json!(tabs);
        }
        _ => {}
    }
    let tooling = args["toolingHoles"].as_bool().unwrap_or(true);
    let at = |k: &str| -> Vec<Value> { lay[k].as_array().cloned().unwrap_or_default() };
    let shifts: Vec<Value> = at("boards").iter().map(|b| json!([r3(f(&b[0]) - x0), r3(f(&b[1]) - y0)])).collect();
    let mut warnings = Vec::new();
    if !outside.is_empty() { warnings.push(json!({"copperOutsideOutline": outside, "why": "every copy repeats it into the rails and the neighbouring boards: move it onto the board or delete it"})); }
    let fails = jlc.iter().filter(|r| r["ok"] == json!(false)).count();
    json!({"success": true, "rows": rows, "cols": cols, "boardMm": [r3(w), r3(h)], "panelMm": lay["size"], "gapMm": gap, "railMm": rail,
        "boardsAt": lay["boards"], "copyShift": shifts,
        "outlineLayer": {"lines": lines.iter().map(|l| json!({"x1": l[0], "y1": l[1], "x2": l[2], "y2": l[3], "width": 0.1})).collect::<Vec<_>>(), "texts": texts},
        "holes": holes, "toolingHoles": if tooling { json!(at("toolingHoles").iter().map(|p| json!({"x": p[0], "y": p[1], "drill": 2.0})).collect::<Vec<_>>()) } else { json!([]) },
        "fiducials": if tooling { json!(at("fiducials").iter().map(|p| json!({"x": p[0], "y": p[1]})).collect::<Vec<_>>()) } else { json!([]) },
        "separation": sep_out, "jlcpcb": if jlc.is_empty() { Value::Null } else { json!({"rules": jlc, "fits": fails == 0}) },
        "nearestCopperToEdge": clear.map(|(mm, mut what)| { what["mm"] = json!(mm); what }), "warnings": warnings,
        "_hint": "A plan in panel mm from its lower-left corner. Place each copy of the board shifted by copyShift (board coordinates plus the shift land it at boardsAt), draw the panel's outline (0, 0, panelMm) and outlineLayer's lines and texts on the outline layer, drill holes (non-plated mouse bites) and toolingHoles, and place the fiducials. jlcpcb.rules lists each rule with ok true, false or null (a note)."})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_layout_places_boards_and_rails() {
        let l = panel_layout(40.0, 20.0, 2, 3, 2.0, 5.0, 0.0);
        assert_eq!(l["size"], json!([134.0, 52.0]));
        assert_eq!(l["boards"].as_array().unwrap().len(), 6);
        assert_eq!(l["boards"][4], json!([47.0, 27.0]));
        assert_eq!(l["toolingHoles"].as_array().unwrap().len(), 3);
        assert_eq!(l["fiducials"][1], json!([126.5, 2.5]));
        assert!(panel_layout(40.0, 20.0, 1, 2, 0.0, 3.0, 0.0)["toolingHoles"].as_array().unwrap().is_empty());
        // tabs: the boards stand a gap off the rails
        assert_eq!(panel_layout(40.0, 20.0, 1, 1, 2.0, 5.0, 2.0)["boards"][0], json!([7.0, 7.0]));
    }

    #[test]
    fn vcut_lines_run_across_the_panel_at_every_board_edge() {
        let (xs, ys) = vcut_lines(40.0, 20.0, 2, 3, 5.0);
        assert_eq!(xs, vec![5.0, 45.0, 85.0, 125.0]);
        assert_eq!(ys, vec![5.0, 25.0, 45.0]);
        // no rails: the panel's edges are milled, only the shared edges are cut
        let (xs, ys) = vcut_lines(40.0, 20.0, 2, 3, 0.0);
        assert_eq!((xs, ys), (vec![40.0, 80.0], vec![20.0]));
        let (lines, texts) = vcut_marks(&[5.0], &[5.0], 130.0, 50.0, 5.0);
        assert_eq!(lines, vec![[5.0, 0.0, 5.0, 50.0], [0.0, 5.0, 130.0, 5.0]]);
        assert_eq!(texts.len(), 4);
        // in the rail's inner half, clear of the fiducial at (7.5, 2.5) and its 2 mm opening
        assert_eq!(texts[0], (5.4, 3.7, 0));
        // near the right edge the label goes left of its line
        let (_, t2) = vcut_marks(&[125.0], &[], 130.0, 50.0, 5.0);
        assert_eq!(t2[0].0, 119.6);
    }

    #[test]
    fn jlc_vscore_rules_catch_a_small_thin_rounded_panel() {
        let r = jlc_vscore(130.0, 50.0, Some(0.39), Some(0.3), (4, 3), 5.0, 40.0, 20.0, true);
        let bad: Vec<&str> = r.iter().filter(|x| x["ok"] == json!(false)).map(|x| x["rule"].as_str().unwrap()).collect();
        assert_eq!(bad.len(), 4, "{bad:?}");
        assert!(bad.iter().any(|b| b.starts_with("panel 70")) && bad.iter().any(|b| b.starts_with("board 0.6")) && bad.iter().any(|b| b.starts_with("copper")) && bad.iter().any(|b| b.starts_with("straight")));
        let good = jlc_vscore(130.0, 90.0, Some(1.6), Some(1.2), (4, 5), 5.0, 40.0, 20.0, false);
        assert!(good.iter().all(|x| x["ok"] != json!(false)));
    }

    #[test]
    fn edge_clearance_finds_the_nearest_copper_and_strays() {
        let st = json!({"tracks": [{"layer": "F.Cu", "width": 0.2, "net": "A", "start": {"x": 0.5, "y": 10.0}, "end": {"x": 20.0, "y": 10.0}},
                                   {"layer": "L57", "width": 0.1, "net": "", "start": {"x": 0.0, "y": 0.0}, "end": {"x": 40.0, "y": 0.0}}],
                        "pads": [{"ref": "U3", "name": "1", "x": 60.0, "y": 5.0, "sizeX": 1.0, "sizeY": 1.0}]});
        let (near, out) = edge_clearance(&st, (0.0, 0.0, 40.0, 20.0));
        assert_eq!(near.unwrap().0, 0.4);
        assert_eq!(out.len(), 1);
        assert!(mech_outline(&st, (0.0, 0.0, 40.0, 20.0)));
    }

    #[test]
    fn tab_plan_follows_jlcpcb() {
        let (lines, holes, tabs) = tab_plan(40.0, 20.0, 1, 2, 2.0, 5.0);
        // two boards, 4 edges each with 2 tabs (40 mm edges: 2; 20 mm: 2)
        assert_eq!(tabs, 16);
        assert_eq!(holes.len(), 16 * 5);
        // holes sit a third of a hole into the board: 0.1 mm off the edge into the gap
        assert!(holes.contains(&[17.0, 6.9]));
        // a tab between the boards is bridged: no rout line crosses x = 47..49 at the tab's height
        assert!(!lines.iter().any(|l| l[0] == l[2] && (l[0] - 47.0).abs() < 1e-9 && l[1] <= 12.0 && l[3] >= 12.0));
        assert_eq!(broken(0.0, 10.0, &[(2.0, 4.0), (6.0, 8.0)]), vec![(0.0, 2.0), (4.0, 6.0), (8.0, 10.0)]);
        assert_eq!(tab_centres(40.0), vec![10.0, 30.0]);
        assert_eq!(tab_centres(120.0).len(), 3);
    }
}

//! `fusion_bodies`: each part's 3D bodies as boxes (the EDA Bridge Standard's bodies companion), read
//! from the design's 3D PCB: footprint rectangles in the board frame and height ranges above the
//! surface the part sits on. A part whose model is an assembly (an Adom molecule carries its own small
//! board on pins, its connector on top) is one box per sub-occurrence holding bodies, so a part under
//! the molecule's raised board is not read as a collision; a single-body part is one box. Fusion keeps the 3D PCB in its own document, so the read switches the
//! design to it and back to the 2D board (inside Fusion; no window or mouse is taken).
use serde_json::{json, Value};

const SCRIPT: &str = r#"
import adsk.fusion
des = adsk.fusion.Design.cast(app.activeProduct)
if des is None:
    result = {'error': 'the 3D PCB is not in front (active product: %s)' % app.activeProduct.productType}
else:
    rc = des.rootComponent
    def box(b):
        return [round(v * 10, 4) for v in (b.minPoint.x, b.minPoint.y, b.minPoint.z, b.maxPoint.x, b.maxPoint.y, b.maxPoint.z)]
    import math
    def turn_of(occ):
        # the part's own x axis in the board plane: its turn in degrees (a bottom part's flip included)
        try:
            m = occ.transform2
            return math.degrees(math.atan2(m.getCell(1, 0), m.getCell(0, 0)))
        except Exception:
            return 0.0
    def turned(bodies, deg):
        # the bodies' union as a rectangle along the part's own axes: [cx, cy, w, h] in mm, or None
        a = math.radians(deg)
        u = adsk.core.Vector3D.create(math.cos(a), math.sin(a), 0)
        v = adsk.core.Vector3D.create(-math.sin(a), math.cos(a), 0)
        lu = lv = 1e9; hu = hv = -1e9
        for b in bodies:
            ob = app.measureManager.getOrientedBoundingBox(b, u, v)
            if ob is None:
                return None
            c = ob.centerPoint
            cu = c.x * u.x + c.y * u.y; cv = c.x * v.x + c.y * v.y
            lu = min(lu, cu - ob.length / 2); hu = max(hu, cu + ob.length / 2)
            lv = min(lv, cv - ob.width / 2); hv = max(hv, cv + ob.width / 2)
        mu, mv = (lu + hu) / 2, (lv + hv) / 2
        return [round((mu * u.x + mv * v.x) * 10, 4), round((mu * u.y + mv * v.y) * 10, 4), round((hu - lu) * 10, 4), round((hv - lv) * 10, 4)]
    def boxes_of(occ, deg):
        # one box per occurrence that holds shown bodies: the bodies' union, then its sub-occurrences;
        # a part turned off the axes adds its rectangle along its own axes, [cx, cy, w, h]
        out = []
        shown = [b for b in occ.bRepBodies if b.isLightBulbOn and b.boundingBox]
        own = [box(b.boundingBox) for b in shown]
        if own:
            row = [min(b[i] for b in own) for i in range(3)] + [max(b[i] for b in own) for i in range(3, 6)]
            if abs(((deg + 45) % 90) - 45) > 0.5:
                t = turned(shown, deg)
                if t: row += t
            out.append(row)
        for ch in occ.childOccurrences:
            if ch.isLightBulbOn:
                out += boxes_of(ch, deg)
        return out
    board = None
    parts = []
    for o in rc.occurrences:
        if o.component.name == 'Board':
            core = [b for b in o.bRepBodies if b.name == 'Board'] or list(o.bRepBodies)
            if core:
                board = box(core[0].boundingBox)
        if o.component.name == 'Packages':
            for c in o.childOccurrences:
                deg = turn_of(c)
                bxs = boxes_of(c, deg)
                row = {'ref': c.name.rsplit(':', 1)[-1], 'package': c.name.rsplit(':', 1)[0], 'box': box(c.boundingBox) if c.boundingBox else None,
                       'boxes': bxs, 'visible': c.isLightBulbOn, 'rotation': round(deg, 3)}
                if abs(((deg + 45) % 90) - 45) > 0.5:
                    # the whole part along its own axes, over every shown body it holds
                    allb = []
                    def walk(x):
                        allb.extend(b for b in x.bRepBodies if b.isLightBulbOn and b.boundingBox)
                        for ch in x.childOccurrences:
                            if ch.isLightBulbOn: walk(ch)
                    walk(c)
                    row['turned'] = turned(allb, deg) if allb else None
                parts.append(row)
    result = {'board': board, 'parts': parts, 'document': app.activeDocument.name}
"#;

/// The 3D read turned into the standard's boxes, with what could not be seen named.
/// `through`: the parts with drilled pads, whose leads pass through the board.
pub fn shape(read: &Value, components: &[Value], through: &std::collections::HashSet<String>) -> Value {
    let Some(board) = read["board"].as_array().filter(|b| b.len() == 6) else {
        return json!({"success": false, "errorCode": "no_3d_board", "error": "the 3D PCB has no Board body: Fusion has not built the 3D board for this design",
                      "_hint": "fusion_show_3d_board once (Fusion builds it), then call fusion_bodies again."});
    };
    let f = |v: &Value| v.as_f64().unwrap_or(0.0);
    let (bottom, top) = (f(&board[2]), f(&board[5]));
    let mut bodies = Vec::new();
    let mut no_body = Vec::new();
    let mut suspect = Vec::new();
    let mut parts_with_body = 0usize;
    let place: std::collections::HashMap<&str, &Value> = components.iter().filter_map(|c| c["ref"].as_str().map(|r| (r, c))).collect();
    for p in read["parts"].as_array().into_iter().flatten() {
        let r = p["ref"].as_str().unwrap_or("");
        let b: Vec<f64> = p["box"].as_array().map(|a| a.iter().map(f).collect()).unwrap_or_default();
        if b.len() != 6 || (b[3] - b[0]).abs() < 1e-6 && (b[4] - b[1]).abs() < 1e-6 && (b[5] - b[2]).abs() < 1e-6 { no_body.push(json!(r)); continue; }
        // which side: the 2D board says; a part with no 2D row goes by where its body is
        let side = place.get(r).and_then(|c| c["side"].as_str()).unwrap_or(if (b[2] + b[5]) / 2.0 < (bottom + top) / 2.0 { "bottom" } else { "top" });
        let (z0, z1) = if side == "bottom" { (bottom - b[5], bottom - b[2]) } else { (b[2] - top, b[5] - top) };
        let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
        let mut row = json!({"ref": r, "side": side, "x0": r3(b[0]), "y0": r3(b[1]), "x1": r3(b[3]), "y1": r3(b[4]), "z0": r3(z0), "z1": r3(z1), "package": p["package"]});
        // a part turned off the axes: its rectangle along its own axes, so the engine does not count the
        // box's empty corners (standard engine 0.20.0)
        let turned = |row: &mut Value, t: &[f64]| { if t.len() == 4 { for (k, v) in [("cx", t[0]), ("cy", t[1]), ("w", t[2]), ("h", t[3])] { row[k] = json!(r3(v)); } row["rotation"] = json!(r3(f(&p["rotation"]))); } };
        if let Some(t) = p["turned"].as_array() { turned(&mut row, &t.iter().map(f).collect::<Vec<_>>()); }
        let tht = through.contains(r);
        if tht {
            // leads through the board: how far they stand out of the far side matters to an enclosure
            let past = if side == "bottom" { b[5] - top } else { bottom - b[2] };
            row["throughHole"] = json!(true);
            row["leadsPastFarSideMm"] = json!(r3(past.max(0.0)));
        }
        // an assembly model: one row per sub-box, each with its own height range on the part's side
        let subs: Vec<Vec<f64>> = p["boxes"].as_array().map(|a| a.iter().filter_map(|x| x.as_array().map(|v| v.iter().map(f).collect::<Vec<f64>>())).filter(|v| v.len() == 6 || v.len() == 10).collect()).unwrap_or_default();
        if subs.len() > 1 {
            for s in &subs {
                let (z0, z1) = if side == "bottom" { (bottom - s[5], bottom - s[2]) } else { (s[2] - top, s[5] - top) };
                let mut sr = row.clone();
                for k in ["cx", "cy", "w", "h", "rotation"] { if let Some(o) = sr.as_object_mut() { o.remove(k); } }
                for (k, v) in [("x0", s[0]), ("y0", s[1]), ("x1", s[3]), ("y1", s[4]), ("z0", z0), ("z1", z1)] { sr[k] = json!(r3(v)); }
                if s.len() == 10 { turned(&mut sr, &s[6..10]); }
                bodies.push(sr);
            }
        } else {
            bodies.push(row);
        }
        parts_with_body += 1;
        // a body that is not where the board puts its part: the model is off its footprint, floating,
        // or sunk into the board (a library model's origin or orientation is wrong, or the 3D PCB is
        // stale after a 2D move)
        let mut why = Vec::new();
        if let Some(c) = place.get(r) {
            let (x, y) = (f(&c["x"]), f(&c["y"]));
            if x < b[0] - 0.5 || x > b[3] + 0.5 || y < b[1] - 0.5 || y > b[4] + 0.5 { why.push(format!("the part is at ({x:.3}, {y:.3}) on the board but its body spans x {:.3}..{:.3}, y {:.3}..{:.3}", b[0], b[3], b[1], b[4])); }
        }
        if z0 > 0.5 { why.push(format!("the body floats {:.3} mm above the board", z0)); }
        if z0 < -0.2 && !tht { why.push(format!("the body sinks {:.3} mm into the board", -z0)); }
        if !why.is_empty() { suspect.push(json!({"ref": r, "why": why})); }
    }
    let tallest = |s: &str| bodies.iter().filter(|b| b["side"] == s).max_by(|a, b| f(&a["z1"]).partial_cmp(&f(&b["z1"])).unwrap()).map(|b| json!({"ref": b["ref"], "heightMm": b["z1"]}));
    let n = bodies.len();
    json!({"success": true, "mutated": false, "bodies": bodies, "count": n, "parts": parts_with_body, "noBody": no_body, "suspect": suspect,
           "tallest": {"top": tallest("top"), "bottom": tallest("bottom")},
           "surface": {"topZ": top, "bottomZ": bottom, "note": "z0/z1 are above the board's core surface on the part's side (below it for bottom parts): the copper and paste a part sits on are inside z0"},
           "complete": no_body.is_empty(),
           "_hint": if suspect.is_empty() { "A box per part, or per sub-assembly for a part whose model is an assembly (a molecule's own board, its connector, its pins): footprint rectangle and height range above the surface it sits on. Parts in noBody have no 3D model, so no height." } else { "suspect names parts whose 3D body is not where the board puts the part (off its footprint, floating or sunk): fix the library model's origin or orientation, or let Fusion update the 3D PCB, before trusting a height check on them. Parts in noBody have no 3D model." }})
}

/// `fusion_bodies {expectDocument, boardId?}`
pub fn bodies(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let mut st = crate::routing::call("routing_state", args, pass);
    if st["errorCode"] == "wrong_document" {
        // the 3D PCB (or the schematic) is in front: bring the board back and read again
        let _ = pass("show_2d_board", json!({}));
        st = crate::routing::call("routing_state", args, pass);
    }
    if st["success"].as_bool() != Some(true) { return st; }
    let shown = pass("show_3d_board", json!({}));
    if shown["success"].as_bool() == Some(false) {
        return json!({"success": false, "errorCode": "no_3d_board", "error": format!("could not switch the design to its 3D PCB: {}", shown["error"].as_str().unwrap_or("")), "show3d": shown});
    }
    let r = pass("run_modeling_script", json!({"script": SCRIPT}));
    // back to the board the caller was reading, whatever the read did
    let back = pass("show_2d_board", json!({}));
    let read = r["data"]["result"].clone();
    if !read.is_object() || read["error"].is_string() {
        return json!({"success": false, "errorCode": "no_3d_board", "error": read["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the 3D read failed: {}", r["error"].as_str().unwrap_or(""))), "returnedTo2d": back["success"]});
    }
    let through: std::collections::HashSet<String> = st["pads"].as_array().into_iter().flatten()
        .filter(|p| p["drill"].as_f64().unwrap_or(0.0) > 0.0).filter_map(|p| p["ref"].as_str().map(str::to_string)).collect();
    let mut out = shape(&read, st["components"].as_array().map(|v| v.as_slice()).unwrap_or(&[]), &through);
    out["boardId"] = st["boardId"].clone();
    out["revision"] = st["revision"].clone();
    out["returnedTo2d"] = json!(back["success"].as_bool() != Some(false));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boxes_sides_and_suspects() {
        let read = json!({"board": [0, 0, 0, 50.4, 26.4, 1.5], "parts": [
            {"ref": "R14", "package": "RESC1005X40", "box": [42.24, 7.79, 1.543, 43.36, 8.41, 1.963]},
            {"ref": "D3", "package": "LED", "box": [-0.895, 8.202, 3.133, 0.765, 9.052, 3.973]},
            {"ref": "U9", "package": "SOT", "box": [10, 10, -1.2, 12, 12, -0.03]},
            {"ref": "TP1", "package": "PAD", "box": [0, 0, 0, 0, 0, 0]},
            {"ref": "J1", "package": "HDR", "box": [4.17, 24.22, -1.695, 8.23, 26.18, 6.045]}]});
        let comps = vec![json!({"ref": "R14", "x": 42.8, "y": 8.1, "side": "top"}), json!({"ref": "D3", "x": 3.0, "y": 8.6, "side": "top"}),
                         json!({"ref": "U9", "x": 11.0, "y": 11.0, "side": "bottom"})];
        let through: std::collections::HashSet<String> = ["J1".to_string()].into_iter().collect();
        let r = shape(&read, &comps, &through);
        assert_eq!(r["success"], true);
        assert_eq!(r["bodies"][0], json!({"ref": "R14", "side": "top", "x0": 42.24, "y0": 7.79, "x1": 43.36, "y1": 8.41, "z0": 0.043, "z1": 0.463, "package": "RESC1005X40"}));
        assert_eq!(r["bodies"][2]["z0"], 0.03);
        assert_eq!(r["bodies"][2]["z1"], 1.2);
        assert_eq!(r["noBody"], json!(["TP1"]));
        let s = &r["suspect"];
        assert_eq!(s.as_array().unwrap().len(), 1);
        assert_eq!(s[0]["ref"], "D3");
        assert_eq!(s[0]["why"].as_array().unwrap().len(), 2, "off its footprint and floating");
        assert_eq!(r["tallest"]["bottom"]["ref"], "U9");
        // a through-hole header's leads are not a sunk body: they stand 1.695 mm out of the bottom
        let j1 = r["bodies"].as_array().unwrap().iter().find(|b| b["ref"] == "J1").unwrap();
        assert_eq!(j1["throughHole"], true);
        assert_eq!(j1["leadsPastFarSideMm"], 1.695);
    }
    #[test]
    fn a_turned_part_carries_its_own_rectangle() {
        // a 4 x 4 mm QFN turned 45 degrees: its world box is 5.66 square, its own rectangle 4 x 4
        let read = json!({"board": [0, 0, 0, 30, 30, 1.5], "parts": [
            {"ref": "U1", "package": "QFN", "box": [7.17, 7.17, 1.5, 12.83, 12.83, 2.4], "rotation": 45.0, "turned": [10.0, 10.0, 4.0, 4.0]},
            {"ref": "R1", "package": "R", "box": [1, 1, 1.5, 2, 1.5, 1.9], "rotation": 90.0}]});
        let comps = vec![json!({"ref": "U1", "x": 10.0, "y": 10.0, "side": "top"}), json!({"ref": "R1", "x": 1.5, "y": 1.25, "side": "top"})];
        let r = shape(&read, &comps, &Default::default());
        let u1 = &r["bodies"][0];
        assert_eq!((u1["cx"].as_f64(), u1["w"].as_f64(), u1["rotation"].as_f64(), u1["x0"].as_f64()), (Some(10.0), Some(4.0), Some(45.0), Some(7.17)));
        assert!(r["bodies"][1]["cx"].is_null(), "a part on the axes is its box");
        // the engine reads it as the turned rectangle
        let b = adom_eda_checks::run("height_check", &json!({"bodies": r["bodies"], "args": {}}));
        assert_ne!(b["success"], json!(false), "{b}");
    }

    #[test]
    fn a_molecule_is_a_box_per_sub_assembly() {
        // the USB-C molecule on BQ25792: its own board rides 1 mm above the main board on pins, its
        // connector on top; one union box would sit on the capacitors under it
        let read = json!({"board": [0, 0, 0, 50.4, 26.4, 1.582], "parts": [
            {"ref": "U$2", "package": "USBC", "box": [6.9, 11.98, -0.03, 26.42, 30.42, 8.08],
             "boxes": [[6.9, 11.98, 2.585, 26.42, 30.42, 4.182], [7.2, 16.0, 4.182, 16.0, 26.0, 8.08], [8.0, 12.0, -0.03, 9.0, 13.0, 2.585]]},
            {"ref": "C11", "package": "CAPC2012X110", "box": [26.465, 12.94, 1.638, 27.935, 15.16, 2.758], "boxes": [[26.465, 12.94, 1.638, 27.935, 15.16, 2.758]]}]});
        let r = shape(&read, &[json!({"ref": "U$2", "x": 16.0, "y": 21.0, "side": "top"})], &Default::default());
        let rows = r["bodies"].as_array().unwrap();
        assert_eq!(rows.iter().filter(|b| b["ref"] == "U$2").count(), 3);
        assert_eq!(rows.iter().filter(|b| b["ref"] == "C11").count(), 1);
        assert_eq!(r["parts"], 2);
        let board = rows.iter().find(|b| b["ref"] == "U$2" && b["z1"] == 2.6).unwrap();
        assert_eq!(board["z0"], 1.003);
        assert_eq!(r["tallest"]["top"]["heightMm"], 6.498);
    }
    #[test]
    fn no_board_body_is_named() {
        assert_eq!(shape(&json!({"parts": []}), &[], &Default::default())["errorCode"], "no_3d_board");
    }
}

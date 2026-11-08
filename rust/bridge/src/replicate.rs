//! `fusion_replicate_layout`: the standard's replicate_layout. One placed channel's arrangement
//! copied to matching channels: each target keeps its anchor part where it is, and every other part
//! takes the source's offset and turn from the anchor, turned by the anchors' rotation difference.
//! A bottom-side part stays mirrored. Applied with one guarded move_parts (read back, undone on a
//! mismatch).

use serde_json::{json, Value};

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }

/// The moves: `parts` are routing_state's components ({ref, x, y, rotation, side, pattern}).
pub fn plan(parts: &[Value], source: &[String], targets: &[Vec<String>], anchor: Option<&str>) -> Result<(Vec<Value>, Vec<Value>), String> {
    if source.len() < 2 { return Err("source is the placed channel's parts, two or more".into()); }
    let a = match anchor { Some(r) => source.iter().position(|s| s.eq_ignore_ascii_case(r)).ok_or(format!("anchor {r} is not in source"))?, None => 0 };
    let find = |r: &str| parts.iter().find(|p| p["ref"].as_str().map(|x| x.eq_ignore_ascii_case(r)).unwrap_or(false));
    let missing: Vec<&String> = source.iter().chain(targets.iter().flatten()).filter(|r| find(r).is_none()).collect();
    if !missing.is_empty() { return Err(format!("not on the board: {}", missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))); }
    let src = find(&source[a]).unwrap();
    let (mut moves, mut mismatches) = (Vec::new(), Vec::new());
    for t in targets {
        if t.len() != source.len() { return Err(format!("target {:?} has {} parts; source has {} (same order as source)", t, t.len(), source.len())); }
        let anc = find(&t[a]).unwrap();
        let d = f(&anc["rotation"]) - f(&src["rotation"]);
        let (s, c) = d.to_radians().sin_cos();
        for (i, r) in t.iter().enumerate() {
            let (sp, tp) = (find(&source[i]).unwrap(), find(r).unwrap());
            if sp["pattern"] != tp["pattern"] { mismatches.push(json!({"ref": r, "footprint": tp["pattern"], "source": source[i], "sourceFootprint": sp["pattern"]})); }
            if i == a { continue; }
            let (dx, dy) = (f(&sp["x"]) - f(&src["x"]), f(&sp["y"]) - f(&src["y"]));
            let x = f(&anc["x"]) + dx * c - dy * s;
            let y = f(&anc["y"]) + dx * s + dy * c;
            let rot = (f(&sp["rotation"]) + d).rem_euclid(360.0);
            let rot = r3(rot) % 360.0;
            // a bottom part keeps its mirror: EAGLE's MR turn
            let rotation = if tp["side"] == "bottom" { json!(format!("MR{}", rot)) } else { json!(rot) };
            moves.push(json!({"ref": r, "x": r3(x), "y": r3(y), "rotation": rotation}));
        }
    }
    Ok((moves, mismatches))
}

fn refs(v: &Value) -> Vec<String> { v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect() }

pub fn replicate_layout(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let source = refs(&args["source"]);
    let targets: Vec<Vec<String>> = args["targets"].as_array().into_iter().flatten().map(refs).collect();
    if targets.is_empty() { return json!({"success": false, "errorCode": "bad_args", "error": "targets is [[refs], ...], each in the same order as source"}); }
    let st = crate::routing::call("routing_state", args, pass);
    if st["success"].as_bool() != Some(true) { return st; }
    let parts = st["components"].as_array().cloned().unwrap_or_default();
    let (moves, mismatches) = match plan(&parts, &source, &targets, args["anchor"].as_str()) { Ok(x) => x, Err(e) => return json!({"success": false, "errorCode": "bad_args", "error": e}) };
    if args["dryRun"].as_bool() == Some(true) || moves.is_empty() {
        return json!({"success": true, "mutated": false, "moved": 0, "moves": moves, "footprintMismatches": mismatches, "revision": st["revision"], "boardId": st["boardId"]});
    }
    let mv = crate::routing::call("move_parts", &json!({"boardId": st["boardId"], "expectDocument": args["expectDocument"], "expectedRevision": st["revision"], "parts": moves}), pass);
    if mv["success"].as_bool() != Some(true) { let mut e = mv; e["moves"] = json!(moves); return e; }
    json!({"success": true, "mutated": true, "moved": moves.len(), "moves": moves, "footprintMismatches": mismatches, "revision": mv["revision"], "boardId": mv["boardId"], "documentName": mv["documentName"],
           "_hint": if mismatches.is_empty() { "Each target channel now sits like the source around its anchor. Route it the same way with fusion_copy_routing." }
                    else { "Moved, but footprintMismatches lists parts whose footprint differs from their source part: their pads will not line up with the source's routing." }})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_copies_round_its_turned_anchor() {
        let p = |r: &str, x: f64, y: f64, rot: f64, side: &str| json!({"ref": r, "x": x, "y": y, "rotation": rot, "side": side, "pattern": "R0603"});
        let parts = vec![p("U1", 10.0, 10.0, 0.0, "top"), p("R1", 13.0, 10.0, 90.0, "top"), p("C1", 10.0, 12.0, 0.0, "top"),
                         p("U2", 40.0, 10.0, 90.0, "top"), p("R2", 0.0, 0.0, 0.0, "top"), p("C2", 0.0, 0.0, 0.0, "bottom")];
        let (m, mm) = plan(&parts, &["U1".into(), "R1".into(), "C1".into()], &[vec!["U2".into(), "R2".into(), "C2".into()]], None).unwrap();
        assert!(mm.is_empty());
        // turned 90 degrees round U2: R1's +3 in x becomes +3 in y; C1's +2 in y becomes -2 in x
        assert_eq!(m[0], json!({"ref": "R2", "x": 40.0, "y": 13.0, "rotation": 180.0}));
        assert_eq!(m[1], json!({"ref": "C2", "x": 38.0, "y": 10.0, "rotation": "MR90"}));
        assert!(plan(&parts, &["U1".into(), "R1".into()], &[vec!["U2".into()]], None).is_err());
        assert!(plan(&parts, &["U1".into(), "R1".into()], &[vec!["U2".into(), "R9".into()]], None).unwrap_err().contains("R9"));
    }
}

//! Placement helpers from the KiCad bridge's place_tools.rs: the gaps between parts (their boxes or their pad copper)
//! and a plan for decoupling capacitors beside an IC's power pins, answered as move_parts arguments. The board model has
//! no courtyards, so a part is the box of its pads and, with the bodies companion, of its 3D body, as placement_check
//! measures it. Pure.
use crate::route::{pad_shape, Shape};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn f(v: &Value) -> f64 { v.as_f64().unwrap_or(0.0) }
fn r3(v: f64) -> f64 { (v * 1000.0).round() / 1000.0 }
type P = (f64, f64);
type Rect = (f64, f64, f64, f64);

/// The gap between two boxes, 0 when they touch or overlap.
fn rect_gap(a: Rect, b: Rect) -> f64 {
    let dx = (b.0 - a.2).max(a.0 - b.2).max(0.0);
    let dy = (b.1 - a.3).max(a.1 - b.3).max(0.0);
    (dx * dx + dy * dy).sqrt()
}

fn union(a: Rect, b: Rect) -> Rect { (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)) }

/// References and pad names in natural order: C2 before C10.
fn ref_key(r: &str) -> (String, u64, String) {
    let head: String = r.chars().take_while(|c| !c.is_ascii_digit()).collect();
    let rest = &r[head.len()..];
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    (head.to_uppercase(), num.parse().unwrap_or(0), rest[num.len()..].to_string())
}

fn prefix(r: &str) -> String { r.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_uppercase() }

pub fn is_ground(net: &str) -> bool {
    let n = net.trim_start_matches('/').to_uppercase();
    n.contains("GND") || n.starts_with("VSS") || n == "0V" || n.ends_with("_VSS")
}

/// A net named like a supply: +5V, VCC, VDD_CORE, VBAT, 3V3, 1V8.
fn power_name(net: &str) -> bool {
    let n = net.trim_start_matches('/').to_uppercase();
    let c: Vec<char> = n.chars().collect();
    n.starts_with('+') || n.starts_with('V') || ["VCC", "VDD", "VBAT", "VIN", "VBUS", "PWR", "SUPPLY"].iter().any(|k| n.contains(k))
        || (1..c.len()).any(|i| c[i] == 'V' && c[i - 1].is_ascii_digit())
}

/// The pad's box, turned with it.
fn pad_box(at: P, sx: f64, sy: f64, rot: f64) -> Rect {
    let (c, s) = (rot.to_radians().cos().abs(), rot.to_radians().sin().abs());
    let (hx, hy) = ((sx * c + sy * s) / 2.0, (sx * s + sy * c) / 2.0);
    (at.0 - hx, at.1 - hy, at.0 + hx, at.1 + hy)
}

fn turn(p: P, deg: f64) -> P {
    let (c, s) = (deg.to_radians().cos(), deg.to_radians().sin());
    (p.0 * c - p.1 * s, p.0 * s + p.1 * c)
}

/// One part: where it is, its pads and its bodies' boxes.
struct Part { at: P, rot: f64, bottom: bool, pads: Vec<Value>, bodies: Vec<Rect> }

impl Part {
    /// A point of the part, after moving the part to `at` turned to `rot` (counterclockwise as seen).
    fn moved(&self, p: P, at: P, rot: f64) -> P {
        let q = turn((p.0 - self.at.0, p.1 - self.at.1), rot - self.rot);
        (at.0 + q.0, at.1 + q.1)
    }
    /// The part's box at `at` turned to `rot`: its pads, and its bodies' boxes turned with it.
    fn box_at(&self, at: P, rot: f64) -> Option<Rect> {
        let d = rot - self.rot;
        let mut b: Option<Rect> = None;
        for p in &self.pads {
            let r = pad_box(self.moved((f(&p["x"]), f(&p["y"])), at, rot), f(&p["sizeX"]), f(&p["sizeY"]), f(&p["rotation"]) + d);
            b = Some(b.map(|x| union(x, r)).unwrap_or(r));
        }
        for x in &self.bodies {
            for c in [(x.0, x.1), (x.2, x.1), (x.2, x.3), (x.0, x.3)] {
                let q = self.moved(c, at, rot);
                let r = (q.0, q.1, q.0, q.1);
                b = Some(b.map(|x| union(x, r)).unwrap_or(r));
            }
        }
        b
    }
}

/// The board's parts by reference (as given), with their bodies when the companion came.
fn parts(st: &Value, input: &Value) -> BTreeMap<String, Part> {
    let mut m: BTreeMap<String, Part> = BTreeMap::new();
    for c in st["components"].as_array().cloned().unwrap_or_default() {
        let Some(r) = c["ref"].as_str().filter(|r| !r.is_empty()) else { continue };
        m.insert(r.to_string(), Part { at: (f(&c["x"]), f(&c["y"])), rot: f(&c["rotation"]), bottom: c["side"] == "bottom", pads: Vec::new(), bodies: Vec::new() });
    }
    for p in st["pads"].as_array().cloned().unwrap_or_default() {
        if let Some(part) = p["ref"].as_str().and_then(|r| m.get_mut(r)) { part.pads.push(p); }
    }
    for b in crate::bodies::input_boxes(input).unwrap_or_default() {
        if let Some(part) = m.get_mut(&b.part) { part.bodies.push((b.x0, b.y0, b.x1, b.y1)); }
    }
    m
}

fn find<'a>(m: &'a BTreeMap<String, Part>, r: &str) -> Option<(&'a String, &'a Part)> { m.iter().find(|(k, _)| k.eq_ignore_ascii_case(r)) }

/// check_clearance: input {board, bodies?, args: {refs?, mode?: courtyard|pads, thresholdMm?, includeSameNet?, maxPairs?}}.
/// courtyard: part boxes (pads, and 3D bodies when given) on the same side; pads: pad copper of different parts that
/// shares a copper layer, by its real outline.
pub fn check_clearance(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let mode = args["mode"].as_str().unwrap_or("courtyard");
    if mode != "courtyard" && mode != "pads" { return crate::input::refuse("bad_args", &format!("mode is courtyard or pads, not {mode}")); }
    let threshold = args.get("thresholdMm").or_else(|| args.get("threshold")).and_then(|v| v.as_f64()).unwrap_or(if mode == "pads" { 0.2 } else { 0.25 }).max(0.0);
    let limit = args["maxPairs"].as_u64().unwrap_or(200) as usize;
    let m = parts(&st, input);
    let refs: Option<Vec<String>> = args["refs"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).map(|s| s.trim().to_uppercase()).collect());
    if let Some(rs) = &refs {
        let unknown: Vec<&String> = rs.iter().filter(|r| find(&m, r).is_none()).collect();
        if !unknown.is_empty() { return crate::input::refuse("no_such_part", &format!("not parts on this board: {unknown:?}")); }
    }
    let wanted = |r: &str| refs.as_ref().map(|rs| rs.iter().any(|x| x.eq_ignore_ascii_case(r))).unwrap_or(true);
    let order = |a: &str, b: &str| if ref_key(a) <= ref_key(b) { (a.to_string(), b.to_string()) } else { (b.to_string(), a.to_string()) };
    let (mut pairs, mut checked) = (Vec::new(), 0usize);
    let measured;
    let mut stacked = Vec::new();
    if mode == "courtyard" {
        let boxes: Vec<(&String, bool, Rect)> = m.iter().filter_map(|(r, p)| Some((r, p.bottom, p.box_at(p.at, p.rot)?))).collect();
        let bodies = crate::bodies::input_boxes(input).unwrap_or_default();
        let pads = st["pads"].as_array().cloned().unwrap_or_default();
        for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                let ((ra, sa, a), (rb, sb, b)) = (boxes[i], boxes[j]);
                if sa != sb || !(wanted(ra) || wanted(rb)) { continue; }
                checked += 1;
                let d = rect_gap(a, b);
                if d < threshold {
                    let (x, y) = order(ra, rb);
                    let row = json!({"a": x, "b": y, "side": if sa { "bottom" } else { "top" }, "distanceMm": r3(d), "overlapping": d == 0.0});
                    // a part under a raised one (pads apart, no bodies meeting in 3D) is a stack, as placement_check says (fusion-lane #22)
                    if crate::placement::is_stacked(&pads, &bodies, ra, rb, threshold / 2.0) { stacked.push(row) } else { pairs.push(row) }
                }
            }
        }
        measured = if crate::bodies::input_boxes(input).is_some() { "part box: its pads and 3D body" } else { "part box: its pads only (pass bodies to add the 3D bodies)" };
    } else {
        let same_net = args["includeSameNet"].as_bool().unwrap_or(false);
        let pads: Vec<&Value> = m.values().flat_map(|p| p.pads.iter()).collect();
        let shapes: Vec<(Shape, Rect)> = pads.iter().map(|p| { let s = pad_shape(p); let b = s.bounds(); (s, b) }).collect();
        let through = |p: &Value| p["layer"] == "*.Cu" || f(&p["drill"]) > 0.0;
        for i in 0..pads.len() {
            for j in i + 1..pads.len() {
                let (a, b) = (pads[i], pads[j]);
                let (ra, rb) = (a["ref"].as_str().unwrap_or(""), b["ref"].as_str().unwrap_or(""));
                if ra == rb || !(wanted(ra) || wanted(rb)) { continue; }
                let (na, nb) = (a["net"].as_str().unwrap_or(""), b["net"].as_str().unwrap_or(""));
                if !same_net && !na.is_empty() && na == nb { continue; }
                if !(through(a) || through(b) || a["layer"] == b["layer"]) { continue; }
                checked += 1;
                if rect_gap(shapes[i].1, shapes[j].1) >= threshold { continue; }
                let d = shapes[i].0.gap(&shapes[j].0);
                if d < threshold {
                    let (ka, kb) = (format!("{ra}.{}", a["name"].as_str().unwrap_or("")), format!("{rb}.{}", b["name"].as_str().unwrap_or("")));
                    let ((x, nx), (y, ny)) = if ref_key(&ka) <= ref_key(&kb) { ((ka, na), (kb, nb)) } else { ((kb, nb), (ka, na)) };
                    pairs.push(json!({"a": x, "b": y, "netA": nx, "netB": ny, "distanceMm": r3(d), "touching": d == 0.0}));
                }
            }
        }
        measured = "pads: their real copper (copperPoints when given, round pads as circles or stadiums, others as their turned rectangle)";
    }
    pairs.sort_by(|x, y| f(&x["distanceMm"]).partial_cmp(&f(&y["distanceMm"])).unwrap_or(std::cmp::Ordering::Equal));
    let total = pairs.len();
    pairs.truncate(limit);
    json!({"success": true, "mode": mode, "thresholdMm": threshold, "measuredOn": measured, "pairsChecked": checked, "violationCount": total,
        "pairs": pairs, "truncated": total > limit, "clean": total == 0, "stacked": stacked,
        "_hint": if total == 0 { "No pair is closer than thresholdMm. The tool's DRC (run_drc) is the authority on copper clearance." }
                 else { "pairs are closest first. Move parts apart with move_parts (find_space finds room); overlapping or touching pairs come first. The tool's DRC (run_drc) is the authority on copper clearance." }})
}

/// The side of the box nearest a pin: its outward normal and name (y up: top is +y).
fn side_of(b: Rect, pin: P) -> (P, &'static str) {
    let d = [(pin.0 - b.0, (-1.0, 0.0), "left"), (b.2 - pin.0, (1.0, 0.0), "right"), (pin.1 - b.1, (0.0, -1.0), "bottom"), (b.3 - pin.1, (0.0, 1.0), "top")];
    let best = d.iter().min_by(|a, b| a.0.partial_cmp(&b.0).unwrap()).unwrap();
    (best.1, best.2)
}

/// A capacitor across ground and one other net: (its pad on that net, the net).
fn decoupling_role(r: &str, p: &Part) -> Option<(Value, String)> {
    if prefix(r) != "C" || p.pads.len() != 2 { return None; }
    let net = |q: &Value| q["net"].as_str().unwrap_or("").to_string();
    if p.pads.iter().filter(|q| is_ground(&net(q))).count() != 1 { return None; }
    p.pads.iter().find(|q| !net(q).is_empty() && !is_ground(&net(q))).map(|q| (q.clone(), net(q)))
}

/// place_decoupling: input {board, bodies?, args: {ic, caps?, gapMm?}}. Each cap goes `gapMm` outside the IC's box on the
/// side of the power pin it serves, turned so its power pad faces the pin and lined up with it; a spot taken by another
/// part slides along that side. Answers `moveArgs` for move_parts and checks the plan with placement_check.
pub fn place_decoupling(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let Some(ic_ref) = args.get("ic").or_else(|| args.get("ref")).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) else {
        return crate::input::refuse("bad_args", "ic is required: the IC's reference (\"U1\")");
    };
    let gap = args.get("gapMm").or_else(|| args.get("gap")).and_then(|v| v.as_f64()).unwrap_or(0.3).max(0.0);
    let m = parts(&st, input);
    let Some((ic_ref, ic)) = find(&m, ic_ref) else { return crate::input::refuse("no_such_part", &format!("{ic_ref} is not a part on this board")) };
    let Some(ic_box) = ic.box_at(ic.at, ic.rot) else { return crate::input::refuse("no_pads", &format!("{ic_ref} has no pads in the board model")) };
    let net = |q: &Value| q["net"].as_str().unwrap_or("").to_string();
    let mut ic_nets: Vec<String> = ic.pads.iter().map(net).filter(|n| !n.is_empty() && !is_ground(n)).collect();
    ic_nets.sort();
    ic_nets.dedup();
    let roles: BTreeMap<&String, (Value, String)> = m.iter().filter_map(|(r, p)| Some((r, decoupling_role(r, p)?))).collect();
    // a supply: named like one, or two or more caps across it to ground (one cap on a signal is a crystal's load or a
    // filter, not decoupling)
    let mut across: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, n) in roles.values() { *across.entry(n.as_str()).or_insert(0) += 1; }
    let supply = |n: &str| power_name(n) || across.get(n).copied().unwrap_or(0) >= 2;
    let supplies: Vec<String> = ic_nets.iter().filter(|n| supply(n)).cloned().collect();
    // caps given by name may sit on any of the IC's nets
    let power = if args["caps"].is_array() { ic_nets.clone() } else { supplies.clone() };
    let pad_at = |q: &Value| (f(&q["x"]), f(&q["y"]));
    let (mut left_out, mut cap_refs): (Vec<Value>, Vec<String>) = (Vec::new(), Vec::new());
    match args["caps"].as_array() {
        Some(a) => cap_refs = a.iter().filter_map(|v| v.as_str()).map(String::from).collect(),
        None => {
            // the other ICs, for caps that serve the same supply nearer another one
            let ics: Vec<(&String, &Part)> = m.iter().filter(|(r, p)| matches!(prefix(r).as_str(), "U" | "IC") && p.pads.len() >= 3 && p.bottom == ic.bottom).collect();
            for (r, (pp, n)) in &roles {
                if !ic_nets.contains(n) { continue; }
                if !supply(n) {
                    left_out.push(json!({"cap": r, "net": n, "why": "signal_net", "detail": format!("{n} has one cap to ground and is not named like a supply: a crystal's load or a filter. Pass caps to place it anyway")}));
                    continue;
                }
                let near = |p: &Part| p.pads.iter().filter(|q| net(q) == *n).map(|q| { let (a, b) = (pad_at(q), pad_at(pp)); ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt() }).fold(f64::MAX, f64::min);
                let own = near(ic);
                if let Some((other, _)) = ics.iter().filter(|(o, _)| *o != ic_ref).map(|(o, p)| (o, near(p))).filter(|(_, d)| *d < own).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap()) {
                    left_out.push(json!({"cap": r, "net": n, "why": "nearer_another_ic", "detail": format!("{r} is nearer {other}'s {n} pin; it decouples that IC. Pass caps to place it here anyway")}));
                    continue;
                }
                cap_refs.push(r.to_string());
            }
        }
    }
    cap_refs.sort_by_key(|r| ref_key(r));
    if cap_refs.is_empty() {
        return json!({"success": false, "errorCode": "no_caps", "error": format!("no capacitor across ground and a supply of {ic_ref}: pass caps:[...]"), "icPowerNets": supplies, "leftOut": left_out});
    }
    // a cap the plan cannot place stays where it is, so its spot stays taken: plan again with it fixed until none is new
    // (fusion-lane #22: C32 was planned onto C29, which had no free spot and never moved)
    let ring = crate::coppermap::outline_ring(&st);
    let on_board = |b: Rect| ring.len() < 3 || [(b.0, b.1), (b.2, b.1), (b.2, b.3), (b.0, b.3)].iter().all(|c| crate::route::inside(*c, &ring));
    // room for the IC's fanout: a cap's pads keep clear of the stubs and vias plan_fanout would give the pins of other
    // nets (altium-lane: caps placed first sat on 5 of U4's escape stubs and vias)
    let (fan, fan_clear) = if args["leaveFanout"].as_bool() == Some(false) { (Vec::new(), 0.0) } else { fanout_room(input, ic_ref) };
    let pads_clear = |cap: &Part, at: P, rot: f64| cap.pads.iter().all(|q| {
        let mut moved = q.clone();
        let c = cap.moved(pad_at(q), at, rot);
        moved["x"] = json!(c.0); moved["y"] = json!(c.1); moved["rotation"] = json!(f(&q["rotation"]) + rot - cap.rot);
        moved.as_object_mut().map(|o| o.remove("copperPoints"));
        let s = pad_shape(&moved);
        fan.iter().all(|(n, piece)| *n == net(q) || s.gap(piece) >= fan_clear - 1e-6)
    });
    let mut stay: Vec<String> = Vec::new();
    let (mut used, mut rows, mut moves, mut skipped): (Vec<String>, Vec<Value>, Vec<(String, P, f64)>, Vec<Value>);
    loop {
        let moving: Vec<String> = cap_refs.iter().map(|r| r.to_uppercase()).filter(|r| !stay.contains(r)).collect();
        let mut taken: Vec<Rect> = m.iter().filter(|(r, p)| p.bottom == ic.bottom && !moving.contains(&r.to_uppercase())).filter_map(|(_, p)| p.box_at(p.at, p.rot)).collect();
        // a part on the other side still takes the spot of its through-hole pads, and so do the board's own holes
        for p in st["pads"].as_array().into_iter().flatten() {
            let through = p["layer"] == "*.Cu" || f(&p["drill"]) > 0.0;
            let other_side = p["ref"].as_str().and_then(|r| m.get(r)).map(|q| q.bottom != ic.bottom).unwrap_or(true);
            if through && other_side { taken.push(pad_box(pad_at(p), f(&p["sizeX"]).max(f(&p["drill"])), f(&p["sizeY"]).max(f(&p["drill"])), f(&p["rotation"]))); }
        }
        (used, rows, moves, skipped) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for cr in &cap_refs {
            let Some((cr, cap)) = find(&m, cr) else { skipped.push(json!({"cap": cr, "why": "no_such_part"})); continue };
            if cap.bottom != ic.bottom {
                skipped.push(json!({"cap": cr, "why": "other_side", "detail": format!("{cr} is on the other side from {ic_ref}: move it over first (move_parts with side)")}));
                continue;
            }
            let Some(pp) = cap.pads.iter().find(|q| power.contains(&net(q))) else { skipped.push(json!({"cap": cr, "why": "no_shared_power_net"})); continue };
            let n = net(pp);
            let mut pins: Vec<&Value> = ic.pads.iter().filter(|q| net(q) == n).collect();
            pins.sort_by_key(|q| (used.contains(&q["name"].as_str().unwrap_or("").to_string()), ref_key(q["name"].as_str().unwrap_or(""))));
            let pin = pins[0];
            let pin_name = pin["name"].as_str().unwrap_or("").to_string();
            used.push(pin_name.clone());
            let pin_at = pad_at(pin);
            let (nv, side) = side_of(ic_box, pin_at);
            // the turn that points the power pad at the IC
            let rel = |rot: f64| cap.moved(pad_at(pp), (0.0, 0.0), rot);
            let rot = [0.0, 90.0, 180.0, 270.0].into_iter().max_by(|a, b| {
                let (va, vb) = (rel(*a), rel(*b));
                (-(va.0 * nv.0 + va.1 * nv.1)).partial_cmp(&(-(vb.0 * nv.0 + vb.1 * nv.1))).unwrap()
            }).unwrap();
            let Some(cy) = cap.box_at((0.0, 0.0), rot) else { skipped.push(json!({"cap": cr, "why": "no_pads"})); continue };
            let v = rel(rot);
            let base = match side {
                "left" => (ic_box.0 - gap - cy.2, pin_at.1 - v.1),
                "right" => (ic_box.2 + gap - cy.0, pin_at.1 - v.1),
                "top" => (pin_at.0 - v.0, ic_box.3 + gap - cy.1),
                _ => (pin_at.0 - v.0, ic_box.1 - gap - cy.3),
            };
            let along = if nv.0 != 0.0 { (0.0, 1.0) } else { (1.0, 0.0) };
            let step = if nv.0 != 0.0 { cy.3 - cy.1 } else { cy.2 - cy.0 } + gap;
            let mut chosen = None;
            for k in 0..13 {
                let s = if k == 0 { 0.0 } else if k % 2 == 1 { ((k + 1) / 2) as f64 * step } else { -((k / 2) as f64) * step };
                let at = (r3(base.0 + along.0 * s), r3(base.1 + along.1 * s));
                let Some(b) = cap.box_at(at, rot) else { continue };
                if taken.iter().all(|t| rect_gap(*t, b) >= gap - 1e-3) && on_board(b) && pads_clear(cap, at, rot) { chosen = Some((at, b, s)); break; }
            }
            let Some((at, b, slid)) = chosen else { skipped.push(json!({"cap": cr, "why": "no_free_spot", "pin": format!("{ic_ref}.{pin_name}"), "side": side})); continue };
            taken.push(b);
            let pa = cap.moved(pad_at(pp), at, rot);
            rows.push(json!({"cap": cr, "net": n, "pin": format!("{ic_ref}.{pin_name}"), "side": side, "x": at.0, "y": at.1, "rotation": rot,
                "powerPad": format!("{cr}.{}", pp["name"].as_str().unwrap_or("")), "padToPinMm": r3(((pa.0 - pin_at.0).powi(2) + (pa.1 - pin_at.1).powi(2)).sqrt()),
                "slidMm": r3(slid), "from": {"x": cap.at.0, "y": cap.at.1, "rotation": cap.rot}}));
            moves.push((cr.clone(), at, rot));
        }
        let new: Vec<String> = skipped.iter().filter_map(|x| x["cap"].as_str()).filter(|c| find(&m, c).is_some()).map(|c| c.to_uppercase()).filter(|c| !stay.contains(c)).collect();
        if new.is_empty() { break; }
        stay.extend(new);
    }
    let validation = validate(&st, input, &m, &moves);
    let parts_arg: Vec<Value> = moves.iter().map(|(r, at, rot)| json!({"ref": r, "x": at.0, "y": at.1, "rotation": rot})).collect();
    json!({"success": true, "ic": ic_ref, "icPowerNets": supplies, "gapMm": gap, "placements": rows, "skipped": skipped, "leftOut": left_out,
        "moveArgs": {"parts": parts_arg, "expectedRevision": st["revision"]}, "validation": validation, "revision": st["revision"],
        "measuredOn": if crate::bodies::input_boxes(input).is_some() { "part boxes: pads and 3D bodies" } else { "part boxes: pads only (pass bodies to keep clear of 3D bodies too)" },
        "_hint": "Read the plan, then send moveArgs to move_parts. The caps' old tracks stay where they were: route the stubs again (plan_route, route_net) and repour. leftOut lists caps the default left for another IC or a signal net; name them in caps to place them here."})
}

/// The copper plan_fanout would put down for an IC's pins, by net, and the clearance it keeps; nothing when the IC is no
/// package fanout plans (fewer than four surface pads).
fn fanout_room(input: &Value, ic: &str) -> (Vec<(String, Shape)>, f64) {
    let mut q = input.clone();
    q["args"] = json!({"ref": ic});
    let r = plan_fanout(&q);
    if r["success"] != json!(true) { return (Vec::new(), 0.0); }
    let st = crate::input::board(input).unwrap_or_default();
    let at = |name: &str| st["pads"].as_array().into_iter().flatten().find(|p| format!("{}.{}", p["ref"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or("")) == name).map(|p| (f(&p["x"]), f(&p["y"])));
    let (w, v) = (f(&r["widthMm"]) / 2.0, f(&r["via"]["size"]) / 2.0);
    let mut out = Vec::new();
    for row in r["pads"].as_array().into_iter().flatten() {
        let (Some(pa), Some(n)) = (row["pad"].as_str().and_then(at), row["net"].as_str()) else { continue };
        let pt = |x: &Value| (f(&x[0]), f(&x[1]));
        if row["stubEnd"].is_array() { out.push((n.to_string(), Shape::Cap(pa, pt(&row["stubEnd"]), w))); }
        if row["via"].is_array() { let e = pt(&row["via"]); out.push((n.to_string(), Shape::Cap(pa, e, w))); out.push((n.to_string(), Shape::Cap(e, e, v))); }
    }
    (out, f(&r["clearanceMm"]))
}

/// placement_check on the board with the caps moved, keeping what concerns them.
fn validate(st: &Value, input: &Value, m: &BTreeMap<String, Part>, moves: &[(String, P, f64)]) -> Value {
    if moves.is_empty() { return json!({"ok": false, "reason": "nothing to place"}); }
    let mut board = st.clone();
    let mut bodies = crate::input::part(input, "bodies");
    for (r, at, rot) in moves {
        let p = &m[r];
        let d = rot - p.rot;
        for c in board["components"].as_array_mut().into_iter().flatten().filter(|c| c["ref"] == json!(r)) {
            c["x"] = json!(at.0); c["y"] = json!(at.1); c["rotation"] = json!(rot);
        }
        for q in board["pads"].as_array_mut().into_iter().flatten().filter(|q| q["ref"] == json!(r)) {
            let n = p.moved((f(&q["x"]), f(&q["y"])), *at, *rot);
            q["x"] = json!(n.0); q["y"] = json!(n.1); q["rotation"] = json!((f(&q["rotation"]) + d).rem_euclid(360.0));
            q.as_object_mut().map(|o| o.remove("copperPoints"));
        }
        if let Some(b) = bodies.as_mut() {
            let rows = if b.get("bodies").is_some() { b["bodies"].as_array_mut() } else { b.as_array_mut() };
            for x in rows.into_iter().flatten().filter(|x| x["ref"] == json!(r)) {
                let cs: Vec<P> = [(f(&x["x0"]), f(&x["y0"])), (f(&x["x1"]), f(&x["y0"])), (f(&x["x1"]), f(&x["y1"])), (f(&x["x0"]), f(&x["y1"]))].iter().map(|c| p.moved(*c, *at, *rot)).collect();
                x["x0"] = json!(cs.iter().map(|c| c.0).fold(f64::MAX, f64::min)); x["x1"] = json!(cs.iter().map(|c| c.0).fold(f64::MIN, f64::max));
                x["y0"] = json!(cs.iter().map(|c| c.1).fold(f64::MAX, f64::min)); x["y1"] = json!(cs.iter().map(|c| c.1).fold(f64::MIN, f64::max));
            }
        }
    }
    let r = crate::placement::placement_check(&json!({"board": board, "bodies": bodies, "args": {}}));
    if r["success"] != json!(true) { return json!({"ok": false, "check": r}); }
    let ours = |e: &Value| moves.iter().any(|(m, _, _)| e["ref"] == json!(m) || e["parts"].as_array().map(|a| a.contains(&json!(m))).unwrap_or(false));
    let keep = |k: &str| -> Vec<Value> { r[k].as_array().cloned().unwrap_or_default().into_iter().filter(|e| ours(e)).collect() };
    let (ov, off, holes, near) = (keep("courtyardOverlaps"), keep("offOrOverEdge"), keep("onMountingHoles"), keep("nearEdge"));
    json!({"ok": ov.is_empty() && off.is_empty() && holes.is_empty(), "by": "placement_check on the board with the caps moved", "courtyardOverlaps": ov, "offOrOverEdge": off, "onMountingHoles": holes, "nearEdge": near})
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() { return None; }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(v[v.len() / 2])
}

/// The board's copper layers, top to bottom: from the stack's count when given, else the layers its copper uses.
pub(crate) fn copper_layers(st: &Value, input: &Value) -> Vec<String> {
    if let Some(n) = crate::input::stack(input).and_then(|s| s["copperLayers"].as_u64()).filter(|n| *n >= 2) {
        let mut v = vec!["F.Cu".to_string()];
        v.extend((1..n - 1).map(|i| format!("In{i}.Cu")));
        v.push("B.Cu".into());
        return v;
    }
    let mut inner: Vec<u32> = Vec::new();
    for k in ["tracks", "arcs", "pads", "polygons"] {
        for x in st[k].as_array().into_iter().flatten() {
            if let Some(i) = x["layer"].as_str().and_then(|l| l.strip_prefix("In")).and_then(|r| r.strip_suffix(".Cu")).and_then(|n| n.parse().ok()) {
                if !inner.contains(&i) { inner.push(i); }
            }
        }
    }
    inner.sort();
    std::iter::once("F.Cu".to_string()).chain(inner.iter().map(|i| format!("In{i}.Cu"))).chain(std::iter::once("B.Cu".to_string())).collect()
}

/// plan_fanout: input {board, stack?, rules?, args: {ref, style?, escapeLayer?, viaMm?, viaDrillMm?, widthMm?, clearanceMm?,
/// pitchMm?, stubMm?, surfaceOuterRing?, viaPerimeter?}}. A BGA's outer ring escapes on the surface and each inner ball
/// takes a dogbone via (or a via in the pad); a perimeter package's pads take an outward stub (with a via at its end for
/// ground or `viaPerimeter`), an exposed pad a via grid. `calls` are route_net arguments, one per net. Every via and
/// stub is measured against other nets' pads on the whole board (`conflicts`).
pub fn plan_fanout(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let args = &input["args"];
    let Some(r) = args["ref"].as_str().map(str::trim).filter(|s| !s.is_empty()) else { return crate::input::refuse("bad_args", "ref is required: the BGA or QFN/QFP (\"U3\")") };
    let m = parts(&st, input);
    let Some((r, part)) = find(&m, r) else { return crate::input::refuse("no_such_part", &format!("{r} is not a part on this board")) };
    let side = if part.bottom { "B.Cu" } else { "F.Cu" };
    let copper = copper_layers(&st, input);
    let escape = match args["escapeLayer"].as_str() {
        Some(l) if copper.iter().any(|c| c == l) => l.to_string(),
        Some(l) => return json!({"success": false, "errorCode": "no_such_layer", "error": format!("{l} is not a copper layer of this board"), "copperLayers": copper}),
        None => copper.iter().find(|l| l.starts_with("In")).cloned().unwrap_or_else(|| if part.bottom { "F.Cu".into() } else { "B.Cu".into() }),
    };
    let pads: Vec<&Value> = part.pads.iter().filter(|p| f(&p["drill"]) == 0.0).collect();
    if pads.len() < 4 { return crate::input::refuse("not_a_package", &format!("{r} has {} surface pads: fanout is for a BGA or a QFN/QFP", pads.len())); }
    let at = |p: &Value| (f(&p["x"]), f(&p["y"]));
    let net = |p: &Value| p["net"].as_str().unwrap_or("").to_string();
    let name = |p: &Value| format!("{r}.{}", p["name"].as_str().unwrap_or(""));
    let areas: Vec<f64> = pads.iter().map(|p| f(&p["sizeX"]) * f(&p["sizeY"])).collect();
    let med_area = median(areas.clone()).unwrap_or(0.0);
    let (thermal, signal): (Vec<usize>, Vec<usize>) = (0..pads.len()).partition(|i| areas[*i] > 4.0 * med_area);
    let d = |a: P, b: P| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    let nn: Vec<f64> = signal.iter().map(|&i| signal.iter().filter(|&&j| j != i).map(|&j| d(at(pads[i]), at(pads[j]))).fold(f64::MAX, f64::min)).collect();
    let pitch = match args["pitchMm"].as_f64() { Some(v) if v > 0.0 => v, _ => match median(nn) { Some(v) => r3(v), None => return crate::input::refuse("not_a_package", "no pad pitch") } };
    let (cx, cy) = (signal.iter().map(|&i| at(pads[i]).0).sum::<f64>() / signal.len() as f64, signal.iter().map(|&i| at(pads[i]).1).sum::<f64>() / signal.len() as f64);
    let bx = signal.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, &i| { let p = at(pads[i]); (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1)) });
    let interior = signal.iter().filter(|&&i| { let p = at(pads[i]); (p.0 - bx.0).min(bx.2 - p.0).min(p.1 - bx.1).min(bx.3 - p.1) > pitch * 0.5 }).count();
    let kind = if interior > 0 { "bga" } else { "perimeter" };
    let fine = pitch < 0.8 - 1e-9;
    let via_size = args.get("viaMm").or_else(|| args.get("viaSizeMm")).and_then(|v| v.as_f64()).unwrap_or(if fine { 0.4 } else if pitch < 1.0 - 1e-9 { 0.45 } else { 0.6 });
    let via_drill = args["viaDrillMm"].as_f64().unwrap_or(if via_size <= 0.45 { 0.2 } else { 0.3 });
    if via_drill >= via_size { return crate::input::refuse("bad_args", "viaDrillMm must be smaller than viaMm"); }
    let width = args["widthMm"].as_f64().unwrap_or(if fine { 0.1 } else { 0.15 });
    let rule = crate::fab::routing_clearance(&crate::input::rules(input));
    let clearance = args["clearanceMm"].as_f64().unwrap_or(0.0).max(rule.unwrap_or(0.1));
    let pad_d = median(signal.iter().map(|&i| f(&pads[i]["sizeX"]).max(f(&pads[i]["sizeY"]))).collect()).unwrap_or(0.0);
    let room = r3(pitch / 2f64.sqrt() - pad_d / 2.0 - via_size / 2.0);
    let fits = room + 1e-9 >= clearance;
    let want = args["style"].as_str().unwrap_or("auto");
    let style = match want {
        "auto" => if fits { "dogbone" } else { "via-in-pad" },
        "dogbone" | "via-in-pad" => want,
        o => return crate::input::refuse("bad_args", &format!("style is auto, dogbone or via-in-pad, not {o}")),
    };
    let mut warnings = Vec::new();
    if kind == "bga" && style == "dogbone" && !fits {
        warnings.push(format!("a {via_size} mm dogbone via leaves {room} mm to the balls at {pitch} mm pitch, under the {clearance} mm clearance: use via-in-pad or a smaller via"));
    }
    let via_perimeter = args["viaPerimeter"].as_bool().unwrap_or(false);
    let stub = args["stubMm"].as_f64().unwrap_or(0.4);
    let wp = |p: P, l: &str| json!({"x": r3(p.0), "y": r3(p.1), "layer": l});
    let sign = |v: f64| if v < -1e-9 { -1.0 } else { 1.0 };
    let (mut rows, mut calls): (Vec<Value>, BTreeMap<String, Vec<Value>>) = (Vec::new(), BTreeMap::new());
    // what the plan puts down, to measure: (pad, net, via or stub as a capsule)
    let mut pieces: Vec<(String, String, &'static str, Shape)> = Vec::new();
    type Pieces = Vec<(String, String, &'static str, Shape)>;
    fn via(pieces: &mut Pieces, calls: &mut BTreeMap<String, Vec<Value>>, rows: &mut Vec<Value>, from: String, n: &str, v: P, size: f64, path: Vec<Value>, row: Value) {
        pieces.push((from, n.to_string(), "via", Shape::Cap(v, v, size / 2.0)));
        calls.entry(n.to_string()).or_default().push(json!(path));
        rows.push(row);
    }
    // the board's routed copper: (net, layer or "" for every layer, shape, kind). Net-less tracks and arcs are a hatched
    // pour's pieces, which re-pour round new copper (altium-lane #22: a plan with "0 conflicts" shorted U4's routing)
    let mut routed: Vec<(String, String, Shape, &'static str)> = Vec::new();
    for t in st["tracks"].as_array().into_iter().flatten().filter(|t| !net(t).is_empty()) {
        routed.push((net(t), t["layer"].as_str().unwrap_or("").to_string(), Shape::Cap((f(&t["start"]["x"]), f(&t["start"]["y"])), (f(&t["end"]["x"]), f(&t["end"]["y"])), f(&t["width"]) / 2.0), "track"));
    }
    for a in st["arcs"].as_array().into_iter().flatten().filter(|a| !net(a).is_empty()) {
        let (c, rad) = ((f(&a["center"]["x"]), f(&a["center"]["y"])), f(&a["radius"]));
        let (a1, mut a2) = (f(&a["startAngle"]), f(&a["endAngle"]));
        if a2 <= a1 { a2 += 360.0; }
        let k = ((a2 - a1) / 10.0).ceil().max(1.0) as usize;
        let p = |deg: f64| (c.0 + rad * deg.to_radians().cos(), c.1 + rad * deg.to_radians().sin());
        for i in 0..k { routed.push((net(a), a["layer"].as_str().unwrap_or("").to_string(), Shape::Cap(p(a1 + (a2 - a1) * i as f64 / k as f64), p(a1 + (a2 - a1) * (i + 1) as f64 / k as f64), f(&a["width"]) / 2.0), "arc")); }
    }
    let holes: Vec<(P, f64)> = st["vias"].as_array().into_iter().flatten().map(|v| ((f(&v["position"]["x"]), f(&v["position"]["y"])), f(&v["size"]) / 2.0))
        .chain(st["pads"].as_array().into_iter().flatten().filter(|p| f(&p["drill"]) > 0.0).map(|p| (at(p), f(&p["sizeX"]).max(f(&p["sizeY"])).max(f(&p["drill"])) / 2.0))).collect();
    for v in st["vias"].as_array().into_iter().flatten().filter(|v| !net(v).is_empty()) {
        let c = (f(&v["position"]["x"]), f(&v["position"]["y"]));
        routed.push((net(v), String::new(), Shape::Cap(c, c, f(&v["size"]) / 2.0), "via"));
    }
    let include_routed = args["includeRouted"].as_bool().unwrap_or(false);
    for &i in &signal {
        let p = pads[i];
        let (pa, n) = (at(p), net(p));
        if n.is_empty() { rows.push(json!({"pad": name(p), "net": "", "action": "skip", "why": "no net"})); continue; }
        // a pin its net's copper already reaches is routed: a stub from it would run into that routing
        if !include_routed {
            let ps = pad_shape(p);
            if routed.iter().any(|(rn, l, s, _)| *rn == n && (l.is_empty() || l == side) && ps.gap(s) <= 1e-6) {
                rows.push(json!({"pad": name(p), "net": n, "action": "skip", "why": "already routed"}));
                continue;
            }
        }
        if kind == "bga" {
            let ring = ((pa.0 - bx.0) / pitch).round().min(((bx.2 - pa.0) / pitch).round()).min(((pa.1 - bx.1) / pitch).round()).min(((bx.3 - pa.1) / pitch).round()) as i64;
            if ring == 0 && args["surfaceOuterRing"].as_bool() != Some(false) {
                let (nv, _) = side_of(bx, pa);
                let end = (pa.0 + nv.0 * pitch, pa.1 + nv.1 * pitch);
                pieces.push((name(p), n.clone(), "stub", Shape::Cap(pa, end, width / 2.0)));
                calls.entry(n.clone()).or_default().push(json!([{"pad": name(p)}, wp(end, side)]));
                rows.push(json!({"pad": name(p), "net": n, "ring": ring, "action": "surface", "layer": side, "stubEnd": [r3(end.0), r3(end.1)]}));
            } else if style == "dogbone" {
                let v = (pa.0 + sign(pa.0 - cx) * pitch / 2.0, pa.1 + sign(pa.1 - cy) * pitch / 2.0);
                pieces.push((name(p), n.clone(), "stub", Shape::Cap(pa, v, width / 2.0)));
                via(&mut pieces, &mut calls, &mut rows, name(p), &n, v, via_size, vec![json!({"pad": name(p)}), wp(v, side), wp(v, &escape)],
                    json!({"pad": name(p), "net": n, "ring": ring, "action": "dogbone", "layer": escape, "via": [r3(v.0), r3(v.1)]}));
            } else {
                via(&mut pieces, &mut calls, &mut rows, name(p), &n, pa, via_size, vec![wp(pa, side), wp(pa, &escape)],
                    json!({"pad": name(p), "net": n, "ring": ring, "action": "via-in-pad", "layer": escape, "via": [r3(pa.0), r3(pa.1)], "note": "via-in-pad needs filled and capped vias (IPC-4761 type VII) from the fab"}));
            }
        } else {
            let (nv, edge) = side_of(bx, pa);
            let (c, s) = (f(&p["rotation"]).to_radians().cos().abs(), f(&p["rotation"]).to_radians().sin().abs());
            let half = if nv.0 != 0.0 { f(&p["sizeX"]) * c + f(&p["sizeY"]) * s } else { f(&p["sizeX"]) * s + f(&p["sizeY"]) * c } / 2.0;
            let with_via = via_perimeter || is_ground(&n);
            let reach = half + stub + if with_via { via_size / 2.0 } else { 0.0 };
            let end = (pa.0 + nv.0 * reach, pa.1 + nv.1 * reach);
            pieces.push((name(p), n.clone(), "stub", Shape::Cap(pa, end, width / 2.0)));
            if with_via {
                via(&mut pieces, &mut calls, &mut rows, name(p), &n, end, via_size, vec![json!({"pad": name(p)}), wp(end, side), wp(end, &escape)],
                    json!({"pad": name(p), "net": n, "edge": edge, "action": "stub+via", "layer": escape, "via": [r3(end.0), r3(end.1)]}));
            } else {
                calls.entry(n.clone()).or_default().push(json!([{"pad": name(p)}, wp(end, side)]));
                rows.push(json!({"pad": name(p), "net": n, "edge": edge, "action": "surface", "layer": side, "stubEnd": [r3(end.0), r3(end.1)]}));
            }
        }
    }
    for &i in &thermal {
        let p = pads[i];
        let n = net(p);
        if n.is_empty() { continue; }
        let (pa, w, h) = (at(p), f(&p["sizeX"]), f(&p["sizeY"]));
        let (w, h) = if (f(&p["rotation"]).rem_euclid(180.0) - 90.0).abs() < 1.0 { (h, w) } else { (w, h) };
        let vp = (via_size + 0.5).max(1.0);
        let (nx, ny) = ((((w - via_size) / vp).floor() as i64 + 1).max(1), (((h - via_size) / vp).floor() as i64 + 1).max(1));
        let (mut grid, mut kept_clear) = (Vec::new(), 0);
        for a in 0..nx {
            for b in 0..ny {
                let v = (pa.0 + (a as f64 - (nx - 1) as f64 / 2.0) * vp, pa.1 + (b as f64 - (ny - 1) as f64 / 2.0) * vp);
                // clear of the holes already there, as thermal_vias keeps them (a grid on U4's 9 GND vias was hole-to-hole)
                if holes.iter().any(|(c, r)| ((c.0 - v.0).powi(2) + (c.1 - v.1).powi(2)).sqrt() < r + via_size / 2.0 + clearance - 1e-9) { kept_clear += 1; continue; }
                pieces.push((name(p), n.clone(), "via", Shape::Cap(v, v, via_size / 2.0)));
                calls.entry(n.clone()).or_default().push(json!([wp(v, side), wp(v, &escape)]));
                grid.push(json!([r3(v.0), r3(v.1)]));
            }
        }
        rows.push(json!({"pad": name(p), "net": n, "action": "thermal-via-grid", "layer": escape, "vias": grid, "viaPitchMm": vp, "keptClearOfHoles": kept_clear}));
    }
    // each via and stub against every other net's pad on the board (vias on every layer, stubs on the part's side)
    let all: Vec<(Value, Shape)> = st["pads"].as_array().cloned().unwrap_or_default().into_iter().map(|p| { let s = pad_shape(&p); (p, s) }).collect();
    let (mut worst, mut conflicts) = (f64::MAX, Vec::new());
    for (from, n, what, s) in &pieces {
        for (p, ps) in &all {
            if net(p) == *n && !n.is_empty() { continue; }
            if *what == "stub" && !(p["layer"] == side || p["layer"] == "*.Cu" || f(&p["drill"]) > 0.0) { continue; }
            let g = s.gap(ps);
            if *what == "via" { worst = worst.min(g); }
            if g < clearance - 1e-9 {
                conflicts.push(json!({"from": from, "what": what, "with": "pad", "pad": format!("{}.{}", p["ref"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or("")), "padNet": p["net"], "gapMm": r3(g)}));
            }
        }
        // and against other nets' tracks, arcs and vias: on every layer for a via, on the part's side for a stub
        for (rn, l, rs, kind) in &routed {
            if rn == n || (*what == "stub" && !l.is_empty() && l != side) { continue; }
            let g = s.gap(rs);
            if g < clearance - 1e-9 {
                let (x0, y0, x1, y1) = rs.bounds();
                conflicts.push(json!({"from": from, "what": what, "with": kind, "net": rn, "layer": if l.is_empty() { "every" } else { l.as_str() }, "near": [r3((x0 + x1) / 2.0), r3((y0 + y1) / 2.0)], "gapMm": r3(g)}));
            }
        }
    }
    // other nets' pours the plan runs into: not conflicts (a pour re-pours round new copper), but until the repour the
    // tool's DRC sees the pour's pieces as shorts (altium-lane #22), and a stub across a pour can cut it in two
    let mut crosses: Vec<Value> = Vec::new();
    for p in st["polygons"].as_array().into_iter().flatten() {
        let (pn, pl) = (net(p), p["layer"].as_str().unwrap_or("").to_string());
        let ring: Vec<P> = p["points"].as_array().into_iter().flatten().map(|q| (f(&q[0]), f(&q[1]))).collect();
        if pn.is_empty() || ring.len() < 3 { continue; }
        let poly = Shape::Poly(ring);
        let hit = pieces.iter().any(|(_, n, what, s)| *n != pn && (*what == "via" || pl == side) && s.gap(&poly) <= 1e-9);
        if hit && !crosses.iter().any(|c| c["net"] == json!(pn) && c["layer"] == json!(pl)) { crosses.push(json!({"net": pn, "layer": pl})); }
    }
    let via_count = pieces.iter().filter(|x| x.2 == "via").count();
    let calls: Vec<Value> = calls.into_iter().map(|(n, paths)| json!({"net": n, "layer": side, "width": width, "viaSize": via_size, "viaDrill": via_drill, "paths": paths})).collect();
    json!({"success": true, "ref": r, "package": kind, "padCount": pads.len(), "pitchMm": pitch, "side": side, "escapeLayer": escape, "copperLayers": copper,
        "style": if kind == "bga" { style } else { "stubs" }, "requestedStyle": want, "dogboneRoomMm": room, "dogboneFits": fits,
        "via": {"size": via_size, "drill": via_drill}, "widthMm": width, "clearanceMm": clearance,
        "pads": rows, "calls": calls, "callCount": calls.len(), "viaCount": via_count,
        "minViaToPadMm": if worst < f64::MAX { json!(r3(worst)) } else { Value::Null }, "conflicts": conflicts, "crossesPours": crosses, "warnings": warnings, "revision": st["revision"],
        "_hint": "A plan only: send each call to route_net with expectedRevision: revision. conflicts lists vias and stubs closer than clearanceMm to another net's pad, track, arc or via (vias on every layer, stubs on the part's side): apply nothing while it is not empty. Pins already reached by their net's copper are skipped (includeRouted plans them anyway); the exposed-pad grid steps round holes already there. crossesPours names other nets' pours the plan runs into: repour BEFORE running DRC (until then the pour's pieces read as shorts), then check that none of them split (splitNets). Via-in-pad needs filled and capped vias: tell the user first."})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(r: &str, name: &str, x: f64, y: f64, net: &str) -> Value {
        json!({"ref": r, "name": name, "x": x, "y": y, "sizeX": 0.6, "sizeY": 0.6, "rotation": 0, "layer": "F.Cu", "drill": 0, "net": net, "shape": "rect"})
    }

    /// U1: a 4-pin part at the origin with VDD on its right; C1 (VDD/GND) far away, C2 on a crystal net, C3 near U2.
    fn board() -> Value {
        json!({"revision": "r1", "outline": [[-20, -20], [20, -20], [20, 20], [-20, 20]], "nets": ["VDD", "GND", "XIN"],
            "components": [{"ref": "U1", "x": 0, "y": 0, "rotation": 0, "side": "top"}, {"ref": "U2", "x": 12, "y": 0, "rotation": 0, "side": "top"},
                           {"ref": "C1", "x": -10, "y": 10, "rotation": 0, "side": "top"}, {"ref": "C2", "x": -10, "y": -10, "rotation": 0, "side": "top"},
                           {"ref": "C3", "x": 12, "y": 3, "rotation": 0, "side": "top"}],
            "pads": [pad("U1", "1", -2.0, 1.0, "XIN"), pad("U1", "2", -2.0, -1.0, "GND"), pad("U1", "3", 2.0, -1.0, "GND"), pad("U1", "4", 2.0, 1.0, "VDD"),
                     pad("U2", "1", 11.0, 1.0, "VDD"), pad("U2", "2", 11.0, -1.0, "GND"), pad("U2", "3", 13.0, 0.0, "GND"),
                     pad("C1", "1", -10.5, 10.0, "VDD"), pad("C1", "2", -9.5, 10.0, "GND"),
                     pad("C2", "1", -10.5, -10.0, "XIN"), pad("C2", "2", -9.5, -10.0, "GND"),
                     pad("C3", "1", 11.5, 3.0, "VDD"), pad("C3", "2", 12.5, 3.0, "GND")]})
    }

    #[test]
    fn a_cap_lands_beside_its_pin_with_the_power_pad_facing_it() {
        let r = place_decoupling(&json!({"board": board(), "args": {"ic": "U1"}}));
        assert_eq!(r["success"], json!(true), "{r}");
        // C1 only: C2 sits on a crystal net, C3 is nearer U2's VDD pin
        assert_eq!(r["placements"].as_array().unwrap().len(), 1, "{r}");
        let p = &r["placements"][0];
        assert_eq!((p["cap"].as_str(), p["pin"].as_str(), p["side"].as_str()), (Some("C1"), Some("U1.4"), Some("right")));
        // U1's box ends at x 2.3; 0.3 out, the power pad (now the cap's left pad) at 2.9, level with the pin
        assert_eq!(p["rotation"], json!(0.0));
        assert!((f(&p["x"]) - 3.4).abs() < 1e-9 && (f(&p["y"]) - 1.0).abs() < 1e-9, "{p}");
        assert!((f(&p["padToPinMm"]) - 0.9).abs() < 1e-9);
        let why: Vec<&str> = r["leftOut"].as_array().unwrap().iter().map(|l| l["why"].as_str().unwrap()).collect();
        assert_eq!(why, vec!["signal_net", "nearer_another_ic"]);
        assert_eq!(r["moveArgs"]["parts"][0], json!({"ref": "C1", "x": 3.4, "y": 1.0, "rotation": 0.0}));
        assert_eq!(r["moveArgs"]["expectedRevision"], "r1");
        assert_eq!(r["validation"]["ok"], json!(true), "{r}");
        // U1's supplies only: XIN has one cap and is not named like a supply (fusion-lane #22)
        assert_eq!(r["icPowerNets"], json!(["VDD"]));
    }

    #[test]
    fn a_cap_leaves_room_for_the_ics_fanout() {
        // a second ground pin, U1.5, just under the VDD pin: its fanout via lands where C1 would go (altium-lane)
        let mut b = board();
        b["pads"].as_array_mut().unwrap().push(pad("U1", "5", 2.0, 0.4, "GND"));
        let r = place_decoupling(&json!({"board": b, "args": {"ic": "U1", "caps": ["C1"]}}));
        assert!(f(&r["placements"][0]["slidMm"]).abs() > 0.0, "{r}");
        let close = place_decoupling(&json!({"board": b, "args": {"ic": "U1", "caps": ["C1"], "leaveFanout": false}}));
        assert_eq!(close["placements"][0]["slidMm"], json!(0.0), "{close}");
    }

    #[test]
    fn a_cap_that_cannot_move_keeps_its_spot() {
        // C2 sits where C1 would go, and cannot be placed (its net is none of U1's): C1 slides past it (fusion-lane #22)
        let mut b = board();
        b["components"][3]["x"] = json!(3.4); b["components"][3]["y"] = json!(1.0);
        b["pads"][9] = pad("C2", "1", 2.9, 1.0, "OTHER");
        b["pads"][10] = pad("C2", "2", 3.9, 1.0, "GND");
        let r = place_decoupling(&json!({"board": b, "args": {"ic": "U1", "caps": ["C1", "C2"]}}));
        assert_eq!(r["skipped"][0]["why"], "no_shared_power_net", "{r}");
        assert!(f(&r["placements"][0]["slidMm"]).abs() > 0.0, "{r}");
        assert_eq!(r["validation"]["ok"], json!(true), "{r}");
    }

    #[test]
    fn a_cap_turns_to_face_the_pin_and_slides_off_a_taken_spot() {
        // C1 turned round (power pad on its right) must turn 180 to face U1's right side; a part on the spot pushes it along
        let mut b = board();
        b["pads"][7]["x"] = json!(-9.5); b["pads"][8]["x"] = json!(-10.5);
        b["components"].as_array_mut().unwrap().push(json!({"ref": "R9", "x": 3.4, "y": 1.0, "rotation": 0, "side": "top"}));
        b["pads"].as_array_mut().unwrap().push(pad("R9", "1", 3.4, 1.0, ""));
        let r = place_decoupling(&json!({"board": b, "args": {"ic": "U1", "caps": ["C1"]}}));
        let p = &r["placements"][0];
        assert_eq!(p["rotation"], json!(180.0), "{r}");
        assert!(f(&p["slidMm"]).abs() > 0.0, "{r}");
        assert_eq!(r["validation"]["ok"], json!(true), "{r}");
        // a bottom part's through-hole pad on the spot pushes it along too; its surface pads do not
        let mut b = board();
        b["components"].as_array_mut().unwrap().push(json!({"ref": "J9", "x": 3.4, "y": 1.0, "rotation": 0, "side": "bottom"}));
        b["pads"].as_array_mut().unwrap().push(pad("J9", "1", 3.4, 1.0, ""));
        assert_eq!(place_decoupling(&json!({"board": b, "args": {"ic": "U1", "caps": ["C1"]}}))["placements"][0]["slidMm"], json!(0.0));
        b["pads"][13]["layer"] = json!("*.Cu");
        b["pads"][13]["drill"] = json!(0.4);
        assert!(f(&place_decoupling(&json!({"board": b, "args": {"ic": "U1", "caps": ["C1"]}}))["placements"][0]["slidMm"]).abs() > 0.0);
    }

    #[test]
    fn the_verb_names_what_it_cannot_do() {
        assert_eq!(place_decoupling(&json!({"board": board(), "args": {}}))["errorCode"], "bad_args");
        assert_eq!(place_decoupling(&json!({"board": board(), "args": {"ic": "U7"}}))["errorCode"], "no_such_part");
        let mut b = board();
        b["components"][2]["side"] = json!("bottom");
        let r = place_decoupling(&json!({"board": b, "args": {"ic": "U1", "caps": ["C1"]}}));
        assert_eq!(r["skipped"][0]["why"], "other_side");
    }

    #[test]
    fn clearance_by_part_box_and_by_pad_copper() {
        // C3's box sits 1.4 mm above U2's
        let r = check_clearance(&json!({"board": board(), "args": {"thresholdMm": 1.5}}));
        assert_eq!(r["pairs"][0]["a"], "C3");
        assert_eq!(r["pairs"][0]["b"], "U2");
        assert!((f(&r["pairs"][0]["distanceMm"]) - 1.4).abs() < 1e-9, "{r}");
        let p = check_clearance(&json!({"board": board(), "args": {"mode": "pads", "refs": ["C3"], "thresholdMm": 2.0}}));
        // same-net VDD pads are skipped; C3.2 (GND) to U2.1 (VDD) is the closest
        assert_eq!((p["pairs"][0]["a"].as_str(), p["pairs"][0]["b"].as_str()), (Some("C3.2"), Some("U2.1")), "{p}");
        assert_eq!(check_clearance(&json!({"board": board(), "args": {"refs": ["Q1"]}}))["errorCode"], "no_such_part");
        // a part under a raised one: pads apart, bodies apart in height, listed as stacked like placement_check does
        let st = json!({"components": [{"ref": "M1", "x": 0, "y": 0, "rotation": 0, "side": "top"}, {"ref": "R1", "x": 0, "y": 0, "rotation": 0, "side": "top"}],
            "pads": [pad("M1", "1", -3.0, 0.0, "A"), pad("M1", "2", 3.0, 0.0, "B"), pad("R1", "1", -0.5, 0.0, "C"), pad("R1", "2", 0.5, 0.0, "D")]});
        let bodies = json!({"bodies": [{"ref": "M1", "side": "top", "x0": -3.5, "y0": -2.0, "x1": 3.5, "y1": 2.0, "z0": 2.0, "z1": 3.0},
                                       {"ref": "R1", "side": "top", "x0": -0.8, "y0": -0.4, "x1": 0.8, "y1": 0.4, "z0": 0.0, "z1": 0.5}]});
        let c = check_clearance(&json!({"board": st, "bodies": bodies, "args": {}}));
        assert_eq!((c["pairs"].as_array().unwrap().len(), c["stacked"].as_array().unwrap().len()), (0, 1), "{c}");
        assert_eq!(check_clearance(&json!({"board": st, "args": {}}))["pairs"].as_array().unwrap().len(), 1);
        assert_eq!(check_clearance(&json!({"board": board(), "args": {"mode": "x"}}))["errorCode"], "bad_args");
    }

    fn bga(pitch: f64, ball: f64) -> Value {
        // a 4 x 4 BGA at the origin, nets N0..N15, plus a stray resistor pad
        let mut pads = Vec::new();
        for i in 0..4 { for j in 0..4 {
            let (x, y) = ((i as f64 - 1.5) * pitch, (j as f64 - 1.5) * pitch);
            pads.push(json!({"ref": "U1", "name": format!("{}{}", (b'A' + j as u8) as char, i + 1), "x": x, "y": y, "sizeX": ball, "sizeY": ball, "rotation": 0, "layer": "F.Cu", "drill": 0, "net": format!("N{}", i * 4 + j), "shape": "round"}));
        } }
        json!({"revision": "r2", "outline": [[-10, -10], [10, -10], [10, 10], [-10, 10]], "components": [{"ref": "U1", "x": 0, "y": 0, "rotation": 0, "side": "top"}], "pads": pads})
    }

    #[test]
    fn a_bga_escapes_its_outer_ring_and_dogbones_the_rest() {
        let r = plan_fanout(&json!({"board": bga(1.0, 0.45), "stack": {"copperLayers": 4}, "args": {"ref": "U1"}}));
        assert_eq!((r["package"].as_str(), r["style"].as_str(), r["escapeLayer"].as_str()), (Some("bga"), Some("dogbone"), Some("In1.Cu")), "{r}");
        let count = |a: &str| r["pads"].as_array().unwrap().iter().filter(|p| p["action"] == a).count();
        assert_eq!((count("surface"), count("dogbone")), (12, 4));
        // B2 sits at (-0.5, -0.5): its via goes half a pitch out, away from the centre, and changes layer there
        let b2 = r["calls"].as_array().unwrap().iter().find(|c| c["paths"][0][0]["pad"] == "U1.B2").unwrap();
        assert_eq!(b2["paths"][0][1], json!({"x": -1.0, "y": -1.0, "layer": "F.Cu"}));
        assert_eq!(b2["paths"][0][2], json!({"x": -1.0, "y": -1.0, "layer": "In1.Cu"}));
        assert_eq!(r["conflicts"], json!([]), "{r}");
        // at 0.5 mm pitch the dogbone does not fit: via in pad
        let fine = plan_fanout(&json!({"board": bga(0.5, 0.3), "args": {"ref": "U1"}}));
        assert_eq!((fine["style"].as_str(), fine["escapeLayer"].as_str()), (Some("via-in-pad"), Some("B.Cu")), "{fine}");
        assert_eq!(plan_fanout(&json!({"board": bga(0.5, 0.3), "args": {"ref": "U1", "style": "dogbone"}}))["warnings"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_qfn_takes_stubs_a_ground_via_and_a_via_grid_and_names_what_it_would_hit() {
        let mut pads = Vec::new();
        for k in 0..4 { pads.push(json!({"ref": "U2", "name": format!("{}", k + 1), "x": -2.0, "y": 0.75 - k as f64 * 0.5, "sizeX": 0.8, "sizeY": 0.25, "rotation": 0, "layer": "F.Cu", "drill": 0, "net": if k == 3 { "GND".to_string() } else { format!("S{k}") }, "shape": "rect"})); }
        for k in 0..4 { pads.push(json!({"ref": "U2", "name": format!("{}", k + 5), "x": 2.0, "y": -0.75 + k as f64 * 0.5, "sizeX": 0.8, "sizeY": 0.25, "rotation": 0, "layer": "F.Cu", "drill": 0, "net": format!("T{k}"), "shape": "rect"})); }
        pads.push(json!({"ref": "U2", "name": "9", "x": 0.0, "y": 0.0, "sizeX": 2.6, "sizeY": 2.6, "rotation": 0, "layer": "F.Cu", "drill": 0, "net": "GND", "shape": "rect"}));
        // R1's pad right where U2.5's stub ends
        pads.push(json!({"ref": "R1", "name": "1", "x": 2.9, "y": -0.75, "sizeX": 0.5, "sizeY": 0.5, "rotation": 0, "layer": "F.Cu", "drill": 0, "net": "X", "shape": "rect"}));
        let b = json!({"components": [{"ref": "U2", "x": 0, "y": 0, "rotation": 0, "side": "top"}, {"ref": "R1", "x": 2.9, "y": -0.75, "rotation": 0, "side": "top"}], "pads": pads});
        let r = plan_fanout(&json!({"board": b, "args": {"ref": "U2"}}));
        assert_eq!((r["package"].as_str(), r["pitchMm"].as_f64()), (Some("perimeter"), Some(0.5)), "{r}");
        let action = |pad: &str| r["pads"].as_array().unwrap().iter().find(|p| p["pad"] == pad).unwrap()["action"].clone();
        assert_eq!((action("U2.1"), action("U2.4"), action("U2.9")), (json!("surface"), json!("stub+via"), json!("thermal-via-grid")));
        // U2.1 at x -2 with a 0.8 mm pad: 0.4 + the 0.4 stub reaches x -2.8
        assert_eq!(r["pads"][0]["stubEnd"], json!([-2.8, 0.75]));
        assert_eq!(r["conflicts"].as_array().unwrap().iter().map(|c| c["pad"].as_str().unwrap()).collect::<Vec<_>>(), vec!["R1.1"], "{r}");
        assert_eq!(plan_fanout(&json!({"board": b, "args": {"ref": "R1"}}))["errorCode"], "not_a_package");
        // on a routed board (altium-lane #22): U2.1 already has its track, so it is skipped; another net's track across
        // U2.2's stub is a conflict; a via in the exposed pad's centre keeps the grid off it
        let mut routed = b.clone();
        routed["tracks"] = json!([{"net": "S0", "layer": "F.Cu", "width": 0.2, "start": {"x": -2.0, "y": 0.75}, "end": {"x": -4.0, "y": 0.75}},
                                  {"net": "Q", "layer": "F.Cu", "width": 0.2, "start": {"x": -2.6, "y": -2.0}, "end": {"x": -2.6, "y": 0.4}}]);
        routed["vias"] = json!([{"net": "GND", "size": 0.6, "drill": 0.3, "position": {"x": 0.0, "y": 0.0}}]);
        let r = plan_fanout(&json!({"board": routed, "args": {"ref": "U2"}}));
        assert_eq!(r["pads"][0]["why"], "already routed", "{r}");
        assert!(r["conflicts"].as_array().unwrap().iter().any(|c| c["with"] == "track" && c["net"] == "Q" && c["from"] == "U2.2"), "{r}");
        let grid = r["pads"].as_array().unwrap().iter().find(|p| p["action"] == "thermal-via-grid").unwrap();
        assert!(grid["keptClearOfHoles"].as_u64().unwrap() >= 1, "{grid}");
        assert!(grid["vias"].as_array().unwrap().iter().all(|v| (f(&v[0]).powi(2) + f(&v[1]).powi(2)).sqrt() >= 0.3 + 0.3 + 0.1 - 1e-9));
        assert_eq!(plan_fanout(&json!({"board": routed, "args": {"ref": "U2", "includeRouted": true}}))["pads"][0]["action"], "surface");
        assert_eq!(plan_fanout(&json!({"board": b, "args": {"ref": "U2", "escapeLayer": "In3.Cu"}}))["errorCode"], "no_such_layer");
    }

    #[test]
    fn pad_copper_gaps_are_exact() {
        let a = pad_shape(&json!({"x": 0.0, "y": 0.0, "sizeX": 1.0, "sizeY": 1.0, "shape": "rect"}));
        let b = pad_shape(&json!({"x": 2.0, "y": 0.0, "sizeX": 1.0, "sizeY": 1.0, "shape": "round"}));
        assert!((a.gap(&b) - 1.0).abs() < 1e-9);
        let inner = pad_shape(&json!({"x": 0.0, "y": 0.0, "sizeX": 0.2, "sizeY": 0.2, "shape": "rect"}));
        assert_eq!(a.gap(&inner), 0.0);
        assert_eq!(inner.gap(&a), 0.0);
    }
}

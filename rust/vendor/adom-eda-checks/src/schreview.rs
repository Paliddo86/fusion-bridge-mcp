//! A schematic review before the board is updated: every placed pin from the sheets (with its electrical type) against
//! the compiled netlist, and the symbols' pins against the footprints' pads. From the Altium bridge's schreview.rs. Unconnected pins (power pins as errors), nets with one pin, outputs driving each
//! other, inputs nothing drives, and IC power pins with no decoupling capacitor to ground. The design checks after it
//! (I2C pull-ups, reset and enable pins, LED resistors, regulator and bulk capacitors, annotation, no-connect flags on
//! wired pins, two names on one net) are ported from kicad-bridge's design_review and sch_check (#29).
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Altium's pin electrical types (the ELECTRICAL field).
pub fn electrical(n: i64) -> &'static str {
    match n { 0 => "input", 1 => "io", 2 => "output", 3 => "open_collector", 4 => "passive", 5 => "hiz", 6 => "open_emitter", 7 => "power", _ => "unknown" }
}

/// A pin's type from the sheet model: Altium's code, or the name the standard uses (Fusion also sends `supply`, a
/// supply symbol's pin that drives its net, and `not_connected`).
pub fn pin_type(v: &Value) -> String {
    match v {
        Value::Number(n) => electrical(n.as_i64().unwrap_or(-1)).to_string(),
        Value::String(t) => match t.to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "bidirectional" | "bidi" | "i/o" => "io".into(),
            "pwr" => "power".into(),
            "sup" => "supply".into(),
            "nc" => "not_connected".into(),
            "oc" | "opencollector" => "open_collector".into(),
            "pas" => "passive".into(),
            "tri" | "tristate" | "3state" => "hiz".into(),
            "in" => "input".into(),
            "out" => "output".into(),
            // KiCad's names
            "power_in" => "power".into(),
            "power_out" => "supply".into(),
            "tri_state" => "hiz".into(),
            "no_connect" | "unconnected" => "not_connected".into(),
            "unspecified" | "free" => "unknown".into(),
            o => o.to_string(),
        },
        _ => "unknown".into(),
    }
}

pub fn is_ground(net: &str) -> bool {
    let n = net.to_ascii_uppercase();
    n == "GND" || n == "0V" || n == "VSS" || n == "AGND" || n == "DGND" || n == "PGND" || n == "GNDA" || n == "GNDD" || n.starts_with("GND_") || n.ends_with("_GND")
}

fn prefix(des: &str) -> String { des.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_uppercase() }

/// The review, from the parts (read_sheet_file components: designator, pins with number/name/electrical) and the nets
/// ({net: ["U1.8", ...]}). Pure.
pub fn review(parts: &[Value], nets: &Map<String, Value>) -> Value {
    // pin -> net
    let mut net_of: BTreeMap<String, String> = BTreeMap::new();
    for (n, ps) in nets { for p in ps.as_array().cloned().unwrap_or_default() { if let Some(s) = p.as_str() { net_of.insert(s.to_ascii_uppercase(), n.clone()); } } }
    // every placed pin, once (a multi-part chip appears once per part on the sheets)
    let mut pins: BTreeMap<String, (String, String, String, String)> = BTreeMap::new(); // key -> (designator, number, name, type)
    for c in parts {
        let des = c["designator"].as_str().unwrap_or("").to_string();
        if des.is_empty() || des.contains('?') { continue; }
        // a supply symbol with no package (Fusion's SUPPLY1..) joins its net by pin name and has no pin numbers: it is
        // no part on the board (fusion-lane)
        if c["pins"].as_array().map(|a| !a.is_empty() && a.iter().all(|p| p["number"].as_str().map(|n| n.trim().is_empty()).unwrap_or(true))).unwrap_or(false) { continue; }
        for p in c["pins"].as_array().cloned().unwrap_or_default() {
            let num = p["number"].as_str().unwrap_or("").to_string();
            pins.entry(format!("{des}.{num}").to_ascii_uppercase()).or_insert((des.clone(), num, p["name"].as_str().unwrap_or("").to_string(), pin_type(&p["electrical"])));
        }
    }
    let nc = |name: &str| { let n = name.to_ascii_uppercase(); n == "NC" || n == "N/C" || n == "DNC" || n.starts_with("NC_") || n == "NO CONNECT" };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut infos = Vec::new();
    let found = design_checks(parts, nets, &pins, &net_of);
    // a reset or enable pin reported floating is not reported again as an unconnected pin
    let floating: BTreeSet<String> = found.iter().filter(|(_, f)| f["check"] == "reset_enable" && f["floating"] == true).filter_map(|(_, f)| f["pin"].as_str().map(|s| s.to_ascii_uppercase())).collect();
    for (sev, f) in found { match sev { "error" => errors.push(f), "warning" => warnings.push(f), _ => infos.push(f) } }
    for (key, (des, num, name, ty)) in &pins {
        if net_of.contains_key(key) || nc(name) || ty == "not_connected" || floating.contains(key) { continue; }
        let f = json!({"check": "unconnected_pin", "pin": format!("{des}.{num}"), "name": name, "type": ty});
        if ty == "power" { errors.push(f) } else { warnings.push(f) }
    }
    for (n, ps) in nets {
        let members = ps.as_array().map(|a| a.len()).unwrap_or(0);
        if members == 1 { warnings.push(json!({"check": "single_pin_net", "net": n, "pin": ps[0]})); }
        let kinds: Vec<&str> = ps.as_array().cloned().unwrap_or_default().iter().filter_map(|p| p.as_str()).filter_map(|p| pins.get(&p.to_ascii_uppercase())).map(|t| t.3.as_str()).collect();
        let outs = kinds.iter().filter(|k| **k == "output").count();
        if outs >= 2 { errors.push(json!({"check": "outputs_together", "net": n, "outputs": outs})); }
        let driven = kinds.iter().any(|k| matches!(*k, "output" | "io" | "power" | "supply" | "passive" | "open_collector" | "open_emitter" | "hiz" | "unknown"));
        if !kinds.is_empty() && kinds.iter().all(|k| *k == "input") && !driven && members > 0 {
            warnings.push(json!({"check": "undriven_input", "net": n, "inputs": kinds.len()}));
        }
    }
    // decoupling: each IC power pin on a supply net (not ground) wants a capacitor between that net and a ground
    let caps_between: BTreeMap<String, Vec<String>> = {
        let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut cap_nets: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (key, (des, ..)) in &pins {
            if prefix(des) == "C" { if let Some(n) = net_of.get(key) { cap_nets.entry(des.clone()).or_default().insert(n.clone()); } }
        }
        for (c, ns) in cap_nets {
            if ns.iter().any(|n| is_ground(n)) { for n in ns.iter().filter(|n| !is_ground(n)) { m.entry(n.clone()).or_default().push(c.clone()); } }
        }
        m
    };
    let mut supplies: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (key, (des, _, _, ty)) in &pins {
        if ty != "power" || !matches!(prefix(des).as_str(), "U" | "IC") { continue; }
        if let Some(n) = net_of.get(key).filter(|n| !is_ground(n)) { supplies.entry(n.clone()).or_default().insert(des.clone()); }
    }
    let mut decoupling = Vec::new();
    for (net, ics) in &supplies {
        let caps = caps_between.get(net).cloned().unwrap_or_default();
        // an inductor on a 'supply' pin's net makes it a switch node (a regulator's SW/LX pin): no capacitor belongs
        // there (the Bluetooth Sentinel's SMPS_LX read as missing decoupling)
        let switch = nets.get(net).and_then(|p| p.as_array()).map(|a| a.iter().filter_map(|p| p.as_str()).any(|p| prefix(p.split('.').next().unwrap_or("")) == "L")).unwrap_or(false);
        let row = json!({"net": net, "ics": ics, "capacitors": caps, "ok": !caps.is_empty() || switch, "switchNode": switch});
        if caps.is_empty() && !switch { warnings.push(json!({"check": "no_decoupling", "net": net, "ics": ics})); }
        decoupling.push(row);
    }
    json!({"pins": pins.len(), "nets": nets.len(), "errors": errors, "warnings": warnings, "infos": infos, "decoupling": decoupling, "ok": errors.is_empty()})
}

/// A net name's last segment, upper case (KiCad's hierarchical "/sheet/SDA" reads as SDA).
fn tail(net: &str) -> String { net.rsplit('/').next().unwrap_or(net).to_ascii_uppercase() }

/// A supply rail by its name: +3V3, VCC, VDD..., 5V, 3V3, 1V8.
pub fn looks_like_rail(net: &str) -> bool {
    let n = tail(net);
    if n.starts_with('+') || (n.starts_with('-') && n.chars().nth(1).map_or(false, |c| c.is_ascii_digit())) { return true; }
    if ["VCC", "VDD", "VBUS", "VBAT", "VIN", "VSYS", "VREF", "VIO", "VAA", "AVDD", "DVDD", "IOVDD", "VMOT", "VOUT", "VREG"].iter().any(|p| n.starts_with(p)) { return true; }
    let core = n.trim_start_matches('V');
    let digits = core.chars().take_while(|c| c.is_ascii_digit()).count();
    digits > 0 && core.chars().nth(digits) == Some('V')
}

/// A pin name without overbar markup and separators: `~{RESET}` and `/RESET` read RESET.
pub fn norm_pin(name: &str) -> String { name.replace("~{", "").replace(['}', '~', '/', '#', ' '], "").to_ascii_uppercase() }

/// Farads from a value like 100n, 4.7uF, 10µ, 1u0 or 22pF; None when unreadable (a bare number has no unit).
pub fn farads(v: &str) -> Option<f64> {
    let s = v.trim().replace(['µ', 'μ'], "u").to_ascii_lowercase();
    let s = s.split([' ', '/', ',']).next().unwrap_or("").trim_end_matches('f').to_string();
    let (mut num, mut mult, mut unit) = (String::new(), 1.0, false);
    for c in s.chars() {
        if c.is_ascii_digit() || c == '.' { num.push(c); }
        else if !unit {
            mult = match c { 'p' => 1e-12, 'n' => 1e-9, 'u' => 1e-6, 'm' => 1e-3, _ => return None };
            if !num.contains('.') { num.push('.'); }
            unit = true;
        } else { return None; }
    }
    if !unit { return None; }
    num.trim_end_matches('.').parse::<f64>().ok().map(|x| x * mult)
}

fn fmt_farads(f: f64) -> String {
    let (v, u) = if f >= 1e-6 { (f / 1e-6, "u") } else if f >= 1e-9 { (f / 1e-9, "n") } else { (f / 1e-12, "p") };
    format!("{}{u}", ((v * 1000.0).round() / 1000.0))
}

#[derive(Clone, Copy, PartialEq)]
enum Kind { Capacitor, Resistor, Led, Regulator, Ic, Connector, Other }

/// One part with every unit's pins merged: (designator, library id, value, [(number, name, type)]).
struct Part { des: String, lib: String, value: String, pins: Vec<(String, String, String)> }

fn kind(p: &Part) -> Kind {
    let pre = prefix(&p.des);
    let lib = p.lib.to_ascii_lowercase();
    let sym = lib.rsplit(':').next().unwrap_or(&lib).to_string();
    let val = p.value.to_ascii_lowercase();
    if (lib.contains("led") && (pre == "D" || pre == "LED")) || sym.starts_with("led") || pre == "LED" { return Kind::Led; }
    if pre == "C" || sym == "c" || sym.starts_with("c_") || sym == "cp" || sym.starts_with("cp_") { return Kind::Capacitor; }
    if pre == "R" || sym == "r" || sym.starts_with("r_") { return Kind::Resistor; }
    if matches!(pre.as_str(), "J" | "P" | "CN") || lib.starts_with("connector") { return Kind::Connector; }
    if lib.starts_with("regulator_") || lib.contains("regulator") || val.contains("ldo") || val.contains("1117") { return Kind::Regulator; }
    if pre == "U" || pre == "IC" {
        let has_in = p.pins.iter().any(|x| matches!(norm_pin(&x.1).as_str(), "VIN" | "VI" | "IN"));
        let has_out = p.pins.iter().any(|x| matches!(norm_pin(&x.1).as_str(), "VOUT" | "VO" | "OUT") && x.2 == "supply");
        return if has_in && has_out { Kind::Regulator } else { Kind::Ic };
    }
    Kind::Other
}

/// The design checks: (severity, finding). Each finding carries check, refs, nets, message and fix. Pure.
fn design_checks(parts: &[Value], nets: &Map<String, Value>, pins: &BTreeMap<String, (String, String, String, String)>, net_of: &BTreeMap<String, String>) -> Vec<(&'static str, Value)> {
    let s = |v: &Value, k: &[&str]| k.iter().find_map(|k| v[*k].as_str().filter(|x| !x.is_empty())).unwrap_or("").to_string();
    // parts, units merged by designator
    let mut by: BTreeMap<String, Part> = BTreeMap::new();
    for (key, (des, num, name, ty)) in pins {
        let _ = key;
        let e = by.entry(des.clone()).or_insert_with(|| Part { des: des.clone(), lib: String::new(), value: String::new(), pins: Vec::new() });
        e.pins.push((num.clone(), name.clone(), ty.clone()));
    }
    for c in parts {
        let des = s(c, &["designator"]);
        if let Some(e) = by.get_mut(&des) {
            if e.lib.is_empty() { e.lib = s(c, &["libRef", "libId", "lib", "symbol", "library"]); }
            if e.value.is_empty() { e.value = s(c, &["value", "comment"]); }
        }
    }
    let kinds: BTreeMap<String, Kind> = by.values().map(|p| (p.des.clone(), kind(p))).collect();
    let net_pin = |des: &str, num: &str| net_of.get(&format!("{des}.{num}").to_ascii_uppercase()).cloned();
    let members = |net: &str| -> Vec<(String, String)> {
        nets.get(net).and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|p| p.as_str()).filter_map(|p| p.rsplit_once('.').map(|(d, n)| (d.to_string(), n.to_string()))).collect()).unwrap_or_default()
    };
    let single = |net: &Option<String>| net.as_ref().map(|n| members(n).len() <= 1).unwrap_or(true);
    // two-terminal parts of a kind on a net: (designator, the net at its other end)
    let two_terminal = |net: &str, k: Kind| -> Vec<(String, String)> {
        by.values().filter(|p| kinds[&p.des] == k && p.pins.len() == 2).filter_map(|p| {
            let (a, b) = (net_pin(&p.des, &p.pins[0].0)?, net_pin(&p.des, &p.pins[1].0)?);
            if a == net { Some((p.des.clone(), b)) } else if b == net { Some((p.des.clone(), a)) } else { None }
        }).collect()
    };
    let caps_to_ground = |net: &str| -> Vec<(String, Option<f64>)> {
        two_terminal(net, Kind::Capacitor).into_iter().filter(|(_, o)| is_ground(o)).map(|(d, _)| { let v = farads(&by[&d].value); (d, v) }).collect()
    };
    let rails: BTreeSet<String> = nets.keys().filter(|n| !is_ground(n) && (looks_like_rail(n) || members(n).iter().any(|(d, k)| pins.get(&format!("{d}.{k}").to_ascii_uppercase()).map(|t| t.3 == "supply").unwrap_or(false)))).cloned().collect();
    let refs_on = |net: &str| -> Vec<String> { members(net).into_iter().map(|(d, _)| d).collect::<BTreeSet<_>>().into_iter().collect() };
    let mut out: Vec<(&'static str, Value)> = Vec::new();
    let mut add = |sev: &'static str, check: &str, refs: Vec<String>, nets: Vec<String>, message: String, fix: String, extra: Value| {
        let mut f = json!({"check": check, "refs": refs, "nets": nets, "message": message, "fix": fix});
        if let Some(o) = extra.as_object() { for (k, v) in o { f[k] = v.clone(); } }
        out.push((sev, f));
    };

    // i2c_pullups: an SDA or SCL line with no resistor to a supply rail
    for net in nets.keys() {
        let t = tail(net);
        let named = t.split(|c: char| !c.is_ascii_alphanumeric()).any(|x| x.starts_with("SDA") || x.starts_with("SCL"));
        let by_pin = members(net).iter().any(|(d, k)| pins.get(&format!("{d}.{k}").to_ascii_uppercase()).map(|t| matches!(norm_pin(&t.2).as_str(), "SDA" | "SCL" | "I2C_SDA" | "I2C_SCL")).unwrap_or(false));
        if !(named || by_pin) || members(net).len() <= 1 { continue; }
        if two_terminal(net, Kind::Resistor).iter().any(|(_, o)| rails.contains(o)) { continue; }
        add("warning", "i2c_pullups", refs_on(net), vec![net.clone()], format!("I2C line {net} has no pull-up resistor to a supply rail"),
            format!("I2C drivers only pull low: add a resistor from {net} to the I/O rail (4.7k for 100 kHz, 2.2k for 400 kHz), once per bus, unless the pull-up is on another board."), json!({}));
    }

    // reset_enable: a reset, enable or boot pin floating, or with nothing setting its level
    const NAMES: &[&str] = &["RESET", "RST", "NRST", "RESETN", "RSTN", "NRESET", "MCLR", "EN", "ENABLE", "CE", "CHIP_EN", "CHIPEN", "SHDN", "NSHDN", "SHDNN", "PWRDN", "ON", "ONOFF", "RUN"];
    for p in by.values().filter(|p| matches!(kinds[&p.des], Kind::Ic | Kind::Regulator)) {
        for (num, name, ty) in &p.pins {
            let n = norm_pin(name);
            if !(NAMES.contains(&n.as_str()) || n.starts_with("RESET") || n.starts_with("NRST") || n.starts_with("RST")) || matches!(ty.as_str(), "output" | "supply" | "not_connected") { continue; }
            let net = net_pin(&p.des, num);
            let pin = format!("{}.{num}", p.des);
            if single(&net) {
                add("error", "reset_enable", vec![p.des.clone()], net.iter().cloned().collect(), format!("{pin} {name} is floating"),
                    format!("Tie {pin} to its active state (the supply for an enable that is always on, or a 10k pull-up with 100 nF to ground for a reset), or drive it from a GPIO."), json!({"pin": pin, "floating": true}));
                continue;
            }
            let net = net.unwrap();
            let defined = rails.contains(&net) || is_ground(&net) || members(&net).iter().any(|(d, k)| {
                let e = pins.get(&format!("{d}.{k}").to_ascii_uppercase()).map(|t| t.3.as_str()).unwrap_or("");
                (*d != p.des && matches!(e, "output" | "io" | "open_collector" | "open_emitter" | "supply" | "hiz")) || matches!(kinds.get(d), Some(Kind::Resistor) | Some(Kind::Connector))
            });
            if !defined {
                add("warning", "reset_enable", vec![p.des.clone()], vec![net.clone()], format!("Nothing sets the level of {pin} {name} (net {net})"),
                    format!("Add a pull resistor on {net} to the inactive level, or drive it from a GPIO or a supervisor."), json!({"pin": pin}));
            }
        }
    }

    // led_resistor: an LED with no series resistor on either side
    for p in by.values().filter(|p| kinds[&p.des] == Kind::Led) {
        let pn: Vec<String> = p.pins.iter().filter_map(|x| net_pin(&p.des, &x.0)).collect();
        let limited = pn.iter().any(|n| members(n).iter().any(|(d, _)| *d != p.des && kinds.get(d) == Some(&Kind::Resistor)));
        if !limited {
            add("warning", "led_resistor", vec![p.des.clone()], pn, format!("LED {} has no series resistor on either side", p.des),
                format!("Put a resistor in series with {}: R = (Vsupply - Vf) / I, for example 330R to 1k at 3.3 V; skip it only if a constant-current driver feeds the LED.", p.des), json!({}));
        }
    }

    // regulator_caps: a regulator with no input or output capacitor, or too little on the output
    for p in by.values().filter(|p| kinds[&p.des] == Kind::Regulator) {
        let outs: BTreeSet<String> = p.pins.iter().filter(|x| x.2 == "supply" || matches!(norm_pin(&x.1).as_str(), "VOUT" | "VO" | "OUT")).filter_map(|x| net_pin(&p.des, &x.0)).collect();
        let ins: BTreeSet<String> = p.pins.iter().filter(|x| x.2 == "power" || matches!(norm_pin(&x.1).as_str(), "VIN" | "VI" | "IN")).filter_map(|x| net_pin(&p.des, &x.0)).filter(|n| !is_ground(n) && !outs.contains(n)).collect();
        for net in ins {
            if caps_to_ground(&net).is_empty() {
                add("warning", "regulator_caps", vec![p.des.clone()], vec![net.clone()], format!("Regulator {} has no input capacitor on {net}", p.des),
                    format!("Add the input capacitor the datasheet asks for (typically 1 uF to 10 uF ceramic) from {net} to ground beside {}.", p.des), json!({}));
            }
        }
        for net in outs {
            let caps = caps_to_ground(&net);
            let biggest = caps.iter().filter_map(|c| c.1).fold(None, |a: Option<f64>, f| Some(a.map_or(f, |x| x.max(f))));
            if caps.is_empty() {
                add("warning", "regulator_caps", vec![p.des.clone()], vec![net.clone()], format!("Regulator {} has no output capacitor on {net}", p.des),
                    format!("Most LDOs need a minimum output capacitance to be stable: add the datasheet's (typically 1 uF to 10 uF ceramic) from {net} to ground beside {}.", p.des), json!({}));
            } else if caps.iter().all(|c| c.1.is_some()) && biggest.map_or(false, |b| b < 0.99e-6) {
                add("warning", "regulator_caps", vec![p.des.clone()], vec![net.clone()], format!("Regulator {} output {net} has only {} of capacitance", p.des, fmt_farads(biggest.unwrap_or(0.0))),
                    format!("Add a 1 uF to 10 uF ceramic from {net} to ground beside {} (check the datasheet's minimum and ESR range).", p.des), json!({}));
            }
        }
    }

    // bulk_capacitance: a rail feeding ICs with no capacitor of 4.7 uF or more
    for net in &rails {
        let loads = members(net).iter().filter(|(d, k)| matches!(kinds.get(d), Some(Kind::Ic) | Some(Kind::Regulator)) && pins.get(&format!("{d}.{k}").to_ascii_uppercase()).map(|t| t.3 == "power").unwrap_or(false)).count();
        if loads == 0 { continue; }
        let caps = caps_to_ground(net);
        if caps.iter().any(|c| c.1.map_or(true, |f| f >= 4.7e-6 * 0.99)) { continue; }
        let biggest = caps.iter().filter_map(|c| c.1).fold(0.0, f64::max);
        let message = if caps.is_empty() { format!("Rail {net} feeds {loads} IC pin(s) but has no capacitance to ground") } else { format!("Rail {net} feeds {loads} IC pin(s) but has no bulk capacitor (largest is {})", fmt_farads(biggest)) };
        add("info", "bulk_capacitance", caps.iter().map(|c| c.0.clone()).collect(), vec![net.clone()], message,
            format!("Add a 10 uF (or larger) capacitor from {net} to ground where the rail enters the board or leaves its regulator."), json!({}));
    }

    // annotation: a designator not yet annotated, or one designator on two different symbols (one footprint on the board)
    let mut unannotated = BTreeSet::new();
    let mut units: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut pinsets: BTreeMap<String, Vec<BTreeSet<String>>> = BTreeMap::new();
    for c in parts {
        let des = s(c, &["designator"]);
        if des.is_empty() || des.contains('?') { unannotated.insert(if des.is_empty() { "A symbol".to_string() } else { des }); continue; }
        let unit = s(c, &["unit", "partId", "subpart"]);
        if !unit.is_empty() { *units.entry((des.clone(), unit)).or_default() += 1; }
        else { pinsets.entry(des).or_default().push(c["pins"].as_array().map(|a| a.iter().filter_map(|p| p["number"].as_str().map(String::from)).collect()).unwrap_or_default()); }
    }
    for d in unannotated {
        add("error", "annotation", vec![d.clone()], vec![], format!("{d} is not annotated"), "Annotate the schematic, or give the part a designator, before updating the board: an unannotated part never becomes a footprint.".into(), json!({}));
    }
    let mut dups: BTreeMap<String, usize> = units.into_iter().filter(|(_, n)| *n > 1).map(|((d, _), n)| (d, n)).collect();
    // with no unit field, two symbols sharing a designator and a pin number are two parts, not two units of one
    for (d, sets) in pinsets { if sets.len() > 1 && (0..sets.len()).any(|i| (i + 1..sets.len()).any(|j| !sets[i].is_disjoint(&sets[j]))) { dups.insert(d, sets.len()); } }
    for (d, n) in dups {
        add("error", "annotation", vec![d.clone()], vec![], format!("Reference {d} is used by {n} different symbols"), format!("Renumber one of them or re-annotate: two parts with one designator become one footprint and their nets merge."), json!({}));
    }

    // no_connect_connected: a pin flagged no-connect that is wired to something
    for (key, (des, num, name, ty)) in pins {
        if ty != "not_connected" { continue; }
        let Some(net) = net_of.get(key) else { continue };
        if members(net).len() > 1 {
            add("warning", "no_connect_connected", vec![des.clone()], vec![net.clone()], format!("No-connect flag on {des}.{num} {name} but the pin is wired to {net}"),
                "Either remove the no-connect flag or the wire: the pin is marked unused but carries a net.".into(), json!({"pin": format!("{des}.{num}")}));
        }
    }
    out
}

/// Symbol pins against footprint pads, part by part: pins with no pad (a net that can never reach copper), pads with no
/// pin (copper no net reaches: fine for mounting and mechanical pads), and parts on only one side. Pure.
pub fn pin_pad(parts: &[Value], pads: &[Value], board_refs: &[String]) -> Value {
    let mut pins: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for c in parts {
        let des = c["designator"].as_str().unwrap_or("").to_string();
        if des.is_empty() || des.contains('?') { continue; }
        // a supply symbol with no package (Fusion's SUPPLY1..) joins its net by pin name and has no pin numbers: it is
        // no part on the board (fusion-lane)
        if c["pins"].as_array().map(|a| !a.is_empty() && a.iter().all(|p| p["number"].as_str().map(|n| n.trim().is_empty()).unwrap_or(true))).unwrap_or(false) { continue; }
        let e = pins.entry(des).or_default();
        for p in c["pins"].as_array().cloned().unwrap_or_default() { if let Some(n) = p["number"].as_str() { e.insert(n.to_ascii_uppercase()); } }
    }
    let mut padset: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for p in pads {
        let (Some(r), Some(n)) = (p["ref"].as_str().filter(|r| !r.is_empty()), p["name"].as_str()) else { continue };
        padset.entry(r.to_string()).or_default().insert(n.to_ascii_uppercase());
    }
    let mut mismatches = Vec::new();
    let mut ok = 0;
    for (des, ps) in &pins {
        let Some(pd) = padset.get(des) else { continue };
        let no_pad: Vec<&String> = ps.iter().filter(|n| !pd.contains(*n)).collect();
        // unnamed or '0' pads and mounting pads (MH*, EP*, SH*, MP*) carry no pin by design
        let no_pin: Vec<&String> = pd.iter().filter(|n| !ps.contains(*n) && !n.is_empty() && *n != "0" && !["MH", "MP", "SH", "EP", "PAD", "TAB"].iter().any(|m| n.starts_with(m))).collect();
        if no_pad.is_empty() && no_pin.is_empty() { ok += 1; continue; }
        mismatches.push(json!({"ref": des, "pinsWithoutPad": no_pad, "padsWithoutPin": no_pin, "pins": ps.len(), "pads": pd.len()}));
    }
    let on_board: BTreeSet<String> = board_refs.iter().cloned().collect();
    let sheet_only: Vec<&String> = pins.keys().filter(|d| !on_board.contains(*d) && !padset.contains_key(*d)).collect();
    let board_only: Vec<&String> = on_board.iter().filter(|d| !pins.contains_key(*d)).collect();
    json!({"partsChecked": ok + mismatches.len(), "matching": ok, "mismatches": mismatches, "onSheetsOnly": sheet_only, "onBoardOnly": board_only,
           "ok": mismatches.iter().all(|m| m["pinsWithoutPad"].as_array().map(|a| a.is_empty()).unwrap_or(true))})
}

/// The sheet parts companion (read_sheet_file's components, each with its pins): a list, or a reply with `components`.
fn sheet_parts(input: &Value) -> Option<Vec<Value>> {
    let p = crate::input::part(input, "sheetParts")?;
    p.get("components").or(Some(&p)).and_then(|x| x.as_array()).cloned()
}

type Pt = (f64, f64);
fn pt(v: &Value) -> Option<Pt> { Some((v.get(0)?.as_f64()?, v.get(1)?.as_f64()?)) }
fn d2(a: Pt, b: Pt) -> f64 { ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt() }
fn on_seg(p: Pt, a: Pt, b: Pt) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    d2(p, (a.0 + t * dx, a.1 + t * dy))
}
fn r2(v: f64) -> f64 { (v * 100.0).round() / 100.0 }

/// The checks that need a sheet's drawing (kicad-bridge's sch_check, #29), from the sheetDrawing companion: one entry
/// per sheet {sheet, wires [{a, b}], buses?, junctions [[x, y]], noConnects, labels [{text, kind, at}], pins [{ref,
/// number, at}], symbols [{ref, box: [x0, y0, x1, y1]}]}, mm, y down. A point connects when it lies within 0.01 mm of
/// another item; a wire end on another wire's middle connects only with a junction there. Pure.
pub fn drawing_checks(sheets: &[Value]) -> Vec<Value> {
    const TOL: f64 = 0.01;
    let mut out = Vec::new();
    for sh in sheets {
        let name = sh["sheet"].as_str().unwrap_or("").to_string();
        let list = |k: &str| sh[k].as_array().cloned().unwrap_or_default();
        let wires: Vec<(Pt, Pt)> = list("wires").iter().filter_map(|w| Some((pt(&w["a"])?, pt(&w["b"])?))).collect();
        let points = |k: &str| -> Vec<Pt> { list(k).iter().filter_map(pt).collect() };
        let (junctions, ncs) = (points("junctions"), points("noConnects"));
        let labels: Vec<(String, String, Pt)> = list("labels").iter().filter_map(|l| Some((l["text"].as_str()?.to_string(), l["kind"].as_str().unwrap_or("local").to_string(), pt(&l["at"])?))).collect();
        let pins: Vec<(String, Pt)> = list("pins").iter().filter_map(|p| Some((format!("{}.{}", p["ref"].as_str()?, p["number"].as_str().map(String::from).or_else(|| p["number"].as_i64().map(|n| n.to_string()))?), pt(&p["at"])?))).collect();
        let near = |p: Pt, q: Pt| d2(p, q) <= TOL;
        // dangling wire ends
        for (i, (a, b)) in wires.iter().enumerate() {
            for e in [*a, *b] {
                let joined = pins.iter().any(|x| near(e, x.1)) || labels.iter().any(|x| near(e, x.2)) || junctions.iter().any(|j| near(e, *j)) || ncs.iter().any(|n| near(e, *n))
                    || wires.iter().enumerate().any(|(k, (c, d))| k != i && (near(e, *c) || near(e, *d) || (on_seg(e, *c, *d) <= TOL && junctions.iter().any(|j| near(e, *j)))));
                if joined { continue; }
                let mut cand: Vec<(f64, String, Pt)> = pins.iter().map(|x| (d2(e, x.1), format!("pin {}", x.0), x.1)).chain(labels.iter().map(|x| (d2(e, x.2), format!("label {}", x.0), x.2))).filter(|c| c.0 <= 1.27).collect();
                cand.sort_by(|x, y| x.0.total_cmp(&y.0));
                let hint = cand.first().map(|c| format!("; {} is {} mm away at ({}, {})", c.1, r2(c.0), r2(c.2 .0), r2(c.2 .1))).unwrap_or_default();
                out.push(json!({"check": "dangling_wire_end", "sheet": name, "at": [r2(e.0), r2(e.1)], "refs": [], "nets": [],
                    "message": format!("Wire end at ({}, {}) connects to nothing{hint}", r2(e.0), r2(e.1)),
                    "fix": "Extend the wire onto the pin or label it was meant for (a pin's end, not its middle), or delete the stub."}));
            }
        }
        // labels touching no wire or pin
        for (text, kind, at) in &labels {
            if wires.iter().any(|(a, b)| on_seg(*at, *a, *b) <= TOL) || pins.iter().any(|p| near(*at, p.1)) { continue; }
            out.push(json!({"check": "floating_label", "sheet": name, "at": [r2(at.0), r2(at.1)], "refs": [], "nets": [text], "kind": kind,
                "message": format!("Label {text} at ({}, {}) touches no wire or pin", r2(at.0), r2(at.1)),
                "fix": "Move the label onto its wire (its anchor point must sit on the wire), or delete it: a label off its wire names nothing."}));
        }
        // symbols drawn over each other (more than 0.1 mm both ways)
        let syms: Vec<(String, [f64; 4])> = list("symbols").iter().filter_map(|s| {
            let b = s["box"].as_array()?;
            let v: Vec<f64> = b.iter().filter_map(|x| x.as_f64()).collect();
            (v.len() == 4).then(|| (s["ref"].as_str().unwrap_or("").to_string(), [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])]))
        }).collect();
        for i in 0..syms.len() {
            for j in i + 1..syms.len() {
                let (a, b) = (&syms[i].1, &syms[j].1);
                let (ox, oy) = (a[2].min(b[2]) - a[0].max(b[0]), a[3].min(b[3]) - a[1].max(b[1]));
                if ox > 0.1 && oy > 0.1 {
                    out.push(json!({"check": "overlap", "sheet": name, "refs": [syms[i].0, syms[j].0], "nets": [],
                        "message": format!("{} and {} overlap", syms[i].0, syms[j].0),
                        "fix": "Move one of them (sch_edit moveBy) so their bodies and pins are apart; overlapping pins can join nets by accident."}));
                }
            }
        }
    }
    out
}

/// schematic_review: input {sheetParts, netlist, sheetDrawing?}. The netlist companion is {nets: {net: ["U1.8", ...]}, names?: {net:
/// [every label or port name joined into it]}} (or the nets map). sheetDrawing (see drawing_checks) adds the checks
/// that need the drawing; without it they are listed in `unread`. Parts may carry value (or comment), libRef (or libId)
/// and unit, which the design checks use.
pub fn schematic_review(input: &Value) -> Value {
    let Some(parts) = sheet_parts(input) else { return crate::input::refuse("bad_args", "pass the sheets' parts (read_sheet_file with pins, every sheet) as sheetParts") };
    let Some(n) = crate::input::part(input, "netlist") else { return crate::input::refuse("bad_args", "pass the compiled netlist ({nets: {net: [\"U1.8\", ...]}}) as netlist") };
    let nets = n.get("nets").or(Some(&n)).and_then(|x| x.as_object()).cloned().unwrap_or_default();
    let mut out = review(&parts, &nets);
    // shorted_nets: two different names joined into one net (two labels on one wire)
    if let Some(names) = n.get("names").and_then(|x| x.as_object()) {
        for (net, ns) in names {
            let mut set: Vec<String> = ns.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            set.sort(); set.dedup();
            if set.len() > 1 {
                out["warnings"].as_array_mut().unwrap().push(json!({"check": "shorted_nets", "refs": [], "nets": [net], "names": set,
                    "message": format!("{} name the same net", set.join(" and ")), "fix": "If they are meant to be one net, keep one name; if not, a wire or label joins two nets by mistake: find it and split them."}));
            }
        }
    }
    // the checks that need the drawing, when the bridge sends it
    match crate::input::part(input, "sheetDrawing") {
        Some(d) => {
            let sheets = d.get("sheets").or(Some(&d)).and_then(|x| x.as_array()).cloned().unwrap_or_default();
            for f in drawing_checks(&sheets) { out["warnings"].as_array_mut().unwrap().push(f); }
            out["drawingChecked"] = json!(true);
        }
        None => { out["drawingChecked"] = json!(false); out["unread"] = json!(["dangling_wire_end", "floating_label", "overlap"]); }
    }
    out["success"] = json!(true);
    out["_hint"] = json!("errors stop a board (a power pin on no net, two outputs on one net, a floating reset or enable, a designator unannotated or used twice); warnings deserve a look (pins on no net, one-pin nets, inputs nothing drives, an IC supply with no capacitor to ground, I2C with no pull-up, an LED with no resistor, a regulator without its capacitors, a no-connect flag on a wired pin, two names on one net); infos are gaps (a rail with no bulk capacitor). Each design finding has message and fix. Pin types come from the symbols: a passive pin is never 'undriven'. Fix on the sheets (sch_connect, sch_place_part), then update_pcb.");
    out
}

/// pin_pad_check: input {board, sheetParts}. The symbols' pins against the placed footprints' pads.
pub fn pin_pad_check(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let Some(parts) = sheet_parts(input) else { return crate::input::refuse("bad_args", "pass the sheets' parts (read_sheet_file with pins, every sheet) as sheetParts") };
    let refs: Vec<String> = st["components"].as_array().cloned().unwrap_or_default().iter().filter_map(|c| c["ref"].as_str().map(String::from)).collect();
    let mut out = pin_pad(&parts, st["pads"].as_array().map(|a| a.as_slice()).unwrap_or(&[]), &refs);
    out["success"] = json!(true);
    out["_hint"] = json!("pinsWithoutPad is a part whose footprint cannot carry every pin's net (a wrong footprint, or pin numbers that do not match): fix the footprint or the symbol (swap_footprint, build_footprint) before routing. padsWithoutPin is copper no net reaches. onSheetsOnly / onBoardOnly differ until update_pcb.");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(des: &str, pins: &[(&str, &str, i64)]) -> Value {
        json!({"designator": des, "pins": pins.iter().map(|(n, nm, e)| json!({"number": n, "name": nm, "electrical": e})).collect::<Vec<_>>()})
    }

    #[test]
    fn the_drawing_checks_find_kicads_examples() {
        let sheet = json!({"sheet": "/", "wires": [{"a": [101.6, 76.2], "b": [127.0, 76.2]}, {"a": [127.0, 76.2], "b": [127.0, 90.0]}, {"a": [60.0, 40.0], "b": [80.0, 40.0]}],
            "junctions": [], "noConnects": [],
            "labels": [{"text": "SDA", "kind": "local", "at": [120.65, 63.5]}, {"text": "SCL", "kind": "local", "at": [70.0, 40.0]}],
            "pins": [{"ref": "U1", "number": "3", "at": [101.6, 75.89]}, {"ref": "R1", "number": "1", "at": [127.0, 90.0]}, {"ref": "R1", "number": "2", "at": [80.0, 40.0]}],
            "symbols": [{"ref": "R3", "box": [10.0, 10.0, 20.0, 15.0]}, {"ref": "C7", "box": [18.0, 12.0, 25.0, 18.0]}, {"ref": "C8", "box": [20.0, 30.0, 25.0, 35.0]}]});
        let f = drawing_checks(&[sheet]);
        let msgs: Vec<&str> = f.iter().map(|x| x["message"].as_str().unwrap()).collect();
        // the wire's left end misses U1.3 by 0.31 mm; the far-left wire starts at (60, 40) on nothing; SDA floats; SCL
        // sits on its wire's middle, so it is fine
        assert!(msgs.contains(&"Wire end at (101.6, 76.2) connects to nothing; pin U1.3 is 0.31 mm away at (101.6, 75.89)"), "{msgs:#?}");
        assert!(msgs.contains(&"Wire end at (60, 40) connects to nothing"), "{msgs:#?}");
        assert!(msgs.contains(&"Label SDA at (120.65, 63.5) touches no wire or pin"));
        assert!(!msgs.iter().any(|m| m.contains("SCL")));
        assert!(msgs.contains(&"R3 and C7 overlap") && !msgs.iter().any(|m| m.contains("C8")));
        assert_eq!(f.len(), 4, "{msgs:#?}");
        // without the drawing the three checks are listed unread
        let r = schematic_review(&json!({"sheetParts": [], "netlist": {"nets": {}}}));
        assert_eq!(r["unread"], json!(["dangling_wire_end", "floating_label", "overlap"]));
    }

    #[test]
    fn the_design_checks_find_kicads_examples() {
        let p = |des: &str, lib: &str, value: &str, pins: &[(&str, &str, &str)]| json!({"designator": des, "libRef": lib, "value": value,
            "pins": pins.iter().map(|(n, nm, e)| json!({"number": n, "name": nm, "electrical": e})).collect::<Vec<_>>()});
        let parts = vec![
            // an MCU on +3V3 with I2C, a reset with only a capacitor on it, and its decoupling
            p("U1", "MCU:X", "", &[("1", "VDD", "power_in"), ("2", "GND", "power_in"), ("3", "SDA", "bidirectional"), ("4", "SCL", "bidirectional"), ("5", "~{RESET}", "input"), ("6", "PA0", "output")]),
            p("C1", "Device:C", "100n", &[("1", "", "passive"), ("2", "", "passive")]),
            p("C2", "Device:C", "100n", &[("1", "", "passive"), ("2", "", "passive")]),
            // an LDO from +12V to +3V3 with no input capacitor and only C1's 100 nF on its output
            p("U2", "Regulator_Linear:AMS1117-3.3", "AMS1117-3.3", &[("1", "GND", "power_in"), ("2", "VO", "power_out"), ("3", "VI", "power_in")]),
            // R1 pulls SCL up; SDA has none
            p("R1", "Device:R", "4k7", &[("1", "", "passive"), ("2", "", "passive")]),
            // an LED straight from a GPIO to ground
            p("D1", "Device:LED", "red", &[("1", "K", "passive"), ("2", "A", "passive")]),
            // two different parts both called C5, and one not annotated
            p("C5", "Device:C", "1u", &[("1", "", "passive"), ("2", "", "passive")]),
            p("C5", "Device:C", "1u", &[("1", "", "passive"), ("2", "", "passive")]),
            p("R?", "Device:R", "10k", &[("1", "", "passive"), ("2", "", "passive")]),
            // a no-connect pin that is wired
            p("U3", "X:Y", "", &[("1", "NC1", "no_connect"), ("2", "IN", "input")]),
        ];
        let nets = json!({"+3V3": ["U1.1", "C1.1", "U2.2", "R1.1", "C5.1"], "/LED": ["U1.6", "D1.2"], "GND": ["U1.2", "C1.2", "U2.1", "C2.2", "D1.1", "C5.2"], "+12V": ["U2.3"],
            "/SDA": ["U1.3", "U3.1"], "/SCL": ["U1.4", "R1.2"], "/NRST": ["U1.5", "C2.1"], "X": ["U3.2"]});
        let r = review(&parts, nets.as_object().unwrap());
        let msgs: Vec<String> = ["errors", "warnings", "infos"].iter().flat_map(|k| r[*k].as_array().unwrap().iter().filter_map(move |f| f["message"].as_str().map(|m| format!("{k}: {m}")))).collect();
        for want in ["warnings: I2C line /SDA has no pull-up resistor to a supply rail",
                     "warnings: Nothing sets the level of U1.5 ~{RESET} (net /NRST)",
                     "warnings: LED D1 has no series resistor on either side",
                     "warnings: Regulator U2 has no input capacitor on +12V",
                     "errors: R? is not annotated",
                     "errors: Reference C5 is used by 2 different symbols",
                     "warnings: No-connect flag on U3.1 NC1 but the pin is wired to /SDA"] {
            assert!(msgs.iter().any(|m| m == want), "missing {want:?} in {msgs:#?}");
        }
        // SCL has its pull-up; the output's biggest capacitor is C5's 1 uF, so no 'only 100n'; +3V3 has no 4.7 uF bulk
        assert!(!msgs.iter().any(|m| m.contains("/SCL")));
        assert!(!msgs.iter().any(|m| m.contains("output +3V3 has only")));
        assert!(msgs.iter().any(|m| m.starts_with("infos: Rail +3V3 feeds")), "{msgs:#?}");
        assert_eq!(farads("4.7uF"), Some(4.7e-6));
        assert_eq!(farads("1u0"), Some(1e-6));
        assert_eq!(farads("100"), None);
        assert!(looks_like_rail("+3V3") && looks_like_rail("3V3") && looks_like_rail("VCC_IO") && !looks_like_rail("SDA"));
    }

    #[test]
    fn pin_types_come_as_codes_or_names() {
        assert_eq!(pin_type(&json!(7)), "power");
        assert_eq!(pin_type(&json!("Bidirectional")), "io");
        assert_eq!(pin_type(&json!("sup")), "supply");
        let parts = vec![json!({"designator": "U1", "pins": [
            {"number": "1", "name": "IN", "electrical": "input"}, {"number": "2", "name": "X", "electrical": "not_connected"}]}),
            json!({"designator": "P1", "pins": [{"number": "1", "name": "VCC", "electrical": "supply"}]})];
        let nets = json!({"VCC": ["U1.1", "P1.1"]});
        let r = review(&parts, nets.as_object().unwrap());
        assert!(r["warnings"].as_array().unwrap().is_empty(), "{r}");
    }

    #[test]
    fn the_review_finds_each_kind_of_problem() {
        let parts = vec![
            part("U1", &[("1", "VCC", 7), ("2", "GND", 7), ("3", "OUT", 2), ("4", "NC", 0), ("5", "EN", 0), ("6", "DATA", 0)]),
            part("U2", &[("1", "VDD", 7), ("2", "GND", 7), ("3", "OUT", 2), ("4", "IN", 0)]),
            part("C1", &[("1", "", 4), ("2", "", 4)]),
            part("R1", &[("1", "", 4), ("2", "", 4)]),
            part("U3", &[("1", "SW", 7)]), part("L1", &[("1", "", 4), ("2", "", 4)]),
        ];
        let nets = json!({"3V3": ["U1.1", "C1.1"], "VDD": ["U2.1"], "LX": ["U3.1", "L1.1"], "GND": ["U1.2", "U2.2", "C1.2"], "BUS": ["U1.3", "U2.3"], "SENSE": ["U2.4"], "R": ["R1.1", "R1.2"]});
        let r = review(&parts, nets.as_object().unwrap());
        let checks = |k: &str| r[k].as_array().unwrap().iter().map(|f| f["check"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        // U1.6 DATA floats (input, warning); U1.5 EN floats (a floating enable, error, not also an unconnected pin);
        // NC is skipped; two outputs share BUS (error)
        assert!(checks("errors").contains(&"outputs_together".to_string()));
        assert!(checks("warnings").contains(&"unconnected_pin".to_string()));
        assert!(r["errors"].as_array().unwrap().iter().any(|f| f["check"] == "reset_enable" && f["pin"] == "U1.5"), "{r}");
        assert!(!r["warnings"].as_array().unwrap().iter().any(|f| f["pin"] == "U1.5"));
        assert!(!r["warnings"].as_array().unwrap().iter().any(|f| f["pin"] == "U1.4"));
        // VDD has one pin and no capacitor; SENSE is an input nothing drives
        assert!(checks("warnings").contains(&"single_pin_net".to_string()));
        assert!(checks("warnings").contains(&"no_decoupling".to_string()));
        assert!(checks("warnings").contains(&"undriven_input".to_string()));
        let dec = r["decoupling"].as_array().unwrap();
        assert!(dec.iter().any(|d| d["net"] == "3V3" && d["ok"] == json!(true)));
        assert!(dec.iter().any(|d| d["net"] == "VDD" && d["ok"] == json!(false)));
        // a regulator's switch pin on an inductor is not a missing capacitor
        assert!(dec.iter().any(|d| d["net"] == "LX" && d["switchNode"] == json!(true) && d["ok"] == json!(true)));
    }
    #[test]
    fn pin_pad_finds_footprints_that_do_not_fit_their_symbols() {
        let parts = vec![part("U1", &[("1", "A", 0), ("2", "B", 0), ("3", "C", 0)]), part("R1", &[("1", "", 4), ("2", "", 4)]), part("C9", &[("1", "", 4), ("2", "", 4)])];
        let pad = |r: &str, n: &str| json!({"ref": r, "name": n});
        let pads = vec![pad("U1", "1"), pad("U1", "2"), pad("U1", "4"), pad("U1", "EP"), pad("R1", "1"), pad("R1", "2"), pad("J9", "1")];
        let r = pin_pad(&parts, &pads, &["U1".into(), "R1".into(), "J9".into()]);
        assert_eq!(r["matching"], 1);
        let m = &r["mismatches"][0];
        assert_eq!(m["ref"], "U1");
        assert_eq!(m["pinsWithoutPad"], json!(["3"]));
        assert_eq!(m["padsWithoutPin"], json!(["4"]));
        assert_eq!(r["onSheetsOnly"], json!(["C9"]));
        assert_eq!(r["onBoardOnly"], json!(["J9"]));
        assert_eq!(r["ok"], json!(false));
    }

    #[test]
    fn the_verbs_take_the_companions_bare_or_in_a_reply() {
        let parts = json!({"success": true, "components": [part("U1", &[("1", "VCC", 7), ("2", "GND", 7)]), part("C1", &[("1", "", 4), ("2", "", 4)])]});
        let r = schematic_review(&json!({"sheetParts": parts, "netlist": {"nets": {"3V3": ["U1.1", "C1.1"], "GND": ["U1.2", "C1.2"]}}}));
        assert_eq!((r["success"].clone(), r["ok"].clone(), r["pins"].clone()), (json!(true), json!(true), json!(4)), "{r}");
        assert_eq!(schematic_review(&json!({"netlist": {}}))["errorCode"], "bad_args");
        let b = json!({"components": [{"ref": "U1"}, {"ref": "C1"}], "pads": [{"ref": "U1", "name": "1"}, {"ref": "U1", "name": "2"}, {"ref": "C1", "name": "1"}, {"ref": "C1", "name": "2"}]});
        let p = pin_pad_check(&json!({"board": b, "sheetParts": parts}));
        assert_eq!((p["matching"].clone(), p["ok"].clone()), (json!(2), json!(true)), "{p}");
    }

    #[test]
    fn a_supply_symbol_without_pin_numbers_is_no_part() {
        let parts = vec![part("U1", &[("1", "VCC", 7)]), json!({"designator": "SUPPLY1", "pins": [{"number": "", "name": "VCC", "electrical": "supply"}]})];
        let r = review(&parts, json!({"VCC": ["U1.1"]}).as_object().unwrap());
        assert_eq!(r["pins"], json!(1), "{r}");
        assert!(r["errors"].as_array().unwrap().is_empty());
    }
}

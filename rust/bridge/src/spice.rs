//! `fusion_simulate`: the standard's simulate (#23) without Fusion's SIM dialog. The design's own
//! SPICE netlist (export_spice: EAGLE's EXPORT SPICENETLIST) gets the analysis and a .control block
//! that writes the probes, and the ngspice Fusion bundles (LibEagle/ngspice/bin/ngspice_con.exe, the
//! engine Fusion's SIM runs) runs it in batch mode in a child process. Signals come back as x/y data.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Fusion's bundled ngspice: the newest webdeploy build that has one.
fn ngspice() -> Option<PathBuf> {
    let base = PathBuf::from(std::env::var("LOCALAPPDATA").ok()?).join("Autodesk").join("webdeploy").join("production");
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&base).ok()?.flatten()
        .map(|d| d.path().join("Applications").join("Electron").join("LibEagle").join("ngspice").join("bin").join("ngspice_con.exe"))
        .filter(|p| p.is_file())
        .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p))
        .collect();
    found.sort();
    found.pop().map(|(_, p)| p)
}

fn num(v: &Value, k: &str) -> Option<f64> { v[k].as_f64().or_else(|| v[k].as_str().and_then(|s| s.parse().ok())) }

/// The analysis line, or a refusal.
fn analysis_line(a: &Value) -> Result<String, Value> {
    let bad = |m: &str| json!({"success": false, "errorCode": "bad_args", "error": m});
    let g = |k: &str| num(a, k).map(|v| format!("{v}")).unwrap_or_default();
    match a["type"].as_str().unwrap_or("op") {
        "op" => Ok(".op".into()),
        "dc" => match (a["source"].as_str(), num(a, "start"), num(a, "stop"), num(a, "step")) {
            (Some(s), Some(_), Some(_), Some(_)) => Ok(format!(".dc {s} {} {} {}", g("start"), g("stop"), g("step"))),
            _ => Err(bad("dc needs source, start, stop and step")),
        },
        "ac" => {
            let sweep = a["sweep"].as_str().unwrap_or("dec");
            if !matches!(sweep, "dec" | "oct" | "lin") { return Err(bad("ac sweep is dec, oct or lin")); }
            match (num(a, "points"), num(a, "start"), num(a, "stop")) {
                (Some(p), Some(_), Some(_)) => Ok(format!(".ac {sweep} {} {} {}", p as u64, g("start"), g("stop"))),
                _ => Err(bad("ac needs points, start and stop (Hz)")),
            }
        }
        "tran" => match (num(a, "step"), num(a, "stop")) {
            (Some(_), Some(_)) => Ok(format!(".tran {} {}{}", g("step"), g("stop"), num(a, "start").map(|s| format!(" {s}")).unwrap_or_default())),
            _ => Err(bad("tran needs step and stop (s)")),
        },
        "noise" => {
            let sweep = a["sweep"].as_str().unwrap_or("dec");
            if !matches!(sweep, "dec" | "oct" | "lin") { return Err(bad("noise sweep is dec, oct or lin")); }
            match (a["source"].as_str(), a["output"].as_str(), num(a, "points"), num(a, "start"), num(a, "stop")) {
                (Some(src), Some(out), Some(p), Some(_), Some(_)) => {
                    let node = match a["reference"].as_str() { Some(r) => format!("v({},{})", out.to_lowercase(), r.to_lowercase()), None => format!("v({})", out.to_lowercase()) };
                    Ok(format!(".noise {node} {src} {sweep} {} {} {}", p as u64, g("start"), g("stop")))
                }
                _ => Err(bad("noise needs source (the input source), output (a net), points, start and stop (Hz)")),
            }
        }
        "raw" => a["text"].as_str().filter(|t| t.trim_start().starts_with('.')).map(|t| t.trim().to_string()).ok_or_else(|| bad("raw needs text, a SPICE analysis line such as .ac dec 20 10 1meg")),
        t => Err(bad(&format!("analysis type {t} is op, dc, ac, tran, noise or raw"))),
    }
}

/// The nodes the devices connect (their pins), as ngspice knows them (lowercase).
fn nodes(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in lines {
        let t: Vec<&str> = l.split_whitespace().collect();
        if t.is_empty() || "*.+_".contains(&t[0][..1]) { continue; }
        // two nodes for two-terminal elements; a subcircuit (X) every token but its name and model
        let k = match t[0].chars().next().unwrap_or(' ').to_ascii_uppercase() { 'X' => t.len().saturating_sub(1), 'Q' | 'J' => 4.min(t.len()), 'M' => 5.min(t.len()), 'E' | 'G' => 5.min(t.len()), _ => 3.min(t.len()) };
        for n in &t[1..k.max(1)] { let n = n.to_lowercase(); if n != "0" && !out.contains(&n) { out.push(n); } }
    }
    out
}

/// At most `n` points: every step-th one, plus the first, the last and each local extremum.
pub fn decimate(x: &[f64], ys: &[Vec<f64>], n: usize) -> Vec<usize> {
    let len = x.len();
    if len <= n || n < 3 { return (0..len).collect(); }
    let mut keep = std::collections::BTreeSet::new();
    keep.insert(0); keep.insert(len - 1);
    for y in ys { for i in 1..len - 1 { if (y[i] > y[i - 1] && y[i] >= y[i + 1]) || (y[i] < y[i - 1] && y[i] <= y[i + 1]) { keep.insert(i); } } }
    let step = ((len as f64) / (n.saturating_sub(keep.len().min(n - 2)).max(2) as f64)).ceil().max(1.0) as usize;
    let mut i = 0; while i < len { keep.insert(i); i += step; }
    let mut v: Vec<usize> = keep.into_iter().collect();
    while v.len() > n { let drop = v.len() / 2; v.remove(drop.max(1).min(v.len() - 2)); }
    v
}

/// The netlist's instance for a part as the caller names it: EAGLE's netlist calls R4 R_R4.
fn instance(body: &[String], part: &str) -> Option<String> {
    body.iter().filter_map(|l| l.split_whitespace().next()).find(|n| n.eq_ignore_ascii_case(part) || n.to_uppercase().ends_with(&format!("_{}", part.to_uppercase()))).map(str::to_lowercase)
}

/// A SPICE value: 4.7k, 100n, 1meg, 10 (unit letters after the scale are ignored, as ngspice does).
pub fn spice_value(t: &str) -> Option<f64> {
    let t = t.trim().to_lowercase();
    let end = t.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e')).unwrap_or(t.len());
    // an exponent's e is part of the number only when digits follow it
    let (mut num, mut rest) = (&t[..end], &t[end..]);
    if num.ends_with('e') { num = &num[..num.len() - 1]; rest = &t[end - 1..]; }
    num.parse::<f64>().ok()?;
    // the scale as a decimal exponent, so 100n parses to exactly 1e-7
    let exp = if rest.starts_with("meg") { 6 } else { match rest.chars().next() { Some('f') => -15, Some('p') => -12, Some('n') => -9, Some('u') => -6, Some('m') => -3, Some('k') => 3, Some('g') => 9, Some('t') => 12, _ => 0 } };
    let (mant, e0) = match num.split_once('e') { Some((m, e)) => (m, e.parse::<i32>().ok()?), None => (num, 0) };
    format!("{mant}e{}", e0 + exp).parse().ok()
}

/// splitmix64: the same seed draws the same values on every machine.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 { self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15); let mut z = self.0; z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9); z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB); ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64 }
}

/// Monte Carlo runs: every R, C and L with a plain value drawn within its tolerance (uniform, or
/// gaussian with sigma a third of it, clipped to it), applied with alter; the step names the values.
fn monte_carlo(mc: &Value, body: &[String]) -> Result<Vec<(String, Vec<String>)>, String> {
    let n = mc["runs"].as_u64().filter(|n| *n > 0).ok_or("monteCarlo.runs is the number of runs (1 or more)")? as usize;
    let dist = mc["distribution"].as_str().unwrap_or("uniform");
    if !matches!(dist, "uniform" | "gaussian") { return Err(format!("monteCarlo.distribution is uniform or gaussian, not {dist}")); }
    let tol = |k: char| -> Option<f64> { let t = &mc["tolerances"]; [("resistor", 'r'), ("capacitor", 'c'), ("inductor", 'l')].iter().find(|(_, c)| *c == k).and_then(|(name, _)| t[*name].as_f64()).map(|p| p / 100.0) };
    let parts: Vec<(String, String, f64, f64)> = body.iter().filter_map(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        let k = t.first()?.chars().next()?.to_ascii_lowercase();
        let tl = tol(k)?;
        let v = spice_value(t.get(3)?)?;
        let name = t[0].split_once('_').map(|(_, r)| r).unwrap_or(t[0]).to_string();
        Some((t[0].to_lowercase(), name, v, tl))
    }).collect();
    if parts.is_empty() { return Err("monteCarlo: no resistor, capacitor or inductor with a tolerance and a plain value in the netlist".into()); }
    let mut rng = Rng(mc["seed"].as_u64().unwrap_or(1));
    let mut out = Vec::new();
    for k in 0..n {
        let (mut set, mut drawn) = (Vec::new(), Vec::new());
        for (inst, name, v, tl) in &parts {
            let d = if dist == "uniform" { (rng.next() * 2.0 - 1.0) * tl } else {
                let (u1, u2) = (rng.next().max(1e-300), rng.next());
                ((-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos() * tl / 3.0).clamp(-tl, *tl)
            };
            let x = { let x = v * (1.0 + d); let m = 10f64.powi(5 - x.abs().log10().ceil() as i32); (x * m).round() / m };
            set.push(format!("alter {inst} = {x}"));
            drawn.push(format!("{name} = {x}"));
        }
        out.push((format!("run {}: {}", k + 1, drawn.join(", ")), set));
    }
    Ok(out)
}

/// The values a sweep steps through: lin by the increment, dec and oct by points per decade or octave.
fn sweep_values(start: f64, stop: f64, step: f64, scale: &str) -> Result<Vec<f64>, String> {
    if !(start.is_finite() && stop.is_finite() && step.is_finite()) || step <= 0.0 { return Err("start, stop and a step above 0 are required".into()); }
    let mut v = Vec::new();
    match scale {
        "lin" => { let n = ((stop - start) / step).floor() as i64; if n < 0 { return Err("stop is below start".into()); } for k in 0..=n { v.push(start + k as f64 * step); } }
        "dec" | "oct" => {
            if start <= 0.0 || stop < start { return Err("dec and oct need 0 < start <= stop".into()); }
            let base: f64 = if scale == "dec" { 10.0 } else { 2.0 };
            let mut k = 0.0; loop { let x = start * base.powf(k / step); if x > stop * (1.0 + 1e-9) { break; } v.push(x); k += 1.0; if v.len() > 10_000 { break; } }
        }
        o => return Err(format!("scale is lin, dec or oct, not {o}")),
    }
    Ok(v.into_iter().map(|x| (x * 1e9).round() / 1e9).collect())
}

/// The runs of a simulation: one, or one per sweep value and/or temperature, each with its label
/// (\"R4 = 300\", \"T = 85\") and the ngspice control lines that set it up.
fn runs(args: &Value, body: &[String]) -> Result<Vec<(Option<String>, Vec<String>)>, Value> {
    let bad = |e: String| json!({"success": false, "errorCode": "bad_args", "error": e});
    let mut sweep: Vec<(String, Vec<String>)> = vec![];
    if args["sweep"].is_object() {
        let sw = &args["sweep"];
        let param = sw["parameter"].as_str().filter(|p| !p.is_empty()).ok_or_else(|| bad("sweep.parameter is a part (\"R4\") or a model parameter (\"Q1.BF\")".into()))?;
        let vals = sweep_values(num(sw, "start").unwrap_or(f64::NAN), num(sw, "stop").unwrap_or(f64::NAN), num(sw, "step").unwrap_or(f64::NAN), sw["scale"].as_str().unwrap_or("lin")).map_err(|e| bad(format!("sweep: {e}")))?;
        let (part, mparam) = match param.split_once('.') { Some((p, m)) => (p, Some(m)), None => (param, None) };
        // the netlist names an instance R_R4 (EAGLE's prefix) or R4
        let inst = body.iter().filter_map(|l| l.split_whitespace().next()).find(|n| n.eq_ignore_ascii_case(part) || n.to_uppercase().ends_with(&format!("_{}", part.to_uppercase())))
            .ok_or_else(|| bad(format!("sweep: no part {part} in the netlist")))?.to_lowercase();
        let set: Box<dyn Fn(f64) -> String> = match mparam {
            None => Box::new(move |v| format!("alter {inst} = {v}")),
            Some(m) => {
                let models: Vec<String> = body.iter().filter(|l| l.to_lowercase().starts_with(".model")).filter_map(|l| l.split_whitespace().nth(1).map(str::to_lowercase)).collect();
                let line = body.iter().find(|l| l.split_whitespace().next().map(|n| n.eq_ignore_ascii_case(&inst)).unwrap_or(false)).unwrap();
                let model = line.split_whitespace().map(str::to_lowercase).find(|t| models.contains(t)).ok_or_else(|| bad(format!("sweep: {part} has no .model in the netlist to set {m} on")))?;
                let m = m.to_lowercase();
                Box::new(move |v| format!("altermod {model} {m} = {v}"))
            }
        };
        sweep = vals.iter().map(|v| (format!("{param} = {v}"), vec![set(*v)])).collect();
    }
    let mut temps: Vec<(String, Vec<String>)> = vec![];
    if args["temperatures"].is_object() {
        let t = &args["temperatures"];
        let vals = sweep_values(num(t, "start").unwrap_or(f64::NAN), num(t, "stop").unwrap_or(f64::NAN), num(t, "step").unwrap_or(f64::NAN), "lin").map_err(|e| bad(format!("temperatures: {e}")))?;
        temps = vals.iter().map(|v| (format!("T = {v}"), vec![format!("option temp = {v}")])).collect();
    }
    if args["monteCarlo"].is_object() {
        if !sweep.is_empty() { return Err(bad("monteCarlo and sweep together: pick one (a Monte Carlo run already varies every part)".into())); }
        sweep = monte_carlo(&args["monteCarlo"], body).map_err(bad)?;
    }
    let out: Vec<(Option<String>, Vec<String>)> = match (sweep.is_empty(), temps.is_empty()) {
        (true, true) => vec![(None, vec![])],
        (false, true) => sweep.into_iter().map(|(l, c)| (Some(l), c)).collect(),
        (true, false) => temps.into_iter().map(|(l, c)| (Some(l), c)).collect(),
        (false, false) => temps.iter().flat_map(|(tl, tc)| sweep.iter().map(move |(sl, sc)| (Some(format!("{sl}, {tl}")), [tc.clone(), sc.clone()].concat()))).collect(),
    };
    let cap = args["maxRuns"].as_u64().unwrap_or(50) as usize;
    if out.len() > cap { return Err(json!({"success": false, "errorCode": "bad_args", "error": format!("{} runs asked; the cap is {cap} (maxRuns raises it)", out.len())})); }
    Ok(out)
}

pub fn simulate(args: &Value, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let started = Instant::now();
    let Some(ng) = ngspice() else {
        return json!({"success": false, "errorCode": "no_simulator", "error": "Fusion's ngspice (LibEagle/ngspice/bin/ngspice_con.exe) was not found under %LOCALAPPDATA%/Autodesk/webdeploy"});
    };
    // the netlist: a file given (netlistPath), else the design's own (export_spice)
    let (path, export) = match args["netlistPath"].as_str() {
        Some(p) => (p.to_string(), Value::Null),
        None => {
            let e = crate::sch::call("export_spice", args, pass);
            if e["success"].as_bool() != Some(true) { return e; }
            (e["path"].as_str().unwrap_or("").to_string(), e)
        }
    };
    let text = match std::fs::read_to_string(&path) { Ok(t) => t, Err(e) => return json!({"success": false, "errorCode": "netlist_unreadable", "error": format!("{path}: {e}")}) };
    let lines: Vec<String> = text.lines().map(|l| l.trim().to_string()).collect();
    let unmapped: Vec<String> = lines.iter().filter(|l| l.starts_with('_')).filter_map(|l| l.split_whitespace().next().map(|s| s[1..].to_string())).collect();
    // a partly modelled sheet still runs and names what it left out; one with no modelled part cannot
    let devices = lines.iter().filter(|l| !l.is_empty() && !l.starts_with(['.', '*', '+', '_'])).count();
    if devices == 0 {
        return json!({"success": false, "errorCode": "no_model", "error": format!("no part on the sheet has a SPICE model ({} without: {})", unmapped.len(), unmapped.iter().take(20).cloned().collect::<Vec<_>>().join(", ")),
                      "notSimulated": unmapped, "netlist": path, "_hint": "Use simulation-capable parts (Fusion's ngspice-simulation library) or map them (MAPTOMODEL)."});
    }
    // the deck: the netlist's devices and models, the analysis, and a .control block that writes the probes
    let given = lines.iter().filter(|l| l.to_lowercase().starts_with(".op") || l.to_lowercase().starts_with(".tran") || l.to_lowercase().starts_with(".ac") || l.to_lowercase().starts_with(".dc") || l.to_lowercase().starts_with(".noise")).cloned().collect::<Vec<_>>();
    let an = if args["analysis"].is_object() { match analysis_line(&args["analysis"]) { Ok(l) => l, Err(e) => return e } } else { given.first().cloned().unwrap_or_else(|| ".op".into()) };
    let kind = an.trim_start_matches('.').split_whitespace().next().unwrap_or("op").to_lowercase();
    let mut body: Vec<String> = Vec::new(); let mut in_ctl = false;
    for l in &lines {
        let ll = l.to_lowercase();
        if ll.starts_with(".control") { in_ctl = true; continue; }
        if ll.starts_with(".endc") { in_ctl = false; continue; }
        if in_ctl || ll == ".end" || l.starts_with('_') || given.contains(l) { continue; }
        body.push(l.clone());
    }
    // a noise analysis names its input source as the caller knows it (V1); the netlist calls it V_V1
    let an = if kind == "noise" {
        let mut t: Vec<String> = an.split_whitespace().map(str::to_string).collect();
        if t.len() > 2 { if let Some(i) = instance(&body, &t[2]) { t[2] = i; } else { return json!({"success": false, "errorCode": "bad_args", "error": format!("noise source {} is not in the netlist", t[2])}); } }
        t.join(" ")
    } else { an };
    let noise = kind == "noise";
    let all = nodes(&lines);
    let cap = args["maxProbes"].as_u64().unwrap_or(32) as usize;
    let mut probes: Vec<(String, String)> = Vec::new();   // (signal name, ngspice vector)
    let mut missing: Vec<Value> = Vec::new();
    match args["probes"].as_array() {
        Some(ps) if !ps.is_empty() => for p in ps {
            if let Some(n) = p["net"].as_str() {
                let k = n.to_lowercase();
                if all.contains(&k) { probes.push((format!("V({n})"), k)); } else { missing.push(p.clone()); }
            } else if let Some(c) = p["current"].as_str() {
                probes.push((format!("I({c})"), format!("i({})", c.to_lowercase())));
            }
        },
        _ => for n in all.iter().take(cap) { probes.push((format!("V({})", n.to_uppercase()), n.clone())); },
    }
    if probes.is_empty() {
        return json!({"success": false, "errorCode": "no_such_probe", "error": "none of the probes is a net of the netlist", "missing": missing, "available": all});
    }
    let ac = kind == "ac";
    // AC: magnitude in dB and the continuous (unwrapped) phase; otherwise the value itself
    let vecs: Vec<String> = probes.iter().flat_map(|(_, v)| {
        let current = v.starts_with("i(");
        match (ac, current) {
            (true, true) => vec![format!("db({v})"), format!("cph({v})")],
            (true, false) => vec![format!("vdb({v})"), format!("cph({v})")],
            (false, true) => vec![v.clone()],
            (false, false) => vec![format!("v({v})")],
        }
    }).collect();
    let runs = match runs(args, &body) { Ok(r) => r, Err(e) => return e };
    let dir = std::env::temp_dir().join("adom-spice").join(format!("run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let deck = dir.join("deck.cir"); let log = dir.join("ngspice.log");
    let outs: Vec<PathBuf> = (0..runs.len()).map(|i| dir.join(format!("signals-{i}.txt"))).collect();
    // ngspice has no .step: each run alters the circuit, runs and writes its own file
    let mut ctl = String::from(".control\nset wr_singlescale\nset wr_vecnames\nset numdgt=10\n");
    let tots: Vec<PathBuf> = (0..runs.len()).map(|i| dir.join(format!("totals-{i}.txt"))).collect();
    for (i, (_, set)) in runs.iter().enumerate() {
        for l in set { ctl.push_str(l); ctl.push('\n'); }
        if noise {
            // each noise run makes two plots, the spectrum and the integrated totals, numbered in order
            ctl.push_str(&format!("run\nsetplot noise{}\nwrdata {} onoise_spectrum inoise_spectrum\nsetplot noise{}\nwrdata {} onoise_total inoise_total\n",
                2 * i + 1, outs[i].to_string_lossy().replace('\\', "/"), 2 * i + 2, tots[i].to_string_lossy().replace('\\', "/")));
        } else {
            ctl.push_str(&format!("run\nwrdata {} {}\n", outs[i].to_string_lossy().replace('\\', "/"), vecs.join(" ")));
        }
    }
    let deck_text = format!("{}\n{an}\n{ctl}.endc\n.end\n", body.join("\n"));
    if let Err(e) = std::fs::write(&deck, &deck_text) { return json!({"success": false, "errorCode": "write_failed", "error": e.to_string()}); }
    let mut cmd = std::process::Command::new(&ng);
    cmd.arg("-b").arg(&deck).arg("-o").arg(&log).current_dir(Path::new(&path).parent().unwrap_or(&dir));
    #[cfg(windows)] { use std::os::windows::process::CommandExt; cmd.creation_flags(0x0800_0000); }
    let mut child = match cmd.spawn() { Ok(c) => c, Err(e) => return json!({"success": false, "errorCode": "simulation_failed", "error": format!("ngspice did not start: {e}")}) };
    let limit = Duration::from_secs(args["timeoutSec"].as_u64().unwrap_or(120).clamp(5, 600));
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > limit => { let _ = child.kill(); return json!({"success": false, "errorCode": "simulation_failed", "error": format!("ngspice ran past {}s and was stopped", limit.as_secs()), "deck": deck}); }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return json!({"success": false, "errorCode": "simulation_failed", "error": e.to_string()}),
        }
    }
    let logtext = std::fs::read_to_string(&log).unwrap_or_default();
    let version = logtext.split("ngspice-").nth(1).and_then(|s| s.split(|c: char| !c.is_ascii_digit() && c != '.').next()).filter(|s| !s.is_empty()).map(str::to_string)
        .or_else(|| ng.parent().and_then(|b| b.parent()).and_then(|r| std::fs::read_dir(r.join("docs")).ok()).and_then(|d| d.flatten().find_map(|f| f.file_name().to_string_lossy().strip_prefix("ngspice-").and_then(|s| s.split('-').next()).map(str::to_string))));
    let engine = format!("ngspice {} (Fusion's bundled LibEagle/ngspice)", version.unwrap_or_else(|| "?".into()));
    let (x_unit, y_unit) = match kind.as_str() { "ac" => ("Hz", "dB"), "noise" => ("Hz", "V/rtHz"), "tran" => ("s", "V"), "dc" => ("V", "V"), _ => ("", "V") };
    let mut totals: Vec<Value> = Vec::new();
    let n = args["points"].as_u64().unwrap_or(200) as usize;
    let per = if ac { 2 } else { 1 };
    let (mut signals, mut rows_n, mut kept) = (Vec::new(), 0usize, 0usize);
    for (i, (label, _)) in runs.iter().enumerate() {
        let data = match std::fs::read_to_string(&outs[i]) { Ok(d) => d, Err(_) => {
            let msg: Vec<&str> = logtext.lines().filter(|l| { let l = l.to_lowercase(); l.contains("error") || l.contains("fatal") || l.contains("no such vector") }).take(8).collect();
            return json!({"success": false, "errorCode": "simulation_failed", "error": format!("{}{}", label.as_ref().map(|l| format!("run {l}: ")).unwrap_or_default(), if msg.is_empty() { "ngspice wrote no data".to_string() } else { msg.join(" | ") }), "engine": engine, "deck": deck, "log": log});
        } };
        let mut rows = data.lines().filter(|l| !l.trim().is_empty());
        let header: Vec<String> = rows.next().unwrap_or("").split_whitespace().map(str::to_string).collect();
        let cols: Vec<Vec<f64>> = rows.map(|l| l.split_whitespace().filter_map(|t| t.parse::<f64>().ok()).collect()).filter(|r: &Vec<f64>| r.len() == header.len()).collect();
        if cols.is_empty() { return json!({"success": false, "errorCode": "simulation_failed", "error": "ngspice wrote no rows", "engine": engine}); }
        let col = |j: usize| cols.iter().map(|r| r[j]).collect::<Vec<f64>>();
        let x = col(0);
        if noise {
            let ys: Vec<Vec<f64>> = (1..=2).map(col).collect();
            let keep = decimate(&x, &ys, n);
            rows_n = rows_n.max(x.len()); kept = kept.max(keep.len());
            // the totals plot: the last numbers written are onoise_total and inoise_total (V)
            let tv: Vec<f64> = std::fs::read_to_string(&tots[i]).unwrap_or_default().lines().last().unwrap_or("").split_whitespace().filter_map(|t| t.parse().ok()).collect();
            let tot = if tv.len() >= 2 { json!({"onoise": tv[tv.len() - 2], "inoise": tv[tv.len() - 1]}) } else { Value::Null };
            for (k, name) in ["onoise", "inoise"].iter().enumerate() {
                let mut s = json!({"signal": name, "x": keep.iter().map(|&j| x[j]).collect::<Vec<_>>(), "y": keep.iter().map(|&j| ys[k][j]).collect::<Vec<_>>(), "xUnit": x_unit, "yUnit": y_unit});
                if let Some(l) = label { s["step"] = json!(l); }
                signals.push(s);
            }
            let mut t = tot; if let (Some(l), true) = (label, t.is_object()) { t["step"] = json!(l); }
            totals.push(t);
            continue;
        }
        let ys: Vec<Vec<f64>> = (0..probes.len()).map(|k| col(1 + k * per)).collect();
        let keep = decimate(&x, &ys, n);
        rows_n = rows_n.max(x.len()); kept = kept.max(keep.len());
        for (k, (name, _)) in probes.iter().enumerate() {
            let y = &ys[k];
            let mut s = json!({"signal": name, "x": keep.iter().map(|&i| x[i]).collect::<Vec<_>>(), "y": keep.iter().map(|&i| y[i]).collect::<Vec<_>>(), "xUnit": x_unit,
                               "yUnit": if name.starts_with("I(") && !ac { "A" } else { y_unit }});
            if ac { let ph = col(2 + k * 2); s["phase"] = json!(keep.iter().map(|&i| ph[i].to_degrees()).collect::<Vec<_>>()); }
            if let Some(l) = label { s["step"] = json!(l); }
            signals.push(s);
        }
    }
    let mut out = json!({"success": true, "mutated": false, "analysis": an, "signals": signals, "missing": missing, "notSimulated": unmapped, "available": all, "rows": rows_n, "points": kept, "runs": runs.len(),
           "engine": engine, "netlist": path, "deck": deck, "export": export, "elapsedSec": (started.elapsed().as_secs_f64() * 100.0).round() / 100.0,
           "_hint": "signals are ngspice's own output for the design's SPICE netlist (EAGLE's EXPORT SPICENETLIST); points is a ceiling (first, last and extrema kept). For AC, y is the magnitude in dB and phase the unwrapped phase in degrees."});
    // noise: the integrated output and input noise (V), one per run when there are several
    if noise { out["totals"] = if totals.len() == 1 { totals.remove(0) } else { json!(totals) }; }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn analyses_and_nodes() {
        assert_eq!(analysis_line(&json!({"type": "ac", "sweep": "dec", "points": 20, "start": 10, "stop": 1e6})).unwrap(), ".ac dec 20 10 1000000");
        assert_eq!(analysis_line(&json!({"type": "tran", "step": 1e-6, "stop": 1e-3})).unwrap(), ".tran 0.000001 0.001");
        assert_eq!(analysis_line(&json!({"type": "dc", "source": "V1", "start": 0, "stop": 5, "step": 0.1})).unwrap(), ".dc V1 0 5 0.1");
        assert!(analysis_line(&json!({"type": "ac", "sweep": "dec"})).is_err());
        assert_eq!(analysis_line(&json!({"type": "raw", "text": ".ac dec 5 1 1k"})).unwrap(), ".ac dec 5 1 1k");
        let l: Vec<String> = ["* x", "V_V1 IN 0 DC 0 AC 1", "R_R1 IN OUT 1k", "C_C1 OUT 0 100n", "_U1 N_3 PART", ".end"].iter().map(|s| s.to_string()).collect();
        assert_eq!(nodes(&l), vec!["in", "out"]);
    }
    #[test]
    fn sweeps_and_temperatures_make_one_run_each() {
        let body: Vec<String> = ["V_V1 IN 0 DC 0 AC 1", "R_R4 IN OUT 1k", "Q_Q1 C B E QMOD", ".model QMOD NPN(BF=100)"].iter().map(|s| s.to_string()).collect();
        let r = runs(&json!({"sweep": {"parameter": "R4", "start": 100, "stop": 300, "step": 100}}), &body).unwrap();
        assert_eq!(r.iter().map(|x| (x.0.clone().unwrap(), x.1[0].clone())).collect::<Vec<_>>(),
                   vec![("R4 = 100".into(), "alter r_r4 = 100".into()), ("R4 = 200".into(), "alter r_r4 = 200".into()), ("R4 = 300".into(), "alter r_r4 = 300".into())]);
        let r = runs(&json!({"sweep": {"parameter": "Q1.BF", "start": 50, "stop": 200, "step": 2, "scale": "oct"}, "temperatures": {"start": -40, "stop": 85, "step": 125}}), &body).unwrap();
        assert_eq!(r.len(), 5 * 2, "50, 70.7, 100, 141, 200 at two temperatures");
        assert_eq!(r[0].1, vec!["option temp = -40".to_string(), "altermod qmod bf = 50".to_string()]);
        assert_eq!(r[0].0.as_deref(), Some("Q1.BF = 50, T = -40"));
        assert!(runs(&json!({"sweep": {"parameter": "R9", "start": 1, "stop": 2, "step": 1}}), &body).is_err());
        assert!(runs(&json!({"sweep": {"parameter": "R4", "start": 1, "stop": 1000, "step": 1}}), &body).is_err(), "over the run cap");
        assert_eq!(runs(&json!({}), &body).unwrap().len(), 1);
        assert_eq!(sweep_values(10.0, 1000.0, 1.0, "dec").unwrap(), vec![10.0, 100.0, 1000.0]);
    }

    #[test]
    fn noise_lines_and_monte_carlo_draws() {
        assert_eq!(analysis_line(&json!({"type": "noise", "source": "V1", "output": "OUT", "sweep": "dec", "points": 10, "start": 10, "stop": 1e6})).unwrap(), ".noise v(out) V1 dec 10 10 1000000");
        assert!(analysis_line(&json!({"type": "noise", "source": "V1"})).is_err());
        assert_eq!((spice_value("4.7k"), spice_value("100n"), spice_value("1meg"), spice_value("10"), spice_value("1e3")), (Some(4700.0), Some(1e-7), Some(1e6), Some(10.0), Some(1000.0)));
        let body: Vec<String> = ["V_V1 IN 0 DC 0 AC 1", "R_R1 IN OUT 1k", "C_C1 OUT 0 100n"].iter().map(|s| s.to_string()).collect();
        let mc = json!({"monteCarlo": {"runs": 3, "distribution": "gaussian", "seed": 7, "tolerances": {"resistor": 1, "capacitor": 10}}});
        let a = runs(&mc, &body).unwrap();
        assert_eq!(a.len(), 3);
        assert_eq!(a, runs(&mc, &body).unwrap(), "the same seed draws the same values");
        for (label, set) in &a {
            assert!(label.as_ref().unwrap().contains("R1 = ") && label.as_ref().unwrap().contains("C1 = "));
            let r: f64 = set[0].rsplit(' ').next().unwrap().parse().unwrap();
            let c: f64 = set[1].rsplit(' ').next().unwrap().parse().unwrap();
            assert!((990.0..=1010.0).contains(&r) && (0.9e-7..=1.1e-7).contains(&c), "{set:?}");
        }
        assert_ne!(runs(&json!({"monteCarlo": {"runs": 1, "seed": 8, "tolerances": {"resistor": 5}}}), &body).unwrap(), runs(&json!({"monteCarlo": {"runs": 1, "seed": 9, "tolerances": {"resistor": 5}}}), &body).unwrap());
        assert!(runs(&json!({"monteCarlo": {"runs": 2, "tolerances": {"resistor": 1}}, "sweep": {"parameter": "R1", "start": 1, "stop": 2, "step": 1}}), &body).is_err());
    }

    #[test]
    fn decimation_keeps_ends_and_peaks() {
        let x: Vec<f64> = (0..1000).map(|i| i as f64).collect();
        let y: Vec<f64> = x.iter().map(|v| if *v == 500.0 { 10.0 } else { 0.0 }).collect();
        let k = decimate(&x, &[y], 50);
        assert!(k.len() <= 50 && k.contains(&0) && k.contains(&999) && k.contains(&500));
    }
}

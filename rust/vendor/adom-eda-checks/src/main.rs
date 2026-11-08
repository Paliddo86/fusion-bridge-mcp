//! adom-eda-checks <check> [bundle.json | -]
//! adom-eda-checks <check> --board b.json [--stack s.json] [--pours p.json] [--prices r.json] [--args '{...}']
//! adom-eda-checks import_svg --svg art.svg --args '{"at": [10, 10], "widthMm": 20}'
//! Prints the check's reply as JSON. `adom-eda-checks list` names the checks.
use serde_json::{json, Value};
use std::io::Read;

fn read(path: &str) -> Result<Value, String> {
    let text = if path == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).map_err(|e| format!("stdin: {e}"))?;
        s
    } else {
        std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?
    };
    serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

fn bundle(rest: &[String]) -> Result<Value, String> {
    if rest.len() == 1 { return read(&rest[0]); }
    let mut b = json!({});
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        let val = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--board" | "--stack" | "--pours" | "--rules" | "--silk" | "--fab" | "--bodies" | "--prices" | "--sheet-parts" | "--netlist" | "--drc" | "--library" | "--sheet-drawing" => {
                let key = match flag.as_str() { "--sheet-parts" => "sheetParts", "--sheet-drawing" => "sheetDrawing", f => &f[2..] };
                b[key] = read(val)?;
            }
            "--args" => b["args"] = serde_json::from_str(val).map_err(|e| format!("--args: {e}"))?,
            // SVG artwork is text, not JSON
            "--svg" => b["svg"] = Value::String(std::fs::read_to_string(val).map_err(|e| format!("{val}: {e}"))?),
            _ => return Err(format!("unknown flag {flag}")),
        }
    }
    Ok(b)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let out = match argv.first().map(String::as_str) {
        None | Some("-h") | Some("--help") => json!({"success": true, "usage": "adom-eda-checks <check> [bundle.json | -] | <check> --board b.json [--stack s.json] [--pours p.json] [--prices r.json] [--args '{...}']", "checks": adom_eda_checks::CHECKS, "writers": adom_eda_checks::writers::WRITERS}),
        Some("list") => json!({"success": true, "checks": adom_eda_checks::CHECKS, "writers": adom_eda_checks::writers::WRITERS}),
        Some(check) => match bundle(&argv[1..]) {
            Ok(b) => adom_eda_checks::run(check, &b),
            Err(e) => json!({"success": false, "errorCode": "bad_args", "error": e, "_hint": "Pass one bundle file {board, stack, pours, args}, or --board, --stack, --pours and --args."}),
        },
    };
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
    if out.get("success") == Some(&json!(false)) { std::process::exit(1); }
}

//! One shareable page per board: every check the engine has, run against one bundle, written as a single self-contained
//! HTML page with the board's picture when the bridge passes one. From the Altium bridge's report.rs.
use serde_json::{json, Value};

fn esc(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;") }
fn txt(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        // lists read as plain text, not JSON
        Value::Array(a) => a.iter().map(txt).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// One section of the page: a title, a verdict (true, false, None = information) and rows of (label, value). Pure.
pub struct Section { pub title: String, pub ok: Option<bool>, pub summary: String, pub rows: Vec<(String, String)> }

/// The sections from the checks' replies (any may be an error reply: it shows as such). Pure.
pub fn sections(c: &Value) -> Vec<Section> {
    let mut out = Vec::new();
    let failed = |r: &Value| r.get("success").and_then(|b| b.as_bool()) == Some(false);
    let err = |title: &str, r: &Value| Section { title: title.into(), ok: None, summary: format!("not run: {}", txt(r.get("error").unwrap_or(&Value::Null))), rows: vec![] };
    // design rules
    let d = &c["drc"];
    out.push(if d.is_null() { Section { title: "Design rules (the tool's DRC)".into(), ok: None, summary: "not run: pass the tool's DRC reply (run_drc) as drc".into(), rows: vec![] } } else if failed(d) { err("Design rules (the tool's DRC)", d) } else {
        // each tool counts in its own words: Altium errors/unconnected, Fusion violationCount/airwires
        let errs = d["errors"].as_i64().or(d["violationCount"].as_i64());
        let open = d["unconnected"].as_i64().or(d["airwires"].as_i64()).or(d["unrouted"].as_i64());
        let shown = |v: Option<i64>| v.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
        Section { title: "Design rules (the tool's DRC)".into(), ok: match (errs, open) { (Some(e), Some(u)) => Some(e == 0 && u == 0), (Some(e), None) if e > 0 => Some(false), _ => None },
            summary: format!("{} violations, {} unconnected", shown(errs), shown(open)),
            rows: d["violations"].as_array().cloned().unwrap_or_default().iter().take(30).map(|v| (txt(&v["rule"]), txt(&v["objects"]))).collect() } });
    let f = &c["fab"];
    // the vendor as each finding's source names it, else the record's own name
    let vendor = f["outsideLimits"][0]["source"]["name"].as_str().or(f["fab"]["vendor"].as_str()).or(f["fab"]["name"].as_str()).unwrap_or("").to_string();
    let fab_title = format!("Fab limits ({})", vendor.chars().take(40).collect::<String>());
    let fab_title = if fab_title == "Fab limits ()" { "Fab limits".to_string() } else { fab_title };
    out.push(if failed(f) { err(&fab_title, f) } else {
        let mut rows: Vec<(String, String)> = f["outsideLimits"].as_array().cloned().unwrap_or_default().iter().map(|o| (txt(&o["limit"]), format!("board {} mm, limit {} mm", txt(&o["boardMm"]), txt(&o["limitMm"])))).collect();
        for k in ["track", "viaDrill", "padHole", "edgeClearance", "silkText", "boardSize"] { if let Some(m) = f["measured"].get(k) { rows.push((format!("measured {k}"), format!("{} mm", txt(&m["mm"])))); } }
        Section { title: fab_title.clone(), ok: f["fits"].as_bool(), summary: format!("{} outside the fab's limits", f["outsideLimits"].as_array().map(|a| a.len()).unwrap_or(0)), rows } });
    let a = &c["assembly"];
    out.push(if failed(a) { err("JLCPCB assembly", a) } else {
        let rows = ["economic", "standard"].iter().flat_map(|t| a[*t]["rules"].as_array().cloned().unwrap_or_default().into_iter().filter(|r| r["ok"] == json!(false)).map(move |r| (format!("{t}: {}", txt(&r["rule"])), txt(&r["detail"])))).collect();
        Section { title: "JLCPCB assembly".into(), ok: Some(!a["service"].is_null()), summary: format!("tier: {}", if a["service"].is_null() { "none passes yet".to_string() } else { txt(&a["service"]) }), rows } });
    let s = &c["schematic"];
    out.push(if failed(s) { err("Schematic review", s) } else {
        let rows = s["errors"].as_array().cloned().unwrap_or_default().iter().chain(s["warnings"].as_array().cloned().unwrap_or_default().iter()).take(30)
            .map(|w| (txt(&w["check"]), [w.get("pin"), w.get("net"), w.get("ics")].iter().flatten().map(|v| txt(v)).collect::<Vec<_>>().join(" "))).collect();
        Section { title: "Schematic review".into(), ok: s["ok"].as_bool(), summary: format!("{} errors, {} warnings, {} pins on {} nets", s["errors"].as_array().map(|a| a.len()).unwrap_or(0), s["warnings"].as_array().map(|a| a.len()).unwrap_or(0), txt(&s["pins"]), txt(&s["nets"])), rows } });
    let p = &c["pinPad"];
    out.push(if failed(p) { err("Symbols against footprints", p) } else {
        Section { title: "Symbols against footprints".into(), ok: p["ok"].as_bool(), summary: format!("{} of {} parts match", txt(&p["matching"]), txt(&p["partsChecked"])),
            rows: p["mismatches"].as_array().cloned().unwrap_or_default().iter().map(|m| (txt(&m["ref"]), format!("pins without pad {} / pads without pin {}", txt(&m["pinsWithoutPad"]), txt(&m["padsWithoutPin"])))).collect() } });
    let i = &c["impedance"];
    out.push(if failed(i) { err("Controlled impedance", i) } else {
        Section { title: "Controlled impedance (50 ohm, 90 ohm pairs)".into(), ok: None, summary: "track widths from the stack-up".into(),
            rows: i["layers"].as_array().cloned().unwrap_or_default().iter().map(|l| (txt(&l["layer"]), format!("{}: 50 ohm {} mm; 90 ohm pair {} / {} mm", txt(&l["model"]), txt(&l["width"]), txt(&l["diffWidth"]), txt(&l["diffGap"])))).collect() } });
    let r = &c["returnPath"];
    out.push(if failed(r) { err("Return paths", r) } else {
        Section { title: "Return paths".into(), ok: r["ok"].as_bool(), summary: format!("{} net-layer runs over a gap in the reference copper", r["nets"].as_array().map(|a| a.len()).unwrap_or(0)),
            rows: r["nets"].as_array().cloned().unwrap_or_default().iter().take(20).map(|n| (format!("{} on {}", txt(&n["net"]), txt(&n["layer"])), format!("{} mm of {} mm ({}%)", txt(&n["unreferencedMm"]), txt(&n["lengthMm"]), txt(&n["percent"])))).collect() } });
    let b = &c["balance"];
    out.push(if failed(b) { err("Copper balance", b) } else {
        Section { title: "Copper balance".into(), ok: b["balanced"].as_bool(), summary: "copper share per layer; mirrored pairs within 20 points".into(),
            rows: b["layers"].as_array().cloned().unwrap_or_default().iter().map(|l| (txt(&l["layer"]), format!("{}% ({} mm2)", txt(&l["percent"]), txt(&l["copperMm2"])))).collect() } });
    let h = &c["heights"];
    out.push(if failed(h) { err("3D heights and clearance", h) } else {
        let mut rows: Vec<(String, String)> = h["heights"].as_array().cloned().unwrap_or_default().iter().take(8).map(|x| (txt(&x["ref"]), format!("{} mm, {}", txt(&x["heightMm"]), txt(&x["side"])))).collect();
        for col in h["collisions"].as_array().cloned().unwrap_or_default() { rows.push(("collision".into(), format!("{} by {} mm", txt(&col["parts"]), txt(&col["overlapMm"])))); }
        Section { title: "3D heights and clearance".into(), ok: Some(h["collisions"].as_array().map(|a| a.is_empty()).unwrap_or(true)), summary: format!("{} bodies; {} parts without a 3D body", txt(&h["bodies"]), h["noBody"].as_array().map(|a| a.len()).unwrap_or(0)), rows } });
    let v = &c["review"];
    out.push(if failed(v) { err("Design review", v) } else {
        Section { title: "Design review".into(), ok: Some(v["counts"]["error"].as_i64() == Some(0)), summary: format!("{} errors, {} warnings, {} notes", txt(&v["counts"]["error"]), txt(&v["counts"]["warning"]), txt(&v["counts"]["info"])),
            rows: v["findings"].as_array().cloned().unwrap_or_default().iter().take(30).map(|x| (format!("{} ({})", txt(&x["check"]), txt(&x["severity"])), txt(&x["message"]))).collect() } });
    out
}

/// The page. Dark by default with a light scheme, tokens on :root. Pure.
pub fn page(board: &str, when: &str, version: &str, shot: Option<&str>, secs: &[Section]) -> String {
    let badge = |ok: Option<bool>| match ok { Some(true) => "<span class=\"b ok\">pass</span>", Some(false) => "<span class=\"b no\">needs work</span>", None => "<span class=\"b info\">info</span>" };
    let mut body = String::new();
    for s in secs {
        body.push_str(&format!("<section><h2>{} {}</h2><p class=\"sum\">{}</p>", esc(&s.title), badge(s.ok), esc(&s.summary)));
        if !s.rows.is_empty() {
            body.push_str("<table>");
            for (k, v) in &s.rows { body.push_str(&format!("<tr><th>{}</th><td>{}</td></tr>", esc(k), esc(v))); }
            body.push_str("</table>");
        }
        body.push_str("</section>");
    }
    let pass = secs.iter().filter(|s| s.ok == Some(true)).count();
    let work = secs.iter().filter(|s| s.ok == Some(false)).count();
    let img = shot.map(|b| format!("<img alt=\"the board\" src=\"data:image/png;base64,{b}\">")).unwrap_or_default();
    format!(r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="color-scheme" content="dark"><title>Board report</title><style>
:root{{--bg:#101418;--panel:#182028;--ink:#e6edf3;--dim:#9aa7b4;--line:#2a3542;--ok:#3fb950;--no:#f0883e;--info:#58a6ff}}
body{{margin:0;background:var(--bg);color:var(--ink);font:15px/1.5 system-ui,-apple-system,Segoe UI,sans-serif}}
main{{max-width:960px;margin:0 auto;padding:24px 16px}} h1{{font-size:24px;margin:0 0 4px}} .meta{{color:var(--dim);margin:0 0 16px}}
img{{width:100%;border:1px solid var(--line);border-radius:8px;margin:8px 0 16px}}
section{{background:var(--panel);border:1px solid var(--line);border-radius:8px;padding:12px 16px;margin:12px 0}}
h2{{font-size:17px;margin:0;display:flex;gap:8px;align-items:center}} .sum{{color:var(--dim);margin:4px 0 8px}}
.b{{font-size:12px;border-radius:999px;padding:1px 8px;border:1px solid}} .ok{{color:var(--ok)}} .no{{color:var(--no)}} .info{{color:var(--info)}}
table{{width:100%;border-collapse:collapse;font-size:13px}} th,td{{text-align:left;padding:4px 6px;border-top:1px solid var(--line);vertical-align:top;overflow-wrap:anywhere}} th{{width:34%;color:var(--dim);font-weight:500}}
</style></head><body><main><h1>{}</h1><p class="meta">{} &middot; {} pass, {} need work &middot; {}</p>{}{}</main></body></html>"#,
        esc(board), esc(when), pass, work, esc(version), img, body)
}

/// board_report: input {board, stack?, pours?, rules?, bodies?, fab?, sheetParts?, netlist?, drc?, picture?, args: {title?,
/// date?, lcscMissing?, panelMm?, gridMm?, alwaysChecked?}}; the last four go to assembly_check and review_board. Every check the engine has, run on this bundle, as one self-contained HTML page (`text`), with the tool's
/// own DRC reply (`drc`) and a base64 PNG of the board (`picture`) when the bridge passes them. A section whose input
/// is missing says so instead of passing.
pub fn board_report(input: &Value) -> Value {
    if let Err(e) = crate::input::board(input) { return e; }
    let with = |args: Value| { let mut i = input.clone(); i["args"] = args; i };
    let need = |k: &str, what: &str| crate::input::part(input, k).is_none().then(|| json!({"success": false, "error": format!("pass {what} as {k}")}));
    let mut c = json!({});
    c["schematic"] = need("sheetParts", "the sheets' parts").or_else(|| need("netlist", "the compiled netlist")).unwrap_or_else(|| crate::run("schematic_review", &with(json!({}))));
    c["pinPad"] = need("sheetParts", "the sheets' parts").unwrap_or_else(|| crate::run("pin_pad_check", &with(json!({}))));
    c["fab"] = need("fab", "a pcb-design-rules record").unwrap_or_else(|| crate::run("fab_check", &with(json!({}))));
    // the bridge's BOM lines without an LCSC number, when it passes them (fusion-lane: without, the tier read economic)
    c["assembly"] = crate::run("assembly_check", &with(json!({"lcscMissing": input["args"]["lcscMissing"], "panelMm": input["args"]["panelMm"]})));
    c["impedance"] = need("stack", "the stack-up").unwrap_or_else(|| crate::run("impedance", &with(json!({"z0": 50, "diff": 90}))));
    c["returnPath"] = need("stack", "the stack-up").unwrap_or_else(|| crate::run("return_path", &with(json!({}))));
    c["balance"] = need("stack", "the stack-up").unwrap_or_else(|| crate::run("copper_balance", &with(json!({}))));
    c["heights"] = need("bodies", "the 3D bodies").unwrap_or_else(|| crate::run("height_check", &with(json!({}))));
    c["review"] = crate::run("review_board", &with(json!({"gridMm": input["args"]["gridMm"], "alwaysChecked": input["args"]["alwaysChecked"]})));
    c["drc"] = crate::input::part(input, "drc").unwrap_or(Value::Null);
    let secs = sections(&c);
    let args = &input["args"];
    let title = args["title"].as_str().unwrap_or("board");
    let date = args["date"].as_str().map(String::from).unwrap_or_else(|| crate::writers::ipc2581::now().replace('T', " ").replace('Z', " UTC"));
    let picture = input["picture"].as_str();
    let text = page(title, &date, &format!("adom-eda-checks {}", crate::VERSION), picture, &secs);
    json!({"success": true, "text": text, "sections": secs.iter().map(|s| json!({"title": s.title, "ok": s.ok, "summary": s.summary})).collect::<Vec<_>>(),
        "pass": secs.iter().filter(|s| s.ok == Some(true)).count(), "needsWork": secs.iter().filter(|s| s.ok == Some(false)).count(), "picture": picture.is_some(),
        "_hint": "text is a single self-contained HTML page: the bridge writes it beside the project and opens or sends it. Each section's verdict comes from the engine check named in it: run that check for the full reply. A section marked 'not run' names the companion to pass."})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_read_every_check_and_the_page_escapes() {
        let c = json!({"drc": {"success": true, "errors": 0, "unconnected": 0, "violations": []},
                       "fab": {"success": true, "fits": false, "fab": {"vendor": "JLCPCB"}, "outsideLimits": [{"limit": "minTrack", "boardMm": 0.08, "limitMm": 0.1}], "measured": {"track": {"mm": 0.08}}},
                       "assembly": {"success": false, "error": "no outline"},
                       "schematic": {"success": true, "ok": true, "errors": [], "warnings": [{"check": "single_pin_net", "net": "<X>"}], "pins": 10, "nets": 4}});
        let s = sections(&c);
        assert_eq!(s[0].ok, Some(true));
        assert_eq!((s[1].title.as_str(), s[1].ok), ("Fab limits (JLCPCB)", Some(false)));
        assert!(s[2].summary.starts_with("not run"));
        let html = page("lane", "2026-10-26", "adom-eda-checks", None, &s);
        assert!(html.contains("&lt;X&gt;") && !html.contains("<X>"));
        assert_eq!(txt(&json!(["Pad U2-5", "Pad U3-4"])), "Pad U2-5, Pad U3-4");
    }

    #[test]
    fn the_report_runs_the_engines_checks_and_names_what_is_missing() {
        let st = json!({"outline": [[0, 0], [40, 0], [40, 20], [0, 20]], "components": [{"ref": "R1", "x": 10.0, "y": 10.0, "rotation": 0.0, "side": "top", "pattern": "0603"}],
            "pads": [{"ref": "R1", "name": "1", "x": 9.2, "y": 10.0, "sizeX": 0.8, "sizeY": 0.9, "rotation": 0, "layer": "F.Cu", "drill": 0, "net": "A", "shape": "rect"},
                     {"ref": "R1", "name": "2", "x": 10.8, "y": 10.0, "sizeX": 0.8, "sizeY": 0.9, "rotation": 0, "layer": "F.Cu", "drill": 0, "net": "B", "shape": "rect"}]});
        let r = board_report(&json!({"board": st, "args": {"title": "Test", "date": "2026-10-29 12:00 UTC"}}));
        assert_eq!(r["success"], json!(true), "{r}");
        let text = r["text"].as_str().unwrap();
        assert!(text.starts_with("<!doctype html>") && text.contains("<h1>Test</h1>"));
        let sec = |t: &str| r["sections"].as_array().unwrap().iter().find(|s| s["title"].as_str().unwrap_or("").starts_with(t)).cloned().unwrap();
        assert!(sec("Design rules")["summary"].as_str().unwrap().contains("pass the tool's DRC"));
        assert!(sec("Fab limits")["summary"].as_str().unwrap().contains("not run"));
        assert_eq!(sec("Design review")["ok"], json!(true), "{r}");
        assert!(sec("JLCPCB assembly")["summary"].as_str().unwrap().starts_with("tier"));
        // Fusion's DRC counts in its own words; the bridge's LCSC gaps reach the assembly section
        let f = board_report(&json!({"board": st, "drc": {"violationCount": 3, "airwires": 0}, "args": {"lcscMissing": 2}}));
        let sec = |t: &str| f["sections"].as_array().unwrap().iter().find(|s| s["title"].as_str().unwrap_or("").starts_with(t)).cloned().unwrap();
        assert_eq!((sec("Design rules")["summary"].clone(), sec("Design rules")["ok"].clone()), (json!("3 violations, 0 unconnected"), json!(false)));
        assert_eq!(sec("JLCPCB assembly")["ok"], json!(false), "{f}");
    }
}

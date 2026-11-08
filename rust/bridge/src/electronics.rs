//! `fusion_electronics_query`: the typed electronics read model. One verb, one row shape, two
//! sources: Autodesk's own MCP `electronics_read` (49 entity classes, filters, pagination) when
//! its server answers, and the board's `board_info` XML for the common entities when it does not
//! (the free tier, or the server simply off). The caller never learns which; `source` says.
use crate::addin::Ctx;
use serde_json::{json, Value};

/// Entity classes Autodesk's MCP serves (the enum in its tool schema, 2026-09-04).
pub const ENTITIES: &[&str] = &["Attribute", "Bus", "Circle", "Contact", "Element", "Frame", "Gate", "Hole", "Grid", "Instance", "Junction", "Label", "Layer", "Module", "ModuleInstance", "Net", "Signal", "Pad", "Part", "Pin", "PinRef", "PolyCutout", "Spline", "PolyPour", "PolyShape", "Port", "PortRef", "Rectangle", "Segment", "Sheet", "Schematic", "Board", "Library", "Smd", "Symbol", "DeviceSet", "Device", "Package", "Package3D", "Technology", "Text", "VariantDef", "Variant", "Via", "ContactRef", "Wire", "Dimension", "NetClass", "Error", "PortNetMappingEntry"];
/// What the board_info fallback can answer without the MCP server.
const BOARD_ENTITIES: &[&str] = &["Board", "Element", "Library", "Package", "Layer", "Signal", "Net", "Error", "Wire"];

fn canonical(entity: &str) -> Option<&'static str> {
    let e = entity.trim().trim_start_matches("electronics.").to_lowercase();
    ENTITIES.iter().find(|x| x.to_lowercase() == e).copied()
}
fn attrs(tag: &str) -> Value {
    let re = regex::Regex::new(r#"([\w:-]+)="([^"]*)""#).unwrap();
    let mut o = serde_json::Map::new();
    for c in re.captures_iter(tag) { o.insert(c[1].to_string(), json!(c[2].to_string())); }
    Value::Object(o)
}
fn num(v: &Value) -> Option<f64> { v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())) }
fn section<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}"); let close = format!("</{tag}>");
    let i = xml.find(&open)?; let j = xml[i..].find(&close)? + i;
    Some(&xml[i..j])
}

/// Rows for one entity from the board_info payload (`data.rawXml` + `data.summary`).
pub fn rows_from_board_info(entity: &str, data: &Value) -> Result<Vec<Value>, String> {
    let xml = data["rawXml"].as_str().unwrap_or("");
    let summary = &data["summary"];
    let board = regex::Regex::new(r"<g ([^>]*boardThickness[^>]*)>").unwrap().captures(xml).map(|c| attrs(&c[1])).unwrap_or(json!({}));
    let unit = board["unit"].as_str().unwrap_or("mm").to_string();
    let g_re = regex::Regex::new(r"<g ([^>]*)/?>").unwrap();
    Ok(match entity {
        "Board" => vec![json!({"name": board["name"], "unit": unit, "boardThickness": num(&board["boardThickness"]), "topCopperThickness": num(&board["topCopperThickness"]), "bottomCopperThickness": num(&board["bottomCopperThickness"]), "layerSetup": board["layersetup"], "topLayer": num(&board["topLayer"]), "bottomLayer": num(&board["bottomLayer"]), "viaLimit": num(&board["viaLimit"]), "componentCount": summary["componentCount"], "netCount": summary["netCount"]})],
        "Element" => {
            // the summary carries placements; the libraries section links each to its package
            let mut rows: Vec<Value> = summary["components"].as_array().cloned().unwrap_or_default().into_iter().map(|c| json!({"name": c["name"], "value": c["value"], "library": c["library"], "package": c["package"], "x": num(&c["x"]), "y": num(&c["y"]), "unit": unit, "angle": c["rotation"].as_str().and_then(|r| r.trim_start_matches(|ch: char| ch.is_alphabetic()).parse::<f64>().ok()), "rotation": c["rotation"], "mirrored": c["rotation"].as_str().map(|r| r.starts_with('M')).unwrap_or(false), "locked": c["locked"]})).collect();
            if rows.is_empty() { if let Some(lib) = section(xml, "libraries") {
                let mut pkg = String::new();
                for c in g_re.captures_iter(lib) { let a = attrs(&c[1]); if a["package"].is_string() { pkg = a["package"].as_str().unwrap_or("").to_string(); } else if a["name"].is_string() { rows.push(json!({"name": a["name"], "package": pkg, "x": num(&a["x"]), "y": num(&a["y"]), "unit": unit, "rotation": a["rot"], "angle": a["rot"].as_str().and_then(|r| r.trim_start_matches(|ch: char| ch.is_alphabetic()).parse::<f64>().ok()), "locked": a["LOCKED"].as_str() == Some("true"), "objectId": a["id"]})); } }
            } }
            rows
        }
        "Library" | "Package" => {
            let mut rows = Vec::new();
            if let Some(lib) = section(xml, "libraries") { for c in g_re.captures_iter(lib) { let a = attrs(&c[1]); if a["package"].is_string() {
                let bounds: Vec<f64> = a["bounds"].as_str().unwrap_or("").split(',').filter_map(|s| s.trim().parse().ok()).collect();
                rows.push(if entity == "Library" { json!({"name": a["library"], "libraryUrn": a["library_urn"], "package": a["package"], "id": a["id"]}) } else { json!({"name": a["package"], "library": a["library"], "package3dUrn": a["package3d_urn"], "packageUrn": a["package_urn"], "type": a["type"], "usePlaceholder": a["use_placeholder"].as_str() == Some("true"), "version": a["version"], "bounds": bounds, "unit": unit, "id": a["id"]}) });
            } } }
            if entity == "Library" { let mut seen = std::collections::HashSet::new(); rows.retain(|r| seen.insert(r["name"].to_string())); }
            rows
        }
        "Layer" => {
            let copper: Vec<String> = regex::Regex::new(r#"<g id="(\d+)_copper""#).unwrap().captures_iter(xml).map(|c| c[1].to_string()).collect();
            let (top, bottom) = (num(&board["topLayer"]).unwrap_or(1.0) as i64, num(&board["bottomLayer"]).unwrap_or(16.0) as i64);
            let mut rows: Vec<Value> = copper.iter().filter_map(|n| n.parse::<i64>().ok()).map(|n| json!({"number": n, "name": if n == top { "Top" } else if n == bottom { "Bottom" } else if n == 29 { "tStop" } else if n == 30 { "bStop" } else { "" }, "kind": if n == top || n == bottom || (n > top && n < bottom) { "copper" } else { "drawing" }, "hasGeometry": true})).collect();
            rows.sort_by_key(|r| r["number"].as_i64().unwrap_or(0)); rows
        }
        "Signal" | "Net" => summary["nets"].as_array().cloned().unwrap_or_default().into_iter().map(|n| if n.is_string() { json!({"name": n}) } else { n }).collect(),
        "Error" => section(xml, "drc").map(|d| g_re.captures_iter(d).map(|c| attrs(&c[1])).collect()).unwrap_or_default(),
        "Wire" => {
            let mut rows = Vec::new();
            let cre = regex::Regex::new(r"<copper ([^>]*)/>").unwrap();
            for grp in regex::Regex::new(r#"(?s)<g id="(\d+)_copper"[^>]*>(.*?)</g>"#).unwrap().captures_iter(xml) {
                let layer: i64 = grp[1].parse().unwrap_or(0);
                for c in cre.captures_iter(&grp[2]) { let a = attrs(&c[1]); let pts: Vec<f64> = a["d"].as_str().unwrap_or("").split_whitespace().filter_map(|s| s.parse().ok()).collect(); rows.push(json!({"layer": layer, "signal": a["name"], "id": a["id"], "shape": if a["circle"].as_str().map(|s| !s.is_empty()).unwrap_or(false) { "circle" } else { "polygon" }, "points": pts.chunks(3).map(|p| json!([p.get(0), p.get(1)])).collect::<Vec<_>>(), "unit": unit})); }
            }
            rows
        }
        _ => return Err(format!("{entity} is not in the board_info fallback")),
    })
}

fn apply_query(mut rows: Vec<Value>, args: &Value) -> (Vec<Value>, usize) {
    if let Some(fs) = args["filters"].as_array() {
        for f in fs {
            let (p, op) = (f["property"].as_str().unwrap_or("").to_string(), f["op"].as_str().unwrap_or("eq").to_string());
            let want = f["value"].clone();
            rows.retain(|r| { let got = &r[&p]; match op.as_str() { "lt" => num(got).zip(num(&want)).map(|(a, b)| a < b).unwrap_or(false), "gt" => num(got).zip(num(&want)).map(|(a, b)| a > b).unwrap_or(false), _ => got == &want || (got.as_str().map(|s| Some(s) == want.as_str()).unwrap_or(false)) || num(got).zip(num(&want)).map(|(a, b)| a == b).unwrap_or(false) } });
        }
    }
    let total = rows.len();
    if let Some(fields) = args["fields"].as_array() { let keep: Vec<&str> = fields.iter().filter_map(|f| f.as_str()).collect(); rows = rows.into_iter().map(|r| { let mut o = json!({}); for k in &keep { o[*k] = r[*k].clone(); } o }).collect(); }
    let offset = args["offset"].as_u64().unwrap_or(0) as usize; let limit = args["limit"].as_u64().unwrap_or(100) as usize;
    (rows.into_iter().skip(offset).take(limit).collect(), total)
}

/// `fusion_electronics_query {entity, fields?, filters?, limit?, offset?, source?: auto|mcp|board}`.
pub fn query(ctx: &Ctx, args: &Value, version: &str, pass: &dyn Fn(&str, Value) -> Value) -> Value {
    let Some(entity) = args["entity"].as_str().or(args["entity_type"].as_str()).and_then(canonical) else {
        return json!({"success": false, "errorCode": "unknown_entity", "error": format!("entity must be one of Autodesk's electronics classes, got {}", fusion_lbr::py_repr_str(args["entity"].as_str().unwrap_or(""))), "entities": ENTITIES, "boardFallbackEntities": BOARD_ENTITIES, "_hint": "Pass entity:'Element' (board placements), 'Signal' (board nets), 'Net' (schematic nets), 'Layer', 'Error' (DRC/ERC), 'Via', 'Smd', 'Wire', 'Part', 'Instance', 'Package', 'Library'... The full list is `entities`."});
    };
    let source = args["source"].as_str().unwrap_or("auto");
    let _ = ctx;
    if source != "board" {
        let sess = crate::mcp::new_session(version);
        if sess["error"].is_null() {
            let mut object = json!({});
            if let Some(f) = args["fields"].as_array() { object["fields"] = json!(f); }
            if let Some(f) = args["filters"].as_array() { object["filters"] = json!(f); }
            object["pagination"] = json!({"limit": args["limit"].as_u64().unwrap_or(100), "offset": args["offset"].as_u64().unwrap_or(0)});
            let r = crate::mcp::rpc(version, "tools/call", Some(json!({"name": "fusion_mcp_electronics_read", "arguments": {"entity_type": format!("electronics.{entity}"), "object": object}})), args["timeout"].as_u64().unwrap_or(120));
            if r["_mcpError"].is_null() {
                let res = &r["result"];
                let text = res["content"].as_array().and_then(|c| c.iter().find_map(|x| x["text"].as_str())).unwrap_or("").to_string();
                let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                if res["isError"].as_bool() == Some(true) || parsed["success"].as_bool() == Some(false) {
                    let err = parsed["error"].as_str().map(str::to_string).unwrap_or_else(|| text.chars().take(300).collect());
                    if source == "mcp" || !BOARD_ENTITIES.contains(&entity) { return json!({"success": false, "source": "mcp", "entity": entity, "error": err, "_hint": "Autodesk's electronics_read needs an ACTIVE electronics document (fusion_show_2d_board / fusion_show_schematic first). Some classes exist only in one editor: board classes (Element, Signal, Via, Smd, Wire, Error) in the board, schematic classes (Part, Instance, Net, Sheet, Segment) in the schematic. resource://mcp.electronics_entity_types (fusion_mcp_resources) lists the workspace each wants."}); }
                } else {
                    let rows = parsed["rows"].as_array().or(parsed["data"].as_array()).or(parsed["results"].as_array()).or(parsed["items"].as_array()).cloned().or_else(|| parsed.as_array().cloned()).unwrap_or_default();
                    let total = parsed["total"].as_u64().or(parsed["count"].as_u64()).map(|n| n as usize).unwrap_or(rows.len());
                    let unit = parsed["coordinate_unit"].as_str().unwrap_or("document grid units");
                    return json!({"success": true, "source": "mcp", "entity": entity, "count": rows.len(), "total": total, "hasMore": parsed["pagination"]["hasMore"], "coordinateUnit": unit, "rows": rows, "units": format!("Autodesk MCP: x/y in {unit} (coordinate_unit of the active document), angles in degrees; per-class fields in resource://mcp.electronics_schema_<class>"),
                        "raw": if rows.is_empty() { parsed } else { Value::Null },
                        "_hint": "Typed rows from Autodesk's electronics_read over the ACTIVE electronics document. Filter with filters:[{property, op:'eq'|'lt'|'gt', value}] (strings: eq only), project with fields:[...], page with limit/offset. When the MCP server is off this verb falls back to the board_info XML for Board/Element/Library/Package/Layer/Signal/Wire/Error (source:'board', coordinates in mm)."});
                }
            } else if source == "mcp" { return json!({"success": false, "source": "mcp", "entity": entity, "error": r["_mcpError"], "_hint": crate::mcp::off_hint((false, false))}); }
        } else if source == "mcp" { return json!({"success": false, "source": "mcp", "entity": entity, "error": sess["error"], "_hint": crate::mcp::off_hint((false, false))}); }
    }
    if !BOARD_ENTITIES.contains(&entity) {
        return json!({"success": false, "errorCode": "needs_mcp", "source": "board", "entity": entity, "error": format!("{entity} is only readable through Autodesk's MCP server, which is not answering."), "boardFallbackEntities": BOARD_ENTITIES, "_hint": "fusion_mcp_enable turns the server on in ~10s (needs Fusion running); then re-run. The board_info fallback covers Board, Element, Library, Package, Layer, Signal, Wire and Error."});
    }
    let bi = pass("board_info", json!({}));
    if bi["success"].as_bool() != Some(true) { return json!({"success": false, "source": "board", "entity": entity, "error": bi["error"], "_hint": bi["_hint"].as_str().unwrap_or("Open a board first: fusion_show_2d_board (or fusion_open_board {filePath}), then retry.")}); }
    match rows_from_board_info(entity, &bi["data"]) {
        Ok(rows) => { let (page, total) = apply_query(rows, args); json!({"success": true, "source": "board", "entity": entity, "count": page.len(), "total": total, "rows": page, "units": "board_info: coordinates in the board's unit (mm), rotations as Eagle strings (R0, MR90) with `angle` in degrees",
            "_hint": "Rows from the board's own geometry export (Autodesk's MCP server was off or not asked). Element = placements with package and library, Wire = copper shapes per layer, Layer = layers that carry geometry, Error = DRC rows, Signal = board nets. For the schematic side (Part, Instance, Net, Pin, Sheet) and for pads, vias, attributes and variants, turn the MCP server on: fusion_mcp_enable."}) }
        Err(e) => json!({"success": false, "source": "board", "entity": entity, "error": e}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn reads_the_board_export_compat_layout() {
        // what board_info_compat writes when Fusion has no Electron.boardInfo (2706 on)
        let xml = r#"<boardinfo>
<g name="B" unit="mm" boardThickness="1.57" topCopperThickness="0.035" bottomCopperThickness="0.035" layersetup="(1*16)" topLayer="1" bottomLayer="16" viaLimit="2">
<libraries>
<g library="L" package="R0603" library_urn="" package3d_urn="">
<g name="R1" x="1" y="2" rot="R90" LOCKED="false"/>
</g>
</libraries>
<g id="1_copper">
<copper name="GND" id="w1" d="0 0 0 5 0 0"/>
</g>
<drc/>
</g>
</boardinfo>"#;
        let data = json!({"rawXml": xml, "summary": {"components": [{"name": "R1", "library": "L", "package": "R0603", "value": "10k", "x": "1", "y": "2", "rotation": "R90"}], "nets": ["GND"]}});
        let b = rows_from_board_info("Board", &data).unwrap();
        assert_eq!(b[0]["boardThickness"], json!(1.57)); assert_eq!(b[0]["layerSetup"], "(1*16)");
        assert_eq!(rows_from_board_info("Element", &data).unwrap()[0]["name"], "R1");
        assert_eq!(rows_from_board_info("Package", &data).unwrap()[0]["name"], "R0603");
        assert_eq!(rows_from_board_info("Layer", &data).unwrap()[0]["number"], 1);
        assert_eq!(rows_from_board_info("Wire", &data).unwrap()[0]["signal"], "GND");
        assert_eq!(rows_from_board_info("Signal", &data).unwrap()[0]["name"], "GND");
    }
    #[test]
    fn board_fallback_reads_the_sample() {
        let d: Value = serde_json::from_str(include_str!("../tests/fixtures/board_info_sample.json")).unwrap();
        let el = rows_from_board_info("Element", &d["data"]).unwrap();
        assert_eq!(el.len(), 1); assert_eq!(el[0]["name"], "U1"); assert_eq!(el[0]["package"], "ADS8588SIPM"); assert_eq!(el[0]["x"], json!(9.476)); assert_eq!(el[0]["angle"], json!(0.0));
        let b = rows_from_board_info("Board", &d["data"]).unwrap(); assert_eq!(b[0]["boardThickness"], json!(1.5)); assert_eq!(b[0]["layerSetup"], "(1*16)");
        let p = rows_from_board_info("Package", &d["data"]).unwrap(); assert_eq!(p[0]["name"], "ADS8588SIPM"); assert!(p[0]["package3dUrn"].as_str().unwrap().starts_with("urn:adsk"));
        let l = rows_from_board_info("Layer", &d["data"]).unwrap(); assert_eq!(l.iter().map(|r| r["number"].as_i64().unwrap()).collect::<Vec<_>>(), vec![1, 29]);
        let w = rows_from_board_info("Wire", &d["data"]).unwrap(); assert_eq!(w.len(), 68); assert_eq!(w[0]["layer"], 1); assert_eq!(w[0]["points"].as_array().unwrap().len(), 4);
        let (page, total) = apply_query(rows_from_board_info("Wire", &d["data"]).unwrap(), &json!({"filters": [{"property": "layer", "op": "eq", "value": 29}], "limit": 3, "fields": ["id", "layer"]}));
        assert!(total > 0 && total < 68); assert_eq!(page.len(), 3); assert!(page[0]["points"].is_null());
        assert!(canonical("electronics.element").is_some() && canonical("VIA") == Some("Via") && canonical("nope").is_none());
    }
}

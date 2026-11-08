//! Net classes from the names a designer already gave the nets: ground, power, clocks, the fast interfaces (USB,
//! Ethernet, HDMI, PCIe, DDR, MIPI, LVDS, SATA), and the differential pairs their _P/_N (+/-, DP/DN) suffixes spell;
//! proposed, with the net_classes and diff_pairs arguments that write them. From the Altium bridge's autoclass.rs.
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The class a net's name puts it in, if any. Pure.
pub fn classify(net: &str) -> Option<&'static str> {
    let n = net.to_ascii_uppercase();
    let core = n.trim_start_matches(['+', '-', '/', '\\']);
    if crate::placetools::is_ground(&n) || core.starts_with("GND") { return Some("GND"); }
    let power = core.contains("VBUS") || core.starts_with("VCC") || core.starts_with("VDD") || core.starts_with("VBAT") || core.starts_with("VIN") || core.starts_with("VBUS")
        || core.starts_with("VSYS") || core.starts_with("VREF") || core.starts_with("PWR") || core.starts_with("VCORE") || core.starts_with("VOUT")
        || (core.starts_with('V') && core.len() > 1 && core.as_bytes()[1].is_ascii_digit())
        || core.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && (core.contains('V'));
    // a supply's name wins over what it feeds (VDD_XTAL is the crystal's supply, not a clock)
    if power { return Some("Power"); }
    let fast = ["USB", "ETH", "RGMII", "RMII", "SGMII", "HDMI", "PCIE", "DDR", "MIPI", "CSI", "DSI", "LVDS", "SATA", "SERDES", "QSPI", "SDIO", "EMMC"];
    if fast.iter().any(|f| core.contains(f)) { return Some("HighSpeed"); }
    if core.contains("CLK") || core.contains("XTAL") || core.contains("OSC") || core.ends_with("SCK") { return Some("Clock"); }
    None
}

/// Differential pairs by name: X_P/X_N, XP/XN, X+/X-, X_DP/X_DM. Pure.
pub fn pairs(nets: &[String]) -> Vec<(String, String, String)> {
    let set: BTreeMap<String, &String> = nets.iter().map(|n| (n.to_ascii_uppercase(), n)).collect();
    let mut out = Vec::new();
    for (u, orig) in &set {
        // longest suffixes first: USB_DP must pair as USB (DP/DN), not as USB_D (P/N)
        for (p, q) in [("_DP", "_DN"), ("_DP", "_DM"), ("DP", "DN"), ("DP", "DM"), ("_P", "_N"), ("+", "-"), ("P", "N")] {
            if let Some(stem) = u.strip_suffix(p) {
                if stem.is_empty() { continue; }
                if let Some(neg) = set.get(&format!("{stem}{q}")) {
                    let name = stem.trim_end_matches('_').to_string();
                    if !out.iter().any(|(n, _, _): &(String, String, String)| *n == name) { out.push((name, (*orig).clone(), (*neg).clone())); }
                    break;
                }
            }
        }
    }
    out
}

/// auto_classes: input {board, args: {powerWidthMm?}}. A proposal from the net names, and `applyArgs`: one net_classes
/// call per class (Power with a width rule) and one diff_pairs call per pair, for the bridge to run.
pub fn auto_classes(input: &Value) -> Value {
    let st = match crate::input::board(input) { Ok(b) => b, Err(e) => return e };
    let nets: Vec<String> = st["nets"].as_array().cloned().unwrap_or_default().iter().filter_map(|n| n.as_str().map(String::from)).collect();
    let mut classes: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for n in &nets { if let Some(c) = classify(n) { classes.entry(c).or_default().push(n.clone()); } }
    let prs = pairs(&nets);
    let width = input["args"]["powerWidthMm"].as_f64().unwrap_or(0.5);
    let net_classes: Vec<Value> = classes.iter().map(|(c, members)| { let mut a = json!({"name": c, "nets": members}); if *c == "Power" { a["width"] = json!(width); } a }).collect();
    let diff_pairs: Vec<Value> = prs.iter().map(|(n, p, q)| json!({"name": n, "pos": p, "neg": q})).collect();
    json!({"success": true, "applied": false, "proposal": {"classes": classes, "pairs": diff_pairs, "unclassified": nets.iter().filter(|n| classify(n).is_none()).count()},
        "applyArgs": {"net_classes": net_classes, "diff_pairs": diff_pairs},
        "_hint": "A proposal from the net names. Check it, then run each applyArgs.net_classes entry through net_classes (Power gets a width rule, powerWidthMm) and each applyArgs.diff_pairs entry through diff_pairs. Then impedance sizes the pairs and current_capacity checks the power nets."})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_sort_into_classes_and_pairs() {
        for (n, c) in [("GND", Some("GND")), ("AGND", Some("GND")), ("+3V3", Some("Power")), ("VDD_CORE", Some("Power")), ("5V", Some("Power")), ("VBUS", Some("Power")),
                       ("USB_DP", Some("HighSpeed")), ("ETH_TX_P", Some("HighSpeed")), ("MCLK", Some("Clock")), ("VDD_XTAL", Some("Power")), ("USB_VBUS", Some("Power")), ("XTAL_IN", Some("Clock")), ("SPI_SCK", Some("Clock")), ("N1", None), ("LED_RED", None)] {
            assert_eq!(classify(n), c, "{n}");
        }
        let nets: Vec<String> = ["USB_DP", "USB_DN", "ETH_TX_P", "ETH_TX_N", "LVDS0+", "LVDS0-", "SDA"].iter().map(|s| s.to_string()).collect();
        let p = pairs(&nets);
        assert_eq!(p.len(), 3);
        assert!(p.iter().any(|(n, a, b)| n == "USB" && a == "USB_DP" && b == "USB_DN"));
        assert!(p.iter().any(|(n, a, b)| n == "LVDS0" && a == "LVDS0+" && b == "LVDS0-"));
    }

    #[test]
    fn the_verb_answers_the_arguments_that_write_them() {
        let r = auto_classes(&json!({"board": {"nets": ["GND", "+3V3", "USB_DP", "USB_DN", "LED"]}, "args": {"powerWidthMm": 0.6}}));
        assert_eq!(r["proposal"]["unclassified"], json!(1));
        assert!(r["applyArgs"]["net_classes"].as_array().unwrap().iter().any(|c| c["name"] == "Power" && c["width"] == json!(0.6)));
        assert_eq!(r["applyArgs"]["diff_pairs"], json!([{"name": "USB", "pos": "USB_DP", "neg": "USB_DN"}]));
    }
}

use std::fs;
#[test]
fn alert_parsing_matches_the_python_bridge() {
    let g: serde_json::Value = serde_json::from_str(&fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/alerts.json")).unwrap()).unwrap();
    let lines: Vec<String> = serde_json::from_value(g["lines"].clone()).unwrap();
    // cutoff 2026-09-03T07:30:00 treated as UTC, exactly as the golden generator did
    let cutoff: i64 = 20_699 * 86_400 + 7 * 3_600 + 30 * 60; // 2026-09-03 is day 20699 since the epoch
    let got = fusion_core::alerts::parse_alert_lines(lines.iter().map(|s| s.as_str()), cutoff);
    let want: Vec<fusion_core::alerts::Alert> = serde_json::from_value(g["expected"].clone()).unwrap();
    assert_eq!(got, want);
    let since = fusion_core::alerts::alerts_since(&got, "20260903T074100");
    assert!(since.iter().all(|a| a.at.as_deref().map(|s| s > "20260903T074100").unwrap_or(false)));
}

//! Fusion's own alert toasts. A CEF alert has no top-level window, so it never shows in a
//! dialog enumeration; Fusion writes it to its AppLogFile as a `New Alert message` line,
//! and the bridge reads that instead of guessing from pixels. Bridge-side on purpose: it
//! works with the add-in wedged and needs no Fusion restart to deploy.
//!
//! The parser is pure (lines in, alerts out) so it is golden-tested against the Python
//! bridge's `_fusion_alerts`; `HostPaths::fusion_log_dir` says where the log is.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct Alert {
    /// The log stamp `YYYYMMDDTHHMMSS` (local time, as Fusion writes it), None when the line
    /// had none. Watermarks compare these strings lexically.
    pub at: Option<String>,
    pub text: String,
}

/// `YYYYMMDDTHHMMSS` → seconds since the epoch, treating the stamp as UTC (the caller
/// supplies its cutoff in the same convention, so the comparison is consistent).
fn stamp_secs(s: &str) -> Option<i64> {
    if s.len() != 15 || s.as_bytes()[8] != b'T' { return None; }
    let n = |a: usize, b: usize| s[a..b].parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (n(0, 4)?, n(4, 6)?, n(6, 8)?, n(9, 11)?, n(11, 13)?, n(13, 15)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 { return None; }
    // days from civil (Howard Hinnant), no leap-second handling, matching strptime().timestamp()
    let (y2, m2) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + se)
}

/// Parse `New Alert message` lines; those stamped before `cutoff_secs` are dropped, unstamped
/// lines are kept. Text is the Short Message, else the Tooltip, else the raw remainder.
pub fn parse_alert_lines<'a>(lines: impl IntoIterator<Item = &'a str>, cutoff_secs: i64) -> Vec<Alert> {
    let mut out = Vec::new();
    for line in lines {
        if !line.contains("New Alert message") { continue; }
        let trimmed = line.trim_start_matches([' ', '\t']);
        let at: Option<String> = if trimmed.len() >= 15 && stamp_secs(&trimmed[..15]).is_some() { Some(trimmed[..15].to_string()) } else { None };
        if let Some(a) = &at { if stamp_secs(a).unwrap() < cutoff_secs { continue; } }
        let rest = line.splitn(2, "New Alert message.").nth(1).unwrap_or("").trim_end();
        let field = |key: &str| -> Option<String> {
            let i = rest.find(key)?;
            let after = &rest[i + key.len()..];
            let end = ["," ,"\n"].iter().filter_map(|d| {
                // a field runs to the next ", <Label>:" boundary
                after.match_indices(d).map(|(k, _)| k).find(|&k| d == &"\n" || after[k..].starts_with(", Short Message:") || after[k..].starts_with(", Long Message:"))
            }).min().unwrap_or(after.len());
            Some(after[..end].trim().to_string())
        };
        let mut text = field("Short Message:").filter(|s| !s.is_empty()).or_else(|| field("Tooltip:").filter(|s| !s.is_empty())).unwrap_or_default();
        if text.is_empty() { text = rest.trim().chars().take(400).collect(); }
        out.push(Alert { at, text: text.chars().take(400).collect() });
    }
    out
}

/// Alerts strictly AFTER a watermark stamp (the newest `at` seen when an action started), so
/// a stale alert from an earlier action is never blamed on this one.
pub fn alerts_since<'a>(alerts: &'a [Alert], watermark: &str) -> Vec<&'a Alert> {
    if watermark.is_empty() { return alerts.iter().collect(); }
    alerts.iter().filter(|a| a.at.as_deref().unwrap_or("") > watermark).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stamps_round_trip() {
        assert_eq!(stamp_secs("19700101T000000"), Some(0));
        assert_eq!(stamp_secs("20260903T073000"), Some(20_699 * 86_400 + 7 * 3_600 + 30 * 60));
        assert_eq!(stamp_secs("20000301T000000"), Some(951868800));
        assert!(stamp_secs("2026090T073000").is_none());
    }
}

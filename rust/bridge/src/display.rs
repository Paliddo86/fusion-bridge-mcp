//! Fusion's own graphics facts, read from its machine options file: when Fusion has detected an
//! indirect display adapter (DisplayLink and other USB displays) it switches its graphics driver
//! for the whole process, and Fusion Electronics then dies in its D3D12 canvas
//! (SkiaWindowContextDx12, "unrecoverable d3d12") whatever monitor Fusion is on (page issue #121:
//! three crashes with the adapters plugged in, none without). The bridge cannot fix that; it can
//! say so before a tour or an import starts instead of after the crash.
use serde_json::{json, Value};

/// `%APPDATA%/Autodesk/Neutron Platform/Options/NMachineSpecificOptions.xml` (Fusion writes it UTF-16).
fn options_path() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|a| std::path::Path::new(&a).join("Autodesk").join("Neutron Platform").join("Options").join("NMachineSpecificOptions.xml"))
}

fn text_of(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn value_of(text: &str, element: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r#"<{element}\b[^>]*\bValue="([^"]*)""#)).ok()?;
    re.captures(text).map(|c| c[1].to_string())
}

/// The warning for an options file's bytes, or None when Fusion has not detected an indirect adapter.
pub fn parse(bytes: &[u8]) -> Option<Value> {
    let text = text_of(bytes);
    if value_of(&text, "IndirectDisplayAdapterDetected").as_deref() != Some("1") { return None; }
    let driver = value_of(&text, "driverOptionId");
    Some(json!({
        "indirectDisplayAdapter": true,
        "graphicsDriver": driver,
        "risk": "Fusion Electronics may crash with an unrecoverable D3D12 error (SkiaWindowContextDx12), on any monitor",
        "fix": "Unplug the DisplayLink (USB) display adapters and restart Fusion; Fusion re-detects at launch.",
        "source": "Fusion's NMachineSpecificOptions.xml: IndirectDisplayAdapterDetected=1",
    }))
}

/// The warning for this machine, read fresh each call (the file is small and Fusion rewrites it at launch).
pub fn warning() -> Option<Value> {
    options_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| parse(&b))
}

/// Add the warning to a reply (and to its hint) when there is one.
pub fn annotate(mut reply: Value) -> Value {
    if let (Some(w), true) = (warning(), reply.is_object()) {
        let line = "DisplayLink detected: Fusion Electronics may crash with an unrecoverable D3D12 error. Unplug the DisplayLink adapters and restart Fusion before a tour or an Electronics import.";
        reply["_hint"] = json!(format!("{line} {}", reply["_hint"].as_str().unwrap_or("")).trim().to_string());
        reply["displayWarning"] = w;
    }
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    fn utf16(s: &str) -> Vec<u8> { let mut b = vec![0xFF, 0xFE]; for u in s.encode_utf16() { b.extend_from_slice(&u.to_le_bytes()); } b }

    #[test]
    fn displaylink_box_is_flagged() {
        // the lines from the box in page issue #121
        let xml = r#"<?xml version="1.0" encoding="UTF-16"?><OptionGroups><GraphicsOptionGroup>
<driverOptionId UserName="Graphics driver" Value="VirtualDeviceDx11"/>
<IndirectDisplayAdapterDetected UserName="IndirectDisplayAdapterDetected" Value="1"/></GraphicsOptionGroup></OptionGroups>"#;
        let w = parse(&utf16(xml)).expect("flagged");
        assert_eq!(w["graphicsDriver"], "VirtualDeviceDx11");
        assert!(parse(xml.as_bytes()).is_some(), "UTF-8 too");
    }

    #[test]
    fn ordinary_box_is_not() {
        assert!(parse(&utf16(r#"<OptionGroups><DisplayOptionGroup UserName="Display"></DisplayOptionGroup></OptionGroups>"#)).is_none());
        assert!(parse(&utf16(r#"<IndirectDisplayAdapterDetected UserName="x" Value="0"/>"#)).is_none());
    }
}

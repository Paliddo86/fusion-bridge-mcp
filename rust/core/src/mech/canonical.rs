//! Python's `json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False)` with its
//! default `ensure_ascii`, byte for byte. Request digests and journal records written by the
//! Python add-in must decode and re-encode identically here (tests/golden/mech_canonical.json).

use serde_json::Value;

pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = String::new();
    write_value(value, &mut out);
    out.into_bytes()
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => out.push_str(&i.to_string()),
            (None, Some(u), _) => out.push_str(&u.to_string()),
            (_, _, f) => out.push_str(&float_repr(f.unwrap_or(0.0))),
        },
        Value::String(s) => write_str(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 { out.push(','); }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            // Python sorts by code point; UTF-8 byte order is the same order.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 { out.push(','); }
                write_str(key, out);
                out.push(':');
                write_value(&map[*key], out);
            }
            out.push('}');
        }
    }
}

/// `ascii_escape_unicode`: printable ASCII stays, the six short escapes, everything else
/// (DEL included) is `\uXXXX` in lowercase hex, astral characters as surrogate pairs.
fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) { out.push_str(&format!("\\u{unit:04x}")); }
            }
        }
    }
    out.push('"');
}

/// `float.__repr__`: the shortest round-trip digits, positional while the decimal point sits
/// within -4 < decpt <= 16, else `d.ddde+XX` with at least two exponent digits.
pub fn float_repr(x: f64) -> String {
    if x == 0.0 { return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() }; }
    let sign = if x < 0.0 { "-" } else { "" };
    let shortest = format!("{:e}", x.abs());
    let (mantissa, exp) = shortest.split_once('e').unwrap_or((&shortest, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let decpt = exp + 1;
    if decpt <= -4 || decpt > 16 {
        let rest = if digits.len() > 1 { format!(".{}", &digits[1..]) } else { String::new() };
        format!("{sign}{}{rest}e{}{:02}", &digits[..1], if exp < 0 { '-' } else { '+' }, exp.abs())
    } else if decpt <= 0 {
        format!("{sign}0.{}{digits}", "0".repeat((-decpt) as usize))
    } else if decpt as usize >= digits.len() {
        format!("{sign}{digits}{}.0", "0".repeat(decpt as usize - digits.len()))
    } else {
        format!("{sign}{}.{}", &digits[..decpt as usize], &digits[decpt as usize..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn floats_follow_python_repr() {
        for (x, want) in [(1e-5, "1e-05"), (1e16, "1e+16"), (1e15, "1000000000000000.0"), (0.0001, "0.0001"), (-0.0, "-0.0"), (100.0, "100.0"), (5e-324, "5e-324"), (1.5e16, "1.5e+16")] {
            assert_eq!(float_repr(x), want, "{x}");
        }
    }
    #[test]
    fn keys_sort_and_strings_escape_like_python() {
        assert_eq!(String::from_utf8(encode(&json!({"b": 1, "a": [true, null], "\u{e9}": "\u{7f}\u{1f600}"}))).unwrap(), "{\"a\":[true,null],\"b\":1,\"\\u00e9\":\"\\u007f\\ud83d\\ude00\"}");
    }
}

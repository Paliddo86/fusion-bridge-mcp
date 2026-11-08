//! Rust replacement for catalog authoring; no Python execution or CAD operations.
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fmt, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
pub mod literal;
pub type Result<T> = std::result::Result<T, String>;
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
pub fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Unique, E> {
                Ok(Unique(json!(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Unique, E> {
                Ok(Unique(json!(v)))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Unique, E> {
                Ok(Unique(json!(v)))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Unique, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| Unique(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Unique, E> {
                Ok(Unique(json!(v)))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Unique, E> {
                Ok(Unique(json!(v)))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut v = vec![];
                while let Some(Unique(x)) = a.next_element()? {
                    v.push(x)
                }
                Ok(Unique(Value::Array(v)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut m = Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if m.contains_key(&k) {
                        return Err(de::Error::custom(format!("duplicate key: {k}")));
                    }
                    let Unique(v) = a.next_value()?;
                    m.insert(k, v);
                }
                Ok(Unique(Value::Object(m)))
            }
        }
        d.deserialize_any(V)
    }
}
pub fn parse(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > MAX_BYTES {
        return Err("input exceeds 4MiB".into());
    }
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let Unique(v) = Unique::deserialize(&mut d).map_err(|e| e.to_string())?;
    d.end().map_err(|e| e.to_string())?;
    Ok(v)
}
pub fn read(path: &Path) -> Result<Vec<u8>> {
    let mut v = vec![];
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut v)
        .map_err(|e| e.to_string())?;
    if v.len() > MAX_BYTES {
        return Err("input exceeds 4MiB".into());
    }
    Ok(v)
}
pub fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(o) => {
            let mut keys: Vec<_> = o.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), sorted(&o[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        _ => v.clone(),
    }
}
/// Python json.dumps(indent=2,ensure_ascii=True) encoding, including surrogate pairs.
pub fn encode(v: &Value, sort: bool) -> Result<Vec<u8>> {
    let v = if sort { sorted(v) } else { v.clone() };
    let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    let text = python_exponents(&text);
    let mut out = String::new();
    for c in text.chars() {
        if (c as u32) >= 127 {
            let mut b = [0; 2];
            for u in c.encode_utf16(&mut b) {
                out.push_str(&format!("\\u{u:04x}"));
            }
        } else {
            out.push(c)
        }
    }
    out.push('\n');
    if out.len() > MAX_BYTES {
        return Err("encoded output exceeds 4MiB".into());
    }
    Ok(out.into_bytes())
}
fn keys(v: &Value, wanted: &[&str]) -> Result<()> {
    let o = v.as_object().ok_or("expected object")?;
    if o.len() != wanted.len() || wanted.iter().any(|k| !o.contains_key(*k)) {
        return Err("unexpected or missing object fields".into());
    }
    Ok(())
}
fn expand(v: &Value, schema: &Value, status: &str) -> Result<Value> {
    if let Some(o) = v.as_object() {
        if o.contains_key("$mechanicalSchema") {
            keys(v, &["$mechanicalSchema"])?;
            if v["$mechanicalSchema"] != true {
                return Err("invalid schema marker".into());
            }
            return Ok(schema.clone());
        }
        if o.contains_key("$mechanicalOperationEnum") {
            keys(v, &["$mechanicalOperationEnum"])?;
            if v["$mechanicalOperationEnum"] != true {
                return Err("invalid enum marker".into());
            }
            return Ok(json!(format!(
                "required enum: {}",
                schema
                    .as_object()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("|")
            )));
        }
        if o.contains_key("$statusDefault") {
            keys(v, &["$statusDefault"])?;
            if v["$statusDefault"] != true {
                return Err("invalid status marker".into());
            }
            return Ok(json!(status));
        }
        let mut out = Map::new();
        for (k, x) in o {
            out.insert(k.clone(), expand(x, schema, status)?);
        }
        return Ok(Value::Object(out));
    }
    if let Some(a) = v.as_array() {
        return a
            .iter()
            .map(|x| expand(x, schema, status))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array);
    }
    Ok(v.clone())
}
#[derive(Clone)]
pub struct Catalog {
    rows: Vec<Value>,
    metadata: Value,
    canonical: Value,
}
impl Catalog {
    pub fn load(source: &[u8], schema_bytes: &[u8]) -> Result<Self> {
        let s = parse(source)?;
        keys(
            &s,
            &[
                "formatVersion",
                "defaultStatus",
                "mechanicalSchemaSha256",
                "mechanicalOperationOrder",
                "metadata",
                "rows",
            ],
        )?;
        if s["formatVersion"] != 1 {
            return Err("unsupported canonical format".into());
        }
        let schema = parse(schema_bytes)?;
        let o = schema
            .as_object()
            .filter(|o| !o.is_empty())
            .ok_or("schema must be nonempty object")?;
        if s["mechanicalSchemaSha256"].as_str() != Some(&sha(schema_bytes)) {
            return Err("mechanical schema digest mismatch".into());
        }
        if s["mechanicalOperationOrder"] != json!(o.keys().collect::<Vec<_>>()) {
            return Err("mechanical schema order mismatch".into());
        }
        for (name, op) in o {
            if name.is_empty() || !op.is_object() {
                return Err("invalid operation schema".into());
            }
        }
        let status = s["defaultStatus"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("default status missing")?;
        keys(&s["metadata"], &["contract", "frozen", "note", "source"])?;
        if s["metadata"]
            .as_object()
            .unwrap()
            .values()
            .any(|v| !v.is_string())
        {
            return Err("metadata must contain strings".into());
        }
        fn schemas(v: &Value) -> usize {
            match v {
                Value::Object(o) => {
                    if o.contains_key("$mechanicalSchema") {
                        1
                    } else {
                        o.values().map(schemas).sum()
                    }
                }
                Value::Array(a) => a.iter().map(schemas).sum(),
                _ => 0,
            }
        }
        let expanded_bound = schemas(&s["rows"])
            .checked_mul(schema_bytes.len())
            .and_then(|n| n.checked_add(source.len()))
            .ok_or("expansion bound")?;
        if expanded_bound > MAX_BYTES / 2 {
            return Err("schema expansion exceeds bounded catalog budget".into());
        }
        let table = s["rows"]
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 4096)
            .ok_or("row count bound")?;
        let mut seen = BTreeSet::new();
        let mut rows = vec![];
        for raw in table {
            let expanded = expand(raw, &schema, status)?;
            let r = expanded
                .as_array()
                .filter(|r| r.len() == 8)
                .ok_or("canonical row must have exactly eight fields")?;
            let name = r[0]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 256)
                .ok_or("invalid verb name")?;
            if !seen.insert(name.to_owned()) {
                return Err(format!("duplicate verb: {name}"));
            }
            if !r[1].is_string()
                || ![2, 3, 7].iter().all(|&i| r[i].is_object())
                || r[4].as_u64().is_none_or(|n| n == 0 || n > 86400)
                || !(r[5].is_null() || r[5].is_string())
                || !r[6].is_boolean()
            {
                return Err(format!("invalid row types: {name}"));
            }
            rows.push(json!({"name":r[0],"summary":r[1],"input":r[2],"output":r[3],"timeoutSeconds":r[4],"statusVerb":r[5],"longRunning":r[6],"example":r[7]}));
        }
        Ok(Self {
            rows,
            metadata: s["metadata"].clone(),
            canonical: s,
        })
    }
    pub fn describe(&self) -> Value {
        let rows: Vec<_> = self
            .rows
            .iter()
            .filter(|v| !v["name"].as_str().unwrap().ends_with("_note"))
            .cloned()
            .collect();
        json!({"success":true,"bridge":"fusion360","verbCount":rows.len(),"verbs":rows,"data":{"bridge":"fusion360","verbCount":rows.len(),"verbs":rows}})
    }
    pub fn frozen(&self) -> Value {
        let mut rows = self.rows.clone();
        rows.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        let mut out = self.metadata.clone();
        out["verbs"] = json!(rows);
        sorted(&out)
    }
    pub fn check(&self, baseline: &[u8]) -> Result<Value> {
        let old = parse(baseline)?;
        let generated = self.frozen();
        if old != generated {
            return Err(
                "complete frozen contract differs from baseline; no output published".into(),
            );
        }
        let bytes = encode(&generated, true)?;
        if bytes != baseline {
            return Err("baseline encoding/order differs; no silent normalization".into());
        }
        Ok(
            json!({"success":true,"frozenVerbs":self.rows.len(),"runtimeVerbs":self.describe()["verbCount"],"completeContractByteEqual":true,"contractSha256":sha(&bytes)}),
        )
    }
    pub fn manifest(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        let mut m = parse(bytes)?;
        if !m.is_object() || !m["verbs"].is_array() {
            return Err("bridge manifest verbs array missing".into());
        }
        let names = self.frozen()["verbs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].clone())
            .collect::<Vec<_>>();
        if m["verbs"] == json!(names) {
            return Ok(bytes.to_vec());
        }
        m["verbs"] = json!(names);
        encode(&m, false)
    }
    pub fn freeze(&self, baseline: &[u8], manifest: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
        self.check(baseline)?;
        Ok(vec![
            ("describe.json".into(), baseline.to_vec()),
            ("bridge.json".into(), self.manifest(manifest)?),
        ])
    }
    /// Explicit authoring mode. Like the legacy freezer, unchanged verbs preserve
    /// the entire old contract; changed verbs refresh metadata and frozen date.
    /// The caller supplies the local date instead of consulting an implicit clock.
    pub fn refreeze(
        &self,
        prior: Option<&[u8]>,
        manifest: &[u8],
        date: &str,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        validate_date(date)?;
        let mut next = self.frozen();
        next["frozen"] = json!(date);
        let (doc, bytes) = if let Some(bytes) = prior {
            let old = parse(bytes)?;
            if !old.is_object() {
                return Err("prior contract must be an object".into());
            }
            if let Some(verbs) = old.get("verbs") {
                if !verbs.is_array() {
                    return Err("prior contract verbs must be an array".into());
                }
            }
            if old.get("verbs") == next.get("verbs") {
                keys(&old, &["contract", "frozen", "note", "source", "verbs"])?;
                if ["contract", "frozen", "note", "source"]
                    .iter()
                    .any(|key| !old[*key].is_string())
                {
                    return Err("unchanged prior metadata must contain strings".into());
                }
                (old, bytes.to_vec())
            } else {
                let bytes = encode(&next, true)?;
                (next, bytes)
            }
        } else {
            let bytes = encode(&next, true)?;
            (next, bytes)
        };
        let mut canonical = self.canonical.clone();
        let mut metadata = doc;
        metadata.as_object_mut().unwrap().remove("verbs");
        canonical["metadata"] = metadata;
        Ok(vec![
            ("describe.json".into(), bytes),
            ("bridge.json".into(), self.manifest(manifest)?),
            ("catalog.json".into(), encode(&canonical, false)?),
        ])
    }
}
fn validate_date(date: &str) -> Result<()> {
    let b = date.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || b.iter()
            .enumerate()
            .any(|(i, c)| i != 4 && i != 7 && !c.is_ascii_digit())
    {
        return Err("date must be explicit YYYY-MM-DD".into());
    }
    let y: u32 = date[..4].parse().unwrap();
    let m: u32 = date[5..7].parse().unwrap();
    let d: u32 = date[8..].parse().unwrap();
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    if y == 0 || d == 0 || d > days {
        return Err("invalid Gregorian date".into());
    }
    Ok(())
}
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
/// Publish a completed fresh directory without replacing any existing path.
pub fn publish(dest: &Path, files: &[(String, Vec<u8>)]) -> Result<()> {
    if files.is_empty()
        || files.iter().any(|(n, b)| {
            n.contains(['/', '\\']) || n == "." || n == ".." || n.is_empty() || b.len() > MAX_BYTES
        })
    {
        return Err("invalid output bundle".into());
    }
    let mut names = BTreeSet::new();
    if files.iter().any(|(n, _)| !names.insert(n)) {
        return Err("duplicate output name".into());
    }
    let parent = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temp = parent.join(format!(
        ".contract-staging-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&temp).map_err(|e| e.to_string())?;
    let guard = Temp(temp.clone());
    for (n, b) in files {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temp.join(n))
            .map_err(|e| e.to_string())?;
        f.write_all(b)
            .and_then(|_| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    rename_exclusive(&temp, dest)?;
    drop(guard);
    Ok(())
}
#[cfg(target_os = "linux")]
fn rename_exclusive(a: &Path, b: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn renameat2(
            olddirfd: i32,
            oldpath: *const std::ffi::c_char,
            newdirfd: i32,
            newpath: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    let a = CString::new(a.as_os_str().as_bytes()).map_err(|_| "NUL path")?;
    let b = CString::new(b.as_os_str().as_bytes()).map_err(|_| "NUL path")?;
    if unsafe { renameat2(-100, a.as_ptr(), -100, b.as_ptr(), 1) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}
#[cfg(windows)]
fn rename_exclusive(a: &Path, b: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileW(from: *const u16, to: *const u16) -> i32;
    }
    let convert = |p: &Path| -> Result<Vec<u16>> {
        let mut x: Vec<u16> = p.as_os_str().encode_wide().collect();
        if x.contains(&0) {
            return Err("NUL path".into());
        }
        x.push(0);
        Ok(x)
    };
    let a = convert(a)?;
    let b = convert(b)?;
    if unsafe { MoveFileW(a.as_ptr(), b.as_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}
#[cfg(not(any(target_os = "linux", windows)))]
fn rename_exclusive(_: &Path, _: &Path) -> Result<()> {
    Err("exclusive directory publication unsupported on this platform".into())
}
#[cfg(test)]
mod tests;

fn python_exponents(s: &str) -> String {
    let mut out = String::new();
    let mut p = 0;
    let b = s.as_bytes();
    let mut quoted = false;
    let mut escaped = false;
    while p < b.len() {
        let c = s[p..].chars().next().unwrap();
        if quoted {
            out.push(c);
            p += c.len_utf8();
            if escaped {
                escaped = false
            } else if c == '\\' {
                escaped = true
            } else if c == '"' {
                quoted = false
            }
            continue;
        }
        if c == '"' {
            quoted = true;
            out.push(c);
            p += 1;
            continue;
        }
        if c == '-' || c.is_ascii_digit() {
            let start = p;
            p += 1;
            while p < b.len() && (b[p].is_ascii_digit() || b".eE+-".contains(&b[p])) {
                p += 1
            }
            let token = &s[start..p];
            if let Some(i) = token.find(['e', 'E']) {
                let e: i32 = token[i + 1..].parse().expect("serializer exponent");
                out.push_str(&token[..i]);
                out.push('e');
                out.push(if e < 0 { '-' } else { '+' });
                out.push_str(&format!("{:02}", e.unsigned_abs()));
            } else {
                out.push_str(token)
            }
            continue;
        }
        out.push(c);
        p += c.len_utf8();
    }
    out
}

//! One-time bounded data extraction from the archived literal table. No evaluator.
use crate::*;
struct Parser<'a> {
    s: &'a str,
    p: usize,
    depth: usize,
}
impl Parser<'_> {
    fn skip(&mut self) {
        loop {
            while self
                .s
                .as_bytes()
                .get(self.p)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.p += 1;
            }
            if self.s.as_bytes().get(self.p) == Some(&b'#') {
                while self.s.as_bytes().get(self.p).is_some_and(|b| *b != b'\n') {
                    self.p += 1;
                }
            } else {
                break;
            }
        }
    }
    fn consume(&mut self, s: &str) -> bool {
        self.skip();
        if self.s[self.p..].starts_with(s) {
            self.p += s.len();
            true
        } else {
            false
        }
    }
    fn need(&mut self, s: &str) -> Result<()> {
        if self.consume(s) {
            Ok(())
        } else {
            Err(format!("expected {s} at byte {}", self.p))
        }
    }
    fn string(&mut self) -> Result<String> {
        self.skip();
        let quote = *self.s.as_bytes().get(self.p).ok_or("missing string")?;
        if quote != b'"' && quote != b'\'' {
            return Err("expected quoted string".into());
        }
        self.p += 1;
        let mut out = String::new();
        while self.p < self.s.len() {
            let c = self.s[self.p..].chars().next().unwrap();
            self.p += c.len_utf8();
            if c as u32 == quote as u32 {
                return Ok(out);
            }
            if c != '\\' {
                out.push(c);
                continue;
            }
            let e = self.s[self.p..].chars().next().ok_or("truncated escape")?;
            self.p += e.len_utf8();
            out.push(match e {
                '\\' => '\\',
                '\'' => '\'',
                '"' => '"',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                'b' => '\x08',
                'f' => '\x0c',
                'a' => '\x07',
                'v' => '\x0b',
                'u' | 'U' | 'x' => {
                    let n = if e == 'u' {
                        4
                    } else if e == 'U' {
                        8
                    } else {
                        2
                    };
                    let end = self
                        .p
                        .checked_add(n)
                        .filter(|i| *i <= self.s.len())
                        .ok_or("truncated hex escape")?;
                    let v = u32::from_str_radix(&self.s[self.p..end], 16)
                        .map_err(|_| "invalid escape")?;
                    self.p = end;
                    char::from_u32(v).ok_or("invalid Unicode escape")?
                }
                _ => return Err(format!("unsupported literal escape at {}", self.p)),
            });
        }
        Err("unterminated string".into())
    }
    fn value(&mut self) -> Result<Value> {
        self.skip();
        self.depth += 1;
        if self.depth > 64 {
            return Err("literal depth bound".into());
        }
        let result = self.inner();
        self.depth -= 1;
        result
    }
    fn inner(&mut self) -> Result<Value> {
        self.skip();
        if self.s[self.p..].starts_with("\"required enum: \" + \"|\".join(_MECHANICAL_SCHEMA)") {
            self.p += "\"required enum: \" + \"|\".join(_MECHANICAL_SCHEMA)".len();
            return Ok(json!({"$mechanicalOperationEnum":true}));
        }
        if self.consume("_MECHANICAL_SCHEMA") {
            return Ok(json!({"$mechanicalSchema":true}));
        }
        if self.consume("_STATUS") {
            return Ok(json!({"$statusDefault":true}));
        }
        if self.consume("None") {
            return Ok(Value::Null);
        }
        if self.consume("True") {
            return Ok(json!(true));
        }
        if self.consume("False") {
            return Ok(json!(false));
        }
        let c = *self.s.as_bytes().get(self.p).ok_or("missing value")?;
        if c == b'"' || c == b'\'' {
            return self.string().map(Value::String);
        }
        if c == b'[' || c == b'(' {
            self.p += 1;
            let end = if c == b'[' { "]" } else { ")" };
            let mut out = vec![];
            while !self.consume(end) {
                out.push(self.value()?);
                if self.consume(end) {
                    break;
                }
                self.need(",")?;
            }
            return Ok(Value::Array(out));
        }
        if c == b'{' {
            self.p += 1;
            let mut out = Map::new();
            while !self.consume("}") {
                let k = self.string()?;
                self.need(":")?;
                let v = self.value()?;
                if out.insert(k, v).is_some() {
                    return Err("duplicate literal key".into());
                }
                if self.consume("}") {
                    break;
                }
                self.need(",")?;
            }
            return Ok(Value::Object(out));
        }
        let start = self.p;
        while self
            .s
            .as_bytes()
            .get(self.p)
            .is_some_and(|b| b.is_ascii_digit() || b"-+.eE".contains(b))
        {
            self.p += 1;
        }
        if start == self.p {
            return Err(format!("unsupported executable syntax at {}", self.p));
        }
        parse(self.s[start..self.p].as_bytes())
    }
}
pub fn extract(python_text: &str, schema: &[u8], baseline: &[u8]) -> Result<Value> {
    if python_text.len() > MAX_BYTES {
        return Err("source bound".into());
    }
    if sha(python_text.as_bytes())
        != "d03c128566455226bca6b7535a2690577f39c2ac851cc71982f7afae8e1fcc35"
    {
        return Err("legacy source is not the audited exact baseline".into());
    }
    let status_start = python_text
        .find("\n_STATUS = ")
        .ok_or("status assignment missing")?
        + "\n_STATUS = ".len();
    let mut p = Parser {
        s: python_text,
        p: status_start,
        depth: 0,
    };
    let default = p.string()?;
    let start = python_text
        .find("\n_T = [")
        .ok_or("table assignment missing")?
        + "\n_T = ".len();
    let mut p = Parser {
        s: python_text,
        p: start,
        depth: 0,
    };
    let rows = p.value()?;
    p.skip();
    if !python_text[p.p..].starts_with("def _verbs():") {
        return Err("unexpected code between table and _verbs".into());
    }
    let schema_value = parse(schema)?;
    let o = schema_value.as_object().ok_or("invalid schema")?;
    let mut metadata = parse(baseline)?;
    metadata
        .as_object_mut()
        .ok_or("baseline object")?
        .shift_remove("verbs");
    Ok(
        json!({"formatVersion":1,"defaultStatus":default,"mechanicalSchemaSha256":sha(schema),"mechanicalOperationOrder":o.keys().collect::<Vec<_>>(),"metadata":metadata,"rows":rows}),
    )
}

//! JSON values written exactly as Python's `json.dumps` writes them (key order kept, `ensure_ascii`, float `repr`,
//! `indent=n` / compact separators), and a small reader for the checks.

use crate::py::{Num, float_repr};

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Obj),
}

/// An object with Python dict semantics: insertion order; assigning an existing key keeps its place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Obj(pub Vec<(String, Json)>);

impl Obj {
    pub fn new() -> Obj {
        Obj(Vec::new())
    }
    pub fn set(&mut self, k: &str, v: impl Into<Json>) {
        let v = v.into();
        if let Some(e) = self.0.iter_mut().find(|e| e.0 == k) {
            e.1 = v;
        } else {
            self.0.push((k.to_string(), v));
        }
    }
    /// Builder form of `set`.
    pub fn with(mut self, k: &str, v: impl Into<Json>) -> Obj {
        self.set(k, v);
        self
    }
    pub fn get(&self, k: &str) -> Option<&Json> {
        self.0.iter().find(|e| e.0 == k).map(|e| &e.1)
    }
    pub fn get_mut(&mut self, k: &str) -> Option<&mut Json> {
        self.0.iter_mut().find(|e| e.0 == k).map(|e| &mut e.1)
    }
    /// `dict.setdefault(k, v)`.
    pub fn entry(&mut self, k: &str, v: impl Into<Json>) -> &mut Json {
        if self.get(k).is_none() {
            self.0.push((k.to_string(), v.into()));
        }
        self.get_mut(k).unwrap()
    }
    pub fn remove(&mut self, k: &str) -> Option<Json> {
        let i = self.0.iter().position(|e| e.0 == k)?;
        Some(self.0.remove(i).1)
    }
    pub fn contains(&self, k: &str) -> bool {
        self.get(k).is_some()
    }
}

impl Json {
    pub fn arr(&mut self) -> &mut Vec<Json> {
        match self {
            Json::Arr(a) => a,
            _ => panic!("not a JSON array"),
        }
    }
    pub fn obj(&mut self) -> &mut Obj {
        match self {
            Json::Obj(o) => o,
            _ => panic!("not a JSON object"),
        }
    }
    pub fn as_obj(&self) -> Option<&Obj> {
        match self {
            Json::Obj(o) => Some(o),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(i) => Some(*i),
            _ => None,
        }
    }
    pub fn get(&self, k: &str) -> Option<&Json> {
        self.as_obj().and_then(|o| o.get(k))
    }

    /// `json.dumps(v, separators=(',', ':'))`.
    pub fn compact(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, None, 0, (",", ":"));
        s
    }
    /// `json.dumps(v, indent=n)`.
    pub fn indent(&self, n: usize) -> String {
        let mut s = String::new();
        self.write(&mut s, Some(n), 0, (",", ": "));
        s
    }
    /// `json.dumps(v)` (default separators `', '` / `': '`).
    pub fn plain(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, None, 0, (", ", ": "));
        s
    }

    fn write(&self, s: &mut String, ind: Option<usize>, level: usize, sep: (&str, &str)) {
        let nl = |s: &mut String, level: usize| {
            if let Some(n) = ind {
                s.push('\n');
                for _ in 0..n * level {
                    s.push(' ');
                }
            }
        };
        match self {
            Json::Null => s.push_str("null"),
            Json::Bool(b) => s.push_str(if *b { "true" } else { "false" }),
            Json::Int(i) => s.push_str(&i.to_string()),
            Json::Float(f) => s.push_str(&float_repr(*f)),
            Json::Str(t) => write_str(s, t),
            Json::Arr(a) => {
                if a.is_empty() {
                    s.push_str("[]");
                    return;
                }
                s.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        s.push_str(sep.0);
                    }
                    nl(s, level + 1);
                    v.write(s, ind, level + 1, sep);
                }
                nl(s, level);
                s.push(']');
            }
            Json::Obj(o) => {
                if o.0.is_empty() {
                    s.push_str("{}");
                    return;
                }
                s.push('{');
                for (i, (k, v)) in o.0.iter().enumerate() {
                    if i > 0 {
                        s.push_str(sep.0);
                    }
                    nl(s, level + 1);
                    write_str(s, k);
                    s.push_str(sep.1);
                    v.write(s, ind, level + 1, sep);
                }
                nl(s, level);
                s.push('}');
            }
        }
    }

    /// Parses JSON text (as written by `json.dumps`; for the checks).
    pub fn parse(text: &str) -> Result<Json, String> {
        let b = text.as_bytes();
        let mut p = 0;
        let v = parse_value(b, &mut p)?;
        skip_ws(b, &mut p);
        if p != b.len() {
            return Err(format!("trailing data at {p}"));
        }
        Ok(v)
    }
}

fn write_str(s: &mut String, t: &str) {
    s.push('"');
    for c in t.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            '\u{8}' => s.push_str("\\b"),
            '\u{c}' => s.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7E => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    s.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => s.push(c),
        }
    }
    s.push('"');
}

fn skip_ws(b: &[u8], p: &mut usize) {
    while *p < b.len() && matches!(b[*p], b' ' | b'\n' | b'\r' | b'\t') {
        *p += 1;
    }
}

fn parse_value(b: &[u8], p: &mut usize) -> Result<Json, String> {
    skip_ws(b, p);
    let Some(&c) = b.get(*p) else { return Err("unexpected end".into()) };
    match c {
        b'{' => {
            *p += 1;
            let mut o = Obj::new();
            skip_ws(b, p);
            if b.get(*p) == Some(&b'}') {
                *p += 1;
                return Ok(Json::Obj(o));
            }
            loop {
                skip_ws(b, p);
                let Json::Str(k) = parse_value(b, p)? else { return Err(format!("key expected at {p}")) };
                skip_ws(b, p);
                if b.get(*p) != Some(&b':') {
                    return Err(format!("':' expected at {p}"));
                }
                *p += 1;
                let v = parse_value(b, p)?;
                o.0.push((k, v));
                skip_ws(b, p);
                match b.get(*p) {
                    Some(b',') => *p += 1,
                    Some(b'}') => {
                        *p += 1;
                        return Ok(Json::Obj(o));
                    }
                    _ => return Err(format!("',' or '}}' expected at {p}")),
                }
            }
        }
        b'[' => {
            *p += 1;
            let mut a = Vec::new();
            skip_ws(b, p);
            if b.get(*p) == Some(&b']') {
                *p += 1;
                return Ok(Json::Arr(a));
            }
            loop {
                a.push(parse_value(b, p)?);
                skip_ws(b, p);
                match b.get(*p) {
                    Some(b',') => *p += 1,
                    Some(b']') => {
                        *p += 1;
                        return Ok(Json::Arr(a));
                    }
                    _ => return Err(format!("',' or ']' expected at {p}")),
                }
            }
        }
        b'"' => {
            *p += 1;
            let mut units: Vec<u16> = Vec::new();
            let mut out = String::new();
            let flush = |units: &mut Vec<u16>, out: &mut String| {
                if !units.is_empty() {
                    out.push_str(&String::from_utf16_lossy(units));
                    units.clear();
                }
            };
            loop {
                let Some(&c) = b.get(*p) else { return Err("unterminated string".into()) };
                *p += 1;
                match c {
                    b'"' => break,
                    b'\\' => {
                        let e = *b.get(*p).ok_or("bad escape")?;
                        *p += 1;
                        if e == b'u' {
                            let h = std::str::from_utf8(b.get(*p..*p + 4).ok_or("bad \\u")?).map_err(|e| e.to_string())?;
                            units.push(u16::from_str_radix(h, 16).map_err(|e| e.to_string())?);
                            *p += 4;
                            continue;
                        }
                        flush(&mut units, &mut out);
                        out.push(match e {
                            b'n' => '\n',
                            b'r' => '\r',
                            b't' => '\t',
                            b'b' => '\u{8}',
                            b'f' => '\u{c}',
                            other => other as char,
                        });
                    }
                    _ => {
                        flush(&mut units, &mut out);
                        // copy a whole UTF-8 sequence
                        let start = *p - 1;
                        let mut end = *p;
                        while end < b.len() && (b[end] & 0xC0) == 0x80 {
                            end += 1;
                        }
                        out.push_str(std::str::from_utf8(&b[start..end]).map_err(|e| e.to_string())?);
                        *p = end;
                    }
                }
            }
            flush(&mut units, &mut out);
            Ok(Json::Str(out))
        }
        b't' if b[*p..].starts_with(b"true") => {
            *p += 4;
            Ok(Json::Bool(true))
        }
        b'f' if b[*p..].starts_with(b"false") => {
            *p += 5;
            Ok(Json::Bool(false))
        }
        b'n' if b[*p..].starts_with(b"null") => {
            *p += 4;
            Ok(Json::Null)
        }
        _ => {
            let start = *p;
            while *p < b.len() && matches!(b[*p], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9' | b'I' | b'n' | b'f' | b'i' | b't' | b'y' | b'N' | b'a') {
                *p += 1;
            }
            let t = std::str::from_utf8(&b[start..*p]).unwrap();
            if t.is_empty() {
                return Err(format!("unexpected '{}' at {start}", c as char));
            }
            if !t.contains(['.', 'e', 'E', 'I', 'N'])
                && let Ok(i) = t.parse::<i64>()
            {
                return Ok(Json::Int(i));
            }
            let f = match t {
                "NaN" => f64::NAN,
                "Infinity" => f64::INFINITY,
                "-Infinity" => f64::NEG_INFINITY,
                _ => t.parse::<f64>().map_err(|e| format!("{t}: {e}"))?,
            };
            Ok(Json::Float(f))
        }
    }
}

impl From<bool> for Json {
    fn from(v: bool) -> Json {
        Json::Bool(v)
    }
}
impl From<i64> for Json {
    fn from(v: i64) -> Json {
        Json::Int(v)
    }
}
impl From<i32> for Json {
    fn from(v: i32) -> Json {
        Json::Int(v as i64)
    }
}
impl From<u32> for Json {
    fn from(v: u32) -> Json {
        Json::Int(v as i64)
    }
}
impl From<usize> for Json {
    fn from(v: usize) -> Json {
        Json::Int(v as i64)
    }
}
impl From<u16> for Json {
    fn from(v: u16) -> Json {
        Json::Int(v as i64)
    }
}
impl From<i16> for Json {
    fn from(v: i16) -> Json {
        Json::Int(v as i64)
    }
}
impl From<u8> for Json {
    fn from(v: u8) -> Json {
        Json::Int(v as i64)
    }
}
impl From<f64> for Json {
    fn from(v: f64) -> Json {
        Json::Float(v)
    }
}
impl From<Num> for Json {
    fn from(v: Num) -> Json {
        match v {
            Num::I(i) => Json::Int(i),
            Num::F(f) => Json::Float(f),
        }
    }
}
impl From<&str> for Json {
    fn from(v: &str) -> Json {
        Json::Str(v.to_string())
    }
}
impl From<String> for Json {
    fn from(v: String) -> Json {
        Json::Str(v)
    }
}
impl From<Obj> for Json {
    fn from(v: Obj) -> Json {
        Json::Obj(v)
    }
}
impl<T: Into<Json>> From<Vec<T>> for Json {
    fn from(v: Vec<T>) -> Json {
        Json::Arr(v.into_iter().map(Into::into).collect())
    }
}
impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(v: Option<T>) -> Json {
        v.map_or(Json::Null, Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dumps_like_python() {
        let o = Obj::new().with("a", 1).with("b", vec![1.5, -0.0]).with("c", Obj::new()).with("d", "x·\"y\"").with("e", Vec::<i64>::new());
        let v = Json::Obj(o);
        assert_eq!(v.compact(), "{\"a\":1,\"b\":[1.5,-0.0],\"c\":{},\"d\":\"x\\u00b7\\\"y\\\"\",\"e\":[]}");
        assert_eq!(v.indent(1), "{\n \"a\": 1,\n \"b\": [\n  1.5,\n  -0.0\n ],\n \"c\": {},\n \"d\": \"x\\u00b7\\\"y\\\"\",\n \"e\": []\n}");
        assert_eq!(Json::parse(&v.indent(1)).unwrap(), v);
        assert_eq!(Json::from(vec![1, 2]).indent(0), "[\n1,\n2\n]");
    }
}

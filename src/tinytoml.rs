//! A small reader for the subset of TOML that UFOverlay's language packs and
//! keyboard layouts use — so the files are standard TOML (editor support,
//! translation tools) without shipping a full TOML parser (+214 KB).
//!
//! Supported: comments, `[table]` / `[a.b]` headers, bare / quoted / dotted
//! keys, basic `"…"` (with escapes) and literal `'…'` strings, integers
//! (decimal, 0x, 0o, 0b, `_`), floats, booleans, arrays (multi-line, nested,
//! trailing comma).
//! Not supported (reported as an error): multi-line strings, inline tables,
//! arrays of tables, dates. `cargo test` checks every shipped file against the
//! real `toml` crate, so this reader can't silently drift from the standard.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Array(Vec<Value>),
    Table(Table),
}

pub type Table = BTreeMap<String, Value>;

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }
    #[allow(dead_code)] // part of the reader's API; used by tests
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_table(&self) -> Option<&Table> {
        match self {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }
}

/// `get(&root, "ime.label")`
pub fn get<'a>(t: &'a Table, path: &str) -> Option<&'a Value> {
    let mut cur = t;
    let mut parts = path.split('.').peekable();
    while let Some(p) = parts.next() {
        let v = cur.get(p)?;
        if parts.peek().is_none() {
            return Some(v);
        }
        cur = v.as_table()?;
    }
    None
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
    line: usize,
}

type R<T> = Result<T, String>;

impl<'a> P<'a> {
    fn err<T>(&self, msg: &str) -> R<T> {
        Err(format!("line {}: {msg}", self.line))
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn peek_at(&self, k: usize) -> Option<u8> {
        self.s.get(self.i + k).copied()
    }
    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        if c == b'\n' {
            self.line += 1;
        }
        Some(c)
    }
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.i += 1;
        }
    }
    fn skip_comment(&mut self) {
        if self.peek() == Some(b'#') {
            while !matches!(self.peek(), None | Some(b'\n')) {
                self.i += 1;
            }
        }
    }
    /// Whitespace, newlines and comments (inside arrays / between items).
    fn skip_all(&mut self) {
        loop {
            self.skip_ws();
            self.skip_comment();
            match self.peek() {
                Some(b'\n') | Some(b'\r') => {
                    self.bump();
                }
                _ => return,
            }
        }
    }
    fn end_of_line(&mut self) -> R<()> {
        self.skip_ws();
        self.skip_comment();
        if self.peek() == Some(b'\r') {
            self.i += 1;
        }
        match self.peek() {
            None => Ok(()),
            Some(b'\n') => {
                self.bump();
                Ok(())
            }
            _ => self.err("unexpected text after value"),
        }
    }

    fn key_part(&mut self) -> R<String> {
        match self.peek() {
            Some(b'"') => self.basic_string(),
            Some(b'\'') => self.literal_string(),
            _ => {
                let start = self.i;
                while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
                    self.i += 1;
                }
                if start == self.i {
                    return self.err("expected a key");
                }
                Ok(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
            }
        }
    }

    fn key_path(&mut self) -> R<Vec<String>> {
        let mut path = vec![self.key_part()?];
        loop {
            self.skip_ws();
            if self.peek() != Some(b'.') {
                return Ok(path);
            }
            self.i += 1;
            self.skip_ws();
            path.push(self.key_part()?);
        }
    }

    fn basic_string(&mut self) -> R<String> {
        if self.peek_at(1) == Some(b'"') && self.peek_at(2) == Some(b'"') {
            return self.err("multi-line strings are not supported");
        }
        self.i += 1; // opening quote
        let mut out: Vec<u8> = Vec::new();
        loop {
            match self.peek() {
                None | Some(b'\n') => return self.err("unterminated string"),
                Some(b'"') => {
                    self.i += 1;
                    return String::from_utf8(out).or_else(|_| self.err("invalid UTF-8"));
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = self.bump().ok_or_else(|| format!("line {}: bad escape", self.line))?;
                    match c {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' | b'U' => {
                            let n = if c == b'u' { 4 } else { 8 };
                            let hex = std::str::from_utf8(self.s.get(self.i..self.i + n).unwrap_or_default()).unwrap_or("");
                            let ch = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
                            let Some(ch) = ch else { return self.err("bad \\u escape") };
                            self.i += n;
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return self.err("unknown escape"),
                    }
                }
                Some(c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    fn literal_string(&mut self) -> R<String> {
        if self.peek_at(1) == Some(b'\'') && self.peek_at(2) == Some(b'\'') {
            return self.err("multi-line strings are not supported");
        }
        self.i += 1;
        let start = self.i;
        loop {
            match self.peek() {
                None | Some(b'\n') => return self.err("unterminated string"),
                Some(b'\'') => {
                    let s = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
                    self.i += 1;
                    return Ok(s);
                }
                _ => self.i += 1,
            }
        }
    }

    fn number_or_bool(&mut self) -> R<Value> {
        let start = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'+' | b'-' | b'.')) {
            self.i += 1;
        }
        let raw = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
        match raw.as_str() {
            "true" => return Ok(Value::Bool(true)),
            "false" => return Ok(Value::Bool(false)),
            "" => return self.err("expected a value"),
            _ => {}
        }
        let t = raw.replace('_', "");
        let radix = |p: &str, r: u32| i64::from_str_radix(&t[p.len()..], r).ok().map(Value::Int);
        let v = if t.starts_with("0x") {
            radix("0x", 16)
        } else if t.starts_with("0o") {
            radix("0o", 8)
        } else if t.starts_with("0b") {
            radix("0b", 2)
        } else if t.contains(['.', 'e', 'E']) {
            t.parse::<f64>().ok().map(Value::Float)
        } else {
            t.parse::<i64>().ok().map(Value::Int)
        };
        match v {
            Some(v) => Ok(v),
            None => self.err(&format!("unsupported value {raw:?}")),
        }
    }

    fn value(&mut self) -> R<Value> {
        match self.peek() {
            Some(b'"') => self.basic_string().map(Value::Str),
            Some(b'\'') => self.literal_string().map(Value::Str),
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_all();
                    if self.peek() == Some(b']') {
                        self.i += 1;
                        return Ok(Value::Array(items));
                    }
                    items.push(self.value()?);
                    self.skip_all();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {}
                        _ => return self.err("expected , or ] in array"),
                    }
                }
            }
            Some(b'{') => self.err("inline tables are not supported"),
            _ => self.number_or_bool(),
        }
    }
}

fn table_at<'a>(root: &'a mut Table, path: &[String], line: usize) -> R<&'a mut Table> {
    let mut cur = root;
    for p in path {
        let e = cur.entry(p.clone()).or_insert_with(|| Value::Table(Table::new()));
        cur = match e {
            Value::Table(t) => t,
            _ => return Err(format!("line {line}: {p:?} is not a table")),
        };
    }
    Ok(cur)
}

pub fn parse(src: &str) -> R<Table> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let mut p = P { s: src.as_bytes(), i: 0, line: 1 };
    let mut root = Table::new();
    let mut current: Vec<String> = Vec::new();
    loop {
        p.skip_all();
        let Some(c) = p.peek() else { return Ok(root) };
        if c == b'[' {
            if p.peek_at(1) == Some(b'[') {
                return p.err("arrays of tables are not supported");
            }
            p.i += 1;
            p.skip_ws();
            current = p.key_path()?;
            p.skip_ws();
            if p.peek() != Some(b']') {
                return p.err("expected ]");
            }
            p.i += 1;
            table_at(&mut root, &current, p.line)?;
            p.end_of_line()?;
            continue;
        }
        let mut path = p.key_path()?;
        p.skip_ws();
        if p.peek() != Some(b'=') {
            return p.err("expected =");
        }
        p.i += 1;
        p.skip_ws();
        let v = p.value()?;
        let line = p.line;
        p.end_of_line()?;
        let leaf = path.pop().unwrap();
        let mut full = current.clone();
        full.extend(path);
        let t = table_at(&mut root, &full, line)?;
        if t.contains_key(&leaf) {
            return Err(format!("line {line}: duplicate key {leaf:?}"));
        }
        t.insert(leaf, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subset_parses() {
        let t = parse(
            "# c\nname = \"한국어\" # trailing\nlcid = [0x12, 1_0]\n[ime]\nlabel = '한/영'\nbig = true\n\
             esc = \"a\\\"b\\u00e9\"\n[a.b]\nrows = [\n  \"x\", # one\n  \"y\",\n]\nf = 1.5\n",
        )
        .unwrap();
        assert_eq!(get(&t, "name").and_then(Value::as_str), Some("한국어"));
        assert_eq!(get(&t, "lcid"), Some(&Value::Array(vec![Value::Int(0x12), Value::Int(10)])));
        assert_eq!(get(&t, "ime.label").and_then(Value::as_str), Some("한/영"));
        assert_eq!(get(&t, "ime.esc").and_then(Value::as_str), Some("a\"bé"));
        assert_eq!(get(&t, "a.b.rows").and_then(Value::as_array).map(|a| a.len()), Some(2));
        assert_eq!(get(&t, "a.b.f").and_then(Value::as_float), Some(1.5));
    }

    #[test]
    fn unsupported_and_broken_input_is_an_error_with_a_line() {
        assert!(parse("a = {x = 1}").unwrap_err().contains("inline tables"));
        assert!(parse("a = \"\"\"x\"\"\"").unwrap_err().contains("multi-line"));
        assert!(parse("[[t]]").unwrap_err().contains("arrays of tables"));
        assert!(parse("a = 1\na = 2").unwrap_err().contains("line 2"));
        assert!(parse("a = \"open").is_err());
    }

    fn from_real(v: toml::Value) -> Value {
        match v {
            toml::Value::String(s) => Value::Str(s),
            toml::Value::Integer(i) => Value::Int(i),
            toml::Value::Float(f) => Value::Float(f),
            toml::Value::Boolean(b) => Value::Bool(b),
            toml::Value::Array(a) => Value::Array(a.into_iter().map(from_real).collect()),
            toml::Value::Table(t) => Value::Table(t.into_iter().map(|(k, v)| (k, from_real(v))).collect()),
            toml::Value::Datetime(_) => panic!("dates are not used"),
        }
    }

    /// Every shipped file must be valid standard TOML *and* read identically
    /// by this reader and by the real `toml` crate (dev-dependency only).
    #[test]
    fn shipped_files_match_the_real_toml_parser() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("i18n");
        let mut n = 0;
        for sub in ["lang", "keyboards"] {
            for e in std::fs::read_dir(root.join(sub)).unwrap().flatten() {
                if e.path().extension().and_then(|x| x.to_str()) != Some("toml") {
                    continue;
                }
                let txt = std::fs::read_to_string(e.path()).unwrap();
                let real: toml::Table = toml::from_str(&txt).unwrap_or_else(|err| panic!("{:?} is not valid TOML: {err}", e.path()));
                let real: Table = real.into_iter().map(|(k, v)| (k, from_real(v))).collect();
                let ours = parse(&txt).unwrap_or_else(|err| panic!("{:?}: {err}", e.path()));
                assert_eq!(ours, real, "{:?} reads differently", e.path());
                n += 1;
            }
        }
        assert!(n >= 6, "expected the built-in packs and layouts, found {n}");
    }
}

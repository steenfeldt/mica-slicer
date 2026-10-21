// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Small JSON codec for project metadata. Geometry never passes through this tree.
use super::{ThreeMfError, invalid};
use std::{collections::BTreeMap, io::Write};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Json {
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
    Atom(String),
}
impl From<&str> for Json {
    fn from(s: &str) -> Self {
        Self::String(s.into())
    }
}
impl From<String> for Json {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}
impl Json {
    pub fn object(items: impl IntoIterator<Item = (impl Into<String>, Json)>) -> Self {
        Self::Object(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
    pub fn number(n: impl ToString) -> Self {
        Self::Atom(n.to_string())
    }
    pub fn get(&self, k: &str) -> Result<&Self, ThreeMfError> {
        self.map()?
            .get(k)
            .ok_or_else(|| invalid(format!("missing JSON field {k}")))
    }
    pub fn map(&self) -> Result<&BTreeMap<String, Self>, ThreeMfError> {
        if let Self::Object(v) = self {
            Ok(v)
        } else {
            Err(invalid("expected JSON object"))
        }
    }
    pub fn array(&self) -> Result<&[Self], ThreeMfError> {
        if let Self::Array(v) = self {
            Ok(v)
        } else {
            Err(invalid("expected JSON array"))
        }
    }
    pub fn text(&self) -> Result<&str, ThreeMfError> {
        if let Self::String(v) | Self::Atom(v) = self {
            Ok(v)
        } else {
            Err(invalid("expected JSON scalar"))
        }
    }
    pub fn num<T: std::str::FromStr>(&self) -> Result<T, ThreeMfError> {
        self.text()?
            .parse()
            .map_err(|_| invalid("invalid JSON number"))
    }
    pub fn write(&self, w: &mut impl Write) -> std::io::Result<()> {
        match self {
            Self::String(s) => string(w, s),
            Self::Atom(s) => w.write_all(s.as_bytes()),
            Self::Array(a) => {
                w.write_all(b"[")?;
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        w.write_all(b",")?;
                    }
                    v.write(w)?;
                }
                w.write_all(b"]")
            }
            Self::Object(a) => {
                w.write_all(b"{")?;
                for (i, (k, v)) in a.iter().enumerate() {
                    if i > 0 {
                        w.write_all(b",")?;
                    }
                    string(w, k)?;
                    w.write_all(b":")?;
                    v.write(w)?;
                }
                w.write_all(b"}")
            }
        }
    }
    pub fn encode(&self) -> String {
        let mut bytes = Vec::new();
        self.write(&mut bytes).expect("Vec write");
        String::from_utf8(bytes).expect("JSON is UTF-8")
    }
}
fn string(w: &mut impl Write, s: &str) -> std::io::Result<()> {
    w.write_all(b"\"")?;
    for c in s.chars() {
        match c {
            '"' => w.write_all(b"\\\"")?,
            '\\' => w.write_all(b"\\\\")?,
            '\n' => w.write_all(b"\\n")?,
            '\r' => w.write_all(b"\\r")?,
            '\t' => w.write_all(b"\\t")?,
            c if c < ' ' => write!(w, "\\u{:04x}", c as u32)?,
            c => w.write_all(c.encode_utf8(&mut [0; 4]).as_bytes())?,
        }
    }
    w.write_all(b"\"")
}
pub(crate) fn parse(text: &str) -> Result<Json, ThreeMfError> {
    let mut p = Parser(text);
    let value = p.value(0)?;
    if !p.0.trim().is_empty() {
        return Err(invalid("trailing JSON"));
    }
    Ok(value)
}
struct Parser<'a>(&'a str);
impl Parser<'_> {
    fn take(&mut self, s: &str) -> bool {
        self.0 = self.0.trim_start_matches([' ', '\r', '\n', '\t']);
        if let Some(rest) = self.0.strip_prefix(s) {
            self.0 = rest;
            true
        } else {
            false
        }
    }
    fn string(&mut self) -> Result<String, ThreeMfError> {
        if !self.take("\"") {
            return Err(invalid("expected JSON string"));
        }
        let mut out = String::new();
        loop {
            let c = self
                .0
                .chars()
                .next()
                .ok_or_else(|| invalid("truncated JSON string"))?;
            self.0 = &self.0[c.len_utf8()..];
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let e = self
                        .0
                        .chars()
                        .next()
                        .ok_or_else(|| invalid("truncated JSON escape"))?;
                    self.0 = &self.0[e.len_utf8()..];
                    out.push(match e {
                        '"' | '\\' | '/' => e,
                        'b' => '\x08',
                        'f' => '\x0c',
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        'u' => {
                            let mut n = self.hex()?;
                            if (0xd800..=0xdbff).contains(&n) {
                                if !self.0.starts_with("\\u") {
                                    return Err(invalid("missing JSON surrogate"));
                                }
                                self.0 = &self.0[2..];
                                let lo = self.hex()?;
                                if !(0xdc00..=0xdfff).contains(&lo) {
                                    return Err(invalid("invalid JSON surrogate"));
                                }
                                n = 0x10000 + ((n - 0xd800) << 10) + (lo - 0xdc00);
                            }
                            char::from_u32(n).ok_or_else(|| invalid("invalid JSON unicode"))?
                        }
                        _ => return Err(invalid("invalid JSON escape")),
                    });
                }
                c if c < ' ' => return Err(invalid("JSON control character")),
                c => out.push(c),
            }
        }
    }
    fn hex(&mut self) -> Result<u32, ThreeMfError> {
        let s = self
            .0
            .get(..4)
            .ok_or_else(|| invalid("truncated JSON unicode"))?;
        if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid("invalid JSON unicode"));
        }
        let n = u32::from_str_radix(s, 16).map_err(|_| invalid("invalid JSON unicode"))?;
        self.0 = &self.0[4..];
        Ok(n)
    }
    fn value(&mut self, depth: usize) -> Result<Json, ThreeMfError> {
        if depth > 64 {
            return Err(invalid("JSON nesting exceeds 64"));
        }
        self.0 = self.0.trim_start_matches([' ', '\r', '\n', '\t']);
        if self.0.starts_with('"') {
            return Ok(Json::String(self.string()?));
        }
        if self.take("{") {
            let mut map = BTreeMap::new();
            if !self.take("}") {
                loop {
                    let key = self.string()?;
                    if !self.take(":") {
                        return Err(invalid("missing JSON colon"));
                    }
                    if map.insert(key, self.value(depth + 1)?).is_some() {
                        return Err(invalid("duplicate JSON key"));
                    }
                    if self.take("}") {
                        break;
                    }
                    if !self.take(",") {
                        return Err(invalid("missing JSON comma"));
                    }
                }
            }
            return Ok(Json::Object(map));
        }
        if self.take("[") {
            let mut a = Vec::new();
            if !self.take("]") {
                loop {
                    a.push(self.value(depth + 1)?);
                    if self.take("]") {
                        break;
                    }
                    if !self.take(",") {
                        return Err(invalid("missing JSON comma"));
                    }
                }
            }
            return Ok(Json::Array(a));
        }
        let end = self
            .0
            .find(|c: char| c == ',' || c == ']' || c == '}' || c.is_ascii_whitespace())
            .unwrap_or(self.0.len());
        let atom = &self.0[..end];
        let valid_number = {
            let mut s = atom.strip_prefix('-').unwrap_or(atom);
            if let Some(rest) = s.strip_prefix('0') {
                s = rest;
            } else {
                let n = s.bytes().take_while(u8::is_ascii_digit).count();
                if n == 0 {
                    s = "invalid";
                } else {
                    s = &s[n..];
                }
            }
            if let Some(rest) = s.strip_prefix('.') {
                let n = rest.bytes().take_while(u8::is_ascii_digit).count();
                s = if n == 0 { "invalid" } else { &rest[n..] };
            }
            if s.starts_with(['e', 'E']) {
                let rest = &s[1..];
                let rest = rest.strip_prefix(['+', '-']).unwrap_or(rest);
                let n = rest.bytes().take_while(u8::is_ascii_digit).count();
                s = if n == 0 { "invalid" } else { &rest[n..] };
            }
            s.is_empty() && !atom.is_empty() && atom != "-"
        };
        if !matches!(atom, "true" | "false" | "null") && !valid_number {
            return Err(invalid("invalid JSON value"));
        }
        self.0 = &self.0[end..];
        Ok(Json::Atom(atom.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strings_and_strict_syntax() {
        let j = parse(r#"{"a":["\ud83d\ude00\n\"\\",-1.2e+4,true,null]}"#).unwrap();
        assert_eq!(parse(&j.encode()).unwrap(), j);
        for s in [
            "[1,]",
            "{\"x\":1,\"x\":2}",
            "01",
            "+1",
            "1.",
            "\"\\ud800\"",
            "[]x",
            "1e+-2",
            "\"\\u+123\"",
        ] {
            assert!(parse(s).is_err(), "{s}");
        }
    }
}

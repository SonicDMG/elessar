//! Parsing, conditions, transforms and fragments (spec §2). Phase 1b.
//!
//! Built against `golden/records/<pack>/<source>.json` (per-source records
//! and skipped rows dumped from the Python engine, which must match exactly).
//! Exactness is why the JSON and CSV readers here are hand-written rather
//! than `serde_json`/`csv`: skip reasons quote CPython's scanner messages and
//! columns, excerpts re-serialise floats, and pointers follow `csv.reader`'s
//! `line_num`. The CPython behaviour they follow lives in [`py`].

use crate::pack::{Mapping, Pack, Source};
use crate::store::{get_path, present, Excerpt, Record, Skipped, Table};
use crate::time;
use indexmap::IndexMap;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use serde_json::{Map, Number, Value};

// ---------------------------------------------------------------- Python semantics

/// The CPython behaviour the engine's output depends on: `str.strip`,
/// `str.splitlines`, `str()`, `float()`, `int()`, `json.loads`, `json.dumps`.
pub mod py {
    use serde_json::{Map, Number, Value};
    use std::borrow::Cow;
    use std::fmt::Write;
    use std::ops::Range;

    /// `str.isspace`, which unlike `char::is_whitespace` includes `\x1c`–`\x1f`.
    pub fn is_space(c: char) -> bool {
        matches!(c, '\t'..='\r' | '\x1c'..='\x1f' | ' ' | '\u{85}' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
            | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
    }

    pub fn strip(s: &str) -> &str {
        s.trim_matches(is_space)
    }

    /// `str.splitlines()` as byte ranges (no line ends, no empty last line).
    pub fn splitlines(text: &str) -> Vec<Range<usize>> {
        let b = text.as_bytes();
        let mut out = Vec::new();
        let (mut start, mut i) = (0, 0);
        while i < b.len() {
            let brk = match b[i] {
                b'\r' if b.get(i + 1) == Some(&b'\n') => 2,
                b'\n' | b'\r' | 0x0b | 0x0c | 0x1c | 0x1d | 0x1e => 1,
                0xc2 if b.get(i + 1) == Some(&0x85) => 2,
                0xe2 if b.get(i + 1) == Some(&0x80) && matches!(b.get(i + 2), Some(0xa8 | 0xa9)) => 3,
                _ => 0,
            };
            if brk > 0 {
                out.push(start..i);
                i += brk;
                start = i;
            } else {
                i += 1;
            }
        }
        if start < b.len() {
            out.push(start..b.len());
        }
        out
    }

    pub fn truthy(v: &Value) -> bool {
        match v {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64() != Some(0.0),
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
        }
    }

    pub fn type_name(v: &Value) -> &'static str {
        match v {
            Value::Null => "NoneType",
            Value::Bool(_) => "bool",
            Value::Number(n) if n.is_f64() => "float",
            Value::Number(_) => "int",
            Value::String(_) => "str",
            Value::Array(_) => "list",
            Value::Object(_) => "dict",
        }
    }

    /// `repr(float)`: shortest round-trip digits, exponent form outside `1e-4 <= |x| < 1e16`.
    pub fn float_repr(f: f64) -> String {
        if f.is_nan() {
            return "nan".into();
        }
        if f.is_infinite() {
            return if f > 0.0 { "inf".into() } else { "-inf".into() };
        }
        let sci = format!("{f:e}");
        let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
        let (sign, mantissa) = match mantissa.strip_prefix('-') {
            Some(m) => ("-", m),
            None => ("", mantissa),
        };
        let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
        let exp: i32 = exp.parse().unwrap_or(0);
        let decpt = exp + 1;
        if decpt <= -4 || decpt > 16 {
            let (first, rest) = digits.split_at(1);
            let dot = if rest.is_empty() { "" } else { "." };
            let (esign, eabs) = if exp < 0 { ('-', -exp) } else { ('+', exp) };
            format!("{sign}{first}{dot}{rest}e{esign}{eabs:02}")
        } else if decpt <= 0 {
            format!("{sign}0.{}{digits}", "0".repeat(-decpt as usize))
        } else if decpt as usize >= digits.len() {
            format!("{sign}{digits}{}.0", "0".repeat(decpt as usize - digits.len()))
        } else {
            let (whole, frac) = digits.split_at(decpt as usize);
            format!("{sign}{whole}.{frac}")
        }
    }

    fn number_repr(n: &Number) -> String {
        match n.as_f64() {
            Some(f) if n.is_f64() => float_repr(f),
            _ => n.to_string(),
        }
    }

    /// `repr(str)`. The printable test is approximate outside ASCII and the
    /// common format/space characters; it only shows in `str()` of containers.
    pub fn repr_str(s: &str) -> String {
        let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
        let mut out = String::with_capacity(s.len() + 2);
        out.push(quote);
        for c in s.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c == quote => {
                    out.push('\\');
                    out.push(c);
                }
                c if (c as u32) < 0x20 || ('\u{7f}'..='\u{a0}').contains(&c) || c == '\u{ad}' => {
                    let _ = write!(out, "\\x{:02x}", c as u32);
                }
                c if matches!(c, '\u{1680}' | '\u{2000}'..='\u{200f}' | '\u{2028}'..='\u{202f}' | '\u{205f}'..='\u{206f}' | '\u{3000}' | '\u{feff}') => {
                    let _ = write!(out, "\\u{:04x}", c as u32);
                }
                c => out.push(c),
            }
        }
        out.push(quote);
        out
    }

    fn write_repr(v: &Value, out: &mut String) {
        match v {
            Value::String(s) => out.push_str(&repr_str(s)),
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write_repr(x, out);
                }
                out.push(']');
            }
            Value::Object(o) => {
                out.push('{');
                for (i, (k, x)) in o.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&repr_str(k));
                    out.push_str(": ");
                    write_repr(x, out);
                }
                out.push('}');
            }
            other => out.push_str(&str_of(other)),
        }
    }

    /// `str(v)`. Differs from `store::string_form` on integral floats, which
    /// Python prints as `1.0`; conditions compare this form.
    pub fn str_of(v: &Value) -> Cow<'_, str> {
        match v {
            Value::String(s) => Cow::Borrowed(s),
            Value::Null => Cow::Borrowed("None"),
            Value::Bool(true) => Cow::Borrowed("True"),
            Value::Bool(false) => Cow::Borrowed("False"),
            Value::Number(n) => Cow::Owned(number_repr(n)),
            container => {
                let mut out = String::new();
                write_repr(container, &mut out);
                Cow::Owned(out)
            }
        }
    }

    /// One or more digits with single underscores between them; the end index.
    fn digitpart(b: &[u8], mut i: usize) -> Option<usize> {
        if !b.get(i)?.is_ascii_digit() {
            return None;
        }
        i += 1;
        loop {
            match b.get(i) {
                Some(c) if c.is_ascii_digit() => i += 1,
                Some(b'_') if b.get(i + 1).is_some_and(|c| c.is_ascii_digit()) => i += 2,
                _ => return Some(i),
            }
        }
    }

    /// `float(str)` grammar: signs, `inf`/`nan` in any case, underscores between digits.
    pub fn parse_float(s: &str) -> Option<f64> {
        let t = strip(s);
        let b = t.as_bytes();
        let i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
        let word = t[i..].to_ascii_lowercase();
        if word == "inf" || word == "infinity" {
            return Some(if b[0] == b'-' { f64::NEG_INFINITY } else { f64::INFINITY });
        }
        if word == "nan" {
            return Some(f64::NAN);
        }
        let mut j = digitpart(b, i);
        let has_int = j.is_some();
        let mut end = j.unwrap_or(i);
        if b.get(end) == Some(&b'.') {
            j = digitpart(b, end + 1);
            match j {
                Some(e) => end = e,
                None if has_int => end += 1,
                None => return None,
            }
        } else if !has_int {
            return None;
        }
        if matches!(b.get(end), Some(b'e' | b'E')) {
            let k = end + 1 + usize::from(matches!(b.get(end + 1), Some(b'+' | b'-')));
            end = digitpart(b, k)?;
        }
        if end != b.len() {
            return None;
        }
        if t.contains('_') {
            t.replace('_', "").parse().ok()
        } else {
            t.parse().ok()
        }
    }

    /// `int(str)` in base 10: surrounding space, a sign, underscores between digits.
    pub fn parse_int(s: &str) -> Option<i128> {
        let t = strip(s);
        let b = t.as_bytes();
        let i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
        if digitpart(b, i)? != b.len() {
            return None;
        }
        if t.contains('_') {
            t.replace('_', "").parse().ok()
        } else {
            t.parse().ok()
        }
    }

    /// `float(v)`.
    pub fn to_float(v: &Value) -> Result<f64, String> {
        match v {
            Value::Number(n) => Ok(n.as_f64().unwrap_or(f64::NAN)),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::String(s) => parse_float(s).ok_or_else(|| format!("could not convert string to float: {}", repr_str(s))),
            other => Err(format!("float() argument must be a string or a number, not '{}'", type_name(other))),
        }
    }

    /// `int(v)`: floats truncate, strings must be integer literals.
    pub fn to_int(v: &Value) -> Result<i128, String> {
        match v {
            Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
                (Some(i), _, _) => Ok(i as i128),
                (_, Some(u), _) => Ok(u as i128),
                (_, _, Some(f)) => Ok(f as i128),
                _ => Err("cannot convert float NaN to integer".into()),
            },
            Value::Bool(b) => Ok(*b as i128),
            Value::String(s) => parse_int(s).ok_or_else(|| format!("invalid literal for int() with base 10: {}", repr_str(s))),
            other => Err(format!("int() argument must be a string, a bytes-like object or a number, not '{}'", type_name(other))),
        }
    }

    /// `dd/mm/yyyy` (separators `/`, `.`, `-`) to `yyyy-mm-dd`; the year is kept as written.
    pub fn date_dmy(v: &str) -> Result<String, String> {
        let parts: Vec<&str> = v.split(['/', '.', '-']).collect();
        let [d, m, y] = parts[..] else {
            return Err(if parts.len() < 3 {
                format!("not enough values to unpack (expected 3, got {})", parts.len())
            } else {
                "too many values to unpack (expected 3)".to_string()
            });
        };
        let int = |s: &str| parse_int(s).ok_or_else(|| format!("invalid literal for int() with base 10: {}", repr_str(s)));
        let m = int(m)?;
        let d = int(d)?;
        Ok(format!("{y}-{m:02}-{d:02}"))
    }

    // ------------------------------------------------------------ json.loads

    /// A `json.JSONDecodeError`: `msg` and `pos` as byte offset into the text.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct JsonError {
        pub msg: &'static str,
        pub pos: usize,
    }

    /// Python raises `RecursionError` here, which is not a `ValueError`: the
    /// payload fails as a whole even in line-oriented formats.
    pub const TOO_DEEP: &str = "maximum recursion depth exceeded";
    const MAX_DEPTH: usize = 200;

    impl JsonError {
        /// `e.pos + 1`: a 1-based column in characters, for a one-line document.
        pub fn column(&self, text: &str) -> usize {
            text[..self.pos].chars().count() + 1
        }

        /// `str(e)`: `msg: line L column C (char P)`.
        pub fn describe(&self, text: &str) -> String {
            let before = &text[..self.pos];
            let line = before.matches('\n').count() + 1;
            let col = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
            format!("{}: line {line} column {col} (char {})", self.msg, before.chars().count())
        }
    }

    /// `json.loads(text)`. Accepts what Python accepts (`NaN`, `Infinity`,
    /// duplicate keys: first position, last value). Floats parse exactly,
    /// which `serde_json` does not without `float_roundtrip`. What a
    /// `serde_json::Value` cannot hold: non-finite numbers become null, ints
    /// beyond `u64` become floats, lone surrogates become U+FFFD.
    pub fn json_loads(text: &str) -> Result<Value, JsonError> {
        if text.starts_with('\u{feff}') {
            return Err(JsonError { msg: "Unexpected UTF-8 BOM (decode using utf-8-sig)", pos: 0 });
        }
        let scan = Scanner { text, b: text.as_bytes() };
        let (v, end) = scan.value(scan.ws(0), 0)?;
        let end = scan.ws(end);
        if end != text.len() {
            return Err(JsonError { msg: "Extra data", pos: end });
        }
        Ok(v)
    }

    struct Scanner<'a> {
        text: &'a str,
        b: &'a [u8],
    }

    fn err<T>(msg: &'static str, pos: usize) -> Result<T, JsonError> {
        Err(JsonError { msg, pos })
    }

    impl Scanner<'_> {
        fn ws(&self, mut i: usize) -> usize {
            while matches!(self.b.get(i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
                i += 1;
            }
            i
        }

        fn value(&self, i: usize, depth: usize) -> Result<(Value, usize), JsonError> {
            let rest = &self.b[i.min(self.b.len())..];
            match rest.first() {
                None => err("Expecting value", i),
                Some(b'"') => {
                    let (s, end) = self.string(i + 1)?;
                    Ok((Value::String(s), end))
                }
                Some(b'{') => self.object(i + 1, depth + 1),
                Some(b'[') => self.array(i + 1, depth + 1),
                _ if rest.starts_with(b"null") => Ok((Value::Null, i + 4)),
                _ if rest.starts_with(b"true") => Ok((Value::Bool(true), i + 4)),
                _ if rest.starts_with(b"false") => Ok((Value::Bool(false), i + 5)),
                _ if rest.starts_with(b"NaN") => Ok((Value::Null, i + 3)),
                _ if rest.starts_with(b"Infinity") => Ok((Value::Null, i + 8)),
                _ if rest.starts_with(b"-Infinity") => Ok((Value::Null, i + 9)),
                _ => self.number(i),
            }
        }

        fn number(&self, start: usize) -> Result<(Value, usize), JsonError> {
            let b = self.b;
            let digit = |i: usize| b.get(i).is_some_and(|c| c.is_ascii_digit());
            let mut i = start + usize::from(b[start] == b'-');
            match b.get(i) {
                Some(b'1'..=b'9') => {
                    while digit(i) {
                        i += 1;
                    }
                }
                Some(b'0') => i += 1,
                _ => return err("Expecting value", start),
            }
            let mut is_float = false;
            if b.get(i) == Some(&b'.') && digit(i + 1) {
                is_float = true;
                i += 2;
                while digit(i) {
                    i += 1;
                }
            }
            if matches!(b.get(i), Some(b'e' | b'E')) {
                let mut j = i + 1;
                if matches!(b.get(j), Some(b'-' | b'+')) {
                    j += 1;
                }
                if digit(j) {
                    is_float = true;
                    i = j;
                    while digit(i) {
                        i += 1;
                    }
                }
            }
            let tok = &self.text[start..i];
            let value = if is_float {
                float_value(tok.parse().unwrap_or(f64::NAN))
            } else if let Ok(n) = tok.parse::<i64>() {
                Value::Number(n.into())
            } else if let Ok(n) = tok.parse::<u64>() {
                Value::Number(n.into())
            } else {
                float_value(tok.parse().unwrap_or(f64::NAN))
            };
            Ok((value, i))
        }

        fn array(&self, start: usize, depth: usize) -> Result<(Value, usize), JsonError> {
            if depth > MAX_DEPTH {
                return err(TOO_DEEP, start);
            }
            let mut out = Vec::new();
            let mut i = self.ws(start);
            if self.b.get(i) == Some(&b']') {
                return Ok((Value::Array(out), i + 1));
            }
            loop {
                let (v, end) = self.value(i, depth)?;
                out.push(v);
                i = self.ws(end);
                match self.b.get(i) {
                    Some(b']') => return Ok((Value::Array(out), i + 1)),
                    Some(b',') => i = self.ws(i + 1),
                    _ => return err("Expecting ',' delimiter", i),
                }
            }
        }

        fn object(&self, start: usize, depth: usize) -> Result<(Value, usize), JsonError> {
            if depth > MAX_DEPTH {
                return err(TOO_DEEP, start);
            }
            let mut out = Map::new();
            let mut i = self.ws(start);
            if self.b.get(i) == Some(&b'}') {
                return Ok((Value::Object(out), i + 1));
            }
            loop {
                if self.b.get(i) != Some(&b'"') {
                    return err("Expecting property name enclosed in double quotes", i);
                }
                let (key, end) = self.string(i + 1)?;
                i = self.ws(end);
                if self.b.get(i) != Some(&b':') {
                    return err("Expecting ':' delimiter", i);
                }
                i = self.ws(i + 1);
                let (v, end) = self.value(i, depth)?;
                out.insert(key, v);
                i = self.ws(end);
                match self.b.get(i) {
                    Some(b'}') => return Ok((Value::Object(out), i + 1)),
                    Some(b',') => i = self.ws(i + 1),
                    _ => return err("Expecting ',' delimiter", i),
                }
            }
        }

        fn hex4(&self, start: usize) -> Option<u32> {
            let mut c = 0;
            for i in start..start + 4 {
                c = c * 16 + (*self.b.get(i)? as char).to_digit(16)?;
            }
            Some(c)
        }

        /// The body of a string whose opening quote is at `start - 1`; returns the end after the closing quote.
        fn string(&self, start: usize) -> Result<(String, usize), JsonError> {
            let begin = start - 1;
            let mut out = String::new();
            let mut i = start;
            loop {
                let mut j = i;
                loop {
                    match self.b.get(j) {
                        None => return err("Unterminated string starting at", begin),
                        Some(b'"' | b'\\') => break,
                        Some(c) if *c <= 0x1f => return err("Invalid control character at", j),
                        _ => j += 1,
                    }
                }
                out.push_str(&self.text[i..j]);
                if self.b[j] == b'"' {
                    return Ok((out, j + 1));
                }
                let esc = j + 1;
                let c = match self.b.get(esc) {
                    None => return err("Unterminated string starting at", begin),
                    Some(b'"') => '"',
                    Some(b'\\') => '\\',
                    Some(b'/') => '/',
                    Some(b'b') => '\u{8}',
                    Some(b'f') => '\u{c}',
                    Some(b'n') => '\n',
                    Some(b'r') => '\r',
                    Some(b't') => '\t',
                    Some(b'u') => {
                        let (c, end) = self.unicode_escape(esc)?;
                        out.push(c);
                        i = end;
                        continue;
                    }
                    Some(_) => return err("Invalid \\escape", j),
                };
                out.push(c);
                i = esc + 1;
            }
        }

        /// `\uXXXX` with its `u` at `u_pos`, joining a surrogate pair; the end index.
        fn unicode_escape(&self, u_pos: usize) -> Result<(char, usize), JsonError> {
            let hex = u_pos + 1;
            let end = hex + 4;
            // CPython requires one more character after the digits (`end >= len` fails).
            let Some(c) = self.hex4(hex).filter(|_| end < self.b.len()) else {
                return err("Invalid \\uXXXX escape", u_pos);
            };
            if (0xd800..0xdc00).contains(&c)
                && self.text[end..].chars().nth(6).is_some()
                && self.b[end] == b'\\'
                && self.b[end + 1] == b'u'
            {
                let Some(c2) = self.hex4(end + 2) else {
                    return err("Invalid \\uXXXX escape", end + 1);
                };
                if (0xdc00..0xe000).contains(&c2) {
                    let joined = 0x10000 + ((c - 0xd800) << 10) + (c2 - 0xdc00);
                    return Ok((char::from_u32(joined).unwrap_or('\u{fffd}'), end + 6));
                }
            }
            Ok((char::from_u32(c).unwrap_or('\u{fffd}'), end))
        }
    }

    fn float_value(f: f64) -> Value {
        Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null)
    }

    // ------------------------------------------------------------ json.dumps

    /// `json.dumps(v, separators=(", ", ": "), sort_keys=sort_keys)` with the
    /// default `ensure_ascii`.
    pub fn dumps(v: &Value, sort_keys: bool) -> String {
        let mut out = String::new();
        write_json(v, sort_keys, &mut out);
        out
    }

    fn write_json(v: &Value, sort_keys: bool, out: &mut String) {
        match v {
            Value::Null => out.push_str("null"),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            Value::Number(n) => out.push_str(&number_repr(n)),
            Value::String(s) => write_json_str(s, out),
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write_json(x, sort_keys, out);
                }
                out.push(']');
            }
            Value::Object(o) => {
                let mut items: Vec<(&String, &Value)> = o.iter().collect();
                if sort_keys {
                    // UTF-8 byte order is code point order, which is how Python sorts str keys.
                    items.sort_by_key(|(k, _)| *k);
                }
                out.push('{');
                for (i, (k, x)) in items.into_iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write_json_str(k, out);
                    out.push_str(": ");
                    write_json(x, sort_keys, out);
                }
                out.push('}');
            }
        }
    }

    fn write_json_str(s: &str, out: &mut String) {
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
                    for u in c.encode_utf16(&mut units) {
                        let _ = write!(out, "\\u{:04x}", u);
                    }
                }
            }
        }
        out.push('"');
    }
}

// ---------------------------------------------------------------- parsers

const SKIP_EXCERPT_CHARS: usize = 160;

fn skip(skipped: &mut Vec<Skipped>, pointer: String, reason: String, excerpt: &str) {
    let excerpt = match excerpt.char_indices().nth(SKIP_EXCERPT_CHARS) {
        Some((end, _)) => &excerpt[..end],
        None => excerpt,
    };
    skipped.push(Skipped { pointer, reason, excerpt: excerpt.to_string() });
}

fn opt_str<'a>(opts: Option<&'a Value>, key: &str, default: &'a str) -> Result<&'a str, String> {
    match opts.and_then(|o| o.get(key)) {
        None => Ok(default),
        Some(Value::String(s)) => Ok(s),
        Some(other) => Err(format!("option '{key}' must be a string, got {other}")),
    }
}

fn required_str<'a>(opts: Option<&'a Value>, key: &str) -> Result<&'a str, String> {
    match opts.and_then(|o| o.get(key)) {
        Some(Value::String(s)) => Ok(s),
        Some(other) => Err(format!("option '{key}' must be a string, got {other}")),
        None => Err(format!("'{key}'")),
    }
}

/// Parse a payload into a table. Rows that cannot be parsed are reported in
/// `Table::skipped`, never fatal; a payload that fails as a whole is `Err`.
pub fn parse(source: &Source, raw: String) -> Result<Table, String> {
    let opts = source.options.as_ref();
    let mut skipped = Vec::new();
    let records = match source.format.as_str() {
        "csv" => parse_csv(&raw, opts, &mut skipped)?,
        "jsonl" => parse_jsonl(&raw, &mut skipped)?,
        "nested-json" => parse_nested_json(&raw, opts, &mut skipped)?,
        "xml" => parse_xml(&raw, opts)?,
        "columnar" => parse_columnar(&raw, opts, &mut skipped)?,
        "kv" => parse_kv(&raw, opts, &mut skipped)?,
        other => return Err(format!("unknown format '{other}'")),
    };
    Ok(Table { source: source.id.clone(), raw, records, skipped })
}

// --- csv

/// `csv.reader` with the default dialect over `str.splitlines()` output.
/// Lines arrive without their ends, so a quoted field spanning lines is the
/// lines joined with nothing between them, and `line_num` is its last line.
struct CsvReader<'a> {
    raw: &'a str,
    lines: &'a [std::ops::Range<usize>],
    delimiter: char,
    line_num: usize,
}

const CSV_FIELD_LIMIT: usize = 131072;

#[derive(PartialEq)]
enum CsvState {
    StartRecord,
    StartField,
    InField,
    InQuotedField,
    QuoteInQuotedField,
}

impl CsvReader<'_> {
    fn next_row(&mut self) -> Result<Option<Vec<String>>, String> {
        use CsvState::*;
        let mut fields = Vec::new();
        let mut field = String::new();
        let mut field_len = 0;
        let mut state = StartRecord;
        loop {
            let Some(range) = self.lines.get(self.line_num) else {
                if state == InQuotedField {
                    fields.push(field);
                    return Ok(Some(fields));
                }
                return Ok(None);
            };
            self.line_num += 1;
            // `None` marks the end of the line, as CPython's EOL sentinel does.
            for c in self.raw[range.clone()].chars().map(Some).chain([None]) {
                if c == Some('\0') {
                    return Err("line contains NUL".into());
                }
                if state == StartRecord {
                    if c.is_none() {
                        continue;
                    }
                    state = StartField;
                }
                let mut add = false;
                match state {
                    StartRecord => {}
                    StartField | InField => match c {
                        None => {
                            fields.push(std::mem::take(&mut field));
                            field_len = 0;
                            state = StartRecord;
                        }
                        Some('"') if state == StartField => state = InQuotedField,
                        Some(d) if d == self.delimiter => {
                            fields.push(std::mem::take(&mut field));
                            field_len = 0;
                            state = StartField;
                        }
                        Some(_) => {
                            add = true;
                            state = InField;
                        }
                    },
                    InQuotedField => match c {
                        None => {}
                        Some('"') => state = QuoteInQuotedField,
                        Some(_) => add = true,
                    },
                    QuoteInQuotedField => match c {
                        Some('"') => {
                            add = true;
                            state = InQuotedField;
                        }
                        Some(d) if d == self.delimiter => {
                            fields.push(std::mem::take(&mut field));
                            field_len = 0;
                            state = StartField;
                        }
                        None => {
                            fields.push(std::mem::take(&mut field));
                            field_len = 0;
                            state = StartRecord;
                        }
                        Some(_) => {
                            add = true;
                            state = InField;
                        }
                    },
                }
                if add {
                    if field_len >= CSV_FIELD_LIMIT {
                        return Err(format!("field larger than field limit ({CSV_FIELD_LIMIT})"));
                    }
                    field_len += 1;
                    field.extend(c);
                }
            }
            if state == StartRecord {
                return Ok(Some(fields));
            }
        }
    }
}

fn stripped_owned(s: String) -> String {
    let t = py::strip(&s);
    if t.len() == s.len() {
        s
    } else {
        t.to_string()
    }
}

/// `csv.DictReader(lines, restkey="__extra__")`, including what a repeated
/// header name or a column named `__extra__` does to the dict it builds.
fn parse_csv(raw: &str, opts: Option<&Value>, skipped: &mut Vec<Skipped>) -> Result<Vec<Record>, String> {
    let delimiter = opt_str(opts, "delimiter", ",")?;
    let mut chars = delimiter.chars();
    let (Some(delimiter), None) = (chars.next(), chars.next()) else {
        return Err("\"delimiter\" must be a 1-character string".into());
    };
    let lines = py::splitlines(raw);
    let mut reader = CsvReader { raw, lines: &lines, delimiter, line_num: 0 };
    let mut out = Vec::new();
    let Some(fieldnames) = reader.next_row()? else {
        return Ok(out);
    };
    let keys: Vec<&str> = fieldnames.iter().map(|k| py::strip(k)).collect();
    let extra_column = fieldnames.iter().rposition(|k| k == "__extra__");
    let (lf, mut last_of) = (fieldnames.len(), None);
    while let Some(row) = reader.next_row()? {
        if row.is_empty() {
            continue;
        }
        let line_no = reader.line_num;
        let line = lines[line_no - 1].clone();
        let lr = row.len();
        if lf > lr {
            let have = last_of
                .get_or_insert_with(|| {
                    let mut last = IndexMap::new();
                    for (i, k) in fieldnames.iter().enumerate() {
                        last.insert(k.as_str(), i);
                    }
                    last
                })
                .values()
                .filter(|last| **last < lr)
                .count();
            skip(skipped, format!("line {line_no}"), format!("truncated row: {have} of {lf} columns"), &raw[line]);
            continue;
        }
        if lf < lr || extra_column.is_some() {
            let n = match extra_column {
                Some(i) if lf == lr => row[i].chars().count(),
                _ => lr - lf,
            };
            skip(skipped, format!("line {line_no}"), format!("{n} extra column(s)"), &raw[line]);
            continue;
        }
        let mut data = Map::with_capacity(lf);
        for (key, value) in keys.iter().zip(row) {
            data.insert((*key).to_string(), Value::String(stripped_owned(value)));
        }
        out.push(Record { index: out.len() as u32, pointer: format!("line {line_no}"), excerpt: Excerpt::Slice(line), data: Value::Object(data) });
    }
    Ok(out)
}

// --- jsonl, kv

fn parse_jsonl(raw: &str, skipped: &mut Vec<Skipped>) -> Result<Vec<Record>, String> {
    let mut out = Vec::new();
    for (i, range) in py::splitlines(raw).into_iter().enumerate() {
        let line = &raw[range.clone()];
        if py::strip(line).is_empty() {
            continue;
        }
        let n = i + 1;
        match py::json_loads(line) {
            Err(e) if e.msg == py::TOO_DEEP => return Err(e.msg.into()),
            Err(e) => skip(skipped, format!("line {n}"), format!("invalid JSON: {} at column {}", e.msg, e.column(line)), line),
            Ok(data) if !data.is_object() => skip(skipped, format!("line {n}"), "not a JSON object".into(), line),
            Ok(data) => out.push(Record { index: out.len() as u32, pointer: format!("line {n}"), excerpt: Excerpt::Slice(range), data }),
        }
    }
    Ok(out)
}

fn parse_kv(raw: &str, opts: Option<&Value>, skipped: &mut Vec<Skipped>) -> Result<Vec<Record>, String> {
    let pair_sep = opt_str(opts, "pair_sep", ";")?;
    let kv_sep = opt_str(opts, "kv_sep", "=")?;
    let mut out = Vec::new();
    let lines = py::splitlines(raw);
    if !lines.is_empty() && (pair_sep.is_empty() || kv_sep.is_empty()) {
        return Err("empty separator".into());
    }
    for (i, range) in lines.into_iter().enumerate() {
        let line = &raw[range.clone()];
        let stripped = py::strip(line);
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        let mut data = Map::new();
        for pair in stripped.split(pair_sep) {
            if let Some((k, v)) = pair.split_once(kv_sep) {
                data.insert(py::strip(k).to_string(), Value::String(py::strip(v).to_string()));
            }
        }
        let n = i + 1;
        if data.is_empty() {
            skip(skipped, format!("line {n}"), "no key=value pairs".into(), line);
            continue;
        }
        out.push(Record { index: out.len() as u32, pointer: format!("line {n}"), excerpt: Excerpt::Slice(range), data: Value::Object(data) });
    }
    Ok(out)
}

// --- nested-json, columnar

fn loads_whole(raw: &str) -> Result<Value, String> {
    py::json_loads(raw).map_err(|e| e.describe(raw))
}

/// `store::get_path`, moving the value out of the document.
fn take_path(doc: &mut Value, path: &str) -> Option<Value> {
    let obj = doc.as_object_mut()?;
    if obj.contains_key(path) {
        return obj.get_mut(path).map(Value::take);
    }
    let mut cur = doc;
    for part in path.split('.') {
        cur = cur.as_object_mut()?.get_mut(part)?;
    }
    Some(cur.take())
}

/// `for x in (v or [])`: lists as they are, dicts by key, strings by character.
fn py_items(v: Option<Value>) -> Result<Vec<Value>, String> {
    let Some(v) = v.filter(py::truthy) else {
        return Ok(Vec::new());
    };
    match v {
        Value::Array(a) => Ok(a),
        Value::Object(o) => Ok(o.into_iter().map(|(k, _)| Value::String(k)).collect()),
        Value::String(s) => Ok(s.chars().map(|c| Value::String(c.to_string())).collect()),
        other => Err(format!("'{}' object is not iterable", py::type_name(&other))),
    }
}

fn parse_nested_json(raw: &str, opts: Option<&Value>, skipped: &mut Vec<Skipped>) -> Result<Vec<Record>, String> {
    let path = required_str(opts, "records")?;
    let mut doc = loads_whole(raw)?;
    let items = py_items(take_path(&mut doc, path))?;
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.into_iter().enumerate() {
        let excerpt = py::dumps(&item, false);
        if !item.is_object() {
            skip(skipped, format!("{path}[{i}]"), "not an object".into(), &excerpt);
            continue;
        }
        out.push(Record { index: out.len() as u32, pointer: format!("{path}[{i}]"), excerpt: Excerpt::Owned(excerpt), data: item });
    }
    Ok(out)
}

fn field_names(fields: Option<&Value>) -> Result<Vec<String>, String> {
    match fields {
        Some(Value::Array(a)) => a
            .iter()
            .map(|f| match f {
                Value::Array(_) | Value::Object(_) => Err(format!("unhashable type: '{}'", py::type_name(f))),
                other => Ok(py::str_of(other).into_owned()),
            })
            .collect(),
        Some(Value::Object(o)) => Ok(o.keys().cloned().collect()),
        Some(Value::String(s)) => Ok(s.chars().map(String::from).collect()),
        other => Err(format!("object of type '{}' has no len()", other.map_or("NoneType", py::type_name))),
    }
}

fn parse_columnar(raw: &str, opts: Option<&Value>, skipped: &mut Vec<Skipped>) -> Result<Vec<Record>, String> {
    let mut doc = loads_whole(raw)?;
    let fields = get_path(&doc, opt_str(opts, "fields", "schema.fields")?).cloned();
    let rows_path = opt_str(opts, "rows", "rows")?;
    let rows = py_items(take_path(&mut doc, rows_path))?;
    let mut out = Vec::with_capacity(rows.len());
    if rows.is_empty() {
        return Ok(out);
    }
    let fields = field_names(fields.as_ref())?;
    for (i, row) in rows.into_iter().enumerate() {
        let excerpt = py::dumps(&row, false);
        let values = match row {
            Value::Array(values) if values.len() == fields.len() => values,
            other => {
                let n = other.as_array().map_or("no".to_string(), |a| a.len().to_string());
                skip(skipped, format!("{rows_path}[{i}]"), format!("{n} values for {} fields", fields.len()), &excerpt);
                continue;
            }
        };
        let mut data = Map::with_capacity(fields.len());
        for (field, value) in fields.iter().zip(values) {
            data.insert(field.clone(), value);
        }
        out.push(Record { index: out.len() as u32, pointer: format!("{rows_path}[{i}]"), excerpt: Excerpt::Owned(excerpt), data: Value::Object(data) });
    }
    Ok(out)
}

// --- xml

/// An element as `xml.etree.ElementTree` holds it. Namespaced names are
/// `{uri}local`; an empty `text`/`tail` stands for Python's `None`.
struct XmlEl {
    tag: String,
    attrib: Vec<(String, String)>,
    text: String,
    tail: String,
    children: Vec<XmlEl>,
}

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

fn check_xml_name(name: &str) -> Result<(), String> {
    let bad = |c: char| !(c.is_alphanumeric() || matches!(c, '_' | ':' | '-' | '.') || !c.is_ascii());
    let first_bad = name.chars().next().is_none_or(|c| c.is_ascii_digit() || matches!(c, '-' | '.'));
    if first_bad || name.chars().any(bad) {
        return Err(format!("not well-formed (invalid token): {name:?}"));
    }
    Ok(())
}

/// Expand references the way expat does: the five predefined entities and
/// character references. Literal whitespace in an attribute value becomes a space.
fn xml_unescape(s: &str, attr: bool, out: &mut String) -> Result<(), String> {
    let mut rest = s;
    loop {
        let stop = rest.find(|c| c == '&' || (attr && matches!(c, '\t' | '\n' | '<')));
        let Some(stop) = stop else {
            out.push_str(rest);
            return Ok(());
        };
        out.push_str(&rest[..stop]);
        let c = rest.as_bytes()[stop];
        if c != b'&' {
            if c == b'<' {
                return Err("not well-formed (invalid token): '<' in an attribute value".into());
            }
            out.push(' ');
            rest = &rest[stop + 1..];
            continue;
        }
        let after = &rest[stop + 1..];
        let Some(semi) = after.find(';') else {
            return Err("not well-formed (invalid token): '&'".into());
        };
        let name = &after[..semi];
        match name {
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "amp" => out.push('&'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let Some(num) = name.strip_prefix('#') else {
                    check_xml_name(name)?;
                    return Err(format!("undefined entity &{name};"));
                };
                let code = match num.strip_prefix('x') {
                    Some(hex) => u32::from_str_radix(hex, 16),
                    None => num.parse(),
                };
                let c = code.ok().filter(|c| matches!(c, 9 | 10 | 13 | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff)).and_then(char::from_u32);
                out.push(c.ok_or_else(|| format!("reference to invalid character number: &{name};"))?);
            }
        }
        rest = &after[semi + 1..];
    }
}

fn xml_open(e: &BytesStart, ns: &mut Vec<(String, String)>) -> Result<XmlEl, String> {
    let raw_name = std::str::from_utf8(e.name().0).map_err(|e| e.to_string())?;
    check_xml_name(raw_name)?;
    let mut attrs = Vec::new();
    for attr in e.attributes() {
        let attr = attr.map_err(|e| e.to_string())?;
        let key = std::str::from_utf8(attr.key.0).map_err(|e| e.to_string())?;
        check_xml_name(key)?;
        let raw = std::str::from_utf8(&attr.value).map_err(|e| e.to_string())?;
        let mut value = String::with_capacity(raw.len());
        xml_unescape(raw, true, &mut value)?;
        if key == "xmlns" {
            ns.push((String::new(), value));
        } else if let Some(prefix) = key.strip_prefix("xmlns:") {
            ns.push((prefix.to_string(), value));
        } else {
            attrs.push((key.to_string(), value));
        }
    }
    let uri_of = |prefix: &str| -> Option<&str> {
        if prefix == "xml" {
            return Some(XML_NS);
        }
        ns.iter().rev().find(|(p, _)| p == prefix).map(|(_, u)| u.as_str())
    };
    let tag = match raw_name.split_once(':') {
        Some((prefix, local)) => format!("{{{}}}{local}", uri_of(prefix).ok_or_else(|| format!("unbound prefix: {raw_name}"))?),
        None => match uri_of("") {
            Some(uri) if !uri.is_empty() => format!("{{{uri}}}{raw_name}"),
            _ => raw_name.to_string(),
        },
    };
    let mut expanded = false;
    for (key, _) in &mut attrs {
        if let Some((prefix, local)) = key.split_once(':') {
            let uri = uri_of(prefix).ok_or_else(|| format!("unbound prefix: {key}"))?;
            *key = format!("{{{uri}}}{local}");
            expanded = true;
        }
    }
    if expanded && (1..attrs.len()).any(|i| attrs[..i].iter().any(|(k, _)| *k == attrs[i].0)) {
        return Err("duplicate attribute".into());
    }
    Ok(XmlEl { tag, attrib: attrs, text: String::new(), tail: String::new(), children: Vec::new() })
}

/// `ET.fromstring(text)`: comments and processing instructions vanish, CDATA
/// joins the text, line ends normalise to `\n`. `Err` for anything expat would
/// reject that the checks here can see. Internal DTD entities are not expanded.
fn xml_parse(raw: &str) -> Result<XmlEl, String> {
    let normalised;
    let text = if raw.contains('\r') {
        normalised = raw.replace("\r\n", "\n").replace('\r', "\n");
        &normalised
    } else {
        raw
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.bytes().any(|b| b < 0x20 && !matches!(b, b'\t' | b'\n')) {
        return Err("not well-formed (invalid token)".into());
    }
    let mut reader = Reader::from_str(text);
    reader.config_mut().check_comments = true;
    let mut stack: Vec<XmlEl> = Vec::new();
    let mut marks: Vec<usize> = Vec::new();
    let mut ns: Vec<(String, String)> = Vec::new();
    let mut root = None;
    let mut first = true;
    loop {
        let event = reader.read_event().map_err(|e| e.to_string())?;
        let is_first = std::mem::replace(&mut first, false);
        let close = match &event {
            Event::Start(e) | Event::Empty(e) => {
                if stack.is_empty() && root.is_some() {
                    return Err("junk after document element".into());
                }
                marks.push(ns.len());
                stack.push(xml_open(e, &mut ns)?);
                matches!(event, Event::Empty(_))
            }
            Event::End(_) => true,
            Event::Text(t) => {
                let s = std::str::from_utf8(t).map_err(|e| e.to_string())?;
                match stack.last_mut() {
                    Some(el) => {
                        if s.contains("]]>") {
                            return Err("not well-formed (invalid token): ']]>' in text".into());
                        }
                        let target = el.children.last_mut().map_or(&mut el.text, |c| &mut c.tail);
                        xml_unescape(s, false, target)?;
                    }
                    None if s.chars().all(|c| matches!(c, ' ' | '\t' | '\n')) => {}
                    None => return Err(if root.is_some() { "junk after document element".into() } else { "syntax error".to_string() }),
                }
                false
            }
            Event::CData(t) => {
                let s = std::str::from_utf8(t).map_err(|e| e.to_string())?;
                let Some(el) = stack.last_mut() else {
                    return Err("not well-formed (invalid token): CDATA outside the root".into());
                };
                el.children.last_mut().map_or(&mut el.text, |c| &mut c.tail).push_str(s);
                false
            }
            Event::Decl(_) if !is_first => return Err("XML or text declaration not at start of entity".into()),
            Event::Eof => break,
            _ => false,
        };
        if close {
            let Some(el) = stack.pop() else {
                return Err("not well-formed (invalid token): unmatched end tag".into());
            };
            ns.truncate(marks.pop().unwrap_or(0));
            match stack.last_mut() {
                Some(parent) => parent.children.push(el),
                None => root = Some(el),
            }
        }
    }
    if !stack.is_empty() {
        return Err("no element found: unclosed element".into());
    }
    root.ok_or_else(|| "no element found".to_string())
}

/// `root.findall(path)` for the path forms packs use: tags, `*`, `.`, `//`
/// between steps. Predicates (`t[1]`, `t[@a='b']`) are `Err`: no pack uses them.
fn xml_find<'a>(root: &'a XmlEl, path: &str) -> Result<Vec<&'a XmlEl>, String> {
    if path.is_empty() {
        return Ok(Vec::new());
    }
    fn descendants<'a>(el: &'a XmlEl, tag: &str, out: &mut Vec<&'a XmlEl>) {
        for c in &el.children {
            if tag == "*" || c.tag == tag {
                out.push(c);
            }
            descendants(c, tag, out);
        }
    }
    let unsupported = || format!("unsupported element path: {}", py::repr_str(path));
    let implicit;
    let path = if path.ends_with('/') {
        implicit = format!("{path}*");
        &implicit
    } else {
        path
    };
    let mut result = vec![root];
    let mut rest = path;
    let mut descend = false;
    if rest.starts_with('/') {
        return Err("cannot use absolute path on element".into());
    }
    loop {
        let mut depth = 0;
        let end = rest
            .char_indices()
            .find(|(_, c)| {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                *c == '/' && depth == 0
            })
            .map_or(rest.len(), |(i, _)| i);
        let step = &rest[..end];
        let plain = depth == 0 && !step.is_empty() && step != ".." && !step.contains(['[', ']', '@', '(', ')', '=', ',', '\'', '"']);
        if !plain || (descend && step == ".") {
            return Err(unsupported());
        }
        if step != "." {
            let mut next = Vec::new();
            for el in &result {
                if descend {
                    descendants(el, step, &mut next);
                } else {
                    next.extend(el.children.iter().filter(|c| step == "*" || c.tag == step));
                }
            }
            result = next;
        }
        if end == rest.len() {
            return Ok(result);
        }
        rest = &rest[end + 1..];
        descend = false;
        if let Some(r) = rest.strip_prefix('/') {
            rest = r;
            descend = true;
        }
    }
}

/// `_flatten_xml`: `@attr`, `#text`, `child`, `child.@attr`; a repeated key keeps the first.
fn xml_flatten(el: &XmlEl, prefix: &str, out: &mut Map<String, Value>) {
    for (k, v) in &el.attrib {
        out.entry(format!("{prefix}@{k}")).or_insert_with(|| Value::String(v.clone()));
    }
    let text = py::strip(&el.text);
    if !text.is_empty() {
        let key = if prefix.is_empty() { "#text" } else { &prefix[..prefix.len() - 1] };
        out.entry(key.to_string()).or_insert_with(|| Value::String(text.to_string()));
    }
    for child in &el.children {
        xml_flatten(child, &format!("{prefix}{}.", child.tag), out);
    }
}

fn xml_known_prefix(uri: &str) -> Option<&'static str> {
    Some(match uri {
        "http://www.w3.org/1999/xhtml" => "html",
        "http://www.w3.org/1999/02/22-rdf-syntax-ns#" => "rdf",
        "http://schemas.xmlsoap.org/wsdl/" => "wsdl",
        "http://www.w3.org/2001/XMLSchema" => "xs",
        "http://www.w3.org/2001/XMLSchema-instance" => "xsi",
        "http://purl.org/dc/elements/1.1/" => "dc",
        _ => return None,
    })
}

/// The (uri, prefix) table `ET.tostring` builds for an element, in document
/// order. The `xml` prefix is implicit: never declared, never numbered.
fn xml_collect_ns(el: &XmlEl, ns: &mut Vec<(String, String)>) {
    let mut add = |name: &str| {
        if let Some((uri, _)) = name.strip_prefix('{').and_then(|n| n.rsplit_once('}')) {
            if uri != XML_NS && !ns.iter().any(|(u, _)| u == uri) {
                let prefix = xml_known_prefix(uri).map_or_else(|| format!("ns{}", ns.len()), String::from);
                ns.push((uri.to_string(), prefix));
            }
        }
    };
    add(&el.tag);
    for (k, _) in &el.attrib {
        add(k);
    }
    for c in &el.children {
        xml_collect_ns(c, ns);
    }
}

fn xml_write_name(name: &str, ns: &[(String, String)], out: &mut String) {
    match name.strip_prefix('{').and_then(|n| n.rsplit_once('}')) {
        Some((uri, local)) => {
            if uri == XML_NS {
                out.push_str("xml:");
            } else if let Some((_, prefix)) = ns.iter().find(|(u, _)| u == uri) {
                out.push_str(prefix);
                out.push(':');
            }
            out.push_str(local);
        }
        None => out.push_str(name),
    }
}

fn xml_escape_cdata(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
}

fn xml_escape_attrib(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\r' => out.push_str("&#13;"),
            '\n' => out.push_str("&#10;"),
            '\t' => out.push_str("&#09;"),
            c => out.push(c),
        }
    }
}

fn xml_serialize(el: &XmlEl, ns: &[(String, String)], top: bool, out: &mut String) {
    out.push('<');
    xml_write_name(&el.tag, ns, out);
    if top && !ns.is_empty() {
        let mut sorted: Vec<&(String, String)> = ns.iter().collect();
        sorted.sort_by(|a, b| a.1.cmp(&b.1));
        for (uri, prefix) in sorted {
            out.push_str(" xmlns:");
            out.push_str(prefix);
            out.push_str("=\"");
            xml_escape_attrib(uri, out);
            out.push('"');
        }
    }
    for (k, v) in &el.attrib {
        out.push(' ');
        xml_write_name(k, ns, out);
        out.push_str("=\"");
        xml_escape_attrib(v, out);
        out.push('"');
    }
    if el.text.is_empty() && el.children.is_empty() {
        out.push_str(" />");
    } else {
        out.push('>');
        xml_escape_cdata(&el.text, out);
        for c in &el.children {
            xml_serialize(c, ns, false, out);
        }
        out.push_str("</");
        xml_write_name(&el.tag, ns, out);
        out.push('>');
    }
    xml_escape_cdata(&el.tail, out);
}

/// `re.sub(r"\s+\n", "\n", s).strip()`: within a whitespace run, everything up
/// to its last newline collapses to that newline, if anything precedes it.
fn xml_collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = py::strip(s);
    while let Some(start) = rest.find(py::is_space) {
        out.push_str(&rest[..start]);
        let run = &rest[start..];
        let len = run.find(|c| !py::is_space(c)).unwrap_or(run.len());
        let (run, after) = run.split_at(len);
        match run.rfind('\n') {
            Some(last) if last > 0 => out.push_str(&run[last..]),
            _ => out.push_str(run),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

fn xml_excerpt(el: &XmlEl) -> String {
    let mut ns = Vec::new();
    xml_collect_ns(el, &mut ns);
    let mut out = String::new();
    xml_serialize(el, &ns, true, &mut out);
    xml_collapse(&out)
}

fn parse_xml(raw: &str, opts: Option<&Value>) -> Result<Vec<Record>, String> {
    let root = xml_parse(raw)?;
    let path = required_str(opts, "records")?;
    let found = xml_find(&root, path)?;
    let mut out = Vec::with_capacity(found.len());
    for (i, el) in found.into_iter().enumerate() {
        let mut data = Map::new();
        xml_flatten(el, "", &mut data);
        out.push(Record {
            index: i as u32,
            pointer: format!("/{}/{}[{}]", root.tag, el.tag, i + 1),
            excerpt: Excerpt::Owned(xml_excerpt(el)),
            data: Value::Object(data),
        });
    }
    Ok(out)
}

/// The value a `time.document.path` names: a root JSON path for nested-json
/// and columnar, `@attr` or a child path's text on an XML root. `Ok(None)`
/// when it is absent.
pub(crate) fn document_path_value(raw: &str, format: &str, path: &str) -> Result<Option<Value>, String> {
    match format {
        "nested-json" | "columnar" => Ok(get_path(&loads_whole(raw)?, path).cloned()),
        "xml" => {
            let root = xml_parse(raw)?;
            if let Some(name) = path.strip_prefix('@') {
                let v = root.attrib.iter().find(|(k, _)| k == name);
                return Ok(v.map(|(_, v)| Value::String(v.clone())));
            }
            Ok(xml_find(&root, path)?.first().map(|el| Value::String(el.text.clone())))
        }
        other => Err(format!("document path is not supported for {other}; use a regex or a constant")),
    }
}

// ---------------------------------------------------------------- conditions

/// `str(value) in [str(x) for x in arg]`. `None` where Python raises `TypeError`.
fn str_in(value: &str, arg: &Value) -> Option<bool> {
    match arg {
        Value::Array(a) => Some(a.iter().any(|x| py::str_of(x) == value)),
        Value::Object(o) => Some(o.contains_key(value)),
        Value::String(s) => {
            let mut chars = value.chars();
            Some(matches!((chars.next(), chars.next()), (Some(c), None) if s.contains(c)))
        }
        _ => None,
    }
}

fn test(value: Option<&Value>, cond: &Value) -> bool {
    let Value::Object(ops) = cond else {
        return value.is_some_and(|v| py::str_of(v) == py::str_of(cond));
    };
    for (op, arg) in ops {
        if op == "exists" {
            if present(value) != py::truthy(arg) {
                return false;
            }
            continue;
        }
        let Some(v) = value else {
            return false;
        };
        let holds = match op.as_str() {
            "eq" => py::str_of(v) == py::str_of(arg),
            "ne" => py::str_of(v) != py::str_of(arg),
            "gt" | "gte" | "lt" | "lte" => {
                let (Ok(a), Ok(b)) = (py::to_float(v), py::to_float(arg)) else {
                    return false;
                };
                match op.as_str() {
                    "gt" => a > b,
                    "gte" => a >= b,
                    "lt" => a < b,
                    _ => a <= b,
                }
            }
            "in" => str_in(&py::str_of(v), arg) == Some(true),
            "nin" => str_in(&py::str_of(v), arg) == Some(false),
            "contains" => py::str_of(v).to_lowercase().contains(&py::str_of(arg).to_lowercase()),
            "startswith" => py::str_of(v).starts_with(&*py::str_of(arg)),
            _ => true,
        };
        if !holds {
            return false;
        }
    }
    true
}

/// `where` / `when` evaluation (spec §1, conditions). A field holding null
/// counts as absent. A truthy `where` that is not an object, and `in`/`nin`
/// with a non-iterable argument, crash the Python engine; here they are false.
pub fn matches(data: &Value, where_: Option<&Value>) -> bool {
    let Some(where_) = where_ else {
        return true;
    };
    let Value::Object(fields) = where_ else {
        return !py::truthy(where_);
    };
    fields.iter().all(|(field, cond)| test(get_path(data, field).filter(|v| !v.is_null()), cond))
}

// ---------------------------------------------------------------- transforms

fn is_cased(c: char) -> bool {
    c.is_lowercase() || c.is_uppercase() || matches!(c, '\u{1c5}' | '\u{1c8}' | '\u{1cb}' | '\u{1f2}')
}

/// `str.title()`: a cased character after a cased one lowers, any other titlecases.
fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut prev_cased = false;
    while let Some(c) = chars.next() {
        if prev_cased {
            if c == 'Σ' && !chars.peek().copied().is_some_and(is_cased) {
                out.push('ς');
            } else {
                out.extend(c.to_lowercase());
            }
        } else {
            match c {
                '\u{1c4}'..='\u{1c6}' => out.push('\u{1c5}'),
                '\u{1c7}'..='\u{1c9}' => out.push('\u{1c8}'),
                '\u{1ca}'..='\u{1cc}' => out.push('\u{1cb}'),
                '\u{1f1}'..='\u{1f3}' => out.push('\u{1f2}'),
                '\u{10d0}'..='\u{10ff}' => out.push(c),
                _ => {
                    let mut upper = c.to_uppercase();
                    out.extend(upper.next());
                    for rest in upper {
                        out.extend(rest.to_lowercase());
                    }
                }
            }
        }
        prev_cased = is_cased(c);
    }
    out
}

fn name_last_first(v: &str) -> String {
    match v.split_once(',') {
        Some((last, first)) => format!("{} {}", py::strip(first), py::strip(last)),
        None => v.to_string(),
    }
}

/// Apply one transform by name (spec §1, transforms). `Err` when the value
/// does not fit. `float` of a non-finite value is `Err` too: JSON values
/// cannot hold it.
pub fn transform(name: &str, value: &Value) -> Result<Value, String> {
    let text = || py::str_of(value);
    Ok(match name {
        "lower" => Value::String(text().to_lowercase()),
        "upper" => Value::String(text().to_uppercase()),
        "strip" => Value::String(py::strip(&text()).to_string()),
        "title" => Value::String(py_title(&text())),
        "int" => {
            let f = py::to_float(value)?;
            if f.is_nan() {
                return Err("cannot convert float NaN to integer".into());
            }
            if f.is_infinite() {
                return Err("cannot convert float infinity to integer".into());
            }
            let t = f.trunc();
            if t >= i64::MIN as f64 && t < i64::MAX as f64 {
                Value::Number((t as i64).into())
            } else if t >= 0.0 && t < u64::MAX as f64 {
                Value::Number((t as u64).into())
            } else {
                Value::Number(Number::from_f64(t).ok_or("number out of range")?)
            }
        }
        "float" => Value::Number(Number::from_f64(py::to_float(value)?).ok_or("cannot represent a non-finite float")?),
        "name_last_first" => Value::String(name_last_first(&text())),
        "date_dmy" => Value::String(py::date_dmy(&text())?),
        "epoch_ms" => Value::String(time::iso_of_micros(time::micros_from_ms(py::to_int(value)?)?)),
        "epoch_s" => Value::String(time::iso_of_micros(time::micros_from_s(py::to_int(value)?)?)),
        other => return Err(format!("unknown transform '{other}'")),
    })
}

// ---------------------------------------------------------------- fragments

/// One property value with its provenance, as produced by a mapping.
#[derive(Debug, Clone)]
pub struct FragProp {
    pub value: Value,
    pub field: String,
}

/// One mapping applied to one record. Id: `<source>#<record index>:<Type>:<mapping index>`.
#[derive(Debug, Clone)]
pub struct Fragment {
    pub id: String,
    pub otype: String,
    pub source: String,
    pub record: u32,
    /// The record's pointer (spec §2), the provenance of every prop here.
    pub pointer: String,
    pub mapping: usize,
    pub props: IndexMap<String, FragProp>,
}

/// Fragments of a table under the mappings that target its source. `mappings`
/// carries each mapping's index in the pack (part of the fragment id). Ordered
/// by mapping, then record. A transform failure skips that property (the
/// Python engine raises and fails the whole recompute).
pub fn fragments(table: &Table, mappings: &[(usize, &Mapping)]) -> Vec<Fragment> {
    let mut out = Vec::new();
    for (mi, m) in mappings {
        for r in &table.records {
            if !matches(&r.data, m.when.as_ref()) {
                continue;
            }
            let mut props = IndexMap::new();
            'props: for (prop, spec) in &m.props {
                let field = spec.field();
                let Some(v) = get_path(&r.data, field).filter(|v| present(Some(v))) else {
                    continue;
                };
                let mut value = v.clone();
                for name in spec.transforms() {
                    match transform(name, &value) {
                        Ok(next) => value = next,
                        Err(_) => continue 'props,
                    }
                }
                props.insert(prop.clone(), FragProp { value, field: field.to_string() });
            }
            if !props.is_empty() {
                out.push(Fragment {
                    id: format!("{}#{}:{}:{}", table.source, r.index, m.otype, mi),
                    otype: m.otype.clone(),
                    source: table.source.clone(),
                    record: r.index,
                    pointer: r.pointer.clone(),
                    mapping: *mi,
                    props,
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------- source checks

/// The parse-dependent pack checks (spec §1, warnings): parse every file
/// source, report skipped rows, dead mapped fields, time-field problems and
/// undated sources. Returns warnings, or `Err` for a payload that fails as a
/// whole. In the Python validator's order and wording; `Pack::warnings` holds
/// the checks that come before these.
pub fn check_sources(pack: &Pack) -> Result<Vec<String>, Vec<String>> {
    let mut warnings = Vec::new();
    let mut untimed = Vec::new();
    let never = |table: &Table, field: &str| table.records.iter().all(|r| !present(get_path(&r.data, field)));
    for (sid, s) in &pack.sources {
        if s.transport != "file" {
            continue;
        }
        let failed = |e: String| vec![format!("source '{sid}' failed to parse as {}: {e}", s.format)];
        let path = s.path.as_ref().ok_or_else(|| failed("no file".into()))?;
        let raw = std::fs::read_to_string(path).map_err(|e| failed(e.to_string()))?;
        let table = parse(s, raw).map_err(failed)?;
        if let Some(first) = table.skipped.first() {
            warnings.push(format!("source '{sid}': {} row(s) skipped, e.g. {}: {}", table.skipped.len(), first.pointer, first.reason));
        }
        if table.records.is_empty() {
            warnings.push(format!("source '{sid}' parsed to zero records"));
            continue;
        }
        for (mi, m) in pack.mappings.iter().enumerate() {
            if m.source != *sid {
                continue;
            }
            for spec in m.props.values() {
                if never(&table, spec.field()) {
                    warnings.push(format!("mapping #{}: field '{}' never appears in '{sid}'", mi + 1, spec.field()));
                }
            }
        }
        let Some(tconf) = &s.time else {
            untimed.push(sid.as_str());
            continue;
        };
        if tconf.document.is_some() {
            time::document_time(&table.raw, s, tconf).map_err(|e| vec![format!("manifest: source '{sid}' document time: {e}")])?;
        } else if let Some(fields) = &tconf.field {
            for field in fields.list() {
                if never(&table, field) {
                    warnings.push(format!("manifest: time field '{field}' never appears in '{sid}'"));
                }
            }
            let (mut bad, mut first_bad, mut missing) = (0, None, 0);
            for r in &table.records {
                match time::record_time(&r.data, tconf) {
                    Ok(None) => missing += 1,
                    Ok(Some(_)) => {}
                    Err(e) => {
                        bad += 1;
                        first_bad.get_or_insert_with(|| format!("{} ({e})", r.pointer));
                    }
                }
            }
            if let Some(example) = first_bad {
                warnings.push(format!("manifest: {bad} record(s) of '{sid}' have a time that does not parse as {} and will count as undated, e.g. {example}", tconf.unit));
            }
            if missing > 0 {
                warnings.push(format!("manifest: {missing} record(s) of '{sid}' have no time value and will count as undated"));
            }
        }
    }
    if !untimed.is_empty() && untimed.len() < pack.sources.len() {
        warnings.push(format!("undated sources (no time config), treated as unknown_time={}: {}", pack.unknown_time, untimed.join(", ")));
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn floats_print_like_python_repr() {
        for (f, want) in [
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (1.5e300, "1.5e+300"),
            (-0.0, "-0.0"),
            (0.0, "0.0"),
            (123456789012345680.0, "1.2345678901234568e+17"),
            (5e-324, "5e-324"),
            (156.25, "156.25"),
        ] {
            assert_eq!(py::float_repr(f), want);
        }
    }

    #[test]
    fn splitlines_breaks_where_python_does() {
        let text = "a\r\nb\rc\x0bd\x1ee\u{85}f\u{2028}g\n\nh\n";
        let lines: Vec<&str> = py::splitlines(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(lines, ["a", "b", "c", "d", "e", "f", "g", "", "h"]);
    }

    #[test]
    fn json_errors_carry_python_messages_and_columns() {
        for (text, msg, column) in [
            (r#"{"a":1,}"#, "Expecting property name enclosed in double quotes", 8),
            ("[1,]", "Expecting value", 4),
            (r#"{"a" 1}"#, "Expecting ':' delimiter", 6),
            (r#"{"a":1 "b":2}"#, "Expecting ',' delimiter", 8),
            (r#"{"a":"x"#, "Unterminated string starting at", 6),
            (r#"{"a":"\q"}"#, "Invalid \\escape", 7),
            (r#"{"a":"\u12"}"#, "Invalid \\uXXXX escape", 8),
            ("tru", "Expecting value", 1),
            (r#"{"a":NaN}x"#, "Extra data", 10),
            ("1e", "Extra data", 2),
            ("{\"é\":\"é", "Unterminated string starting at", 6),
            ("{\"a\":\"\x01\"}", "Invalid control character at", 7),
            ("[", "Expecting value", 2),
            ("{", "Expecting property name enclosed in double quotes", 2),
        ] {
            let e = py::json_loads(text).unwrap_err();
            assert_eq!((e.msg, e.column(text)), (msg, column), "{text}");
        }
    }

    #[test]
    fn deep_json_fails_without_overflowing_the_stack() {
        let text = "[".repeat(100_000);
        assert_eq!(py::json_loads(&text).unwrap_err().msg, py::TOO_DEEP);
    }

    #[test]
    fn conditions_follow_python_semantics() {
        let d = json!({"s": "Hello", "n": 5, "f": 1.0, "e": "", "z": null, "t": true, "x.y": 1, "x": {"y": 2}});
        let yes = |w: Value| assert!(matches(&d, Some(&w)), "{w}");
        let no = |w: Value| assert!(!matches(&d, Some(&w)), "{w}");
        yes(json!({"s": "Hello", "n": "5"}));
        no(json!({"s": "hello"}));
        no(json!({"f": 1})); // str(1.0) != str(1)
        yes(json!({"f": 1.0}));
        yes(json!({"f": {"eq": "1.0"}, "t": "True", "x.y": 1}));
        yes(json!({"e": {"exists": false}, "z": {"exists": false}, "gone": {"exists": false}, "s": {"exists": true}}));
        no(json!({"e": {"exists": true}}));
        no(json!({"z": {"eq": "None"}}));
        yes(json!({"n": {"gt": "4.5", "lte": 5}, "t": {"gte": 1}}));
        no(json!({"s": {"gt": 1}}));
        no(json!({"gone": {"lt": 1}}));
        yes(json!({"n": {"in": [5, "6"]}, "s": {"nin": ["x"], "contains": "ELL", "startswith": "He"}}));
        no(json!({"s": {"startswith": "he"}}));
        assert!(matches(&d, None));
        assert!(matches(&d, Some(&json!({}))));
    }

    #[test]
    fn transforms_follow_python_semantics() {
        let t = |n: &str, v: Value| transform(n, &v).unwrap();
        assert_eq!(t("title", json!("o'neil mary-jane 3d")), json!("O'Neil Mary-Jane 3D"));
        assert_eq!(t("int", json!("12.9")), json!(12));
        assert_eq!(t("int", json!(-3.5)), json!(-3));
        assert_eq!(t("float", json!("1_000.5")), json!(1000.5));
        assert_eq!(t("name_last_first", json!("Doe, Jane Q")), json!("Jane Q Doe"));
        assert_eq!(t("date_dmy", json!("5.3-2024")), json!("2024-03-05"));
        assert_eq!(t("epoch_ms", json!(1791245789999i64)), json!("2026-10-06T00:16:29Z"));
        assert_eq!(t("epoch_s", json!("1791245789")), json!("2026-10-06T00:16:29Z"));
        assert_eq!(t("lower", json!(1.0)), json!("1.0"));
        assert!(transform("int", &json!("abc")).is_err());
        assert!(transform("date_dmy", &json!("nope")).is_err());
    }

    #[test]
    fn xml_excerpts_collapse_blank_line_ends() {
        assert_eq!(xml_collapse("<a>  \n   \n  <b />  \n</a>\n  "), "<a>\n  <b />\n</a>");
        assert_eq!(xml_collapse("a\n  b"), "a\n  b");
    }
}

//! Parsed records of a source (spec §2).
//!
//! A `Table` keeps the raw payload once and every record as a pointer, an
//! excerpt range into the raw text, and its parsed fields. Field data is a
//! JSON tree today; the scale work (interned strings, typed columns) changes
//! the inside of this module without changing `Record`'s API, which is the
//! only thing the rest of the engine uses.

use serde::Serialize;
use serde_json::Value;
use std::ops::Range;

#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub pointer: String,
    pub reason: String,
    pub excerpt: String,
}

#[derive(Debug, Clone)]
pub struct Record {
    /// 0-based position among the kept records of the source (skipped rows do not count).
    pub index: u32,
    /// Where in the payload: `line N`, `data.flows[i]`, `/root/tag[i]`, `rows[i]`.
    pub pointer: String,
    /// The record's own raw text, as a range into the table's payload, or an
    /// owned string when the raw form is synthesised (nested JSON, columnar rows).
    pub excerpt: Excerpt,
    pub data: Value,
}

#[derive(Debug, Clone)]
pub enum Excerpt {
    Slice(Range<usize>),
    Owned(String),
}

#[derive(Debug, Clone)]
pub struct Table {
    pub source: String,
    pub raw: String,
    pub records: Vec<Record>,
    pub skipped: Vec<Skipped>,
}

impl Table {
    pub fn excerpt<'a>(&'a self, r: &'a Record) -> &'a str {
        match &r.excerpt {
            Excerpt::Slice(range) => &self.raw[range.clone()],
            Excerpt::Owned(s) => s,
        }
    }

    pub fn get(&self, index: u32) -> Option<&Record> {
        self.records.get(index as usize)
    }
}

/// Read a dotted path. An exact key wins (so flattened XML keys like
/// `window.@start` resolve directly); otherwise descend one segment at a time.
pub fn get_path<'a>(data: &'a Value, path: &str) -> Option<&'a Value> {
    let obj = data.as_object()?;
    if let Some(v) = obj.get(path) {
        return Some(v);
    }
    let mut cur = data;
    for part in path.split('.') {
        cur = cur.as_object()?.get(part)?;
    }
    Some(cur)
}

/// A value's string form as the Python engine prints it: strings as they
/// are, numbers without a trailing `.0` when integral, booleans as
/// `True`/`False` (Python's `str`), null as `None`.
pub fn string_form(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True".into() } else { "False".into() },
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 && f.abs() < 1e16 { format!("{}", f as i64) } else { format!("{f}") }
            } else {
                n.to_string()
            }
        }
        other => other.to_string(),
    }
}

/// True when the value counts as present (not null, not the empty string).
pub fn present(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

//! Parsing, conditions, transforms and fragments (spec §2). Phase 1b.
//!
//! Interface fixed in phase 0; bodies are built against
//! `golden/records/<pack>/<source>.json` (per-source records and skipped
//! rows dumped from the Python engine, which must match exactly).

use crate::pack::{Mapping, Source};
use crate::store::{Skipped, Table};
use indexmap::IndexMap;
use serde_json::Value;

/// Parse a payload into a table. Rows that cannot be parsed are reported in
/// `Table::skipped`, never fatal; a payload that fails as a whole is `Err`.
pub fn parse(source: &Source, raw: String) -> Result<Table, String> {
    let _ = (source, raw);
    todo!("phase 1b: parsers")
}

/// `where` / `when` evaluation (spec §1, conditions).
pub fn matches(data: &Value, where_: Option<&Value>) -> bool {
    let _ = (data, where_);
    todo!("phase 1b: conditions")
}

/// Apply one transform by name (spec §1, transforms). `Err` when the value does not fit.
pub fn transform(name: &str, value: &Value) -> Result<Value, String> {
    let _ = (name, value);
    todo!("phase 1b: transforms")
}

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
    pub mapping: usize,
    pub props: IndexMap<String, FragProp>,
}

/// Fragments of a table under the mappings that target its source. `mappings`
/// carries each mapping's index in the pack (part of the fragment id).
pub fn fragments(table: &Table, mappings: &[(usize, &Mapping)]) -> Vec<Fragment> {
    let _ = (table, mappings);
    todo!("phase 1b: fragments")
}

/// The parse-dependent pack checks (spec §1, warnings): parse every file
/// source, report skipped rows, dead mapped fields, time-field problems and
/// undated sources. Returns warnings, or `Err` for a payload that fails as a whole.
pub fn check_sources(pack: &crate::pack::Pack) -> Result<Vec<String>, Vec<String>> {
    let _ = pack;
    todo!("phase 1b: source checks")
}

#[allow(dead_code)]
fn _unused(_: Skipped) {}

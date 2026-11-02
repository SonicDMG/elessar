//! Signal rules (spec §5). Phase 2.
//!
//! Built against `golden/board/<pack>.json` (per-signal hits as record
//! pointers, activations and link weights dumped from the Python engine).

use crate::pack::{Signal, Strength};
use crate::pipeline::matches;
use crate::resolve::Resolution;
use crate::store::{get_path, present, string_form, Record, Table};
use crate::time::parse_time;
use indexmap::{Equivalent, IndexMap};
use serde_json::Value;
use std::borrow::Cow;
use std::hash::{Hash, Hasher};

/// One hit: the records (source, index) it came from and the fragment ids of those records.
#[derive(Debug, Clone)]
pub struct Hit {
    pub records: Vec<(String, u32)>,
    pub fragments: Vec<String>,
}

pub struct Context<'a> {
    /// Connected sources' tables, in rack order.
    pub tables: &'a IndexMap<String, Table>,
    /// (source, record index) -> fragment ids
    pub frags_by_record: &'a IndexMap<(String, u32), Vec<String>>,
    pub resolution: &'a Resolution,
}

/// Looks up `frags_by_record` without allocating a `String` for the key. Hashes as `(String, u32)` does.
struct RecordKey<'a>(&'a str, u32);

impl Hash for RecordKey<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
        self.1.hash(state);
    }
}

impl Equivalent<(String, u32)> for RecordKey<'_> {
    fn equivalent(&self, key: &(String, u32)) -> bool {
        self.0 == key.0 && self.1 == key.1
    }
}

fn frags_of<'c>(ctx: &'c Context, source: &str, index: u32) -> &'c [String] {
    ctx.frags_by_record.get(&RecordKey(source, index)).map(Vec::as_slice).unwrap_or(&[])
}

fn record_hit(ctx: &Context, source: &str, index: u32) -> Hit {
    Hit { records: vec![(source.to_string(), index)], fragments: frags_of(ctx, source, index).to_vec() }
}

/// The signal's source table and its records matching `where`, in file order.
fn matching<'c>(signal: &'c Signal, ctx: &'c Context) -> Option<(&'c str, impl Iterator<Item = &'c Record>)> {
    let source = signal.source.as_deref()?;
    let table = ctx.tables.get(source)?;
    Some((source, table.records.iter().filter(move |r| matches(&r.data, signal.r#where.as_ref()))))
}

fn eval_match(signal: &Signal, ctx: &Context) -> Vec<Hit> {
    let Some((source, recs)) = matching(signal, ctx) else { return Vec::new() };
    recs.map(|r| record_hit(ctx, source, r.index)).collect()
}

fn in_window(hour: f64, start: f64, end: f64) -> bool {
    if start <= end {
        start <= hour && hour < end
    } else {
        hour >= start || hour < end
    }
}

/// Hour of day in UTC with minutes as a fraction; seconds do not count.
fn hour_of_day(epoch_ms: i64) -> f64 {
    let minutes = epoch_ms.rem_euclid(86_400_000) / 60_000;
    (minutes / 60) as f64 + (minutes % 60) as f64 / 60.0
}

fn eval_time_window(signal: &Signal, ctx: &Context) -> Vec<Hit> {
    let Some(params) = signal.params.as_ref() else { return Vec::new() };
    let Some(field) = params.get("field").and_then(Value::as_str) else { return Vec::new() };
    let unit = params.get("unit").and_then(Value::as_str).unwrap_or("iso");
    let hours = params.get("hours").and_then(Value::as_array);
    let hour = |i: usize| hours.and_then(|h| h.get(i)).and_then(Value::as_f64);
    let (Some(start), Some(end)) = (hour(0), hour(1)) else { return Vec::new() };
    let Some((source, recs)) = matching(signal, ctx) else { return Vec::new() };
    recs.filter(|r| {
        let raw = get_path(&r.data, field);
        // An unparseable time is no hit. (Python raises here and takes the recompute down.)
        present(raw) && raw.and_then(|v| parse_time(v, unit).ok()).is_some_and(|t| in_window(hour_of_day(t), start, end))
    })
    .map(|r| record_hit(ctx, source, r.index))
    .collect()
}

fn group_key(value: Option<&Value>) -> Cow<'_, str> {
    match value {
        Some(Value::String(s)) => Cow::Borrowed(s),
        Some(v) => Cow::Owned(string_form(v)),
        None => Cow::Borrowed("None"),
    }
}

fn eval_count(signal: &Signal, ctx: &Context) -> Vec<Hit> {
    let params = signal.params.as_ref();
    let Some(group_by) = params.and_then(|p| p.get("group_by")).and_then(Value::as_str) else { return Vec::new() };
    let min = params.and_then(|p| p.get("min")).and_then(Value::as_f64).unwrap_or(2.0);
    let Some((source, recs)) = matching(signal, ctx) else { return Vec::new() };
    let mut groups: IndexMap<Cow<str>, Vec<u32>> = IndexMap::new();
    for r in recs {
        groups.entry(group_key(get_path(&r.data, group_by))).or_default().push(r.index);
    }
    groups
        .values()
        .filter(|indices| indices.len() as f64 >= min)
        .map(|indices| Hit {
            records: indices.iter().map(|&i| (source.to_string(), i)).collect(),
            fragments: indices.iter().flat_map(|&i| frags_of(ctx, source, i)).cloned().collect(),
        })
        .collect()
}

/// `<source>#<record index>:<Type>:<mapping index>` -> (source, record index).
/// The first `#` followed by digits and a colon ends the source id.
fn fragment_record(id: &str) -> Option<(&str, u32)> {
    id.match_indices('#').find_map(|(at, _)| {
        let rest = &id[at + 1..];
        let index = rest[..rest.find(':')?].parse().ok()?;
        Some((&id[..at], index))
    })
}

fn eval_co_resolved(signal: &Signal, ctx: &Context) -> Vec<Hit> {
    let params = signal.params.as_ref();
    let Some(otype) = params.and_then(|p| p.get("type")).and_then(Value::as_str) else { return Vec::new() };
    let Some(clauses) = params.and_then(|p| p.get("clauses")).and_then(Value::as_array) else { return Vec::new() };
    let mut hits = Vec::new();
    'clusters: for cluster in ctx.resolution.clusters.values().filter(|c| c.otype == otype) {
        let mut records = Vec::with_capacity(clauses.len());
        for clause in clauses {
            let source = clause.get("source").and_then(Value::as_str);
            let found = cluster.fragments.iter().find_map(|fid| {
                let (fsource, index) = fragment_record(fid)?;
                if Some(fsource) != source {
                    return None;
                }
                let record = ctx.tables.get(fsource)?.get(index)?;
                matches(&record.data, clause.get("where")).then_some((fsource, index))
            });
            match found {
                Some((fsource, index)) => records.push((fsource.to_string(), index)),
                None => continue 'clusters,
            }
        }
        hits.push(Hit { records, fragments: cluster.fragments.clone() });
    }
    hits
}

pub fn evaluate(signal: &Signal, ctx: &Context) -> Vec<Hit> {
    match signal.kind.as_str() {
        "match" => eval_match(signal, ctx),
        "time_window" => eval_time_window(signal, ctx),
        "count" => eval_count(signal, ctx),
        "co_resolved" => eval_co_resolved(signal, ctx),
        _ => Vec::new(),
    }
}

/// A field as a float the way Python's `float()` reads it. NaN counts as non-numeric.
fn as_number(value: &Value) -> Option<f64> {
    let n = match value {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse().ok()?,
        Value::Bool(b) => f64::from(u8::from(*b)),
        _ => return None,
    };
    (!n.is_nan()).then_some(n)
}

/// Strength: a fixed number, or the max of a field over the hit records.
pub fn strength(signal: &Signal, hits: &[Hit], ctx: &Context) -> f64 {
    let field = match &signal.strength {
        Strength::Fixed(n) => return *n,
        Strength::Field { field } => field,
    };
    hits.iter()
        .flat_map(|h| &h.records)
        .filter_map(|(source, index)| ctx.tables.get(source)?.get(*index))
        .filter_map(|r| as_number(get_path(&r.data, field)?))
        .fold(None, |best: Option<f64>, n| Some(best.map_or(n, |b| b.max(n))))
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_is_half_open_and_wraps_midnight() {
        assert!(in_window(9.0, 9.0, 17.0));
        assert!(!in_window(17.0, 9.0, 17.0));
        assert!(in_window(23.5, 22.0, 6.0));
        assert!(in_window(0.0, 22.0, 6.0));
        assert!(!in_window(6.0, 22.0, 6.0));
        assert!(!in_window(12.0, 22.0, 6.0));
    }

    #[test]
    fn hour_of_day_ignores_seconds_and_handles_times_before_the_epoch() {
        // 01:58:59.999
        let t = (3600 + 58 * 60 + 59) * 1000 + 999;
        assert_eq!(hour_of_day(t), 1.0 + 58.0 / 60.0);
        // One minute before 1970 is 23:59.
        assert_eq!(hour_of_day(-60_000), 23.0 + 59.0 / 60.0);
    }

    #[test]
    fn fragment_ids_split_into_source_and_record() {
        assert_eq!(fragment_record("orders#107:Seller:1"), Some(("orders", 107)));
        assert_eq!(fragment_record("a#b#3:T:0"), Some(("a#b", 3)));
        assert_eq!(fragment_record("no-marker"), None);
    }

    #[test]
    fn record_key_finds_tuple_keys() {
        let mut m: IndexMap<(String, u32), Vec<String>> = IndexMap::new();
        m.insert(("orders".into(), 7), vec!["f".into()]);
        assert!(m.get(&RecordKey("orders", 7)).is_some());
        assert!(m.get(&RecordKey("orders", 8)).is_none());
    }

    #[test]
    fn numbers_read_as_python_floats() {
        use serde_json::json;
        assert_eq!(as_number(&json!(0.5)), Some(0.5));
        assert_eq!(as_number(&json!(" 0.5 ")), Some(0.5));
        assert_eq!(as_number(&json!(true)), Some(1.0));
        assert_eq!(as_number(&json!("high")), None);
        assert_eq!(as_number(&json!("nan")), None);
        assert_eq!(as_number(&Value::Null), None);
        assert_eq!(as_number(&json!([1])), None);
    }

    #[test]
    fn group_keys_use_the_string_form() {
        use serde_json::json;
        assert_eq!(group_key(Some(&json!("a"))), "a");
        assert_eq!(group_key(Some(&json!(7))), "7");
        assert_eq!(group_key(Some(&Value::Null)), "None");
        assert_eq!(group_key(None), "None");
    }
}

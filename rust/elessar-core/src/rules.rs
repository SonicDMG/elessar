//! Signal rules (spec §5). Phase 2.
//!
//! Built against `golden/board/<pack>.json` (per-signal hits as record
//! pointers, activations and link weights dumped from the Python engine).

use crate::pack::Signal;
use crate::resolve::Resolution;
use crate::store::Table;
use indexmap::IndexMap;

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

pub fn evaluate(signal: &Signal, ctx: &Context) -> Vec<Hit> {
    let _ = (signal, ctx);
    todo!("phase 2: rules")
}

/// Strength: a fixed number, or the max of a field over the hit records.
pub fn strength(signal: &Signal, hits: &[Hit], ctx: &Context) -> f64 {
    let _ = (signal, hits, ctx);
    todo!("phase 2: strength")
}

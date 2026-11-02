//! Entity resolution (spec §3, §4). Phase 1c.
//!
//! Built against `golden/clusters/<pack>.json` (clusters, titles, conflicts
//! and provenance pointers dumped from the Python engine, which must match)
//! and the proposals pinned by each pack's `expect.json` (scores may differ:
//! Rust uses Jaro-Winkler, see the spec).

use crate::pack::ObjectType;
use crate::pipeline::Fragment;
use crate::types::{PropValue, Proposal};
use indexmap::IndexMap;
use std::collections::HashSet;

/// `norm`: lowercase, keep `[0-9a-z@._ -]`, collapse whitespace, trim.
pub fn norm(value: &str) -> String {
    let _ = value;
    todo!("phase 1c: norm")
}

/// `fuzzy_norm`: lowercase, non `[0-9a-z ]` to space, collapse, trim.
pub fn fuzzy_norm(value: &str) -> String {
    let _ = value;
    todo!("phase 1c: fuzzy_norm")
}

/// Jaro-Winkler similarity on normalised strings (prefix scale 0.1, max prefix 4).
pub fn similarity(a: &str, b: &str) -> f64 {
    let _ = (a, b);
    todo!("phase 1c: similarity")
}

#[derive(Debug, Clone)]
pub struct Cluster {
    /// The smallest fragment id in the cluster (string order).
    pub id: String,
    pub otype: String,
    /// Fragment ids in source-rank-then-id order.
    pub fragments: Vec<String>,
    /// Sources in rack order.
    pub sources: Vec<String>,
    pub props: IndexMap<String, Vec<PropValue>>,
    pub title: Option<String>,
    pub conflicts: Vec<String>,
}

pub struct Resolution {
    pub clusters: IndexMap<String, Cluster>,
    /// fragment id -> cluster id
    pub cluster_of: IndexMap<String, String>,
}

/// Merge fragments by shared key values and committed pairs (spec §4).
/// `source_rank` is rack order. Excerpts for provenance are filled by the caller.
pub fn resolve(fragments: &[Fragment], object_types: &IndexMap<String, ObjectType>, committed: &[(String, String)], source_rank: &[String]) -> Resolution {
    let _ = (fragments, object_types, committed, source_rank);
    todo!("phase 1c: resolve")
}

/// Fuzzy merge proposals between clusters of the same type, excluding rejected ids.
pub fn proposals(res: &Resolution, fragments: &IndexMap<String, Fragment>, object_types: &IndexMap<String, ObjectType>, rejected: &HashSet<String>) -> Vec<Proposal> {
    let _ = (res, fragments, object_types, rejected);
    todo!("phase 1c: proposals")
}

/// The cluster a participant anchors to: the first cluster of its type with a
/// property whose `norm` equals the anchor value.
pub fn anchor<'a>(res: &'a Resolution, otype: &str, prop: &str, value: &str) -> Option<&'a Cluster> {
    let _ = (res, otype, prop, value);
    todo!("phase 1c: anchor")
}

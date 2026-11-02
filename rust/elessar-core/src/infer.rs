//! Noisy-OR with inhibitors and competition (spec §6). Phase 2.

use crate::types::Breakdown;
use indexmap::IndexMap;

pub const FEEDBACK_BETA: f64 = 0.15;

#[derive(Debug, Clone)]
pub struct Parent {
    pub from: String,
    pub w: f64,
}

#[derive(Debug, Clone)]
pub struct ParticipantIn {
    pub leak: f64,
    pub present: bool,
    pub parents: Vec<Parent>,
}

#[derive(Debug, Clone)]
pub struct EdgeIn {
    pub from: String,
    pub to: String,
    pub w: f64,
    pub kind: String,
}

pub struct Inference {
    pub p: IndexMap<String, f64>,
    pub breakdown: IndexMap<String, Option<Breakdown>>,
    pub stages: Vec<IndexMap<String, f64>>,
}

/// Products left to right in `f64`, in the orders the spec fixes.
pub fn infer(signals: &IndexMap<String, f64>, participants: &IndexMap<String, ParticipantIn>, priors: &IndexMap<String, f64>, edges: &[EdgeIn], feedback: bool) -> Inference {
    let _ = (signals, participants, priors, edges, feedback);
    todo!("phase 2: infer")
}

//! The JSON shapes the frontend reads (spec §12). Field names and order are
//! the contract; keep them in sync with `docs/engine-spec.md`.

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct Board {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScenarioView {
    pub id: String,
    pub title: String,
    pub premise: String,
    pub board: Board,
    pub narrative: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceView {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub format: String,
    pub transport: String,
    pub color: String,
    pub confidence: f64,
    pub blurb: String,
    pub connected: bool,
    pub latency_ms: Option<u64>,
    pub bytes: Option<usize>,
    pub error: Option<String>,
    pub records: usize,
    pub fragments: usize,
    pub skipped: usize,
    pub skipped_examples: Vec<crate::store::Skipped>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Provenance {
    pub source: String,
    pub pointer: String,
    pub field: String,
    pub fragment: String,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PropValue {
    pub value: Value,
    pub provenance: Vec<Provenance>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Term {
    pub from: String,
    pub w: f64,
    pub p: f64,
    pub term: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Inhibitor {
    pub from: String,
    pub w: f64,
    pub p: f64,
    pub factor: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Breakdown {
    pub leak: f64,
    pub support: Vec<Term>,
    pub inhibit: Vec<Inhibitor>,
    pub support_p: f64,
    pub inhibit_factor: f64,
    pub raw: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub divisor: Option<f64>,
    pub r#final: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HitRecord {
    pub source: String,
    pub pointer: String,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HitView {
    pub records: Vec<HitRecord>,
    pub participants: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LinkView {
    pub participant: String,
    pub hits: usize,
    pub w: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct JevRecordView {
    pub index: u32,
    pub source: String,
    pub pointer: String,
    pub excerpt: String,
    pub noul: Option<f64>,
    pub error: Option<String>,
    pub model: Option<String>,
    pub rule_hit: bool,
    pub jev_hit: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct JevView {
    pub question: String,
    pub criteria: Option<Value>,
    pub r#where: Option<Value>,
    pub threshold: f64,
    pub records: Vec<JevRecordView>,
    pub complete: bool,
    pub asked: bool,
    pub strength: f64,
    pub hit_count: usize,
    pub disagreements: usize,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SignalDetail {
    pub status: String,
    pub sources: Vec<String>,
    pub kind: String,
    pub rule: Value,
    pub explain: String,
    pub confidence: f64,
    pub strength: f64,
    pub engine_used: String,
    pub rule_strength: f64,
    pub rule_hit_count: usize,
    pub jev: Option<JevView>,
    pub judgement: String,
    pub hits: Vec<HitView>,
    pub links: Vec<LinkView>,
    pub implicates: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParticipantDetail {
    pub r#type: String,
    pub anchor: IndexMap<String, Value>,
    pub breakdown: Option<Breakdown>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub props: Option<IndexMap<String, Vec<PropValue>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflicts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fragment_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merges: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HypothesisDetail {
    pub blurb: String,
    pub prior: f64,
    pub breakdown: Breakdown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Detail {
    Signal(SignalDetail),
    Participant(ParticipantDetail),
    Hypothesis(HypothesisDetail),
}

#[derive(Debug, Clone, Serialize)]
pub struct Offboard {
    pub kind: String,
    pub restore: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<String>>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub id: String,
    pub code: String,
    pub band: String,
    pub label: String,
    pub col: u32,
    pub base_row: u32,
    pub p: f64,
    pub step: u8,
    pub present: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept_reason: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offboard: Option<Option<Offboard>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crowned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leading: Option<bool>,
    pub detail: Detail,
}

#[derive(Debug, Clone, Serialize)]
pub struct EdgeView {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub w: f64,
    pub flow: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MergeSide {
    pub cluster: String,
    pub property: String,
    pub value: Value,
    pub source: String,
    pub fragment: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub id: String,
    pub r#type: String,
    pub score: f64,
    pub a: MergeSide,
    pub b: MergeSide,
    pub conflict: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommittedMerge {
    pub id: String,
    pub active: bool,
    pub a: Option<Value>,
    pub b: Option<Value>,
    pub a_source: String,
    pub b_source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Merges {
    pub pending: Vec<Proposal>,
    pub committed: Vec<CommittedMerge>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub id: String,
    pub source: String,
    pub pointer: String,
    pub excerpt: String,
    pub participants: Vec<String>,
    pub signals: Vec<String>,
    pub severity: f64,
    pub acked: bool,
    pub at: String,
    pub t: i64,
    pub dated_by: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Undated {
    pub id: String,
    pub source: String,
    pub pointer: String,
    pub excerpt: String,
    pub participants: Vec<String>,
    pub signals: Vec<String>,
    pub severity: f64,
    pub acked: bool,
    pub why: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordRef {
    pub source: String,
    pub pointer: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Link {
    pub a: String,
    pub b: String,
    pub records: Vec<RecordRef>,
    pub weight: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct JevStats {
    pub available: bool,
    pub model: String,
    pub calls: u64,
    pub cached: u64,
    pub errors: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CrownReason {
    pub from: String,
    pub term: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Crown {
    pub hypothesis: String,
    pub p: f64,
    pub verdict: String,
    pub at: String,
    pub reasons: Vec<CrownReason>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub at: String,
    pub action: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Thresholds {
    pub steps: [f64; 2],
    pub crownable: f64,
    pub capture_p: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Insight {
    pub kind: String,
    pub title: String,
    pub text: String,
    pub points: u32,
}

/// `GET /api/state`. `agent` is added by the server crate.
#[derive(Debug, Clone, Serialize)]
pub struct State {
    pub scenario: ScenarioView,
    pub sources: Vec<SourceView>,
    pub nodes: Vec<Node>,
    pub edges: Vec<EdgeView>,
    pub stages: Vec<IndexMap<String, f64>>,
    pub merges: Merges,
    pub events: Vec<Event>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events_truncated: Option<bool>,
    pub undated: Vec<Undated>,
    pub unknown_time: String,
    pub links: Vec<Link>,
    pub object_types: Value,
    pub feedback: bool,
    pub engine: String,
    pub jev: JevStats,
    pub crown: Option<Crown>,
    pub score: u32,
    pub earned: Vec<String>,
    pub audit: Vec<AuditEntry>,
    pub thresholds: Thresholds,
}

pub const STEP_AT: [f64; 2] = [0.33, 0.66];
pub const CROWNABLE: f64 = 0.85;
pub const CAPTURE_P: f64 = 0.08;
pub const CAPTURE_INHIBIT: f64 = 0.5;
pub const MAX_EVENTS_V1: usize = 5000;

pub fn step_for(p: f64) -> u8 {
    STEP_AT.iter().filter(|t| p >= **t).count() as u8
}

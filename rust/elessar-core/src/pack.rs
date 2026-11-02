//! Scenario packs: load, validate, lay out (spec §1, §7).
//!
//! `load_pack` performs every structural check; the checks that need the
//! source files parsed (`pipeline::check_sources`) run afterwards and only
//! add warnings or a parse error.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub const FORMATS: &[&str] = &["csv", "jsonl", "nested-json", "xml", "columnar", "kv"];
pub const TRANSFORMS: &[&str] = &["lower", "upper", "strip", "title", "int", "float", "name_last_first", "date_dmy", "epoch_ms", "epoch_s"];
pub const OPERATORS: &[&str] = &["exists", "eq", "ne", "gt", "gte", "lt", "lte", "in", "nin", "contains", "startswith"];
pub const RULE_KINDS: &[&str] = &["match", "time_window", "count", "co_resolved"];
pub const TIME_UNITS: &[&str] = &["ms", "s", "iso", "date_dmy"];
pub const UNKNOWN_TIME: &[&str] = &["throughout", "at_connect", "after"];
pub const MAX_BAND: usize = 9;
pub const BOARD_HEIGHT: u32 = 9;
pub const PALETTE: &[&str] = &["#7ee787", "#f0883e", "#58a6ff", "#d2a8ff", "#f778ba", "#e3b341", "#56d4dd", "#ff7b72"];

#[derive(Debug, thiserror::Error)]
#[error("{dir}: {} error(s)", errors.len())]
pub struct PackError {
    pub dir: String,
    pub errors: Vec<String>,
}

// ------------------------------------------------------------------ manifest

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Fields {
    One(String),
    Many(Vec<String>),
}

impl Fields {
    pub fn list(&self) -> Vec<&str> {
        match self {
            Fields::One(s) => vec![s.as_str()],
            Fields::Many(v) => v.iter().map(|s| s.as_str()).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentTime {
    pub constant: Option<String>,
    pub regex: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeConfig {
    #[serde(default)]
    pub field: Option<Fields>,
    #[serde(default)]
    pub document: Option<DocumentTime>,
    #[serde(default = "default_unit")]
    pub unit: String,
}

fn default_unit() -> String {
    "iso".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    pub format: String,
    #[serde(default = "default_transport")]
    pub transport: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub options: Option<Value>,
    #[serde(default)]
    pub time: Option<TimeConfig>,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    #[serde(default = "default_latency")]
    pub latency_ms: u64,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub blurb: String,
    /// Absolute path for file sources, filled in by the loader.
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

fn default_transport() -> String {
    "file".into()
}
fn default_confidence() -> f64 {
    0.9
}
fn default_latency() -> u64 {
    150
}

#[derive(Debug, Clone, Deserialize)]
struct Manifest {
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    premise: Option<String>,
    #[serde(default)]
    unknown_time: Option<String>,
    #[serde(default)]
    sources: Vec<Value>,
}

// ------------------------------------------------------------------ mapping

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fuzzy {
    pub properties: Vec<String>,
    #[serde(default = "default_threshold")]
    pub threshold: f64,
}

fn default_threshold() -> f64 {
    0.86
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectType {
    pub title: Fields,
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub multi: Vec<String>,
    #[serde(default)]
    pub fuzzy: Option<Fuzzy>,
}

/// A property spec: a field path, or `{"from": path, "transform": name | [names]}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PropSpec {
    Path(String),
    Detailed {
        from: String,
        #[serde(default)]
        transform: Option<Fields>,
    },
}

impl PropSpec {
    pub fn field(&self) -> &str {
        match self {
            PropSpec::Path(p) => p,
            PropSpec::Detailed { from, .. } => from,
        }
    }
    pub fn transforms(&self) -> Vec<&str> {
        match self {
            PropSpec::Path(_) => vec![],
            PropSpec::Detailed { transform, .. } => transform.as_ref().map(|t| t.list()).unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mapping {
    pub source: String,
    #[serde(rename = "type")]
    pub otype: String,
    #[serde(default)]
    pub when: Option<Value>,
    pub props: IndexMap<String, PropSpec>,
}

#[derive(Debug, Clone, Deserialize)]
struct MappingFile {
    #[serde(default)]
    object_types: IndexMap<String, ObjectType>,
    #[serde(default)]
    mappings: Vec<Mapping>,
}

// ------------------------------------------------------------------ signals

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Strength {
    Fixed(f64),
    Field { field: String },
}

impl Default for Strength {
    fn default() -> Self {
        Strength::Fixed(1.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Implicates {
    #[serde(default)]
    pub types: Option<Vec<String>>,
    #[serde(default = "default_imp_w")]
    pub w: f64,
}

fn default_imp_w() -> f64 {
    0.5
}

impl Default for Implicates {
    fn default() -> Self {
        Implicates { types: None, w: 0.5 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JevSpec {
    pub question: String,
    #[serde(default)]
    pub r#where: Option<Value>,
    #[serde(default)]
    pub criteria: Option<Value>,
    #[serde(default = "default_jev_threshold")]
    pub threshold: f64,
}

fn default_jev_threshold() -> f64 {
    0.5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub r#where: Option<Value>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub strength: Strength,
    #[serde(default)]
    pub implicates: Implicates,
    #[serde(default)]
    pub explain: String,
    #[serde(default)]
    pub jev: Option<JevSpec>,
}

impl Signal {
    /// Sources that must be connected before the signal can be evaluated.
    pub fn required_sources(&self) -> Vec<String> {
        if self.kind == "co_resolved" {
            let mut v: Vec<String> = self
                .params
                .as_ref()
                .and_then(|p| p.get("clauses"))
                .and_then(|c| c.as_array())
                .map(|cs| cs.iter().filter_map(|c| c.get("source").and_then(|s| s.as_str()).map(String::from)).collect())
                .unwrap_or_default();
            v.sort();
            v.dedup();
            v
        } else {
            self.source.clone().into_iter().collect()
        }
    }
}

// ------------------------------------------------------------------ model

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Participant {
    pub id: String,
    #[serde(rename = "type")]
    pub otype: String,
    pub anchor: IndexMap<String, Value>,
    #[serde(default = "default_leak")]
    pub leak: f64,
    #[serde(default)]
    pub label: Option<String>,
}

fn default_leak() -> f64 {
    0.02
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hypothesis {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    pub prior: f64,
    #[serde(default)]
    pub blurb: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RawEdge {
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    from_type: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    w: Option<f64>,
    #[serde(default = "default_edge_kind")]
    kind: String,
}

fn default_edge_kind() -> String {
    "support".into()
}

/// An edge after `from_type` expansion: always from a signal or participant id to a hypothesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub w: f64,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelFile {
    #[serde(default)]
    participants: Vec<Participant>,
    #[serde(default)]
    entities: Option<Value>,
    #[serde(default)]
    hypotheses: Vec<Hypothesis>,
    #[serde(default)]
    edges: Vec<RawEdge>,
}

// ------------------------------------------------------------------ layout

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutNode {
    pub id: String,
    pub code: String,
    pub band: String,
    pub label: String,
    pub col: u32,
    pub base_row: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub nodes: Vec<LayoutNode>,
}

fn spread(n: usize, m: usize) -> Vec<usize> {
    (0..n).map(|i| ((i as f64 + 0.5) * m as f64 / n as f64) as usize).collect()
}

pub fn layout(signals: &[Signal], participants: &[Participant], hypotheses: &[Hypothesis]) -> Layout {
    let widest = signals.len().max(participants.len()).max(hypotheses.len());
    let width = (2 * widest + 1) as u32;
    let odd: Vec<u32> = (1..width.saturating_sub(1)).step_by(2).collect();
    let even: Vec<u32> = (0..width.saturating_sub(2)).step_by(2).collect();
    let mut nodes = Vec::new();
    let mut place = |band: &str, items: Vec<(String, String)>, prefix: &str, cols: &[u32], base_row: u32| {
        let slots = spread(items.len(), cols.len());
        for (i, ((id, label), slot)) in items.into_iter().zip(slots).enumerate() {
            nodes.push(LayoutNode { id, code: format!("{}{}", prefix, i + 1), band: band.into(), label, col: cols[slot], base_row });
        }
    };
    place("hypothesis", hypotheses.iter().map(|h| (h.id.clone(), h.label.clone().unwrap_or(h.id.clone()))).collect(), "H", &odd, 2);
    place("participant", participants.iter().map(|p| (p.id.clone(), p.label.clone().unwrap_or(p.id.clone()))).collect(), "P", &even, 5);
    place("signal", signals.iter().map(|s| (s.id.clone(), s.label.clone().unwrap_or(s.id.clone()))).collect(), "S", &odd, 8);
    Layout { width, height: BOARD_HEIGHT, nodes }
}

// ------------------------------------------------------------------ the pack

#[derive(Debug, Clone, Serialize)]
pub struct Pack {
    pub id: String,
    pub dir: PathBuf,
    pub title: String,
    pub premise: String,
    pub unknown_time: String,
    pub sources: IndexMap<String, Source>,
    pub source_order: Vec<String>,
    pub object_types: IndexMap<String, ObjectType>,
    pub mappings: Vec<Mapping>,
    pub signals: Vec<Signal>,
    pub participants: Vec<Participant>,
    pub hypotheses: Vec<Hypothesis>,
    pub edges: Vec<Edge>,
    pub narrative: Value,
    pub layout: Layout,
    pub warnings: Vec<String>,
}

impl Pack {
    pub fn source(&self, id: &str) -> Option<&Source> {
        self.sources.get(id)
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(dir: &Path, name: &str, errors: &mut Vec<String>) -> Option<T> {
    let path = dir.join(name);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            errors.push(format!("missing {name}"));
            return None;
        }
    };
    match serde_json::from_str::<Value>(&text) {
        Err(e) => {
            errors.push(format!("{name}: invalid JSON at line {}: {}", e.line(), e));
            None
        }
        Ok(v) => match serde_json::from_value::<T>(v) {
            Ok(t) => Some(t),
            Err(e) => {
                errors.push(format!("{name}: {e}"));
                None
            }
        },
    }
}

/// Validate a `where`/`when` object: every operator object may use only known operators.
pub fn check_where(where_: Option<&Value>, ctx: &str, errors: &mut Vec<String>) {
    let Some(Value::Object(map)) = where_ else { return };
    for (field, cond) in map {
        if let Value::Object(ops) = cond {
            let bad: Vec<&String> = ops.keys().filter(|k| !OPERATORS.contains(&k.as_str())).collect();
            if !bad.is_empty() {
                errors.push(format!("{ctx}: unknown operator(s) {bad:?} on '{field}'"));
            }
        }
    }
}

pub fn load_pack(dir: &Path) -> Result<Pack, PackError> {
    let dir_s = dir.display().to_string();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let manifest: Option<Manifest> = read_json(dir, "manifest.json", &mut errors);
    let mapping: Option<MappingFile> = read_json(dir, "mapping.json", &mut errors);
    let signals: Option<Vec<Signal>> = read_json(dir, "signals.json", &mut errors);
    let model: Option<ModelFile> = read_json(dir, "model.json", &mut errors);
    let narrative: Option<Value> = read_json(dir, "narrative.json", &mut errors);
    if !errors.is_empty() {
        return Err(PackError { dir: dir_s, errors });
    }
    let (manifest, mapping, signals, model, narrative) = (manifest.unwrap(), mapping.unwrap(), signals.unwrap(), model.unwrap(), narrative.unwrap());

    // --- manifest
    let pid = manifest.id.clone().unwrap_or_else(|| dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    let mut sources: IndexMap<String, Source> = IndexMap::new();
    for (i, raw) in manifest.sources.iter().enumerate() {
        let sid = raw.get("id").and_then(|v| v.as_str()).map(String::from);
        let Some(sid) = sid else {
            errors.push(format!("manifest: source #{} has no id", i + 1));
            continue;
        };
        let mut s: Source = match serde_json::from_value(raw.clone()) {
            Ok(s) => s,
            Err(e) => {
                errors.push(format!("manifest: source '{sid}': {e}"));
                continue;
            }
        };
        if sources.contains_key(&sid) {
            errors.push(format!("manifest: duplicate source id '{sid}'"));
        }
        if !FORMATS.contains(&s.format.as_str()) {
            errors.push(format!("manifest: source '{sid}' has unknown format '{}' (known: {:?})", s.format, FORMATS));
        }
        match s.transport.as_str() {
            "file" => {
                let path = dir.join(s.file.clone().unwrap_or_default());
                if s.file.is_none() || !path.is_file() {
                    errors.push(format!("manifest: source '{sid}' file not found: {}", s.file.clone().unwrap_or_default()));
                }
                s.path = Some(path);
            }
            "http" => {
                if !s.url.as_deref().map(|u| u.starts_with("http://") || u.starts_with("https://")).unwrap_or(false) {
                    errors.push(format!("manifest: source '{sid}' needs an http(s) url"));
                }
            }
            other => errors.push(format!("manifest: source '{sid}' has unknown transport '{other}'")),
        }
        if !(0.0..=1.0).contains(&s.confidence) {
            errors.push(format!("manifest: source '{sid}' confidence must be 0..1"));
        }
        if let Some(t) = &s.time {
            match (&t.field, &t.document) {
                (Some(_), Some(_)) | (None, None) => errors.push(format!("manifest: source '{sid}' time needs exactly one of 'field' (per record) or 'document' (whole source)")),
                (Some(f), None) => {
                    if f.list().iter().any(|p| p.is_empty()) {
                        errors.push(format!("manifest: source '{sid}' time.field must be a path or a list of paths"));
                    }
                }
                (None, Some(d)) => {
                    let n = [d.constant.is_some(), d.regex.is_some(), d.path.is_some()].iter().filter(|b| **b).count();
                    if n != 1 {
                        errors.push(format!("manifest: source '{sid}' time.document needs exactly one of [\"constant\", \"regex\", \"path\"]"));
                    } else if d.path.is_some() && !["nested-json", "columnar", "xml"].contains(&s.format.as_str()) {
                        errors.push(format!("manifest: source '{sid}' time.document.path works for nested-json, columnar and xml; use regex or constant for {}", s.format));
                    }
                    if let Some(r) = &d.regex {
                        if let Err(e) = regex::Regex::new(r) {
                            errors.push(format!("manifest: source '{sid}' time.document.regex: {e}"));
                        }
                    }
                }
            }
            if !TIME_UNITS.contains(&t.unit.as_str()) {
                errors.push(format!("manifest: source '{sid}' has unknown time.unit '{}' (known: {:?})", t.unit, TIME_UNITS));
            }
        }
        if s.color.is_empty() {
            s.color = PALETTE[i % PALETTE.len()].to_string();
        }
        if s.name.is_empty() {
            s.name = sid.clone();
        }
        sources.insert(sid, s);
    }
    if sources.is_empty() {
        errors.push("manifest: no sources".into());
    }
    let unknown_time = manifest.unknown_time.clone().unwrap_or_else(|| "throughout".into());
    if !UNKNOWN_TIME.contains(&unknown_time.as_str()) {
        errors.push(format!("manifest: unknown_time must be one of {:?} (got '{unknown_time}')", UNKNOWN_TIME));
    }

    // --- mapping
    let types = mapping.object_types;
    for (tname, t) in &types {
        if let Some(f) = &t.fuzzy {
            if f.properties.is_empty() {
                errors.push(format!("mapping: object type '{tname}' fuzzy config needs properties"));
            }
        }
    }
    let mut mapped: IndexMap<&str, HashSet<String>> = types.keys().map(|t| (t.as_str(), HashSet::new())).collect();
    for (mi, m) in mapping.mappings.iter().enumerate() {
        let ctx = format!("mapping #{} ({} -> {})", mi + 1, m.source, m.otype);
        if !sources.contains_key(&m.source) {
            errors.push(format!("{ctx}: unknown source"));
        }
        if !types.contains_key(&m.otype) {
            errors.push(format!("{ctx}: unknown object type"));
        }
        if m.props.is_empty() {
            errors.push(format!("{ctx}: no props"));
        }
        check_where(m.when.as_ref(), &ctx, &mut errors);
        for (prop, spec) in &m.props {
            for tf in spec.transforms() {
                if !TRANSFORMS.contains(&tf) {
                    errors.push(format!("{ctx}: unknown transform '{tf}' on '{prop}' (known: {:?})", TRANSFORMS));
                }
            }
            if let Some(set) = mapped.get_mut(m.otype.as_str()) {
                set.insert(prop.clone());
            }
        }
    }
    for (tname, t) in &types {
        for key in &t.keys {
            if !mapped.get(tname.as_str()).map(|s| s.contains(key)).unwrap_or(false) {
                warnings.push(format!("mapping: key '{key}' on '{tname}' is never mapped"));
            }
        }
    }

    // --- signals
    let mut sig_ids = HashSet::new();
    for s in &signals {
        let ctx = format!("signal '{}'", s.id);
        if s.id.is_empty() || !sig_ids.insert(s.id.clone()) {
            errors.push(format!("signals: missing or duplicate id '{}'", s.id));
        }
        if !RULE_KINDS.contains(&s.kind.as_str()) {
            errors.push(format!("{ctx}: unknown kind '{}' (known: {:?})", s.kind, RULE_KINDS));
            continue;
        }
        for src in s.required_sources() {
            if !sources.contains_key(&src) {
                errors.push(format!("{ctx}: unknown source '{src}'"));
            }
        }
        if s.kind != "co_resolved" && s.source.is_none() {
            errors.push(format!("{ctx}: needs a source"));
        }
        check_where(s.r#where.as_ref(), &ctx, &mut errors);
        if s.kind == "co_resolved" {
            let ptype = s.params.as_ref().and_then(|p| p.get("type")).and_then(|t| t.as_str());
            if !ptype.map(|t| types.contains_key(t)).unwrap_or(false) {
                errors.push(format!("{ctx}: co_resolved needs a known params.type"));
            }
            if let Some(cs) = s.params.as_ref().and_then(|p| p.get("clauses")).and_then(|c| c.as_array()) {
                for c in cs {
                    check_where(c.get("where"), &ctx, &mut errors);
                }
            }
        }
        if s.kind == "time_window" {
            let hours = s.params.as_ref().and_then(|p| p.get("hours")).and_then(|h| h.as_array()).map(|h| h.len()).unwrap_or(0);
            if hours != 2 {
                errors.push(format!("{ctx}: time_window needs params.hours [start, end]"));
            }
        }
        if let Some(j) = &s.jev {
            if s.kind != "match" && s.kind != "time_window" {
                errors.push(format!("{ctx}: a jev block works on match and time_window signals only"));
            }
            if j.question.trim().is_empty() {
                errors.push(format!("{ctx}: jev needs a question"));
            } else if !(0.0..=1.0).contains(&j.threshold) {
                errors.push(format!("{ctx}: jev.threshold must be 0..1"));
            } else {
                check_where(j.r#where.as_ref(), &format!("{ctx} jev"), &mut errors);
            }
        }
        if let Some(ts) = &s.implicates.types {
            for t in ts {
                if !types.contains_key(t) {
                    errors.push(format!("{ctx}: implicates unknown type '{t}'"));
                }
            }
        }
        if !(0.0..=1.0).contains(&s.implicates.w) {
            errors.push(format!("{ctx}: implicates.w must be 0..1"));
        }
    }

    // --- model
    if model.entities.is_some() {
        errors.push("model: 'entities' was renamed to 'participants'; rename the key".into());
    }
    let parts = model.participants;
    let hyps = model.hypotheses;
    let part_ids: HashSet<&str> = parts.iter().map(|p| p.id.as_str()).collect();
    let hyp_ids: HashSet<&str> = hyps.iter().map(|h| h.id.as_str()).collect();
    for p in &parts {
        if !types.contains_key(&p.otype) {
            errors.push(format!("model: participant '{}' has unknown type '{}'", p.id, p.otype));
        }
        if p.anchor.len() != 1 {
            errors.push(format!("model: participant '{}' needs exactly one anchor property", p.id));
        }
    }
    for h in &hyps {
        if !(0.0..=1.0).contains(&h.prior) {
            errors.push(format!("model: hypothesis '{}' needs a prior in 0..1", h.id));
        }
    }
    let all_ids: Vec<&str> = signals.iter().map(|s| s.id.as_str()).chain(parts.iter().map(|p| p.id.as_str())).chain(hyps.iter().map(|h| h.id.as_str())).collect();
    let mut dupes: Vec<&str> = all_ids.iter().filter(|x| all_ids.iter().filter(|y| y == x).count() > 1).cloned().collect();
    dupes.sort();
    dupes.dedup();
    if !dupes.is_empty() {
        errors.push(format!("model: ids must be unique across signals, participants and hypotheses: {dupes:?}"));
    }
    for (band, n) in [("signals", signals.len()), ("participants", parts.len()), ("hypotheses", hyps.len())] {
        if n > MAX_BAND {
            errors.push(format!("model: at most {MAX_BAND} {band} (got {n})"));
        }
    }
    if hyps.is_empty() {
        errors.push("model: at least one hypothesis".into());
    }
    let mut edges = Vec::new();
    for (i, e) in model.edges.iter().enumerate() {
        let ctx = format!("model edge #{}", i + 1);
        let to = e.to.clone().unwrap_or_default();
        if !hyp_ids.contains(to.as_str()) {
            errors.push(format!("{ctx}: 'to' must be a hypothesis (got '{to}'); signal->participant links come from signals.json implicates"));
        }
        if e.kind != "support" && e.kind != "inhibit" {
            errors.push(format!("{ctx}: kind must be support or inhibit"));
        }
        let w = e.w.unwrap_or(-1.0);
        if !(0.0..=1.0).contains(&w) {
            errors.push(format!("{ctx}: w must be 0..1"));
        }
        if let Some(ft) = &e.from_type {
            if !types.contains_key(ft) {
                errors.push(format!("{ctx}: unknown from_type '{ft}'"));
            }
            for p in parts.iter().filter(|p| &p.otype == ft) {
                edges.push(Edge { from: p.id.clone(), to: to.clone(), w, kind: e.kind.clone(), via_type: Some(ft.clone()) });
            }
        } else if let Some(f) = &e.from {
            if sig_ids.contains(f) || part_ids.contains(f.as_str()) {
                edges.push(Edge { from: f.clone(), to: to.clone(), w, kind: e.kind.clone(), via_type: None });
            } else {
                errors.push(format!("{ctx}: 'from' must be a signal or participant id (got '{f}')"));
            }
        } else {
            errors.push(format!("{ctx}: 'from' must be a signal or participant id (got '')"));
        }
    }
    if !errors.is_empty() {
        return Err(PackError { dir: dir_s, errors });
    }
    let source_order = sources.keys().cloned().collect();
    let layout = layout(&signals, &parts, &hyps);
    Ok(Pack {
        id: pid,
        dir: dir.to_path_buf(),
        title: manifest.title.unwrap_or_default(),
        premise: manifest.premise.unwrap_or_default(),
        unknown_time,
        sources,
        source_order,
        object_types: types,
        mappings: mapping.mappings,
        signals,
        participants: parts,
        hypotheses: hyps,
        edges,
        narrative,
        layout,
        warnings,
    })
}

/// Every pack under `root`: loaded packs by id, and failures by directory name.
pub fn discover(root: &Path) -> (IndexMap<String, Pack>, IndexMap<String, Vec<String>>) {
    let mut packs = IndexMap::new();
    let mut failures = IndexMap::new();
    let mut names: Vec<PathBuf> = fs::read_dir(root).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default();
    names.sort();
    for path in names {
        if !path.is_dir() || !path.join("manifest.json").is_file() {
            continue;
        }
        let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match load_pack(&path) {
            Ok(p) => {
                packs.insert(p.id.clone(), p);
            }
            Err(e) => {
                failures.insert(name, e.errors);
            }
        }
    }
    (packs, failures)
}

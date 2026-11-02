//! The board re-inferred at every event time (spec §9).
//!
//! One sweep over time-sorted records. Fragments join a union-find as their
//! records arrive, so the partition at a slice is the one `resolve` would
//! produce for the records seen so far, and no slice rebuilds it. Rules are
//! evaluated once on the whole tables and each slice only asks which of those
//! hits have arrived; `co_resolved` looks at the clusters of the moment. Slices
//! read session memory and proposals and change neither.

use crate::infer::{self, Parent, ParticipantIn};
use crate::pack::{Signal, Strength};
use crate::pipeline::{self, Fragment};
use crate::resolve::{self, Resolution};
use crate::rules::{self, Context};
use crate::session::{allowed_mask, sorted_participants, Session};
use crate::store::{get_path, string_form};
use crate::time;
use crate::types::{Breakdown, Detail, State};
use indexmap::IndexMap;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// Re-inferences per pack; more distinct times are thinned.
pub const MAX_SLICES: usize = 80;

const NONE: u32 = u32::MAX;

/// `GET /api/timeline` as a JSON value (shape in spec §9), cached per board state.
pub fn timeline(session: &mut Session) -> Value {
    timeline_ref(session).clone()
}

/// The cached timeline without a copy, for a server that serialises it.
pub fn timeline_ref(session: &mut Session) -> &Value {
    let version = session.version;
    if session.timeline_cache.as_ref().is_none_or(|(v, _)| *v != version) {
        session.timeline_cache = Some((version, build(session)));
    }
    &session.timeline_cache.as_ref().expect("filled above").1
}

/// `round(x, 4)`: decimal rounding of the exact binary value, as Python's is.
fn round4(x: f64) -> f64 {
    format!("{x:.4}").parse().unwrap_or(x)
}

/// A slice's hypothesis entry: the derivation terms worth showing.
fn hypothesis_entry(p: f64, status: &str, present: bool, b: &Breakdown) -> Value {
    let support: Vec<Value> = b.support.iter().filter(|x| x.term > 0.001).map(|x| json!({"from": x.from, "w": round4(x.w), "p": round4(x.p), "term": round4(x.term)})).collect();
    let inhibit: Vec<Value> = b.inhibit.iter().filter(|x| x.w * x.p > 0.001).map(|x| json!({"from": x.from, "w": round4(x.w), "p": round4(x.p), "cut": round4(x.w * x.p)})).collect();
    json!({"p": round4(p), "status": status, "present": present, "support": support, "inhibit": inhibit, "raw": round4(b.raw), "divisor": round4(b.divisor.unwrap_or(1.0))})
}

/// The compact view of a whole board, for `final`.
fn slice_of_state(st: &State) -> Value {
    let (mut signals, mut participants, mut hypotheses) = (Map::new(), Map::new(), Map::new());
    for n in &st.nodes {
        match &n.detail {
            Detail::Signal(_) => {
                signals.insert(n.id.clone(), json!({"p": round4(n.p), "status": n.status}));
            }
            Detail::Participant(_) => {
                participants.insert(n.id.clone(), json!({"p": round4(n.p), "status": n.status, "present": n.present}));
            }
            Detail::Hypothesis(d) => {
                hypotheses.insert(n.id.clone(), hypothesis_entry(n.p, &n.status, n.present, &d.breakdown));
            }
        }
    }
    json!({"signals": signals, "participants": participants, "hypotheses": hypotheses})
}

/// Fragments that have arrived, partitioned into clusters. Sets are threaded
/// as circular lists through `next`, so a cluster's members can be walked.
struct Clusters {
    parent: Vec<u32>,
    next: Vec<u32>,
    /// On a root: the lowest fragment position in the set. `resolve` lists
    /// clusters in this order, and a participant anchors to the first that fits.
    first: Vec<u32>,
    /// Interned key values (type, key, norm) of each fragment, and the first
    /// fragment seen with each.
    keys: Vec<Vec<u32>>,
    key_first: Vec<u32>,
    /// Other ends of the committed merges touching each fragment.
    committed: HashMap<u32, Vec<u32>>,
}

impl Clusters {
    fn new(frags: &[Fragment], session: &Session) -> Clusters {
        let mut interned: HashMap<(&str, &str, String), u32> = HashMap::new();
        let mut keys = Vec::with_capacity(frags.len());
        for f in frags {
            let mut of_frag = Vec::new();
            for key in session.pack.object_types.get(&f.otype).map_or(&[][..], |t| &t.keys[..]) {
                if let Some(prop) = f.props.get(key) {
                    let next = interned.len() as u32;
                    of_frag.push(*interned.entry((f.otype.as_str(), key.as_str(), resolve::norm(&string_form(&prop.value)))).or_insert(next));
                }
            }
            keys.push(of_frag);
        }
        let position: HashMap<&str, u32> = frags.iter().enumerate().map(|(i, f)| (f.id.as_str(), i as u32)).collect();
        let mut committed: HashMap<u32, Vec<u32>> = HashMap::new();
        for (a, b) in session.committed_pairs() {
            if let (Some(&a), Some(&b)) = (position.get(a), position.get(b)) {
                committed.entry(a).or_default().push(b);
                committed.entry(b).or_default().push(a);
            }
        }
        let n = frags.len();
        Clusters { parent: vec![NONE; n], next: (0..n as u32).collect(), first: (0..n as u32).collect(), key_first: vec![NONE; interned.len()], keys, committed }
    }

    fn arrived(&self, x: u32) -> bool {
        self.parent[x as usize] != NONE
    }

    fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let up = self.parent[self.parent[x as usize] as usize];
            self.parent[x as usize] = up;
            x = up;
        }
        x
    }

    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        self.parent[rb as usize] = ra;
        self.first[ra as usize] = self.first[ra as usize].min(self.first[rb as usize]);
        self.next.swap(ra as usize, rb as usize);
    }

    fn add(&mut self, x: u32) {
        self.parent[x as usize] = x;
        for k in 0..self.keys[x as usize].len() {
            let key = self.keys[x as usize][k] as usize;
            match self.key_first[key] {
                NONE => self.key_first[key] = x,
                other => self.union(other, x),
            }
        }
        for k in 0..self.committed.get(&x).map_or(0, Vec::len) {
            let other = self.committed[&x][k];
            if self.arrived(other) {
                self.union(x, other);
            }
        }
    }

    fn members(&self, root: u32) -> Members<'_> {
        Members { next: &self.next, at: root, root, done: false }
    }
}

struct Members<'a> {
    next: &'a [u32],
    at: u32,
    root: u32,
    done: bool,
}

impl Iterator for Members<'_> {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        if self.done {
            return None;
        }
        let here = self.at;
        self.at = self.next[here as usize];
        self.done = self.at == self.root;
        Some(here)
    }
}

/// A record a hit came from: its position in the sweep and, for a signal with
/// a field strength, the field's value.
#[derive(Clone, Copy)]
struct HitRec {
    rec: u32,
    value: Option<f64>,
}

/// What a signal can do over the whole data; slices take the arrived part.
enum Plan {
    /// One group per hit. `min` is 1 for per-record rules (`match`,
    /// `time_window`): one arrived record is a hit. For `count`, the group's
    /// size at the moment has to reach it.
    Hits { min: f64, groups: Vec<Vec<HitRec>> },
    CoResolved { full: u64, entry: Vec<u32>, bits: HashMap<u32, u64> },
}

struct JevCandidate {
    rec: HitRec,
    noul: Option<f64>,
}

struct SignalPlan<'a> {
    signal: &'a Signal,
    available: bool,
    factor: f64,
    allowed: u64,
    plan: Plan,
    jev: Option<(f64, Vec<JevCandidate>)>,
}

/// What a signal adds up to at one moment.
#[derive(Default)]
struct Tally {
    hits: usize,
    strength: Option<f64>,
    linked: Vec<usize>,
}

impl Tally {
    fn add(&mut self, parts: u64, values: impl IntoIterator<Item = Option<f64>>) {
        self.hits += 1;
        for v in values.into_iter().flatten() {
            self.strength = Some(self.strength.map_or(v, |s: f64| s.max(v)));
        }
        let mut bits = parts;
        while bits != 0 {
            self.linked[bits.trailing_zeros() as usize] += 1;
            bits &= bits - 1;
        }
    }
}

struct Sweep<'a> {
    session: &'a Session,
    frags: &'a [Fragment],
    clusters: Clusters,
    /// Per record, in rack then file order.
    arrived: Vec<bool>,
    rec_frags: Vec<Vec<u32>>,
    /// Bits of the participants anchored to a root, valid during a slice.
    root_parts: Vec<u64>,
    /// Roots already looked at in the current `co_resolved` evaluation.
    stamp: Vec<u32>,
    visit: u32,
    plans: Vec<SignalPlan<'a>>,
    /// Per participant: fragments whose anchor property has the anchor value.
    candidates: Vec<Vec<u32>>,
    rank: HashMap<&'a str, usize>,
    order: Vec<usize>,
}

impl<'a> Sweep<'a> {
    fn new(session: &'a Session, offsets: &HashMap<&str, u32>, total_records: usize) -> Sweep<'a> {
        let frags = &session.fragments[..];
        let pack = &session.pack;
        let mut rec_frags = vec![Vec::new(); total_records];
        for (i, f) in frags.iter().enumerate() {
            rec_frags[(offsets[f.source.as_str()] + f.record) as usize].push(i as u32);
        }
        let candidates = pack
            .participants
            .iter()
            .map(|e| {
                let Some((prop, value)) = e.anchor.iter().next() else { return Vec::new() };
                let want = resolve::norm(&string_form(value));
                frags.iter().enumerate().filter(|(_, f)| f.otype == e.otype && f.props.get(prop).is_some_and(|v| resolve::norm(&string_form(&v.value)) == want)).map(|(i, _)| i as u32).collect()
            })
            .collect();
        let empty = Resolution { clusters: IndexMap::new(), cluster_of: IndexMap::new() };
        let ctx = Context { tables: &session.tables, frags_by_record: &session.frags_by_record, resolution: &empty };
        let plans = pack.signals.iter().map(|sig| signal_plan(session, sig, &ctx, offsets, frags)).collect();
        Sweep {
            session,
            frags,
            clusters: Clusters::new(frags, session),
            arrived: vec![false; total_records],
            rec_frags,
            root_parts: vec![0; frags.len()],
            stamp: vec![0; frags.len()],
            visit: 0,
            plans,
            candidates,
            rank: pack.source_order.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect(),
            order: sorted_participants(pack),
        }
    }

    fn reveal(&mut self, rec: u32) {
        self.arrived[rec as usize] = true;
        for k in 0..self.rec_frags[rec as usize].len() {
            let f = self.rec_frags[rec as usize][k];
            self.clusters.add(f);
        }
    }

    /// Rack order, then id: the order of a cluster's fragments.
    fn frag_key(&self, x: u32) -> (usize, &str) {
        let f = &self.frags[x as usize];
        (self.rank.get(f.source.as_str()).copied().unwrap_or(usize::MAX), f.id.as_str())
    }

    /// Whether the cluster has a title, read the way `resolve` picks it: the
    /// first title property present, its first value in rack order.
    fn titled(&self, root: u32) -> bool {
        let f0 = &self.frags[root as usize];
        let Some(otype) = self.session.pack.object_types.get(&f0.otype) else { return false };
        for name in otype.title.list() {
            let best = self.clusters.members(root).filter(|&m| self.frags[m as usize].props.contains_key(name)).min_by_key(|&m| self.frag_key(m));
            if let Some(m) = best {
                return !string_form(&self.frags[m as usize].props[name].value).is_empty();
            }
        }
        false
    }

    fn rec_parts(&mut self, rec: u32) -> u64 {
        let mut parts = 0;
        for k in 0..self.rec_frags[rec as usize].len() {
            let root = self.clusters.find(self.rec_frags[rec as usize][k]);
            parts |= self.root_parts[root as usize];
        }
        parts
    }

    fn tally_hits(&mut self, groups: &[Vec<HitRec>], min: f64, allowed: u64, t: &mut Tally) {
        for group in groups {
            let seen = group.iter().filter(|h| self.arrived[h.rec as usize]).count();
            if seen == 0 || (seen as f64) < min {
                continue;
            }
            let mut parts = 0;
            for h in group {
                if self.arrived[h.rec as usize] {
                    parts |= self.rec_parts(h.rec);
                }
            }
            t.add(parts & allowed, group.iter().filter(|h| self.arrived[h.rec as usize]).map(|h| h.value));
        }
    }

    fn tally_co_resolved(&mut self, sig: &Signal, entry: &[u32], bits: &HashMap<u32, u64>, full: u64, allowed: u64, t: &mut Tally) {
        let field = match &sig.strength {
            Strength::Field { field } => Some(field.as_str()),
            Strength::Fixed(_) => None,
        };
        self.visit += 1;
        for &x in entry {
            if !self.clusters.arrived(x) {
                continue;
            }
            let root = self.clusters.find(x);
            if self.stamp[root as usize] == self.visit {
                continue;
            }
            self.stamp[root as usize] = self.visit;
            let met = self.clusters.members(root).fold(0, |m, f| m | bits.get(&f).copied().unwrap_or(0));
            if met & full != full {
                continue;
            }
            let values: Vec<Option<f64>> = match field {
                Some(field) => (0..u64::BITS).filter(|c| (full >> c) & 1 == 1).map(|c| self.clause_value(root, bits, c, field)).collect(),
                None => Vec::new(),
            };
            t.add(self.root_parts[root as usize] & allowed, values);
        }
    }

    /// The field value of the record a clause found: its first match in cluster order.
    fn clause_value(&self, root: u32, bits: &HashMap<u32, u64>, clause: u32, field: &str) -> Option<f64> {
        let found = self.clusters.members(root).filter(|f| (bits.get(f).copied().unwrap_or(0) >> clause) & 1 == 1).min_by_key(|&f| self.frag_key(f))?;
        let f = &self.frags[found as usize];
        let record = self.session.tables.get(&f.source)?.get(f.record)?;
        rules::as_number(get_path(&record.data, field)?)
    }

    /// The compact view of the board with the records revealed so far.
    fn slice(&mut self) -> Value {
        let session = self.session;
        let pack = &session.pack;

        // participant slots -> clusters
        let mut anchored: Vec<Option<u32>> = vec![None; pack.participants.len()];
        for (i, candidates) in self.candidates.iter().enumerate() {
            let mut best: Option<(u32, u32)> = None;
            for &x in candidates {
                if self.clusters.arrived(x) {
                    let root = self.clusters.find(x);
                    let first = self.clusters.first[root as usize];
                    if best.is_none_or(|(_, f)| first < f) {
                        best = Some((root, first));
                    }
                }
            }
            anchored[i] = best.map(|(root, _)| root);
        }
        for (i, root) in anchored.iter().enumerate() {
            if let Some(root) = root {
                self.root_parts[*root as usize] |= 1 << i;
            }
        }

        // signals
        let plans = std::mem::take(&mut self.plans);
        let mut acts: IndexMap<String, f64> = IndexMap::with_capacity(plans.len());
        let mut parents: Vec<Vec<Parent>> = vec![Vec::new(); pack.participants.len()];
        let mut signals = Map::new();
        for plan in &plans {
            let (status, activation) = self.tally_signal(plan, &mut parents);
            acts.insert(plan.signal.id.clone(), activation);
            signals.insert(plan.signal.id.clone(), json!({"p": round4(activation), "status": status}));
        }
        self.plans = plans;

        for root in anchored.iter().flatten() {
            self.root_parts[*root as usize] = 0;
        }

        let participants_in: IndexMap<String, ParticipantIn> = pack
            .participants
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.clone(), ParticipantIn { leak: e.leak, present: anchored[i].is_some(), parents: std::mem::take(&mut parents[i]) }))
            .collect();
        let inference = infer::infer(&acts, &participants_in, &session.priors, &session.edges_in, session.feedback);

        let mut participants = Map::new();
        for (e, root) in pack.participants.iter().zip(&anchored) {
            let status = session.participant_status(&e.id, root.map(|r| self.titled(r)));
            participants.insert(e.id.clone(), json!({"p": round4(inference.p[&e.id]), "status": status, "present": root.is_some()}));
        }
        let mut hypotheses = Map::new();
        for h in &pack.hypotheses {
            let Some(Some(b)) = inference.breakdown.get(&h.id) else { continue };
            let p = inference.p[&h.id];
            let standing = session.hypothesis_standing(&h.id, p, b);
            hypotheses.insert(h.id.clone(), hypothesis_entry(p, standing.status, standing.present, b));
        }
        json!({"signals": signals, "participants": participants, "hypotheses": hypotheses})
    }

    /// Hits, activation and links of one signal now; links go to `parents`.
    fn tally_signal(&mut self, plan: &SignalPlan, parents: &mut [Vec<Parent>]) -> (&'static str, f64) {
        if !plan.available {
            return ("dormant", 0.0);
        }
        let sig = plan.signal;
        let mut t = Tally { linked: vec![0; parents.len()], ..Tally::default() };
        // Jev drives a signal only while every candidate seen so far has an answer.
        let jev_complete = plan.jev.as_ref().filter(|(_, cands)| {
            let mut seen = cands.iter().filter(|c| self.arrived[c.rec.rec as usize]).peekable();
            seen.peek().is_some() && seen.all(|c| c.noul.is_some())
        });
        let strength = if let Some((threshold, cands)) = jev_complete {
            for c in cands {
                if self.arrived[c.rec.rec as usize] && c.noul.is_some_and(|n| n >= *threshold) {
                    let parts = self.rec_parts(c.rec.rec) & plan.allowed;
                    t.add(parts, [c.noul]);
                }
            }
            t.strength.unwrap_or(0.0)
        } else {
            match &plan.plan {
                Plan::Hits { min, groups } => self.tally_hits(groups, *min, plan.allowed, &mut t),
                Plan::CoResolved { full, entry, bits } => self.tally_co_resolved(sig, entry, bits, *full, plan.allowed, &mut t),
            }
            match &sig.strength {
                Strength::Fixed(n) => *n,
                Strength::Field { .. } => t.strength.unwrap_or(0.0),
            }
        };
        if t.hits == 0 {
            return ("quiet", 0.0);
        }
        let most = t.linked.iter().copied().max().unwrap_or(0);
        for &i in &self.order {
            if t.linked[i] > 0 {
                parents[i].push(Parent { from: sig.id.clone(), w: sig.implicates.w * t.linked[i] as f64 / most as f64 });
            }
        }
        let confidence = sig.required_sources().iter().filter_map(|s| self.session.pack.source(s)).map(|s| s.confidence).reduce(f64::min).unwrap_or(0.0);
        ("firing", confidence * strength * plan.factor)
    }
}

fn signal_plan<'a>(session: &'a Session, sig: &'a Signal, ctx: &Context, offsets: &HashMap<&str, u32>, frags: &[Fragment]) -> SignalPlan<'a> {
    let req = sig.required_sources();
    let available = req.iter().all(|s| session.tables.contains_key(s));
    let factor = session.judgement(&sig.id);
    let allowed = allowed_mask(sig, &session.pack.participants);
    let value_of = |source: &str, index: u32| -> Option<f64> {
        let Strength::Field { field } = &sig.strength else { return None };
        rules::as_number(get_path(&session.tables.get(source)?.get(index)?.data, field)?)
    };
    let hit_rec = |source: &str, index: u32| HitRec { rec: offsets[source] + index, value: value_of(source, index) };
    let plan = if !available {
        Plan::Hits { min: 1.0, groups: Vec::new() }
    } else if sig.kind == "co_resolved" {
        co_resolved_plan(session, sig, frags)
    } else {
        let min = if sig.kind == "count" { sig.params.as_ref().and_then(|p| p.get("min")).and_then(Value::as_f64).unwrap_or(2.0) } else { 1.0 };
        let groups = rules::evaluate(sig, ctx).iter().map(|h| h.records.iter().map(|(s, i)| hit_rec(s, *i)).collect()).collect();
        Plan::Hits { min, groups }
    };
    let jev = sig.jev.as_ref().filter(|_| available && session.engine == "jev").map(|spec| {
        let sid = sig.source.as_deref().unwrap_or("");
        let mut cands = Vec::new();
        if let Some(table) = session.tables.get(sid) {
            for r in &table.records {
                if pipeline::matches(&r.data, spec.r#where.as_ref()) {
                    let noul = session.jev.get(&format!("{}|{sid}|{}", sig.id, r.index)).and_then(|a| a.noul);
                    cands.push(JevCandidate { rec: HitRec { rec: offsets[sid] + r.index, value: None }, noul });
                }
            }
        }
        (spec.threshold, cands)
    });
    SignalPlan { signal: sig, available, factor, allowed, plan, jev }
}

/// For each fragment of the signal's type, which clauses its record meets. A
/// cluster's clauses are met when its members' bits cover them all; `entry`
/// lists where to look: fragments meeting the first clause. Up to 64 clauses.
fn co_resolved_plan(session: &Session, sig: &Signal, frags: &[Fragment]) -> Plan {
    let params = sig.params.as_ref();
    let otype = params.and_then(|p| p.get("type")).and_then(Value::as_str);
    let clauses = params.and_then(|p| p.get("clauses")).and_then(Value::as_array);
    let (Some(otype), Some(clauses)) = (otype, clauses) else { return Plan::CoResolved { full: 0, entry: Vec::new(), bits: HashMap::new() } };
    let mut bits = HashMap::new();
    let mut entry = Vec::new();
    for (i, f) in frags.iter().enumerate().filter(|(_, f)| f.otype == otype) {
        let Some(record) = session.tables.get(&f.source).and_then(|t| t.get(f.record)) else { continue };
        let met = clauses
            .iter()
            .enumerate()
            .filter(|(_, c)| c.get("source").and_then(Value::as_str) == Some(f.source.as_str()) && pipeline::matches(&record.data, c.get("where")))
            .fold(0u64, |m, (c, _)| m | 1 << c);
        if met != 0 {
            bits.insert(i as u32, met);
        }
        if met & 1 == 1 || clauses.is_empty() {
            entry.push(i as u32);
        }
    }
    Plan::CoResolved { full: (1u64 << clauses.len()) - 1, entry, bits }
}

fn build(session: &Session) -> Value {
    let policy = session.pack.unknown_time.as_str();

    // Every record gets a position; dated ones line up by time, stable on source and record order.
    let mut offsets: HashMap<&str, u32> = HashMap::new();
    let mut total = 0u32;
    let mut dated: Vec<(i64, u32)> = Vec::new();
    let mut undated_recs = Vec::new();
    let mut rec_ids: Vec<(&str, u32)> = Vec::new();
    for (sid, table) in &session.tables {
        offsets.insert(sid.as_str(), total);
        for r in &table.records {
            let rec = total + r.index;
            rec_ids.push((sid.as_str(), r.index));
            match session.effective_time(sid, session.times[sid][r.index as usize]) {
                Some(t) => dated.push((t, rec)),
                None => undated_recs.push(rec),
            }
        }
        total += table.records.len() as u32;
    }
    dated.sort_by_key(|e| e.0);

    let mut times: Vec<i64> = dated.iter().map(|e| e.0).collect();
    times.dedup();
    if times.len() > MAX_SLICES {
        let last = (times.len() - 1) as f64;
        let mut picks: Vec<usize> = (0..MAX_SLICES).map(|k| (k as f64 * last / (MAX_SLICES - 1) as f64).round_ties_even() as usize).collect();
        picks.dedup();
        times = picks.into_iter().map(|k| times[k]).collect();
    }

    let undated_ids: Vec<&str> = session.state().undated.iter().map(|u| u.id.as_str()).collect();
    let mut sweep = Sweep::new(session, &offsets, total as usize);
    if policy != "after" {
        for &rec in &undated_recs {
            sweep.reveal(rec);
        }
    }

    let event_id = |rec: u32| {
        let (sid, index) = rec_ids[rec as usize];
        format!("{sid}:{index}")
    };

    let mut slices: Vec<Value> = Vec::with_capacity(times.len() + 2);
    let mut cursor = 0;
    let mut seen = 0usize;
    let mut last_at = Value::Null;
    for i in 0..=times.len() {
        let t = if i == 0 { None } else { Some(times[i - 1]) };
        let until = t.unwrap_or(-1);
        let mut arrived = Vec::new();
        while cursor < dated.len() && dated[cursor].0 <= until {
            let rec = dated[cursor].1;
            sweep.reveal(rec);
            if t.is_some() {
                arrived.push(event_id(rec));
            }
            cursor += 1;
        }
        seen += arrived.len();
        last_at = t.map_or(Value::Null, |t| Value::String(time::iso(t)));
        let mut slice = sweep.slice();
        let m = slice.as_object_mut().expect("a slice is an object");
        m.insert("t".into(), json!(t));
        m.insert("at".into(), last_at.clone());
        m.insert("index".into(), json!(i));
        m.insert("events_seen".into(), json!(seen));
        m.insert("arrived".into(), json!(arrived));
        m.insert("undated".into(), if i == 0 && policy == "throughout" { json!(undated_ids) } else { json!([]) });
        slices.push(slice);
    }
    if policy == "after" && !undated_ids.is_empty() {
        for &rec in &undated_recs {
            sweep.reveal(rec);
        }
        let mut slice = sweep.slice();
        let m = slice.as_object_mut().expect("a slice is an object");
        m.insert("t".into(), json!(times.last()));
        m.insert("at".into(), last_at);
        m.insert("index".into(), json!(slices.len()));
        m.insert("events_seen".into(), json!(seen));
        m.insert("arrived".into(), json!([]));
        m.insert("undated".into(), json!(undated_ids));
        m.insert("undated_arrive".into(), json!(true));
        slices.push(slice);
    }

    json!({
        "slices": slices,
        "final": slice_of_state(session.state()),
        "event_count": dated.len(),
        "undated_count": undated_ids.len(),
        "unknown_time": policy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_decimals_round_the_exact_value() {
        assert_eq!(round4(0.12345), 0.1235); // 0.12345 is a little above the tie in binary
        assert_eq!(round4(0.00005), 0.0001);
        assert_eq!(round4(1.0), 1.0);
        assert_eq!(round4(0.0), 0.0);
    }

    fn pieces(frags: &[&str], keys: &[&[u32]]) -> Clusters {
        let n = frags.len();
        let key_count = keys.iter().flat_map(|k| k.iter()).max().map_or(0, |m| *m as usize + 1);
        Clusters { parent: vec![NONE; n], next: (0..n as u32).collect(), first: (0..n as u32).collect(), keys: keys.iter().map(|k| k.to_vec()).collect(), key_first: vec![NONE; key_count], committed: HashMap::new() }
    }

    #[test]
    fn clusters_grow_as_fragments_arrive_in_any_order() {
        let mut c = pieces(&["a", "b", "c", "d"], &[&[0], &[1], &[0, 1], &[2]]);
        c.add(3);
        c.add(1);
        assert!(!c.arrived(0));
        assert_ne!(c.find(1), c.find(3));
        c.add(2);
        assert_eq!(c.find(1), c.find(2));
        c.add(0);
        assert_eq!(c.find(0), c.find(1));
        let root = c.find(0);
        let mut members: Vec<u32> = c.members(root).collect();
        members.sort();
        assert_eq!(members, [0, 1, 2]);
        assert_eq!(c.first[root as usize], 0);
        let lone = c.find(3);
        assert_eq!(c.members(lone).collect::<Vec<_>>(), [3]);
    }

    #[test]
    fn committed_merges_wait_for_both_ends() {
        let mut c = pieces(&["a", "b"], &[&[], &[]]);
        c.committed.insert(0, vec![1]);
        c.committed.insert(1, vec![0]);
        c.add(0);
        assert_eq!(c.members(0).count(), 1);
        c.add(1);
        assert_eq!(c.find(0), c.find(1));
    }
}

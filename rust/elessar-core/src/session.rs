//! One investigation: decisions, the recompute, actions, insights (spec §5, §7, §8, §10).
//!
//! The session holds decisions (which sources are connected, merges,
//! judgements, a crown). The board is recomputed from those on every action,
//! so a disconnect or a reverted merge is never a special case. Parsed tables
//! and their fragments are the only things kept across recomputes.
//!
//! Fetching lives in the server. It calls [`Session::connect`] with a payload,
//! or [`Session::connect_failed`] when the fetch fails.

use crate::infer::{self, EdgeIn, ParticipantIn};
use crate::pack::{Pack, Participant, Signal, Source};
use crate::pipeline::{self, py, Fragment};
use crate::resolve::{self, Cluster, Resolution};
use crate::rules::{self, Context, Hit};
use crate::store::{string_form, Table};
use crate::time;
use crate::types::*;
use indexmap::IndexMap;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// How much of a signal's activation survives a judgement; `None` for an unknown judgement.
pub fn judgement_factor(judgement: &str) -> Option<f64> {
    match judgement {
        "trust" => Some(1.0),
        "discount" => Some(0.5),
        "dismiss" => Some(0.0),
        _ => None,
    }
}

/// Points, title and text of an insight kind. Packs may override title and
/// text in `narrative.insights.<kind>`.
fn insight_template(kind: &str) -> (u32, &'static str, &'static str) {
    match kind {
        "shape_shock" => (50, "Shape Shock", "Raw sources agree on nothing: delimiters, timestamps, naming, nesting. Fusion starts by mapping each one onto a single ontology."),
        "first_link" => (100, "First Link", "A signal attached to a participant because one of its records resolved into that participant. Links come from resolution, not hand-wiring."),
        "identity_resolved" => (150, "Identity Resolved: {label}", "Another source shared a key with this participant, so their fragments merged automatically. It now has a name."),
        "merge_committed" => (150, "Merge Committed", "A fuzzy match is only a proposal. You decided these are the same thing, and you can still revert it."),
        "fusion_signal" => (150, "Fusion-Only Signal: {label}", "This rule needs facts from different sources on one participant. It could not fire until resolution joined them."),
        "inhibited" => (100, "Inhibited", "Evidence can argue against a hypothesis: {source} cut {target} by {pct}%."),
        "explained_away" => (200, "Explained Away: {label}", "Inhibitors and competing hypotheses moved this explanation off the board, into the column on the left. The reason is next to it, and you can bring it back."),
        "crowned" => (500, "Crowned: {label}", "The posterior backs your call at {p}%."),
        "crowned_premature" => (0, "Called Early: {label}", "Logged, not scored: the posterior is only {p}%. Connect more sources or revisit your judgements."),
        "crowned_contradicted" => (0, "Overruled: {label}", "Logged, not scored: the board favors {best} at {best_p}%."),
        "full_fusion" => (250, "Full Fusion", "Every source connected: many shapes, one view."),
        _ => (0, "", ""),
    }
}

type Fmt = Vec<(&'static str, String)>;

/// `str.format` for named fields. `{{` and `}}` are literal braces; an unknown
/// field stays as written (Python raises, which would fail the action).
fn format_text(template: &str, fmt: &Fmt) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(at) = rest.find(['{', '}']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        let name = tail[1..].find('}').map(|end| &tail[1..end + 1]);
        match name.and_then(|n| fmt.iter().find(|(k, _)| *k == n).map(|(_, v)| (n, v))) {
            Some((n, v)) => {
                out.push_str(v);
                rest = &tail[n.len() + 2..];
            }
            None => {
                out.push_str(&tail[..1]);
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `round(x)` as Python prints a percentage: to the nearest integer, ties to even.
fn round_int(x: f64) -> i64 {
    x.round_ties_even() as i64
}

fn now_ms() -> i64 {
    // Whole seconds, as the Python engine's `_now` keeps them.
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64 * 1000)
}

/// One sentence saying why a hypothesis was moved to the side of the board (spec §7).
fn capture_reason(b: &Breakdown, p: f64, names: &HashMap<&str, String>) -> String {
    let mut cuts: Vec<&Inhibitor> = b.inhibit.iter().collect();
    cuts.sort_by(|x, y| (y.w * y.p).total_cmp(&(x.w * x.p)));
    let parts: Vec<String> = cuts
        .into_iter()
        .filter(|i| i.w * i.p >= 0.05)
        .take(3)
        .map(|i| format!("{} cut it by {}%", names.get(i.from.as_str()).map_or(i.from.as_str(), String::as_str), round_int(i.w * i.p * 100.0)))
        .collect();
    let mut text = format!("Explained away at {}%: {}.", round_int(p * 100.0), parts.join("; "));
    let divisor = b.divisor.unwrap_or(1.0);
    if divisor > 1.0 {
        text.push_str(&format!(" Competing explanations then divided what was left by {divisor:.2}."));
    }
    text
}

/// A connected source: what the source card reports.
#[derive(Debug, Clone)]
struct Connection {
    latency_ms: u64,
    bytes: usize,
    at_ms: i64,
}

/// A parsed payload whose source is not connected, kept for `reconnect` and
/// so that connecting the same payload again does not parse it again.
#[derive(Debug, Clone)]
struct Parked {
    table: Table,
    times: Vec<Option<i64>>,
    latency_ms: u64,
}

/// Clusters and the proposals between them. Only connecting, disconnecting
/// and merge decisions change them, so other actions reuse the last ones.
struct Resolved {
    res: Resolution,
    pending: Vec<Proposal>,
}

/// What Jev said about one record for one signal (`"signal|source|index"`).
#[derive(Debug, Clone, Default)]
pub struct JevAnswer {
    pub noul: Option<f64>,
    pub model: Option<String>,
    pub ms: Option<u64>,
    pub cached: bool,
    pub error: Option<String>,
}

/// The result of one Jev request (one record, all of its questions).
/// `answers` is `{<signal id with - as _>: {"noul": p}}`.
#[derive(Debug, Clone, Default)]
pub struct JevResult {
    pub answers: Option<Value>,
    pub model: Option<String>,
    pub ms: Option<u64>,
    pub cost: Option<f64>,
    pub cached: bool,
    pub error: Option<String>,
}

/// One request to make of Jev (spec §11). The cache key's payload is
/// `{"model", "state": state, "questions": questions}`.
#[derive(Debug, Clone)]
pub struct JevJob {
    pub index: u32,
    pub pointer: String,
    pub excerpt: String,
    pub state: Value,
    pub questions: Value,
    /// Ids of the signals asking.
    pub signals: Vec<String>,
}

#[derive(Clone)]
pub struct Session {
    pub pack: Pack,
    /// Prefix of the `/mock/...` URLs named in audit entries.
    pub base_url: String,
    /// Epoch ms now, in whole seconds. Replaceable, because `at_connect` dates records by it.
    pub clock: fn() -> i64,

    // decisions
    connected: HashMap<String, Connection>,
    errors: HashMap<String, String>,
    committed: Vec<String>,
    rejected: HashSet<String>,
    judgements: HashMap<String, String>,
    crown: Option<Crown>,
    kept: HashSet<String>,
    pub(crate) participant_sources: HashMap<String, Vec<String>>,
    pub(crate) feedback: bool,
    pub(crate) engine: String,
    pub(crate) jev: HashMap<String, JevAnswer>,
    jev_available: bool,
    jev_model: String,
    jev_calls: u64,
    jev_cached: u64,
    jev_errors: u64,
    jev_cost: f64,
    acked: HashSet<String>,
    earned: BTreeMap<String, u32>,
    audit: Vec<AuditEntry>,
    peeked: HashSet<String>,

    // data: connected tables in rack order, their fragments in rack, mapping, record order
    pub(crate) tables: IndexMap<String, Table>,
    pub(crate) times: HashMap<String, Vec<Option<i64>>>,
    pub(crate) fragments: Vec<Fragment>,
    pub(crate) frags_by_record: IndexMap<(String, u32), Vec<String>>,
    parked: HashMap<String, Parked>,

    // the board
    resolved: Option<Arc<Resolved>>,
    pub(crate) priors: IndexMap<String, f64>,
    pub(crate) edges_in: Vec<EdgeIn>,
    state: Option<State>,
    pub(crate) version: u64,
    pub(crate) timeline_cache: Option<(u64, Value)>,
}

/// One signal's evaluation, on the way to its node.
struct SignalOutcome {
    activation: f64,
    detail: SignalDetail,
    /// (participant position, signal -> participant weight)
    links: Vec<(usize, f64)>,
    hit_records: Vec<(String, u32)>,
}

impl Session {
    pub fn new(pack: Pack) -> Session {
        let priors = pack.hypotheses.iter().map(|h| (h.id.clone(), h.prior)).collect();
        let edges_in = pack.edges.iter().map(|e| EdgeIn { from: e.from.clone(), to: e.to.clone(), w: e.w, kind: e.kind.clone() }).collect();
        let mut s = Session {
            pack,
            base_url: String::new(),
            clock: now_ms,
            connected: HashMap::new(),
            errors: HashMap::new(),
            committed: Vec::new(),
            rejected: HashSet::new(),
            judgements: HashMap::new(),
            crown: None,
            kept: HashSet::new(),
            participant_sources: HashMap::new(),
            feedback: false,
            engine: "rules".into(),
            jev: HashMap::new(),
            jev_available: false,
            jev_model: "jev-latest".into(),
            jev_calls: 0,
            jev_cached: 0,
            jev_errors: 0,
            jev_cost: 0.0,
            acked: HashSet::new(),
            earned: BTreeMap::new(),
            audit: Vec::new(),
            peeked: HashSet::new(),
            tables: IndexMap::new(),
            times: HashMap::new(),
            fragments: Vec::new(),
            frags_by_record: IndexMap::new(),
            parked: HashMap::new(),
            resolved: None,
            priors,
            edges_in,
            state: None,
            version: 0,
            timeline_cache: None,
        };
        s.recompute();
        s
    }

    pub fn state(&self) -> &State {
        self.state.as_ref().expect("the board is computed in new")
    }

    // ------------------------------------------------------------ what the server reads

    /// Tell the state what the server knows about Jev: whether a key is set, which model.
    pub fn set_jev_status(&mut self, available: bool, model: &str) {
        self.jev_available = available;
        self.jev_model = model.to_string();
        if let Some(st) = self.state.as_mut() {
            st.jev.available = available;
            st.jev.model = model.to_string();
        }
    }

    pub fn jev_available(&self) -> bool {
        self.jev_available
    }

    pub fn is_connected(&self, source: &str) -> bool {
        self.connected.contains_key(source)
    }

    /// Ids of the connected sources in rack order.
    pub fn connected_sources(&self) -> Vec<String> {
        self.tables.keys().cloned().collect()
    }

    /// Whether `reconnect` can bring this source back without a fetch.
    pub fn has_payload(&self, source: &str) -> bool {
        self.connected.contains_key(source) || self.parked.contains_key(source)
    }

    /// The parsed payload of a source, connected or not, if one was ever fetched.
    pub fn table(&self, source: &str) -> Option<&Table> {
        self.tables.get(source).or_else(|| self.parked.get(source).map(|p| &p.table))
    }

    /// The audit trail in full; `state().audit` carries its last 80 entries.
    pub fn audit(&self) -> &[AuditEntry] {
        &self.audit
    }

    /// Append to the audit trail, for what the server does on the session's behalf (Jev cache, agent).
    pub fn log(&mut self, action: &str, detail: &str) {
        self.audit.push(AuditEntry { at: time::iso((self.clock)()), action: action.to_string(), detail: detail.to_string() });
        self.sync_log();
    }

    /// Recompute after a change made outside an action, like a forgotten Jev answer.
    pub fn refresh(&mut self) {
        self.recompute();
    }

    // ------------------------------------------------------------ connecting

    /// Connect a source from an already-fetched payload (the server fetches;
    /// tests read files). A payload that fails to parse as a whole leaves the
    /// source as it was and reports the error on its card.
    pub fn connect(&mut self, source: &str, raw: String, latency_ms: u64) -> Vec<Insight> {
        self.transact(|s, _| {
            s.attach(source, raw, latency_ms);
            Ok(())
        })
        .unwrap_or_default()
    }

    /// The server could not fetch a source: report it on the source card.
    pub fn connect_failed(&mut self, source: &str, error: &str) -> Vec<Insight> {
        self.transact(|s, _| {
            s.fail(source, error.to_string());
            Ok(())
        })
        .unwrap_or_default()
    }

    pub fn disconnect(&mut self, source: &str) -> Vec<Insight> {
        self.transact(|s, _| {
            s.detach(source);
            s.log_entry("disconnect", source.to_string());
            Ok(())
        })
        .unwrap_or_default()
    }

    fn source_url(&self, sid: &str, source: &Source) -> String {
        match (&source.url, source.transport.as_str()) {
            (Some(url), "http") => url.clone(),
            _ => format!("{}/mock/{}/{}", self.base_url, self.pack.id, sid),
        }
    }

    fn fail(&mut self, source: &str, error: String) {
        self.log_entry("connect-failed", format!("{source}: {error}"));
        self.errors.insert(source.to_string(), error);
    }

    fn attach(&mut self, source: &str, raw: String, latency_ms: u64) {
        let Some(src) = self.pack.source(source).cloned() else {
            return self.fail(source, format!("unknown source '{source}'"));
        };
        // The same payload again keeps its parse; a bad one leaves things as they were.
        let fresh = if self.table(source).is_some_and(|t| t.raw == raw) {
            None
        } else {
            match pipeline::parse(&src, raw) {
                Ok(table) => Some(table),
                Err(e) => return self.fail(source, e),
            }
        };
        self.detach(source);
        let kept = self.parked.remove(source);
        let (table, times) = match (fresh, kept) {
            (Some(table), _) => {
                let times = record_times(&table, &src);
                (table, times)
            }
            (None, Some(p)) => (p.table, p.times),
            (None, None) => unreachable!("an unchanged payload was on the rack or parked"),
        };
        self.install(source, &src, table, times, latency_ms);
    }

    /// Bring a source back from its last payload.
    fn bring_back(&mut self, source: &str) -> Result<(), String> {
        self.detach(source);
        let parked = self.parked.remove(source).ok_or_else(|| format!("no payload for source '{source}': fetch and connect it"))?;
        let src = self.pack.source(source).cloned().ok_or_else(|| format!("unknown source '{source}'"))?;
        self.install(source, &src, parked.table, parked.times, parked.latency_ms);
        Ok(())
    }

    /// Put a table in rack order and its fragments with it.
    fn install(&mut self, source: &str, src: &Source, table: Table, times: Vec<Option<i64>>, latency_ms: u64) {
        let rank = |pack: &Pack, s: &str| pack.source_order.iter().position(|x| x == s).unwrap_or(usize::MAX);
        let here = rank(&self.pack, source);
        let mappings: Vec<_> = self.pack.mappings.iter().enumerate().filter(|(_, m)| m.source == source).collect();
        let frags = pipeline::fragments(&table, &mappings);
        for f in &frags {
            self.frags_by_record.entry((f.source.clone(), f.record)).or_default().push(f.id.clone());
        }
        let bytes = table.raw.len();
        let table_at = self.tables.keys().filter(|s| rank(&self.pack, s) < here).count();
        let frag_at = self.fragments.partition_point(|f| rank(&self.pack, &f.source) < here);
        self.fragments.splice(frag_at..frag_at, frags);
        self.resolved = None;
        let skipped = table.skipped.first().map(|bad| (table.skipped.len(), bad.pointer.clone(), bad.reason.clone()));
        self.tables.shift_insert(table_at, source.to_string(), table);
        self.times.insert(source.to_string(), times);
        self.connected.insert(source.to_string(), Connection { latency_ms, bytes, at_ms: (self.clock)() });
        self.errors.remove(source);
        self.log_entry("connect", format!("{source}: {bytes} bytes in {latency_ms} ms from {}", self.source_url(source, src)));
        if let Some((n, pointer, reason)) = skipped {
            self.log_entry("skipped", format!("{source}: {n} row(s) could not be parsed, e.g. {pointer}: {reason}"));
        }
    }

    /// Take a source off the rack, keeping its parse.
    fn detach(&mut self, source: &str) {
        let Some(conn) = self.connected.remove(source) else { return };
        if let Some(table) = self.tables.shift_remove(source) {
            let times = self.times.remove(source).unwrap_or_default();
            self.parked.insert(source.to_string(), Parked { table, times, latency_ms: conn.latency_ms });
        }
        self.fragments.retain(|f| f.source != source);
        self.resolved = None;
        self.frags_by_record.retain(|k, _| k.0 != source);
    }

    /// The committed merges as pairs of fragment ids.
    pub(crate) fn committed_pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.committed.iter().filter_map(|pid| pid.split_once('|'))
    }

    /// The multiplier of a signal's current judgement.
    pub(crate) fn judgement(&self, signal: &str) -> f64 {
        self.judgements.get(signal).and_then(|j| judgement_factor(j)).unwrap_or(1.0)
    }

    /// Epoch ms of a record once the undated policy has had its say. `None` means undated.
    pub(crate) fn effective_time(&self, source: &str, raw_time: Option<i64>) -> Option<i64> {
        raw_time.or_else(|| if self.pack.unknown_time == "at_connect" { self.connected.get(source).map(|c| c.at_ms) } else { None })
    }

    // ------------------------------------------------------------ the recompute

    pub(crate) fn recompute(&mut self) {
        let resolved = match &self.resolved {
            Some(r) => r.clone(),
            None => {
                let r = Arc::new(self.resolve());
                self.resolved = Some(r.clone());
                r
            }
        };
        let res = &resolved.res;
        let anchored: Vec<Option<&Cluster>> = self.pack.participants.iter().map(|e| anchor_of(res, e)).collect();
        for (e, c) in self.pack.participants.iter().zip(&anchored) {
            if let Some(c) = c {
                self.participant_sources.insert(e.id.clone(), c.sources.clone());
            }
        }
        let state = self.build(res, &resolved.pending, &anchored);
        self.state = Some(state);
        self.version += 1;
    }

    fn resolve(&self) -> Resolved {
        let pairs: Vec<(String, String)> = self.committed_pairs().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let res = resolve::resolve(&self.fragments, &self.pack.object_types, &pairs, &self.pack.source_order);
        // The fragment map only names a side's source, which provenance also knows.
        let pending = resolve::proposals(&res, &IndexMap::new(), &self.pack.object_types, &self.rejected);
        Resolved { res, pending }
    }

    fn build(&self, res: &Resolution, pending: &[Proposal], anchored: &[Option<&Cluster>]) -> State {
        let pack = &self.pack;
        let ctx = Context { tables: &self.tables, frags_by_record: &self.frags_by_record, resolution: res };

        // fragment id -> bits of the participants anchored to its cluster
        let mut frag_parts: HashMap<&str, u64> = HashMap::new();
        for (i, c) in anchored.iter().enumerate() {
            for fid in c.iter().flat_map(|c| &c.fragments) {
                *frag_parts.entry(fid.as_str()).or_insert(0) |= 1 << i;
            }
        }
        let order = sorted_participants(pack);

        // signals
        let mut acts: IndexMap<String, f64> = IndexMap::with_capacity(pack.signals.len());
        let mut parents: Vec<Vec<infer::Parent>> = vec![Vec::new(); pack.participants.len()];
        let mut details = Vec::with_capacity(pack.signals.len());
        let mut record_signals: HashMap<(String, u32), Vec<usize>> = HashMap::new();
        for (si, sig) in pack.signals.iter().enumerate() {
            let out = self.evaluate_signal(sig, &ctx, &frag_parts, &order);
            acts.insert(sig.id.clone(), out.activation);
            for (part, w) in &out.links {
                parents[*part].push(infer::Parent { from: sig.id.clone(), w: *w });
            }
            for key in out.hit_records {
                let on = record_signals.entry(key).or_default();
                if on.last() != Some(&si) {
                    on.push(si);
                }
            }
            details.push(out.detail);
        }

        let participants_in: IndexMap<String, ParticipantIn> = pack
            .participants
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.clone(), ParticipantIn { leak: e.leak, present: anchored[i].is_some(), parents: std::mem::take(&mut parents[i]) }))
            .collect();
        let inference = infer::infer(&acts, &participants_in, &self.priors, &self.edges_in, self.feedback);
        let p = &inference.p;

        // edges for drawing
        let mut edges = Vec::new();
        for (sig, d) in pack.signals.iter().zip(&details) {
            for link in &d.links {
                edges.push(EdgeView { from: sig.id.clone(), to: link.participant.clone(), kind: "support".into(), w: link.w, flow: link.w * acts[&sig.id] });
            }
        }
        for e in &pack.edges {
            edges.push(EdgeView { from: e.from.clone(), to: e.to.clone(), kind: e.kind.clone(), w: e.w, flow: e.w * p.get(&e.from).copied().unwrap_or(0.0) });
        }

        // nodes
        let layout: HashMap<&str, &crate::pack::LayoutNode> = pack.layout.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        let node = |id: &str, label: Option<String>, status: &str, present: bool, detail: Detail| {
            let at = layout[id];
            let pr = p.get(id).copied().unwrap_or(0.0);
            Node {
                id: id.to_string(),
                code: at.code.clone(),
                band: at.band.clone(),
                label: label.unwrap_or_else(|| at.label.clone()),
                col: at.col,
                base_row: at.base_row,
                p: pr,
                step: step_for(pr),
                present,
                status: status.to_string(),
                kept: None,
                kept_reason: None,
                offboard: None,
                crowned: None,
                leading: None,
                detail,
            }
        };

        let mut nodes = Vec::with_capacity(pack.layout.nodes.len());
        for (sig, d) in pack.signals.iter().zip(details) {
            let status = d.status.clone();
            nodes.push(node(&sig.id, None, &status, true, Detail::Signal(d)));
        }

        for (e, c) in pack.participants.iter().zip(anchored) {
            let breakdown = inference.breakdown.get(&e.id).cloned().flatten();
            let detail = self.participant_detail(e, *c, breakdown, res);
            let status = self.participant_status(&e.id, c.map(cluster_titled));
            let offboard = (status == "removed").then(|| {
                let srcs = self.participant_sources[&e.id].clone();
                let names: Vec<&str> = srcs.iter().map(|s| pack.sources[s].name.as_str()).collect();
                Offboard {
                    kind: "removed".into(),
                    restore: "reconnect".into(),
                    reason: format!("No connected source describes it any more. It came from: {}.", names.join(", ")),
                    sources: Some(srcs),
                }
            });
            let label = match c {
                Some(c) if status == "resolved" => c.title.clone().unwrap_or_default(),
                _ => e.anchor.values().next().map(string_form).unwrap_or_default(),
            };
            let mut n = node(&e.id, Some(label), status, c.is_some(), Detail::Participant(detail));
            n.offboard = Some(offboard);
            nodes.push(n);
        }

        let names: HashMap<&str, String> = pack.layout.nodes.iter().map(|n| (n.id.as_str(), format!("{} {}", n.code, n.label))).collect();
        let mut best: Option<(&str, f64)> = None;
        for h in &pack.hypotheses {
            let ph = p[&h.id];
            if best.is_none_or(|(_, top)| ph > top) {
                best = Some((&h.id, ph));
            }
        }
        for h in &pack.hypotheses {
            let Some(b) = inference.breakdown.get(&h.id).cloned().flatten() else { continue };
            let ph = p[&h.id];
            let standing = self.hypothesis_standing(&h.id, ph, &b);
            let reason = standing.captured.then(|| capture_reason(&b, ph, &names));
            let mut n = node(&h.id, None, standing.status, standing.present, Detail::Hypothesis(HypothesisDetail { blurb: h.blurb.clone(), prior: h.prior, breakdown: b }));
            n.kept = Some(standing.kept);
            n.kept_reason = Some(if standing.kept { reason.clone() } else { None });
            n.offboard = Some(if standing.captured && !standing.kept {
                reason.map(|reason| Offboard { kind: "captured".into(), restore: "keep".into(), sources: None, reason })
            } else {
                None
            });
            n.crowned = Some(self.crown.as_ref().is_some_and(|c| c.hypothesis == h.id));
            n.leading = Some(best.is_some_and(|(id, _)| id == h.id));
            nodes.push(n);
        }

        let (events, events_truncated, undated, links) = self.events_and_links(anchored, &acts, &record_signals, &order);

        State {
            scenario: ScenarioView {
                id: pack.id.clone(),
                title: pack.title.clone(),
                premise: pack.premise.clone(),
                board: Board { width: pack.layout.width, height: pack.layout.height },
                narrative: pack.narrative.clone(),
            },
            sources: self.source_views(),
            nodes,
            edges,
            stages: inference.stages,
            merges: Merges { pending: pending.to_vec(), committed: self.committed_views() },
            events,
            events_truncated: events_truncated.then_some(true),
            undated,
            unknown_time: pack.unknown_time.clone(),
            links,
            object_types: serde_json::to_value(&pack.object_types).unwrap_or(Value::Null),
            feedback: self.feedback,
            engine: self.engine.clone(),
            jev: JevStats { available: self.jev_available, model: self.jev_model.clone(), calls: self.jev_calls, cached: self.jev_cached, errors: self.jev_errors, cost: self.jev_cost },
            crown: self.crown.clone(),
            score: self.earned.values().sum(),
            earned: self.earned.keys().cloned().collect(),
            audit: self.last_audit(),
            thresholds: Thresholds { steps: STEP_AT, crownable: CROWNABLE, capture_p: CAPTURE_P },
        }
    }

    fn evaluate_signal(&self, sig: &Signal, ctx: &Context, frag_parts: &HashMap<&str, u64>, order: &[usize]) -> SignalOutcome {
        let pack = &self.pack;
        let req = sig.required_sources();
        let available = req.iter().all(|s| self.tables.contains_key(s));
        let rule_hits = if available { rules::evaluate(sig, ctx) } else { Vec::new() };
        let rule_strength = if rule_hits.is_empty() { 0.0 } else { rules::strength(sig, &rule_hits, ctx) };
        let jev = if available { sig.jev.as_ref().map(|_| self.jev_view(sig, &rule_hits)) } else { None };
        let jev_hits = jev.as_ref().filter(|v| self.engine == "jev" && v.complete).map(|v| {
            let source = sig.source.clone().unwrap_or_default();
            let hits: Vec<Hit> = v
                .records
                .iter()
                .filter(|r| r.jev_hit)
                .map(|r| {
                    let key = (source.clone(), r.index);
                    let fragments = self.frags_by_record.get(&key).cloned().unwrap_or_default();
                    Hit { records: vec![key], fragments }
                })
                .collect();
            (hits, v.strength)
        });
        let engine_used = if jev_hits.is_some() { "jev" } else { "rules" };
        let (hits, strength) = match &jev_hits {
            Some((hits, strength)) => (hits, *strength),
            None => (&rule_hits, rule_strength),
        };

        let confidence = req.iter().filter_map(|s| pack.source(s)).map(|s| s.confidence).reduce(f64::min).unwrap_or(0.0);
        let judgement = self.judgements.get(&sig.id).map_or("trust", String::as_str);
        let activation = if hits.is_empty() { 0.0 } else { confidence * strength * judgement_factor(judgement).unwrap_or(1.0) };

        let allowed = allowed_mask(sig, &pack.participants);
        let mut linked = vec![0usize; pack.participants.len()];
        let mut hit_records = Vec::new();
        let mut hit_out = Vec::with_capacity(hits.len());
        for h in hits {
            hit_records.extend(h.records.iter().cloned());
            let parts = h.fragments.iter().fold(0u64, |m, fid| m | frag_parts.get(fid.as_str()).copied().unwrap_or(0)) & allowed;
            for (i, n) in linked.iter_mut().enumerate() {
                *n += ((parts >> i) & 1) as usize;
            }
            let records = h
                .records
                .iter()
                .filter_map(|(source, index)| {
                    let table = self.tables.get(source)?;
                    let r = table.get(*index)?;
                    Some(HitRecord { source: source.clone(), pointer: r.pointer.clone(), excerpt: table.excerpt(r).to_string() })
                })
                .collect();
            hit_out.push(HitView { records, participants: mask_ids(parts, order, pack) });
        }
        let most = linked.iter().copied().max().unwrap_or(0);
        let mut links = Vec::new();
        let mut link_views = Vec::new();
        for &i in order {
            if linked[i] > 0 {
                let w = sig.implicates.w * linked[i] as f64 / most as f64;
                links.push((i, w));
                link_views.push(LinkView { participant: pack.participants[i].id.clone(), hits: linked[i], w });
            }
        }

        let detail = SignalDetail {
            status: if !available { "dormant" } else if hits.is_empty() { "quiet" } else { "firing" }.into(),
            sources: req,
            kind: sig.kind.clone(),
            rule: rule_json(sig),
            explain: sig.explain.clone(),
            confidence,
            strength,
            engine_used: engine_used.into(),
            rule_strength,
            rule_hit_count: rule_hits.len(),
            jev,
            judgement: judgement.to_string(),
            hits: hit_out,
            links: link_views,
            implicates: implicates_json(sig),
        };
        SignalOutcome { activation, detail, links, hit_records }
    }

    /// What Jev said about each candidate record, next to what the rule said.
    fn jev_view(&self, sig: &Signal, rule_hits: &[Hit]) -> JevView {
        let Some(spec) = sig.jev.as_ref() else { unreachable!("called for signals with a jev block") };
        let sid = sig.source.as_deref().unwrap_or("");
        let rule_keys: HashSet<u32> = rule_hits.iter().flat_map(|h| &h.records).filter(|(s, _)| s == sid).map(|(_, i)| *i).collect();
        let mut records = Vec::new();
        if let Some(table) = self.tables.get(sid) {
            for r in &table.records {
                if !pipeline::matches(&r.data, spec.r#where.as_ref()) {
                    continue;
                }
                let res = self.jev.get(&format!("{}|{sid}|{}", sig.id, r.index));
                let noul = res.and_then(|a| a.noul);
                records.push(JevRecordView {
                    index: r.index,
                    source: sid.to_string(),
                    pointer: r.pointer.clone(),
                    excerpt: table.excerpt(r).to_string(),
                    noul,
                    error: res.and_then(|a| a.error.clone()),
                    model: res.and_then(|a| a.model.clone()),
                    rule_hit: rule_keys.contains(&r.index),
                    jev_hit: noul.is_some_and(|n| n >= spec.threshold),
                });
            }
        }
        let scored: Vec<&JevRecordView> = records.iter().filter(|r| r.noul.is_some()).collect();
        let hits: Vec<&&JevRecordView> = scored.iter().filter(|r| r.jev_hit).collect();
        JevView {
            question: spec.question.clone(),
            criteria: spec.criteria.clone(),
            r#where: spec.r#where.clone(),
            threshold: spec.threshold,
            complete: !records.is_empty() && scored.len() == records.len(),
            asked: !scored.is_empty(),
            strength: hits.iter().filter_map(|r| r.noul).fold(0.0, f64::max),
            hit_count: hits.len(),
            disagreements: scored.iter().filter(|r| r.rule_hit != r.jev_hit).count(),
            model: scored.iter().find_map(|r| r.model.clone()),
            records,
        }
    }

    fn participant_detail(&self, e: &Participant, c: Option<&Cluster>, breakdown: Option<Breakdown>, res: &Resolution) -> ParticipantDetail {
        let mut d = ParticipantDetail { r#type: e.otype.clone(), anchor: e.anchor.clone(), breakdown, title: None, props: None, conflicts: None, sources: None, fragment_count: None, merges: None };
        if let Some(c) = c {
            let mut props = c.props.clone();
            for prov in props.values_mut().flatten().flat_map(|v| &mut v.provenance) {
                if let Some((source, index)) = rules::fragment_record(&prov.fragment) {
                    if let Some(table) = self.tables.get(source) {
                        if let Some(r) = table.get(index) {
                            prov.excerpt = table.excerpt(r).to_string();
                        }
                    }
                }
            }
            d.title = Some(c.title.clone());
            d.props = Some(props);
            d.conflicts = Some(c.conflicts.clone());
            d.sources = Some(c.sources.clone());
            d.fragment_count = Some(c.fragments.len());
            d.merges = Some(self.committed.iter().filter(|pid| pid.split('|').any(|f| res.cluster_of.get(f) == Some(&c.id))).cloned().collect());
        }
        d
    }

    /// `titled` is `None` for a participant without a cluster.
    pub(crate) fn participant_status(&self, id: &str, titled: Option<bool>) -> &'static str {
        match titled {
            Some(true) => "resolved",
            Some(false) => "unresolved",
            None if self.participant_sources.contains_key(id) => "removed",
            None => "absent",
        }
    }

    pub(crate) fn hypothesis_standing(&self, id: &str, p: f64, b: &Breakdown) -> Standing {
        let captured = p <= CAPTURE_P && b.inhibit_factor <= CAPTURE_INHIBIT;
        let kept = captured && self.kept.contains(id);
        let status = if captured && !kept {
            "captured"
        } else if kept {
            "kept"
        } else if p >= CROWNABLE {
            "crownable"
        } else {
            "open"
        };
        Standing { captured, kept, status, present: !captured || kept }
    }

    fn events_and_links(&self, anchored: &[Option<&Cluster>], acts: &IndexMap<String, f64>, record_signals: &HashMap<(String, u32), Vec<usize>>, order: &[usize]) -> (Vec<Event>, bool, Vec<Undated>, Vec<Link>) {
        let pack = &self.pack;
        // (source, record index) -> bits of the participants its fragments resolve into
        let mut record_parts: HashMap<(&str, u32), u64> = HashMap::new();
        for (i, c) in anchored.iter().enumerate() {
            for fid in c.iter().flat_map(|c| &c.fragments) {
                if let Some(key) = rules::fragment_record(fid) {
                    *record_parts.entry(key).or_insert(0) |= 1 << i;
                }
            }
        }
        let shared = |sid: &str, index: u32| {
            let parts = record_parts.get(&(sid, index)).copied().unwrap_or(0);
            let mut sigs: Vec<usize> = record_signals.get(&(sid.to_string(), index)).cloned().unwrap_or_default();
            sigs.sort_by(|a, b| pack.signals[*a].id.cmp(&pack.signals[*b].id));
            let severity = sigs.iter().map(|i| acts[&pack.signals[*i].id]).fold(None, |m: Option<f64>, a| Some(m.map_or(a, |m| m.max(a)))).unwrap_or(0.0);
            let signals: Vec<String> = sigs.into_iter().map(|i| pack.signals[i].id.clone()).collect();
            (mask_ids(parts, order, pack), signals, severity)
        };

        let mut dated: Vec<(i64, usize, u32)> = Vec::new();
        let mut undated = Vec::new();
        // Pairs are keyed by rank in id order, so a < b by id.
        let mut pairs: BTreeMap<(usize, usize), Vec<RecordRef>> = BTreeMap::new();
        for (ti, (sid, table)) in self.tables.iter().enumerate() {
            let times = &self.times[sid];
            let tconf = pack.sources[sid].time.as_ref().filter(|t| t.field.is_some() || t.document.is_some());
            for r in &table.records {
                let parts = record_parts.get(&(sid.as_str(), r.index)).copied().unwrap_or(0);
                if parts.count_ones() >= 2 {
                    let ranks: Vec<usize> = (0..order.len()).filter(|k| (parts >> order[*k]) & 1 == 1).collect();
                    for (x, a) in ranks.iter().enumerate() {
                        for b in &ranks[x + 1..] {
                            pairs.entry((*a, *b)).or_default().push(RecordRef { source: sid.clone(), pointer: r.pointer.clone() });
                        }
                    }
                }
                match self.effective_time(sid, times[r.index as usize]) {
                    Some(t) => dated.push((t, ti, r.index)),
                    None => {
                        let (participants, signals, severity) = shared(sid, r.index);
                        let id = format!("{sid}:{}", r.index);
                        undated.push(Undated {
                            acked: self.acked.contains(&id),
                            id,
                            source: sid.clone(),
                            pointer: r.pointer.clone(),
                            excerpt: table.excerpt(r).to_string(),
                            participants,
                            signals,
                            severity,
                            why: if tconf.is_none() { "no time config" } else { "no usable time value" }.into(),
                        });
                    }
                }
            }
        }

        // Stable: ties keep source order, then record order.
        dated.sort_by_key(|e| e.0);
        let truncated = dated.len() > MAX_EVENTS_V1;
        let keep_from = dated.len().saturating_sub(MAX_EVENTS_V1);
        let events = dated[keep_from..]
            .iter()
            .map(|&(t, ti, index)| {
                let (sid, table) = self.tables.get_index(ti).expect("position from this table list");
                let r = &table.records[index as usize];
                let (participants, signals, severity) = shared(sid, index);
                let id = format!("{sid}:{index}");
                let dated_by = if self.times[sid][index as usize].is_none() {
                    "connect"
                } else if pack.sources[sid].time.as_ref().is_some_and(|c| c.document.is_some()) {
                    "document"
                } else {
                    "record"
                };
                Event {
                    acked: self.acked.contains(&id),
                    id,
                    source: sid.clone(),
                    pointer: r.pointer.clone(),
                    excerpt: table.excerpt(r).to_string(),
                    participants,
                    signals,
                    severity,
                    at: time::iso(t),
                    t,
                    dated_by: dated_by.into(),
                }
            })
            .collect();

        let mut links: Vec<Link> = pairs
            .into_iter()
            .map(|((a, b), records)| Link { a: pack.participants[order[a]].id.clone(), b: pack.participants[order[b]].id.clone(), weight: records.len(), records })
            .collect();
        links.sort_by(|x, y| y.weight.cmp(&x.weight).then_with(|| x.a.cmp(&y.a)).then_with(|| x.b.cmp(&y.b)));
        (events, truncated, undated, links)
    }

    fn source_views(&self) -> Vec<SourceView> {
        self.pack
            .source_order
            .iter()
            .filter_map(|sid| {
                let s = self.pack.source(sid)?;
                let conn = self.connected.get(sid);
                let table = self.tables.get(sid);
                Some(SourceView {
                    id: sid.clone(),
                    name: s.name.clone(),
                    kind: s.kind.clone(),
                    format: s.format.clone(),
                    transport: s.transport.clone(),
                    color: s.color.clone(),
                    confidence: s.confidence,
                    blurb: s.blurb.clone(),
                    connected: conn.is_some(),
                    latency_ms: conn.map(|c| c.latency_ms),
                    bytes: conn.map(|c| c.bytes),
                    error: self.errors.get(sid).cloned(),
                    records: table.map_or(0, |t| t.records.len()),
                    fragments: if table.is_some() { self.fragments.iter().filter(|f| &f.source == sid).count() } else { 0 },
                    skipped: table.map_or(0, |t| t.skipped.len()),
                    skipped_examples: table.map_or(Vec::new(), |t| t.skipped.iter().take(5).cloned().collect()),
                })
            })
            .collect()
    }

    fn committed_views(&self) -> Vec<CommittedMerge> {
        if self.committed.is_empty() {
            return Vec::new();
        }
        let wanted: HashSet<&str> = self.committed.iter().flat_map(|pid| pid.split('|')).collect();
        let found: HashMap<&str, &Fragment> = self.fragments.iter().filter(|f| wanted.contains(f.id.as_str())).map(|f| (f.id.as_str(), f)).collect();
        let props = |id: &str| -> Option<Value> {
            let f = found.get(id)?;
            let mut out = Map::new();
            for (name, prop) in &f.props {
                out.insert(name.clone(), json!({"value": prop.value, "source": f.source, "pointer": f.pointer, "field": prop.field}));
            }
            Some(Value::Object(out))
        };
        self.committed
            .iter()
            .filter_map(|pid| {
                let (a, b) = pid.split_once('|')?;
                let source_of = |id: &'_ str| id.split('#').next().unwrap_or("").to_string();
                Some(CommittedMerge { id: pid.clone(), active: found.contains_key(a) && found.contains_key(b), a: props(a), b: props(b), a_source: source_of(a), b_source: source_of(b) })
            })
            .collect()
    }

    fn last_audit(&self) -> Vec<AuditEntry> {
        self.audit[self.audit.len().saturating_sub(80)..].to_vec()
    }

    // ------------------------------------------------------------ actions

    fn log_entry(&mut self, action: &str, detail: String) {
        self.audit.push(AuditEntry { at: time::iso((self.clock)()), action: action.to_string(), detail });
    }

    /// The parts of the state that change outside a recompute.
    fn sync_log(&mut self) {
        let (audit, score, earned) = (self.last_audit(), self.earned.values().sum(), self.earned.keys().cloned().collect());
        if let Some(st) = self.state.as_mut() {
            st.audit = audit;
            st.score = score;
            st.earned = earned;
        }
    }

    fn earn(&mut self, key: &str, kind: &str, events: &mut Vec<Insight>, fmt: Fmt) {
        if self.earned.contains_key(key) {
            return;
        }
        let (points, title, text) = insight_template(kind);
        let over = self.pack.narrative.get("insights").and_then(|i| i.get(kind));
        let pick = |name: &str, default: &str| over.and_then(|o| o.get(name)).and_then(Value::as_str).unwrap_or(default).to_string();
        let (title, text) = (format_text(&pick("title", title), &fmt), format_text(&pick("text", text), &fmt));
        self.earned.insert(key.to_string(), points);
        self.log_entry("insight", format!("{title} (+{points})"));
        events.push(Insight { kind: kind.to_string(), title, text, points });
    }

    /// Run a change, recompute, and earn what it moved. A change that fails
    /// must fail before touching anything.
    fn transact(&mut self, change: impl FnOnce(&mut Session, &mut Vec<Insight>) -> Result<(), String>) -> Result<Vec<Insight>, String> {
        let before: HashMap<String, String> = self.state().nodes.iter().map(|n| (n.id.clone(), n.status.clone())).collect();
        let mut events = Vec::new();
        change(self, &mut events)?;
        self.recompute();
        self.diff(&before, &mut events);
        self.sync_log();
        Ok(events)
    }

    fn diff(&mut self, before: &HashMap<String, String>, events: &mut Vec<Insight>) {
        let mut earned: Vec<(String, &'static str, Fmt)> = Vec::new();
        {
            let st = self.state();
            let band: HashMap<&str, &str> = st.nodes.iter().map(|n| (n.id.as_str(), n.band.as_str())).collect();
            let names: HashMap<&str, String> = st.nodes.iter().map(|n| (n.id.as_str(), format!("{} {}", n.code, n.label))).collect();
            if st.edges.iter().any(|e| e.flow > 0.0 && band.get(e.to.as_str()) == Some(&"participant") && band.get(e.from.as_str()) == Some(&"signal")) {
                earned.push(("first_link".into(), "first_link", Vec::new()));
            }
            for n in &st.nodes {
                let was = before.get(&n.id).map(String::as_str);
                let name = || vec![("label", names[n.id.as_str()].clone())];
                match &n.detail {
                    Detail::Participant(_) if n.status == "resolved" && was != Some("resolved") => earned.push((format!("identity_resolved:{}", n.id), "identity_resolved", name())),
                    Detail::Signal(d) if n.status == "firing" && was != Some("firing") && d.kind == "co_resolved" => earned.push((format!("fusion_signal:{}", n.id), "fusion_signal", name())),
                    Detail::Hypothesis(_) if n.status == "captured" && was != Some("captured") => earned.push((format!("explained_away:{}", n.id), "explained_away", name())),
                    _ => {}
                }
            }
            for n in &st.nodes {
                let Detail::Hypothesis(d) = &n.detail else { continue };
                for i in &d.breakdown.inhibit {
                    let cut = i.w * i.p;
                    if cut >= 0.3 {
                        let source = names.get(i.from.as_str()).cloned().unwrap_or_else(|| i.from.clone());
                        earned.push(("inhibited".into(), "inhibited", vec![("source", source), ("target", names[n.id.as_str()].clone()), ("pct", round_int(cut * 100.0).to_string())]));
                    }
                }
            }
            if self.connected.len() == self.pack.sources.len() {
                earned.push(("full_fusion".into(), "full_fusion", Vec::new()));
            }
        }
        for (key, kind, fmt) in earned {
            self.earn(&key, kind, events, fmt);
        }
    }

    /// Apply an action payload (spec §10) and return the insights it earned.
    /// The server handles `connect` for a source with no payload yet (see
    /// [`Session::has_payload`]), and the `agent_*` actions.
    pub fn act(&mut self, payload: &Value) -> Result<Vec<Insight>, String> {
        let action = payload.get("action").and_then(Value::as_str).unwrap_or("");
        match action {
            "agent_reset" | "agent_decide" => Err("agent actions live in the server".into()),
            "reset" => {
                self.reset();
                Ok(vec![Insight { kind: "reset".into(), title: "Board reset".into(), text: String::new(), points: 0 }])
            }
            "connect" => {
                let sid = field(payload, "source")?;
                if !self.has_payload(sid) {
                    return Err(format!("no payload for source '{sid}': fetch and connect it"));
                }
                self.transact(|s, _| s.bring_back(sid))
            }
            "reconnect" => {
                let list = payload.get("sources").and_then(Value::as_array).ok_or("missing or unknown field 'sources'")?;
                let sources: Vec<&str> = list.iter().filter_map(Value::as_str).filter(|sid| !self.is_connected(sid)).collect();
                let missing: Vec<&str> = sources.iter().copied().filter(|sid| !self.has_payload(sid)).collect();
                if !missing.is_empty() {
                    return Err(format!("no payload for source(s) {}: fetch and connect them", missing.join(", ")));
                }
                self.transact(|s, _| sources.iter().try_for_each(|sid| s.bring_back(sid)))
            }
            "disconnect" => Ok(self.disconnect(field(payload, "source")?)),
            "engine" => {
                let engine = field(payload, "engine")?;
                if !matches!(engine, "rules" | "jev") {
                    return Err("engine must be rules or jev".into());
                }
                self.transact(|s, _| {
                    s.engine = engine.to_string();
                    s.log_entry("engine", format!("evidence engine -> {engine}"));
                    Ok(())
                })
            }
            "keep" => {
                let id = field(payload, "hypothesis")?;
                self.transact(|s, _| {
                    s.kept.insert(id.to_string());
                    s.log_entry("keep-on-board", format!("{id}: kept on the board despite the evidence"));
                    Ok(())
                })
            }
            "release" => {
                let id = field(payload, "hypothesis")?;
                self.transact(|s, _| {
                    s.kept.remove(id);
                    s.log_entry("release", format!("{id}: the board may move it aside again"));
                    Ok(())
                })
            }
            "judge" => {
                let (signal, judgement) = (field(payload, "signal")?, field(payload, "judgement")?);
                if judgement_factor(judgement).is_none() {
                    return Err("judgement must be trust, discount or dismiss".into());
                }
                self.transact(|s, _| {
                    s.judgements.insert(signal.to_string(), judgement.to_string());
                    s.log_entry("judge", format!("{signal} -> {judgement}"));
                    Ok(())
                })
            }
            "merge" => {
                let pid = proposal_id(payload)?;
                // Proposals pair clusters of one type; a cluster has one type throughout.
                let mut types = pid.split('|').map(fragment_type);
                if let (Some(Some(a)), Some(Some(b))) = (types.next(), types.next()) {
                    if a != b {
                        return Err(format!("a merge joins fragments of one type, not {a} and {b}"));
                    }
                }
                self.transact(|s, events| {
                    if !s.committed.iter().any(|p| p == pid) {
                        s.committed.push(pid.to_string());
                    }
                    s.rejected.remove(pid);
                    s.resolved = None;
                    s.log_entry("merge", pid.to_string());
                    s.earn("merge_committed", "merge_committed", events, Vec::new());
                    Ok(())
                })
            }
            "reject" => {
                let pid = proposal_id(payload)?;
                self.transact(|s, _| {
                    s.rejected.insert(pid.to_string());
                    s.resolved = None;
                    s.log_entry("reject-merge", pid.to_string());
                    Ok(())
                })
            }
            "unmerge" => {
                let pid = proposal_id(payload)?;
                self.transact(|s, _| {
                    s.committed.retain(|p| p != pid);
                    s.resolved = None;
                    s.log_entry("revert-merge", pid.to_string());
                    Ok(())
                })
            }
            "feedback" => {
                let on = payload.get("on").ok_or("missing or unknown field 'on'")?;
                self.transact(|s, _| {
                    s.feedback = py::truthy(on);
                    s.log_entry("feedback", if s.feedback { "on" } else { "off" }.to_string());
                    Ok(())
                })
            }
            "peek" => {
                let source = field(payload, "source")?;
                self.transact(|s, events| {
                    s.peeked.insert(source.to_string());
                    s.log_entry("peek", source.to_string());
                    s.earn("shape_shock", "shape_shock", events, Vec::new());
                    Ok(())
                })
            }
            "ack" | "unack" => {
                let eid = payload.get("event").and_then(Value::as_str).unwrap_or("");
                let (source, pointer) = self.locate_event(eid).ok_or_else(|| format!("unknown event '{eid}'"))?;
                self.transact(|s, _| {
                    if action == "ack" {
                        s.acked.insert(eid.to_string());
                    } else {
                        s.acked.remove(eid);
                    }
                    s.log_entry(action, format!("{eid} ({source} {pointer})"));
                    Ok(())
                })
            }
            "crown" => {
                let id = field(payload, "hypothesis")?;
                self.transact(|s, events| s.crown_hypothesis(id, events))
            }
            other => Err(format!("unknown action '{other}'")),
        }
    }

    /// The source and pointer of a record of a connected source, from its event id.
    fn locate_event(&self, eid: &str) -> Option<(String, String)> {
        let (source, index) = eid.rsplit_once(':')?;
        let r = self.tables.get(source)?.get(index.parse().ok()?)?;
        Some((source.to_string(), r.pointer.clone()))
    }

    fn crown_hypothesis(&mut self, id: &str, events: &mut Vec<Insight>) -> Result<(), String> {
        let st = self.state();
        let hyps: Vec<&Node> = st.nodes.iter().filter(|n| n.band == "hypothesis").collect();
        let n = hyps.iter().find(|n| n.id == id).ok_or_else(|| format!("unknown hypothesis '{id}'"))?;
        let mut best = hyps[0];
        for h in &hyps {
            if h.p > best.p {
                best = h;
            }
        }
        let Detail::Hypothesis(d) = &n.detail else { unreachable!("hypothesis band holds hypotheses") };
        let label = format!("{} {}", n.code, n.label);
        let (verdict, kind) = if best.id != id {
            ("contradicted", "crowned_contradicted")
        } else if n.p < CROWNABLE {
            ("premature", "crowned_premature")
        } else {
            ("backed", "crowned")
        };
        let mut reasons: Vec<&Term> = d.breakdown.support.iter().collect();
        reasons.sort_by(|a, b| b.term.total_cmp(&a.term));
        let at = time::iso((self.clock)());
        let crown = Crown { hypothesis: id.to_string(), p: n.p, verdict: verdict.to_string(), at, reasons: reasons.iter().take(3).map(|r| CrownReason { from: r.from.clone(), term: r.term }).collect() };
        let fmt = vec![
            ("label", label.clone()),
            ("p", round_int(n.p * 100.0).to_string()),
            ("best", format!("{} {}", best.code, best.label)),
            ("best_p", round_int(best.p * 100.0).to_string()),
        ];
        let line = format!("{label} at {}% ({verdict})", round_int(n.p * 100.0));
        self.crown = Some(crown);
        self.log_entry("crown", line);
        let key = if verdict == "backed" { format!("crown:{verdict}") } else { format!("crown:{verdict}:{id}:{}", self.audit.len()) };
        self.earn(&key, kind, events, fmt);
        Ok(())
    }

    /// Clear every decision. Parsed payloads stay, so `reconnect` still works without a fetch.
    pub fn reset(&mut self) {
        for sid in self.connected_sources() {
            self.detach(&sid);
        }
        self.errors.clear();
        self.committed.clear();
        self.rejected.clear();
        self.judgements.clear();
        self.crown = None;
        self.kept.clear();
        self.participant_sources.clear();
        self.feedback = false;
        self.engine = "rules".into();
        self.jev.clear();
        self.jev_calls = 0;
        self.jev_cached = 0;
        self.jev_errors = 0;
        self.jev_cost = 0.0;
        self.earned.clear();
        self.audit.clear();
        self.peeked.clear();
        self.acked.clear();
        self.resolved = None;
        self.recompute();
    }

    // ------------------------------------------------------------ Jev

    /// What to ask Jev about one source: one request per record, carrying
    /// every opted-in question whose `where` matches. `table` is the source's
    /// own table, or any parse of it for sources that are not connected
    /// (the cache inspector covers the whole pack).
    pub fn jev_jobs(&self, source: &str, table: &Table, skip_known: bool) -> Vec<JevJob> {
        let signals: Vec<&Signal> = self.pack.signals.iter().filter(|s| s.jev.is_some() && s.source.as_deref() == Some(source)).collect();
        let name = self.pack.source(source).map_or(source, |s| s.name.as_str());
        let mut jobs = Vec::new();
        for r in &table.records {
            let mut questions = Map::new();
            let mut ids = Vec::new();
            for sig in &signals {
                let Some(spec) = &sig.jev else { continue };
                if skip_known && self.jev.contains_key(&format!("{}|{source}|{}", sig.id, r.index)) {
                    continue;
                }
                if !pipeline::matches(&r.data, spec.r#where.as_ref()) {
                    continue;
                }
                let mut q = json!({"type": "noul", "instructions": spec.question});
                if let Some(criteria) = spec.criteria.as_ref().filter(|c| py::truthy(c)) {
                    q["criteria"] = criteria.clone();
                }
                questions.insert(sig.id.replace('-', "_"), q);
                ids.push(sig.id.clone());
            }
            if !questions.is_empty() {
                jobs.push(JevJob {
                    index: r.index,
                    pointer: r.pointer.clone(),
                    excerpt: table.excerpt(r).to_string(),
                    state: json!({"source": name, "record": r.data}),
                    questions: Value::Object(questions),
                    signals: ids,
                });
            }
        }
        jobs
    }

    /// Store answers keyed `"signal|source|index"` and recompute.
    pub fn set_jev_answers(&mut self, answers: impl IntoIterator<Item = (String, JevAnswer)>) -> Vec<Insight> {
        self.transact(|s, _| {
            s.jev.extend(answers);
            Ok(())
        })
        .unwrap_or_default()
    }

    /// Drop stored answers, for the cache inspector's `forget`. Does not recompute.
    pub fn forget_jev_answers(&mut self, mut forget: impl FnMut(&str) -> bool) {
        self.jev.retain(|k, _| !forget(k));
    }

    /// Record the results of the requests made for one source: answers per
    /// signal, the stats and the audit line. Results are by record index.
    pub fn apply_jev_results(&mut self, source: &str, results: &[(u32, JevResult)]) -> Vec<Insight> {
        let signals: Vec<String> = self.pack.signals.iter().filter(|s| s.jev.is_some() && s.source.as_deref() == Some(source)).map(|s| s.id.clone()).collect();
        self.transact(|s, _| {
            let (mut calls, mut cached, mut errors, mut cost) = (0u64, 0u64, 0u64, 0.0);
            let mut latencies = Vec::new();
            for (index, res) in results {
                if res.error.is_some() {
                    errors += 1;
                } else if res.cached {
                    cached += 1;
                } else {
                    calls += 1;
                    cost += res.cost.unwrap_or(0.0);
                    latencies.push(res.ms.unwrap_or(0));
                }
                for sig in &signals {
                    let answer = res.answers.as_ref().and_then(|a| a.get(sig.replace('-', "_")));
                    if answer.is_none() && res.error.is_none() {
                        continue;
                    }
                    s.jev.insert(
                        format!("{sig}|{source}|{index}"),
                        JevAnswer { noul: answer.and_then(|a| a.get("noul")).and_then(Value::as_f64), model: res.model.clone(), ms: res.ms, cached: res.cached, error: res.error.clone() },
                    );
                }
            }
            s.jev_calls += calls;
            s.jev_cached += cached;
            s.jev_errors += errors;
            s.jev_cost += cost;
            let avg = if latencies.is_empty() { String::new() } else { format!(", avg {} ms", latencies.iter().sum::<u64>() / latencies.len() as u64) };
            s.log_entry("jev", format!("{source}: {} records, {calls} calls, {cached} cached, {errors} errors, ${cost:.5}{avg}", results.len()));
            Ok(())
        })
        .unwrap_or_default()
    }
}

/// A hypothesis's place on the board (spec §7).
pub(crate) struct Standing {
    pub captured: bool,
    pub kept: bool,
    pub status: &'static str,
    pub present: bool,
}

fn field<'a>(payload: &'a Value, name: &str) -> Result<&'a str, String> {
    payload.get(name).and_then(Value::as_str).ok_or_else(|| format!("missing or unknown field '{name}'"))
}

fn proposal_id(payload: &Value) -> Result<&str, String> {
    let pid = field(payload, "proposal")?;
    if pid.split('|').count() == 2 {
        Ok(pid)
    } else {
        Err(format!("not a proposal id: '{pid}'"))
    }
}

/// The object type in a fragment id `<source>#<record index>:<Type>:<mapping index>`.
fn fragment_type(id: &str) -> Option<&str> {
    let (source, index) = rules::fragment_record(id)?;
    let head = id.rsplit_once(':')?.0;
    head.strip_prefix(source)?.strip_prefix('#')?.strip_prefix(index.to_string().as_str())?.strip_prefix(':')
}

/// Epoch ms of each record, before any undated policy. A document time that
/// cannot be derived and a value that does not parse both count as undated.
fn record_times(table: &Table, source: &Source) -> Vec<Option<i64>> {
    let Some(conf) = source.time.as_ref() else { return vec![None; table.records.len()] };
    if conf.document.is_some() {
        let t = time::document_time(&table.raw, source, conf).ok();
        return vec![t; table.records.len()];
    }
    table.records.iter().map(|r| time::record_time(&r.data, conf).ok().flatten()).collect()
}

fn anchor_of<'a>(res: &'a Resolution, e: &Participant) -> Option<&'a Cluster> {
    let (prop, value) = e.anchor.iter().next()?;
    resolve::anchor(res, &e.otype, prop, &string_form(value))
}

/// A cluster counts as titled when its title is not empty. Empty happens: a
/// `strip` transform can turn whitespace into the empty string.
fn cluster_titled(c: &Cluster) -> bool {
    c.title.as_deref().is_some_and(|t| !t.is_empty())
}

/// Participant positions in id order.
pub(crate) fn sorted_participants(pack: &Pack) -> Vec<usize> {
    let mut order: Vec<usize> = (0..pack.participants.len()).collect();
    order.sort_by(|a, b| pack.participants[*a].id.cmp(&pack.participants[*b].id));
    order
}

/// Bits of the participants a signal may implicate. An empty `types` means any type.
pub(crate) fn allowed_mask(sig: &Signal, participants: &[Participant]) -> u64 {
    let types = sig.implicates.types.as_ref().filter(|t| !t.is_empty());
    participants.iter().enumerate().filter(|(_, e)| types.is_none_or(|t| t.contains(&e.otype))).fold(0, |m, (i, _)| m | 1 << i)
}

fn mask_ids(mask: u64, order: &[usize], pack: &Pack) -> Vec<String> {
    if mask == 0 {
        return Vec::new();
    }
    order.iter().filter(|i| (mask >> **i) & 1 == 1).map(|i| pack.participants[*i].id.clone()).collect()
}

fn rule_json(sig: &Signal) -> Value {
    let mut rule = Map::new();
    if let Some(source) = &sig.source {
        rule.insert("source".into(), json!(source));
    }
    if let Some(where_) = &sig.r#where {
        rule.insert("where".into(), where_.clone());
    }
    if let Some(params) = &sig.params {
        rule.insert("params".into(), params.clone());
    }
    rule.insert("strength".into(), serde_json::to_value(&sig.strength).unwrap_or(Value::Null));
    Value::Object(rule)
}

fn implicates_json(sig: &Signal) -> Value {
    let mut imp = Map::new();
    if let Some(types) = &sig.implicates.types {
        imp.insert("types".into(), json!(types));
    }
    imp.insert("w".into(), json!(sig.implicates.w));
    Value::Object(imp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insight_text_formats_like_str_format() {
        let fmt: Fmt = vec![("label", "H1 Ring".into()), ("p", "91".into())];
        assert_eq!(format_text("Crowned: {label} at {p}%", &fmt), "Crowned: H1 Ring at 91%");
        assert_eq!(format_text("a {{b}} {label}", &fmt), "a {b} H1 Ring");
        assert_eq!(format_text("{missing} stays", &fmt), "{missing} stays");
        assert_eq!(format_text("tail {", &fmt), "tail {");
    }

    #[test]
    fn fragment_ids_name_their_type() {
        assert_eq!(fragment_type("orders#107:Seller:1"), Some("Seller"));
        assert_eq!(fragment_type("a#b#3:T:U:0"), Some("T:U"));
        assert_eq!(fragment_type("nothing"), None);
    }

    #[test]
    fn percentages_round_ties_to_even_as_python_does() {
        assert_eq!(round_int(0.5), 0);
        assert_eq!(round_int(1.5), 2);
        assert_eq!(round_int(2.5), 2);
        assert_eq!(round_int(84.4), 84);
    }

    #[test]
    fn capture_reason_keeps_the_wording_of_the_spec() {
        let i = |from: &str, w: f64, p: f64| Inhibitor { from: from.into(), w, p, factor: 1.0 - w * p };
        let b = Breakdown {
            leak: 0.1,
            support: vec![],
            inhibit: vec![i("a", 0.5, 0.2), i("b", 0.9, 0.9), i("c", 0.1, 0.1), i("d", 0.5, 0.5), i("e", 0.5, 0.6)],
            support_p: 0.1,
            inhibit_factor: 0.1,
            raw: 0.01,
            divisor: Some(1.5),
            r#final: 0.01,
        };
        let names = HashMap::from([("b", "S2 Bee".to_string())]);
        assert_eq!(
            capture_reason(&b, 0.0649, &names),
            "Explained away at 6%: S2 Bee cut it by 81%; e cut it by 30%; d cut it by 25%. Competing explanations then divided what was left by 1.50."
        );
        let quiet = Breakdown { divisor: Some(1.0), ..b };
        assert!(!capture_reason(&quiet, 0.05, &names).contains("Competing"));
    }
}

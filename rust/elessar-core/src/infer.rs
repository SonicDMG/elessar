//! Noisy-OR with inhibitors and competition (spec §6). Phase 2.

use crate::types::{Breakdown, Inhibitor, Term};
use indexmap::IndexMap;
use std::collections::HashMap;

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

/// Noisy-OR for one node: `support = 1 - (1 - leak) * prod(1 - w*p)`,
/// `inhibit = prod(1 - v*p)`, `raw = support * inhibit`. `final` starts as `raw`.
fn noisy_or(leak: f64, supports: Vec<(String, f64, f64)>, inhibitors: Vec<(String, f64, f64)>) -> Breakdown {
    let mut keep = 1.0 - leak;
    let mut support = Vec::with_capacity(supports.len());
    for (from, w, p) in supports {
        let term = w * p;
        keep *= 1.0 - term;
        support.push(Term { from, w, p, term });
    }
    let support_p = 1.0 - keep;
    let mut inhibit_factor = 1.0;
    let mut inhibit = Vec::with_capacity(inhibitors.len());
    for (from, w, p) in inhibitors {
        let factor = 1.0 - w * p;
        inhibit_factor *= factor;
        inhibit.push(Inhibitor { from, w, p, factor });
    }
    let raw = support_p * inhibit_factor;
    Breakdown { leak, support, inhibit, support_p, inhibit_factor, raw, divisor: None, r#final: raw }
}

struct Pass<'a> {
    participants: &'a IndexMap<String, ParticipantIn>,
    priors: &'a IndexMap<String, f64>,
    edges: &'a [EdgeIn],
    p: IndexMap<String, f64>,
    breakdown: IndexMap<String, Option<Breakdown>>,
    stages: Vec<IndexMap<String, f64>>,
}

impl Pass<'_> {
    fn belief(&self, id: &str) -> f64 {
        self.p.get(id).copied().unwrap_or(0.0)
    }

    fn participants(&mut self, boost: Option<&HashMap<&str, f64>>) {
        let participants = self.participants;
        for (id, e) in participants {
            if !e.present {
                self.p.insert(id.clone(), 0.0);
                self.breakdown.insert(id.clone(), None);
                continue;
            }
            let mut sup: Vec<(String, f64, f64)> = e.parents.iter().map(|par| (par.from.clone(), par.w, self.belief(&par.from))).collect();
            // A participant no hypothesis pushes back on gets no feedback term at all.
            if let Some(&b) = boost.and_then(|m| m.get(id.as_str())) {
                if b != 0.0 {
                    sup.push(("feedback".to_string(), FEEDBACK_BETA, b));
                }
            }
            let b = noisy_or(e.leak, sup, Vec::new());
            self.p.insert(id.clone(), b.raw);
            self.breakdown.insert(id.clone(), Some(b));
        }
        self.stages.push(self.p.clone());
    }

    fn hypotheses(&mut self) {
        let (priors, edges) = (self.priors, self.edges);
        let mut total = 0.0;
        for (id, prior) in priors {
            let parents = |kind: &str| -> Vec<(String, f64, f64)> {
                edges.iter().filter(|e| &e.to == id && e.kind == kind).map(|e| (e.from.clone(), e.w, self.belief(&e.from))).collect()
            };
            let b = noisy_or(*prior, parents("support"), parents("inhibit"));
            total += b.raw;
            self.p.insert(id.clone(), b.raw);
            self.breakdown.insert(id.clone(), Some(b));
        }
        self.stages.push(self.p.clone());
        let divisor = if total > 1.0 { total } else { 1.0 };
        for id in priors.keys() {
            if let Some(Some(b)) = self.breakdown.get_mut(id) {
                b.divisor = Some(divisor);
                b.r#final = b.raw / divisor;
                self.p.insert(id.clone(), b.r#final);
            }
        }
        self.stages.push(self.p.clone());
    }
}

/// Products left to right in `f64`, in the orders the spec fixes.
pub fn infer(signals: &IndexMap<String, f64>, participants: &IndexMap<String, ParticipantIn>, priors: &IndexMap<String, f64>, edges: &[EdgeIn], feedback: bool) -> Inference {
    let mut pass = Pass { participants, priors, edges, p: signals.clone(), breakdown: IndexMap::new(), stages: vec![signals.clone()] };
    pass.participants(None);
    pass.hypotheses();
    if feedback {
        let mut boost: HashMap<&str, f64> = HashMap::new();
        for e in edges {
            if e.kind == "support" && participants.contains_key(&e.from) && priors.contains_key(&e.to) {
                let b = pass.belief(&e.to) * e.w;
                let slot = boost.entry(e.from.as_str()).or_insert(0.0);
                *slot = slot.max(b);
            }
        }
        pass.participants(Some(&boost));
        pass.hypotheses();
    }
    Inference { p: pass.p, breakdown: pass.breakdown, stages: pass.stages }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(items: &[(&str, f64, bool, &[(&str, f64)])]) -> IndexMap<String, ParticipantIn> {
        items.iter().map(|(id, leak, present, parents)| {
            ((*id).to_string(), ParticipantIn { leak: *leak, present: *present, parents: parents.iter().map(|(f, w)| Parent { from: (*f).to_string(), w: *w }).collect() })
        }).collect()
    }

    fn map(items: &[(&str, f64)]) -> IndexMap<String, f64> {
        items.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    fn edge(from: &str, to: &str, w: f64, kind: &str) -> EdgeIn {
        EdgeIn { from: from.into(), to: to.into(), w, kind: kind.into() }
    }

    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-12, "{a} != {b}");
    }

    #[test]
    fn participant_noisy_or_by_hand() {
        // 1 - (1 - 0.02)(1 - 0.5*0.8)(1 - 0.25*0.4) = 1 - 0.98*0.6*0.9 = 0.4708
        let out = infer(&map(&[("s1", 0.8), ("s2", 0.4)]), &parts(&[("a", 0.02, true, &[("s1", 0.5), ("s2", 0.25)])]), &map(&[("h", 0.1)]), &[], false);
        near(out.p["a"], 0.4708);
        let b = out.breakdown["a"].as_ref().unwrap();
        near(b.support[0].term, 0.4);
        near(b.support[1].term, 0.1);
        assert_eq!(b.divisor, None);
        assert_eq!(b.r#final, b.raw);
    }

    #[test]
    fn absent_participant_is_zero_with_no_breakdown() {
        let out = infer(&map(&[("s", 1.0)]), &parts(&[("a", 0.02, false, &[("s", 0.9)])]), &map(&[("h", 0.1)]), &[edge("a", "h", 0.5, "support")], false);
        assert_eq!(out.p["a"], 0.0);
        assert!(out.breakdown["a"].is_none());
        near(out.p["h"], 0.1);
    }

    #[test]
    fn inhibitor_multiplies_support() {
        // support = 1 - 0.9*(1 - 0.5*0.8) = 0.46; inhibit = 1 - 0.5*0.6 = 0.7; raw = 0.322
        let out = infer(&map(&[("s", 0.8), ("t", 0.6)]), &IndexMap::new(), &map(&[("h", 0.1)]), &[edge("s", "h", 0.5, "support"), edge("t", "h", 0.5, "inhibit")], false);
        let b = out.breakdown["h"].as_ref().unwrap();
        near(b.support_p, 0.46);
        near(b.inhibit[0].factor, 0.7);
        near(b.inhibit_factor, 0.7);
        near(b.raw, 0.322);
        assert_eq!(b.divisor, Some(1.0));
        near(out.p["h"], 0.322);
    }

    #[test]
    fn competition_divides_by_the_sum_only_above_one() {
        // raws: 1 - 0.5*(1 - 0.8) = 0.9 and 1 - 0.5*(1 - 0.4) = 0.7, sum 1.6
        let edges = [edge("s", "h1", 0.8, "support"), edge("s", "h2", 0.4, "support")];
        let out = infer(&map(&[("s", 1.0)]), &IndexMap::new(), &map(&[("h1", 0.5), ("h2", 0.5)]), &edges, false);
        near(out.breakdown["h1"].as_ref().unwrap().divisor.unwrap(), 1.6);
        near(out.p["h1"], 0.9 / 1.6);
        near(out.p["h2"], 0.7 / 1.6);
        // Exactly one is not "exceeds": raws 0.5 + 0.5 stay as they are.
        let out = infer(&IndexMap::new(), &IndexMap::new(), &map(&[("h1", 0.5), ("h2", 0.5)]), &[], false);
        assert_eq!(out.breakdown["h1"].as_ref().unwrap().divisor, Some(1.0));
        assert_eq!(out.p["h1"], 0.5);
    }

    #[test]
    fn stages_are_cumulative_snapshots() {
        let edges = [edge("s", "h1", 0.8, "support"), edge("s", "h2", 0.4, "support")];
        let out = infer(&map(&[("s", 1.0)]), &parts(&[("a", 0.1, true, &[("s", 0.5)])]), &map(&[("h1", 0.5), ("h2", 0.5)]), &edges, false);
        assert_eq!(out.stages.len(), 4);
        assert_eq!(out.stages[0].len(), 1);
        assert_eq!(out.stages[1].len(), 2);
        near(out.stages[2]["h1"], 0.9);
        near(out.stages[3]["h1"], 0.9 / 1.6);
        assert_eq!(out.stages[3], out.p);
    }

    #[test]
    fn feedback_adds_one_term_then_repeats_both_passes() {
        // First pass: a = 1 - 0.9*0.5 = 0.55; h = 1 - 0.8*(1 - 0.5*0.55) = 0.42.
        // Boost = 0.42*0.5 = 0.21, so a = 1 - 0.9*0.5*(1 - 0.15*0.21) = 0.564175.
        let out = infer(&map(&[("s", 1.0)]), &parts(&[("a", 0.1, true, &[("s", 0.5)]), ("idle", 0.1, true, &[])]), &map(&[("h", 0.2)]), &[edge("a", "h", 0.5, "support")], true);
        assert_eq!(out.stages.len(), 7);
        let b = out.breakdown["a"].as_ref().unwrap();
        assert_eq!(b.support.last().unwrap().from, "feedback");
        assert_eq!(b.support.last().unwrap().w, FEEDBACK_BETA);
        near(b.support.last().unwrap().p, 0.21);
        near(out.p["a"], 0.564175);
        near(out.p["h"], 1.0 - 0.8 * (1.0 - 0.5 * 0.564175));
        // Supports no hypothesis, so no boost and no term.
        assert!(out.breakdown["idle"].as_ref().unwrap().support.is_empty());
    }

    #[test]
    fn feedback_takes_the_max_over_supported_hypotheses() {
        let edges = [edge("a", "h1", 0.5, "support"), edge("a", "h2", 1.0, "support")];
        let out = infer(&IndexMap::new(), &parts(&[("a", 0.5, true, &[])]), &map(&[("h1", 0.0), ("h2", 0.0)]), &edges, true);
        // First pass: h1 = 0.25, h2 = 0.5, sum below one; boost = max(0.125, 0.5).
        near(out.breakdown["a"].as_ref().unwrap().support[0].p, 0.5);
    }
}

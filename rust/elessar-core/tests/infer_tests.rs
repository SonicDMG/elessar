//! Phase 2: inference must reproduce the Python engine's beliefs from the same inputs.
//! The board golden's breakdowns carry each node's inputs (from, w, p), so inference is tested in isolation.
mod common;

use elessar_core::infer::{infer, EdgeIn, Parent, ParticipantIn};
use indexmap::IndexMap;

#[test]
#[ignore = "phase 2: remove when infer::infer is implemented"]
fn beliefs_match_the_python_engine() {
    for id in common::pack_ids() {
        let p = common::pack(&id);
        let g = common::golden("board", &id);
        let nodes = g["nodes"].as_object().unwrap();
        let mut signals = IndexMap::new();
        for s in &p.signals {
            signals.insert(s.id.clone(), nodes[s.id.as_str()]["p"].as_f64().unwrap());
        }
        let mut participants = IndexMap::new();
        for e in &p.participants {
            let n = &nodes[e.id.as_str()];
            let parents = n["breakdown"]["support"].as_array().map(|a| a.iter()
                .map(|t| Parent { from: t["from"].as_str().unwrap().into(), w: t["w"].as_f64().unwrap() }).collect()).unwrap_or_default();
            participants.insert(e.id.clone(), ParticipantIn { leak: e.leak, present: n["present"].as_bool().unwrap(), parents });
        }
        let priors: IndexMap<String, f64> = p.hypotheses.iter().map(|h| (h.id.clone(), h.prior)).collect();
        let edges: Vec<EdgeIn> = p.edges.iter().map(|e| EdgeIn { from: e.from.clone(), to: e.to.clone(), w: e.w, kind: e.kind.clone() }).collect();
        let out = infer(&signals, &participants, &priors, &edges, false);
        for (nid, n) in nodes {
            let want = n["p"].as_f64().unwrap();
            let got = out.p[nid];
            assert!(common::close(got, want), "{id} {nid}: want {want}, got {got}");
        }
        let stages = g["stages"].as_array().unwrap();
        assert_eq!(out.stages.len(), stages.len(), "{id}: stage count");
    }
}

//! Phase 2: inference must reproduce the Python engine's beliefs from the same inputs.
//! The board golden's breakdowns carry each node's inputs (from, w, p), so inference is tested in isolation.
mod common;

use elessar_core::infer::{infer, EdgeIn, Parent, ParticipantIn};
use indexmap::IndexMap;

#[test]
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

/// The board golden with every float kept as its text. serde_json's default parser can land
/// one ULP off on 17-digit numbers (no `float_roundtrip`), which a bit-exact test cannot have.
/// The golden is indented JSON, so a number that ends a line is never inside a string.
fn golden_exact(id: &str) -> serde_json::Value {
    let path = common::repo_root().join("golden/board").join(format!("{id}.json"));
    let text = std::fs::read_to_string(path).unwrap();
    let float = regex::Regex::new(r"(?m)^(\s*(?:\x22[^\x22]*\x22: )?)(-?\d+\.\d+(?:[eE][-+]?\d+)?)(,?)$").unwrap();
    serde_json::from_str(&float.replace_all(&text, "$1\"$2\"$3")).unwrap()
}

fn num(v: &serde_json::Value) -> f64 {
    v.as_str().map(|s| s.parse().unwrap()).or_else(|| v.as_f64()).unwrap_or_else(|| panic!("not a number: {v}"))
}

/// The same inputs, held to the last bit: every stage value and every breakdown number.
#[test]
fn stages_and_breakdowns_are_bit_exact() {
    fn same(ctx: &str, got: f64, want: &serde_json::Value) {
        let want = num(want);
        assert_eq!(got.to_bits(), want.to_bits(), "{ctx}: want {want}, got {got}");
    }
    for id in common::pack_ids() {
        let p = common::pack(&id);
        let g = golden_exact(&id);
        let nodes = g["nodes"].as_object().unwrap();
        let signals: IndexMap<String, f64> = p.signals.iter().map(|s| (s.id.clone(), num(&nodes[s.id.as_str()]["p"]))).collect();
        let mut participants = IndexMap::new();
        for e in &p.participants {
            let n = &nodes[e.id.as_str()];
            let parents = n["breakdown"]["support"].as_array().map(|a| a.iter()
                .map(|t| Parent { from: t["from"].as_str().unwrap().into(), w: num(&t["w"]) }).collect()).unwrap_or_default();
            participants.insert(e.id.clone(), ParticipantIn { leak: e.leak, present: n["present"].as_bool().unwrap(), parents });
        }
        let priors: IndexMap<String, f64> = p.hypotheses.iter().map(|h| (h.id.clone(), h.prior)).collect();
        let edges: Vec<EdgeIn> = p.edges.iter().map(|e| EdgeIn { from: e.from.clone(), to: e.to.clone(), w: e.w, kind: e.kind.clone() }).collect();
        let out = infer(&signals, &participants, &priors, &edges, false);

        for (i, want) in g["stages"].as_array().unwrap().iter().enumerate() {
            let want = want.as_object().unwrap();
            assert_eq!(out.stages[i].len(), want.len(), "{id} stage {i}: node count");
            for (nid, v) in want {
                same(&format!("{id} stage {i} {nid}"), out.stages[i][nid], v);
            }
        }
        for (nid, n) in nodes {
            let Some(b) = out.breakdown.get(nid) else { continue };
            let w = &n["breakdown"];
            let Some(b) = b else {
                assert!(w.is_null(), "{id} {nid}: golden has a breakdown, inference has none");
                continue;
            };
            let ctx = format!("{id} {nid}");
            same(&ctx, b.leak, &w["leak"]);
            same(&ctx, b.support_p, &w["support_p"]);
            same(&ctx, b.inhibit_factor, &w["inhibit_factor"]);
            same(&ctx, b.raw, &w["raw"]);
            same(&ctx, b.r#final, &w["final"]);
            match b.divisor {
                Some(d) => same(&ctx, d, &w["divisor"]),
                None => assert!(w.get("divisor").is_none(), "{ctx}: divisor"),
            }
            let (ws, wi) = (w["support"].as_array().unwrap(), w["inhibit"].as_array().unwrap());
            assert_eq!(b.support.len(), ws.len(), "{ctx}: support count");
            assert_eq!(b.inhibit.len(), wi.len(), "{ctx}: inhibit count");
            for (t, w) in b.support.iter().zip(ws) {
                assert_eq!(t.from, w["from"].as_str().unwrap(), "{ctx}: support order");
                same(&ctx, t.w, &w["w"]);
                same(&ctx, t.p, &w["p"]);
                same(&ctx, t.term, &w["term"]);
            }
            for (t, w) in b.inhibit.iter().zip(wi) {
                assert_eq!(t.from, w["from"].as_str().unwrap(), "{ctx}: inhibit order");
                same(&ctx, t.w, &w["w"]);
                same(&ctx, t.p, &w["p"]);
                same(&ctx, t.factor, &w["factor"]);
            }
        }
    }
}

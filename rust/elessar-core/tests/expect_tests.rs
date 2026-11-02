//! Phase 3: the acceptance specs (`expect.json`), end to end. Properties of the
//! session are in `properties_tests.rs`, the comparison with Python in `python_diff_tests.rs`.
mod common;

use elessar_core::session::Session;
use serde_json::{json, Value};

fn play(id: &str, order: &[String], spec: &Value) -> Session {
    let p = common::pack(id);
    let mut s = Session::new(p);
    for sid in order {
        let raw = common::read_source(&s.pack, sid);
        s.connect(sid, raw, 0);
        loop {
            let pending: Vec<Value> = serde_json::to_value(&s.state().merges.pending).unwrap().as_array().unwrap().clone();
            let Some(pr) = pending.first() else { break };
            let (a, b) = (pr["a"]["value"].as_str().unwrap_or("").to_string(), pr["b"]["value"].as_str().unwrap_or("").to_string());
            let reject = spec["merges"]["reject"].as_array().unwrap().iter().any(|pair| {
                let (x, y) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
                (x == a && y == b) || (x == b && y == a)
            });
            let decision = if reject { "reject" } else if spec["merges"]["default"] == "commit" { "merge" } else { "reject" };
            s.act(&json!({"action": decision, "proposal": pr["id"]})).unwrap();
        }
    }
    s
}

fn summary(s: &Session) -> Value {
    let st = s.state();
    let ids = |band: &str, status: &str| -> Vec<&str> {
        let mut v: Vec<&str> = st.nodes.iter().filter(|n| n.band == band && n.status == status).map(|n| n.id.as_str()).collect();
        v.sort();
        v
    };
    json!({
        "crownable": ids("hypothesis", "crownable"),
        "captured": ids("hypothesis", "captured"),
        "firing": ids("signal", "firing"),
        "resolved": ids("participant", "resolved"),
        "undated": st.undated.len(),
        "skipped": st.sources.iter().filter(|x| x.skipped > 0).map(|x| (x.id.clone(), json!(x.skipped))).collect::<serde_json::Map<String, Value>>(),
        "beliefs": st.nodes.iter().map(|n| (n.id.clone(), json!((n.p * 1e6).round() / 1e6))).collect::<serde_json::Map<String, Value>>(),
    })
}

#[test]
fn packs_meet_their_expect_json() {
    for id in common::pack_ids() {
        let p = common::pack(&id);
        let Ok(text) = std::fs::read_to_string(p.dir.join("expect.json")) else { continue };
        let spec: Value = serde_json::from_str(&text).unwrap();
        let s = play(&id, &p.source_order, &spec);
        let got = summary(&s);
        let want = &spec["final"];
        assert_eq!(got["crownable"], json!([want["crownable"]]), "{id}: crownable");
        for key in ["captured", "firing", "resolved"] {
            let mut w: Vec<&str> = want[key].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
            w.sort();
            assert_eq!(got[key], json!(w), "{id}: {key}");
        }
        assert_eq!(got["undated"], want["undated"], "{id}: undated");
        assert_eq!(got["skipped"], want["skipped"], "{id}: skipped");
    }
}

#[test]
fn connection_order_never_changes_the_final_board() {
    for id in common::pack_ids() {
        let p = common::pack(&id);
        let Ok(text) = std::fs::read_to_string(p.dir.join("expect.json")) else { continue };
        let spec: Value = serde_json::from_str(&text).unwrap();
        let base = summary(&play(&id, &p.source_order, &spec));
        let n = p.source_order.len();
        let orders: Vec<Vec<String>> = if spec["order_independent"] == "all" {
            permutations(&p.source_order)
        } else {
            let k = spec["order_independent"].as_u64().unwrap_or(0) as usize;
            (0..k).map(|i| { let mut o = p.source_order.clone(); o.rotate_left(i % n); if i % 2 == 1 { o.reverse(); } o }).collect()
        };
        for o in orders {
            assert_eq!(summary(&play(&id, &o, &spec)), base, "{id}: order {o:?} changed the final board");
        }
    }
}

/// Every proposal the spec pins shows up at some point while connecting, with no decision made.
#[test]
fn pinned_proposals_are_seen() {
    for id in common::pack_ids() {
        let p = common::pack(&id);
        let Ok(text) = std::fs::read_to_string(p.dir.join("expect.json")) else { continue };
        let spec: Value = serde_json::from_str(&text).unwrap();
        let mut s = Session::new(p.clone());
        let mut seen: Vec<(String, String)> = Vec::new();
        for sid in &p.source_order {
            let raw = common::read_source(&s.pack, sid);
            s.connect(sid, raw, 0);
            for pr in &s.state().merges.pending {
                seen.push((elessar_core::store::string_form(&pr.a.value), elessar_core::store::string_form(&pr.b.value)));
            }
        }
        for pair in spec["proposals_seen"].as_array().unwrap() {
            let (a, b) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
            assert!(seen.iter().any(|(x, y)| (x == a && y == b) || (x == b && y == a)), "{id}: {a} / {b} never proposed, saw {seen:?}");
        }
    }
}

fn permutations(items: &[String]) -> Vec<Vec<String>> {
    if items.len() <= 1 { return vec![items.to_vec()]; }
    let mut out = Vec::new();
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let head = rest.remove(i);
        for mut tail in permutations(&rest) { tail.insert(0, head.clone()); out.push(tail); }
    }
    out
}

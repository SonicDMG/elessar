//! The whole state and timeline against a fresh dump from the Python engine.
//!
//! Needs a dump, so it passes without checking anything when there is none:
//!
//!     python3 tools/py_dump.py . <dir>
//!     python3 tools/py_synth.py . <dir> 40      # optional: random packs under <dir>/synth
//!     ELESSAR_PY_DUMP=<dir> cargo test -p elessar-core --test python_diff_tests -- --nocapture
//!
//! `tools/py_dump.py` dumps the shipped packs; `tools/py_synth.py` writes random
//! small packs that reach the timeline sweep's corners under `<dir>/synth`.
//!
//! Two comparisons:
//! - **The engine's own decisions** (`expect.json`): the fields the spec says
//!   must agree are asserted; belief differences above 1e-6 are printed, since
//!   similarity may move proposals and so numbers.
//! - **Python's decisions replayed** (its merge ids applied here): similarity
//!   can no longer matter, so the full JSON must match within 1e-9 on every
//!   board and timeline, apart from what `ALLOWED` lists.
mod common;

use elessar_core::session::{JevAnswer, Session};
use elessar_core::timeline::timeline;
use elessar_core::types::Detail;
use serde_json::{json, Value};
use std::path::PathBuf;

const TOLERANCE: f64 = 1e-9;

/// Paths that may differ when decisions are the same, and why:
/// - `merges.pending`: which fuzzy pairs reach the threshold is the similarity's call (spec §13).
/// - `object_types`: the Rust loader fills defaults the frontend does not read (only keys are used).
const ALLOWED: &[&str] = &["merges.pending", "object_types"];

fn dump_of(id: &str) -> Option<Value> {
    let dir = PathBuf::from(std::env::var_os("ELESSAR_PY_DUMP")?);
    let text = std::fs::read_to_string(dir.join(format!("{id}.json"))).unwrap_or_else(|e| panic!("{id}: {e}"));
    Some(serde_json::from_str(&text).unwrap())
}

fn clock() -> i64 {
    // 2026-11-02T00:00:00Z, the dump's frozen clock.
    1_793_577_600_000
}

fn session(id: &str, policy: Option<&str>) -> Session {
    let mut p = common::pack(id);
    if let Some(policy) = policy {
        p.unknown_time = policy.to_string();
    }
    let mut s = Session::new(p);
    s.clock = clock;
    s.base_url = "offline".into();
    s
}

fn connect(s: &mut Session, sid: &str) -> Vec<elessar_core::types::Insight> {
    let raw = common::read_source(&s.pack, sid);
    s.connect(sid, raw, 0)
}

fn connect_all(s: &mut Session) {
    for sid in s.pack.source_order.clone() {
        connect(s, &sid);
    }
}

fn apply_decisions(s: &mut Session, dump: &Value) {
    for pid in dump["rejected"].as_array().unwrap() {
        s.act(&json!({"action": "reject", "proposal": pid})).unwrap();
    }
    for pid in dump["committed"].as_array().unwrap() {
        s.act(&json!({"action": "merge", "proposal": pid})).unwrap();
    }
}

/// Every difference between two JSON values, as `path: python vs rust`.
fn diff(path: &str, py: &Value, rs: &Value, tolerance: f64, out: &mut Vec<String>) {
    match (py, rs) {
        (Value::Number(a), Value::Number(b)) => {
            let (x, y) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            if (x - y).abs() > tolerance {
                out.push(format!("{path}: {x} vs {y}"));
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                out.push(format!("{path}: length {} vs {}", a.len(), b.len()));
            }
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                diff(&format!("{path}[{i}]"), x, y, tolerance, out);
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            for (k, x) in a {
                match b.get(k) {
                    Some(y) => diff(&format!("{path}.{k}"), x, y, tolerance, out),
                    None => out.push(format!("{path}.{k}: only in python ({x})")),
                }
            }
            for (k, y) in b {
                if !a.contains_key(k) {
                    out.push(format!("{path}.{k}: only in rust ({y})"));
                }
            }
        }
        _ if py != rs => out.push(format!("{path}: {py} vs {rs}")),
        _ => {}
    }
}

fn is_allowed(d: &str) -> bool {
    ALLOWED.iter().any(|a| d.contains(a))
}

/// Full JSON equality apart from `ALLOWED`. Reports up to 30 differences.
fn assert_same(what: &str, py: &Value, rs: &Value, failures: &mut Vec<String>) {
    let mut out = Vec::new();
    diff("", py, rs, TOLERANCE, &mut out);
    if std::env::var_os("ELESSAR_SHOW_ALLOWED").is_some() {
        for d in out.iter().filter(|d| is_allowed(d)) {
            println!("allowed: {what}: {d}");
        }
    }
    let real: Vec<&String> = out.iter().filter(|d| !is_allowed(d)).collect();
    if !real.is_empty() {
        failures.push(format!("{what}: {} difference(s)\n  {}", real.len(), real.iter().take(30).map(|s| s.as_str()).collect::<Vec<_>>().join("\n  ")));
    }
}

fn state_json(s: &Session) -> Value {
    serde_json::to_value(s.state()).unwrap()
}

fn pending_pairs(state: &Value) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = state["merges"]["pending"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let (a, b) = (p["a"]["value"].to_string(), p["b"]["value"].to_string());
            if a < b { (a, b) } else { (b, a) }
        })
        .collect();
    pairs.sort();
    pairs
}

fn finish(failures: Vec<String>) {
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn replaying_python_decisions_gives_the_same_json() {
    let mut failures = Vec::new();
    for id in common::pack_ids() {
        let Some(dump) = dump_of(&id) else {
            println!("no ELESSAR_PY_DUMP: nothing compared");
            return;
        };

        let mut s = session(&id, None);
        connect_all(&mut s);
        // The merge decisions are the same set; only their audit lines came in another order.
        apply_decisions(&mut s, &dump);
        let mut state = state_json(&s);
        let mut py_state = dump["decided"].clone();
        for st in [&mut state, &mut py_state] {
            for k in ["audit", "earned", "score"] {
                st[k] = Value::Null;
            }
        }
        assert_same(&format!("{id}.decided"), &py_state, &state, &mut failures);
        let tl = timeline(&mut s);
        assert_same(&format!("{id}.timeline"), &dump["timeline"], &tl, &mut failures);

        for policy in ["throughout", "after", "at_connect"] {
            let mut s = session(&id, Some(policy));
            connect_all(&mut s);
            apply_decisions(&mut s, &dump);
            let py = &dump["policies"][policy];
            let mut py_state = py["state"].clone();
            let mut state = state_json(&s);
            for st in [&mut state, &mut py_state] {
                for k in ["audit", "earned", "score"] {
                    st[k] = Value::Null;
                }
            }
            assert_same(&format!("{id}.{policy}.state"), &py_state, &state, &mut failures);
            let tl = timeline(&mut s);
            assert_same(&format!("{id}.{policy}.timeline"), &py["timeline"], &tl, &mut failures);
        }

        // Half the rack, one source connected then dropped: a participant may be `removed`.
        let half = dump["partial"]["sources"].as_array().unwrap().len() + 1;
        let mut s = session(&id, None);
        for sid in s.pack.source_order.clone().iter().take(half) {
            connect(&mut s, sid);
        }
        apply_decisions(&mut s, &dump);
        let first = s.pack.source_order[0].clone();
        s.disconnect(&first);
        let mut py = dump["partial"].clone();
        let mut state = state_json(&s);
        for st in [&mut state, &mut py["state"]] {
            for k in ["audit", "earned", "score"] {
                st[k] = Value::Null;
            }
        }
        assert_same(&format!("{id}.partial.state"), &py["state"], &state, &mut failures);
        let tl = timeline(&mut s);
        assert_same(&format!("{id}.partial.timeline"), &py["timeline"], &tl, &mut failures);

        // Judgements, feedback, a kept hypothesis.
        let mut s = session(&id, None);
        connect_all(&mut s);
        apply_decisions(&mut s, &dump);
        for m in dump["judged"]["moves"].as_array().unwrap() {
            s.act(m).unwrap();
        }
        let mut py = dump["judged"].clone();
        let mut state = state_json(&s);
        for st in [&mut state, &mut py["state"]] {
            for k in ["audit", "earned", "score"] {
                st[k] = Value::Null;
            }
        }
        assert_same(&format!("{id}.judged.state"), &py["state"], &state, &mut failures);
        let tl = timeline(&mut s);
        assert_same(&format!("{id}.judged.timeline"), &py["timeline"], &tl, &mut failures);
    }
    finish(failures);
}

#[test]
fn a_walk_through_every_action_matches_step_by_step() {
    let mut failures = Vec::new();
    for id in common::pack_ids() {
        let Some(dump) = dump_of(&id) else {
            println!("no ELESSAR_PY_DUMP: nothing compared");
            return;
        };
        let mut s = session(&id, None);
        for (i, step) in dump["script"].as_array().unwrap().iter().enumerate() {
            let payload = &step["payload"];
            let what = format!("{id}.script[{i}] {payload}");
            let insights = if payload["action"] == "connect" {
                connect(&mut s, payload["source"].as_str().unwrap())
            } else {
                match s.act(payload) {
                    Ok(i) => i,
                    Err(e) => {
                        failures.push(format!("{what}: {e}"));
                        continue;
                    }
                }
            };
            assert_same(&format!("{what} insights"), &step["insights"], &serde_json::to_value(&insights).unwrap(), &mut failures);
            assert_same(&format!("{what} state"), &step["state"], &state_json(&s), &mut failures);
        }
    }
    finish(failures);
}

/// Random packs, replayed step by step: state, insights and the whole timeline after every action.
#[test]
fn synthetic_packs_match_at_every_step() {
    let Some(dir) = std::env::var_os("ELESSAR_PY_DUMP") else {
        println!("no ELESSAR_PY_DUMP: nothing compared");
        return;
    };
    let root = PathBuf::from(dir).join("synth");
    let Ok(entries) = std::fs::read_dir(&root) else {
        println!("no synth packs in the dump: nothing compared");
        return;
    };
    let mut dirs: Vec<PathBuf> = entries.filter_map(|e| Some(e.ok()?.path())).collect();
    dirs.sort();
    let mut failures = Vec::new();
    let mut steps = 0;
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().into_owned();
        let dump: Value = serde_json::from_str(&std::fs::read_to_string(d.join("dump.json")).unwrap()).unwrap();
        let pack = elessar_core::pack::load_pack(d).unwrap_or_else(|e| panic!("{name}: {:?}", e.errors));
        let mut s = Session::new(pack);
        s.clock = clock;
        s.base_url = "offline".into();
        for step in dump["steps"].as_array().unwrap() {
            steps += 1;
            let payload = &step["payload"];
            let what = format!("{name} {payload}");
            let insights = if payload["action"] == "connect" {
                connect(&mut s, payload["source"].as_str().unwrap())
            } else {
                if payload["action"] == "engine" {
                    // Python stored these answers just before switching the engine.
                    let answers: Vec<(String, JevAnswer)> = dump["jev"].as_object().unwrap().iter().map(|(k, a)| (k.clone(), JevAnswer { noul: a["noul"].as_f64(), model: a["model"].as_str().map(String::from), ms: a["ms"].as_u64(), cached: true, error: None })).collect();
                    let earned = s.set_jev_answers(answers);
                    assert!(earned.is_empty(), "{what}: answers under the rules engine moved the board");
                }
                match s.act(payload) {
                    Ok(i) => i,
                    Err(e) => {
                        failures.push(format!("{what}: {e}"));
                        continue;
                    }
                }
            };
            assert_same(&format!("{what} insights"), &step["insights"], &serde_json::to_value(&insights).unwrap(), &mut failures);
            assert_same(&format!("{what} state"), &step["state"], &state_json(&s), &mut failures);
            assert_same(&format!("{what} timeline"), &step["timeline"], &timeline(&mut s), &mut failures);
        }
    }
    println!("{} packs, {steps} steps compared", dirs.len());
    finish(failures);
}

/// The engine's own decisions per `expect.json`, as the acceptance tests make them.
#[test]
fn own_decisions_agree_on_what_the_spec_fixes() {
    for id in common::pack_ids() {
        let Some(dump) = dump_of(&id) else {
            println!("no ELESSAR_PY_DUMP: nothing compared");
            return;
        };
        let spec: Value = serde_json::from_str(&std::fs::read_to_string(common::pack(&id).dir.join("expect.json")).unwrap()).unwrap();

        let mut s = session(&id, None);
        connect_all(&mut s);

        // Proposals before any decision: every pinned pair on both sides; the rest is reported.
        let (py_pairs, rs_pairs) = (pending_pairs(&dump["undecided"]), pending_pairs(&state_json(&s)));
        for pair in &py_pairs {
            if !rs_pairs.contains(pair) {
                println!("{id}: only python proposes {pair:?}");
            }
        }
        for pair in &rs_pairs {
            if !py_pairs.contains(pair) {
                println!("{id}: only rust proposes {pair:?}");
            }
        }

        // Decide until none is pending, as the Python dump did.
        loop {
            let Some(pr) = s.state().merges.pending.first().cloned() else { break };
            let (a, b) = (elessar_core::store::string_form(&pr.a.value), elessar_core::store::string_form(&pr.b.value));
            let reject = spec["merges"]["reject"].as_array().unwrap().iter().any(|pair| (pair[0] == a.as_str() && pair[1] == b.as_str()) || (pair[0] == b.as_str() && pair[1] == a.as_str()));
            let decision = if reject || spec["merges"]["default"] != "commit" { "reject" } else { "merge" };
            s.act(&json!({"action": decision, "proposal": pr.id})).unwrap();
        }

        let (rs, py) = (state_json(&s), &dump["decided"]);
        let by_id = |st: &Value| -> Vec<(String, String, String, bool)> {
            st["nodes"].as_array().unwrap().iter().map(|n| (n["id"].as_str().unwrap().into(), n["code"].as_str().unwrap().into(), n["status"].as_str().unwrap().into(), n["present"].as_bool().unwrap())).collect()
        };
        assert_eq!(by_id(&rs), by_id(py), "{id}: node ids, codes, statuses, present");

        let events = |st: &Value, key: &str| -> Vec<Value> { st[key].as_array().unwrap().iter().map(|e| json!([e["id"], e["participants"], e["signals"]])).collect() };
        assert_eq!(events(&rs, "events"), events(py, "events"), "{id}: event ids, participants, signals");
        assert_eq!(events(&rs, "undated"), events(py, "undated"), "{id}: undated ids, participants, signals");

        let links = |st: &Value| -> Vec<Value> { st["links"].as_array().unwrap().iter().map(|l| json!([l["a"], l["b"], l["weight"]])).collect() };
        assert_eq!(links(&rs), links(py), "{id}: link pairs and weights");
        assert_eq!(pending_pairs(&rs), pending_pairs(py), "{id}: merges pending once decided");

        let mut moved = 0;
        let beliefs = |st: &Value| -> Vec<(String, f64)> { st["nodes"].as_array().unwrap().iter().map(|n| (n["id"].as_str().unwrap().to_string(), n["p"].as_f64().unwrap())).collect() };
        for ((node, a), (_, b)) in beliefs(py).iter().zip(beliefs(&rs).iter()) {
            if (a - b).abs() > 1e-6 {
                println!("warning: {id}: belief of {node}: python {a}, rust {b}");
                moved += 1;
            }
        }
        // Types in `Detail` agree with the band, which the JSON form cannot show.
        for n in &s.state().nodes {
            match (&n.detail, n.band.as_str()) {
                (Detail::Signal(_), "signal") | (Detail::Participant(_), "participant") | (Detail::Hypothesis(_), "hypothesis") => {}
                _ => panic!("{id}: {} has a detail of another band", n.id),
            }
        }
        println!("{id}: {} python proposals, {} rust proposals, {moved} belief(s) moved", py_pairs.len(), rs_pairs.len());
    }
}

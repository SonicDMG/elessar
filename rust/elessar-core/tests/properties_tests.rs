//! Properties of the session and the timeline that hold on any pack.
mod common;

use elessar_core::session::{JevAnswer, Session};
use elessar_core::timeline::{timeline, timeline_ref};
use elessar_core::types::{Detail, Node, MAX_EVENTS_V1};
use proptest::prelude::*;
use serde_json::{json, Value};

fn clock() -> i64 {
    1_793_577_600_000
}

fn spec_of(s: &Session) -> Value {
    serde_json::from_str(&std::fs::read_to_string(s.pack.dir.join("expect.json")).unwrap()).unwrap()
}

fn fresh(id: &str, policy: Option<&str>) -> Session {
    let mut p = common::pack(id);
    if let Some(policy) = policy {
        p.unknown_time = policy.to_string();
    }
    let mut s = Session::new(p);
    s.clock = clock;
    s
}

fn connect(s: &mut Session, sid: &str) {
    let raw = common::read_source(&s.pack, sid);
    s.connect(sid, raw, 7);
}

fn connect_all(s: &mut Session) {
    for sid in s.pack.source_order.clone() {
        connect(s, &sid);
    }
}

fn decide(s: &mut Session) {
    let spec = spec_of(s);
    while let Some(pr) = s.state().merges.pending.first().cloned() {
        let (a, b) = (elessar_core::store::string_form(&pr.a.value), elessar_core::store::string_form(&pr.b.value));
        let listed = spec["merges"]["reject"].as_array().unwrap().iter().any(|pair| (pair[0] == a.as_str() && pair[1] == b.as_str()) || (pair[0] == b.as_str() && pair[1] == a.as_str()));
        let decision = if listed || spec["merges"]["default"] != "commit" { "reject" } else { "merge" };
        s.act(&json!({"action": decision, "proposal": pr.id})).unwrap();
    }
}

fn played(id: &str, policy: Option<&str>) -> Session {
    let mut s = fresh(id, policy);
    connect_all(&mut s);
    decide(&mut s);
    s
}

/// The whole state without its log: what an action that reverts itself must give back.
fn board(s: &Session) -> Value {
    let mut v = serde_json::to_value(s.state()).unwrap();
    for key in ["audit", "earned", "score"] {
        v.as_object_mut().unwrap().remove(key);
    }
    v
}

fn node<'a>(s: &'a Session, id: &str) -> &'a Node {
    s.state().nodes.iter().find(|n| n.id == id).unwrap()
}

fn assert_beliefs_in_range(s: &Session, what: &str) {
    for n in &s.state().nodes {
        assert!((0.0..=1.0).contains(&n.p), "{what}: belief of {} is {}", n.id, n.p);
    }
    for (i, stage) in s.state().stages.iter().enumerate() {
        for (id, p) in stage {
            assert!((0.0..=1.0).contains(p), "{what}: stage {i}: {id} is {p}");
        }
    }
}

/// The timeline's last slice shows the board, and every slice stays in range.
fn assert_timeline_sound(s: &mut Session, what: &str) {
    let tl = timeline(s);
    let slices = tl["slices"].as_array().unwrap();
    let last = slices.last().unwrap();
    for band in ["signals", "participants", "hypotheses"] {
        assert_eq!(last[band], tl["final"][band], "{what}: last slice's {band}");
    }
    for slice in slices {
        for band in ["signals", "participants", "hypotheses"] {
            for (id, entry) in slice[band].as_object().unwrap() {
                let p = entry["p"].as_f64().unwrap();
                assert!((0.0..=1.0).contains(&p), "{what}: slice {}: {id} is {p}", slice["index"]);
            }
        }
    }
    for (i, slice) in slices.iter().enumerate() {
        assert_eq!(slice["index"], json!(i), "{what}");
    }
    assert_eq!(slices[0]["t"], Value::Null, "{what}: slice 0 is reference data only");
}

#[test]
fn disconnect_then_reconnect_is_the_identity() {
    for id in common::pack_ids() {
        let mut s = played(&id, None);
        let want = board(&s);
        for sid in s.pack.source_order.clone() {
            s.act(&json!({"action": "disconnect", "source": sid})).unwrap();
            assert!(!s.is_connected(&sid), "{id}: {sid}");
            s.act(&json!({"action": "reconnect", "sources": [sid]})).unwrap();
            assert_eq!(board(&s), want, "{id}: {sid} from its kept payload");

            s.disconnect(&sid);
            connect(&mut s, &sid);
            assert_eq!(board(&s), want, "{id}: {sid} from a fresh payload");
        }
    }
}

#[test]
fn a_disconnected_source_leaves_removed_participants_that_say_where_they_came_from() {
    let mut s = played("supply-chain", None);
    let before: Vec<String> = s.state().sources.iter().map(|x| x.id.clone()).collect();
    for sid in &before {
        s.disconnect(sid);
    }
    let removed: Vec<&Node> = s.state().nodes.iter().filter(|n| n.status == "removed").collect();
    assert!(!removed.is_empty());
    for n in removed {
        assert!(!n.present);
        let off = n.offboard.as_ref().unwrap().as_ref().unwrap();
        assert_eq!((off.kind.as_str(), off.restore.as_str()), ("removed", "reconnect"));
        assert!(off.reason.starts_with("No connected source describes it any more. It came from: "), "{}", off.reason);
    }
}

#[test]
fn merge_then_revert_is_the_identity() {
    for id in common::pack_ids() {
        let mut s = fresh(&id, None);
        connect_all(&mut s);
        let want = board(&s);
        let pending = s.state().merges.pending.clone();
        assert!(!pending.is_empty(), "{id}: nothing proposed");
        for pr in pending {
            s.act(&json!({"action": "merge", "proposal": pr.id})).unwrap();
            assert_ne!(board(&s), want, "{id}: {} changed nothing", pr.id);
            s.act(&json!({"action": "unmerge", "proposal": pr.id})).unwrap();
            assert_eq!(board(&s), want, "{id}: reverting {}", pr.id);
        }
    }
}

#[test]
fn rejected_proposals_are_not_proposed_again() {
    for id in common::pack_ids() {
        let mut s = fresh(&id, None);
        connect_all(&mut s);
        while let Some(pr) = s.state().merges.pending.first().cloned() {
            s.act(&json!({"action": "reject", "proposal": pr.id})).unwrap();
            assert!(s.state().merges.pending.iter().all(|p| p.id != pr.id));
        }
        // Nothing merged: every source's fragments stayed apart.
        assert!(s.state().merges.committed.is_empty());
    }
}

#[test]
fn every_belief_stays_in_range() {
    for id in common::pack_ids() {
        let mut s = fresh(&id, None);
        assert_beliefs_in_range(&s, &id);
        for sid in s.pack.source_order.clone() {
            connect(&mut s, &sid);
            assert_beliefs_in_range(&s, &id);
        }
        decide(&mut s);
        s.act(&json!({"action": "feedback", "on": true})).unwrap();
        assert_beliefs_in_range(&s, &id);
        for sig in s.pack.signals.clone() {
            s.act(&json!({"action": "judge", "signal": sig.id, "judgement": "dismiss"})).unwrap();
            assert_beliefs_in_range(&s, &id);
        }
    }
}

#[test]
fn the_last_slice_is_the_live_board_under_every_policy() {
    for id in common::pack_ids() {
        for policy in ["throughout", "after", "at_connect"] {
            let mut s = played(&id, Some(policy));
            assert_timeline_sound(&mut s, &format!("{id} {policy}"));
        }
    }
}

#[test]
fn undated_policies_shape_the_slices() {
    let tl = timeline(&mut played("fraud-ring", Some("throughout")));
    let undated = tl["undated_count"].as_u64().unwrap() as usize;
    assert_eq!(undated, 20);
    assert_eq!(tl["slices"][0]["undated"].as_array().unwrap().len(), undated);
    assert_eq!(tl["slices"][1]["undated"].as_array().unwrap().len(), 0);

    let tl = timeline(&mut played("fraud-ring", Some("after")));
    let slices = tl["slices"].as_array().unwrap();
    assert_eq!(slices[0]["undated"], json!([]));
    assert!(slices[..slices.len() - 1].iter().all(|s| s.get("undated_arrive").is_none()));
    let last = slices.last().unwrap();
    assert_eq!(last["undated_arrive"], json!(true));
    assert_eq!(last["undated"].as_array().unwrap().len(), undated);
    assert_eq!(last["t"], slices[slices.len() - 2]["t"]);
    assert_eq!(last["arrived"], json!([]));

    // Under at_connect nothing is undated: those records are events dated by their connection.
    let s = played("fraud-ring", Some("at_connect"));
    assert!(s.state().undated.is_empty());
    let by_connect = s.state().events.iter().filter(|e| e.dated_by == "connect").count();
    assert_eq!(by_connect, undated);
    assert!(s.state().events.iter().filter(|e| e.dated_by == "connect").all(|e| e.t == clock()));
}

#[test]
fn thinning_keeps_the_first_and_last_and_loses_no_event() {
    let mut s = played("fraud-ring", None);
    let tl = timeline(&mut s);
    let slices = tl["slices"].as_array().unwrap();
    assert_eq!(slices.len(), 81, "slice 0 and 80 times");
    let times: Vec<i64> = s.state().events.iter().map(|e| e.t).collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(slices[1]["t"], json!(times[0]));
    assert_eq!(slices[80]["t"], json!(times[times.len() - 1]));
    let mut arrived: Vec<&str> = slices.iter().flat_map(|sl| sl["arrived"].as_array().unwrap()).map(|e| e.as_str().unwrap()).collect();
    assert_eq!(arrived.len(), times.len());
    assert_eq!(slices[80]["events_seen"], json!(times.len()));
    let in_order: Vec<&str> = s.state().events.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(arrived, in_order, "arrivals are the event list in order");
    arrived.dedup();
    assert_eq!(arrived.len(), times.len());
}

#[test]
fn a_timeline_is_cached_and_touches_nothing() {
    for id in common::pack_ids() {
        let mut s = played(&id, None);
        let want = board(&s);
        let sources_before = s.state().merges.pending.len();
        let first = timeline_ref(&mut s) as *const Value;
        let second = timeline_ref(&mut s) as *const Value;
        assert_eq!(first, second, "{id}: the second call builds nothing");
        assert_eq!(board(&s), want, "{id}: a timeline must not change the board");
        assert_eq!(s.state().merges.pending.len(), sources_before);

        // A new state gets a new timeline.
        let sig = s.pack.signals[0].id.clone();
        s.act(&json!({"action": "judge", "signal": sig, "judgement": "dismiss"})).unwrap();
        let tl = timeline(&mut s);
        assert_eq!(tl["final"]["signals"][&sig]["p"], json!(0.0), "{id}");
    }
}

#[test]
fn a_slice_does_not_touch_participant_memory() {
    // Absent from slice 0 onward, and never on the live board: stays absent, not removed.
    let mut s = fresh("supply-chain", None);
    connect(&mut s, "inventory");
    let tl = timeline(&mut s);
    assert_eq!(tl["final"]["participants"]["kestrel"]["status"], "absent");
    assert_eq!(node(&s, "kestrel").status, "absent");
    connect(&mut s, "shipments");
    connect(&mut s, "port");
    let _ = timeline(&mut s);
    s.disconnect("shipments");
    assert_eq!(node(&s, "kestrel").status, "removed");
}

#[test]
fn ack_and_unack_round_trip_on_an_event_and_an_undated_record() {
    let mut s = played("fraud-ring", None);
    let want = board(&s);
    let event = s.state().events[3].id.clone();
    let undated = s.state().undated[1].id.clone();

    s.act(&json!({"action": "ack", "event": event})).unwrap();
    assert!(s.state().events.iter().find(|e| e.id == event).unwrap().acked);
    assert!(s.state().events.iter().filter(|e| e.id != event).all(|e| !e.acked));
    s.act(&json!({"action": "unack", "event": event})).unwrap();
    assert_eq!(board(&s), want);

    s.act(&json!({"action": "ack", "event": undated})).unwrap();
    assert!(s.state().undated.iter().find(|e| e.id == undated).unwrap().acked);
    s.act(&json!({"action": "unack", "event": undated})).unwrap();
    assert_eq!(board(&s), want);

    let audit = s.audit().len();
    assert_eq!(s.act(&json!({"action": "ack", "event": "nope:1"})).unwrap_err(), "unknown event 'nope:1'");
    assert_eq!(s.audit().len(), audit, "a refused action logs nothing");
}

#[test]
fn reset_returns_to_the_empty_board() {
    for id in common::pack_ids() {
        let mut s = played(&id, None);
        s.act(&json!({"action": "peek", "source": s.pack.source_order[0]})).unwrap();
        s.act(&json!({"action": "feedback", "on": true})).unwrap();
        let hyp = s.pack.hypotheses[0].id.clone();
        s.act(&json!({"action": "crown", "hypothesis": hyp})).unwrap();
        let reset = s.act(&json!({"action": "reset"})).unwrap();
        assert_eq!(reset[0].kind, "reset");
        let empty = fresh(&id, None);
        assert_eq!(serde_json::to_value(s.state()).unwrap(), serde_json::to_value(empty.state()).unwrap(), "{id}");
        assert!(s.audit().is_empty());

        // The payloads are still known, and memory is gone: nothing is `removed`.
        assert!(s.state().nodes.iter().all(|n| n.status != "removed"), "{id}");
        let sid = s.pack.source_order[0].clone();
        assert!(s.has_payload(&sid));
        s.act(&json!({"action": "reconnect", "sources": [sid]})).unwrap();
        assert!(s.is_connected(&sid));
    }
}

#[test]
fn what_the_core_cannot_do_is_refused_with_a_reason() {
    let mut s = fresh("supply-chain", None);
    assert_eq!(s.act(&json!({"action": "agent_reset"})).unwrap_err(), "agent actions live in the server");
    assert_eq!(s.act(&json!({"action": "agent_decide", "proposal": "x", "decision": "approve"})).unwrap_err(), "agent actions live in the server");
    assert!(s.act(&json!({"action": "reconnect", "sources": ["port"]})).unwrap_err().contains("port"));
    assert!(s.act(&json!({"action": "connect", "source": "port"})).unwrap_err().contains("port"));
    assert_eq!(s.act(&json!({"action": "fly"})).unwrap_err(), "unknown action 'fly'");
    assert_eq!(s.act(&json!({"action": "judge", "signal": "x", "judgement": "maybe"})).unwrap_err(), "judgement must be trust, discount or dismiss");
    assert_eq!(s.act(&json!({"action": "engine", "engine": "magic"})).unwrap_err(), "engine must be rules or jev");
    assert!(s.act(&json!({"action": "crown", "hypothesis": "nope"})).is_err());
    assert!(s.act(&json!({"action": "merge", "proposal": "no-pipe"})).is_err());
    assert!(s.act(&json!({"action": "merge"})).is_err());
    assert!(s.act(&json!({"action": "merge", "proposal": "port#0:Port:4|shipments#0:Supplier:0"})).unwrap_err().contains("one type"));
    assert!(s.audit().is_empty());
}

#[test]
fn a_payload_that_fails_as_a_whole_leaves_the_source_unconnected() {
    let mut s = fresh("supply-chain", None);
    s.connect("inventory", "{not json".to_string(), 3);
    let card = s.state().sources.iter().find(|x| x.id == "inventory").unwrap();
    assert!(!card.connected);
    assert!(card.error.is_some());
    assert_eq!(s.audit().last().unwrap().action, "connect-failed");

    connect(&mut s, "inventory");
    let card = s.state().sources.iter().find(|x| x.id == "inventory").unwrap();
    assert!(card.connected);
    assert_eq!(card.error, None);
    assert_eq!(card.latency_ms, Some(7));

    // A connected source keeps its data when a new payload is bad.
    let records = card.records;
    s.connect("inventory", "{not json".to_string(), 3);
    let card = s.state().sources.iter().find(|x| x.id == "inventory").unwrap();
    assert!(card.connected);
    assert_eq!(card.records, records);
    assert!(card.error.is_some());

    s.connect_failed("port", "HTTP 500");
    let card = s.state().sources.iter().find(|x| x.id == "port").unwrap();
    assert_eq!(card.error.as_deref(), Some("HTTP 500"));
    s.connect("nope", "x".to_string(), 0);
    assert_eq!(s.audit().last().unwrap().action, "connect-failed");
}

#[test]
fn crowns_score_once_when_backed_and_log_every_other_call() {
    let mut s = played("supply-chain", None);
    let got = s.act(&json!({"action": "crown", "hypothesis": "congestion"})).unwrap();
    assert_eq!(got[0].kind, "crowned_contradicted");
    assert_eq!(s.state().crown.as_ref().unwrap().verdict, "contradicted");
    let got = s.act(&json!({"action": "crown", "hypothesis": "congestion"})).unwrap();
    assert_eq!(got[0].kind, "crowned_contradicted", "each overruled call is logged");

    let score = s.state().score;
    let got = s.act(&json!({"action": "crown", "hypothesis": "documents"})).unwrap();
    assert_eq!(got[0].kind, "crowned");
    assert_eq!(s.state().score, score + 500);
    assert!(node(&s, "documents").crowned.unwrap());
    assert!(!node(&s, "congestion").crowned.unwrap());
    let got = s.act(&json!({"action": "crown", "hypothesis": "documents"})).unwrap();
    assert!(got.is_empty(), "a backed crown earns once");
    assert_eq!(s.state().score, score + 500);
}

#[test]
fn a_kept_hypothesis_stays_on_the_board_with_its_reason() {
    let mut s = played("supply-chain", None);
    let n = node(&s, "shortfall");
    assert_eq!(n.status, "captured");
    let reason = n.offboard.as_ref().unwrap().as_ref().unwrap().reason.clone();
    assert!(reason.starts_with("Explained away at "), "{reason}");
    assert!(!n.present);

    s.act(&json!({"action": "keep", "hypothesis": "shortfall"})).unwrap();
    let n = node(&s, "shortfall");
    assert_eq!(n.status, "kept");
    assert!(n.present);
    assert_eq!(n.kept, Some(true));
    assert_eq!(n.kept_reason, Some(Some(reason)));
    assert_eq!(n.offboard.as_ref().map(|o| o.is_none()), Some(true));

    s.act(&json!({"action": "release", "hypothesis": "shortfall"})).unwrap();
    assert_eq!(node(&s, "shortfall").status, "captured");
}

/// `jev_jobs` must ask what the Python engine asked, so its answers are reusable (spec §11).
#[test]
fn jev_jobs_hit_the_python_cache() {
    let dir = common::repo_root().join(".cache").join("jev");
    if !dir.is_dir() {
        println!("no .cache/jev: nothing compared");
        return;
    }
    for id in common::pack_ids() {
        let mut s = fresh(&id, None);
        connect_all(&mut s);
        let (mut asked, mut found) = (0, 0);
        for sid in s.pack.source_order.clone() {
            let table = s.table(&sid).unwrap();
            for job in s.jev_jobs(&sid, table, true) {
                asked += 1;
                let payload = json!({"model": "jev-latest", "state": job.state, "questions": job.questions});
                found += usize::from(elessar_core::jevcache::cache_path(&dir, &payload).is_file());
            }
        }
        println!("{id}: {found}/{asked} asks are in the cache");
        if id != "fraud-ring" {
            assert_eq!(found, asked, "{id}");
        }
    }
}

#[test]
fn the_jev_engine_takes_over_only_when_every_candidate_has_an_answer() {
    let mut s = fresh("supply-chain", None);
    connect_all(&mut s);
    let sig = s.pack.signals.iter().find(|x| x.id == "container-hold").unwrap().clone();
    let source = sig.source.clone().unwrap();
    s.act(&json!({"action": "engine", "engine": "jev"})).unwrap();
    let detail = |s: &Session| {
        let Detail::Signal(d) = &node(s, "container-hold").detail else { panic!() };
        d.clone()
    };
    assert_eq!(detail(&s).engine_used, "rules", "nothing answered yet");
    assert!(!detail(&s).jev.unwrap().asked);

    let indices: Vec<u32> = detail(&s).jev.unwrap().records.iter().map(|r| r.index).collect();
    assert!(indices.len() > 1);
    let answer = |noul| JevAnswer { noul: Some(noul), model: Some("jev-test".into()), ms: Some(1), cached: true, error: None };
    let key = |i: u32| format!("container-hold|{source}|{i}");
    s.set_jev_answers(vec![(key(indices[0]), answer(0.9))]);
    let view = detail(&s).jev.unwrap();
    assert!(view.asked && !view.complete);
    assert_eq!(detail(&s).engine_used, "rules", "one answer is not all of them");

    s.set_jev_answers(indices.iter().skip(1).map(|i| (key(*i), answer(0.2))));
    let d = detail(&s);
    assert_eq!(d.engine_used, "jev");
    assert_eq!(d.strength, 0.9);
    assert_eq!(d.hits.len(), 1, "only the record over the threshold is a hit");
    assert_eq!(d.jev.as_ref().unwrap().hit_count, 1);
    assert_eq!(d.jev.as_ref().unwrap().model.as_deref(), Some("jev-test"));

    // Both engines are reported, and the board follows the switch back.
    s.act(&json!({"action": "engine", "engine": "rules"})).unwrap();
    assert_eq!(detail(&s).engine_used, "rules");
    assert_eq!(detail(&s).strength, sig_strength(&sig));
}

fn sig_strength(sig: &elessar_core::pack::Signal) -> f64 {
    match &sig.strength {
        elessar_core::pack::Strength::Fixed(n) => *n,
        _ => panic!("expected a fixed strength"),
    }
}

#[test]
fn the_v1_event_list_is_capped_to_the_newest() {
    let mut s = fresh("supply-chain", None);
    let n = MAX_EVENTS_V1 + 250;
    let mut raw = String::new();
    for i in 0..n {
        raw.push_str(&format!("{{\"ts\": \"2026-01-{:02}T{:02}:{:02}:00Z\", \"container\": \"MSCU7781234\", \"event\": \"GATE_IN\", \"location\": \"CNSHA\", \"supplier\": \"S\"}}\n", i / 1440 + 1, (i / 60) % 24, i % 60));
    }
    s.connect("shipments", raw, 0);
    let st = s.state();
    assert_eq!(st.events.len(), MAX_EVENTS_V1);
    assert_eq!(st.events_truncated, Some(true));
    assert!(st.events.windows(2).all(|w| w[0].t <= w[1].t));
    assert_eq!(st.events[0].id, "shipments:250", "the oldest 250 go");
    assert_eq!(st.events.last().unwrap().id, format!("shipments:{}", n - 1));

    // What was cut is still an event: the timeline counts it, and it can be acknowledged.
    s.act(&json!({"action": "ack", "event": "shipments:3"})).unwrap();
    let tl = timeline(&mut s);
    assert_eq!(tl["event_count"], json!(n));
    assert_eq!(tl["slices"].as_array().unwrap().last().unwrap()["events_seen"], json!(n));

    s.disconnect("shipments");
    assert_eq!(s.state().events_truncated, None);
}

/// A two-source pack written for one test, for what the shipped packs do not reach.
fn small_pack(name: &str, signals: Value, participants: Value, a: &str, b: &str) -> Session {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("small-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sources")).unwrap();
    let write = |file: &str, v: &Value| std::fs::write(dir.join(file), v.to_string()).unwrap();
    let source = |id: &str| json!({"id": id, "name": id, "format": "jsonl", "file": format!("sources/{id}.jsonl"), "confidence": 1.0, "time": {"field": "t", "unit": "iso"}});
    write("manifest.json", &json!({"id": name, "title": "T", "premise": "P", "sources": [source("a"), source("b")]}));
    write("mapping.json", &json!({
        "object_types": {"Person": {"title": "name", "keys": ["email"]}},
        "mappings": [
            {"source": "a", "type": "Person", "props": {"email": "email", "name": "name"}},
            {"source": "b", "type": "Person", "props": {"email": "email", "name": "name"}}
        ]
    }));
    write("signals.json", &signals);
    write("model.json", &json!({"participants": participants, "hypotheses": [{"id": "h", "label": "H", "prior": 0.1}], "edges": []}));
    write("narrative.json", &json!({"intro": "", "finale": "", "insights": {}}));
    std::fs::write(dir.join("sources/a.jsonl"), a).unwrap();
    std::fs::write(dir.join("sources/b.jsonl"), b).unwrap();
    let mut s = Session::new(elessar_core::pack::load_pack(&dir).unwrap_or_else(|e| panic!("{:?}", e.errors)));
    connect_all(&mut s);
    s
}

#[test]
fn two_co_resolved_signals_on_one_type_both_see_each_cluster() {
    let co = |id: &str, kind: &str| json!({"id": id, "kind": "co_resolved", "strength": 1.0, "params": {"type": "Person", "clauses": [{"source": "a", "where": {"kind": kind}}, {"source": "b"}]}});
    let mut s = small_pack(
        "two-co-resolved",
        json!([co("first", "x"), co("second", "x")]),
        json!([{"id": "p", "type": "Person", "anchor": {"email": "e@x"}}]),
        "{\"email\": \"e@x\", \"name\": \"Ed\", \"kind\": \"x\", \"t\": \"2026-01-01T00:00:00Z\"}\n",
        "{\"email\": \"e@x\", \"name\": \"Ed\", \"t\": \"2026-01-02T00:00:00Z\"}\n",
    );
    assert_eq!(node(&s, "first").status, "firing");
    assert_eq!(node(&s, "second").status, "firing");
    let tl = timeline(&mut s);
    assert_eq!(tl["slices"][1]["signals"]["second"]["status"], "quiet", "one source's half is not enough");
    assert_eq!(tl["slices"][2]["signals"]["first"]["status"], "firing");
    assert_eq!(tl["slices"][2]["signals"]["second"]["status"], "firing");
}

#[test]
fn an_anchor_moves_to_the_earlier_cluster_when_it_arrives() {
    // Two people named Ada. A participant anchors to the first cluster in rack
    // order, so `a`'s Ada takes over from `b`'s the moment she shows up.
    let mut s = small_pack(
        "anchor-moves",
        json!([{"id": "on-b", "kind": "match", "source": "b", "strength": 1.0, "implicates": {"w": 1.0}}]),
        json!([{"id": "ada", "type": "Person", "anchor": {"name": "Ada"}, "leak": 0.0}]),
        "{\"email\": \"late@x\", \"name\": \"Ada\", \"t\": \"2026-01-02T00:00:00Z\"}\n",
        "{\"email\": \"early@x\", \"name\": \"Ada\", \"t\": \"2026-01-01T00:00:00Z\"}\n",
    );
    let tl = timeline(&mut s);
    assert_eq!(tl["slices"][0]["participants"]["ada"]["present"], json!(false));
    assert_eq!(tl["slices"][1]["participants"]["ada"]["p"], json!(1.0), "only b's Ada so far: the signal implicates her");
    assert_eq!(tl["slices"][2]["participants"]["ada"]["p"], json!(0.0), "a's Ada comes first in the rack: the signal is about someone else now");
    assert_eq!(node(&s, "ada").p, 0.0, "as on the live board");
}

#[derive(Debug, Clone)]
enum Move {
    Connect(usize),
    Disconnect(usize),
    Reconnect(usize),
    Judge(usize, usize),
    Feedback(bool),
    Keep(usize),
    Release(usize),
    MergeFirst,
    RejectFirst,
    UnmergeFirst,
    Crown(usize),
    Engine(bool),
    Reset,
}

fn moves() -> impl Strategy<Value = Move> {
    prop_oneof![
        4 => (0..9usize).prop_map(Move::Connect),
        2 => (0..9usize).prop_map(Move::Disconnect),
        1 => (0..9usize).prop_map(Move::Reconnect),
        2 => (0..9usize, 0..3usize).prop_map(|(a, b)| Move::Judge(a, b)),
        1 => any::<bool>().prop_map(Move::Feedback),
        1 => (0..9usize).prop_map(Move::Keep),
        1 => (0..9usize).prop_map(Move::Release),
        2 => Just(Move::MergeFirst),
        1 => Just(Move::RejectFirst),
        1 => Just(Move::UnmergeFirst),
        1 => (0..9usize).prop_map(Move::Crown),
        1 => any::<bool>().prop_map(Move::Engine),
        1 => Just(Move::Reset),
    ]
}

fn apply_move(s: &mut Session, m: &Move) {
    let at = |list: &[String], i: usize| list[i % list.len()].clone();
    let sources = s.pack.source_order.clone();
    let signals: Vec<String> = s.pack.signals.iter().map(|x| x.id.clone()).collect();
    let hyps: Vec<String> = s.pack.hypotheses.iter().map(|x| x.id.clone()).collect();
    let payload = match m {
        Move::Connect(i) => {
            connect(s, &at(&sources, *i));
            return;
        }
        Move::Disconnect(i) => json!({"action": "disconnect", "source": at(&sources, *i)}),
        Move::Reconnect(i) => {
            let sid = at(&sources, *i);
            if !s.has_payload(&sid) {
                return;
            }
            json!({"action": "reconnect", "sources": [sid]})
        }
        Move::Judge(i, j) => {
            let judgement = ["trust", "discount", "dismiss"][*j];
            json!({"action": "judge", "signal": at(&signals, *i), "judgement": judgement})
        }
        Move::Feedback(on) => json!({"action": "feedback", "on": on}),
        Move::Keep(i) => json!({"action": "keep", "hypothesis": at(&hyps, *i)}),
        Move::Release(i) => json!({"action": "release", "hypothesis": at(&hyps, *i)}),
        Move::MergeFirst | Move::RejectFirst => {
            let Some(pr) = s.state().merges.pending.first() else { return };
            let action = if matches!(m, Move::MergeFirst) { "merge" } else { "reject" };
            json!({"action": action, "proposal": pr.id})
        }
        Move::UnmergeFirst => {
            let Some(c) = s.state().merges.committed.first() else { return };
            json!({"action": "unmerge", "proposal": c.id})
        }
        Move::Crown(i) => json!({"action": "crown", "hypothesis": at(&hyps, *i)}),
        Move::Engine(jev) => {
            let engine = if *jev { "jev" } else { "rules" };
            json!({"action": "engine", "engine": engine})
        }
        Move::Reset => json!({"action": "reset"}),
    };
    s.act(&payload).unwrap();
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    /// Whatever a user does, beliefs stay in range and the replay ends on the board.
    #[test]
    fn any_session_stays_sound(pack in 0..2usize, policy in 0..3usize, script in prop::collection::vec(moves(), 1..14)) {
        let id = ["change-cascade", "supply-chain"][pack];
        let mut s = fresh(id, Some(["throughout", "after", "at_connect"][policy]));
        for m in &script {
            apply_move(&mut s, m);
            assert_beliefs_in_range(&s, id);
        }
        assert_timeline_sound(&mut s, id);
    }

    /// The decisions alone determine the board: replaying them on a fresh session gives it back.
    #[test]
    fn the_board_depends_only_on_decisions(script in prop::collection::vec(moves(), 1..10)) {
        let mut s = fresh("supply-chain", None);
        for m in &script {
            apply_move(&mut s, m);
        }
        let want = board(&s);
        let again = board(&{
            let mut t = fresh("supply-chain", None);
            for m in &script {
                apply_move(&mut t, m);
            }
            t
        });
        prop_assert_eq!(want, again);
    }
}

/// Timing, not an assertion: `cargo test --release -p elessar-core --test properties_tests -- --ignored --nocapture`.
/// Set `ELESSAR_TIMING_PACK` to a pack directory to time another pack.
#[test]
#[ignore]
fn timings() {
    use std::time::Instant;
    let p = match std::env::var_os("ELESSAR_TIMING_PACK") {
        Some(dir) => elessar_core::pack::load_pack(std::path::Path::new(&dir)).unwrap(),
        None => common::pack("fraud-ring"),
    };
    let records = |s: &Session| s.state().sources.iter().map(|x| x.records).sum::<usize>();
    let mut s = Session::new(p);
    let start = Instant::now();
    connect_all(&mut s);
    println!("{} records, connect all: {:?}", records(&s), start.elapsed());
    let sig = s.pack.signals[0].id.clone();
    let start = Instant::now();
    s.act(&json!({"action": "judge", "signal": sig, "judgement": "discount"})).unwrap();
    println!("one action (recompute): {:?}", start.elapsed());
    let start = Instant::now();
    let slices = timeline_ref(&mut s)["slices"].as_array().unwrap().len();
    println!("timeline, {slices} slices: {:?}", start.elapsed());
    let start = Instant::now();
    timeline_ref(&mut s);
    println!("timeline again: {:?}", start.elapsed());
    println!("state JSON: {} bytes", serde_json::to_string(s.state()).unwrap().len());
}

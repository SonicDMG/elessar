//! The agent against scripted model replies, with no network.
//!
//! `fixtures/agent_fixtures.json` is what the Python agent does with the same
//! scripts on the same board (see `fixtures/gen_agent_fixtures.py`). Request
//! bodies and tool results are compared byte for byte.

#[allow(dead_code)]
#[path = "../src/agent.rs"]
mod agent;

use agent::{piece_detail, py_dumps, py_dumps_compact, snapshot, Agent, Backends, MAX_STEPS, MAX_TOOL_RESULT, SYSTEM};
use elessar_core::pack::load_pack;
use elessar_core::session::{JevResult, Session};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::OnceLock;

fn repo_root() -> PathBuf {
    // elessar-server/ -> rust/ -> repo
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
}

fn fixtures() -> &'static Value {
    static FIXTURES: OnceLock<Value> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agent_fixtures.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    })
}

fn clock() -> i64 {
    // 2026-11-02T00:00:00Z, the generator's frozen clock.
    1_793_577_600_000
}

fn read_source(s: &Session, sid: &str) -> String {
    std::fs::read_to_string(s.pack.sources.get(sid).unwrap().path.as_ref().unwrap()).unwrap()
}

fn new_session() -> Session {
    let pack = load_pack(&repo_root().join("scenarios/change-cascade")).unwrap_or_else(|e| panic!("{e}: {:?}", e.errors));
    let mut s = Session::new(pack);
    s.clock = clock;
    s.base_url = "offline".into();
    s
}

/// The generator's `fake_ask`.
fn fake_jev(state: &Value, questions: &Value) -> JevResult {
    let n = state["record"].as_object().map_or(0, |r| r.len());
    let answers: serde_json::Map<String, Value> = questions.as_object().unwrap().keys().map(|k| (k.clone(), json!({"noul": ((n * 7 + k.len() * 3) % 17) as f64 / 17.0 + 0.02}))).collect();
    JevResult { answers: Some(Value::Object(answers)), model: Some("jev-test".into()), ms: Some(5), cost: Some(0.001), cached: false, error: None }
}

/// Scripted replies, every request recorded. Sources fetch from the pack's files.
struct Fake {
    replies: RefCell<VecDeque<Result<Value, String>>>,
    requests: RefCell<Vec<Value>>,
    jev_on: bool,
    jev_asks: Cell<usize>,
}

impl Fake {
    fn new(replies: Vec<Result<Value, String>>, jev_on: bool) -> Fake {
        Fake { replies: RefCell::new(replies.into()), requests: RefCell::new(Vec::new()), jev_on, jev_asks: Cell::new(0) }
    }

    fn says(replies: Vec<Value>) -> Fake {
        Fake::new(replies.into_iter().map(Ok).collect(), false)
    }

    fn script(&self, replies: Vec<Result<Value, String>>) {
        *self.replies.borrow_mut() = replies.into();
    }

    fn last_messages(&self) -> Vec<Value> {
        self.requests.borrow().last().unwrap()["messages"].as_array().unwrap().clone()
    }
}

impl Backends for Fake {
    fn messages(&self, body: &Value) -> Result<Value, String> {
        self.requests.borrow_mut().push(body.clone());
        self.replies.borrow_mut().pop_front().expect("the script ran out of replies")
    }
    fn jev(&self, state: &Value, questions: &Value) -> Result<JevResult, String> {
        self.jev_asks.set(self.jev_asks.get() + 1);
        Ok(fake_jev(state, questions))
    }
    fn jev_available(&self) -> bool {
        self.jev_on
    }
    fn jev_model(&self) -> String {
        "jev-test".into()
    }
    fn fetch_source(&self, session: &Session, source: &str) -> Result<(String, u64), String> {
        Ok((read_source(session, source), 0))
    }
}

/// Only what the trait requires, to see its defaults.
struct NoFetch;

impl Backends for NoFetch {
    fn messages(&self, _: &Value) -> Result<Value, String> {
        Err("no model here".into())
    }
    fn jev(&self, _: &Value, _: &Value) -> Result<JevResult, String> {
        Err("Jev is off".into())
    }
    fn jev_available(&self) -> bool {
        false
    }
    fn jev_model(&self) -> String {
        String::new()
    }
}

/// What the server's action handler does around `Session::act`.
fn replay(s: &mut Session, payload: &Value) {
    match payload["action"].as_str().unwrap() {
        "connect" => {
            let sid = payload["source"].as_str().unwrap();
            let raw = read_source(s, sid);
            s.connect(sid, raw, 0);
        }
        "engine" => {
            s.act(payload).unwrap();
            for sid in s.connected_sources() {
                let table = s.table(&sid).unwrap();
                let results: Vec<(u32, JevResult)> = s.jev_jobs(&sid, table, true).iter().map(|j| (j.index, fake_jev(&j.state, &j.questions))).collect();
                if !results.is_empty() {
                    s.apply_jev_results(&sid, &results);
                }
            }
        }
        _ => {
            s.act(payload).unwrap();
        }
    }
}

fn session_from(setup: &Value) -> Session {
    let mut s = new_session();
    for payload in setup.as_array().unwrap() {
        replay(&mut s, payload);
    }
    s
}

/// Blank the number after a marker, wherever it appears.
fn blank_number(text: &str, marker: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(marker) {
        let (head, tail) = rest.split_at(at + marker.len());
        out.push_str(head);
        out.push('0');
        rest = tail.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
    }
    out.push_str(rest);
    out
}

/// What may differ between the engines' texts: `replied in N ms` depends on the
/// wall clock, and the fuzzy similarity measure is an allowed divergence (spec
/// section 13), bare in a snapshot and escaped inside a request body.
fn normalize(text: &str) -> String {
    let text = blank_number(text, "replied in ");
    let text = blank_number(&text, "\"similarity\":");
    blank_number(&text, "\\\"similarity\\\":")
}

/// `same`, for text that went through a board snapshot.
fn same_board(what: &str, got: &str, want: &str) {
    same(what, &normalize(got), &normalize(want));
}

/// The first place two long strings differ, with some context.
fn same(what: &str, got: &str, want: &str) {
    if got == want {
        return;
    }
    let at = got.bytes().zip(want.bytes()).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len()));
    let window = |s: &str| {
        let from = at.saturating_sub(120);
        let to = (at + 160).min(s.len());
        String::from_utf8_lossy(&s.as_bytes()[from..to]).into_owned()
    };
    panic!("{what}: differs at byte {at} (lengths {} and {})\n  rust:   {}\n  python: {}", got.len(), want.len(), window(got), window(want));
}

/// A turn or a view with its timings out of the way.
fn untimed(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(items) = v.get_mut("transcript").and_then(Value::as_array_mut) {
        for t in items.iter_mut() {
            *t = untimed(t);
        }
    }
    if let Some(map) = v.as_object_mut().filter(|m| m.contains_key("ms")) {
        map.insert("ms".into(), json!(0));
        // Summed from parsed text: equal in value to within float parsing.
        let cost = map["cost"].as_f64().unwrap();
        map.insert("cost".into(), json!((cost * 1e9).round() / 1e9));
    }
    v
}

fn text_reply(text: &str) -> Value {
    json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": text}], "usage": {"cost": 0.01}})
}

fn tool_reply(name: &str, input: Value) -> Value {
    json!({"stop_reason": "tool_use", "content": [{"type": "tool_use", "id": "toolu_1", "name": name, "input": input}], "usage": {"cost": 0.02}})
}

fn tool_result(messages: &[Value]) -> &Value {
    &messages.last().unwrap()["content"][0]
}

fn decided_session() -> Session {
    session_from(&fixtures()["scenes"]["decided"]["setup"])
}

/// No message may be empty, and every tool call must be answered in the next
/// message; the API refuses a request otherwise.
fn assert_valid_history(messages: &[Value]) {
    for (i, m) in messages.iter().enumerate() {
        match &m["content"] {
            Value::String(s) => assert!(!s.is_empty()),
            Value::Array(blocks) => {
                assert!(!blocks.is_empty(), "message {i} is empty");
                let uses: Vec<&Value> = blocks.iter().filter(|b| b["type"] == "tool_use").map(|b| &b["id"]).collect();
                if !uses.is_empty() {
                    let answered: Vec<&Value> = messages[i + 1]["content"].as_array().unwrap().iter().map(|r| &r["tool_use_id"]).collect();
                    assert_eq!(uses, answered, "message {i}'s tool calls are not answered");
                }
            }
            other => panic!("message {i} has content {other}"),
        }
    }
}

// ---------------------------------------------------------------- against Python

#[test]
fn prompt_and_tools_are_pythons() {
    let f = fixtures();
    same("system", SYSTEM, f["system"].as_str().unwrap());
    same("tools", &py_dumps(agent::tools()), f["tools"].as_str().unwrap());
    assert_eq!(MAX_STEPS as u64, f["max_steps"].as_u64().unwrap());
}

#[test]
fn snapshots_and_pieces_are_pythons_on_every_scene() {
    for (name, scene) in fixtures()["scenes"].as_object().unwrap() {
        let s = session_from(&scene["setup"]);
        same_board(&format!("{name} snapshot"), &py_dumps_compact(&snapshot(s.state(), None)), scene["snapshot"].as_str().unwrap());
        same_board(&format!("{name} snapshot with a selection"), &py_dumps_compact(&snapshot(s.state(), scene["selected"].as_str())), scene["snapshot_selected"].as_str().unwrap());
        let pieces = scene["pieces"].as_object().unwrap();
        assert_eq!(pieces.len(), s.state().nodes.len(), "{name}");
        for (code, want) in pieces {
            same(&format!("{name} {code}"), &py_dumps(&piece_detail(s.state(), code)), want.as_str().unwrap());
        }
    }
}

/// Every step of every conversation: the last request of each ask (which holds
/// the whole history, with every tool result, as bytes), the turn, the insights
/// an approval earned, the beliefs after it, and the final view and audit trail.
#[test]
fn conversations_are_pythons() {
    for (name, c) in fixtures()["conversations"].as_object().unwrap() {
        let mut s = session_from(&c["setup"]);
        let mut a = Agent::new(Some("test/model".into()));
        for (i, step) in c["steps"].as_array().unwrap().iter().enumerate() {
            let what = format!("{name} step {i}");
            if let Some(id) = step["decide"].as_str() {
                let decision = step["decision"].as_str().unwrap();
                // The server logs first, so the audit trail reads as Python's does.
                let status = if decision == "approve" { "approved" } else { "declined" };
                let label = a.view()["proposals"][id]["label"].as_str().unwrap().to_string();
                s.log("agent", &format!("you {status} the agent's proposal to {label}"));
                let prop = a.decide(&mut s, id, decision).unwrap();
                same(&format!("{what} events"), &py_dumps(&prop["events"]), &py_dumps(&step["events"]));
                let beliefs: serde_json::Map<String, Value> = s.state().nodes.iter().map(|n| (n.code.clone(), json!(n.p))).collect();
                same(&format!("{what} beliefs"), &py_dumps(&Value::Object(beliefs)), step["beliefs"].as_str().unwrap());
                continue;
            }
            let replies = step["responses"].as_array().unwrap().iter().cloned().map(Ok).collect();
            let fake = Fake::new(replies, c["jev_on"].as_bool().unwrap());
            let turn = a.ask(&mut s, step["ask"].as_str().unwrap(), step["selected"].as_str(), &fake);
            let requests = fake.requests.borrow();
            assert_eq!(requests.len() as u64, step["request_count"].as_u64().unwrap(), "{what}");
            same_board(&what, &py_dumps(requests.last().unwrap()), step["last_request"].as_str().unwrap());
            same(&format!("{what} turn"), &py_dumps(&untimed(&turn)), &py_dumps(&untimed(&step["turn"])));
        }
        same(&format!("{name} view"), &py_dumps(&untimed(&a.view())), &py_dumps(&untimed(&json!({"available": true, "model": "test/model", "transcript": c["view"]["transcript"], "proposals": c["view"]["proposals"]}))));
        let audit: Vec<String> = s.audit().iter().map(|e| format!("{}: {}", e.action, e.detail)).collect();
        same_board(&format!("{name} audit"), &py_dumps(&json!(audit)), &py_dumps(&c["audit"]));
    }
}

// ---------------------------------------------------------------- the loop

#[test]
fn a_plain_reply_is_a_turn() {
    let mut s = decided_session();
    let mut a = Agent::new(Some("m".into()));
    let fake = Fake::says(vec![text_reply("Hello.")]);
    let turn = a.ask(&mut s, "Hi", None, &fake);
    assert_eq!(turn["role"], "assistant");
    assert_eq!(turn["text"], "Hello.");
    assert_eq!(turn["tools"], json!([]));
    assert_eq!(turn["proposals"], json!([]));
    assert_eq!(turn["highlights"], json!([]));
    assert_eq!(turn["cost"], 0.01);
    let body = &fake.requests.borrow()[0];
    let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["model", "max_tokens", "system", "tools", "thinking", "output_config", "messages"]);
    assert_eq!(body["model"], "m");
    assert_eq!(body["max_tokens"], 16000);
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "medium");
    assert_eq!(body["tools"].as_array().unwrap().last().unwrap()["cache_control"]["type"], "ephemeral");
    let last = s.audit().last().unwrap();
    assert_eq!(last.action, "agent");
    assert!(last.detail.starts_with("replied in ") && last.detail.ends_with(" ms, 0 tool calls, $0.0100"), "{}", last.detail);
}

#[test]
fn a_what_if_leaves_the_session_untouched() {
    let mut s = decided_session();
    let before = serde_json::to_string(s.state()).unwrap();
    let firing = s.state().nodes.iter().find(|n| n.band == "signal" && n.status == "firing").unwrap().code.clone();
    let mut a = Agent::new(Some("m".into()));
    let change = json!({"changes": [
        {"action": "judge", "signal": firing, "judgement": "dismiss"},
        {"action": "disconnect", "source": "flow"},
        {"action": "engine", "engine": "jev"},
    ]});
    let mut fake = Fake::says(vec![tool_reply("what_if", change), text_reply("Moved.")]);
    fake.jev_on = true;
    let audit = s.audit().len();
    a.ask(&mut s, "What if?", None, &fake);
    assert_eq!(s.audit().len(), audit + 1, "only the agent's own line");
    let after = s.state();
    assert!(after.sources.iter().find(|x| x.id == "flow").unwrap().connected);
    assert_eq!(after.engine, "rules");
    // Everything but the audit trail is as it was.
    let strip = |text: &str| {
        let mut v: Value = serde_json::from_str(text).unwrap();
        v.as_object_mut().unwrap().remove("audit");
        v
    };
    assert_eq!(strip(&serde_json::to_string(after).unwrap()), strip(&before));
    assert!(fake.jev_asks.get() > 0, "switching the copy to Jev asks Jev");
}

#[test]
fn a_what_if_can_connect_a_source_never_fetched() {
    let mut s = session_from(&fixtures()["scenes"]["partial"]["setup"]);
    let change = json!({"changes": [{"action": "connect", "source": "advisory"}]});
    let mut a = Agent::new(Some("m".into()));
    let fake = Fake::says(vec![tool_reply("what_if", change.clone()), text_reply("Done.")]);
    let turn = a.ask(&mut s, "What if?", None, &fake);
    assert_eq!(turn["tools"][0]["error"], false);
    assert!(!s.is_connected("advisory") && !s.has_payload("advisory"));

    // A server with no `fetch_source` tells the model why, as a tool error.
    struct Scripted(NoFetch, Fake);
    impl Backends for Scripted {
        fn messages(&self, body: &Value) -> Result<Value, String> {
            self.1.messages(body)
        }
        fn jev(&self, a: &Value, b: &Value) -> Result<JevResult, String> {
            self.0.jev(a, b)
        }
        fn jev_available(&self) -> bool {
            self.0.jev_available()
        }
        fn jev_model(&self) -> String {
            self.0.jev_model()
        }
    }
    let plain = Scripted(NoFetch, Fake::says(vec![tool_reply("what_if", change), text_reply("Done.")]));
    a.ask(&mut s, "What if?", None, &plain);
    let messages = plain.1.last_messages();
    let result = tool_result(&messages);
    assert_eq!(result["is_error"], true);
    assert_eq!(result["content"], "{\"error\": \"what-if could not connect advisory: no payload for source 'advisory'\"}");
}

#[test]
fn tool_results_are_cut_at_the_limit() {
    let mut s = new_session();
    let long = "é".repeat(5000);
    s.connect("activity", format!("{}\n", json!({"t": 1792104000000u64, "actor": "B-5540", "target": "door-c2", "verb": "GRANT", "detail": long})), 0);
    let pointer = s.table("activity").unwrap().records[0].pointer.clone();
    let mut a = Agent::new(Some("m".into()));
    let fake = Fake::says(vec![tool_reply("get_record", json!({"source": "activity", "pointer": pointer})), text_reply("Long.")]);
    a.ask(&mut s, "Show it.", None, &fake);
    let messages = fake.requests.borrow()[1]["messages"].as_array().unwrap().clone();
    let content = tool_result(&messages)["content"].as_str().unwrap();
    assert_eq!(content.chars().count(), MAX_TOOL_RESULT);
    assert!(content.is_ascii(), "non-ASCII is escaped, so a cut can never split a character");
}

#[test]
fn approving_applies_and_declining_does_not() {
    let mut s = decided_session();
    let firing = s.state().nodes.iter().find(|n| n.band == "signal" && n.status == "firing").unwrap().clone();
    let propose = |judgement: &str, reason: &str| json!({"type": "tool_use", "id": format!("t{judgement}"), "name": "propose_action", "input": {"action": "judge", "signal": firing.code, "judgement": judgement, "reason": reason}});
    let mut a = Agent::new(Some("m".into()));
    let reply = json!({"stop_reason": "tool_use", "content": [propose("discount", "Weak."), propose("dismiss", "Very weak.")]});
    let fake = Fake::says(vec![reply, text_reply("Two ideas.")]);
    let turn = a.ask(&mut s, "Ideas?", None, &fake);
    assert_eq!(turn["proposals"], json!(["p1", "p2"]));
    let p = firing.p;
    assert_eq!(s.state().nodes.iter().find(|n| n.id == firing.id).unwrap().p, p, "a proposal changes nothing");

    let declined = a.decide(&mut s, "p2", "decline").unwrap();
    assert_eq!(declined["status"], "declined");
    assert_eq!(s.state().nodes.iter().find(|n| n.id == firing.id).unwrap().p, p);

    let approved = a.decide(&mut s, "p1", "approve").unwrap();
    assert_eq!(approved["status"], "approved");
    assert_eq!(approved["payload"], json!({"action": "judge", "signal": firing.id, "judgement": "discount"}));
    assert_eq!(s.state().nodes.iter().find(|n| n.id == firing.id).unwrap().p, p * 0.5);

    assert_eq!(a.decide(&mut s, "p1", "approve").unwrap_err(), "no pending proposal with that id");
    assert_eq!(a.decide(&mut s, "p9", "approve").unwrap_err(), "no pending proposal with that id");

    let view = a.view();
    assert_eq!(view["proposals"]["p1"]["status"], "approved");
    assert_eq!(view["proposals"]["p2"]["status"], "declined");
    assert!(view["proposals"]["p1"].get("events").is_none(), "the view keeps Python's shape");

    fake.script(vec![Ok(text_reply("Noted."))]);
    a.ask(&mut s, "Next?", None, &fake);
    let messages = fake.last_messages();
    let content = messages.last().unwrap()["content"].as_str().unwrap();
    assert!(
        content.ends_with(&format!(
            "</board_state>\n\nSince your last reply the user: declined your proposal to dismiss {code}; approved your proposal to discount {code}.\n\nNext?",
            code = firing.code
        )),
        "{}",
        &content[content.len() - 200..]
    );

    fake.script(vec![Ok(text_reply("Fine."))]);
    a.ask(&mut s, "Again?", None, &fake);
    let messages = fake.last_messages();
    assert!(messages.last().unwrap()["content"].as_str().unwrap().ends_with("</board_state>\n\nAgain?"), "notes are said once");
}

#[test]
fn approving_a_connect_asks_for_a_fetch_only_when_there_is_no_payload() {
    let mut s = session_from(&fixtures()["scenes"]["partial"]["setup"]);
    let mut a = Agent::new(Some("m".into()));
    let fake = Fake::says(vec![tool_reply("propose_action", json!({"action": "connect", "source": "advisory", "reason": "More."})), text_reply("Connect it.")]);
    a.ask(&mut s, "Next?", None, &fake);
    let prop = a.decide(&mut s, "p1", "approve").unwrap();
    assert_eq!(prop["status"], "approved");
    assert_eq!(prop["fetch"], "advisory");
    assert!(!s.is_connected("advisory"), "fetching is the server's");
    assert_eq!(prop["events"], json!([]));

    s.connect("advisory", read_source(&new_session(), "advisory"), 0);
    s.disconnect("advisory");
    fake.script(vec![Ok(tool_reply("propose_action", json!({"action": "connect", "source": "advisory", "reason": "Again."}))), Ok(text_reply("Again."))]);
    a.ask(&mut s, "Back?", None, &fake);
    let prop = a.decide(&mut s, "p2", "approve").unwrap();
    assert!(prop.get("fetch").is_none());
    assert!(s.is_connected("advisory"), "a kept payload connects right away");
}

#[test]
fn an_approved_action_returns_the_insights_it_earned() {
    let mut s = session_from(&fixtures()["scenes"]["undecided"]["setup"]);
    let mut a = Agent::new(Some("m".into()));
    let fake = Fake::says(vec![tool_reply("propose_action", json!({"action": "merge", "merge": "M1", "reason": "Same person."})), text_reply("Merge it.")]);
    a.ask(&mut s, "Merge?", None, &fake);
    let prop = a.decide(&mut s, "p1", "approve").unwrap();
    assert!(prop["events"].as_array().unwrap().iter().any(|e| e["kind"] == "merge_committed"), "{}", prop["events"]);
}

#[test]
fn a_failed_request_drops_the_turn_and_keeps_the_notes() {
    let mut s = decided_session();
    let mut a = Agent::new(Some("m".into()));
    let fake = Fake::says(vec![tool_reply("propose_action", json!({"action": "engine", "engine": "jev", "reason": "Compare."})), text_reply("Ok.")]);
    a.ask(&mut s, "Ideas?", None, &fake);
    a.decide(&mut s, "p1", "decline").unwrap();
    let history = fake.last_messages().len();

    let err = "The agent couldn't reach the model (HTTP 500): boom";
    fake.script(vec![Err(err.to_string())]);
    let turn = a.ask(&mut s, "Hello?", None, &fake);
    assert_eq!(turn["text"], err);
    assert_eq!(a.view()["transcript"].as_array().unwrap().len(), 4, "the user still sees the exchange");

    fake.script(vec![Ok(text_reply("Back."))]);
    a.ask(&mut s, "Again?", None, &fake);
    let messages = fake.last_messages();
    assert_eq!(messages.len(), history + 2, "the unanswered turn is gone");
    assert_valid_history(&messages);
    assert!(messages.last().unwrap()["content"].as_str().unwrap().contains("Since your last reply the user: declined your proposal to switch the evidence engine to jev.\n\nAgain?"));
}

/// Python pops one message here, which leaves a tool call with no answer, and
/// every later request fails until the conversation is reset.
#[test]
fn a_request_failing_after_tool_calls_leaves_a_valid_history() {
    let mut s = decided_session();
    let mut a = Agent::new(Some("m".into()));
    let fake = Fake::new(vec![Ok(tool_reply("highlight", json!({"codes": ["H1"]}))), Err("The agent couldn't reach the model: reset".into())], false);
    let turn = a.ask(&mut s, "Hello?", None, &fake);
    assert_eq!(turn["text"], "The agent couldn't reach the model: reset");
    assert_eq!(turn["tools"].as_array().unwrap().len(), 1);
    assert_eq!(turn["highlights"].as_array().unwrap().len(), 1, "the board can still pulse");

    fake.script(vec![Ok(text_reply("Back."))]);
    a.ask(&mut s, "Again?", None, &fake);
    let messages = fake.last_messages();
    assert_eq!(messages.len(), 1);
    assert_valid_history(&messages);
}

#[test]
fn replies_the_api_would_refuse_to_see_again_leave_the_history() {
    let thinking = json!({"type": "thinking", "thinking": "hmm", "signature": "s"});
    let cases = [
        ("an empty refusal", json!({"stop_reason": "refusal", "content": []}), "The agent declined to answer that."),
        ("a refusal after thinking", json!({"stop_reason": "refusal", "content": [thinking.clone()]}), "The agent declined to answer that."),
        ("an empty reply", json!({"stop_reason": "end_turn", "content": []}), ""),
        ("no content at all", json!({}), ""),
        ("cut off while thinking", json!({"stop_reason": "max_tokens", "content": [thinking]}), "\n\n(The reply was cut off at the length limit.)"),
        (
            "cut off in a tool call",
            json!({"stop_reason": "max_tokens", "content": [{"type": "text", "text": "Let me"}, {"type": "tool_use", "id": "t", "name": "highlight", "input": {}}]}),
            "Let me\n\n(The reply was cut off at the length limit.)",
        ),
        ("tool_use with no call", json!({"stop_reason": "tool_use", "content": []}), ""),
    ];
    for (what, reply, want) in cases {
        let mut s = decided_session();
        let mut a = Agent::new(Some("m".into()));
        let fake = Fake::says(vec![reply, text_reply("Back.")]);
        let turn = a.ask(&mut s, "Hello?", None, &fake);
        assert_eq!(turn["text"], want, "{what}");
        assert_eq!(turn["tools"], json!([]), "{what}");
        a.ask(&mut s, "Again?", None, &fake);
        let messages = fake.last_messages();
        assert_eq!(messages.len(), 1, "{what}");
        assert_valid_history(&messages);
    }
}

#[test]
fn the_step_limit_stops_the_loop_and_leaves_a_valid_history() {
    let mut s = decided_session();
    let mut a = Agent::new(Some("m".into()));
    let mut replies: Vec<Value> = (0..MAX_STEPS).map(|_| tool_reply("highlight", json!({"codes": ["H1"]}))).collect();
    replies.push(text_reply("Back."));
    let fake = Fake::says(replies);
    let turn = a.ask(&mut s, "Forever", None, &fake);
    assert_eq!(fake.requests.borrow().len(), MAX_STEPS);
    assert_eq!(turn["text"], "\n\n(Stopped after the maximum number of steps.)");
    assert_eq!(turn["tools"].as_array().unwrap().len(), MAX_STEPS);
    a.ask(&mut s, "Still there?", None, &fake);
    assert_valid_history(&fake.last_messages());
}

#[test]
fn reset_forgets_everything_but_the_model() {
    let mut s = decided_session();
    let mut a = Agent::new(Some("m".into()));
    let fake = Fake::says(vec![tool_reply("propose_action", json!({"action": "engine", "engine": "jev", "reason": "Compare."})), text_reply("Ok.")]);
    a.ask(&mut s, "Ideas?", None, &fake);
    a.decide(&mut s, "p1", "decline").unwrap();
    a.reset();
    assert_eq!(a.view(), json!({"available": true, "model": "m", "transcript": [], "proposals": {}}));
    fake.script(vec![Ok(tool_reply("propose_action", json!({"action": "engine", "engine": "rules", "reason": "Back."}))), Ok(text_reply("Ok."))]);
    a.ask(&mut s, "Fresh?", None, &fake);
    let first = fake.requests.borrow().iter().rev().nth(1).unwrap()["messages"].clone();
    assert_eq!(first.as_array().unwrap().len(), 1);
    assert!(first[0]["content"].as_str().unwrap().ends_with("</board_state>\n\nFresh?"), "no notes survive");
    assert!(a.view()["proposals"].get("p1").is_some(), "ids start over");
}

#[test]
fn an_agent_without_a_model_is_off() {
    let mut s = new_session();
    let mut a = Agent::new(None);
    assert!(!a.available());
    let fake = Fake::says(vec![]);
    let turn = a.ask(&mut s, "Hi", None, &fake);
    assert_eq!(turn["text"], "The agent is off: add OPENROUTER_API_KEY to .env");
    assert!(fake.requests.borrow().is_empty());
    assert_eq!(a.view(), json!({"available": false, "model": null, "transcript": [], "proposals": {}}));
}

/// Walks a real transcript as `renderAgent` in `web/panel.js` and `web/app.js` read it.
#[test]
fn the_view_has_what_the_frontend_reads() {
    let mut s = decided_session();
    let mut a = Agent::new(Some("anthropic/claude-sonnet-5.5".into()));
    let fake = Fake::says(vec![
        json!({"stop_reason": "tool_use", "usage": {"cost": 0.25}, "content": [
            {"type": "tool_use", "id": "a", "name": "highlight", "input": {"codes": ["H1", "S1"]}},
            {"type": "tool_use", "id": "b", "name": "propose_action", "input": {"action": "crown", "hypothesis": "H3", "reason": "It leads."}},
            {"type": "tool_use", "id": "c", "name": "get_piece", "input": {"code": "Z9"}},
        ]}),
        text_reply("**H3** leads."),
    ]);
    let turn = a.ask(&mut s, "Why?", None, &fake);
    assert!(turn["highlights"].as_array().unwrap().iter().all(Value::is_string));
    assert_eq!(turn["highlights"].as_array().unwrap().len(), 2, "app.js: r.turn.highlights.length");

    let view = a.view();
    assert!(view["available"].as_bool().unwrap());
    assert!(view["model"].is_string());
    let transcript = view["transcript"].as_array().unwrap();
    assert_eq!(transcript[0], json!({"role": "user", "text": "Why?"}));
    let bot = &transcript[1];
    assert_eq!(bot["role"], "assistant");
    assert!(bot["text"].is_string());
    assert!(bot["ms"].is_u64(), "(t.ms / 1000).toFixed(1)");
    assert!(bot["cost"].is_f64(), "t.cost.toFixed(4)");
    let tools = bot["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 3);
    assert_eq!(tools[0], json!({"name": "highlight", "input": {"codes": ["H1", "S1"]}, "error": false}));
    assert_eq!(tools[2]["error"], true);
    for pid in bot["proposals"].as_array().unwrap() {
        let p = &view["proposals"][pid.as_str().unwrap()];
        assert_eq!(&p["id"], pid);
        assert_eq!(p["status"], "pending");
        assert_eq!(p["label"], "crown H3");
        assert_eq!(p["reason"], "It leads.");
    }
}

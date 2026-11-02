#!/usr/bin/env python3
"""Write agent_fixtures.json: what the Python agent says for scripted model
replies, so agent_tests.rs can compare the Rust agent's output byte for byte.

    python3 rust/elessar-server/tests/fixtures/gen_agent_fixtures.py

Offline, as tools/expect.py is: fetch reads the pack's files, the clock is
frozen, Jev and the model are fakes. Each conversation stores every request
body as json.dumps wrote it. Two adjustments make runs comparable:
- The snapshot reads the audit trail as it is now. Python's `state["audit"]`
  stops at the last recompute, so it lacks lines logged after it. The Rust
  state is always current; that one difference is deliberate.
- Elapsed time is frozen, so `replied in N ms` reads 0 ms. The test normalises it.
"""
import json
import os
import sys
import types

ROOT = os.path.dirname(os.path.abspath(__file__))
for _ in range(4):
    ROOT = os.path.dirname(ROOT)
sys.path.insert(0, ROOT)
os.environ.pop("OPENROUTER_API_KEY", None)

from server import agent, jev, scenario, session as S  # noqa: E402

CLOCK = "2026-11-02T00:00:00Z"
S._now = lambda: CLOCK
agent.time = types.SimpleNamespace(time=lambda: 0.0)
agent.MODEL = "test/model"
jev.MODEL = "jev-test"
jev.available = lambda: False


def fake_ask(state, questions):
    """Deterministic and computable from the request; the Rust fake is the same."""
    n = len(state["record"])
    answers = {k: {"noul": ((n * 7 + len(k) * 3) % 17) / 17.0 + 0.02} for k in questions}
    return {"answers": answers, "model": "jev-test", "ms": 5, "cost": 0.001, "cached": False}


jev.ask = fake_ask


class Offline(S.Session):
    def fetch(self, sid):
        with open(self.pack["sources"][sid]["path"], encoding="utf-8") as f:
            raw = f.read()
        return raw, 0, len(raw.encode("utf-8"))


PACK = scenario.load_pack(os.path.join(ROOT, "scenarios", "change-cascade"))
ALL = PACK["source_order"]


def live_snapshot(s, selected=None):
    return REAL_SNAPSHOT({**s.state, "audit": s.audit[-80:]}, selected)


REAL_SNAPSHOT = agent.snapshot


def build(setup, jev_steps=False):
    """Replay a setup, which the Rust test replays too. Sources connect with Jev
    off: Python logs Jev before the insights a connect earns and the Rust core
    after, which would only reorder the audit trail."""
    s = Offline(PACK, "offline")
    for payload in setup:
        jev.available = (lambda: True) if jev_steps and payload["action"] == "engine" else (lambda: False)
        s.act(payload)
    jev.available = lambda: False
    return s


def connect_all(sources=ALL):
    return [{"action": "connect", "source": sid} for sid in sources]


def decided():
    """The merges of expect.json committed, as concrete ids."""
    s = build(connect_all())
    steps = connect_all()
    while s.state["merges"]["pending"]:
        step = {"action": "merge", "proposal": s.state["merges"]["pending"][0]["id"]}
        s.act(step)
        steps.append(step)
    return steps


def code_of(s, node_id):
    return next(n["code"] for n in s.state["nodes"] if n["id"] == node_id)


DECIDED = decided()
SCENES = {
    "empty": [],
    "partial": connect_all(ALL[:3]),
    "undecided": connect_all(),
    "decided": DECIDED,
    "judged": DECIDED + [
        {"action": "judge", "signal": "off-hours", "judgement": "discount"},
        {"action": "judge", "signal": "fx-healthy", "judgement": "dismiss"},
        {"action": "feedback", "on": True},
        {"action": "keep", "hypothesis": "planned"},
        {"action": "crown", "hypothesis": "misconfig"},
    ],
    "disconnected": DECIDED + [{"action": "disconnect", "source": ALL[2]}, {"action": "disconnect", "source": ALL[3]}],
    "jev_engine": DECIDED + [{"action": "engine", "engine": "jev"}],
}


def scene_fixture(setup, jev_on):
    s = build(setup, jev_steps=jev_on)
    selected = next((n["id"] for n in s.state["nodes"] if n["band"] == "hypothesis"), None)
    return {
        "setup": setup,
        "snapshot": json.dumps(live_snapshot(s), separators=(",", ":")),
        "snapshot_selected": json.dumps(live_snapshot(s, selected), separators=(",", ":")),
        "selected": selected,
        "pieces": {n["code"]: json.dumps(agent.piece_detail(s.state, n["code"])) for n in s.state["nodes"]},
    }


# ---------------------------------------------------------------- conversations

def text(t, stop="end_turn", cost=0.0012):
    return {"id": "msg", "type": "message", "role": "assistant", "stop_reason": stop,
            "content": [{"type": "thinking", "thinking": "hmm", "signature": "sig=="}, {"type": "text", "text": t}],
            "usage": {"input_tokens": 10, "output_tokens": 2, "cost": cost}}


def calls(*uses, say=None):
    content = [{"type": "thinking", "thinking": "planning", "signature": "sig=="}]
    if say:
        content.append({"type": "text", "text": say})
    for i, (name, args) in enumerate(uses):
        content.append({"type": "tool_use", "id": f"toolu_{name}_{i}", "name": name, "input": args})
    return {"id": "msg", "type": "message", "role": "assistant", "stop_reason": "tool_use", "content": content,
            "usage": {"input_tokens": 10, "output_tokens": 2, "cost": 0.002}}


def records_of(s, sid):
    return [r["pointer"] for r in s._records(sid)]


def conversation(setup, steps, jev_on=False):
    """steps: {"ask", "selected"?, "responses"} or {"decide", "decision"}. Only
    the last request of an ask is kept: history is append-only, so it contains
    what the earlier ones sent."""
    s = build(setup)
    jev.available = (lambda: True) if jev_on else (lambda: False)
    agent.snapshot = lambda st, sel=None: live_snapshot(s, sel)
    out = []
    for step in steps:
        if "decide" in step:
            r = s.act({"action": "agent_decide", "proposal": step["decide"], "decision": step["decision"]})
            out.append({**step, "events": r["events"], "beliefs": json.dumps({n["code"]: n["p"] for n in s.state["nodes"]})})
            continue
        script = list(step["responses"])
        requests = []

        def call(body, script=script, requests=requests):
            requests.append(json.dumps(body))
            return json.loads(json.dumps(script.pop(0)))

        agent._call = call
        turn = s.agent.ask(step["ask"], step.get("selected"))
        out.append({**step, "request_count": len(requests), "last_request": requests[-1], "turn": json.loads(json.dumps(turn))})
    view = json.loads(json.dumps(s.agent.view()))
    agent.snapshot = REAL_SNAPSHOT
    jev.available = lambda: False
    return {"setup": setup, "jev_on": jev_on, "steps": out, "view": view,
            "audit": [f"{a['action']}: {a['detail']}" for a in s.audit]}


def probe():
    """What the conversations refer to: real codes and pointers from the board."""
    s = build(DECIDED)
    codes = {b: [n["code"] for n in s.state["nodes"] if n["band"] == b] for b in ("signal", "participant", "hypothesis")}
    firing = next(n["code"] for n in s.state["nodes"] if n["band"] == "signal" and n["status"] == "firing")
    return s, codes, firing


def conversations():
    s, codes, firing = probe()
    sig, part, hyp = codes["signal"][0], codes["participant"][0], codes["hypothesis"][0]
    source = ALL[0]
    pointers = records_of(s, source)
    leading = code_of(s, "misconfig")
    out = {}

    out["plain"] = conversation(DECIDED, [
        {"ask": "What explains this so far?", "selected": s.state["nodes"][0]["id"], "responses": [text("Mostly H1.\n\nCafé ☃ \U0001F600.")]},
        {"ask": "And now?", "responses": [text("Still H1.", cost=0.5)]},
    ])

    out["tools"] = conversation(DECIDED, [
        {"ask": "Walk me through it — every tool.", "selected": s.state["nodes"][-1]["id"], "responses": [
            calls(("get_piece", {"code": sig}), ("get_piece", {"code": " " + part.lower() + " "}), ("get_piece", {"code": hyp}), ("get_piece", {"code": "Z9"}), ("get_piece", {}),
                  say="Looking."),
            calls(("get_record", {"source": source, "pointer": pointers[0]}), ("get_record", {"source": source, "pointer": "nope"}), ("get_record", {"source": "nowhere", "pointer": "x"}), ("get_record", {"source": source})),
            calls(("what_if", {"changes": [{"action": "judge", "signal": firing, "judgement": "dismiss"}]}),
                  ("what_if", {"changes": [{"action": "disconnect", "source": ALL[2]}, {"action": "disconnect", "source": ALL[3]}]}),
                  ("what_if", {"changes": [{"action": "engine", "engine": "jev"}]}),
                  ("what_if", {"changes": [{"action": "judge", "signal": firing, "judgement": "trust"}]}),
                  ("what_if", {"changes": [{"action": "disconnect", "source": "nowhere"}]}),
                  ("what_if", {"changes": [{"action": "judge", "signal": "S99", "judgement": "trust"}]}),
                  ("what_if", {"changes": [{"action": "merge", "merge": "M9"}]}),
                  ("what_if", {"changes": [{"action": "engine", "engine": "magic"}]}),
                  ("what_if", {"changes": [{"action": "explode"}]}),
                  ("what_if", {"changes": [{}]}),
                  ("what_if", {})),
            calls(("ask_jev", {"question": "Is this an outage?", "source": source}),
                  ("ask_jev", {"question": "Is this “bad”?", "source": source, "pointers": pointers[:2]}),
                  ("ask_jev", {"question": "Empty?", "source": source, "pointers": ["nope"]}),
                  ("ask_jev", {"question": "Where?", "source": "nowhere"}),
                  ("ask_jev", {"question": "No source"})),
            calls(("highlight", {"codes": [hyp.lower(), "Z9", sig, hyp]}), ("highlight", {}),
                  ("propose_action", {"action": "judge", "signal": firing.lower(), "judgement": "discount", "reason": "It looks weak."}),
                  ("propose_action", {"action": "crown", "hypothesis": leading, "reason": "It leads."}),
                  ("propose_action", {"action": "keep", "hypothesis": "S1", "reason": "Wrong band."}),
                  ("propose_action", {"action": "engine", "engine": "jev"}),
                  ("no_such_tool", {"x": 1}),
                  ("get_piece", None)),
            text("That is everything."),
        ]},
    ], jev_on=True)

    out["jev_off"] = conversation(DECIDED, [
        {"ask": "Ask Jev.", "responses": [calls(("ask_jev", {"question": "Q?", "source": source})), text("Jev is off.")]},
    ])

    out["partial"] = conversation(connect_all(ALL[:3]), [
        {"ask": "What if I connect the rest?", "responses": [
            calls(("what_if", {"changes": [{"action": "connect", "source": ALL[3]}]}),
                  ("what_if", {"changes": [{"action": "connect", "source": sid} for sid in ALL[3:]]}),
                  ("get_record", {"source": ALL[4], "pointer": "x"}),
                  ("ask_jev", {"question": "Q?", "source": ALL[4]}),
                  ("propose_action", {"action": "disconnect", "source": ALL[0], "reason": "Noisy."})),
            text("Connect them."),
        ]},
    ], jev_on=True)

    out["undecided"] = conversation(connect_all(), [
        {"ask": "What about the merge?", "responses": [
            calls(("what_if", {"changes": [{"action": "merge", "merge": "m1"}]}),
                  ("what_if", {"changes": [{"action": "reject_merge", "merge": "M1"}]}),
                  ("propose_action", {"action": "merge", "merge": "m1", "reason": "Same person."}),
                  ("propose_action", {"action": "reject_merge", "merge": "M1", "reason": "Or not."})),
            text("Commit it."),
        ]},
        {"decide": "p1", "decision": "approve"},
        {"ask": "Better?", "responses": [text("Better.")]},
    ])

    out["decide"] = conversation(DECIDED, [
        {"ask": "Suggest things.", "responses": [
            calls(("propose_action", {"action": "judge", "signal": firing, "judgement": "dismiss", "reason": "Weak."}),
                  ("propose_action", {"action": "crown", "hypothesis": leading, "reason": "It leads."}),
                  ("propose_action", {"action": "disconnect", "source": ALL[1], "reason": "Noisy."})),
            text("Three ideas."),
        ]},
        {"decide": "p1", "decision": "approve"},
        {"decide": "p3", "decision": "decline"},
        {"ask": "Done. Next?", "responses": [text("Noted.")]},
        {"decide": "p2", "decision": "approve"},
        {"ask": "Next?", "responses": [text("Crowned.")]},
    ])

    # What a real reply can do at the end of a turn, with the next ask showing
    # the history the API would see.
    out["stops"] = conversation(DECIDED, [
        {"ask": "Refuse, with words.", "responses": [text("I won't do that.", stop="refusal")]},
        {"ask": "Too long.", "responses": [text("A long", stop="max_tokens")]},
        {"ask": "Tools, then too long.", "responses": [calls(("highlight", {"codes": [hyp]}), say="First."), text("Then", stop="max_tokens")]},
        {"ask": "Forever.", "responses": [calls(("highlight", {"codes": [sig]}), say=f"Step {i}.") for i in range(agent.MAX_STEPS)]},
        {"ask": "Forever, silently.", "responses": [calls(("highlight", {"codes": [part]})) for _ in range(agent.MAX_STEPS)]},
        {"ask": "Still there?", "responses": [text("Yes.", stop="stop_sequence")]},
    ])
    return out


def main():
    fixtures = {
        "pack": "change-cascade",
        "system": agent.SYSTEM,
        "tools": json.dumps(agent.TOOLS),
        "max_steps": agent.MAX_STEPS,
        "scenes": {name: scene_fixture(setup, jev_on=name == "jev_engine") for name, setup in SCENES.items()},
        "conversations": conversations(),
    }
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "agent_fixtures.json")
    with open(path, "w", encoding="utf-8") as f:
        json.dump(fixtures, f, indent=1)
        f.write("\n")
    print(f"{path}: {os.path.getsize(path)} bytes")


main()

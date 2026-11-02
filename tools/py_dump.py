#!/usr/bin/env python3
"""Dump what the Python engine says, for rust/elessar-core/tests/python_diff_tests.rs.

    python3 py_dump.py <repo> <out dir>     # then ELESSAR_PY_DUMP=<out dir> cargo test ...

Offline: fetch reads the pack's files, as tools/expect.py does. The clock is
frozen so at_connect is comparable. Writes <out dir>/<pack>.json:
  undecided    state, every source connected, no merge decided
  decided      state after deciding merges per expect.json, until none is pending
  committed    the merge ids Python committed; rejected: the ones it rejected
  timeline     of `decided`
  policies     {throughout|after|at_connect: {state, timeline}} with those decisions
  partial      {state, timeline} with half the rack connected
  judged       {state, timeline} with judgements, feedback and a kept hypothesis
  script       [{payload, insights, state}] a walk through every action
"""
import copy
import json
import os
import sys

repo, out = sys.argv[1], sys.argv[2]
sys.path.insert(0, repo)
from server import scenario, session as S  # noqa: E402

CLOCK = "2026-11-02T00:00:00Z"
S._now = lambda: CLOCK


class Offline(S.Session):
    def fetch(self, sid):
        with open(self.pack["sources"][sid]["path"], encoding="utf-8") as f:
            raw = f.read()
        return raw, 0, len(raw.encode("utf-8"))


def pair_in(p, pairs):
    a, b = str(p["a"]["value"]), str(p["b"]["value"])
    return any({a, b} == {x, y} for x, y in pairs)


def view(s):
    return {k: v for k, v in s.public_state().items() if k != "agent"}


def decide(s, spec):
    """Same loop as the Rust play(): the first pending proposal until none is left."""
    while s.state["merges"]["pending"]:
        p = s.state["merges"]["pending"][0]
        reject = pair_in(p, spec["merges"]["reject"]) or spec["merges"]["default"] != "commit"
        s.act({"action": "reject" if reject else "merge", "proposal": p["id"]})


def connected(pack, order=None):
    s = Offline(pack, "offline")
    for sid in order or pack["source_order"]:
        s.act({"action": "connect", "source": sid})
    return s


def apply(s, committed, rejected):
    for p in rejected:
        s.act({"action": "reject", "proposal": p})
    for p in committed:
        s.act({"action": "merge", "proposal": p})


def board(s):
    return {"state": view(s), "timeline": s.timeline()}


def script(pack, spec):
    """Every action once, with ids chosen from the board as it goes."""
    s = Offline(pack, "offline")
    steps = []

    def act(**payload):
        res = s.act(payload)
        steps.append({"payload": payload, "insights": res["events"], "state": view(s)})

    order = pack["source_order"]
    act(action="peek", source=order[0])
    act(action="peek", source=order[-1])
    for sid in order:
        act(action="connect", source=sid)
    pending = s.state["merges"]["pending"]
    if pending:
        act(action="reject", proposal=pending[-1]["id"])
        act(action="merge", proposal=pending[0]["id"])
        act(action="unmerge", proposal=pending[0]["id"])
        act(action="merge", proposal=pending[0]["id"])
    decide_steps = []
    while s.state["merges"]["pending"]:
        p = s.state["merges"]["pending"][0]
        reject = pair_in(p, spec["merges"]["reject"]) or spec["merges"]["default"] != "commit"
        act(action="reject" if reject else "merge", proposal=p["id"])
    nodes = s.state["nodes"]
    sigs = [n["id"] for n in nodes if n["band"] == "signal"]
    hyps = [n for n in nodes if n["band"] == "hypothesis"]
    best = max(hyps, key=lambda n: n["p"])
    other = next(n for n in hyps if n["id"] != best["id"])
    captured = [n["id"] for n in hyps if n["status"] == "captured"]
    act(action="crown", hypothesis=other["id"])
    act(action="crown", hypothesis=best["id"])
    act(action="crown", hypothesis=best["id"])
    if captured:
        act(action="keep", hypothesis=captured[0])
        act(action="release", hypothesis=captured[0])
    act(action="judge", signal=sigs[0], judgement="discount")
    act(action="judge", signal=sigs[1], judgement="dismiss")
    act(action="judge", signal=sigs[0], judgement="trust")
    act(action="judge", signal=sigs[1], judgement="trust")
    act(action="feedback", on=True)
    act(action="feedback", on=False)
    if s.state["events"]:
        act(action="ack", event=s.state["events"][0]["id"])
        act(action="unack", event=s.state["events"][0]["id"])
    if s.state["undated"]:
        act(action="ack", event=s.state["undated"][0]["id"])
        act(action="unack", event=s.state["undated"][0]["id"])
    act(action="disconnect", source=order[0])
    act(action="disconnect", source=order[1])
    act(action="crown", hypothesis=best["id"])
    act(action="reconnect", sources=[order[0], order[1]])
    act(action="engine", engine="jev")
    act(action="engine", engine="rules")
    act(action="reset")
    act(action="connect", source=order[-1])
    return steps


def dump(pid):
    pack = scenario.load_pack(os.path.join(repo, "scenarios", pid))
    pack["jev_off"] = True
    spec = json.load(open(os.path.join(pack["dir"], "expect.json")))
    s = connected(pack)
    d = {"undecided": view(s)}
    decide(s, spec)
    committed, rejected = list(s.committed), sorted(s.rejected)
    d.update(decided=view(s), committed=committed, rejected=rejected, timeline=s.timeline())

    d["policies"] = {}
    for policy in ("throughout", "after", "at_connect"):
        p2 = dict(pack, unknown_time=policy)
        s2 = connected(p2)
        apply(s2, committed, rejected)
        d["policies"][policy] = board(s2)

    half = pack["source_order"][: len(pack["source_order"]) // 2 + 1]
    s3 = connected(pack, half)
    apply(s3, committed, rejected)
    s3.act({"action": "disconnect", "source": half[0]})
    d["partial"] = {"sources": half[1:], "state_before": None, **board(s3)}

    s4 = connected(pack)
    apply(s4, committed, rejected)
    sigs = [n["id"] for n in s4.state["nodes"] if n["band"] == "signal"]
    hyps = [n for n in s4.state["nodes"] if n["band"] == "hypothesis"]
    moves = [{"action": "judge", "signal": sigs[0], "judgement": "discount"},
             {"action": "judge", "signal": sigs[-1], "judgement": "dismiss"},
             {"action": "feedback", "on": True}]
    for h in hyps:
        if h["status"] == "captured":
            moves.append({"action": "keep", "hypothesis": h["id"]})
            break
    for m in moves:
        s4.act(m)
    d["judged"] = {"moves": moves, **board(s4)}

    d["script"] = script(pack, spec)
    with open(os.path.join(out, f"{pid}.json"), "w") as f:
        json.dump(d, f)
    print(pid, os.path.getsize(os.path.join(out, f"{pid}.json")), "bytes;", len(committed), "committed;", len(rejected), "rejected")


os.environ.pop("OPENROUTER_API_KEY", None)
for pid in sorted(x for x in os.listdir(os.path.join(repo, "scenarios")) if os.path.isfile(os.path.join(repo, "scenarios", x, "manifest.json"))):
    dump(pid)

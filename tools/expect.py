#!/usr/bin/env python3
"""Check an engine against each pack's expect.json: the executable definition
of what a scenario must do once every source is connected and the planted
merges are decided. Today it drives the Python engine in-process; the Rust
engine runs the same checks in its own test suite.

    python3 tools/expect.py            # every pack
    python3 tools/expect.py fraud-ring

expect.json:
  merges.default      "commit" | "reject" for proposals not listed in merges.reject
  merges.reject       [[a, b], ...] value pairs (either order) to reject
  proposals_seen      [[a, b], ...] value pairs that must be proposed at some point
  final.crownable     hypothesis id that must be crownable at the end
  final.captured      hypothesis ids that must be explained away
  final.firing        signal ids that must be firing
  final.resolved      participant ids that must be resolved (named)
  final.undated       count of undated records
  final.skipped       {source: rows skipped}
  order_independent   "all" (every connection order), an integer (that many
                      random orders) or false; the final board must not depend
                      on order
"""
import itertools
import json
import os
import random
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
from server import scenario  # noqa: E402
from server.session import Session  # noqa: E402


class Offline(Session):
    def fetch(self, sid):
        with open(self.pack["sources"][sid]["path"], encoding="utf-8") as f:
            raw = f.read()
        return raw, 0, len(raw)


def pair_in(p, pairs):
    a, b = str(p["a"]["value"]), str(p["b"]["value"])
    return any({a, b} == {x, y} for x, y in pairs)


def play(pack, order, spec):
    s = Offline(pack, "offline")
    seen = []
    for sid in order:
        s.act({"action": "connect", "source": sid})
        for p in list(s.state["merges"]["pending"]):
            seen.append((str(p["a"]["value"]), str(p["b"]["value"])))
            decision = "reject" if pair_in(p, spec["merges"]["reject"]) else spec["merges"]["default"]
            s.act({"action": "merge" if decision == "commit" else "reject", "proposal": p["id"]})
    return s.state, seen


def summary(state):
    by = {n["id"]: n for n in state["nodes"]}
    return {
        "crownable": sorted(i for i, n in by.items() if n["band"] == "hypothesis" and n["status"] == "crownable"),
        "captured": sorted(i for i, n in by.items() if n["band"] == "hypothesis" and n["status"] == "captured"),
        "firing": sorted(i for i, n in by.items() if n["band"] == "signal" and n["status"] == "firing"),
        "resolved": sorted(i for i, n in by.items() if n["band"] == "participant" and n["status"] == "resolved"),
        "undated": len(state.get("undated", [])),
        "skipped": {x["id"]: x["skipped"] for x in state["sources"] if x.get("skipped")},
        "beliefs": {i: round(n["p"], 6) for i, n in by.items()},
    }


def check(pid):
    pack = scenario.load_pack(os.path.join(ROOT, "scenarios", pid))
    spec_path = os.path.join(pack["dir"], "expect.json")
    if not os.path.isfile(spec_path):
        print(f"{pid}: no expect.json, skipped")
        return True
    spec = json.load(open(spec_path))
    order = pack["source_order"]
    state, seen = play(pack, order, spec)
    got = summary(state)
    want = spec["final"]
    failures = []
    if got["crownable"] != [want["crownable"]]:
        failures.append(f"crownable: want [{want['crownable']}], got {got['crownable']}")
    for key in ("captured", "firing", "resolved"):
        if got[key] != sorted(want[key]):
            failures.append(f"{key}: want {sorted(want[key])}, got {got[key]}")
    if got["undated"] != want["undated"]:
        failures.append(f"undated: want {want['undated']}, got {got['undated']}")
    if got["skipped"] != want["skipped"]:
        failures.append(f"skipped: want {want['skipped']}, got {got['skipped']}")
    for a, b in spec.get("proposals_seen", []):
        if not any({a, b} == {x, y} for x, y in seen):
            failures.append(f"proposal never seen: {a} / {b} (seen: {sorted(set(seen))})")

    oi = spec.get("order_independent", False)
    if oi:
        orders = list(itertools.permutations(order)) if oi == "all" else None
        if orders is None:
            rng = random.Random(1)
            orders = []
            for _ in range(int(oi)):
                o = list(order); rng.shuffle(o); orders.append(tuple(o))
        base = {k: v for k, v in got.items()}
        for o in orders:
            other = summary(play(pack, o, spec)[0])
            if other != base:
                diff = {k: (base[k], other[k]) for k in base if base[k] != other[k]}
                failures.append(f"order {list(o)} changes the final board: {diff}")
                break
        checked = f", order-independent over {len(orders)} orders" if not failures else ""
    else:
        checked = ""

    beliefs = " ".join(f"{i}={p:.2f}" for i, p in got["beliefs"].items() if i in {n["id"] for n in state["nodes"] if n["band"] == "hypothesis"})
    if failures:
        print(f"FAIL  {pid}")
        for f in failures:
            print(f"      {f}")
        return False
    print(f"OK    {pid}: {beliefs}{checked}")
    return True


if __name__ == "__main__":
    packs = sys.argv[1:] or sorted(d for d in os.listdir(os.path.join(ROOT, "scenarios"))
                                  if os.path.isfile(os.path.join(ROOT, "scenarios", d, "manifest.json")))
    ok = all([check(p) for p in packs])
    sys.exit(0 if ok else 1)

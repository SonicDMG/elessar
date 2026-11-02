#!/usr/bin/env python3
"""Random small packs that reach the timeline sweep's corners, and what Python says about them.

    python3 py_synth.py <repo> <out dir> [count]

Each pack lands in <out dir>/synth/<n>/ with a dump.json next to it. The corners:
keys that join clusters only once a later record arrives, several clusters
holding an anchor value (the first in fragment order wins, and that changes
over time), two participants on one cluster, committed merges with an end that
arrives late, an empty title after `strip`, implicates limited by type, count
groups that reach `min` late, co_resolved with a field strength and clauses in
one or two sources, undated records, unparseable times, document times, Jev
answers with gaps, judgements and feedback.
"""
import json
import os
import random
import shutil
import sys

repo, out = sys.argv[1], sys.argv[2]
count = int(sys.argv[3]) if len(sys.argv) > 3 else 40
sys.path.insert(0, repo)
from server import scenario, session as S  # noqa: E402

S._now = lambda: "2026-11-02T00:00:00Z"
os.environ.pop("OPENROUTER_API_KEY", None)


class Offline(S.Session):
    def fetch(self, sid):
        with open(self.pack["sources"][sid]["path"], encoding="utf-8") as f:
            raw = f.read()
        return raw, 0, len(raw.encode("utf-8"))


NAMES = ["Ada Vale", "Ada Vayle", "Bo Reyes", "Cy Kwan", "Di North", "Ed Stone", "  ", "Flo Hart"]
EMAILS = ["a@x.io", "b@x.io", "c@x.io", "d@x.io", "e@x.io"]
HOSTS = ["h1", "h2", "h3", "h4"]
KINDS = ["hold", "ok", "fail", "late"]


def stamp(rng):
    day = rng.randint(1, 5)
    return f"2026-10-{day:02d}T{rng.randint(0, 23):02d}:{rng.choice([0, 15, 30, 59]):02d}:00Z"


def row(rng, i):
    r = {"id": f"r{i}", "email": rng.choice(EMAILS), "name": rng.choice(NAMES), "host": rng.choice(HOSTS),
         "kind": rng.choice(KINDS), "score": rng.choice([0.1, 0.4, 0.7, 0.95, "x", -0.5, 2]), "t": stamp(rng)}
    roll = rng.random()
    if roll < 0.08:
        r["t"] = ""
    elif roll < 0.16:
        r["t"] = "not a time"
    elif roll < 0.2:
        del r["t"]
    for k in ("email", "name", "host"):
        if rng.random() < 0.12:
            del r[k]
    return r


def where(rng):
    return rng.choice([None, {"kind": rng.choice(KINDS)}, {"kind": {"in": rng.sample(KINDS, 2)}},
                       {"score": {"gte": 0.4}}, {"host": {"exists": True}}])


def strength(rng):
    return rng.choice([rng.choice([0.5, 0.8, 1.0]), {"field": "score"}])


def build(n, d):
    rng = random.Random(1000 + n)
    os.makedirs(os.path.join(d, "sources"))
    source_ids = ["alpha", "beta", "gamma", "delta"][: rng.randint(2, 4)]
    sources = []
    for sid in source_ids:
        rows = [row(rng, i) for i in range(rng.randint(4, 22))]
        time_conf = rng.choice([{"field": "t", "unit": "iso"}] * 4 + [None, {"document": {"constant": "2026-10-03T12:00:00Z"}, "unit": "iso"}])
        with open(os.path.join(d, "sources", sid + ".jsonl"), "w") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")
        s = {"id": sid, "name": sid.title(), "kind": "k", "format": "jsonl", "file": f"sources/{sid}.jsonl", "confidence": rng.choice([0.6, 0.8, 0.95])}
        if time_conf:
            s["time"] = time_conf
        sources.append(s)
    with open(os.path.join(d, "manifest.json"), "w") as f:
        json.dump({"id": f"synth-{n}", "title": "T", "premise": "P", "unknown_time": rng.choice(["throughout", "after", "at_connect"]), "sources": sources}, f)

    types = {
        "Person": {"title": rng.choice(["name", ["name", "email"], ["nick", "name"]]), "keys": rng.choice([["email"], ["email", "name"]]), "multi": ["host"],
                   "fuzzy": {"properties": ["name"], "threshold": 0.8}},
        "Host": {"title": "label", "keys": ["hid"]},
    }
    mappings = []
    for sid in source_ids:
        if rng.random() < 0.9:
            mappings.append({"source": sid, "type": "Person", "props": {"email": "email", "name": {"from": "name", "transform": ["strip"]}}})
        if rng.random() < 0.3:
            mappings.append({"source": sid, "type": "Person", "when": {"kind": "ok"}, "props": {"name": "name", "host": "host"}})
        if rng.random() < 0.8:
            props = {"hid": "host"}
            if rng.random() < 0.5:
                props["label"] = {"from": "kind", "transform": "upper"}
            m = {"source": sid, "type": "Host", "props": props}
            if rng.random() < 0.3:
                m["when"] = {"kind": {"ne": "late"}}
            mappings.append(m)
    with open(os.path.join(d, "mapping.json"), "w") as f:
        json.dump({"object_types": types, "mappings": mappings}, f)

    signals = []
    for i in range(rng.randint(3, 7)):
        sid = rng.choice(source_ids)
        kind = rng.choice(["match", "match", "time_window", "count", "co_resolved"])
        sig = {"id": f"sig-{i}", "label": f"Signal {i}", "kind": kind, "strength": strength(rng), "explain": "e"}
        imp = {"w": rng.choice([0.4, 0.8, 1.0])}
        if rng.random() < 0.4:
            imp["types"] = rng.sample(["Person", "Host"], 1)
        elif rng.random() < 0.1:
            imp["types"] = []
        sig["implicates"] = imp
        if kind == "co_resolved":
            n_clauses = rng.randint(1, 3)
            sig["params"] = {"type": rng.choice(["Person", "Host"]),
                             "clauses": [{"source": rng.choice(source_ids), **({"where": w} if (w := where(rng)) else {})} for _ in range(n_clauses)]}
        else:
            sig["source"] = sid
            w = where(rng)
            if w:
                sig["where"] = w
            if kind == "time_window":
                sig["params"] = {"field": "t", "unit": "iso", "hours": rng.choice([[9, 17], [22, 6]])}
                # Python raises on a time that does not parse, so keep this rule to good ones.
                sig["where"] = {**(sig.get("where") or {}), "t": {"startswith": "2026"}}
            elif kind == "count":
                sig["params"] = {"group_by": rng.choice(["host", "kind", "email"]), "min": rng.randint(2, 4)}
            if kind in ("match", "time_window") and rng.random() < 0.5:
                jev = {"question": "q?", "threshold": rng.choice([0.5, 0.7])}
                if rng.random() < 0.5:
                    jev["where"] = {"kind": {"ne": "ok"}}
                if rng.random() < 0.5:
                    jev["criteria"] = {"true": "y", "false": "n"}
                sig["jev"] = jev
        signals.append(sig)
    with open(os.path.join(d, "signals.json"), "w") as f:
        json.dump(signals, f)

    parts = []
    used = rng.sample(EMAILS, rng.randint(1, 3))
    for i, e in enumerate(used):
        parts.append({"id": f"person-{i}", "type": "Person", "anchor": {"email": e}})
    if rng.random() < 0.5:
        parts.append({"id": "named", "type": "Person", "anchor": {"name": rng.choice(["Ada Vale", "Bo Reyes"])}, "leak": 0.05})
    for h in rng.sample(HOSTS, rng.randint(1, 2)):
        parts.append({"id": f"host-{h}", "type": "Host", "anchor": {"hid": h}})
    hyps = [{"id": f"hyp{i}", "label": f"Hyp {i}", "prior": rng.choice([0.02, 0.1, 0.3]), "blurb": "b"} for i in range(rng.randint(2, 4))]
    edges = []
    for h in hyps:
        for p in rng.sample(parts, rng.randint(1, len(parts))):
            edges.append({"from": p["id"], "to": h["id"], "w": rng.choice([0.3, 0.6, 0.9])})
        for s in rng.sample(signals, rng.randint(1, min(3, len(signals)))):
            edges.append({"from": s["id"], "to": h["id"], "w": rng.choice([0.5, 0.8, 0.97]), "kind": rng.choice(["support", "inhibit", "inhibit"])})
    if rng.random() < 0.4:
        edges.append({"from_type": "Host", "to": hyps[0]["id"], "w": 0.4})
    with open(os.path.join(d, "model.json"), "w") as f:
        json.dump({"participants": parts, "hypotheses": hyps, "edges": edges}, f)
    with open(os.path.join(d, "narrative.json"), "w") as f:
        json.dump({"intro": "i", "finale": "f", "insights": {"first_link": {"title": "Linked {{now}}"}}}, f)


def view(s):
    return {k: v for k, v in s.public_state().items() if k != "agent"}


def play(n, d):
    pack = scenario.load_pack(d)
    rng = random.Random(5000 + n)
    s = Offline(pack, "offline")
    steps = []
    order = list(pack["source_order"])
    rng.shuffle(order)

    def act(**payload):
        res = s.act(payload)
        steps.append({"payload": payload, "insights": res["events"], "state": view(s), "timeline": s.timeline()})

    for sid in order:
        act(action="connect", source=sid)
        pending = s.state["merges"]["pending"]
        if pending and rng.random() < 0.8:
            p = rng.choice(pending)
            # The fuzzy measure may not propose the same pairs: Rust replays these ids.
            act(action="merge" if rng.random() < 0.7 else "reject", proposal=p["id"])

    # Merges no similarity proposed, across clusters, so ends can arrive late.
    frag_ids = sorted({f for n_ in s.state["nodes"] if n_["band"] == "participant" and n_["detail"].get("props")
                       for vals in n_["detail"]["props"].values() for v in vals for f in [x["fragment"] for x in v["provenance"]]})
    same_type = {}
    for fid in frag_ids:
        same_type.setdefault(fid.split(":")[1], []).append(fid)
    pools = [v for v in same_type.values() if len(v) >= 2]
    if pools and rng.random() < 0.6:
        a, b = rng.sample(rng.choice(pools), 2)
        act(action="merge", proposal="|".join(sorted((a, b))))

    sigs = [x["id"] for x in pack["signals"]]
    act(action="judge", signal=rng.choice(sigs), judgement=rng.choice(["discount", "dismiss"]))
    if rng.random() < 0.5:
        act(action="feedback", on=True)
    hyps = [x for x in s.state["nodes"] if x["band"] == "hypothesis"]
    cap = [h["id"] for h in hyps if h["status"] == "captured"]
    if cap:
        act(action="keep", hypothesis=cap[0])

    # Jev answers: some signals complete, some with gaps. Stored the way _ask_jev stores them.
    answers = {}
    for sig in pack["signals"]:
        if not sig.get("jev"):
            continue
        sid = sig["source"]
        if sid not in s.connected:
            continue
        gaps = rng.random() < 0.3
        for r in s._records(sid):
            if gaps and rng.random() < 0.3:
                continue
            answers[f"{sig['id']}|{sid}|{r['index']}"] = {"noul": rng.choice([0.05, 0.3, 0.55, 0.75, 0.99]), "model": "jev-test", "ms": 1, "cached": True, "error": None}
    s.jev.update(answers)
    act(action="engine", engine="jev")
    if len(order) > 1 and rng.random() < 0.5:
        act(action="disconnect", source=order[0])
        act(action="reconnect", sources=[order[0]])
    steps[-1]["end"] = True
    return {"steps": steps, "jev": answers}


root = os.path.join(out, "synth")
shutil.rmtree(root, ignore_errors=True)
done = 0
for n in range(count):
    d = os.path.join(root, f"{n:03d}")
    os.makedirs(d)
    build(n, d)
    try:
        dump = play(n, d)
    except Exception as e:
        print(f"{n}: python failed: {type(e).__name__}: {e}")
        shutil.rmtree(d)
        continue
    with open(os.path.join(d, "dump.json"), "w") as f:
        json.dump(dump, f)
    done += 1
print(f"{done}/{count} packs dumped in {root}")

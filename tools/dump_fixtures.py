#!/usr/bin/env python3
"""Dump module-level fixtures from the Python engine for the Rust port's
unit tests. These are internal shapes (records, clusters, board numbers),
not API responses; they are regenerated on purpose only.

    python3 tools/dump_fixtures.py          # every pack -> golden/

golden/records/<pack>/<source>.json   {"records": [{index, pointer, excerpt, data}], "skipped": [...]}
golden/clusters/<pack>.json           all sources connected, no merges decided:
                                      {"clusters": {id: {type, fragments, sources, title, conflicts,
                                       props: {name: [{value, provenance: [{source, pointer, field, fragment}]}]}}},
                                       "proposals": [{id, type, score, a, b, conflict}]}
golden/board/<pack>.json              all sources connected, planted merges decided (expect.json):
                                      {"signals": {id: {status, activation, strength, confidence,
                                       hits: [[{source, pointer}], ...], links: [{participant, hits, w}]}},
                                       "nodes": {id: {p, status, present, breakdown}}, "stages": [...],
                                       "events": n, "undated": n, "links": [{a, b, weight}]}
"""
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
from server import pipeline, scenario  # noqa: E402
from server.session import Session  # noqa: E402

sys.path.insert(0, os.path.join(ROOT, "tools"))
from expect import Offline, pair_in  # noqa: E402


def dump(path, obj):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        json.dump(obj, f, indent=1, sort_keys=True)
        f.write("\n")


def records(pack):
    for sid, s in pack["sources"].items():
        if s["transport"] != "file":
            continue
        skipped = []
        with open(s["path"], encoding="utf-8") as f:
            recs = pipeline.parse(sid, s["format"], f.read(), s.get("options"), skipped)
        dump(os.path.join(ROOT, "golden", "records", pack["id"], f"{sid}.json"), {
            "records": [{"index": r["index"], "pointer": r["pointer"], "excerpt": r["excerpt"], "data": r["data"]} for r in recs],
            "skipped": skipped,
        })


def clusters(pack):
    s = Offline(pack, "offline")
    for sid in pack["source_order"]:
        s.act({"action": "connect", "source": sid})
    st = s.state
    out = {}
    # rebuild the clusters the way compute does, so ids and ordering are the engine's
    from server import resolve as rs
    records_, fragments = {}, []
    for sid in pack["source_order"]:
        recs = s._records(sid)
        records_[sid] = recs
        fragments.extend(pipeline.to_fragments(recs, [(mi, m) for mi, m in pack["mappings"] if m["source"] == sid]))
    res, _ = rs.resolve(fragments, pack["object_types"], [], pack["source_order"])
    by_frag = {f["id"]: f for f in fragments}
    for cid, c in res.items():
        out[cid] = {"type": c["type"], "fragments": c["fragments"], "sources": c["sources"], "title": c["title"],
                    "conflicts": c["conflicts"],
                    "props": {k: [{"value": v["value"], "provenance": [{kk: p[kk] for kk in ("source", "pointer", "field", "fragment")} for p in v["provenance"]]} for v in vals]
                              for k, vals in c["props"].items()}}
    props = rs.proposals(res, by_frag, pack["object_types"], set())
    dump(os.path.join(ROOT, "golden", "clusters", f"{pack['id']}.json"), {"clusters": out, "proposals": props})


def board(pack):
    spec_path = os.path.join(pack["dir"], "expect.json")
    spec = json.load(open(spec_path)) if os.path.isfile(spec_path) else {"merges": {"default": "commit", "reject": []}}
    s = Offline(pack, "offline")
    for sid in pack["source_order"]:
        s.act({"action": "connect", "source": sid})
        for p in list(s.state["merges"]["pending"]):
            decision = "reject" if pair_in(p, spec["merges"]["reject"]) else spec["merges"]["default"]
            s.act({"action": "merge" if decision == "commit" else "reject", "proposal": p["id"]})
    st = s.state
    sigs = {}
    for n in st["nodes"]:
        if n["band"] == "signal":
            d = n["detail"]
            sigs[n["id"]] = {"status": n["status"], "activation": n["p"], "strength": d["strength"], "confidence": d["confidence"],
                             "hits": [[{"source": r["source"], "pointer": r["pointer"]} for r in h["records"]] for h in d["hits"]],
                             "links": d["links"]}
    nodes = {n["id"]: {"p": n["p"], "status": n["status"], "present": n["present"], "breakdown": n["detail"].get("breakdown")}
             for n in st["nodes"]}
    dump(os.path.join(ROOT, "golden", "board", f"{pack['id']}.json"), {
        "signals": sigs, "nodes": nodes, "stages": st["stages"], "events": len(st["events"]), "undated": len(st["undated"]),
        "links": [{"a": l["a"], "b": l["b"], "weight": l["weight"]} for l in st["links"]],
        "merges_committed": [m["id"] for m in st["merges"]["committed"]],
    })


if __name__ == "__main__":
    packs = sys.argv[1:] or sorted(d for d in os.listdir(os.path.join(ROOT, "scenarios"))
                                  if os.path.isfile(os.path.join(ROOT, "scenarios", d, "manifest.json")))
    for pid in packs:
        pack = scenario.load_pack(os.path.join(ROOT, "scenarios", pid))
        records(pack); clusters(pack); board(pack)
        print(f"dumped {pid}")

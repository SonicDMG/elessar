"""Signal rules: records (and resolved clusters) -> signal hits.

The set of rule kinds is fixed in code; packs parameterize them. Adding a new
kind of rule is the one place a new domain needs code.

A hit carries the fragment ids it came from. That is how a signal finds the
entities it implicates: through resolution, never by hand-wiring.
"""
from .pipeline import get_path, matches, parse_time


def _record_hit(rec, frags_by_record):
    return {"records": [rec], "fragments": frags_by_record.get((rec["source"], rec["index"]), [])}


def _match(sig, ctx):
    recs = ctx["records"].get(sig["source"], [])
    return [_record_hit(r, ctx["frags_by_record"]) for r in recs if matches(r["data"], sig.get("where"))]


def _in_window(hour, start, end):
    return start <= hour < end if start <= end else (hour >= start or hour < end)


def _time_window(sig, ctx):
    p = sig["params"]
    start, end = p["hours"]
    hits = []
    for r in ctx["records"].get(sig["source"], []):
        if not matches(r["data"], sig.get("where")):
            continue
        raw = get_path(r["data"], p["field"])
        if raw in (None, ""):
            continue
        t = parse_time(raw, p.get("unit", "iso"))
        if _in_window(t.hour + t.minute / 60, start, end):
            hits.append(_record_hit(r, ctx["frags_by_record"]))
    return hits


def _count(sig, ctx):
    p = sig["params"]
    groups = {}
    for r in ctx["records"].get(sig["source"], []):
        if matches(r["data"], sig.get("where")):
            groups.setdefault(str(get_path(r["data"], p["group_by"])), []).append(r)
    hits = []
    for recs in groups.values():
        if len(recs) >= p.get("min", 2):
            frags = [f for r in recs for f in ctx["frags_by_record"].get((r["source"], r["index"]), [])]
            hits.append({"records": recs, "fragments": frags})
    return hits


def _co_resolved(sig, ctx):
    """Fires for each resolved object whose fragments satisfy every clause.

    Only possible after fusion: the clauses usually name different sources, so
    the rule cannot fire until resolution has joined them into one object."""
    p = sig["params"]
    hits = []
    for c in ctx["clusters"].values():
        if c["type"] != p["type"]:
            continue
        matched = []
        for clause in p["clauses"]:
            found = None
            for fid in c["fragments"]:
                f = ctx["fragments"][fid]
                rec = ctx["record_index"][f["record"]]
                if f["source"] == clause["source"] and matches(rec["data"], clause.get("where")):
                    found = rec
                    break
            if not found:
                break
            matched.append(found)
        else:
            hits.append({"records": matched, "fragments": list(c["fragments"])})
    return hits


KINDS = {"match": _match, "time_window": _time_window, "count": _count, "co_resolved": _co_resolved}


def required_sources(sig):
    if sig["kind"] == "co_resolved":
        return sorted({c["source"] for c in sig["params"]["clauses"]})
    return [sig["source"]]


def strength(sig, hits):
    s = sig.get("strength", 1.0)
    if isinstance(s, dict):
        vals = []
        for h in hits:
            for r in h["records"]:
                try:
                    vals.append(float(get_path(r["data"], s["field"])))
                except (TypeError, ValueError):
                    pass
        return max(vals) if vals else 0.0
    return float(s)


def evaluate(sig, ctx):
    return KINDS[sig["kind"]](sig, ctx)

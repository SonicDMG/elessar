"""Noisy-OR with inhibitors and competing hypotheses.

This is an approximation, not exact belief propagation. Every number it
produces is kept with the terms that made it, so the UI can show the
derivation for any piece.
"""

FEEDBACK_BETA = 0.15


def noisy_or(leak, supports, inhibitors):
    """supports / inhibitors: lists of {"from", "w", "p"}."""
    keep = 1.0 - leak
    terms = []
    for s in supports:
        t = s["w"] * s["p"]
        keep *= 1.0 - t
        terms.append({**s, "term": t})
    support = 1.0 - keep
    factor = 1.0
    iterms = []
    for i in inhibitors:
        f = 1.0 - i["w"] * i["p"]
        factor *= f
        iterms.append({**i, "factor": f})
    return {
        "leak": leak,
        "support": terms,
        "inhibit": iterms,
        "support_p": support,
        "inhibit_factor": factor,
        "raw": support * factor,
    }


def infer(signals, participants, hypotheses, edges, feedback=False):
    """
    signals:    {id: activation}
    participants:   {id: {"leak", "present", "parents": [{"from", "w"}]}}  (parents are signals)
    hypotheses: {id: {"prior"}}
    edges:      [{"from", "to", "w", "kind": "support"|"inhibit"}]  (to a hypothesis)

    Returns (p by node, breakdown by node, stages). Each stage is a snapshot of
    p after one pass; the board animates one stage per tick.
    """
    p = dict(signals)
    stages = [dict(p)]
    out = {}

    def participant_pass(boost=None):
        for eid, e in participants.items():
            if not e["present"]:
                p[eid] = 0.0
                out[eid] = None
                continue
            sup = [{"from": par["from"], "w": par["w"], "p": p.get(par["from"], 0.0)} for par in e["parents"]]
            if boost and boost.get(eid):
                sup.append({"from": "feedback", "w": FEEDBACK_BETA, "p": boost[eid]})
            b = noisy_or(e["leak"], sup, [])
            b["final"] = b["raw"]
            out[eid] = b
            p[eid] = b["raw"]

    def hypothesis_pass():
        raw = {}
        for hid, h in hypotheses.items():
            sup = [{"from": e["from"], "w": e["w"], "p": p.get(e["from"], 0.0)}
                   for e in edges if e["to"] == hid and e["kind"] == "support"]
            inh = [{"from": e["from"], "w": e["w"], "p": p.get(e["from"], 0.0)}
                   for e in edges if e["to"] == hid and e["kind"] == "inhibit"]
            out[hid] = noisy_or(h["prior"], sup, inh)
            raw[hid] = out[hid]["raw"]
            p[hid] = raw[hid]
        stages.append(dict(p))
        total = sum(raw.values())
        divisor = total if total > 1.0 else 1.0
        for hid in hypotheses:
            out[hid]["divisor"] = divisor
            out[hid]["final"] = raw[hid] / divisor
            p[hid] = out[hid]["final"]
        stages.append(dict(p))

    participant_pass()
    stages.append(dict(p))
    hypothesis_pass()

    if feedback:
        boost = {}
        for e in edges:
            if e["kind"] == "support" and e["from"] in participants and e["to"] in hypotheses:
                boost[e["from"]] = max(boost.get(e["from"], 0.0), p[e["to"]] * e["w"])
        participant_pass(boost)
        stages.append(dict(p))
        hypothesis_pass()

    return p, out, stages

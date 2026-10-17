"""One investigation: session state, the full recompute, actions and insights.

State is small (which sources are connected, merge decisions, judgements, a
crown). Everything else is recomputed from scratch on every action, so a
disconnect or a reverted merge is never a special case.
"""
import threading
import time
import urllib.request
from datetime import datetime, timezone

from . import bayes, pipeline, resolve, signals as sigmod

STEP_AT = (0.33, 0.66)
CROWNABLE = 0.85
CAPTURE_P = 0.08
CAPTURE_INHIBIT = 0.5
JUDGEMENTS = {"trust": 1.0, "discount": 0.5, "dismiss": 0.0}

INSIGHTS = {
    "shape_shock": (50, "Shape Shock", "Raw sources agree on nothing: delimiters, timestamps, naming, nesting. Fusion starts by mapping each one onto a single ontology."),
    "first_link": (100, "First Link", "A signal attached to an object because one of its records resolved into that object. Links come from resolution, not hand-wiring."),
    "identity_resolved": (150, "Identity Resolved: {label}", "Another source shared a key with this object, so their fragments merged automatically. It now has a name."),
    "merge_committed": (150, "Merge Committed", "A fuzzy match is only a proposal. You decided these are the same thing, and you can still revert it."),
    "fusion_signal": (150, "Fusion-Only Signal: {label}", "This rule needs facts from different sources on one object. It could not fire until resolution joined them."),
    "inhibited": (100, "Inhibited", "Evidence can argue against a hypothesis: {source} cut {target} by {pct}%."),
    "explained_away": (200, "Explained Away: {label}", "Inhibitors and competing hypotheses pushed this explanation off the board."),
    "crowned": (500, "Crowned: {label}", "The posterior backs your call at {p}%."),
    "crowned_premature": (0, "Called Early: {label}", "Logged, not scored: the posterior is only {p}%. Connect more sources or revisit your judgements."),
    "crowned_contradicted": (0, "Overruled: {label}", "Logged, not scored: the board favors {best} at {best_p}%."),
    "full_fusion": (250, "Full Fusion", "Every source connected: many shapes, one view."),
}


def _now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def step_for(p):
    return sum(p >= t for t in STEP_AT)


class Session:
    def __init__(self, pack, base_url):
        self.pack = pack
        self.base_url = base_url
        self.lock = threading.Lock()
        self._parsed = {}
        self.reset()

    def reset(self):
        self.connected = {}      # source id -> {"raw", "latency_ms", "bytes", "at"}
        self.errors = {}         # source id -> last fetch error
        self.committed = []      # proposal ids "fragA|fragB"
        self.rejected = set()
        self.judgements = {}     # signal id -> trust | discount | dismiss
        self.crown = None
        self.feedback = False
        self.earned = {}         # insight key -> points
        self.audit = []
        self.peeked = set()
        self.state = self.compute()

    # ------------------------------------------------------------ fetching

    def source_url(self, sid):
        s = self.pack["sources"][sid]
        if s["transport"] == "http":
            return s["url"]
        return f"{self.base_url}/mock/{self.pack['id']}/{sid}"

    def fetch(self, sid):
        url = self.source_url(sid)
        t0 = time.time()
        with urllib.request.urlopen(url, timeout=15) as resp:
            body = resp.read()
        return body.decode("utf-8"), int((time.time() - t0) * 1000), len(body)

    def _records(self, sid):
        raw = self.connected[sid]["raw"]
        cached = self._parsed.get(sid)
        if cached and cached[0] == raw:
            return cached[1]
        s = self.pack["sources"][sid]
        recs = pipeline.parse(sid, s["format"], raw, s.get("options"))
        self._parsed[sid] = (raw, recs)
        return recs

    # ------------------------------------------------------------ the recompute

    def compute(self):
        pack = self.pack
        order = [s for s in pack["source_order"] if s in self.connected]
        records, record_index, fragments = {}, {}, []
        source_stats = {}
        for sid in order:
            recs = self._records(sid)
            records[sid] = recs
            for r in recs:
                record_index[(sid, r["index"])] = r
            frags = pipeline.to_fragments(recs, [(mi, m) for mi, m in pack["mappings"] if m["source"] == sid])
            fragments.extend(frags)
            source_stats[sid] = {"records": len(recs), "fragments": len(frags)}

        by_frag = {f["id"]: f for f in fragments}
        frags_by_record = {}
        for f in fragments:
            frags_by_record.setdefault(f["record"], []).append(f["id"])

        committed_pairs = [tuple(pid.split("|")) for pid in self.committed]
        clusters, cluster_of = resolve.resolve(fragments, pack["object_types"], committed_pairs, pack["source_order"])
        pending = resolve.proposals(clusters, by_frag, pack["object_types"], self.rejected)

        # entity slots -> clusters
        ent_cluster = {e["id"]: resolve.anchor(e, clusters, by_frag) for e in pack["entities"]}
        frag_entity = {}
        for eid, c in ent_cluster.items():
            if c:
                for fid in c["fragments"]:
                    frag_entity.setdefault(fid, set()).add(eid)

        ctx = {
            "records": records,
            "record_index": record_index,
            "frags_by_record": frags_by_record,
            "fragments": by_frag,
            "clusters": clusters,
        }

        # signals
        acts, sig_detail = {}, {}
        ent_parents = {e["id"]: [] for e in pack["entities"]}
        ent_types = {e["id"]: e["type"] for e in pack["entities"]}
        for sig in pack["signals"]:
            req = sigmod.required_sources(sig)
            available = all(s in self.connected for s in req)
            hits = sigmod.evaluate(sig, ctx) if available else []
            conf = min(pack["sources"][s]["confidence"] for s in req)
            strength = sigmod.strength(sig, hits) if hits else 0.0
            judgement = self.judgements.get(sig["id"], "trust")
            a = conf * strength * JUDGEMENTS[judgement] if hits else 0.0
            acts[sig["id"]] = a

            imp = sig["implicates"]
            allowed = set(imp.get("types") or ent_types.values())
            linked = {}
            hit_out = []
            for h in hits:
                ents = set()
                for fid in h["fragments"]:
                    ents |= frag_entity.get(fid, set())
                ents = {e for e in ents if ent_types[e] in allowed}
                for e in ents:
                    linked[e] = linked.get(e, 0) + 1
                hit_out.append({
                    "records": [{"source": r["source"], "pointer": r["pointer"], "excerpt": r["excerpt"]} for r in h["records"]],
                    "entities": sorted(ents),
                })
            most = max(linked.values()) if linked else 0
            links = []
            for e, n in sorted(linked.items()):
                w = imp["w"] * n / most
                ent_parents[e].append({"from": sig["id"], "w": w})
                links.append({"entity": e, "hits": n, "w": w})

            sig_detail[sig["id"]] = {
                "status": "dormant" if not available else ("firing" if hits else "quiet"),
                "sources": req,
                "kind": sig["kind"],
                "rule": {k: sig[k] for k in ("source", "where", "params", "strength") if k in sig},
                "explain": sig.get("explain", ""),
                "confidence": conf,
                "strength": strength,
                "judgement": judgement,
                "hits": hit_out,
                "links": links,
                "implicates": imp,
            }

        entities_in = {
            e["id"]: {"leak": e["leak"], "present": ent_cluster[e["id"]] is not None, "parents": ent_parents[e["id"]]}
            for e in pack["entities"]
        }
        hyps_in = {h["id"]: {"prior": h["prior"]} for h in pack["hypotheses"]}
        p, breakdown, stages = bayes.infer(acts, entities_in, hyps_in, pack["edges"], self.feedback)

        # nodes
        layout = {n["id"]: n for n in pack["layout"]["nodes"]}
        nodes = []
        for sig in pack["signals"]:
            d = sig_detail[sig["id"]]
            nodes.append({**layout[sig["id"]], "p": p[sig["id"]], "step": step_for(p[sig["id"]]),
                          "present": True, "status": d["status"], "detail": d})

        for e in pack["entities"]:
            c = ent_cluster[e["id"]]
            detail = {"type": e["type"], "anchor": e["anchor"], "breakdown": breakdown.get(e["id"])}
            if c:
                for vals in c["props"].values():
                    for v in vals:
                        for prov in v["provenance"]:
                            prov["excerpt"] = record_index[by_frag[prov["fragment"]]["record"]]["excerpt"]
                detail.update({
                    "title": c["title"],
                    "props": c["props"],
                    "conflicts": c["conflicts"],
                    "sources": c["sources"],
                    "fragment_count": len(c["fragments"]),
                    "merges": [pid for pid in self.committed if any(f in c["fragments"] for f in pid.split("|"))],
                })
                status = "resolved" if c["title"] else "unresolved"
            else:
                status = "absent"
            label = (c and c["title"]) or e.get("label") if status == "resolved" else next(iter(e["anchor"].values()))
            nodes.append({**layout[e["id"]], "label": label, "p": p[e["id"]], "step": step_for(p[e["id"]]),
                          "present": c is not None, "status": status, "detail": detail})

        best = max(pack["hypotheses"], key=lambda h: p[h["id"]])
        for h in pack["hypotheses"]:
            b = breakdown[h["id"]]
            ph = p[h["id"]]
            captured = ph <= CAPTURE_P and b["inhibit_factor"] <= CAPTURE_INHIBIT
            nodes.append({**layout[h["id"]], "p": ph, "step": step_for(ph), "present": not captured,
                          "status": "captured" if captured else ("crownable" if ph >= CROWNABLE else "open"),
                          "crowned": bool(self.crown and self.crown["hypothesis"] == h["id"]),
                          "leading": h["id"] == best["id"],
                          "detail": {"blurb": h.get("blurb", ""), "prior": h["prior"], "breakdown": b}})

        # edges for drawing
        draw = []
        for sig in pack["signals"]:
            for link in sig_detail[sig["id"]]["links"]:
                draw.append({"from": sig["id"], "to": link["entity"], "kind": "support", "w": link["w"],
                             "flow": link["w"] * acts[sig["id"]]})
        for e in pack["edges"]:
            draw.append({"from": e["from"], "to": e["to"], "kind": e["kind"], "w": e["w"], "flow": e["w"] * p.get(e["from"], 0.0)})

        sources = []
        for sid in pack["source_order"]:
            s = pack["sources"][sid]
            conn = self.connected.get(sid)
            sources.append({
                "id": sid, "name": s["name"], "kind": s.get("kind", ""), "format": s["format"],
                "transport": s["transport"], "color": s["color"], "confidence": s["confidence"],
                "blurb": s.get("blurb", ""), "connected": conn is not None,
                "latency_ms": conn and conn["latency_ms"], "bytes": conn and conn["bytes"],
                "error": self.errors.get(sid),
                **source_stats.get(sid, {"records": 0, "fragments": 0}),
            })

        committed_info = []
        for pid in self.committed:
            a, b = pid.split("|")
            committed_info.append({"id": pid, "active": a in by_frag and b in by_frag,
                                   "a": by_frag.get(a, {}).get("props"), "b": by_frag.get(b, {}).get("props"),
                                   "a_source": a.split("#")[0], "b_source": b.split("#")[0]})

        return {
            "scenario": {"id": pack["id"], "title": pack["title"], "premise": pack["premise"],
                         "board": {"width": pack["layout"]["width"], "height": pack["layout"]["height"]},
                         "narrative": pack["narrative"]},
            "sources": sources,
            "nodes": nodes,
            "edges": draw,
            "stages": stages,
            "merges": {"pending": pending, "committed": committed_info},
            "object_types": pack["object_types"],
            "feedback": self.feedback,
            "crown": self.crown,
            "score": sum(self.earned.values()),
            "earned": sorted(self.earned),
            "audit": self.audit[-80:],
            "thresholds": {"steps": STEP_AT, "crownable": CROWNABLE, "capture_p": CAPTURE_P},
        }

    # ------------------------------------------------------------ actions

    def _log(self, action, detail):
        self.audit.append({"at": _now(), "action": action, "detail": detail})

    def _earn(self, key, kind, events, **fmt):
        if key in self.earned:
            return
        pts, title, text = INSIGHTS[kind]
        override = self.pack["narrative"].get("insights", {}).get(kind, {})
        title = override.get("title", title).format(**fmt)
        text = override.get("text", text).format(**fmt)
        self.earned[key] = pts
        events.append({"kind": kind, "title": title, "text": text, "points": pts})
        self._log("insight", f"{title} (+{pts})")

    def _diff(self, before, after, events):
        bn = {n["id"]: n for n in before["nodes"]}
        names = {n["id"]: f"{n['code']} {n['label']}" for n in after["nodes"]}
        if any(e["flow"] > 0 and e["to"] in bn and bn[e["to"]]["band"] == "entity" and bn[e["from"]]["band"] == "signal"
               for e in after["edges"]):
            self._earn("first_link", "first_link", events)
        for n in after["nodes"]:
            old = bn[n["id"]]
            if n["band"] == "entity" and n["status"] == "resolved" and old["status"] != "resolved":
                self._earn(f"identity_resolved:{n['id']}", "identity_resolved", events, label=names[n["id"]])
            if n["band"] == "signal" and n["status"] == "firing" and old["status"] != "firing" \
                    and n["detail"]["kind"] == "co_resolved":
                self._earn(f"fusion_signal:{n['id']}", "fusion_signal", events, label=names[n["id"]])
            if n["band"] == "hypothesis" and n["status"] == "captured" and old["status"] != "captured":
                self._earn(f"explained_away:{n['id']}", "explained_away", events, label=names[n["id"]])
        for n in after["nodes"]:
            if n["band"] != "hypothesis":
                continue
            for i in n["detail"]["breakdown"]["inhibit"]:
                cut = i["w"] * i["p"]
                if cut >= 0.3:
                    self._earn("inhibited", "inhibited", events, source=names.get(i["from"], i["from"]),
                               target=names[n["id"]], pct=round(cut * 100))
        if len(self.connected) == len(self.pack["sources"]):
            self._earn("full_fusion", "full_fusion", events)

    def act(self, payload):
        with self.lock:
            return self._act(payload)

    def _act(self, payload):
        action = payload.get("action")
        before = self.state
        events = []
        if action == "connect":
            sid = payload["source"]
            try:
                raw, ms, size = self.fetch(sid)
                self.connected[sid] = {"raw": raw, "latency_ms": ms, "bytes": size, "at": _now()}
                self.errors.pop(sid, None)
                self._log("connect", f"{sid}: {size} bytes in {ms} ms from {self.source_url(sid)}")
            except Exception as e:  # surface any fetch failure on the source card
                self.errors[sid] = str(e)
                self._log("connect-failed", f"{sid}: {e}")
        elif action == "disconnect":
            self.connected.pop(payload["source"], None)
            self._log("disconnect", payload["source"])
        elif action == "judge":
            if payload["judgement"] not in JUDGEMENTS:
                raise ValueError("judgement must be trust, discount or dismiss")
            self.judgements[payload["signal"]] = payload["judgement"]
            self._log("judge", f"{payload['signal']} -> {payload['judgement']}")
        elif action == "merge":
            pid = payload["proposal"]
            if pid not in self.committed:
                self.committed.append(pid)
            self.rejected.discard(pid)
            self._log("merge", pid)
            self._earn("merge_committed", "merge_committed", events)
        elif action == "reject":
            self.rejected.add(payload["proposal"])
            self._log("reject-merge", payload["proposal"])
        elif action == "unmerge":
            self.committed = [p for p in self.committed if p != payload["proposal"]]
            self._log("revert-merge", payload["proposal"])
        elif action == "feedback":
            self.feedback = bool(payload["on"])
            self._log("feedback", "on" if self.feedback else "off")
        elif action == "peek":
            self._log("peek", payload["source"])
            self._earn("shape_shock", "shape_shock", events)
        elif action == "crown":
            self._crown(payload["hypothesis"], events)
        elif action == "reset":
            self.reset()
            return {"state": self.state, "events": [{"kind": "reset", "title": "Board reset", "text": "", "points": 0}]}
        else:
            raise ValueError(f"unknown action '{action}'")

        self.state = self.compute()
        self._diff(before, self.state, events)
        self.state["score"] = sum(self.earned.values())
        self.state["earned"] = sorted(self.earned)
        self.state["audit"] = self.audit[-80:]
        return {"state": self.state, "events": events}

    def _crown(self, hid, events):
        nodes = {n["id"]: n for n in self.state["nodes"]}
        hyps = [n for n in self.state["nodes"] if n["band"] == "hypothesis"]
        best = max(hyps, key=lambda n: n["p"])
        n = nodes[hid]
        label = f"{n['code']} {n['label']}"
        b = n["detail"]["breakdown"]
        reasons = sorted(b["support"], key=lambda t: -t["term"])[:3]
        if best["id"] != hid:
            verdict, kind = "contradicted", "crowned_contradicted"
        elif n["p"] < CROWNABLE:
            verdict, kind = "premature", "crowned_premature"
        else:
            verdict, kind = "backed", "crowned"
        self.crown = {"hypothesis": hid, "p": n["p"], "verdict": verdict, "at": _now(),
                      "reasons": [{"from": r["from"], "term": r["term"]} for r in reasons]}
        self._log("crown", f"{label} at {round(n['p'] * 100)}% ({verdict})")
        key = f"crown:{verdict}" if verdict == "backed" else f"crown:{verdict}:{hid}:{len(self.audit)}"
        self._earn(key, kind, events, label=label, p=round(n["p"] * 100),
                   best=f"{best['code']} {best['label']}", best_p=round(best["p"] * 100))

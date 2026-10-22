"""One investigation: session state, the full recompute, actions and insights.

State is small (which sources are connected, merge decisions, judgements, a
crown). Everything else is recomputed from scratch on every action, so a
disconnect or a reverted merge is never a special case.
"""
import threading
import time
import urllib.request
from datetime import datetime, timezone

from . import agent, bayes, jev, pipeline, resolve, signals as sigmod

STEP_AT = (0.33, 0.66)
CROWNABLE = 0.85
CAPTURE_P = 0.08
CAPTURE_INHIBIT = 0.5
JUDGEMENTS = {"trust": 1.0, "discount": 0.5, "dismiss": 0.0}

INSIGHTS = {
    "shape_shock": (50, "Shape Shock", "Raw sources agree on nothing: delimiters, timestamps, naming, nesting. Fusion starts by mapping each one onto a single ontology."),
    "first_link": (100, "First Link", "A signal attached to a participant because one of its records resolved into that participant. Links come from resolution, not hand-wiring."),
    "identity_resolved": (150, "Identity Resolved: {label}", "Another source shared a key with this participant, so their fragments merged automatically. It now has a name."),
    "merge_committed": (150, "Merge Committed", "A fuzzy match is only a proposal. You decided these are the same thing, and you can still revert it."),
    "fusion_signal": (150, "Fusion-Only Signal: {label}", "This rule needs facts from different sources on one participant. It could not fire until resolution joined them."),
    "inhibited": (100, "Inhibited", "Evidence can argue against a hypothesis: {source} cut {target} by {pct}%."),
    "explained_away": (200, "Explained Away: {label}", "Inhibitors and competing hypotheses moved this explanation off the board, into the column on the left. The reason is next to it, and you can bring it back."),
    "crowned": (500, "Crowned: {label}", "The posterior backs your call at {p}%."),
    "crowned_premature": (0, "Called Early: {label}", "Logged, not scored: the posterior is only {p}%. Connect more sources or revisit your judgements."),
    "crowned_contradicted": (0, "Overruled: {label}", "Logged, not scored: the board favors {best} at {best_p}%."),
    "full_fusion": (250, "Full Fusion", "Every source connected: many shapes, one view."),
}


def _now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def step_for(p):
    return sum(p >= t for t in STEP_AT)


def _capture_reason(b, p, names):
    """One sentence saying why a hypothesis was moved to the side of the board."""
    cuts = sorted(b["inhibit"], key=lambda i: -i["w"] * i["p"])
    parts = [f"{names.get(i['from'], i['from'])} cut it by {round(i['w'] * i['p'] * 100)}%"
             for i in cuts if i["w"] * i["p"] >= 0.05][:3]
    text = f"Explained away at {round(p * 100)}%: " + "; ".join(parts) + "."
    if b.get("divisor", 1.0) > 1.0:
        text += f" Competing explanations then divided what was left by {b['divisor']:.2f}."
    return text


class Session:
    def __init__(self, pack, base_url):
        self.pack = pack
        self.base_url = base_url
        self.lock = threading.Lock()
        self.agent_lock = threading.Lock()
        self._parsed = {}
        self._doc_times = {}
        self.reset()

    def reset(self):
        self.connected = {}      # source id -> {"raw", "latency_ms", "bytes", "at"}
        self.errors = {}         # source id -> last fetch error
        self.committed = []      # proposal ids "fragA|fragB"
        self.rejected = set()
        self.judgements = {}     # signal id -> trust | discount | dismiss
        self.crown = None
        self.kept = set()        # hypotheses the user keeps on the board despite being explained away
        self.participant_sources = {} # participant id -> sources it had when last on the board
        self.feedback = False
        self.engine = "rules"    # which evidence engine drives the board: rules | jev
        self.jev = {}            # "signal|source|index" -> Jev result for that record
        self.jev_stats = {"calls": 0, "cached": 0, "errors": 0, "cost": 0.0}
        self.earned = {}         # insight key -> points
        self.audit = []
        self.peeked = set()
        self.acked = set()       # event ids the user acknowledged
        self.agent = agent.Agent(self)
        self.state = self.compute()

    def clone(self):
        """A throwaway copy for what-ifs: same data and decisions, separate state."""
        c = type(self).__new__(type(self))
        c.pack, c.base_url = self.pack, self.base_url
        c.lock, c.agent_lock = threading.Lock(), threading.Lock()
        c._parsed = dict(self._parsed)
        c._doc_times = dict(self._doc_times)
        c.connected, c.errors = dict(self.connected), dict(self.errors)
        c.committed, c.rejected = list(self.committed), set(self.rejected)
        c.judgements, c.crown, c.kept = dict(self.judgements), self.crown, set(self.kept)
        c.participant_sources = dict(self.participant_sources)
        c.feedback, c.engine = self.feedback, self.engine
        c.jev, c.jev_stats = dict(self.jev), dict(self.jev_stats)
        c.earned, c.audit, c.peeked = dict(self.earned), list(self.audit), set(self.peeked)
        c.acked = set(self.acked)
        c.agent = agent.Agent(c)
        c.state = self.state
        return c

    def public_state(self):
        """State plus the parts that change outside a recompute (agent, log)."""
        return {**self.state, "agent": self.agent.view(), "audit": self.audit[-80:],
                "score": sum(self.earned.values()), "earned": sorted(self.earned)}

    def ask_agent(self, message, selected=None):
        with self.agent_lock:
            return self.agent.ask(message, selected)

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

    def _doc_t(self, sid):
        """Epoch ms of a source dated as a whole (`time.document`), memoized per payload."""
        s = self.pack["sources"][sid]
        raw = self.connected[sid]["raw"] if sid in self.connected else None
        if raw is None:
            if s["transport"] != "file":
                return None
            with open(s["path"], encoding="utf-8") as f:
                raw = f.read()
        key = (sid, len(raw), hash(raw))
        if key not in self._doc_times:
            try:
                self._doc_times[key] = int(pipeline.document_time(raw, s["format"], s["time"]).timestamp() * 1000)
            except Exception:  # a document that will not date counts as undated
                self._doc_times[key] = None
        return self._doc_times[key]

    def _connect_t(self, sid):
        c = self.connected.get(sid)
        return int(pipeline.parse_time(c["at"], "iso").timestamp() * 1000) if c else None

    def _record_t(self, sid, r):
        """Epoch ms of a record. None means undated: no time config, an empty
        field, or a value that does not parse. Under unknown_time=at_connect
        undated records are dated at their source's connect time."""
        tconf = self.pack["sources"][sid].get("time")
        t = None
        if tconf and "document" in tconf:
            t = self._doc_t(sid)
        elif tconf:
            try:
                parsed = pipeline.record_time(r["data"], tconf)
                t = None if parsed is None else int(parsed.timestamp() * 1000)
            except (TypeError, ValueError, OverflowError):
                t = None
        if t is None and self.pack["unknown_time"] == "at_connect":
            return self._connect_t(sid)
        return t

    def compute(self, until=None, track=True, undated=True):
        """The whole board from the session's decisions. With `until` (epoch
        ms) only records timestamped at or before it take part; `undated`
        says whether records without a timestamp take part in that slice
        (the pack's unknown_time policy decides when the timeline sets it).
        `track=False` leaves session memory untouched, for time slices that
        must not disturb the live board."""
        pack = self.pack
        order = [s for s in pack["source_order"] if s in self.connected]
        records, record_index, fragments = {}, {}, []
        source_stats = {}
        for sid in order:
            recs = self._records(sid)
            if until is not None:
                recs = [r for r in recs if (lambda t: undated if t is None else t <= until)(self._record_t(sid, r))]
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

        # participant slots -> clusters
        part_cluster = {e["id"]: resolve.anchor(e, clusters, by_frag) for e in pack["participants"]}
        frag_participant = {}
        for eid, c in part_cluster.items():
            if c:
                for fid in c["fragments"]:
                    frag_participant.setdefault(fid, set()).add(eid)

        ctx = {
            "records": records,
            "record_index": record_index,
            "frags_by_record": frags_by_record,
            "fragments": by_frag,
            "clusters": clusters,
        }

        # signals
        acts, sig_detail = {}, {}
        record_signals = {}      # (source, index) -> signal ids with a hit on that record
        part_parents = {e["id"]: [] for e in pack["participants"]}
        part_types = {e["id"]: e["type"] for e in pack["participants"]}
        for sig in pack["signals"]:
            req = sigmod.required_sources(sig)
            available = all(s in self.connected for s in req)
            rule_hits = sigmod.evaluate(sig, ctx) if available else []
            rule_strength = sigmod.strength(sig, rule_hits) if rule_hits else 0.0
            jv = self._jev_view(sig, records, rule_hits) if sig.get("jev") and available else None
            use_jev = self.engine == "jev" and jv is not None and jv["complete"]
            if use_jev:
                hits = [sigmod._record_hit(record_index[(sig["source"], r["index"])], frags_by_record)
                        for r in jv["records"] if r["jev_hit"]]
                strength = jv["strength"]
            else:
                hits, strength = rule_hits, rule_strength
            conf = min(pack["sources"][s]["confidence"] for s in req)
            judgement = self.judgements.get(sig["id"], "trust")
            a = conf * strength * JUDGEMENTS[judgement] if hits else 0.0
            acts[sig["id"]] = a

            imp = sig["implicates"]
            allowed = set(imp.get("types") or part_types.values())
            linked = {}
            hit_out = []
            for h in hits:
                for r in h["records"]:
                    record_signals.setdefault((r["source"], r["index"]), set()).add(sig["id"])
                parts = set()
                for fid in h["fragments"]:
                    parts |= frag_participant.get(fid, set())
                parts = {e for e in parts if part_types[e] in allowed}
                for e in parts:
                    linked[e] = linked.get(e, 0) + 1
                hit_out.append({
                    "records": [{"source": r["source"], "pointer": r["pointer"], "excerpt": r["excerpt"]} for r in h["records"]],
                    "participants": sorted(parts),
                })
            most = max(linked.values()) if linked else 0
            links = []
            for e, n in sorted(linked.items()):
                w = imp["w"] * n / most
                part_parents[e].append({"from": sig["id"], "w": w})
                links.append({"participant": e, "hits": n, "w": w})

            sig_detail[sig["id"]] = {
                "status": "dormant" if not available else ("firing" if hits else "quiet"),
                "sources": req,
                "kind": sig["kind"],
                "rule": {k: sig[k] for k in ("source", "where", "params", "strength") if k in sig},
                "explain": sig.get("explain", ""),
                "confidence": conf,
                "strength": strength,
                "engine_used": "jev" if use_jev else "rules",
                "rule_strength": rule_strength,
                "rule_hit_count": len(rule_hits),
                "jev": jv,
                "judgement": judgement,
                "hits": hit_out,
                "links": links,
                "implicates": imp,
            }

        participants_in = {
            e["id"]: {"leak": e["leak"], "present": part_cluster[e["id"]] is not None, "parents": part_parents[e["id"]]}
            for e in pack["participants"]
        }
        hyps_in = {h["id"]: {"prior": h["prior"]} for h in pack["hypotheses"]}
        p, breakdown, stages = bayes.infer(acts, participants_in, hyps_in, pack["edges"], self.feedback)

        # nodes
        layout = {n["id"]: n for n in pack["layout"]["nodes"]}
        nodes = []
        for sig in pack["signals"]:
            d = sig_detail[sig["id"]]
            nodes.append({**layout[sig["id"]], "p": p[sig["id"]], "step": step_for(p[sig["id"]]),
                          "present": True, "status": d["status"], "detail": d})

        for e in pack["participants"]:
            c = part_cluster[e["id"]]
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
                if track:
                    self.participant_sources[e["id"]] = list(c["sources"])
            else:
                status = "absent"
            offboard = None
            if not c and e["id"] in self.participant_sources:
                srcs = self.participant_sources[e["id"]]
                names = ", ".join(pack["sources"][s]["name"] for s in srcs)
                status = "removed"
                offboard = {"kind": "removed", "restore": "reconnect", "sources": srcs,
                            "reason": f"No connected source describes it any more. It came from: {names}."}
            label = (c and c["title"]) or e.get("label") if status == "resolved" else next(iter(e["anchor"].values()))
            nodes.append({**layout[e["id"]], "label": label, "p": p[e["id"]], "step": step_for(p[e["id"]]),
                          "present": c is not None, "status": status, "offboard": offboard, "detail": detail})

        names = {n["id"]: f"{n['code']} {n['label']}" for n in pack["layout"]["nodes"]}
        best = max(pack["hypotheses"], key=lambda h: p[h["id"]])
        for h in pack["hypotheses"]:
            b = breakdown[h["id"]]
            ph = p[h["id"]]
            captured = ph <= CAPTURE_P and b["inhibit_factor"] <= CAPTURE_INHIBIT
            kept = captured and h["id"] in self.kept
            reason = _capture_reason(b, ph, names) if captured else None
            if captured and not kept:
                status = "captured"
            elif kept:
                status = "kept"
            else:
                status = "crownable" if ph >= CROWNABLE else "open"
            nodes.append({**layout[h["id"]], "p": ph, "step": step_for(ph), "present": not captured or kept,
                          "status": status, "kept": kept, "kept_reason": reason if kept else None,
                          "offboard": {"kind": "captured", "restore": "keep", "reason": reason} if captured and not kept else None,
                          "crowned": bool(self.crown and self.crown["hypothesis"] == h["id"]),
                          "leading": h["id"] == best["id"],
                          "detail": {"blurb": h.get("blurb", ""), "prior": h["prior"], "breakdown": b}})

        # edges for drawing
        draw = []
        for sig in pack["signals"]:
            for link in sig_detail[sig["id"]]["links"]:
                draw.append({"from": sig["id"], "to": link["participant"], "kind": "support", "w": link["w"],
                             "flow": link["w"] * acts[sig["id"]]})
        for e in pack["edges"]:
            draw.append({"from": e["from"], "to": e["to"], "kind": e["kind"], "w": e["w"], "flow": e["w"] * p.get(e["from"], 0.0)})

        events, undated, links = self._timeline(order, records, frags_by_record, frag_participant, record_signals, acts)

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
            "events": events,
            "undated": undated,
            "unknown_time": pack["unknown_time"],
            "links": links,
            "object_types": pack["object_types"],
            "feedback": self.feedback,
            "engine": self.engine,
            "jev": {"available": jev.available(), "model": jev.MODEL, **self.jev_stats},
            "crown": self.crown,
            "score": sum(self.earned.values()),
            "earned": sorted(self.earned),
            "audit": self.audit[-80:],
            "thresholds": {"steps": STEP_AT, "crownable": CROWNABLE, "capture_p": CAPTURE_P},
        }

    def timeline(self):
        """The board re-inferred at every distinct event time: what each
        signal, participant and hypothesis looked like once the records up
        to that moment had arrived. Untimed records are present throughout."""
        events = self.state.get("events", [])
        undated = [u["id"] for u in self.state.get("undated", [])]
        policy = self.pack["unknown_time"]
        times = sorted({e["t"] for e in events})
        slices = []
        seen = 0
        for i, t in enumerate([None] + times):
            st = self.compute(until=(-1 if t is None else t), track=False, undated=(policy != "after"))
            arrived = [e["id"] for e in events if e["t"] == t] if t is not None else []
            seen += len(arrived)
            slices.append({**self._slice(st), "t": t, "at": next((e["at"] for e in events if e["t"] == t), None),
                           "index": i, "events_seen": seen, "arrived": arrived,
                           "undated": undated if (i == 0 and policy == "throughout") else []})
        if policy == "after" and undated:
            last = times[-1] if times else -1
            st = self.compute(until=last, track=False, undated=True)
            slices.append({**self._slice(st), "t": last if times else None, "at": slices[-1]["at"], "index": len(slices),
                           "events_seen": seen, "arrived": [], "undated": undated, "undated_arrive": True})
        return {"slices": slices, "final": self._slice(self.state), "event_count": len(events),
                "undated_count": len(undated), "unknown_time": policy}

    @staticmethod
    def _slice(st):
        """A compact view of one board: activations, involvements, and each
        hypothesis's derivation terms."""
        out = {"signals": {}, "participants": {}, "hypotheses": {}}
        for n in st["nodes"]:
            if n["band"] == "signal":
                out["signals"][n["id"]] = {"p": round(n["p"], 4), "status": n["status"]}
            elif n["band"] == "participant":
                out["participants"][n["id"]] = {"p": round(n["p"], 4), "status": n["status"], "present": n["present"]}
            else:
                b = n["detail"]["breakdown"]
                out["hypotheses"][n["id"]] = {
                    "p": round(n["p"], 4), "status": n["status"], "present": n["present"],
                    "support": [{"from": x["from"], "w": round(x["w"], 4), "p": round(x["p"], 4), "term": round(x["term"], 4)} for x in b["support"] if x["term"] > 0.001],
                    "inhibit": [{"from": x["from"], "w": round(x["w"], 4), "p": round(x["p"], 4), "cut": round(x["w"] * x["p"], 4)} for x in b["inhibit"] if x["w"] * x["p"] > 0.001],
                    "raw": round(b["raw"], 4), "divisor": round(b.get("divisor", 1.0), 4),
                }
        return out

    def _timeline(self, order, records, frags_by_record, frag_participant, record_signals, acts):
        """Timestamped records as events, undated records listed apart (or
        dated at connect time under that policy), and participant pairs
        joined by a shared record."""
        events, undated, pairs = [], [], {}
        for sid in order:
            tconf = self.pack["sources"][sid].get("time")
            for r in records[sid]:
                key = (sid, r["index"])
                parts = set()
                for fid in frags_by_record.get(key, []):
                    parts |= frag_participant.get(fid, set())
                parts = sorted(parts)
                for i, a in enumerate(parts):
                    for b in parts[i + 1:]:
                        pairs.setdefault((a, b), []).append({"source": sid, "pointer": r["pointer"]})
                sigs = sorted(record_signals.get(key, ()))
                eid = f"{sid}:{r['index']}"
                base = {
                    "id": eid, "source": sid, "pointer": r["pointer"], "excerpt": r["excerpt"],
                    "participants": parts, "signals": sigs,
                    "severity": max((acts[g] for g in sigs), default=0.0),
                    "acked": eid in self.acked,
                }
                t_ms = self._record_t(sid, r)
                if t_ms is None:
                    undated.append({**base, "why": "no time config" if not tconf else "no usable time value"})
                    continue
                dated_by = "connect" if (self.pack["unknown_time"] == "at_connect" and t_ms == self._connect_t(sid)
                                         and (not tconf or self._field_t(sid, r) is None)) else ("document" if tconf and "document" in tconf else "record")
                events.append({**base, "at": pipeline._iso(datetime.fromtimestamp(t_ms / 1000, timezone.utc)), "t": t_ms,
                               "dated_by": dated_by})
        events.sort(key=lambda e: e["t"])  # stable: ties keep source order, then record order
        links = [{"a": a, "b": b, "records": recs, "weight": len(recs)} for (a, b), recs in pairs.items()]
        links.sort(key=lambda l: (-l["weight"], l["a"], l["b"]))
        return events, undated, links

    def _field_t(self, sid, r):
        """The record's own parsed time (ignoring policy), or None."""
        tconf = self.pack["sources"][sid].get("time")
        if not tconf or "document" in tconf:
            return None
        try:
            parsed = pipeline.record_time(r["data"], tconf)
        except (TypeError, ValueError, OverflowError):
            return None
        return None if parsed is None else int(parsed.timestamp() * 1000)

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
        if any(e["flow"] > 0 and e["to"] in bn and bn[e["to"]]["band"] == "participant" and bn[e["from"]]["band"] == "signal"
               for e in after["edges"]):
            self._earn("first_link", "first_link", events)
        for n in after["nodes"]:
            old = bn[n["id"]]
            if n["band"] == "participant" and n["status"] == "resolved" and old["status"] != "resolved":
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
        if action == "agent_reset":
            self.agent = agent.Agent(self)
            return {"state": self.public_state(), "events": []}
        if action == "agent_decide":
            prop = self.agent.decide(payload["proposal"], payload["decision"])
            self._log("agent", f"you {prop['status']} the agent's proposal to {prop['label']}")
            if prop["status"] != "approved":
                return {"state": self.public_state(), "events": []}
            payload = prop["payload"]
            action = payload["action"]
        if action == "connect":
            self._connect(payload["source"])
        elif action == "engine":
            if payload["engine"] not in ("rules", "jev"):
                raise ValueError("engine must be rules or jev")
            self.engine = payload["engine"]
            self._log("engine", f"evidence engine -> {self.engine}")
            if self.engine == "jev":
                for sid in list(self.connected):
                    self._ask_jev(sid)
        elif action == "reconnect":
            for sid in payload["sources"]:
                if sid not in self.connected:
                    self._connect(sid)
        elif action == "keep":
            self.kept.add(payload["hypothesis"])
            self._log("keep-on-board", f"{payload['hypothesis']}: kept on the board despite the evidence")
        elif action == "release":
            self.kept.discard(payload["hypothesis"])
            self._log("release", f"{payload['hypothesis']}: the board may move it aside again")
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
        elif action in ("ack", "unack"):
            eid = payload.get("event")
            ev = next((e for e in self.state["events"] + self.state.get("undated", []) if e["id"] == eid), None)
            if ev is None:
                raise ValueError(f"unknown event '{eid}'")
            if action == "ack":
                self.acked.add(eid)
            else:
                self.acked.discard(eid)
            self._log(action, f"{eid} ({ev['source']} {ev['pointer']})")
        elif action == "crown":
            self._crown(payload["hypothesis"], events)
        elif action == "reset":
            self.reset()
            return {"state": self.public_state(), "events": [{"kind": "reset", "title": "Board reset", "text": "", "points": 0}]}
        else:
            raise ValueError(f"unknown action '{action}'")

        self.state = self.compute()
        self._diff(before, self.state, events)
        return {"state": self.public_state(), "events": events}

    def _connect(self, sid):
        try:
            raw, ms, size = self.fetch(sid)
            self.connected[sid] = {"raw": raw, "latency_ms": ms, "bytes": size, "at": _now()}
            self.errors.pop(sid, None)
            self._log("connect", f"{sid}: {size} bytes in {ms} ms from {self.source_url(sid)}")
            self._ask_jev(sid)
        except Exception as e:  # surface any fetch failure on the source card
            self.errors[sid] = str(e)
            self._log("connect-failed", f"{sid}: {e}")

    # ------------------------------------------------------------ Jev

    def _jev_signals(self, sid):
        return [s for s in self.pack["signals"] if s.get("jev") and s.get("source") == sid]

    def _records_any(self, sid):
        """Records of a source whether or not it is connected (file sources
        are read from disk), so the cache inspector can cover the whole pack."""
        if sid in self.connected:
            return self._records(sid)
        s = self.pack["sources"][sid]
        if s["transport"] != "file":
            return []
        cached = self._parsed.get(sid)
        if cached:
            return cached[1]
        with open(s["path"], encoding="utf-8") as f:
            return pipeline.parse(sid, s["format"], f.read(), s.get("options"))

    def _jev_jobs(self, sid, records, skip_known=True):
        """What we ask Jev about one source: one request per record carrying
        every opted-in question whose `where` matches. Returns
        [(record, state, questions, [signal ids])]."""
        name = self.pack["sources"][sid]["name"]
        jobs = []
        for r in records:
            qs, ids = {}, []
            for sig in self._jev_signals(sid):
                spec = sig["jev"]
                if skip_known and f"{sig['id']}|{sid}|{r['index']}" in self.jev:
                    continue
                if not pipeline.matches(r["data"], spec.get("where")):
                    continue
                q = {"type": "noul", "instructions": spec["question"]}
                if spec.get("criteria"):
                    q["criteria"] = spec["criteria"]
                qs[sig["id"].replace("-", "_")] = q
                ids.append(sig["id"])
            if qs:
                jobs.append((r, {"source": name, "record": r["data"]}, qs, ids))
        return jobs

    def _ask_jev(self, sid):
        """Ask Jev every opted-in signal's question about each candidate record
        of one source: one request per record, all of its questions together."""
        sigs = self._jev_signals(sid)
        if not sigs or not jev.available():
            return
        jobs = [(r["index"], state, qs) for r, state, qs, _ in self._jev_jobs(sid, self._records(sid))]
        if not jobs:
            return
        results = jev.ask_many(jobs)
        stats = {"calls": 0, "cached": 0, "errors": 0, "cost": 0.0}
        latencies = []
        for idx, res in results.items():
            if "error" in res:
                stats["errors"] += 1
            elif res["cached"]:
                stats["cached"] += 1
            else:
                stats["calls"] += 1
                stats["cost"] += res["cost"] or 0.0
                latencies.append(res["ms"])
            for sig in sigs:
                key = sig["id"].replace("-", "_")
                answer = (res.get("answers") or {}).get(key)
                if answer is None and "error" not in res:
                    continue
                self.jev[f"{sig['id']}|{sid}|{idx}"] = {
                    "noul": answer.get("noul") if answer else None,
                    "model": res.get("model"),
                    "ms": res.get("ms"),
                    "cached": res.get("cached", False),
                    "error": res.get("error"),
                }
        for k, v in stats.items():
            self.jev_stats[k] += v
        avg = f", avg {sum(latencies) // len(latencies)} ms" if latencies else ""
        self._log("jev", f"{sid}: {len(jobs)} records, {stats['calls']} calls, {stats['cached']} cached, "
                         f"{stats['errors']} errors, ${stats['cost']:.5f}{avg}")

    def jev_inventory(self):
        """Every Jev ask this pack can make, with its cache status: what the
        question is, which records it covers, what Jev answered, when, and by
        which model version. Covers unconnected sources too."""
        codes = {n["id"]: n["code"] for n in self.pack["layout"]["nodes"]}
        signals = []
        paths = set()
        versions = set()
        for sig in self.pack["signals"]:
            if not sig.get("jev"):
                continue
            sid = sig["source"]
            spec = sig["jev"]
            key = sig["id"].replace("-", "_")
            records = []
            for r, state, qs, ids in self._jev_jobs(sid, self._records_any(sid), skip_known=False):
                if sig["id"] not in ids:
                    continue
                path = jev.cache_path(state, qs)
                paths.add(path)
                c = jev.load_cached(path)
                entry = {"pointer": r["pointer"], "excerpt": r["excerpt"][:200], "path": path,
                         "asked_with": [codes.get(i, i) for i in ids if i != sig["id"]], "cached": None}
                if c is not None:
                    ans = (c.get("answers") or {}).get(key) or {}
                    entry["cached"] = {"noul": ans.get("noul"), "model": c.get("model"), "at": c.get("at"),
                                       "ms": c.get("ms"), "legacy": "payload" not in c}
                    if c.get("model"):
                        versions.add(c["model"])
                records.append(entry)
            n_cached = sum(1 for r in records if r["cached"])
            signals.append({
                "id": sig["id"], "code": codes.get(sig["id"], sig["id"]), "label": sig.get("label", sig["id"]),
                "source": sid, "source_name": self.pack["sources"][sid]["name"],
                "connected": sid in self.connected,
                "question": spec["question"], "criteria": spec.get("criteria"), "where": spec.get("where"),
                "threshold": spec.get("threshold", 0.5),
                "records": records, "cached": n_cached, "live": len(records) - n_cached,
            })
        newest = max(versions) if versions else None
        for s in signals:
            for r in s["records"]:
                if r["cached"]:
                    r["cached"]["older_version"] = bool(newest and r["cached"]["model"] and r["cached"]["model"] < newest)
        return {"model": jev.MODEL, "newest_seen": newest, "signals": signals, "paths": sorted(paths)}

    def forget_jev(self, signal_id=None, reask=False):
        """Drop cached answers (for one signal's asks, or all of this pack's),
        and optionally ask again right away for connected sources."""
        inv = self.jev_inventory()
        targets = [s for s in inv["signals"] if signal_id in (None, s["id"])]
        if signal_id and not targets:
            raise ValueError(f"no Jev signal {signal_id}")
        paths = {r["path"] for s in targets for r in s["records"]}
        removed = jev.forget(paths)
        # a cache file holds every question asked about that record, so drop
        # the in-memory answers for all signals that share those asks
        for s in inv["signals"]:
            for r in s["records"]:
                if r["path"] in paths:
                    for sid_key in [k for k in self.jev if k.endswith(f"|{s['source']}|{self._index_of(s['source'], r['pointer'])}")]:
                        self.jev.pop(sid_key, None)
        what = f"{targets[0]['code']} {targets[0]['label']}" if signal_id else "every Jev ask in this pack"
        self._log("jev-cache", f"forgot {removed} cached answer(s) for {what}")
        if reask:
            for sid in sorted({s["source"] for s in targets if s["connected"]}):
                self._ask_jev(sid)
        self.state = self.compute()
        return removed

    def _index_of(self, sid, pointer):
        for r in self._records_any(sid):
            if r["pointer"] == pointer:
                return r["index"]
        return None

    def _jev_view(self, sig, records, rule_hits):
        """What Jev said about each candidate record, next to what the rule said."""
        spec = sig["jev"]
        sid = sig["source"]
        threshold = spec.get("threshold", 0.5)
        rule_keys = {(r["source"], r["index"]) for h in rule_hits for r in h["records"]}
        out = []
        for r in records.get(sid, []):
            if not pipeline.matches(r["data"], spec.get("where")):
                continue
            res = self.jev.get(f"{sig['id']}|{sid}|{r['index']}") or {}
            noul = res.get("noul")
            out.append({
                "index": r["index"], "source": sid, "pointer": r["pointer"], "excerpt": r["excerpt"],
                "noul": noul, "error": res.get("error"), "model": res.get("model"),
                "rule_hit": (sid, r["index"]) in rule_keys,
                "jev_hit": noul is not None and noul >= threshold,
            })
        scored = [r for r in out if r["noul"] is not None]
        hits = [r for r in scored if r["jev_hit"]]
        return {
            "question": spec["question"],
            "criteria": spec.get("criteria"),
            "where": spec.get("where"),
            "threshold": threshold,
            "records": out,
            "complete": bool(out) and len(scored) == len(out),
            "asked": bool(scored),
            "strength": max((r["noul"] for r in hits), default=0.0),
            "hit_count": len(hits),
            "disagreements": sum(1 for r in scored if r["rule_hit"] != r["jev_hit"]),
            "model": next((r["model"] for r in scored if r["model"]), None),
        }

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

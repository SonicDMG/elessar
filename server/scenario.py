"""Scenario packs: load, validate, lay out.

A pack is a directory of data files. This module is the only place that knows
the pack format; the engine downstream only sees the validated structure.
"""
import json
import os

from . import pipeline, signals as sigmod

FILES = ("manifest.json", "mapping.json", "signals.json", "model.json", "narrative.json")
BAND_ROWS = {"hypothesis": 2, "participant": 5, "signal": 8}
BOARD_HEIGHT = 9
MAX_BAND = 9
PALETTE = ["#7ee787", "#f0883e", "#58a6ff", "#d2a8ff", "#f778ba", "#e3b341", "#56d4dd", "#ff7b72"]


# How records without a timestamp take part in the time-lapse:
#   throughout  reference data, present in every slice (default)
#   at_connect  dated at the moment their source was connected
#   after       arrive in one final slice after every timed record
UNKNOWN_TIME = ("throughout", "at_connect", "after")


class PackError(Exception):
    def __init__(self, pack_dir, errors):
        super().__init__(f"{pack_dir}: {len(errors)} error(s)")
        self.pack_dir = pack_dir
        self.errors = errors


def _read_json(path, errors):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except FileNotFoundError:
        errors.append(f"missing {os.path.basename(path)}")
    except json.JSONDecodeError as e:
        errors.append(f"{os.path.basename(path)}: invalid JSON at line {e.lineno}: {e.msg}")
    return None


def _check_where(where, ctx, errors):
    for field, cond in (where or {}).items():
        if isinstance(cond, dict):
            bad = set(cond) - pipeline.CONDITION_OPS
            if bad:
                errors.append(f"{ctx}: unknown operator(s) {sorted(bad)} on '{field}'")


def _spread(n, m):
    return [int((i + 0.5) * m / n) for i in range(n)]


def layout(signals, participants, hypotheses):
    widest = max(len(signals), len(participants), len(hypotheses))
    width = 2 * widest + 1
    odd = list(range(1, width - 1, 2))
    even = list(range(0, width - 2, 2))
    nodes = []
    for band, items, prefix, cols in (
        ("hypothesis", hypotheses, "H", odd),
        ("participant", participants, "P", even),
        ("signal", signals, "S", odd),
    ):
        for i, (item, slot) in enumerate(zip(items, _spread(len(items), len(cols)))):
            nodes.append({
                "id": item["id"],
                "code": f"{prefix}{i + 1}",
                "band": band,
                "label": item.get("label") or item["id"],
                "col": cols[slot],
                "base_row": BAND_ROWS[band],
            })
    return {"width": width, "height": BOARD_HEIGHT, "nodes": nodes}


def load_pack(pack_dir):
    errors, warnings = [], []
    docs = {name: _read_json(os.path.join(pack_dir, name), errors) for name in FILES}
    if errors:
        raise PackError(pack_dir, errors)
    manifest, mapping, sigs, model, narrative = (docs[n] for n in FILES)

    # --- manifest
    pid = manifest.get("id") or os.path.basename(pack_dir.rstrip("/"))
    sources = {}
    for i, s in enumerate(manifest.get("sources", [])):
        sid = s.get("id")
        if not sid:
            errors.append(f"manifest: source #{i + 1} has no id")
            continue
        if sid in sources:
            errors.append(f"manifest: duplicate source id '{sid}'")
        if s.get("format") not in pipeline.PARSERS:
            errors.append(f"manifest: source '{sid}' has unknown format '{s.get('format')}' (known: {sorted(pipeline.PARSERS)})")
        transport = s.setdefault("transport", "file")
        if transport == "file":
            path = os.path.join(pack_dir, s.get("file", ""))
            if not s.get("file") or not os.path.isfile(path):
                errors.append(f"manifest: source '{sid}' file not found: {s.get('file')}")
            s["path"] = path
        elif transport == "http":
            if not str(s.get("url", "")).startswith(("http://", "https://")):
                errors.append(f"manifest: source '{sid}' needs an http(s) url")
        else:
            errors.append(f"manifest: source '{sid}' has unknown transport '{transport}'")
        conf = s.setdefault("confidence", 0.9)
        if not 0 <= conf <= 1:
            errors.append(f"manifest: source '{sid}' confidence must be 0..1")
        tconf = s.get("time")
        if tconf is not None:
            if not isinstance(tconf, dict) or ("field" in tconf) == ("document" in tconf):
                errors.append(f"manifest: source '{sid}' time needs exactly one of 'field' (per record) or 'document' (whole source)")
            else:
                if "field" in tconf:
                    fields = tconf["field"]
                    if isinstance(fields, str):
                        fields = [fields]
                    if not fields or not all(isinstance(f, str) and f for f in fields):
                        errors.append(f"manifest: source '{sid}' time.field must be a path or a list of paths")
                else:
                    doc = tconf["document"]
                    if not isinstance(doc, dict) or len([k for k in pipeline.DOCUMENT_TIME_KEYS if k in doc]) != 1:
                        errors.append(f"manifest: source '{sid}' time.document needs exactly one of {list(pipeline.DOCUMENT_TIME_KEYS)}")
                    elif "path" in doc and s.get("format") not in ("nested-json", "columnar", "xml"):
                        errors.append(f"manifest: source '{sid}' time.document.path works for nested-json, columnar and xml; use regex or constant for {s.get('format')}")
                if tconf.get("unit") not in pipeline.TIME_UNITS:
                    errors.append(f"manifest: source '{sid}' has unknown time.unit '{tconf.get('unit')}' (known: {list(pipeline.TIME_UNITS)})")
        s.setdefault("color", PALETTE[i % len(PALETTE)])
        s.setdefault("latency_ms", 150)
        s.setdefault("name", sid)
        sources[sid] = s
    if not sources:
        errors.append("manifest: no sources")
    unknown_time = manifest.get("unknown_time", "throughout")
    if unknown_time not in UNKNOWN_TIME:
        errors.append(f"manifest: unknown_time must be one of {UNKNOWN_TIME} (got '{unknown_time}')")

    # --- mapping
    types = mapping.get("object_types", {})
    for tname, t in types.items():
        if "title" not in t:
            errors.append(f"mapping: object type '{tname}' needs a title property")
        if t.get("fuzzy") and not t["fuzzy"].get("properties"):
            errors.append(f"mapping: object type '{tname}' fuzzy config needs properties")
    mapped_props = {t: set() for t in types}
    maps = mapping.get("mappings", [])
    for mi, m in enumerate(maps):
        ctx = f"mapping #{mi + 1} ({m.get('source')} -> {m.get('type')})"
        if m.get("source") not in sources:
            errors.append(f"{ctx}: unknown source")
        if m.get("type") not in types:
            errors.append(f"{ctx}: unknown object type")
        if not m.get("props"):
            errors.append(f"{ctx}: no props")
        _check_where(m.get("when"), ctx, errors)
        for prop, spec in (m.get("props") or {}).items():
            _, tfs = pipeline._prop_spec(spec)
            for tf in tfs:
                if tf not in pipeline.TRANSFORMS:
                    errors.append(f"{ctx}: unknown transform '{tf}' on '{prop}' (known: {sorted(pipeline.TRANSFORMS)})")
            if m.get("type") in mapped_props:
                mapped_props[m["type"]].add(prop)
    for tname, t in types.items():
        for key in t.get("keys", []):
            if key not in mapped_props[tname]:
                warnings.append(f"mapping: key '{key}' on '{tname}' is never mapped")

    # --- signals
    sig_ids = set()
    for s in sigs:
        sid = s.get("id")
        ctx = f"signal '{sid}'"
        if not sid or sid in sig_ids:
            errors.append(f"signals: missing or duplicate id '{sid}'")
        sig_ids.add(sid)
        if s.get("kind") not in sigmod.KINDS:
            errors.append(f"{ctx}: unknown kind '{s.get('kind')}' (known: {sorted(sigmod.KINDS)})")
            continue
        for src in sigmod.required_sources(s):
            if src not in sources:
                errors.append(f"{ctx}: unknown source '{src}'")
        _check_where(s.get("where"), ctx, errors)
        if s["kind"] == "co_resolved":
            if s.get("params", {}).get("type") not in types:
                errors.append(f"{ctx}: co_resolved needs a known params.type")
            for c in s.get("params", {}).get("clauses", []):
                _check_where(c.get("where"), ctx, errors)
        if s["kind"] == "time_window" and len(s.get("params", {}).get("hours", [])) != 2:
            errors.append(f"{ctx}: time_window needs params.hours [start, end]")
        if "jev" in s:
            spec = s["jev"]
            if s["kind"] not in ("match", "time_window"):
                errors.append(f"{ctx}: a jev block works on match and time_window signals only")
            if not isinstance(spec, dict) or not str(spec.get("question", "")).strip():
                errors.append(f"{ctx}: jev needs a question")
            elif not 0 <= spec.get("threshold", 0.5) <= 1:
                errors.append(f"{ctx}: jev.threshold must be 0..1")
            else:
                _check_where(spec.get("where"), ctx + " jev", errors)
        imp = s.setdefault("implicates", {"w": 0.5})
        for t in imp.get("types", []):
            if t not in types:
                errors.append(f"{ctx}: implicates unknown type '{t}'")
        if not 0 <= imp.get("w", 0) <= 1:
            errors.append(f"{ctx}: implicates.w must be 0..1")

    # --- model
    if "entities" in model:
        errors.append("model: 'entities' was renamed to 'participants'; rename the key")
    parts, hyps = model.get("participants", []), model.get("hypotheses", [])
    part_ids = {e.get("id") for e in parts}
    hyp_ids = {h.get("id") for h in hyps}
    for e in parts:
        if e.get("type") not in types:
            errors.append(f"model: participant '{e.get('id')}' has unknown type '{e.get('type')}'")
        if len(e.get("anchor", {})) != 1:
            errors.append(f"model: participant '{e.get('id')}' needs exactly one anchor property")
        e.setdefault("leak", 0.02)
    for h in hyps:
        if not 0 <= h.get("prior", -1) <= 1:
            errors.append(f"model: hypothesis '{h.get('id')}' needs a prior in 0..1")
    all_ids = [x.get("id") for x in list(sigs) + parts + hyps]
    dupes = {x for x in all_ids if all_ids.count(x) > 1}
    if dupes:
        errors.append(f"model: ids must be unique across signals, participants and hypotheses: {sorted(dupes)}")
    for band, items in (("signals", sigs), ("participants", parts), ("hypotheses", hyps)):
        if len(items) > MAX_BAND:
            errors.append(f"model: at most {MAX_BAND} {band} (got {len(items)})")
    if not hyps:
        errors.append("model: at least one hypothesis")

    edges = []
    for i, e in enumerate(model.get("edges", [])):
        ctx = f"model edge #{i + 1}"
        if e.get("to") not in hyp_ids:
            errors.append(f"{ctx}: 'to' must be a hypothesis (got '{e.get('to')}'); signal->participant links come from signals.json implicates")
        kind = e.get("kind", "support")
        if kind not in ("support", "inhibit"):
            errors.append(f"{ctx}: kind must be support or inhibit")
        if not 0 <= e.get("w", -1) <= 1:
            errors.append(f"{ctx}: w must be 0..1")
        if "from_type" in e:
            if e["from_type"] not in types:
                errors.append(f"{ctx}: unknown from_type '{e['from_type']}'")
            for part in parts:
                if part.get("type") == e["from_type"]:
                    edges.append({"from": part["id"], "to": e["to"], "w": e["w"], "kind": kind, "via_type": e["from_type"]})
        elif e.get("from") in sig_ids or e.get("from") in part_ids:
            edges.append({"from": e["from"], "to": e["to"], "w": e["w"], "kind": kind})
        else:
            errors.append(f"{ctx}: 'from' must be a signal or participant id (got '{e.get('from')}')")

    if errors:
        raise PackError(pack_dir, errors)

    # --- parse every source once to catch format errors and dead field paths
    untimed = []
    for sid, s in sources.items():
        if s["transport"] != "file":
            continue
        try:
            skipped = []
            with open(s["path"], encoding="utf-8") as f:
                recs = pipeline.parse(sid, s["format"], f.read(), s.get("options"), skipped)
        except Exception as e:  # report any parser failure as a pack error
            raise PackError(pack_dir, [f"source '{sid}' failed to parse as {s['format']}: {e}"])
        if skipped:
            warnings.append(f"source '{sid}': {len(skipped)} row(s) skipped, e.g. {skipped[0]['pointer']}: {skipped[0]['reason']}")
        if not recs:
            warnings.append(f"source '{sid}' parsed to zero records")
            continue
        for mi, m in enumerate(maps):
            if m["source"] != sid:
                continue
            for prop, spec in m["props"].items():
                field, _ = pipeline._prop_spec(spec)
                if all(pipeline.get_path(r["data"], field) in (None, "") for r in recs):
                    warnings.append(f"mapping #{mi + 1}: field '{field}' never appears in '{sid}'")
        tconf = s.get("time")
        if tconf and "document" in tconf:
            try:
                with open(s["path"], encoding="utf-8") as f:
                    pipeline.document_time(f.read(), s["format"], tconf)
            except Exception as e:  # any failure to date the document is a pack error
                raise PackError(pack_dir, [f"manifest: source '{sid}' document time: {e}"])
        elif tconf:
            fields = tconf["field"]
            fields = fields if isinstance(fields, list) else [fields]
            for field in fields:
                if all(pipeline.get_path(r["data"], field) in (None, "") for r in recs):
                    warnings.append(f"manifest: time field '{field}' never appears in '{sid}'")
            bad, missing = [], 0
            for r in recs:
                try:
                    if pipeline.record_time(r["data"], tconf) is None:
                        missing += 1
                except (TypeError, ValueError, OverflowError) as e:
                    bad.append(f"{r['pointer']} ({e})")
            if bad:
                warnings.append(f"manifest: {len(bad)} record(s) of '{sid}' have a time that does not parse as "
                                f"{tconf['unit']} and will count as undated, e.g. {bad[0]}")
            if missing:
                warnings.append(f"manifest: {missing} record(s) of '{sid}' have no time value and will count as undated")
        else:
            untimed.append(sid)
    if untimed and len(untimed) < len(sources):
        warnings.append(f"undated sources (no time config), treated as unknown_time={unknown_time}: {', '.join(untimed)}")

    return {
        "id": pid,
        "dir": pack_dir,
        "title": manifest.get("title", pid),
        "premise": manifest.get("premise", ""),
        "sources": sources,
        "unknown_time": unknown_time,
        "source_order": [s["id"] for s in manifest["sources"]],
        "object_types": types,
        "mappings": list(enumerate(maps)),
        "signals": sigs,
        "participants": parts,
        "hypotheses": hyps,
        "edges": edges,
        "narrative": narrative,
        "layout": layout(sigs, parts, hyps),
        "warnings": warnings,
    }


def discover(root):
    """Return {pack_id: pack} and {dir_name: [errors]} for every pack under root."""
    packs, failures = {}, {}
    for name in sorted(os.listdir(root)):
        path = os.path.join(root, name)
        if not os.path.isdir(path) or not os.path.isfile(os.path.join(path, "manifest.json")):
            continue
        try:
            pack = load_pack(path)
            packs[pack["id"]] = pack
        except PackError as e:
            failures[name] = e.errors
    return packs, failures

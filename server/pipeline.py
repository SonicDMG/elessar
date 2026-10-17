"""Raw payloads -> records -> ontology fragments.

Every parser returns records shaped like
    {"source": id, "index": n, "data": {...}, "pointer": "...", "excerpt": "..."}
where `pointer` says where in the raw payload the record came from (line
number, XPath, JSON path, row index) and `excerpt` is the raw text itself.
That pair is what lets the UI show property-level provenance.
"""
import csv
import json
import re
import xml.etree.ElementTree as ET
from datetime import datetime, timezone


# ---------------------------------------------------------------- field access

def get_path(obj, path):
    """Read a dotted path. Exact keys win, so flattened XML keys like
    'window.@start' resolve directly."""
    if not isinstance(obj, dict):
        return None
    if path in obj:
        return obj[path]
    cur = obj
    for part in path.split("."):
        if isinstance(cur, dict) and part in cur:
            cur = cur[part]
        else:
            return None
    return cur


def _num(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def _test(value, cond):
    if not isinstance(cond, dict):
        return value is not None and str(value) == str(cond)
    for op, arg in cond.items():
        if op == "exists":
            if (value not in (None, "")) != bool(arg):
                return False
            continue
        if value is None:
            return False
        if op == "eq" and str(value) != str(arg):
            return False
        if op == "ne" and str(value) == str(arg):
            return False
        if op in ("gt", "gte", "lt", "lte"):
            v, a = _num(value), _num(arg)
            if v is None or a is None:
                return False
            if op == "gt" and not v > a:
                return False
            if op == "gte" and not v >= a:
                return False
            if op == "lt" and not v < a:
                return False
            if op == "lte" and not v <= a:
                return False
        if op == "in" and str(value) not in [str(x) for x in arg]:
            return False
        if op == "nin" and str(value) in [str(x) for x in arg]:
            return False
        if op == "contains" and str(arg).lower() not in str(value).lower():
            return False
        if op == "startswith" and not str(value).startswith(str(arg)):
            return False
    return True


CONDITION_OPS = {"exists", "eq", "ne", "gt", "gte", "lt", "lte", "in", "nin", "contains", "startswith"}


def matches(data, where):
    """`where` maps field paths to a literal (equality) or an operator dict."""
    if not where:
        return True
    return all(_test(get_path(data, field), cond) for field, cond in where.items())


# ---------------------------------------------------------------- parsers

def _record(source, index, data, pointer, excerpt):
    return {"source": source, "index": index, "data": data, "pointer": pointer, "excerpt": excerpt}


def parse_csv(source, text, opts):
    lines = text.splitlines()
    reader = csv.DictReader(lines, delimiter=opts.get("delimiter", ","))
    out = []
    for i, row in enumerate(reader):
        line_no = reader.line_num
        data = {(k or "").strip(): (v or "").strip() for k, v in row.items()}
        out.append(_record(source, i, data, f"line {line_no}", lines[line_no - 1]))
    return out


def parse_jsonl(source, text, opts):
    out = []
    for n, line in enumerate(text.splitlines(), start=1):
        if not line.strip():
            continue
        out.append(_record(source, len(out), json.loads(line), f"line {n}", line))
    return out


def parse_nested_json(source, text, opts):
    path = opts["records"]
    items = get_path(json.loads(text), path) or []
    return [
        _record(source, i, item, f"{path}[{i}]", json.dumps(item, separators=(", ", ": ")))
        for i, item in enumerate(items)
    ]


def _flatten_xml(el, prefix, out):
    for k, v in el.attrib.items():
        out.setdefault(f"{prefix}@{k}", v)
    text = (el.text or "").strip()
    if text:
        out.setdefault(prefix[:-1] if prefix else "#text", text)
    for child in el:
        _flatten_xml(child, f"{prefix}{child.tag}.", out)


def parse_xml(source, text, opts):
    root = ET.fromstring(text)
    path = opts["records"]
    out = []
    for i, el in enumerate(root.findall(path)):
        data = {}
        _flatten_xml(el, "", data)
        excerpt = re.sub(r"\s+\n", "\n", ET.tostring(el, encoding="unicode")).strip()
        out.append(_record(source, i, data, f"/{root.tag}/{el.tag}[{i + 1}]", excerpt))
    return out


def parse_columnar(source, text, opts):
    doc = json.loads(text)
    fields = get_path(doc, opts.get("fields", "schema.fields"))
    rows_path = opts.get("rows", "rows")
    rows = get_path(doc, rows_path) or []
    return [
        _record(source, i, dict(zip(fields, row)), f"{rows_path}[{i}]", json.dumps(row))
        for i, row in enumerate(rows)
    ]


def parse_kv(source, text, opts):
    pair_sep, kv_sep = opts.get("pair_sep", ";"), opts.get("kv_sep", "=")
    out = []
    for n, line in enumerate(text.splitlines(), start=1):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        data = {}
        for pair in stripped.split(pair_sep):
            if kv_sep in pair:
                k, v = pair.split(kv_sep, 1)
                data[k.strip()] = v.strip()
        out.append(_record(source, len(out), data, f"line {n}", line))
    return out


PARSERS = {
    "csv": parse_csv,
    "jsonl": parse_jsonl,
    "nested-json": parse_nested_json,
    "xml": parse_xml,
    "columnar": parse_columnar,
    "kv": parse_kv,
}

CONTENT_TYPES = {
    "csv": "text/csv",
    "jsonl": "application/x-ndjson",
    "nested-json": "application/json",
    "xml": "application/xml",
    "columnar": "application/json",
    "kv": "text/plain",
}


def parse(source_id, fmt, text, opts):
    return PARSERS[fmt](source_id, text, opts or {})


# ---------------------------------------------------------------- transforms

def _iso(dt):
    return dt.strftime("%Y-%m-%dT%H:%M:%SZ")


def _name_last_first(v):
    if "," in v:
        last, first = v.split(",", 1)
        return f"{first.strip()} {last.strip()}"
    return v


def _date_dmy(v):
    d, m, y = re.split(r"[/.-]", v)
    return f"{y}-{int(m):02d}-{int(d):02d}"


TRANSFORMS = {
    "lower": lambda v: str(v).lower(),
    "upper": lambda v: str(v).upper(),
    "strip": lambda v: str(v).strip(),
    "title": lambda v: str(v).title(),
    "int": lambda v: int(float(v)),
    "float": lambda v: float(v),
    "name_last_first": lambda v: _name_last_first(str(v)),
    "date_dmy": lambda v: _date_dmy(str(v)),
    "epoch_ms": lambda v: _iso(datetime.fromtimestamp(int(v) / 1000, timezone.utc)),
    "epoch_s": lambda v: _iso(datetime.fromtimestamp(int(v), timezone.utc)),
}


def _prop_spec(spec):
    """A property spec is a field path, or {"from": path, "transform": name | [names]}."""
    if isinstance(spec, str):
        return spec, []
    tf = spec.get("transform", [])
    return spec["from"], [tf] if isinstance(tf, str) else list(tf)


def to_fragments(records, mappings):
    """Apply each mapping (with its global index `mi`) to the records of its source."""
    frags = []
    for mi, m in mappings:
        for r in records:
            if not matches(r["data"], m.get("when")):
                continue
            props = {}
            for prop, spec in m["props"].items():
                field, tfs = _prop_spec(spec)
                v = get_path(r["data"], field)
                if v in (None, ""):
                    continue
                for name in tfs:
                    v = TRANSFORMS[name](v)
                props[prop] = {"value": v, "source": r["source"], "pointer": r["pointer"], "field": field}
            if props:
                frags.append({
                    "id": f"{r['source']}#{r['index']}:{m['type']}:{mi}",
                    "type": m["type"],
                    "source": r["source"],
                    "record": (r["source"], r["index"]),
                    "props": props,
                })
    return frags


def parse_time(value, unit):
    if unit == "ms":
        return datetime.fromtimestamp(int(value) / 1000, timezone.utc)
    if unit == "s":
        return datetime.fromtimestamp(int(value), timezone.utc)
    return datetime.strptime(str(value), "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)

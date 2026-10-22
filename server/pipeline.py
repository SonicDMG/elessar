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


def _skip(skipped, pointer, reason, excerpt):
    """Parsers are tolerant: a bad row is reported, not fatal for the source."""
    if skipped is not None:
        skipped.append({"pointer": pointer, "reason": reason, "excerpt": excerpt[:160]})


def parse_csv(source, text, opts, skipped=None):
    lines = text.splitlines()
    reader = csv.DictReader(lines, delimiter=opts.get("delimiter", ","), restkey="__extra__")
    out = []
    for row in reader:
        line_no = reader.line_num
        raw = lines[line_no - 1]
        if None in row.values():
            have = sum(1 for v in row.values() if v is not None)
            _skip(skipped, f"line {line_no}", f"truncated row: {have} of {len(reader.fieldnames)} columns", raw)
            continue
        if "__extra__" in row:
            _skip(skipped, f"line {line_no}", f"{len(row['__extra__'])} extra column(s)", raw)
            continue
        data = {(k or "").strip(): (v or "").strip() for k, v in row.items()}
        out.append(_record(source, len(out), data, f"line {line_no}", raw))
    return out


def parse_jsonl(source, text, opts, skipped=None):
    out = []
    for n, line in enumerate(text.splitlines(), start=1):
        if not line.strip():
            continue
        try:
            data = json.loads(line)
        except ValueError as e:
            _skip(skipped, f"line {n}", f"invalid JSON: {e.msg} at column {e.pos + 1}", line)
            continue
        if not isinstance(data, dict):
            _skip(skipped, f"line {n}", "not a JSON object", line)
            continue
        out.append(_record(source, len(out), data, f"line {n}", line))
    return out


def parse_nested_json(source, text, opts, skipped=None):
    path = opts["records"]
    items = get_path(json.loads(text), path) or []
    out = []
    for i, item in enumerate(items):
        if not isinstance(item, dict):
            _skip(skipped, f"{path}[{i}]", "not an object", json.dumps(item))
            continue
        out.append(_record(source, len(out), item, f"{path}[{i}]", json.dumps(item, separators=(", ", ": "))))
    return out


def _flatten_xml(el, prefix, out):
    for k, v in el.attrib.items():
        out.setdefault(f"{prefix}@{k}", v)
    text = (el.text or "").strip()
    if text:
        out.setdefault(prefix[:-1] if prefix else "#text", text)
    for child in el:
        _flatten_xml(child, f"{prefix}{child.tag}.", out)


def parse_xml(source, text, opts, skipped=None):
    root = ET.fromstring(text)
    path = opts["records"]
    out = []
    for i, el in enumerate(root.findall(path)):
        data = {}
        _flatten_xml(el, "", data)
        excerpt = re.sub(r"\s+\n", "\n", ET.tostring(el, encoding="unicode")).strip()
        out.append(_record(source, i, data, f"/{root.tag}/{el.tag}[{i + 1}]", excerpt))
    return out


def parse_columnar(source, text, opts, skipped=None):
    doc = json.loads(text)
    fields = get_path(doc, opts.get("fields", "schema.fields"))
    rows_path = opts.get("rows", "rows")
    rows = get_path(doc, rows_path) or []
    out = []
    for i, row in enumerate(rows):
        if not isinstance(row, list) or len(row) != len(fields):
            n = len(row) if isinstance(row, list) else "no"
            _skip(skipped, f"{rows_path}[{i}]", f"{n} values for {len(fields)} fields", json.dumps(row))
            continue
        out.append(_record(source, len(out), dict(zip(fields, row)), f"{rows_path}[{i}]", json.dumps(row)))
    return out


def parse_kv(source, text, opts, skipped=None):
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
        if not data:
            _skip(skipped, f"line {n}", "no key=value pairs", line)
            continue
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


def parse(source_id, fmt, text, opts, skipped=None):
    """Records of one payload. Pass a list as `skipped` to collect the rows
    the parser had to drop, each as {"pointer", "reason", "excerpt"}."""
    return PARSERS[fmt](source_id, text, opts or {}, skipped)


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


TIME_UNITS = ("ms", "s", "iso", "date_dmy")


def parse_time(value, unit):
    if unit == "ms":
        return datetime.fromtimestamp(int(value) / 1000, timezone.utc)
    if unit == "s":
        return datetime.fromtimestamp(int(value), timezone.utc)
    if unit == "date_dmy":
        return datetime.strptime(_date_dmy(str(value)), "%Y-%m-%d").replace(tzinfo=timezone.utc)
    text = str(value).strip()
    for fmt in ("%Y-%m-%dT%H:%M:%SZ", "%Y-%m-%dT%H:%MZ", "%Y-%m-%dT%H:%M:%S.%fZ", "%Y-%m-%d"):
        try:
            return datetime.strptime(text, fmt).replace(tzinfo=timezone.utc)
        except ValueError:
            continue
    raise ValueError(f"not an ISO UTC timestamp: {text!r}")


DOCUMENT_TIME_KEYS = ("constant", "regex", "path")


def document_time(raw, fmt, spec):
    """A whole source's timestamp from its document rather than its records:
    `time.document` is {"constant": ...}, {"regex": ...} (group 1 over the
    raw text) or {"path": ...} (a root path for nested-json/columnar, or
    "@attr" / a child path on an XML root)."""
    doc = spec["document"]
    unit = spec.get("unit", "iso")
    if "constant" in doc:
        return parse_time(doc["constant"], unit)
    if "regex" in doc:
        m = re.search(doc["regex"], raw)
        if not m or not m.groups():
            raise ValueError(f"document regex {doc['regex']!r} found no group in the source text")
        return parse_time(m.group(1), unit)
    path = doc["path"]
    if fmt in ("nested-json", "columnar"):
        v = get_path(json.loads(raw), path)
    elif fmt == "xml":
        root = ET.fromstring(raw)
        v = root.attrib.get(path[1:]) if path.startswith("@") else root.findtext(path)
    else:
        raise ValueError(f"document path is not supported for {fmt}; use a regex or a constant")
    if v in (None, ""):
        raise ValueError(f"document path {path!r} is empty")
    return parse_time(v, unit)


def record_time(data, spec):
    """A record's timestamp per a source's `time` config: the first of the
    config's field paths that is present, parsed by its unit. None if absent."""
    fields = spec["field"] if isinstance(spec["field"], list) else [spec["field"]]
    for field in fields:
        v = get_path(data, field)
        if v not in (None, ""):
            return parse_time(v, spec.get("unit", "iso"))
    return None

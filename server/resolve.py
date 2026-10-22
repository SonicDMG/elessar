"""Entity resolution: fragments -> clusters.

Fragments that share a key value merge automatically (the deterministic part,
like hashing). Fuzzy matches on configured properties only become proposals:
a human commits or rejects them, and a committed merge can be reverted,
following Gotham's rule that a resolved object keeps its constituents.
"""
import re
from difflib import SequenceMatcher


def norm(value):
    return re.sub(r"\s+", " ", re.sub(r"[^0-9a-z@._ -]", "", str(value).lower())).strip()


def fuzzy_norm(value):
    return re.sub(r"\s+", " ", re.sub(r"[^0-9a-z ]", " ", str(value).lower())).strip()


class UnionFind:
    def __init__(self, items):
        self.parent = {x: x for x in items}

    def find(self, x):
        while self.parent[x] != x:
            self.parent[x] = self.parent[self.parent[x]]
            x = self.parent[x]
        return x

    def union(self, a, b):
        ra, rb = self.find(a), self.find(b)
        if ra != rb:
            # Keep the smaller id as root so cluster ids are stable.
            lo, hi = sorted((ra, rb))
            self.parent[hi] = lo


def _titles(otype):
    t = otype["title"]
    return [t] if isinstance(t, str) else list(t)


def resolve(fragments, object_types, committed, source_order):
    """Return (clusters by id, cluster id by fragment id)."""
    by_id = {f["id"]: f for f in fragments}
    uf = UnionFind(by_id)

    seen = {}
    for f in fragments:
        for key in object_types[f["type"]].get("keys", []):
            if key in f["props"]:
                k = (f["type"], key, norm(f["props"][key]["value"]))
                if k in seen:
                    uf.union(seen[k], f["id"])
                else:
                    seen[k] = f["id"]

    for a, b in committed:
        if a in by_id and b in by_id:
            uf.union(a, b)

    groups = {}
    for f in fragments:
        groups.setdefault(uf.find(f["id"]), []).append(f)

    rank = {s: i for i, s in enumerate(source_order)}
    clusters, cluster_of = {}, {}
    for root, frags in groups.items():
        frags.sort(key=lambda f: (rank.get(f["source"], 99), f["id"]))
        props = {}
        for f in frags:
            for name, pv in f["props"].items():
                vals = props.setdefault(name, [])
                existing = next((v for v in vals if norm(v["value"]) == norm(pv["value"])), None)
                prov = {"source": pv["source"], "pointer": pv["pointer"], "field": pv["field"], "fragment": f["id"]}
                if existing:
                    existing["provenance"].append(prov)
                else:
                    vals.append({"value": pv["value"], "provenance": [prov]})
        otype = object_types[frags[0]["type"]]
        title = next((props[t][0]["value"] for t in _titles(otype) if t in props), None)
        clusters[root] = {
            "id": root,
            "type": frags[0]["type"],
            "fragments": [f["id"] for f in frags],
            "sources": sorted({f["source"] for f in frags}, key=lambda s: rank.get(s, 99)),
            "props": props,
            "title": title,
            "conflicts": sorted(n for n, vals in props.items() if len(vals) > 1 and n not in otype.get("multi", [])),
        }
        for f in frags:
            cluster_of[f["id"]] = root
    return clusters, cluster_of


def proposals(clusters, by_frag, object_types, rejected):
    """Fuzzy merge proposals between clusters of the same type."""
    out = []
    by_type = {}
    for c in clusters.values():
        by_type.setdefault(c["type"], []).append(c)
    for tname, cs in by_type.items():
        fz = object_types[tname].get("fuzzy")
        if not fz:
            continue
        threshold = fz.get("threshold", 0.86)
        keys = object_types[tname].get("keys", [])
        cs.sort(key=lambda c: c["id"])
        for i, a in enumerate(cs):
            for b in cs[i + 1:]:
                best = None
                for pa in fz["properties"]:
                    for pb in fz["properties"]:
                        for va in a["props"].get(pa, []):
                            for vb in b["props"].get(pb, []):
                                score = SequenceMatcher(None, fuzzy_norm(va["value"]), fuzzy_norm(vb["value"])).ratio()
                                if score >= threshold and (best is None or score > best[0]):
                                    best = (score, pa, va, pb, vb)
                if not best:
                    continue
                score, pa, va, pb, vb = best
                fa, fb = va["provenance"][0]["fragment"], vb["provenance"][0]["fragment"]
                pid = "|".join(sorted((fa, fb)))
                if pid in rejected:
                    continue
                conflict = [
                    k for k in keys
                    if a["props"].get(k) and b["props"].get(k)
                    and not {norm(v["value"]) for v in a["props"][k]} & {norm(v["value"]) for v in b["props"][k]}
                ]
                out.append({
                    "id": pid,
                    "type": tname,
                    "score": round(score, 3),
                    "a": {"cluster": a["id"], "property": pa, "value": va["value"], "source": by_frag[fa]["source"], "fragment": fa},
                    "b": {"cluster": b["id"], "property": pb, "value": vb["value"], "source": by_frag[fb]["source"], "fragment": fb},
                    "conflict": conflict,
                })
    return out


def anchor(participant, clusters, by_frag):
    """Find the cluster a participant slot is anchored to, or None if it is absent."""
    (prop, value), = participant["anchor"].items()
    want = norm(value)
    for c in clusters.values():
        if c["type"] != participant["type"]:
            continue
        for v in c["props"].get(prop, []):
            if norm(v["value"]) == want:
                return c
    return None

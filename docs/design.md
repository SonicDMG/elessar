# ELESSAR — design

A local, Palantir-inspired demo: several differently-shaped data sources fused
into one view, shown as a pixel-art checkers board whose pieces are nodes in a
Bayesian network. Built as both a showcase and a learning tool, so the reveal
moments have to land *and* every number has to be inspectable.

## The question it answers

"Something notable changed. Several systems each saw part of it. None of them
agree on what anything is called. What explains it?"

The engine never knows the domain. A **scenario pack** (a directory of data
files) supplies the sources, mappings, rules and model.

## Decisions

| Area | Decision |
|---|---|
| Purpose | Showcase and learning tool |
| Domain | Generic notable changes and alerts; domain lives in scenario packs |
| Inference | Noisy-OR + inhibitors + competitive normalization (approximate, labeled as such) |
| Layout | Board + object/provenance panel + source rack with raw-payload peek |
| Stack | Python 3.9 standard library backend, vanilla JS canvas frontend, no build |
| Game loop | Full investigator: connect, judge, commit merges, crown |
| Stakes | No fail state; scored reveals that each explain a concept |
| Art | Dark CRT, phosphor accents, hand-authored 16×16 sprites at integer scale |
| Scale | ~20 pieces, hand-authored narrative |
| Packs | `change-cascade` (default, 6 sources) and `supply-chain` (4 sources) |
| Switching | `--scenario` CLI flag and a UI dropdown, both via `/api/scenarios` |
| Port | `localhost:8777` |

## Scenario pack format

```
scenarios/<id>/
  manifest.json    Connect     title, premise, sources (format, file or URL, confidence)
  mapping.json     Ontology    object types (title, keys, fuzzy), field → property mappings
  signals.json     Logic       rules that turn records into signals
  model.json       Inference   entities, hypotheses, edges
  narrative.json   Story       intro, finale, insight text overrides
  sources/         Raw         the payloads the mock services serve, byte for byte
```

All JSON (Python 3.9 has no TOML in the standard library; YAML would need a
dependency). The validator (`python3 -m server.app --check`) fails loudly on
dangling references, unknown formats, transforms or rule kinds, and parses
every source file to warn about mapped fields that never appear.

### What is code and what is data

| Layer | Code (domain-free) | Data (per pack) |
|---|---|---|
| Transport | HTTP server, fetch | `file` or `http`, latency, confidence |
| Parsing | `csv`, `jsonl`, `nested-json`, `xml`, `columnar`, `kv` | delimiter, record path, XPath |
| Mapping | transforms (`lower`, `name_last_first`, `date_dmy`, `epoch_ms`, …) | field → property, `when` filters |
| Resolution | union-find on keys, fuzzy similarity | key, fuzzy and multi-valued properties, threshold |
| Signals | rule kinds `match`, `time_window`, `count`, `co_resolved` | parameters, strength, implicated types |
| Inference | noisy-OR, inhibitors, competition | entities, hypotheses, priors, edges |
| Board | geometry, sprites, animation | nothing: width is derived from the pack |

A new kind of rule needs code. Everything else is data.

## Pipeline

1. **Connect**: the server fetches the source over real HTTP — its own
   `/mock/<pack>/<source>` endpoint, or a real URL. That is why it runs on
   `ThreadingHTTPServer`: a single-threaded server would deadlock fetching
   from itself.
2. **Parse** into records. Each record keeps a raw pointer (line number, XPath,
   JSON path, row index) and the raw excerpt.
3. **Map** records to ontology *fragments*: typed property bags in which every
   value carries `{value, source, pointer, field}`.
4. **Resolve** fragments into clusters. Fragments sharing a key value
   (normalized) merge automatically. Fuzzy matches on configured properties
   become **proposals** the user commits or rejects. Commits are reversible.
   Proposals between clusters whose key values disagree are flagged.
5. **Anchor** model entities to clusters (`"anchor": {"badge": "B-2093"}`).
   An entity with no cluster yet is absent from the board. One whose cluster
   lacks its title property is **unresolved** and drawn grey.
6. **Evaluate signals.** Each hit carries the fragments it came from.
   A signal implicates an entity only if a hit's fragments resolve into that
   entity's cluster, so which entities a signal touches emerges from
   resolution rather than being hand-wired.
7. **Infer.**

## Inference

Signal activation: `a = min(source confidence) × strength × judgement`,
where judgement is trust 1.0 / discount 0.5 / dismiss 0.

Signal → entity weight: `w × hits_linked_to_entity / max_hits_linked`.

For each entity and hypothesis X, with support parents `(p_j, w_j)` and
inhibitor parents `(p_k, v_k)`:

```
support = 1 − (1 − leak) · Π (1 − w_j · p_j)
inhibit =                  Π (1 − v_k · p_k)
p       = support · inhibit
```

A hypothesis's leak is its prior. Hypotheses then compete:
`s = Σ p_H`; if `s > 1`, each is divided by `s` (explaining away).

Edges to hypotheses come from signals or entities, by id or by entity type
(`"from_type": "Actor"` expands to every on-board Actor). Edges only point
upward (signal → entity → hypothesis), so the graph is acyclic by
construction.

An optional **relevance feedback** pass (off by default) boosts each entity by
`0.15 × max(p_H · w)` over the hypotheses it supports, then re-infers. It is
not Bayesian; the toggle exists to show what it distorts.

This is an approximation, not exact belief propagation.

## Board

- Height 9. Width `2 × (largest band) + 1`, so every lane has room to step right.
- Dark squares are `(col + row) % 2 == 1`; every piece always sits on one.
- Bands, top to bottom: hypotheses (base row 2, odd columns), entities
  (base row 5, even columns), signals (base row 8, odd columns).
- Belief advances a piece along `(c, r) → (c+1, r−1) → (c, r−2)`: two legal
  diagonal checkers moves. Steps at `p ≥ 0.33` and `p ≥ 0.66`.
- A hypothesis at `p ≥ 0.85` is **crownable**; the king sprite (stacked double
  piece) appears only when the user crowns it.
- A hypothesis is **explained away** (jumped off the board) when `p ≤ 0.08` and
  live inhibitors have cut it by at least half.
- Each inference stage is one animation tick, so the board plays itself out.

## Investigator verbs

1. **Connect / disconnect** a source.
2. **Judge** a signal: trust, discount, dismiss.
3. **Commit, reject or revert** a merge.
4. **Crown** a hypothesis. The board reports whether the posterior backed it.

Every action is appended to an audit trail.

## Insights (scored, no fail state)

Shape Shock · First Link · Identity Resolved · Merge Committed · Inhibited ·
Explained Away · Crowned · Full Fusion. Each shows one line explaining the
concept that just fired.

## Palette

```
bg #07090c   dark square #11161c   light square #171e26
amber   #ffb23f  signals        cyan  #3fd8ff  entities
magenta #ff5fd2  hypotheses     grey  #4a5663  unresolved / dormant
red     #ff4d4d  inhibitors     gold  #ffd75e  crowns
```

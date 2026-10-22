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
| Layout | Board + details/provenance panel + source rack with raw-payload peek |
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
  model.json       Inference   participants, hypotheses, edges
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
| Inference | noisy-OR, inhibitors, competition | participants, hypotheses, priors, edges |
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
5. **Anchor** model participants to clusters (`"anchor": {"badge": "B-2093"}`).
   A participant with no cluster yet is absent from the board. One whose cluster
   lacks its title property is **unresolved** and drawn grey.
6. **Evaluate signals.** Each hit carries the fragments it came from.
   A signal implicates a participant only if a hit's fragments resolve into that
   participant's cluster, so which participants a signal touches emerges from
   resolution rather than being hand-wired.
7. **Infer.**

## Inference

Signal activation: `a = min(source confidence) × strength × judgement`,
where judgement is trust 1.0 / discount 0.5 / dismiss 0.

Signal → participant weight: `w × hits_linked_to_participant / max_hits_linked`.

For each participant and hypothesis X, with support parents `(p_j, w_j)` and
inhibitor parents `(p_k, v_k)`:

```
support = 1 − (1 − leak) · Π (1 − w_j · p_j)
inhibit =                  Π (1 − v_k · p_k)
p       = support · inhibit
```

A hypothesis's leak is its prior. Hypotheses then compete:
`s = Σ p_H`; if `s > 1`, each is divided by `s` (explaining away).

Edges to hypotheses come from signals or participants, by id or by participant type
(`"from_type": "Actor"` expands to every on-board Actor). Edges only point
upward (signal → participant → hypothesis), so the graph is acyclic by
construction.

An optional **relevance feedback** pass (off by default) boosts each participant by
`0.15 × max(p_H · w)` over the hypotheses it supports, then re-infers. It is
not Bayesian; the toggle exists to show what it distorts.

This is an approximation, not exact belief propagation.

## Evidence engines

A signal's strength can come from either engine; both always run and the
user picks which drives the board.

- **Rules**: `where` conditions and a hand-set strength (or a field such as
  a self-reported confidence).
- **Jev** (TypeSafe's System One model, via OpenRouter): a `noul` yes/no
  question per candidate record. Hits are records at or above the threshold;
  strength is the highest probability among them. Hits link to participants
  through resolution exactly as rule hits do.

The details panel shows both per signal, flagging records where they
disagree. Answers are cached on disk by model, record and question.

## Agent

An in-app agent in the side panel. It answers questions using a fresh board
snapshot per message plus tools: `get_piece`, `get_record`, `what_if` (runs
changes on a throwaway copy of the session), `ask_jev`, `highlight`, and
`propose_action`. It never changes the board directly. Proposals are
approve/decline cards; approving runs the action through the normal action
path and logs it. Inference model via OpenRouter's Anthropic Messages API
(`ELESSAR_INFERENCE_MODEL`, default Claude Sonnet 5.5); decision model for
Jev via `ELESSAR_DECISION_MODEL`.

## Board

- Height 9. Width `2 × (largest band) + 1`, so every lane has room to step right.
- Dark squares are `(col + row) % 2 == 1`; every piece always sits on one.
- Bands, top to bottom: hypotheses (base row 2, odd columns), participants
  (base row 5, even columns), signals (base row 8, odd columns).
- Belief advances a piece along `(c, r) → (c+1, r−1) → (c, r−2)`: two legal
  diagonal checkers moves. Steps at `p ≥ 0.33` and `p ≥ 0.66`.
- A hypothesis at `p ≥ 0.85` is **crownable**; the king sprite (stacked double
  piece) appears only when the user crowns it.
- A hypothesis is **explained away** when `p ≤ 0.08` and live inhibitors have
  cut it by at least half. A participant is **removed** when every source that
  described it is disconnected.
- Nothing simply disappears. Pieces leaving the board hop off its left edge
  into the **Off the board** column, where each piece's sprite sits beside a
  one-sentence reason (for hypotheses: the inhibitors that cut them, by how
  much, and the competition divisor).
- Every off-board piece can return. A hypothesis is **kept** on the board as
  the user's override (pinned, belief unchanged, releasable). A participant
  returns by reconnecting the sources it last came from.
- Each inference stage is one animation tick, so the board plays itself out.

## Investigator verbs

1. **Connect / disconnect** a source.
2. **Judge** a signal: trust, discount, dismiss.
3. **Commit, reject or revert** a merge.
4. **Crown** a hypothesis. The board reports whether the posterior backed it.
5. **Return to board / release** a piece from the Off the board column.

Every action is appended to an audit trail.

## Insights (scored, no fail state)

Shape Shock · First Link · Identity Resolved · Merge Committed · Inhibited ·
Explained Away · Crowned · Full Fusion. Each shows one line explaining the
concept that just fired.

## Palette

```
bg #07090c   dark square #11161c   light square #171e26
amber   #ffb23f  signals        cyan  #3fd8ff  participants
magenta #ff5fd2  hypotheses     grey  #4a5663  unresolved / dormant
red     #ff4d4d  inhibitors     gold  #ffd75e  crowns
```

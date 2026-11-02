# ELESSAR engine specification

The contract both engines (Python `server/`, Rust `rust/`) implement. Where
the two disagree, this document is the arbiter; where this document is
silent, the Python engine's behaviour is the reference and should be added
here. Packs are the executable tests of this spec (`expect.json`, run by
`tools/expect.py` for Python and by `elessar-core`'s tests for Rust).

Sections marked **(allowed divergence)** are where the engines may legitimately
differ; everything else must agree in behaviour, though not necessarily to the
last decimal.

## 1. Packs

A pack is a directory under `scenarios/` with `manifest.json`,
`mapping.json`, `signals.json`, `model.json`, `narrative.json`, `sources/`
and optionally `expect.json`. The format is documented in `README.md`
("Writing a scenario pack"); this section lists what an engine must enforce.

- **Formats**: `csv`, `jsonl`, `nested-json`, `xml`, `columnar`, `kv`.
- **Transports**: `file` (served by the engine's own `/mock/<pack>/<source>`
  endpoint with the declared `latency_ms`) and `http` (fetched from `url`).
- **Time**: `{"field": path | [paths], "unit"}` per record, or
  `{"document": {"constant" | "regex" | "path"}, "unit"}` per source; units
  `ms`, `s`, `iso`, `date_dmy`. Top-level `unknown_time`: `throughout`
  (default), `at_connect`, `after`.
- **Object types**: `title` (property or ordered list), `keys`, `multi`,
  `fuzzy: {properties, threshold}` (threshold default 0.86).
- **Mappings**: `source`, `type`, optional `when`, `props` of
  `path | {"from": path, "transform": name | [names]}`. Transforms: `lower`,
  `upper`, `strip`, `title`, `int`, `float`, `name_last_first`, `date_dmy`,
  `epoch_ms`, `epoch_s`.
- **Conditions** (`where`, `when`): field path → literal (equality on the
  string form) or `{op: arg}` with ops `exists eq ne gt gte lt lte in nin
  contains startswith`. Numeric ops compare as floats and fail when either
  side is not numeric; `contains` is case-insensitive.
- **Signals**: kinds `match`, `time_window` (`params.field`, `params.unit`,
  `params.hours [start, end)` UTC, wrapping midnight), `count`
  (`params.group_by`, `params.min` default 2), `co_resolved`
  (`params.type`, `params.clauses: [{source, where}]`). `strength` is a
  number or `{"field": path}` (max over hit records, non-numeric ignored).
  `implicates: {types?, w}`. Optional `jev` block (`question`, `where?`,
  `criteria?`, `threshold` default 0.5) on `match` and `time_window` only.
- **Model**: `participants` (`id`, `type`, `anchor` with exactly one
  property, `leak` default 0.02), `hypotheses` (`id`, `label`, `prior`,
  `blurb`), `edges` (`from` signal/participant id or `from_type`, `to`
  hypothesis, `w`, `kind` support|inhibit). At most 9 per band; ids unique
  across bands.
- **Validation** must fail on: missing files, invalid JSON, unknown
  formats/transforms/operators/rule kinds, dangling references, bad time
  config, a source file that fails to parse as a whole, a document time that
  cannot be derived, more than 9 per band, no hypotheses, `entities` (the
  old key). It must warn on: mapped fields that never appear, time fields
  that never appear, unparseable or missing time values (with counts and
  an example), skipped rows, zero records, undated sources when others are
  timed, keys never mapped.

## 2. Parsing

Each source yields records in file order with a **pointer** and an
**excerpt** (the raw text of the record):

| format | pointer | notes |
|---|---|---|
| csv | `line N` (1-based physical line) | `delimiter` option; header row; values trimmed; a row with fewer fields than the header is skipped (`truncated row: k of n columns`), with more is skipped (`m extra column(s)`) |
| jsonl | `line N` | blank lines ignored; invalid JSON or a non-object is skipped with the reason |
| nested-json | `<records>[i]` | `records` is a dotted path; non-object items skipped |
| xml | `/<root>/<tag>[i]` (1-based) | `records` is an element path under the root; attributes flatten to `@name`, children to `child`, child attributes to `child.@name`, repeated keys keep the first |
| columnar | `<rows>[i]` | `fields` and `rows` paths; rows of the wrong length skipped |
| kv | `line N` | `pair_sep` (`;`), `kv_sep` (`=`); `#` comments and blank lines ignored; a line with no pair skipped |

Skipped rows are reported per source (`{pointer, reason, excerpt}`), never
fatal. A payload that fails as a whole (bad JSON document, malformed XML) is
a fetch/parse error for that source and leaves it unconnected.

Field access is by dotted path with exact-key precedence (so `window.@start`
resolves directly on flattened XML).

## 3. Normalisation and similarity

- `norm(v)`: lowercase, drop every character not in `[0-9a-z@._ -]`,
  collapse whitespace, trim. Used for key equality, conflict detection and
  anchors.
- `fuzzy_norm(v)`: lowercase, replace every character not in `[0-9a-z ]`
  with a space, collapse whitespace, trim. Used for similarity.
- **Similarity (allowed divergence).** Python: `difflib.SequenceMatcher(
  None, fuzzy_norm(a), fuzzy_norm(b)).ratio()`. Rust: Jaro-Winkler on the
  same normalised strings (standard definition, prefix scale 0.1, max prefix
  4). Both compare against the pack threshold. Packs pin the proposals that
  matter in `expect.json`; `elessar diff` reports the rest.
- **Candidate generation (allowed divergence).** Python compares all pairs
  of clusters of a type; Rust may block by shared normalised tokens or
  3-grams. Blocking must never miss a pair whose similarity would reach the
  threshold on today's packs (checked by the cross-check).

## 4. Resolution

- A **fragment** is one mapping applied to one record: `{type, source,
  record, props}` where each prop is `{value, source, pointer, field}`.
  Fragment id: `<source>#<record index>:<Type>:<mapping index>`.
- Fragments of the same type sharing a key property with equal `norm`
  value merge (transitively). Committed merges (pairs of fragment ids) also
  merge. A **cluster** id is the smallest fragment id in it (string order).
- Cluster properties: values in order of source rank (rack order) then
  fragment id; equal-`norm` values collapse into one value with several
  provenance entries. `conflicts` = properties with more than one value and
  not in the type's `multi`. `title` = the first title property present.
- **Proposals**: for each type with `fuzzy`, each pair of clusters, the best
  similarity over the configured properties at or above the threshold; id =
  the two source fragment ids sorted and joined with `|`; `conflict` = keys
  present on both clusters with no shared `norm` value; rejected ids are
  not re-proposed; committed ids are merged and listed with `active`
  (both fragments present).
- A participant **anchors** to the first cluster of its type with a
  property whose `norm` equals the anchor value; absent if none; resolved
  if the cluster has a title, else unresolved.

## 5. Signals

- A signal is **dormant** until every required source is connected
  (`source`, or every clause source for `co_resolved`), **quiet** when
  connected with no hits, **firing** otherwise.
- `match`: records matching `where`. `time_window`: matching records whose
  parsed time of day falls in `[start, end)` hours UTC. `count`: groups of
  matching records by the string form of `group_by`, size ≥ `min`; one hit
  per group carrying all its records. `co_resolved`: one hit per cluster of
  `params.type` that contains, for every clause, a fragment from that
  clause's source whose record matches the clause `where`.
- Activation `a = min(confidence of required sources) × strength ×
  judgement` (trust 1, discount 0.5, dismiss 0); 0 when quiet or dormant.
- A hit **implicates** the participants whose anchored cluster contains any
  fragment of any of its records, restricted to `implicates.types` if set.
  For participant `e` with `n_e` implicating hits, the signal → participant
  weight is `implicates.w × n_e / max_e n_e`.
- **Jev engine**: for each candidate record (the signal's `jev.where`, else
  all records of the source) the question is asked once (cached); records
  with probability ≥ `threshold` are hits; strength = the highest such
  probability. The engine toggle (`rules` | `jev`) chooses which engine's
  hits and strength drive activation; both are always reported.

## 6. Inference

Noisy-OR with inhibitors and competition. For a node with leak `λ`
(participants: `leak`; hypotheses: `prior`), support parents `(w_j, p_j)`
and inhibitors `(v_k, p_k)`:

```
support = 1 − (1 − λ) · Π_j (1 − w_j · p_j)
inhibit = Π_k (1 − v_k · p_k)
raw     = support · inhibit
```

Evaluation order (so both engines produce the same `stages`):
1. signals: activations;
2. participants: support from signal parents in pack signal order; no
   inhibitors; absent participants are 0;
3. hypotheses: support and inhibitor edges in `model.edges` order
   (`from_type` expands in `participants` order); then competition: if
   `Σ raw > 1`, each hypothesis's final = raw / Σ raw, else final = raw;
4. optional **relevance feedback** (off by default): each participant gets
   one extra support term (w = 0.15, p = max over hypotheses it supports of
   `p_H · w`), then steps 2–3 repeat.

`stages` is the list of node → p snapshots after each pass (signals,
participants, hypotheses raw, hypotheses final, and again after feedback).
Products are evaluated left to right in `f64`.

## 7. Statuses and board

- Thresholds: step 1 at p ≥ 0.33, step 2 at p ≥ 0.66; **crownable** at
  p ≥ 0.85; **captured** (explained away) when p ≤ 0.08 and
  `inhibit ≤ 0.5`; a captured hypothesis the user **kept** stays present.
- Participants: `absent` (no cluster), `unresolved` (cluster without
  title), `resolved`, `removed` (had a cluster earlier in the session, none
  now; `offboard.reason` names the sources it came from).
- Off-board reason for hypotheses: `Explained away at N%: <top three
  inhibitors with ≥ 5% cut>; … Competing explanations then divided what was
  left by D.` (text is part of the UI; keep the wording).
- Layout: board height 9; width `2 × largest band + 1`; hypotheses base row
  2 on odd columns, participants base row 5 on even columns, signals base
  row 8 on odd columns; columns spread as `cols[int((i + 0.5) × ncols / n)]`;
  codes `H1…`, `P1…`, `S1…` in pack order.

## 8. Events, undated records, links

- Each record of a connected source becomes an **event** if it has a time
  (`dated_by`: `record`, `document`, or `connect` under `at_connect`), else
  an **undated** entry with `why` (`no time config` or `no usable time
  value`). Id `<source>:<index>`; `participants` as in §5; `signals` = ids
  of signals with a hit containing the record; `severity` = max activation
  of those signals, else 0; `acked` from the session.
- Events sort by time, stable (source rank, then record order).
- **Links**: for every record whose fragments resolve into two or more
  participants, each pair (a < b by id) gets the record; `weight` = record
  count; sorted by weight desc, then ids.

## 9. Timeline

`timeline()` re-infers the board at each distinct event time `t` with only
records timestamped ≤ t taking part; undated records take part per policy
(`throughout`: every slice, listed in slice 0's `undated`; `after`: a final
extra slice with `undated_arrive: true`; `at_connect`: they are ordinary
events). Slice 0 (`t: null`) is "reference data only". If there are more
than 80 distinct times, 80 evenly spaced ones are kept (first and last
always) and each slice's `arrived` lists every event since the previous
kept time. Slice values: `signals {id: {p, status}}`, `participants {id:
{p, status, present}}`, `hypotheses {id: {p, status, present, support[],
inhibit[] (cut = w·p), raw, divisor}}`, rounded to 4 decimals (tie-breaking
is an allowed divergence). The last slice equals the live board. Slices
must not change session memory (participant source history) or merge
proposals.

## 10. Actions and insights

Actions (`POST /api/action`, `{scenario, action, ...}`): `connect`,
`disconnect`, `reconnect {sources}`, `judge {signal, judgement}`, `merge
{proposal}`, `reject {proposal}`, `unmerge {proposal}`, `keep {hypothesis}`,
`release {hypothesis}`, `crown {hypothesis}`, `feedback {on}`, `engine
{engine}`, `ack {event}`, `unack {event}`, `peek {source}`, `reset`,
`agent_decide {proposal, decision}`, `agent_reset`. Each appends an audit
entry `{at, action, detail}`; the response is `{state, events: [insight]}`
where insights are `{kind, title, text, points}` with kinds `shape_shock
first_link identity_resolved merge_committed fusion_signal inhibited
explained_away crowned crowned_premature crowned_contradicted full_fusion`,
each earned once per key (see `server/session.py` for keys and points).
`crown` records `{hypothesis, p, verdict: backed|premature|contradicted,
at, reasons}`.

## 11. Jev

- One request per record carrying every opted-in question of that source
  whose `jev.where` matches, keyed by the signal id with `-` → `_`:
  `{"model", "state": {"source": <source name>, "record": <record data>},
  "questions": {<key>: {"type": "noul", "instructions", "criteria"?}}}`.
- Cache file name: `sha256(canonical)[:32] + ".json"` where `canonical` is
  the request serialised as Python `json.dumps(payload, sort_keys=True)`:
  keys sorted, separators `", "` and `": "`, floats in shortest round-trip
  form, ints as ints, non-ASCII escaped as `\uXXXX`. Both engines must
  produce this byte string so cached answers are shared.
- Cache entry: `{payload, answers, model, ms, cost, at}`; legacy entries
  lack `payload`.
- The inspector lists every ask the pack can make (connected or not) with
  cache status; `forget` and `reask` operate per signal.

## 12. API v1 (shapes the frontend reads)

`GET /api/scenarios` → `{default, scenarios: [{id, title, premise, sources,
pieces, warnings}], invalid: {dir: [errors]}}`.

`GET /api/state?scenario=` → `{scenario {id, title, premise, board {width,
height}, narrative}, sources[], nodes[], edges[], stages[], merges {pending[],
committed[]}, events[], undated[], unknown_time, links[], object_types,
feedback, engine, jev {available, model, calls, cached, errors, cost},
crown, score, earned[], audit[], thresholds, agent {available, model,
transcript[], proposals{}}}`.

- source: `{id, name, kind, format, transport, color, confidence, blurb,
  connected, latency_ms, bytes, error, records, fragments, skipped,
  skipped_examples[]}`
- node (all): `{id, code, band, label, col, base_row, p, step, present,
  status, detail}`; participants add `offboard`; hypotheses add `kept,
  kept_reason, offboard, crowned, leading`.
- signal detail: `{status, sources[], kind, rule, explain, confidence,
  strength, engine_used, rule_strength, rule_hit_count, jev, judgement,
  hits[{records[{source, pointer, excerpt}], participants[]}],
  links[{participant, hits, w}], implicates}`.
- participant detail: `{type, anchor, breakdown, title, props {name:
  [{value, provenance[{source, pointer, field, fragment, excerpt}]}]},
  conflicts[], sources[], fragment_count, merges[]}`.
- hypothesis detail: `{blurb, prior, breakdown {leak, support[{from, w, p,
  term}], inhibit[{from, w, p, factor}], support_p, inhibit_factor, raw,
  divisor, final}}`.
- edge: `{from, to, kind, w, flow}`; pending merge: `{id, type, score, a
  {cluster, property, value, source, fragment}, b {…}, conflict[]}`;
  event / undated / link as in §8.

`GET /api/timeline?scenario=` → `{slices[], final, event_count,
undated_count, unknown_time}` (§9). `GET /api/jev/cache?scenario=` and
`POST /api/jev/cache` (§11). `POST /api/agent {scenario, message,
selected}` → `{turn, state}`. `GET /mock/<pack>/<source>` serves the raw
payload with its content type after the declared latency.

**Scale cap (v1).** When a pack has more than 5,000 events, `state.events`
carries the newest 5,000 and `state.events_truncated: true`; v2 endpoints
provide the rest.

## 13. Allowed divergences, summary

Similarity measure and candidate generation (§3); 4-decimal tie-breaking in
timeline slices (§9); the exact set of fuzzy proposals outside those pinned
by `expect.json`; audit text wording for engine-specific details (e.g.
timing). Everything else is shared behaviour, and the packs' `expect.json`
plus `elessar diff` are how it is checked.

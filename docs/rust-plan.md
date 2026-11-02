# ELESSAR in Rust: a second engine, designed from scratch

Decision: the Rust engine is **not** a port of the Python one. It is a
second implementation of the same product, designed for scale from the
first line, sharing with the Python app only what users touch: the frontend,
the scenario packs, and the behaviour of the stories. It runs side by side
with the Python app on its own port; the Python app is not replaced.

## What is shared, and what is free

| Shared (contract) | Free to redesign |
|---|---|
| `web/` as it is: the look, the sprites, the modes | every data structure and algorithm |
| the pack format (`manifest`, `mapping`, `signals`, `model`, `narrative`, `sources/`) | how packs are loaded, indexed and evaluated |
| the JSON shapes the frontend reads (`/api/state`, `/api/timeline`, `/api/action`, `/api/agent`, `/api/jev/cache`, `/mock/...`) | internal representations; extra endpoints |
| the *meaning* of the numbers: noisy-OR with inhibitors and competition, thresholds 0.33 / 0.66 / 0.85 / 0.08, judgement multipliers, undated policies | numeric details: similarity measure, rounding, ordering ties |
| the stories: which hypothesis ends crownable, which are explained away, which merges are proposed, what the validator flags | the exact decimals |

"Identical look" is guaranteed by the first row: the Rust server serves the
same files. "Same stories" is guaranteed by acceptance tests, below.

## How parity is judged without goldens

Byte-for-byte goldens against Python are dropped. Three kinds of tests
replace them:

1. **Acceptance specs per pack**, in the pack itself: a new optional file
   `scenarios/<id>/expect.json` states what must be true once every source
   is connected and the planted merges are decided, for example:
   `{"crownable": "misconfig", "captured": ["planned", "dependency", "unsanctioned"], "proposals": [{"a": "Nova Goods", "b": "Nova Goods Co", "decision": "reject"}], "undated": 20, "skipped": {"orders": 1, "payments": 1, "chargebacks": 1}, "signals_firing": [...], "order_independent": true}`.
   Both engines are tested against it (a small Python runner too), so the
   packs become the executable definition of correct behaviour.
2. **Property tests** on the Rust engine: connection order never changes the
   final board; disconnect then reconnect is the identity; merge then revert
   is the identity; every belief in [0, 1]; timeline's last slice equals the
   live board; thinning never drops the first or last slice; parsers never
   panic on arbitrary bytes (fuzzing).
3. **An informational cross-check**: a script runs both engines on the three
   packs and prints every difference in beliefs, statuses, proposals and
   counts. It does not fail the build; it is read at review time so
   divergences are chosen, not accidental.

## The engine, designed for 10⁶ records

### Data model

- **Columnar record store per source.** Each parsed source becomes typed
  columns (string, number, timestamp, bool, missing), with interned strings
  and a parallel column of row pointers and byte offsets into the retained
  raw text. Excerpts are slices of the raw text, never copies.
- **Fragments are not materialised.** A mapping compiles to a projection
  (source, row) → (type, key columns, property columns). Resolution and
  property lookup read through the projection.
- **Entities** are union-find sets over (source, row, mapping) with a
  stable entity id (smallest member, as today), provenance per property
  value, and conflict detection on demand.

### Entity resolution

- **Exact keys** via hash indexes built on connect; incremental merges as
  sources arrive; a rebuild from the remaining sources on disconnect.
- **Fuzzy matching with blocking**: candidates from shared normalised tokens
  or 3-grams, scored with **Jaro-Winkler** on normalised names (defined by a
  published algorithm, so the score is reproducible in any language; the
  pack threshold keeps its meaning, 0.86 stays 0.86). Proposals, commits,
  rejections and reverts keep today's semantics.
- Proposals whose exact keys disagree are flagged, as today.

### Rules and inference

- Pack rules compile to **plans over the indexes**: `match` and
  `time_window` are column filters; `count` is a group index; `co_resolved`
  is one pass over entities. The same four rule kinds, the same condition
  operators, so existing packs load unchanged.
- **Jev** stays a pluggable evaluator per signal with the same cache
  semantics (model + record + question → answer on disk). The canonical
  JSON for the cache key is defined once, in a spec, and shared by both
  engines so answers are reusable across them (one-time refetch is
  acceptable if that proves awkward; it costs a cent).
- **Inference** is the product's math and is kept exactly, with a fixed
  evaluation order written down in the spec (`docs/engine-spec.md`), and
  decimal rounding only at the API boundary.

### Timeline

A single **sweep**: records sorted by time, each slice adds its records to
the indexes and union-find, re-evaluates affected rules and reruns
inference. O(records log records + slices × small). Thinning to ≤ 80
slices is a presentation rule and is applied to the output, not to the
sweep. Undated policies (`throughout`, `at_connect`, `after`) keep their
semantics.

### API: v1 for the current frontend, v2 for scale

- **v1** (`/api/state` etc.) is served with the shapes the frontend reads
  today, so `web/` runs unchanged against either engine.
- **v2** adds what scale needs and v1 cannot carry: paginated and filtered
  events (`/api/v2/events?from=&to=&participant=&severity>=&page=`),
  excerpts on demand (`/api/v2/record/<source>/<pointer>`), progress for
  long connects (server-sent events), and an aggregated event histogram for
  the replay scrubber. v1's `state.events` is capped (newest N plus a
  `truncated: true` flag) once a pack exceeds the cap, so the frontend keeps
  working while v2 is adopted.
- The frontend adopts v2 by **feature detection** (`/api/capabilities`), in
  small, look-preserving changes to `panel.js`/`topology.js` that keep
  working against the Python server (which simply lacks v2).

### Server

`axum` on `tokio`; the engine itself is synchronous and runs on a blocking
pool; one session per pack as today; static files from `web/`; `/mock/...`
with the declared latency; `.env` loading; the same CLI flags
(`--scenario`, `--port`, `--check`). One binary, no runtime to install,
which also removes the Python 3.9 dependency for the Rust path.

### Crate layout

```
rust/
  Cargo.toml                 workspace
  elessar-core/              pack, store, resolve, rules, infer, session, timeline, jevcache
  elessar-server/            axum: v1 + v2 routes, mocks, static, Jev and agent clients
  elessar-bench/             scaled synthetic packs and criterion benchmarks
  elessar-cli/               `elessar check`, `elessar run`, `elessar diff` (the cross-check)
```

Dependencies: `serde`, `serde_json`, `regex`, `csv`, `quick-xml`,
`chrono`, `rayon` (parallel parsing and sweeps), `axum`, `tokio`,
`reqwest` (Jev, agent), `proptest` and `criterion` (dev).

## Scale targets

Measured on `elessar-bench` packs generated by `scenarios/fraud-ring/generate.py --scale N` (same story and dirt proportions, N× the buyers, orders, sessions and tickets):

| Records | connect all | one action | full timeline | memory |
|---|---|---|---|---|
| 10³ | < 50 ms | < 10 ms | < 100 ms | tiny |
| 10⁵ | < 1 s | < 100 ms | < 1 s | < 100 MB |
| 10⁶ | < 10 s | < 300 ms | < 5 s | < 500 MB |

## Steps

| Step | Deliverable | Done when |
|---|---|---|
| 0 | Rust toolchain (`rustup`), workspace skeleton | `cargo build` passes on an empty workspace |
| 1 | `docs/engine-spec.md`: the contract above written precisely (API v1 shapes from live Python responses, inference evaluation order, similarity and normalisation definitions, undated policies, cache key canonicalisation); `expect.json` for the three packs; a Python runner that checks the Python engine against them | the Python engine passes all three `expect.json` |
| 2 | `elessar-core`: store, pack loader and validator, parsers, resolution, rules, inference, session, timeline; acceptance, property and fuzz tests; `elessar diff` cross-check | all three packs pass `expect.json` in Rust; property tests green; the diff report reviewed and its divergences accepted |
| 3 | `elessar-server`: v1 routes, mocks, static, Jev and agent clients; runs on :8778 beside Python on :8777 | the unchanged `web/` works fully against :8778 on all three packs, all four modes, checked in the browser |
| 4 | API v2 + feature-detected frontend adoption; v1 event cap | fraud-ring `--scale 100` (~10⁵ records) is usable in every mode |
| 5 | `elessar-bench` and `docs/rust-bench.md` | targets above measured and recorded for 10³, 10⁵, 10⁶ |

Steps 1 and 2 are this plan's commitment; 3–5 are listed so that 1–2 are
designed with them in mind. Step 2 is split across sessions by module once
the spec and the core types exist (parsers, resolution, rules+inference,
session+timeline), each with its own tests.

## Risks and choices made

- **Similarity changes merge scores.** Jaro-Winkler and difflib disagree
  on some pairs, so a proposal present in one engine may be absent in the
  other. The acceptance specs name the proposals that matter (the planted
  ones); the cross-check surfaces the rest for review. Pack thresholds may
  need a nudge, which is a pack change, applied for both engines.
- **Two engines to keep in step.** Every pack change must pass
  `expect.json` on both. The spec is the arbiter when they disagree.
- **The frontend contract is informal today.** Step 1 makes it formal, which
  is also where any v1 shape that cannot scale gets a documented cap.

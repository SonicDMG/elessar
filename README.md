# ELESSAR

A local, Palantir-inspired demo. Several differently-shaped data sources are
fused into one view: a pixel-art checkers board whose pieces are nodes in a
Bayesian network. You connect sources one at a time, judge the evidence, decide
which records describe the same thing, and crown the explanation you believe.

Not a copy of anything. It borrows five ideas from Palantir's platforms (see
[docs/palantir-research.md](docs/palantir-research.md)) and makes them
something you can watch happen.

## Run it

Python 3.9+ standard library only. No install, no build.

```bash
./run.sh
```

Then open http://127.0.0.1:8777.

```bash
./run.sh --scenario supply-chain   # start on another pack
./run.sh --check                   # validate every pack and exit
./run.sh --port 9000
```

The scenario dropdown switches packs live, and `?scenario=<id>` in the URL
picks one.

## Playing

| Verb | How | What it teaches |
|---|---|---|
| **Connect** | source rack, or keys `1`–`9` | Each source is a real HTTP fetch of a payload in its own wire format |
| **Peek raw** | source rack | What the service actually sends, before any fusion |
| **Judge** | click a signal → trust / discount / dismiss | Posteriors depend on how much you trust the evidence |
| **Merge** | Merges tab → commit / reject / revert | Fuzzy identity is a proposal. You decide, and you can undo it |
| **Crown** | click a hypothesis → crown | Your call; the board tells you whether the posterior backs it |
| **Inspect** | click any piece | Every property value with its source, raw line, and the belief derivation |

There is no way to lose. Insights score points and each one explains the
concept that just fired. Every action lands in the **Log** tab, the audit trail.

### Reading the board

- **Bands**, bottom to top: signals (amber), objects (cyan), hypotheses (magenta).
- **Belief moves a piece** up its lane in legal diagonal checkers moves: one
  square at 33%, two at 66%. The eight pips on a piece's edge are its belief
  in eighths.
- **Grey**: an object no source has named yet, or a signal whose sources are
  connected but which matched nothing. **Hollow**: a signal whose sources are
  not connected.
- **Dotted lines** march toward the node they support. **Red dots** inhibit.
- **Gold halo**: a hypothesis at 85% or more is crownable. A crowned
  hypothesis becomes a king (a stacked double piece).
- **Jumped off the board**: a hypothesis at 8% or less, with inhibitors having
  cut it by at least half, is explained away.
- **Sparks** fly from a source's side of the board into an object when that
  source adds facts to it. A **white flash** means an object just got a name.

### The two packs

- **change-cascade** (default, 6 sources, 19 pieces). Payments failed for 25
  minutes. The obvious answer (a planned change) and the scary one
  (unsanctioned activity) are both wrong. Connected left to right, each
  source flips the lead.
- **supply-chain** (4 sources, 12 pieces). A shipment is late. Here the fuzzy
  merge is load-bearing: the supplier is only cleared once you merge the
  carrier's name for it with the supplier master's.

Both reach the same final board whatever order you connect sources in.

## How it works

```
 mock services (HTTP)      pipeline                      inference         board
 ─────────────────────     ───────────────────────────   ───────────────   ─────────
 csv  jsonl  nested-json → parse → map to fragments →   signals →         pieces,
 xml  columnar  kv          (every value keeps source,   objects →         moves,
                            pointer, raw excerpt)        hypotheses        edges
                         → resolve (keys auto-merge,     (noisy-OR +
                            fuzzy = proposals)            inhibitors +
                         → evaluate signal rules          competition)
```

1. **Connect.** The server fetches `/mock/<pack>/<source>` from itself over
   real HTTP (hence `ThreadingHTTPServer`), or a real URL for
   `transport: "http"` sources.
2. **Parse** into records, each with a raw pointer (line, XPath, JSON path,
   row) and the raw excerpt.
3. **Map** records to typed *fragments*. Every property value carries
   `{value, source, pointer, field}`.
4. **Resolve** fragments into objects. Shared key values merge automatically.
   Fuzzy matches become proposals; committed merges are reversible.
5. **Anchor** the model's objects to resolved clusters.
6. **Signals** fire from rules. A signal implicates an object only if one of
   its hits resolves into that object, so links emerge from resolution and
   are never hand-wired. Weight scales with how many hits link.
7. **Infer:**

   ```
   signal   a = min(source confidence) × strength × judgement
   node     support = 1 − (1 − leak) · Π(1 − w·p)      leak = prior for hypotheses
            p = support · Π(1 − v·p_inhibitor)
   compete  if Σ p_hypotheses > 1, divide each by the sum
   ```

   This is an **approximation**, not exact belief propagation. The
   **relevance feedback** toggle adds a non-Bayesian top-down pass, off by
   default, to show what it distorts.

Every action recomputes everything from a small session state (connected
sources, merge decisions, judgements, crown), so disconnects and reverted
merges are never special cases.

## Writing a scenario pack

A pack is a directory under `scenarios/`. The engine never knows the domain;
swapping data means swapping the directory. Run `./run.sh --check` after every
edit. It reports dangling references, unknown formats, transforms, operators
and rule kinds, and warns about mapped fields that never appear in the data.

```
scenarios/<id>/
  manifest.json    sources
  mapping.json     object types and field mappings
  signals.json     rules
  model.json       objects, hypotheses, edges
  narrative.json   intro, finale, insight text overrides
  sources/         the raw payloads, served byte for byte
```

### manifest.json

```json
{
  "id": "my-pack", "title": "…", "premise": "…",
  "sources": [
    {"id": "roster", "name": "People Directory", "kind": "Directory", "format": "csv",
     "file": "sources/roster.csv", "options": {"delimiter": "|"},
     "confidence": 0.95, "latency_ms": 200, "color": "#7ee787", "blurb": "…"},
    {"id": "live", "format": "jsonl", "transport": "http", "url": "https://example.com/events"}
  ]
}
```

Rack order is manifest order, and so is source priority when objects show
property values.

| format | options |
|---|---|
| `csv` | `delimiter` (default `,`) |
| `jsonl` | — |
| `nested-json` | `records`: dotted path to the array, e.g. `data.flows` |
| `xml` | `records`: element path, e.g. `ticket`. Attributes become `@attr`, children `child`, child attributes `child.@attr` |
| `columnar` | `fields` (default `schema.fields`), `rows` (default `rows`) |
| `kv` | `pair_sep` (default `;`), `kv_sep` (default `=`); `#` lines are comments |

### mapping.json

```json
{
  "object_types": {
    "Actor": {"title": "name", "keys": ["badge", "email"], "multi": ["last_seen"],
              "fuzzy": {"properties": ["name"], "threshold": 0.86}}
  },
  "mappings": [
    {"source": "roster", "type": "Actor", "props": {
      "name": {"from": "FULL_NAME", "transform": "name_last_first"},
      "email": {"from": "EMAIL", "transform": "lower"},
      "badge": "BADGE_ID"}},
    {"source": "advisory", "type": "Actor", "when": {"type": "PERSON"}, "props": {"name": "subject"}}
  ]
}
```

- `title`: the property (or ordered list) that names an object. No title means
  the object is drawn grey as **unresolved**.
- `keys`: equal values (case- and punctuation-insensitive) merge automatically.
- `fuzzy`: compares these properties across objects and proposes merges at or
  above `threshold`. Proposals whose key values disagree are flagged.
- `multi`: properties expected to hold several values, so they are not
  flagged as conflicts.
- Transforms: `lower`, `upper`, `strip`, `title`, `int`, `float`,
  `name_last_first`, `date_dmy`, `epoch_ms`, `epoch_s`. A list applies in order.

### Conditions (`where`, `when`)

A field path mapped to a literal (equality) or an operator object:
`eq ne gt gte lt lte in nin contains startswith exists`.

```json
{"src.env": "prod", "stats.rttMs": {"lt": 100}, "verb": {"in": ["DEPLOY", "ROLLBACK"]}}
```

### signals.json

| kind | fires for | parameters |
|---|---|---|
| `match` | each record in `source` matching `where` | — |
| `time_window` | matching records whose time is within hours `[start, end)` UTC (wraps midnight) | `params.field`, `params.unit` (`ms`, `s`, `iso`), `params.hours` |
| `count` | each group of matching records at least `min` in size | `params.group_by`, `params.min` |
| `co_resolved` | each resolved object with fragments satisfying every clause | `params.type`, `params.clauses: [{source, where}]` |

`co_resolved` is the fusion-only rule: its clauses usually name different
sources, so it cannot fire until resolution has joined them into one object.

```json
{"id": "prod-to-staging", "label": "Prod traffic to staging", "kind": "match", "source": "flow",
 "where": {"src.env": "prod", "dst.env": "staging"},
 "strength": 0.95,
 "implicates": {"types": ["Service"], "w": 0.8},
 "explain": "…"}
```

`strength` is a number, or `{"field": "confidence"}` to take the highest value
of that field across the hits. `implicates.types` limits which object types
the signal can link to (default: any).

### model.json

```json
{
  "entities": [{"id": "vale", "type": "Actor", "anchor": {"badge": "B-2093"}}],
  "hypotheses": [{"id": "planned", "label": "Planned change", "prior": 0.3, "blurb": "…"}],
  "edges": [
    {"from": "approved-change", "to": "planned", "w": 0.8},
    {"from": "prod-to-staging", "to": "planned", "w": 0.95, "kind": "inhibit"},
    {"from_type": "Actor", "to": "unsanctioned", "w": 0.35}
  ]
}
```

- An entity appears on the board once a connected source produces an object
  of its `type` with the `anchor` value.
- Edges go to hypotheses only, from signals or entities, or from every entity
  of a type with `from_type`. Signal → object links come from resolution. So
  the graph is acyclic by construction.
- At most 9 per band. Board width is `2 × largest band + 1`.

### narrative.json

```json
{"intro": "…", "finale": "…",
 "insights": {"explained_away": {"title": "Ruled out: {label}", "text": "…"}}}
```

Insight keys: `shape_shock`, `first_link`, `identity_resolved`,
`merge_committed`, `fusion_signal`, `inhibited`, `explained_away`, `crowned`,
`crowned_premature`, `crowned_contradicted`, `full_fusion`.

### Tuning a story

Weights are the story. A workable loop: connect sources in your intended
order from a script with `server.session.Session` (override `fetch` to read
files), print hypothesis beliefs after each step, and adjust. Losers need
inhibitors strong enough to reach 8% *and* be cut by half; the winner needs to
clear 85% after competition.

## Files

```
run.sh                  start script
server/app.py           HTTP server, mock services, API, --check
server/scenario.py      pack loader, validator, board layout
server/pipeline.py      six parsers, transforms, field → fragment mapping
server/resolve.py       entity resolution, fuzzy proposals, anchoring
server/signals.py       rule kinds
server/bayes.py         noisy-OR, inhibitors, competition, stages
server/session.py       session state, recompute, actions, insights, audit
web/index.html          page shell
web/sprites.js          16×16 sprites, 3×5 pixel font, board tiles
web/board.js            canvas board, moves, edges, sparks, hit testing
web/panel.js            object view, merges, log, source rack, peek, toasts
web/app.js              wiring
web/style.css           dark CRT theme
scenarios/              change-cascade, supply-chain
docs/                   Palantir research notes, design
```

## Limits

- Inference is approximate (noisy-OR with competition), not exact.
- One in-memory session per pack. State resets when the server restarts.
- Bound to 127.0.0.1 with no authentication. It's a local demo.
- The rule kinds are fixed. A new kind is about 20 lines in `server/signals.py`.
- Fonts load from Google Fonts. Offline, the page falls back to system monospace.

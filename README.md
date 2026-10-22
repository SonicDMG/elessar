# ELESSAR

A local demo. Several differently-shaped data sources are
fused into one view: a pixel-art checkers board whose pieces are nodes in a
Bayesian network. You connect sources one at a time, judge the evidence, decide
which records describe the same thing, and crown the explanation you believe.

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

### Optional: models (Jev and the agent)

Copy `.env.example` to `.env` (git-ignored) and add an OpenRouter key:

```
OPENROUTER_API_KEY=sk-or-...
ELESSAR_DECISION_MODEL=jev-latest                    # Jev, the evidence engine
ELESSAR_INFERENCE_MODEL=anthropic/claude-sonnet-5.5  # the agent
```

The key turns on two things:

- **[Jev](https://typesafe.ai/blog/introducing-system-one-models-and-jev)**,
  TypeSafe's System One decision model, judges every connected record. The
  header's **evidence: rules | jev** toggle chooses which engine drives the
  board. See [Evidence engines](#evidence-engines-rules-and-jev).
- **The agent** (the side panel's **Agent** tab) explains the board and
  proposes next steps. See [The agent](#the-agent).

Without a key everything else works on rules alone.

## Playing

| Verb | How | What it teaches |
|---|---|---|
| **Connect** | source rack, or keys `1`–`9` | Each source is a real HTTP fetch of a payload in its own wire format |
| **Peek raw** | source rack | What the service actually sends, before any fusion |
| **Judge** | click a signal → trust / discount / dismiss | Posteriors depend on how much you trust the evidence |
| **Merge** | Merges tab → commit / reject / revert | Fuzzy identity is a proposal. You decide, and you can undo it |
| **Crown** | click a hypothesis → crown | Your call; the board tells you whether the posterior backs it |
| **Return to board** | the Off the board column, left of the board | Being explained away is the model's opinion, not a deletion; you can overrule it |
| **Inspect** | click any piece | Every property value with its source, raw line, and the belief derivation |

There is no way to lose. Insights score points and each one explains the
concept that just fired. Every action lands in the **Log** tab, the audit trail.

### Reading the board

- **Bands**, bottom to top: signals (amber), participants (cyan), hypotheses (magenta).
- **Belief moves a piece** up its lane in legal diagonal checkers moves: one
  square at 33%, two at 66%. The eight pips on a piece's edge are its belief
  in eighths.
- **Grey**: a participant no source has named yet, or a signal whose sources are
  connected but which matched nothing. **Hollow**: a signal whose sources are
  not connected.
- **Dotted lines** march toward the node they support. **Red dots** inhibit.
- **Gold halo**: a hypothesis at 85% or more is crownable. A crowned
  hypothesis becomes a king (a stacked double piece).
- **Off the board**, the column left of the board: nothing just disappears.
  A hypothesis at 8% or less, with inhibitors having cut it by at least half,
  hops off the board's left edge as explained away. A participant does the same
  when every source describing it is disconnected. In the column, each piece
  sits next to the reason it left, and **return to board** always brings it
  back: a hypothesis is kept on the
  board as your call (marked with a pin, belief unchanged, release any time),
  and a participant comes back by reconnecting its sources.
- **Sparks** fly from a source's side of the board into a participant when
  that source adds facts to it. A **white flash** means a participant just got
  a name.

### The two packs

- **change-cascade** (default, 6 sources, 19 pieces). Payments failed for 25
  minutes. The obvious answer (a planned change) and the scary one
  (unsanctioned activity) are both wrong. Connected left to right, each
  source flips the lead, and each wrong explanation moves off the board
  with the evidence that ruled it out.
- **supply-chain** (4 sources, 12 pieces). A shipment is late. Here the fuzzy
  merge is load-bearing: the supplier is only cleared once you merge the
  carrier's name for it with the supplier master's.

- **fraud-ring** (8 sources, ~1,000 records, 22 pieces). A marketplace's
  chargebacks triple in a week. The sources are deliberately dirty:
  timestamps in the wrong format or missing, a malformed JSON line, a
  truncated CSV row, placeholder values, duplicates, a collector's clock an
  hour off, name variants, and a fuzzy merge proposal that is wrong. Most
  records are ordinary business; the rules have to find the needles. The
  pack is produced by `scenarios/fraud-ring/generate.py` (seeded, so the
  committed files are reproducible); the dirt it plants is listed at the
  bottom of that file. The source cards show rows that could not be parsed
  and `./run.sh --check` lists every piece of dirt it can detect.

All three reach the same final board whatever order you connect sources in.

## Modes: Board, Life, Topology, Landscape

The strip above the center area switches between four views of the same
session; the side panel, source rack and header stay put, and selecting a
piece in any view selects it everywhere. `?mode=life`, `?mode=topology` or `?mode=landscape`
in the URL opens one directly.

- **Board** is the checkers view described above.
- **Life** is Conway's Game of Life with each hypothesis as a species. It
  follows the evidence in time: the board is re-inferred at every event
  timestamp (`GET /api/timeline`), and each slice plants cells for what each
  hypothesis's support terms gained (signals and participants, `w × p`),
  while inhibitors remove their cut; then Conway runs for a few generations
  before the next slice. Evidence cells carry a pixel pattern per signal or
  participant (hover for which one, and when); plain cells were born by
  Conway. Hypotheses that share evidence sit in adjacent bands and shared
  terms seed on the border between them. Territory share is shown beside
  the board's belief with a one-line verdict, and each species lists the
  evidence that planted or cut it. Generation 0 is data; later generations
  are what Conway makes of it, so read the lapse as how evidence landed and
  compounded, not as a second inference engine.
- **Topology** is a NetView-style map: a root submap of sources colored by
  the worst status of their participants, drill-down to participants with
  typed device icons and links from shared records, and a **Hypotheses**
  submap with green support and red inhibition edges where explained-away
  hypotheses stay visible with what killed them. An event browser lists
  every timestamped record (and an Undated section for the rest) with
  severity, source, participants and ack; the replay scrubber steps through
  the timeline slices, so statuses at any moment come from real inference
  on the records that had arrived by then.

- **Landscape** is a spike map on a surface plot, read as a fitness
  landscape: hypotheses are lanes on an isometric floor (ordered and
  coloured as in Life), evidence rows run across them, every support term is
  a spike of height `w × p` and every inhibitor a red pit below the floor. A
  smoothed surface rises over the spikes with contour lines, so the
  best-backed hypothesis is the highest ridge and an explained-away one sits
  in a trough. Rotate in four stops, zoom, hover for the exact product,
  click to select, and scrub the same timeline as the other modes to watch
  the relief form as evidence lands.

See `docs/modes.md` for the mode interface and data shapes.

## How it works

```
 mock services (HTTP)      pipeline                      inference         board
 ─────────────────────     ───────────────────────────   ───────────────   ─────────
 csv  jsonl  nested-json → parse → map to fragments →   signals →         pieces,
 xml  columnar  kv          (every value keeps source,   participants →    moves,
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
5. **Anchor** the model's participants to resolved objects.
6. **Signals** fire from rules. A signal implicates a participant only if one
   of its hits resolves into that participant, so links emerge from resolution and
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

## Evidence engines: rules and Jev

Every signal is a question about records. Two engines can answer it:

- **Rules** answer with exact field conditions (`where`) and a strength set
  in the pack, or read from a field like the advisory's own `confidence`.
- **Jev** answers a plain-language yes/no question about each record with a
  calibrated probability. Records at or above the threshold (0.5) are hits,
  and the signal's strength is the highest probability among them.

Both always run. The toggle only decides which one drives the board, so
you can flip between them and watch the beliefs move. Click a signal to see
them side by side: the rule's hits and strength next to Jev's question and
its probability for every record, with disagreements flagged. In
`change-cascade` the instructive one is *Person flagged externally*. The
rule trusts the tip's self-reported 0.41; Jev reads "partner SOC tip,
unverified" and says 0.17.

How it runs:

- One request per record carrying every Jev question for that record's
  source, all records in parallel, through OpenRouter's System One API
  (`POST https://openrouter.ai/api/v1/systemone`). `ELESSAR_DECISION_MODEL`
  picks the model (default `jev-latest`).
- Answers are cached in `.cache/jev/` (git-ignored) by model, record and
  question, so replays are instant, free and repeatable. A Jev answer
  depends only on those three things, so an unchanged pack never needs a
  new call; that also means a demo on a warm cache shows no traffic on
  OpenRouter. The **cache** link in the header opens an inspector listing
  every ask the pack can make, with its question, criteria, the records it
  covers, the cached answer, when it was given and by which model version.
  From there you can forget a signal's answers, re-ask them live, drop
  orphaned entries (questions no pack asks any more), or clear everything.
  An entry is stale when the question text, the record, or the model
  version changed; the inspector flags answers from an older version than
  the newest it has seen.
- Calls, cache hits, cost and latency show in the header and the Log tab.
  A full `change-cascade` run is 17 calls, about 300–400 ms each, costing
  about $0.0003 in total.
- Structural rules (`co_resolved`) and exact clock rules stay rule-only.

## The agent

The agent reads the live board and answers questions about it in the
panel's **Agent** tab: why a hypothesis leads, what a signal
means, where rules and Jev disagree, and what to connect next.

- **Context.** Every message carries a fresh snapshot: each signal (rule and
  Jev strengths), participant, hypothesis and its derivation, anything off
  the board and why, pending merges, recent log entries, and the piece
  you have selected.
- **Tools.** It can open any piece, read raw source records, run **what-ifs**
  on a throwaway copy of the board ("what if I dismissed S4?"), ask Jev new
  yes/no questions about specific records, and **highlight** pieces on the
  board.
- **It proposes; you decide.** The agent cannot change the board. When an
  action would help it adds an approve/decline card. Approving runs the
  action and logs it as the agent's proposal that you approved, and the
  next reply knows what you chose.
- Piece codes in its replies are links; clicking one selects the piece.

It calls the inference model through OpenRouter's Anthropic Messages API
(`POST https://openrouter.ai/api/v1/messages`), with tool use, adaptive
thinking at medium effort, and the instructions prompt-cached. A typical
answer takes 7–12 seconds and costs about $0.04; each reply shows its own
time and cost.

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
  model.json       participants, hypotheses, edges
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
     "time": {"field": "START_DATE", "unit": "date_dmy"},
     "confidence": 0.95, "latency_ms": 200, "color": "#7ee787", "blurb": "…"},
    {"id": "live", "format": "jsonl", "transport": "http", "url": "https://example.com/events"}
  ]
}
```

Rack order is manifest order, and so is source priority when participants show
property values.

`time` is optional and says where each record's timestamp is, for the event
timeline (`state.events`). `field` is a dotted path, or a list of paths tried
in order (`["opened", "window.@start"]`); `unit` is `ms` or `s` (epoch), `iso`
(`YYYY-MM-DDTHH:MM:SSZ`) or `date_dmy` (`dd/mm/yyyy`, read as midnight UTC).
Records whose field is empty produce no event; a source without `time`
produces none. The validator rejects an unknown unit and warns when a path
never appears in the data or a value does not parse.

Timestamps make a source part of the time-lapse (Life) and replay
(Topology). Per record: `"time": {"field": "t", "unit": "ms"}` (`field` may
be a list of paths tried in order; units `ms`, `s`, `iso`, `date_dmy`). For a
source dated as a whole, such as a bulletin or an export with a header date,
use `"time": {"document": {...}, "unit": "iso"}` where `document` is
`{"constant": "..."}`, `{"regex": "..."}` (group 1 over the raw text) or
`{"path": "..."}` (a root path for nested-json/columnar, `@attr` or a child
path on an XML root). Records with no time config, an empty value, or a
value that does not parse are **undated**; `--check` reports them. The
manifest's top-level `unknown_time` says how undated records take part in
the time-lapse: `throughout` (reference data, present in every slice; the
default), `at_connect` (dated when their source was connected) or `after`
(arrive in one final slice after every timed record). Undated records are
listed in `state.undated` so views can label them.

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
  its participant is drawn grey as **unresolved**.
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

Add a `jev` block to let Jev judge the same signal (`match` and
`time_window` kinds only):

```json
"jev": {
  "question": "Does this flow show a production service calling a non-production dependency?",
  "where": {"src.env": "prod"},
  "criteria": {"true": "Source is production, destination is staging or another non-production environment.",
               "false": "Both ends are production."},
  "threshold": 0.5
}
```

`where` is an optional cheap pre-filter that picks which records Jev is asked
about; `criteria` describes what yes and no mean. Jev sees the record plus
the source's name.

### model.json

```json
{
  "participants": [{"id": "vale", "type": "Actor", "anchor": {"badge": "B-2093"}}],
  "hypotheses": [{"id": "planned", "label": "Planned change", "prior": 0.3, "blurb": "…"}],
  "edges": [
    {"from": "approved-change", "to": "planned", "w": 0.8},
    {"from": "prod-to-staging", "to": "planned", "w": 0.95, "kind": "inhibit"},
    {"from_type": "Actor", "to": "unsanctioned", "w": 0.35}
  ]
}
```

- A participant appears on the board once a connected source produces an object
  of its `type` with the `anchor` value.
- Edges go to hypotheses only, from signals or participants, or from every participant
  of a type with `from_type`. Signal → participant links come from resolution. So
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
server/jev.py           Jev client: OpenRouter System One API, parallel calls, disk cache
server/agent.py         the agent: board snapshot, tools, tool loop, proposals
web/index.html          page shell
web/sprites.js          16×16 sprites, 3×5 pixel font, board tiles
web/board.js            canvas board, moves, edges, sparks, hit testing
web/panel.js            details view, merges, log, source rack, peek, toasts
web/app.js              wiring
web/style.css           dark CRT theme
scenarios/              change-cascade, supply-chain
docs/                   research notes, design
```

## Limits

- Inference is approximate (noisy-OR with competition), not exact.
- One in-memory session per pack. State resets when the server restarts.
- Bound to 127.0.0.1 with no authentication. It's a local demo.
- The rule kinds are fixed. A new kind is about 20 lines in `server/signals.py`.
- Fonts load from Google Fonts. Offline, the page falls back to system monospace.

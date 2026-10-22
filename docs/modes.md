# Modes: Board, Life, Topology

Three views over one session. The mode strip above the center area switches
between them; the side panel (Details, Merges, Log, Agent), the source rack
and the header stay the same. Selecting a piece in any mode selects it
everywhere.

## Mode interface (web/app.js)

A mode is an ES module exporting three functions:

```js
export function mount(el, ctx) {}   // el: the mode's container (a flex column that fills the center)
export function update(state) {}    // called after every state change while mounted; also right after mount
export function unmount() {}        // stop timers, drop listeners
```

`ctx` (stable for the page's lifetime):

| member | meaning |
|---|---|
| `ctx.state` | the latest state (same object `update` receives) |
| `ctx.selected` | id of the selected piece, or null |
| `ctx.select(id)` | select a piece (null clears); syncs the board and the Details tab |
| `ctx.act(payload)` | send a session action (same payloads the rest of the UI uses) |
| `ctx.highlight(ids, ms)` | pulse pieces on the Board view (no-op while another mode is showing) |
| `ctx.sprites` | the `sprites.js` module: `COLORS`, `drawPiece`, `drawText`, `rgba`, `mix` |

Each mode owns exactly two files: `web/<mode>.js` and `web/<mode>.css`
(`life`, `topology`, `landscape`). Both
are already linked from `index.html`. Modes must not edit `index.html`,
`app.js`, `board.js`, `panel.js`, `sprites.js` or `style.css`.

Mode containers: `#mode-life`, `#mode-topology` and `#mode-landscape`, each a `.mode-view`
(flex column, `flex: 1`, `min-height: 0`). The Board view keeps its own
markup. The current mode is in the URL as `?mode=<name>` and survives
reloads; `board` is the default.

Look: the same CRT pixel style as the board. Use the palette tokens from
`style.css` (`--bg`, `--panel`, `--line`, `--text`, `--dim`, `--signal`,
`--participant`, `--hypothesis`, `--grey`, `--red`, `--gold`, fonts
`--pixel` for labels and `--mono` for text) and `sprites.js` for pixel
drawing. Canvases draw at native resolution and scale up with
`image-rendering: pixelated`, as `board.js` does.

## Shared state fields (server, Agent A)

Two fields are added to the state object. Modes must tolerate their absence
(render a "waiting for server data" note) until the server work lands.

### `state.events`

One event per source record that has a timestamp, sorted by time:

```json
{
  "id": "activity:3",
  "at": "2026-10-16T01:52:00Z",
  "t": 1792115520000,
  "source": "activity",
  "pointer": "line 4",
  "excerpt": "{\"t\": 1792115520000, ...}",
  "participants": ["vale", "svc-ledger"],
  "signals": ["off-hours"],
  "severity": 0.72,
  "acked": false
}
```

- `participants`: ids of board participants that any fragment of this record
  resolved into (same mechanism that links signals to participants).
- `signals`: ids of signals with a hit containing this record.
- `severity`: the highest activation among those signals, or 0 when none.
- `acked`: set by the `ack` action, cleared by `unack` (`{"action": "ack", "event": "<id>"}`), both logged to the audit trail.

Timestamps come from a new optional per-source manifest field:

```json
"time": {"field": "t", "unit": "ms"}
```

`field` is a dotted path or a list of paths tried in order; `unit` is `ms`,
`s`, `iso` (`YYYY-MM-DDTHH:MM:SSZ`) or `date_dmy`. The validator checks the
unit and warns when the field never appears. Records in a source without
`time`, or whose field is empty, produce no event.

### `state.undated` and `unknown_time`

Records that could not be dated (no time config, empty or unparseable
value) are listed in `state.undated` with the same shape as events minus
`at`/`t`, plus `why`. `state.unknown_time` is the pack's policy for them:
`throughout` (present in every timeline slice; slice 0 lists their ids in
`undated`), `at_connect` (they become normal events dated at connect time,
`dated_by: "connect"`), or `after` (a final timeline slice with
`undated_arrive: true` brings them in). Views should label undated evidence
as such wherever it is shown. Events also carry `dated_by`: `record`,
`document` (the whole source has one date) or `connect`.

### `state.links`

Participant pairs joined by a shared record, undirected, one entry per pair:

```json
{"a": "vale", "b": "svc-ledger", "records": [{"source": "activity", "pointer": "line 4"}], "weight": 2}
```

`weight` is the number of joining records. A record joins a pair when its
fragments resolve into both participants.

## Life mode (Agent B): competing species

Conway's Game of Life where each hypothesis is a species. Same evidence as
the board, a different aggregation rule, so the winner can differ; that
difference is the lesson.

- **Grid** 64×36 cells on a canvas, pixel-scaled to fill the mode view.
- **Species = hypotheses**, colored with distinct tints of the hypothesis
  magenta family (or the pack order palette); legend maps colors to codes.
- **Home regions**: the grid split into vertical bands, one per hypothesis.
- **Seeding** from the live board: for each hypothesis, every support term
  in its derivation (`detail.breakdown.support[].term`, which is `w × p`)
  seeds live cells of that species in its band, density proportional to the
  term. Each inhibitor term (`inhibit[].w × inhibit[].p`) removes that
  proportion of the species' seeds. Deterministic PRNG seeded from the
  state's hypothesis beliefs so the same board always produces the same run.
- **Rules**: B3/S23 with colors (the Immigration variant): a born cell takes
  the majority color of its three live neighbors; a tie goes to the species
  with the higher board belief.
- **Readout**: per-species territory share (live cells / total live) as bars
  beside the board's posterior bars; generation counter; one line of
  interpretation (Life's leader vs the board's leader, same or different).
- **Controls**: play / pause / step / reset / speed; the mode does not
  reseed under the user when the board changes: it shows a "board changed,
  reseed" nudge.
- Clicking a cell selects the hypothesis it belongs to (`ctx.select`).
- Works with no sources connected (empty grid, explanatory note) and on
  both packs.

## Topology mode (Agent C): NetView

A pixel-art homage to IBM Tivoli NetView: hierarchical topology maps,
status colors propagated upward, an event browser tied to the map, and
incident replay.

- **Root submap**: one icon per source; its color is the worst status among
  the participants that source contributed to. Opening a source shows its
  participants; a breadcrumb returns to the root. An "all" submap shows
  every participant.
- **Participant icons** by object type (person, service/server, ticket,
  container, port, part; a generic icon for unknown types), labelled with
  code and name. Links from `state.links` drawn between them, thickness by
  weight. Layout must be stable between updates (positions keyed by id;
  a deterministic ring or simple force layout with fixed iterations).
- **Status**: from belief/involvement: normal (green) < 0.33, marginal
  (yellow) < 0.66, critical (red) ≥ 0.66, unknown (blue) for unresolved
  participants, dimmed when every event touching the node is acked. Worst
  status propagates to the submap icon.
- **Event browser** under the map: `state.events` in time order with
  severity swatch, time, source chip, participants and excerpt. Click to
  center and flash the node; Ack / Unack buttons call `ctx.act`.
- **Incident replay**: a scrubber over the event time span with play /
  pause. At time *t* a node's status is the highest severity among events
  up to *t* that touch it (unknown if none); at the end of the span (live)
  statuses come from beliefs. Playing advances in event order with a short
  pause per event.
- Clicking a node selects the piece (`ctx.select`).
- Works with no sources connected and on both packs.

## Landscape mode (Agent D): spike map on a surface plot

A fitness landscape of the evidence: every evidence term is a spike on an
isometric floor, a smoothed surface rises over them, contour lines show the
relief, and the best-backed hypothesis is the highest ridge. Same CRT pixel
style, hand-rolled isometric canvas, no dependency.

- **A spike is one evidence term**: for each hypothesis, each entry of
  `detail.breakdown.support` (height = `term`, i.e. `w × p`) and each entry of
  `detail.breakdown.inhibit` (a **pit** going below the floor, depth =
  `w × p`, drawn red). Terms below 0.01 are omitted.
- **Floor axes.** X: hypotheses side by side as lanes, ordered so hypotheses
  that share the most support (sum of shared terms) are adjacent (the same
  greedy rule Life uses), each lane in the hypothesis's species colour (use
  Life's palette order so colours match across modes). Y: evidence rows,
  signals first grouped by source in rack order, then participants grouped
  by object type; a row exists only for evidence that appears in at least one
  derivation. A spike stands in the cell (hypothesis lane, evidence row).
- **Surface.** A heightfield over the floor grid: each cell's net value is
  support term minus inhibitor cut; smooth with a small kernel (about 1.5
  cells) and draw as a shaded isometric mesh (lighter facing the light,
  darker away, in the lane's hue) with painter's ordering so spikes and
  surface occlude correctly. Draw contour lines on the floor at fixed levels
  (0.05, 0.1, 0.2, 0.4, 0.6) and label the highest ridge. Each lane gets a
  beacon at its crest with code, label and belief; off-board hypotheses keep
  their lane, dimmed and hatched, tagged "off board", so their pits remain
  visible.
- **Camera.** Isometric 2:1, four rotation stops (buttons and keys Q/E),
  zoom (+/−, wheel), fit. Integer pixel scale as the board does.
- **Interaction.** Hover a spike: tooltip "S4 Prod traffic to staging → H3
  Misconfigured change · 0.65 × 0.86 = 0.56" (pits: "cuts by"). Click a spike
  to `ctx.select` the signal or participant; click a lane beacon or label to
  select the hypothesis. The selected piece's spikes are highlighted and the
  rest dimmed.
- **Time.** A scrubber over `GET /api/timeline` slices with play/pause (same
  endpoint and slice shape Life and Topology use; refetch when the board
  fingerprint changes). Spikes and the surface interpolate between slices
  (~300 ms). The live end uses `state`. Label slice 0 / undated evidence as
  Topology does.
- **Readout line** under the floor: "Highest ridge: H3 Misconfigured change
  86% · tallest spike: S4 → H3 0.56 · deepest pit: S5 → H2 −0.79", plus a
  legend (lane colours, height scale, pits = inhibitors, engine in use from
  `state.engine`).
- Works with no sources connected (flat floor, a note) and on both packs.

## Verification each mode must do before reporting

Run the app in the browser pane (`.claude/launch.json` has `elessar` on 8777
and `elessar-alt` on 8791), switch to the mode, connect sources with the
number keys on both packs, and confirm: no console errors, the mode fills
the center at desktop and narrow widths, selection syncs with Details, and
switching away and back works.

// Life mode: competing species. Each hypothesis is a species in Conway's Game
// of Life (B3/S23, Immigration colouring). Evidence arrives as a time-lapse:
// the server re-infers the board at every event time (/api/timeline) and each
// slice plants what its derivation gained since the last one, then Conway
// runs. Same evidence, a different aggregation rule: the winner can differ.
import { escapeHtml } from "./board.js";

const COLS = 64;
const ROWS = 36;
const CELL = 4;      // native pixels per cell: 3 lit + 1 gap
const LABEL_H = 8;   // strip above the grid for the band codes
const W = COLS * CELL;
const H = LABEL_H + ROWS * CELL;

// Seeding: each support term (w × p) targets that share × SEED_DENSITY of the
// home band; the band never fills past MAX_FILL (denser random soups
// overcrowd and die, so more support must never mean fewer survivors). Over
// the cap every term shrinks by the same factor, taken from the final slice so
// a term's target never drops when later evidence arrives. Each slice plants
// max(0, target − already planted) per term; each inhibitor removes its new
// cut (cut now − cut at the previous slice) as a share of the species' live cells.
const SEED_DENSITY = 0.35;
const MAX_FILL = 0.4;
const BORDER_SPREAD = 6; // shared evidence: columns of jitter around "nearest the border first"

const SPEEDS = [{ label: "1x", gps: 4 }, { label: "2x", gps: 8 }, { label: "4x", gps: 16 }, { label: "8x", gps: 32 }];
const CYCLE_WINDOW = 60; // generations remembered to notice a settled run

// Species colours in pack order; the first is the board's hypothesis magenta.
const PALETTE = ["#ff5fd2", "#5fb8ff", "#9cf06a", "#ffb04a", "#b48cff", "#ff6b6b", "#5ff0d8", "#f0e45f", "#e0e8f0"];

// Evidence cells carry their source node's 3×3 pattern: 1 bright, 0 species
// colour, . dark. Assigned by node order, reused after six. Born cells are plain.
const PATTERNS = [
  { name: "solid", px: "111111111" },
  { name: "hatched", px: "001010100" },
  { name: "dotted", px: "000010000" },
  { name: "ringed", px: "1111.1111" },
  { name: "checked", px: "101010101" },
  { name: "barred", px: "000111000" },
];
const FINE_PX = 6; // below this many CSS pixels per cell, evidence is a flat two-tone shade

let root = null;
let ctx = null;
let els = null;
let sim = null;
let timer = null;
let playing = false;
let speed = 1;
let gensPerSlice = 6;
let hover = null;
let observer = null;
let onKey = null;
let cellPx = 8;
let prefetch = null;  // {fp, promise}: the timeline for a board, fetched once
let loadSeq = 0;
let loadingFp = null;
const evidenceOpen = new Map(); // species id → evidence list open, as the user left it
const swatches = new Map();

// ------------------------------------------------------------------ mode interface

export function mount(el, context) {
  root = el;
  ctx = context;
  sim = null;
  playing = false;
  hover = null;
  prefetch = null;
  loadingFp = null;
  root.innerHTML = `
    <div class="life">
      <div class="life-bar">
        <button data-act="play" class="life-play">play</button>
        <button data-act="step" title="One generation">step</button>
        <button data-act="next" title="Plant the next slice of evidence now">next slice</button>
        <button data-act="reset" title="Back to the first slice of this run">reset</button>
        <span class="life-speed"><span class="dim">speed</span>${SPEEDS.map((s, i) => `<button data-speed="${i}">${s.label}</button>`).join("")}</span>
        <label class="life-k dim" title="Conway generations between slices">gens/slice <input type="number" min="1" max="20" value="${gensPerSlice}"></label>
        <span class="life-nudge" hidden>board changed <button data-act="reseed" class="gold">reseed</button></span>
      </div>
      <div class="life-bar life-time">
        <span class="life-gen">GEN <b>0000</b></span>
        <span class="life-when"></span>
        <input class="life-scrub" type="range" min="0" max="0" value="0" step="1" aria-label="timeline slice">
        <span class="life-slice dim"></span>
        <span class="life-status dim"></span>
      </div>
      <div class="life-main">
        <div class="life-stage">
          <div class="life-wrap">
            <canvas class="life-glow"></canvas>
            <canvas class="life-grid"></canvas>
            <div class="life-scan"></div>
            <div class="tip" hidden></div>
          </div>
        </div>
        <aside class="life-side"></aside>
      </div>
      <p class="life-read"></p>
      <p class="life-note dim small">B3/S23; a born cell takes the majority colour of its three parents (all different: the higher board belief wins).
        Evidence arrives in event time: each slice plants what every support term gained (×${SEED_DENSITY} of the band per unit of w×p; a signal shared by two hypotheses plants on the border between them), inhibitors remove their new cut from the species' live cells, then Conway runs.
        Newly planted cells are immune to death for one generation. <b class="life-key">Patterned cells are evidence</b> (one pattern per signal or participant); <b class="life-key plain">plain cells were born</b> by Conway.</p>
    </div>`;
  const q = (s) => root.querySelector(s);
  els = {
    life: q(".life"), stage: q(".life-stage"), wrap: q(".life-wrap"), grid: q(".life-grid"), glow: q(".life-glow"),
    tip: q(".tip"), side: q(".life-side"), read: q(".life-read"), gen: q(".life-gen b"), when: q(".life-when"),
    status: q(".life-status"), nudge: q(".life-nudge"), play: q(".life-play"), next: q('[data-act="next"]'),
    scrub: q(".life-scrub"), slice: q(".life-slice"), k: q(".life-k input"),
  };
  els.g = els.grid.getContext("2d");
  els.gg = els.glow.getContext("2d");

  q(".life-bar").addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    if (b.dataset.speed) { speed = Number(b.dataset.speed); if (playing) startTimer(); renderBar(); return; }
    const act = b.dataset.act;
    if (!sim && act !== "reseed") return;
    if (act === "play") setPlaying(!playing);
    if (act === "step") { setPlaying(false); tick(); }
    if (act === "next" && sim.slice < lastSlice()) { applySlice(sim.slice + 1); renderAll(); }
    if (act === "reset") { setPlaying(false); rebuild(0); }
    if (act === "reseed") load(ctx.state);
  });
  els.k.addEventListener("change", () => {
    gensPerSlice = Math.max(1, Math.min(20, Math.round(Number(els.k.value)) || 6));
    els.k.value = gensPerSlice;
  });
  // scrubbing replays the run from the first slice, so every slice is reproducible
  els.scrub.addEventListener("input", () => { if (sim) rebuild(Number(els.scrub.value)); });
  els.grid.addEventListener("mousemove", onHover);
  els.grid.addEventListener("mouseleave", () => { hover = null; els.tip.hidden = true; draw(); });
  els.grid.addEventListener("click", (e) => {
    const s = speciesAt(e);
    if (s) ctx.select(s.id);
    draw();
    markSelected();
  });
  // a code in the readout selects that piece; elsewhere a card selects its species
  const pick = (e) => {
    const ref = e.target.closest("[data-sel]");
    const card = e.target.closest("[data-id]");
    if (ref) ctx.select(ref.dataset.sel);
    else if (card && !e.target.closest("summary")) ctx.select(card.dataset.id);
    else return;
    draw();
    markSelected();
  };
  els.side.addEventListener("click", pick);
  els.read.addEventListener("click", pick);
  // remember which evidence lists the user opened or closed across rebuilds
  els.side.addEventListener("toggle", (e) => {
    const card = e.target.closest("[data-id]");
    if (card && e.target.matches("details")) evidenceOpen.set(card.dataset.id, e.target.open);
  }, true);
  // Escape clears the selection in app.js without an update; follow it.
  onKey = () => setTimeout(() => { if (sim) { draw(); markSelected(); } }, 0);
  document.addEventListener("keydown", onKey);
  observer = new ResizeObserver(() => fit());
  observer.observe(root);
}

export function update(state) {
  if (!root || !state) return;
  const fp = fingerprint(state);
  if (!sim || sim.scenario !== state.scenario.id) {
    // a fresh mount or another pack: nothing to disturb, seed straight away
    if (loadingFp !== fp) load(state);
    renderBar();
    return;
  }
  els.nudge.hidden = fp === sim.fp;
  if (fp !== sim.fp) timelineFor(state); // fetch now so reseed is instant
  renderSide();
  renderRead();
}

export function unmount() {
  stopTimer();
  if (observer) observer.disconnect();
  if (onKey) document.removeEventListener("keydown", onKey);
  loadSeq++; // drop any timeline still in flight
  observer = null;
  onKey = null;
  root = null;
  els = null;
  sim = null;
}

// ------------------------------------------------------------------ the timeline

// Everything the timeline depends on: sources, engine, feedback, and every
// hypothesis belief and term (judgements and merges move those).
function fingerprint(state) {
  const parts = [state.scenario.id, state.engine, state.feedback, state.sources.filter((s) => s.connected).map((s) => s.id).join(",")];
  for (const n of hypotheses(state)) {
    const b = (n.detail && n.detail.breakdown) || {};
    parts.push(`${n.id}:${n.p.toFixed(4)}:${(b.support || []).map((t) => t.term.toFixed(4))}:${(b.inhibit || []).map((t) => (t.w * t.p).toFixed(4))}`);
  }
  return parts.join("|");
}

const hypotheses = (state) => state.nodes.filter((n) => n.band === "hypothesis");

function timelineFor(state) {
  const fp = fingerprint(state);
  if (!prefetch || prefetch.fp !== fp) prefetch = { fp, promise: fetchTimeline(state) };
  return prefetch.promise;
}

async function fetchTimeline(state) {
  try {
    const res = await fetch(`/api/timeline?scenario=${encodeURIComponent(state.scenario.id)}`);
    if (!res.ok) throw new Error(res.statusText);
    const tl = await res.json();
    if (tl && Array.isArray(tl.slices) && tl.slices.length) return tl;
  } catch (e) { /* no timeline from the server: the live board as one slice */ }
  return singleSlice(state);
}

// The live board in the timeline's shape, for a server without /api/timeline.
function singleSlice(state) {
  const hyps = {};
  for (const n of hypotheses(state)) {
    const b = (n.detail && n.detail.breakdown) || {};
    hyps[n.id] = {
      p: n.p, status: n.status, present: n.present,
      support: (b.support || []).filter((t) => t.term > 0.001).map((t) => ({ from: t.from, w: t.w, p: t.p, term: t.term })),
      inhibit: (b.inhibit || []).filter((t) => t.w * t.p > 0.001).map((t) => ({ from: t.from, w: t.w, p: t.p, cut: t.w * t.p })),
    };
  }
  return { slices: [{ index: 0, t: null, at: null, events_seen: 0, arrived: [], hypotheses: hyps }], event_count: 0 };
}

async function load(state) {
  const seq = ++loadSeq;
  loadingFp = fingerprint(state);
  const tl = await timelineFor(state);
  if (seq !== loadSeq || !root) return;
  loadingFp = null;
  seed(state, tl);
  setPlaying(sim.anyEvidence);
}

// ------------------------------------------------------------------ seeding

// FNV-1a over the fingerprint seeds a mulberry32 stream: same board, same run.
function hash(str) {
  let h = 0x811c9dc5;
  for (let i = 0; i < str.length; i++) { h ^= str.charCodeAt(i); h = Math.imul(h, 0x01000193); }
  return h >>> 0;
}

function prng(seedValue) {
  let a = seedValue;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function shuffle(list, rand) {
  for (let i = list.length - 1; i > 0; i--) {
    const j = Math.floor(rand() * (i + 1));
    [list[i], list[j]] = [list[j], list[i]];
  }
  return list;
}

// Bands left to right so hypotheses sharing the most support sit side by
// side: start from the heaviest pair, then keep attaching the hypothesis that
// shares most with either end. Ties and unrelated hypotheses keep pack order.
function bandOrder(ids, last) {
  const sup = ids.map((id) => new Map(((last.hypotheses[id] || {}).support || []).map((t) => [t.from, t.term])));
  const weight = (a, b) => {
    let w = 0;
    for (const [from, term] of sup[a]) if (sup[b].has(from)) w += term + sup[b].get(from);
    return w;
  };
  const n = ids.length;
  let best = null;
  for (let a = 0; a < n; a++) for (let b = a + 1; b < n; b++) {
    const w = weight(a, b);
    if (w > 0 && (!best || w > best.w)) best = { a, b, w };
  }
  if (!best) return ids.map((_, i) => i);
  const chain = [best.a, best.b];
  const left = new Set(ids.map((_, i) => i).filter((i) => i !== best.a && i !== best.b));
  while (left.size) {
    let pick = null;
    for (const r of left) {
      for (const end of ["L", "R"]) {
        const w = weight(r, end === "L" ? chain[0] : chain[chain.length - 1]);
        if (!pick || w > pick.w) pick = { r, end, w };
      }
    }
    if (pick.w <= 0) pick = { r: Math.min(...left), end: "R" };
    if (pick.end === "L") chain.unshift(pick.r); else chain.push(pick.r);
    left.delete(pick.r);
  }
  return chain;
}

// A new board (or pack): lay out the species and replay from the first slice.
function seed(state, tl) {
  const info = new Map();
  let k = 0;
  for (const n of state.nodes) {
    info.set(n.id, { id: n.id, code: n.code, label: n.label, band: n.band, pattern: n.band === "hypothesis" ? -1 : k++ % PATTERNS.length });
  }
  const slices = tl.slices;
  const last = slices[slices.length - 1];
  const hyps = hypotheses(state);
  const ids = hyps.map((n) => n.id);
  const order = bandOrder(ids, last);

  // sources that ever back more than one hypothesis plant on the shared border
  const backers = new Map();
  for (const sl of slices) {
    ids.forEach((id, i) => {
      for (const t of (sl.hypotheses[id] || {}).support || []) {
        if (!backers.has(t.from)) backers.set(t.from, new Set());
        backers.get(t.from).add(i);
      }
    });
  }

  const species = hyps.map((n, i) => {
    const pos = order.indexOf(i);
    const x0 = Math.round((pos * COLS) / hyps.length);
    const x1 = Math.round(((pos + 1) * COLS) / hyps.length);
    const cells = (x1 - x0) * ROWS;
    // today's allocation on the final board is every term's target
    const want = (term) => Math.round(term * SEED_DENSITY * cells);
    const finalSup = ((last.hypotheses[n.id] || {}).support || []).map((t) => ({ from: t.from, term: t.term, want: want(t.term) }));
    const cap = Math.floor(cells * MAX_FILL);
    const wanted = finalSup.reduce((a, t) => a + t.want, 0);
    const f = wanted > cap ? cap / wanted : 1;
    finalSup.forEach((t) => { t.alloc = Math.floor(t.want * f); });
    let spare = Math.min(cap, wanted) - finalSup.reduce((a, t) => a + t.alloc, 0);
    for (const t of [...finalSup].sort((a, b) => b.term - a.term)) if (spare > 0) { t.alloc++; spare--; }
    return {
      id: n.id, code: n.code, label: n.label, color: PALETTE[i % PALETTE.length], pos, x0, x1, cells, f, cap,
      final: new Map(finalSup.map((t) => [t.from, t])), want,
    };
  });
  // which edge of its band each shared term plants on: the side facing its partners
  for (const [i, s] of species.entries()) {
    s.sides = new Map();
    for (const [from, set] of backers) {
      if (!set.has(i) || set.size < 2) continue;
      const sides = new Set([...set].filter((j) => j !== i).map((j) => (species[j].pos < s.pos ? "L" : "R")));
      s.sides.set(from, sides.size === 2 ? "both" : [...sides][0]);
    }
  }
  const anyEvidence = slices.some((sl) => species.some((s) => ((sl.hypotheses[s.id] || {}).support || []).some((t) => s.want(t.term) > 0)));

  sim = {
    scenario: state.scenario.id, fp: fingerprint(state), slices, eventCount: tl.event_count || 0, info, species, anyEvidence,
    connected: state.sources.some((s) => s.connected), byPos: [...species].sort((a, b) => a.pos - b.pos),
    sources: [...info.values()].filter((n) => n.band !== "hypothesis"),
    lastDate: (slices.filter((sl) => sl.at).pop() || {}).at?.slice(0, 10),
  };
  sim.sourceIndex = new Map(sim.sources.map((n, j) => [n.id, j + 1]));
  for (const s of species) s.styles = styles(s.color, false);
  els.nudge.hidden = true;
  els.scrub.max = String(lastSlice());
  rebuild(0);
  fit();
}

const lastSlice = () => sim.slices.length - 1;

// Replay from the first slice up to slice `to`: a fresh PRNG from the
// fingerprint, the same cell orders, K generations between slices.
function rebuild(to) {
  const rand = prng(hash(sim.fp));
  sim.rand = rand;
  const n = COLS * ROWS;
  Object.assign(sim, {
    grid: new Uint8Array(n), mark: new Uint16Array(n), when: new Uint16Array(n), bornAt: new Uint32Array(n),
    immune: new Uint8Array(n), gen: 0, slice: -1, sinceSlice: 0, settled: null, seen: new Map(),
  });
  for (const s of sim.species) {
    // interior: the band in random order; edges: nearest the border first, jittered
    const cells = [];
    for (let r = 0; r < ROWS; r++) for (let c = s.x0; c < s.x1; c++) cells.push(r * COLS + c);
    const edge = (side) => cells
      .map((at) => ({ at, key: (side === "L" ? at % COLS - s.x0 : s.x1 - 1 - (at % COLS)) + rand() * BORDER_SPREAD }))
      .sort((a, b) => a.key - b.key).map((e) => e.at);
    const L = edge("L"), R = edge("R");
    s.orders = { interior: shuffle([...cells], rand), L, R, both: L.flatMap((at, j) => [at, R[j]]) };
    s.planted = new Map();
    s.removed = new Map();
    s.cutBefore = new Map();
  }
  applySlice(0);
  for (let j = 1; j <= to && j <= lastSlice(); j++) {
    for (let g = 0; g < gensPerSlice; g++) generation();
    applySlice(j);
  }
  renderSide();
  renderAll();
}

// One slice of evidence: every support term plants its growth since the last
// slice, then every inhibitor removes its new cut from the species' live cells.
function applySlice(j) {
  const sl = sim.slices[j];
  sim.slice = j;
  sim.sinceSlice = 0;
  sim.settled = null;
  sim.seen = new Map();
  const view = (t, v) => ({ ...(sim.info.get(t.from) || { id: t.from, code: t.from, label: t.from }), id: t.from, v });
  sim.species.forEach((s, i) => {
    const h = sl.hypotheses[s.id] || { p: 0, support: [], inhibit: [] };
    s.p = h.p;
    s.present = h.present !== false;
    s.support = (h.support || []).map((t) => view(t, t.term));
    s.inhibit = (h.inhibit || []).map((t) => view(t, t.cut));
    for (const t of h.support || []) {
      const have = s.planted.get(t.from) || 0;
      const need = Math.max(0, target(s, t.from, t.term) - have);
      if (need) s.planted.set(t.from, have + plant(i, t.from, need, j));
    }
  });
  sim.species.forEach((s, i) => {
    const h = sl.hypotheses[s.id] || {};
    const now = new Map((h.inhibit || []).map((t) => [t.from, t.cut]));
    for (const [from, cut] of now) {
      const delta = cut - (s.cutBefore.get(from) || 0);
      if (delta > 0) s.removed.set(from, (s.removed.get(from) || 0) + cull(i, delta));
    }
    s.cutBefore = now;
  });
  for (const s of sim.species) {
    s.offboard = !s.present;
    s.styles = styles(s.color, s.offboard);
    s.support.forEach((t) => { t.planted = s.planted.get(t.id) || 0; });
    s.inhibit.forEach((t) => { t.removed = s.removed.get(t.id) || 0; });
    const bySize = (a, b) => b.v - a.v;
    s.support.sort(bySize);
    s.inhibit.sort(bySize);
  }
  // ties at birth go to the higher board belief at this slice, then pack order
  const rank = sim.species.map((s, i) => i).sort((a, b) => sim.species[b].p - sim.species[a].p || a - b);
  sim.rankOf = new Uint8Array(sim.species.length + 1);
  rank.forEach((s, k) => { sim.rankOf[s + 1] = k; });
  sim.sideDirty = true;
  count();
}

// The term's target: today's final allocation once the term reaches its
// final size, otherwise its own share shrunk by the band's final cap factor.
function target(s, from, term) {
  const want = s.want(term);
  const fin = s.final.get(from);
  return fin && want >= fin.want ? fin.alloc : Math.floor(want * s.f);
}

function plant(i, from, n, j) {
  const s = sim.species[i];
  const side = s.sides.get(from);
  const order = side ? s.orders[side] : s.orders.interior;
  const mark = sim.sourceIndex.get(from) || 0;
  let placed = 0;
  for (const at of order) {
    if (placed === n) break;
    if (sim.grid[at]) continue;
    sim.grid[at] = i + 1;
    sim.mark[at] = mark;
    sim.when[at] = j;
    sim.bornAt[at] = sim.gen;
    sim.immune[at] = 1;
    placed++;
  }
  return placed;
}

function cull(i, share) {
  const live = [];
  for (let at = 0; at < sim.grid.length; at++) if (sim.grid[at] === i + 1) live.push(at);
  const k = Math.round(share * live.length);
  for (const at of shuffle(live, sim.rand).slice(0, k)) {
    sim.grid[at] = 0;
    sim.mark[at] = 0;
    sim.immune[at] = 0;
  }
  return k;
}

// ------------------------------------------------------------------ the automaton

function generation() {
  const { grid, rankOf } = sim;
  const n = COLS * ROWS;
  const next = new Uint8Array(n), mark = new Uint16Array(n), when = new Uint16Array(n), bornAt = new Uint32Array(n);
  const nb = [0, 0, 0];
  sim.gen++;
  for (let r = 0; r < ROWS; r++) {
    for (let c = 0; c < COLS; c++) {
      // bounded grid: outside the edge is dead
      let k = 0;
      for (let dr = -1; dr <= 1; dr++) {
        const rr = r + dr;
        if (rr < 0 || rr >= ROWS) continue;
        for (let dc = -1; dc <= 1; dc++) {
          const cc = c + dc;
          if ((dr === 0 && dc === 0) || cc < 0 || cc >= COLS) continue;
          const s = grid[rr * COLS + cc];
          if (s) { if (k < 3) nb[k] = s; k++; }
        }
      }
      const i = r * COLS + c;
      const s = grid[i];
      // survivors (and freshly planted evidence, immune this once) keep their marks
      if (s && (k === 2 || k === 3 || sim.immune[i])) {
        next[i] = s; mark[i] = sim.mark[i]; when[i] = sim.when[i]; bornAt[i] = sim.bornAt[i];
      } else if (!s && k === 3) {
        // Immigration: majority of the three parents; all different → best rank
        const [a, b, d] = nb;
        next[i] = a === b || a === d ? a : b === d ? b : [a, b, d].sort((x, y) => rankOf[x] - rankOf[y])[0];
        bornAt[i] = sim.gen;
      }
    }
  }
  Object.assign(sim, { grid: next, mark, when, bornAt, immune: new Uint8Array(n) });
}

// One tick of play: a generation; after K of them the next slice lands. Once
// the last slice is in, a run that repeats itself (or dies out) is settled.
function tick() {
  if (!sim || sim.settled) return;
  generation();
  sim.sinceSlice++;
  if (sim.slice < lastSlice()) {
    if (sim.sinceSlice >= gensPerSlice) applySlice(sim.slice + 1);
    else count();
  } else {
    count();
    const h = hashGrid(sim.grid);
    if (sim.total === 0) sim.settled = { kind: "extinct", gen: sim.gen };
    else if (sim.seen.has(h)) sim.settled = { kind: "settled", gen: sim.gen, period: sim.gen - sim.seen.get(h) };
    sim.seen.set(h, sim.gen);
    if (sim.seen.size > CYCLE_WINDOW) sim.seen.delete(sim.seen.keys().next().value);
    if (sim.settled) setPlaying(false);
  }
  renderAll();
}

function hashGrid(g) {
  let h = 0x811c9dc5;
  for (let i = 0; i < g.length; i++) { h ^= g[i]; h = Math.imul(h, 0x01000193); }
  return h >>> 0;
}

function count() {
  const counts = sim.species.map(() => 0);
  for (const s of sim.grid) if (s) counts[s - 1]++;
  sim.counts = counts;
  sim.total = counts.reduce((a, n) => a + n, 0);
}

// ------------------------------------------------------------------ timer

function setPlaying(on) {
  playing = !!on && !!sim && sim.anyEvidence && !sim.settled;
  if (playing) startTimer(); else stopTimer();
  renderBar();
}

function startTimer() {
  stopTimer();
  timer = setInterval(tick, 1000 / SPEEDS[speed].gps);
}

function stopTimer() {
  if (timer) clearInterval(timer);
  timer = null;
}

// ------------------------------------------------------------------ drawing

function css(arr, a = 1) {
  return `rgba(${arr.map((v) => v | 0).join(",")},${a})`;
}

function styles(color, offboard) {
  const { mix, COLORS } = ctx.sprites;
  const base = mix(color, COLORS.grey, offboard ? 0.35 : 0); // explained-away species fade toward grey
  return {
    live: css(base),
    bright: css(mix(base, [255, 255, 255], 0.5)),
    plain: css(mix(base, COLORS.bg, 0.3)),
    hole: css(mix(base, COLORS.bg, 0.8)),
    dead: css(mix(COLORS.dark, color, 0.07)),
    label: css(offboard ? mix(base, COLORS.bg, 0.3) : base),
    glow: css(base, 0.55),
  };
}

// A source's pattern as a tiny image, for the evidence rows.
function swatch(pattern, color) {
  const key = `${pattern}|${color}`;
  if (!swatches.has(key)) {
    const st = styles(color, false);
    const c = document.createElement("canvas");
    c.width = 3; c.height = 3;
    const g = c.getContext("2d");
    [...PATTERNS[pattern].px].forEach((ch, k) => {
      g.fillStyle = ch === "1" ? st.bright : ch === "." ? st.hole : st.live;
      g.fillRect(k % 3, Math.floor(k / 3), 1, 1);
    });
    swatches.set(key, c.toDataURL());
  }
  return swatches.get(key);
}

// Fill the stage. As on the board, the canvas is drawn at a whole number of
// device pixels per native pixel and scaled by CSS with pixelated rendering.
function fit() {
  if (!els || !sim) return;
  const flow = matchMedia("(max-width: 1100px)").matches; // style.css lets the page scroll here
  const stack = root.clientWidth < 720;
  els.life.classList.toggle("life-flow", flow || stack);
  els.life.classList.toggle("life-stack", stack);
  const availW = Math.max(0, els.stage.clientWidth - 2);
  const availH = flow || stack ? window.innerHeight * 0.62 : Math.max(0, els.stage.clientHeight - 2);
  const scale = Math.max(1, Math.min(availW / W, availH / H));
  const cssW = Math.floor(W * scale);
  const cssH = Math.floor(H * scale);
  cellPx = 3 * scale;
  const k = Math.max(1, Math.ceil(scale * (window.devicePixelRatio || 1)));
  if (els.grid.width !== W * k) {
    for (const c of [els.grid, els.glow]) { c.width = W * k; c.height = H * k; }
    els.g.setTransform(k, 0, 0, k, 0, 0);
    els.gg.setTransform(k, 0, 0, k, 0, 0);
  }
  els.wrap.style.width = `${cssW}px`;
  els.wrap.style.height = `${cssH}px`;
  els.wrap.style.setProperty("--scan", `${scale}px`);
  // beside the grid in a scrolling page, the species list scrolls within the grid's height
  els.side.style.maxHeight = flow && !stack ? `${cssH}px` : "";
  draw();
}

function draw() {
  if (!els || !sim) return;
  const { COLORS, drawText } = ctx.sprites;
  const g = els.g, gg = els.gg;
  g.fillStyle = COLORS.bg;
  g.fillRect(0, 0, W, H);
  gg.clearRect(0, 0, W, H);
  const selected = ctx.selected;
  const fine = cellPx >= FINE_PX;

  // band codes along the top, underlined for the selected species
  for (const s of sim.species) {
    const bx = s.x0 * CELL, bw = (s.x1 - s.x0) * CELL - 1;
    drawText(g, s.code, bx + Math.floor((bw - (s.code.length * 4 - 1)) / 2), 1, s.styles.label);
    if (s.id === selected) { g.fillStyle = s.styles.live; g.fillRect(bx, LABEL_H - 1, bw, 1); }
  }

  // cells: evidence in its source's pattern, born cells plain, dead tinted by band
  const owner = new Uint8Array(COLS);
  sim.species.forEach((s, i) => { for (let c = s.x0; c < s.x1; c++) owner[c] = i; });
  for (let r = 0; r < ROWS; r++) {
    for (let c = 0; c < COLS; c++) {
      const i = r * COLS + c;
      const x = c * CELL, y = LABEL_H + r * CELL;
      const s = sim.grid[i];
      if (!s) {
        const home = sim.species[owner[c]];
        g.fillStyle = home ? home.styles.dead : COLORS.dark;
        g.fillRect(x, y, 3, 3);
        continue;
      }
      const st = sim.species[s - 1].styles;
      const m = sim.mark[i];
      if (!m) {
        g.fillStyle = st.plain;
        g.fillRect(x, y, 3, 3);
      } else if (!fine) {
        g.fillStyle = st.bright; // too small for patterns: evidence is the bright tone
        g.fillRect(x, y, 3, 3);
      } else {
        const px = PATTERNS[sim.sources[m - 1].pattern].px;
        for (let k = 0; k < 9; k++) {
          g.fillStyle = px[k] === "1" ? st.bright : px[k] === "." ? st.hole : st.live;
          g.fillRect(x + (k % 3), y + Math.floor(k / 3), 1, 1);
        }
      }
      gg.fillStyle = st.glow;
      gg.fillRect(x, y, 3, 3);
    }
  }

  // band seams, and a dotted frame around the selected species' home band
  g.fillStyle = "rgba(255,255,255,0.10)";
  for (const s of sim.species) if (s.x0 > 0) for (let y = LABEL_H; y < H; y += 2) g.fillRect(s.x0 * CELL - 1, y, 1, 1);
  const sel = sim.species.find((s) => s.id === selected);
  if (sel) {
    g.fillStyle = "rgba(232,241,255,0.7)";
    const x0 = sel.x0 * CELL - 1, x1 = sel.x1 * CELL - 1;
    for (let x = Math.max(0, x0); x <= x1; x += 2) { g.fillRect(x, LABEL_H - 1, 1, 1); g.fillRect(x, H - 1, 1, 1); }
    for (let y = LABEL_H - 1; y < H; y += 2) { if (x0 >= 0) g.fillRect(x0, y, 1, 1); g.fillRect(Math.min(W - 1, x1), y, 1, 1); }
  }
  if (hover) {
    g.fillStyle = "rgba(232,241,255,0.55)";
    g.fillRect(hover.c * CELL - 1, LABEL_H + hover.r * CELL - 1, 5, 1);
    g.fillRect(hover.c * CELL - 1, LABEL_H + hover.r * CELL + 3, 5, 1);
    g.fillRect(hover.c * CELL - 1, LABEL_H + hover.r * CELL, 1, 3);
    g.fillRect(hover.c * CELL + 3, LABEL_H + hover.r * CELL, 1, 3);
  }
}

// ------------------------------------------------------------------ hit testing

function cellAt(e) {
  const r = els.grid.getBoundingClientRect();
  const x = ((e.clientX - r.left) / r.width) * W;
  const y = ((e.clientY - r.top) / r.height) * H;
  const c = Math.min(COLS - 1, Math.max(0, Math.floor(x / CELL)));
  const row = y < LABEL_H ? -1 : Math.min(ROWS - 1, Math.floor((y - LABEL_H) / CELL));
  return { c, r: row };
}

// A live cell belongs to its species; an empty cell (or the label strip) to the band's owner.
function speciesAt(e) {
  if (!sim || !sim.species.length) return null;
  const { c, r } = cellAt(e);
  const s = r >= 0 ? sim.grid[r * COLS + c] : 0;
  return s ? sim.species[s - 1] : sim.species.find((sp) => c >= sp.x0 && c < sp.x1);
}

// "02:00:00" on the incident's day, the full date otherwise; slice 0 is reference data.
function sliceTime(j, full) {
  const at = sim.slices[j] && sim.slices[j].at;
  if (!at) return "reference data";
  const d = at.slice(0, 10), t = at.slice(11, 19);
  return full || d !== sim.lastDate ? `${d} ${t}` : t;
}

function onHover(e) {
  if (!sim) return;
  const at = cellAt(e);
  const s = speciesAt(e);
  hover = at.r >= 0 ? at : null;
  els.grid.style.cursor = s ? "pointer" : "default";
  if (!s) { els.tip.hidden = true; draw(); return; }
  const idx = at.r >= 0 ? at.r * COLS + at.c : -1;
  const live = idx >= 0 && sim.grid[idx];
  const i = sim.species.indexOf(s);
  const share = sim.total ? Math.round((sim.counts[i] / sim.total) * 100) : 0;
  let origin = "home band";
  if (live) {
    const src = sim.mark[idx] ? sim.sources[sim.mark[idx] - 1] : null;
    origin = src ? `planted by <b>${escapeHtml(src.code)}</b> ${escapeHtml(src.label)} · ${sliceTime(sim.when[idx])}` : `born gen ${sim.bornAt[idx]}`;
  }
  els.tip.innerHTML = `<b>${s.code}</b> ${escapeHtml(s.label)} · ${origin}<br><span>${sim.counts[i]} cells, ${share}% of territory</span>`;
  const wr = els.wrap.getBoundingClientRect();
  const left = e.clientX - wr.left + 14;
  els.tip.hidden = false;
  els.tip.style.left = `${Math.max(0, Math.min(left, wr.width - els.tip.offsetWidth - 4))}px`;
  els.tip.style.top = `${e.clientY - wr.top + 12}px`;
  draw();
}

// ------------------------------------------------------------------ readout

function renderAll() {
  draw();
  renderBar();
  if (sim && sim.sideDirty) renderSide(); else updateSide();
  renderRead();
}

function renderBar() {
  if (!els) return;
  els.play.textContent = playing ? "pause" : "play";
  els.play.classList.toggle("on", playing);
  els.play.disabled = !sim || !sim.anyEvidence || !!sim.settled;
  root.querySelectorAll("[data-speed]").forEach((b) => b.classList.toggle("on", Number(b.dataset.speed) === speed));
  if (!sim) { els.status.textContent = "loading the timeline…"; return; }
  const last = lastSlice();
  els.next.disabled = sim.slice >= last;
  els.scrub.disabled = last === 0;
  els.scrub.value = String(sim.slice);
  els.gen.textContent = String(sim.gen).padStart(4, "0");
  const sl = sim.slices[sim.slice];
  els.when.textContent = `· ${sliceTime(sim.slice, true)}${sim.eventCount ? ` · ${sl.events_seen}/${sim.eventCount} events` : ""}`;
  els.slice.textContent = `slice ${sim.slice}/${last}`;
  let status = `${sim.total} live`;
  if (!sim.anyEvidence) status = "nothing seeded";
  else if (sim.settled && sim.settled.kind === "extinct") status = `extinct at gen ${sim.settled.gen}`;
  else if (sim.settled) status = `${sim.total} live · settled at gen ${sim.settled.gen}${sim.settled.period > 1 ? `, period ${sim.settled.period}` : ", still"}`;
  else if (playing) status += ` · ${SPEEDS[speed].gps} gen/s`;
  els.status.textContent = status;
}

function leaders() {
  const sp = sim.species;
  if (!sp.length) return {};
  const byBelief = (a, b) => sp[b].p - sp[a].p || a - b;
  const idx = sp.map((s, i) => i);
  const board = [...idx].sort(byBelief)[0];
  const life = sim.total ? [...idx].sort((a, b) => sim.counts[b] - sim.counts[a] || byBelief(a, b))[0] : -1;
  return { board, life };
}

const pct = (v) => `${Math.round(v * 100)}%`;

// A clickable code: selects that piece everywhere (signal, participant or species).
function ref(id, code, color) {
  return `<button class="life-ref" data-sel="${escapeHtml(id)}" style="--c:${color}">${escapeHtml(code)}</button>`;
}
const evRef = (t) => ref(t.id, t.code, ctx.sprites.COLORS[t.band] || ctx.sprites.COLORS.grey);
const spRef = (s) => ref(s.id, s.code, s.color);

// Evidence rows at the current slice: support terms (+w×p, their pattern,
// cells planted so far) then inhibitors (−w×p in red, cells removed so far).
function evidenceHtml(s) {
  const pat = (t) => (t.pattern >= 0 ? `<img class="life-pat" src="${swatch(t.pattern, s.color)}" alt="" title="${PATTERNS[t.pattern].name}: cells planted by ${escapeHtml(t.code)}">` : "<span></span>");
  const row = (t, kind) => `<div class="life-ev-row ${kind}">${kind === "sup" ? pat(t) : "<span></span>"}${evRef(t)}<span class="lbl" title="${escapeHtml(t.label)}">${escapeHtml(t.label)}</span>`
    + `<span class="v">${kind === "sup" ? "+" : "−"}${t.v.toFixed(2)}</span>`
    + `<span class="n" title="${kind === "sup" ? "cells this term has planted" : "live cells this inhibitor has removed"}">${kind === "sup" ? t.planted : t.removed ? `−${t.removed}` : "0"}</span></div>`;
  const sup = s.support.filter((t) => t.v >= 0.01);
  const inh = s.inhibit.filter((t) => t.v >= 0.01);
  if (!sup.length && !inh.length) return `<p class="dim small life-ev-none">No evidence yet: every term is below 0.01.</p>`;
  return sup.map((t) => row(t, "sup")).join("") + inh.map((t) => row(t, "inh")).join("");
}

// "230 seeds · S4 77 · S6 65": cells each support term has planted so far.
function accounting(s) {
  const planted = [...s.planted.values()].reduce((a, n) => a + n, 0);
  const removed = [...s.removed.values()].reduce((a, n) => a + n, 0);
  const parts = s.support.filter((t) => t.planted > 0).sort((a, b) => b.planted - a.planted).map((t) => `${escapeHtml(t.code)} ${t.planted}`);
  const head = `${planted} seed${planted === 1 ? "" : "s"}` + (removed ? ` <span class="cut" title="live cells removed by inhibitors">−${removed}</span>` : "")
    + (s.f < 1 ? ` <span class="dim" title="band fill capped at ${MAX_FILL * 100}%">cap</span>` : "");
  return [head, ...parts].join(" · ");
}

function renderSide() {
  if (!els || !sim) return;
  sim.sideDirty = false;
  if (!sim.species.length) { els.side.innerHTML = `<p class="dim">This pack has no hypotheses.</p>`; return; }
  const stale = !els.nudge.hidden;
  const narrow = els.life.classList.contains("life-stack");
  const html = `<h3>Species <span class="dim">life territory · board belief at this slice${stale ? " (earlier board)" : ""}</span></h3>`
    + sim.byPos.map((s) => {
      // evidence starts open beside the grid, closed when stacked, always open for the explained away
      const open = evidenceOpen.has(s.id) ? evidenceOpen.get(s.id) : !narrow || s.offboard;
      return `<div class="life-sp${s.offboard ? " off" : ""}" data-id="${escapeHtml(s.id)}" style="--c:${s.color}">
        <div class="life-sp-head"><span class="sw"></span><span class="code">${escapeHtml(s.code)}</span>
          <span class="lbl">${escapeHtml(s.label)}</span></div>
        <div class="life-row"><span class="k">life</span><span class="bar"><i data-f="life-bar"></i></span><span class="v" data-f="life-v"></span></div>
        <div class="life-row board"><span class="k">board</span><span class="bar"><i style="width:${pct(s.p)}"></i></span><span class="v">${pct(s.p)}</span></div>
        <div class="life-tags small" data-f="tags"></div>
        <details class="life-ev"${open ? " open" : ""}><summary>${accounting(s)}</summary>${evidenceHtml(s)}</details>
      </div>`;
    }).join("");
  // rebuild only when something shown changed, so clicks land on live nodes
  if (html !== els.side.dataset.html) { els.side.innerHTML = html; els.side.dataset.html = html; }
  markSelected();
  updateSide();
}

// Per generation: territory bars and lead tags only, so clicks never land on a rebuilt node.
function updateSide() {
  if (!els || !sim || !sim.species.length) return;
  const { board, life } = leaders();
  sim.species.forEach((s, i) => {
    const card = els.side.querySelector(`[data-id="${CSS.escape(s.id)}"]`);
    if (!card) return;
    const share = sim.total ? sim.counts[i] / sim.total : 0;
    card.querySelector('[data-f="life-bar"]').style.width = pct(share);
    card.querySelector('[data-f="life-v"]').textContent = pct(share);
    const tags = [i === life ? "life lead" : "", i === board ? "board lead" : "", s.offboard ? "off board" : ""].filter(Boolean);
    const html = tags.map((t) => `<span>${t}</span>`).join("");
    const el = card.querySelector('[data-f="tags"]');
    if (el.innerHTML !== html) el.innerHTML = html;
  });
}

function markSelected() {
  if (!els) return;
  els.side.querySelectorAll(".life-sp").forEach((c) => c.classList.toggle("sel", c.dataset.id === ctx.selected));
}

function renderRead() {
  if (!els || !sim) return;
  const sp = sim.species;
  let text;
  if (!sp.length) text = "Nothing to grow: this pack defines no hypotheses.";
  else if (!sim.anyEvidence) {
    text = sim.connected
      ? "No hypothesis has a support term above zero yet, so nothing seeds. Connect more sources (keys 1–9), then reseed."
      : "No sources connected: no evidence, so nothing seeds. Connect sources with keys 1–9, then reseed.";
  } else if (!sim.total) {
    text = sim.slice < lastSlice()
      ? `Nothing alive yet at ${sliceTime(sim.slice)}: evidence so far is too thin to plant.`
      : `Every species died out by generation ${sim.gen}. Sparse seeds do not survive B3/S23: thin support does not persist.`;
  } else {
    const { board, life } = leaders();
    const L = sp[life], B = sp[board];
    const share = num(Math.round((sim.counts[life] / sim.total) * 100));
    text = life === board
      ? `Life agrees with the board: ${spRef(L)} ${escapeHtml(L.label)} leads both, ${share}% of territory against ${num(Math.round(L.p * 100))}% belief.`
      : `Life and the board disagree: ${spRef(L)} ${escapeHtml(L.label)} holds the most territory (${share}%), while the board leads with ${spRef(B)} ${escapeHtml(B.label)} (${num(Math.round(B.p * 100))}%). Same evidence, a different aggregation rule.`;
    text += ` ${evidenceLine(L, life === board ? null : B, life)}`;
  }
  if (!els.nudge.hidden) text += ` <span class="dim">This run is from an earlier board.</span>`;
  // rebuild only when the words change, so a click on a code is never lost
  // mid-run; otherwise just refresh the numbers in place
  const shape = text.replace(NUM_RE, '<span class="life-num"></span>');
  if (shape !== els.read.dataset.shape) { els.read.innerHTML = text; els.read.dataset.shape = shape; return; }
  const values = [...text.matchAll(NUM_RE)].map((m) => m[1]);
  els.read.querySelectorAll(".life-num").forEach((el, k) => { if (el.textContent !== values[k]) el.textContent = values[k]; });
}

// Numbers that change every generation live in their own spans.
const num = (v) => `<span class="life-num">${v}</span>`;
const NUM_RE = /<span class="life-num">([^<]*)<\/span>/g;

// "H3 is backed by S4, S6, S1; H1 was cut by S4, S6." The strongest terms
// behind the leaders, then the rivals that inhibitors cut by half or more.
function evidenceLine(L, B, life) {
  const top = (list) => list.filter((t) => t.v >= 0.01).slice(0, 3).map(evRef).join(", ");
  const backed = [L, B].filter((s) => s && top(s.support)).map((s, k) => `${spRef(s)} ${k ? "by" : "is backed by"} ${top(s.support)}`);
  let line = backed.length ? `${backed.join("; ")}.` : "";
  const sum = (m) => [...m.values()].reduce((a, n) => a + n, 0);
  const cut = sim.species
    .filter((s, i) => i !== life && top(s.inhibit) && (s.offboard || (sum(s.planted) && sum(s.removed) >= sum(s.planted) / 2)))
    .sort((a, b) => b.offboard - a.offboard || sum(b.removed) - sum(a.removed))
    .slice(0, 2);
  if (cut.length) line += ` ${cut.map((s, k) => `${spRef(s)} ${k ? "by" : "was cut by"} ${top(s.inhibit)}`).join("; ")}.`;
  return line;
}

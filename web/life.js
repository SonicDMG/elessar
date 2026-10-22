// Life mode: competing species. Each hypothesis is a species in Conway's Game
// of Life (B3/S23, Immigration colouring), seeded from the same derivation the
// board uses. Same evidence, a different aggregation rule: the winner can differ.
import { escapeHtml } from "./board.js";

const COLS = 64;
const ROWS = 36;
const CELL = 4;      // native pixels per cell: 3 lit + 1 gap
const LABEL_H = 8;   // strip above the grid for the band codes
const W = COLS * CELL;
const H = LABEL_H + ROWS * CELL;

// Seeding: each support term (w × p) fills that share × SEED_DENSITY of the
// home band; the band never starts fuller than MAX_FILL (denser random soups
// overcrowd and die, so more support must never mean fewer survivors). Each inhibitor
// (w × p) then removes that share of the seeds, like the board's Π(1 − v·p).
const SEED_DENSITY = 0.35;
const MAX_FILL = 0.4;

const SPEEDS = [{ label: "1x", gps: 4 }, { label: "2x", gps: 8 }, { label: "4x", gps: 16 }, { label: "8x", gps: 32 }];
const CYCLE_WINDOW = 60; // generations remembered to notice a settled run

// Species colours in pack order; the first is the board's hypothesis magenta.
const PALETTE = ["#ff5fd2", "#5fb8ff", "#9cf06a", "#ffb04a", "#b48cff", "#ff6b6b", "#5ff0d8", "#f0e45f", "#e0e8f0"];

let root = null;
let ctx = null;
let els = null;
let sim = null;
let timer = null;
let playing = false;
let speed = 1;
let hover = null;
let observer = null;
let onKey = null;

// ------------------------------------------------------------------ mode interface

export function mount(el, context) {
  root = el;
  ctx = context;
  sim = null;
  playing = false;
  hover = null;
  root.innerHTML = `
    <div class="life">
      <div class="life-bar">
        <button data-act="play" class="life-play">play</button>
        <button data-act="step">step</button>
        <button data-act="reset" title="Back to generation 0 of this run">reset</button>
        <span class="life-speed"><span class="dim">speed</span>${SPEEDS.map((s, i) => `<button data-speed="${i}">${s.label}</button>`).join("")}</span>
        <span class="life-gen">GEN <b>0</b></span>
        <span class="life-status dim"></span>
        <span class="life-nudge" hidden>board changed <button data-act="reseed" class="gold">reseed</button></span>
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
      <p class="life-note dim small">B3/S23. A born cell takes the majority colour of its three parents (all different: the higher board belief wins).
        Seeds: each support term w×p fills that share ×${SEED_DENSITY} of the species' band; each inhibitor w×p removes that share of its seeds.</p>
    </div>`;
  const q = (s) => root.querySelector(s);
  els = {
    life: q(".life"), stage: q(".life-stage"), wrap: q(".life-wrap"), grid: q(".life-grid"), glow: q(".life-glow"),
    tip: q(".tip"), side: q(".life-side"), read: q(".life-read"), gen: q(".life-gen b"), status: q(".life-status"),
    nudge: q(".life-nudge"), play: q(".life-play"),
  };
  els.g = els.grid.getContext("2d");
  els.gg = els.glow.getContext("2d");

  q(".life-bar").addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    if (b.dataset.speed) { speed = Number(b.dataset.speed); if (playing) startTimer(); renderBar(); return; }
    const act = b.dataset.act;
    if (act === "play") setPlaying(!playing);
    if (act === "step") { setPlaying(false); step(); }
    if (act === "reset") { setPlaying(false); restart(); }
    if (act === "reseed") { seed(ctx.state); setPlaying(sim.seeded > 0); }
  });
  els.grid.addEventListener("mousemove", onHover);
  els.grid.addEventListener("mouseleave", () => { hover = null; els.tip.hidden = true; draw(); });
  els.grid.addEventListener("click", (e) => {
    const s = speciesAt(e);
    if (s) ctx.select(s.id);
    draw();
    renderSide();
  });
  els.side.addEventListener("click", (e) => {
    const row = e.target.closest("[data-id]");
    if (row) { ctx.select(row.dataset.id); draw(); renderSide(); }
  });
  // Escape clears the selection in app.js without an update; follow it.
  onKey = () => setTimeout(() => { if (sim) { draw(); renderSide(); } }, 0);
  document.addEventListener("keydown", onKey);
  observer = new ResizeObserver(() => fit());
  observer.observe(root);
}

export function update(state) {
  if (!root || !state) return;
  if (!sim || sim.scenario !== state.scenario.id) {
    // a fresh mount or another pack: nothing to disturb, seed straight away
    seed(state);
    setPlaying(sim.seeded > 0);
  }
  els.nudge.hidden = fingerprint(state) === sim.fp;
  renderSide();
  renderRead();
}

export function unmount() {
  stopTimer();
  if (observer) observer.disconnect();
  if (onKey) document.removeEventListener("keydown", onKey);
  observer = null;
  onKey = null;
  root = null;
  els = null;
  sim = null;
}

// ------------------------------------------------------------------ seeding

// The board, reduced to what seeding reads: every hypothesis belief and term.
function fingerprint(state) {
  const parts = [state.scenario.id];
  for (const n of hypotheses(state)) {
    const b = (n.detail && n.detail.breakdown) || {};
    parts.push(`${n.id}:${n.p.toFixed(4)}:${(b.support || []).map((t) => t.term.toFixed(4))}:${(b.inhibit || []).map((t) => (t.w * t.p).toFixed(4))}`);
  }
  return parts.join("|");
}

const hypotheses = (state) => state.nodes.filter((n) => n.band === "hypothesis");

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

function seed(state) {
  const fp = fingerprint(state);
  const rand = prng(hash(fp));
  const hyps = hypotheses(state);
  const grid = new Uint8Array(COLS * ROWS);
  const species = hyps.map((n, i) => {
    const x0 = Math.round((i * COLS) / hyps.length);
    const x1 = Math.round(((i + 1) * COLS) / hyps.length);
    const b = (n.detail && n.detail.breakdown) || {};
    const support = (b.support || []).map((t) => t.term);
    const inhibit = (b.inhibit || []).map((t) => t.w * t.p);

    // home band cells in a seeded random order; support terms take the next
    // share of them, inhibitors drop a share of what was taken
    const cells = [];
    for (let r = 0; r < ROWS; r++) for (let c = x0; c < x1; c++) cells.push(r * COLS + c);
    shuffle(cells, rand);
    const cap = Math.floor(cells.length * MAX_FILL);
    let taken = 0;
    for (const term of support) taken = Math.min(cap, taken + Math.round(term * SEED_DENSITY * cells.length));
    let seeds = cells.slice(0, taken);
    for (const v of inhibit) {
      const drop = Math.round(v * seeds.length);
      seeds = shuffle(seeds, rand).slice(drop);
    }
    for (const at of seeds) grid[at] = i + 1;
    return {
      id: n.id, code: n.code, label: n.label, p: n.p, offboard: !!n.offboard, x0, x1, color: PALETTE[i % PALETTE.length],
      supportSum: support.reduce((a, t) => a + t, 0), planted: taken, seeded: seeds.length,
    };
  });
  // ties at birth go to the higher board belief, then pack order
  const rank = species.map((s, i) => i).sort((a, b) => species[b].p - species[a].p || a - b);
  const rankOf = new Uint8Array(species.length + 1);
  rank.forEach((s, k) => { rankOf[s + 1] = k; });

  sim = {
    scenario: state.scenario.id, fp, species, rankOf, seedGrid: grid.slice(), grid, born: new Uint8Array(COLS * ROWS),
    gen: 0, seeded: species.reduce((a, s) => a + s.seeded, 0),
    connected: state.sources.some((s) => s.connected), counts: [], settled: null, seen: new Map(),
  };
  for (const s of species) s.styles = styles(s.color, s.offboard);
  els.nudge.hidden = true;
  count();
  fit();
  renderAll();
}

// Back to generation 0 of this run (same seed, not the current board).
function restart() {
  if (!sim) return;
  sim.grid = sim.seedGrid.slice();
  sim.born.fill(0);
  sim.gen = 0;
  sim.settled = null;
  sim.seen = new Map();
  count();
  renderAll();
}

// ------------------------------------------------------------------ the automaton

function step() {
  if (!sim || sim.settled) return;
  const { grid, rankOf } = sim;
  const next = new Uint8Array(COLS * ROWS);
  const born = new Uint8Array(COLS * ROWS);
  const nb = [0, 0, 0];
  for (let r = 0; r < ROWS; r++) {
    for (let c = 0; c < COLS; c++) {
      // bounded grid: outside the edge is dead
      let n = 0;
      for (let dr = -1; dr <= 1; dr++) {
        const rr = r + dr;
        if (rr < 0 || rr >= ROWS) continue;
        for (let dc = -1; dc <= 1; dc++) {
          const cc = c + dc;
          if ((dr === 0 && dc === 0) || cc < 0 || cc >= COLS) continue;
          const s = grid[rr * COLS + cc];
          if (s) { if (n < 3) nb[n] = s; n++; }
        }
      }
      const i = r * COLS + c;
      const s = grid[i];
      if (s && (n === 2 || n === 3)) next[i] = s;
      else if (!s && n === 3) {
        // Immigration: majority of the three parents; all different → best rank
        const [a, b, d] = nb;
        next[i] = a === b || a === d ? a : b === d ? b : [a, b, d].sort((x, y) => rankOf[x] - rankOf[y])[0];
        born[i] = 1;
      }
    }
  }
  sim.grid = next;
  sim.born = born;
  sim.gen++;
  count();

  // a run that repeats itself (still life, oscillator, or extinction) is settled
  const h = hashGrid(next);
  if (sim.total === 0) sim.settled = { kind: "extinct", gen: sim.gen };
  else if (sim.seen.has(h)) sim.settled = { kind: "settled", gen: sim.gen, period: sim.gen - sim.seen.get(h) };
  sim.seen.set(h, sim.gen);
  if (sim.seen.size > CYCLE_WINDOW) sim.seen.delete(sim.seen.keys().next().value);
  if (sim.settled) setPlaying(false);
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
  playing = !!on && !!sim && sim.seeded > 0 && !sim.settled;
  if (playing) startTimer(); else stopTimer();
  renderBar();
}

function startTimer() {
  stopTimer();
  timer = setInterval(step, 1000 / SPEEDS[speed].gps);
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
    hi: css(mix(base, [255, 255, 255], 0.35)),
    born: css(mix(base, [255, 255, 255], 0.55)),
    dead: css(mix(COLORS.dark, color, 0.07)),
    label: css(offboard ? mix(base, COLORS.bg, 0.3) : base),
    glow: css(base, 0.55),
  };
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
  const k = Math.max(1, Math.ceil(scale * (window.devicePixelRatio || 1)));
  if (els.grid.width !== W * k) {
    for (const c of [els.grid, els.glow]) { c.width = W * k; c.height = H * k; }
    els.g.setTransform(k, 0, 0, k, 0, 0);
    els.gg.setTransform(k, 0, 0, k, 0, 0);
  }
  els.wrap.style.width = `${cssW}px`;
  els.wrap.style.height = `${cssH}px`;
  els.wrap.style.setProperty("--scan", `${scale}px`);
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

  // band codes along the top, underlined for the selected species
  for (const s of sim.species) {
    const bx = s.x0 * CELL, bw = (s.x1 - s.x0) * CELL - 1;
    drawText(g, s.code, bx + Math.floor((bw - (s.code.length * 4 - 1)) / 2), 1, s.styles.label);
    if (s.id === selected) { g.fillStyle = s.styles.live; g.fillRect(bx, LABEL_H - 1, bw, 1); }
  }

  // cells: live in species colour (newborns brighter), dead tinted by band
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
      g.fillStyle = sim.born[i] ? st.born : st.live;
      g.fillRect(x, y, 3, 3);
      g.fillStyle = st.hi;
      g.fillRect(x, y, 1, 1);
      gg.fillStyle = st.glow;
      gg.fillRect(x, y, 3, 3);
    }
  }

  // band seams, and a dotted frame around the selected species' home band
  g.fillStyle = "rgba(255,255,255,0.10)";
  for (const s of sim.species.slice(1)) for (let y = LABEL_H; y < H; y += 2) g.fillRect(s.x0 * CELL - 1, y, 1, 1);
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

function onHover(e) {
  if (!sim) return;
  const at = cellAt(e);
  const s = speciesAt(e);
  hover = at.r >= 0 ? at : null;
  els.grid.style.cursor = s ? "pointer" : "default";
  if (!s) { els.tip.hidden = true; draw(); return; }
  const live = at.r >= 0 && sim.grid[at.r * COLS + at.c];
  const i = sim.species.indexOf(s);
  const share = sim.total ? Math.round((sim.counts[i] / sim.total) * 100) : 0;
  els.tip.innerHTML = `<b>${s.code}</b> ${escapeHtml(s.label)}<br><span>${live ? "live cell" : "home band"} · ${sim.counts[i]} cells, ${share}% of territory</span>`
    + `<br><span>seeded ${s.seeded} (support ${s.supportSum.toFixed(2)} → ${s.planted}, inhibitors −${s.planted - s.seeded})</span>`;
  const wr = els.wrap.getBoundingClientRect();
  const left = e.clientX - wr.left + 14;
  els.tip.hidden = false;
  els.tip.style.left = `${Math.min(left, wr.width - els.tip.offsetWidth - 4)}px`;
  els.tip.style.top = `${e.clientY - wr.top + 12}px`;
  draw();
}

// ------------------------------------------------------------------ readout

function renderAll() {
  draw();
  renderBar();
  renderSide();
  renderRead();
}

function renderBar() {
  if (!els) return;
  els.play.textContent = playing ? "pause" : "play";
  els.play.classList.toggle("on", playing);
  els.play.disabled = !sim || !sim.seeded || !!sim.settled;
  root.querySelectorAll("[data-speed]").forEach((b) => b.classList.toggle("on", Number(b.dataset.speed) === speed));
  if (!sim) return;
  els.gen.textContent = String(sim.gen).padStart(4, "0");
  let status = `${sim.total} live`;
  if (!sim.seeded) status = "nothing seeded";
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

function renderSide() {
  if (!els || !sim) return;
  if (!sim.species.length) { els.side.innerHTML = `<p class="dim">This pack has no hypotheses.</p>`; return; }
  const { board, life } = leaders();
  const stale = !els.nudge.hidden;
  const pct = (v) => `${Math.round(v * 100)}%`;
  els.side.innerHTML = `<h3>Species <span class="dim">life territory · board belief${stale ? " at seed" : ""}</span></h3>`
    + sim.species.map((s, i) => {
      const share = sim.total ? sim.counts[i] / sim.total : 0;
      const tags = [i === life ? "life lead" : "", i === board ? "board lead" : "", s.offboard ? "off board" : ""].filter(Boolean);
      return `<div class="life-sp${s.id === ctx.selected ? " sel" : ""}" data-id="${escapeHtml(s.id)}" style="--c:${s.color}">
        <div class="life-sp-head"><span class="sw"></span><span class="code">${escapeHtml(s.code)}</span>
          <span class="lbl">${escapeHtml(s.label)}</span></div>
        <div class="life-row"><span class="k">life</span><span class="bar"><i style="width:${pct(share)}"></i></span><span class="v">${pct(share)}</span></div>
        <div class="life-row board"><span class="k">board</span><span class="bar"><i style="width:${pct(s.p)}"></i></span><span class="v">${pct(s.p)}</span></div>
        <div class="life-tags small">${tags.map((t) => `<span>${t}</span>`).join("")}<span class="dim">seeded ${s.seeded}</span></div>
      </div>`;
    }).join("");
}

function renderRead() {
  if (!els || !sim) return;
  const sp = sim.species;
  let text;
  if (!sp.length) text = "Nothing to grow: this pack defines no hypotheses.";
  else if (!sim.seeded) {
    text = sim.connected
      ? "No hypothesis has a support term above zero yet, so nothing seeds. Connect more sources (keys 1–9), then reseed."
      : "No sources connected: no evidence, so nothing seeds. Connect sources with keys 1–9, then reseed.";
  } else if (!sim.total) {
    text = `Every species died out by generation ${sim.gen}. Sparse seeds do not survive B3/S23: thin support does not persist.`;
  } else {
    const { board, life } = leaders();
    const L = sp[life], B = sp[board];
    const share = Math.round((sim.counts[life] / sim.total) * 100);
    text = life === board
      ? `Life agrees with the board: <b style="color:${L.color}">${escapeHtml(L.code)}</b> ${escapeHtml(L.label)} leads both, ${share}% of territory against ${Math.round(L.p * 100)}% belief.`
      : `Life and the board disagree: <b style="color:${L.color}">${escapeHtml(L.code)}</b> ${escapeHtml(L.label)} holds the most territory (${share}%), while the board leads with <b style="color:${B.color}">${escapeHtml(B.code)}</b> ${escapeHtml(B.label)} (${Math.round(B.p * 100)}%). Same evidence, a different aggregation rule.`;
  }
  if (!els.nudge.hidden) text += ` <span class="dim">This run is from an earlier board.</span>`;
  els.read.innerHTML = text;
}

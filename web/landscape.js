// Landscape mode: a spike map on a surface plot. Every evidence term in a
// hypothesis's derivation stands on an isometric floor as a spike (support,
// w × p) or hangs below it as a red pit (inhibitor, w × p). A smoothed surface
// is laid over them, so the best-backed hypothesis reads as the highest ridge.
// Drawn by a tiny software rasterizer at native resolution, back to front
// (painter's order), and scaled up by a whole number with pixelated rendering.
import { escapeHtml as h } from "./board.js";

// Floor layout, in cells: a lane per hypothesis LANE_W cells wide, a row per
// piece of evidence one cell deep, a GAP row between signals and participants,
// a MARGIN of flat floor all round. The mesh samples each cell SUB × SUB times.
const LANE_W = 3;
const GAP = 1;
const MARGIN = 1;
const SUB = 4;

// Smoothing: a compact bump of radius KR cells, (1 − (d/KR)²)², divided by its
// sum over the row lattice, so a run of equal terms makes a plateau at their
// height and a lone spike pokes through a gentler hill. Lanes sit LANE_W
// apart, so the surface never bleeds from one hypothesis into the next.
const KR = 1.5;
const kern = (d) => (d >= KR ? 0 : (1 - (d / KR) ** 2) ** 2);
const KSUM = 1 + 2 * kern(1);

// Projection: isometric 2:1, a cell is a 2·CELL × CELL diamond, a height of 1
// rises HS native pixels.
const CELL = 8;
const HS = 56;
const SLAB = 4;      // floor thickness on the faces toward the viewer
const STEM = 9;      // beacon stem above the crest, at least
const TOP_ROOM = 22; // above the tallest spike: beacons and their labels

const LEVELS = [0.05, 0.1, 0.2, 0.4, 0.6];
const MIN_TERM = 0.01;
const TWEEN_MS = 300;
const PLAY_MS = 850;
const ZOOMS = [1, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10, 12];
const MAX_SCALE = ZOOMS[ZOOMS.length - 1];
const STEPS = 5; // shading bands per unit of light, dithered between

// Light from the viewer's upper left, a little behind, in camera space (a, b, up).
const LIGHT = (() => { const v = [-0.55, -0.3, 0.78]; const n = Math.hypot(...v); return v.map((x) => x / n); })();
const BAYER = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5].map((v) => (v + 0.5) / 16);

// Species colours in pack order, as Life uses them, so a hypothesis keeps its colour across modes.
const PALETTE = ["#ff5fd2", "#5fb8ff", "#9cf06a", "#ffb04a", "#b48cff", "#ff6b6b", "#5ff0d8", "#f0e45f", "#e0e8f0"];

let L = null;        // everything the mounted mode holds; null when unmounted
let fetchSeq = 0;    // drops timeline responses that arrive after unmount or a newer fetch

// ------------------------------------------------------------------ mode interface

export function mount(el, context) {
  el.innerHTML = `
    <div class="ls">
      <div class="ls-bar">
        <span class="ls-cam">
          <button data-cam="left" title="Rotate the floor left (Q)">⟲ Q</button>
          <button data-cam="right" title="Rotate the floor right (E)">E ⟳</button>
          <button data-cam="out" title="Zoom out (−, wheel)">−</button>
          <span class="ls-zoom dim"></span>
          <button data-cam="in" title="Zoom in (+, wheel)">+</button>
          <button data-cam="fit" title="Fit the floor to the view (F)">fit</button>
        </span>
        <span class="ls-time">
          <button class="ls-play" title="Replay the timeline slice by slice">▶ play</button>
          <input class="ls-scrub" type="range" min="0" max="0" value="0" step="1" aria-label="timeline slice">
          <span class="ls-when"></span>
        </span>
      </div>
      <div class="ls-stage">
        <canvas class="ls-canvas"></canvas>
        <div class="ls-labels"><span class="ls-tick"></span><span class="ls-tick"></span></div>
        <div class="ls-scan"></div>
        <p class="ls-note" hidden></p>
        <div class="tip" hidden></div>
      </div>
      <p class="ls-read"></p>
      <p class="ls-legend dim small"></p>
    </div>`;
  const q = (s) => el.querySelector(s);
  L = {
    ctx: context, root: el, state: null, scenario: null, fp: null,
    els: {
      stage: q(".ls-stage"), canvas: q(".ls-canvas"), labels: q(".ls-labels"), ticks: [...el.querySelectorAll(".ls-tick")],
      tip: q(".tip"), note: q(".ls-note"), read: q(".ls-read"), legend: q(".ls-legend"),
      play: q(".ls-play"), scrub: q(".ls-scrub"), when: q(".ls-when"), zoom: q(".ls-zoom"),
    },
    cam: { rot: 0, scale: 0, panX: 0, panY: 0, auto: true },
    lanes: [], rows: [], groups: [], geo: null, layoutKey: "", projCache: new Map(),
    tl: null, at: null, playing: false, timer: null,
    view: null, shown: new Map(), start: new Map(), goal: new Map(), goalSig: "", t0: 0, raf: 0,
    hits: { spikes: [], crests: [], codes: [] }, hover: null, drag: null, wheel: 0,
    img: null, buf: null, NW: 0, NH: 0, ox: 0, oy: 0, beacons: [],
  };
  L.g = L.els.canvas.getContext("2d");
  L.off = document.createElement("canvas"); // the native image, before it is copied up
  L.offCtx = L.off.getContext("2d");
  wire();
}

export function update(state) {
  if (!L || !state) return;
  L.state = state;
  if (L.scenario !== state.scenario.id) {
    // another pack: a fresh camera and nothing to tween from
    L.scenario = state.scenario.id;
    L.cam = { rot: 0, scale: 0, panX: 0, panY: 0, auto: true };
    L.tl = null;
    L.shown = new Map();
  }
  layout(state);
  const fp = fingerprint(state);
  if (fp !== L.fp) {
    // the board moved: back to live, and fetch the timeline for this board
    L.fp = fp;
    pause();
    L.at = null;
    syncTimeline(state);
  }
  retarget();
}

export function unmount() {
  if (!L) return;
  pause();
  if (L.raf) cancelAnimationFrame(L.raf);
  if (L.observer) L.observer.disconnect();
  document.removeEventListener("keydown", L.onKey);
  fetchSeq++;
  L = null;
}

// ------------------------------------------------------------------ the timeline

// Everything the timeline depends on, as Life fingerprints it: sources,
// engine, feedback, and every hypothesis belief and term.
function fingerprint(state) {
  const parts = [state.scenario.id, state.engine, state.feedback, state.sources.filter((s) => s.connected).map((s) => s.id).join(",")];
  for (const n of hypotheses(state)) {
    const b = breakdown(n);
    parts.push(`${n.id}:${n.p.toFixed(4)}:${(b.support || []).map((t) => t.term.toFixed(4))}:${(b.inhibit || []).map((t) => (t.w * t.p).toFixed(4))}`);
  }
  return parts.join("|");
}

const hypotheses = (state) => state.nodes.filter((n) => n.band === "hypothesis");
const breakdown = (n) => (n.detail && n.detail.breakdown) || {};

// The board re-inferred at every event time (GET /api/timeline). Until it
// arrives, or if it fails, only the live end is shown.
function syncTimeline(state) {
  const seq = ++fetchSeq;
  L.tl = { status: "loading", slices: null };
  const done = (status, d) => {
    if (!L || seq !== fetchSeq) return;
    L.tl = status === "ok"
      ? { status, slices: d.slices, policy: d.unknown_time || state.unknown_time || "throughout", events: d.event_count || 0, undated: d.undated_count || 0 }
      : { status, slices: null };
    const sl = L.tl.slices;
    const ts = sl ? sl.filter((x) => x.t != null).map((x) => x.t) : [];
    const span = ts.length ? ts[ts.length - 1] - ts[0] : 0;
    L.clock = { years: span > 300 * 86400e3, days: span > 20 * 3600e3, dateOnly: ts.length > 0 && ts.every((t) => t % 86400e3 === 0) };
    if (L.at != null && (!sl || L.at >= sl.length)) L.at = null;
    layout(L.state); // rows also cover evidence that only appears earlier in the timeline
    retarget();
  };
  fetch(`/api/timeline?scenario=${encodeURIComponent(state.scenario.id)}`)
    .then((r) => (r.ok ? r.json() : Promise.reject(new Error(r.statusText))))
    .then((d) => done(Array.isArray(d.slices) && d.slices.length ? "ok" : "error", d))
    .catch(() => done("error", null));
}

const slices = () => (L.tl && L.tl.status === "ok" ? L.tl.slices : null);

function setAt(j) {
  L.at = j;
  retarget();
}

// Playing steps one slice at a time and ends on the live board.
function play() {
  const sl = slices();
  if (!sl) return;
  if (L.playing) return pause();
  L.playing = true;
  if (L.at == null || L.at >= sl.length - 1) setAt(0); else renderBar();
  const step = () => {
    if (!L || !L.playing) return;
    const all = slices();
    if (!all || L.at == null) return pause();
    if (L.at + 1 >= all.length) { L.playing = false; setAt(null); return; }
    setAt(L.at + 1);
    L.timer = setTimeout(step, PLAY_MS);
  };
  L.timer = setTimeout(step, PLAY_MS);
}

function pause() {
  if (!L) return;
  L.playing = false;
  clearTimeout(L.timer);
  L.timer = null;
  renderBar();
}

// A slice's time as Topology's clock shows it: the date when the timeline
// spans years, the month and day when it spans days, the time unless every
// record is dated by day only. Slice 0 and the undated-arrival slice say what they are.
function sliceTime(sl) {
  if (sl.undated_arrive) return "after the last timed record";
  if (!sl.at) return "before any timed record";
  const c = L.clock || {}, d = c.years ? sl.at.slice(0, 10) : c.days ? sl.at.slice(5, 10) : "";
  return [d, c.dateOnly ? "" : sl.at.slice(11, 19)].filter(Boolean).join(" ");
}

// Undated records a slice brings in: all of them at slice 0 under
// "throughout", or the arrival slice under "after".
const undatedIn = (sl) => (!sl ? [] : sl.undated_arrive ? sl.undated || [] : sl.index === 0 ? sl.undated || [] : []);

// Signals whose every hit is an undated record, and participants only undated
// records name: their spikes say "from undated evidence".
function undatedOnly(state) {
  const undated = state.undated || [];
  const keys = new Set(undated.map((u) => `${u.source}|${u.pointer}`));
  const dated = new Set((state.events || []).flatMap((e) => e.participants || []));
  const out = new Set(undated.flatMap((u) => u.participants || []).filter((id) => !dated.has(id)));
  if (!keys.size) return out;
  for (const n of state.nodes) {
    if (n.band !== "signal") continue;
    const recs = ((n.detail && n.detail.hits) || []).flatMap((x) => x.records || []);
    if (recs.length && recs.every((r) => keys.has(`${r.source}|${r.pointer}`))) out.add(n.id);
  }
  return out;
}

// ------------------------------------------------------------------ layout

// Lanes left to right so hypotheses sharing the most support sit side by
// side (Life's band rule): start from the heaviest pair, then keep attaching
// the hypothesis that shares most with either end. Ties keep pack order.
function bandOrder(ids, sup) {
  const s = ids.map((id) => sup.get(id) || new Map());
  const weight = (a, b) => {
    let w = 0;
    for (const [from, term] of s[a]) if (s[b].has(from)) w += term + s[b].get(from);
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

// Lanes and rows from the live board. A row exists for every piece of
// evidence in some derivation (live or earlier in the timeline): signals by
// source in rack order, then participants by object type.
function layout(state) {
  const { COLORS, mix } = L.ctx.sprites;
  const hyps = hypotheses(state);
  const sup = new Map(hyps.map((n) => [n.id, new Map((breakdown(n).support || []).map((t) => [t.from, t.term]))]));
  L.lanes = bandOrder(hyps.map((n) => n.id), sup).map((i) => {
    const n = hyps[i];
    return { id: n.id, code: n.code, label: n.label, color: PALETTE[i % PALETTE.length] };
  });

  const used = new Set();
  for (const n of hyps) for (const t of [...(breakdown(n).support || []), ...(breakdown(n).inhibit || [])]) used.add(t.from);
  for (const sl of slices() || []) {
    for (const hy of Object.values(sl.hypotheses || {})) for (const t of [...(hy.support || []), ...(hy.inhibit || [])]) used.add(t.from);
  }
  const rack = new Map(state.sources.map((s, i) => [s.id, i]));
  const types = Object.keys(state.object_types || {});
  const groupOf = (n) => {
    if (n.band === "signal") {
      const src = ((n.detail && n.detail.sources) || []).filter((s) => rack.has(s)).sort((a, b) => rack.get(a) - rack.get(b))[0];
      const s = src != null ? state.sources[rack.get(src)] : null;
      return { key: `s:${src}`, rank: s ? rack.get(src) : 999, name: s ? s.name : "no source", color: s ? s.color : COLORS.grey, band: "signal" };
    }
    const t = (n.detail && n.detail.type) || "other";
    const ti = types.indexOf(t);
    // participant types alternate between two shades of the band's cyan
    return { key: `p:${t}`, rank: 1000 + (ti < 0 ? 999 : ti), name: t, band: "participant",
             color: hex(mix(COLORS.participant, COLORS.bg, (Math.max(0, ti) % 2) * 0.4)) };
  };
  const rows = [];
  state.nodes.forEach((n, k) => {
    if (n.band !== "hypothesis" && used.has(n.id)) rows.push({ id: n.id, code: n.code, label: n.label, band: n.band, k, group: groupOf(n) });
  });
  rows.sort((a, b) => a.group.rank - b.group.rank || a.k - b.k);
  let v = 0;
  rows.forEach((r, j) => { if (j && r.band !== rows[j - 1].band) v += GAP; r.v = v++; });
  L.rows = rows;
  L.groups = rows.filter((r, j) => !j || r.group.key !== rows[j - 1].group.key).map((r) => r.group);
  L.undated = undatedOnly(state);

  const key = [state.scenario.id, L.lanes.map((l) => l.id).join(","), rows.map((r) => r.id).join(",")].join("|");
  if (key === L.layoutKey) return;
  L.layoutKey = key;
  L.geo = buildGeo(L.lanes, rows);
  L.projCache.clear();
  L.cam.panX = L.cam.panY = 0;
  resize();
}

// The sample mesh over the floor: for every vertex, which cells' terms reach
// it through the kernel and with what weight; for every quad, its lane and row.
function buildGeo(lanes, rows) {
  const nl = lanes.length, nr = rows.length;
  const UW = Math.max(1, nl) * LANE_W + 2 * MARGIN;
  const VD = (nr ? rows[nr - 1].v + 1 : 6) + 2 * MARGIN; // no rows yet: a plain floor
  const NU = UW * SUB, NV = VD * SUB, NX = NU + 1;
  const nVert = NX * (NV + 1);
  const laneC = (i) => MARGIN + i * LANE_W + LANE_W / 2;
  const rowC = rows.map((r) => MARGIN + r.v + 0.5);
  const start = new Int32Array(nVert + 1), idx = [], wt = [];
  for (let iv = 0; iv <= NV; iv++) {
    for (let iu = 0; iu <= NU; iu++) {
      start[iv * NX + iu] = idx.length;
      const u = iu / SUB, v = iv / SUB;
      const i = Math.floor((u - MARGIN) / LANE_W);
      if (i < 0 || i >= nl) continue;
      const du = u - laneC(i);
      if (Math.abs(du) >= KR) continue;
      rowC.forEach((c, j) => {
        const k = kern(Math.hypot(du, v - c));
        if (k > 0) { idx.push(i * nr + j); wt.push(k / KSUM); }
      });
    }
  }
  start[nVert] = idx.length;
  const qLane = new Int16Array(NU).fill(-1), qRow = new Int16Array(NV).fill(-1);
  const uIn = new Uint8Array(NU), vIn = new Uint8Array(NV);
  for (let iu = 0; iu < NU; iu++) {
    const u = (iu + 0.5) / SUB - MARGIN;
    uIn[iu] = u >= 0 && u < UW - 2 * MARGIN ? 1 : 0;
    const i = Math.floor(u / LANE_W);
    if (u >= 0 && i < nl) qLane[iu] = i;
  }
  for (let iv = 0; iv < NV; iv++) {
    const v = (iv + 0.5) / SUB - MARGIN;
    vIn[iv] = v >= 0 && v < VD - 2 * MARGIN ? 1 : 0;
    qRow[iv] = rows.findIndex((r) => v >= r.v && v < r.v + 1);
  }
  const cellVert = new Int32Array(nl * nr), cellKey = [];
  for (let i = 0; i < nl; i++) {
    for (let j = 0; j < nr; j++) {
      cellVert[i * nr + j] = Math.round(rowC[j] * SUB) * NX + Math.round(laneC(i) * SUB);
      cellKey.push(`${lanes[i].id}|${rows[j].id}`);
    }
  }
  return { nl, nr, UW, VD, NU, NV, NX, nVert, start, idx: Int32Array.from(idx), wt: Float32Array.from(wt),
           qLane, qRow, uIn, vIn, cellVert, cellKey, laneC, rowC };
}

// Four rotation stops about the floor's centre.
function rotAB(du, dv, r) {
  return r === 0 ? [du, dv] : r === 1 ? [-dv, du] : r === 2 ? [-du, -dv] : [dv, -du];
}

// Per rotation: every vertex's floor position on screen and its depth (a + b,
// larger is nearer), the painter's draw list (quads and spikes back to front)
// and the bounding box used to fit the view.
function proj(rot) {
  if (L.projCache.has(rot)) return L.projCache.get(rot);
  const g = L.geo;
  const cu = g.UW / 2, cv = g.VD / 2;
  const x = new Float32Array(g.nVert), y = new Float32Array(g.nVert), d = new Float32Array(g.nVert);
  for (let iv = 0; iv <= g.NV; iv++) {
    for (let iu = 0; iu <= g.NU; iu++) {
      const vi = iv * g.NX + iu;
      const [a, b] = rotAB(iu / SUB - cu, iv / SUB - cv, rot);
      x[vi] = (a - b) * CELL;
      y[vi] = ((a + b) * CELL) / 2;
      d[vi] = a + b;
    }
  }
  const nq = g.NU * g.NV, nc = g.nl * g.nr;
  const keys = new Float64Array(nq + nc), codes = new Int32Array(nq + nc);
  for (let q = 0; q < nq; q++) {
    const v00 = Math.floor(q / g.NU) * g.NX + (q % g.NU);
    keys[q] = (d[v00] + d[v00 + g.NX + 1]) / 2;
    codes[q] = q;
  }
  // a spike draws after the quads level with its foot, before the ones in front
  for (let c = 0; c < nc; c++) { keys[nq + c] = d[g.cellVert[c]] + 1e-3; codes[nq + c] = -1 - c; }
  const order = Int32Array.from([...codes.keys()].sort((p, q) => keys[p] - keys[q] || p - q).map((k) => codes[k]));
  const corners = [0, g.NU, g.NV * g.NX, g.NV * g.NX + g.NU];
  const xs = corners.map((c) => x[c]), ys = corners.map((c) => y[c]);
  const box = { x0: Math.min(...xs) - 6, x1: Math.max(...xs) + 6, y0: Math.min(...ys) - HS - TOP_ROOM, y1: Math.max(...ys) + SLAB + 8 };
  const far = corners.reduce((m, c) => (d[c] < d[m] ? c : m), corners[0]);
  const P = { x, y, d, order, box, cx: (box.x0 + box.x1) / 2, cy: (box.y0 + box.y1) / 2, far };
  L.projCache.set(rot, P);
  return P;
}

// ------------------------------------------------------------------ values and tweening

// The board at the scrubber's position: per lane belief and presence, per
// cell the support term and the inhibitor cut with their w and p.
function viewData() {
  const sl = L.at == null ? null : (slices() || [])[L.at];
  const lanes = new Map(), cells = new Map();
  for (const n of hypotheses(L.state)) {
    let p, present, sup, inh;
    if (sl) {
      const hy = sl.hypotheses[n.id] || {};
      p = hy.p || 0;
      present = hy.present !== false;
      sup = hy.support || [];
      inh = (hy.inhibit || []).map((t) => ({ ...t, cut: t.cut != null ? t.cut : t.w * t.p }));
    } else {
      const b = breakdown(n);
      p = n.p;
      present = !!n.present;
      sup = b.support || [];
      inh = (b.inhibit || []).map((t) => ({ ...t, cut: t.w * t.p }));
    }
    lanes.set(n.id, { p, present, kept: !!n.kept });
    for (const t of sup) {
      const c = cell(cells, `${n.id}|${t.from}`);
      c.s = t.term; c.sw = t.w; c.sp = t.p;
    }
    for (const t of inh) {
      const c = cell(cells, `${n.id}|${t.from}`);
      c.i = t.cut; c.iw = t.w; c.ip = t.p;
    }
  }
  return { slice: sl, lanes, cells };
}

function cell(map, key) {
  if (!map.has(key)) map.set(key, { s: 0, i: 0 });
  return map.get(key);
}

// New targets: tween what is on screen toward them over TWEEN_MS.
function retarget() {
  if (!L || !L.state) return;
  L.view = viewData();
  const sig = [...L.view.cells].map(([k, v]) => `${k}:${v.s.toFixed(4)}:${v.i.toFixed(4)}`).join(",");
  if (sig !== L.goalSig) {
    L.goalSig = sig;
    L.start = new Map([...L.shown].map(([k, v]) => [k, { ...v }]));
    L.goal = L.view.cells;
    L.t0 = performance.now();
    kick();
  } else draw();
  renderText();
}

function kick() {
  if (L && !L.raf) L.raf = requestAnimationFrame(frame);
}

function frame(now) {
  if (!L) return;
  L.raf = 0;
  const t = Math.min(1, (now - L.t0) / TWEEN_MS);
  const e = t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2;
  const keys = new Set([...L.start.keys(), ...L.goal.keys()]);
  for (const k of keys) {
    const a = L.start.get(k) || { s: 0, i: 0 }, b = L.goal.get(k) || { s: 0, i: 0 };
    L.shown.set(k, { s: a.s + (b.s - a.s) * e, i: a.i + (b.i - a.i) * e });
  }
  draw();
  if (t < 1) kick();
}

// The relief on the cell lattice for a set of cell values: what the surface
// reads at each spike's foot. Used for the crest of each lane and the readout.
function lattice(values) {
  const g = L.geo, out = new Float32Array(g.nl * g.nr);
  const net = (i, j) => {
    const v = values.get(g.cellKey[i * g.nr + j]);
    if (!v) return 0;
    return (v.s >= MIN_TERM ? v.s : 0) - (v.i >= MIN_TERM ? v.i : 0);
  };
  for (let i = 0; i < g.nl; i++) {
    for (let j = 0; j < g.nr; j++) {
      let s = 0;
      for (let k = 0; k < g.nr; k++) s += net(i, k) * kern(Math.abs(g.rowC[j] - g.rowC[k]));
      out[i * g.nr + j] = s / KSUM;
    }
  }
  return out;
}

// Highest ridge: the lane whose surface crest stands highest. Tallest spike
// and deepest pit: the spike at the surface's highest point and the pit at
// its lowest, so a lone big term in a lane the rest of its evidence cancels
// does not outrank a hill the evidence agrees on.
function summary() {
  const g = L.geo, goal = L.goal;
  const field = lattice(goal);
  const out = { ridge: null, spike: null, pit: null, crest: new Map() };
  for (let i = 0; i < g.nl; i++) {
    let best = 0;
    for (let j = 0; j < g.nr; j++) {
      const c = i * g.nr + j, v = goal.get(g.cellKey[c]), f = field[c];
      best = Math.max(best, f);
      if (v && v.s >= MIN_TERM && (!out.spike || f > out.spike.f)) out.spike = { li: i, rj: j, f, v };
      if (v && v.i >= MIN_TERM && (!out.pit || f < out.pit.f)) out.pit = { li: i, rj: j, f, v };
    }
    out.crest.set(L.lanes[i].id, best);
    const p = (L.view.lanes.get(L.lanes[i].id) || {}).p || 0;
    if (best >= MIN_TERM && (!out.ridge || best > out.ridge.f + 1e-9 || (Math.abs(best - out.ridge.f) < 1e-9 && p > out.ridge.p))) {
      out.ridge = { i, f: best, p };
    }
  }
  return out;
}

// ------------------------------------------------------------------ camera

function stageSize() {
  return { w: Math.max(1, L.els.stage.clientWidth), h: Math.max(1, L.els.stage.clientHeight) };
}

function fitScale() {
  const { w, h: hh } = stageSize(), b = proj(L.cam.rot).box;
  return Math.max(1, Math.min(MAX_SCALE, Math.min(w / (b.x1 - b.x0), hh / (b.y1 - b.y0))));
}

// The canvas covers the stage. As on the board, the scale may be fractional,
// so the native image is copied up at a whole number of device pixels per
// native pixel (at or above the target) and the browser scales it down
// slightly: every native pixel ends up the same size.
function resize() {
  if (!L || !L.geo) return;
  const { w, h: hh } = stageSize();
  if (L.cam.auto || !L.cam.scale) { L.cam.scale = fitScale(); L.cam.panX = L.cam.panY = 0; }
  const s = L.cam.scale;
  const NW = Math.ceil(w / s), NH = Math.ceil(hh / s);
  const k = Math.max(1, Math.ceil(s * (window.devicePixelRatio || 1)));
  if (NW !== L.NW || NH !== L.NH) {
    L.NW = NW; L.NH = NH;
    L.off.width = NW;
    L.off.height = NH;
    L.img = L.offCtx.createImageData(NW, NH);
    L.buf = new Uint32Array(L.img.data.buffer);
  }
  if (k !== L.k || L.els.canvas.width !== NW * k || L.els.canvas.height !== NH * k) {
    L.k = k;
    L.els.canvas.width = NW * k;
    L.els.canvas.height = NH * k;
  }
  L.els.canvas.style.width = `${NW * s}px`;
  L.els.canvas.style.height = `${NH * s}px`;
  L.els.stage.style.setProperty("--scan", `${s}px`);
  L.els.zoom.textContent = `×${s < 10 ? s.toFixed(1) : Math.round(s)}`;
  L.hover = null;
  L.els.tip.hidden = true;
  measureLabels();
  draw();
}

function fit() {
  L.cam.auto = true;
  resize();
}

function rotate(dir) {
  L.cam.rot = (L.cam.rot + dir + 4) % 4;
  L.cam.panX = L.cam.panY = 0;
  resize();
}

// Zoom one step along the ladder, keeping the scene point under (cx, cy) in place.
function zoom(dir, cx, cy) {
  const s0 = L.cam.scale;
  const s1 = dir > 0 ? ZOOMS.find((z) => z > s0 + 0.01) : [...ZOOMS].reverse().find((z) => z < s0 - 0.01);
  if (!s1) return;
  const { w, h: hh } = stageSize();
  if (cx == null) { cx = w / 2; cy = hh / 2; }
  const P = proj(L.cam.rot);
  const sx = cx / s0 - L.ox, sy = cy / s0 - L.oy;
  L.cam.scale = s1;
  L.cam.auto = false;
  L.cam.panX = cx / s1 - sx - Math.ceil(w / s1) / 2 + P.cx;
  L.cam.panY = cy / s1 - sy - Math.ceil(hh / s1) / 2 + P.cy;
  resize();
}

// ------------------------------------------------------------------ rasterizer

const hex = (c) => `#${c.map((v) => Math.round(v).toString(16).padStart(2, "0")).join("")}`;
const rgb = (hx) => L.ctx.sprites.mix(hx, hx, 0);
const pack = (c) => (255 << 24) | ((c[2] & 255) << 16) | ((c[1] & 255) << 8) | (c[0] & 255);

function shadeCol(c, f) {
  const { mix } = L.ctx.sprites;
  return f >= 1 ? mix(c, [255, 255, 255], Math.min(1, (f - 1) * 0.55)) : mix(c, [0, 0, 0], Math.min(1, (1 - f) * 0.9));
}

// A filled triangle, pixel centres inside, no anti-aliasing. Two colours
// dithered by a Bayer threshold t; every sixth diagonal hatched when asked.
function tri(x0, y0, x1, y1, x2, y2, cA, cB, t, hatch, cH) {
  const W = L.NW, H = L.NH, buf = L.buf;
  const area = (x1 - x0) * (y2 - y0) - (y1 - y0) * (x2 - x0);
  if (!area) return;
  const s = area > 0 ? 1 : -1;
  const minX = Math.max(0, Math.floor(Math.min(x0, x1, x2))), maxX = Math.min(W - 1, Math.ceil(Math.max(x0, x1, x2)));
  const minY = Math.max(0, Math.floor(Math.min(y0, y1, y2))), maxY = Math.min(H - 1, Math.ceil(Math.max(y0, y1, y2)));
  for (let py = minY; py <= maxY; py++) {
    const cy = py + 0.5;
    for (let px = minX; px <= maxX; px++) {
      const cx = px + 0.5;
      if (((x1 - x0) * (cy - y0) - (y1 - y0) * (cx - x0)) * s < 0) continue;
      if (((x2 - x1) * (cy - y1) - (y2 - y1) * (cx - x1)) * s < 0) continue;
      if (((x0 - x2) * (cy - y2) - (y0 - y2) * (cx - x2)) * s < 0) continue;
      let c = BAYER[((py & 3) << 2) | (px & 3)] < t ? cB : cA;
      if (hatch && (px + py) % 6 === 0) c = cH;
      buf[py * W + px] = c;
    }
  }
}

function px(x, y, c) {
  if (x >= 0 && y >= 0 && x < L.NW && y < L.NH) L.buf[y * L.NW + x] = c;
}

// Bresenham, optionally dotted (every other pixel).
function line(x0, y0, x1, y1, c, dotted) {
  x0 = Math.round(x0); y0 = Math.round(y0); x1 = Math.round(x1); y1 = Math.round(y1);
  const dx = Math.abs(x1 - x0), sx = x0 < x1 ? 1 : -1, dy = -Math.abs(y1 - y0), sy = y0 < y1 ? 1 : -1;
  let err = dx + dy, k = 0;
  for (;;) {
    if (!dotted || !(k++ & 1)) px(x0, y0, c);
    if (x0 === x1 && y0 === y1) break;
    const e2 = 2 * err;
    if (e2 >= dy) { err += dy; x0 += sx; }
    if (e2 <= dx) { err += dx; y0 += sy; }
  }
}

function vline(x, ya, yb, c, dotted) {
  const a = Math.round(Math.min(ya, yb)), b = Math.round(Math.max(ya, yb));
  for (let y = a; y <= b; y++) if (!dotted || (y & 1)) px(x, y, c);
}

function rect(x, y, w, hh, c) {
  for (let j = 0; j < hh; j++) for (let i = 0; i < w; i++) px(x + i, y + j, c);
}

// ------------------------------------------------------------------ drawing

function draw() {
  if (!L || !L.geo || !L.buf || !L.view) return;
  const { COLORS, mix, drawText } = L.ctx.sprites;
  const g = L.geo, P = proj(L.cam.rot), rot = L.cam.rot;
  const ox = Math.round(L.NW / 2 - P.cx + L.cam.panX), oy = Math.round(L.NH / 2 - P.cy + L.cam.panY);
  L.ox = ox; L.oy = oy;
  L.buf.fill(pack(rgb(COLORS.bg)));

  // the values on screen now (mid-tween), and the surface they make
  const nc = g.nl * g.nr;
  const sup = new Float32Array(nc), inh = new Float32Array(nc), net = new Float32Array(nc);
  for (let c = 0; c < nc; c++) {
    const v = L.shown.get(g.cellKey[c]);
    if (!v) continue;
    sup[c] = v.s >= MIN_TERM ? v.s : 0;
    inh[c] = v.i >= MIN_TERM ? v.i : 0;
    net[c] = sup[c] - inh[c];
  }
  const Hv = new Float32Array(g.nVert);
  for (let vi = 0; vi < g.nVert; vi++) {
    let s = 0;
    for (let k = g.start[vi]; k < g.start[vi + 1]; k++) s += g.wt[k] * net[g.idx[k]];
    Hv[vi] = s;
  }
  const X = (vi) => P.x[vi] + ox, Y = (vi, z = Hv[vi]) => P.y[vi] + oy - z * HS;

  // focus: the selected hypothesis's lane, or the selected evidence's row
  const sel = L.ctx.selected;
  const selLane = L.lanes.findIndex((l) => l.id === sel), selRow = L.rows.findIndex((r) => r.id === sel);
  const focus = selLane >= 0 || selRow >= 0;
  const bg = rgb(COLORS.bg), grey = rgb(COLORS.grey), red = rgb(COLORS.red), white = [255, 255, 255];
  const lane = L.lanes.map((l, i) => {
    const info = L.view.lanes.get(l.id) || { present: true };
    const off = !info.present;
    let hue = rgb(l.color);
    if (off) hue = mix(hue, grey, 0.5); // explained away: toward grey, and hatched
    return { hue, off, dim: selLane >= 0 && i !== selLane };
  });
  const dark = rgb(COLORS.dark), light = rgb(COLORS.light);
  const groupRgb = L.rows.map((r) => rgb(r.group.color));

  // a height ruler at the far corner, behind everything
  {
    const vi = P.far, x = Math.round(X(vi)), y0 = Y(vi, 0);
    const c = pack(mix(grey, bg, 0.2));
    vline(x, y0 - HS, y0, c);
    for (let k = 1; k <= 4; k++) { const y = Math.round(y0 - (k / 4) * HS); px(x - 1, y, c); px(x + 1, y, c); if (k % 2 === 0) px(x - 2, y, c); }
    const t = L.els.ticks;
    [0.5, 1].forEach((v, k) => {
      t[k].textContent = v.toFixed(1);
      t[k].style.transform = `translate(${(x + 4) * L.cam.scale}px, ${(y0 - v * HS) * L.cam.scale - 7}px)`;
    });
  }

  const hits = { spikes: [], crests: [], codes: [] };
  const pits = [];
  const hov = L.hover && L.hover.kind === "spike" ? L.hover.c : -1;

  // one mesh quad: floor colour, lifted toward the lane's hue (red below the
  // floor) by its height, lit by its slope, dithered between shading bands
  const quad = (q) => {
    const iu = q % g.NU, iv = (q / g.NU) | 0;
    const v00 = iv * g.NX + iu, v10 = v00 + 1, v01 = v00 + g.NX, v11 = v01 + 1;
    const h00 = Hv[v00], h10 = Hv[v10], h01 = Hv[v01], h11 = Hv[v11];
    const li = g.qLane[iu], rj = g.qRow[iv];
    const cu = Math.floor(iu / SUB), cv = Math.floor(iv / SUB);
    let col = (cu + cv) & 1 ? dark : light;
    const ls = li >= 0 ? lane[li] : null;
    if (ls && g.vIn[iv]) col = mix(col, ls.hue, li === selLane ? 0.12 : 0.05);
    else if (ls) col = mix(dark, ls.hue, 0.22); // the lane's colour on the front and back margin
    else if (rj >= 0 && !g.uIn[iu]) col = mix(dark, groupRgb[rj], 0.22); // the row's group colour on the side margins
    if (rj >= 0 && rj === selRow && g.uIn[iu]) col = mix(col, white, 0.07);
    const e = (h00 + h10 + h01 + h11) / 4;
    const lo = Math.min(h00, h10, h01, h11), hi = Math.max(h00, h10, h01, h11);
    let f = 1;
    if (ls && (hi > 0.002 || lo < -0.002)) {
      const tone = e >= 0 ? ls.hue : mix(red, ls.hue, 0.15);
      col = mix(col, tone, Math.min(0.95, 0.22 + Math.abs(e) * 2.4));
      // slope in native px per native px, turned into camera space, against the light
      const step = (CELL * 1.118) / SUB;
      const gu = (((h10 + h11) - (h00 + h01)) / 2) * HS / step, gv = (((h01 + h11) - (h00 + h10)) / 2) * HS / step;
      const [ga, gb] = rotAB(gu, gv, rot);
      f = (-ga * LIGHT[0] - gb * LIGHT[1] + LIGHT[2]) / Math.hypot(ga, gb, 1) / LIGHT[2];
      f = Math.max(0.3, Math.min(1.7, f));
    }
    if (ls && ls.off) col = mix(col, grey, 0.25);
    if (ls && ls.dim) col = mix(col, bg, 0.45);
    const qf = (f - 1) * STEPS, k = Math.floor(qf), t = qf - k;
    const cA = pack(k === 0 ? col : shadeCol(col, 1 + k / STEPS)), cB = t > 0 ? pack(shadeCol(col, 1 + (k + 1) / STEPS)) : cA;
    // explained-away lanes are hatched on their flat floor; their relief stays readable
    const hatch = !!(ls && ls.off && g.uIn[iu] && g.vIn[iv] && hi < 0.02 && lo > -0.02);
    const cH = hatch ? pack(mix(col, grey, 0.35)) : 0;
    const x00 = X(v00), x10 = X(v10), x01 = X(v01), x11 = X(v11);
    const y00 = Y(v00), y10 = Y(v10), y01 = Y(v01), y11 = Y(v11);
    tri(x00, y00, x10, y10, x11, y11, cA, cB, t, hatch, cH);
    tri(x00, y00, x11, y11, x01, y01, cA, cB, t, hatch, cH);
    // cell grid: both quads beside a cell edge draw it, so the later one keeps it
    const gc = pack(shadeCol(col, Math.min(f, 1) * 0.8));
    if (iv % SUB === 0) line(x00, y00, x10, y10, gc);
    if ((iv + 1) % SUB === 0) line(x01, y01, x11, y11, gc);
    if (iu % SUB === 0) line(x00, y00, x01, y01, gc);
    if ((iu + 1) % SUB === 0) line(x10, y10, x11, y11, gc);
    // contour lines on the relief at fixed levels, red below the floor
    if (!ls || (hi < LEVELS[0] && lo > -LEVELS[0])) return;
    LEVELS.forEach((lv, n) => {
      for (const t of [lv, -lv]) {
        if (t < lo || t > hi) continue;
        const cc = pack(mix(col, t > 0 ? mix(ls.hue, white, 0.6) : [255, 150, 150], 0.4 + n * 0.12));
        contour(t, [[x00, P.y[v00] + oy, h00], [x10, P.y[v10] + oy, h10], [x11, P.y[v11] + oy, h11], [x01, P.y[v01] + oy, h01]], cc, t < 0);
      }
    });
  };

  // one cell's spike (up from the floor, in the lane's hue, capped in the
  // evidence band's colour) and pit (down from it, red)
  const spike = (c) => {
    const s = sup[c], i = inh[c];
    if (!s && !i) return;
    const li = (c / g.nr) | 0, rj = c % g.nr, ls = lane[li], row = L.rows[rj];
    const vi = g.cellVert[c], x = Math.round(X(vi)), y0 = Y(vi, 0);
    const lit = !focus ? 0 : li === selLane || rj === selRow ? 1 : -1;
    const band = rgb(COLORS[row.band] || COLORS.grey);
    const tone = (c0, k) => pack(lit > 0 ? mix(c0, white, 0.35 + k) : lit < 0 ? mix(c0, bg, 0.6) : mix(c0, white, k));
    if (s) {
      const top = Math.round(y0 - s * HS);
      const base = ls.off ? mix(ls.hue, grey, 0.2) : ls.hue;
      vline(x - 1, top, y0, tone(base, 0.25));
      vline(x, top, y0, tone(shadeCol(base, 0.7), 0));
      rect(x - 1, top - 2, 2, 2, tone(band, 0.1));
      if (c === hov || lit > 0) { const w = pack(white); line(x - 2, top - 3, x + 1, top - 3, w); px(x - 2, top - 2, w); px(x + 1, top - 2, w); px(x - 2, top - 1, w); px(x + 1, top - 1, w); }
      hits.spikes.push({ c, kind: "sup", x, ya: top - 3, yb: Math.round(y0), d: P.d[vi] });
    }
    if (i) {
      const bot = Math.round(y0 + i * HS);
      vline(x - 1, y0, bot, tone(red, 0.15));
      vline(x, y0, bot, tone(shadeCol(red, 0.65), 0));
      pits.push({ c, x, y0, bot, band, tone, hl: c === hov || lit > 0 });
      hits.spikes.push({ c, kind: "inh", x, ya: Math.round(y0), yb: bot + 3, d: P.d[vi] });
    }
  };

  for (const code of P.order) if (code >= 0) quad(code); else spike(-1 - code);

  // the floor's edge on the two faces toward the viewer
  const edges = [
    { vs: (k) => k, n: g.NU, out: [0, -1] }, { vs: (k) => g.NV * g.NX + k, n: g.NU, out: [0, 1] },
    { vs: (k) => k * g.NX, n: g.NV, out: [-1, 0] }, { vs: (k) => k * g.NX + g.NU, n: g.NV, out: [1, 0] },
  ];
  for (const ed of edges) {
    const [oa, ob] = rotAB(ed.out[0], ed.out[1], rot);
    if (oa + ob <= 0) continue;
    const c = pack(mix(dark, bg, oa > 0 ? 0.55 : 0.25));
    for (let k = 0; k < ed.n; k++) {
      const p = ed.vs(k), q = ed.vs(k + 1);
      tri(X(p), Y(p), X(q), Y(q), X(q), Y(q) + SLAB, c, c, 0, false, 0);
      tri(X(p), Y(p), X(q), Y(q) + SLAB, X(p), Y(p) + SLAB, c, c, 0, false, 0);
    }
  }

  // pits below the surface stay legible: their hidden part dotted, the cap on top
  for (const p of pits) {
    vline(p.x, p.y0, p.bot, p.tone(shadeCol(red, 0.65), 0.1), true);
    rect(p.x - 1, p.bot + 1, 2, 2, p.tone(p.band, 0.1));
    if (p.hl) { const w = pack(white); line(p.x - 2, p.bot + 3, p.x + 1, p.bot + 3, w); px(p.x - 2, p.bot + 1, w); px(p.x + 1, p.bot + 1, w); px(p.x - 2, p.bot + 2, w); px(p.x + 1, p.bot + 2, w); }
  }

  // beacons: a marker at each lane's crest and a dotted stem up to its label,
  // labels lifted clear of one another
  const placed = [];
  const s = L.cam.scale;
  const crests = L.lanes.map((l, i) => {
    const iu = Math.round(g.laneC(i) * SUB);
    let best = -1, bz = 0.01;
    for (let iv = 0; iv <= g.NV; iv++) { const vi = iv * g.NX + iu; if (Hv[vi] > bz) { bz = Hv[vi]; best = vi; } }
    if (best < 0) { best = Math.round((g.VD / 2) * SUB) * g.NX + iu; bz = 0; }
    return { i, vi: best, z: Math.max(0, bz), x: Math.round(X(best)), y: Math.round(Y(best, Math.max(0, bz))) };
  }).sort((a, b) => a.y - b.y || a.x - b.x);
  for (const cr of crests) {
    const el = L.beacons[cr.i];
    if (!el) continue;
    const w = (el._w || 60) / s, hh = (el._h || 16) / s;
    const box = { x: cr.x - w / 2, y: cr.y - STEM - hh, w, h: hh };
    for (let n = 0; n < 24; n++) {
      const hit = placed.find((b) => box.x < b.x + b.w + 1 && box.x + box.w + 1 > b.x && box.y < b.y + b.h + 1 && box.y + box.h + 1 > b.y);
      if (!hit) break;
      box.y = hit.y - hh - 1;
    }
    placed.push(box);
    const ls = lane[cr.i];
    const c = pack(ls.dim ? mix(ls.hue, bg, 0.5) : mix(ls.hue, white, 0.2));
    vline(cr.x, box.y + hh, cr.y - 2, c, true);
    px(cr.x, cr.y - 1, c); px(cr.x - 1, cr.y, c); px(cr.x + 1, cr.y, c); px(cr.x, cr.y + 1, c);
    px(cr.x, cr.y, pack(white));
    el.style.transform = `translate(${Math.round(box.x * s)}px, ${Math.round(box.y * s)}px)`;
    hits.crests.push({ id: L.lanes[cr.i].id, x: cr.x, y: cr.y });
  }

  L.offCtx.putImageData(L.img, 0, 0);
  L.g.setTransform(1, 0, 0, 1, 0, 0);
  L.g.imageSmoothingEnabled = false;
  L.g.drawImage(L.off, 0, 0, L.NW * L.k, L.NH * L.k);
  L.g.setTransform(L.k, 0, 0, L.k, 0, 0);

  // codes on the margins nearest the viewer: lanes at the end of each lane,
  // evidence at the end of each row
  const depthAt = (u, v) => { const [a, b] = rotAB(u - g.UW / 2, v - g.VD / 2, rot); return a + b; };
  const at = (u, v) => { const [a, b] = rotAB(u - g.UW / 2, v - g.VD / 2, rot); return [Math.round((a - b) * CELL + ox), Math.round(((a + b) * CELL) / 2 + oy)]; };
  const vNear = depthAt(g.UW / 2, g.VD) > depthAt(g.UW / 2, 0) ? g.VD - MARGIN / 2 : MARGIN / 2;
  const uNear = depthAt(g.UW, g.VD / 2) > depthAt(0, g.VD / 2) ? g.UW - MARGIN / 2 : MARGIN / 2;
  const css = (c) => `rgb(${c.map((v) => v | 0).join(",")})`;
  L.lanes.forEach((l, i) => {
    const [x, y] = at(g.laneC(i), vNear), w = l.code.length * 4 - 1;
    drawText(L.g, l.code, x - Math.floor(w / 2), y - 2, css(lane[i].dim ? mix(lane[i].hue, bg, 0.5) : mix(lane[i].hue, white, 0.25)));
    hits.codes.push({ id: l.id, x: x - Math.floor(w / 2) - 1, y: y - 3, w: w + 2, h: 7 });
  });
  L.rows.forEach((r, j) => {
    const [x, y] = at(uNear, g.rowC[j]), w = r.code.length * 4 - 1;
    const c = rgb(COLORS[r.band] || COLORS.grey);
    drawText(L.g, r.code, x - Math.floor(w / 2), y - 2, css(focus && j !== selRow ? mix(c, bg, 0.5) : j === selRow ? mix(c, white, 0.4) : c));
    hits.codes.push({ id: r.id, x: x - Math.floor(w / 2) - 1, y: y - 3, w: w + 2, h: 7 });
  });
  hits.spikes.sort((a, b) => b.d - a.d);
  L.hits = hits;
}

// Marching squares on one quad: where the relief crosses level t, a segment
// at that height, dotted below the floor. Corners are [x, floor y, height] in drawing order.
function contour(t, k, c, dotted) {
  const pts = [];
  for (let e = 0; e < 4; e++) {
    const a = k[e], b = k[(e + 1) % 4];
    if ((a[2] < t) === (b[2] < t)) continue;
    const f = (t - a[2]) / (b[2] - a[2]);
    pts.push([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f - t * HS]);
  }
  if (pts.length >= 2) line(pts[0][0], pts[0][1], pts[1][0], pts[1][1], c, dotted);
  if (pts.length === 4) line(pts[2][0], pts[2][1], pts[3][0], pts[3][1], c, dotted);
}

// ------------------------------------------------------------------ hit testing

function nativeAt(e) {
  const r = L.els.canvas.getBoundingClientRect();
  return { x: (e.clientX - r.left) / L.cam.scale, y: (e.clientY - r.top) / L.cam.scale };
}

// Front-most first: a spike's shaft and cap, a crest marker, a margin code.
function hitTest(e) {
  if (!L.hits) return null;
  const { x, y } = nativeAt(e);
  for (const s of L.hits.spikes) {
    if (x >= s.x - 2.5 && x <= s.x + 1.5 && y >= s.ya - 1 && y <= s.yb + 1) return { kind: "spike", c: s.c, which: s.kind };
  }
  for (const c of L.hits.crests) if (Math.abs(x - c.x - 0.5) <= 2.5 && Math.abs(y - c.y - 0.5) <= 2.5) return { kind: "piece", id: c.id };
  for (const c of L.hits.codes) if (x >= c.x && x <= c.x + c.w && y >= c.y && y <= c.y + c.h) return { kind: "piece", id: c.id };
  return null;
}

const pct = (v) => `${Math.round(v * 100)}%`;
const f2 = (v) => v.toFixed(2);
const node = (id) => L.state.nodes.find((n) => n.id === id) || { id, code: id, label: id, band: "?" };

function spikeTip(hit) {
  const g = L.geo, li = (hit.c / g.nr) | 0, rj = hit.c % g.nr;
  const ln = L.lanes[li], rw = L.rows[rj], v = L.goal.get(g.cellKey[hit.c]) || { s: 0, i: 0 };
  const info = L.view.lanes.get(ln.id) || {};
  const head = `<b>${h(rw.code)}</b> ${h(rw.label)} → <b>${h(ln.code)}</b> ${h(ln.label)}`;
  const body = hit.which === "sup"
    ? ` · ${f2(v.sw)} × ${f2(v.sp)} = ${f2(v.s)}`
    : ` · cuts by ${f2(v.iw)} × ${f2(v.ip)} = ${f2(v.i)}`;
  const where = L.at == null ? "live" : sliceTime(L.view.slice);
  const meta = [hit.which === "sup" ? "support term w × p" : "inhibitor: a pit", `${rw.band} · ${rw.group.name}`, where];
  if (L.undated.has(rw.id)) meta.push("from undated evidence");
  if (!info.present) meta.push(`${ln.code} off board`);
  return `${head}${body}<br><span>${meta.map(h).join(" · ")}</span>`;
}

function pieceTip(id) {
  const ln = L.lanes.find((l) => l.id === id);
  if (ln) {
    const info = L.view.lanes.get(id) || {};
    const crest = L.sum ? L.sum.crest.get(id) || 0 : 0;
    const terms = [...L.goal].filter(([k]) => k.startsWith(`${id}|`)).map(([, v]) => v);
    const ns = terms.filter((v) => v.s >= MIN_TERM).length, ni = terms.filter((v) => v.i >= MIN_TERM).length;
    return `<b>${h(ln.code)}</b> ${h(ln.label)} · belief ${pct(info.p || 0)}${info.present ? "" : " · off board"}<br><span>crest ${f2(crest)} · ${ns} spike${ns === 1 ? "" : "s"}, ${ni} pit${ni === 1 ? "" : "s"}</span>`;
  }
  const rw = L.rows.find((r) => r.id === id);
  if (!rw) return "";
  const n = node(id);
  return `<b>${h(rw.code)}</b> ${h(rw.label)} · ${pct(n.p || 0)}<br><span>${h(rw.band)} · ${h(rw.group.name)}${L.undated.has(id) ? " · undated evidence" : ""}</span>`;
}

function showTip(e, html) {
  const tip = L.els.tip;
  if (!html) { tip.hidden = true; return; }
  tip.innerHTML = html;
  tip.hidden = false;
  const r = L.els.stage.getBoundingClientRect();
  const left = e.clientX - r.left + 14, top = e.clientY - r.top + 14;
  tip.style.left = `${Math.max(0, Math.min(left, r.width - tip.offsetWidth - 4))}px`;
  tip.style.top = `${top + tip.offsetHeight > r.height ? Math.max(0, e.clientY - r.top - tip.offsetHeight - 8) : top}px`;
}

function onHover(e) {
  const hit = hitTest(e);
  const key = hit ? `${hit.kind}:${hit.c ?? hit.id}:${hit.which || ""}` : "";
  L.els.stage.style.cursor = hit ? "pointer" : "";
  if (key !== (L.hoverKey || "")) { L.hoverKey = key; L.hover = hit; draw(); }
  showTip(e, hit ? (hit.kind === "spike" ? spikeTip(hit) : pieceTip(hit.id)) : "");
}

function select(id) {
  L.ctx.select(id);
  draw();
  renderText();
}

// ------------------------------------------------------------------ text

function renderText() {
  if (!L || !L.geo) return;
  L.sum = summary();
  renderBar();
  renderLabels();
  renderRead();
  renderLegend();
  renderNote();
}

function renderBar() {
  if (!L || !L.els) return;
  const sl = slices(), e = L.els;
  e.play.textContent = L.playing ? "❚❚ pause" : "▶ play";
  e.play.classList.toggle("on", L.playing);
  e.play.disabled = !sl || sl.length < 2;
  e.scrub.disabled = !sl;
  e.scrub.max = String(sl ? sl.length : 0);
  e.scrub.value = String(L.at == null ? (sl ? sl.length : 0) : L.at);
  let text;
  if (L.state && !Array.isArray(L.state.events)) text = "timeline: waiting for server data";
  else if (!L.tl || L.tl.status === "loading") text = "live · timeline loading…";
  else if (!sl) text = "live · timeline unavailable";
  else if (L.at == null) text = `<b class="live">LIVE</b> · the board now · ${L.tl.events} event${L.tl.events === 1 ? "" : "s"}`;
  else {
    const x = sl[L.at], und = undatedIn(x).length;
    text = `<b class="replay">REPLAY</b> ${h(sliceTime(x))} · slice ${x.index ?? L.at}/${sl.length - 1} · ${x.events_seen || 0}/${L.tl.events} events`
      + (x.undated_arrive ? ` · <span class="undated">undated evidence arrives</span>`
        : und ? ` · <span class="undated">+ ${und} undated</span>` : "");
  }
  e.when.innerHTML = text;
}

// A clickable code: selects that piece everywhere.
function ref(id, code, color) {
  return `<button class="ls-ref" data-sel="${h(id)}" style="--c:${color}">${h(code)}</button>`;
}
const laneRef = (i) => ref(L.lanes[i].id, L.lanes[i].code, L.lanes[i].color);
const rowRef = (j) => ref(L.rows[j].id, L.rows[j].code, L.ctx.sprites.COLORS[L.rows[j].band] || L.ctx.sprites.COLORS.grey);

// One label per lane beacon: code, label, belief; off-board lanes tagged,
// the highest ridge marked. Rebuilt only when the words change.
function renderLabels() {
  const top = L.sum.ridge ? L.lanes[L.sum.ridge.i].id : null;
  const html = L.lanes.map((l) => {
    const info = L.view.lanes.get(l.id) || { p: 0, present: true };
    const cls = ["ls-beacon", info.present ? "" : "off", l.id === top ? "top" : "", l.id === L.ctx.selected ? "sel" : ""].filter(Boolean).join(" ");
    return `<button class="${cls}" data-sel="${h(l.id)}" style="--c:${l.color}"><b>${h(l.code)}</b><span class="lbl">${h(l.label)}</span><i>${pct(info.p)}</i>`
      + `${info.present ? "" : "<em>off board</em>"}${l.id === top ? "<u>highest ridge</u>" : ""}</button>`;
  }).join("");
  if (html !== L.labelsHtml) {
    L.labelsHtml = html;
    L.els.labels.querySelectorAll(".ls-beacon").forEach((b) => b.remove());
    L.els.labels.insertAdjacentHTML("beforeend", html);
    L.beacons = [...L.els.labels.querySelectorAll(".ls-beacon")];
    measureLabels();
    draw();
  }
}

function measureLabels() {
  for (const b of L.beacons) { b._w = b.offsetWidth; b._h = b.offsetHeight; }
}

function renderRead() {
  const { sum } = L, conn = L.state.sources.some((s) => s.connected);
  let text;
  if (!L.lanes.length) text = "This pack defines no hypotheses: nothing to raise.";
  else if (!sum.ridge && !sum.spike && !sum.pit) {
    text = !conn ? "Flat floor: no sources connected, so no evidence term stands above 0.01."
      : L.at != null ? "Flat floor at this point in the replay: no evidence term above 0.01 had arrived yet."
        : "Flat floor: no evidence term above 0.01 yet.";
  } else {
    const parts = [];
    if (sum.ridge) {
      const l = L.lanes[sum.ridge.i];
      parts.push(`<span title="the lane whose smoothed surface stands highest">Highest ridge: ${laneRef(sum.ridge.i)} ${h(l.label)} ${pct(sum.ridge.p)}</span>`);
    } else parts.push("No ridge above the floor");
    if (sum.spike) parts.push(`<span title="the support spike at the surface's highest point">tallest spike: ${rowRef(sum.spike.rj)} → ${laneRef(sum.spike.li)} ${f2(sum.spike.v.s)}</span>`);
    if (sum.pit) parts.push(`<span title="the inhibitor pit at the surface's lowest point">deepest pit: ${rowRef(sum.pit.rj)} → ${laneRef(sum.pit.li)} <span class="cut">−${f2(sum.pit.v.i)}</span></span>`);
    text = parts.join(" · ");
  }
  if (text !== L.readHtml) { L.readHtml = text; L.els.read.innerHTML = text; }
}

function renderLegend() {
  const chip = (color, text, cls = "") => `<span class="ls-chip ${cls}" style="--c:${color}"><i></i>${text}</span>`;
  const lanes = L.lanes.map((l) => chip(l.color, h(l.code), (L.view.lanes.get(l.id) || {}).present === false ? "off" : "")).join("");
  const groups = L.groups.map((g) => chip(g.color, h(g.name))).join("");
  const engine = L.state.engine === "jev" ? "Jev" : "rules";
  const html = `<span class="ls-group">lanes ${lanes}</span>`
    + `<span class="ls-group">rows ${groups || '<span class="dim">none yet</span>'}</span>`
    + `<span class="ls-group">height = w × p (ruler at the far corner, ticks every 0.25) · surface smoothed over ${KR} cells · contours at ${LEVELS.join(" · ")}</span>`
    + `<span class="ls-group"><span class="ls-pit"></span>pits = inhibitors, w × p below the floor, dotted where the surface hides them · hatched lanes are off the board</span>`
    + `<span class="ls-group">evidence engine: <b>${engine}</b></span>`
    + `<span class="ls-group">Q / E rotate · + / − or wheel zoom · drag to pan · F fit</span>`;
  if (html !== L.legendHtml) { L.legendHtml = html; L.els.legend.innerHTML = html; }
}

function renderNote() {
  const conn = L.state.sources.some((s) => s.connected);
  const note = !L.lanes.length ? "This pack has no hypotheses."
    : !conn ? "No sources connected: a flat floor. Connect sources with keys 1–9 and every evidence term rises as a spike (or sinks as a pit)."
      : "";
  L.els.note.hidden = !note;
  L.els.note.textContent = note;
}

// ------------------------------------------------------------------ wiring

function wire() {
  const e = L.els;
  L.root.querySelector(".ls-cam").addEventListener("click", (ev) => {
    const b = ev.target.closest("[data-cam]");
    if (!b || !L.geo) return;
    const a = b.dataset.cam;
    if (a === "left") rotate(-1);
    if (a === "right") rotate(1);
    if (a === "in") zoom(1);
    if (a === "out") zoom(-1);
    if (a === "fit") fit();
  });
  e.play.addEventListener("click", play);
  e.scrub.addEventListener("input", () => {
    const sl = slices();
    if (!sl) return;
    const v = Number(e.scrub.value); // before pause() redraws the bar
    pause();
    setAt(v >= sl.length ? null : v);
  });

  // drag pans; a press that does not move is a click
  e.canvas.addEventListener("pointerdown", (ev) => {
    if (ev.button !== 0) return;
    L.drag = { x: ev.clientX, y: ev.clientY, panX: L.cam.panX, panY: L.cam.panY, moved: false };
    e.canvas.setPointerCapture(ev.pointerId);
  });
  e.canvas.addEventListener("pointermove", (ev) => {
    const d = L.drag;
    if (d) {
      const dx = ev.clientX - d.x, dy = ev.clientY - d.y;
      if (!d.moved && Math.abs(dx) + Math.abs(dy) > 4) { d.moved = true; e.tip.hidden = true; e.stage.classList.add("panning"); }
      if (d.moved) {
        L.cam.panX = d.panX + Math.round(dx / L.cam.scale);
        L.cam.panY = d.panY + Math.round(dy / L.cam.scale);
        draw();
        return;
      }
    }
    onHover(ev);
  });
  e.canvas.addEventListener("pointerup", (ev) => {
    const d = L.drag;
    L.drag = null;
    e.stage.classList.remove("panning");
    if (e.canvas.hasPointerCapture(ev.pointerId)) e.canvas.releasePointerCapture(ev.pointerId);
    if (!d || d.moved) return;
    const hit = hitTest(ev);
    if (!hit) return;
    if (hit.kind === "spike") select(L.rows[hit.c % L.geo.nr].id);
    else select(hit.id);
    onHover(ev);
  });
  e.canvas.addEventListener("pointerleave", () => {
    if (L.drag) return;
    e.tip.hidden = true;
    if (L.hover) { L.hover = null; L.hoverKey = ""; draw(); }
  });
  // trackpads send many small deltas: one zoom step per ~60 px of scroll
  e.stage.addEventListener("wheel", (ev) => {
    ev.preventDefault();
    L.wheel += ev.deltaY;
    if (Math.abs(L.wheel) < 60 && Math.abs(ev.deltaY) < 60) return;
    const r = e.stage.getBoundingClientRect();
    zoom(L.wheel < 0 ? 1 : -1, ev.clientX - r.left, ev.clientY - r.top);
    L.wheel = 0;
  }, { passive: false });

  // beacons, readout codes and legend chips select their piece
  const pick = (ev) => {
    const b = ev.target.closest("[data-sel]");
    if (b) select(b.dataset.sel);
  };
  e.labels.addEventListener("click", pick);
  e.read.addEventListener("click", pick);
  e.labels.addEventListener("pointermove", (ev) => {
    const b = ev.target.closest(".ls-beacon");
    showTip(ev, b ? pieceTip(b.dataset.sel) : "");
  });
  e.labels.addEventListener("pointerleave", () => { e.tip.hidden = true; });

  L.onKey = (ev) => {
    if (!L) return;
    const tag = ev.target.tagName;
    if (tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA" || ev.metaKey || ev.ctrlKey || ev.altKey) return;
    // Escape clears the selection in app.js without an update; follow it
    if (ev.key === "Escape") { setTimeout(() => { if (L) { draw(); renderText(); } }, 0); return; }
    if (!L.geo || !document.body.contains(L.root) || L.root.hidden) return;
    const k = ev.key.toLowerCase();
    if (k === "q") rotate(-1);
    else if (k === "e") rotate(1);
    else if (k === "+" || k === "=") zoom(1);
    else if (k === "-" || k === "_") zoom(-1);
    else if (k === "f") fit();
  };
  document.addEventListener("keydown", L.onKey);
  L.observer = new ResizeObserver(() => resize());
  L.observer.observe(e.stage);
}

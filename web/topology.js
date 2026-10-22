// Topology mode: a pixel-art homage to IBM Tivoli NetView.
// A root submap of sources (each colored by the worst status among the
// participants it fed), drill-down submaps of participant "devices" joined by
// shared records, an event browser tied to the map, and incident replay.
// The map is drawn at native resolution (1 unit = 1 pixel) and upscaled by a
// whole number with image-rendering: pixelated; labels are HTML on top so
// they stay in the pixel font at any scale.
import { COLORS, mix, linePoints } from "./sprites.js";
import { escapeHtml as h } from "./board.js";

// NetView status colors, tuned to sit on the dark palette. Unmanaged (tan) is
// for things the session cannot speak for: disconnected sources, participants
// no source has named yet or that left the board.
const STATUS = {
  unmanaged: { rank: -1, color: "#9a8866", label: "unmanaged" },
  unknown: { rank: 0, color: "#4f8dff", label: "unknown" },
  normal: { rank: 1, color: "#3fcf76", label: "normal" },
  marginal: { rank: 2, color: "#f2cf3a", label: "marginal" },
  critical: { rank: 3, color: "#ff4d4d", label: "critical" },
};
const statusFor = (p) => (p >= 0.66 ? "critical" : p >= 0.33 ? "marginal" : "normal");

const STEP_MS = 650; // replay: pause per event
const FLASH_MS = 900;
const LINK_GREY = "#3a4a5c";

// 12x12 device glyphs, drawn dark on a status-colored plate ('#' = glyph).
const GLYPHS = {
  person: [
    "....####....", "...######...", "...######...", "...######...", "....####....", "............",
    "..########..", ".##########.", ".##########.", ".##########.", ".##########.", "............",
  ],
  server: [
    "############", "#..........#", "#.####..##.#", "#..........#", "############", "#..........#",
    "#.####..##.#", "#..........#", "############", "#..........#", "#.####..##.#", "############",
  ],
  ticket: [
    ".#######....", ".#.....##...", ".#.....#.#..", ".#.....####.", ".#........#.", ".#.######.#.",
    ".#........#.", ".#.######.#.", ".#........#.", ".#.####...#.", ".#........#.", ".##########.",
  ],
  container: [
    "............", "............", "############", "#.#.#.#.#.##", "#.#.#.#.#.##", "#.#.#.#.#.##",
    "#.#.#.#.#.##", "#.#.#.#.#.##", "############", ".##......##.", "............", "............",
  ],
  port: [
    ".....##.....", "....#..#....", "....#..#....", ".....##.....", "..########..", ".....##.....",
    ".....##.....", ".....##.....", "##...##...##", ".##..##..##.", "..########..", "....####....",
  ],
  part: [
    "..#.#..#.#..", "..#.#..#.#..", ".##########.", ".#........#.", ".#.######.#.", ".#.#....#.#.",
    ".#.#....#.#.", ".#.######.#.", ".#........#.", ".##########.", "..#.#..#.#..", "..#.#..#.#..",
  ],
  org: [
    ".##.........", ".##.........", ".##.#..#..#.", ".####.##.##.", ".##########.", ".#........#.",
    ".#.##..##.#.", ".#.##..##.#.", ".#........#.", ".#...##...#.", ".#...##...#.", ".##########.",
  ],
  generic: [
    "............", "..########..", ".#.......##.", "##########.#", "#........#.#", "#.######.#.#",
    "#........#.#", "#.######.#.#", "#........#.#", "#........##.", "##########..", "............",
  ],
  // submap icons: a source (a small network) and the fused session (a cloud)
  network: [
    "............", "....####....", "....#..#....", "....####....", ".....##.....", "..########..",
    "..##....##..", ".####..####.", ".#..#..#..#.", ".####..####.", "............", "............",
  ],
  cloud: [
    "............", "............", ".....###....", "...##...#...", "..#......##.", ".##........#",
    "#..........#", "#..........#", ".##########.", "............", "............", "............",
  ],
};

// Object type name -> glyph. Order matters ("container" before "port").
const TYPE_ICON = [
  [/actor|person|people|user|employee|staff|contact|owner/i, "person"],
  [/service|server|host|api|app|system|device|node/i, "server"],
  [/ticket|change|incident|issue|case|request/i, "ticket"],
  [/container|shipment|crate/i, "container"],
  [/port|harbou?r|dock|terminal/i, "port"],
  [/part|component|sku|item|product/i, "part"],
  [/supplier|vendor|org|company|carrier|factory/i, "org"],
];
const glyphFor = (type) => (TYPE_ICON.find(([re]) => re.test(type || "")) || [null, "generic"])[1];

const css = (c, a = 1) => `rgba(${c[0] | 0},${c[1] | 0},${c[2] | 0},${a})`;
const rgb = (hex) => mix(hex, hex, 0);
const BLACK = [0, 0, 0], WHITE = [255, 255, 255];

let C = null; // everything for the mounted view; null while unmounted

const TEMPLATE = `
  <div class="topo-bar">
    <nav class="topo-crumbs"></nav>
    <div class="topo-legend">
      ${["normal", "marginal", "critical", "unknown", "unmanaged"].map((s) => `<span><i style="--s:${STATUS[s].color}"></i>${s}</span>`).join("")}
      <span><i class="dimmed" style="--s:${STATUS.critical.color}"></i>acked</span>
    </div>
    <button class="topo-fit" title="Re-center the map (or double-click it)">fit</button>
  </div>
  <div class="topo-note" hidden></div>
  <div class="topo-map">
    <canvas></canvas>
    <div class="topo-labels"></div>
    <div class="topo-scan"></div>
    <div class="topo-banner"></div>
    <div class="topo-foot"></div>
    <div class="topo-tip" hidden></div>
  </div>
  <div class="topo-replay">
    <button class="topo-play">▶ replay</button>
    <div class="topo-track" tabindex="0" role="slider" aria-label="Incident replay time">
      <div class="topo-rail"><div class="topo-ticks"></div><div class="topo-head"></div></div>
    </div>
    <button class="topo-live" title="Back to live: statuses from beliefs">LIVE</button>
    <span class="topo-clock"></span>
  </div>
  <section class="topo-events">
    <header class="topo-ev-head"></header>
    <ol class="topo-ev-list"></ol>
  </section>`;

// ------------------------------------------------------------------ lifecycle

export function mount(el, ctx) {
  el.innerHTML = TEMPLATE;
  const q = (s) => el.querySelector(s);
  C = {
    el, ctx, state: null,
    map: "root",              // "root" | "all" | a source id
    pos: new Map(),           // participant id -> world {x, y}, stable per scenario
    srcPos: new Map(),        // source id -> world {x, y} on the root submap
    need: { w: 240, h: 150 }, // native size the layouts want, for choosing the scale
    items: [], edges: [], events: null, span: null,
    cam: { x: 0, y: 0, tx: 0, ty: 0 }, panned: false, drag: null,
    scale: 3, W: 0, H: 0, k: 1,
    flash: new Map(), hover: null, lastSel: undefined, labelKey: "",
    t: null,                  // replay time in ms, or null for live
    playing: false, timer: 0, raf: 0,
    mapEl: q(".topo-map"), canvas: q("canvas"), labels: q(".topo-labels"), banner: q(".topo-banner"),
    note: q(".topo-note"), foot: q(".topo-foot"), tip: q(".topo-tip"), crumbs: q(".topo-crumbs"),
    play: q(".topo-play"), track: q(".topo-track"), rail: q(".topo-rail"), ticks: q(".topo-ticks"),
    head: q(".topo-head"), live: q(".topo-live"), clock: q(".topo-clock"),
    evBox: q(".topo-events"), evHead: q(".topo-ev-head"), evList: q(".topo-ev-list"),
  };
  C.g = C.canvas.getContext("2d");
  wire();
  C.ro = new ResizeObserver(() => fit());
  C.ro.observe(C.mapEl);
  C.raf = requestAnimationFrame(frame);
}

export function update(state) {
  if (!C) return;
  const first = !C.state || C.state.scenario.id !== state.scenario.id;
  C.state = state;
  if (first) {
    pause();
    C.map = "root";
    C.t = null;
    C.panned = false;
    layout(state);
  }
  if (C.map !== "root" && C.map !== "all" && !state.sources.some((s) => s.id === C.map)) C.map = "root";
  readEvents(state);
  // a replay position outside the new span (events changed) snaps back to live
  if (C.t != null && (!C.span || C.t > C.span.max)) C.t = null;
  derive();
  renderCrumbs();
  renderTicks();
  renderReplay();
  renderEvents();
  fit();
}

export function unmount() {
  if (!C) return;
  clearTimeout(C.timer);
  cancelAnimationFrame(C.raf);
  C.ro.disconnect();
  C.el.innerHTML = "";
  C = null;
}

// ------------------------------------------------------------------ model

// Deterministic layouts, keyed by id and computed once per scenario, so a
// piece sits in the same place on every submap and across every update.
function layout(state) {
  const parts = state.nodes.filter((n) => n.band === "participant").map((n) => n.id);
  const srcs = state.sources.map((s) => s.id);
  const rp = Math.max(44, parts.length * 11), rs = Math.max(60, srcs.length * 14);
  ring(parts, C.pos, rp, Math.round(rp * 0.58));
  ring(srcs, C.srcPos, rs, Math.round(rs * 0.58));
  C.need = { w: 2 * Math.max(rp, rs) + 16 + 84, h: Math.round(1.16 * Math.max(rp, rs)) + 16 + 26 };
}

function ring(ids, out, rx, ry) {
  out.clear();
  ids.forEach((id, i) => {
    if (ids.length === 1) return out.set(id, { x: 0, y: 0 });
    const a = -Math.PI / 2 + (i / ids.length) * Math.PI * 2;
    out.set(id, { x: Math.round(Math.cos(a) * rx), y: Math.round(Math.sin(a) * ry) });
  });
}

function readEvents(state) {
  if (!Array.isArray(state.events)) { C.events = null; C.span = null; return; }
  C.events = state.events
    .map((e) => ({ ...e, t: Number.isFinite(e.t) ? e.t : Date.parse(e.at), participants: e.participants || [] }))
    .filter((e) => Number.isFinite(e.t))
    .sort((a, b) => a.t - b.t);
  if (!C.events.length) { C.span = null; return; }
  // The scrubber is spaced by event order, not by clock: one even step per
  // distinct timestamp, so a record dated years earlier (a hire date, say)
  // does not squash the incident into a single pixel. Its far left is "before
  // the first event".
  const stops = [...new Set(C.events.map((e) => e.t))];
  const first = stops[0], last = stops[stops.length - 1];
  C.span = { min: first - 1, max: last, stops, years: last - first > 300 * 86400e3, days: last - first > 20 * 3600e3 };
}

// Events touching a participant up to time t (all of them when live).
function touching(id, t) {
  if (!C.events) return [];
  return C.events.filter((e) => (t == null || e.t <= t) && e.participants.includes(id));
}

// Status of one participant: from belief when live, from the worst event
// severity seen so far when replaying. Dimmed when every touching event is acked.
function nodeStatus(n, t) {
  if (!n.present) return { status: "unmanaged", ghost: !n.offboard, off: !!n.offboard, dim: false };
  const evs = touching(n.id, t);
  const dim = evs.length > 0 && evs.every((e) => e.acked);
  if (t == null) return { status: n.status === "unresolved" ? "unknown" : statusFor(n.p), dim };
  if (!evs.length) return { status: "unknown", dim };
  return { status: statusFor(Math.max(...evs.map((e) => e.severity || 0))), dim };
}

// Worst status propagates upward; acked (dimmed) children only count when
// every child is acked, so acknowledging an alarm clears it from the parent.
function worst(list) {
  const live = list.filter((x) => x.status !== "unmanaged");
  if (!live.length) return null;
  const open = live.filter((x) => !x.dim);
  const pool = open.length ? open : live;
  const status = pool.reduce((a, b) => (STATUS[b.status].rank > STATUS[a.status].rank ? b : a)).status;
  return { status, dim: !open.length };
}

const sourcesOf = (n) => (n.detail && n.detail.sources) || [];
const thickFor = (w) => (w >= 4 ? 3 : w >= 2 ? 2 : 1);

// Rebuild what the current submap shows: icons, links and their labels.
function derive() {
  const s = C.state, T = C.t;
  const parts = s.nodes.filter((n) => n.band === "participant");
  const info = new Map(parts.map((n) => [n.id, nodeStatus(n, T)]));
  const items = [], edges = [];
  if (C.map === "root") {
    const anyOn = s.sources.some((x) => x.connected);
    const all = worst(parts.map((n) => info.get(n.id)));
    items.push({ id: "@all", kind: "all", x: 0, y: 0, glyph: "cloud", status: anyOn ? (all ? all.status : "unknown") : "unmanaged",
                 dim: anyOn && all ? all.dim : false, count: parts.filter((n) => n.present).length });
    s.sources.forEach((src, i) => {
      const members = parts.filter((n) => n.present && sourcesOf(n).includes(src.id));
      const agg = worst(members.map((n) => info.get(n.id)));
      const p = C.srcPos.get(src.id);
      const it = { id: `@src:${src.id}`, kind: "source", src, key: i + 1, x: p.x, y: p.y, glyph: "network",
                   status: !src.connected ? "unmanaged" : agg ? agg.status : "unknown",
                   dim: src.connected && agg ? agg.dim : false, count: members.length };
      items.push(it);
      // spokes: each source feeds the fused session; thickness by participants fed
      edges.push({ a: items[0], b: it, thick: src.connected ? thickFor(members.length) : 1, dashed: !src.connected,
                   color: rgb(src.connected ? STATUS[it.status].color : STATUS.unmanaged.color), ids: [] });
    });
  } else {
    const members = C.map === "all" ? parts : parts.filter((n) => sourcesOf(n).includes(C.map));
    const byId = new Map();
    for (const n of members) {
      const p = C.pos.get(n.id);
      const it = { id: n.id, kind: "participant", node: n, x: p.x, y: p.y, glyph: glyphFor(n.detail && n.detail.type), ...info.get(n.id) };
      items.push(it);
      byId.set(n.id, it);
    }
    for (const l of Array.isArray(s.links) ? s.links : []) {
      const a = byId.get(l.a), b = byId.get(l.b);
      if (!a || !b || a.ghost || b.ghost) continue;
      if (C.map !== "all" && !(l.records || []).some((r) => r.source === C.map)) continue;
      const bad = STATUS[a.status].rank >= STATUS[b.status].rank ? a : b;
      edges.push({ a, b, thick: thickFor(l.weight || 1), color: mix(LINK_GREY, STATUS[bad.status].color, 0.55), ids: [a.id, b.id], link: l });
    }
  }
  C.items = items;
  C.edges = edges;
  renderLabels();
  renderMapText();
  if (!C.panned) frameCamera();
}

// ------------------------------------------------------------------ navigation

function go(map) {
  C.map = map;
  C.panned = false;
  C.hover = null;
  C.tip.hidden = true;
  derive();
  renderCrumbs();
  renderEvents();
}

function frameCamera() {
  if (!C.items.length) { C.cam.tx = 0; C.cam.ty = 0; return; }
  const xs = C.items.map((i) => i.x), ys = C.items.map((i) => i.y);
  C.cam.tx = (Math.min(...xs) + Math.max(...xs)) / 2;
  C.cam.ty = (Math.min(...ys) + Math.max(...ys)) / 2 + 4; // labels hang below the icons
}

// Center the map on some participants (opening the "all" submap if they are
// not on this one) and flash them.
function centerOn(ids) {
  ids = ids.filter((id) => C.pos.has(id));
  if (!ids.length) return;
  if (!ids.some((id) => C.items.some((it) => it.id === id))) go("all");
  const pts = ids.map((id) => C.pos.get(id));
  C.cam.tx = pts.reduce((a, p) => a + p.x, 0) / pts.length;
  C.cam.ty = pts.reduce((a, p) => a + p.y, 0) / pts.length;
  C.panned = true;
  flashIds(ids);
}

function flashIds(ids) {
  const now = performance.now();
  for (const id of ids) C.flash.set(id, now);
}

// ------------------------------------------------------------------ canvas

// Pick a whole-number scale that fits the larger of the two layouts, so icons
// keep the same size on every submap, and size the canvas to the map box.
function fit() {
  if (!C || !C.state) return;
  const cssW = C.mapEl.clientWidth, cssH = C.mapEl.clientHeight;
  if (!cssW || !cssH) return;
  // half steps are still whole device pixels on a HiDPI screen
  const dpr = window.devicePixelRatio || 1, q = dpr >= 2 ? 2 : 1;
  const scale = Math.max(2, Math.min(4, Math.floor(q * Math.min(cssW / C.need.w, cssH / C.need.h)) / q));
  const W = Math.max(1, Math.floor(cssW / scale)), H = Math.max(1, Math.floor(cssH / scale));
  const k = Math.max(1, Math.ceil(scale * dpr));
  if (W !== C.W || H !== C.H || k !== C.k || scale !== C.scale) {
    Object.assign(C, { W, H, k, scale });
    C.canvas.width = W * k;
    C.canvas.height = H * k;
    C.canvas.style.width = `${W * scale}px`;
    C.canvas.style.height = `${H * scale}px`;
    C.g.setTransform(k, 0, 0, k, 0, 0);
    C.g.imageSmoothingEnabled = false;
    C.mapEl.style.setProperty("--scan", `${scale}px`);
    C.labelKey = "";
  }
}

const sx = (wx) => Math.round(wx - C.cam.x + C.W / 2);
const sy = (wy) => Math.round(wy - C.cam.y + C.H / 2);

function frame(now) {
  if (!C) return;
  C.raf = requestAnimationFrame(frame);
  if (!C.state || !C.W) return;
  const cam = C.cam;
  cam.x += (cam.tx - cam.x) * 0.2;
  cam.y += (cam.ty - cam.y) * 0.2;
  if (Math.abs(cam.tx - cam.x) < 0.3) cam.x = cam.tx;
  if (Math.abs(cam.ty - cam.y) < 0.3) cam.y = cam.ty;
  draw(now);
  placeLabels();
  // selection can change from the panel or the agent without an update
  if (C.ctx.selected !== C.lastSel) { C.lastSel = C.ctx.selected; markRows(); }
}

function draw(now) {
  const g = C.g, { W, H } = C;
  g.fillStyle = COLORS.bg;
  g.fillRect(0, 0, W, H);
  // a dotted grid that pans with the map
  g.fillStyle = "rgba(255,255,255,0.05)";
  const ox = ((sx(0) % 8) + 8) % 8, oy = ((sy(0) % 8) + 8) % 8;
  for (let x = ox; x < W; x += 8) for (let y = oy; y < H; y += 8) g.fillRect(x, y, 1, 1);

  const focus = C.hover && C.hover.kind === "participant" ? C.hover.id : C.ctx.selected;
  for (const e of C.edges) drawLink(g, e, focus);
  for (const it of C.items) drawItem(g, it, now);
}

function drawLink(g, e, focus) {
  const touches = focus && e.ids.includes(focus);
  let alpha = e.dashed ? 0.35 : 0.75;
  if (focus && e.ids.length) alpha = touches ? 1 : 0.25;
  g.fillStyle = css(e.color, alpha);
  const pts = linePoints(sx(e.a.x), sy(e.a.y), sx(e.b.x), sy(e.b.y));
  const off = Math.floor(e.thick / 2);
  pts.forEach(([x, y], i) => {
    if (e.dashed && i % 4 > 1) return;
    g.fillRect(x - off, y - off, e.thick, e.thick);
  });
}

function drawItem(g, it, now) {
  const x = sx(it.x) - 8, y = sy(it.y) - 8;
  const base = rgb(STATUS[it.status].color);
  let face = it.dim ? mix(base, COLORS.dark, 0.62) : base;
  const age = now - (C.flash.get(it.id) ?? -1e9);
  if (age < FLASH_MS) face = mix(face, WHITE, Math.floor(age / 150) % 2 ? 0.1 : 0.85); // NetView-style blink

  if (it.ghost) {
    // never named: a dotted outline and a faint glyph
    g.fillStyle = css(base, 0.5);
    for (let i = 0; i < 16; i += 2) {
      g.fillRect(x + i, y, 1, 1); g.fillRect(x + i, y + 15, 1, 1);
      g.fillRect(x, y + i, 1, 1); g.fillRect(x + 15, y + i, 1, 1);
    }
    glyph(g, x + 2, y + 2, GLYPHS[it.glyph], css(base, 0.35));
  } else {
    if (it.kind !== "participant") plate(g, x + 2, y - 2, mix(face, BLACK, 0.45)); // submaps look stacked
    plate(g, x, y, face);
    glyph(g, x + 2, y + 2, GLYPHS[it.glyph], css(mix(face, BLACK, 0.74)));
  }
  if (it.kind === "source") {
    g.fillStyle = it.src.color; // which source this is, under the plate
    g.fillRect(x + 2, y + 17, 12, 1);
  }
  // critical and unacked: a blinking red ring
  if (it.status === "critical" && !it.dim && !it.ghost && Math.floor(now / 420) % 2 === 0) {
    g.fillStyle = css(base, 0.55);
    g.fillRect(x - 2, y - 2, 20, 1); g.fillRect(x - 2, y + 17, 20, 1);
    g.fillRect(x - 2, y - 2, 1, 20); g.fillRect(x + 17, y - 2, 1, 20);
  }
  if (it.id === C.ctx.selected) brackets(g, x, y, COLORS.white);
  else if (C.hover && C.hover.id === it.id) brackets(g, x, y, "rgba(232,241,255,0.45)");
}

// A beveled plate in the status color: the face of every NetView symbol.
function plate(g, x, y, face) {
  g.fillStyle = "#05070a";
  g.fillRect(x, y, 16, 16);
  g.fillStyle = css(face);
  g.fillRect(x + 1, y + 1, 14, 14);
  g.fillStyle = css(mix(face, WHITE, 0.35));
  g.fillRect(x + 1, y + 1, 14, 1);
  g.fillRect(x + 1, y + 1, 1, 14);
  g.fillStyle = css(mix(face, BLACK, 0.4));
  g.fillRect(x + 1, y + 14, 14, 1);
  g.fillRect(x + 14, y + 1, 1, 14);
}

function glyph(g, x, y, rows, color) {
  g.fillStyle = color;
  rows.forEach((row, ry) => {
    for (let rx = 0; rx < row.length; rx++) if (row[rx] === "#") g.fillRect(x + rx, y + ry, 1, 1);
  });
}

// Corner brackets just outside a 16x16 icon.
function brackets(g, x, y, color) {
  g.fillStyle = color;
  for (const [cx, cy, dx, dy] of [[x - 3, y - 3, 1, 1], [x + 18, y - 3, -1, 1], [x - 3, y + 18, 1, -1], [x + 18, y + 18, -1, -1]]) {
    g.fillRect(dx > 0 ? cx : cx - 2, cy, 3, 1);
    g.fillRect(cx, dy > 0 ? cy : cy - 2, 1, 3);
  }
}

function hit(e) {
  const r = C.canvas.getBoundingClientRect();
  const px = (e.clientX - r.left) / C.scale, py = (e.clientY - r.top) / C.scale;
  for (let i = C.items.length - 1; i >= 0; i--) {
    const it = C.items[i], x = sx(it.x), y = sy(it.y);
    if (px >= x - 9 && px < x + 11 && py >= y - 11 && py < y + 10) return it;
  }
  return null;
}

// ------------------------------------------------------------------ html over the map

function labelHtml(it) {
  if (it.kind === "all") return `<b>ALL</b> participants`;
  if (it.kind === "source") return `<b style="color:${it.src.color}">${it.key}</b> ${h(it.src.name)}`;
  const n = it.node;
  const tag = it.ghost ? ` <i>not named yet</i>` : it.off ? ` <i>off board</i>` : "";
  return `<b>${h(n.code)}</b> ${h(n.label)}${tag}`;
}

function renderLabels() {
  C.labels.innerHTML = C.items.map((it) =>
    `<div class="topo-label${it.ghost || it.status === "unmanaged" ? " faint" : ""}">${labelHtml(it)}</div>`).join("");
  C.labelKey = "";
}

function placeLabels() {
  const key = `${sx(0)},${sy(0)},${C.scale},${C.items.length}`;
  if (key === C.labelKey) return;
  C.labelKey = key;
  const els = C.labels.children;
  C.items.forEach((it, i) => {
    const el = els[i];
    if (!el) return;
    el.style.transform = `translate(${sx(it.x) * C.scale}px, ${(sy(it.y) + 11) * C.scale}px) translateX(-50%)`;
  });
}

function renderMapText() {
  const s = C.state;
  const live = C.t == null;
  C.banner.innerHTML = live
    ? `<b class="live">LIVE</b> status from beliefs`
    : `<b class="replay">REPLAY</b> ${clock(C.t)} · ${C.events.filter((e) => e.t <= C.t).length}/${C.events.length} events`;

  let note = "";
  const keys = `1–${s.sources.length}`;
  if (C.map === "root" && !s.sources.some((x) => x.connected)) {
    note = `No sources connected. Press ${keys} or use the rack below: each source icon turns from tan (unmanaged) to the worst status among the participants it names.`;
  } else if (C.map !== "root" && C.map !== "all") {
    const src = s.sources.find((x) => x.id === C.map);
    const i = s.sources.indexOf(src) + 1;
    if (!src.connected) note = `${h(src.name)} is not connected. Press ${i} to connect it.`;
    else if (!C.items.length) note = `${h(src.name)} has not named any participant yet.`;
  } else if (C.map === "all" && !C.items.some((it) => !it.ghost)) {
    note = `No participant has been named yet. Connect a source (${keys}).`;
  }
  C.note.hidden = !note;
  C.note.innerHTML = note;

  if (C.map === "root") C.foot.textContent = "click a submap to open it · spokes: participants each source names";
  else if (!Array.isArray(s.links)) C.foot.textContent = "links: waiting for server data";
  else C.foot.textContent = `${C.edges.length} link${C.edges.length === 1 ? "" : "s"} · thickness = shared records · drag to pan`;
}

function renderCrumbs() {
  const s = C.state;
  let html = `<button data-go="root" class="${C.map === "root" ? "on" : ""}">Root</button>`;
  if (C.map !== "root") {
    const src = s.sources.find((x) => x.id === C.map);
    const name = C.map === "all" ? "All participants" : `<i style="--c:${src.color}"></i>${h(src.name)}`;
    html += `<span>›</span><button data-go="${h(C.map)}" class="on">${name}</button>`;
  }
  C.crumbs.innerHTML = html;
}

function showTip(e, it) {
  if (!it) { C.tip.hidden = true; return; }
  let body;
  const st = STATUS[it.status].label + (it.dim ? " · acked" : "");
  if (it.kind === "all") body = `<b>ALL</b> participants<br><span>${st} · ${it.count} on the board</span>`;
  else if (it.kind === "source") {
    body = `<b>${h(it.src.name)}</b><br><span>${it.src.connected ? `${st} · names ${it.count} participant${it.count === 1 ? "" : "s"}` : "not connected · unmanaged"}</span>`;
  } else {
    const n = it.node, evs = touching(n.id, C.t);
    const where = it.ghost ? "not named by any source yet" : it.off ? "off the board" : `${st} · ${Math.round(n.p * 100)}% belief`;
    const evText = C.events ? ` · ${evs.length} event${evs.length === 1 ? "" : "s"}${evs.length ? ` (${evs.filter((x) => x.acked).length} acked)` : ""}` : "";
    body = `<b>${h(n.code)}</b> ${h(n.label)}<br><span>${h((n.detail && n.detail.type) || "participant")} · ${where}${evText}</span>`;
  }
  const r = C.mapEl.getBoundingClientRect();
  C.tip.innerHTML = body;
  C.tip.hidden = false;
  const left = Math.min(e.clientX - r.left + 14, r.width - C.tip.offsetWidth - 4);
  C.tip.style.left = `${Math.max(4, left)}px`;
  C.tip.style.top = `${e.clientY - r.top + 12}px`;
}

// ------------------------------------------------------------------ replay

function clock(t) {
  const iso = new Date(t).toISOString();
  const date = !C.span ? "" : C.span.years ? iso.slice(0, 10) : C.span.days ? iso.slice(5, 10) : "";
  return (date ? `${date} ` : "") + iso.slice(11, 19);
}

// scrubber position (0..100) of a time: how many timestamps have happened by then
const pct = (t) => (C.span.stops.filter((s) => s <= t).length / C.span.stops.length) * 100;

function renderTicks() {
  if (!C.span) { C.ticks.innerHTML = ""; return; }
  // a break mark wherever the clock jumps more than six hours between stops
  const { stops } = C.span;
  const gaps = stops.slice(1).map((t, i) => (t - stops[i] > 6 * 3600e3 ? `<b class="gap" style="left:${((i + 1.5) / stops.length) * 100}%"></b>` : ""));
  C.ticks.innerHTML = gaps.join("") + C.events.map((e) => {
    const sev = e.severity || 0;
    return `<i style="left:${pct(e.t).toFixed(2)}%;--s:${STATUS[statusFor(sev)].color};height:${4 + Math.round(sev * 10)}px"
      class="${e.acked ? "acked" : ""}"></i>`;
  }).join("");
}

function renderReplay() {
  const ok = !!C.span;
  C.play.disabled = !ok;
  C.track.classList.toggle("off", !ok);
  C.live.classList.toggle("on", C.t == null);
  C.play.textContent = C.playing ? "❚❚ pause" : "▶ replay";
  C.head.style.left = `${C.t == null || !ok ? 100 : pct(C.t)}%`;
  if (!C.events) C.clock.textContent = "replay: waiting for server data";
  else if (!ok) C.clock.textContent = "replay: no events yet";
  else C.clock.textContent = C.t == null ? `live · ${clock(C.span.max)}` : clock(C.t);
}

function setT(t) {
  C.t = t;
  derive();
  renderReplay();
  markRows();
}

function play() {
  const evs = C.events;
  if (!evs || !evs.length) return;
  if (C.playing) return pause();
  C.playing = true;
  // from live or the end, start over; otherwise carry on from here
  let i = C.t == null ? -1 : evs.findIndex((e) => e.t > C.t);
  if (i < 0) { i = 0; setT(C.span.min); } else renderReplay();
  const tick = () => {
    if (!C || !C.playing) return;
    if (i >= evs.length) { C.playing = false; setT(null); return; }
    const t = evs[i].t, ids = [];
    while (i < evs.length && evs[i].t === t) ids.push(...evs[i++].participants);
    setT(t);
    flashIds(ids);
    scrollToCursor();
    C.timer = setTimeout(tick, STEP_MS);
  };
  C.timer = setTimeout(tick, 350);
}

function pause() {
  if (!C) return;
  C.playing = false;
  clearTimeout(C.timer);
  if (C.state) renderReplay();
}

function scrub(e) {
  const r = C.rail.getBoundingClientRect();
  const f = Math.max(0, Math.min(1, (e.clientX - r.left) / r.width));
  const i = Math.round(f * C.span.stops.length);
  setT(i === 0 ? C.span.min : C.span.stops[i - 1]);
}

// Arrow keys step event by event; End returns to live.
function stepKey(key) {
  const ts = C.span.stops;
  if (key === "Home") return setT(C.span.min);
  if (key === "End") return setT(null);
  if (key === "ArrowRight") {
    if (C.t == null) return;
    const next = ts.find((t) => t > C.t);
    return setT(next == null ? null : next);
  }
  if (key === "ArrowLeft") {
    const cur = C.t == null ? Infinity : C.t;
    const prev = ts.filter((t) => t < cur).pop();
    setT(prev == null ? C.span.min : prev);
  }
}

// ------------------------------------------------------------------ event browser

function renderEvents() {
  const s = C.state;
  if (!C.events) {
    C.evHead.innerHTML = `<b>Event browser</b>`;
    C.evList.innerHTML = `<li class="topo-wait">Waiting for server data: this server does not send <code>state.events</code> yet,
      so the browser and the replay are idle. The map above still shows live status from beliefs.</li>`;
    return;
  }
  const src = s.sources.find((x) => x.id === C.map);
  const rows = src ? C.events.filter((e) => e.source === src.id) : C.events;
  const acked = rows.filter((e) => e.acked).length;
  C.evHead.innerHTML = `<b>Event browser</b> <span class="dim">${rows.length} event${rows.length === 1 ? "" : "s"}${acked ? ` · ${acked} acked` : ""}${src ? ` · from ${h(src.name)} only` : ""}</span>`;
  if (!rows.length) {
    C.evList.innerHTML = `<li class="topo-wait">${C.events.length ? "No events from this source." : "No events yet: connect a source whose records carry timestamps."}</li>`;
    return;
  }
  const codes = new Map(s.nodes.map((n) => [n.id, n.code]));
  const top = C.evBox.scrollTop;
  C.evList.innerHTML = rows.map((e) => {
    const sv = s.sources.find((x) => x.id === e.source);
    const sev = e.severity || 0;
    return `<li data-ev="${h(e.id)}" class="${e.acked ? "acked" : ""}">
      <i class="sev" style="--s:${STATUS[statusFor(sev)].color}" title="severity ${Math.round(sev * 100)}%"></i>
      <time>${clock(e.t)}</time>
      <span class="chip" style="--c:${sv ? sv.color : "#888"}">${h(sv ? sv.name : e.source)}</span>
      <span class="who">${e.participants.map((id) => `<button class="pid" data-pid="${h(id)}" title="select">${h(codes.get(id) || id)}</button>`).join("") || `<span class="dim">–</span>`}</span>
      <code class="ex" title="${h(`${e.pointer || ""}  ${e.excerpt || ""}`)}">${h(e.excerpt || "")}</code>
      <button class="ack" data-ack>${e.acked ? "unack" : "ack"}</button>
    </li>`;
  }).join("");
  C.evBox.scrollTop = top;
  markRows();
}

// Replay marks (future rows dimmed, the latest one is the cursor) and rows
// that touch the selected piece.
function markRows() {
  if (!C.events) return;
  const byId = new Map(C.events.map((e) => [e.id, e]));
  const rows = [...C.evList.querySelectorAll("li[data-ev]")];
  let cur = null;
  for (const li of rows) {
    const e = byId.get(li.dataset.ev);
    if (!e) continue;
    const future = C.t != null && e.t > C.t;
    li.classList.toggle("future", future);
    li.classList.toggle("sel", !!C.ctx.selected && e.participants.includes(C.ctx.selected));
    li.classList.remove("cur");
    if (C.t != null && !future) cur = li;
  }
  if (cur) cur.classList.add("cur");
}

function scrollToCursor() {
  const li = C.evList.querySelector("li.cur");
  if (!li) return;
  const box = C.evBox, top = li.offsetTop - box.offsetTop;
  if (top < box.scrollTop + 24 || top > box.scrollTop + box.clientHeight - li.offsetHeight) {
    box.scrollTop = Math.max(0, top - box.clientHeight / 2);
  }
}

// ------------------------------------------------------------------ input

function wire() {
  const cv = C.canvas;
  cv.addEventListener("pointerdown", (e) => {
    cv.setPointerCapture(e.pointerId);
    C.drag = { x: e.clientX, y: e.clientY, cx: C.cam.tx, cy: C.cam.ty, moved: false };
  });
  cv.addEventListener("pointermove", (e) => {
    if (C.drag) {
      const dx = (e.clientX - C.drag.x) / C.scale, dy = (e.clientY - C.drag.y) / C.scale;
      if (Math.abs(dx) + Math.abs(dy) > 1.5) C.drag.moved = true;
      if (C.drag.moved) {
        C.cam.tx = C.cam.x = C.drag.cx - dx;
        C.cam.ty = C.cam.y = C.drag.cy - dy;
        C.panned = true;
        C.tip.hidden = true;
        return;
      }
    }
    const it = hit(e);
    C.hover = it;
    cv.style.cursor = it ? "pointer" : C.drag ? "grabbing" : "grab";
    showTip(e, it);
  });
  cv.addEventListener("pointerup", () => {
    C.dragged = !!(C.drag && C.drag.moved);
    C.drag = null;
  });
  cv.addEventListener("click", (e) => {
    if (C.dragged) { C.dragged = false; return; }
    const it = hit(e);
    if (!it) return;
    if (it.kind === "all") go("all");
    else if (it.kind === "source") go(it.src.id);
    else C.ctx.select(it.id);
  });
  cv.addEventListener("pointerleave", () => { C.hover = null; C.tip.hidden = true; });
  cv.addEventListener("dblclick", (e) => { if (!hit(e)) { C.panned = false; frameCamera(); } });
  C.el.querySelector(".topo-fit").addEventListener("click", () => { C.panned = false; frameCamera(); });
  C.crumbs.addEventListener("click", (e) => {
    const b = e.target.closest("[data-go]");
    if (b) go(b.dataset.go);
  });

  C.play.addEventListener("click", play);
  C.live.addEventListener("click", () => { pause(); setT(null); });
  C.track.addEventListener("pointerdown", (e) => {
    if (!C.span) return;
    pause();
    C.track.setPointerCapture(e.pointerId);
    scrub(e);
    const move = (ev) => scrub(ev);
    const up = () => {
      C.track.removeEventListener("pointermove", move);
      C.track.removeEventListener("pointerup", up);
    };
    C.track.addEventListener("pointermove", move);
    C.track.addEventListener("pointerup", up);
  });
  C.track.addEventListener("keydown", (e) => {
    if (!C.span || !["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) return;
    e.preventDefault();
    pause();
    stepKey(e.key);
  });

  C.evList.addEventListener("click", (e) => {
    const li = e.target.closest("li[data-ev]");
    if (!li) return;
    const ev = C.events.find((x) => x.id === li.dataset.ev);
    if (!ev) return;
    if (e.target.closest("[data-ack]")) {
      C.ctx.act({ action: ev.acked ? "unack" : "ack", event: ev.id });
      return;
    }
    const pid = e.target.closest("[data-pid]");
    if (pid) {
      centerOn([pid.dataset.pid]);
      C.ctx.select(pid.dataset.pid);
      return;
    }
    centerOn(ev.participants);
  });
}

// The checkers board: piece positions, move animation, edges, hit testing.
import { TILE, COLORS, drawBoard, drawPiece, drawHalo, drawSelection, linePoints, rgba } from "./sprites.js";

const MOVE_MS = 240;
const STAGE_MS = 420;
const STEP_AT = [0.33, 0.66];

const stepFor = (p) => STEP_AT.filter((t) => p >= t).length;

// Belief advances a piece by legal diagonal checkers moves:
// (c, r) -> (c+1, r-1) -> (c, r-2)
function squareFor(node, step) {
  return { c: node.col + (step === 1 ? 1 : 0), r: node.base_row - step };
}

function pieceMode(n) {
  if (n.band === "signal") return n.status === "dormant" ? "hollow" : n.status === "quiet" ? "grey" : "solid";
  if (n.band === "entity") return n.status === "unresolved" ? "grey" : "solid";
  return "solid";
}

export class Board {
  constructor(canvas, glow, tooltip) {
    this.canvas = canvas;
    this.glow = glow;
    this.tooltip = tooltip;
    this.ctx = canvas.getContext("2d");
    this.gctx = glow.getContext("2d");
    this.pieces = new Map();
    this.particles = [];
    this.selected = null;
    this.hover = null;
    this.onSelect = () => {};
    this.sourceColors = {};
    this.sourceIndex = {};
    canvas.addEventListener("mousemove", (e) => this._hover(e));
    canvas.addEventListener("mouseleave", () => { this.hover = null; this.tooltip.hidden = true; });
    canvas.addEventListener("click", (e) => {
      const id = this._hit(e);
      this.select(id);
      this.onSelect(id);
    });
    window.addEventListener("resize", () => this._fit());
    requestAnimationFrame((t) => this._frame(t));
  }

  select(id) {
    this.selected = id;
  }

  setState(state, animate) {
    const first = !this.state || this.state.scenario.id !== state.scenario.id;
    const prev = this.state;
    this.state = state;
    this.nodes = new Map(state.nodes.map((n) => [n.id, n]));
    state.sources.forEach((s, i) => { this.sourceColors[s.id] = s.color; this.sourceIndex[s.id] = i; });
    const { width, height } = state.scenario.board;
    if (first) {
      this.pieces.clear();
      this.particles = [];
      this.canvas.width = this.glow.width = width * TILE;
      this.canvas.height = this.glow.height = height * TILE;
      this._fit();
    }
    const now = performance.now();
    const prevNodes = prev && !first ? new Map(prev.nodes.map((n) => [n.id, n])) : new Map();

    for (const n of state.nodes) {
      let pc = this.pieces.get(n.id);
      const final = squareFor(n, n.step);
      if (!pc || first || !animate) {
        pc = { id: n.id, x: final.c * TILE, y: final.r * TILE, queue: [], move: null, alpha: n.present ? 1 : 0, fade: null, flash: 0, step: n.step };
        this.pieces.set(n.id, pc);
        continue;
      }
      const old = prevNodes.get(n.id);

      // presence changes: drop in from above, or hop off the board
      if (n.present && !(old && old.present)) {
        const sq = squareFor(n, 0);
        pc.x = sq.c * TILE; pc.y = sq.r * TILE - 24; pc.alpha = 0; pc.step = 0; pc.queue = [];
        pc.queue.push({ c: sq.c, r: sq.r, at: now, drop: true });
      }
      const leaving = !n.present && old && old.present;
      if (!n.present && !leaving) continue;

      // one diagonal move per step change, scheduled stage by stage
      let step = pc.step;
      const base = now + (pc.queue.length ? STAGE_MS : 0);
      state.stages.forEach((stage, k) => {
        const p = stage[n.id];
        if (p === undefined) return;
        const target = stepFor(p);
        while (step !== target) {
          step += target > step ? 1 : -1;
          const sq = squareFor(n, step);
          pc.queue.push({ c: sq.c, r: sq.r, at: base + k * STAGE_MS });
        }
      });
      if (step !== n.step) {
        const sq = squareFor(n, n.step);
        pc.queue.push({ c: sq.c, r: sq.r, at: base + state.stages.length * STAGE_MS });
      }
      pc.step = n.step;
      if (leaving) pc.queue.push({ leave: true, at: base + (state.stages.length + 1) * STAGE_MS });

      if (n.band === "entity" && old) {
        if (n.status === "resolved" && old.status !== "resolved") pc.flashAt = now + STAGE_MS;
        const before = new Set((old.detail && old.detail.sources) || []);
        for (const s of (n.detail && n.detail.sources) || []) {
          if (!before.has(s)) this._sparks(s, n, now);
        }
      }
    }
  }

  // fusion sparks: pixels fly from the source's side of the board into the object
  _sparks(sourceId, node, now) {
    const { width, height } = this.state.scenario.board;
    const count = this.state.sources.length;
    const sx = ((this.sourceIndex[sourceId] + 0.5) / count) * width * TILE;
    const sq = squareFor(node, node.step);
    for (let i = 0; i < 10; i++) {
      this.particles.push({
        x0: sx + (Math.random() - 0.5) * 10, y0: height * TILE + 2,
        x1: sq.c * TILE + 8, y1: sq.r * TILE + 6,
        t0: now + i * 35, dur: 520 + Math.random() * 200,
        color: this.sourceColors[sourceId] || COLORS.white,
      });
    }
  }

  _fit() {
    if (!this.state) return;
    const { width, height } = this.state.scenario.board;
    const wrap = this.canvas.parentElement;
    const availW = wrap.parentElement.clientWidth - 4;
    // leave room for the header, captured tray, toasts and source rack
    const availH = window.innerHeight - 360;
    const fit = Math.min(Math.floor(availW / (width * TILE)), Math.floor(availH / (height * TILE)));
    const scale = Math.max(2, Math.min(5, fit));
    this.scale = scale;
    wrap.style.width = `${width * TILE * scale}px`;
    wrap.style.height = `${height * TILE * scale}px`;
    for (const c of [this.canvas, this.glow]) {
      c.style.width = `${width * TILE * scale}px`;
      c.style.height = `${height * TILE * scale}px`;
    }
    wrap.style.setProperty("--scan", `${scale}px`);
  }

  _hit(e) {
    if (!this.state) return null;
    const r = this.canvas.getBoundingClientRect();
    const x = ((e.clientX - r.left) / r.width) * this.canvas.width;
    const y = ((e.clientY - r.top) / r.height) * this.canvas.height;
    for (const [id, pc] of this.pieces) {
      const n = this.nodes.get(id);
      if (!n || !n.present || pc.alpha < 0.5) continue;
      if (x >= pc.x && x < pc.x + TILE && y >= pc.y && y < pc.y + TILE) return id;
    }
    return null;
  }

  _hover(e) {
    const id = this._hit(e);
    this.hover = id;
    this.canvas.style.cursor = id ? "pointer" : "default";
    if (!id) { this.tooltip.hidden = true; return; }
    const n = this.nodes.get(id);
    const wrapRect = this.canvas.parentElement.getBoundingClientRect();
    this.tooltip.hidden = false;
    this.tooltip.innerHTML = `<b>${n.code}</b> ${escapeHtml(n.label)}<br><span>${Math.round(n.p * 100)}% · ${n.status}</span>`;
    this.tooltip.style.left = `${e.clientX - wrapRect.left + 14}px`;
    this.tooltip.style.top = `${e.clientY - wrapRect.top + 10}px`;
  }

  _advance(now) {
    for (const pc of this.pieces.values()) {
      pc.flash = pc.flashAt ? Math.max(0, Math.min(1, 1 - (now - pc.flashAt) / 700)) * (now >= pc.flashAt ? 1 : 0) : 0;
      if (pc.fade) {
        const t = Math.min(1, (now - pc.fade.t0) / 500);
        pc.alpha = 1 - t;
        pc.y = pc.fade.y - 18 * Math.sin(t * Math.PI * 0.6);
        if (t >= 1) pc.fade = null;
      }
      if (pc.move) {
        const m = pc.move;
        const t = Math.min(1, (now - m.t0) / m.dur);
        const e = t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2;
        pc.x = m.x0 + (m.x1 - m.x0) * e;
        pc.y = m.y0 + (m.y1 - m.y0) * e - (m.drop ? 0 : Math.sin(t * Math.PI) * 5);
        if (m.drop) pc.alpha = Math.min(1, t * 2);
        if (t >= 1) { pc.x = m.x1; pc.y = m.y1; pc.move = null; }
        continue;
      }
      const next = pc.queue[0];
      if (next && now >= next.at) {
        pc.queue.shift();
        if (next.leave) {
          pc.fade = { t0: now, y: pc.y };
          continue;
        }
        pc.move = { x0: pc.x, y0: pc.y, x1: next.c * TILE, y1: next.r * TILE, t0: now, dur: next.drop ? 380 : MOVE_MS, drop: next.drop };
      }
    }
  }

  _frame(now) {
    requestAnimationFrame((t) => this._frame(t));
    if (!this.state) return;
    this._advance(now);
    const ctx = this.ctx;
    const { width, height } = this.state.scenario.board;
    drawBoard(ctx, width, height);
    this._edges(ctx, now);

    // pieces, back rows first so lower pieces overlap upper ones
    const order = [...this.pieces.values()].sort((a, b) => a.y - b.y);
    for (const pc of order) {
      const n = this.nodes.get(pc.id);
      if (!n || pc.alpha <= 0.01) continue;
      const x = Math.round(pc.x), y = Math.round(pc.y);
      if (n.band === "hypothesis" && n.status === "crownable" && !n.crowned) drawHalo(ctx, x, y, now);
      drawPiece(ctx, x, y, {
        color: COLORS[n.band], p: n.p, code: n.code, mode: pieceMode(n),
        king: !!n.crowned, alpha: pc.alpha, flash: pc.flash,
      });
      if (pc.id === this.selected) drawSelection(ctx, x, y, COLORS.white);
      else if (pc.id === this.hover) drawSelection(ctx, x, y, "rgba(232,241,255,0.45)");
    }
    this._particles(ctx, now);
    this._glow(now);
  }

  _edges(ctx, now) {
    const focus = this.selected || this.hover;
    const phase = Math.floor(now / 110);
    for (const e of this.state.edges) {
      if (e.flow <= 0.01) continue;
      const a = this.pieces.get(e.from), b = this.pieces.get(e.to);
      const na = this.nodes.get(e.from), nb = this.nodes.get(e.to);
      if (!a || !b || !na.present || !nb.present || a.alpha < 0.5 || b.alpha < 0.5) continue;
      const touches = focus && (e.from === focus || e.to === focus);
      const strength = Math.min(1, e.flow);
      let alpha = 0.18 + 0.55 * strength;
      if (focus) alpha = touches ? 0.95 : 0.07;
      const color = e.kind === "inhibit" ? COLORS.red : COLORS[na.band];
      ctx.fillStyle = rgba(color, alpha);
      const pts = linePoints(a.x + 8, a.y + 6, b.x + 8, b.y + 6);
      for (let i = 3; i < pts.length - 3; i++) {
        // marching toward the child; inhibitors are sparse red dots
        const k = (i + phase) % 4;
        const on = e.kind === "inhibit" ? k === 0 : k < 2;
        if (on) ctx.fillRect(pts[i][0], pts[i][1], 1, 1);
      }
    }
  }

  _particles(ctx, now) {
    this.particles = this.particles.filter((p) => now < p.t0 + p.dur);
    for (const p of this.particles) {
      if (now < p.t0) continue;
      const t = (now - p.t0) / p.dur;
      const e = 1 - Math.pow(1 - t, 3);
      const x = p.x0 + (p.x1 - p.x0) * e;
      const y = p.y0 + (p.y1 - p.y0) * e - Math.sin(t * Math.PI) * 10;
      ctx.fillStyle = rgba(p.color, 1 - t * 0.4);
      ctx.fillRect(Math.round(x), Math.round(y), 1, 1);
    }
  }

  _glow(now) {
    const g = this.gctx;
    g.clearRect(0, 0, this.glow.width, this.glow.height);
    for (const pc of this.pieces.values()) {
      const n = this.nodes.get(pc.id);
      if (!n || pc.alpha < 0.05 || pieceMode(n) !== "solid") continue;
      let a = n.p >= 0.66 ? 0.25 + 0.5 * n.p : 0;
      let color = COLORS[n.band];
      if (n.band === "hypothesis" && n.status === "crownable") {
        a = 0.55 + 0.3 * Math.sin(now / 220);
        if (!n.crowned) color = COLORS.gold;
      }
      if (pc.flash > 0) { a = Math.max(a, pc.flash); color = COLORS.white; }
      if (a <= 0) continue;
      g.fillStyle = rgba(color, a * pc.alpha);
      g.fillRect(Math.round(pc.x) + 2, Math.round(pc.y) + 2, 12, 11);
    }
    for (const p of this.particles) {
      if (now < p.t0) continue;
      const t = (now - p.t0) / p.dur;
      const x = p.x0 + (p.x1 - p.x0) * (1 - Math.pow(1 - t, 3));
      const y = p.y0 + (p.y1 - p.y0) * (1 - Math.pow(1 - t, 3)) - Math.sin(t * Math.PI) * 10;
      g.fillStyle = rgba(p.color, 0.7);
      g.fillRect(Math.round(x) - 1, Math.round(y) - 1, 3, 3);
    }
  }
}

export function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

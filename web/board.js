// The checkers board: piece positions, move animation, edges, hit testing.
import { TILE, COLORS, drawBoard, drawPiece, drawHalo, drawPin, drawSelection, linePoints, rgba } from "./sprites.js";

const MOVE_MS = 240;
const STAGE_MS = 420;
const STEP_AT = [0.33, 0.66];
const OFF_MIN = 220; // off-board column: minimum width, and how much spare width it may take
const OFF_GROW = 180;

const stepFor = (p) => STEP_AT.filter((t) => p >= t).length;

// Belief advances a piece by legal diagonal checkers moves:
// (c, r) -> (c+1, r-1) -> (c, r-2)
function squareFor(node, step) {
  return { c: node.col + (step === 1 ? 1 : 0), r: node.base_row - step };
}

export function pieceMode(n) {
  if (n.offboard) return n.band === "hypothesis" ? "solid" : "grey";
  if (n.band === "signal") return n.status === "dormant" ? "hollow" : n.status === "quiet" ? "grey" : "solid";
  if (n.band === "participant") return n.status === "unresolved" ? "grey" : "solid";
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
    // refit whenever the stage changes size: window resize or the panel being dragged
    new ResizeObserver(() => this._fit()).observe(canvas.parentElement.parentElement);
    requestAnimationFrame((t) => this._frame(t));
  }

  select(id) {
    this.selected = id;
  }

  // The agent points at pieces: they pulse white for a while.
  highlight(ids, ms = 9000) {
    this.highlights = { ids: new Set(ids), until: performance.now() + ms };
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
      this.k = 0;
      this._fit();
    }
    const now = performance.now();
    const prevNodes = prev && !first ? new Map(prev.nodes.map((n) => [n.id, n])) : new Map();
    const where = (n) => (!n ? "hidden" : n.present ? "on" : n.offboard ? "side" : "hidden");

    for (const n of state.nodes) {
      let pc = this.pieces.get(n.id);
      const place = where(n);
      if (!pc || first || !animate) {
        const at = squareFor(n, n.step);
        pc = { id: n.id, x: at.c * TILE, y: at.r * TILE, queue: [], move: null, alpha: place === "on" ? 1 : 0,
               fade: null, flash: 0, step: n.step };
        this.pieces.set(n.id, pc);
        continue;
      }
      const old = prevNodes.get(n.id);
      const was = where(old);

      if (place === "hidden" && was !== "hidden") pc.queue.push({ leave: true, at: now });
      if (place !== "on" && was !== "on") continue;

      // arriving on the board: drop in from above, or hop back in from the
      // off-board column on the left
      if (place === "on" && was !== "on") {
        const sq = squareFor(n, 0);
        pc.queue = [];
        if (was === "side") {
          pc.x = -TILE; pc.y = sq.r * TILE; pc.alpha = 0;
          pc.queue.push({ c: sq.c, r: sq.r, at: now, hop: true, fade: "in" });
        } else {
          pc.x = sq.c * TILE; pc.y = sq.r * TILE - 24; pc.alpha = 0;
          pc.queue.push({ c: sq.c, r: sq.r, at: now, drop: true });
        }
        pc.step = 0;
      }
      const leaving = place === "side";

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
      // leaving: hop off the left edge toward the off-board column
      if (leaving) pc.queue.push({ c: -1, r: squareFor(n, n.step).r, at: base + (state.stages.length + 1) * STAGE_MS, hop: true, fade: "out" });

      if (n.band === "participant" && old) {
        if (n.status === "resolved" && old.status !== "resolved") pc.flashAt = now + STAGE_MS;
        const before = new Set((old.detail && old.detail.sources) || []);
        for (const s of (n.detail && n.detail.sources) || []) {
          if (!before.has(s)) this._sparks(s, n, now);
        }
      }
    }
  }

  // fusion sparks: pixels fly from the source's side of the board into the participant
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

  // Fill the space the stage gives us. The scale may be fractional, so the
  // canvas is drawn at a whole number of device pixels per board pixel (at or
  // above the target) and the browser scales it down slightly. That keeps
  // every board pixel the same size instead of a mix of 3s and 4s.
  _fit() {
    if (!this.state) return;
    const { width, height } = this.state.scenario.board;
    const wrap = this.canvas.parentElement;
    const row = wrap.parentElement;
    const off = row.querySelector(".offboard");
    const others = [...row.children].filter((c) => c !== wrap && c !== off);
    const gap = parseFloat(getComputedStyle(row).columnGap) || 0;
    const fixed = others.reduce((w, c) => w + c.offsetWidth + gap, 0) + (off ? OFF_MIN + gap : 0);
    const availW = Math.max(0, row.clientWidth - fixed - 2);
    const availH = Math.max(0, row.clientHeight - 2);
    const scale = Math.max(1.5, Math.min(availW / (width * TILE), availH / (height * TILE)));
    const cssW = Math.floor(width * TILE * scale);
    const cssH = Math.floor(height * TILE * scale);

    // whatever width the board can't use goes to the off-board column
    if (off) off.style.width = `${Math.round(OFF_MIN + Math.min(OFF_GROW, Math.max(0, availW - cssW)))}px`;

    const k = Math.max(1, Math.ceil(scale * (window.devicePixelRatio || 1)));
    if (k !== this.k || this.canvas.width !== width * TILE * k) {
      this.k = k;
      for (const c of [this.canvas, this.glow]) {
        c.width = width * TILE * k;
        c.height = height * TILE * k;
      }
      this.ctx.setTransform(k, 0, 0, k, 0, 0);
      this.gctx.setTransform(k, 0, 0, k, 0, 0);
    }
    wrap.style.width = `${cssW}px`;
    wrap.style.height = `${cssH}px`;
    for (const c of [this.canvas, this.glow]) {
      c.style.width = `${cssW}px`;
      c.style.height = `${cssH}px`;
    }
    wrap.style.setProperty("--scan", `${scale}px`);
    row.style.setProperty("--board-h", `${cssH}px`);
  }

  _hit(e) {
    if (!this.state) return null;
    const r = this.canvas.getBoundingClientRect();
    const { width, height } = this.state.scenario.board;
    const x = ((e.clientX - r.left) / r.width) * width * TILE;
    const y = ((e.clientY - r.top) / r.height) * height * TILE;
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
    const note = n.kept ? "kept on the board by you" : n.status;
    this.tooltip.innerHTML = `<b>${n.code}</b> ${escapeHtml(n.label)}<br><span>${Math.round(n.p * 100)}% · ${note}</span>`;
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
        pc.y = m.y0 + (m.y1 - m.y0) * e - (m.drop ? 0 : Math.sin(t * Math.PI) * (m.hop ? 16 : 5));
        if (m.drop) pc.alpha = Math.min(1, t * 2);
        if (m.fade === "in") pc.alpha = Math.min(1, t * 1.5);
        if (m.fade === "out") pc.alpha = 1 - t;
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
        pc.move = { x0: pc.x, y0: pc.y, x1: next.c * TILE, y1: next.r * TILE, t0: now,
                    dur: next.drop ? 380 : next.hop ? 560 : MOVE_MS, drop: next.drop, hop: next.hop, fade: next.fade };
        if (!next.drop && !next.fade) pc.alpha = 1;
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
      if (this.highlights && now < this.highlights.until && this.highlights.ids.has(pc.id)) drawHalo(ctx, x, y, now * 1.6, COLORS.white);
      drawPiece(ctx, x, y, {
        color: COLORS[n.band], p: n.p, code: n.code, mode: pieceMode(n),
        king: !!n.crowned, alpha: pc.alpha, flash: pc.flash,
      });
      if (n.kept) drawPin(ctx, x, y);
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
    g.clearRect(0, 0, this.state.scenario.board.width * TILE, this.state.scenario.board.height * TILE);
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

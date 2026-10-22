// Hand-authored 16x16 pixel sprites, a 3x5 pixel font, and the board tiles.
// Everything is drawn at native resolution (1 unit = 1 pixel) and upscaled by
// CSS with image-rendering: pixelated, so every line stays chunky.

export const TILE = 16;

export const COLORS = {
  bg: "#07090c",
  dark: "#11161c",
  light: "#171e26",
  signal: "#ffb23f",
  participant: "#3fd8ff",
  hypothesis: "#ff5fd2",
  grey: "#4a5663",
  red: "#ff4d4d",
  gold: "#ffd75e",
  white: "#e8f1ff",
};

// A checker seen from slightly above: top face (T), highlight (h), face shadow
// (d), outline (o), side band (S) and bottom rim (r). Row 11 carries the
// eight belief pips.
const PIECE = [
  "................",
  ".....oooooo.....",
  "...oohhTTTToo...",
  "..ohhTTTTTTTTo..",
  ".ohTTTTTTTTTTTo.",
  ".oTTTTTTTTTTTTo.",
  ".oTTTTTTTTTTTTo.",
  ".oTTTTTTTTTTTdo.",
  ".SoTTTTTTTTTdoS.",
  ".SSooTTTTTdooSS.",
  ".SSSSooooooSSSS.",
  ".rSSSSSSSSSSSSr.",
  "..rSSSSSSSSSSr..",
  "...rrSSSSSSrr...",
  ".....rrrrrr.....",
  "................",
];
const PIP_ROW = 11;
const PIP_COLS = [4, 5, 6, 7, 8, 9, 10, 11];

const FONT = {
  "0": ["111", "101", "101", "101", "111"],
  "1": ["010", "110", "010", "010", "111"],
  "2": ["111", "001", "111", "100", "111"],
  "3": ["111", "001", "111", "001", "111"],
  "4": ["101", "101", "111", "001", "001"],
  "5": ["111", "100", "111", "001", "111"],
  "6": ["111", "100", "111", "101", "111"],
  "7": ["111", "001", "010", "010", "010"],
  "8": ["111", "101", "111", "101", "111"],
  "9": ["111", "101", "111", "001", "111"],
  S: ["011", "100", "010", "001", "110"],
  P: ["111", "101", "111", "100", "100"],
  H: ["101", "101", "111", "101", "101"],
  "?": ["111", "001", "010", "000", "010"],
};

// ------------------------------------------------------------------ color

function rgb(hex) {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function css([r, g, b], a = 1) {
  return `rgba(${r | 0},${g | 0},${b | 0},${a})`;
}

export function mix(a, b, t) {
  const A = typeof a === "string" ? rgb(a) : a;
  const B = typeof b === "string" ? rgb(b) : b;
  return A.map((v, i) => v + (B[i] - v) * t);
}

const darken = (c, t) => mix(c, [0, 0, 0], t);
const lighten = (c, t) => mix(c, [255, 255, 255], t);

export function rgba(hex, a) {
  return css(rgb(hex), a);
}

// ------------------------------------------------------------------ drawing

export function drawText(ctx, text, x, y, color) {
  ctx.fillStyle = color;
  let cx = x;
  for (const ch of text) {
    const g = FONT[ch];
    if (g) {
      g.forEach((row, ry) => {
        [...row].forEach((bit, rx) => {
          if (bit === "1") ctx.fillRect(cx + rx, y + ry, 1, 1);
        });
      });
    }
    cx += 4;
  }
}

/**
 * opts: color (hex), p (0..1), code, mode ("solid" | "grey" | "hollow"),
 *       king, alpha, flash (0..1)
 */
export function drawPiece(ctx, x, y, opts) {
  const { color, p = 0, code = "", mode = "solid", king = false, alpha = 1, flash = 0 } = opts;
  ctx.save();
  ctx.globalAlpha = alpha;
  if (king) {
    drawPieceLayer(ctx, x, y + 2, { color, p, mode, pips: false, flash: 0 });
    drawPieceLayer(ctx, x, y - 2, { color, p, mode, pips: true, gold: true, flash, code });
  } else {
    drawPieceLayer(ctx, x, y, { color, p, mode, pips: true, flash, code });
  }
  ctx.restore();
}

function drawPieceLayer(ctx, x, y, { color, p, mode, pips, gold, flash, code }) {
  const base = mode === "solid" ? color : COLORS.grey;
  const bright = mode === "hollow" ? 0 : 0.35 + 0.65 * p;
  const face = mix(darken(base, 0.7), base, bright);
  const pal = {
    T: face,
    h: lighten(face, 0.35),
    d: darken(face, 0.25),
    o: darken(base, 0.55),
    S: darken(base, 0.45),
    r: darken(base, 0.75),
  };
  const lit = gold ? rgb(COLORS.gold) : lighten(base, 0.35);
  const unlit = darken(base, 0.65);
  const pipsOn = Math.round(p * PIP_COLS.length);

  for (let ry = 0; ry < 16; ry++) {
    const row = PIECE[ry];
    for (let rx = 0; rx < 16; rx++) {
      const ch = row[rx];
      if (ch === ".") continue;
      if (mode === "hollow" && ch !== "o" && ch !== "r") continue;
      let c = pal[ch];
      if (pips && ry === PIP_ROW && PIP_COLS.includes(rx) && mode !== "hollow") {
        c = PIP_COLS.indexOf(rx) < pipsOn ? lit : unlit;
      }
      if (flash > 0) c = mix(c, [255, 255, 255], flash);
      ctx.fillStyle = css(c);
      ctx.fillRect(x + rx, y + ry, 1, 1);
    }
  }
  if (code) {
    const textColor = mode === "hollow" ? darken(base, 0.2) : bright > 0.6 ? darken(base, 0.82) : lighten(base, 0.55);
    drawText(ctx, code, x + 4, y + 4, css(textColor));
  }
}

/** A pulsing gold ring for crownable hypotheses. */
export function drawHalo(ctx, x, y, t, color = COLORS.gold) {
  const a = 0.35 + 0.35 * Math.sin(t / 220);
  ctx.fillStyle = rgba(color, a);
  for (let ry = 0; ry < 16; ry++) {
    for (let rx = 0; rx < 16; rx++) {
      if (PIECE[ry][rx] !== ".") continue;
      const near = [[1, 0], [-1, 0], [0, 1], [0, -1]].some(([dx, dy]) => {
        const r = PIECE[ry + dy];
        return r && r[rx + dx] && r[rx + dx] !== ".";
      });
      if (near) ctx.fillRect(x + rx, y + ry, 1, 1);
    }
  }
}

/** Corner brackets around a square. */
export function drawSelection(ctx, x, y, color) {
  ctx.fillStyle = color;
  const pts = [[0, 0], [1, 0], [0, 1], [15, 0], [14, 0], [15, 1], [0, 15], [1, 15], [0, 14], [15, 15], [14, 15], [15, 14]];
  for (const [dx, dy] of pts) ctx.fillRect(x + dx, y + dy, 1, 1);
}

const BAND_TINT = { 0: "hypothesis", 1: "hypothesis", 2: "hypothesis", 3: "participant", 4: "participant", 5: "participant", 6: "signal", 7: "signal", 8: "signal" };

export function drawBoard(ctx, w, h) {
  for (let r = 0; r < h; r++) {
    for (let c = 0; c < w; c++) {
      ctx.fillStyle = (c + r) % 2 === 1 ? COLORS.dark : COLORS.light;
      ctx.fillRect(c * TILE, r * TILE, TILE, TILE);
      const band = BAND_TINT[r];
      if (band) {
        ctx.fillStyle = rgba(COLORS[band], (c + r) % 2 === 1 ? 0.035 : 0.015);
        ctx.fillRect(c * TILE, r * TILE, TILE, TILE);
      }
    }
  }
  // band seams: a dotted pixel line between bands
  for (const r of [3, 6]) {
    ctx.fillStyle = "rgba(255,255,255,0.06)";
    for (let x = 0; x < w * TILE; x += 2) ctx.fillRect(x, r * TILE, 1, 1);
  }
}

/** A small pin: the user is keeping this piece on the board. */
export function drawPin(ctx, x, y) {
  ctx.fillStyle = COLORS.white;
  ctx.fillRect(x + 12, y, 3, 2);
  ctx.fillStyle = COLORS.red;
  ctx.fillRect(x + 13, y, 1, 1);
  ctx.fillStyle = "rgba(232,241,255,0.7)";
  ctx.fillRect(x + 13, y + 2, 1, 2);
}

/** Integer points on a line (Bresenham). */
export function linePoints(x0, y0, x1, y1) {
  x0 |= 0; y0 |= 0; x1 |= 0; y1 |= 0;
  const pts = [];
  const dx = Math.abs(x1 - x0), sx = x0 < x1 ? 1 : -1;
  const dy = -Math.abs(y1 - y0), sy = y0 < y1 ? 1 : -1;
  let err = dx + dy;
  for (;;) {
    pts.push([x0, y0]);
    if (x0 === x1 && y0 === y1) break;
    const e2 = 2 * err;
    if (e2 >= dy) { err += dy; x0 += sx; }
    if (e2 <= dx) { err += dx; y0 += sy; }
  }
  return pts;
}

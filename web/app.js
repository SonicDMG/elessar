// Wiring: fetch state, send actions, route renders.
import { Board, escapeHtml } from "./board.js";
import * as P from "./panel.js";
import * as sprites from "./sprites.js";
import * as Life from "./life.js";
import * as Topology from "./topology.js";
import * as Landscape from "./landscape.js";

const $ = (s) => document.querySelector(s);
const board = new Board($("#board"), $("#glow"), $("#tip"));
let state = null;
let scenarioId = null;
let selected = null;
let busy = false;

board.onSelect = (id) => {
  selected = id;
  setTab("object");
};

// ---------------------------------------------------------------- modes
// Board is built in; Life and Topology are modules with mount/update/unmount
// (see docs/modes.md). They share the session through `ctx`.
const MODES = { life: Life, topology: Topology, landscape: Landscape };
let mode = "board";
const ctx = {
  get state() { return state; },
  get selected() { return selected; },
  select(id) {
    selected = id;
    board.select(id);
    setTab("object");
  },
  act,
  highlight(ids, ms) { if (mode === "board") board.highlight(ids, ms); },
  sprites,
};

function setMode(name) {
  if (!(name in MODES) && name !== "board") name = "board";
  if (name === mode && document.querySelector(`[data-mode="${name}"].on`)) return;
  if (MODES[mode]) MODES[mode].unmount();
  mode = name;
  document.querySelectorAll("[data-mode]").forEach((b) => b.classList.toggle("on", b.dataset.mode === name));
  document.querySelectorAll("[data-mode-view]").forEach((v) => { v.hidden = v.dataset.modeView !== name; });
  const url = new URL(location.href);
  if (name === "board") url.searchParams.delete("mode"); else url.searchParams.set("mode", name);
  history.replaceState(null, "", url);
  if (MODES[name]) {
    MODES[name].mount($(`#mode-${name}`), ctx);
    if (state) MODES[name].update(state);
  }
}

async function getJSON(url, opts) {
  const res = await fetch(url, opts);
  const body = await res.json();
  if (!res.ok) throw new Error(body.error || res.statusText);
  return body;
}

async function init() {
  const { default: def, scenarios, invalid } = await getJSON("/api/scenarios");
  const fromUrl = new URLSearchParams(location.search).get("scenario");
  scenarioId = scenarios.some((s) => s.id === fromUrl) ? fromUrl : def;
  const sel = $("#scenario");
  sel.innerHTML = scenarios.map((s) => `<option value="${s.id}">${escapeHtml(s.title)} · ${s.sources} sources · ${s.pieces} pieces</option>`).join("")
    + Object.keys(invalid).map((name) => `<option disabled>${escapeHtml(name)} (invalid pack)</option>`).join("");
  sel.value = scenarioId;
  setMode(new URLSearchParams(location.search).get("mode") || "board");
  sel.addEventListener("change", () => {
    scenarioId = sel.value;
    const url = new URL(location.href);
    url.searchParams.set("scenario", scenarioId);
    history.replaceState(null, "", url);
    load();
  });
  await load();
}

async function load() {
  state = await getJSON(`/api/state?scenario=${scenarioId}`);
  selected = null;
  board.select(null);
  render(false);
}

async function act(payload) {
  if (busy) return;
  busy = true;
  try {
    const r = await getJSON("/api/action", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ scenario: scenarioId, ...payload }),
    });
    state = r.state;
    if (payload.action === "reset") { selected = null; board.select(null); }
    render(payload.action !== "reset");
    r.events.forEach((ev, i) => setTimeout(() => P.toast($("#toasts"), ev), 300 + i * 450));
    if (r.events.some((e) => e.kind === "crowned")) setTimeout(finale, 1400);
  } catch (e) {
    P.toast($("#toasts"), { kind: "error", title: "Error", text: e.message, points: 0 });
    render(false);
  } finally {
    busy = false;
  }
}

function render(animate) {
  board.setState(state, animate);
  $("#premise").textContent = state.scenario.premise;
  $("#score").textContent = state.score;
  $("#feedback").checked = state.feedback;
  document.querySelectorAll("[data-engine]").forEach((b) => {
    b.classList.toggle("on", b.dataset.engine === state.engine);
    b.disabled = b.dataset.engine === "jev" && !state.jev.available;
  });
  $("#jev-status").textContent = state.jev.available
    ? `${state.jev.calls} calls · ${state.jev.cached} cached · $${state.jev.cost.toFixed(5)}`
    : "jev off: no OPENROUTER_API_KEY";
  const n = state.merges.pending.length;
  $("#mergecount").hidden = !n;
  $("#mergecount").textContent = n;
  P.renderRack($("#rack"), state, act, peek);
  P.renderOffboard($("#offboard"), state, act, (id) => { selected = id; board.select(id); setTab("object"); });
  if (MODES[mode]) MODES[mode].update(state);
  renderPanel();
}

function renderPanel() {
  P.renderObject($("#tab-object"), state, selected, act);
  P.renderMerges($("#tab-merges"), state, act);
  P.renderLog($("#tab-log"), state);
  renderAgentTab();
}

function renderAgentTab() {
  P.renderAgent($("#agent-log"), $("#agent-meta"), state, {
    ask: askAgent,
    decide: (pid, decision) => act({ action: "agent_decide", proposal: pid, decision }),
    selectCode: (code) => {
      const n = state.nodes.find((x) => x.code === code);
      if (!n) return;
      board.highlight([n.id], 4000);
      selected = n.id;
      board.select(n.id);
      P.renderObject($("#tab-object"), state, selected, act);
    },
  });
  const log = $("#agent-log");
  log.scrollTop = log.scrollHeight;
}

let asking = false;
async function askAgent(text) {
  const message = (text ?? $("#agent-input").value).trim();
  if (!message || asking) return;
  asking = true;
  $("#agent-input").value = "";
  setTab("agent");
  const log = $("#agent-log");
  log.insertAdjacentHTML("beforeend", `<div class="msg user">${escapeHtml(message)}</div><div class="msg bot thinking">The agent is reading the board…</div>`);
  log.scrollTop = log.scrollHeight;
  try {
    const r = await getJSON("/api/agent", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ scenario: scenarioId, message, selected }),
    });
    state = r.state;
    if (r.turn.highlights.length) board.highlight(r.turn.highlights);
    render(false);
  } catch (e) {
    log.querySelector(".thinking")?.remove();
    P.toast($("#toasts"), { kind: "error", title: "Agent", text: e.message, points: 0 });
  } finally {
    asking = false;
  }
}

function setTab(name) {
  document.querySelectorAll(".tabs button").forEach((b) => b.classList.toggle("on", b.dataset.tab === name));
  for (const t of ["object", "merges", "log", "agent"]) $(`#tab-${t}`).hidden = t !== name;
  if (state) renderPanel();
}

async function jevCache(op, signal) {
  const opts = op ? { method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify(op === "reask_all" ? { scenario: scenarioId, op: "reask" } : { scenario: scenarioId, op, signal }) } : undefined;
  try {
    const r = await getJSON(`/api/jev/cache?scenario=${scenarioId}`, opts);
    const rep = op ? r.report : r;
    if (op) { state = r.state; render(false); }
    P.renderJevCache($("#modal"), rep, state, jevCache);
  } catch (e) {
    P.toast($("#toasts"), { kind: "error", title: "Jev cache", text: e.message, points: 0 });
  }
}

async function peek(sourceId) {
  await P.showPeek($("#modal"), state, sourceId);
  act({ action: "peek", source: sourceId });
}

function finale() {
  const text = state.scenario.narrative.finale;
  if (!text) return;
  $("#modal .modal-body").innerHTML = `<header class="fin"><b>♛ Case closed</b></header><p>${escapeHtml(text)}</p>
    <p class="dim small">Score ${state.score}. Every step is in the Log tab, the audit trail.</p>`;
  $("#modal").hidden = false;
}

// Side panel width: drag its left edge; double-click to reset. Remembered per browser.
const PANEL_DEFAULT = 370;
function setPanelWidth(px, save) {
  const w = Math.round(Math.max(280, Math.min(px, window.innerWidth * 0.6, 760)));
  document.documentElement.style.setProperty("--panel-w", `${w}px`);
  if (save) {
    try { localStorage.setItem("elessar.panelWidth", String(w)); } catch (e) { /* storage unavailable */ }
  }
}
try {
  const saved = Number(localStorage.getItem("elessar.panelWidth"));
  if (saved) setPanelWidth(saved, false);
} catch (e) { /* storage unavailable */ }

$("#resizer").addEventListener("pointerdown", (e) => {
  const handle = e.currentTarget;
  handle.setPointerCapture(e.pointerId);
  handle.classList.add("dragging");
  document.body.classList.add("resizing");
  const move = (ev) => setPanelWidth(window.innerWidth - ev.clientX, false);
  const up = (ev) => {
    handle.releasePointerCapture(ev.pointerId);
    handle.classList.remove("dragging");
    document.body.classList.remove("resizing");
    handle.removeEventListener("pointermove", move);
    handle.removeEventListener("pointerup", up);
    setPanelWidth(window.innerWidth - ev.clientX, true);
  };
  handle.addEventListener("pointermove", move);
  handle.addEventListener("pointerup", up);
});
$("#resizer").addEventListener("dblclick", () => setPanelWidth(PANEL_DEFAULT, true));

document.querySelectorAll(".tabs button").forEach((b) => b.addEventListener("click", () => setTab(b.dataset.tab)));
document.querySelectorAll("[data-mode]").forEach((b) => b.addEventListener("click", () => setMode(b.dataset.mode)));
$("#reset").addEventListener("click", () => act({ action: "reset" }));
$("#jev-cache").addEventListener("click", () => jevCache());
$("#agent-form").addEventListener("submit", (e) => { e.preventDefault(); askAgent(); });
$("#agent-input").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); askAgent(); }
});
$("#agent-new").addEventListener("click", () => act({ action: "agent_reset" }));
document.querySelectorAll("[data-engine]").forEach((b) =>
  b.addEventListener("click", () => act({ action: "engine", engine: b.dataset.engine })));
$("#feedback").addEventListener("change", (e) => act({ action: "feedback", on: e.target.checked }));
$("#modal .close").addEventListener("click", () => { $("#modal").hidden = true; });
$("#modal").addEventListener("click", (e) => { if (e.target.id === "modal") $("#modal").hidden = true; });
document.addEventListener("keydown", (e) => {
  if (e.target.tagName === "SELECT" || e.target.tagName === "INPUT" || e.target.tagName === "TEXTAREA") return;
  if (e.key === "Escape") {
    if (!$("#modal").hidden) $("#modal").hidden = true;
    else { selected = null; board.select(null); renderPanel(); }
  }
  const i = Number(e.key) - 1;
  if (state && i >= 0 && i < state.sources.length) {
    const s = state.sources[i];
    act({ action: s.connected ? "disconnect" : "connect", source: s.id });
  }
});

init().catch((e) => P.toast($("#toasts"), { kind: "error", title: "Could not start", text: e.message, points: 0 }));

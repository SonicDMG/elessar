// Wiring: fetch state, send actions, route renders.
import { Board, escapeHtml } from "./board.js";
import * as P from "./panel.js";

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
  sel.addEventListener("change", () => {
    scenarioId = sel.value;
    history.replaceState(null, "", `?scenario=${scenarioId}`);
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
  const n = state.merges.pending.length;
  $("#mergecount").hidden = !n;
  $("#mergecount").textContent = n;
  P.renderRack($("#rack"), state, act, peek);
  P.renderCaptured($("#captured"), state);
  renderPanel();
}

function renderPanel() {
  P.renderObject($("#tab-object"), state, selected, act);
  P.renderMerges($("#tab-merges"), state, act);
  P.renderLog($("#tab-log"), state);
}

function setTab(name) {
  document.querySelectorAll(".tabs button").forEach((b) => b.classList.toggle("on", b.dataset.tab === name));
  for (const t of ["object", "merges", "log"]) $(`#tab-${t}`).hidden = t !== name;
  if (state) renderPanel();
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

document.querySelectorAll(".tabs button").forEach((b) => b.addEventListener("click", () => setTab(b.dataset.tab)));
$("#reset").addEventListener("click", () => act({ action: "reset" }));
$("#feedback").addEventListener("change", (e) => act({ action: "feedback", on: e.target.checked }));
$("#modal .close").addEventListener("click", () => { $("#modal").hidden = true; });
$("#modal").addEventListener("click", (e) => { if (e.target.id === "modal") $("#modal").hidden = true; });
document.addEventListener("keydown", (e) => {
  if (e.target.tagName === "SELECT" || e.target.tagName === "INPUT") return;
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

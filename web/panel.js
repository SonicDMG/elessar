// Everything that is HTML rather than pixels: details view, merges, log,
// source rack, raw-payload peek and insight toasts.
import { escapeHtml as h, pieceMode } from "./board.js";
import { COLORS, drawPiece, drawPin } from "./sprites.js";

const pct = (p) => `${Math.round(p * 100)}%`;
const num = (x, d = 2) => Number(x).toFixed(d);
const BAND_NAME = { signal: "Signal", participant: "Participant", hypothesis: "Hypothesis" };

function chip(state, sourceId, extra = "") {
  const s = state.sources.find((x) => x.id === sourceId);
  const color = s ? s.color : "#888";
  return `<span class="chip" style="--c:${color}" ${extra}>${h(s ? s.name : sourceId)}</span>`;
}

function nameOf(state, id) {
  const n = state.nodes.find((x) => x.id === id);
  return n ? `<b class="code b-${n.band}">${n.code}</b> ${h(n.label)}` : h(id);
}

function bar(p, band) {
  return `<span class="bar b-${band}"><i style="width:${Math.round(p * 100)}%"></i></span>`;
}

// ------------------------------------------------------------------ details view

export function renderObject(el, state, id, act) {
  const n = state.nodes.find((x) => x.id === id);
  if (!n) {
    el.innerHTML = `<div class="empty">
      <p>${h(state.scenario.narrative.intro || "")}</p>
      <p class="dim">Click any piece to inspect it: where each value came from, and how its belief was derived.</p>
    </div>`;
    return;
  }
  const head = `<header class="obj-head b-${n.band}">
      <div><span class="code">${n.code}</span> <span class="kind">${BAND_NAME[n.band]}${n.detail.type ? " · " + h(n.detail.type) : ""}</span></div>
      <h2>${h(n.label)}</h2>
      <div class="belief">${bar(n.p, n.band)} <b>${pct(n.p)}</b> <span class="status s-${n.status}">${n.status}</span></div>
    </header>`;
  let body = offboardCallout(n);
  if (n.band === "signal") body += signalView(state, n);
  if (n.band === "participant") body += participantView(state, n);
  if (n.band === "hypothesis") body += hypothesisView(state, n);
  el.innerHTML = head + body;
  wireRestore(el, act);

  el.querySelectorAll("[data-judge]").forEach((b) =>
    b.addEventListener("click", () => act({ action: "judge", signal: n.id, judgement: b.dataset.judge })));
  el.querySelectorAll("[data-crown]").forEach((b) =>
    b.addEventListener("click", () => act({ action: "crown", hypothesis: n.id })));
  el.querySelectorAll("[data-unmerge]").forEach((b) =>
    b.addEventListener("click", () => act({ action: "unmerge", proposal: b.dataset.unmerge })));
  el.querySelectorAll("[data-raw]").forEach((b) =>
    b.addEventListener("click", () => {
      const pre = b.parentElement.querySelector("pre");
      pre.hidden = !pre.hidden;
    }));
}

function restoreButton(n) {
  const o = n.offboard;
  if (o.restore === "keep") return `<button class="gold" data-keep="${h(n.id)}">return to board</button>`;
  return `<button class="gold" data-reconnect="${h(o.sources.join(","))}">return to board (reconnect ${o.sources.length > 1 ? "its sources" : "its source"})</button>`;
}

function offboardCallout(n) {
  if (n.offboard) {
    return `<div class="callout off">
      <b>Off the board.</b> ${h(n.offboard.reason)}
      <div class="btns">${restoreButton(n)}</div>
      ${n.offboard.restore === "keep" ? `<p class="dim small">Returning it keeps it in play as your call. It won't change its belief; to do that, judge the signals that cut it.</p>` : ""}
    </div>`;
  }
  if (n.kept) {
    return `<div class="callout kept">
      <b>Kept on the board by you.</b> On the evidence alone it would be off the board. ${h(n.kept_reason)}
      <div class="btns"><button data-release="${h(n.id)}">let the board move it aside</button></div>
    </div>`;
  }
  return "";
}

function wireRestore(el, act) {
  el.querySelectorAll("[data-keep]").forEach((b) =>
    b.addEventListener("click", () => act({ action: "keep", hypothesis: b.dataset.keep })));
  el.querySelectorAll("[data-release]").forEach((b) =>
    b.addEventListener("click", () => act({ action: "release", hypothesis: b.dataset.release })));
  el.querySelectorAll("[data-reconnect]").forEach((b) =>
    b.addEventListener("click", () => act({ action: "reconnect", sources: b.dataset.reconnect.split(",") })));
}

function signalView(state, n) {
  const d = n.detail;
  const judge = ["trust", "discount", "dismiss"].map((j) =>
    `<button data-judge="${j}" class="${d.judgement === j ? "on" : ""}">${j}</button>`).join("");
  const formula = n.status === "firing"
    ? `<div class="formula">a = confidence ${num(d.confidence)} × strength ${num(d.strength)} <span class="dim">(${d.engine_used})</span> × judgement ${num({ trust: 1, discount: 0.5, dismiss: 0 }[d.judgement], 1)} = <b>${num(n.p)}</b></div>`
    : `<div class="formula dim">${n.status === "dormant" ? `Waiting on: ${d.sources.map((s) => chip(state, s)).join(" ")}` : "Sources connected, but no record matched the rule."}</div>`;
  const links = d.links.length
    ? d.links.map((l) => `<li>${nameOf(state, l.participant)} <span class="dim">${l.hits} hit${l.hits > 1 ? "s" : ""} → w ${num(l.w)}</span></li>`).join("")
    : `<li class="dim">No participants linked${n.status === "firing" ? ": the hits haven't resolved into any participant on the board yet." : "."}</li>`;
  const hits = d.hits.map((hit, i) => `<li>
      <div>${hit.records.map((r) => `${chip(state, r.source)} <span class="ptr">${h(r.pointer)}</span>`).join(" + ")}
        <button class="link" data-raw>raw</button>
        <pre hidden>${hit.records.map((r) => h(r.excerpt)).join("\n\n")}</pre></div>
    </li>`).join("");
  return `
    <p>${h(d.explain)}</p>
    <section><h3>Rule <span class="dim">${h(d.kind)}</span></h3><pre class="rule">${h(JSON.stringify(d.rule, null, 1))}</pre></section>
    <section><h3>Activation</h3>${formula}</section>
    ${enginesView(state, n)}
    <section><h3>Your judgement</h3><div class="btns">${judge}</div></section>
    <section><h3>Implicates <span class="dim">${d.implicates.types ? d.implicates.types.join(", ") : "any type"}, w ${num(d.implicates.w)}</span></h3><ul>${links}</ul></section>
    ${d.hits.length ? `<section><h3>Hits (${d.hits.length})</h3><ul class="hits">${hits}</ul></section>` : ""}`;
}

// Rule vs Jev, side by side: what each concluded, and where they disagree.
function enginesView(state, n) {
  const d = n.detail;
  const drives = (e) => (d.engine_used === e ? `<span class="drives">drives the board</span>` : "");
  const rule = `<div class="engine-card ${d.engine_used === "rules" ? "on" : ""}">
      <div class="ec-head"><b>Rule</b> <span class="dim">${h(d.kind)}</span> ${drives("rules")}</div>
      <div>${d.rule_hit_count} hit${d.rule_hit_count === 1 ? "" : "s"} · strength <b>${num(d.rule_strength)}</b></div>
      <div class="dim small">Exact field conditions; strength is ${typeof d.rule.strength === "object" ? `the record's own <code>${h(d.rule.strength.field)}</code>` : "hand-set in the pack"}.</div>
    </div>`;
  const j = d.jev;
  let jevCard;
  if (!j) {
    jevCard = `<div class="engine-card dim"><div class="ec-head"><b>Jev</b></div>
      <div class="small">${d.kind === "co_resolved" ? "Structural rule: it joins facts across sources, so there's nothing for Jev to judge." : "This signal has no Jev question."}</div></div>`;
  } else if (!j.asked) {
    jevCard = `<div class="engine-card dim"><div class="ec-head"><b>Jev</b></div>
      <div class="small">${state.jev.available ? "Not asked yet: connect the source." : "Off: add OPENROUTER_API_KEY to .env."}</div></div>`;
  } else {
    const rows = j.records.map((r) => {
      const p = r.noul == null ? null : r.noul;
      const dis = p != null && r.rule_hit !== r.jev_hit;
      return `<tr class="${dis ? "disagree" : ""}">
        <td><button class="link" data-raw>${h(r.pointer)}</button><pre hidden>${h(r.excerpt)}</pre></td>
        <td class="jbar"><span class="bar b-signal"><i style="width:${p == null ? 0 : Math.round(p * 100)}%"></i></span></td>
        <td>${p == null ? `<span class="err">${h(r.error || "—")}</span>` : num(p)}</td>
        <td class="small">${r.rule_hit ? "rule ✓" : "rule ✗"}</td>
        <td class="small">${dis ? "disagree" : ""}</td>
      </tr>`;
    }).join("");
    jevCard = `<div class="engine-card ${d.engine_used === "jev" ? "on" : ""}">
      <div class="ec-head"><b>Jev</b> <span class="dim">${h(j.model || "")}</span> ${drives("jev")}</div>
      <div class="q">“${h(j.question)}”</div>
      <table class="jev-rows">${rows}</table>
      <div>${j.hit_count} hit${j.hit_count === 1 ? "" : "s"} at ≥ ${num(j.threshold)} · strength <b>${num(j.strength)}</b>
        ${j.disagreements ? ` · <span class="warn">${j.disagreements} disagreement${j.disagreements === 1 ? "" : "s"} with the rule</span>` : ` · <span class="dim">agrees with the rule</span>`}</div>
      <div class="dim small">Each number is Jev's calibrated probability that the answer is yes for that record. Strength is the highest one among its hits.</div>
    </div>`;
  }
  return `<section><h3>Evidence engines</h3><div class="engines">${rule}${jevCard}</div></section>`;
}

function participantView(state, n) {
  const d = n.detail;
  if (n.status === "absent" || n.status === "removed") {
    return `<p class="dim">Not on the board yet: no connected source has produced a ${h(d.type)} with ${h(Object.entries(d.anchor).map(([k, v]) => `${k} = ${v}`).join(""))}.</p>`;
  }
  const rows = Object.entries(d.props).map(([prop, vals]) => {
    const conflict = d.conflicts.includes(prop) ? ` <span class="warn" title="sources disagree">≠</span>` : "";
    const cells = vals.map((v) => `<div class="val">
        <span>${h(v.value)}</span>
        ${v.provenance.map((pr) => `<span class="prov">${chip(state, pr.source)}<button class="link" data-raw>${h(pr.pointer)}</button><pre hidden>${h(pr.field)} ← ${h(pr.excerpt)}</pre></span>`).join("")}
      </div>`).join("");
    return `<tr><th>${h(prop)}${conflict}</th><td>${cells}</td></tr>`;
  }).join("");
  const missing = n.status === "unresolved"
    ? `<p class="callout">Unresolved: no connected source has given this ${h(d.type)} a name yet. It is known only by ${h(Object.keys(d.anchor)[0])}.</p>` : "";
  const merges = (d.merges || []).map((pid) =>
    `<li><span class="dim">committed merge</span> <button data-unmerge="${h(pid)}">revert</button></li>`).join("");
  return `${missing}
    <section><h3>Properties <span class="dim">${d.fragment_count} fragments from ${d.sources.length} source${d.sources.length > 1 ? "s" : ""}</span></h3>
      <div class="sources">${d.sources.map((s) => chip(state, s)).join(" ")}</div>
      <table class="props">${rows}</table></section>
    ${merges ? `<section><h3>Merges</h3><ul>${merges}</ul></section>` : ""}
    <section><h3>Involvement</h3>${derivation(state, d.breakdown, false)}</section>`;
}

function hypothesisView(state, n) {
  const d = n.detail;
  const c = state.crown && state.crown.hypothesis === n.id ? state.crown : null;
  const crown = c
    ? `<p class="callout v-${c.verdict}">Crowned at ${pct(c.p)}: <b>${c.verdict}</b>.</p>`
    : `<div class="btns"><button data-crown class="${n.status === "crownable" ? "gold" : ""}">♛ crown this explanation</button></div>
       <p class="dim small">${n.status === "crownable" ? "The posterior is above 85%. It's your call." : "You can crown at any time. The board will tell you whether the evidence backs you."}</p>`;
  return `<p>${h(d.blurb)}</p>
    <section><h3>Decision</h3>${crown}</section>
    <section><h3>Belief</h3>${derivation(state, d.breakdown, true)}</section>`;
}

function derivation(state, b, isHyp) {
  if (!b) return `<p class="dim">No belief yet.</p>`;
  const sup = b.support.map((t) => `<tr><td>${t.from === "feedback" ? "relevance feedback" : nameOf(state, t.from)}</td><td>${num(t.w)} × ${num(t.p)}</td><td>${num(t.term)}</td></tr>`).join("");
  const inh = b.inhibit.map((t) => `<tr class="inh"><td>${nameOf(state, t.from)}</td><td>1 − ${num(t.w)} × ${num(t.p)}</td><td>× ${num(t.factor)}</td></tr>`).join("");
  return `<table class="deriv">
      <tr class="dim"><td>${isHyp ? "prior" : "leak"}</td><td></td><td>${num(b.leak)}</td></tr>
      ${sup}
      <tr class="sum"><td>support = 1 − (1 − leak)·Π(1 − w·p)</td><td></td><td>${num(b.support_p)}</td></tr>
      ${inh}
      ${b.inhibit.length ? `<tr class="sum"><td>× inhibition</td><td></td><td>${num(b.inhibit_factor)}</td></tr>` : ""}
      ${isHyp ? `<tr class="sum"><td>÷ competition (Σ raw, if > 1)</td><td></td><td>${num(b.divisor)}</td></tr>` : ""}
      <tr class="final"><td>belief</td><td></td><td>${num(b.final)}</td></tr>
    </table>`;
}

// ------------------------------------------------------------------ merges + log

export function renderMerges(el, state, act) {
  const { pending, committed } = state.merges;
  const side = (x) => `<div class="side">${chip(state, x.source)} <b>${h(x.value)}</b> <span class="dim">${h(x.property)}</span></div>`;
  const items = pending.map((p) => `<li class="proposal">
      <div class="score">${pct(p.score)} similar · ${h(p.type)}</div>
      ${side(p.a)}${side(p.b)}
      ${p.conflict.length ? `<div class="warn">Key conflict on ${p.conflict.map(h).join(", ")}: these may be different things.</div>` : ""}
      <div class="btns"><button data-merge="${h(p.id)}" class="gold">commit merge</button><button data-reject="${h(p.id)}">reject</button></div>
    </li>`).join("");
  const done = committed.map((c) => `<li class="committed ${c.active ? "" : "dim"}">
      ${chip(state, c.a_source)} ⟷ ${chip(state, c.b_source)} ${c.active ? "" : "(inactive: a source is disconnected)"}
      <button data-unmerge="${h(c.id)}">revert</button></li>`).join("");
  el.innerHTML = `
    <p class="dim small">Exact key matches merge automatically. Fuzzy matches are only proposals: you decide, and you can revert.</p>
    ${items ? `<ul>${items}</ul>` : `<p class="dim">No pending proposals.</p>`}
    ${done ? `<h3>Committed</h3><ul>${done}</ul>` : ""}`;
  el.querySelectorAll("[data-merge]").forEach((b) => b.addEventListener("click", () => act({ action: "merge", proposal: b.dataset.merge })));
  el.querySelectorAll("[data-reject]").forEach((b) => b.addEventListener("click", () => act({ action: "reject", proposal: b.dataset.reject })));
  el.querySelectorAll("[data-unmerge]").forEach((b) => b.addEventListener("click", () => act({ action: "unmerge", proposal: b.dataset.unmerge })));
}

// ------------------------------------------------------------------ agent

// A small, safe markdown subset: paragraphs, "- " bullets, **bold**, `code`.
// Piece codes like H3 or P1 become links that select the piece.
function md(text, state) {
  const codes = new Set(state.nodes.map((n) => n.code));
  const inline = (s) => h(s)
    .replace(/`([^`]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>")
    .replace(/\b([SPH][0-9])\b/g, (m) => (codes.has(m) ? `<button class="link piece-ref" data-code="${m}">${m}</button>` : m));
  const out = [];
  let list = null;
  for (const line of String(text || "").split("\n")) {
    const bullet = line.match(/^\s*[-*] (.*)/);
    if (bullet) {
      list = list || [];
      list.push(`<li>${inline(bullet[1])}</li>`);
      continue;
    }
    if (list) { out.push(`<ul>${list.join("")}</ul>`); list = null; }
    if (line.trim()) out.push(`<p>${inline(line.replace(/^#+\s*/, ""))}</p>`);
  }
  if (list) out.push(`<ul>${list.join("")}</ul>`);
  return out.join("");
}

const SUGGESTIONS = [
  "What explains this so far, and how sure should I be?",
  "Which source should I connect next, and why?",
  "Where do the rules and Jev disagree, and who's right?",
  "Why did the leading hypothesis win? Walk me through the math.",
];

export function renderAgent(el, meta, state, { ask, decide, selectCode }) {
  const a = state.agent;
  meta.textContent = a.available ? `${a.model}` : "off: add OPENROUTER_API_KEY to .env";
  if (!a.transcript.length) {
    el.innerHTML = `<div class="agent-intro">
        <p><b>The agent</b> reads the live board: every signal, participant, hypothesis and its derivation. Ask it to explain, compare, or suggest a next step. It can run what-ifs on a copy of the board and ask Jev new questions. It only <i>proposes</i> changes; you approve them.</p>
        <div class="suggest">${SUGGESTIONS.map((s) => `<button data-suggest="${h(s)}">${h(s)}</button>`).join("")}</div>
      </div>`;
  } else {
    el.innerHTML = a.transcript.map((t) => {
      if (t.role === "user") return `<div class="msg user">${h(t.text)}</div>`;
      const tools = t.tools.length
        ? `<details class="tools"><summary>${t.tools.length} step${t.tools.length === 1 ? "" : "s"}: ${[...new Set(t.tools.map((x) => x.name))].join(", ")}</summary>
            <ul>${t.tools.map((x) => `<li class="${x.error ? "err" : ""}"><code>${h(x.name)}</code> ${h(JSON.stringify(x.input))}</li>`).join("")}</ul></details>` : "";
      const props = t.proposals.map((pid) => {
        const p = a.proposals[pid];
        const buttons = p.status === "pending"
          ? `<button class="gold" data-decide="approve" data-pid="${p.id}">approve</button><button data-decide="decline" data-pid="${p.id}">decline</button>`
          : `<span class="status s-${p.status === "approved" ? "resolved" : "captured"}">${p.status}</span>`;
        return `<div class="proposal-card"><div><b>Proposed:</b> ${h(p.label)}</div><div class="dim">${h(p.reason)}</div><div class="btns">${buttons}</div></div>`;
      }).join("");
      return `<div class="msg bot">${md(t.text, state)}${props}${tools}<div class="dim small">${(t.ms / 1000).toFixed(1)} s · $${t.cost.toFixed(4)}</div></div>`;
    }).join("");
  }
  el.querySelectorAll("[data-suggest]").forEach((b) => b.addEventListener("click", () => ask(b.dataset.suggest)));
  el.querySelectorAll("[data-decide]").forEach((b) => b.addEventListener("click", () => decide(b.dataset.pid, b.dataset.decide)));
  el.querySelectorAll(".piece-ref").forEach((b) => b.addEventListener("click", () => selectCode(b.dataset.code)));
}

export function renderLog(el, state) {
  el.innerHTML = `<ol class="log">${[...state.audit].reverse().map((a) =>
    `<li><span class="t">${h(a.at.slice(11, 19))}</span> <b>${h(a.action)}</b> ${h(a.detail)}</li>`).join("")}</ol>`;
}

// ------------------------------------------------------------------ rack

export function renderRack(el, state, act, peek) {
  el.innerHTML = state.sources.map((s, i) => `
    <article class="card ${s.connected ? "on" : ""}" style="--c:${s.color}">
      <div class="stripe"></div>
      <div class="card-head"><span class="n">${i + 1}</span><b>${h(s.name)}</b></div>
      <div class="meta"><span class="fmt">${h(s.format)}</span> <span class="dim">${h(s.kind)}</span></div>
      <div class="stats">${s.connected
        ? `${s.latency_ms} ms · ${s.bytes} B · ${s.records} rec → ${s.fragments} frag`
        : s.error ? `<span class="err">${h(s.error)}</span>` : `<span class="dim">conf ${num(s.confidence)} · ${h(s.transport)}</span>`}</div>
      <div class="btns">
        <button data-src="${s.id}" class="${s.connected ? "" : "gold"}">${s.connected ? "disconnect" : "connect"}</button>
        <button data-peek="${s.id}">peek raw</button>
      </div>
    </article>`).join("");
  el.querySelectorAll("[data-src]").forEach((b) => b.addEventListener("click", () => {
    const s = state.sources.find((x) => x.id === b.dataset.src);
    b.disabled = true;
    b.textContent = s.connected ? "…" : "fetching…";
    act({ action: s.connected ? "disconnect" : "connect", source: s.id });
  }));
  el.querySelectorAll("[data-peek]").forEach((b) => b.addEventListener("click", () => peek(b.dataset.peek)));
}

export function renderOffboard(el, state, act, select) {
  const off = state.nodes.filter((n) => n.offboard)
    .sort((a, b) => (a.band === b.band ? 0 : a.band === "hypothesis" ? -1 : 1));
  const kept = state.nodes.filter((n) => n.kept);
  const item = (n, body) => `<li class="off-item${n.kept ? " kept-item" : ""}">
      <button class="sprite" data-select="${h(n.id)}" title="Inspect ${h(n.code)}"><canvas width="16" height="16" data-sprite="${h(n.id)}"></canvas></button>
      <div class="off-body">
        <button class="link code b-${n.band}" data-select="${h(n.id)}">${n.code} ${h(n.label)}</button>
        <span class="dim">${pct(n.p)}</span>
        ${body}
      </div>
    </li>`;
  el.innerHTML = `<h3>Off the board <span class="dim">${off.length}</span></h3>
    ${off.length ? "" : `<p class="dim small">Nothing has left the board. Explained-away hypotheses, and participants whose sources are disconnected, wait here with the reason, and can always come back.</p>`}
    <ul>${off.map((n) => item(n, `<div class="why">${h(n.offboard.reason)}</div><div class="btns">${restoreButton(n)}</div>`)).join("")}</ul>
    ${kept.length ? `<h3>Kept on the board</h3><ul>${kept.map((n) => item(n,
      `<div class="why">Your call: on the evidence alone it would be off the board.</div>
       <div class="btns"><button data-release="${h(n.id)}">let the board move it aside</button></div>`)).join("")}</ul>` : ""}`;
  el.querySelectorAll("canvas[data-sprite]").forEach((c) => {
    const n = state.nodes.find((x) => x.id === c.dataset.sprite);
    drawPiece(c.getContext("2d"), 0, 0, { color: COLORS[n.band], p: n.p, code: n.code, mode: pieceMode(n) });
    if (n.kept) drawPin(c.getContext("2d"), 0, 0);
  });
  wireRestore(el, act);
  el.querySelectorAll("[data-select]").forEach((b) => b.addEventListener("click", () => select(b.dataset.select)));
}

export async function showPeek(modal, state, sourceId) {
  const s = state.sources.find((x) => x.id === sourceId);
  const res = await fetch(`/mock/${state.scenario.id}/${sourceId}`);
  const text = await res.text();
  modal.querySelector(".modal-body").innerHTML = `
    <header style="--c:${s.color}"><span class="fmt">${h(s.format)}</span> <b>${h(s.name)}</b>
      <span class="dim">${h(res.headers.get("content-type") || "")} · ${text.length} bytes</span></header>
    <p>${h(s.blurb || "")}</p>
    <p class="dim small">This is exactly what the service sends: GET /mock/${h(state.scenario.id)}/${h(sourceId)}</p>
    <pre class="raw">${h(text)}</pre>`;
  modal.hidden = false;
}

export function toast(el, ev) {
  const t = document.createElement("div");
  t.className = `toast k-${ev.kind}`;
  t.innerHTML = `<div class="tt"><b>${h(ev.title)}</b>${ev.points ? `<span class="pts">+${ev.points}</span>` : ""}</div>${ev.text ? `<div>${h(ev.text)}</div>` : ""}`;
  el.appendChild(t);
  while (el.children.length > 3) el.firstElementChild.remove();
  setTimeout(() => t.classList.add("out"), 6500);
  setTimeout(() => t.remove(), 7200);
}

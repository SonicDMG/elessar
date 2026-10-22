// Topology mode: NetView-style map. Placeholder until the mode is built.
let root = null;
export function mount(el, ctx) {
  root = el;
  root.innerHTML = `<div class="mode-placeholder"><b>Topology</b><p class="dim">NetView-style map: not built yet.</p></div>`;
}
export function update(state) {}
export function unmount() { root = null; }

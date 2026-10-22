// Landscape mode: spikes on a surface. Placeholder until the mode is built.
let root = null;
export function mount(el, ctx) {
  root = el;
  root.innerHTML = `<div class="mode-placeholder"><b>Landscape</b><p class="dim">Spike map on a surface plot: not built yet.</p></div>`;
}
export function update(state) {}
export function unmount() { root = null; }

// Life mode: competing species. Placeholder until the mode is built.
let root = null;
export function mount(el, ctx) {
  root = el;
  root.innerHTML = `<div class="mode-placeholder"><b>Life</b><p class="dim">Competing species: not built yet.</p></div>`;
}
export function update(state) {}
export function unmount() { root = null; }

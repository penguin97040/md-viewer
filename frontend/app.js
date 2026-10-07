import { Viewer } from './viewer.js';
import { Toc } from './toc.js';
import { Find } from './find.js';
import { enhance, retheme } from './extras.js';

const go = window.go.main.App;
const rt = window.runtime;
const $ = (id) => document.getElementById(id);
const html = document.documentElement;
const isMac = navigator.platform.toUpperCase().includes('MAC');

const sc = $('scroller');
const docEl = $('doc');
const tocNav = $('toc');
const baseEl = $('base');

let settings = null;
let info = null;

// ---- Settings ----

const DEFAULTS = { theme: 'dark', wrap: true, fontSize: 17, lineHeight: 1.65, width: 'medium', font: 'sans', liveReload: true, toc: true };
const LIMITS = { fontSize: [12, 28], lineHeight: [1.2, 2.2] };

function applySettings(prev) {
  const s = settings;
  html.dataset.theme = s.theme;
  html.dataset.wrap = s.wrap ? 'on' : 'off';
  html.dataset.width = s.width;
  html.dataset.font = s.font;
  html.style.setProperty('--font-size', s.fontSize + 'px');
  html.style.setProperty('--line-height', String(s.lineHeight));
  tocNav.hidden = !s.toc || !info;
  $('btn-toc').classList.toggle('on', s.toc);
  syncSettingsUI();
  if (!prev) return;
  if (prev.theme !== s.theme) retheme(docEl, s.theme);
  if (['fontSize', 'lineHeight', 'width', 'font', 'wrap', 'toc'].some((k) => prev[k] !== s[k])) {
    viewer.relayout();
    toc.render();
  }
}

let saveTimer;
function update(patch) {
  const prev = settings;
  settings = { ...settings, ...patch };
  applySettings(prev);
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => go.SaveSettings(settings).catch(() => {}), 300);
}

function syncSettingsUI() {
  for (const seg of document.querySelectorAll('.seg')) {
    for (const b of seg.children) b.classList.toggle('on', b.dataset.v === settings[seg.dataset.key]);
  }
  for (const st of document.querySelectorAll('.step')) {
    const v = settings[st.dataset.key];
    st.querySelector('output').textContent = st.dataset.key === 'lineHeight' ? v.toFixed(2) : v;
  }
  for (const cb of document.querySelectorAll('#settings input[type=checkbox]')) cb.checked = settings[cb.dataset.key];
}

function stepSetting(key, dir) {
  const [lo, hi] = LIMITS[key];
  const step = key === 'lineHeight' ? 0.05 : 1;
  const v = Math.round((settings[key] + dir * step) * 100) / 100;
  update({ [key]: Math.min(hi, Math.max(lo, v)) });
}

document.querySelectorAll('.seg').forEach((seg) => seg.addEventListener('click', (e) => {
  const b = e.target.closest('button');
  if (b) update({ [seg.dataset.key]: b.dataset.v });
}));
document.querySelectorAll('.step').forEach((st) => st.addEventListener('click', (e) => {
  const b = e.target.closest('button');
  if (b) stepSetting(st.dataset.key, +b.dataset.d);
}));
document.querySelectorAll('#settings input[type=checkbox]').forEach((cb) =>
  cb.addEventListener('change', () => update({ [cb.dataset.key]: cb.checked })));
$('reset').addEventListener('click', () => update({ ...DEFAULTS, toc: settings.toc }));

// ---- Document ----

const viewer = new Viewer(sc, docEl, {
  fetchChunk: (id, i) => go.Chunk(id, i),
  afterMount: (el) => enhance(el, settings.theme),
});

const toc = new Toc(tocNav, $('toc-inner'), (h) => viewer.goTo(h.chunk, h.id));

const find = new Find({
  bar: $('findbar'), input: $('find-input'), count: $('find-count'),
  prev: $('find-prev'), next: $('find-next'), close: $('find-close'),
}, { viewer, search: (id, q) => go.Search(id, q), docId: () => info?.id });

let firstHeading = []; // chunk -> index of its first heading

async function show(next, { keep = false, fragment = '' } = {}) {
  if (!next) return;
  // Go sends compact arrays; expand them once here.
  next.headings = (next.headings || []).map(([level, text, id, chunk]) => ({ level, text, id, chunk }));
  next.chunks = next.chunks.map(([bytes, lines]) => ({ bytes, lines }));
  const anchor = keep && info && info.path === next.path ? viewer.anchor() : null;
  const sameFile = info && info.path === next.path;
  info = next;
  baseEl.href = next.base;
  document.body.classList.add('has-doc');
  $('title').textContent = next.name;
  $('title').title = next.path;
  if (!(await viewer.load(next, anchor))) return;

  firstHeading = new Array(next.chunks.length + 1).fill(next.headings.length);
  for (let k = next.headings.length - 1; k >= 0; k--) firstHeading[next.headings[k].chunk] = k;
  for (let c = next.chunks.length - 1; c >= 0; c--) firstHeading[c] = Math.min(firstHeading[c], firstHeading[c + 1]);

  toc.set(next.headings, sameFile && keep);
  tocNav.hidden = !settings.toc;
  if (fragment) jumpTo(fragment);
  updateCurrent();
  find.reset();
}

function showError(err) {
  toast(String(err?.message || err || 'Something went wrong'));
}

let toastTimer;
function toast(msg) {
  const t = $('toast');
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), 4000);
}

async function openDialog() {
  try {
    await show(await go.OpenDialog());
  } catch (e) {
    showError(e);
  }
}

async function reload() {
  if (!info) return;
  try {
    await show(await go.Reload(), { keep: true });
  } catch (e) {
    showError(e);
  }
}

// ---- Links ----

function jumpTo(id) {
  if (!info || !id) return;
  try { id = decodeURIComponent(id); } catch { /* keep as is */ }
  // Prefer an element already on screen (footnote ids can repeat across
  // sections of very large files), then the anchor map from Go.
  const el = document.getElementById(id);
  if (el && docEl.contains(el)) {
    sc.scrollTop = viewer.top(el) - 12;
    return;
  }
  let chunk = info.anchors[id];
  if (chunk === undefined) {
    const lower = id.toLowerCase();
    chunk = info.headings.find((h) => h.id === id || h.id === lower)?.chunk;
  }
  if (chunk !== undefined) viewer.goTo(chunk, id);
}

const MD_EXT = /\.(md|markdown|mdown|mkd|mkdn|mdx|txt)$/i;

docEl.addEventListener('click', async (e) => {
  const a = e.target.closest('a[href]');
  if (!a) return;
  e.preventDefault();
  const href = a.getAttribute('href');
  if (href.startsWith('#')) return jumpTo(href.slice(1));
  if (/^[a-z][a-z0-9+.-]*:/i.test(href)) return go.OpenURL(href);
  const [path, frag = ''] = href.split('#');
  if (MD_EXT.test(path.split('?')[0])) {
    try {
      await show(await go.OpenLink(path), { fragment: frag });
    } catch (err) {
      showError(err);
    }
  }
});

// ---- Current heading ----

let rafPending = false;
function updateCurrent() {
  rafPending = false;
  if (!info || !info.headings.length) return;
  const hs = info.headings;
  const y = sc.scrollTop + 80;
  const ci = viewer.chunkAt(y);
  let cur = firstHeading[ci] - 1;
  const chunk = viewer.chunks[ci];
  for (let k = firstHeading[ci]; k < hs.length && hs[k].chunk === ci; k++) {
    if (chunk.state !== 'mounted') break;
    const el = chunk.el.querySelector('#' + CSS.escape(hs[k].id));
    if (!el || viewer.top(el) > y) break;
    cur = k;
  }
  toc.setCurrent(cur);
}

sc.addEventListener('scroll', () => {
  if (!rafPending) {
    rafPending = true;
    requestAnimationFrame(updateCurrent);
  }
}, { passive: true });

// ---- UI wiring ----

const settingsPop = $('settings');
function toggleSettings(force) {
  settingsPop.hidden = force !== undefined ? !force : !settingsPop.hidden;
  $('btn-settings').classList.toggle('on', !settingsPop.hidden);
}

$('btn-open').addEventListener('click', openDialog);
$('empty-open').addEventListener('click', openDialog);
$('btn-reload').addEventListener('click', reload);
$('btn-toc').addEventListener('click', () => update({ toc: !settings.toc }));
$('btn-find').addEventListener('click', () => (find.isOpen ? find.close() : info && find.open()));
$('btn-settings').addEventListener('click', (e) => {
  e.stopPropagation();
  toggleSettings();
});
document.addEventListener('mousedown', (e) => {
  if (!settingsPop.hidden && !settingsPop.contains(e.target) && !$('btn-settings').contains(e.target)) toggleSettings(false);
});

document.addEventListener('keydown', (e) => {
  const mod = isMac ? e.metaKey : e.ctrlKey;
  const k = e.key.toLowerCase();
  if (e.key === 'F5' || (mod && k === 'r')) {
    e.preventDefault();
    reload();
  } else if (mod && k === 'o') {
    e.preventDefault();
    openDialog();
  } else if (mod && k === 'f') {
    e.preventDefault();
    if (info) find.open();
  } else if (e.key === 'F3') {
    e.preventDefault();
    if (find.isOpen) find.step(e.shiftKey ? -1 : 1);
    else if (info) find.open();
  } else if (mod && k === 'b') {
    e.preventDefault();
    update({ toc: !settings.toc });
  } else if (mod && (k === '=' || k === '+')) {
    e.preventDefault();
    stepSetting('fontSize', 1);
  } else if (mod && k === '-') {
    e.preventDefault();
    stepSetting('fontSize', -1);
  } else if (mod && k === '0') {
    e.preventDefault();
    update({ fontSize: DEFAULTS.fontSize });
  } else if (e.altKey && k === 'z') {
    e.preventDefault();
    update({ wrap: !settings.wrap });
  } else if (mod && e.shiftKey && k === 'd') {
    e.preventDefault();
    update({ theme: settings.theme === 'dark' ? 'light' : 'dark' });
  } else if (e.key === 'Escape') {
    if (!settingsPop.hidden) toggleSettings(false);
    else if (find.isOpen) find.close();
  } else if (mod && (k === 'p' || k === 'u' || k === 'g' || k === 'j')) {
    e.preventDefault(); // block browser print, view source, etc.
  }
});

// Ctrl + mouse wheel changes the font size rather than zooming the page.
window.addEventListener('wheel', (e) => {
  if (!(isMac ? e.metaKey : e.ctrlKey)) return;
  e.preventDefault();
  stepSetting('fontSize', e.deltaY < 0 ? 1 : -1);
}, { passive: false });

if (isMac) {
  for (const el of document.querySelectorAll('[title]')) el.title = el.title.replace('Ctrl+', '⌘');
  document.querySelector('#empty .hint').textContent = '⌘O';
}

// Keep the TOC list correct when the window resizes.
new ResizeObserver(() => toc.render()).observe(tocNav);

rt.EventsOn('doc:opened', (next) => show(next));
rt.EventsOn('doc:changed', (next) => {
  if (info && next.path === info.path) show(next, { keep: true });
});
rt.EventsOn('doc:error', showError);

// File drops. Registering here (not in Go) is what makes Wails listen for
// drops on Windows; false means the whole window accepts them.
rt.OnFileDrop(async (_x, _y, paths) => {
  if (!paths?.length) return;
  try {
    await show(await go.Open(paths[0]));
  } catch (e) {
    showError(e);
  }
}, false);

let dragDepth = 0;
const isFileDrag = (e) => e.dataTransfer?.types.includes('Files');
window.addEventListener('dragenter', (e) => {
  if (isFileDrag(e) && ++dragDepth === 1) document.body.classList.add('dragging');
});
window.addEventListener('dragleave', (e) => {
  if (isFileDrag(e) && --dragDepth <= 0) {
    dragDepth = 0;
    document.body.classList.remove('dragging');
  }
});
window.addEventListener('drop', () => {
  dragDepth = 0;
  document.body.classList.remove('dragging');
});

// ---- Start ----

(async () => {
  try {
    settings = { ...DEFAULTS, ...(await go.Settings()) };
  } catch {
    settings = { ...DEFAULTS };
  }
  applySettings(null);
  go.Version().then((v) => ($('version').textContent = 'MD Viewer ' + v));
  try {
    await show(await go.Initial());
  } catch (e) {
    showError(e);
  }
  sc.focus();
})();

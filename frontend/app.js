import { Tabs } from './tabs.js';
import { Toc } from './toc.js';
import { Find } from './find.js';
import { enhance, retheme, installInteractions, printDiagrams, clearPrintDiagrams, idle } from './extras.js';

const go = window.go.main.App;
const rt = window.runtime;
const $ = (id) => document.getElementById(id);
const html = document.documentElement;
const isMac = navigator.platform.toUpperCase().includes('MAC');

const tocNav = $('toc');
const baseEl = $('base');

let settings = null;

// ---- Settings ----

const DEFAULTS = { theme: 'dark', wrap: true, fontSize: 17, lineHeight: 1.65, width: 'medium', font: 'sans', liveReload: true, toc: true };
const LIMITS = { fontSize: [12, 28], lineHeight: [1.2, 2.2] };
const LAYOUT_KEYS = ['fontSize', 'lineHeight', 'width', 'font', 'wrap', 'toc'];

function applySettings(prev) {
  const s = settings;
  html.dataset.theme = s.theme;
  html.dataset.wrap = s.wrap ? 'on' : 'off';
  html.dataset.width = s.width;
  html.dataset.font = s.font;
  html.style.setProperty('--font-size', s.fontSize + 'px');
  html.style.setProperty('--line-height', String(s.lineHeight));
  tocNav.hidden = !s.toc || !tabs.active;
  $('btn-toc').classList.toggle('on', s.toc);
  syncSettingsUI();
  if (!prev) return;
  const tab = tabs.active;
  if (prev.theme !== s.theme && tab) {
    retheme(tab.docEl, s.theme);
    tab.theme = s.theme;
  }
  if (LAYOUT_KEYS.some((k) => prev[k] !== s[k])) {
    // Hidden tabs measure zero, so they catch up when shown.
    for (const t of tabs.list) if (t !== tab) t.dirty = true;
    tab?.viewer.relayout();
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

// ---- Tabs ----

let rafPending = false;
function updateCurrent() {
  rafPending = false;
  const tab = tabs.active;
  if (tab?.info?.headings) toc.setCurrent(tab.currentHeading());
}

const tabs = new Tabs($('tabs'), $('docs'), {
  fetchChunk: (id, i) => go.Chunk(id, i),
  afterMount: (el) => enhance(el, settings.theme),
  onScroll: (tab) => {
    if (tab === tabs.active && !rafPending) {
      rafPending = true;
      requestAnimationFrame(updateCurrent);
    }
  },
  onActivate: activated,
  onClose: (tab) => {
    const id = tab.pending?.id ?? tab.info?.id;
    if (id) go.Close(id);
  },
});

const toc = new Toc(tocNav, $('toc-inner'), (h) => tabs.active?.viewer.goTo(h.chunk, h.id));

const find = new Find({
  bar: $('findbar'), input: $('find-input'), count: $('find-count'),
  prev: $('find-prev'), next: $('find-next'), close: $('find-close'),
}, { viewer: () => tabs.active?.viewer ?? null, search: (id, q) => go.Search(id, q) });

let shown = null; // the tab the TOC and title currently describe

// activated runs when a tab is shown (or loaded), or with null when the last
// tab closes.
async function activated(tab) {
  if (shown && shown !== tab) shown.tocScroll = tocNav.scrollTop;
  shown = tab;
  if (!tab) {
    document.body.classList.remove('has-doc');
    tocNav.hidden = true;
    go.SetTitle('');
    if (find.isOpen) find.close();
    return;
  }
  if (!tab.info?.chunks) return; // still loading; called again when loaded
  document.body.classList.add('has-doc');
  baseEl.href = tab.info.base;
  go.SetTitle(tab.info.name);

  if (tab.pending) {
    const next = tab.pending;
    tab.pending = null;
    await tab.load(next, true);
    if (tabs.active !== tab) return;
  }
  if (tab.dirty) {
    tab.dirty = false;
    tab.viewer.relayout();
  }
  if (tab.anchor) {
    tab.viewer.restore(tab.anchor);
    tab.anchor = null;
  }
  if (tab.theme && tab.theme !== settings.theme) retheme(tab.docEl, settings.theme);
  tab.theme = settings.theme;

  toc.set(tab.info.headings);
  tocNav.hidden = !settings.toc;
  tocNav.scrollTop = tab.tocScroll;
  toc.render();
  updateCurrent();
  find.reset();
  tab.scroller.focus({ preventScroll: true });
}

// openPath shows a file, switching to its tab if it is already open.
async function openPath(path, fragment = '') {
  const existing = tabs.byPath(path);
  if (existing) {
    tabs.activate(existing);
    if (fragment) existing.jumpTo(fragment);
    return;
  }
  try {
    await tabs.add(await go.Open(path), fragment);
  } catch (e) {
    showError(e);
  }
}

async function openMany(paths) {
  for (const p of paths || []) await openPath(p);
}

async function openDialog() {
  try {
    const p = await go.PickFile(tabs.active?.info?.id ?? 0);
    if (p) await openPath(p);
  } catch (e) {
    showError(e);
  }
}

// refresh redraws the TOC and find state after the active tab reloads.
function refresh(tab) {
  if (tab !== tabs.active) return;
  toc.set(tab.info.headings, true);
  updateCurrent();
  find.reset();
}

async function reload() {
  const tab = tabs.active;
  if (!tab?.info?.id) return;
  const id = tab.info.id;
  try {
    const info = await go.Reload(id);
    if (tab.info.id !== id) return; // a live reload got there first
    await tab.load(info, true);
    refresh(tab);
  } catch (e) {
    if (tab.info.id === id) showError(e);
  }
}

rt.EventsOn('doc:changed', async (old, info) => {
  const tab = tabs.list.find((t) => (t.pending?.id ?? t.info?.id) === old);
  if (!tab) return;
  if (tab !== tabs.active) {
    tab.pending = info; // loaded when the tab is next shown
    return;
  }
  await tab.load(info, true);
  refresh(tab);
});
rt.EventsOn('open:paths', openMany);
rt.EventsOn('doc:error', showError);

// ---- Messages ----

function showError(err) {
  toast(String(err?.message || err || 'Something went wrong'));
}

let toastTimer;
// toast shows a short message. With sticky it stays until replaced or
// hidden; action adds a button.
function toast(msg, { sticky = false, action = null } = {}) {
  const t = $('toast');
  t.textContent = '';
  const span = document.createElement('span');
  span.textContent = msg;
  t.append(span);
  if (action) {
    const b = document.createElement('button');
    b.className = 'link';
    b.textContent = action.label;
    b.addEventListener('click', action.fn);
    t.append(' ', b);
  }
  t.hidden = false;
  clearTimeout(toastTimer);
  if (!sticky) toastTimer = setTimeout(hideToast, 4000);
}

// toastText changes the message of the current toast, leaving its button
// in place so it stays clickable.
function toastText(msg) {
  const span = $('toast').querySelector('span');
  if (span) span.textContent = msg;
}

function hideToast() {
  clearTimeout(toastTimer);
  $('toast').hidden = true;
}

// ---- Links ----

const MD_EXT = /\.(md|markdown|mdown|mkd|mkdn|mdx|txt)$/i;

$('docs').addEventListener('click', async (e) => {
  const a = e.target.closest('.md a[href]');
  const tab = tabs.active;
  if (!a || !tab) return;
  e.preventDefault();
  const href = a.getAttribute('href');
  if (href.startsWith('#')) return tab.jumpTo(href.slice(1));
  if (/^[a-z][a-z0-9+.-]*:/i.test(href)) return go.OpenURL(href);
  const [path, frag = ''] = href.split('#');
  if (!MD_EXT.test(path.split('?')[0])) return;
  try {
    await openPath(await go.ResolveLink(tab.info.id, path), frag);
  } catch (err) {
    showError(err);
  }
});

installInteractions(async (text) => {
  if (!(await rt.ClipboardSetText(text))) throw new Error('clipboard unavailable');
});

// ---- Save as PDF / print ----

let printing = null; // the tab being printed

async function savePdf() {
  const tab = tabs.active;
  if (!tab?.info?.chunks || printing) return;
  printing = tab;
  let cancelled = false;
  const cancel = { label: 'Cancel', fn: () => { cancelled = true; } };
  try {
    toast('Preparing PDF…', { sticky: true, action: cancel });
    const complete = await tab.viewer.mountAll((p) => {
      toastText(`Preparing PDF… ${Math.round(p * 100)}%`);
      return !cancelled;
    });
    if (!complete || cancelled) throw new Error('cancelled');
    // Print in light colours: diagrams get light copies shown only in print.
    if (settings.theme === 'dark') await printDiagrams(tab.docEl);
    await idle();
    await Promise.all([...tab.docEl.querySelectorAll('img')].map((img) => img.decode().catch(() => {})));
    hideToast();
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    window.addEventListener('afterprint', finishPrint, { once: true });
    if (isMac) {
      // The macOS print sheet doesn't fire afterprint; tidy up on the next
      // input once it has closed.
      setTimeout(() => {
        for (const ev of ['pointermove', 'keydown']) window.addEventListener(ev, finishPrint, { once: true });
      }, 1000);
    }
    await go.Print();
  } catch (e) {
    if (e?.message !== 'cancelled') showError(e);
    else hideToast();
    finishPrint();
  }
}

function finishPrint() {
  const tab = printing;
  if (!tab) return;
  printing = null;
  clearPrintDiagrams(tab.docEl);
  tab.viewer.resumeUnmount();
}

// ---- UI wiring ----

const settingsPop = $('settings');
function toggleSettings(force) {
  settingsPop.hidden = force !== undefined ? !force : !settingsPop.hidden;
  $('btn-settings').classList.toggle('on', !settingsPop.hidden);
}

$('btn-open').addEventListener('click', openDialog);
$('empty-open').addEventListener('click', openDialog);
$('btn-reload').addEventListener('click', reload);
$('btn-print').addEventListener('click', savePdf);
$('btn-toc').addEventListener('click', () => update({ toc: !settings.toc }));
$('btn-find').addEventListener('click', () => (find.isOpen ? find.close() : tabs.active && find.open()));
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
  let handled = true;
  if (e.key === 'F5' || (mod && k === 'r')) reload();
  else if (mod && k === 'o') openDialog();
  else if (mod && k === 'w') tabs.close(tabs.active);
  else if (e.ctrlKey && e.key === 'Tab') tabs.next(e.shiftKey ? -1 : 1);
  else if (mod && e.key === 'PageDown') tabs.next(1);
  else if (mod && e.key === 'PageUp') tabs.next(-1);
  else if (mod && !e.shiftKey && /^[1-9]$/.test(e.key)) tabs.select(e.key === '9' ? -1 : +e.key - 1);
  else if (mod && k === 'p') savePdf();
  else if (mod && k === 'f') { if (tabs.active) find.open(); }
  else if (e.key === 'F3') {
    if (find.isOpen) find.step(e.shiftKey ? -1 : 1);
    else if (tabs.active) find.open();
  } else if (mod && k === 'b') update({ toc: !settings.toc });
  else if (mod && (k === '=' || k === '+')) stepSetting('fontSize', 1);
  else if (mod && k === '-') stepSetting('fontSize', -1);
  else if (mod && k === '0') update({ fontSize: DEFAULTS.fontSize });
  else if (e.altKey && k === 'z') update({ wrap: !settings.wrap });
  else if (mod && e.shiftKey && k === 'd') update({ theme: settings.theme === 'dark' ? 'light' : 'dark' });
  else if (mod && (k === 'u' || k === 'g' || k === 'j')) { /* block view source, browser find-next, etc. */ }
  else {
    handled = false;
    if (e.key === 'Escape') {
      if (!settingsPop.hidden) toggleSettings(false);
      else if (find.isOpen) find.close();
    }
  }
  if (handled) e.preventDefault();
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

// File drops. Registering here (not in Go) is what makes Wails listen for
// drops on Windows; false means the whole window accepts them.
rt.OnFileDrop((_x, _y, paths) => openMany(paths), false);

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
    await openMany(await go.Initial());
  } catch (e) {
    showError(e);
  }
})();

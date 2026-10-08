import { Tabs, pathKey } from './tabs.js';
import { Toc } from './toc.js';
import { Find } from './find.js';
import { enhance, retheme, installInteractions, printDiagrams, clearPrintDiagrams, idle } from './extras.js';

const go = window.go.main.App;
const rt = window.runtime;
// The app's own elements, found once before any document is shown: a
// document may use the same ids, and getElementById returns the first.
const shell = new Map([...document.querySelectorAll('[id]')].map((el) => [el.id, el]));
const $ = (id) => shell.get(id);
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
let saves = Promise.resolve();
function update(patch) {
  const prev = settings;
  settings = { ...settings, ...patch };
  applySettings(prev);
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    const snapshot = { ...settings };
    saves = saves.then(() => go.SaveSettings(snapshot)).catch(showError);
  }, 300);
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
  afterMount: (el, tab) => {
    webImages(tab, el);
    enhance(el, settings.theme);
  },
  onScroll: (tab) => {
    if (tab === tabs.active && !rafPending) {
      rafPending = true;
      requestAnimationFrame(updateCurrent);
    }
  },
  onActivate: activated,
  onClose: (tab) => {
    if (tab.key) go.Close(tab.key);
  },
});

const toc = new Toc(tocNav, $('toc-inner'), (h) => tabs.active?.viewer.goTo(h.chunk, h.id));

const find = new Find({
  bar: $('findbar'), input: $('find-input'), count: $('find-count'),
  prev: $('find-prev'), next: $('find-next'), close: $('find-close'),
}, { viewer: () => tabs.active?.viewer ?? null, search: (id, q) => go.Search(id, q) });

let shown = null; // the tab the TOC and title currently describe
let activationGen = 0;

// activated runs when a tab is shown (or loaded), or with null when the last
// tab closes.
async function activated(tab) {
  const gen = ++activationGen;
  find.invalidate();
  if (shown && shown !== tab) shown.tocScroll = tocNav.scrollTop;
  shown = tab;
  updateWebBar();
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
    const loaded = await tab.load(next, true);
    if (tabs.active !== tab || gen !== activationGen) return;
    if (!loaded) {
      if (tab.pending) return activated(tab);
      return;
    }
  }
  if (tab.viewer.info !== tab.info) return;
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
const opening = new Map();
async function openPath(path, fragment = '') {
  const key = pathKey(path);
  const pending = opening.get(key);
  if (pending) {
    const tab = await pending;
    if (tab && tabs.list.includes(tab)) {
      tabs.activate(tab);
      if (fragment) tab.jumpTo(fragment);
    }
    return;
  }
  const existing = tabs.byPath(path);
  if (existing) {
    tabs.activate(existing);
    if (fragment) existing.jumpTo(fragment);
    return;
  }
  const operation = (async () => {
    try {
      const info = await go.Open(path);
      // Go normalises relative paths. Another open may have created a tab
      // for that path while this request was parsing.
      const tab = tabs.byPath(info.path);
      if (tab) {
        if (info.key !== tab.key) await go.Close(info.key);
        if (tabs.list.includes(tab)) {
          tabs.activate(tab);
          if (fragment) tab.jumpTo(fragment);
          return tab;
        }
        return null;
      }
      return await tabs.add(info, fragment);
    } catch (e) {
      showError(e);
      return null;
    }
  })();
  opening.set(key, operation);
  try {
    return await operation;
  } finally {
    if (opening.get(key) === operation) opening.delete(key);
  }
}

async function openMany(paths) {
  for (const p of paths || []) await openPath(p);
}

async function openDialog() {
  try {
    const p = await go.PickFile(tabs.active?.key ?? 0);
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

// showParse shows a newer parse of a tab's file: now if the tab is showing,
// otherwise when it is next shown. Ids only grow, so an older parse that
// arrives late (a reload overtaken by a live reload) is ignored.
async function showParse(tab, info) {
  if (!tabs.list.includes(tab) || info.id <= tab.newestId) return;
  if (tab !== tabs.active) {
    tab.pending = info;
    return;
  }
  find.invalidate();
  if (await tab.load(info, true)) refresh(tab);
  else if (tab === tabs.active && tab.pending) activated(tab);
}

async function reload() {
  const tab = tabs.active;
  if (!tab?.key) return;
  try {
    await showParse(tab, await go.Reload(tab.key));
  } catch (e) {
    if (tabs.list.includes(tab)) showError(e);
  }
}

// Live reloads. A tab closed meanwhile has already closed its document in
// Go, whatever the parse, so there is nothing to tidy up here.
rt.EventsOn('doc:changed', (info) => {
  const tab = tabs.byKey(info.key);
  if (tab) showParse(tab, info);
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

// ---- Web images ----

// Go holds back images on the web (data-web-src) because loading one tells
// its site that the file was opened, and from where. They load, through Go,
// once the reader agrees for that tab.
const isWeb = (u) => /^(https?:|[\\/]{2})/i.test(u.replace(/[\t\n\r]/g, '').trimStart());

function webImages(tab, root) {
  const els = root.querySelectorAll('[data-web-src], [data-web-srcset]');
  if (!els.length) return;
  if (!tab.webImages) {
    tab.hasWebImages = true;
    if (tab === tabs.active) updateWebBar();
    return;
  }
  const via = (u) => (isWeb(u) ? tab.info.web + '?u=' + encodeURIComponent(u) : u);
  for (const el of els) {
    const { webSrc, webSrcset } = el.dataset;
    delete el.dataset.webSrc;
    delete el.dataset.webSrcset;
    if (webSrcset) {
      el.srcset = webSrcset.split(',').map((c) => {
        const [u, ...rest] = c.trim().split(/\s+/);
        return [via(u), ...rest].join(' ');
      }).join(', ');
    }
    if (webSrc) el.src = via(webSrc);
  }
}

function updateWebBar() {
  const tab = tabs.active;
  $('webbar').hidden = !(tab?.hasWebImages && !tab.webImages && !tab.webDismissed);
}

$('web-load').addEventListener('click', () => {
  const tab = tabs.active;
  if (!tab) return;
  tab.webImages = true;
  webImages(tab, tab.docEl);
  updateWebBar();
});
$('web-close').addEventListener('click', () => {
  if (tabs.active) tabs.active.webDismissed = true;
  updateWebBar();
});

// ---- Links ----

const MD_EXT = /\.(md|markdown|mdown|mkd|mkdn|mdx|txt)$/i;

// Every link is handled here: the web view itself never navigates.
async function followLink(e) {
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
    await openPath(await go.ResolveLink(tab.key, path), frag);
  } catch (err) {
    showError(err);
  }
}

$('docs').addEventListener('click', followLink);
// A middle click would otherwise open the link in a new web view window.
$('docs').addEventListener('auxclick', (e) => {
  if (e.button === 1) followLink(e);
});
// Links and images dragged out of a document would carry their addresses
// to wherever they are dropped, including back into the web view.
$('docs').addEventListener('dragstart', (e) => {
  if (e.target.closest?.('.md a, .md img')) e.preventDefault();
});

installInteractions(async (text) => {
  if (!(await rt.ClipboardSetText(text))) throw new Error('clipboard unavailable');
});

// ---- Save as PDF / print ----

let printing = null; // { tab, done } while a PDF is prepared or printed

async function savePdf() {
  const tab = tabs.active;
  if (!tab?.info?.chunks || printing) return;
  // Aborting done removes whichever end-of-print listeners are left, so
  // none can fire during a later print.
  const done = new AbortController();
  printing = { tab, done };
  const gen = tab.viewer.gen;
  const activity = tab.viewer.activity;
  let cancelled = false;
  const cancel = { label: 'Cancel', fn: () => { cancelled = true; } };
  // check stops if the PDF can no longer come from what this tab shows:
  // the print CSS prints only the visible tab.
  const check = () => {
    if (cancelled) throw new Error('cancelled');
    if (tabs.active !== tab) throw new Error('Save as PDF stopped because the tab changed');
    if (tab.viewer.activity !== activity) throw new Error('Save as PDF stopped because the tab changed');
    if (tab.viewer.gen !== gen) throw new Error('Save as PDF stopped because the file changed');
  };
  try {
    toast('Preparing PDF…', { sticky: true, action: cancel });
    const complete = await tab.viewer.mountAll((p) => {
      toastText(`Preparing PDF… ${Math.round(p * 100)}%`);
      return !cancelled;
    });
    check();
    if (!complete) throw new Error('Save as PDF stopped because part of the file could not be loaded');
    // Print in light colours: diagrams get light copies shown only in print.
    if (settings.theme === 'dark') await printDiagrams(tab.docEl);
    await idle();
    await Promise.all([...tab.docEl.querySelectorAll('img')].map((img) => img.decode().catch(() => {})));
    check();
    hideToast();
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    check();
    window.addEventListener('afterprint', finishPrint, { once: true, signal: done.signal });
    if (isMac) {
      // The macOS print sheet doesn't fire afterprint; tidy up on the next
      // input once it has closed.
      setTimeout(() => {
        for (const ev of ['pointermove', 'keydown']) window.addEventListener(ev, finishPrint, { once: true, signal: done.signal });
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
  if (!printing) return;
  const { tab, done } = printing;
  printing = null;
  done.abort();
  clearPrintDiagrams(tab.docEl);
  tab.viewer.resumeUnmount();
}

// ---- UI wiring ----

const settingsPop = $('settings');
function toggleSettings(force) {
  settingsPop.hidden = force !== undefined ? !force : !settingsPop.hidden;
  $('btn-settings').classList.toggle('on', !settingsPop.hidden);
}

// Toolbar buttons don't take focus when clicked, so keyboard shortcuts used
// afterwards don't leave a focus ring on them.
for (const b of document.querySelectorAll('#bar .icon')) b.addEventListener('mousedown', (e) => e.preventDefault());

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

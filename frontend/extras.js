// Diagrams (mermaid) and maths (KaTeX). Each library is loaded only when a
// document first needs it.

const loaded = new Map();

function loadScript(src) {
  if (!loaded.has(src)) {
    loaded.set(src, new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = src;
      s.onload = resolve;
      s.onerror = () => { loaded.delete(src); reject(new Error('could not load ' + src)); };
      document.head.append(s);
    }));
  }
  return loaded.get(src);
}

function loadStyle(href) {
  if (!loaded.has(href)) {
    const l = document.createElement('link');
    l.rel = 'stylesheet';
    l.href = href;
    document.head.append(l);
    loaded.set(href, Promise.resolve());
  }
}

// ---- Maths ----

const mathCache = new Map();
let mathDone = Promise.resolve();

function renderMath(root) {
  mathDone = mathDone.then(() => renderMathNow(root));
}

async function renderMathNow(root) {
  const els = root.querySelectorAll('.math:not([data-done])');
  if (!els.length) return;
  loadStyle('/vendor/katex/katex.min.css');
  try {
    await loadScript('/vendor/katex/katex.min.js');
  } catch {
    return;
  }
  for (const el of els) {
    if (!el.isConnected) continue;
    const display = el.classList.contains('display');
    const src = el.textContent;
    const key = (display ? 'D' : 'I') + src;
    let html = mathCache.get(key);
    if (html === undefined) {
      html = window.katex.renderToString(src, { displayMode: display, throwOnError: false, output: 'htmlAndMathml' });
      if (mathCache.size > 2000) mathCache.clear();
      mathCache.set(key, html);
    }
    el.dataset.done = '1';
    el.dataset.src = src;
    el.innerHTML = html;
  }
}

// ---- Diagrams ----

let mermaidTheme = null;
let queue = Promise.resolve();
let seq = 0;
const svgCache = new Map();

async function mermaidReady(theme) {
  await loadScript('/vendor/mermaid.min.js');
  if (mermaidTheme !== theme) {
    window.mermaid.initialize({
      startOnLoad: false,
      securityLevel: 'strict',
      theme: theme === 'dark' ? 'dark' : 'default',
      fontFamily: getComputedStyle(document.body).fontFamily,
    });
    mermaidTheme = theme;
  }
}

// renderDiagrams turns <pre class="mermaid"> blocks into diagrams. Renders run
// one at a time because mermaid is not safe to call concurrently.
function renderDiagrams(root, theme) {
  const els = [...root.querySelectorAll('pre.mermaid')];
  if (!els.length) return;
  queue = queue.then(async () => {
    try {
      await mermaidReady(theme);
    } catch {
      return;
    }
    for (const pre of els) {
      if (!pre.isConnected) continue;
      const src = pre.textContent;
      const key = theme + '\n' + src;
      const out = document.createElement('div');
      out.className = 'diagram';
      out.dataset.src = src;
      let svg = svgCache.get(key);
      if (svg === undefined) {
        try {
          svg = (await window.mermaid.render('mmd-' + (++seq), src)).svg;
        } catch (e) {
          svg = null;
          out.classList.add('diagram-error');
          out.textContent = 'Diagram error: ' + (e?.message || e);
        }
        if (svg) {
          if (svgCache.size > 300) svgCache.clear();
          svgCache.set(key, svg);
        }
      }
      if (svg) out.innerHTML = svg;
      if (pre.isConnected) pre.replaceWith(out);
    }
  });
}

// ---- Printing ----

// printDiagrams adds light-theme copies of diagrams, shown only in print,
// so a dark-theme document prints on white paper without the screen
// flashing to light.
export function printDiagrams(root) {
  const els = [...root.querySelectorAll('.diagram:not(.has-print):not(.diagram-print)')];
  if (!els.length) return queue;
  queue = queue.then(async () => {
    try {
      await mermaidReady('light');
    } catch {
      return;
    }
    for (const d of els) {
      if (!d.isConnected || d.classList.contains('diagram-error')) continue;
      const key = 'light\n' + d.dataset.src;
      let svg = svgCache.get(key);
      if (svg === undefined) {
        try {
          svg = (await window.mermaid.render('mmd-' + (++seq), d.dataset.src)).svg;
          svgCache.set(key, svg);
        } catch {
          continue;
        }
      }
      const copy = document.createElement('div');
      copy.className = 'diagram diagram-print';
      copy.innerHTML = svg;
      d.classList.add('has-print');
      d.after(copy);
    }
  });
  return queue;
}

export function clearPrintDiagrams(root) {
  for (const d of root.querySelectorAll('.diagram-print')) d.remove();
  for (const d of root.querySelectorAll('.has-print')) d.classList.remove('has-print');
}

// idle resolves once queued maths and diagrams have been drawn.
export function idle() {
  return Promise.all([queue, mathDone]);
}

// ---- Code copy and table sort ----

const COPY_ICON = '<svg viewBox="0 0 24 24"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/></svg>';
const DONE_ICON = '<svg viewBox="0 0 24 24"><path d="m5 12.5 4.5 4.5L19 7.5"/></svg>';

function addCopyButtons(root) {
  for (const block of root.querySelectorAll('.code')) {
    if (block.querySelector(':scope > .copy')) continue;
    const b = document.createElement('button');
    b.className = 'copy';
    b.title = 'Copy';
    b.setAttribute('aria-label', 'Copy code');
    b.innerHTML = COPY_ICON;
    block.append(b);
  }
}

function markSortable(root) {
  for (const th of root.querySelectorAll('table > thead > tr > th')) {
    th.classList.add('sortable');
  }
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });

function asNumber(s) {
  const t = s.replace(/[\s,$€£%]/g, '');
  return /^[-+]?(\d+\.?\d*|\.\d+)(e[-+]?\d+)?$/i.test(t) ? parseFloat(t) : null;
}

function compareCells(a, b) {
  if (a === b) return 0;
  if (a === '') return 1; // empty cells last
  if (b === '') return -1;
  const na = asNumber(a);
  const nb = asNumber(b);
  if (na !== null && nb !== null) return na - nb;
  return collator.compare(a, b);
}

// sortBy cycles a column through ascending, descending and original order.
function sortBy(th) {
  const table = th.closest('table');
  const body = table?.tBodies[0];
  if (!body) return;
  if (!table._order) table._order = [...body.rows];
  const next = { none: 'ascending', ascending: 'descending', descending: 'none' }[th.getAttribute('aria-sort') || 'none'];
  for (const h of th.parentElement.cells) h.removeAttribute('aria-sort');
  let rows = table._order;
  if (next !== 'none') {
    th.setAttribute('aria-sort', next);
    const col = th.cellIndex;
    const key = (r) => r.cells[col]?.textContent.trim() ?? '';
    const keyed = rows.map((r) => [key(r), r]);
    keyed.sort((x, y) => {
      const c = compareCells(x[0], y[0]);
      if (x[0] === '' || y[0] === '') return c; // keep empties last either way
      return next === 'ascending' ? c : -c;
    });
    rows = keyed.map((x) => x[1]);
  }
  const frag = document.createDocumentFragment();
  for (const r of rows) frag.append(r);
  body.append(frag);
}

// installInteractions wires the copy buttons and sortable headers for the
// whole page.
export function installInteractions(copyText) {
  document.addEventListener('click', async (e) => {
    const btn = e.target.closest('.md .copy');
    if (btn) {
      e.preventDefault();
      const code = btn.parentElement.querySelector('pre')?.textContent ?? '';
      try {
        await copyText(code.replace(/\n$/, ''));
        btn.innerHTML = DONE_ICON;
        btn.classList.add('done');
        clearTimeout(btn._t);
        btn._t = setTimeout(() => {
          btn.innerHTML = COPY_ICON;
          btn.classList.remove('done');
        }, 1200);
      } catch { /* clipboard unavailable */ }
      return;
    }
    const th = e.target.closest('.md th.sortable');
    if (th && !e.target.closest('a')) sortBy(th);
  });
}

// enhance runs after a chunk is mounted.
export function enhance(root, theme) {
  addCopyButtons(root);
  markSortable(root);
  renderMath(root);
  renderDiagrams(root, theme);
}

// retheme redraws diagrams already on screen in the new theme.
export function retheme(root, theme) {
  clearPrintDiagrams(root);
  for (const d of root.querySelectorAll('.diagram')) {
    const pre = document.createElement('pre');
    pre.className = 'mermaid';
    pre.textContent = d.dataset.src;
    d.replaceWith(pre);
  }
  renderDiagrams(root, theme);
}

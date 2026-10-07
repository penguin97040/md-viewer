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

async function renderMath(root) {
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

// enhance runs after a chunk is mounted.
export function enhance(root, theme) {
  renderMath(root);
  renderDiagrams(root, theme);
}

// retheme redraws diagrams already on screen in the new theme.
export function retheme(root, theme) {
  for (const d of root.querySelectorAll('.diagram')) {
    const pre = document.createElement('pre');
    pre.className = 'mermaid';
    pre.textContent = d.dataset.src;
    d.replaceWith(pre);
  }
  renderDiagrams(root, theme);
}

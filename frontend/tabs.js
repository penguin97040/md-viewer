// Tabs. Each tab owns a scroller, an article and a Viewer, so switching back
// to a tab is instant and keeps what was already rendered.

import { Viewer } from './viewer.js';

// Windows and macOS file systems usually ignore case in names; Linux ones
// don't.
const FOLD_CASE = /^(win|mac)/i.test(navigator.platform);
const fold = (s) => (FOLD_CASE ? s.toLowerCase() : s);

export class Tab {
  constructor(host, { fetchChunk, afterMount, onScroll }) {
    this.key = 0;           // Go's key for this tab's document
    this.info = null;
    this.pending = null;    // a newer parse to load when the tab is next shown
    this.firstHeading = []; // chunk -> index of its first heading
    this.anchor = null;     // reading position while hidden
    this.tocScroll = 0;
    this.dirty = false;     // typography changed while hidden
    this.theme = null;      // theme its diagrams were drawn in
    this.hasWebImages = false; // its document has images on the web
    this.webImages = false;    // the reader agreed to load them
    this.webDismissed = false; // the reader closed the bar that asks

    this.scroller = document.createElement('div');
    this.scroller.className = 'scroller';
    this.scroller.tabIndex = -1;
    this.scroller.hidden = true;
    this.docEl = document.createElement('article');
    this.docEl.className = 'md';
    this.scroller.append(this.docEl);
    host.append(this.scroller);

    this.viewer = new Viewer(this.scroller, this.docEl, { fetchChunk, afterMount: (el) => afterMount(el, this) });
    this.viewer.setActive(false);
    this.scroller.addEventListener('scroll', () => onScroll(this), { passive: true });
  }

  // newestId is the id of the latest parse this tab has, shown or pending.
  get newestId() {
    return this.pending?.id ?? this.info?.id ?? 0;
  }

  // load shows a parse of this tab's file, as Go sent it. keep holds the
  // reading position (reloads). If the tab is hidden before the parse can
  // be shown, it waits in pending until the tab is shown again.
  async load(info, keep = false) {
    // Go sends compact arrays; expand them into a copy, leaving info as it
    // was in case it has to wait in pending.
    const doc = {
      ...info,
      headings: (info.headings || []).map(([level, text, id, chunk]) => ({ level, text, id, chunk })),
      chunks: info.chunks.map(([bytes, lines]) => ({ bytes, lines })),
    };
    // Keep the reading position: the one saved when the tab was hidden, or
    // the live one.
    let anchor = null;
    if (keep) anchor = this.anchor ?? (this.info?.chunks ? this.viewer.anchor() : null);
    this.anchor = null;
    this.info = doc;
    // Relative images resolve against <base>, so point it at this file's
    // folder before anything is mounted.
    if (!this.scroller.hidden) document.getElementById('base').href = doc.base;
    if (!(await this.viewer.load(doc, anchor))) {
      // Hidden meanwhile: hidden elements measure zero and <base> belongs
      // to another tab, so wait until this one is shown.
      if (this.info === doc && this.viewer.paused) {
        this.pending = info;
        this.anchor ??= anchor;
      }
      return false;
    }
    const n = doc.chunks.length;
    const fh = new Array(n + 1).fill(doc.headings.length);
    for (let k = doc.headings.length - 1; k >= 0; k--) fh[doc.headings[k].chunk] = k;
    for (let c = n - 1; c >= 0; c--) fh[c] = Math.min(fh[c], fh[c + 1]);
    this.firstHeading = fh;
    return true;
  }

  // jumpTo scrolls to an element id: one already on screen first (footnote
  // ids can repeat across sections of very large files), then Go's maps.
  jumpTo(id) {
    if (!this.info || !id) return;
    try { id = decodeURIComponent(id); } catch { /* keep as is */ }
    const el = this.docEl.querySelector('#' + CSS.escape(id));
    if (el) {
      this.scroller.scrollTop = this.viewer.top(el) - 12;
      return;
    }
    let chunk = this.info.anchors[id];
    if (chunk === undefined) {
      const lower = id.toLowerCase();
      chunk = this.info.headings.find((h) => h.id === id || h.id === lower)?.chunk;
    }
    if (chunk !== undefined) this.viewer.goTo(chunk, id);
  }

  // currentHeading returns the index of the heading being read.
  currentHeading() {
    const hs = this.info?.headings;
    if (!hs?.length) return -1;
    const v = this.viewer;
    const y = this.scroller.scrollTop + 80;
    const ci = v.chunkAt(y);
    let cur = this.firstHeading[ci] - 1;
    const chunk = v.chunks[ci];
    for (let k = this.firstHeading[ci]; k < hs.length && hs[k].chunk === ci; k++) {
      if (chunk.state !== 'mounted') break;
      const el = chunk.el.querySelector('#' + CSS.escape(hs[k].id));
      if (!el || v.top(el) > y) break;
      cur = k;
    }
    return cur;
  }

  show() {
    this.scroller.hidden = false;
    this.viewer.setActive(true);
  }

  hide() {
    if (this.info) this.anchor = this.viewer.anchor();
    this.viewer.setActive(false);
    this.scroller.hidden = true;
  }

  destroy() {
    this.viewer.clear();
    this.scroller.remove();
  }
}

export class Tabs {
  constructor(strip, host, { fetchChunk, afterMount, onScroll, onActivate, onClose }) {
    this.strip = strip;
    this.host = host;
    this.opts = { fetchChunk, afterMount, onScroll };
    this.onActivate = onActivate;
    this.onClose = onClose;
    this.list = [];
    this.active = null;

    strip.addEventListener('click', (e) => {
      const el = e.target.closest('.tab');
      if (!el) return;
      const tab = this.list.find((t) => t.el === el);
      if (e.target.closest('.x')) this.close(tab);
      else this.activate(tab);
    });
    strip.addEventListener('auxclick', (e) => {
      const el = e.target.closest('.tab');
      if (el && e.button === 1) this.close(this.list.find((t) => t.el === el));
    });
    strip.addEventListener('mousedown', (e) => {
      if (e.button === 1) e.preventDefault(); // no autoscroll on middle click
    });
    strip.addEventListener('wheel', (e) => {
      if (e.deltaY && !e.ctrlKey && !e.metaKey) {
        strip.scrollLeft += e.deltaY;
        e.preventDefault();
      }
    }, { passive: false });
  }

  byPath(path) {
    const p = fold(path);
    return this.list.find((t) => t.info && fold(t.info.path) === p);
  }

  byKey(key) {
    return this.list.find((t) => t.key === key);
  }

  // add makes a tab for a freshly opened document and shows it.
  async add(info, fragment = '') {
    const tab = new Tab(this.host, this.opts);
    tab.el = document.createElement('div');
    tab.el.className = 'tab';
    tab.el.setAttribute('role', 'tab');
    tab.el.innerHTML = '<span class="name"></span><button class="x" tabindex="-1" aria-label="Close tab">' +
      '<svg viewBox="0 0 24 24"><path d="M7 7l10 10M17 7 7 17"/></svg></button>';
    const at = this.active ? this.list.indexOf(this.active) + 1 : this.list.length;
    this.list.splice(at, 0, tab);
    this.strip.insertBefore(tab.el, this.list[at + 1]?.el || null);
    tab.key = info.key;
    tab.info = { path: info.path, name: info.name }; // for labels until loaded
    this.label();
    this.activate(tab, false);
    await tab.load(info);
    if (this.active === tab) {
      this.onActivate(tab);
      if (fragment) tab.jumpTo(fragment);
    }
    return tab;
  }

  activate(tab, notify = true) {
    if (!tab) return;
    if (this.active && this.active !== tab) this.active.hide();
    this.active = tab;
    for (const t of this.list) {
      t.el.classList.toggle('active', t === tab);
      t.el.setAttribute('aria-selected', String(t === tab));
    }
    tab.show();
    tab.el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    if (notify) this.onActivate(tab);
  }

  close(tab) {
    if (!tab) return;
    const i = this.list.indexOf(tab);
    this.list.splice(i, 1);
    tab.el.remove();
    tab.destroy();
    this.onClose(tab);
    if (this.active === tab) {
      this.active = null;
      const next = this.list[Math.min(i, this.list.length - 1)];
      if (next) this.activate(next);
      else this.onActivate(null);
    }
    this.label();
  }

  next(dir) {
    if (this.list.length < 2) return;
    const i = this.list.indexOf(this.active);
    this.activate(this.list[(i + dir + this.list.length) % this.list.length]);
  }

  select(n) {
    this.activate(n < 0 ? this.list[this.list.length - 1] : this.list[n]);
  }

  // label names the tabs, adding the folder when two files share a name.
  label() {
    const count = new Map();
    for (const t of this.list) count.set(fold(t.info.name), (count.get(fold(t.info.name)) || 0) + 1);
    for (const t of this.list) {
      let text = t.info.name;
      if (count.get(fold(text)) > 1) {
        const parts = t.info.path.split(/[\\/]/);
        if (parts.length > 1) text += ' — ' + parts[parts.length - 2];
      }
      t.el.querySelector('.name').textContent = text;
      t.el.title = t.info.path;
    }
  }
}

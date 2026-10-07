// Virtual document view.
//
// The document is a column of chunk elements. A chunk holds a placeholder
// height until it nears the viewport, then its HTML is fetched from Go and
// mounted. Far away chunks are emptied again (keeping their height) so the
// DOM stays small for large files.

const MOUNT_MARGIN = '200% 0px'; // mount within ~2 screens
const KEEP_MARGIN = '600% 0px';  // unmount beyond ~6 screens
const UNMOUNT_MIN_BYTES = 1 << 20; // small documents stay fully mounted

// Chromium keeps the reading position steady by itself when content above
// changes height. Elsewhere (WebKit) it is done by hand in onResize.
const NATIVE_ANCHOR = CSS.supports('overflow-anchor', 'auto');

export class Viewer {
  constructor(scroller, root, { fetchChunk, afterMount }) {
    this.sc = scroller;
    this.root = root;
    this.fetchChunk = fetchChunk;
    this.afterMount = afterMount;
    this.info = null;
    this.chunks = [];
    this.gen = 0;
    this.sumEst = 0;
    this.sumReal = 0;
    this.paused = false;  // hidden tab: ignore observer noise
    this.holding = false; // printing: keep everything mounted

    this.mountObs = new IntersectionObserver((es) => {
      for (const e of es) if (e.isIntersecting) this.mount(+e.target.dataset.i);
    }, { root: scroller, rootMargin: MOUNT_MARGIN });

    this.keepObs = new IntersectionObserver((es) => {
      for (const e of es) if (!e.isIntersecting) this.unmount(+e.target.dataset.i);
    }, { root: scroller, rootMargin: KEEP_MARGIN });

    this.resizeObs = new ResizeObserver((es) => this.onResize(es));
  }

  get ratio() {
    return this.sumEst > 0 ? this.sumReal / this.sumEst : 1;
  }

  // metrics reads the current typography once for rawEstimate.
  metrics() {
    const cs = getComputedStyle(this.root);
    const fs = parseFloat(cs.fontSize) || 17;
    const lh = parseFloat(cs.lineHeight) || fs * 1.6;
    const width = Math.max(200, this.root.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight));
    return { lh, perLine: Math.max(20, width / (fs * 0.5)) };
  }

  // Rough height of a chunk from its size and the current typography.
  rawEstimate(c, m) {
    return (c.bytes / m.perLine + c.lines * 0.45) * m.lh;
  }

  // load shows a document, resolving false if it didn't. With an anchor
  // (from anchor()), the reading position is restored, e.g. after a live
  // reload.
  async load(info, anchor) {
    const gen = ++this.gen;
    const n = info.chunks.length;
    const start = anchor ? Math.min(anchor.chunk, n - 1) : 0;

    // Fetch the first visible chunks before swapping, to avoid a flash.
    const pre = new Map();
    await Promise.all([start, start + 1, start + 2].filter((i) => i < n).map(async (i) => {
      try { pre.set(i, await this.fetchChunk(info.id, i)); } catch { /* mounted later */ }
    }));
    // Superseded, or the tab was hidden meanwhile (it can't be measured
    // now); the caller loads it again when shown.
    if (gen !== this.gen || this.paused) return false;

    this.mountObs.disconnect();
    this.keepObs.disconnect();
    this.resizeObs.disconnect();
    this.info = info;
    this.big = info.size > UNMOUNT_MIN_BYTES;
    this.root.textContent = '';
    // Keep what was learnt about this document's heights across a reload.
    const ratio = anchor ? this.ratio : 1;
    this.sumEst = 1000;
    this.sumReal = 1000 * ratio;

    const frag = document.createDocumentFragment();
    this.chunks = info.chunks.map((ci, i) => {
      const el = document.createElement('div');
      el.className = 'chunk';
      el.dataset.i = i;
      frag.append(el);
      return { i, el, bytes: ci.bytes, lines: ci.lines, state: 'empty', h: 0, measured: false, raw: 0 };
    });
    this.root.append(frag);

    const m = this.metrics();
    for (const c of this.chunks) {
      c.raw = this.rawEstimate(c, m);
      c.h = Math.round(c.raw * ratio);
      c.el.style.height = c.h + 'px';
    }
    for (const [i, html] of pre) this.mountHTML(this.chunks[i], html);

    if (anchor) this.restore(anchor);
    else this.sc.scrollTop = 0;

    for (const c of this.chunks) {
      this.mountObs.observe(c.el);
      if (this.big) this.keepObs.observe(c.el);
    }
    return true;
  }

  clear() {
    this.gen++;
    this.mountObs.disconnect();
    this.keepObs.disconnect();
    this.resizeObs.disconnect();
    this.info = null;
    this.chunks = [];
    this.root.textContent = '';
  }

  mount(i) {
    const c = this.chunks[i];
    if (!c) return Promise.resolve();
    if (c.state === 'mounted') return Promise.resolve();
    if (c.state === 'loading') return c.promise;
    c.state = 'loading';
    const gen = this.gen;
    c.promise = this.fetchChunk(this.info.id, i).then((html) => {
      if (gen !== this.gen || c.state !== 'loading') return;
      if (this.paused) {
        c.state = 'empty'; // tab hidden meanwhile; it can't be measured now
        return;
      }
      this.mountHTML(c, html);
    }, () => {
      if (c.state === 'loading') c.state = 'empty';
    });
    return c.promise;
  }

  mountHTML(c, html) {
    const before = c.h;
    c.el.innerHTML = html;
    c.el.style.height = '';
    c.state = 'mounted';
    const h = c.el.offsetHeight;
    if (!c.measured) {
      c.measured = true;
      this.sumEst += c.raw;
      this.sumReal += h;
      this.scheduleReestimate();
    }
    c.h = h;
    if (!NATIVE_ANCHOR && h !== before && this.above(c, before)) this.sc.scrollTop += h - before;
    this.resizeObs.observe(c.el);
    this.afterMount(c.el, c.i);
  }

  unmount(i) {
    if (this.paused || this.holding) return;
    const c = this.chunks[i];
    if (!c || c.state !== 'mounted' || !this.big) {
      if (c && c.state === 'loading') c.state = 'empty';
      return;
    }
    this.resizeObs.unobserve(c.el);
    c.h = c.el.offsetHeight;
    c.el.style.height = c.h + 'px';
    c.el.textContent = '';
    c.state = 'empty';
  }

  // above reports whether chunk c (with height h) lies wholly above the
  // viewport, so a change in its height would move what the reader sees.
  above(c, h) {
    return c.el.offsetTop + h <= this.sc.scrollTop + 1;
  }

  onResize(entries) {
    if (this.paused) return;
    for (const e of entries) {
      const c = this.chunks[+e.target.dataset.i];
      if (!c || c.state !== 'mounted' || c.el !== e.target) continue;
      const h = c.el.offsetHeight;
      if (h === c.h) continue;
      if (!NATIVE_ANCHOR && this.above(c, c.h)) this.sc.scrollTop += h - c.h;
      c.h = h;
    }
  }

  // Once some chunks are measured, refine estimates for those not yet seen.
  // Only chunks below the viewport change, so nothing visible moves.
  scheduleReestimate() {
    clearTimeout(this.reTimer);
    this.reTimer = setTimeout(() => {
      const r = this.ratio;
      const bottom = this.sc.scrollTop + this.sc.clientHeight;
      for (const c of this.chunks) {
        if (c.measured || c.state !== 'empty' || c.el.offsetTop < bottom) continue;
        c.h = Math.round(c.raw * r);
        c.el.style.height = c.h + 'px';
      }
    }, 250);
  }

  // relayout re-estimates unmounted chunks after typography changes, keeping
  // the reading position.
  relayout() {
    if (!this.info) return;
    const a = this.anchor();
    const r = this.ratio;
    const m = this.metrics();
    for (const c of this.chunks) {
      if (c.state === 'mounted') continue;
      c.raw = this.rawEstimate(c, m);
      c.measured = false;
      c.h = Math.round(c.raw * r);
      c.el.style.height = c.h + 'px';
    }
    this.sumEst = this.sumReal = 0;
    for (const c of this.chunks) {
      if (c.state !== 'mounted') continue;
      c.raw = this.rawEstimate(c, m);
      this.sumEst += c.raw;
      this.sumReal += c.el.offsetHeight;
    }
    this.restore(a);
  }

  // chunkAt returns the index of the chunk at scroll offset y.
  chunkAt(y) {
    let lo = 0, hi = this.chunks.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.chunks[mid].el.offsetTop <= y) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }

  anchor() {
    if (!this.chunks.length) return { chunk: 0, frac: 0 };
    const y = this.sc.scrollTop;
    const i = this.chunkAt(y);
    const el = this.chunks[i].el;
    const h = Math.max(1, el.offsetHeight);
    return { chunk: i, frac: Math.max(0, Math.min(1, (y - el.offsetTop) / h)) };
  }

  restore(a) {
    const c = this.chunks[Math.min(a.chunk, this.chunks.length - 1)];
    if (c) this.sc.scrollTop = c.el.offsetTop + a.frac * c.el.offsetHeight;
  }

  // top returns an element's offset within the scrolled content.
  top(el) {
    return el.getBoundingClientRect().top - this.sc.getBoundingClientRect().top + this.sc.scrollTop;
  }

  // goTo scrolls to chunk i, or to the element with the given id inside it.
  async goTo(i, id) {
    const c = this.chunks[i];
    if (!c) return;
    if (c.state !== 'mounted') {
      this.sc.scrollTop = c.el.offsetTop;
      await this.mount(i);
    }
    let el = null;
    if (id) el = c.el.querySelector('#' + CSS.escape(id)) || document.getElementById(id);
    this.sc.scrollTop = this.top(el || c.el) - 12;
  }

  // ensure mounts chunk i and returns its element.
  async ensure(i) {
    await this.mount(i);
    return this.chunks[i]?.el;
  }

  mounted() {
    return this.chunks.filter((c) => c.state === 'mounted');
  }

  // setActive pauses the viewer while its tab is hidden, when every element
  // measures zero and would otherwise be unmounted or mis-measured.
  setActive(on) {
    this.paused = !on;
  }

  // mountAll renders the whole document (for printing) and keeps it mounted
  // until resumeUnmount. onProgress gets the fraction done; returning false
  // stops early, as do hiding the tab and loading another parse. It
  // resolves true only when every chunk is mounted.
  async mountAll(onProgress) {
    this.holding = true;
    const gen = this.gen;
    const todo = this.chunks.filter((c) => c.state !== 'mounted').map((c) => c.i);
    const total = this.chunks.length;
    let done = total - todo.length;
    let next = 0;
    let stopped = false;
    const worker = async () => {
      while (!stopped && next < todo.length) {
        await this.mount(todo[next++]);
        if (this.paused || gen !== this.gen || onProgress?.(++done / total) === false) stopped = true;
      }
    };
    await Promise.all(Array.from({ length: 6 }, worker));
    return !stopped && gen === this.gen && this.chunks.every((c) => c.state === 'mounted');
  }

  // resumeUnmount ends mountAll, emptying chunks far from the view again.
  resumeUnmount() {
    this.holding = false;
    if (!this.big) return;
    const top = this.sc.scrollTop;
    const h = this.sc.clientHeight;
    for (const c of this.chunks) {
      if (c.state !== 'mounted') continue;
      const y = c.el.offsetTop;
      if (y + c.el.offsetHeight < top - 6 * h || y > top + 7 * h) this.unmount(c.i);
    }
  }
}

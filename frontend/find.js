// Find in document. Go counts matches per chunk across the whole file; the
// chunk holding the current match is mounted and its matches highlighted.

const HAS_HIGHLIGHT = typeof CSS !== 'undefined' && 'highlights' in CSS && typeof Highlight !== 'undefined';
const SKIP = 'pre.mermaid, .diagram, .math, .katex, a.footnote-ref, a.footnote-backref, script, style';

// foldedText walks the text of root, lower-cased with whitespace collapsed
// (matching Go's foldText), and maps each folded character back to its
// text node and offset.
function foldedText(root) {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => (n.parentElement?.closest(SKIP) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
  });
  let s = '';
  const nodes = [];
  const offs = [];
  let space = true; // trims leading whitespace
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const t = n.data;
    for (let k = 0; k < t.length; k++) {
      const ch = t[k];
      if (/\s/.test(ch)) {
        if (!space) {
          s += ' ';
          nodes.push(n);
          offs.push(k);
          space = true;
        }
        continue;
      }
      const low = ch.toLowerCase();
      for (let j = 0; j < low.length; j++) {
        s += low[j];
        nodes.push(n);
        offs.push(k);
      }
      space = false;
    }
  }
  return { s, nodes, offs };
}

function fold(q) {
  return q.toLowerCase().split(/\s+/).filter(Boolean).join(' ');
}

function rangesIn(root, q) {
  const { s, nodes, offs } = foldedText(root);
  const out = [];
  if (!q) return out;
  for (let at = s.indexOf(q); at >= 0; at = s.indexOf(q, at + q.length)) {
    const end = at + q.length - 1;
    const r = document.createRange();
    r.setStart(nodes[at], offs[at]);
    r.setEnd(nodes[end], offs[end] + 1);
    out.push(r);
  }
  return out;
}

export class Find {
  constructor({ bar, input, count, prev, next, close }, { viewer, search, docId }) {
    Object.assign(this, { bar, input, countEl: count, viewer, search, docId });
    this.q = '';
    this.counts = [];
    this.total = 0;
    this.pos = -1; // global match index
    this.gen = 0;

    input.addEventListener('input', () => {
      clearTimeout(this.timer);
      this.timer = setTimeout(() => this.run(), 160);
    });
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        if (this.q !== fold(input.value)) this.run();
        else this.step(e.shiftKey ? -1 : 1);
      } else if (e.key === 'Escape') {
        e.preventDefault();
        this.close();
      }
    });
    prev.addEventListener('click', () => this.step(-1));
    next.addEventListener('click', () => this.step(1));
    close.addEventListener('click', () => this.close());
  }

  get isOpen() {
    return !this.bar.hidden;
  }

  open() {
    this.bar.hidden = false;
    const sel = window.getSelection().toString().trim();
    if (sel && !sel.includes('\n') && sel.length < 200) this.input.value = sel;
    this.input.focus();
    this.input.select();
    if (this.input.value && fold(this.input.value) !== this.q) this.run();
  }

  close() {
    this.bar.hidden = true;
    this.clearMarks();
    this.q = '';
    this.counts = [];
    this.total = 0;
    this.pos = -1;
    this.countEl.textContent = '';
    this.viewer.sc.focus();
  }

  // reset is called when the document changes.
  reset() {
    if (!this.isOpen) return;
    this.q = '';
    this.run();
  }

  async run() {
    const q = fold(this.input.value);
    const gen = ++this.gen;
    this.q = q;
    this.clearMarks();
    if (!q || !this.viewer.info) {
      this.counts = [];
      this.total = 0;
      this.pos = -1;
      this.countEl.textContent = '';
      return;
    }
    let counts;
    try {
      counts = await this.search(this.docId(), q);
    } catch {
      return;
    }
    if (gen !== this.gen) return;
    this.counts = counts;
    this.total = counts.reduce((a, b) => a + b, 0);
    if (!this.total) {
      this.pos = -1;
      this.countEl.textContent = 'No results';
      return;
    }
    // Start from the first match at or after the top of the view.
    const sc = this.viewer.sc;
    const here = this.viewer.chunkAt(sc.scrollTop);
    let k = 0;
    for (let i = 0; i < here; i++) k += counts[i];
    const c = this.viewer.chunks[here];
    if (c?.state === 'mounted' && counts[here]) {
      const top = sc.getBoundingClientRect().top;
      const ranges = rangesIn(c.el, q);
      const idx = ranges.findIndex((r) => r.getBoundingClientRect().top >= top);
      k += idx < 0 ? counts[here] : Math.min(idx, counts[here] - 1);
    }
    if (k >= this.total) k = 0;
    this.show(k);
  }

  step(d) {
    if (!this.total) return;
    this.show((this.pos + d + this.total) % this.total);
  }

  locate(k) {
    let i = 0;
    while (i < this.counts.length && k >= this.counts[i]) k -= this.counts[i++];
    return [i, k];
  }

  async show(k) {
    const gen = this.gen;
    this.pos = k;
    let [ci, nth] = this.locate(k);
    const el = await this.viewer.ensure(ci);
    if (gen !== this.gen || !el) return;
    const ranges = rangesIn(el, this.q);
    // Go's count can differ slightly from the DOM (e.g. raw HTML), so trust
    // the DOM once the chunk is on screen.
    if (ranges.length !== this.counts[ci]) {
      const before = k - nth;
      this.total += ranges.length - this.counts[ci];
      this.counts[ci] = ranges.length;
      if (!ranges.length) {
        if (!this.total) {
          this.countEl.textContent = 'No results';
          return;
        }
        return this.show(before % this.total);
      }
      nth = Math.min(nth, ranges.length - 1);
      this.pos = before + nth;
    }
    this.countEl.textContent = `${this.pos + 1} of ${this.total}`;
    this.mark(ranges, nth);
  }

  mark(ranges, nth) {
    const cur = ranges[nth];
    if (HAS_HIGHLIGHT) {
      CSS.highlights.set('find', new Highlight(...ranges));
      CSS.highlights.set('find-current', new Highlight(cur));
    } else {
      const sel = window.getSelection();
      sel.removeAllRanges();
      sel.addRange(cur);
    }
    const sc = this.viewer.sc;
    const r = cur.getBoundingClientRect();
    const box = sc.getBoundingClientRect();
    if (r.top < box.top + 40 || r.bottom > box.bottom - 40) {
      sc.scrollTop += r.top - box.top - sc.clientHeight / 3;
    }
    const inner = r.left - box.left;
    if (inner < 0 || inner > box.width) cur.startContainer.parentElement?.scrollIntoView({ inline: 'center', block: 'nearest' });
  }

  clearMarks() {
    if (HAS_HIGHLIGHT) {
      CSS.highlights.delete('find');
      CSS.highlights.delete('find-current');
    }
  }
}

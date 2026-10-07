// Table of contents: a virtual list with fixed row heights, so documents with
// tens of thousands of headings stay quick.

const ROW = 28;
const OVERSCAN = 10;

export class Toc {
  constructor(nav, inner, onPick) {
    this.nav = nav;
    this.inner = inner;
    this.onPick = onPick;
    this.items = [];
    this.cur = -1;
    this.rows = new Map(); // index -> element
    this.base = 1;
    nav.addEventListener('scroll', () => this.render(), { passive: true });
    inner.addEventListener('click', (e) => {
      const row = e.target.closest('.toc-item');
      if (row) this.onPick(this.items[+row.dataset.i]);
    });
  }

  // set replaces the entries. keepScroll leaves the list where it was, for
  // reloads of the same file.
  set(items, keepScroll = false) {
    this.items = items;
    this.cur = -1;
    this.base = items.reduce((m, h) => Math.min(m, h.level), 6);
    this.inner.textContent = '';
    this.rows.clear();
    if (!items.length) {
      this.inner.style.height = '';
      this.inner.innerHTML = '<div class="toc-empty">No headings</div>';
      return;
    }
    this.inner.style.height = items.length * ROW + 'px';
    if (!keepScroll) this.nav.scrollTop = 0;
    this.render();
  }

  render() {
    if (this.nav.hidden || !this.items.length) return;
    const first = Math.max(0, Math.floor(this.nav.scrollTop / ROW) - OVERSCAN);
    const last = Math.min(this.items.length - 1, Math.ceil((this.nav.scrollTop + this.nav.clientHeight) / ROW) + OVERSCAN);
    for (const [i, el] of this.rows) {
      if (i < first || i > last) {
        el.remove();
        this.rows.delete(i);
      }
    }
    for (let i = first; i <= last; i++) {
      let el = this.rows.get(i);
      if (!el) {
        const h = this.items[i];
        el = document.createElement('div');
        el.className = 'toc-item l' + (h.level - this.base + 1);
        el.dataset.i = i;
        el.style.top = i * ROW + 'px';
        el.style.paddingLeft = 14 + (h.level - this.base) * 14 + 'px';
        el.textContent = h.text || '(untitled)';
        el.title = h.text;
        this.inner.append(el);
        this.rows.set(i, el);
      }
      el.classList.toggle('current', i === this.cur);
    }
  }

  setCurrent(i) {
    if (i === this.cur) return;
    this.cur = i;
    if (i >= 0 && !this.nav.hidden) {
      const y = i * ROW;
      if (y < this.nav.scrollTop || y + ROW > this.nav.scrollTop + this.nav.clientHeight) {
        this.nav.scrollTop = y - this.nav.clientHeight / 3;
      }
    }
    this.render();
  }
}

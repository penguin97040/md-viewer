// HTML drags use the web view's native drag session. Only an opaque Go-issued
// token crosses windows; native adapters decide cancellation and release point.
const FORMAT = 'application/x-md-viewer-tab';

export function installMovement(tabs, { go, state, freeze, resume, order, error }) {
  const strip = tabs.strip;
  const marker = document.createElement('div');
  marker.className = 'tab-insertion';
  marker.hidden = true;
  document.body.append(marker);
  let armed = null;
  let dragging = null;
  let index = 0;
  let frame = 0;
  let lastX = 0;
  let escape = false;
  let serial = 0;
  let arming = Promise.resolve();
  const sources = new Map();
  const handled = new Set();

  const isTab = (e) => e.dataTransfer?.types.includes(FORMAT) || e.dataTransfer?.types.includes('text/plain');
  const dragToken = (e) => {
    const custom = e.dataTransfer.getData(FORMAT);
    if (/^[a-f0-9]{64}$/.test(custom)) return custom;
    const text = e.dataTransfer.getData('text/plain');
    return /^md-viewer-tab:[a-f0-9]{64}$/.test(text) ? text.slice(14) : '';
  };
  const stop = () => { cancelAnimationFrame(frame); frame = 0; marker.hidden = true; };
  const insertion = (x) => {
    for (let i = 0; i < tabs.list.length; i++) {
      const r = tabs.list[i].el.getBoundingClientRect();
      if (x < r.left + r.width / 2) return i;
    }
    return tabs.list.length;
  };
  const show = () => {
    index = insertion(lastX);
    const r = strip.getBoundingClientRect();
    const next = tabs.list[index]?.el.getBoundingClientRect();
    const prev = tabs.list[index - 1]?.el.getBoundingClientRect();
    const x = next?.left ?? prev?.right ?? r.left;
    marker.style.cssText = `left:${Math.max(r.left, Math.min(x, r.right - 2))}px;top:${r.top + 5}px;height:${r.height - 10}px`;
    marker.hidden = false;
  };
  const scroll = () => {
    const r = strip.getBoundingClientRect();
    const d = lastX < r.left + 36 ? -12 : lastX > r.right - 36 ? 12 : 0;
    if (d) { strip.scrollLeft += d; show(); }
    frame = requestAnimationFrame(scroll);
  };
  const cancelArmed = async () => {
    const old = armed;
    armed = null;
    if (old?.token) await go.CancelTransfer(old.token).catch(error);
  };
  strip.addEventListener('mousedown', async (e) => {
    if (e.button !== 0 || e.target.closest('.x')) return;
    const tab = tabs.list.find((t) => t.el === e.target.closest('.tab'));
    if (!tab || tab.transferring || tab.incoming) return;
    const gen = ++serial;
    arming = arming.then(async () => {
      await cancelArmed();
      if (gen !== serial) return;
      try {
        const token = await go.BeginTransfer(tab.key, state(tab));
        if (gen !== serial || tab.closed) { await go.CancelTransfer(token); return; }
        armed = { tab, token };
      } catch (err) { error(err); }
    });
  });
  strip.addEventListener('dragstart', (e) => {
    const tab = tabs.list.find((t) => t.el === e.target.closest('.tab'));
    if (!armed || armed.tab !== tab || e.target.closest('.x')) { e.preventDefault(); return; }
    dragging = armed;
    sources.set(dragging.token, dragging);
    armed = null;
    escape = false;
    dragging.dropped = false;
    e.dataTransfer.effectAllowed = 'move';
    e.dataTransfer.setData(FORMAT, dragging.token);
    // WebKit's native pasteboard can omit custom MIME data between processes.
    // The marked-text fallback carries the same opaque token, never a path.
    e.dataTransfer.setData('text/plain', 'md-viewer-tab:' + dragging.token);
    tab.el.classList.add('moving');
    freeze(tab, dragging.token);
    dragging.started = go.StartTabDrag(dragging.token, state(tab)).catch(error);
  });
  strip.addEventListener('dragover', (e) => {
    if (!isTab(e)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = 'move';
    lastX = e.clientX;
    show();
    if (!frame) frame = requestAnimationFrame(scroll);
  });
  strip.addEventListener('dragleave', (e) => {
    if (!strip.contains(e.relatedTarget)) stop();
  });
  const processDrop = async (token, at) => {
    if (handled.has(token)) return;
    handled.add(token);
    if (handled.size > 128) handled.delete(handled.values().next().value);
    stop();
    const source = sources.get(token);
    if (source) {
      if (source.tab.closed) return;
      source.dropped = true;
      tabs.reorder(source.tab, at);
      order();
      await go.CancelTransfer(token).catch(error);
      resume(source.tab);
    } else {
      try { await go.AcceptTransfer(token, at); } catch (err) { error(err); }
    }
  };
  strip.addEventListener('drop', (e) => {
    if (!isTab(e)) return;
    e.preventDefault();
    e.stopPropagation();
    const token = dragToken(e);
    if (token) processDrop(token, insertion(e.clientX));
  });
  strip.addEventListener('dragend', async () => {
    const old = dragging;
    dragging = null;
    stop();
    if (!old) return;
    old.tab.el.classList.remove('moving');
    await old.started;
    // Go refuses to detach over any registered viewer, and preserves an
    // accepted move while its destination is loading.
    if (!old.dropped) {
      try { await go.EndTabDrag(old.token, escape); } catch (err) {
        await go.CancelTransfer(old.token).catch(error);
        resume(old.tab);
        error(err);
      }
    } else await go.EndTabDrag(old.token, true).catch(error);
    sources.delete(old.token);
  });
  window.addEventListener('mouseup', () => {
    if (!dragging || dragging.tab.closed || !dragging.tab.transferring) { serial++; arming = arming.then(cancelArmed); }
  });
  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      escape = true;
      if (dragging) go.CancelTransfer(dragging.token).catch(error);
      else { serial++; arming = arming.then(cancelArmed); }
      stop();
    }
  }, true);
  // Document drops and unknown content never navigate the web view.
  window.addEventListener('dragover', (e) => {
    e.preventDefault();
    if (isTab(e) && !strip.contains(e.target)) { e.dataTransfer.dropEffect = 'none'; stop(); }
  });
  window.addEventListener('drop', (e) => {
    e.preventDefault();
    if (isTab(e)) {
      const token = dragToken(e);
      if (token) go.CancelTransfer(token).catch(error);
      if (dragging?.token === token) dragging.dropped = true;
    }
    stop();
  });
  const inStrip = ({ x, y, valid }) => {
    const r = strip.getBoundingClientRect();
    return valid && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
  };
  return {
    hover(point) {
      if (!inStrip(point)) { stop(); return; }
      lastX = point.x;
      show();
      if (!frame) frame = requestAnimationFrame(scroll);
    },
    drop(point) {
      stop();
      if (inStrip(point) && /^[a-f0-9]{64}$/.test(point.token)) processDrop(point.token, insertion(point.x));
    },
    leave: stop,
  };

}

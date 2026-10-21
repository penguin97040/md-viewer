// Native window controls and the compact toolbar share one app header.
export function installHeader({ rt, go, shell, isMac }) {
  const $ = (id) => shell.get(id);
  const html = document.documentElement;
  html.dataset.platform = isMac ? 'mac' : 'other';
  // Wails can retain a resize edge after a native pointer grab. Clear it
  // for interior presses before the runtime mistakes a tab click for resizing.
  if (!isMac) window.addEventListener('mousedown', (e) => {
    const flags = window.wails?.flags;
    if (!flags?.enableResize || !flags.resizeEdge) return;
    const edge = flags.borderThickness;
    if (e.clientX >= edge && e.clientY >= edge && e.clientX < innerWidth - edge && e.clientY < innerHeight - edge) {
      flags.resizeEdge = undefined;
      html.style.cursor = flags.defaultCursor || '';
    }
  }, { capture: true });
  const maximise = $('win-maximise');
  let stateRequest = 0;
  async function syncWindow() {
    if (isMac || !rt.WindowIsMaximised) return;
    const request = ++stateRequest;
    try {
      const full = await rt.WindowIsMaximised();
      if (request !== stateRequest) return;
      maximise.title = full ? 'Restore' : 'Maximise';
      maximise.setAttribute('aria-label', maximise.title);
      $('maximise-path').setAttribute('d', full ? 'M5 9h10v10H5zM9 9V5h10v10h-4' : 'M6 6h12v12H6z');
    } catch { /* the window may be closing */ }
  }
  const toggleMaximise = () => { rt.WindowToggleMaximise?.(); syncWindow(); };
  $('win-minimise').addEventListener('click', () => rt.WindowMinimise?.());
  maximise.addEventListener('click', toggleMaximise);
  $('win-close').addEventListener('click', () => go.CloseWindow().catch(() => {}));
  $('bar').addEventListener('dblclick', (e) => {
    if (getComputedStyle(e.target).getPropertyValue('--wails-draggable').trim() === 'drag') toggleMaximise();
  });
  window.addEventListener('resize', syncWindow);
  window.addEventListener('focus', syncWindow);
  syncWindow();

  const tools = $('tools'), trigger = $('btn-tools');
  function closeTools() {
    if (tools.contains(document.activeElement) && matchMedia('(max-width: 819px)').matches) trigger.focus();
    tools.classList.remove('open');
    trigger.setAttribute('aria-expanded', 'false');
  }
  trigger.addEventListener('click', () => {
    const open = tools.classList.toggle('open');
    trigger.setAttribute('aria-expanded', String(open));
    if (open) tools.querySelector('button').focus();
  });
  tools.addEventListener('click', (e) => { if (e.target.closest('button')) closeTools(); });
  document.addEventListener('mousedown', (e) => {
    if (!tools.contains(e.target) && !trigger.contains(e.target)) closeTools();
  });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && tools.classList.contains('open')) {
      closeTools();
      trigger.focus();
    }
  });
  matchMedia('(max-width: 819px)').addEventListener('change', closeTools);

  for (const link of $('settings').querySelectorAll('[data-licence]')) {
    link.addEventListener('click', async (e) => {
      e.preventDefault();
      try {
        const response = await fetch(link.getAttribute('href'));
        if (!response.ok) throw new Error('Licence unavailable');
        $('licence-text').textContent = await response.text();
        if (!$('licence-dialog').open) $('licence-dialog').showModal();
      } catch {
        $('licence-text').textContent = 'Licence unavailable. Please try again.';
        if (!$('licence-dialog').open) $('licence-dialog').showModal();
      }
    });
  }
  $('licence-close').addEventListener('click', () => $('licence-dialog').close());
}

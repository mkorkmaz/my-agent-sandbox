export function initializeSidebar(resized: () => void): void {
  const shell = document.querySelector<HTMLElement>('.app-shell')!;
  const sidebar = document.getElementById('sidebar')!;
  const handle = document.getElementById('sidebar-resizer')!;
  const window = document.defaultView!;
  const minimum = 280;
  let preferredWidth = minimum;
  let dragging = false;

  try {
    const saved = Number(window.localStorage.getItem('tugboat.sidebarWidth'));
    if (Number.isFinite(saved) && saved >= minimum) preferredWidth = saved;
  } catch (error) { console.warn('Could not restore the sidebar width.', error); }

  function resize(width: number): void {
    const maximum = Math.max(minimum, window.innerWidth - 480);
    const clamped = Math.round(Math.min(maximum, Math.max(minimum, width)));
    shell.style.setProperty('--sidebar-width', clamped + 'px');
    handle.setAttribute('aria-valuenow', String(clamped));
    handle.setAttribute('aria-valuemax', String(maximum));
    resized();
  }

  function save(): void {
    try { window.localStorage.setItem('tugboat.sidebarWidth', String(preferredWidth)); }
    catch (error) { console.warn('Could not save the sidebar width.', error); }
  }

  handle.addEventListener('pointerdown', event => {
    if (event.button !== 0) return;
    event.preventDefault();
    handle.focus();
    handle.setPointerCapture(event.pointerId);
    dragging = true;
    shell.classList.add('resizing-sidebar');
  });
  handle.addEventListener('pointermove', event => {
    if (!dragging) return;
    resize(event.clientX - sidebar.getBoundingClientRect().left);
    preferredWidth = Number(handle.getAttribute('aria-valuenow'));
  });
  handle.addEventListener('lostpointercapture', () => {
    dragging = false;
    shell.classList.remove('resizing-sidebar');
    save();
  });
  handle.addEventListener('pointerup', event => { handle.releasePointerCapture(event.pointerId); });
  handle.addEventListener('pointercancel', event => { handle.releasePointerCapture(event.pointerId); });
  handle.addEventListener('keydown', event => {
    const current = Number(handle.getAttribute('aria-valuenow'));
    const widths: Record<string, number> = { ArrowLeft: current - 16, ArrowRight: current + 16, Home: minimum, End: window.innerWidth - 480 };
    if (!(event.key in widths)) return;
    event.preventDefault();
    resize(widths[event.key]);
    preferredWidth = Number(handle.getAttribute('aria-valuenow'));
    save();
  });
  window.addEventListener('resize', () => { resize(preferredWidth); });
  resize(preferredWidth);
}

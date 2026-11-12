import { invoke } from '@tauri-apps/api/core';

export function initializeSidebar(resized: () => void): void {
  const shell = document.querySelector<HTMLElement>('.app-shell')!;
  const sidebar = document.getElementById('sidebar')!;
  const handle = document.getElementById('sidebar-resizer')!;
  const window = document.defaultView!;
  const minimum = 280;
  let preferredWidth = minimum;
  let dragging = false;
  let revision = 0;
  let saving = Promise.resolve();

  function resize(width: number): void {
    const maximum = Math.max(minimum, window.innerWidth - 480);
    const clamped = Math.round(Math.min(maximum, Math.max(minimum, width)));
    shell.style.setProperty('--sidebar-width', clamped + 'px');
    handle.setAttribute('aria-valuenow', String(clamped));
    handle.setAttribute('aria-valuemax', String(maximum));
    resized();
  }

  function save(): void {
    ++revision;
    const sidebarWidth = preferredWidth;
    saving = saving.then(() => invoke<void>('save_interface_preferences', { sidebarWidth }))
      .catch(error => { console.warn('Could not save the sidebar width.', error); });
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
  void invoke<{ sidebarWidth: number | null }>('load_interface_preferences').then(async preferences => {
    if (revision !== 0) return;
    const legacy = preferences.sidebarWidth === null ? Number(window.localStorage.getItem('tugboat.sidebarWidth')) : null;
    const saved = preferences.sidebarWidth ?? legacy;
    if (saved === null || !Number.isFinite(saved) || saved < minimum || saved > 16384) return;
    preferredWidth = saved;
    resize(preferredWidth);
    if (preferences.sidebarWidth !== null) window.localStorage.removeItem('tugboat.sidebarWidth');
    if (preferences.sidebarWidth === null) saving = saving.then(async () => {
      await invoke('save_interface_preferences', { sidebarWidth: saved });
      window.localStorage.removeItem('tugboat.sidebarWidth');
    }).catch(error => { console.warn('Could not migrate the sidebar width.', error); });
  }).catch(error => { console.warn('Could not restore the sidebar width.', error); });
}

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { SidebarDrag, SidebarPreferences } from './sidebar-state';
import { appData } from '../app/app-data';

export function initializeSidebar(resized: () => void): void {
  const shell = document.querySelector<HTMLElement>('.app-shell')!;
  const sidebar = document.getElementById('sidebar')!;
  const handle = document.getElementById('sidebar-resizer')!;
  const window = document.defaultView!;
  const minimum = 280;
  let editRevision = 0;
  let readRevision = 0;
  const drag = new SidebarDrag();
  const preferences = new SidebarPreferences();

  function resize(width: number): void {
    const maximum = Math.max(minimum, window.innerWidth - 480);
    const clamped = Math.round(Math.min(maximum, Math.max(minimum, width)));
    shell.style.setProperty('--sidebar-width', clamped + 'px');
    handle.setAttribute('aria-valuenow', String(clamped));
    handle.setAttribute('aria-valuemax', String(maximum));
    resized();
  }

  function save(): void {
    const sidebarWidth = appData.sidebarWidth;
    preferences.save({
      write: () => invoke<void>('save_interface_preferences', { sidebarWidth }),
      failed: error => { console.warn('Could not save the sidebar width.', error); },
    });
  }

  handle.addEventListener('pointerdown', event => {
    if (event.button !== 0 || !drag.start(event.pointerId)) return;
    event.preventDefault();
    handle.focus();
    handle.setPointerCapture(event.pointerId);
    shell.classList.add('resizing-sidebar');
  });
  handle.addEventListener('pointermove', event => {
    if (!drag.moves(event.pointerId)) return;
    ++editRevision;
    preferences.edit();
    resize(event.clientX - sidebar.getBoundingClientRect().left);
    appData.sidebarWidth = Number(handle.getAttribute('aria-valuenow'));
  });
  handle.addEventListener('lostpointercapture', event => {
    if (!drag.end(event.pointerId)) return;
    shell.classList.remove('resizing-sidebar');
    save();
  });
  handle.addEventListener('pointerup', event => { if (drag.moves(event.pointerId)) handle.releasePointerCapture(event.pointerId); });
  handle.addEventListener('pointercancel', event => { if (drag.moves(event.pointerId)) handle.releasePointerCapture(event.pointerId); });
  handle.addEventListener('keydown', event => {
    const current = Number(handle.getAttribute('aria-valuenow'));
    const widths: Record<string, number> = { ArrowLeft: current - 16, ArrowRight: current + 16, Home: minimum, End: window.innerWidth - 480 };
    if (!(event.key in widths)) return;
    event.preventDefault();
    ++editRevision;
    resize(widths[event.key]);
    appData.sidebarWidth = Number(handle.getAttribute('aria-valuenow'));
    save();
  });
  window.addEventListener('resize', () => { resize(appData.sidebarWidth); });
  resize(appData.sidebarWidth);
  void listen<string[]>('records-changed', async event => {
    if (!event.payload.includes('interface') || drag.active()) return;
    const read = ++readRevision, edit = editRevision;
    try {
      const result = await invoke<{ sidebarWidth: number | null }>('load_interface_preferences');
      if (read !== readRevision || edit !== editRevision || drag.active()) return;
      const width = result.sidebarWidth;
      if (width === null || !Number.isFinite(width) || width < minimum || width > 16384) return;
      appData.sidebarWidth = width;
      resize(appData.sidebarWidth);
    } catch (error) { console.warn('Could not refresh the sidebar width.', error); }
  }).catch(error => { console.warn('Could not watch the sidebar width.', error); });
  const initialRead = ++readRevision, initialEdit = editRevision;
  void invoke<{ sidebarWidth: number | null }>('load_interface_preferences').then(async result => {
    if (initialRead !== readRevision || initialEdit !== editRevision) return;
    if (!preferences.restore()) return;
    const legacy = result.sidebarWidth === null ? Number(window.localStorage.getItem('tugboat.sidebarWidth')) : null;
    const saved = result.sidebarWidth ?? legacy;
    if (saved === null || !Number.isFinite(saved) || saved < minimum || saved > 16384) return;
    appData.sidebarWidth = saved;
    resize(appData.sidebarWidth);
    if (result.sidebarWidth !== null) window.localStorage.removeItem('tugboat.sidebarWidth');
    if (result.sidebarWidth === null) preferences.migrate({
      write: async () => {
        await invoke('save_interface_preferences', { sidebarWidth: saved });
        window.localStorage.removeItem('tugboat.sidebarWidth');
      },
      failed: error => { console.warn('Could not migrate the sidebar width.', error); },
    });
  }).catch(error => { preferences.failed(); console.warn('Could not restore the sidebar width.', error); });
}

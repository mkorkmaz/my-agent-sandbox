import { invoke } from '@tauri-apps/api/core';
import { sameProjectView, type Project, type ProjectContext } from './projects';

type Mapping = NonNullable<Project['preview']>['mapping'];
type Status = 'closed' | 'publishing' | 'guest-stopped' | 'mapping-unavailable' | 'waiting-for-server' | 'ready' | 'closing';
type View = { projectId: string; status: Status; mapping: Mapping | null };

const messages: Record<Status, string> = {
  closed: 'No local preview is published.',
  publishing: 'Brig has not confirmed publication. Retry or close the preview.',
  'guest-stopped': 'The port is approved. Start the project environment to open it.',
  'mapping-unavailable': 'The approved port is unavailable. Check the environment or try to publish it again.',
  'waiting-for-server': 'The port is published. The guest web server has not responded.',
  ready: 'The guest web server is ready.',
  closing: 'Brig has not confirmed closure. Retry closing the preview.',
};

function validPort(value: number): boolean {
  return Number.isInteger(value) && value >= 1 && value <= 65535;
}

export function initializeProjectPreview(current: () => ProjectContext | null): () => void {
  const section = document.querySelector<HTMLDetailsElement>('#project-preview-section')!;
  const form = document.querySelector<HTMLFormElement>('#publish-project-preview')!;
  const host = document.querySelector<HTMLInputElement>('#preview-host-port')!;
  const guest = document.querySelector<HTMLInputElement>('#preview-guest-port')!;
  const publish = document.querySelector<HTMLButtonElement>('#publish-preview-port')!;
  const check = document.querySelector<HTMLButtonElement>('#check-preview-port')!;
  const open = document.querySelector<HTMLButtonElement>('#open-preview-browser')!;
  const close = document.querySelector<HTMLButtonElement>('#close-preview-port')!;
  const status = document.querySelector<HTMLParagraphElement>('#project-preview-status')!;
  let revision = 0;
  let busy = false;
  let approval: View | null = null;

  function render(): void {
    const mapped = approval?.mapping != null;
    const canPublish = !mapped || approval?.status === 'publishing' || approval?.status === 'mapping-unavailable';
    host.readOnly = guest.readOnly = mapped;
    publish.disabled = busy || !canPublish;
    check.disabled = busy;
    open.disabled = busy || (approval?.status !== 'ready' && approval?.status !== 'waiting-for-server');
    close.disabled = busy || !mapped;
  }

  function show(view: View): void {
    approval = view;
    if (view.mapping) {
      host.value = String(view.mapping.hostPort);
      guest.value = String(view.mapping.guestPort);
    }
    status.textContent = view.mapping ? `127.0.0.1:${view.mapping.hostPort}. ${messages[view.status]}` : messages[view.status];
  }

  async function request(action: 'read' | 'publish' | 'close' | 'open'): Promise<void> {
    const context = current();
    if (!context || busy || !section.open) return;
    const previousMapping = approval?.mapping;
    const mapping = action === 'publish' ? { hostPort: host.valueAsNumber, guestPort: guest.valueAsNumber } : null;
    if (mapping && (!validPort(mapping.hostPort) || !validPort(mapping.guestPort))) {
      status.textContent = 'Use a Mac port and guest port from 1 to 65535.';
      return;
    }
    const token = ++revision;
    const active = () => token === revision && section.open && sameProjectView(current(), context);
    const target = { identifier: context.id, environmentId: context.environmentId };
    busy = true;
    render();
    status.textContent = 'Checking the local preview…';
    try {
      const result = await invoke<View>('project_preview', { ...target, action: action === 'open' ? 'read' : action, mapping });
      if (!active()) return;
      if (result.projectId !== target.identifier || !Object.hasOwn(messages, result.status)
        || (result.mapping && (!validPort(result.mapping.hostPort) || !validPort(result.mapping.guestPort)))) {
        throw new Error('The preview result does not match the selected project.');
      }
      show(result);
      if (action === 'open' && result.status === 'ready' && result.mapping) {
        if (result.mapping.hostPort !== previousMapping?.hostPort || result.mapping.guestPort !== previousMapping.guestPort) {
          status.textContent = 'The approved preview has changed. Check the port and select Open in browser again.';
          return;
        }
        await invoke('open_project_preview', { ...target, mapping: result.mapping });
      }
    } catch (error) {
      if (active()) status.textContent = String(error);
    } finally {
      if (token === revision) { busy = false; render(); }
    }
  }

  section.addEventListener('toggle', () => {
    if (section.open) { void request('read'); return; }
    revision++;
    busy = false;
    render();
  });
  form.addEventListener('submit', event => { event.preventDefault(); void request('publish'); });
  check.addEventListener('click', () => { void request('read'); });
  open.addEventListener('click', () => { void request('open'); });
  close.addEventListener('click', () => { void request('close'); });
  return () => {
    revision++;
    busy = false;
    approval = null;
    section.open = false;
    form.reset();
    status.textContent = '';
    render();
  };
}

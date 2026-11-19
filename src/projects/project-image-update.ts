import { invoke } from '@tauri-apps/api/core';
import { setBusy } from '../ui/loading';
import { RequestState } from '../ui/request-state';
import { sameProject, sameProjectView, type ProjectContext } from './projects';

type Versions = { codex: string; claudeCode: string };
type Images = { previous: string; candidate: string; previousVersions: Versions; candidateVersions: Versions };
const kinds = {
  directory: 'Directory',
  file: 'File',
  link: 'Link',
  'device-or-deletion': 'Device or deleted entry',
  'special-file': 'Special file',
};
type Change = { path: string; kind: keyof typeof kinds };
type Status =
  | ({ status: 'available' } & Images)
  | ({ status: 'review'; changes: Change[]; token: string } & Images)
  | { status: 'current'; image: string; versions: Versions }
  | { status: 'recovery'; previous: string; candidate: string; message: string }
  | { status: 'updated'; previous: string; candidate: string; versions: Versions }
  | { status: 'restored'; image: string; versions: Versions; message: string }
  | { status: 'recovered' };
type View = Status & { projectId: string; environmentId: string };
type Action = { kind: 'read' | 'review' | 'recover' } | { kind: 'apply' | 'keep-current'; token: string };

function text(value: unknown): value is string {
  return typeof value === 'string' && value.trim().length > 0 && !/[\u0000-\u001f\u007f]/u.test(value);
}

function image(value: unknown): value is string {
  return typeof value === 'string' && /^[a-z0-9./:-]+@sha256:[a-f0-9]{64}$/u.test(value);
}

function versions(value: Versions | undefined): boolean {
  return !!value && text(value.codex) && text(value.claudeCode);
}

function checked(view: View, context: ProjectContext): boolean {
  if (!view || view.projectId !== context.id || view.environmentId !== context.environmentId) return false;
  switch (view.status) {
    case 'available':
      return image(view.previous) && image(view.candidate) && versions(view.previousVersions) && versions(view.candidateVersions);
    case 'review':
      return image(view.previous) && image(view.candidate) && versions(view.previousVersions) && versions(view.candidateVersions)
        && text(view.token) && Array.isArray(view.changes)
        && view.changes.every(change => text(change?.path) && change.path.startsWith('/') && Object.hasOwn(kinds, change.kind));
    case 'current':
      return image(view.image) && versions(view.versions);
    case 'restored':
      return image(view.image) && versions(view.versions) && text(view.message);
    case 'recovery':
      return image(view.previous) && image(view.candidate) && text(view.message);
    case 'updated':
      return image(view.previous) && image(view.candidate) && versions(view.versions);
    case 'recovered':
      return true;
  }
  return false;
}

function row(label: string, value: string): HTMLElement {
  const item = document.createElement('div');
  const name = document.createElement('dt');
  const data = document.createElement('dd');
  name.textContent = label;
  data.textContent = value;
  item.append(name, data);
  return item;
}

export function initializeProjectImageUpdate(
  current: () => ProjectContext | null,
  running: () => boolean,
  changed: (context: ProjectContext) => Promise<void>,
): { reset: () => void; render: () => void } {
  const section = document.querySelector<HTMLDetailsElement>('#project-image-update')!;
  const message = document.getElementById('project-image-update-status')!;
  const images = document.getElementById('project-image-update-images')!;
  const impact = document.getElementById('project-image-update-impact')!;
  const files = document.getElementById('project-image-update-files')!;
  const empty = document.getElementById('project-image-update-empty')!;
  const requests = new RequestState();
  let view: View | null = null;
  const buttons = [
    ['read', 'check-project-image'], ['review', 'review-project-image'], ['apply', 'apply-project-image'],
    ['keep-current', 'keep-project-image'], ['recover', 'recover-project-image'],
  ] as const;

  function visible(kind: Action['kind']): boolean {
    if (kind === 'read') return true;
    if (kind === 'review') return view?.status === 'available';
    if (kind === 'recover') return view?.status === 'recovery';
    return view?.status === 'review';
  }

  function render(): void {
    for (const [kind, id] of buttons) {
      const button = document.querySelector<HTMLButtonElement>('#' + id)!;
      button.hidden = !visible(kind);
      button.disabled = button.hidden || requests.pending() || (kind !== 'read' && running());
    }
    setBusy(message, requests.pending());
  }

  function show(next: View | null): void {
    view = next;
    const rows: HTMLElement[] = [];
    const addImage = (label: string, ref: string, versions?: Versions) => {
      rows.push(row(label, ref));
      if (versions) rows.push(row(label + ' agents', 'Codex ' + versions.codex + ' · Claude Code ' + versions.claudeCode));
    };
    if (next?.status === 'available' || next?.status === 'review') {
      addImage(next.status === 'available' ? 'Current image pin' : 'Current image', next.previous, next.previousVersions);
      addImage('Candidate image', next.candidate, next.candidateVersions);
    }
    if (next?.status === 'recovery') { addImage('Previous image', next.previous); addImage('Candidate image', next.candidate); }
    if (next?.status === 'updated') { addImage('Previous image', next.previous); addImage('Current image', next.candidate, next.versions); }
    if (next?.status === 'current' || next?.status === 'restored') addImage('Current image', next.image, next.versions);
    images.replaceChildren(...rows);
    images.hidden = !rows.length;
    const changes = next?.status === 'review' ? next.changes : [];
    impact.hidden = next?.status !== 'review';
    empty.hidden = !!changes.length;
    files.hidden = !changes.length;
    files.replaceChildren(...changes.map(change => {
      const item = document.createElement('li');
      item.textContent = kinds[change.kind] + ' · ' + change.path;
      return item;
    }));
  }

  async function request(kind: Action['kind']): Promise<void> {
    const context = current();
    if (!context || !section.open || requests.pending() || !visible(kind) || (kind !== 'read' && running())) return;
    const action: Action | null = kind === 'apply' || kind === 'keep-current'
      ? (view?.status === 'review' ? { kind, token: view.token } : null)
      : { kind };
    if (!action) return;
    const tag = requests.begin();
    const active = () => section.open && requests.accepts(tag) && sameProjectView(current(), context);
    message.textContent = {
      read: 'Checking the guest image…', review: 'Verifying the candidate and reviewing guest system files…',
      apply: 'Starting and verifying the candidate image…', 'keep-current': 'Restoring the current guest image…',
      recover: 'Recovering the guest image…',
    }[kind];
    render();
    let refresh = kind !== 'read';
    try {
      const result = await invoke<View>('project_image_update', { identifier: context.id, environmentId: context.environmentId, action });
      if (!checked(result, context)) throw new Error('The image update response does not match the requested project or operation.');
      refresh ||= result.status === 'review' || result.status === 'recovery';
      if (!active()) return;
      show(result);
      message.textContent = statusMessage(result);
    } catch (error) {
      if (!active()) return;
      show(null);
      message.textContent = String(error);
    } finally {
      try {
        if (refresh && sameProject(current(), context)) await changed(context);
      } finally {
        if (requests.accepts(tag)) { requests.finish(tag); render(); }
      }
    }
  }

  function reset(): void {
    requests.invalidate();
    show(null);
    message.textContent = '';
    section.open = false;
    render();
  }

  for (const [kind, id] of buttons) document.getElementById(id)!.addEventListener('click', () => { void request(kind); });
  section.addEventListener('toggle', () => {
    if (section.open) { void request('read'); return; }
    reset();
  });
  return { reset, render };
}

function statusMessage(view: View): string {
  switch (view.status) {
    case 'available': return 'An image update is available.';
    case 'current': return 'Already up to date.';
    case 'review': return 'The guest is stopped. Review the listed system changes before applying.';
    case 'recovery': return view.message;
    case 'updated': return 'The updated image is running. Tugboat verified both agent versions.';
    case 'restored': return 'The previous image and system disk are running. ' + view.message;
    case 'recovered': return 'Guest image recovery completed. Check the current image status.';
  }
}

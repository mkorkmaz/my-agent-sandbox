import { invoke } from '@tauri-apps/api/core';
import { sameProjectView, type ProjectContext } from './projects';

type Status = 'none' | 'saving' | 'saved' | 'revoking' | 'revoked-pending-cleanup' | 'unavailable';
type View = { projectId: string; repository: string | null; status: Status; running: boolean };

const messages: Record<Status, string> = {
  none: 'No GitHub fetch access is saved.',
  saving: 'Saving GitHub access is incomplete. Enter the token again to retry, or revoke the grant.',
  saved: 'Fetch-only access is saved for both agents. GitHub fetch permission is unverified.',
  revoking: 'Revocation is incomplete. Retry revoking access.',
  'revoked-pending-cleanup': 'GitHub fetch access is revoked. Retry revoking access to finish removing the saved grant.',
  unavailable: 'Brig cannot verify the saved grant. Replace the token or revoke access.',
};

export function initializeProjectGitHubAccess(current: () => ProjectContext | null): () => void {
  const section = document.querySelector<HTMLDetailsElement>('#project-github-access')!;
  const form = document.querySelector<HTMLFormElement>('#project-github-form')!;
  const repository = document.querySelector<HTMLInputElement>('#project-github-repository')!;
  const token = document.querySelector<HTMLInputElement>('#project-github-token')!;
  const save = document.querySelector<HTMLButtonElement>('#save-project-github')!;
  const check = document.querySelector<HTMLButtonElement>('#check-project-github')!;
  const revoke = document.querySelector<HTMLButtonElement>('#revoke-project-github')!;
  const cancel = document.querySelector<HTMLButtonElement>('#cancel-project-github')!;
  const stopImpact = document.querySelector<HTMLParagraphElement>('#project-github-stop-impact')!;
  const status = document.querySelector<HTMLParagraphElement>('#project-github-status')!;
  let latest: View | null = null;
  let revision = 0;
  let busy = false;

  function render(): void {
    const saved = latest?.repository != null;
    repository.readOnly = saved;
    repository.disabled = busy;
    const revoking = latest?.status === 'revoking' || latest?.status === 'revoked-pending-cleanup';
    token.disabled = busy || revoking;
    save.disabled = busy || !latest || revoking;
    check.disabled = busy;
    revoke.disabled = busy || !saved;
    cancel.disabled = busy;
    stopImpact.hidden = latest?.running !== true;
    if (latest?.running) {
      save.textContent = saved ? 'Stop environment and replace token' : 'Stop environment and save access';
      return;
    }
    save.textContent = saved ? 'Replace token' : 'Save fetch access';
  }

  async function request(action: 'read' | 'save' | 'revoke'): Promise<void> {
    const context = current();
    if (!context || busy || !section.open) return;
    const selectedRepository = action === 'save' ? repository.value.trim() : null;
    const password = action === 'save' ? token.value : null;
    token.value = '';
    const target = { identifier: context.id, environmentId: context.environmentId };
    const operation = ++revision;
    const active = () => operation === revision && section.open && sameProjectView(current(), context);
    busy = true;
    render();
    status.textContent = action === 'revoke' ? 'Revoking GitHub access…' : 'Checking GitHub access…';
    try {
      if (action === 'save' && latest?.running) {
        status.textContent = 'Stopping the project environment…';
        await invoke('stop_project', target);
        if (!active()) return;
      }
      const result = await invoke<View>('project_github_access', { ...target, action, repository: selectedRepository, token: password });
      if (!active()) return;
      if (result.projectId !== target.identifier || !Object.hasOwn(messages, result.status)) {
        throw new Error('The GitHub access result belongs to another project.');
      }
      latest = result;
      if (result.repository || action === 'revoke') repository.value = result.repository ?? '';
      status.textContent = messages[result.status];
    } catch (error) {
      if (active()) status.textContent = typeof error === 'string' ? error : 'Could not confirm the GitHub access change. Check access again.';
    } finally {
      if (operation === revision) { busy = false; render(); }
    }
  }

  section.addEventListener('toggle', () => {
    if (section.open) { void request('read'); return; }
    revision++;
    busy = false;
    token.value = '';
    render();
  });
  form.addEventListener('submit', event => { event.preventDefault(); void request('save'); });
  check.addEventListener('click', () => { void request('read'); });
  revoke.addEventListener('click', () => { void request('revoke'); });
  cancel.addEventListener('click', () => {
    token.value = '';
    repository.value = latest?.repository ?? '';
    section.open = false;
  });
  return () => {
    revision++;
    busy = false;
    latest = null;
    section.open = false;
    form.reset();
    status.textContent = '';
    render();
  };
}

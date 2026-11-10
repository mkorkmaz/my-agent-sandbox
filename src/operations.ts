import { invoke } from '@tauri-apps/api/core';
import type { Project } from './projects';

interface Context { id: string; fingerprint: string | null }
interface Preview { reference: string; workspace: string; description: string; fingerprint: string; external: boolean; hasEnvironment: boolean }

export function initializeOperations(current: () => Context | null, open: (command: string, arguments_: Record<string, unknown>, caption: string) => Promise<void>, stopped: (project: Project) => void, removed: (message: string) => Promise<void>): void {
  for (const [id, operation, caption] of [['git-status', 'git-status', 'Git status'], ['git-diff', 'git-diff', 'Git diff'], ['project-tests', 'tests', 'Project tests'], ['project-doctor', 'doctor', 'Brig doctor'], ['project-info', 'info', 'Brig session info'], ['project-policy', 'policy-check', 'Policy bindings'], ['project-logs', 'logs', 'Last 100 log lines']]) {
    document.getElementById(id)!.addEventListener('click', () => {
      const context = current(); if (!context) return;
      void open('start_project_operation', { identifier: context.id, fingerprint: context.fingerprint, operation }, caption);
    });
  }
  const status = document.getElementById('operation-status')!;
  const stop = document.getElementById('stop-project') as HTMLButtonElement;
  stop.addEventListener('click', async () => {
    const context = current(); if (!context) return;
    stop.disabled = true; status.textContent = 'Stopping Brig environment…';
    try {
      const project = await invoke<Project>('stop_project', { identifier: context.id });
      if (current()?.id === project.id) { stopped(project); status.textContent = 'Environment stopped. Guest home and conversations are preserved.'; }
    } catch (error) { if (current()?.id === context.id) status.textContent = String(error); }
    finally { stop.disabled = false; }
  });
  const dialog = document.getElementById('removal-dialog') as HTMLDialogElement;
  const confirmation = document.getElementById('removal-confirmation') as HTMLInputElement;
  const confirm = document.getElementById('confirm-removal') as HTMLButtonElement;
  let preview: Preview | null = null;
  let removingId: string | null = null;
  document.getElementById('preview-removal')!.addEventListener('click', async () => {
    const context = current(); if (!context) return;
    status.textContent = 'Preparing removal preview…';
    try {
      preview = await invoke<Preview>('preview_removal', { identifier: context.id });
      if (current()?.id !== context.id) return;
      status.textContent = '';
      removingId = context.id;
      document.getElementById('removal-description')!.textContent = preview.description;
      document.getElementById('removal-reference')!.textContent = preview.reference;
      document.getElementById('removal-workspace')!.textContent = preview.workspace;
      document.getElementById('removal-external')!.hidden = !preview.external;
      confirmation.value = ''; confirm.disabled = true;
      document.getElementById('removal-error')!.textContent = '';
      (document.getElementById('logout-on-removal') as HTMLInputElement).checked = false;
      (document.getElementById('logout-on-removal') as HTMLInputElement).disabled = !preview.hasEnvironment;
      dialog.showModal(); confirmation.focus();
    } catch (error) { if (current()?.id === context.id) status.textContent = String(error); }
  });
  confirmation.addEventListener('input', () => { confirm.disabled = confirmation.value !== preview?.reference; });
  document.getElementById('cancel-removal')!.addEventListener('click', () => { dialog.close(); document.getElementById('preview-removal')!.focus(); });
  confirm.addEventListener('click', async () => {
    if (!preview || !removingId || confirmation.value !== preview.reference) return;
    confirm.disabled = true;
    try {
      const message = await invoke<string>('remove_project', { identifier: removingId, fingerprint: preview.fingerprint, confirmation: confirmation.value, logout: (document.getElementById('logout-on-removal') as HTMLInputElement).checked });
      dialog.close(); await removed(message);
    } catch (error) { document.getElementById('removal-error')!.textContent = String(error); }
    finally { confirm.disabled = false; }
  });
}

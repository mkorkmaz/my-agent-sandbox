import { invoke } from '@tauri-apps/api/core';
import { showDialog } from './dialogs';
import { sameProject, sameProjectView, type Project, type ProjectContext } from './projects';
import { setBusy } from './loading';

interface Preview { reference: string; workspace: string; description: string; fingerprint: string; external: boolean; logout?: boolean | null }

export function initializeOperations(current: () => ProjectContext | null, open: (command: string, arguments_: Record<string, unknown>, caption: string) => Promise<void>, stopped: (project: Project) => Promise<void>, removed: (message: string) => Promise<void>): () => void {
  let request = 0;
  for (const [id, operation, caption] of [['git-status', 'git-status', 'Git status'], ['git-diff', 'git-diff', 'Working tree diff'], ['git-staged-diff', 'git-staged-diff', 'Staged diff'], ['project-tests', 'tests', 'Project tests'], ['project-doctor', 'doctor', 'Brig doctor'], ['project-info', 'info', 'Brig session info'], ['project-policy', 'policy-check', 'Policy bindings'], ['project-logs', 'logs', 'Last 100 log lines']]) {
    document.getElementById(id)!.addEventListener('click', () => {
      const context = current(); if (!context) return;
      void open('start_project_operation', { identifier: context.id, fingerprint: context.fingerprint, operation }, caption);
    });
  }
  const status = document.getElementById('operation-status')!;
  const stop = document.getElementById('stop-project') as HTMLButtonElement;
  stop.addEventListener('click', async () => {
    const context = current(); if (!context || stop.disabled) return;
    const pending = ++request;
    stop.disabled = true; status.textContent = 'Stopping Brig environment…';
    setBusy(stop, true);
    try {
      const project = await invoke<Project>('stop_project', { identifier: context.id });
      if (sameProject(current(), context)) await stopped(project);
      if (sameProjectView(current(), context) && pending === request) status.textContent = 'Environment stopped. Guest home and conversations are preserved.';
    } catch (error) { if (sameProjectView(current(), context) && pending === request) status.textContent = String(error); }
    finally { if (sameProjectView(current(), context)) { setBusy(stop, false); stop.disabled = false; } }
  });
  const dialog = document.getElementById('removal-dialog') as HTMLDialogElement;
  const confirmation = document.getElementById('removal-confirmation') as HTMLInputElement;
  const confirm = document.getElementById('confirm-removal') as HTMLButtonElement;
  let preview: Preview | null = null;
  let removingId: string | null = null;
  let previewContext: ProjectContext | null = null;
  const previewButton = document.getElementById('preview-removal') as HTMLButtonElement;
  previewButton.addEventListener('click', async () => {
    const context = current(); if (!context || previewButton.disabled) return;
    const pending = ++request;
    previewButton.disabled = true; setBusy(previewButton, true);
    preview = null; previewContext = null; removingId = null;
    status.textContent = 'Preparing removal preview…';
    try {
      const result = await invoke<Preview>('preview_removal', { identifier: context.id });
      if (!sameProjectView(current(), context) || pending !== request) return;
      preview = result; previewContext = context;
      status.textContent = '';
      removingId = context.id;
      document.getElementById('removal-description')!.textContent = preview.description;
      document.getElementById('removal-reference')!.textContent = preview.reference;
      document.getElementById('removal-workspace')!.textContent = preview.workspace;
      document.getElementById('removal-external')!.hidden = !preview.external;
      confirmation.value = ''; confirm.disabled = true;
      document.getElementById('removal-error')!.textContent = '';
      showDialog(dialog, 'preview-removal'); confirmation.focus();
    } catch (error) { if (sameProjectView(current(), context) && pending === request) status.textContent = String(error); }
    finally { if (sameProjectView(current(), context)) { previewButton.disabled = false; setBusy(previewButton, false); } }
  });
  confirmation.addEventListener('input', () => { confirm.disabled = confirmation.value !== preview?.reference; });
  document.getElementById('cancel-removal')!.addEventListener('click', () => { dialog.close(); });
  confirm.addEventListener('click', async () => {
    if (!preview || !removingId || !previewContext || !sameProjectView(current(), previewContext) || confirmation.value !== preview.reference) return;
    const context = previewContext;
    const pending = ++request;
    confirm.disabled = true;
    setBusy(confirm, true);
    try {
      const message = await invoke<string>('remove_project', { identifier: removingId, fingerprint: preview.fingerprint, confirmation: confirmation.value, logout: preview.logout ?? false });
      if (sameProjectView(current(), context)) dialog.close();
      if (sameProject(current(), context)) await removed(message);
    } catch (error) { if (sameProjectView(current(), context) && pending === request) document.getElementById('removal-error')!.textContent = String(error); }
    finally { if (sameProjectView(current(), context) && pending === request) { setBusy(confirm, false); confirm.disabled = false; } }
  });
  return () => {
    ++request; preview = null; removingId = null; previewContext = null;
    status.textContent = ''; stop.disabled = false; confirm.disabled = true; confirmation.value = '';
    for (const button of [stop, previewButton, confirm]) setBusy(button, false);
    previewButton.disabled = false;
    for (const id of ['removal-description', 'removal-reference', 'removal-workspace', 'removal-error']) document.getElementById(id)!.textContent = '';
    document.getElementById('removal-external')!.hidden = true;
    if (dialog.open) dialog.close();
  };
}

import { invoke } from '@tauri-apps/api/core';
import { showDialog } from '../ui/dialogs';
import { sameProject, sameProjectView, type Project, type ProjectContext } from './projects';
import { setBusy } from '../ui/loading';
import { RequestState } from '../ui/request-state';
import { RemovalReview } from './removal-review';
import type { OpenTerminal } from '../terminal/terminal-launch';

interface Preview { reference: string; workspace: string; description: string; fingerprint: string; external: boolean; logout?: boolean | null }

export function initializeOperations(current: () => ProjectContext | null, open: OpenTerminal, stopped: (project: Project) => Promise<void>, removed: (message: string) => Promise<void>): () => void {
  const requests = new RequestState();
  for (const [id, operation, caption] of [['git-status', 'git-status', 'Git status'], ['git-diff', 'git-diff', 'Working tree diff'], ['git-staged-diff', 'git-staged-diff', 'Staged diff'], ['project-tests', 'tests', 'Project tests'], ['project-doctor', 'doctor', 'Brig doctor'], ['project-info', 'info', 'Brig session info'], ['project-policy', 'policy-check', 'Policy bindings'], ['project-logs', 'logs', 'Last 100 log lines']] as const) {
    document.getElementById(id)!.addEventListener('click', () => {
      const context = current(); if (!context) return;
      void open('start_project_operation', { identifier: context.id, environmentId: context.environmentId, fingerprint: context.fingerprint, operation }, caption);
    });
  }
  const status = document.getElementById('operation-status')!;
  const stop = document.getElementById('stop-project') as HTMLButtonElement;
  stop.addEventListener('click', async () => {
    const context = current(); if (!context || stop.disabled) return;
    const pending = requests.begin();
    stop.disabled = true; status.textContent = 'Stopping Brig environment…';
    setBusy(stop, true);
    try {
      const project = await invoke<Project>('stop_project', { identifier: context.id, environmentId: context.environmentId });
      if (sameProject(current(), context)) await stopped(project);
      if (sameProjectView(current(), context) && requests.accepts(pending)) status.textContent = 'Environment stopped. Guest home and conversations are preserved.';
    } catch (error) { if (sameProjectView(current(), context) && requests.accepts(pending)) status.textContent = String(error); }
    finally { if (sameProjectView(current(), context)) { requests.finish(pending); setBusy(stop, false); stop.disabled = false; } }
  });
  const dialog = document.getElementById('removal-dialog') as HTMLDialogElement;
  const confirmation = document.getElementById('removal-confirmation') as HTMLInputElement;
  const confirm = document.getElementById('confirm-removal') as HTMLButtonElement;
  const review = new RemovalReview<{ preview: Preview; context: ProjectContext; all: boolean }>();
  const previewButtons = ['preview-removal', 'preview-project-removal']
    .map(id => document.getElementById(id) as HTMLButtonElement | null)
    .filter((button): button is HTMLButtonElement => button !== null);
  for (const previewButton of previewButtons) previewButton.addEventListener('click', async () => {
    const context = current(); if (!context || previewButton.disabled) return;
    const all = previewButton.id === 'preview-project-removal';
    const pending = requests.begin();
    for (const button of previewButtons) button.disabled = true;
    setBusy(previewButton, true);
    review.read(pending);
    status.textContent = 'Preparing removal preview…';
    try {
      const result = await invoke<Preview>('preview_removal', { identifier: context.id, ...(all ? {} : { environmentId: context.environmentId }) });
      if (!sameProjectView(current(), context) || !requests.accepts(pending)) return;
      if (!review.review(pending, { preview: result, context, all }, result.reference)) return;
      const preview = review.current()!.preview;
      status.textContent = '';
      document.getElementById('removal-description')!.textContent = preview.description;
      document.getElementById('removal-reference')!.textContent = preview.reference;
      document.getElementById('removal-workspace')!.textContent = preview.workspace;
      document.getElementById('removal-external')!.hidden = !preview.external;
      confirmation.value = ''; confirm.disabled = true;
      document.getElementById('removal-error')!.textContent = '';
      showDialog(dialog, previewButton.id); confirmation.focus();
    } catch (error) { if (sameProjectView(current(), context) && requests.accepts(pending)) status.textContent = String(error); }
    finally { if (sameProjectView(current(), context)) requests.finish(pending); if (sameProjectView(current(), context)) for (const button of previewButtons) { button.disabled = false; setBusy(button, false); } }
  });
  confirmation.addEventListener('input', () => { confirm.disabled = !review.confirms(confirmation.value); });
  document.getElementById('cancel-removal')!.addEventListener('click', () => { dialog.close(); });
  dialog.addEventListener('close', () => {
    if (dialog.open) return;
    review.reset();
    confirmation.value = '';
    confirm.disabled = true;
    setBusy(confirm, false);
  });
  confirm.addEventListener('click', async () => {
    if (!dialog.open) return;
    const snapshot = review.current();
    if (!snapshot || !sameProjectView(current(), snapshot.context)) return;
    const removal = review.remove(confirmation.value);
    if (!removal) return;
    const { preview, context, all } = removal;
    const pending = requests.begin();
    confirm.disabled = true;
    setBusy(confirm, true);
    try {
      const message = await invoke<string>('remove_project', { identifier: context.id, ...(all ? {} : { environmentId: context.environmentId }), fingerprint: preview.fingerprint, confirmation: confirmation.value, logout: preview.logout ?? false });
      review.completed(removal);
      if (sameProjectView(current(), context) && requests.accepts(pending)) dialog.close();
      if (sameProject(current(), context)) await removed(message);
    } catch (error) {
      if (sameProjectView(current(), context) && requests.accepts(pending) && dialog.open) {
        review.retry(removal);
        document.getElementById('removal-error')!.textContent = String(error);
      }
    }
    finally {
      if (sameProjectView(current(), context) && requests.accepts(pending)) {
        requests.finish(pending);
        setBusy(confirm, false);
        confirm.disabled = !dialog.open || !review.confirms(confirmation.value);
      }
    }
  });
  return () => {
    requests.invalidate(); review.reset();
    status.textContent = ''; stop.disabled = false; confirm.disabled = true; confirmation.value = '';
    for (const button of [stop, ...previewButtons, confirm]) setBusy(button, false);
    for (const button of previewButtons) button.disabled = false;
    for (const id of ['removal-description', 'removal-reference', 'removal-workspace', 'removal-error']) document.getElementById(id)!.textContent = '';
    document.getElementById('removal-external')!.hidden = true;
    if (dialog.open) dialog.close();
  };
}

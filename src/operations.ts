import { invoke } from '@tauri-apps/api/core';
import { sameProject, sameProjectView, type Project, type ProjectContext } from './projects';

interface Preview { reference: string; workspace: string; description: string; fingerprint: string; external: boolean; hasEnvironment: boolean; logout?: boolean | null }

export function initializeOperations(current: () => ProjectContext | null, open: (command: string, arguments_: Record<string, unknown>, caption: string) => Promise<void>, stopped: (project: Project) => void, removed: (message: string) => Promise<void>): () => void {
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
    const context = current(); if (!context) return;
    const pending = ++request;
    stop.disabled = true; status.textContent = 'Stopping Brig environment…';
    try {
      const project = await invoke<Project>('stop_project', { identifier: context.id });
      if (sameProject(current(), context)) stopped(project);
      if (sameProjectView(current(), context) && pending === request) status.textContent = 'Environment stopped. Guest home and conversations are preserved.';
    } catch (error) { if (sameProjectView(current(), context) && pending === request) status.textContent = String(error); }
    finally { if (sameProjectView(current(), context)) stop.disabled = false; }
  });
  const dialog = document.getElementById('removal-dialog') as HTMLDialogElement;
  const confirmation = document.getElementById('removal-confirmation') as HTMLInputElement;
  const confirm = document.getElementById('confirm-removal') as HTMLButtonElement;
  let preview: Preview | null = null;
  let removingId: string | null = null;
  let previewContext: ProjectContext | null = null;
  document.getElementById('preview-removal')!.addEventListener('click', async () => {
    const context = current(); if (!context) return;
    const pending = ++request;
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
      (document.getElementById('logout-on-removal') as HTMLInputElement).checked = preview.logout ?? false;
      (document.getElementById('logout-on-removal') as HTMLInputElement).disabled = !preview.hasEnvironment || preview.logout != null;
      dialog.showModal(); confirmation.focus();
    } catch (error) { if (sameProjectView(current(), context) && pending === request) status.textContent = String(error); }
  });
  confirmation.addEventListener('input', () => { confirm.disabled = confirmation.value !== preview?.reference; });
  document.getElementById('cancel-removal')!.addEventListener('click', () => { dialog.close(); document.getElementById('preview-removal')!.focus(); });
  confirm.addEventListener('click', async () => {
    if (!preview || !removingId || !previewContext || !sameProjectView(current(), previewContext) || confirmation.value !== preview.reference) return;
    const context = previewContext;
    const pending = ++request;
    confirm.disabled = true;
    try {
      const message = await invoke<string>('remove_project', { identifier: removingId, fingerprint: preview.fingerprint, confirmation: confirmation.value, logout: (document.getElementById('logout-on-removal') as HTMLInputElement).checked });
      if (sameProjectView(current(), context)) dialog.close();
      if (sameProject(current(), context)) await removed(message);
    } catch (error) { if (sameProjectView(current(), context) && pending === request) document.getElementById('removal-error')!.textContent = String(error); }
    finally { if (sameProjectView(current(), context) && pending === request) confirm.disabled = false; }
  });
  return () => {
    ++request; preview = null; removingId = null; previewContext = null;
    status.textContent = ''; stop.disabled = false; confirm.disabled = true; confirmation.value = '';
    for (const id of ['removal-description', 'removal-reference', 'removal-workspace', 'removal-error']) document.getElementById(id)!.textContent = '';
    document.getElementById('removal-external')!.hidden = true;
    const logout = document.getElementById('logout-on-removal') as HTMLInputElement;
    logout.checked = false; logout.disabled = true;
    if (dialog.open) dialog.close();
  };
}

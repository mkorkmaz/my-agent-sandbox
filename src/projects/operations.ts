import { invoke } from '@tauri-apps/api/core';
import { showDialog } from '../ui/dialogs';
import { conversationKey, isChat, sameProject, sameProjectView, terminalMatches, terminalNames, type Conversation, type Project, type ProjectContext, type TerminalStatus } from './projects';
import { setBusy } from '../ui/loading';
import { RequestState } from '../ui/request-state';
import { RemovalReview } from './removal-review';
import type { OpenTerminal } from '../terminal/terminal-launch';

interface Preview { reference: string; workspace: string; description: string; fingerprint: string; external: boolean }

export function renderStopImpact(project: Project | null, terminals: TerminalStatus[], conversations: Conversation[]): void {
  const guestPurposes = ['environment', 'login', 'codex', 'resume', 'claude', 'claude-resume', 'shell', 'git-status', 'git-diff', 'git-staged-diff', 'tests'];
  const affected = terminals.filter(terminal => project && terminal.running && terminalMatches(terminal, project) && guestPurposes.includes(terminal.purpose));
  const list = document.getElementById('stop-impact-list')!;
  list.replaceChildren(...affected.map(terminal => {
    const item = document.createElement('li');
    const key = isChat(terminal) && terminal.conversationId ? conversationKey({ ...terminal, id: terminal.conversationId }) : null;
    const title = key ? conversations.find(conversation => conversationKey(conversation) === key)?.title : null;
    item.textContent = [terminalNames[terminal.purpose], title, 'Terminal ' + terminal.identifier].filter(Boolean).join(' · ');
    return item;
  }));
  document.getElementById('stop-impact')!.hidden = !affected.length;
}

function requiresNewRemovalReview(error: unknown): error is { currentReference: string; currentDescription: string } {
  return typeof error === 'object' && error !== null
    && 'currentReference' in error && typeof error.currentReference === 'string'
    && 'currentDescription' in error && typeof error.currentDescription === 'string';
}

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
  const cancel = document.getElementById('cancel-removal')!;
  const errorMessage = document.getElementById('removal-error')!;
  const review = new RemovalReview<{ preview: Preview; context: ProjectContext }>();
  const previewButton = document.getElementById('preview-removal') as HTMLButtonElement;
  previewButton.addEventListener('click', async () => {
    const context = current(); if (!context || previewButton.disabled) return;
    const pending = requests.begin();
    previewButton.disabled = true;
    setBusy(previewButton, true);
    review.read(pending);
    status.textContent = 'Preparing removal preview…';
    try {
      const result = await invoke<Preview>('preview_removal', { identifier: context.id, environmentId: context.environmentId });
      if (!sameProjectView(current(), context) || !requests.accepts(pending)) return;
      if (!review.review(pending, { preview: result, context }, result.reference)) return;
      const preview = review.current()!.preview;
      status.textContent = '';
      document.getElementById('removal-description')!.textContent = preview.description;
      document.getElementById('removal-reference')!.textContent = preview.reference;
      document.getElementById('removal-workspace')!.textContent = preview.workspace;
      document.getElementById('removal-external')!.hidden = !preview.external;
      confirmation.value = ''; confirmation.disabled = false; confirm.disabled = true;
      errorMessage.textContent = '';
      showDialog(dialog, previewButton.id); confirmation.focus();
    } catch (error) { if (sameProjectView(current(), context) && requests.accepts(pending)) status.textContent = String(error); }
    finally { if (sameProjectView(current(), context)) { requests.finish(pending); previewButton.disabled = false; setBusy(previewButton, false); } }
  });
  confirmation.addEventListener('input', () => { confirm.disabled = !review.confirms(confirmation.value); });
  cancel.addEventListener('click', () => { dialog.close(); });
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
    const { preview, context } = removal;
    const pending = requests.begin();
    confirm.disabled = true;
    setBusy(confirm, true);
    try {
      const message = await invoke<string>('remove_project', { identifier: context.id, environmentId: context.environmentId, fingerprint: preview.fingerprint, confirmation: confirmation.value });
      review.completed(removal);
      if (sameProjectView(current(), context) && requests.accepts(pending)) dialog.close();
      if (sameProject(current(), context)) await removed(message);
    } catch (error) {
      if (!sameProjectView(current(), context) || !requests.accepts(pending) || !dialog.open) return;
      if (requiresNewRemovalReview(error)) {
        review.reset();
        confirmation.value = '';
        confirmation.disabled = true;
        document.getElementById('removal-reference')!.textContent = error.currentReference;
        document.getElementById('removal-description')!.textContent = error.currentDescription;
        document.getElementById('removal-workspace')!.textContent = '';
        document.getElementById('removal-external')!.hidden = true;
        errorMessage.textContent = 'The removal scope changed. Close this dialog and open a fresh preview to review the current scope.';
        cancel.focus();
        return;
      }
      review.retry(removal);
      errorMessage.textContent = String(error);
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
    for (const button of [stop, previewButton, confirm]) setBusy(button, false);
    previewButton.disabled = false;
    for (const id of ['removal-description', 'removal-reference', 'removal-workspace', 'removal-error']) document.getElementById(id)!.textContent = '';
    document.getElementById('removal-external')!.hidden = true;
    if (dialog.open) dialog.close();
  };
}

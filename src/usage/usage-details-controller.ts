import { invoke } from '@tauri-apps/api/core';
import { renderUsageDetails, usageButtonId, usageDetailsStatus } from '../dashboard/dashboard';
import { showDialog } from '../ui/dialogs';
import { renderSkeleton, setBusy, setLoading } from '../ui/loading';
import { conversationKey, sameProject, type Conversation, type ProjectContext } from '../projects/projects';
import { UsageDetailsState } from './usage-details-state';
import type { ConversationUsageDetails } from './usage-types';

interface UsageDetailsContext {
  currentProject(): ProjectContext | null;
  conversations(): Conversation[];
  fingerprint(sessionId: string, environmentId: string | null): string;
  updateTerminalLayout(): Promise<void>;
}

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error('Missing interface element ' + id);
  return found as T;
}

export function initializeUsageDetails(context: UsageDetailsContext) {
  const state = new UsageDetailsState();
  const dialog = element<HTMLDialogElement>('session-usage-dialog');
  const title = element('session-usage-title');
  const content = element('session-usage-details');
  const status = element('session-usage-status');
  const closeButton = element('close-session-usage');

  function active(project: ProjectContext, sessionId: string, revision: number): boolean {
    return sameProject(context.currentProject(), project) && dialog.open && state.accepts(sessionId, revision);
  }

  async function load(project: ProjectContext, sessionId: string, revision: number): Promise<void> {
    if (!active(project, sessionId, revision)) return;
    const request = state.begin();
    if (!request) return;
    const { fingerprint, environmentId, details: previous } = request;
    setLoading(content, previous === null);
    setBusy(status, previous !== null);
    if (previous) status.textContent = usageDetailsStatus(previous) + ' Updating recorded usage…';
    try {
      const details = await invoke<ConversationUsageDetails>('conversation_usage', { identifier: project.id, environmentId, sessionId });
      if (!active(project, sessionId, revision) || !state.unchanged(fingerprint)) return;
      if (!state.show(request, details)) return;
      renderUsageDetails(details);
      status.textContent = usageDetailsStatus(details);
    } catch (error) {
      if (!active(project, sessionId, revision) || !state.unchanged(fingerprint)) return;
      state.fail(request);
      status.textContent = previous ? usageDetailsStatus(previous) + ' Could not refresh usage. Showing the last recorded data. Close and reopen usage to retry. ' + String(error) : String(error);
    } finally {
      finish(project, request);
    }
  }

  function finish(project: ProjectContext, request: NonNullable<ReturnType<UsageDetailsState['begin']>>): void {
    if (!active(project, request.sessionId, request.revision)) return;
    state.finish(request);
    if (!state.unchanged(request.fingerprint)) {
      void load(project, request.sessionId, request.revision);
      return;
    }
    setLoading(content, false);
    setBusy(status, false);
    if (!state.details()) content.replaceChildren();
  }

  function refresh(): void {
    const selection = state.current();
    const project = context.currentProject();
    if (!dialog.open || !selection || !project) return;
    const { sessionId, environmentId, revision } = selection;
    if (!state.refresh(context.fingerprint(sessionId, environmentId))) return;
    const selected = context.conversations().find(conversation => conversation.id === sessionId && (conversation.environmentId ?? null) === environmentId);
    if (selected) title.textContent = selected.title;
    void load(project, sessionId, revision);
  }

  async function open(conversation: Conversation): Promise<void> {
    const project = context.currentProject();
    if (!project) return;
    const sessionId = conversation.id;
    const environmentId = conversation.environmentId ?? null;
    const revision = state.open(project.id, sessionId, environmentId, context.fingerprint(sessionId, environmentId));
    title.textContent = conversation.title;
    const cached = state.details();
    if (cached) {
      renderUsageDetails(cached);
      status.textContent = usageDetailsStatus(cached);
    }
    if (!cached) {
      renderSkeleton(content);
      setLoading(content, true);
      content.removeAttribute('data-usage-session-id');
      status.textContent = 'Loading session usage…';
    }
    showDialog(dialog, usageButtonId(conversationKey(conversation)));
    closeButton.focus();
    await context.updateTerminalLayout();
    if (!cached) await load(project, sessionId, revision);
  }

  closeButton.addEventListener('click', () => { dialog.close(); });
  dialog.addEventListener('close', () => {
    void context.updateTerminalLayout();
    if (dialog.open) return;
    state.close();
    title.textContent = '';
    content.replaceChildren();
    status.textContent = '';
    setLoading(content, false);
    setBusy(status, false);
  });

  return { open, refresh, clearCache: () => state.clearCache() };
}

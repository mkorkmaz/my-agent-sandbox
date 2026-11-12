import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './style.css';
import { initializeHost } from './host';
import { initializeSettings } from './settings';
import { renderProjects, renderConversations, renderTerminals, isStandaloneConversation, standaloneConversations, terminalCaption, terminalNames, sameProject, sameProjectView, type Conversation, type Project, type ProjectContext, type TerminalStatus } from './projects';
import { prepareProject, renderPlan, type Plan } from './preparation';
import { initializeBoundaryTests, resetBoundaryChoice, useBrigBoundary } from './boundaries';
import { initializeOperations } from './operations';
import { initializeProjectPolicy } from './project-policy';
import { renderDashboard, renderUsageDetails, usageButtonId, usageDetailsStatus, emptyUsage, type Dashboard, type SessionState } from './dashboard';
import { showDialog } from './dialogs';
import type { ConversationUsageDetails } from './usage-types';
import { initializeSidebar } from './sidebar';
import { initializeDataLocation } from './data-location';
import { initializeChatgptAccount, type ChatgptAccount, type ChatgptAccountStatus } from './chatgpt-account';
import { renderSkeleton, setBusy, setLoading } from './loading';

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error('Missing interface element ' + id);
  return found as T;
}

const projectView = element('project-view');
const settingsView = element('settings-view');
const projectNavigation = element<HTMLButtonElement>('project-navigation');
const settingsNavigation = element<HTMLButtonElement>('settings-navigation');
const chooseFolderButton = element<HTMLButtonElement>('choose-folder');
const projectSettingsDialog = element<HTMLDialogElement>('project-settings-dialog');
const usageDialog = element<HTMLDialogElement>('session-usage-dialog');
const folderError = element('folder-error');
let choosingFolder = false;
let selectingProject = false;
let selectedProject: Project | null = null;
let projectEpoch = 0;
let projectSettingsEpoch = 0;
let projectPreferencesRequest = 0;
let sharedChatgpt: ChatgptAccount | null = null;
let accountSyncKey: string | null = null;
let accountConnectionError: string | null = null;
let syncingAccount = false;
let connectedAccountProject: string | null = null;
let usageRequest = 0;
let usageSessionId: string | null = null;
let usageFingerprint: string | null = null;
let usagePending = false;
let usageDetails: ConversationUsageDetails | null = null;
let conversationStateRequest = 0;
let resetPolicyDetails = () => {};
let resetBoundaryDetails = () => {};
let resetOperationDetails = () => {};
let terminalIdentifier: number | null = null;
let startingTerminal = false;
let pendingEnvironmentProject: string | null = null;
const environmentTerminals = new Map<string, number>();
const closingTerminals = new Set<number>();
let preparingProject: string | null = null;
let selectedPlanFingerprint: string | null = null;
let planLoading = false;
let planRequest = 0;
let planRetryAction: 'prepare' | 'preview' | 'environment' = 'prepare';
let projects: Project[] = [];
let projectsLoaded = false;
let conversations: Conversation[] = [];
let dashboard: Dashboard = { conversations: [], usage: emptyUsage, coverage: 'unknown', pricingDate: '2026-11-10', warnings: [] };
let sessionView: SessionState = 'active';
let changingSession = false;
let dashboardLoaded = false;
let dashboardError: string | null = null;
let selectedConversationId: string | null = null;
let terminals: TerminalStatus[] = [];
const lastTerminals = new Map<string, number>();
let terminalListRequest = 0;
let terminalViewRequest = 0;
let pendingLayout = false;
let layoutWork: Promise<void> | null = null;
let projectListRequest = 0;
let rescanningProjects = false;
let projectRescanStarted = false;
let projectRescanError: string | null = null;
let projectRescanTimer: number | null = null;
let conversationListRequest = 0;
let dashboardUpdates = 0;
let dashboardSequence = 0;
let loadingConversations = false;
const terminalPanel = element('terminal-panel');
const terminalSurface = element('terminal-surface');
const startTerminalButton = element<HTMLButtonElement>('start-terminal');
renderSkeleton(element('recent-project-list'), 3);
renderSkeleton(element('sidebar-project-list'));
element('recent-projects').hidden = false;
setLoading(element('recent-projects'), true);
setLoading(element('sidebar-project-list'), true);

function projectContext(): ProjectContext | null { return selectedProject ? { id: selectedProject.id, fingerprint: selectedPlanFingerprint, epoch: projectEpoch, view: projectSettingsEpoch } : null; }
function resetProjectDetails(): void {
  ++projectSettingsEpoch; ++projectPreferencesRequest;
  element('guest-terminal-status').textContent = '';
  setBusy(element('save-project-preferences'), false);
  setLoading(element('project-preference-fields'), false);
  element('project-preferences-status').textContent = '';
  element('local-terminal-status').textContent = '';
  element<HTMLFieldSetElement>('project-preference-fields').disabled = true;
  for (const id of ['project-name', 'project-cpus', 'project-memory']) element<HTMLInputElement>(id).value = '';
  for (const id of ['project-verification', 'project-network']) element<HTMLSelectElement>(id).value = '';
  resetPolicyDetails(); resetBoundaryDetails(); resetOperationDetails();
}

function isChat(status: TerminalStatus): boolean { return ['codex', 'resume'].includes(status.purpose); }
function environmentReady(): boolean { return selectedProject?.stage === 'environment-ready' && selectedPlanFingerprint !== null && !planLoading; }
function canStartChat(): boolean {
  const context = projectContext();
  return environmentReady() && !syncingAccount && sharedChatgpt?.status()?.state === 'signed-in'
    && connectedAccountProject === [context?.id, context?.epoch, context?.fingerprint].join('|');
}

async function syncProjectAccount(): Promise<void> {
  const context = projectContext();
  if (!context || !sharedChatgpt || !environmentReady() || syncingAccount) return;
  const key = [context.id, context.epoch, context.fingerprint, sharedChatgpt.status()?.generation].join('|');
  if (accountSyncKey === key) return;
  accountSyncKey = key; syncingAccount = true; connectedAccountProject = null;
  accountConnectionError = null;
  element<HTMLButtonElement>('start-codex').disabled = true;
  updateCodexActions();
  try {
    const status = await invoke<ChatgptAccountStatus>('sync_project_chatgpt', { identifier: context.id });
    if (!sameProject(projectContext(), context)) return;
    accountSyncKey = [context.id, context.epoch, context.fingerprint, status.generation].join('|');
    connectedAccountProject = status.state === 'signed-in' ? [context.id, context.epoch, context.fingerprint].join('|') : null;
    sharedChatgpt.accept(status);
  } catch (error) {
    if (sameProject(projectContext(), context)) {
      accountConnectionError = String(error);
      sharedChatgpt.connectionFailed(String(error));
    }
  }
  finally { syncingAccount = false; updateCodexActions(); }
}

function planFeedback(message: string | null, retry = false, busy = false): void {
  element('project-plan-feedback').hidden = message === null;
  element('project-plan-status').textContent = message ?? '';
  setBusy(element('project-plan-status'), busy);
  setBusy(element('preparation-status'), busy);
  element('retry-project-plan').hidden = !retry;
  element('retry-project-plan').textContent = planRetryAction === 'environment' ? 'Retry environment' : 'Retry setup';
}

function environmentFeedback(message: string | null = null): void {
  if (selectedProject?.stage === 'environment-ready') { planFeedback(null); return; }
  if (message) { planRetryAction = 'environment'; planFeedback(message, true); return; }
  if (selectedProject?.stage === 'environment-starting') {
    planFeedback('Starting the project environment with your saved settings…', false, true); return;
  }
  const error = selectedProject?.environmentAttempt?.error;
  if (error) { planRetryAction = 'environment'; planFeedback(error, true); return; }
  if (selectedProject?.stage === 'ready' && selectedPlanFingerprint) {
    planRetryAction = 'environment'; planFeedback('The project environment is stopped.', true);
    element('retry-project-plan').textContent = 'Start environment';
  }
}

async function ensureSelectedEnvironment(): Promise<void> {
  if (!selectedProject || !selectedPlanFingerprint || !['ready', 'environment-starting'].includes(selectedProject.stage) || selectedProject.policyDetached) return;
  if (terminals.some(status => status.running && ['codex', 'resume', 'login', 'shell'].includes(status.purpose))) {
    planFeedback('End running sessions to apply the saved environment settings.'); return;
  }
  planFeedback('Starting the project environment with your saved settings…', false, true);
  await openTerminal('prepare_environment', { identifier: selectedProject.id, fingerprint: selectedPlanFingerprint }, 'Starting project environment…', false);
}

function currentTerminal(): TerminalStatus | null {
  const remembered = selectedProject ? lastTerminals.get(selectedProject.id) : null;
  return terminals.find(status => status.identifier === (terminalIdentifier ?? remembered))
    ?? terminals.find(status => status.running && isChat(status))
    ?? terminals.find(status => status.running)
    ?? terminals.at(-1) ?? null;
}

function chatConversationId(status: TerminalStatus): string | null {
  const conversation = conversations.find(conversation => conversation.id === status.conversationId);
  if (!conversation) return status.conversationId;
  if (conversation.relationStatus === 'resolved') return conversations.find(root => root.id === conversation.rootSessionId && isStandaloneConversation(root))?.id ?? null;
  return isStandaloneConversation(conversation) ? conversation.id : null;
}

function renderSessionSidebar(): void {
  const chats = terminals.filter(status => status.running && isChat(status));
  const rows = standaloneConversations(conversations, 'active').map(conversation => {
    const terminal = chats.find(status => chatConversationId(status) === conversation.id);
    return { ...conversation, available: terminal ? true : conversation.available, terminalId: terminal?.identifier, busy: terminal?.busy };
  });
  for (const terminal of chats) {
    if (rows.some(row => row.terminalId === terminal.identifier)) continue;
    const conversationId = chatConversationId(terminal);
    const saved = conversations.find(conversation => conversation.id === conversationId);
    rows.unshift({ ...(saved ?? { id: conversationId ?? 'terminal-' + terminal.identifier, title: (terminal.conversationId && !conversationId ? 'Codex process ' : 'New Chat ') + terminal.identifier, updatedAt: Date.now(), state: 'active' as const, available: true, usage: emptyUsage, warning: null, lineage: { kind: 'unknown' as const, parentSessionId: null, agentPath: null, nickname: null, sourceVersion: null, lineageVersion: 1 as const }, relationStatus: 'unknown' as const, rootSessionId: null, subagentCount: 0, familyUsage: emptyUsage, coverage: 'unknown' as const, usageRevision: 'pending-terminal-' + terminal.identifier }), available: true, terminalId: terminal.identifier, busy: terminal.busy });
  }
  const selected = terminals.find(status => status.identifier === terminalIdentifier);
  selectedConversationId = (selected ? chatConversationId(selected) : null) ?? (selected && isChat(selected) ? 'terminal-' + selected.identifier : null);
  renderConversations(rows, selectedConversationId, resumeConversation);
  renderTerminals(terminals, terminalIdentifier, returnToTerminal);
  if (chats.length) element('session-list-status').hidden = true;
}

async function refreshTerminals(): Promise<void> {
  if (!selectedProject) return;
  const projectId = selectedProject.id;
  const request = ++terminalListRequest;
  const result = await invoke<TerminalStatus[]>('terminal_sessions', { projectId });
  if (selectedProject?.id !== projectId || request !== terminalListRequest) return;
  terminals = result;
  if (terminalIdentifier !== null && !terminals.some(status => status.identifier === terminalIdentifier)) {
    terminalIdentifier = null; terminalPanel.hidden = true;
    showFolderError('This terminal output is no longer retained. Reopen another result from Terminals or run the operation again.');
  }
  renderSessionSidebar();
  updateTerminalActions(currentTerminal());
  void syncMenus();
}

function updateTerminalActions(status: TerminalStatus | null): void {
  const button = element<HTMLButtonElement>('return-to-terminal');
  button.hidden = status === null || (!status.running && status.purpose === 'environment');
  button.textContent = status && ['codex', 'resume'].includes(status.purpose) ? 'Return to chat' : 'Return to terminal';
  button.className = status?.running ? 'primary-button' : 'secondary-button';
  const terminalRunning = terminals.some(terminal => terminal.running);
  element('close-terminal').textContent = terminals.find(terminal => terminal.identifier === terminalIdentifier)?.running ? 'End process' : 'Close output';
  const closing = terminalIdentifier !== null && closingTerminals.has(terminalIdentifier);
  element<HTMLButtonElement>('close-terminal').disabled = closing;
  setBusy(element('close-terminal'), closing);
  projectView.classList.toggle('has-live-terminal', terminalRunning);
  element<HTMLButtonElement>('start-codex').disabled = !canStartChat() || startingTerminal || selectingProject || choosingFolder || preparingProject !== null;
  element<HTMLButtonElement>('start-codex').title = 'New Chat';
}

async function syncMenus(): Promise<void> {
  const busy = choosingFolder || selectingProject || preparingProject !== null || startingTerminal;
  chooseFolderButton.disabled = busy;
  element<HTMLButtonElement>('start-codex').disabled = busy || !canStartChat();
  element<HTMLButtonElement>('guest-shell').disabled = busy || !environmentReady();
  projectNavigation.disabled = busy;
  element<HTMLButtonElement>('refresh-sessions').disabled = busy || loadingConversations;
  setBusy(element('refresh-sessions'), loadingConversations);
  setBusy(chooseFolderButton, choosingFolder);
  setBusy(element('terminal-status'), startingTerminal);
  for (const button of document.querySelectorAll<HTMLButtonElement>('.sidebar-project, .sidebar-session, .recent-project, .project-record-forget')) button.disabled = busy || button.dataset.available === 'false';
  renderSessionDashboard();
  scheduleProjectRescan();
  try { await invoke('update_menu_state', { projectId: selectedProject?.id ?? null, busy }); }
  catch { showFolderError('Could not update the native menu. Reopen the app.'); }
}

function terminalLayout(): { x: number; y: number; width: number; height: number; visible: boolean } {
  const rectangle = terminalSurface.getBoundingClientRect();
  return { x: rectangle.x, y: rectangle.y, width: rectangle.width, height: rectangle.height, visible: !projectView.hidden && !terminalPanel.hidden && !document.querySelector('dialog[open]') };
}

function updateTerminalLayout(): Promise<void> {
  pendingLayout = true;
  if (layoutWork) return layoutWork;
  layoutWork = (async () => {
    while (pendingLayout) {
      pendingLayout = false;
      if (terminalIdentifier !== null) await invoke('terminal_layout', { identifier: terminalIdentifier, layout: terminalLayout() });
    }
  })().catch(error => { showFolderError(String(error)); }).finally(() => { layoutWork = null; });
  return layoutWork;
}

function showFolderError(message: string): void { folderError.textContent = message; folderError.hidden = false; }

function showTerminalStatus(message: string): void {
  element('terminal-status').textContent = message;
  element('terminal-status').title = message;
}

function renderProjectLists(): void {
  renderProjects(projects, selectedProject, project => selectProject(project, true), showForgetProject);
  setLoading(element('recent-projects'), !projectsLoaded);
  setLoading(element('sidebar-project-list'), !projectsLoaded);
  if (!projectsLoaded && !selectedProject) {
    renderSkeleton(element('recent-project-list'), 3);
    renderSkeleton(element('sidebar-project-list'));
    element('recent-projects').hidden = false;
    element('empty-projects').hidden = true;
  }
  renderProjectAvailability();
  scheduleProjectRescan();
}

function renderProjectAvailability(): void {
  const unavailable = projects.some(project => project.available === false);
  projectRescanStatus(projectRescanError ?? (unavailable ? 'Some project folders are unavailable. Connect the drive; Tugboat checks again automatically.' : null));
}

function projectRescanStatus(message: string | null): void {
  const status = element('project-rescan-status');
  status.hidden = selectedProject !== null || message === null;
  if (status.textContent !== (message ?? '')) status.textContent = message ?? '';
}

function scheduleProjectRescan(): void {
  const window = document.defaultView!;
  const needed = projectRescanStarted && !projectView.hidden && selectedProject === null
    && (projectRescanError !== null || projects.some(project => project.available === false));
  const busy = rescanningProjects || choosingFolder || selectingProject || preparingProject !== null || startingTerminal;
  if (!needed || busy) {
    if (projectRescanTimer !== null) window.clearTimeout(projectRescanTimer);
    projectRescanTimer = null;
    return;
  }
  if (projectRescanTimer !== null) return;
  projectRescanTimer = window.setTimeout(() => { projectRescanTimer = null; void rescanProjects(); }, 8000);
}

const forgetProjectDialog = element<HTMLDialogElement>('forget-project-dialog');
let forgettingProject: Project | null = null;
let forgetting = false;
function showForgetProject(project: Project): void {
  forgettingProject = project;
  element('forget-project-name').textContent = project.name + ' · ' + project.path;
  element('forget-project-error').textContent = '';
  showDialog(forgetProjectDialog); element('cancel-forget-project').focus();
  void updateTerminalLayout();
}
element('cancel-forget-project').addEventListener('click', () => { if (!forgetting) forgetProjectDialog.close(); });
forgetProjectDialog.addEventListener('cancel', event => { if (forgetting) event.preventDefault(); });
forgetProjectDialog.addEventListener('close', () => {
  const id = forgettingProject?.id; forgettingProject = null;
  const button = [...document.querySelectorAll<HTMLButtonElement>('.project-record-forget')].find(button => button.dataset.projectId === id);
  (button ?? chooseFolderButton).focus(); void updateTerminalLayout();
});
element('confirm-forget-project').addEventListener('click', async () => {
  if (!forgettingProject || forgetting) return;
  const id = forgettingProject.id; forgetting = true;
  const confirm = element<HTMLButtonElement>('confirm-forget-project'); confirm.disabled = true;
  setBusy(confirm, true);
  try {
    await invoke('forget_project', { identifier: id });
    projects = projects.filter(project => project.id !== id); renderProjectLists();
    forgetProjectDialog.close(); await refreshProjects();
  } catch (error) { element('forget-project-error').textContent = String(error); }
  finally { forgetting = false; setBusy(confirm, false); confirm.disabled = false; }
});

async function refreshProjects(): Promise<void> {
  const request = ++projectListRequest;
  try {
    const result = await invoke<Project[]>('recent_projects');
    if (request !== projectListRequest) return;
    projectsLoaded = true;
    projects = result;
    renderProjectLists();
  } catch (error) { if (request === projectListRequest) { projectsLoaded = true; renderProjectLists(); element('empty-projects').hidden = true; showFolderError(String(error)); } }
}

async function rescanProjects(): Promise<void> {
  if (rescanningProjects || selectedProject || choosingFolder || selectingProject || preparingProject !== null || startingTerminal) return;
  rescanningProjects = true;
  const initial = !projectRescanStarted;
  if (initial) {
    element('empty-projects').hidden = true;
    projectRescanStatus('Checking project folders…');
  }
  projectRescanStarted = true;
  const request = ++projectListRequest;
  void syncMenus();
  try {
    const result = await invoke<Project[]>('rescan_projects');
    if (request !== projectListRequest) return;
    const changed = initial || projectRescanError !== null || JSON.stringify(projects) !== JSON.stringify(result);
    projectRescanError = null;
    projectsLoaded = true;
    projects = result;
    if (changed) renderProjectLists();
    if (!changed) renderProjectAvailability();
  } catch (error) {
    if (request === projectListRequest) {
      projectsLoaded = true;
      projectRescanError = 'Could not check project folders. Tugboat will try again automatically. ' + String(error);
      renderProjectLists();
      element('empty-projects').hidden = true;
    }
  } finally {
    rescanningProjects = false;
    if (request !== projectListRequest) void refreshProjects();
    void syncMenus();
  }
}

async function refreshConversations(): Promise<void> {
  if (!selectedProject) return;
  const projectId = selectedProject.id;
  const request = ++conversationListRequest;
  const initialUpdates = dashboardUpdates;
  loadingConversations = true;
  dashboardError = null;
  renderSessionDashboard();
  const status = element('session-list-status');
  status.hidden = false;
  status.textContent = 'Loading sessions…';
  element<HTMLButtonElement>('refresh-sessions').disabled = true;
  setBusy(element('refresh-sessions'), true);
  try {
    const result = await invoke<Dashboard>('project_dashboard', { identifier: projectId });
    if (request !== conversationListRequest || selectedProject?.id !== projectId) return;
    if (result.sequence === undefined && initialUpdates !== dashboardUpdates) return;
    if (!applyDashboard(result)) return;
    const hasActive = standaloneConversations(conversations, 'active').length > 0 || terminals.some(terminal => terminal.running && isChat(terminal));
    status.textContent = hasActive ? '' : 'No active sessions. Start a New Chat or restore an archived session.';
    status.hidden = hasActive;
  } catch (error) {
    if (request === conversationListRequest && selectedProject?.id === projectId && initialUpdates === dashboardUpdates) {
      dashboardError = String(error);
      status.textContent = dashboardError;
    }
  } finally {
    if (request === conversationListRequest) {
      loadingConversations = false;
      setBusy(element('refresh-sessions'), false);
      renderSessionDashboard();
      element<HTMLButtonElement>('refresh-sessions').disabled = startingTerminal || selectingProject || preparingProject !== null;
    }
  }
}

function acceptDashboardUpdate(sequence?: number): boolean {
  if (sequence !== undefined) {
    if (sequence <= dashboardSequence) return false;
    dashboardSequence = sequence;
  }
  dashboardUpdates++;
  return true;
}
function applyDashboard(result: Dashboard): boolean {
  if (!acceptDashboardUpdate(result.sequence)) return false;
  dashboard = result;
  dashboardLoaded = true;
  dashboardError = null;
  conversations = result.conversations;
  renderSessionSidebar();
  if (selectedConversationId && !terminalPanel.hidden) {
    const current = conversations.find(conversation => conversation.id === selectedConversationId);
    if (current) showTerminalStatus(current.title);
  }
  renderSessionDashboard();
  refreshOpenUsageDetails();
  return true;
}
function renderSessionDashboard(): void {
  renderDashboard(dashboard, sessionView, resumeConversation, changeConversationState, showUsageDetails, changingSession || startingTerminal || selectingProject || choosingFolder || preparingProject !== null);
  const loading = !dashboardLoaded && !dashboardError;
  const liveChat = terminals.some(terminal => terminal.running && isChat(terminal));
  setLoading(element('project-usage'), loading);
  setLoading(element('session-table'), loading);
  setLoading(element('sidebar-session-list'), loading && !liveChat);
  if (!dashboardLoaded && dashboardError && !liveChat) element('sidebar-session-list').replaceChildren();
  element('project-usage').hidden = !dashboardLoaded && !loading;
  element('usage-note').hidden = !dashboardLoaded;
  if (loading) {
    for (const value of element('project-usage').querySelectorAll('dd')) { value.replaceChildren(); value.setAttribute('aria-hidden', 'true'); }
    renderSkeleton(element('session-table-body'), 7);
    if (!liveChat) renderSkeleton(element('sidebar-session-list'));
    element('session-table').hidden = false;
  }
  element('project-overview').setAttribute('aria-busy', String(loadingConversations || (!dashboardLoaded && !dashboardError)));
  if (!dashboardLoaded || dashboardError) {
    const status = element('dashboard-session-status');
    status.textContent = dashboardError ? dashboardError + ' Select Refresh to try again.' : 'Loading sessions and usage…';
    status.hidden = false;
    if (!dashboardLoaded && !loading) element('session-table').hidden = true;
  }
  setBusy(element('dashboard-session-status'), changingSession);
  if (changingSession) {
    element('dashboard-session-status').textContent = 'Updating session…';
    element('dashboard-session-status').hidden = false;
  }
}
async function changeConversationState(conversation: Conversation, state: SessionState): Promise<void> {
  if (!selectedProject || changingSession || !isStandaloneConversation(conversation)) return;
  const context = projectContext()!;
  const request = ++conversationStateRequest;
  const projectId = context.id;
  changingSession = true; renderSessionDashboard();
  try {
    await invoke('change_conversation_state', { identifier: projectId, sessionId: conversation.id, state });
    if (sameProject(projectContext(), context)) { folderError.hidden = true; await refreshConversations(); }
  } catch (error) { if (sameProject(projectContext(), context)) showFolderError(String(error)); }
  finally { if (request === conversationStateRequest) { changingSession = false; renderSessionDashboard(); if (sameProject(projectContext(), context) && !document.querySelector('dialog[open]')) document.querySelector<HTMLButtonElement>('[data-session-state="' + sessionView + '"]')?.focus(); } }
}
function selectedUsageFingerprint(sessionId: string): string {
  const children = new Map<string, Conversation[]>();
  const nodesById = new Map(conversations.map(conversation => [conversation.id, conversation]));
  for (const conversation of conversations) {
    const parent = conversation.lineage.parentSessionId;
    if (!parent || conversation.lineage.kind !== 'subagent' || conversation.lineage.lineageVersion !== 1 || conversation.lineage.sourceVersion !== '0.167.0' || conversation.relationStatus === 'invalid') continue;
    const parentNode = nodesById.get(parent);
    if (!parentNode || parentNode.relationStatus === 'invalid') continue;
    if (parentNode.rootSessionId && conversation.rootSessionId !== parentNode.rootSessionId) continue;
    const siblings = children.get(parent) ?? []; siblings.push(conversation); children.set(parent, siblings);
  }
  const members = new Set([sessionId]);
  const pending = [sessionId];
  while (pending.length) for (const child of children.get(pending.pop()!) ?? []) {
    if (members.has(child.id)) continue;
    members.add(child.id); pending.push(child.id);
  }
  const nodes = conversations.filter(conversation => members.has(conversation.id)).sort((first, second) => first.id < second.id ? -1 : first.id > second.id ? 1 : 0);
  return JSON.stringify([sessionId, nodes.map(conversation => [conversation.id, conversation.usageRevision, conversation.relationStatus, conversation.rootSessionId, conversation.coverage, conversation.title, conversation.updatedAt, conversation.available, conversation.warning, conversation.subagentCount, conversation.lineage.kind, conversation.lineage.parentSessionId, conversation.lineage.agentPath, conversation.lineage.nickname, conversation.lineage.sourceVersion, conversation.lineage.lineageVersion, conversation.usage, conversation.familyUsage])]);
}

function activeUsage(context: ProjectContext, sessionId: string, request: number): boolean {
  return sameProject(projectContext(), context) && usageDialog.open && request === usageRequest && usageSessionId === sessionId;
}

async function loadUsageDetails(context: ProjectContext, sessionId: string, request: number): Promise<void> {
  if (!activeUsage(context, sessionId, request) || usagePending) return;
  usagePending = true;
  setLoading(element('session-usage-details'), usageDetails === null);
  setBusy(element('session-usage-status'), usageDetails !== null);
  const fingerprint = usageFingerprint;
  if (usageDetails) element('session-usage-status').textContent = usageDetailsStatus(usageDetails) + ' Updating recorded usage…';
  try {
    const details = await invoke<ConversationUsageDetails>('conversation_usage', { identifier: context.id, sessionId });
    if (!activeUsage(context, sessionId, request) || usageFingerprint !== fingerprint) return;
    if (details.sessionId !== sessionId || !details.agents.some(agent => agent.sessionId === sessionId)) throw new Error('The recorded usage response does not match this session. Refresh the session list and try again.');
    renderUsageDetails(details);
    usageDetails = details;
    element('session-usage-status').textContent = usageDetailsStatus(details);
  } catch (error) {
    if (activeUsage(context, sessionId, request) && usageFingerprint === fingerprint) element('session-usage-status').textContent = usageDetails ? usageDetailsStatus(usageDetails) + ' Could not refresh usage. Showing the last recorded data. Close and reopen usage to retry. ' + String(error) : String(error);
  } finally {
    if (activeUsage(context, sessionId, request)) {
      usagePending = false;
      if (usageFingerprint === fingerprint) {
        setLoading(element('session-usage-details'), false);
        setBusy(element('session-usage-status'), false);
        if (!usageDetails) element('session-usage-details').replaceChildren();
      }
      if (usageFingerprint !== fingerprint) void loadUsageDetails(context, sessionId, request);
    }
  }
}

function refreshOpenUsageDetails(): void {
  if (!usageDialog.open || !usageSessionId || !selectedProject) return;
  const fingerprint = selectedUsageFingerprint(usageSessionId);
  if (fingerprint === usageFingerprint) return;
  usageFingerprint = fingerprint;
  const selected = conversations.find(conversation => conversation.id === usageSessionId);
  if (selected) element('session-usage-title').textContent = selected.title;
  void loadUsageDetails(projectContext()!, usageSessionId, usageRequest);
}

async function showUsageDetails(conversation: Conversation): Promise<void> {
  if (!selectedProject) return;
  const context = projectContext()!;
  const sessionId = conversation.id;
  const request = ++usageRequest;
  usageSessionId = sessionId; usageFingerprint = selectedUsageFingerprint(sessionId); usagePending = false; usageDetails = null;
  element('session-usage-title').textContent = conversation.title;
  renderSkeleton(element('session-usage-details'));
  setLoading(element('session-usage-details'), true);
  element('session-usage-details').removeAttribute('data-usage-session-id');
  element('session-usage-status').textContent = 'Loading session usage…';
  showDialog(usageDialog, usageButtonId(sessionId)); element('close-session-usage').focus(); await updateTerminalLayout();
  await loadUsageDetails(context, sessionId, request);
}

async function resumeConversation(conversation: Conversation): Promise<void> {
  if (!selectedProject || startingTerminal || selectingProject || preparingProject !== null || !isStandaloneConversation(conversation)) return;
  const terminal = terminals.find(status => status.running && (status.identifier === conversation.terminalId || chatConversationId(status) === conversation.id));
  if (terminal) {
    showView('project');
    folderError.hidden = true;
    await returnToTerminal(terminal.identifier);
    return;
  }
  const projectId = selectedProject.id;
  showView('project');
  if (selectedProject.stage !== 'environment-ready') {
    showFolderError('Prepare the environment before resuming this session.');
    await showProjectSettings();
    element('prepare-environment').focus();
    return;
  }
  if (!selectedPlanFingerprint) await refreshSelectedPlan();
  if (selectedProject?.id !== projectId || !selectedPlanFingerprint) return;
  await openTerminal('start_codex_session', { identifier: projectId, fingerprint: selectedPlanFingerprint, action: 'resume', sessionId: conversation.id, useBrigBoundary: useBrigBoundary(projectContext()) }, conversation.title);
}

async function selectProject(project: Project, recordOpen = false): Promise<void> {
  if (startingTerminal || selectingProject || preparingProject !== null) { showFolderError('Wait for project setup to finish.'); return; }
  selectingProject = true;
  const triggers = [...document.querySelectorAll<HTMLElement>('[data-project-id]')].filter(button => button.dataset.projectId === project.id);
  for (const trigger of triggers) setBusy(trigger, true);
  void syncMenus();
  try { await applySelectedProject(recordOpen ? await invoke<Project>('open_project', { identifier: project.id }) : project); }
  catch (error) { showFolderError(String(error)); }
  finally { for (const trigger of triggers) setBusy(trigger, false); selectingProject = false; updateCodexActions(); void syncMenus(); }
}

async function applySelectedProject(project: Project): Promise<void> {
  await hideTerminal();
  terminalIdentifier = null;
  selectedProject = project;
  ++projectEpoch; ++terminalViewRequest; ++conversationStateRequest; changingSession = false;
  resetProjectDetails();
  element('preparation-status').textContent = '';
  element('sharing-plan-values').replaceChildren();
  selectedConversationId = null;
  terminals = [];
  ++terminalListRequest;
  conversations = [];
  ++conversationListRequest;
  loadingConversations = false;
  setBusy(element('refresh-sessions'), false);
  dashboardLoaded = false;
  dashboardSequence = 0;
  dashboardError = null;
  dashboard = { conversations: [], usage: emptyUsage, coverage: 'unknown', pricingDate: '2026-11-10', warnings: [] }; sessionView = 'active'; renderSessionDashboard();
  renderConversations([], null, resumeConversation);
  element('session-list-status').textContent = 'Loading sessions…';
  element('session-list-status').hidden = false;
  renderProjectLists();
  projectView.classList.add('has-project');
  selectedPlanFingerprint = null;
  ++planRequest; planLoading = false; planFeedback(null);
  resetBoundaryChoice();
  folderError.hidden = true;
  element('codex-actions').hidden = true;
  element('sharing-plan').hidden = true;
  element('return-to-terminal').hidden = true;
  element('project-heading').textContent = project.name;
  element('project-heading').title = project.name;
  element('project-description').hidden = true;
  element('project-meta').hidden = false;
  chooseFolderButton.hidden = true;
  element('project-settings-button').hidden = false;
  element('project-dashboard').hidden = false;
  const path = element('selected-folder');
  path.textContent = project.path;
  path.title = project.path;
  path.hidden = false;
  startTerminalButton.hidden = false;
  const reference = element('project-reference');
  reference.textContent = project.profile + '@' + project.label;
  reference.hidden = false;
  element('project-actions').hidden = false;
  showView('project');
  await refreshTerminals();
  if (selectedProject.id !== project.id) return;
  const status = currentTerminal();
  if (status?.running) await refreshSelectedPlan();
  updateCodexActions();
  if (!status?.running) await prepareSelectedProject();
  if (selectedProject?.id === project.id) await ensureSelectedEnvironment();
  await refreshProjects();
  void refreshConversations();
}

async function hideTerminal(): Promise<void> {
  terminalPanel.hidden = true;
  await updateTerminalLayout();
}

async function returnToTerminal(identifier?: number): Promise<void> {
  if (!selectedProject) return;
  const projectId = selectedProject.id;
  const request = ++terminalViewRequest;
  closeProjectDialogs();
  await refreshTerminals();
  const status = identifier === undefined ? currentTerminal() : terminals.find(status => status.identifier === identifier);
  if (!status || selectedProject?.id !== projectId || request !== terminalViewRequest) return;
  await hideTerminal();
  if (selectedProject?.id !== projectId || request !== terminalViewRequest) return;
  terminalIdentifier = status.identifier;
  lastTerminals.set(projectId, status.identifier);
  updateTerminalActions(status);
  renderSessionSidebar();
  terminalPanel.hidden = false;
  const title = conversations.find(conversation => conversation.id === selectedConversationId)?.title;
  showTerminalStatus(status.running ? (title ?? terminalNames[status.purpose] ?? 'Operation terminal') : terminalCaption(status));
  await updateTerminalLayout();
  if (!projectView.hidden && request === terminalViewRequest) await invoke('terminal_focus', { identifier: status.identifier });
}

async function refreshSelectedPlan(): Promise<void> {
  if (!selectedProject) return;
  const id = selectedProject.id;
  const request = ++planRequest;
  planLoading = true;
  const previousFingerprint = selectedPlanFingerprint;
  selectedPlanFingerprint = null;
  element('sharing-plan').hidden = true;
  element('prepare-environment').hidden = true;
  planFeedback('Loading the current execution plan…', false, true);
  void syncMenus();
  try {
    const result = await invoke<{ project: Project; plan: Plan; fingerprint: string }>('preview_project', { identifier: id });
    if (selectedProject?.id !== id || request !== planRequest) return;
    if (previousFingerprint !== result.fingerprint) resetBoundaryChoice();
    selectedProject = result.project;
    selectedPlanFingerprint = result.fingerprint;
    planLoading = false;
    planFeedback(null);
    renderPlan(result.plan);
    element('prepare-environment').hidden = false;
    element('refresh-plan').hidden = false;
    updateCodexActions();
    element('preparation-status').textContent = 'Execution plan refreshed.';
    environmentFeedback();
  } catch (error) {
    if (selectedProject?.id !== id || request !== planRequest) return;
    selectedPlanFingerprint = null;
    element('sharing-plan').hidden = true;
    element('refresh-plan').hidden = true;
    element('preparation-status').textContent = String(error);
    planRetryAction = 'preview'; planFeedback(String(error), true);
  } finally { if (request === planRequest) { planLoading = false; updateCodexActions(); void syncMenus(); } }
}

async function prepareSelectedProject(chosenReference: string | null = null): Promise<void> {
  if (selectedProject === null || preparingProject !== null) return;
  const project = selectedProject;
  selectedPlanFingerprint = null;
  const request = ++planRequest; planLoading = true;
  planFeedback('Preparing the execution plan…', false, true);
  resetBoundaryChoice();
  preparingProject = project.id;
  void syncMenus();
  element('preparation-status').textContent = 'Preparing Brig project configuration…';
  element('retry-preparation').hidden = true;
  element('session-candidates').hidden = true;
  element('sharing-plan').hidden = true;
  element('codex-actions').hidden = true;
  element('prepare-environment').hidden = true;
  element('refresh-plan').hidden = true;
  try {
    const result = await prepareProject(project, chosenReference);
    if (selectedProject?.id !== project.id || request !== planRequest) return;
    selectedProject = result.project;
    selectedPlanFingerprint = result.fingerprint;
    planLoading = false;
    planRetryAction = 'prepare';
    planFeedback(result.plan ? null : result.message, !result.plan && !result.candidates.length && result.project.stage === 'environment-unverified');
    element('project-reference').textContent = result.project.profile + '@' + result.project.label;
    element('preparation-status').textContent = result.message ?? 'Brig configuration ready.';
    if (result.plan) {
      renderPlan(result.plan);
      element('prepare-environment').hidden = false;
      element('refresh-plan').hidden = false;
      updateCodexActions();
    }
    if (result.candidates.length) {
      const choice = element<HTMLSelectElement>('session-choice');
      choice.replaceChildren(...result.candidates.map(reference => { const option = document.createElement('option'); option.value = reference; option.textContent = reference; return option; }));
      element('session-candidates').hidden = false;
    }
  } catch (error) {
    if (selectedProject?.id !== project.id || request !== planRequest) return;
    element('preparation-status').textContent = String(error);
    element('retry-preparation').hidden = false;
    planRetryAction = 'prepare'; planFeedback(String(error), true);
  } finally { preparingProject = null; if (request === planRequest) planLoading = false; updateCodexActions(); void syncMenus(); }
}

function updateCodexActions(): void {
  const ready = selectedProject?.stage === 'environment-ready';
  const configured = selectedPlanFingerprint !== null && ['ready', 'environment-starting', 'environment-ready'].includes(selectedProject?.stage ?? '');
  element('resume-removal').hidden = selectedProject?.stage !== 'removing';
  element('codex-actions').hidden = !ready;
  element('project-overview').hidden = !configured;
  element('project-setup').hidden = ready || planLoading || selectingProject || preparingProject !== null
    || selectedProject?.stage === 'environment-starting' || selectedPlanFingerprint !== null;
  element<HTMLButtonElement>('start-codex').disabled = !canStartChat() || startingTerminal || selectingProject || choosingFolder || preparingProject !== null;
  element('project-account-feedback').hidden = !environmentReady() || canStartChat();
  element('project-account-status').textContent = accountConnectionError ?? (syncingAccount
    ? 'Connecting the shared ChatGPT account…' : sharedChatgpt?.status()?.message ?? 'Checking the shared ChatGPT account…');
  setBusy(element('project-account-status'), syncingAccount);
  element<HTMLButtonElement>('guest-shell').disabled = !environmentReady() || startingTerminal || selectingProject || choosingFolder || preparingProject !== null;
  sharedChatgpt?.updateAvailability();
  if (ready) void syncProjectAccount();
}

for (const [id, action, caption] of [['start-codex', 'codex', 'New Chat'], ['guest-shell', 'shell', 'Guest terminal']]) {
  element(id).addEventListener('click', () => {
    if (selectedProject === null || selectedPlanFingerprint === null) return;
    void openTerminal('start_codex_session', { identifier: selectedProject.id, fingerprint: selectedPlanFingerprint, action, useBrigBoundary: useBrigBoundary(projectContext()) }, caption);
  });
}
element('project-provider-settings').addEventListener('click', () => {
  showView('settings'); element('shared-chatgpt-heading').focus(); void sharedChatgpt?.refresh();
});

function showView(view: 'project' | 'settings'): void {
  closeProjectDialogs();
  const settings = view === 'settings';
  projectView.hidden = settings;
  settingsView.hidden = !settings;
  projectNavigation.removeAttribute('aria-current');
  settingsNavigation.removeAttribute('aria-current');
  element('sidebar-current-project').removeAttribute('aria-current');
  (settings ? settingsNavigation : selectedProject ? element('sidebar-current-project') : projectNavigation).setAttribute('aria-current', 'page');
  (settings ? element('settings-heading') : element('project-heading')).focus();
  scheduleProjectRescan();
  sharedChatgpt?.updateAvailability();
  if (settings) void sharedChatgpt?.refresh();
  void updateTerminalLayout();
  if (!settings && !terminalPanel.hidden && terminalIdentifier !== null) void invoke('terminal_focus', { identifier: terminalIdentifier });
}

async function showProjects(message = 'Choose where Brig & Codex will work.'): Promise<void> {
  if (startingTerminal || selectingProject || preparingProject !== null) return;
  await hideTerminal();
  selectedProject = null;
  ++projectEpoch; ++terminalViewRequest; ++conversationStateRequest; changingSession = false;
  resetProjectDetails();
  element('preparation-status').textContent = '';
  element('sharing-plan-values').replaceChildren();
  terminalIdentifier = null;
  terminals = [];
  ++terminalListRequest;
  selectedPlanFingerprint = null;
  ++planRequest; planLoading = false; planFeedback(null);
  selectedConversationId = null;
  conversations = [];
  ++conversationListRequest;
  loadingConversations = false;
  setBusy(element('refresh-sessions'), false);
  renderConversations([], null, resumeConversation);
  resetBoundaryChoice();
  projectView.classList.remove('has-project');
  updateTerminalActions(null);
  for (const id of ['project-actions', 'project-meta', 'selected-branch', 'project-settings-button', 'project-dashboard']) element(id).hidden = true;
  startTerminalButton.hidden = true;
  element('project-heading').textContent = 'Projects';
  element('project-heading').removeAttribute('title');
  element('project-description').textContent = message;
  element('project-description').hidden = false;
  chooseFolderButton.hidden = false;
  folderError.hidden = true;

  renderProjectLists();
  showView('project');
  void syncMenus();
  await refreshProjects();
}

async function chooseFolder(): Promise<void> {
  if (choosingFolder || selectedProject) return;
  if (startingTerminal || preparingProject !== null) {
    folderError.textContent = 'Wait for project setup to finish.';
    folderError.hidden = false;
    return;
  }
  choosingFolder = true;
  void syncMenus();
  chooseFolderButton.disabled = true;
  folderError.hidden = true;
  try {
    const selected = await invoke<string | null>('choose_project_folder');
    if (!selected) return;
    await selectProject(await invoke<Project>('register_project', { folder: selected }));
  } catch (error) {
    folderError.textContent = String(error);
    folderError.hidden = false;
    showView('project');
  } finally {
    choosingFolder = false;
    chooseFolderButton.disabled = false;
    void syncMenus();
    if (!projectView.hidden && !document.querySelector('dialog[open]') && terminalPanel.hidden) element('project-heading').focus();
    if (!projectView.hidden && !document.querySelector('dialog[open]') && !terminalPanel.hidden && terminalIdentifier !== null) void invoke('terminal_focus', { identifier: terminalIdentifier });
  }
}

function closeProjectDialogs(): void {
  for (const dialog of [projectSettingsDialog, usageDialog, element<HTMLDialogElement>('removal-dialog')]) if (dialog.open) dialog.close();
}
async function showProjectSettings(returnFocusId = 'project-settings-button'): Promise<void> {
  if (!selectedProject) return;
  resetProjectDetails();
  const context = projectContext()!;
  const projectId = context.id;
  const request = ++projectPreferencesRequest;
  element<HTMLFieldSetElement>('project-preference-fields').disabled = true;
  setLoading(element('project-preference-fields'), true);
  element('project-preferences-status').textContent = 'Loading project settings…';
  showDialog(projectSettingsDialog, returnFocusId); await updateTerminalLayout();
  try {
    const result = await invoke<{ project: Project; options: { cpus: number; memoryMb: number; verification: string; network: string } }>('read_project_preferences', { identifier: projectId });
    if (!sameProjectView(projectContext(), context) || request !== projectPreferencesRequest || !projectSettingsDialog.open) return;
    element<HTMLInputElement>('project-name').value = result.project.name;
    element<HTMLInputElement>('project-cpus').value = String(result.options.cpus);
    element<HTMLInputElement>('project-memory').value = String(result.options.memoryMb);
    element<HTMLSelectElement>('project-verification').value = result.options.verification;
    element<HTMLSelectElement>('project-network').value = result.options.network;
    element<HTMLFieldSetElement>('project-preference-fields').disabled = false;
    element('project-preferences-status').textContent = '';
  } catch (error) { if (sameProjectView(projectContext(), context) && request === projectPreferencesRequest && projectSettingsDialog.open) element('project-preferences-status').textContent = String(error); }
  finally { if (sameProjectView(projectContext(), context) && request === projectPreferencesRequest) setLoading(element('project-preference-fields'), false); }
}
element('project-settings-button').addEventListener('click', () => { void showProjectSettings(); });
element('setup-project').addEventListener('click', () => { void showProjectSettings('setup-project'); });
element('close-project-settings').addEventListener('click', () => { projectSettingsDialog.close(); });
element('close-session-usage').addEventListener('click', () => { usageDialog.close(); });
for (const dialog of [projectSettingsDialog, usageDialog]) dialog.addEventListener('close', () => { void updateTerminalLayout(); });
usageDialog.addEventListener('close', () => { if (!usageDialog.open) { ++usageRequest; usageSessionId = null; usageFingerprint = null; usagePending = false; usageDetails = null; element('session-usage-title').textContent = ''; element('session-usage-details').replaceChildren(); element('session-usage-status').textContent = ''; setLoading(element('session-usage-details'), false); setBusy(element('session-usage-status'), false); } });
projectSettingsDialog.addEventListener('close', () => { if (!projectSettingsDialog.open) resetProjectDetails(); });
element<HTMLFormElement>('project-preferences').addEventListener('submit', async event => {
  event.preventDefault(); if (!selectedProject) return;
  const context = projectContext()!; const id = context.id; const request = ++projectPreferencesRequest; const fields = element<HTMLFieldSetElement>('project-preference-fields'); fields.disabled = true;
  element('project-preferences-status').textContent = 'Saving project settings…';
  setBusy(element('save-project-preferences'), true);
  try {
    const project = await invoke<Project>('save_project_preferences', { identifier: id, name: element<HTMLInputElement>('project-name').value, options: { cpus: Number(element<HTMLInputElement>('project-cpus').value), memoryMb: Number(element<HTMLInputElement>('project-memory').value), verification: element<HTMLSelectElement>('project-verification').value, network: element<HTMLSelectElement>('project-network').value } });
    if (!sameProject(projectContext(), context)) return;
    selectedProject = project; element('project-heading').textContent = project.name; element('project-heading').title = project.name;
    await prepareSelectedProject(); await ensureSelectedEnvironment(); updateCodexActions(); await refreshProjects();
    if (sameProjectView(projectContext(), context) && request === projectPreferencesRequest) element('project-preferences-status').textContent = 'Project settings saved.';
  } catch (error) { if (sameProjectView(projectContext(), context) && request === projectPreferencesRequest) element('project-preferences-status').textContent = String(error); }
  finally { if (sameProjectView(projectContext(), context) && request === projectPreferencesRequest) { setBusy(element('save-project-preferences'), false); fields.disabled = false; } }
});
for (const tab of document.querySelectorAll<HTMLButtonElement>('[data-session-state]')) tab.addEventListener('click', () => { sessionView = tab.dataset.sessionState as SessionState; renderSessionDashboard(); });
await listen<[string, Dashboard]>('project-usage-updated', event => {
  if (selectedProject?.id !== event.payload[0]) return;
  if (!applyDashboard(event.payload[1])) return;
  const hasActive = standaloneConversations(conversations, 'active').length > 0 || terminals.some(terminal => terminal.running && isChat(terminal));
  element('session-list-status').hidden = hasActive;
  element('session-list-status').textContent = hasActive ? '' : 'No active sessions. Start a New Chat or restore an archived session.';
});
await listen<[string, string, number]>('project-usage-error', event => {
  if (selectedProject?.id === event.payload[0] && acceptDashboardUpdate(event.payload[2])) {
    dashboardError = event.payload[1];
    renderSessionDashboard();
  }
});

projectNavigation.addEventListener('click', () => { void showProjects(); });
element('sidebar-current-project').addEventListener('click', async () => { await hideTerminal(); showView('project'); void refreshProjects(); void refreshConversations(); });
element('refresh-sessions').addEventListener('click', () => { void refreshConversations(); });
settingsNavigation.addEventListener('click', () => showView('settings'));
chooseFolderButton.addEventListener('click', () => { void chooseFolder(); });
document.defaultView!.addEventListener('focus', () => {
  if (projectRescanStarted && !projectView.hidden && selectedProject === null
    && (projectRescanError !== null || projects.some(project => project.available === false))) void rescanProjects();
});
element('retry-preparation').addEventListener('click', async () => { await prepareSelectedProject(); await ensureSelectedEnvironment(); });
element('retry-project-plan').addEventListener('click', async () => {
  if (planRetryAction === 'environment') { await ensureSelectedEnvironment(); return; }
  if (planRetryAction === 'preview') { await refreshSelectedPlan(); await ensureSelectedEnvironment(); return; }
  await prepareSelectedProject(); await ensureSelectedEnvironment();
});
element('resume-removal').addEventListener('click', async () => { await showProjectSettings('resume-removal'); element('preview-removal').click(); });
element('adopt-session').addEventListener('click', async () => { await prepareSelectedProject(element<HTMLSelectElement>('session-choice').value); await ensureSelectedEnvironment(); });
element('refresh-plan').addEventListener('click', () => { void refreshSelectedPlan(); });
element('return-to-terminal').addEventListener('click', () => { void returnToTerminal(); });
element('hide-terminal').addEventListener('click', async () => { await hideTerminal(); element('return-to-terminal').hidden = false; element('project-heading').focus(); void refreshProjects(); void refreshConversations(); });
element('prepare-environment').addEventListener('click', () => {
  if (selectedProject === null || selectedPlanFingerprint === null) return;
  void openTerminal('prepare_environment', { identifier: selectedProject.id, fingerprint: selectedPlanFingerprint }, 'Preparing Brig environment…');
});
startTerminalButton.addEventListener('click', () => {
  if (selectedProject) void openTerminal('terminal_start', { projectId: selectedProject.id }, 'Local terminal');
});

async function openTerminal(command: string, arguments_: Record<string, unknown>, caption: string, foreground = true): Promise<void> {
  if (selectedProject === null || startingTerminal) return;
  const projectId = selectedProject.id;
  startingTerminal = true;
  void syncMenus();
  let previous: TerminalStatus | null;
  try { await refreshTerminals(); previous = currentTerminal(); }
  catch (error) {
    startingTerminal = false; void syncMenus();
    if (foreground) showFolderError(String(error));
    if (!foreground) environmentFeedback('Could not start the project environment. ' + String(error));
    return;
  }
  const existingChat = typeof arguments_.sessionId === 'string' ? terminals.find(status => status.running && status.conversationId === arguments_.sessionId) : undefined;
  const purpose = command === 'terminal_start' ? 'local' : command === 'prepare_environment' ? 'environment' : String(arguments_.action);
  const launchingChat = command === 'start_codex_session' && ['codex', 'resume'].includes(String(arguments_.action));
  const blocking = command === 'start_project_operation' ? undefined : terminals.find(status => status.running
    && ['local', 'environment', 'login', 'codex', 'resume', 'shell'].includes(status.purpose)
    && (foreground || status.purpose !== 'local')
    && !(launchingChat && isChat(status)));
  if (existingChat || blocking) {
    startingTerminal = false;
    void syncMenus();
    if (existingChat || blocking?.purpose === purpose) {
      if (!foreground) {
        if (purpose === 'environment') {
          environmentTerminals.set(projectId, blocking!.identifier);
          selectedProject = { ...selectedProject, stage: 'environment-starting' };
          updateCodexActions(); environmentFeedback();
        }
        return;
      }
      folderError.hidden = true;
      closeProjectDialogs();
      await returnToTerminal((existingChat ?? blocking)!.identifier);
    } else if (blocking) {
      const names: Record<string, string> = { codex: 'chat', resume: 'chat', login: 'account sign-in', shell: 'guest terminal', local: 'local terminal', environment: 'environment setup' };
      const message = 'End the running ' + (names[blocking.purpose] ?? 'terminal') + ' before opening ' + caption + '.';
      if (projectSettingsDialog.open) element(purpose === 'local' ? 'local-terminal-status' : purpose === 'environment' ? 'preparation-status' : 'guest-terminal-status').textContent = message;
      else showFolderError(message);
    }
    return;
  }
  element('local-terminal-status').textContent = '';
  if (foreground) {
    closeProjectDialogs();
    await hideTerminal();
    terminalIdentifier = null;
  }
  startTerminalButton.disabled = true;
  if (foreground) terminalPanel.hidden = false;
  chooseFolderButton.disabled = true;
  folderError.hidden = true;
  const preparingEnvironment = command === 'prepare_environment';
  if (foreground) showTerminalStatus('Opening ' + caption + '…');
  if (preparingEnvironment) {
    pendingEnvironmentProject = projectId;
    selectedProject = { ...selectedProject, stage: 'environment-starting' };
    updateCodexActions(); environmentFeedback();
  }
  try {
    const layout = terminalLayout();
    const identifier = await invoke<number>(command, { ...arguments_, layout: { ...layout, visible: foreground && layout.visible } });
    if (foreground) terminalIdentifier = identifier;
    if (preparingEnvironment) {
      environmentTerminals.set(projectId, identifier);
      pendingEnvironmentProject = null;
      await refreshSelectedPlan();
    }
    if (foreground) lastTerminals.set(projectId, identifier);
    await refreshTerminals();
    if (foreground) { showTerminalStatus(caption); await updateTerminalLayout(); }
    const snapshot = await invoke<TerminalStatus | null>('terminal_snapshot', { identifier });
    if (preparingEnvironment && snapshot && !snapshot.running) {
      selectedProject = await invoke<Project>('finish_environment', { identifier: projectId, terminalIdentifier: identifier });
      await refreshSelectedPlan();
      environmentFeedback(selectedProject.stage === 'environment-ready' ? null : (selectedProject.environmentAttempt?.error ?? 'Could not start the project environment. Check its terminal output and try again.'));
    }
    updateTerminalActions(foreground ? snapshot : currentTerminal());
    if (foreground && snapshot && !snapshot.running) showTerminalStatus(terminalCaption(snapshot));
    if (foreground && !projectView.hidden) await invoke('terminal_focus', { identifier });
  } catch (error) {
    if (preparingEnvironment) { pendingEnvironmentProject = null; await refreshSelectedPlan(); }
    if (foreground) { terminalPanel.hidden = true; terminalIdentifier = previous?.identifier ?? null; }
    if (!foreground && preparingEnvironment) environmentFeedback('Could not start the project environment. ' + String(error));
    if (foreground) showFolderError('Could not open the terminal. ' + String(error));
    chooseFolderButton.disabled = false;
  } finally {
    startingTerminal = false;
    startTerminalButton.disabled = false;
    chooseFolderButton.disabled = false;
    await refreshTerminals();
    void syncMenus();
  }
}
element('close-terminal').addEventListener('click', async () => {
  if (terminalIdentifier === null) return;
  const identifier = terminalIdentifier;
  if (closingTerminals.has(identifier)) return;
  closingTerminals.add(identifier);
  updateTerminalActions(currentTerminal());
  const projectId = selectedProject?.id;
  try {
    await invoke('terminal_close', { identifier });
    if (projectId && lastTerminals.get(projectId) === identifier) lastTerminals.delete(projectId);
    if (terminalIdentifier === identifier) {
      terminalIdentifier = null;
      terminalPanel.hidden = true;
      element('project-heading').focus();
    }
    await refreshTerminals();
    void syncMenus();
    chooseFolderButton.disabled = false;
    void refreshConversations();
  } catch (error) {
    if (terminalIdentifier === identifier) showTerminalStatus('Could not end the process. ' + String(error));
  } finally { closingTerminals.delete(identifier); updateTerminalActions(currentTerminal()); }
});
new ResizeObserver(() => { void updateTerminalLayout(); }).observe(terminalSurface);
initializeSidebar(() => { void updateTerminalLayout(); });
await listen<string>('terminal-sessions-changed', async event => {
  if (selectedProject?.id !== event.payload) return;
  try { await refreshTerminals(); }
  catch (error) { showFolderError(String(error)); }
});
await listen<{ identifier: number; projectId: string; purpose: string; code: number | null }>('terminal-exit', async event => {
  if (selectedProject?.id === event.payload.projectId) {
    await refreshTerminals();
  }
  void syncMenus();
  if (event.payload.identifier === terminalIdentifier) {
    const status = terminals.find(status => status.identifier === terminalIdentifier);
    showTerminalStatus(status ? terminalCaption(status) : 'Terminal process ended');
  }
  if (event.payload.purpose === 'login') await sharedChatgpt?.refresh();
  if (['codex', 'resume'].includes(event.payload.purpose) && selectedProject?.id === event.payload.projectId) { void refreshProjects(); void refreshConversations(); }
});
await listen<{ project: Project; terminalIdentifier: number; message: string | null }>('environment-preparation-finished', async event => {
  const result = event.payload;
  if (selectedProject?.id !== result.project.id || pendingEnvironmentProject === result.project.id) return;
  if ((environmentTerminals.get(result.project.id) ?? 0) > result.terminalIdentifier) return;
  environmentTerminals.set(result.project.id, result.terminalIdentifier);
  selectedProject = result.project;
  const context = projectContext()!;
  await refreshSelectedPlan();
  if (!sameProject(projectContext(), context)) return;
  updateCodexActions();
  element('preparation-status').textContent = result.message ?? 'Brig environment and guest agent version verified.';
  environmentFeedback(result.message ?? (selectedProject?.stage === 'environment-ready' ? null : 'The project environment is not running. Try starting it again.'));
  if (result.terminalIdentifier === terminalIdentifier) showTerminalStatus(result.message ? 'Environment setup failed. Prepare the environment again.' : 'Brig environment ready');
});
await listen<{ projectId: string; terminalIdentifier: number; message: string }>('environment-preparation-error', event => {
  if (selectedProject?.id !== event.payload.projectId || (environmentTerminals.get(event.payload.projectId) ?? 0) > event.payload.terminalIdentifier) return;
  element('preparation-status').textContent = event.payload.message;
  environmentFeedback(event.payload.message);
  showFolderError(event.payload.message);
});
await listen<{ identifier: number; message: string }>('terminal-input-error', event => { if (event.payload.identifier === terminalIdentifier) showTerminalStatus(event.payload.message); });

await listen<string>('menu-action', async event => {
  if (event.payload === 'choose-folder' && selectedProject === null) { showView('project'); void chooseFolder(); }
  if (event.payload === 'settings') showView('settings');
  if (event.payload === 'project') { await hideTerminal(); showView('project'); void refreshProjects(); void refreshConversations(); }
  if (event.payload === 'return-terminal') { showView('project'); await returnToTerminal(); }
});
showView('project');
await invoke('frontend_ready');
resetBoundaryDetails = initializeBoundaryTests(projectContext);
resetPolicyDetails = initializeProjectPolicy(() => selectedProject ? { ...projectContext()!, project: selectedProject } : null, async (project, detached) => {
  selectedProject = project;
  updateCodexActions();
  selectedPlanFingerprint = null;
  resetBoundaryChoice();
  element('codex-actions').hidden = true;
  element('prepare-environment').hidden = true;
  element('refresh-plan').hidden = true;
  element('sharing-plan').hidden = true;
  if (!detached) await refreshSelectedPlan();
});
resetOperationDetails = initializeOperations(projectContext, openTerminal, async project => {
  selectedProject = project;
  selectedPlanFingerprint = null;
  resetBoundaryChoice();
  updateCodexActions();
  await refreshTerminals();
  if (selectedProject?.id === project.id) await refreshSelectedPlan();
}, async message => { terminalIdentifier = null; await showProjects(message); });
resetProjectDetails();
await initializeDataLocation();
await initializeSettings();
sharedChatgpt = await initializeChatgptAccount(
  () => environmentReady() && !startingTerminal && !selectingProject && preparingProject === null,
  async pendingProjectId => {
    if (pendingProjectId && selectedProject?.id !== pendingProjectId) {
      const project = projects.find(project => project.id === pendingProjectId);
      if (project) await selectProject(project, true);
    }
    if (!selectedProject) { await showProjects('Open or add a project, then sign in once from Settings.'); return; }
    showView('project');
    if (pendingProjectId) {
      await refreshTerminals();
      const login = terminals.find(terminal => terminal.running && terminal.purpose === 'login');
      if (login) { await returnToTerminal(login.identifier); return; }
    }
    if (!environmentReady()) { showFolderError('Wait for the project environment to be ready, then sign in from Settings.'); return; }
    await openTerminal('start_codex_session', { identifier: selectedProject.id, fingerprint: selectedPlanFingerprint, action: 'login', useBrigBoundary: false }, 'Sign in once with ChatGPT');
  },
  status => {
    if (status.state !== 'signed-in') connectedAccountProject = null;
    updateCodexActions();
  },
);
updateCodexActions();
void initializeHost();
void syncMenus();
await rescanProjects();

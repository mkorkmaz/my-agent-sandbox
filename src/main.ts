import { StartupSequence } from './app/startup-state';
import { FormState } from './ui/form-state';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './style.css';
import { initializeHost } from './settings/host';
import { initializeSettings, initializeSettingsTabs, selectSettingsTab } from './settings/settings';
import { renderProjects, renderConversations, renderTerminals, isStandaloneConversation, standaloneConversations, terminalCaption, terminalNames, conversationKey, terminalMatches, agentName, isChat, sameProject, sameProjectView, type Conversation, type Project, type ProjectContext, type TerminalStatus } from './projects/projects';
import { createAgentLogo } from './providers/agent-logo';
import { prepareProject, renderPlan, type Plan } from './projects/preparation';
import { initializeBoundaryTests, resetBoundaryChoice, useBrigBoundary } from './projects/boundaries';
import { initializeOperations } from './projects/operations';
import { initializeProjectPolicy } from './projects/project-policy';
import { renderDashboard, emptyUsage, savedSessionsStatus, type Dashboard, type SessionState } from './dashboard/dashboard';
import { showDialog } from './ui/dialogs';
import { initializeSidebar } from './sidebar/sidebar';
import { initializeDataLocation } from './settings/data-location';
import { initializeChatgptAccount, type ChatgptAccount, type ChatgptAccountStatus } from './providers/chatgpt-account';
import { initializeClaudeAccount, type ClaudeAccount } from './providers/claude-account';
import { renderSkeleton, setBusy, setLoading } from './ui/loading';
import { TerminalLaunch, type TerminalLaunchRequest } from './terminal/terminal-launch';
import { updateTerminal } from './terminal/terminal-adapter';
import { TerminalLayoutQueue } from './terminal/terminal-layout';
import { RequestState } from './ui/request-state';
import { ProjectRescan } from './projects/project-rescan';
import { DashboardState, type DashboardRequest } from './dashboard/dashboard-state';
import { initializeUsageDetails } from './usage/usage-details-controller';
import { renderWorkspaceStatus, setWorkspaceStatus, showWorkspaceError } from './app/workspace-status';
import { appData, appDataStore, type ProjectPreferences } from './app/app-data';

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error('Missing interface element ' + id);
  return found as T;
}

const projectView = element('project-view');
const settingsView = element('settings-view');
const projectNavigation = element<HTMLButtonElement>('project-navigation');
const settingsNavigation = element<HTMLButtonElement>('settings-navigation');
initializeSettingsTabs();
const chooseFolderButton = element<HTMLButtonElement>('choose-folder');
const projectSettingsDialog = element<HTMLDialogElement>('project-settings-dialog');
const usageDialog = element<HTMLDialogElement>('session-usage-dialog');
const usageInfo = element<HTMLDetailsElement>('usage-info');
document.addEventListener('click', event => { if (!usageInfo.contains(event.target as Node)) usageInfo.open = false; });
document.addEventListener('keydown', event => {
  if (event.key !== 'Escape' || !usageInfo.open) return;
  usageInfo.open = false;
  usageInfo.querySelector<HTMLElement>('summary')!.focus();
});
const folderError = element('folder-error');
const folderChoice = new RequestState();
const projectSelection = new RequestState();
let projectEpoch = 0;
let projectSettingsEpoch = 0;
const projectPreferences = new FormState<ProjectPreferences>();
let projectRecordRevision = 0;
let cachedProjectRecordRevision = 0;
let pendingProjectRecordRefresh = false;
let pendingProjectSettingsRefresh = false;
let pendingProjectRecovery: ProjectContext | null = null;
let sharedChatgpt: ChatgptAccount | null = null;
let claudeAccount: ClaudeAccount | null = null;
const agentSelection = new RequestState();
let accountSyncKey: string | null = null;
let accountConnectionError: string | null = null;
const accountSync = new RequestState();
let connectedAccountProject: string | null = null;
let usageRecordRevision = 0;
const conversationChange = new RequestState();
let resetPolicyDetails = () => {};
let resetBoundaryDetails = () => {};
let resetOperationDetails = () => {};
let terminalIdentifier: number | null = null;
const terminalLaunch = new TerminalLaunch();
let pendingEnvironmentKey: string | null = null;
const environmentTerminals = new Map<string, number>();
const preparingEnvironments = new Map<string, Promise<Awaited<ReturnType<typeof prepareProject>>>>();
function preparationKey(project: Project): string { return project.id + '|' + project.environmentId; }
function preparingSelected(): boolean { return appData.selectedProject !== null && preparingEnvironments.has(preparationKey(appData.selectedProject)); }
let selectedPlanFingerprint: string | null = null;
const planRequests = new RequestState();
let pendingRecordPlanRefresh: ProjectContext | null = null;
let planRetryAction: 'prepare' | 'preview' | 'environment' = 'prepare';
const emptyDashboard: Dashboard = { conversations: [], usage: emptyUsage, coverage: 'unknown', pricingDate: '2026-11-10', warnings: [] };
let sessionView: SessionState = 'active';
const dashboardState = new DashboardState();
let selectedConversationId: string | null = null;
const lastTerminals = new Map<string, number>();
const terminalListRequests = new Map<string, number>();
let terminalViewRequest = 0;
const terminalLayouts = new TerminalLayoutQueue();
let projectListRequest = 0;
const projectRescan = new ProjectRescan();
let projectRescanError: string | null = null;
const terminalPanel = element('terminal-panel');
const terminalSurface = element('terminal-surface');
const startTerminalButton = element<HTMLButtonElement>('start-terminal');
renderSkeleton(element('recent-project-list'), 3);
renderSkeleton(element('sidebar-project-list'));
element('recent-projects').hidden = false;
setLoading(element('recent-projects'), true);
setLoading(element('sidebar-project-list'), true);
const usageDetails = initializeUsageDetails({
  currentProject: projectContext,
  conversations: selectedConversations,
  fingerprint: selectedUsageFingerprint,
  updateTerminalLayout,
});

function selectedConversations(): Conversation[] { return dashboardState.current()?.conversations ?? []; }
function projectContext(): ProjectContext | null { return appData.selectedProject ? { id: appData.selectedProject.id, environmentId: appData.selectedProject.environmentId, fingerprint: selectedPlanFingerprint, epoch: projectEpoch, view: projectSettingsEpoch } : null; }
function resetProjectDetails(): void {
  ++projectSettingsEpoch; projectPreferences.reset();
  element('guest-terminal-status').textContent = '';
  setBusy(element('save-project-preferences'), false);
  setLoading(element('project-preference-fields'), false);
  element('project-preferences-status').textContent = '';
  element('local-terminal-status').textContent = '';
  element<HTMLFieldSetElement>('project-preference-fields').disabled = true;
  for (const id of ['project-name', 'project-cpus', 'project-memory']) element<HTMLInputElement>(id).value = '';
  for (const id of ['project-verification', 'project-network']) element<HTMLSelectElement>(id).value = '';
  resetPolicyDetails(); resetBoundaryDetails(); resetOperationDetails();
  finishProjectRecordRefresh();
}

function environmentReady(): boolean { return appData.selectedProject?.stage === 'environment-ready' && selectedPlanFingerprint !== null && !planRequests.pending(); }
function canStartChat(): boolean {
  const context = projectContext();
  if (appData.selectedProject?.agentKind === 'claude-code') return environmentReady() && claudeAccount?.status()?.state === 'signed-in';
  return environmentReady() && !accountSync.pending() && sharedChatgpt?.status()?.state === 'signed-in'
    && connectedAccountProject === [context?.id, context?.epoch, context?.fingerprint].join('|');
}

async function syncProjectAccount(): Promise<void> {
  const context = projectContext();
  if (!context || appData.selectedProject?.agentKind !== 'codex' || sharedChatgpt?.status()?.state !== 'signed-in' || !environmentReady() || accountSync.pending()) return;
  const key = [context.id, context.epoch, context.fingerprint, sharedChatgpt.status()?.generation].join('|');
  if (accountSyncKey === key) return;
  accountSyncKey = key; const request = accountSync.begin(); connectedAccountProject = null;
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
  finally { accountSync.finish(request); updateCodexActions(); }
}

function planFeedback(message: string | null, retry = false, busy = false): void {
  element('project-plan-feedback').hidden = message === null || busy;
  element('project-plan-status').textContent = message ?? '';
  setBusy(element('project-plan-status'), busy);
  setBusy(element('preparation-status'), busy);
  setWorkspaceStatus('project-plan', busy ? message : null, 'project-dashboard');
  element('retry-project-plan').hidden = !retry;
  element('retry-project-plan').textContent = planRetryAction === 'environment' ? 'Retry environment' : 'Retry setup';
}

function environmentFeedback(message: string | null = null): void {
  if (appData.selectedProject?.stage === 'environment-ready') { planFeedback(null); return; }
  if (message) { planRetryAction = 'environment'; planFeedback(message, true); return; }
  if (appData.selectedProject?.stage === 'environment-starting') {
    planFeedback('Starting the project environment with your saved settings…', false, true); return;
  }
  const error = appData.selectedProject?.environmentAttempt?.error;
  if (error) { planRetryAction = 'environment'; planFeedback(error, true); return; }
  if (appData.selectedProject?.stage === 'ready' && selectedPlanFingerprint) {
    planRetryAction = 'environment'; planFeedback('The project environment is stopped.', true);
    element('retry-project-plan').textContent = 'Start environment';
  }
}

async function ensureSelectedEnvironment(): Promise<void> {
  if (!appData.selectedProject || !selectedPlanFingerprint || !['ready', 'environment-starting'].includes(appData.selectedProject.stage) || appData.selectedProject.policyDetached) return;
  if (appData.terminals.some(status => appData.selectedProject && terminalMatches(status, appData.selectedProject) && status.running && (isChat(status) || ['login', 'shell'].includes(status.purpose)))) {
    planFeedback('End running sessions to apply the saved environment settings.'); return;
  }
  planFeedback('Starting the project environment with your saved settings…', false, true);
  await openTerminal('prepare_environment', { identifier: appData.selectedProject.id, environmentId: appData.selectedProject.environmentId, fingerprint: selectedPlanFingerprint }, 'Starting project environment…', false);
}

function currentTerminal(): TerminalStatus | null {
  const remembered = appData.selectedProject ? lastTerminals.get(appData.selectedProject.id) : null;
  return appData.terminals.find(status => status.identifier === (terminalIdentifier ?? remembered))
    ?? appData.terminals.find(status => status.running && isChat(status))
    ?? appData.terminals.find(status => status.running)
    ?? appData.terminals.at(-1) ?? null;
}

function chatConversationId(status: TerminalStatus): string | null {
  const conversation = selectedConversations().find(conversation => conversation.id === status.conversationId && (status.environmentId ? conversation.environmentId === status.environmentId || (!conversation.environmentId && status.agentKind !== 'claude-code') : !conversation.agentKind || conversation.agentKind === 'codex'));
  if (!conversation) return status.conversationId ? conversationKey({ id: status.conversationId, environmentId: status.environmentId }) : null;
  if (conversation.relationStatus === 'resolved') {
    const root = selectedConversations().find(root => root.id === conversation.rootSessionId && root.environmentId === conversation.environmentId && isStandaloneConversation(root));
    return root ? conversationKey(root) : null;
  }
  return isStandaloneConversation(conversation) ? conversationKey(conversation) : null;
}

function renderSessionSidebar(): void {
  const chats = appData.terminals.filter(status => status.running && isChat(status));
  const rows = standaloneConversations(selectedConversations(), 'active').map(conversation => {
    const terminal = chats.find(status => chatConversationId(status) === conversationKey(conversation));
    return { ...conversation, available: terminal ? true : conversation.available, terminalId: terminal?.identifier, busy: terminal?.busy };
  });
  for (const terminal of chats) {
    if (rows.some(row => row.terminalId === terminal.identifier)) continue;
    const conversationId = chatConversationId(terminal);
    const saved = selectedConversations().find(conversation => conversationKey(conversation) === conversationId);
    rows.unshift({ ...(saved ?? { id: conversationId ? terminal.conversationId! : 'terminal-' + terminal.identifier, environmentId: terminal.environmentId ?? null, agentKind: terminal.agentKind ?? 'codex', usageAvailable: terminal.agentKind !== 'claude-code', title: (terminal.conversationId && !conversationId ? agentName(terminal.agentKind) + ' process ' : 'New Chat ') + terminal.identifier, updatedAt: Date.now(), state: 'active' as const, available: true, usage: emptyUsage, warning: null, lineage: { kind: 'unknown' as const, parentSessionId: null, agentPath: null, nickname: null, sourceVersion: null, lineageVersion: 1 as const }, relationStatus: 'unknown' as const, rootSessionId: null, subagentCount: 0, familyUsage: emptyUsage, coverage: 'unknown' as const, usageRevision: 'pending-terminal-' + terminal.identifier }), available: true, terminalId: terminal.identifier, busy: terminal.busy });
  }
  const selected = appData.terminals.find(status => status.identifier === terminalIdentifier);
  selectedConversationId = (selected ? chatConversationId(selected) : null) ?? (selected && isChat(selected) ? conversationKey({ id: 'terminal-' + selected.identifier, environmentId: selected.environmentId }) : null);
  renderConversations(rows, selectedConversationId, resumeConversation);
  renderTerminals(appData.terminals, terminalIdentifier, returnToTerminal);
  if (chats.length) element('session-list-status').hidden = true;
}

async function refreshTerminals(projectId = appData.selectedProject?.id): Promise<void> {
  if (!projectId) return;
  const request = (terminalListRequests.get(projectId) ?? 0) + 1;
  terminalListRequests.set(projectId, request);
  const result = await invoke<TerminalStatus[]>('terminal_sessions', { projectId });
  if (request !== terminalListRequests.get(projectId)) return;
  if (appData.selectedProject?.id !== projectId && !appData.projects.some(project => project.id === projectId)) return;
  const projectTerminals = new Map(appDataStore.getState().projectTerminals);
  projectTerminals.set(projectId, result);
  appDataStore.setState({ projectTerminals });
  if (appData.selectedProject?.id !== projectId) return;
  appData.terminals = result;
  if (terminalIdentifier !== null && !terminalLaunch.isClosing(terminalIdentifier) && !appData.terminals.some(status => status.identifier === terminalIdentifier)) {
    const missingIdentifier = terminalIdentifier;
    await hideTerminal();
    if (appData.selectedProject?.id !== projectId || request !== terminalListRequests.get(projectId) || terminalIdentifier !== missingIdentifier) return;
    terminalIdentifier = null;
    showWorkspaceError('This terminal output is no longer retained. Reopen another result from Terminals or run the operation again.', 'project-dashboard');
  }
  renderSessionSidebar();
  updateTerminalActions();
  void syncMenus();
}

function updateTerminalActions(): void {
  const terminalRunning = appData.terminals.some(terminal => terminal.running);
  element('close-terminal').textContent = appData.terminals.find(terminal => terminal.identifier === terminalIdentifier)?.running ? 'Close terminal' : 'Close output';
  const closing = terminalIdentifier !== null && terminalLaunch.isClosing(terminalIdentifier);
  element<HTMLButtonElement>('close-terminal').disabled = closing;
  setBusy(element('close-terminal'), closing);
  projectView.classList.toggle('has-live-terminal', terminalRunning);
  element<HTMLButtonElement>('start-codex').disabled = !canStartChat() || terminalLaunch.blocksChanges() || agentSelection.pending() || projectSelection.pending() || folderChoice.pending() || preparingSelected();
  element<HTMLButtonElement>('start-codex').title = 'New Chat';
}

async function syncMenus(): Promise<void> {
  const busy = folderChoice.pending() || projectSelection.pending() || preparingSelected() || terminalLaunch.blocksChanges() || agentSelection.pending();
  chooseFolderButton.disabled = busy;
  element<HTMLSelectElement>('agent-choice').disabled = terminalLaunch.blocksChanges() || projectSelection.pending() || folderChoice.pending();
  element<HTMLButtonElement>('start-codex').disabled = busy || !canStartChat();
  element<HTMLButtonElement>('guest-shell').disabled = busy || !environmentReady();
  projectNavigation.disabled = busy;
  element<HTMLButtonElement>('refresh-sessions').disabled = busy || dashboardState.pending();
  setBusy(element('refresh-sessions'), dashboardState.pending());
  setBusy(chooseFolderButton, folderChoice.pending());
  setBusy(element('terminal-status'), terminalLaunch.blocksChanges());
  for (const button of document.querySelectorAll<HTMLButtonElement>('.sidebar-project, .sidebar-session, .recent-project, .project-record-forget')) button.disabled = (button.classList.contains('sidebar-session') ? terminalLaunch.blocksChanges() || projectSelection.pending() || folderChoice.pending() : busy) || button.dataset.available === 'false';
  renderSessionDashboard();
  scheduleProjectRescan();
  try { await invoke('update_menu_state', { projectId: appData.selectedProject?.id ?? null, busy }); }
  catch { showFolderError('Could not update the native menu. Reopen the app.'); }
}

type TerminalVisibility = 'hidden' | 'obscured' | 'visible';

function terminalVisibility(): TerminalVisibility {
  if (projectView.hidden || terminalPanel.hidden) return 'hidden';
  if (document.querySelector('dialog[open]')) return 'obscured';
  return 'visible';
}

function terminalLayout(): { x: number; y: number; width: number; height: number; visible: boolean } {
  const rectangle = terminalSurface.getBoundingClientRect();
  return { x: rectangle.x, y: rectangle.y, width: rectangle.width, height: rectangle.height, visible: terminalVisibility() === 'visible' };
}

function terminalUpdatesAllowed(identifier: number): boolean {
  return terminalIdentifier === identifier && !terminalLaunch.isClosing(identifier);
}

function updateTerminalLayout(): Promise<void> {
  return terminalLayouts.request(async () => {
    const identifier = terminalIdentifier;
    if (identifier === null || !terminalUpdatesAllowed(identifier)) return;
    const layout = terminalLayout();
    await updateTerminal('terminal_layout', { identifier, layout }, () => !layout.visible || !terminalUpdatesAllowed(identifier));
  }, error => { showFolderError(String(error)); });
}

async function focusTerminal(identifier: number): Promise<void> {
  if (!terminalUpdatesAllowed(identifier)) return;
  await updateTerminal('terminal_focus', { identifier }, () => !terminalUpdatesAllowed(identifier));
}

function showFolderError(message: string): void { folderError.textContent = message; folderError.hidden = false; }

function showTerminalStatus(message: string): void {
  const label = element('terminal-status');
  const title = document.createElement('span'); title.className = 'terminal-status-title'; title.textContent = message;
  label.replaceChildren(title);
  label.title = message;
  label.removeAttribute('aria-label');
  const status = appData.terminals.find(status => status.identifier === terminalIdentifier);
  if (!status || !isChat(status)) return;
  const kind = status.agentKind ?? (status.purpose.startsWith('claude') ? 'claude-code' : 'codex');
  label.prepend(createAgentLogo(kind));
  label.setAttribute('aria-label', agentName(kind) + ': ' + message);
}

function renderProjectLists(): void {
  renderProjects(appData.projects, appData.selectedProject, project => selectProject(project, true), showForgetProject);
  setLoading(element('recent-projects'), !appData.projectsLoaded);
  setLoading(element('sidebar-project-list'), !appData.projectsLoaded);
  if (!appData.projectsLoaded && !appData.selectedProject) {
    renderSkeleton(element('recent-project-list'), 3);
    renderSkeleton(element('sidebar-project-list'));
    element('recent-projects').hidden = false;
    element('empty-projects').hidden = true;
  }
  renderProjectAvailability();
  scheduleProjectRescan();
  renderWorkspaceStatus();
}

function renderProjectAvailability(): void {
  const unavailable = appData.projects.some(project => project.available === false);
  projectRescanStatus(projectRescanError ?? (unavailable ? 'Some project folders are unavailable. Connect the drive; Tugboat checks again automatically.' : null));
}

function projectRescanStatus(message: string | null, busy = false): void {
  const status = element('project-rescan-status');
  status.hidden = appData.selectedProject !== null || message === null || busy;
  if (status.textContent !== (message ?? '')) status.textContent = message ?? '';
}

function scheduleProjectRescan(): void {
  const needed = projectRescan.started() && !projectView.hidden && appData.selectedProject === null
    && (projectRescanError !== null || appData.projects.some(project => project.available === false));
  const busy = projectRescan.scanning() || folderChoice.pending() || projectSelection.pending() || preparingSelected() || terminalLaunch.blocksChanges();
  projectRescan.schedule(needed, busy, rescanProjects);
}

const forgetProjectDialog = element<HTMLDialogElement>('forget-project-dialog');
let forgettingProject: Project | null = null;
let forgetting = false;
function forgetProjectData(id: string): void {
  dashboardState.forget(id);
  terminalListRequests.set(id, (terminalListRequests.get(id) ?? 0) + 1);
  lastTerminals.delete(id);
  const projectTerminals = new Map(appDataStore.getState().projectTerminals);
  projectTerminals.delete(id);
  appDataStore.setState({ projectTerminals, projectPreferences: new Map([...appDataStore.getState().projectPreferences].filter(([, value]) => value.project.id !== id)) });
  usageDetails.clearCache();
}

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
    forgetProjectData(id);
    appData.projects = appData.projects.filter(project => project.id !== id); renderProjectLists();
    forgetProjectDialog.close(); await refreshProjects();
  } catch (error) { element('forget-project-error').textContent = String(error); }
  finally { forgetting = false; setBusy(confirm, false); confirm.disabled = false; }
});

async function refreshProjects(): Promise<void> {
  const request = ++projectListRequest;
  try {
    const result = await invoke<Project[]>('recent_projects');
    if (request !== projectListRequest) return;
    appData.projectsLoaded = true;
    appData.projects = result;
    renderProjectLists();
  } catch (error) { if (request === projectListRequest) { appData.projectsLoaded = true; renderProjectLists(); element('empty-projects').hidden = true; showFolderError(String(error)); } }
}

async function rescanProjects(): Promise<void> {
  if (projectRescan.scanning() || appData.selectedProject || folderChoice.pending() || projectSelection.pending() || preparingSelected() || terminalLaunch.blocksChanges()) return;
  const scan = projectRescan.begin();
  if (scan === null) return;
  const initial = scan.kind === 'initial';
  if (initial) {
    element('empty-projects').hidden = true;
    projectRescanStatus('Checking project folders…', true);
    setWorkspaceStatus('project-folders', 'Checking project folders…', 'recent-projects');
  }
  const request = ++projectListRequest;
  void syncMenus();
  try {
    const result = await invoke<Project[]>('rescan_projects');
    if (request !== projectListRequest) return;
    const changed = initial || projectRescanError !== null || JSON.stringify(appData.projects) !== JSON.stringify(result);
    projectRescanError = null;
    appData.projectsLoaded = true;
    appData.projects = result;
    if (changed) renderProjectLists();
    if (!changed) renderProjectAvailability();
  } catch (error) {
    if (request === projectListRequest) {
      appData.projectsLoaded = true;
      projectRescanError = 'Could not check project folders. Tugboat will try again automatically. ' + String(error);
      renderProjectLists();
      element('empty-projects').hidden = true;
    }
  } finally {
    projectRescan.finish(scan.revision);
    if (initial) setWorkspaceStatus('project-folders', null, 'recent-projects');
    if (request !== projectListRequest) void refreshProjects();
    void syncMenus();
  }
}

async function refreshConversations(saved = false): Promise<void> {
  if (!appData.selectedProject) return;
  const projectId = appData.selectedProject.id;
  const request = dashboardState.begin();
  if (!request) return;
  renderSessionDashboard();
  const status = element('session-list-status');
  status.hidden = true;
  status.textContent = 'Loading sessions…';
  element<HTMLButtonElement>('refresh-sessions').disabled = true;
  setBusy(element('refresh-sessions'), true);
  try {
    const result = await invoke<Dashboard>(saved ? 'saved_project_dashboard' : 'project_dashboard', { identifier: projectId });
    if (!dashboardState.accepts(request) || appData.selectedProject?.id !== projectId) return;
    if (!applyDashboard(saved ? { ...result, sessionAvailability: 'unverified' } : result, request)) return;
    const hasActive = standaloneConversations(selectedConversations(), 'active').length > 0 || appData.terminals.some(terminal => terminal.running && isChat(terminal));
    status.textContent = hasActive ? '' : 'No active sessions. Start a New Chat or restore an archived session.';
    status.hidden = hasActive;
  } catch (error) {
    if (appData.selectedProject?.id === projectId && dashboardState.fail(request, String(error))) {
      status.textContent = dashboardState.error();
      status.hidden = false;
    }
  } finally {
    if (dashboardState.accepts(request)) {
      dashboardState.finish(request);
      setBusy(element('refresh-sessions'), false);
      renderSessionDashboard();
      element<HTMLButtonElement>('refresh-sessions').disabled = terminalLaunch.blocksChanges() || projectSelection.pending() || preparingSelected();
    }
  }
}

function applyDashboard(result: Dashboard, delivery: DashboardRequest | string): boolean {
  const accepted = typeof delivery === 'string' ? dashboardState.event(delivery, result) : dashboardState.snapshot(delivery, result);
  if (!accepted) return false;
  renderSessionSidebar();
  if (selectedConversationId && !terminalPanel.hidden) {
    const current = selectedConversations().find(conversation => conversationKey(conversation) === selectedConversationId);
    if (current) showTerminalStatus(current.title);
  }
  renderSessionDashboard();
  usageDetails.refresh();
  return true;
}
function renderSessionDashboard(): void {
  const dashboard = dashboardState.current();
  renderDashboard(dashboard ?? emptyDashboard, sessionView, resumeConversation, changeConversationState, usageDetails.open, conversationChange.pending() || terminalLaunch.blocksChanges() || projectSelection.pending() || folderChoice.pending());
  const dashboardLoaded = dashboardState.loaded();
  const dashboardError = dashboardState.error();
  const loading = !dashboardLoaded && !dashboardError;
  const checkingSessions = dashboard?.sessionAvailability === 'unverified' && !dashboardError
    && (planRequests.pending() || preparingSelected() || dashboardState.pending());
  for (const id of ['session-table', 'sidebar-session-list']) element(id).classList.toggle('is-checking-sessions', checkingSessions);
  const liveChat = appData.terminals.some(terminal => terminal.running && isChat(terminal));
  setLoading(element('project-usage'), loading);
  setLoading(element('session-table'), loading);
  setLoading(element('sidebar-session-list'), loading && !liveChat);
  if (!dashboardLoaded && dashboardError && !liveChat) element('sidebar-session-list').replaceChildren();
  element('project-usage').hidden = !dashboardLoaded && !loading;
  usageInfo.hidden = !dashboardLoaded;
  if (!dashboardLoaded) usageInfo.open = false;
  if (loading) {
    for (const value of element('project-usage').querySelectorAll('dd')) { value.replaceChildren(); value.setAttribute('aria-hidden', 'true'); }
    renderSkeleton(element('session-table-body'), 7);
    if (!liveChat) renderSkeleton(element('sidebar-session-list'));
    element('session-table').hidden = false;
  }
  element('project-overview').setAttribute('aria-busy', String(dashboardState.pending() || (!dashboardLoaded && !dashboardError)));
  if (!dashboardLoaded || dashboardError) {
    const status = element('dashboard-session-status');
    status.textContent = dashboardError ? dashboardError + ' Select Refresh to try again.' : '';
    status.hidden = !dashboardError;
    if (!dashboardLoaded && !loading) element('session-table').hidden = true;
  }
  const message = conversationChange.pending() ? 'Updating session…'
    : loading ? 'Loading sessions and usage…'
    : dashboardState.pending() ? 'Refreshing sessions and usage…'
    : dashboard?.sessionAvailability === 'unverified' && !dashboardError ? savedSessionsStatus : null;
  setWorkspaceStatus('project-sessions', appData.selectedProject ? message : null, 'project-dashboard');
}
async function changeConversationState(conversation: Conversation, state: SessionState): Promise<void> {
  if (!appData.selectedProject || conversationChange.pending() || !isStandaloneConversation(conversation)) return;
  const context = projectContext()!;
  const request = conversationChange.begin();
  const projectId = context.id;
  renderSessionDashboard();
  try {
    await invoke('change_conversation_state', { identifier: projectId, environmentId: conversation.environmentId, sessionId: conversation.id, state });
    if (sameProject(projectContext(), context)) { folderError.hidden = true; await refreshConversations(); }
  } catch (error) { if (sameProject(projectContext(), context)) showFolderError(String(error)); }
  finally { if (conversationChange.accepts(request)) { conversationChange.finish(request); renderSessionDashboard(); if (sameProject(projectContext(), context) && !document.querySelector('dialog[open]')) document.querySelector<HTMLButtonElement>('[data-session-state="' + sessionView + '"]')?.focus(); } }
}
function selectedUsageFingerprint(sessionId: string, environmentId: string | null): string {
  const children = new Map<string, Conversation[]>();
  const scoped = selectedConversations().filter(conversation => (conversation.environmentId ?? null) === environmentId);
  const nodesById = new Map(scoped.map(conversation => [conversation.id, conversation]));
  for (const conversation of scoped) {
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
  const nodes = scoped.filter(conversation => members.has(conversation.id)).sort((first, second) => first.id < second.id ? -1 : first.id > second.id ? 1 : 0);
  return JSON.stringify([usageRecordRevision, sessionId, nodes.map(conversation => [conversation.id, conversation.usageRevision, conversation.relationStatus, conversation.rootSessionId, conversation.coverage, conversation.title, conversation.updatedAt, conversation.available, conversation.warning, conversation.subagentCount, conversation.lineage.kind, conversation.lineage.parentSessionId, conversation.lineage.agentPath, conversation.lineage.nickname, conversation.lineage.sourceVersion, conversation.lineage.lineageVersion, conversation.usage, conversation.familyUsage])]);
}

async function selectAgent(kind: Project['agentKind'], prepare = true, expectedEnvironment?: string | null): Promise<void> {
  if (!appData.selectedProject || terminalLaunch.blocksChanges()) return;
  const previousContext = projectContext()!;
  const projectId = appData.selectedProject.id, request = agentSelection.begin(); void syncMenus();
  try {
    const project = await invoke<Project>('ensure_project_environment', { identifier: projectId, agentKind: kind });
    if (appData.selectedProject?.id !== projectId || !agentSelection.accepts(request)) return;
    if (project.agentKind !== kind || (expectedEnvironment && project.environmentId !== expectedEnvironment)) throw new Error('This chat belongs to another environment. Refresh the project list.');
    await hideTerminal(); closeProjectDialogs();
    terminalIdentifier = null; appData.selectedProject = project; ++projectEpoch; planRequests.invalidate();
    pendingProjectRecovery = null;
    selectedPlanFingerprint = null; appData.plan = null; resetProjectDetails(); resetBoundaryChoice();
    connectedAccountProject = null; accountSyncKey = null; accountConnectionError = null;
    element<HTMLSelectElement>('agent-choice').value = kind;
    const reference = element('project-reference');
    reference.textContent = project.profile + '@' + project.label;
    reference.title = reference.textContent;
    folderError.hidden = true; updateCodexActions();
    if (!prepare) { await refreshSelectedPlan(); return; }
    await prepareSelectedProject();
    if (appData.selectedProject?.id === projectId && appData.selectedProject.environmentId === project.environmentId) await ensureSelectedEnvironment();
  } catch (error) {
    if (appData.selectedProject?.id === projectId && agentSelection.accepts(request)) {
      element<HTMLSelectElement>('agent-choice').value = appData.selectedProject.agentKind;
      showFolderError(String(error));
      if (sameProject(projectContext(), previousContext)) void ensureSelectedEnvironment().then(
        () => {
          if (!sameProject(projectContext(), previousContext)) return;
          showFolderError(String(error));
          if (selectedPlanFingerprint) void refreshConversations();
        },
        failure => { if (sameProject(projectContext(), previousContext)) showFolderError(String(error) + ' ' + String(failure)); },
      );
    }
  }
  finally { if (agentSelection.accepts(request)) { agentSelection.finish(request); updateCodexActions(); void syncMenus(); } }
}
element<HTMLSelectElement>('agent-choice').addEventListener('change', event => {
  const kind = (event.target as HTMLSelectElement).value as Project['agentKind'];
  void selectAgent(kind);
});

async function resumeConversation(conversation: Conversation): Promise<void> {
  if (!appData.selectedProject || terminalLaunch.blocksChanges() || projectSelection.pending() || !isStandaloneConversation(conversation)) return;
  const terminal = appData.terminals.find(status => status.running && (status.identifier === conversation.terminalId || chatConversationId(status) === conversationKey(conversation)));
  if (terminal) {
    showView('project');
    folderError.hidden = true;
    await returnToTerminal(terminal.identifier);
    return;
  }
  const projectId = appData.selectedProject.id;
  if ((conversation.agentKind ?? 'codex') !== appData.selectedProject.agentKind || (conversation.environmentId && conversation.environmentId !== appData.selectedProject.environmentId)) await selectAgent(conversation.agentKind ?? 'codex', false, conversation.environmentId);
  if (appData.selectedProject?.id !== projectId || (conversation.environmentId && appData.selectedProject.environmentId !== conversation.environmentId)) return;
  showView('project');
  if (preparingSelected()) { showFolderError('Wait for this environment setup to finish.'); return; }
  if (appData.selectedProject.stage !== 'environment-ready') {
    showFolderError('Prepare the environment before resuming this session.');
    await showProjectSettings();
    element('prepare-environment').focus();
    return;
  }
  if (!selectedPlanFingerprint) await refreshSelectedPlan();
  if (appData.selectedProject?.id !== projectId || !selectedPlanFingerprint) return;
  await openTerminal('start_agent_session', { identifier: projectId, environmentId: appData.selectedProject.environmentId, fingerprint: selectedPlanFingerprint, action: 'resume', sessionId: conversation.id, useBrigBoundary: appData.selectedProject.agentKind === 'claude-code' || useBrigBoundary() }, conversation.title);
}

async function selectProject(project: Project, recordOpen = false): Promise<void> {
  if (terminalLaunch.blocksChanges() || projectSelection.pending() || preparingSelected()) { showFolderError('Wait for project setup to finish.'); return; }
  const selectionTicket = projectSelection.begin();
  const triggers = [...document.querySelectorAll<HTMLElement>('[data-project-id]')].filter(button => button.dataset.projectId === project.id);
  for (const trigger of triggers) setBusy(trigger, true);
  void syncMenus();
  try { await applySelectedProject(recordOpen ? await invoke<Project>('open_project', { identifier: project.id }) : project); }
  catch (error) { showFolderError(String(error)); }
  finally { for (const trigger of triggers) setBusy(trigger, false); projectSelection.finish(selectionTicket); updateCodexActions(); void syncMenus(); }
}

async function applySelectedProject(project: Project): Promise<void> {
  await hideTerminal();
  terminalIdentifier = null;
  appData.selectedProject = project;
  pendingProjectRecovery = null;
  const existing = appData.projects.find(item => item.id === project.id);
  if (!existing) ++projectListRequest;
  appData.projects = existing
    ? appData.projects.map(item => item.id === project.id ? { ...item, ...project } : item)
    : [...appData.projects, project];
  ++projectEpoch; ++terminalViewRequest; conversationChange.invalidate();
  resetProjectDetails();
  element('preparation-status').textContent = '';
  appData.plan = null;
  element('sharing-plan-values').replaceChildren();
  selectedConversationId = null;
  appData.terminals = appDataStore.getState().projectTerminals.get(project.id) ?? [];
  dashboardState.reset(appData.selectedProject?.id ?? null);
  setBusy(element('refresh-sessions'), false);
  sessionView = 'active'; renderSessionDashboard();
  renderSessionSidebar();
  element('session-list-status').textContent = dashboardState.loaded() ? 'No active sessions. Start a New Chat or restore an archived session.' : 'Loading sessions…';
  element('session-list-status').hidden = !dashboardState.loaded() || standaloneConversations(selectedConversations(), 'active').length > 0;
  renderProjectLists();
  projectView.classList.add('has-project');
  selectedPlanFingerprint = null;
  planRequests.invalidate(); planFeedback(null);
  resetBoundaryChoice();
  folderError.hidden = true;
  element('codex-actions').hidden = true;
  element('sharing-plan').hidden = true;
  element('project-heading').textContent = project.name;
  element('project-heading').title = project.name;
  element('project-description').hidden = true;
  element('project-meta').hidden = false;
  chooseFolderButton.hidden = true;
  element<HTMLSelectElement>('agent-choice').value = project.agentKind;
  element('project-settings-button').hidden = false;
  element('project-dashboard').hidden = false;
  const path = element('selected-folder');
  path.textContent = project.path;
  path.title = project.path;
  path.hidden = false;
  startTerminalButton.hidden = false;
  const reference = element('project-reference');
  reference.textContent = project.profile + '@' + project.label;
  reference.title = reference.textContent;
  reference.hidden = false;
  element('project-actions').hidden = false;
  showView('project');
  if (!dashboardState.loaded()) void refreshConversations(true);
  void finishOpeningProject(project, projectEpoch);
}

async function finishOpeningProject(project: Project, epoch: number): Promise<void> {
  const current = () => appData.selectedProject?.id === project.id && appData.selectedProject.environmentId === project.environmentId && projectEpoch === epoch;
  try {
    if (!appDataStore.getState().projectTerminals.has(project.id)) await refreshTerminals();
    if (!current()) return;
    const status = currentTerminal();
    const preview = status?.running || (project.policy?.fingerprint && !project.policyDetached && ['ready', 'environment-ready'].includes(project.stage));
    if (preview) await refreshSelectedPlan();
    if (!preview) await prepareSelectedProject();
    if (!current()) return;
    updateCodexActions();
    if (!agentSelection.pending()) await ensureSelectedEnvironment();
    if (!current()) return;
    if (current() && (!dashboardState.loaded() || dashboardState.current()?.sessionAvailability === 'unverified')) void refreshConversations();
  } catch (error) { if (current()) showFolderError(String(error)); }
}

async function hideTerminal(): Promise<void> {
  terminalPanel.hidden = true;
  await updateTerminalLayout();
}

async function returnToTerminal(identifier?: number): Promise<void> {
  if (!appData.selectedProject) return;
  const projectId = appData.selectedProject.id;
  const request = ++terminalViewRequest;
  closeProjectDialogs();
  await refreshTerminals();
  const status = identifier === undefined ? currentTerminal() : appData.terminals.find(status => status.identifier === identifier);
  if (!status || appData.selectedProject?.id !== projectId || request !== terminalViewRequest) return;
  if ((status.environmentId && status.environmentId !== appData.selectedProject.environmentId) || (!status.environmentId && status.purpose !== 'local' && appData.selectedProject.agentKind !== 'codex')) await selectAgent(status.agentKind ?? 'codex', false, status.environmentId);
  await hideTerminal();
  if (appData.selectedProject?.id !== projectId || request !== terminalViewRequest) return;
  terminalIdentifier = status.identifier;
  lastTerminals.set(projectId, status.identifier);
  updateTerminalActions();
  renderSessionSidebar();
  terminalPanel.hidden = false;
  const title = selectedConversations().find(conversation => conversationKey(conversation) === selectedConversationId)?.title;
  showTerminalStatus(status.running ? (title ?? terminalNames[status.purpose] ?? 'Operation terminal') : terminalCaption(status));
  await updateTerminalLayout();
  if (!projectView.hidden && request === terminalViewRequest) await focusTerminal(status.identifier);
}

async function refreshSelectedPlan(): Promise<void> {
  if (!appData.selectedProject) return;
  const id = appData.selectedProject.id;
  const environmentId = appData.selectedProject.environmentId;
  const request = planRequests.begin();
  const previousFingerprint = selectedPlanFingerprint;
  selectedPlanFingerprint = null;
  appData.plan = null;
  element('sharing-plan').hidden = true;
  element('prepare-environment').hidden = true;
  planFeedback('Loading the current execution plan…', false, true);
  void syncMenus();
  try {
    const result = await invoke<{ project: Project; plan: Plan; fingerprint: string }>('preview_project', { identifier: id, environmentId });
    if (appData.selectedProject?.id !== id || appData.selectedProject.environmentId !== environmentId || !planRequests.accepts(request)) return;
    if (result.project.id !== id || result.project.environmentId !== environmentId) throw new Error('The execution plan belongs to another environment.');
    if (previousFingerprint !== result.fingerprint) resetBoundaryChoice();
    appData.selectedProject = result.project;
    selectedPlanFingerprint = result.fingerprint;
    planRequests.finish(request);
    planFeedback(null);
    appData.plan = result.plan;
    renderPlan(appData.plan!);
    element('prepare-environment').hidden = false;
    element('refresh-plan').hidden = false;
    updateCodexActions();
    element('preparation-status').textContent = 'Execution plan refreshed.';
    environmentFeedback();
  } catch (error) {
    if (appData.selectedProject?.id !== id || appData.selectedProject.environmentId !== environmentId || !planRequests.accepts(request)) return;
    selectedPlanFingerprint = null;
    element('sharing-plan').hidden = true;
    element('refresh-plan').hidden = true;
    element('preparation-status').textContent = String(error);
    planRetryAction = 'preview'; planFeedback(String(error), true);
  } finally { if (planRequests.accepts(request)) { planRequests.finish(request); updateCodexActions(); void syncMenus(); } finishPendingRecordPlanRefresh(); }
}

async function prepareSelectedProject(chosenReference: string | null = null): Promise<void> {
  if (appData.selectedProject === null) return;
  const project = appData.selectedProject;
  const existing = preparingEnvironments.get(preparationKey(project));
  if (existing && chosenReference !== null) { planFeedback('Wait for this environment setup to finish.'); return; }
  const work = existing ?? prepareProject(project, chosenReference);
  preparingEnvironments.set(preparationKey(project), work);
  selectedPlanFingerprint = null;
  appData.plan = null;
  const request = planRequests.begin();
  planFeedback('Preparing the execution plan…', false, true);
  resetBoundaryChoice();
  void syncMenus();
  element('preparation-status').textContent = 'Preparing Brig project configuration…';
  element('retry-preparation').hidden = true;
  element('session-candidates').hidden = true;
  element('sharing-plan').hidden = true;
  element('codex-actions').hidden = true;
  element('prepare-environment').hidden = true;
  element('refresh-plan').hidden = true;
  try {
    const result = await work;
    if (appData.selectedProject?.id !== project.id || appData.selectedProject.environmentId !== project.environmentId || !planRequests.accepts(request)) return;
    if (result.project.id !== project.id || result.project.environmentId !== project.environmentId) throw new Error('The execution plan belongs to another environment.');
    appData.selectedProject = result.project;
    selectedPlanFingerprint = result.fingerprint;
    planRequests.finish(request);
    planRetryAction = 'prepare';
    planFeedback(result.plan ? null : result.message, !result.plan && !result.candidates.length && result.project.stage === 'environment-unverified');
    const reference = element('project-reference');
    reference.textContent = result.project.profile + '@' + result.project.label;
    reference.title = reference.textContent;
    element('preparation-status').textContent = result.message ?? 'Brig configuration ready.';
    if (result.plan) {
      appData.plan = result.plan;
      renderPlan(appData.plan!);
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
    if (appData.selectedProject?.id !== project.id || appData.selectedProject.environmentId !== project.environmentId || !planRequests.accepts(request)) return;
    element('preparation-status').textContent = String(error);
    element('retry-preparation').hidden = false;
    planRetryAction = 'prepare'; planFeedback(String(error), true);
  } finally { if (preparingEnvironments.get(preparationKey(project)) === work) preparingEnvironments.delete(preparationKey(project)); if (planRequests.accepts(request)) planRequests.finish(request); updateCodexActions(); void syncMenus(); finishPendingRecordPlanRefresh(); }
}

function updateCodexActions(): void {
  const ready = appData.selectedProject?.stage === 'environment-ready';
  element('resume-removal').hidden = appData.selectedProject?.stage !== 'removing';
  element('codex-actions').hidden = !ready;
  element('project-overview').hidden = appData.selectedProject === null;
  element('project-setup').hidden = ready || planRequests.pending() || projectSelection.pending() || preparingSelected()
    || appData.selectedProject?.stage === 'environment-starting' || selectedPlanFingerprint !== null;
  element<HTMLButtonElement>('start-codex').disabled = !canStartChat() || terminalLaunch.blocksChanges() || agentSelection.pending() || projectSelection.pending() || folderChoice.pending() || preparingSelected();
  const claude = appData.selectedProject?.agentKind === 'claude-code';
  const accountBusy = !claude && accountSync.pending() && environmentReady();
  element('project-account-feedback').hidden = !environmentReady() || canStartChat() || accountBusy;
  element('project-account-status').textContent = claude ? claudeAccount?.status()?.message ?? 'Connect your shared Claude account in Settings, Providers.' : accountConnectionError ?? (accountSync.pending()
    ? 'Connecting the shared ChatGPT account…' : sharedChatgpt?.status()?.message ?? 'Checking the shared ChatGPT account…');
  setBusy(element('project-account-status'), !claude && accountSync.pending());
  setWorkspaceStatus('project-account', accountBusy ? element('project-account-status').textContent : null, 'project-dashboard');
  element('codex-boundary-options').hidden = claude;
  claudeAccount?.select();
  element<HTMLButtonElement>('guest-shell').disabled = !environmentReady() || terminalLaunch.blocksChanges() || projectSelection.pending() || folderChoice.pending() || preparingSelected();
  sharedChatgpt?.updateAvailability();
  if (ready && !claude) void syncProjectAccount();
}

for (const [id, action, caption] of [['start-codex', 'new-chat', 'New Chat'], ['guest-shell', 'shell', 'Guest terminal']] as const) {
  element(id).addEventListener('click', () => {
    if (appData.selectedProject === null || selectedPlanFingerprint === null) return;
    void openTerminal('start_agent_session', { identifier: appData.selectedProject.id, environmentId: appData.selectedProject.environmentId, fingerprint: selectedPlanFingerprint, action, useBrigBoundary: appData.selectedProject.agentKind === 'claude-code' || useBrigBoundary() }, caption);
  });
}
element('project-provider-settings').addEventListener('click', () => {
  showView('settings'); selectSettingsTab('providers'); element(appData.selectedProject?.agentKind === 'claude-code' ? 'claude-account-heading' : 'shared-chatgpt-heading').focus();
});

function showView(view: 'project' | 'settings'): void {
  closeProjectDialogs();
  const settings = view === 'settings';
  projectView.hidden = settings;
  settingsView.hidden = !settings;
  renderWorkspaceStatus();
  projectNavigation.removeAttribute('aria-current');
  settingsNavigation.removeAttribute('aria-current');
  element('sidebar-current-project').removeAttribute('aria-current');
  (settings ? settingsNavigation : appData.selectedProject ? element('sidebar-current-project') : projectNavigation).setAttribute('aria-current', 'page');
  (settings ? element('settings-heading') : element('project-heading')).focus();
  scheduleProjectRescan();
  sharedChatgpt?.updateAvailability();
  void updateTerminalLayout();
  if (!settings && !terminalPanel.hidden && terminalIdentifier !== null) void focusTerminal(terminalIdentifier).catch(error => { showFolderError(String(error)); });
}

async function showProjects(message = 'Choose where Brig and your agents will work.'): Promise<void> {
  if (terminalLaunch.blocksChanges() || projectSelection.pending() || preparingSelected()) return;
  await hideTerminal();
  appData.selectedProject = null;
  pendingProjectRecovery = null;
  ++projectEpoch; ++terminalViewRequest; conversationChange.invalidate();
  resetProjectDetails();
  element('preparation-status').textContent = '';
  appData.plan = null;
  element('sharing-plan-values').replaceChildren();
  terminalIdentifier = null;
  appData.terminals = [];
  selectedPlanFingerprint = null;
  planRequests.invalidate(); planFeedback(null);
  selectedConversationId = null;
  dashboardState.reset(null);
  setBusy(element('refresh-sessions'), false);
  renderConversations([], null, resumeConversation);
  resetBoundaryChoice();
  projectView.classList.remove('has-project');
  updateTerminalActions();
  for (const id of ['project-actions', 'project-meta', 'selected-branch', 'project-settings-button', 'project-reference', 'project-dashboard']) element(id).hidden = true;
  startTerminalButton.hidden = true;
  element('project-heading').textContent = 'Projects';
  element('project-heading').removeAttribute('title');
  element('project-description').textContent = message;
  element('project-description').hidden = false;
  chooseFolderButton.hidden = false;
  claudeAccount?.select();
  folderError.hidden = true;

  renderProjectLists();
  showView('project');
  void syncMenus();
}

async function chooseFolder(): Promise<void> {
  if (folderChoice.pending() || appData.selectedProject) return;
  if (terminalLaunch.blocksChanges() || preparingSelected()) {
    folderError.textContent = 'Wait for project setup to finish.';
    folderError.hidden = false;
    return;
  }
  const choiceTicket = folderChoice.begin();
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
    folderChoice.finish(choiceTicket);
    chooseFolderButton.disabled = false;
    void syncMenus();
    if (!projectView.hidden && !document.querySelector('dialog[open]') && terminalPanel.hidden) element('project-heading').focus();
    if (!projectView.hidden && !document.querySelector('dialog[open]') && !terminalPanel.hidden && terminalIdentifier !== null) void focusTerminal(terminalIdentifier).catch(error => { showFolderError(String(error)); });
  }
}

function closeProjectDialogs(): void {
  for (const dialog of [projectSettingsDialog, usageDialog, element<HTMLDialogElement>('removal-dialog')]) if (dialog.open) dialog.close();
}

function projectPreferencesKey(project: Pick<Project, 'id' | 'environmentId'>): string {
  return JSON.stringify([project.id, project.environmentId]);
}

function rememberProjectPreferences(value: ProjectPreferences): void {
  const cached = new Map(appDataStore.getState().projectPreferences);
  cached.set(projectPreferencesKey(value.project), value);
  appDataStore.setState({ projectPreferences: cached });
}

async function refreshCachedProjectPreferences(settingsChanged: boolean): Promise<void> {
  const revision = ++cachedProjectRecordRevision;
  const selectedKey = appData.selectedProject ? projectPreferencesKey(appData.selectedProject) : null;
  const entries = [...appDataStore.getState().projectPreferences].filter(([key]) => key !== selectedKey);
  await Promise.all(entries.map(async ([key, previous]) => {
    try {
      const result = await invoke<ProjectPreferences>('read_project_preferences', { identifier: previous.project.id, environmentId: previous.project.environmentId });
      if (revision !== cachedProjectRecordRevision || appDataStore.getState().projectPreferences.get(key) !== previous) return;
      if (result.project.id !== previous.project.id || result.project.environmentId !== previous.project.environmentId) throw new Error('The project settings belong to another environment.');
      if (appData.selectedProject && projectPreferencesKey(appData.selectedProject) === key) {
        void refreshSelectedProjectRecords(settingsChanged);
        return;
      }
      rememberProjectPreferences(result);
    } catch (error) {
      if (revision !== cachedProjectRecordRevision || appDataStore.getState().projectPreferences.get(key) !== previous) return;
      const cached = new Map(appDataStore.getState().projectPreferences);
      cached.delete(key);
      appDataStore.setState({ projectPreferences: cached });
      if (appData.selectedProject && projectPreferencesKey(appData.selectedProject) === key) void refreshSelectedProjectRecords(settingsChanged);
      console.warn('Could not refresh cached project settings.', error);
    }
  }));
}

function projectPreferenceFields(value: ProjectPreferences): Record<string, string> {
  return {
    'project-name': value.project.name,
    'project-cpus': String(value.options.cpus),
    'project-memory': String(value.options.memoryMb),
    'project-verification': value.options.verification,
    'project-network': value.options.network,
  };
}

function renderProjectPreferences(value: ProjectPreferences, previous: ProjectPreferences | null): void {
  const original = previous ? projectPreferenceFields(previous) : null;
  for (const [id, next] of Object.entries(projectPreferenceFields(value))) {
    const field = element<HTMLInputElement | HTMLSelectElement>(id);
    if (!original || field.value === original[id]) field.value = next;
  }
}

function projectPlanInputs(project: Project): string {
  return JSON.stringify([project.path, project.profile, project.label, project.network, project.options, project.policy, project.policyDetached, project.ownsBinding]);
}

function finishPendingRecordPlanRefresh(): void {
  const context = pendingRecordPlanRefresh;
  if (!context || planRequests.pending()) return;
  pendingRecordPlanRefresh = null;
  if (sameProject(projectContext(), context) && selectedPlanFingerprint) void refreshSelectedPlan();
}

async function refreshSelectedProjectRecords(settingsChanged = false): Promise<void> {
  const context = projectContext();
  if (!context || !appData.selectedProject) return;
  const revision = ++projectRecordRevision;
  if (projectPreferences.busy()) { pendingProjectRecordRefresh = true; pendingProjectSettingsRefresh ||= settingsChanged; return; }
  const selected = appData.selectedProject;
  try {
    const result = await invoke<ProjectPreferences>('read_project_preferences', { identifier: context.id, environmentId: context.environmentId });
    if (revision !== projectRecordRevision || !sameProject(projectContext(), context)) return;
    if (projectPreferences.busy()) { pendingProjectRecordRefresh = true; pendingProjectSettingsRefresh ||= settingsChanged; return; }
    if (appData.selectedProject !== selected) { void refreshSelectedProjectRecords(settingsChanged); return; }
    if (result.project.id !== context.id || result.project.environmentId !== context.environmentId) throw new Error('The project settings belong to another environment.');
    const previous = appDataStore.getState().projectPreferences.get(projectPreferencesKey(selected)) ?? null;
    const planChanged = settingsChanged || projectPlanInputs(selected) !== projectPlanInputs(result.project)
      || (previous !== null && JSON.stringify(previous.options) !== JSON.stringify(result.options));
    rememberProjectPreferences(result);
    if (projectSettingsDialog.open && projectPreferences.current()) {
      const original = projectPreferences.current();
      projectPreferences.replace(result);
      renderProjectPreferences(projectPreferences.current()!, original);
    }
    appData.selectedProject = result.project;
    element('project-heading').textContent = result.project.name;
    element('project-heading').title = result.project.name;
    element('selected-folder').textContent = result.project.path;
    element('selected-folder').title = result.project.path;
    element('project-reference').textContent = result.project.profile + '@' + result.project.label;
    element('project-reference').title = element('project-reference').textContent;
    renderProjectLists(); updateCodexActions();
    if (planChanged && (selectedPlanFingerprint || planRequests.pending())) {
      pendingRecordPlanRefresh = context;
      finishPendingRecordPlanRefresh();
    }
    if (pendingProjectRecovery && sameProject(context, pendingProjectRecovery)) {
      const recovery = pendingProjectRecovery;
      pendingProjectRecovery = null;
      folderError.hidden = true;
      void finishOpeningProject(result.project, recovery.epoch);
    }
  } catch (error) {
    if (revision !== projectRecordRevision || !sameProject(projectContext(), context)) return;
    selectedPlanFingerprint = null; appData.plan = null; planRequests.invalidate(); updateCodexActions();
    showFolderError('Could not update the project records. ' + String(error));
  }
}

function finishProjectRecordRefresh(): void {
  if (!pendingProjectRecordRefresh) return;
  pendingProjectRecordRefresh = false;
  const settingsChanged = pendingProjectSettingsRefresh;
  pendingProjectSettingsRefresh = false;
  void refreshSelectedProjectRecords(settingsChanged);
}

async function showProjectSettings(returnFocusId = 'project-settings-button'): Promise<void> {
  if (!appData.selectedProject) return;
  resetProjectDetails();
  const context = projectContext()!;
  const projectId = context.id;
  const request = projectPreferences.read();
  const initial = appDataStore.getState().projectPreferences.get(projectPreferencesKey(appData.selectedProject));
  element<HTMLFieldSetElement>('project-preference-fields').disabled = true;
  if (!initial) {
    setLoading(element('project-preference-fields'), true);
    element('project-preferences-status').textContent = 'Loading project settings…';
  }
  element('managed-environment').textContent = agentName(appData.selectedProject.agentKind) + ' · ' + appData.selectedProject.profile + '@' + appData.selectedProject.label;
  showDialog(projectSettingsDialog, returnFocusId); await updateTerminalLayout();
  if (!sameProjectView(projectContext(), context) || !projectPreferences.accepts(request) || !projectSettingsDialog.open) return;
  const cached = appDataStore.getState().projectPreferences.get(projectPreferencesKey(appData.selectedProject!));
  if (cached) {
    projectPreferences.receive(request, cached);
    projectPreferences.finish(request);
    renderProjectPreferences(projectPreferences.current()!, null);
    element<HTMLFieldSetElement>('project-preference-fields').disabled = false;
    element('project-preferences-status').textContent = '';
    setLoading(element('project-preference-fields'), false);
    finishProjectRecordRefresh();
    return;
  }
  setLoading(element('project-preference-fields'), true);
  element('project-preferences-status').textContent = 'Loading project settings…';
  try {
    const result = await invoke<ProjectPreferences>('read_project_preferences', { identifier: projectId, environmentId: context.environmentId });
    if (!sameProjectView(projectContext(), context) || !projectPreferences.accepts(request) || !projectSettingsDialog.open) return;
    if (result.project.id !== context.id || result.project.environmentId !== context.environmentId) throw new Error('The project settings belong to another environment.');
    projectPreferences.receive(request, result);
    rememberProjectPreferences(result);
    renderProjectPreferences(projectPreferences.current()!, null);
    element<HTMLFieldSetElement>('project-preference-fields').disabled = false;
    element('project-preferences-status').textContent = '';
  } catch (error) { if (sameProjectView(projectContext(), context) && projectPreferences.accepts(request) && projectSettingsDialog.open) element('project-preferences-status').textContent = String(error); }
  finally { if (sameProjectView(projectContext(), context) && projectPreferences.accepts(request)) { projectPreferences.finish(request); setLoading(element('project-preference-fields'), false); finishProjectRecordRefresh(); } }
}
element('project-settings-button').addEventListener('click', () => { void showProjectSettings(); });
element('setup-project').addEventListener('click', () => { void showProjectSettings('setup-project'); });
element('close-project-settings').addEventListener('click', () => { projectSettingsDialog.close(); });
projectSettingsDialog.addEventListener('close', () => { void updateTerminalLayout(); });
projectSettingsDialog.addEventListener('close', () => { if (!projectSettingsDialog.open) resetProjectDetails(); });
element<HTMLFormElement>('project-preferences').addEventListener('submit', async event => {
  event.preventDefault(); if (!appData.selectedProject || !projectSettingsDialog.open) return;
  const submission = projectPreferences.submit();
  if (!submission) return;
  const context = projectContext()!; const id = context.id; const request = submission.revision; const fields = element<HTMLFieldSetElement>('project-preference-fields'); fields.disabled = true;
  element('project-preferences-status').textContent = 'Saving project settings…';
  setBusy(element('save-project-preferences'), true);
  const options = { cpus: Number(element<HTMLInputElement>('project-cpus').value), memoryMb: Number(element<HTMLInputElement>('project-memory').value), verification: element<HTMLSelectElement>('project-verification').value, network: element<HTMLSelectElement>('project-network').value };
  try {
    const project = await invoke<Project>('save_project_preferences', { identifier: id, environmentId: context.environmentId, name: element<HTMLInputElement>('project-name').value, options });
    if (!sameProject(projectContext(), context)) return;
    const saved = { project, options };
    projectPreferences.saved(request, saved);
    rememberProjectPreferences(saved);
    appData.selectedProject = project; element('project-heading').textContent = project.name; element('project-heading').title = project.name;
    await prepareSelectedProject(); await ensureSelectedEnvironment(); updateCodexActions(); await refreshProjects();
    if (sameProjectView(projectContext(), context) && projectPreferences.accepts(request)) element('project-preferences-status').textContent = 'Project settings saved.';
  } catch (error) { if (sameProjectView(projectContext(), context) && projectPreferences.accepts(request)) element('project-preferences-status').textContent = String(error); }
  finally { if (sameProjectView(projectContext(), context) && projectPreferences.accepts(request)) { projectPreferences.finish(request); setBusy(element('save-project-preferences'), false); fields.disabled = false; finishProjectRecordRefresh(); } }
});
for (const tab of document.querySelectorAll<HTMLButtonElement>('[data-session-state]')) tab.addEventListener('click', () => { sessionView = tab.dataset.sessionState as SessionState; renderSessionDashboard(); });
await listen<[string, Dashboard]>('project-usage-updated', event => {
  if (!appData.projects.some(project => project.id === event.payload[0])) return;
  if (appData.selectedProject?.id !== event.payload[0]) {
    dashboardState.event(event.payload[0], event.payload[1]);
    return;
  }
  if (!applyDashboard(event.payload[1], event.payload[0])) return;
  const hasActive = standaloneConversations(selectedConversations(), 'active').length > 0 || appData.terminals.some(terminal => terminal.running && isChat(terminal));
  element('session-list-status').hidden = hasActive;
  element('session-list-status').textContent = hasActive ? '' : 'No active sessions. Start a New Chat or restore an archived session.';
});
await listen<[string, string, number]>('project-usage-error', event => {
  if (appData.selectedProject?.id === event.payload[0] && dashboardState.errorEvent(event.payload[0], event.payload[2], event.payload[1])) {
    renderSessionDashboard();
  }
});
async function refreshRecordedDashboard(projectId: string): Promise<void> {
  try {
    const result = await invoke<Dashboard>('recorded_project_dashboard', { identifier: projectId });
    const previous = dashboardState.cachedDashboard(projectId);
    if (!previous) return;
    const availability = new Map(previous.conversations.map(conversation => [conversationKey(conversation), conversation.available]));
    const snapshot: Dashboard = {
      ...result,
      conversations: result.availabilityVerified === true ? result.conversations : result.conversations.map(conversation => ({ ...conversation, available: conversation.available && availability.get(conversationKey(conversation)) === true })),
      ...(result.availabilityVerified === false || (result.availabilityVerified !== true && previous.sessionAvailability === 'unverified') ? { sessionAvailability: 'unverified' } : {}),
    };
    if (appData.selectedProject?.id !== projectId) { dashboardState.event(projectId, snapshot); return; }
    applyDashboard(snapshot, projectId);
  } catch (error) {
    if (appData.selectedProject?.id === projectId) showFolderError('Could not update saved usage. ' + String(error));
  }
}
await listen<string[]>('records-changed', async event => {
  const projectsChanged = event.payload.includes('projects');
  const settingsChanged = event.payload.includes('settings');
  const updates: Promise<unknown>[] = [];
  if (projectsChanged || settingsChanged) {
    updates.push(refreshCachedProjectPreferences(settingsChanged));
    updates.push((async () => {
      if (projectsChanged) {
        await refreshProjects();
        const ids = new Set(appData.projects.map(project => project.id));
        const cachedIds = new Set([...dashboardState.projectIds(), ...appDataStore.getState().projectTerminals.keys(), ...[...appDataStore.getState().projectPreferences.values()].map(value => value.project.id)]);
        for (const id of cachedIds) if (!ids.has(id)) forgetProjectData(id);
        if (appData.selectedProject && !ids.has(appData.selectedProject.id)) {
          dashboardState.reset(null);
          selectedPlanFingerprint = null; appData.plan = null; planRequests.invalidate();
          renderSessionDashboard(); renderSessionSidebar(); updateCodexActions();
          showFolderError('This project is no longer in the saved records.');
          return;
        }
        if (appData.selectedProject && !dashboardState.matches(appData.selectedProject.id)) {
          pendingProjectRecovery = projectContext();
          dashboardState.reset(appData.selectedProject.id);
          renderSessionDashboard(); renderSessionSidebar();
          void refreshConversations(true);
        }
      }
      await refreshSelectedProjectRecords(settingsChanged);
    })());
  }
  if (event.payload.includes('usage')) {
    ++usageRecordRevision; usageDetails.clearCache();
    updates.push(Promise.all(dashboardState.projectIds().map(refreshRecordedDashboard)).then(usageDetails.refresh));
  }
  if (event.payload.includes('chatgpt') && sharedChatgpt) updates.push(sharedChatgpt.refresh());
  if (event.payload.includes('claude') && claudeAccount) updates.push(claudeAccount.refresh());
  await Promise.all(updates);
});

projectNavigation.addEventListener('click', () => { void showProjects(); });
element('sidebar-current-project').addEventListener('click', async () => { await hideTerminal(); showView('project'); });
element('refresh-sessions').addEventListener('click', () => { void refreshConversations(); });
settingsNavigation.addEventListener('click', () => showView('settings'));
chooseFolderButton.addEventListener('click', () => { void chooseFolder(); });
document.defaultView!.addEventListener('focus', () => {
  if (projectRescan.started() && !projectView.hidden && appData.selectedProject === null
    && (projectRescanError !== null || appData.projects.some(project => project.available === false))) void rescanProjects();
});
async function retryProjectSetup(action: 'prepare' | 'preview' | 'environment', chosenReference: string | null = null): Promise<void> {
  const context = projectContext();
  if (!context) return;
  try {
    if (action === 'prepare') await prepareSelectedProject(chosenReference);
    if (action === 'preview') await refreshSelectedPlan();
    if (!sameProject(projectContext(), context) || agentSelection.pending()) return;
    await ensureSelectedEnvironment();
    if (sameProject(projectContext(), context) && selectedPlanFingerprint) void refreshConversations();
  } catch (error) {
    if (sameProject(projectContext(), context)) {
      const message = 'Could not start the project environment. ' + String(error);
      environmentFeedback(message); showFolderError(message);
    }
  }
}
element('retry-preparation').addEventListener('click', () => { void retryProjectSetup('prepare'); });
element('retry-project-plan').addEventListener('click', () => { void retryProjectSetup(planRetryAction); });
element('resume-removal').addEventListener('click', async () => { await showProjectSettings('resume-removal'); element('preview-removal').click(); });
element('adopt-session').addEventListener('click', () => { void retryProjectSetup('prepare', element<HTMLSelectElement>('session-choice').value); });
element('refresh-plan').addEventListener('click', () => { void refreshSelectedPlan(); });
element('hide-terminal').addEventListener('click', async () => { await hideTerminal(); element('project-heading').focus(); });
element('prepare-environment').addEventListener('click', () => {
  if (appData.selectedProject === null || selectedPlanFingerprint === null) return;
  void openTerminal('prepare_environment', { identifier: appData.selectedProject.id, environmentId: appData.selectedProject.environmentId, fingerprint: selectedPlanFingerprint }, 'Preparing Brig environment…');
});
startTerminalButton.addEventListener('click', () => {
  if (appData.selectedProject) void openTerminal('terminal_start', { projectId: appData.selectedProject.id }, 'Local terminal');
});

async function openTerminal(...[command, arguments_, caption, foreground = true]: TerminalLaunchRequest): Promise<void> {
  if (appData.selectedProject === null || !terminalLaunch.begin()) return;
  const projectId = appData.selectedProject.id;
  const environmentId = appData.selectedProject.environmentId;
  const environmentKey = [projectId, environmentId].join('|');
  void syncMenus();
  let previous: TerminalStatus | null;
  try { await refreshTerminals(); previous = currentTerminal(); }
  catch (error) {
    terminalLaunch.finish(); void syncMenus();
    if (foreground) showFolderError(String(error));
    if (!foreground) environmentFeedback('Could not start the project environment. ' + String(error));
    return;
  }
  const sessionId = command === 'start_agent_session' && arguments_.action === 'resume' ? arguments_.sessionId : null;
  const existingChat = sessionId === null ? undefined : appData.terminals.find(status => status.running && terminalMatches(status, appData.selectedProject!) && status.conversationId === sessionId);
  const purpose = command === 'terminal_start' ? 'local' : command === 'prepare_environment' ? 'environment' : command === 'start_agent_session' ? arguments_.action : arguments_.operation;
  const launchingChat = command === 'start_agent_session' && (arguments_.action === 'new-chat' || arguments_.action === 'resume');
  const blocking = command === 'start_project_operation' ? undefined : appData.terminals.find(status => status.running
    && (status.purpose === 'local' || terminalMatches(status, appData.selectedProject!))
    && ['local', 'environment', 'login', 'codex', 'resume', 'claude', 'claude-resume', 'shell'].includes(status.purpose)
    && (foreground || status.purpose !== 'local')
    && !(launchingChat && isChat(status)));
  if (existingChat || blocking) {
    terminalLaunch.finish();
    void syncMenus();
    if (existingChat || blocking?.purpose === purpose) {
      if (!foreground) {
        if (purpose === 'environment') {
          environmentTerminals.set(environmentKey, blocking!.identifier);
          appData.selectedProject = { ...appData.selectedProject, stage: 'environment-starting' };
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
  terminalLaunch.start();
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
    pendingEnvironmentKey = environmentKey;
    appData.selectedProject = { ...appData.selectedProject, stage: 'environment-starting' };
    updateCodexActions(); environmentFeedback();
  }
  try {
    const layout = terminalLayout();
    const identifier = await invoke<number>(command, { ...arguments_, layout: { ...layout, visible: foreground && layout.visible } });
    terminalLaunch.publish();
    if (foreground) terminalIdentifier = identifier;
    if (preparingEnvironment) {
      environmentTerminals.set(environmentKey, identifier);
      pendingEnvironmentKey = null;
      await refreshSelectedPlan();
    }
    if (foreground) lastTerminals.set(projectId, identifier);
    await refreshTerminals();
    if (foreground) { showTerminalStatus(caption); await updateTerminalLayout(); }
    const snapshot = await invoke<TerminalStatus | null>('terminal_snapshot', { identifier });
    if (preparingEnvironment && snapshot && !snapshot.running) {
      appData.selectedProject = await invoke<Project>('finish_environment', { identifier: projectId, environmentId, terminalIdentifier: identifier });
      await refreshSelectedPlan();
      environmentFeedback(appData.selectedProject.stage === 'environment-ready' ? null : (appData.selectedProject.environmentAttempt?.error ?? 'Could not start the project environment. Check its terminal output and try again.'));
    }
    updateTerminalActions();
    if (foreground && snapshot && !snapshot.running) showTerminalStatus(terminalCaption(snapshot));
    if (foreground && !projectView.hidden) await focusTerminal(identifier);
  } catch (error) {
    if (preparingEnvironment) { pendingEnvironmentKey = null; await refreshSelectedPlan(); }
    if (foreground) { terminalPanel.hidden = true; terminalIdentifier = previous?.identifier ?? null; }
    if (!foreground && preparingEnvironment) environmentFeedback('Could not start the project environment. ' + String(error));
    if (foreground) showFolderError('Could not open the terminal. ' + String(error));
    chooseFolderButton.disabled = false;
  } finally {
    terminalLaunch.finish();
    startTerminalButton.disabled = false;
    chooseFolderButton.disabled = false;
    await refreshTerminals();
    void syncMenus();
  }
}
element('close-terminal').addEventListener('click', async () => {
  if (terminalIdentifier === null) return;
  const identifier = terminalIdentifier;
  if (!terminalLaunch.close(identifier)) return;
  updateTerminalActions();
  const projectId = appData.selectedProject?.id;
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
    if (terminalIdentifier === identifier) showTerminalStatus('Could not close the terminal. ' + String(error));
  } finally {
    terminalLaunch.closed(identifier); updateTerminalActions();
    if (terminalUpdatesAllowed(identifier)) void updateTerminalLayout();
  }
});
new ResizeObserver(() => { void updateTerminalLayout(); }).observe(terminalSurface);
initializeSidebar(() => { void updateTerminalLayout(); });
await listen<string>('terminal-sessions-changed', async event => {
  if (appData.selectedProject?.id !== event.payload && !appDataStore.getState().projectTerminals.has(event.payload)) return;
  try { await refreshTerminals(event.payload); }
  catch (error) { if (appData.selectedProject?.id === event.payload) showFolderError(String(error)); }
});
await listen<{ identifier: number; projectId: string; environmentId?: string | null; agentKind?: Project['agentKind'] | null; purpose: string; code: number | null }>('terminal-exit', async event => {
  if (appData.selectedProject?.id === event.payload.projectId || appDataStore.getState().projectTerminals.has(event.payload.projectId)) {
    await refreshTerminals(event.payload.projectId);
  }
  void syncMenus();
  if (event.payload.identifier === terminalIdentifier) {
    const status = appData.terminals.find(status => status.identifier === terminalIdentifier);
    showTerminalStatus(status ? terminalCaption(status) : 'Terminal process ended');
  }
  if (event.payload.purpose === 'login' && event.payload.agentKind !== 'claude-code') await sharedChatgpt?.refresh();
  if (['codex', 'resume', 'claude', 'claude-resume'].includes(event.payload.purpose) && appData.selectedProject?.id === event.payload.projectId) { void refreshProjects(); void refreshConversations(); }
});
await listen<{ project: Project; terminalIdentifier: number; message: string | null }>('environment-preparation-finished', async event => {
  const result = event.payload;
  const key = [result.project.id, result.project.environmentId].join('|');
  if (appData.selectedProject?.id !== result.project.id || appData.selectedProject.environmentId !== result.project.environmentId || pendingEnvironmentKey === key) return;
  if ((environmentTerminals.get(key) ?? 0) > result.terminalIdentifier) return;
  environmentTerminals.set(key, result.terminalIdentifier);
  appData.selectedProject = result.project;
  const context = projectContext()!;
  await refreshSelectedPlan();
  if (!sameProject(projectContext(), context)) return;
  updateCodexActions();
  element('preparation-status').textContent = result.message ?? 'Brig environment and guest agent version verified.';
  environmentFeedback(result.message ?? (appData.selectedProject?.stage === 'environment-ready' ? null : 'The project environment is not running. Try starting it again.'));
  if (!result.message && appData.selectedProject?.stage === 'environment-ready') void refreshConversations();
  if (result.terminalIdentifier === terminalIdentifier) showTerminalStatus(result.message ? 'Environment setup failed. Prepare the environment again.' : 'Brig environment ready');
});
await listen<{ projectId: string; environmentId: string; terminalIdentifier: number; message: string }>('environment-preparation-error', event => {
  const key = [event.payload.projectId, event.payload.environmentId].join('|');
  if (appData.selectedProject?.id !== event.payload.projectId || appData.selectedProject.environmentId !== event.payload.environmentId || (environmentTerminals.get(key) ?? 0) > event.payload.terminalIdentifier) return;
  element('preparation-status').textContent = event.payload.message;
  environmentFeedback(event.payload.message);
  showFolderError(event.payload.message);
});
await listen<{ identifier: number; message: string }>('terminal-input-error', event => { if (event.payload.identifier === terminalIdentifier) showTerminalStatus(event.payload.message); });

await listen<string>('menu-action', async event => {
  if (event.payload === 'choose-folder' && appData.selectedProject === null) { showView('project'); void chooseFolder(); }
  if (event.payload === 'settings') showView('settings');
  if (event.payload === 'project') { await hideTerminal(); showView('project'); }
  if (event.payload === 'return-terminal') { showView('project'); await returnToTerminal(); }
});
showView('project');
const startup = new StartupSequence();
await startup.run('window', () => invoke('frontend_ready'));
await startup.run('controllers', () => {
resetBoundaryDetails = initializeBoundaryTests(projectContext);
resetPolicyDetails = initializeProjectPolicy(() => appData.selectedProject ? { ...projectContext()!, project: appData.selectedProject } : null, async (project, detached) => {
  appData.selectedProject = project;
  updateCodexActions();
  selectedPlanFingerprint = null;
  appData.plan = null;
  resetBoundaryChoice();
  element('codex-actions').hidden = true;
  element('prepare-environment').hidden = true;
  element('refresh-plan').hidden = true;
  element('sharing-plan').hidden = true;
  if (!detached) {
    const context = projectContext()!;
    await refreshSelectedPlan();
    if (sameProject(projectContext(), context) && selectedPlanFingerprint) void refreshConversations();
  }
});
resetOperationDetails = initializeOperations(projectContext, openTerminal, async project => {
  appData.selectedProject = project;
  selectedPlanFingerprint = null;
  appData.plan = null;
  resetBoundaryChoice();
  updateCodexActions();
  await refreshTerminals();
  if (appData.selectedProject?.id === project.id) await refreshSelectedPlan();
}, async message => {
  const projectId = appData.selectedProject?.id;
  terminalIdentifier = null;
  await showProjects(message);
  await refreshProjects();
  const survivor = appData.projects.find(project => project.id === projectId);
  if (!survivor) { if (projectId) forgetProjectData(projectId); return; }
  await selectProject(survivor);
  const active = currentTerminal();
  if (active?.running) await returnToTerminal(active.identifier);
});
resetProjectDetails();
});
await startup.run('records', initializeDataLocation);
await startup.run('settings', initializeSettings);
sharedChatgpt = await startup.run('chatgpt', () => initializeChatgptAccount(status => {
  if (status.state !== 'signed-in') connectedAccountProject = null;
  updateCodexActions();
}));
claudeAccount = await startup.run('claude', () => initializeClaudeAccount(updateCodexActions));
updateCodexActions();
void initializeHost();
void syncMenus();
await startup.run('projects', rescanProjects);

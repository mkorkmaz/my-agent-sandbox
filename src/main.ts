import { StartupSequence } from './app/startup-state';
import { FormState } from './ui/form-state';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './style.css';
import { initializeHost, type HostReadiness } from './settings/host';
import { initializeSettings, initializeSettingsTabs, selectSettingsTab } from './settings/settings';
import { initializeAppUpdates } from './settings/app-updates';
import { renderProjects, renderConversations, renderTerminals, isStandaloneConversation, standaloneConversations, terminalCaption, terminalNames, conversationKey, terminalMatches, agentName, isChat, sameProject, sameProjectView, type AgentKind, type Conversation, type Project, type ProjectContext, type TerminalStatus } from './projects/projects';
import { createAgentLogo } from './providers/agent-logo';
import { prepareProject, renderPlan, type Plan } from './projects/preparation';
import { initializeBoundaryTests, resetBoundaryChoice, useBrigBoundary } from './projects/boundaries';
import { initializeOperations, renderStopImpact } from './projects/operations';
import { initializeGatewayLogs } from './projects/gateway-logs';
import { initializeProjectPreview } from './projects/project-preview';
import { initializeProjectGitHubAccess } from './projects/project-github-access';
import { initializeProjectImageUpdate } from './projects/project-image-update';
import { initializeProjectPolicy } from './projects/project-policy';
import { renderDashboard, emptyUsage, savedSessionsStatus, type Dashboard, type SessionState } from './dashboard/dashboard';
import { closeDialog, dialogOpen, hasOpenDialog, showDialog } from './ui/dialogs';
import { initializeSidebar } from './sidebar/sidebar';
import { initializeHelp } from './help/help';
import { initializeDataLocation } from './settings/data-location';
import { initializeChatgptAccount, type ChatgptAccount, type ChatgptAccountStatus } from './providers/chatgpt-account';
import { initializeClaudeAccount, type ClaudeAccount } from './providers/claude-account';
import { renderSkeleton, setBusy, setLoading } from './ui/loading';
import { TerminalLaunch, type TerminalLaunchRequest } from './terminal/terminal-launch';
import { updateTerminal, type TerminalLayout } from './terminal/terminal-adapter';
import { TerminalLayoutQueue } from './terminal/terminal-layout';
import { EnvironmentBoot } from './terminal/environment-boot';
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
const helpView = element('help-view');
const projectNavigation = element<HTMLButtonElement>('project-navigation');
const settingsNavigation = element<HTMLButtonElement>('settings-navigation');
const helpNavigation = element<HTMLButtonElement>('help-navigation');
initializeSettingsTabs();
initializeHelp();
const chooseFolderButton = element<HTMLButtonElement>('choose-folder');
const refreshSessionsButton = element<HTMLButtonElement>('refresh-sessions');
const projectSettingsDialog = element('project-settings-dialog');
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
let accountSyncKey: string | null = null;
let accountConnectionError: string | null = null;
const accountSync = new RequestState();
let connectedAccountProject: string | null = null;
let usageRecordRevision = 0;
const conversationChange = new RequestState();
let resetPolicyDetails = () => {};
let resetBoundaryDetails = () => {};
let resetOperationDetails = () => {};
let resetGatewayLogs = () => {};
let resetProjectPreview = () => {};
let resetGitHubAccess = () => {};
let projectImageUpdate: ReturnType<typeof initializeProjectImageUpdate> | null = null;
let terminalIdentifier: number | null = null;
const terminalLaunch = new TerminalLaunch();
let pendingEnvironmentKey: string | null = null;
const environmentTerminals = new Map<string, number>();
const environmentFailures = new Map<string, number>();
const preparingEnvironments = new Map<string, Promise<Awaited<ReturnType<typeof prepareProject>>>>();
const firstChatAgents = new Map<string, AgentKind | null>();
let waitingForFirstSetup: ProjectContext | null = null;
let requestedChat: { project: ProjectContext; agent: AgentKind } | null = null;
let dashboardAgentChosen = false;
let hostReadiness: HostReadiness = 'checking';
function preparationKey(project: Project): string { return project.id + '|' + project.environmentId; }
function preparingSelected(): boolean { return appData.selectedProject !== null && preparingEnvironments.has(preparationKey(appData.selectedProject)); }
let selectedPlanFingerprint: string | null = null;
const planRequests = new RequestState();
let pendingRecordPlanRefresh: ProjectContext | null = null;
type ProjectPlanAction = 'prepare' | 'preview' | 'environment';
let planRetryAction: ProjectPlanAction = 'prepare';
const emptyDashboard: Dashboard = { conversations: [], usage: emptyUsage, coverage: 'unknown', pricingDate: '2026-11-17', warnings: [] };
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
const environmentBoot = new EnvironmentBoot();
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
  resetPolicyDetails(); resetBoundaryDetails(); resetOperationDetails(); resetGatewayLogs(); resetProjectPreview(); resetGitHubAccess();
  projectImageUpdate?.reset();
  finishProjectRecordRefresh();
}

function environmentReady(): boolean { return appData.selectedProject?.stage === 'environment-ready' && selectedPlanFingerprint !== null && !planRequests.pending(); }
function canStartChat(): boolean {
  const context = projectContext();
  if (newChatAgent() === 'claude-code') return environmentReady() && claudeAccount?.status()?.state === 'signed-in';
  return environmentReady() && !accountSync.pending() && sharedChatgpt?.status()?.state === 'signed-in'
    && connectedAccountProject === [context?.id, context?.epoch, context?.fingerprint].join('|');
}

function canRequestChat(): boolean {
  const project = appData.selectedProject;
  if (!project || requestedChat || projectSelection.pending() || folderChoice.pending() || project.policyDetached) return false;
  if (terminalLaunch.blocksChanges() && !environmentBoot.pending(project)) return false;
  const agent = hasFirstChat() ? firstChatAgent() : newChatAgent();
  if (!agent || !providerReady(agent)) return false;
  if (canStartChat()) return true;
  if (environmentBoot.pending(project)) return true;
  if (project.stage === 'environment-ready' && hasFirstChat() && accountSync.pending()) return true;
  if (project.stage === 'selected' && hasFirstChat() && hostReadiness === 'checking') return true;
  if (project.stage === 'ready' && selectedPlanFingerprint) return !appData.terminals.some(terminal => terminal.running && (terminal.purpose === 'local' || terminalMatches(terminal, project)));
  return planRequests.pending() && ['selected', 'creating-policy', 'policy-created', 'attaching-policy', 'ready'].includes(project.stage);
}

function updateNewChatButtons(): void {
  const disabled = !canRequestChat();
  for (const id of ['start-codex', 'first-chat-open']) {
    const button = element<HTMLButtonElement>(id);
    button.disabled = disabled;
    setBusy(button, requestedChat !== null);
  }
}

function continueRequestedChat(): void {
  const request = requestedChat;
  if (!request) return;
  const project = appData.selectedProject;
  if (!project || !sameProject(projectContext(), request.project) || request.agent !== newChatAgent() || !providerReady(request.agent)) {
    requestedChat = null;
    return;
  }
  if (!selectedPlanFingerprint || !canStartChat() || terminalLaunch.blocksChanges() || preparingSelected()) return;
  if (appData.terminals.some(terminal => terminal.running && terminal.purpose === 'environment' && terminalMatches(terminal, project))) return;
  requestedChat = null;
  void openTerminal('start_agent_session', {
    identifier: request.project.id, environmentId: request.project.environmentId, agentKind: request.agent,
    fingerprint: selectedPlanFingerprint, action: 'new-chat', useBrigBoundary: request.agent === 'claude-code' || useBrigBoundary(),
  }, agentName(request.agent));
}

function requestNewChat(): void {
  const context = projectContext();
  if (!context || !canRequestChat()) return;
  requestedChat = { project: context, agent: newChatAgent() };
  if (hasFirstChat() && appData.selectedProject) firstChatAgents.set(preparationKey(appData.selectedProject), requestedChat.agent);
  continueRequestedChat();
  updateNewChatButtons();
  if (requestedChat && appData.selectedProject?.stage === 'ready' && !planRequests.pending() && !environmentBoot.pending(appData.selectedProject)) {
    void startSelectedEnvironment(false);
  }
}

async function syncProjectAccount(): Promise<void> {
  const context = projectContext();
  if (!context || newChatAgent() !== 'codex' || sharedChatgpt?.status()?.state !== 'signed-in' || !environmentReady() || accountSync.pending()) return;
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
      if (requestedChat?.agent === 'codex') requestedChat = null;
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
  const showingBoot = !terminalPanel.hidden && environmentBoot.pending(appData.selectedProject);
  setWorkspaceStatus('project-plan', busy && !showingBoot ? message : null, 'project-dashboard');
  element('retry-project-plan').hidden = !retry;
  element('retry-project-plan').textContent = planRetryAction === 'environment' ? 'Retry environment' : 'Retry setup';
  element('project-preparation-last-stage').hidden = true;
  element('project-preparation-output').hidden = true;
}

function environmentFeedback(message: string | null = null): void {
  if (!selectedPlanFingerprint && !message && !environmentBoot.pending(appData.selectedProject)) return;
  if (message) requestedChat = null;
  if (!message && appData.selectedProject?.stage === 'ready') message = appData.selectedProject.environmentAttempt?.error ?? null;
  if (message) {
    updateNewChatButtons();
    planRetryAction = 'environment';
    planFeedback(message, true);
    const lastStage = environmentBoot.lastStage(appData.selectedProject);
    element('project-preparation-last-stage').textContent = lastStage ?? '';
    element('project-preparation-last-stage').hidden = lastStage === null;
    updatePreparationOutput();
    return;
  }
  if (appData.selectedProject?.stage === 'environment-ready') { planFeedback(null); return; }
  if (appData.selectedProject?.stage === 'environment-starting') {
    planFeedback('Starting the project environment with your saved settings…', false, true); return;
  }
  if (appData.selectedProject?.stage === 'ready' && selectedPlanFingerprint) {
    planRetryAction = 'environment'; planFeedback('The project environment is stopped.', true);
    element('retry-project-plan').textContent = 'Start environment';
  }
}

async function startSelectedEnvironment(foreground: boolean): Promise<void> {
  if (!appData.selectedProject || !selectedPlanFingerprint || !['ready', 'environment-starting'].includes(appData.selectedProject.stage) || appData.selectedProject.policyDetached || terminalLaunch.blocksChanges()) return;
  if (appData.terminals.some(status => appData.selectedProject && terminalMatches(status, appData.selectedProject) && status.running && (isChat(status) || ['login', 'shell'].includes(status.purpose)))) {
    requestedChat = null;
    updateNewChatButtons();
    planFeedback('End running sessions to apply the saved environment settings.'); return;
  }
  planFeedback('Starting the project environment with your saved settings…', false, true);
  await openTerminal('prepare_environment', { identifier: appData.selectedProject.id, environmentId: appData.selectedProject.environmentId, fingerprint: selectedPlanFingerprint }, 'Environment startup', foreground);
}

function currentTerminal(): TerminalStatus | null {
  const remembered = appData.selectedProject ? lastTerminals.get(appData.selectedProject.id) : null;
  return appData.terminals.find(status => status.identifier === (terminalIdentifier ?? remembered))
    ?? appData.terminals.find(status => status.running && isChat(status))
    ?? appData.terminals.find(status => status.running)
    ?? appData.terminals.at(-1) ?? null;
}

function chatConversationId(status: TerminalStatus): string | null {
  if (!isChat(status) || !status.conversationId || status.projectId !== appData.selectedProject?.id) return null;
  const key = conversationKey({ ...status, id: status.conversationId });
  const conversations = selectedConversations();
  const conversation = conversations.find(conversation => conversationKey(conversation) === key);
  if (!conversation) return key;
  if (conversation.relationStatus === 'resolved') {
    const root = conversations.find(root => root.id === conversation.rootSessionId && root.environmentId === conversation.environmentId && root.agentKind === conversation.agentKind && isStandaloneConversation(root));
    return root ? conversationKey(root) : null;
  }
  return isStandaloneConversation(conversation) ? key : null;
}

function renderSessionSidebar(): void {
  const chats = appData.terminals.filter(isChat).filter(status => status.running);
  const pendingTitles = new Map<string, number>();
  const rows = standaloneConversations(selectedConversations(), 'active').map(conversation => {
    const terminal = chats.find(status => chatConversationId(status) === conversationKey(conversation));
    return { ...conversation, available: terminal ? true : conversation.available, terminalId: terminal?.identifier, busy: terminal?.busy };
  });
  for (const terminal of chats) {
    if (rows.some(row => row.terminalId === terminal.identifier)) continue;
    const conversationId = chatConversationId(terminal);
    const saved = selectedConversations().find(conversation => conversationKey(conversation) === conversationId);
    if (saved) {
      rows.unshift({ ...saved, available: true, terminalId: terminal.identifier, busy: terminal.busy });
      continue;
    }
    const agent = agentName(terminal.agentKind);
    const label = terminal.conversationId && !conversationId ? agent + ' process' : 'New ' + agent + ' chat';
    const count = (pendingTitles.get(label) ?? 0) + 1;
    pendingTitles.set(label, count);
    const conversation: Conversation = {
      id: (conversationId && terminal.conversationId) || 'terminal-' + terminal.identifier,
      environmentId: terminal.environmentId,
      agentKind: terminal.agentKind,
      title: count === 1 ? label : label + ' ' + count,
      updatedAt: Date.now(), state: 'active', available: true, warning: null,
      lineage: { kind: 'unknown', parentSessionId: null, agentPath: null, nickname: null, sourceVersion: null, lineageVersion: 1 },
      relationStatus: 'unknown', rootSessionId: null, subagentCount: 0,
      usageAvailable: terminal.agentKind === 'codex', usage: emptyUsage, familyUsage: emptyUsage,
      coverage: 'unknown', usageRevision: 'pending-terminal-' + terminal.identifier,
    };
    rows.unshift({ ...conversation, available: true, terminalId: terminal.identifier, busy: terminal.busy });
  }
  const selected = appData.terminals.find(status => status.identifier === terminalIdentifier);
  selectedConversationId = selected && isChat(selected) ? chatConversationId(selected) ?? conversationKey({ ...selected, id: 'terminal-' + selected.identifier }) : null;
  renderConversations(rows, selectedConversationId, resumeConversation);
  renderTerminals(appData.terminals, terminalIdentifier, returnToTerminal);
  if (chats.length) element('session-list-status').hidden = true;
}

async function refreshTerminals(projectId = appData.selectedProject?.id): Promise<void> {
  if (!projectId) return;
  const request = (terminalListRequests.get(projectId) ?? 0) + 1;
  terminalListRequests.set(projectId, request);
  const result = (await invoke<TerminalStatus[]>('terminal_sessions', { projectId })).filter(status => status.projectId === projectId);
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
  renderStopImpact(appData.selectedProject, appData.terminals, selectedConversations());
  projectImageUpdate?.render();
  environmentBoot.render(appData.selectedProject, terminalIdentifier, terminalVisibility() === 'visible');
  const terminalRunning = appData.terminals.some(terminal => terminal.running);
  const shown = appData.terminals.find(terminal => terminal.identifier === terminalIdentifier);
  const preparationOutput = environmentBoot.hasOutput(appData.selectedProject, terminalIdentifier);
  element('show-preparation-output').hidden = !preparationOutput;
  element('show-preparation-output').textContent = environmentBoot.showingOutput() ? 'Show preparation progress' : 'Show setup output';
  const cancel = element<HTMLButtonElement>('cancel-preparation');
  cancel.hidden = !preparationOutput;
  cancel.disabled = !environmentBoot.canCancel(appData.selectedProject, terminalIdentifier);
  element('close-terminal').hidden = preparationOutput || shown?.purpose === 'environment' && shown.running
    || terminalIdentifier === null && environmentBoot.pending(appData.selectedProject);
  element('close-terminal').textContent = shown?.running ? 'Close terminal' : 'Close output';
  const closing = terminalIdentifier !== null && terminalLaunch.isClosing(terminalIdentifier);
  element<HTMLButtonElement>('close-terminal').disabled = closing;
  setBusy(element('close-terminal'), closing);
  projectView.classList.toggle('has-live-terminal', terminalRunning);
  continueRequestedChat();
  updateNewChatButtons();
  element<HTMLButtonElement>('start-codex').title = 'New Chat';
  updatePreparationOutput();
}

function preparationOutput(): number | undefined {
  const project = appData.selectedProject;
  if (!project || planRetryAction !== 'environment' || element('retry-project-plan').hidden) return;
  const identifier = environmentFailures.get(preparationKey(project));
  return appData.terminals.some(terminal => terminal.identifier === identifier && terminalMatches(terminal, project)) ? identifier : undefined;
}

function updatePreparationOutput(): void {
  element('project-preparation-output').hidden = preparationOutput() === undefined;
}

async function syncMenus(): Promise<void> {
  const busy = folderChoice.pending() || projectSelection.pending() || preparingSelected() || terminalLaunch.blocksChanges();
  chooseFolderButton.disabled = busy;
  element<HTMLButtonElement>('choose-first-chat').disabled = busy;
  element<HTMLSelectElement>('agent-choice').disabled = (terminalLaunch.blocksChanges() && !environmentBoot.pending(appData.selectedProject)) || projectSelection.pending() || folderChoice.pending();
  updateNewChatButtons();
  element<HTMLButtonElement>('guest-shell').disabled = busy || !environmentReady();
  projectNavigation.disabled = busy;
  refreshSessionsButton.disabled = busy;
  setBusy(chooseFolderButton, folderChoice.pending());
  setBusy(element('choose-first-chat'), folderChoice.pending());
  setBusy(element('terminal-status'), terminalLaunch.blocksChanges());
  for (const button of document.querySelectorAll<HTMLButtonElement>('.sidebar-project, .sidebar-session, .recent-project, .project-record-forget')) {
    const status = appData.terminals.find(terminal => String(terminal.identifier) === button.dataset.terminalId);
    const inspectingSetup = pendingEnvironmentKey !== null && status && (status.purpose === 'local' || status.purpose === 'environment' && appData.selectedProject && terminalMatches(status, appData.selectedProject));
    button.disabled = (button.classList.contains('sidebar-session') ? terminalLaunch.blocksChanges() && !inspectingSetup || projectSelection.pending() || folderChoice.pending() : busy) || button.dataset.available === 'false';
  }
  renderSessionDashboard();
  scheduleProjectRescan();
  try { await invoke('update_menu_state', { projectId: appData.selectedProject?.id ?? null, busy }); }
  catch { showFolderError('Could not update the native menu. Reopen the app.'); }
}

type TerminalVisibility = 'hidden' | 'obscured' | 'visible';

function terminalVisibility(): TerminalVisibility {
  if (projectView.hidden || terminalPanel.hidden) return 'hidden';
  if (hasOpenDialog()) return 'obscured';
  return 'visible';
}

function terminalLayout(): TerminalLayout {
  environmentBoot.render(appData.selectedProject, terminalIdentifier, terminalVisibility() === 'visible');
  const rectangle = terminalSurface.getBoundingClientRect();
  const layout: TerminalLayout = { x: rectangle.x, y: rectangle.y, width: rectangle.width, height: rectangle.height, visible: terminalVisibility() === 'visible' };
  const overlay = environmentBoot.bounds(rectangle);
  if (overlay) layout.overlay = overlay;
  return layout;
}

function terminalUpdatesAllowed(identifier: number): boolean {
  return terminalIdentifier === identifier && !terminalLaunch.isClosing(identifier);
}

function updateTerminalLayout(): Promise<void> {
  environmentBoot.render(appData.selectedProject, terminalIdentifier, terminalVisibility() === 'visible');
  return terminalLayouts.request(async () => {
    const identifier = terminalIdentifier;
    if (identifier === null || !terminalUpdatesAllowed(identifier)) return;
    const layout = terminalLayout();
    await updateTerminal('terminal_layout', { identifier, layout }, () => !layout.visible || !terminalUpdatesAllowed(identifier));
  }, error => { showFolderError(String(error)); });
}

const appUpdates = initializeAppUpdates(() => {
  void updateTerminalLayout().then(() => {
    if (terminalIdentifier !== null) return focusTerminal(terminalIdentifier);
  }).catch(error => { showFolderError(String(error)); });
});

async function focusTerminal(identifier: number): Promise<void> {
  if (!terminalUpdatesAllowed(identifier) || terminalVisibility() !== 'visible' || environmentBoot.blocksFocus(identifier)) return;
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
  label.prepend(createAgentLogo(status.agentKind));
  const agent = agentName(status.agentKind);
  label.setAttribute('aria-label', message === agent ? agent : agent + ': ' + message);
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

function shouldRescanProjectLibrary(): boolean {
  return projectRescan.started() && !projectView.hidden && appData.selectedProject === null
    && (appData.projects.length > 0 || projectRescanError !== null);
}

function scheduleProjectRescan(): void {
  const busy = projectRescan.scanning() || folderChoice.pending() || projectSelection.pending() || preparingSelected() || terminalLaunch.blocksChanges();
  projectRescan.schedule(shouldRescanProjectLibrary(), busy, rescanProjects);
}

function rescanVisibleProjectLibrary(): void {
  if (shouldRescanProjectLibrary()) void rescanProjects();
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
      renderSessionDashboard();
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
  const refreshing = dashboardState.pending();
  setBusy(refreshSessionsButton, refreshing);
  refreshSessionsButton.setAttribute('aria-disabled', String(refreshing));
  const dashboard = dashboardState.current();
  renderDashboard(dashboard ?? emptyDashboard, sessionView, resumeConversation, changeConversationState, usageDetails.open, conversationChange.pending() || terminalLaunch.blocksChanges() || projectSelection.pending() || folderChoice.pending());
  const dashboardLoaded = dashboardState.loaded();
  const dashboardError = dashboardState.error();
  const loading = !dashboardLoaded && !dashboardError;
  const checkingSessions = dashboard?.sessionAvailability === 'unverified' && !dashboardError
    && (planRequests.pending() || preparingSelected() || refreshing);
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
  element('project-overview').setAttribute('aria-busy', String(refreshing || (!dashboardLoaded && !dashboardError)));
  if (!dashboardLoaded || dashboardError) {
    const status = element('dashboard-session-status');
    status.textContent = dashboardError ? dashboardError + ' Select Refresh to try again.' : '';
    status.hidden = !dashboardError;
    if (!dashboardLoaded && !loading) element('session-table').hidden = true;
  }
  const message = conversationChange.pending() ? 'Updating session…'
    : loading ? 'Loading sessions and usage…'
    : refreshing ? 'Refreshing sessions and usage…'
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
    await invoke('change_conversation_state', { identifier: projectId, environmentId: conversation.environmentId, agentKind: conversation.agentKind, sessionId: conversation.id, state });
    if (sameProject(projectContext(), context)) { folderError.hidden = true; await refreshConversations(); }
  } catch (error) { if (sameProject(projectContext(), context)) showFolderError(String(error)); }
  finally { if (conversationChange.accepts(request)) { conversationChange.finish(request); renderSessionDashboard(); if (sameProject(projectContext(), context) && !hasOpenDialog()) document.querySelector<HTMLButtonElement>('[data-session-state="' + sessionView + '"]')?.focus(); } }
}
function selectedUsageFingerprint(sessionId: string, environmentId: string, agentKind: AgentKind): string {
  const children = new Map<string, Conversation[]>();
  const scoped = selectedConversations().filter(conversation => conversation.environmentId === environmentId && conversation.agentKind === agentKind);
  const nodesById = new Map(scoped.map(conversation => [conversation.id, conversation]));
  for (const conversation of scoped) {
    const parent = conversation.lineage.parentSessionId;
    const version = conversation.lineage.sourceVersion;
    if (!parent || conversation.lineage.kind !== 'subagent' || conversation.lineage.lineageVersion !== 1 || (version !== '0.167.0' && version !== '0.171.0') || conversation.relationStatus === 'invalid') continue;
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

function newChatAgent(): AgentKind {
  return element<HTMLSelectElement>('agent-choice').value as AgentKind;
}

function hasFirstChat(project = appData.selectedProject): boolean {
  return project !== null && firstChatAgents.has(preparationKey(project));
}

function guidedFirstChat(project = appData.selectedProject): boolean {
  return hasFirstChat(project) && project?.stage !== 'environment-ready';
}

function providerReady(agent: AgentKind): boolean {
  return (agent === 'claude-code' ? claudeAccount?.status() : sharedChatgpt?.status())?.state === 'signed-in';
}

function connectedAgents(): AgentKind[] {
  return (['codex', 'claude-code'] as const).filter(providerReady);
}

function firstChatAgent(): AgentKind | null {
  if (!appData.selectedProject) return null;
  const choice = firstChatAgents.get(preparationKey(appData.selectedProject));
  if (choice) return choice;
  return connectedAgents()[0] ?? null;
}

function firstSetupAvailable(): boolean {
  const agent = firstChatAgent();
  return agent !== null && providerReady(agent) && hostReadiness === 'ready';
}

function resumeFirstSetup(): void {
  const context = waitingForFirstSetup;
  if (!context || !appData.selectedProject || !sameProject(projectContext(), context) || !firstSetupAvailable()) return;
  waitingForFirstSetup = null;
  void finishOpeningProject(appData.selectedProject, context.epoch);
}

function renderFirstChat(): void {
  const visible = hasFirstChat();
  element('first-chat-setup').hidden = !visible;
  element('dashboard-new-chat').hidden = visible;
  if (!visible) {
    if (appData.selectedProject && !dashboardAgentChosen) {
      const connected = connectedAgents();
      if (connected.length === 1) element<HTMLSelectElement>('agent-choice').value = connected[0];
    }
    return;
  }
  const agent = firstChatAgent();
  const connected = agent !== null && providerReady(agent);
  const ready = appData.selectedProject?.stage === 'environment-ready';
  const starting = appData.selectedProject?.stage === 'environment-starting' && environmentBoot.pending(appData.selectedProject);
  const select = element<HTMLSelectElement>('first-chat-agent');
  select.value = agent ?? '';
  select.disabled = projectSelection.pending() || folderChoice.pending();
  if (agent) element<HTMLSelectElement>('agent-choice').value = agent;
  const hostLabels: Record<HostReadiness, string> = {
    checking: 'Checking this Mac…', ready: 'Mac check complete. Brig is available.', 'needs-attention': 'Check Brig on this Mac to continue.',
  };
  element('first-chat-host').textContent = ready ? 'Brig environment ready.'
    : starting ? 'Preparation continues. Open Environment startup under Terminals to see progress.' : hostLabels[hostReadiness];
  element('first-chat-system-checks').hidden = ready || starting || hostReadiness !== 'needs-attention';
  element('first-chat-claude').hidden = agent !== 'claude-code' || connected;
  element('first-chat-chatgpt').hidden = agent !== 'codex' || connected;
  element('first-chat-account').textContent = !agent ? 'Choose an agent to check its account.'
    : connected ? (agent === 'claude-code' ? 'Claude' : 'ChatGPT') + ' account connected.' : '';
  const launch = element<HTMLButtonElement>('first-chat-open');
  launch.hidden = !connected;
  launch.textContent = agent ? 'Open ' + agentName(agent) : 'Start first chat';
  updateNewChatButtons();
}

element<HTMLSelectElement>('agent-choice').addEventListener('change', () => {
  dashboardAgentChosen = true;
  updateCodexActions();
});
element<HTMLSelectElement>('first-chat-agent').addEventListener('change', event => {
  if (!appData.selectedProject || !hasFirstChat()) return;
  const value = (event.currentTarget as HTMLSelectElement).value;
  const agent = value === 'codex' || value === 'claude-code' ? value : null;
  firstChatAgents.set(preparationKey(appData.selectedProject), agent);
  updateCodexActions();
  resumeFirstSetup();
});
element('first-chat-system-checks').addEventListener('click', () => {
  showView('settings');
  selectSettingsTab('system-checks');
  element('check-host').focus();
});

async function resumeConversation(conversation: Conversation): Promise<void> {
  if (!appData.selectedProject || appData.selectedProject.environmentId !== conversation.environmentId || terminalLaunch.blocksChanges() || projectSelection.pending() || !isStandaloneConversation(conversation)) return;
  const projectId = appData.selectedProject.id;
  const key = conversationKey(conversation);
  const terminal = appData.terminals.find(status => status.running && status.projectId === projectId && isChat(status) && (
    chatConversationId(status) === key
    || (status.identifier === conversation.terminalId && conversationKey({ ...status, id: 'terminal-' + status.identifier }) === key)
  ));
  if (terminal) {
    showView('project');
    folderError.hidden = true;
    await returnToTerminal(terminal.identifier);
    return;
  }
  const request = ++terminalViewRequest;
  const current = () => request === terminalViewRequest && appData.selectedProject?.id === projectId
    && appData.selectedProject.environmentId === conversation.environmentId;
  if (!current()) return;
  showView('project');
  if (preparingSelected()) { showFolderError('Wait for this environment setup to finish.'); return; }
  if (appData.selectedProject?.stage === 'updating-image') {
    showFolderError('Finish the guest image update before resuming this chat.');
    await showProjectSettings();
    return;
  }
  if (appData.selectedProject?.stage !== 'environment-ready') {
    showFolderError('Prepare the environment before resuming this session.');
    await showProjectSettings();
    element('prepare-environment').focus();
    return;
  }
  if (!selectedPlanFingerprint) await refreshSelectedPlan();
  if (!current() || !selectedPlanFingerprint) return;
  await openTerminal('start_agent_session', { identifier: projectId, environmentId: conversation.environmentId, agentKind: conversation.agentKind, fingerprint: selectedPlanFingerprint, action: 'resume', sessionId: conversation.id, useBrigBoundary: conversation.agentKind === 'claude-code' || useBrigBoundary() }, conversation.title);
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
  requestedChat = null;
  waitingForFirstSetup = null;
  dashboardAgentChosen = false;
  appData.selectedProject = project;
  if (project.stage === 'selected' && !project.policy && !firstChatAgents.has(preparationKey(project))) {
    firstChatAgents.set(preparationKey(project), null);
  }
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
  sessionView = 'active'; renderSessionDashboard();
  renderSessionSidebar();
  element('session-list-status').textContent = dashboardState.loaded() ? 'No active sessions. Start a New Chat or restore an archived session.' : 'Loading sessions…';
  element('session-list-status').hidden = !dashboardState.loaded() || standaloneConversations(selectedConversations(), 'active').length > 0;
  renderProjectLists();
  projectView.classList.add('has-project');
  selectedPlanFingerprint = null;
  planRequests.invalidate(); planFeedback(null);
  for (const id of ['project-access-summary', 'prepare-environment', 'refresh-plan']) element(id).hidden = true;
  resetBoundaryChoice();
  folderError.hidden = true;
  element('codex-actions').hidden = true;
  element('sharing-plan').hidden = true;
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
    if (project.stage === 'updating-image') {
      showPendingImageUpdate();
      return;
    }
    if (guidedFirstChat(project) && !project.policy && !firstSetupAvailable()) {
      waitingForFirstSetup = projectContext();
      return;
    }
    const status = currentTerminal();
    const preview = status?.running || (project.policy?.fingerprint && !project.policyDetached && ['ready', 'environment-ready'].includes(project.stage));
    if (preview) await refreshSelectedPlan(true);
    if (!preview) await prepareSelectedProject();
    if (!current()) return;
    updateCodexActions();
    await startSelectedEnvironment(!requestedChat && !hasFirstChat() && project.approvedPlan === selectedPlanFingerprint && !projectView.hidden);
    if (!current()) return;
    if (current() && (!dashboardState.loaded() || dashboardState.current()?.sessionAvailability === 'unverified')) void refreshConversations();
  } catch (error) { if (current()) showFolderError(String(error)); }
}

function showPendingImageUpdate(): void {
  planRequests.invalidate();
  selectedPlanFingerprint = null;
  appData.plan = null;
  for (const id of ['sharing-plan', 'prepare-environment', 'refresh-plan']) element(id).hidden = true;
  planFeedback('Continue the guest image update in Project settings.');
  updateCodexActions();
}

async function hideTerminal(): Promise<void> {
  terminalPanel.hidden = true;
  if (environmentBoot.pending(appData.selectedProject)) environmentFeedback();
  await updateTerminalLayout();
}

async function returnToTerminal(identifier?: number, preserveDialogs = false): Promise<void> {
  if (!appData.selectedProject) return;
  const projectId = appData.selectedProject.id;
  const request = ++terminalViewRequest;
  if (!preserveDialogs) closeProjectDialogs();
  await refreshTerminals();
  const status = identifier === undefined ? currentTerminal() : appData.terminals.find(status => status.identifier === identifier);
  if (!status || appData.selectedProject?.id !== projectId || request !== terminalViewRequest) return;
  if (status.projectId !== projectId || (status.environmentId && !terminalMatches(status, appData.selectedProject))) return;
  if (status.purpose === 'environment' && status.running && appData.selectedProject?.stage === 'environment-starting') environmentBoot.begin(appData.selectedProject, status.identifier);
  await hideTerminal();
  if (appData.selectedProject?.id !== projectId || request !== terminalViewRequest) return;
  terminalIdentifier = status.identifier;
  lastTerminals.set(projectId, status.identifier);
  updateTerminalActions();
  renderSessionSidebar();
  if (projectView.hidden && !preserveDialogs) showView('project');
  terminalPanel.hidden = false;
  const title = selectedConversations().find(conversation => conversationKey(conversation) === selectedConversationId)?.title;
  showTerminalStatus(status.running ? (title ?? terminalNames[status.purpose] ?? 'Operation terminal') : terminalCaption(status));
  await updateTerminalLayout();
  if (!projectView.hidden && request === terminalViewRequest) await focusTerminal(status.identifier);
}

async function refreshSelectedPlan(repairMissingBinding = false): Promise<void> {
  if (!appData.selectedProject) return;
  const { id, environmentId } = appData.selectedProject;
  const request = planRequests.begin();
  const previousFingerprint = selectedPlanFingerprint;
  selectedPlanFingerprint = null;
  appData.plan = null;
  element('sharing-plan').hidden = true;
  element('prepare-environment').hidden = true;
  element('project-access-summary').hidden = true;
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
    const missingBinding = typeof error === 'object' && error !== null && 'code' in error && error.code === 'missing-project-policy';
    const project = appData.selectedProject;
    if (missingBinding && repairMissingBinding && project.stage === 'ready' && project.policy?.owned && !project.policyDetached
      && !appData.terminals.some(terminal => terminal.running && terminalMatches(terminal, project))) {
      await prepareSelectedProject();
      return;
    }
    const message = missingBinding && 'message' in error ? String(error.message) : String(error);
    requestedChat = null;
    selectedPlanFingerprint = null;
    element('sharing-plan').hidden = true;
    element('refresh-plan').hidden = true;
    element('preparation-status').textContent = message;
    planRetryAction = 'preview'; planFeedback(message, true);
  } finally { if (planRequests.accepts(request)) { planRequests.finish(request); updateCodexActions(); void syncMenus(); } finishPendingRecordPlanRefresh(); }
}

async function prepareSelectedProject(): Promise<void> {
  if (appData.selectedProject === null) return;
  const project = appData.selectedProject;
  const existing = preparingEnvironments.get(preparationKey(project));
  const work = existing ?? prepareProject(project);
  preparingEnvironments.set(preparationKey(project), work);
  selectedPlanFingerprint = null;
  appData.plan = null;
  const request = planRequests.begin();
  planFeedback('Preparing the execution plan…', false, true);
  resetBoundaryChoice();
  void syncMenus();
  element('preparation-status').textContent = 'Preparing Brig project configuration…';
  element('retry-preparation').hidden = true;
  element('sharing-plan').hidden = true;
  element('codex-actions').hidden = true;
  element('prepare-environment').hidden = true;
  element('refresh-plan').hidden = true;
  updateCodexActions();
  try {
    const result = await work;
    if (appData.selectedProject?.id !== project.id || appData.selectedProject.environmentId !== project.environmentId || !planRequests.accepts(request)) return;
    if (result.project.id !== project.id || result.project.environmentId !== project.environmentId) throw new Error('The execution plan belongs to another environment.');
    appData.selectedProject = result.project;
    if (!result.plan) requestedChat = null;
    selectedPlanFingerprint = result.fingerprint;
    planRequests.finish(request);
    planRetryAction = 'prepare';
    planFeedback(result.plan ? null : result.message, !result.plan && result.project.stage === 'environment-unverified');
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
  } catch (error) {
    if (appData.selectedProject?.id !== project.id || appData.selectedProject.environmentId !== project.environmentId || !planRequests.accepts(request)) return;
    requestedChat = null;
    element('preparation-status').textContent = String(error);
    element('retry-preparation').hidden = false;
    planRetryAction = 'prepare'; planFeedback(String(error), true);
  } finally { if (preparingEnvironments.get(preparationKey(project)) === work) preparingEnvironments.delete(preparationKey(project)); if (planRequests.accepts(request)) planRequests.finish(request); updateCodexActions(); void syncMenus(); finishPendingRecordPlanRefresh(); }
}

function updateCodexActions(): void {
  const ready = appData.selectedProject?.stage === 'environment-ready';
  renderFirstChat();
  const agent = hasFirstChat() ? firstChatAgent() : newChatAgent();
  element('project-access-agent').textContent = agent ? agentName(agent) : 'Choose an agent in the project';
  element('project-access-summary').hidden = appData.plan === null || selectedPlanFingerprint === null;
  element('prepare-environment').hidden = ready || appData.plan === null || selectedPlanFingerprint === null || environmentBoot.pending(appData.selectedProject);
  element('resume-removal').hidden = appData.selectedProject?.stage !== 'removing';
  element('codex-actions').hidden = !ready;
  element('project-overview').hidden = appData.selectedProject === null || guidedFirstChat();
  element('project-setup').hidden = guidedFirstChat() || ready || planRequests.pending() || projectSelection.pending() || preparingSelected()
    || appData.selectedProject?.stage === 'environment-starting' || selectedPlanFingerprint !== null;
  continueRequestedChat();
  updateNewChatButtons();
  const claude = newChatAgent() === 'claude-code';
  const accountBusy = !claude && accountSync.pending() && environmentReady();
  element('project-account-feedback').hidden = hasFirstChat() || !environmentReady() || canStartChat() || accountBusy;
  element('project-account-status').textContent = claude ? claudeAccount?.status()?.message ?? 'Connect your shared Claude account in Settings, Providers.' : accountConnectionError ?? (accountSync.pending()
    ? 'Connecting the shared ChatGPT account…' : sharedChatgpt?.status()?.message ?? 'Checking the shared ChatGPT account…');
  setBusy(element('project-account-status'), !claude && accountSync.pending());
  setWorkspaceStatus('project-account', accountBusy ? element('project-account-status').textContent : null, 'project-dashboard');
  claudeAccount?.select();
  element<HTMLButtonElement>('guest-shell').disabled = !environmentReady() || terminalLaunch.blocksChanges() || projectSelection.pending() || folderChoice.pending() || preparingSelected();
  sharedChatgpt?.updateAvailability();
  if (ready && !claude && agent) void syncProjectAccount();
}

element('start-codex').addEventListener('click', requestNewChat);
element('first-chat-open').addEventListener('click', requestNewChat);
element('guest-shell').addEventListener('click', () => {
  if (!appData.selectedProject || !selectedPlanFingerprint) return;
  const { id, environmentId } = appData.selectedProject;
  void openTerminal('start_agent_session', { identifier: id, environmentId, agentKind: 'codex', fingerprint: selectedPlanFingerprint, action: 'shell', useBrigBoundary: useBrigBoundary() }, 'Guest terminal');
});
element('project-provider-settings').addEventListener('click', () => {
  showView('settings'); selectSettingsTab('providers'); element(newChatAgent() === 'claude-code' ? 'claude-account-heading' : 'shared-chatgpt-heading').focus();
});

function showView(view: 'project' | 'settings' | 'help'): void {
  closeProjectDialogs();
  projectView.hidden = view !== 'project';
  settingsView.hidden = view !== 'settings';
  helpView.hidden = view !== 'help';
  renderWorkspaceStatus();
  for (const navigation of [projectNavigation, settingsNavigation, helpNavigation, element('sidebar-current-project')]) navigation.removeAttribute('aria-current');
  const navigation = { project: appData.selectedProject ? element('sidebar-current-project') : projectNavigation, settings: settingsNavigation, help: helpNavigation };
  navigation[view].setAttribute('aria-current', 'page');
  element(view + '-heading').focus();
  if (view === 'settings') void appUpdates.refresh();
  scheduleProjectRescan();
  sharedChatgpt?.updateAvailability();
  void updateTerminalLayout();
  if (view === 'project' && !terminalPanel.hidden && terminalIdentifier !== null) void focusTerminal(terminalIdentifier).catch(error => { showFolderError(String(error)); });
}

async function showProjects(message = 'Choose where Brig and your agents will work.'): Promise<void> {
  if (terminalLaunch.blocksChanges() || projectSelection.pending() || preparingSelected()) return;
  await hideTerminal();
  requestedChat = null;
  waitingForFirstSetup = null;
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
    renderFirstChat();
    void syncMenus();
    if (!projectView.hidden && !hasOpenDialog() && terminalPanel.hidden) element('project-heading').focus();
    if (!projectView.hidden && !hasOpenDialog() && !terminalPanel.hidden && terminalIdentifier !== null) void focusTerminal(terminalIdentifier).catch(error => { showFolderError(String(error)); });
  }
}

function closeProjectDialogs(): void {
  for (const dialog of [projectSettingsDialog, usageDialog, element<HTMLDialogElement>('removal-dialog')]) if (dialogOpen(dialog)) closeDialog(dialog);
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
  return JSON.stringify([project.path, project.profile, project.label, project.imageRef, project.network, project.options, project.policy, project.policyDetached, project.ownsBinding, project.preview, project.githubAccess]);
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
    if (dialogOpen(projectSettingsDialog) && projectPreferences.current()) {
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
    if (result.project.stage === 'updating-image') {
      showPendingImageUpdate();
      return;
    }
    if (selected.stage === 'updating-image') {
      await refreshSelectedPlan();
      if (!sameProject(projectContext(), context)) return;
      if (selectedPlanFingerprint) void refreshConversations();
    } else if (planChanged && (selectedPlanFingerprint || planRequests.pending())) {
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
  renderStopImpact(appData.selectedProject, appData.terminals, selectedConversations());
  element<HTMLDetailsElement>('sharing-plan').open = false;
  element<HTMLDetailsElement>('project-resources').open = false;
  if (appData.selectedProject.stage === 'updating-image') element<HTMLDetailsElement>('project-image-update').open = true;
  showDialog(projectSettingsDialog, returnFocusId); await updateTerminalLayout();
  if (!sameProjectView(projectContext(), context) || !projectPreferences.accepts(request) || !dialogOpen(projectSettingsDialog)) return;
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
    if (!sameProjectView(projectContext(), context) || !projectPreferences.accepts(request) || !dialogOpen(projectSettingsDialog)) return;
    if (result.project.id !== context.id || result.project.environmentId !== context.environmentId) throw new Error('The project settings belong to another environment.');
    projectPreferences.receive(request, result);
    rememberProjectPreferences(result);
    renderProjectPreferences(projectPreferences.current()!, null);
    element<HTMLFieldSetElement>('project-preference-fields').disabled = false;
    element('project-preferences-status').textContent = '';
  } catch (error) { if (sameProjectView(projectContext(), context) && projectPreferences.accepts(request) && dialogOpen(projectSettingsDialog)) element('project-preferences-status').textContent = String(error); }
  finally { if (sameProjectView(projectContext(), context) && projectPreferences.accepts(request)) { projectPreferences.finish(request); setLoading(element('project-preference-fields'), false); finishProjectRecordRefresh(); } }
}
element('project-settings-button').addEventListener('click', () => { void showProjectSettings(); });
element('setup-project').addEventListener('click', () => { void showProjectSettings('setup-project'); });
element('close-project-settings').addEventListener('click', () => { closeDialog(projectSettingsDialog); });
projectSettingsDialog.addEventListener('close', () => { void updateTerminalLayout(); });
projectSettingsDialog.addEventListener('close', () => { if (!dialogOpen(projectSettingsDialog)) resetProjectDetails(); });
element<HTMLFormElement>('project-preferences').addEventListener('submit', async event => {
  event.preventDefault(); if (!appData.selectedProject || !dialogOpen(projectSettingsDialog)) return;
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
    await prepareSelectedProject();
    if (sameProject(projectContext(), context)) await startSelectedEnvironment(false);
    updateCodexActions(); await refreshProjects();
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

projectNavigation.addEventListener('click', async () => { await showProjects(); rescanVisibleProjectLibrary(); });
element('sidebar-current-project').addEventListener('click', async () => { await hideTerminal(); showView('project'); });
refreshSessionsButton.addEventListener('click', () => {
  if (dashboardState.pending()) return;
  refreshSessionsButton.focus({ preventScroll: true });
  void refreshConversations();
});
settingsNavigation.addEventListener('click', () => showView('settings'));
helpNavigation.addEventListener('click', () => showView('help'));
chooseFolderButton.addEventListener('click', () => { void chooseFolder(); });
element('choose-first-chat').addEventListener('click', () => { void chooseFolder(); });
document.defaultView!.addEventListener('focus', rescanVisibleProjectLibrary);
async function retryProjectSetup(action: ProjectPlanAction): Promise<void> {
  const context = projectContext();
  if (!context) return;
  try {
    if (action === 'prepare') await prepareSelectedProject();
    if (action === 'preview') await refreshSelectedPlan(true);
    if (!sameProject(projectContext(), context)) return;
    if (action === 'environment' && !selectedPlanFingerprint) await refreshSelectedPlan();
    if (sameProject(projectContext(), context)) await startSelectedEnvironment(action === 'environment');
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
element('refresh-plan').addEventListener('click', () => { void refreshSelectedPlan(); });
element('hide-terminal').addEventListener('click', async () => { await hideTerminal(); element('project-heading').focus(); });
element('project-preparation-output').addEventListener('click', () => {
  const identifier = preparationOutput();
  if (identifier !== undefined) void returnToTerminal(identifier);
});
element('show-preparation-output').addEventListener('click', () => {
  if (!environmentBoot.hasOutput(appData.selectedProject, terminalIdentifier)) return;
  environmentBoot.toggleOutput();
  updateTerminalActions();
  void updateTerminalLayout();
});
element('cancel-preparation').addEventListener('click', async () => {
  const project = appData.selectedProject, identifier = terminalIdentifier;
  if (!project || identifier === null || !environmentBoot.requestCancellation(project, identifier)) return;
  requestedChat = null;
  updateTerminalActions();
  try {
    const accepted = await invoke<boolean>('cancel_environment_preparation', { projectId: project.id, environmentId: project.environmentId, identifier });
    if (!accepted) environmentBoot.cancellationUnavailable(project, identifier, false);
  } catch (error) {
    environmentBoot.cancellationUnavailable(project, identifier, true);
    if (terminalIdentifier === identifier) showTerminalStatus('Could not cancel preparation. ' + String(error));
  } finally { updateTerminalActions(); }
});
element('prepare-environment').addEventListener('click', () => {
  if (appData.selectedProject === null || selectedPlanFingerprint === null) return;
  void openTerminal('prepare_environment', { identifier: appData.selectedProject.id, environmentId: appData.selectedProject.environmentId, fingerprint: selectedPlanFingerprint }, 'Environment startup', true, true);
});
startTerminalButton.addEventListener('click', () => {
  if (appData.selectedProject) void openTerminal('terminal_start', { projectId: appData.selectedProject.id }, 'Local terminal');
});

async function finishTerminalLaunch(projectId?: string): Promise<void> {
  try { if (projectId) await refreshTerminals(projectId); }
  finally {
    terminalLaunch.finish();
    const completed = [...appDataStore.getState().projectTerminals.values()].flat().filter(status => status.purpose === 'environment' && !status.running && status.environmentId);
    for (const status of completed) await closeEnvironmentTerminal(status.projectId, status.environmentId!, status.identifier);
  }
}

async function finishPreparationView(context: ProjectContext, request: number): Promise<void> {
  const current = () => sameProject(projectContext(), context) && request === terminalViewRequest;
  if (!current()) return;
  const focused = document.activeElement;
  const restoreFocus = focused === document.body || terminalPanel.contains(focused);
  await hideTerminal();
  if (!current() || !restoreFocus || projectView.hidden || hasOpenDialog()) return;
  const retry = element<HTMLButtonElement>('retry-project-plan');
  const launch = element<HTMLButtonElement>('first-chat-open');
  if (!retry.hidden && !element('project-plan-feedback').hidden) { retry.focus(); return; }
  if (!launch.hidden && !launch.disabled) { launch.focus(); return; }
  element(hasFirstChat() ? 'first-chat-heading' : 'project-heading').focus();
}

async function openTerminal(...[command, arguments_, caption, foreground = true, fromReview = false]: TerminalLaunchRequest): Promise<void> {
  if (appData.selectedProject === null || !terminalLaunch.begin()) return;
  const project = appData.selectedProject;
  const context = projectContext()!;
  const projectId = appData.selectedProject.id;
  const environmentId = appData.selectedProject.environmentId;
  const environmentKey = [projectId, environmentId].join('|');
  const preparingEnvironment = command === 'prepare_environment';
  const viewRequest = foreground ? ++terminalViewRequest : terminalViewRequest;
  const current = () => sameProject(projectContext(), context);
  const ownsView = () => current() && viewRequest === terminalViewRequest;
  let openedIdentifier: number | null = null;
  void syncMenus();
  let previous: TerminalStatus | null;
  try { await refreshTerminals(); previous = currentTerminal(); }
  catch (error) {
    if (current() && preparingEnvironment) requestedChat = null;
    await finishTerminalLaunch(); void syncMenus();
    if (current() && preparingEnvironment) environmentFeedback('Could not start the project environment. ' + String(error));
    if (current() && foreground && !preparingEnvironment) showFolderError(String(error));
    return;
  }
  if (!current()) { await finishTerminalLaunch(); void syncMenus(); return; }
  const sessionId = command === 'start_agent_session' && arguments_.action === 'resume' ? arguments_.sessionId : null;
  const existingChat = command === 'start_agent_session' && sessionId !== null ? appData.terminals.find(status => status.running && isChat(status) && terminalMatches(status, project) && status.agentKind === arguments_.agentKind && status.conversationId === sessionId) : undefined;
  const purpose = command === 'terminal_start' ? 'local' : command === 'prepare_environment' ? 'environment' : command === 'start_agent_session' ? arguments_.action : arguments_.operation;
  const launchingChat = command === 'start_agent_session' && (arguments_.action === 'new-chat' || arguments_.action === 'resume');
  const blocking = command === 'start_project_operation' ? undefined : appData.terminals.find(status => status.running
    && (status.purpose === 'local' || terminalMatches(status, appData.selectedProject!))
    && ['local', 'environment', 'login', 'codex', 'resume', 'claude', 'claude-resume', 'shell'].includes(status.purpose)
    && (foreground && !preparingEnvironment || status.purpose !== 'local')
    && !(launchingChat && isChat(status)));
  if (existingChat || blocking) {
    await finishTerminalLaunch();
    void syncMenus();
    if (existingChat || blocking?.purpose === purpose) {
      if (purpose === 'environment') {
        environmentTerminals.set(environmentKey, blocking!.identifier);
        appData.selectedProject = { ...appData.selectedProject, stage: 'environment-starting' };
        environmentBoot.begin(appData.selectedProject, blocking!.identifier);
        updateCodexActions(); environmentFeedback();
      }
      if (!foreground) return;
      folderError.hidden = true;
      await returnToTerminal((existingChat ?? blocking)!.identifier, preparingEnvironment && !fromReview);
      if (fromReview && !element('environment-boot-overlay').hidden) element('environment-boot-card').focus();
    } else if (blocking) {
      const names: Record<string, string> = { login: 'account sign-in', shell: 'guest terminal', local: 'local terminal', environment: 'environment setup' };
      const blockingChat = isChat(blocking);
      const chatId = blockingChat ? chatConversationId(blocking) : null;
      const title = chatId ? selectedConversations().find(conversation => conversationKey(conversation) === chatId)?.title : null;
      const name = blockingChat ? (title ? `chat “${title}”` : 'chat') : names[blocking.purpose] ?? 'terminal';
      const nextAction = preparingEnvironment ? 'preparing the environment' : 'opening ' + caption;
      const message = `End the running ${name} before ${nextAction}.`;
      if (dialogOpen(projectSettingsDialog)) element(purpose === 'local' ? 'local-terminal-status' : purpose === 'environment' ? 'preparation-status' : 'guest-terminal-status').textContent = message;
      else showFolderError(message);
    }
    return;
  }
  terminalLaunch.start();
  element('local-terminal-status').textContent = '';
  if (foreground) {
    if (!preparingEnvironment || fromReview) closeProjectDialogs();
    await hideTerminal();
    terminalIdentifier = null;
  }
  startTerminalButton.disabled = true;
  if (foreground) terminalPanel.hidden = false;
  chooseFolderButton.disabled = true;
  folderError.hidden = true;
  if (foreground) {
    const loading = preparingEnvironment ? 'Preparing environment'
      : command === 'start_agent_session' && arguments_.action === 'new-chat' ? 'Starting ' + agentName(arguments_.agentKind)
      : command === 'start_agent_session' && arguments_.action === 'resume' ? 'Resuming ' + agentName(arguments_.agentKind)
      : 'Opening ' + caption;
    showTerminalStatus(loading + '…');
  }
  if (preparingEnvironment) {
    pendingEnvironmentKey = environmentKey;
    appData.selectedProject = { ...appData.selectedProject, stage: 'environment-starting' };
    environmentBoot.begin(project);
    updateCodexActions(); environmentFeedback();
    updateTerminalActions();
    if (fromReview && !element('environment-boot-overlay').hidden) element('environment-boot-card').focus();
    void syncMenus();
  }
  try {
    const layout = terminalLayout();
    const identifier = await invoke<number>(command, { ...arguments_, layout: { ...layout, visible: foreground && !preparingEnvironment && layout.visible } });
    openedIdentifier = identifier;
    terminalLaunch.publish();
    if (command === 'start_agent_session' && arguments_.action === 'new-chat' && current()) {
      firstChatAgents.delete(preparationKey(project));
      renderFirstChat();
    }
    if (foreground && ownsView()) terminalIdentifier = identifier;
    if (preparingEnvironment) {
      environmentTerminals.set(environmentKey, identifier);
      if (pendingEnvironmentKey === environmentKey) pendingEnvironmentKey = null;
      environmentBoot.attach(project, identifier);
      if (current()) await refreshSelectedPlan();
    }
    if (foreground && ownsView()) lastTerminals.set(projectId, identifier);
    await refreshTerminals(projectId);
    if (foreground && ownsView()) { showTerminalStatus(caption); await updateTerminalLayout(); }
    const snapshot = await invoke<TerminalStatus | null>('terminal_snapshot', { identifier });
    if (preparingEnvironment && snapshot && !snapshot.running) {
      const result = await invoke<Project>('finish_environment', { identifier: projectId, environmentId, terminalIdentifier: identifier });
      environmentBoot.finish(projectId, environmentId, identifier);
      if (current()) {
        appData.selectedProject = result;
        await refreshSelectedPlan();
        if (current()) {
          environmentFeedback(appData.selectedProject.stage === 'environment-ready' ? null : (appData.selectedProject.environmentAttempt?.error ?? 'Could not start the project environment. Check its terminal output and try again.'));
          if (ownsView() && hasFirstChat()) await finishPreparationView(context, viewRequest);
        }
        await updateTerminalLayout();
      }
    }
    updateTerminalActions();
    if (foreground && ownsView() && snapshot && !snapshot.running) showTerminalStatus(terminalCaption(snapshot));
    if (foreground && ownsView() && !projectView.hidden) await focusTerminal(identifier);
  } catch (error) {
    if (preparingEnvironment) {
      if (current()) requestedChat = null;
      if (openedIdentifier !== null) environmentFailures.set(environmentKey, openedIdentifier);
      if (pendingEnvironmentKey === environmentKey) pendingEnvironmentKey = null;
      environmentBoot.finish(projectId, environmentId, openedIdentifier ?? undefined);
      if (current()) await refreshSelectedPlan();
    }
    if (foreground && ownsView() && (!preparingEnvironment || openedIdentifier === null)) { terminalPanel.hidden = true; terminalIdentifier = previous?.identifier ?? null; }
    if (preparingEnvironment && current()) environmentFeedback('Could not start the project environment. ' + String(error));
    if (foreground && ownsView() && !preparingEnvironment) showFolderError('Could not open the terminal. ' + String(error));
    if (preparingEnvironment && ownsView()) await finishPreparationView(context, viewRequest);
    await updateTerminalLayout();
    chooseFolderButton.disabled = false;
  } finally {
    await finishTerminalLaunch(projectId);
    startTerminalButton.disabled = false;
    chooseFolderButton.disabled = false;
    updateCodexActions(); void syncMenus();
  }
}
async function closeTerminal(identifier: number, projectId: string | undefined, environmentId: string | null | undefined, restoreFocus = false): Promise<void> {
  if (!terminalLaunch.close(identifier)) return;
  updateTerminalActions();
  try {
    await invoke('terminal_close', { identifier });
    if (projectId && environmentId) environmentBoot.finish(projectId, environmentId, identifier);
    if (projectId && lastTerminals.get(projectId) === identifier) lastTerminals.delete(projectId);
    if (terminalIdentifier === identifier) {
      terminalIdentifier = null;
      terminalPanel.hidden = true;
      if (restoreFocus && !projectView.hidden && !hasOpenDialog()) element('project-heading').focus();
    }
    await refreshTerminals(projectId);
    void syncMenus();
    if (appData.selectedProject?.id === projectId) void refreshConversations();
  } catch (error) {
    if (terminalIdentifier === identifier) showTerminalStatus('Could not close the terminal. ' + String(error));
    if (!restoreFocus && appData.selectedProject && appData.selectedProject.id === projectId && appData.selectedProject.environmentId === environmentId) showFolderError('Could not close the environment terminal. ' + String(error));
    if (terminalIdentifier !== identifier) console.error('Could not close terminal ' + identifier + '.', error);
  } finally {
    terminalLaunch.closed(identifier); updateTerminalActions();
    if (terminalUpdatesAllowed(identifier)) void updateTerminalLayout();
  }
}

async function closeEnvironmentTerminal(projectId: string, environmentId: string, identifier: number): Promise<void> {
  if (terminalLaunch.isClosing(identifier)) return;
  try {
    const status = await invoke<TerminalStatus | null>('terminal_snapshot', { identifier });
    if (!status || status.running || status.purpose !== 'environment' || status.projectId !== projectId || status.environmentId !== environmentId) return;
    if (status.exitCode !== 0 || environmentFailures.get([projectId, environmentId].join('|')) === identifier) return;
    await closeTerminal(identifier, projectId, environmentId);
  } catch (error) {
    if (appData.selectedProject?.id === projectId && appData.selectedProject.environmentId === environmentId) showFolderError('Could not close the environment terminal. ' + String(error));
    console.error('Could not inspect environment terminal ' + identifier + '.', error);
  }
}

element('close-terminal').addEventListener('click', async () => {
  if (!element('close-terminal').hidden && terminalIdentifier !== null) await closeTerminal(terminalIdentifier, appData.selectedProject?.id, appData.selectedProject?.environmentId, true);
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
  if (event.payload.purpose === 'environment' && event.payload.environmentId && !terminalLaunch.blocksChanges()) await closeEnvironmentTerminal(event.payload.projectId, event.payload.environmentId, event.payload.identifier);
  void syncMenus();
  if (event.payload.identifier === terminalIdentifier) {
    const status = appData.terminals.find(status => status.identifier === terminalIdentifier);
    showTerminalStatus(status ? terminalCaption(status) : 'Terminal process ended');
  }
  const environmentId = event.payload.environmentId ?? (appData.selectedProject?.id === event.payload.projectId ? appData.selectedProject.environmentId : null);
  if (event.payload.purpose === 'environment' && environmentId && pendingEnvironmentKey !== [event.payload.projectId, environmentId].join('|')) {
    environmentBoot.finish(event.payload.projectId, environmentId, event.payload.identifier);
    await updateTerminalLayout();
  }
  if (event.payload.purpose === 'login' && event.payload.agentKind !== 'claude-code') await sharedChatgpt?.refresh();
  if (['codex', 'resume', 'claude', 'claude-resume'].includes(event.payload.purpose) && appData.selectedProject?.id === event.payload.projectId) { void refreshProjects(); void refreshConversations(); }
});
type PreparationEvent = { projectId: string; environmentId: string; terminalIdentifier: number };
await listen<PreparationEvent>('environment-preparation-verifying', event => {
  const { projectId: id, environmentId, terminalIdentifier } = event.payload;
  environmentBoot.verify({ id, environmentId }, terminalIdentifier);
  updateTerminalActions();
  void updateTerminalLayout();
});
await listen<{ project: Project; terminalIdentifier: number; message: string | null }>('environment-preparation-finished', async event => {
  const result = event.payload;
  const key = [result.project.id, result.project.environmentId].join('|');
  const currentAttempt = () => pendingEnvironmentKey !== key && (environmentTerminals.get(key) ?? 0) <= result.terminalIdentifier;
  if (!currentAttempt()) return;
  const viewRequest = terminalIdentifier === result.terminalIdentifier && !terminalPanel.hidden ? terminalViewRequest : null;
  environmentTerminals.set(key, result.terminalIdentifier);
  if (result.message && requestedChat?.project.id === result.project.id && requestedChat.project.environmentId === result.project.environmentId) requestedChat = null;
  if (result.message) environmentFailures.set(key, result.terminalIdentifier);
  if (!result.message) environmentFailures.delete(key);
  environmentBoot.finish(result.project.id, result.project.environmentId, result.terminalIdentifier);
  if (!terminalLaunch.blocksChanges()) await closeEnvironmentTerminal(result.project.id, result.project.environmentId, result.terminalIdentifier);
  if (!currentAttempt() || appData.selectedProject?.id !== result.project.id || appData.selectedProject.environmentId !== result.project.environmentId) return;
  selectedPlanFingerprint = null;
  appData.selectedProject = result.project;
  const context = projectContext()!;
  await updateTerminalLayout();
  if (!sameProject(projectContext(), context) || !currentAttempt()) return;
  await refreshSelectedPlan();
  if (!sameProject(projectContext(), context) || !currentAttempt()) return;
  updateCodexActions();
  element('preparation-status').textContent = result.message ?? 'Brig environment and guest agent version verified.';
  environmentFeedback(result.message ?? (appData.selectedProject?.stage === 'environment-ready' ? null : 'The project environment is not running. Try starting it again.'));
  if (viewRequest !== null && (hasFirstChat() || result.message)) await finishPreparationView(context, viewRequest);
  if (!result.message && appData.selectedProject?.stage === 'environment-ready') void refreshConversations();
  if (result.terminalIdentifier === terminalIdentifier) showTerminalStatus(result.message ? 'Environment setup failed. Prepare the environment again.' : 'Brig environment ready');
});
await listen<PreparationEvent & { message: string }>('environment-preparation-error', async event => {
  const key = [event.payload.projectId, event.payload.environmentId].join('|');
  const currentAttempt = () => pendingEnvironmentKey !== key && (environmentTerminals.get(key) ?? 0) <= event.payload.terminalIdentifier;
  if (!currentAttempt()) return;
  const viewRequest = terminalIdentifier === event.payload.terminalIdentifier && !terminalPanel.hidden ? terminalViewRequest : null;
  if (requestedChat?.project.id === event.payload.projectId && requestedChat.project.environmentId === event.payload.environmentId) requestedChat = null;
  environmentFailures.set(key, event.payload.terminalIdentifier);
  environmentBoot.finish(event.payload.projectId, event.payload.environmentId, event.payload.terminalIdentifier);
  if (!terminalLaunch.blocksChanges()) await closeEnvironmentTerminal(event.payload.projectId, event.payload.environmentId, event.payload.terminalIdentifier);
  if (!currentAttempt() || appData.selectedProject?.id !== event.payload.projectId || appData.selectedProject.environmentId !== event.payload.environmentId) return;
  const context = projectContext()!;
  await updateTerminalLayout();
  if (!sameProject(projectContext(), context) || !currentAttempt()) return;
  element('preparation-status').textContent = event.payload.message;
  environmentFeedback(event.payload.message);
  if (viewRequest !== null) await finishPreparationView(context, viewRequest);
});
await listen<{ identifier: number; message: string }>('terminal-input-error', event => { if (event.payload.identifier === terminalIdentifier) showTerminalStatus(event.payload.message); });

await listen<string>('menu-action', async event => {
  if (event.payload === 'choose-folder' && appData.selectedProject === null) { showView('project'); void chooseFolder(); }
  if (event.payload === 'settings') showView('settings');
  if (event.payload === 'project') { await hideTerminal(); showView('project'); rescanVisibleProjectLibrary(); }
  if (event.payload === 'return-terminal') { showView('project'); await returnToTerminal(); }
});
showView('project');
const startup = new StartupSequence();
await startup.run('window', () => invoke('frontend_ready'));
await startup.run('controllers', () => {
resetBoundaryDetails = initializeBoundaryTests(projectContext);
resetProjectPreview = initializeProjectPreview(projectContext);
resetGitHubAccess = initializeProjectGitHubAccess(projectContext);
projectImageUpdate = initializeProjectImageUpdate(
  projectContext,
  () => terminalLaunch.blocksChanges() || appData.terminals.some(terminal => terminal.running && terminal.projectId === appData.selectedProject?.id),
  async context => {
    if (sameProject(projectContext(), context)) await refreshSelectedProjectRecords();
  },
);
resetGatewayLogs = initializeGatewayLogs(() => {
  const project = appData.selectedProject;
  return project ? { ...projectContext()!, reference: project.profile + '@' + project.label } : null;
});
appDataStore.subscribe(({ selectedProject: project }, { selectedProject: previous }) => {
  if (project?.profile !== previous?.profile || project?.label !== previous?.label || project?.network !== previous?.network) resetGatewayLogs();
});
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
  resumeFirstSetup();
}));
claudeAccount = await startup.run('claude', () => initializeClaudeAccount(() => {
  updateCodexActions();
  resumeFirstSetup();
}));
updateCodexActions();
void initializeHost(readiness => {
  hostReadiness = readiness;
  if (waitingForFirstSetup && readiness === 'needs-attention') requestedChat = null;
  renderFirstChat(); resumeFirstSetup();
});
void syncMenus();
await startup.run('projects', rescanProjects);

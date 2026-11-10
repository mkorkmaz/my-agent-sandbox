import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './style.css';
import { initializeHost } from './host';
import { initializeSettings } from './settings';
import { renderProjects, renderConversations, type Conversation, type Project, type TerminalStatus } from './projects';
import { prepareProject, renderPlan, type Plan } from './preparation';
import { initializeBoundaryTests, resetBoundaryChoice } from './boundaries';
import { initializeOperations } from './operations';
import { initializeProjectPolicy } from './project-policy';
import { renderDashboard, renderUsageDetails, emptyUsage, type Dashboard, type SessionState, type TurnUsage } from './dashboard';
import { initializeSidebar } from './sidebar';

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
let terminalIdentifier: number | null = null;
let startingTerminal = false;
let preparingProject: string | null = null;
let selectedPlanFingerprint: string | null = null;
let projects: Project[] = [];
let conversations: Conversation[] = [];
let dashboard: Dashboard = { conversations: [], usage: emptyUsage, pricingDate: '2026-11-10' };
let sessionView: SessionState = 'active';
let changingSession = false;
let dashboardLoaded = false;
let terminalRunning = false;
let selectedConversationId: string | null = null;
let terminals: TerminalStatus[] = [];
const lastTerminals = new Map<string, number>();
let terminalListRequest = 0;
let terminalViewRequest = 0;
let pendingLayout = false;
let layoutWork: Promise<void> | null = null;
let projectListRequest = 0;
let conversationListRequest = 0;
let loadingConversations = false;
const terminalPanel = element('terminal-panel');
const terminalSurface = element('terminal-surface');
const startTerminalButton = element<HTMLButtonElement>('start-terminal');

function isChat(status: TerminalStatus): boolean { return ['codex', 'resume'].includes(status.purpose); }

function currentTerminal(): TerminalStatus | null {
  const remembered = selectedProject ? lastTerminals.get(selectedProject.id) : null;
  return terminals.find(status => status.identifier === (terminalIdentifier ?? remembered))
    ?? terminals.find(status => status.running && isChat(status))
    ?? terminals.find(status => status.running)
    ?? terminals.at(-1) ?? null;
}

function renderSessionSidebar(): void {
  const chats = terminals.filter(status => status.running && isChat(status));
  const rows = conversations.filter(conversation => (conversation.state ?? 'active') === 'active').map(conversation => {
    const terminal = chats.find(status => status.conversationId === conversation.id);
    return { ...conversation, available: terminal ? true : conversation.available, terminalId: terminal?.identifier, busy: terminal?.busy };
  });
  for (const terminal of chats) {
    if (rows.some(row => row.terminalId === terminal.identifier)) continue;
    const saved = conversations.find(conversation => conversation.id === terminal.conversationId);
    rows.unshift({ id: terminal.conversationId ?? 'terminal-' + terminal.identifier, title: saved?.title ?? 'New Chat ' + terminal.identifier, updatedAt: saved?.updatedAt ?? Date.now(), available: true, terminalId: terminal.identifier, busy: terminal.busy });
  }
  const selected = terminals.find(status => status.identifier === terminalIdentifier);
  selectedConversationId = selected?.conversationId ?? (selected && isChat(selected) ? 'terminal-' + selected.identifier : null);
  renderConversations(rows, selectedConversationId, resumeConversation);
  if (chats.length) element('session-list-status').hidden = true;
}

async function refreshTerminals(): Promise<void> {
  if (!selectedProject) return;
  const projectId = selectedProject.id;
  const request = ++terminalListRequest;
  const result = await invoke<TerminalStatus[]>('terminal_sessions', { projectId });
  if (selectedProject?.id !== projectId || request !== terminalListRequest) return;
  terminals = result;
  renderSessionSidebar();
  updateTerminalActions(currentTerminal());
  void syncMenus();
}

function updateTerminalActions(status: TerminalStatus | null): void {
  const button = element<HTMLButtonElement>('return-to-terminal');
  button.hidden = status === null || (!status.running && status.purpose === 'environment');
  button.textContent = status && ['codex', 'resume'].includes(status.purpose) ? 'Return to chat' : 'Return to terminal';
  button.className = status?.running ? 'primary-button' : 'secondary-button';
  terminalRunning = terminals.some(terminal => terminal.running);
  projectView.classList.toggle('has-live-terminal', terminalRunning);
  element<HTMLButtonElement>('start-codex').disabled = startingTerminal || selectingProject || choosingFolder || preparingProject !== null;
  element<HTMLButtonElement>('start-codex').title = 'New Chat';
}

async function syncMenus(): Promise<void> {
  const busy = choosingFolder || selectingProject || preparingProject !== null || startingTerminal;
  chooseFolderButton.disabled = busy;
  element<HTMLButtonElement>('start-codex').disabled = busy;
  projectNavigation.disabled = busy;
  element<HTMLButtonElement>('refresh-sessions').disabled = busy || loadingConversations;
  for (const button of document.querySelectorAll<HTMLButtonElement>('.sidebar-project, .sidebar-session, .recent-project')) button.disabled = busy || button.dataset.available === 'false';
  renderSessionDashboard();
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
  renderProjects(projects, selectedProject, project => selectProject(project, true));
}

async function refreshProjects(): Promise<void> {
  const request = ++projectListRequest;
  try {
    const result = await invoke<Project[]>('recent_projects');
    if (request !== projectListRequest) return;
    projects = result;
    renderProjectLists();
  } catch (error) { if (request === projectListRequest) showFolderError(String(error)); }
}

async function refreshConversations(): Promise<void> {
  if (!selectedProject) return;
  const projectId = selectedProject.id;
  const request = ++conversationListRequest;
  loadingConversations = true;
  const status = element('session-list-status');
  status.hidden = false;
  status.textContent = 'Loading sessions…';
  element<HTMLButtonElement>('refresh-sessions').disabled = true;
  try {
    const result = await invoke<Dashboard>('project_dashboard', { identifier: projectId });
    if (request !== conversationListRequest || selectedProject?.id !== projectId) return;
    applyDashboard(result);
    const hasActive = conversations.some(conversation => (conversation.state ?? 'active') === 'active') || terminals.some(terminal => terminal.running && isChat(terminal));
    status.textContent = hasActive ? '' : 'No active sessions. Start a New Chat or restore an archived session.';
    status.hidden = hasActive;
  } catch (error) {
    if (request === conversationListRequest && selectedProject?.id === projectId) { status.textContent = String(error); element('dashboard-session-status').textContent = String(error); element('dashboard-session-status').hidden = false; }
  } finally {
    if (request === conversationListRequest) {
      loadingConversations = false;
      element<HTMLButtonElement>('refresh-sessions').disabled = startingTerminal || selectingProject || preparingProject !== null;
    }
  }
}

function applyDashboard(result: Dashboard): void {
  dashboard = result;
  dashboardLoaded = true;
  conversations = result.conversations;
  renderSessionSidebar();
  if (selectedConversationId && !terminalPanel.hidden) {
    const current = conversations.find(conversation => conversation.id === selectedConversationId);
    if (current) showTerminalStatus(current.title);
  }
  renderSessionDashboard();
}
function renderSessionDashboard(): void {
  renderDashboard(dashboard, sessionView, resumeConversation, changeConversationState, showUsageDetails, changingSession || startingTerminal || selectingProject || choosingFolder || preparingProject !== null);
  element('project-usage').hidden = !dashboardLoaded;
  element('usage-note').hidden = !dashboardLoaded;
  element('project-overview').setAttribute('aria-busy', String(!dashboardLoaded));
  if (!dashboardLoaded) { element('dashboard-session-status').textContent = 'Loading sessions and usage…'; element('dashboard-session-status').hidden = false; element('session-table').hidden = true; }

}
async function changeConversationState(conversation: Conversation, state: SessionState): Promise<void> {
  if (!selectedProject || changingSession) return;
  const projectId = selectedProject.id;
  changingSession = true; renderSessionDashboard();
  try {
    await invoke('change_conversation_state', { identifier: projectId, sessionId: conversation.id, state });
    if (selectedProject?.id === projectId) { folderError.hidden = true; await refreshConversations(); }
  } catch (error) { if (selectedProject?.id === projectId) showFolderError(String(error)); }
  finally { changingSession = false; renderSessionDashboard(); document.querySelector<HTMLButtonElement>('[data-session-state="' + sessionView + '"]')?.focus(); }
}
async function showUsageDetails(conversation: Conversation): Promise<void> {
  if (!selectedProject) return;
  const projectId = selectedProject.id;
  element('session-usage-title').textContent = conversation.title;
  element('session-usage-details').replaceChildren();
  element('session-usage-status').textContent = 'Loading prompt usage…';
  usageDialog.showModal(); element('close-session-usage').focus(); await updateTerminalLayout();
  try {
    const turns = await invoke<TurnUsage[]>('conversation_usage', { identifier: projectId, sessionId: conversation.id });
    if (selectedProject?.id !== projectId || !usageDialog.open) return;
    renderUsageDetails(turns);
    element('session-usage-status').textContent = turns.some(turn => turn.requests.length) ? 'Recorded after each response. Prices are retained with each request.' : 'No token usage has been recorded for this session yet.';
  } catch (error) { element('session-usage-status').textContent = String(error); }
}

async function resumeConversation(conversation: Conversation): Promise<void> {
  if (!selectedProject || startingTerminal || selectingProject || preparingProject !== null) return;
  const terminal = terminals.find(status => status.running && (status.identifier === conversation.terminalId || status.conversationId === conversation.id));
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
  await openTerminal('start_codex_session', { identifier: projectId, fingerprint: selectedPlanFingerprint, action: 'resume', sessionId: conversation.id, useBrigBoundary: element<HTMLInputElement>('use-brig-boundary').checked }, conversation.title);
}

async function selectProject(project: Project, recordOpen = false): Promise<void> {
  if (startingTerminal || selectingProject || preparingProject !== null) { showFolderError('Wait for project setup to finish.'); return; }
  selectingProject = true;
  void syncMenus();
  try { await applySelectedProject(recordOpen ? await invoke<Project>('open_project', { identifier: project.id }) : project); }
  catch (error) { showFolderError(String(error)); }
  finally { selectingProject = false; void syncMenus(); }
}

async function applySelectedProject(project: Project): Promise<void> {
  await hideTerminal();
  terminalIdentifier = null;
  selectedProject = project;
  selectedConversationId = null;
  terminals = [];
  ++terminalListRequest;
  conversations = [];
  ++conversationListRequest;
  loadingConversations = false;
  dashboardLoaded = false;
  dashboard = { conversations: [], usage: emptyUsage, pricingDate: '2026-11-10' }; sessionView = 'active'; renderSessionDashboard();
  renderConversations([], null, resumeConversation);
  element('session-list-status').textContent = 'Loading sessions…';
  element('session-list-status').hidden = false;
  renderProjectLists();
  projectView.classList.add('has-project');
  selectedPlanFingerprint = null;
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
  const names: Record<string, string> = { codex: 'Codex', resume: 'Codex', login: 'ChatGPT sign-in', shell: 'Guest terminal', local: 'Local terminal', environment: 'Environment setup' };
  const title = conversations.find(conversation => conversation.id === selectedConversationId)?.title;
  showTerminalStatus(status.running ? (title ?? names[status.purpose] ?? 'Operation terminal') : 'Terminal process ended');
  await updateTerminalLayout();
  if (!projectView.hidden && request === terminalViewRequest) await invoke('terminal_focus', { identifier: status.identifier });
}

async function refreshSelectedPlan(): Promise<void> {
  if (!selectedProject) return;
  const id = selectedProject.id;
  try {
    const result = await invoke<{ project: Project; plan: Plan; fingerprint: string }>('preview_project', { identifier: id });
    if (selectedProject?.id !== id) return;
    if (selectedPlanFingerprint !== result.fingerprint) resetBoundaryChoice();
    selectedProject = result.project;
    selectedPlanFingerprint = result.fingerprint;
    renderPlan(result.plan);
    element('prepare-environment').hidden = false;
    element('refresh-plan').hidden = false;
    updateCodexActions();
    element('preparation-status').textContent = 'Execution plan refreshed.';
  } catch (error) { if (selectedProject?.id === id) element('preparation-status').textContent = String(error); }
}

async function prepareSelectedProject(chosenReference: string | null = null): Promise<void> {
  if (selectedProject === null || preparingProject !== null) return;
  const project = selectedProject;
  selectedPlanFingerprint = null;
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
    if (selectedProject?.id !== project.id) return;
    selectedProject = result.project;
    selectedPlanFingerprint = result.fingerprint;
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
    if (selectedProject?.id !== project.id) return;
    element('preparation-status').textContent = String(error);
    element('retry-preparation').hidden = false;
  } finally { preparingProject = null; void syncMenus(); }
}

function updateCodexActions(): void {
  const ready = selectedProject?.stage === 'environment-ready';
  element('codex-actions').hidden = !ready;
  element('project-overview').hidden = !ready;
  element('project-setup').hidden = ready;
  projectView.classList.toggle('environment-ready', ready);
}

async function checkLogin(): Promise<void> {
  if (!selectedProject) return;
  try {
    const result = await invoke<{ method: string; message: string }>('login_status', { identifier: selectedProject.id });
    element('login-status').textContent = result.message;
  } catch (error) { element('login-status').textContent = String(error); }
}

for (const [id, action, caption] of [['codex-login', 'login', 'Sign in with ChatGPT'], ['start-codex', 'codex', 'New Chat'], ['guest-shell', 'shell', 'Guest terminal']]) {
  element(id).addEventListener('click', () => {
    if (selectedProject === null || selectedPlanFingerprint === null) return;
    void openTerminal('start_codex_session', { identifier: selectedProject.id, fingerprint: selectedPlanFingerprint, action, useBrigBoundary: element<HTMLInputElement>('use-brig-boundary').checked }, caption);
  });
}
element('check-login').addEventListener('click', () => { void checkLogin(); });

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
  void updateTerminalLayout();
  if (!settings && !terminalPanel.hidden && terminalIdentifier !== null) void invoke('terminal_focus', { identifier: terminalIdentifier });
}

async function showProjects(message = 'Choose where Brig & Codex will work.'): Promise<void> {
  if (startingTerminal || selectingProject || preparingProject !== null) return;
  await hideTerminal();
  selectedProject = null;
  terminalIdentifier = null;
  terminals = [];
  ++terminalListRequest;
  selectedPlanFingerprint = null;
  selectedConversationId = null;
  conversations = [];
  ++conversationListRequest;
  loadingConversations = false;
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
    if (!projectView.hidden && terminalPanel.hidden) element('project-heading').focus();
    if (!projectView.hidden && !terminalPanel.hidden && terminalIdentifier !== null) void invoke('terminal_focus', { identifier: terminalIdentifier });
  }
}

function closeProjectDialogs(): void {
  for (const dialog of [projectSettingsDialog, usageDialog, element<HTMLDialogElement>('removal-dialog')]) if (dialog.open) dialog.close();
}
async function showProjectSettings(): Promise<void> {
  if (!selectedProject) return;
  const projectId = selectedProject.id;
  element<HTMLFieldSetElement>('project-preference-fields').disabled = true;
  element('project-preferences-status').textContent = 'Loading project settings…';
  projectSettingsDialog.showModal(); await updateTerminalLayout();
  try {
    const result = await invoke<{ project: Project; options: { cpus: number; memoryMb: number; verification: string; network: string } }>('read_project_preferences', { identifier: projectId });
    if (selectedProject?.id !== projectId || !projectSettingsDialog.open) return;
    element<HTMLInputElement>('project-name').value = result.project.name;
    element<HTMLInputElement>('project-cpus').value = String(result.options.cpus);
    element<HTMLInputElement>('project-memory').value = String(result.options.memoryMb);
    element<HTMLSelectElement>('project-verification').value = result.options.verification;
    element<HTMLSelectElement>('project-network').value = result.options.network;
    element<HTMLFieldSetElement>('project-preference-fields').disabled = false;
    element('project-preferences-status').textContent = '';
  } catch (error) { element('project-preferences-status').textContent = String(error); }
}
element('project-settings-button').addEventListener('click', () => { void showProjectSettings(); });
element('setup-project').addEventListener('click', () => { void showProjectSettings(); });
element('close-project-settings').addEventListener('click', () => { projectSettingsDialog.close(); });
element('close-session-usage').addEventListener('click', () => { usageDialog.close(); });
for (const dialog of [projectSettingsDialog, usageDialog]) dialog.addEventListener('close', () => { void updateTerminalLayout(); });
element<HTMLFormElement>('project-preferences').addEventListener('submit', async event => {
  event.preventDefault(); if (!selectedProject) return;
  const id = selectedProject.id; const fields = element<HTMLFieldSetElement>('project-preference-fields'); fields.disabled = true;
  element('project-preferences-status').textContent = 'Saving project settings…';
  try {
    const project = await invoke<Project>('save_project_preferences', { identifier: id, name: element<HTMLInputElement>('project-name').value, options: { cpus: Number(element<HTMLInputElement>('project-cpus').value), memoryMb: Number(element<HTMLInputElement>('project-memory').value), verification: element<HTMLSelectElement>('project-verification').value, network: element<HTMLSelectElement>('project-network').value } });
    if (selectedProject?.id !== id) return;
    selectedProject = project; element('project-heading').textContent = project.name; element('project-heading').title = project.name;
    await prepareSelectedProject(); updateCodexActions(); await refreshProjects();
    element('project-preferences-status').textContent = 'Project settings saved. Prepare the environment to apply them.';
  } catch (error) { if (selectedProject?.id === id) element('project-preferences-status').textContent = String(error); }
  finally { fields.disabled = false; }
});
for (const tab of document.querySelectorAll<HTMLButtonElement>('[data-session-state]')) tab.addEventListener('click', () => { sessionView = tab.dataset.sessionState as SessionState; renderSessionDashboard(); });
await listen<[string, Dashboard]>('project-usage-updated', event => {
  if (selectedProject?.id !== event.payload[0]) return;
  applyDashboard(event.payload[1]);
  const hasActive = conversations.some(conversation => (conversation.state ?? 'active') === 'active') || terminals.some(terminal => terminal.running && isChat(terminal));
  element('session-list-status').hidden = hasActive;
  element('session-list-status').textContent = hasActive ? '' : 'No active sessions. Start a New Chat or restore an archived session.';
});
await listen<[string, string]>('project-usage-error', event => {
  if (selectedProject?.id === event.payload[0]) { element('dashboard-session-status').textContent = event.payload[1]; element('dashboard-session-status').hidden = false; }
});

projectNavigation.addEventListener('click', () => { void showProjects(); });
element('sidebar-current-project').addEventListener('click', async () => { await hideTerminal(); showView('project'); void refreshProjects(); void refreshConversations(); });
element('refresh-sessions').addEventListener('click', () => { void refreshConversations(); });
settingsNavigation.addEventListener('click', () => showView('settings'));
chooseFolderButton.addEventListener('click', () => { void chooseFolder(); });
element('retry-preparation').addEventListener('click', () => { void prepareSelectedProject(); });
element('adopt-session').addEventListener('click', () => { void prepareSelectedProject(element<HTMLSelectElement>('session-choice').value); });
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

async function openTerminal(command: string, arguments_: Record<string, unknown>, caption: string): Promise<void> {
  if (selectedProject === null || startingTerminal) return;
  const projectId = selectedProject.id;
  startingTerminal = true;
  void syncMenus();
  let previous: TerminalStatus | null;
  try { await refreshTerminals(); previous = currentTerminal(); }
  catch (error) { startingTerminal = false; void syncMenus(); showFolderError(String(error)); return; }
  const existingChat = typeof arguments_.sessionId === 'string' ? terminals.find(status => status.running && status.conversationId === arguments_.sessionId) : undefined;
  const launchingChat = command === 'start_codex_session' && ['codex', 'resume'].includes(String(arguments_.action));
  const blocking = command === 'start_project_operation' ? undefined : terminals.find(status => status.running
    && ['local', 'environment', 'login', 'codex', 'resume', 'shell'].includes(status.purpose)
    && !(launchingChat && isChat(status)));
  if (existingChat || blocking) {
    startingTerminal = false;
    void syncMenus();
    if (blocking && launchingChat) showFolderError('End the current setup or shell process before starting a chat.');
    else folderError.hidden = true;
    await returnToTerminal((existingChat ?? blocking)!.identifier);
    return;
  }
  closeProjectDialogs();
  await hideTerminal();
  terminalIdentifier = null;
  startTerminalButton.disabled = true;
  terminalPanel.hidden = false;
  chooseFolderButton.disabled = true;
  folderError.hidden = true;
  try {
    terminalIdentifier = await invoke<number>(command, { ...arguments_, layout: terminalLayout() });
    lastTerminals.set(projectId, terminalIdentifier);
    await refreshTerminals();
    showTerminalStatus(caption);
    await updateTerminalLayout();
    const snapshot = await invoke<TerminalStatus | null>('terminal_snapshot', { identifier: terminalIdentifier });
    updateTerminalActions(snapshot);
    if (snapshot && !snapshot.running) showTerminalStatus('Terminal process ended');
    if (!projectView.hidden) await invoke('terminal_focus', { identifier: terminalIdentifier });
  } catch (error) {
    terminalPanel.hidden = true;
    terminalIdentifier = previous?.identifier ?? null;
    folderError.textContent = 'Could not open the terminal. ' + String(error);
    folderError.hidden = false;
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
  const projectId = selectedProject?.id;
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
  if (event.payload.identifier === terminalIdentifier) showTerminalStatus('Terminal process ended');
  if (event.payload.purpose === 'login' && selectedProject?.id === event.payload.projectId) await checkLogin();
  if (['codex', 'resume'].includes(event.payload.purpose) && selectedProject?.id === event.payload.projectId) { void refreshProjects(); void refreshConversations(); }
  if (event.payload.purpose !== 'environment') return;
  try {
    const project = await invoke<Project>('finish_environment', { identifier: event.payload.projectId, terminalIdentifier: event.payload.identifier });
    if (selectedProject?.id === project.id) { selectedProject = project; await refreshSelectedPlan(); updateCodexActions(); element('preparation-status').textContent = 'Brig environment and guest Codex version verified.'; }
    if (event.payload.identifier === terminalIdentifier) showTerminalStatus('Brig environment ready');
  } catch (error) { if (selectedProject?.id === event.payload.projectId) element('preparation-status').textContent = String(error); }
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
initializeBoundaryTests(() => selectedProject ? { id: selectedProject.id, fingerprint: selectedPlanFingerprint } : null);
initializeProjectPolicy(() => selectedProject, async (project, detached) => {
  selectedProject = project;
  updateCodexActions();
  selectedPlanFingerprint = null;
  resetBoundaryChoice();
  element('codex-actions').hidden = true;
  element('prepare-environment').hidden = true;
  element('sharing-plan').hidden = true;
  if (!detached) await refreshSelectedPlan();
});
initializeOperations(() => selectedProject ? { id: selectedProject.id, fingerprint: selectedPlanFingerprint } : null, openTerminal, project => {
  selectedProject = project; selectedPlanFingerprint = null; terminalIdentifier = null; terminalPanel.hidden = true; updateTerminalActions(null); resetBoundaryChoice(); updateCodexActions(); void refreshSelectedPlan(); void syncMenus();
}, async message => { terminalIdentifier = null; await showProjects(message); });
await initializeSettings();
void initializeHost();
void syncMenus();
await refreshProjects();

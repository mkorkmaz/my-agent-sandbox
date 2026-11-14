import type { Conversation as RecordedConversation, SessionState } from '../usage/usage-types';
import { createAgentLogo } from '../providers/agent-logo';

export type ProjectStage = 'selected' | 'creating-policy' | 'policy-created' | 'attaching-policy'
  | 'editing-policy' | 'policy-detached' | 'ready' | 'environment-unverified'
  | 'environment-starting' | 'environment-ready' | 'stopping' | 'removing';

export interface Project {
  id: string;
  environmentId: string;
  agentKind: 'codex' | 'claude-code';
  name: string;
  path: string;
  profile: string;
  label: string;
  ownsBinding: boolean;
  policyDetached?: boolean;
  ownsSession: boolean;
  policy: { name: string; owned: boolean; fingerprint: string | null } | null;
  stage: ProjectStage;
  network?: string;
  options?: { cpus: number; memoryMb: number; verification: string; network: string } | null;
  updatedAt?: number;
  lastOpenedAt?: number | null;
  gitBranch?: string | null;
  available?: boolean;
  environmentAttempt?: { completed: boolean; error: string | null; terminalIdentifier: number } | null;
}

export type AgentKind = Project['agentKind'];
export function agentName(kind: AgentKind | null | undefined): string { return kind === 'claude-code' ? 'Claude Code' : 'Codex'; }
export interface Conversation extends RecordedConversation { terminalId?: number; busy?: boolean }
export function conversationKey(conversation: Pick<Conversation, 'id'> & { environmentId?: string | null }): string {
  return conversation.environmentId ? conversation.environmentId + '/' + conversation.id : conversation.id;
}
export function terminalMatches(status: TerminalStatus, project: Pick<Project, 'environmentId' | 'agentKind'>): boolean {
  return status.environmentId ? status.environmentId === project.environmentId : status.purpose !== 'local' && project.agentKind === 'codex';
}
export function isChat(status: TerminalStatus): boolean { return ['codex', 'resume', 'claude', 'claude-resume'].includes(status.purpose); }

export function isStandaloneConversation(conversation: Conversation): boolean {
  return conversation.lineage.kind !== 'subagent' && !['resolved', 'orphan', 'invalid'].includes(conversation.relationStatus);
}

export function standaloneConversations(conversations: Conversation[], state: SessionState): Conversation[] {
  return conversations.filter(conversation => isStandaloneConversation(conversation) && conversation.state === state);
}

export interface TerminalStatus { identifier: number; projectId: string; environmentId?: string | null; agentKind?: AgentKind | null; purpose: string; running: boolean; exitCode: number | null; conversationId: string | null; busy: boolean }

export const terminalNames: Record<string, string> = { claude: 'Claude Code', 'claude-resume': 'Claude Code', codex: 'Codex', resume: 'Codex', login: 'ChatGPT sign-in', shell: 'Guest terminal', local: 'Local terminal', environment: 'Environment startup', 'git-status': 'Git status', 'git-diff': 'Working tree diff', 'git-staged-diff': 'Staged diff', tests: 'Project tests', doctor: 'Brig doctor', info: 'Session info', 'policy-check': 'Policy bindings', logs: 'Recent logs' };

export function terminalCaption(status: TerminalStatus): string {
  const name = status.purpose === 'login' && status.agentKind === 'claude-code' ? 'Claude sign-in' : terminalNames[status.purpose] ?? 'Operation terminal';
  return status.running ? name + ' · running' : name + (status.exitCode === null ? ' · ended' : ' · exit ' + status.exitCode);
}

function preserveListFocus(list: HTMLElement, key: 'sessionId' | 'terminalId' | 'projectId'): () => void {
  if (!document.hasFocus()) return () => {};
  const buttons = [...list.querySelectorAll<HTMLButtonElement>('button')];
  const active = document.activeElement;
  const index = buttons.findIndex(button => button === active);
  const identity = index < 0 ? null : buttons[index].dataset[key];
  return () => {
    if (index < 0 || (document.activeElement !== document.body && document.activeElement !== active && !list.contains(document.activeElement))) return;
    const candidates = [...list.querySelectorAll<HTMLButtonElement>('button')];
    const replacement = candidates.find(button => button.dataset[key] === identity && !button.disabled)
      ?? candidates.slice(index).find(button => !button.disabled)
      ?? candidates.slice(0, index).reverse().find(button => !button.disabled)
      ?? document.getElementById(key === 'projectId' ? 'choose-folder' : 'refresh-sessions');
    replacement?.focus({ preventScroll: true });
  };
}

export function renderTerminals(terminals: TerminalStatus[], selected: number | null, open: (identifier: number) => Promise<void>): void {
  const list = document.getElementById('sidebar-terminal-list')!;
  const restoreFocus = preserveListFocus(list, 'terminalId');
  list.replaceChildren();
  const visible = terminals.filter(status => !isChat(status) || (!status.running && !status.conversationId));
  document.getElementById('sidebar-terminals')!.hidden = visible.length === 0;
  for (const status of visible) {
    const row = document.createElement('li');
    const button = document.createElement('button');
    button.className = 'sidebar-session'; button.dataset.terminalId = String(status.identifier);
    button.title = terminalCaption(status); button.setAttribute('aria-label', button.title);
    if (status.identifier === selected) button.setAttribute('aria-current', 'true');
    const title = document.createElement('span'); title.className = 'sidebar-session-title'; title.textContent = button.title;
    button.append(title); button.addEventListener('click', () => { void open(status.identifier); });
    row.append(button); list.append(row);
  }
  restoreFocus();
}

export interface ProjectContext { id: string; environmentId: string; fingerprint: string | null; epoch: number; view: number }
export function sameProject(current: ProjectContext | null, captured: ProjectContext): boolean { return current?.id === captured.id && current.environmentId === captured.environmentId && current.epoch === captured.epoch; }
export function sameProjectView(current: ProjectContext | null, captured: ProjectContext): boolean { return sameProject(current, captured) && current?.view === captured.view; }

function initials(name: string): string {
  return name.split(/[-_\s]+/).filter(Boolean).slice(0, 2).map(part => Array.from(part)[0]).join('').toLocaleUpperCase();
}

function projectIdentity(project: Project): HTMLElement[] {
  const monogram = document.createElement('span');
  monogram.className = 'project-monogram';
  monogram.textContent = initials(project.name);
  monogram.setAttribute('aria-hidden', 'true');
  const identity = document.createElement('span');
  identity.className = 'project-row-identity';
  const name = document.createElement('strong');
  name.className = 'project-row-name';
  name.textContent = project.name;
  identity.append(name);
  if (project.gitBranch) identity.append(branchLabel(project.gitBranch));
  return [monogram, identity];
}

function branchLabel(branch: string): HTMLElement {
  const label = document.createElement('span');
  label.className = 'project-branch';
  label.title = branch;
  const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  for (const [name, value] of Object.entries({ viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', 'stroke-width': '1.5', 'stroke-linecap': 'round', 'stroke-linejoin': 'round', 'aria-hidden': 'true', focusable: 'false' })) icon.setAttribute(name, value);
  const path = document.createElementNS(icon.namespaceURI, 'path');
  path.setAttribute('d', 'M6 6v12M18 6v2a6 6 0 0 1-6 6H6');
  icon.append(path);
  for (const [cx, cy] of [[6, 4], [6, 20], [18, 4]]) {
    const circle = document.createElementNS(icon.namespaceURI, 'circle');
    circle.setAttribute('cx', String(cx)); circle.setAttribute('cy', String(cy)); circle.setAttribute('r', '2');
    icon.append(circle);
  }
  const text = document.createElement('span');
  text.textContent = branch;
  label.append(icon, text);
  return label;
}

function projectButton(project: Project, className: string, select: (project: Project) => Promise<void>): HTMLButtonElement {
  const button = document.createElement('button');
  button.className = className;
  button.dataset.projectId = project.id;
  button.dataset.available = String(project.available !== false);
  button.disabled = project.available === false;
  button.title = project.name + '\n' + project.path;
  button.append(...projectIdentity(project));
  button.addEventListener('click', () => { void select(project); });
  return button;
}

export function renderProjects(projects: Project[], selected: Project | null, select: (project: Project) => Promise<void>, forget: (project: Project) => void): void {
  const ordered = [...projects].sort((first, second) => (second.lastOpenedAt ?? (second.updatedAt ?? 0) * 1000) - (first.lastOpenedAt ?? (first.updatedAt ?? 0) * 1000));
  const sidebar = document.getElementById('sidebar-project-list')!;
  const list = document.getElementById('recent-project-list')!;
  const restoreSidebarFocus = preserveListFocus(sidebar, 'projectId');
  const restoreLibraryFocus = preserveListFocus(list, 'projectId');
  sidebar.replaceChildren(); list.replaceChildren();
  for (const project of ordered) {
    const sidebarRow = document.createElement('li'); sidebarRow.append(projectButton(project, 'sidebar-project', select)); sidebar.append(sidebarRow);
    const row = document.createElement('tr'); row.title = project.path;
    const name = document.createElement('td');
    const button = projectButton({ ...project, gitBranch: null }, 'recent-project', select); name.append(button);
    if (project.available === false) {
      const actions = document.createElement('div'); actions.className = 'project-record-actions';
      const unavailable = document.createElement('span'); unavailable.textContent = 'Folder unavailable';
      const remove = document.createElement('button'); remove.className = 'secondary-button project-record-forget';
      remove.dataset.projectId = project.id; remove.textContent = 'Forget…'; remove.setAttribute('aria-label', 'Forget record for ' + project.name);
      remove.addEventListener('click', () => { forget(project); });
      actions.append(unavailable, remove); name.append(actions);
    }
    const branch = document.createElement('td'); if (project.gitBranch) branch.append(branchLabel(project.gitBranch));
    if (!project.gitBranch) branch.textContent = '—';
    const opened = document.createElement('td'); opened.className = 'project-opened';
    const timestamp = project.lastOpenedAt ?? (project.updatedAt ?? 0) * 1000;
    opened.textContent = timestamp ? new Date(timestamp).toLocaleDateString('en-US', { month: 'short', day: 'numeric' }) : '—';
    row.append(name, branch, opened); list.append(row);
  }
  sidebar.hidden = selected !== null;
  document.getElementById('sidebar-project-context')!.hidden = selected === null;
  document.getElementById('recent-projects')!.hidden = selected !== null || projects.length === 0;
  document.getElementById('project-navigation-label')!.textContent = 'Projects';
  document.getElementById('empty-projects')!.hidden = selected !== null || projects.length > 0;
  const current = document.getElementById('sidebar-current-project')!;
  current.replaceChildren();
  if (selected) {
    const summary = projects.find(project => project.id === selected.id) ?? selected;
    current.title = summary.name + '\n' + summary.path;
    current.append(...projectIdentity(summary));
  }
  const selectedBranch = document.getElementById('selected-branch')!;
  const branch = projects.find(project => project.id === selected?.id)?.gitBranch ?? selected?.gitBranch;
  selectedBranch.hidden = !selected || !branch;
  selectedBranch.replaceChildren();
  if (selected && branch) selectedBranch.append(branchLabel(branch));
  restoreSidebarFocus();
  restoreLibraryFocus();
}

export function renderConversations(conversations: Conversation[], selectedId: string | null, resume: (conversation: Conversation) => Promise<void>): void {
  const list = document.getElementById('sidebar-session-list')!;
  const restoreFocus = preserveListFocus(list, 'sessionId');
  list.replaceChildren();
  for (const conversation of standaloneConversations(conversations, 'active')) {
    const row = document.createElement('li');
    const button = document.createElement('button');
    button.className = 'sidebar-session';
    button.disabled = conversation.available === false;
    button.dataset.available = String(conversation.available !== false);
    button.title = conversation.title + (conversation.subagentCount ? '\nIncludes ' + conversation.subagentCount + ' subagent' + (conversation.subagentCount === 1 ? '' : 's') : '');
    button.setAttribute('aria-label', conversation.title + ', ' + agentName(conversation.agentKind));
    button.append(createAgentLogo(conversation.agentKind));
    if (conversation.busy) {
      const activity = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      for (const [key, value] of Object.entries({ class: 'session-activity', viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', 'stroke-width': '1.5', 'stroke-linecap': 'round', 'aria-hidden': 'true' })) activity.setAttribute(key, value);
      const path = document.createElementNS(activity.namespaceURI, 'path');
      path.setAttribute('d', 'M12 2a10 10 0 1 1-10 10');
      activity.append(path);
      button.append(activity);
      button.setAttribute('aria-label', conversation.title + ', ' + agentName(conversation.agentKind) + ', agent working');
      button.title = conversation.title + '\nAgent working';
    }
    if (conversationKey(conversation) === selectedId) button.setAttribute('aria-current', 'true');
    button.dataset.sessionId = conversationKey(conversation);
    const title = document.createElement('span');
    title.className = 'sidebar-session-title';
    title.textContent = conversation.title;
    button.append(title);
    button.addEventListener('click', () => { void resume(conversation); });
    row.append(button); list.append(row);
  }
  restoreFocus();
}

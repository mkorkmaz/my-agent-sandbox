export interface Project {
  id: string;
  name: string;
  path: string;
  profile: string;
  label: string;
  ownsBinding: boolean;
  ownsSession: boolean;
  policy: { name: string; owned: boolean; fingerprint: string | null } | null;
  stage: string;
  updatedAt?: number;
  lastOpenedAt?: number | null;
  gitBranch?: string | null;
}

export interface UsageSummary { input: number; cached: number; cacheWrite: number; output: number; reasoning: number; total: number; costUsd: number; unpricedRequests: number; requests: number; completedTurns: number; assumedTierRequests: number; missingCacheRequests?: number }
export interface Conversation { id: string; title: string; updatedAt: number; state?: 'active' | 'archived' | 'deleted'; available?: boolean; usage?: UsageSummary; terminalId?: number; busy?: boolean }

export interface TerminalStatus { identifier: number; projectId: string; purpose: string; running: boolean; exitCode: number | null; conversationId: string | null; busy: boolean }

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

export function branchLabel(branch: string): HTMLElement {
  const label = document.createElement('span');
  label.className = 'project-branch';
  label.title = branch;
  const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  for (const [name, value] of Object.entries({ viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', 'stroke-width': '2', 'stroke-linecap': 'round', 'stroke-linejoin': 'round', 'aria-hidden': 'true', focusable: 'false' })) icon.setAttribute(name, value);
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
  button.title = project.name + '\n' + project.path;
  button.append(...projectIdentity(project));
  button.addEventListener('click', () => { void select(project); });
  return button;
}

export function renderProjects(projects: Project[], selected: Project | null, select: (project: Project) => Promise<void>): void {
  const ordered = [...projects].sort((first, second) => (second.lastOpenedAt ?? (second.updatedAt ?? 0) * 1000) - (first.lastOpenedAt ?? (first.updatedAt ?? 0) * 1000));
  const sidebar = document.getElementById('sidebar-project-list')!;
  const list = document.getElementById('recent-project-list')!;
  sidebar.replaceChildren(); list.replaceChildren();
  for (const project of ordered) {
    const sidebarRow = document.createElement('li'); sidebarRow.append(projectButton(project, 'sidebar-project', select)); sidebar.append(sidebarRow);
    const row = document.createElement('tr'); row.title = project.path;
    const name = document.createElement('td');
    const button = projectButton({ ...project, gitBranch: null }, 'recent-project', select); name.append(button);
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
  document.getElementById('project-view')!.classList.toggle('has-project-list', selected === null && projects.length > 0);
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
}

export function renderConversations(conversations: Conversation[], selectedId: string | null, resume: (conversation: Conversation) => Promise<void>): void {
  const list = document.getElementById('sidebar-session-list')!;
  list.replaceChildren();
  for (const conversation of conversations.filter(conversation => (conversation.state ?? 'active') === 'active')) {
    const row = document.createElement('li');
    const button = document.createElement('button');
    button.className = 'sidebar-session';
    button.disabled = conversation.available === false;
    button.dataset.available = String(conversation.available !== false);
    button.title = conversation.title;
    button.setAttribute('aria-label', conversation.title);
    if (conversation.busy) {
      const activity = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      for (const [key, value] of Object.entries({ class: 'session-activity', viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', 'stroke-width': '2', 'stroke-linecap': 'round', 'aria-hidden': 'true' })) activity.setAttribute(key, value);
      const path = document.createElementNS(activity.namespaceURI, 'path');
      path.setAttribute('d', 'M12 2a10 10 0 1 1-10 10');
      activity.append(path);
      button.append(activity);
      button.setAttribute('aria-label', conversation.title + ', agent working');
      button.title = conversation.title + '\nAgent working';
    }
    if (conversation.id === selectedId) button.setAttribute('aria-current', 'true');
    button.dataset.sessionId = conversation.id;
    const title = document.createElement('span');
    title.className = 'sidebar-session-title';
    title.textContent = conversation.title;
    button.append(title);
    button.addEventListener('click', () => { void resume(conversation); });
    row.append(button); list.append(row);
  }
}

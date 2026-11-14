import { createStore } from 'zustand/vanilla';
import type { Project, TerminalStatus } from '../projects/projects';
import type { ChatgptAccountStatus } from '../providers/chatgpt-account';
import type { ClaudeAccountStatus } from '../providers/claude-account';
import type { LoginStatus } from '../providers/provider-sign-in';
import type { Plan } from '../projects/preparation';

export interface ProjectPreferences {
  project: Project;
  options: { cpus: number; memoryMb: number; verification: string; network: string };
}

interface AppData {
  selectedProject: Project | null;
  projects: Project[];
  projectsLoaded: boolean;
  terminals: TerminalStatus[];
  projectTerminals: Map<string, TerminalStatus[]>;
  chatgptAccount: ChatgptAccountStatus | null;
  claudeAccount: ClaudeAccountStatus | null;
  providerLogins: { chatgpt: LoginStatus | null; claude: LoginStatus | null };
  sidebarWidth: number;
  projectPreferences: Map<string, ProjectPreferences>;
  plan: Plan | null;
}

export const appDataStore = createStore<AppData>(() => ({
  selectedProject: null,
  projects: [],
  projectsLoaded: false,
  terminals: [],
  projectTerminals: new Map(),
  chatgptAccount: null,
  claudeAccount: null,
  providerLogins: { chatgpt: null, claude: null },
  sidebarWidth: 280,
  projectPreferences: new Map(),
  plan: null,
}));

export const appData = {
  get selectedProject(): Project | null { return appDataStore.getState().selectedProject; },
  set selectedProject(value: Project | null) { appDataStore.setState({ selectedProject: value }); },
  get projects(): Project[] { return appDataStore.getState().projects; },
  set projects(value: Project[]) { appDataStore.setState({ projects: value }); },
  get projectsLoaded(): boolean { return appDataStore.getState().projectsLoaded; },
  set projectsLoaded(value: boolean) { appDataStore.setState({ projectsLoaded: value }); },
  get terminals(): TerminalStatus[] { return appDataStore.getState().terminals; },
  set terminals(value: TerminalStatus[]) { appDataStore.setState({ terminals: value }); },
  get chatgptAccount(): ChatgptAccountStatus | null { return appDataStore.getState().chatgptAccount; },
  set chatgptAccount(value: ChatgptAccountStatus | null) { appDataStore.setState({ chatgptAccount: value }); },
  get claudeAccount(): ClaudeAccountStatus | null { return appDataStore.getState().claudeAccount; },
  set claudeAccount(value: ClaudeAccountStatus | null) { appDataStore.setState({ claudeAccount: value }); },
  get sidebarWidth(): number { return appDataStore.getState().sidebarWidth; },
  set sidebarWidth(value: number) { appDataStore.setState({ sidebarWidth: value }); },
  get plan(): Plan | null { return appDataStore.getState().plan; },
  set plan(value: Plan | null) { appDataStore.setState({ plan: value }); },
};

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { setBusy, setLoading } from '../ui/loading';
import { FormState } from '../ui/form-state';
import { renderWorkspaceStatus, setWorkspaceStatus } from '../app/workspace-status';
import { createStore } from 'zustand/vanilla';

interface AgentDefaults { profile: string; allowedHosts: string[] }

export interface Settings {
  schemaVersion: number;
  cliPath: string | null;
  codex: AgentDefaults;
  claudeCode: AgentDefaults;
  cpus: number;
  memoryMb: number;
  verification: 'require' | 'warn' | 'off';
  network: 'isolated' | 'offline';
  telemetry: boolean;
}

interface TelemetryStatus { recorded: string | null; effective: string; message: string | null }

function input(id: string): HTMLInputElement { return document.getElementById(id) as HTMLInputElement; }
function select(id: string): HTMLSelectElement { return document.getElementById(id) as HTMLSelectElement; }

export function selectSettingsTab(name: 'general' | 'agent-profile' | 'providers' | 'system-checks'): void {
  for (const tab of document.querySelectorAll<HTMLButtonElement>('#settings-tabs [role="tab"]')) {
    const selected = tab.id === 'settings-tab-' + name;
    tab.setAttribute('aria-selected', String(selected));
    tab.tabIndex = selected ? 0 : -1;
    document.getElementById(tab.getAttribute('aria-controls')!)!.hidden = !selected;
  }
  renderWorkspaceStatus();
}

export function initializeSettingsTabs(): void {
  const tabs = [...document.querySelectorAll<HTMLButtonElement>('#settings-tabs [role="tab"]')];
  for (const tab of tabs) {
    tab.addEventListener('click', () => selectSettingsTab(tab.id.replace('settings-tab-', '') as Parameters<typeof selectSettingsTab>[0]));
    tab.addEventListener('keydown', event => {
      const index = tabs.indexOf(tab);
      const destinations: Record<string, number> = { ArrowRight: (index + 1) % tabs.length, ArrowLeft: (index + tabs.length - 1) % tabs.length, Home: 0, End: tabs.length - 1 };
      const destination = destinations[event.key];
      if (destination === undefined) return;
      event.preventDefault();
      tabs[destination].click();
      tabs[destination].focus();
    });
  }
}

function renderTelemetry(status: TelemetryStatus): void {
  const names: Record<string, string> = { on: 'On', off: 'Off', unanswered: 'Not set yet' };
  document.getElementById('telemetry-status')!.textContent = 'Saved Brig preference: ' + (names[status.recorded ?? ''] ?? 'Unavailable') + '. Effective for app commands: ' + names[status.effective] + '.' + (status.message ? ' ' + status.message : '');
}

function settingsFields(settings: Settings): Record<string, string> {
  return {
    'cli-path': settings.cliPath ?? '',
    'codex-profile': settings.codex.profile,
    'claude-code-profile': settings.claudeCode.profile,
    'cpu-count': String(settings.cpus),
    'memory-mb': String(settings.memoryMb),
    'image-verification': settings.verification,
    'network-preference': settings.network,
    'allowed-hosts': settings.codex.allowedHosts.join('\n'),
    'claude-code-allowed-hosts': settings.claudeCode.allowedHosts.join('\n'),
  };
}

function renderSettings(settings: Settings, previous: Settings | null): void {
  const values = settingsFields(settings), original = previous ? settingsFields(previous) : null;
  for (const [id, value] of Object.entries(values)) {
    const field = document.getElementById(id) as HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement;
    if (original && field.value !== original[id]) continue;
    field.value = value;
  }
  const telemetry = input('telemetry-preference');
  if (!previous || telemetry.checked === previous.telemetry) telemetry.checked = settings.telemetry;
}

export async function initializeSettings(): Promise<void> {
  try {
    const version = await invoke<string>('application_version');
    document.getElementById('application-version')!.textContent = version;
    document.getElementById('sidebar-version')!.textContent = 'v.' + version;
  }
  catch { document.getElementById('application-version')!.textContent = 'Unavailable'; }
  finally { setLoading(document.getElementById('application-version')!, false); }
  const form = document.getElementById('brig-settings') as HTMLFormElement;
  const statuses = ['settings-status', 'agent-profile-status'].map(id => document.getElementById(id)!);
  const saves = ['save-settings', 'save-agent-profile'].map(id => document.getElementById(id) as HTMLButtonElement);
  const fields = ['settings-fields', 'agent-profile-fields'].map(id => document.getElementById(id) as HTMLFieldSetElement);
  function setStatus(message: string, busy = false): void {
    for (const status of statuses) { status.textContent = message; status.hidden = busy; }
    setWorkspaceStatus('settings-save', busy ? message : null, 'settings-view');
  }
  const state = new FormState<Settings>();
  const telemetryData = createStore<{ value: TelemetryStatus | null }>(() => ({ value: null }));
  function acceptTelemetry(value: TelemetryStatus): void {
    telemetryData.setState({ value });
    renderTelemetry(telemetryData.getState().value!);
  }
  let settingsReadRevision = 0;
  let pendingSettingsRead = false;
  async function refreshSettings(): Promise<void> {
    if (state.busy()) { pendingSettingsRead = true; return; }
    const revision = ++settingsReadRevision;
    try {
      const settings = await invoke<Settings>('load_settings');
      if (revision !== settingsReadRevision) return;
      if (state.busy()) { pendingSettingsRead = true; return; }
      const previous = state.current();
      if (!state.replace(settings)) return;
      renderSettings(state.current()!, previous);
      updateNetworkHint();
      for (const fieldset of fields) fieldset.disabled = false;
      const telemetry = await invoke<TelemetryStatus>('telemetry_status');
      if (revision === settingsReadRevision) acceptTelemetry(telemetry);
    } catch (error) { if (revision === settingsReadRevision) setStatus(String(error)); }
  }
  await listen<string[]>('records-changed', event => {
    if (event.payload.includes('settings')) return refreshSettings();
  });
  const initialRead = state.read();
  try {
    const settings = await invoke<Settings>('load_settings');
    state.receive(initialRead, settings);
    renderSettings(state.current()!, null);
    acceptTelemetry(await invoke<TelemetryStatus>('telemetry_status'));
  } catch (error) { state.fail(initialRead); setStatus(String(error)); }
  finally { state.finish(initialRead); for (const fieldset of fields) setLoading(fieldset, false); if (pendingSettingsRead) { pendingSettingsRead = false; void refreshSettings(); } }
  const network = select('network-preference');
  const networkHint = document.getElementById('network-hint')!;
  function updateNetworkHint(): void {
    const guest = network.value === 'offline' ? 'Offline blocks guest command traffic. Sign-in and online Codex and Claude Code sessions are unavailable.' : 'Brig/hvi allows guest command traffic only to destinations permitted by the policy.';
    networkHint.textContent = guest + ' Hosted Codex tools, including web search, have a separate scope. Tugboat does not read or set the web search mode.';
  }
  network.addEventListener('change', updateNetworkHint);
  updateNetworkHint();
  document.getElementById('settings-view')!.addEventListener('invalid', event => {
    const control = event.target as HTMLInputElement;
    if (control.form !== form) return;
    const firstInvalid = [...form.elements].find(element => (element as HTMLInputElement).willValidate && !(element as HTMLInputElement).validity.valid);
    if (control !== firstInvalid) return;
    const panel = control.closest<HTMLElement>('[role="tabpanel"]')!;
    selectSettingsTab(panel.id.replace('settings-panel-', '') as Parameters<typeof selectSettingsTab>[0]);
    control.focus();
  }, true);
  form.addEventListener('submit', async event => {
    event.preventDefault();
    const submission = state.submit();
    if (!submission) return;
    ++settingsReadRevision;
    for (const save of saves) { save.disabled = true; setBusy(save, true); }
    setStatus('Saving settings…', true);
    const settings: Settings = {
      ...submission.value,
      schemaVersion: 2,
      cliPath: input('cli-path').value.trim() || null,
      codex: {
        profile: input('codex-profile').value.trim(),
        allowedHosts: [...new Set((document.getElementById('allowed-hosts') as HTMLTextAreaElement).value.split('\n').map(value => value.trim()).filter(Boolean))],
      },
      claudeCode: {
        profile: input('claude-code-profile').value.trim(),
        allowedHosts: [...new Set((document.getElementById('claude-code-allowed-hosts') as HTMLTextAreaElement).value.split('\n').map(value => value.trim()).filter(Boolean))],
      },
      cpus: input('cpu-count').valueAsNumber,
      memoryMb: input('memory-mb').valueAsNumber,
      verification: select('image-verification').value as Settings['verification'],
      network: network.value as Settings['network'],
      telemetry: input('telemetry-preference').checked,
    };
    try {
      const telemetry = await invoke<TelemetryStatus>('save_settings', { settings });
      state.saved(submission.revision, settings);
      acceptTelemetry(telemetry);
      setStatus(telemetry.message ? 'Settings saved. The Brig telemetry preference is waiting to be applied.' : 'Settings saved.');
    } catch (error) { state.fail(submission.revision); setStatus(String(error)); }
    finally {
      state.finish(submission.revision);
      for (const save of saves) { setBusy(save, false); save.disabled = false; }
      saves.find(save => !save.closest('[hidden]'))?.focus();
      if (pendingSettingsRead) { pendingSettingsRead = false; void refreshSettings(); }
    }
  });
  for (const fieldset of fields) fieldset.disabled = state.current() === null;
}

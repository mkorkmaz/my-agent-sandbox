import { invoke } from '@tauri-apps/api/core';

export interface Settings {
  schemaVersion: number;
  cliPath: string | null;
  profile: string;
  cpus: number;
  memoryMb: number;
  verification: 'require' | 'warn' | 'off';
  network: 'isolated' | 'offline';
  allowedHosts: string[];
  telemetry: boolean;
}

interface TelemetryStatus { recorded: string | null; effective: string; message: string | null }

function input(id: string): HTMLInputElement { return document.getElementById(id) as HTMLInputElement; }
function select(id: string): HTMLSelectElement { return document.getElementById(id) as HTMLSelectElement; }

function renderTelemetry(status: TelemetryStatus): void {
  const names: Record<string, string> = { on: 'On', off: 'Off' };
  document.getElementById('telemetry-status')!.textContent = 'Saved Brig preference: ' + (names[status.recorded ?? ''] ?? 'Unavailable') + '. Effective for app commands: ' + names[status.effective] + '.' + (status.message ? ' ' + status.message : '');
}

export async function initializeSettings(): Promise<void> {
  try { document.getElementById('application-version')!.textContent = await invoke<string>('application_version'); }
  catch { document.getElementById('application-version')!.textContent = 'Unavailable'; }
  const status = document.getElementById('settings-status')!;
  const save = document.getElementById('save-settings') as HTMLButtonElement;
  const fields = document.getElementById('settings-fields') as HTMLFieldSetElement;
  let loaded = false;
  try {
    const settings = await invoke<Settings>('load_settings');
    input('cli-path').value = settings.cliPath ?? '';
    input('codex-profile').value = settings.profile;
    input('cpu-count').value = String(settings.cpus);
    input('memory-mb').value = String(settings.memoryMb);
    select('image-verification').value = settings.verification;
    select('network-preference').value = settings.network;
    (document.getElementById('allowed-hosts') as HTMLTextAreaElement).value = settings.allowedHosts.join('\n');
    input('telemetry-preference').checked = settings.telemetry;
    loaded = true;
    renderTelemetry(await invoke<TelemetryStatus>('telemetry_status'));
  } catch (error) { status.textContent = String(error); }
  const network = select('network-preference');
  const networkHint = document.getElementById('network-hint')!;
  function updateNetworkHint(): void {
    const guest = network.value === 'offline' ? 'Offline blocks guest command traffic. Sign-in and online Codex sessions are unavailable.' : 'Brig/hvi allows guest command traffic only to destinations permitted by the policy.';
    networkHint.textContent = guest + ' Hosted Codex tools, including web search, have a separate scope. Tugboat does not read or set the web search mode.';
  }
  network.addEventListener('change', updateNetworkHint);
  updateNetworkHint();
  document.getElementById('brig-settings')!.addEventListener('submit', async event => {
    event.preventDefault();
    save.disabled = true;
    status.textContent = 'Saving settings…';
    const settings: Settings = {
      schemaVersion: 1,
      cliPath: input('cli-path').value.trim() || null,
      profile: input('codex-profile').value.trim(),
      cpus: input('cpu-count').valueAsNumber,
      memoryMb: input('memory-mb').valueAsNumber,
      verification: select('image-verification').value as Settings['verification'],
      network: network.value as Settings['network'],
      allowedHosts: [...new Set((document.getElementById('allowed-hosts') as HTMLTextAreaElement).value.split('\n').map(value => value.trim()).filter(Boolean))],
      telemetry: input('telemetry-preference').checked,
    };
    try {
      const telemetry = await invoke<TelemetryStatus>('save_settings', { settings });
      renderTelemetry(telemetry);
      status.textContent = telemetry.message ? 'Settings saved. The Brig telemetry preference is waiting to be applied.' : 'Settings saved.';
    } catch (error) { status.textContent = String(error); }
    finally { save.disabled = false; save.focus(); }
  });
  fields.disabled = !loaded;
}

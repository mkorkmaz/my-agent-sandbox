import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

interface HostStatus {
  architecture: string;
  osVersion: string;
  tools: { brig: string | null; hull: string | null; homebrew: string | null; git: string | null };
  brigStatus: string;
  version: { data: { version: string } } | null;
  doctor: { data: { name: string; state: string; finding: string; fix?: string }[] } | null;
  message: string | null;
}

export async function initializeHost(): Promise<void> {
  const check = document.getElementById('check-host') as HTMLButtonElement;
  const cancel = document.getElementById('cancel-host') as HTMLButtonElement;
  const install = document.getElementById('install-brig') as HTMLButtonElement;
  const installationCancel = document.getElementById('cancel-installation') as HTMLButtonElement;
  const status = document.getElementById('host-status') as HTMLElement;
  const tools = document.getElementById('host-tools') as HTMLElement;
  const doctor = document.getElementById('host-doctor') as HTMLElement;
  const message = document.getElementById('installation-status') as HTMLElement;
  const path = document.getElementById('cli-path') as HTMLInputElement;
  let previous: { cliPath: string | null; result: HostStatus } | null = null;
  async function inspect(): Promise<void> {
    check.disabled = true;
    cancel.hidden = false;
    cancel.disabled = false;
    status.textContent = 'Checking this Mac…';
    try {
      const cliPath = path.value.trim() || null;
      const checked = await invoke<HostStatus>('inspect_host', { cliPath });
      const labels: Record<string, string> = { missing: 'Brig not found', incompatible: 'Unsupported Brig version', installed: 'Brig installed', error: 'Brig check failed', cancelled: 'Mac check cancelled', 'timed-out': 'Mac check timed out', 'invalid-response': 'Invalid Brig response' };
      const interrupted = ['cancelled', 'timed-out'].includes(checked.brigStatus);
      const retained = interrupted && previous?.cliPath === cliPath ? previous.result : null;
      if (!interrupted) previous = { cliPath, result: checked };
      const result = retained ?? checked;
      const description = [labels[result.brigStatus], result.version?.data.version, 'macOS ' + result.osVersion, result.architecture].filter(Boolean).join(' · ');
      status.textContent = retained ? labels[checked.brigStatus] + '. Showing the previous completed check: ' + description : description;
      if (result.message) status.textContent += '. ' + result.message;
      tools.replaceChildren();
      for (const [name, value] of Object.entries(result.tools)) {
        const row = document.createElement('div');
        const title = document.createElement('dt');
        const content = document.createElement('dd');
        title.textContent = name === 'homebrew' ? 'Homebrew' : name;
        content.textContent = value ?? 'Not found';
        row.append(title, content);
        tools.append(row);
      }
      doctor.replaceChildren();
      for (const finding of result.doctor?.data ?? []) {
        const row = document.createElement('li');
        row.textContent = finding.name + ' · ' + finding.finding + (finding.fix ? '. ' + finding.fix : '');
        if (finding.state === '!!') row.className = 'error-message';
        doctor.append(row);
      }
      install.hidden = result.brigStatus !== 'missing' || !result.tools.homebrew;
      document.getElementById('homebrew-help')!.hidden = !!result.tools.homebrew;
    } catch (error) {
      status.textContent = String(error);
    } finally {
      check.disabled = false;
      cancel.hidden = true;
    }
  }
  check.addEventListener('click', () => { void inspect(); });
  cancel.addEventListener('click', () => { cancel.disabled = true; status.textContent = 'Cancelling the Mac check…'; void invoke('cancel_inspection'); });
  install.addEventListener('click', async () => {
    install.disabled = true;
    installationCancel.hidden = false;
    message.textContent = 'Preparing Homebrew…';
    try {
      const progress = await invoke<{ message: string | null }>('install_brig');
      message.textContent = progress.message;
      await inspect();
    } catch (error) { message.textContent = String(error); }
    finally { install.disabled = false; installationCancel.hidden = true; }
  });
  installationCancel.addEventListener('click', () => { void invoke('cancel_installation'); });
  await listen<{ current: string | null }>('installation-progress', event => {
    const labels: Record<string, string> = { update: 'Updating Homebrew…', tap: 'Adding Brig tap…', trust: 'Trusting Brig tap…', install: 'Installing Brig…', verify: 'Verifying the installed Brig CLI…' };
    message.textContent = labels[event.payload.current ?? ''] ?? 'Finishing installation…';
  });
  await inspect();
}

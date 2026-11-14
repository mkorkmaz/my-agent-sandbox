import { invoke } from '@tauri-apps/api/core';
import { setBusy } from '../ui/loading';
import { setWorkspaceStatus } from '../app/workspace-status';
import { createStore } from 'zustand/vanilla';

type InspectionPhase = 'idle' | 'checking' | 'cancelling';
type BrigStatus = 'missing' | 'incompatible' | 'installed' | 'error' | 'cancelled' | 'timed-out' | 'invalid-response';

interface HostStatus {
  architecture: string;
  osVersion: string;
  tools: Record<string, string | null>;
  brigSource: 'bundled' | 'development' | 'custom';
  brigStatus: BrigStatus;
  version: { data: { version: string } } | null;
  doctor: { data: { name: string; state: string; finding: string; fix?: string }[] } | null;
  message: string | null;
}

export async function initializeHost(): Promise<void> {
  const check = document.getElementById('check-host') as HTMLButtonElement;
  const cancel = document.getElementById('cancel-host') as HTMLButtonElement;
  const status = document.getElementById('host-status') as HTMLElement;
  const tools = document.getElementById('host-tools') as HTMLElement;
  const doctor = document.getElementById('host-doctor') as HTMLElement;
  const path = document.getElementById('cli-path') as HTMLInputElement;
  const data = createStore<{ previous: { cliPath: string | null; result: HostStatus } | null; visible: HostStatus | null }>(() => ({ previous: null, visible: null }));
  let inspectionPhase: InspectionPhase = 'idle';
  async function inspect(): Promise<void> {
    if (inspectionPhase !== 'idle') return;
    inspectionPhase = 'checking';
    check.disabled = true;
    setBusy(check, true);
    cancel.hidden = false;
    cancel.disabled = false;
    status.textContent = 'Checking this Mac…';
    status.hidden = true;
    setWorkspaceStatus('host-check', status.textContent, 'settings-panel-system-checks');
    try {
      const cliPath = path.value.trim() || null;
      const checked = await invoke<HostStatus>('inspect_host', { cliPath });
      const labels: Record<string, string> = { missing: 'Brig not found', incompatible: 'Unsupported Brig version', installed: 'Brig installed', error: 'Brig check failed', cancelled: 'Mac check cancelled', 'timed-out': 'Mac check timed out', 'invalid-response': 'Invalid Brig response' };
      const interrupted = ['cancelled', 'timed-out'].includes(checked.brigStatus);
      const previous = data.getState().previous;
      const retained = interrupted && previous?.cliPath === cliPath ? previous.result : null;
      if (!interrupted) data.setState({ previous: { cliPath, result: checked } });
      data.setState({ visible: retained ?? checked });
      const result = data.getState().visible!;
      const sources = { bundled: 'Bundled Brig', development: 'Development Brig', custom: 'Custom Brig' };
      const description = [labels[result.brigStatus], sources[result.brigSource], result.version?.data.version, 'macOS ' + result.osVersion, result.architecture].filter(Boolean).join(' · ');
      status.textContent = retained ? labels[checked.brigStatus] + '. Showing the previous completed check: ' + description : description;
      if (result.message) status.textContent += '. ' + result.message;
      tools.replaceChildren();
      for (const [name, value] of Object.entries(result.tools)) {
        const row = document.createElement('div');
        const title = document.createElement('dt');
        const content = document.createElement('dd');
        title.textContent = name;
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
    } catch (error) {
      status.textContent = String(error);
    } finally {
      inspectionPhase = 'idle';
      setBusy(check, false);
      status.hidden = false;
      setWorkspaceStatus('host-check', null, 'settings-panel-system-checks');
      check.disabled = false;
      cancel.hidden = true;
    }
  }
  check.addEventListener('click', () => { void inspect(); });
  cancel.addEventListener('click', () => {
    if (inspectionPhase !== 'checking') return;
    inspectionPhase = 'cancelling';
    cancel.disabled = true;
    status.textContent = 'Cancelling the Mac check…';
    setWorkspaceStatus('host-check', status.textContent, 'settings-panel-system-checks');
    void invoke('cancel_inspection');
  });
  await inspect();
}

import { invoke } from '@tauri-apps/api/core';
import { sameProjectView, type ProjectContext } from './projects';
import { RequestState } from '../ui/request-state';
import { dialogOpen } from '../ui/dialogs';
import { setBusy } from '../ui/loading';

const messages = {
  empty: 'The project gateway log is empty.',
  offline: 'Brig reports Offline mode for the selected environment.',
  unavailable: 'Gateway logs are unavailable for the selected environment.',
  timeout: 'The gateway log read timed out.',
  'output-limit': 'The gateway log exceeded the size limit.',
  failed: 'Brig could not read the gateway log.',
};

type GatewayLogs = { status: 'available'; text: string } | { status: keyof typeof messages };

export function initializeGatewayLogs(current: () => (ProjectContext & { reference: string }) | null): () => void {
  const dialog = document.getElementById('project-settings-dialog')!;
  const read = document.getElementById('read-project-gateway-logs') as HTMLButtonElement;
  const copy = document.getElementById('copy-project-gateway-logs') as HTMLButtonElement;
  const view = document.getElementById('project-gateway-log')!;
  const status = document.getElementById('project-gateway-log-status')!;
  const requests = new RequestState();
  let displayed: { text: string; current: () => boolean } | null = null;

  function clearLog(): void {
    displayed = null;
    view.textContent = '';
    view.hidden = true;
    copy.hidden = true;
    copy.disabled = true;
  }

  read.addEventListener('click', async () => {
    const context = current();
    if (!context || !dialogOpen(dialog) || read.disabled) return;
    const request = requests.begin();
    const active = () => dialogOpen(dialog) && requests.accepts(request)
      && sameProjectView(current(), context) && current()?.reference === context.reference;
    clearLog();
    read.disabled = true;
    setBusy(read, true);
    status.textContent = 'Reading gateway logs…';
    try {
      const result = await invoke<GatewayLogs>('read_project_gateway_logs', {
        identifier: context.id, environmentId: context.environmentId, reference: context.reference,
      });
      if (!active()) return;
      if (result.status !== 'available') {
        status.textContent = messages[result.status];
        return;
      }
      displayed = { text: result.text, current: active };
      view.textContent = result.text;
      status.textContent = 'Showing up to 100 lines. Free-form details are hidden.';
      view.hidden = false;
      copy.hidden = false;
      copy.disabled = false;
    } catch {
      if (active()) status.textContent = messages.failed;
    } finally {
      if (active()) {
        requests.finish(request);
        read.disabled = false;
        setBusy(read, false);
      }
    }
  });

  copy.addEventListener('click', async () => {
    const log = displayed;
    if (!log?.current() || copy.disabled) return;
    copy.disabled = true;
    try {
      await invoke('copy_project_gateway_logs', { text: log.text });
      if (log.current()) status.textContent = 'Gateway log copied.';
    } catch {
      if (log.current()) status.textContent = 'Could not copy the gateway log.';
    } finally {
      if (log.current()) copy.disabled = false;
    }
  });

  return () => {
    requests.invalidate();
    clearLog();
    status.textContent = '';
    read.disabled = false;
    setBusy(read, false);
  };
}

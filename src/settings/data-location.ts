import { invoke } from '@tauri-apps/api/core';
import { setBusy, setLoading } from '../ui/loading';
import { FormState } from '../ui/form-state';
import { setWorkspaceStatus } from '../app/workspace-status';

interface StorageLocation {
  directory: string | null;
  defaultDirectory: string;
  pendingDirectory: string | null;
  available: boolean;
  canChange: boolean;
  message: string | null;
}

export async function initializeDataLocation(): Promise<void> {
  const path = document.getElementById('records-folder')!;
  const status = document.getElementById('records-folder-status')!;
  const hint = document.getElementById('records-folder-hint')!;
  const change = document.getElementById('change-records-folder') as HTMLButtonElement;
  const restore = document.getElementById('default-records-folder') as HTMLButtonElement;
  const cancel = document.getElementById('cancel-records-move') as HTMLButtonElement;
  const restart = document.getElementById('restart-for-records') as HTMLButtonElement;
  const state = new FormState<StorageLocation>();

  function render(): void {
    const location = state.current();
    const busy = state.busy();
    path.textContent = location?.directory ?? 'Unavailable';
    setLoading(path, busy && location === null);
    setBusy(status, busy && location !== null);
    change.textContent = location?.available ? 'Change folder…' : 'Locate records folder…';
    change.disabled = busy || !location?.canChange;
    restore.hidden = !location?.available || location.directory === location.defaultDirectory;
    restore.disabled = busy;
    cancel.hidden = !location?.pendingDirectory;
    cancel.disabled = busy;
    restart.hidden = !location?.pendingDirectory && location?.available !== false;
    restart.disabled = busy;
    hint.textContent = location?.available
      ? 'Choose an empty folder. Settings, project records, saved usage and checks move together when Tugboat restarts.'
      : 'Connect the drive and restart Tugboat, or locate your existing records folder. Your saved records are kept.';
    if (!busy) status.textContent = [location?.message, location?.pendingDirectory ? 'Restart Tugboat to use ' + location.pendingDirectory + '. Save any setting changes before restarting.' : ''].filter(Boolean).join(' ');
  }

  async function update(action: () => Promise<StorageLocation | null>, progress: string): Promise<void> {
    if (state.busy()) return;
    const request = state.current() === null ? state.read() : state.submit()!.revision;
    render(); status.textContent = progress; status.hidden = true;
    setWorkspaceStatus('records-folder', progress, 'settings-panel-general');
    let error: string | null = null;
    try {
      const result = await action();
      if (result) state.receive(request, result);
    } catch (caught) { state.fail(request); error = String(caught); }
    finally { state.finish(request); render(); status.hidden = false; setWorkspaceStatus('records-folder', null, 'settings-panel-general'); if (error) status.textContent = error; }
  }

  change.addEventListener('click', () => { void update(async () => {
    const folder = await invoke<string | null>('choose_storage_folder');
    if (!folder) return null;
    return invoke<StorageLocation>('change_storage_location', { folder, locate: state.current()?.available === false });
  }, 'Choosing a records folder…'); });
  restore.addEventListener('click', () => { void update(() => invoke<StorageLocation>('change_storage_location', { folder: null, locate: false }), 'Preparing the default folder…'); });
  cancel.addEventListener('click', () => { void update(() => invoke<StorageLocation>('cancel_storage_move'), 'Cancelling the folder change…'); });
  restart.addEventListener('click', () => { void update(async () => {
    await invoke('restart_for_storage_change');
    return null;
  }, 'Restarting Tugboat…'); });
  await update(() => invoke<StorageLocation>('storage_location'), 'Loading the records folder…');
}

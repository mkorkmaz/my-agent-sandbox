import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { closeDialog, showDialog } from '../ui/dialogs';

interface UpdateOffer {
  id: number;
  version: string;
  notes: string;
  sessionRevision: number;
  sessions: { kind: 'chat' | 'terminal'; label: string }[];
}

interface UpdateStatus {
  revision: number;
  phase: 'disabled' | 'idle' | 'checking' | 'current' | 'available' | 'downloading' | 'ready' | 'shuttingDown' | 'failed' | 'recovery';
  installedVersion: string;
  installedBuildNumber?: string | null;
  offer?: UpdateOffer | null;
  progress?: { downloaded: number; total?: number | null } | null;
  message?: string | null;
  automaticOffer?: boolean;
  installerUrl?: string | null;
  canReopen?: boolean;
}

const phaseMessages: Record<UpdateStatus['phase'], string> = {
  disabled: '',
  idle: '',
  checking: 'Checking for updates…',
  current: 'Tugboat is up to date.',
  available: 'A Tugboat update is available.',
  downloading: 'Downloading the update…',
  ready: 'The update is ready to install.',
  shuttingDown: 'Preparing to install the update and restart Tugboat…',
  failed: 'Could not complete the update request.',
  recovery: 'Tugboat could not restart after the update.',
};

type Action = 'app_update_check' | 'app_update_approve' | 'app_update_later' | 'app_update_reopen' | 'app_update_open_installer';

export function initializeAppUpdates(layoutChanged: () => void): { refresh: () => Promise<void> } {
  const section = document.querySelector<HTMLElement>('#app-updates')!;
  const dialog = document.querySelector<HTMLDialogElement>('#app-update-dialog')!;
  const check = section.querySelector<HTMLButtonElement>('#check-app-updates')!;
  const recheck = dialog.querySelector<HTMLButtonElement>('#recheck-app-update')!;
  const review = section.querySelector<HTMLButtonElement>('#review-app-update')!;
  const approve = dialog.querySelector<HTMLButtonElement>('#approve-app-update')!;
  const dismiss = dialog.querySelector<HTMLButtonElement>('#dismiss-app-update')!;
  const installer = dialog.querySelector<HTMLButtonElement>('#open-app-update-installer')!;
  const reopen = dialog.querySelector<HTMLButtonElement>('#reopen-app')!;
  const release = dialog.querySelector<HTMLElement>('#app-update-release')!;
  const sessions = dialog.querySelector<HTMLUListElement>('#app-update-sessions')!;
  const progress = dialog.querySelector<HTMLProgressElement>('progress')!;
  const status = dialog.querySelector<HTMLElement>('#app-update-status')!;
  const installed = dialog.querySelector<HTMLElement>('#app-update-installed')!;
  const settingsStatus = section.querySelector<HTMLElement>('#app-updates-status')!;
  let state: UpdateStatus | null = null;
  let action: Action | null = null;
  let actionError: string | null = null;
  let readRevision = 0;
  let lastAutomaticOffer = -1;

  function offerForApproval(): UpdateOffer | undefined {
    if (!state || !['available', 'ready'].includes(state.phase)) return;
    return state.offer ?? undefined;
  }

  function focusDialog(): void {
    const target = dismiss.hidden || dismiss.disabled ? status : dismiss;
    if (document.activeElement !== target) target.focus({ preventScroll: true });
  }

  function show(returnFocusId?: string): void {
    if (dialog.open) return;
    showDialog(dialog, returnFocusId);
    focusDialog();
    layoutChanged();
  }

  function render(): void {
    const focus = document.activeElement;
    const phase = state?.phase ?? 'idle';
    const offer = phase === 'recovery' ? null : state?.offer;
    const active = phase === 'shuttingDown' || phase === 'downloading';
    const candidate = offerForApproval();
    section.hidden = (!state && !actionError) || phase === 'disabled';
    check.disabled = !!action || active || phase === 'recovery';
    recheck.hidden = phase !== 'failed' || !!state?.installerUrl;
    recheck.disabled = !!action;
    review.hidden = !offer && !state?.canReopen && !state?.installerUrl && phase !== 'recovery';
    review.textContent = phase === 'recovery' ? 'Review Update Recovery…' : 'Review Update…';
    approve.hidden = !candidate;
    approve.disabled = !!action;
    approve.textContent = candidate?.sessions.length ? 'End Sessions, Update and Restart' : 'Update and Restart';
    dismiss.hidden = phase === 'shuttingDown';
    dismiss.disabled = action === 'app_update_later' || (action === 'app_update_approve' && !active) || action === 'app_update_reopen';
    dismiss.textContent = candidate && action !== 'app_update_check' ? 'Later' : phase === 'downloading' ? 'Continue Working' : 'Close';
    installer.hidden = !state?.installerUrl;
    installer.disabled = !!action;
    reopen.hidden = !state?.canReopen;
    reopen.disabled = !!action;
    release.hidden = !offer;
    if (offer) {
      dialog.querySelector('#app-update-version')!.textContent = offer.version;
      dialog.querySelector('#app-update-notes')!.textContent = offer.notes || 'This release has no release notes.';
      dialog.querySelector('#app-update-impact')!.textContent = (offer.sessions.length ? 'Tugboat will end the sessions listed below. ' : '') + 'Tugboat will stop its Brig guests and restart. Your projects and saved chats stay in place.';
    }
    sessions.hidden = !offer?.sessions.length;
    sessions.replaceChildren(...(offer?.sessions ?? []).map(session => {
      const item = document.createElement('li');
      item.textContent = (session.kind === 'chat' ? 'Chat' : 'Terminal') + ' · ' + session.label;
      return item;
    }));
    progress.hidden = phase !== 'downloading';
    if (state?.progress?.total && state.progress.total > 0) {
      progress.max = state.progress.total;
      progress.value = state.progress.downloaded;
    } else {
      progress.removeAttribute('value');
    }
    const download = phase === 'downloading' && state?.progress
      ? ' ' + Math.floor(state.progress.downloaded / (1024 * 1024)) + ' MiB downloaded.' : '';
    const recovery = phase === 'recovery' && state?.installedVersion
      ? (state.installedVersion === 'Unknown' ? 'Could not determine the installed version. ' : 'Tugboat ' + state.installedVersion + ' is installed. ') : '';
    const message = actionError ?? (action === 'app_update_check' && !active && phase !== 'checking'
      ? phaseMessages.checking : (state?.message ?? phaseMessages[phase]) + download);
    const version = state?.installedVersion;
    const buildNumber = state?.installedBuildNumber;
    installed.hidden = !version || version === 'Unknown';
    installed.textContent = installed.hidden ? '' : `Installed version ${version}${buildNumber ? ` (build ${buildNumber})` : ''}`;
    const dialogMessage = installed.hidden ? recovery + message : message;
    if (status.textContent !== dialogMessage) status.textContent = dialogMessage;
    if (settingsStatus.textContent !== recovery + message) settingsStatus.textContent = recovery + message;
    if (dialog.open && focus && dialog.contains(focus) && (focus.closest('[hidden]') || focus.matches(':disabled'))) focusDialog();
  }

  function accept(next: UpdateStatus): void {
    if (state && next.revision < state.revision) return;
    if (next.offer && state?.offer?.id === next.offer.id && next.offer.sessionRevision < state.offer.sessionRevision) return;
    const previous = state;
    const changed = !previous || next.revision > previous.revision;
    state = next;
    if (changed) actionError = null;
    render();
    if (next.automaticOffer && next.revision > lastAutomaticOffer) {
      lastAutomaticOffer = next.revision;
      show();
    }
    if (changed && (next.phase === 'ready' || next.phase === 'recovery' || (next.phase === 'failed' && previous?.phase === 'downloading'))) show();
  }

  async function refresh(): Promise<void> {
    const revision = ++readRevision;
    try {
      const next = await invoke<UpdateStatus>('app_update_status');
      if (revision === readRevision) accept(next);
    } catch {
      if (revision !== readRevision || state) return;
      actionError = 'Could not read the update status.';
      render();
    }
  }

  async function run(next: Action, args?: Record<string, number>): Promise<void> {
    if (action) return;
    action = next;
    actionError = null;
    render();
    try {
      const result = await invoke<UpdateStatus | void>(next, args);
      if (result) accept(result);
      if (next === 'app_update_later' && dialog.open && (!state?.offer || state.offer.id === args?.offerId)) closeDialog(dialog);
    } catch (error) {
      await refresh();
      actionError = String(error);
    } finally {
      action = null;
      render();
    }
  }

  function checkNow(returnFocusId?: string): void {
    if (state?.phase === 'disabled') return;
    show(returnFocusId);
    if (!check.disabled) void run('app_update_check');
  }

  function close(): void {
    if (dismiss.hidden || dismiss.disabled) return;
    const offer = offerForApproval();
    if (offer && action !== 'app_update_check') { void run('app_update_later', { offerId: offer.id }); return; }
    closeDialog(dialog);
  }

  check.addEventListener('click', () => checkNow(check.id));
  recheck.addEventListener('click', () => { void run('app_update_check'); });
  review.addEventListener('click', () => { show(review.id); void refresh(); });
  approve.addEventListener('click', () => {
    const offer = offerForApproval();
    if (offer) void run('app_update_approve', { offerId: offer.id, sessionRevision: offer.sessionRevision });
  });
  installer.addEventListener('click', () => { void run('app_update_open_installer'); });
  reopen.addEventListener('click', () => { void run('app_update_reopen'); });
  dismiss.addEventListener('click', close);
  dialog.addEventListener('cancel', event => { event.preventDefault(); close(); });
  dialog.addEventListener('close', layoutChanged);
  void Promise.all([
    listen<UpdateStatus>('app-update', event => accept(event.payload)),
    listen('app-update-menu', () => checkNow()),
    listen('terminal-sessions-changed', () => { if (dialog.open && offerForApproval()) void refresh(); }),
  ]).then(refresh).catch(() => {
    actionError = 'Could not connect to application updates.';
    render();
  });
  return { refresh };
}

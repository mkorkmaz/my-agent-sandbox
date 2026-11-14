import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { setBusy } from '../ui/loading';
import { setWorkspaceStatus } from '../app/workspace-status';
import { appDataStore } from '../app/app-data';

type Provider = 'chatgpt' | 'claude';
export interface LoginStatus {
  provider: Provider;
  revision: number;
  phase: 'idle' | 'preparing' | 'waiting' | 'exchanging' | 'saving' | 'failed';
  authorizationUrl: string | null;
  userCode: string | null;
  message: string;
}
export interface ProviderSignIn {
  active(): boolean;
  message(): string | null;
  render(signedIn: boolean, accountBusy: boolean): void;
}

export async function initializeProviderSignIn(
  provider: Provider,
  identifiers: { signIn: string; cancel: string; status: string; deviceCode?: { input: string; field: string } },
  changed: () => void,
  completed: () => Promise<void>,
): Promise<ProviderSignIn> {
  const element = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
  const signIn = element<HTMLButtonElement>(identifiers.signIn), cancel = element<HTMLButtonElement>(identifiers.cancel);
  const label = provider === 'chatgpt' ? 'ChatGPT' : 'Claude';
  let operation: 'idle' | 'starting' | 'opening' | 'cancelling' = 'idle';
  let account = { signedIn: false, busy: false };
  let error: string | null = null;
  const active = () => {
    const current = appDataStore.getState().providerLogins[provider];
    return current !== null && !['idle', 'failed'].includes(current.phase);
  };

  function render(): void {
    const current = appDataStore.getState().providerLogins[provider];
    const waiting = current?.phase === 'waiting';
    const busy = active() && !waiting || operation !== 'idle';
    signIn.textContent = waiting ? 'Return to browser' : active() ? 'Connecting ' + label + '…'
      : account.signedIn ? 'Sign in with another account' : 'Sign in with ' + label;
    signIn.disabled = account.busy || busy;
    cancel.hidden = !active() || current?.phase === 'saving';
    cancel.disabled = account.busy || operation === 'cancelling';
    const message = element(identifiers.status);
    message.textContent = error ?? current?.message ?? '';
    setBusy(message, busy && operation !== 'cancelling');
    const progress = busy && !waiting && operation !== 'cancelling' && !error ? message.textContent || 'Connecting ' + label + '…' : null;
    message.hidden = progress !== null;
    setWorkspaceStatus('provider-' + provider, progress, 'settings-panel-providers');
    if (identifiers.deviceCode) {
      const code = current?.userCode ?? '';
      element(identifiers.deviceCode.field).hidden = !waiting || code === '';
      element<HTMLInputElement>(identifiers.deviceCode.input).value = code;
    }
  }

  function accept(status: LoginStatus): void {
    const current = appDataStore.getState().providerLogins[provider];
    if (status.provider !== provider || current && status.revision < current.revision) return;
    const wasActive = active();
    appDataStore.setState(state => ({ providerLogins: { ...state.providerLogins, [provider]: status } }));
    error = null; render(); changed();
    if (wasActive && !active()) void completed();
  }

  async function run(action: 'start' | 'open' | 'cancel'): Promise<void> {
    if (operation !== 'idle' && action !== 'cancel') return;
    const restoreFocus = action === 'cancel' && document.activeElement === cancel;
    operation = action === 'start' ? 'starting' : action === 'open' ? 'opening' : 'cancelling';
    error = null; render();
    try {
      if (action === 'open') await invoke('open_provider_login', { provider });
      if (action === 'start') accept(await invoke<LoginStatus>('start_provider_login', { provider }));
      if (action === 'cancel') accept(await invoke<LoginStatus>('cancel_provider_login', { provider }));
    } catch (caught) { error = String(caught); }
    finally {
      operation = 'idle'; render(); changed();
      if (restoreFocus && !signIn.disabled && (document.activeElement === cancel || document.activeElement === document.body)) signIn.focus();
    }
  }

  signIn.addEventListener('click', () => { void run(appDataStore.getState().providerLogins[provider]?.phase === 'waiting' ? 'open' : 'start'); });
  cancel.addEventListener('click', () => { void run('cancel'); });
  if (identifiers.deviceCode) element<HTMLInputElement>(identifiers.deviceCode.input).addEventListener('focus', event => { (event.target as HTMLInputElement).select(); });
  await listen<LoginStatus>('provider-login-changed', event => { accept(event.payload); });
  try { accept(await invoke<LoginStatus>('provider_login_status', { provider })); }
  catch (caught) { error = String(caught); render(); }
  return { active, message: () => active() ? appDataStore.getState().providerLogins[provider]?.message ?? null : null,
    render: (signedIn, busy) => { account = { signedIn, busy }; render(); } };
}

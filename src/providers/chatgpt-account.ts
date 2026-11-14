import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { setBusy, setLoading } from '../ui/loading';
import { initializeProviderSignIn, type ProviderSignIn } from './provider-sign-in';
import { setWorkspaceStatus } from '../app/workspace-status';
import { appData } from '../app/app-data';

type AccountOperation = { phase: 'idle' } | { phase: 'working'; caption: string };

export interface ChatgptAccountStatus {
  generation: number;
  pendingProjectId: string | null;
  pendingEnvironmentId: string | null;
  pendingFinalizing?: boolean;
  state: 'signed-in' | 'signed-out' | 'needs-sign-in' | 'signing-in';
  email: string | null;
  message: string;
  candidates: { projectId: string; name: string; email: string | null; path?: string }[];
}

export interface ChatgptAccount {
  status(): ChatgptAccountStatus | null;
  refresh(): Promise<void>;
  updateAvailability(): void;
  accept(status: ChatgptAccountStatus): void;
  connectionFailed(message: string): void;
}

export async function initializeChatgptAccount(
  changed: (status: ChatgptAccountStatus) => void,
): Promise<ChatgptAccount> {
  const element = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
  const retry = element<HTMLButtonElement>('shared-chatgpt-retry');
  const disconnect = element<HTMLButtonElement>('shared-chatgpt-disconnect');
  const useSaved = element<HTMLButtonElement>('use-saved-chatgpt');
  const choices = element<HTMLSelectElement>('saved-chatgpt-project');
  const message = element('shared-chatgpt-status');
  let operation: AccountOperation = { phase: 'idle' };
  let request = 0, error: string | null = null;
  let candidateSignature = '';
  let providerLogin: ProviderSignIn | null = null;

  function status(): ChatgptAccountStatus | null {
    if (appData.chatgptAccount && providerLogin?.active()) return { ...appData.chatgptAccount, state: 'signing-in', message: providerLogin.message() ?? 'Connecting ChatGPT…' };
    return appData.chatgptAccount;
  }

  function render(): void {
    const pending = appData.chatgptAccount?.state === 'signing-in' || providerLogin?.active() === true;
    const busy = operation.phase === 'working';
    const progress = operation.phase === 'working' ? operation.caption : null;
    element('shared-chatgpt-section').setAttribute('aria-busy', String(busy));
    setLoading(element('shared-chatgpt-email'), appData.chatgptAccount === null && error === null);
    setBusy(message, busy || appData.chatgptAccount?.pendingFinalizing === true);
    element('shared-chatgpt-email').textContent = appData.chatgptAccount?.email ?? (appData.chatgptAccount?.state === 'signed-in' || appData.chatgptAccount?.state === 'needs-sign-in' ? 'ChatGPT account' : appData.chatgptAccount ? 'Not connected' : 'Unavailable');
    message.textContent = progress ?? error ?? (pending ? 'Connecting ChatGPT…' : appData.chatgptAccount?.message) ?? 'Could not read the shared account. Select Retry connection.';
    message.hidden = busy;
    setWorkspaceStatus('chatgpt-account', busy ? message.textContent : null, 'settings-panel-providers');
    providerLogin?.render(appData.chatgptAccount?.state === 'signed-in', busy || appData.chatgptAccount?.state === 'signing-in');
    retry.hidden = !appData.chatgptAccount?.pendingFinalizing && (appData.chatgptAccount?.state === 'needs-sign-in' || (!error && appData.chatgptAccount?.state !== 'signed-in'));
    retry.textContent = appData.chatgptAccount?.pendingFinalizing ? 'Finish connection' : error ? 'Retry connection' : 'Check connection';
    retry.disabled = busy || providerLogin?.active() === true;
    disconnect.hidden = !(appData.chatgptAccount?.state === 'signed-in' || appData.chatgptAccount?.state === 'needs-sign-in');
    disconnect.disabled = busy || pending;
    const candidates = appData.chatgptAccount?.candidates ?? [];
    const signature = JSON.stringify(candidates);
    if (signature !== candidateSignature) {
      const selected = choices.value;
      choices.replaceChildren(...candidates.map(candidate => {
        const option = document.createElement('option'); option.value = candidate.projectId;
        option.textContent = candidate.name + (candidate.email ? ' · ' + candidate.email : '');
        if (candidate.path) { option.textContent += ' · ' + candidate.path; option.title = candidate.path; }
        return option;
      }));
      if (candidates.some(candidate => candidate.projectId === selected)) choices.value = selected;
      candidateSignature = signature;
    }
    element('saved-chatgpt-accounts').hidden = candidates.length === 0 || pending;
    choices.disabled = busy;
    useSaved.disabled = busy || pending || candidates.length === 0;
  }

  function accept(status: ChatgptAccountStatus): void {
    if (appData.chatgptAccount && status.generation < appData.chatgptAccount.generation) return;
    if (!appData.chatgptAccount || status.generation > appData.chatgptAccount.generation || status.state !== appData.chatgptAccount.state) error = null;
    appData.chatgptAccount = status; render(); changed(providerLogin?.active() ? { ...status, state: 'signing-in' } : status);
  }

  async function refresh(): Promise<void> {
    const sequence = ++request;
    try {
      const status = await invoke<ChatgptAccountStatus>('shared_chatgpt_status');
      if (sequence === request) accept(status);
    } catch (caught) { if (sequence === request) { error = String(caught); render(); } }
  }

  async function run(action: () => Promise<ChatgptAccountStatus | void>, caption: string): Promise<void> {
    if (operation.phase !== 'idle') return;
    operation = { phase: 'working', caption }; error = null; render();
    let failure: string | null = null;
    try { const status = await action(); if (status) accept(status); else await refresh(); }
    catch (caught) { failure = String(caught); await refresh(); }
    finally {
      operation = { phase: 'idle' }; error = failure; render();
      if (document.activeElement === useSaved && useSaved.closest('[hidden]')) disconnect.focus();
    }
  }

  useSaved.addEventListener('click', () => { void run(() => invoke<ChatgptAccountStatus>('use_project_chatgpt', { identifier: choices.value }), 'Connecting the saved ChatGPT sign-in…'); });
  disconnect.addEventListener('click', () => { void run(() => invoke<ChatgptAccountStatus>('disconnect_shared_chatgpt'), 'Signing out of Tugboat…'); });
  retry.addEventListener('click', () => { void run(async () => appData.chatgptAccount?.state === 'signed-in' || appData.chatgptAccount?.pendingFinalizing
    ? invoke<ChatgptAccountStatus>('retry_shared_chatgpt') : refresh(), 'Checking the shared ChatGPT account…'); });
  await listen<ChatgptAccountStatus>('shared-chatgpt-changed', event => { accept(event.payload); });
  await listen<string>('shared-chatgpt-error', event => { error = event.payload; render(); });
  providerLogin = await initializeProviderSignIn('chatgpt', {
    signIn: 'shared-chatgpt-sign-in', cancel: 'shared-chatgpt-cancel', status: 'shared-chatgpt-hint',
    deviceCode: { input: 'chatgpt-device-code', field: 'chatgpt-device-code-field' },
  }, () => { render(); const account = status(); if (account) changed(account); }, refresh);
  await refresh();
  return { status, refresh, updateAvailability: render, accept,
    connectionFailed: message => { error = message; render(); } };
}

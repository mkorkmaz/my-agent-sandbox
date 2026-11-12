import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { setBusy, setLoading } from './loading';

export interface ChatgptAccountStatus {
  generation: number;
  pendingProjectId: string | null;
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
  canSignIn: () => boolean,
  signIn: (projectId: string | null) => Promise<void>,
  changed: (status: ChatgptAccountStatus) => void,
): Promise<ChatgptAccount> {
  const element = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
  const signInButton = element<HTMLButtonElement>('shared-chatgpt-sign-in');
  const retry = element<HTMLButtonElement>('shared-chatgpt-retry');
  const disconnect = element<HTMLButtonElement>('shared-chatgpt-disconnect');
  const cancel = element<HTMLButtonElement>('shared-chatgpt-cancel');
  const useSaved = element<HTMLButtonElement>('use-saved-chatgpt');
  const choices = element<HTMLSelectElement>('saved-chatgpt-project');
  const message = element('shared-chatgpt-status');
  let current: ChatgptAccountStatus | null = null;
  let busy = false, request = 0, error: string | null = null, progress: string | null = null;
  let candidateSignature = '';

  function render(): void {
    const pending = current?.state === 'signing-in';
    element('shared-chatgpt-section').setAttribute('aria-busy', String(busy));
    setLoading(element('shared-chatgpt-email'), current === null && error === null);
    setBusy(message, busy || current?.pendingFinalizing === true);
    element('shared-chatgpt-email').textContent = current?.email ?? (current?.state === 'signed-in' || current?.state === 'needs-sign-in' ? 'ChatGPT account' : current ? 'Not connected' : 'Unavailable');
    message.textContent = progress ?? error ?? current?.message ?? 'Could not read the shared account. Select Retry connection.';
    signInButton.textContent = pending ? current?.pendingFinalizing ? 'Retry connection' : 'Return to sign-in'
      : !canSignIn() ? current?.state === 'signed-in' ? 'Open a project to change account' : 'Open a project to sign in'
      : current?.state === 'signed-in' ? 'Sign in with another account' : 'Sign in with ChatGPT';
    signInButton.disabled = busy;
    retry.hidden = current?.state === 'needs-sign-in' || (!error && current?.state !== 'signed-in');
    retry.textContent = error ? 'Retry connection' : 'Check connection';
    retry.disabled = busy;
    disconnect.hidden = !(current?.state === 'signed-in' || current?.state === 'needs-sign-in');
    disconnect.disabled = busy;
    cancel.hidden = !pending || current?.pendingFinalizing === true;
    cancel.disabled = busy;
    element('shared-chatgpt-hint').textContent = pending && !current?.pendingFinalizing
      ? 'Open the official address shown in the terminal in your browser and enter the device code.'
      : !canSignIn() ? current?.state === 'signed-in'
        ? 'To change accounts, open a project and wait for its environment to be ready.'
        : 'Open a project and wait for its environment to be ready, then sign in once.' : '';
    const candidates = current?.candidates ?? [];
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
    useSaved.disabled = busy || candidates.length === 0;
  }

  function accept(status: ChatgptAccountStatus): void {
    if (current && status.generation < current.generation) return;
    if (!current || status.generation > current.generation || status.state !== current.state) error = null;
    current = status; render(); changed(status);
  }

  async function refresh(): Promise<void> {
    const sequence = ++request;
    try {
      const status = await invoke<ChatgptAccountStatus>('shared_chatgpt_status');
      if (sequence === request) accept(status);
    } catch (caught) { if (sequence === request) { error = String(caught); render(); } }
  }

  async function run(action: () => Promise<ChatgptAccountStatus | void>, caption: string): Promise<void> {
    if (busy) return;
    busy = true; progress = caption; error = null; render();
    let failure: string | null = null;
    try { const status = await action(); if (status) accept(status); else await refresh(); }
    catch (caught) { failure = String(caught); await refresh(); }
    finally {
      busy = false; progress = null; error = failure; render();
      if (document.activeElement === useSaved && useSaved.closest('[hidden]')) disconnect.focus();
    }
  }

  signInButton.addEventListener('click', () => { void run(() => current?.pendingFinalizing
    ? invoke<ChatgptAccountStatus>('retry_shared_chatgpt') : signIn(current?.pendingProjectId ?? null), 'Opening ChatGPT sign-in…'); });
  useSaved.addEventListener('click', () => { void run(() => invoke<ChatgptAccountStatus>('use_project_chatgpt', { identifier: choices.value }), 'Connecting the saved ChatGPT sign-in…'); });
  disconnect.addEventListener('click', () => { void run(() => invoke<ChatgptAccountStatus>('disconnect_shared_chatgpt'), 'Signing out of Tugboat…'); });
  cancel.addEventListener('click', () => { void run(() => invoke<ChatgptAccountStatus>('cancel_shared_chatgpt_login'), 'Cancelling ChatGPT sign-in…'); });
  retry.addEventListener('click', () => { void run(async () => current?.state === 'signed-in'
    ? invoke<ChatgptAccountStatus>('retry_shared_chatgpt') : refresh(), 'Checking the shared ChatGPT account…'); });
  await listen<ChatgptAccountStatus>('shared-chatgpt-changed', event => { accept(event.payload); });
  await listen<string>('shared-chatgpt-error', event => { error = event.payload; render(); });
  await refresh();
  return { status: () => current, refresh, updateAvailability: render, accept,
    connectionFailed: message => { error = message; render(); } };
}

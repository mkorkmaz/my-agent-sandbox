import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { setBusy, setLoading } from '../ui/loading';
import { initializeProviderSignIn, type ProviderSignIn } from './provider-sign-in';
import { setWorkspaceStatus } from '../app/workspace-status';
import { appData } from '../app/app-data';

export interface ClaudeAccountStatus {
  generation: number;
  state: 'signed-in' | 'signed-out' | 'signing-in' | 'needs-sign-in';
  email: string | null;
  message: string;
}

export interface ClaudeAccount {
  status(): ClaudeAccountStatus | null;
  select(): void;
  refresh(): Promise<void>;
}

export async function initializeClaudeAccount(changed: () => void): Promise<ClaudeAccount> {
  const element = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
  let providerLogin: ProviderSignIn | null = null;
  let operation: 'idle' | 'working' = 'idle';
  let request = 0, error: string | null = null;

  function status(): ClaudeAccountStatus | null {
    if (appData.claudeAccount && providerLogin?.active()) return { ...appData.claudeAccount, state: 'signing-in', message: providerLogin.message() ?? 'Connecting Claude…' };
    return appData.claudeAccount;
  }

  function render(): void {
    const busy = operation === 'working', pending = providerLogin?.active() === true;
    element('claude-account-section').setAttribute('aria-busy', String(busy));
    setLoading(element('claude-account-email'), appData.claudeAccount === null && error === null);
    element('claude-account-email').textContent = appData.claudeAccount?.email ?? (appData.claudeAccount?.state === 'signed-in' || appData.claudeAccount?.state === 'needs-sign-in' ? 'Claude account' : appData.claudeAccount ? 'Not connected' : 'Unavailable');
    element('claude-account-status').textContent = error ?? (busy ? 'Checking the shared Claude account…' : pending ? 'Connecting Claude…' : appData.claudeAccount?.message) ?? 'Could not read the shared account. Select Retry connection.';
    setBusy(element('claude-account-status'), busy);
    element('claude-account-status').hidden = busy;
    setWorkspaceStatus('claude-account', busy ? element('claude-account-status').textContent : null, 'settings-panel-providers');
    providerLogin?.render(appData.claudeAccount?.state === 'signed-in', busy);
    const check = element<HTMLButtonElement>('claude-check-account');
    check.textContent = error ? 'Retry connection' : 'Check connection';
    check.disabled = busy || pending;
    check.hidden = appData.claudeAccount?.state === 'needs-sign-in' || (!error && appData.claudeAccount?.state !== 'signed-in');
    const signOut = element<HTMLButtonElement>('claude-sign-out');
    signOut.hidden = appData.claudeAccount?.state !== 'signed-in' && appData.claudeAccount?.state !== 'needs-sign-in';
    signOut.disabled = busy || pending;
  }

  function accept(result: ClaudeAccountStatus): void {
    if (appData.claudeAccount && result.generation < appData.claudeAccount.generation) return;
    if (!appData.claudeAccount || result.generation > appData.claudeAccount.generation || result.state !== appData.claudeAccount.state) error = null;
    appData.claudeAccount = result; render(); changed();
  }

  async function refresh(): Promise<void> {
    const sequence = ++request;
    try { const result = await invoke<ClaudeAccountStatus>('shared_claude_status'); if (sequence === request) accept(result); }
    catch (caught) { if (sequence === request) { error = String(caught); render(); } }
  }

  async function run(command: string): Promise<void> {
    if (operation !== 'idle') return;
    operation = 'working'; error = null; render();
    let failure: string | null = null;
    try { accept(await invoke<ClaudeAccountStatus>(command)); }
    catch (caught) { failure = String(caught); await refresh(); }
    finally {
      operation = 'idle'; error = failure; render(); changed();
      if (document.activeElement === element('claude-sign-out') && element('claude-sign-out').hidden) element(command === 'disconnect_shared_claude' || element('claude-check-account').hidden ? 'claude-sign-in' : 'claude-check-account').focus();
    }
  }

  element('claude-check-account').addEventListener('click', () => { void run(error ? 'shared_claude_status' : 'retry_shared_claude'); });
  element('claude-sign-out').addEventListener('click', () => { void run('disconnect_shared_claude'); });
  await listen<ClaudeAccountStatus>('shared-claude-changed', event => { accept(event.payload); });
  await listen<string>('shared-claude-error', event => { error = event.payload; render(); });
  providerLogin = await initializeProviderSignIn('claude', {
    signIn: 'claude-sign-in', cancel: 'claude-cancel', status: 'claude-sign-in-status',
  }, () => { render(); changed(); }, refresh);
  await refresh();
  return { status, select: render, refresh };
}

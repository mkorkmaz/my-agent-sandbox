import { isStandaloneConversation, standaloneConversations, type Conversation } from './projects';
import type { Dashboard, SessionState } from './usage-types';
export type { Dashboard, SessionState } from './usage-types';

import { cachedCount, cachePercentage, cacheWrites, count, money, usageCount } from './usage-format';
export { emptyUsage, money, count } from './usage-format';
export { renderUsageDetails, usageDetailsStatus } from './usage-details';

const icons: Record<string, string> = {
  archive: '<rect x="3" y="3" width="18" height="4" rx="1"/><path d="M5 7v13h14V7M10 12h4"/>',
  trash: '<path d="M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7M14 10v7"/>',
  restore: '<path d="M3 10a9 9 0 1 1 2 8M3 4v6h6"/>',
  usage: '<path d="M4 19V9M10 19V4M16 19v-7M22 19H2"/>',
};
function actionButton(icon: string, label: string, action: () => void): HTMLButtonElement {
  const button = document.createElement('button');
  button.className = 'icon-button session-action'; button.title = label; button.setAttribute('aria-label', label);
  button.innerHTML = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + icons[icon] + '</svg>';
  button.addEventListener('click', action); return button;
}

export function usageButtonId(sessionId: string): string { return 'usage-for-' + sessionId; }

function renderSessionRows(body: HTMLElement, sessions: Conversation[], state: SessionState, resume: (conversation: Conversation) => Promise<void>, change: (conversation: Conversation, state: SessionState) => Promise<void>, details: (conversation: Conversation) => Promise<void>, busy: boolean, readOnly = false): void {
  body.replaceChildren();
  for (const conversation of sessions) {
    const row = document.createElement('tr'); row.dataset.sessionId = conversation.id;
    const titleCell = document.createElement('td');
    const title = document.createElement(readOnly ? 'span' : 'button'); title.className = 'session-title'; title.title = conversation.title; title.textContent = conversation.title;
    if (title.tagName === 'BUTTON') {
      title.setAttribute('aria-label', conversation.title); (title as HTMLButtonElement).disabled = state === 'deleted' || busy || conversation.available === false;
      title.addEventListener('click', () => { void resume(conversation); });
    }
    const date = document.createElement('span'); date.className = 'session-date'; date.textContent = new Date(conversation.updatedAt).toLocaleString('en-US', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
    if (conversation.subagentCount && !readOnly) date.textContent += ' · Includes ' + conversation.subagentCount + ' subagent' + (conversation.subagentCount === 1 ? '' : 's');
    if (readOnly) date.textContent += ' · ' + (conversation.relationStatus === 'orphan' ? 'Parent session unavailable' : conversation.relationStatus === 'invalid' ? 'Invalid session relationship' : 'Unverified subagent relationship') + ' · Recorded state: ' + conversation.state;
    if (conversation.available === false) date.textContent += ' · Session file unavailable';
    if (conversation.warning) date.textContent += ' · ' + conversation.warning;
    titleCell.append(title, date); row.append(titleCell);
    const totals = readOnly ? conversation.usage : conversation.familyUsage;
    for (const value of [usageCount(totals, 'input', true), cachedCount(totals, true), usageCount(totals, 'output', true), usageCount(totals, 'total', true), money(totals)]) {
      const cell = document.createElement('td'); cell.className = 'number-column'; cell.textContent = value; cell.title = [usageCount(totals, 'input') + ' input', cachedCount(totals) + ' cached input', cacheWrites(totals) + ' cache writes', usageCount(totals, 'output') + ' output', usageCount(totals, 'total') + ' total tokens', money(totals) + ' API equivalent'].join(' · '); row.append(cell);
    }
    const actions = document.createElement('td'); actions.className = 'session-row-actions';
    const usage = actionButton('usage', 'Usage for ' + conversation.title, () => { void details(conversation); });
    usage.id = usageButtonId(conversation.id);
    usage.setAttribute('aria-haspopup', 'dialog');
    usage.setAttribute('aria-controls', 'session-usage-dialog');
    actions.append(usage);
    if (!readOnly && state !== 'active') actions.append(actionButton('restore', 'Restore ' + conversation.title, () => { void change(conversation, 'active'); }));
    if (!readOnly && state === 'active') actions.append(actionButton('archive', 'Archive ' + conversation.title, () => { void change(conversation, 'archived'); }));
    if (!readOnly && state !== 'deleted') actions.append(actionButton('trash', 'Delete ' + conversation.title, () => { void change(conversation, 'deleted'); }));
    for (const button of actions.querySelectorAll<HTMLButtonElement>('button')) button.disabled = busy;
    row.append(actions); body.append(row);
  }
}

export function renderDashboard(dashboard: Dashboard, state: SessionState, resume: (conversation: Conversation) => Promise<void>, change: (conversation: Conversation, state: SessionState) => Promise<void>, details: (conversation: Conversation) => Promise<void>, busy = false): void {
  const usage = dashboard.usage;
  const summary = document.getElementById('project-usage')!;
  const values = [['Total tokens', usageCount(usage, 'total')], ['Input', usageCount(usage, 'input')], ['Cached input', cachedCount(usage)], ['Output', usageCount(usage, 'output')], ['Total API cost', money(usage)]];
  summary.replaceChildren(...values.map(([label, value]) => {
    const row = document.createElement('div'); const term = document.createElement('dt'); term.textContent = label;
    const description = document.createElement('dd');
    description.textContent = value;
    if (label === 'Cached input' && !usage.missingCachedRequests && !usage.unmeasuredTurns) {
      description.textContent = count(usage.cached) + ' ';
      const percentage = document.createElement('small'); percentage.textContent = '(' + cachePercentage(usage.cached, usage.input) + ')'; description.append(percentage);
    }
    row.append(term, description); return row;
  }));
  const notes = ['API equivalent in USD', 'OpenAI rates from ' + dashboard.pricingDate, 'Includes archived and deleted sessions'];
  notes.push('Cache writes ' + cacheWrites(usage));
  if (usage.missingCachedRequests) notes.push('Some cache-read counts are unavailable');
  if (usage.unmeasuredTurns) notes.push(count(usage.unmeasuredTurns) + ' agent turn(s) without request measurements');
  if (usage.unpricedRequests) notes.push(count(usage.unpricedRequests) + ' request(s) unpriced');
  if (usage.assumedTierRequests) notes.push('Standard rates assumed where service tier is not recorded');
  notes.push('ChatGPT subscription billing is separate.');
  document.getElementById('usage-note')!.textContent = notes.join(' · ');
  const body = document.getElementById('session-table-body')!;
  const unlinkedBody = document.getElementById('unlinked-subagent-body')!;
  const sessions = standaloneConversations(dashboard.conversations, state);
  const focus = document.activeElement as HTMLElement | null;
  const focusedId = document.hasFocus() && (body.contains(focus) || unlinkedBody.contains(focus)) ? focus?.closest<HTMLElement>('[data-session-id]')?.dataset.sessionId : undefined;
  const focusedLabel = focus?.getAttribute('aria-label');
  renderSessionRows(body, sessions, state, resume, change, details, busy);
  const unlinked = dashboard.conversations.filter(conversation => !isStandaloneConversation(conversation) && conversation.relationStatus !== 'resolved');
  document.getElementById('unlinked-subagents')!.hidden = unlinked.length === 0;
  renderSessionRows(unlinkedBody, unlinked, state, resume, change, details, busy, true);
  const status = document.getElementById('dashboard-session-status')!;
  const warning = dashboard.warnings?.join(' ');
  status.textContent = warning || (sessions.length ? '' : state === 'active' ? 'No sessions yet. Start a New Chat to begin.' : state === 'archived' ? 'No archived sessions. Archive a session to keep it out of the main list.' : 'Trash is empty. Deleted sessions can be restored here.');
  status.hidden = sessions.length > 0 && !warning;
  document.getElementById('session-table')!.hidden = sessions.length === 0;
  for (const tab of document.querySelectorAll<HTMLButtonElement>('[data-session-state]')) tab.setAttribute('aria-pressed', String(tab.dataset.sessionState === state));
  if (focusedId && focusedLabel) {
    const target = [...body.querySelectorAll<HTMLButtonElement>('button'), ...unlinkedBody.querySelectorAll<HTMLButtonElement>('button')].find(button => button.closest<HTMLElement>('[data-session-id]')?.dataset.sessionId === focusedId && button.getAttribute('aria-label') === focusedLabel);
    target?.focus({ preventScroll: true });
  }
}

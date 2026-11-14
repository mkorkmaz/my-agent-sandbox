import { agentName, conversationKey, isStandaloneConversation, standaloneConversations, type Conversation } from '../projects/projects';
import type { Dashboard as RecordedDashboard, SessionState } from '../usage/usage-types';
export type { SessionState } from '../usage/usage-types';
export type Dashboard = RecordedDashboard & { sessionAvailability?: 'unverified'; availabilityVerified?: boolean };
export const savedSessionsStatus = 'Showing saved sessions while checking for updates.';

import { cachedCount, cachePercentage, cacheWrites, count, money, usageCount } from '../usage/usage-format';
export { emptyUsage } from '../usage/usage-format';
export { renderUsageDetails, usageDetailsStatus } from '../usage/usage-details';

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

function renderSessionRows(body: HTMLElement, sessions: Conversation[], state: SessionState, resume: (conversation: Conversation) => Promise<void>, change: (conversation: Conversation, state: SessionState) => Promise<void>, details: (conversation: Conversation) => Promise<void>, busy: boolean, readOnly = false, unverified = false): void {
  body.replaceChildren();
  for (const conversation of sessions) {
    const row = document.createElement('tr'); row.dataset.sessionId = conversationKey(conversation);
    const titleCell = document.createElement('td');
    const title = document.createElement(readOnly ? 'span' : 'button'); title.className = 'session-title'; title.title = conversation.title; title.textContent = conversation.title;
    if (title.tagName === 'BUTTON') {
      title.setAttribute('aria-label', conversation.title + (conversation.agentKind ? ', ' + agentName(conversation.agentKind) : '')); (title as HTMLButtonElement).disabled = state === 'deleted' || busy || conversation.available === false;
      title.addEventListener('click', () => { void resume(conversation); });
    }
    const date = document.createElement('span'); date.className = 'session-date';
    const agent = document.createElement('span'); agent.className = 'session-agent-tag'; agent.textContent = agentName(conversation.agentKind);
    let description = new Date(conversation.updatedAt).toLocaleString('en-US', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
    if (conversation.subagentCount && !readOnly) description += ' · Includes ' + conversation.subagentCount + ' subagent' + (conversation.subagentCount === 1 ? '' : 's');
    if (readOnly) description += ' · ' + (conversation.relationStatus === 'orphan' ? 'Parent session unavailable' : conversation.relationStatus === 'invalid' ? 'Invalid session relationship' : 'Unverified subagent relationship') + ' · Recorded state: ' + conversation.state;
    if (conversation.available === false) description += unverified ? ' · Session availability not verified' : ' · Session file unavailable';
    if (conversation.warning) description += ' · ' + conversation.warning;
    date.append(agent, document.createTextNode(' ' + description));
    titleCell.append(title, date); row.append(titleCell);
    const totals = readOnly ? conversation.usage : conversation.familyUsage;
    for (const value of conversation.usageAvailable === false ? ['—', '—', '—', '—', 'Usage unavailable'] : [usageCount(totals, 'input', true), cachedCount(totals, true), usageCount(totals, 'output', true), usageCount(totals, 'total', true), money(totals)]) {
      const cell = document.createElement('td'); cell.className = 'number-column'; cell.textContent = value; cell.title = conversation.usageAvailable === false ? 'No token measurements are available for this chat.' : [usageCount(totals, 'input') + ' input', cachedCount(totals) + ' cached input', cacheWrites(totals) + ' cache writes', usageCount(totals, 'output') + ' output', usageCount(totals, 'total') + ' total tokens', money(totals) + ' API equivalent', 'Recorded source coverage: ' + conversation.coverage].join(' · '); row.append(cell);
    }
    const actionTarget = conversation.title + (conversation.agentKind ? ', ' + agentName(conversation.agentKind) : '');
    const actions = document.createElement('td'); actions.className = 'session-row-actions';
    const usage = actionButton('usage', 'Usage for ' + actionTarget, () => { void details(conversation); });
    usage.id = usageButtonId(conversationKey(conversation));
    usage.setAttribute('aria-haspopup', 'dialog');
    usage.setAttribute('aria-controls', 'session-usage-dialog');
    if (conversation.usageAvailable !== false) actions.append(usage);
    if (!readOnly && state !== 'active') actions.append(actionButton('restore', 'Restore ' + actionTarget, () => { void change(conversation, 'active'); }));
    if (!readOnly && state === 'active') actions.append(actionButton('archive', 'Archive ' + actionTarget, () => { void change(conversation, 'archived'); }));
    if (!readOnly && state !== 'deleted') actions.append(actionButton('trash', 'Delete ' + actionTarget, () => { void change(conversation, 'deleted'); }));
    for (const button of actions.querySelectorAll<HTMLButtonElement>('button')) button.disabled = busy;
    row.append(actions); body.append(row);
  }
}

export function renderDashboard(dashboard: Dashboard, state: SessionState, resume: (conversation: Conversation) => Promise<void>, change: (conversation: Conversation, state: SessionState) => Promise<void>, details: (conversation: Conversation) => Promise<void>, busy = false): void {
  const usage = dashboard.usage;
  const descriptions = document.getElementById('project-usage')!.querySelectorAll('dd');
  const values = [usageCount(usage, 'total'), usageCount(usage, 'input'), cachedCount(usage), usageCount(usage, 'output'), money(usage)];
  values.forEach((value, index) => {
    const description = descriptions[index];
    description.removeAttribute('aria-hidden');
    description.textContent = dashboard.usageAvailable === false ? (usage.requests > 0 ? value + ' (partial)' : 'Usage unavailable') : value;
    if (dashboard.usageAvailable !== false && index === 2 && !usage.missingCachedRequests && !usage.unmeasuredTurns) {
      description.textContent = count(usage.cached) + ' ';
      const percentage = document.createElement('small'); percentage.textContent = '(' + cachePercentage(usage.cached, usage.input) + ')'; description.append(percentage);
    }
  });
  const hasClaude = dashboard.conversations.some(conversation => conversation.agentKind === 'claude-code');
  const hasCodex = dashboard.conversations.some(conversation => conversation.agentKind !== 'claude-code');
  const notes = ['Token API equivalent in USD', 'Includes archived and deleted sessions'];
  if (hasCodex) notes.push('OpenAI catalogue checked ' + dashboard.pricingDate);
  if (hasClaude) notes.push(dashboard.anthropicPricingDate ? 'Anthropic catalogue checked ' + dashboard.anthropicPricingDate : 'See request details for dated Anthropic rates');
  notes.push('Each request retains its dated rates');
  notes.push('Cache writes ' + cacheWrites(usage));
  if (usage.missingCachedRequests) notes.push('Some cache-read counts are unavailable');
  if (usage.unmeasuredTurns) notes.push(count(usage.unmeasuredTurns) + ' agent turn(s) without request measurements');
  if (usage.unpricedRequests) notes.push(count(usage.unpricedRequests) + ' request(s) unpriced');
  if (usage.assumedTierRequests) notes.push('Standard rates assumed where service tier is not recorded');
  notes.push(hasClaude ? 'Subscription charges, account discounts, and server-tool charges are separate.' : 'ChatGPT subscription billing is separate.');
  if (dashboard.usageAvailable === false) notes.push('Some chats have no token measurements. Totals include recorded usage only.');
  if (hasClaude) notes.push('Claude totals include recorded main-session and discovered subagent messages. Missing transcripts, unverified relationships, and unrecorded work limit coverage. Missing billing metadata uses labelled standard/global estimates and 5-minute cache writes. Invalid or unsupported prices stay unpriced.');
  if (dashboard.coverage !== 'complete') notes.push('Recorded source coverage: ' + dashboard.coverage + '.');
  document.getElementById('usage-note')!.textContent = notes.join(' · ');
  const body = document.getElementById('session-table-body')!;
  const unlinkedBody = document.getElementById('unlinked-subagent-body')!;
  const sessions = standaloneConversations(dashboard.conversations, state);
  const focus = document.activeElement as HTMLElement | null;
  const focusedId = document.hasFocus() && (body.contains(focus) || unlinkedBody.contains(focus)) ? focus?.closest<HTMLElement>('[data-session-id]')?.dataset.sessionId : undefined;
  const focusedLabel = focus?.getAttribute('aria-label');
  const unverified = dashboard.sessionAvailability === 'unverified';
  renderSessionRows(body, sessions, state, resume, change, details, busy, false, unverified);
  const unlinked = dashboard.conversations.filter(conversation => !isStandaloneConversation(conversation) && conversation.relationStatus !== 'resolved');
  document.getElementById('unlinked-subagents')!.hidden = unlinked.length === 0;
  renderSessionRows(unlinkedBody, unlinked, state, resume, change, details, busy, true, unverified);
  const status = document.getElementById('dashboard-session-status')!;
  const warning = dashboard.warnings?.filter(message => !unverified || message !== savedSessionsStatus).join(' ');
  status.textContent = warning || (sessions.length || unverified ? '' : state === 'active' ? 'No sessions yet. Start a New Chat to begin.' : state === 'archived' ? 'No archived sessions. Archive a session to keep it out of the main list.' : 'Trash is empty. Deleted sessions can be restored here.');
  status.hidden = (sessions.length > 0 || unverified) && !warning;
  document.getElementById('session-table')!.hidden = sessions.length === 0;
  for (const tab of document.querySelectorAll<HTMLButtonElement>('[data-session-state]')) tab.setAttribute('aria-pressed', String(tab.dataset.sessionState === state));
  if (focusedId && focusedLabel) {
    const target = [...body.querySelectorAll<HTMLButtonElement>('button'), ...unlinkedBody.querySelectorAll<HTMLButtonElement>('button')].find(button => button.closest<HTMLElement>('[data-session-id]')?.dataset.sessionId === focusedId && button.getAttribute('aria-label') === focusedLabel);
    target?.focus({ preventScroll: true });
  }
}

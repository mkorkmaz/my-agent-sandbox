import type { Conversation, UsageSummary } from './projects';

export interface Dashboard { conversations: Conversation[]; usage: UsageSummary; pricingDate: string }
export type SessionState = 'active' | 'archived' | 'deleted';
export const emptyUsage: UsageSummary = { input: 0, cached: 0, cacheWrite: 0, output: 0, reasoning: 0, total: 0, costUsd: 0, unpricedRequests: 0, requests: 0, completedTurns: 0, assumedTierRequests: 0 };
export function count(value: number): string { return new Intl.NumberFormat('en-US').format(value); }
function compactCount(value: number): string { return value >= 10000 ? new Intl.NumberFormat('en-US', { notation: 'compact', maximumFractionDigits: 1 }).format(value) : count(value); }
function cachePercentage(cached: number, input: number): string {
  return new Intl.NumberFormat('en-US', { style: 'percent', maximumFractionDigits: 1 }).format(input > 0 ? cached / input : 0);
}
function cachedCount(usage: UsageSummary, compact = false): string {
  const value = compact ? compactCount(usage.cached) : count(usage.cached);
  if (usage.missingCacheRequests) return usage.missingCacheRequests === usage.requests ? 'Unknown' : value + ' + unknown';
  return value + ' (' + cachePercentage(usage.cached, usage.input) + ')';
}
export function money(usage: UsageSummary): string {
  const amount = new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', minimumFractionDigits: 2, maximumFractionDigits: 4 }).format(usage.costUsd);
  if (usage.unpricedRequests && usage.requests === usage.unpricedRequests) return 'Unavailable';
  return amount + (usage.unpricedRequests ? ' + unpriced' : '');
}

const icons: Record<string, string> = {
  archive: '<rect x="3" y="3" width="18" height="4" rx="1"/><path d="M5 7v13h14V7M10 12h4"/>',
  trash: '<path d="M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7M14 10v7"/>',
  restore: '<path d="M3 10a9 9 0 1 1 2 8M3 4v6h6"/>',
  usage: '<path d="M4 19V9M10 19V4M16 19v-7M22 19H2"/>',
};
function actionButton(icon: string, label: string, action: () => void): HTMLButtonElement {
  const button = document.createElement('button');
  button.className = 'icon-button session-action'; button.title = label; button.setAttribute('aria-label', label);
  button.innerHTML = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + icons[icon] + '</svg>';
  button.addEventListener('click', action); return button;
}

export function renderDashboard(dashboard: Dashboard, state: SessionState, resume: (conversation: Conversation) => Promise<void>, change: (conversation: Conversation, state: SessionState) => Promise<void>, details: (conversation: Conversation) => Promise<void>, busy = false): void {
  const usage = dashboard.usage;
  const summary = document.getElementById('project-usage')!;
  const values = [['Total tokens', count(usage.total)], ['Input', count(usage.input)], ['Cached input', cachedCount(usage)], ['Output', count(usage.output)], ['Total API cost', money(usage)]];
  summary.replaceChildren(...values.map(([label, value]) => {
    const row = document.createElement('div'); const term = document.createElement('dt'); term.textContent = label;
    const description = document.createElement('dd');
    description.textContent = value;
    if (label === 'Cached input' && !usage.missingCacheRequests) {
      description.textContent = count(usage.cached) + ' ';
      const percentage = document.createElement('small'); percentage.textContent = '(' + cachePercentage(usage.cached, usage.input) + ')'; description.append(percentage);
    }
    row.append(term, description); return row;
  }));
  const notes = ['API equivalent in USD', 'OpenAI rates from ' + dashboard.pricingDate, 'Includes archived and deleted sessions'];
  if (usage.missingCacheRequests) notes.push('Some cache counts are unavailable');
  if (usage.unpricedRequests) notes.push(count(usage.unpricedRequests) + ' request(s) unpriced');
  if (usage.assumedTierRequests) notes.push('Standard rates assumed where service tier is not recorded');
  notes.push('ChatGPT subscription billing is separate.');
  document.getElementById('usage-note')!.textContent = notes.join(' · ');
  const body = document.getElementById('session-table-body')!;
  const sessions = dashboard.conversations.filter(conversation => (conversation.state ?? 'active') === state);
  const focus = document.activeElement as HTMLElement | null;
  const focusedId = focus?.closest<HTMLElement>('[data-session-id]')?.dataset.sessionId;
  const focusedLabel = focus?.getAttribute('aria-label');
  body.replaceChildren();
  for (const conversation of sessions) {
    const row = document.createElement('tr'); row.dataset.sessionId = conversation.id;
    const titleCell = document.createElement('td');
    const title = document.createElement('button'); title.className = 'session-title'; title.title = conversation.title; title.textContent = conversation.title;
    title.setAttribute('aria-label', conversation.title); title.disabled = state === 'deleted' || busy || conversation.available === false;
    title.addEventListener('click', () => { void resume(conversation); });
    const date = document.createElement('span'); date.className = 'session-date'; date.textContent = new Date(conversation.updatedAt).toLocaleString('en-US', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
    if (conversation.available === false) date.textContent += ' · Session file unavailable';
    titleCell.append(title, date); row.append(titleCell);
    const totals = conversation.usage ?? emptyUsage;
    for (const value of [compactCount(totals.input), cachedCount(totals, true), compactCount(totals.output), compactCount(totals.total), money(totals)]) {
      const cell = document.createElement('td'); cell.className = 'number-column'; cell.textContent = value; cell.title = [count(totals.input) + ' input', cachedCount(totals) + ' cached input', count(totals.output) + ' output', count(totals.total) + ' total tokens', money(totals) + ' API equivalent'].join(' · '); row.append(cell);
    }
    const actions = document.createElement('td'); actions.className = 'session-row-actions';
    actions.append(actionButton('usage', 'Usage for ' + conversation.title, () => { void details(conversation); }));
    if (state !== 'active') actions.append(actionButton('restore', 'Restore ' + conversation.title, () => { void change(conversation, 'active'); }));
    if (state === 'active') actions.append(actionButton('archive', 'Archive ' + conversation.title, () => { void change(conversation, 'archived'); }));
    if (state !== 'deleted') actions.append(actionButton('trash', 'Delete ' + conversation.title, () => { void change(conversation, 'deleted'); }));
    for (const button of actions.querySelectorAll<HTMLButtonElement>('button')) button.disabled = busy;
    row.append(actions); body.append(row);
  }
  const status = document.getElementById('dashboard-session-status')!;
  status.textContent = sessions.length ? '' : state === 'active' ? 'No sessions yet. Start a New Chat to begin.' : state === 'archived' ? 'No archived sessions. Archive a session to keep it out of the main list.' : 'Trash is empty. Deleted sessions can be restored here.';
  status.hidden = sessions.length > 0;
  document.getElementById('session-table')!.hidden = sessions.length === 0;
  for (const tab of document.querySelectorAll<HTMLButtonElement>('[data-session-state]')) tab.setAttribute('aria-pressed', String(tab.dataset.sessionState === state));
  if (focusedId && focusedLabel) {
    const target = [...body.querySelectorAll<HTMLButtonElement>('button')].find(button => button.closest<HTMLElement>('[data-session-id]')?.dataset.sessionId === focusedId && button.getAttribute('aria-label') === focusedLabel);
    target?.focus({ preventScroll: true });
  }
}

interface RequestUsage { at: string; model: string | null; tokens: { input: number; cached: number; cacheWrite: number; output: number; cacheKnown?: boolean }; costUsd: number | null; unpricedReason: string | null; rates: { date: string; tier: string; tierAssumed: boolean } | null }
export interface TurnUsage { id: string; status: string; requests: RequestUsage[] }

function appendDetail(cell: HTMLTableCellElement, text: string): void {
  const detail = document.createElement('small');
  detail.textContent = text;
  cell.append(detail);
}

function appendRequestRow(body: HTMLTableSectionElement, request: RequestUsage): void {
  const row = body.insertRow();
  const model = document.createElement('th');
  model.scope = 'row';
  model.textContent = request.model ?? 'Unknown model';
  row.append(model);

  const tokens = request.tokens;
  const cached = tokens.cacheKnown === false ? 'Unknown' : count(tokens.cached) + ' (' + cachePercentage(tokens.cached, tokens.input) + ')';
  for (const value of [count(tokens.input), cached, count(tokens.cacheWrite), count(tokens.output)]) {
    row.insertCell().textContent = value;
  }
  const cost = row.insertCell();
  cost.textContent = request.costUsd === null ? 'Unpriced' : '$' + request.costUsd.toFixed(6);
  if (request.costUsd === null && request.unpricedReason) appendDetail(cost, request.unpricedReason);
  if (request.rates) {
    appendDetail(model, request.rates.tier + ' tier' + (request.rates.tierAssumed ? ' (assumed)' : ''));
    const rateDate = document.createElement('time');
    rateDate.dateTime = request.rates.date;
    rateDate.textContent = 'Rates ' + request.rates.date;
    cost.append(rateDate);
  }
}

export function renderUsageDetails(turns: TurnUsage[]): void {
  const content = document.getElementById('session-usage-details')!;
  const template = document.querySelector<HTMLTemplateElement>('#prompt-usage-template')!;
  const report = document.createDocumentFragment();
  for (const [index, turn] of turns.entries()) {
    if (!turn.requests.length) continue;
    const section = document.importNode(template.content, true);
    const heading = section.querySelector('h3')!;
    heading.id = 'usage-prompt-' + (index + 1);
    heading.textContent = 'Prompt ' + (index + 1) + ' · ' + turn.status;
    const table = section.querySelector('table')!;
    table.setAttribute('aria-labelledby', heading.id);
    section.querySelector('.table-scroll')!.setAttribute('aria-labelledby', heading.id);

    const usage = { ...emptyUsage, requests: turn.requests.length };
    for (const request of turn.requests) {
      appendRequestRow(table.tBodies[0], request);
      if (request.costUsd === null) usage.unpricedRequests++;
      else usage.costUsd += request.costUsd;
    }
    table.tFoot!.querySelector('td')!.textContent = money(usage);
    report.append(section);
  }
  content.replaceChildren(report);
}

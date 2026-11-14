import type { AgentUsageDetails, ConversationUsageDetails, PromptUsage, RequestRef, RequestUsage, TurnRef, TurnUsage, UsageSummary } from './usage-types';
import { cachedCount, cachePercentage, cacheWrites, count, emptyUsage, money, summaryText, usageCount } from './usage-format';

function identity(kind: string, ...parts: string[]): string { return 'usage-' + kind + '-' + parts.map(part => encodeURIComponent(part)).join(':'); }
function requestIdentity(ref: RequestRef): string { return identity('request', ref.sessionId, ref.turnId, ref.requestKey); }
function turnIdentity(ref: TurnRef): string { return identity('turn', ref.sessionId, ref.turnId); }
function agentName(agent: AgentUsageDetails): string {
  return agent.title + (agent.lineage.nickname && agent.lineage.nickname !== agent.title ? ' · ' + agent.lineage.nickname : '') + (agent.lineage.agentPath ? ' · ' + agent.lineage.agentPath : '');
}
function note(text: string, className = 'support-text'): HTMLParagraphElement {
  const paragraph = document.createElement('p'); paragraph.className = className; paragraph.textContent = text; return paragraph;
}
function appendDetail(cell: HTMLTableCellElement, text: string): void {
  const detail = document.createElement('small'); detail.textContent = text; cell.append(detail);
}
function oldCost(value: number | null): string { return value === null ? 'unpriced' : '$' + value.toFixed(6); }

function appendRequestRow(body: HTMLTableSectionElement, request: RequestUsage, sessionId: string, turnId: string): void {
  const row = body.insertRow();
  row.id = requestIdentity({ sessionId, turnId, requestKey: request.key }); row.tabIndex = -1; row.dataset.usageFocusKey = row.id;
  const model = document.createElement('th'); model.scope = 'row'; model.textContent = request.model ?? 'Unknown model'; row.append(model);
  const tokens = request.tokens;
  const cached = (tokens.cachedKnown ?? tokens.cacheKnown) === false ? 'Unknown' : count(tokens.cached) + ' (' + cachePercentage(tokens.cached, tokens.input) + ')';
  const writes = (tokens.cacheWriteKnown ?? tokens.cacheKnown) === false ? 'Unknown' : count(tokens.cacheWrite);
  for (const value of [count(tokens.input), cached, writes, count(tokens.output)]) row.insertCell().textContent = value;
  if (request.anthropic?.cacheCreation) {
    const split = request.anthropic.cacheCreation;
    appendDetail(row.cells[3], count(split.ephemeral5mInputTokens) + ' at 5 minutes · ' + count(split.ephemeral1hInputTokens) + ' at 1 hour');
  }
  const cost = row.insertCell();
  const anthropic = request.rates?.anthropic;
  cost.textContent = request.costUsd === null ? 'Unpriced' : '$' + request.costUsd.toFixed(6) + (anthropic?.assumptions.length ? ' (estimate)' : '') + (request.partial ? ' (partial)' : '');
  if (request.costUsd === null && request.unpricedReason) appendDetail(cost, request.unpricedReason);
  if (request.rates) {
    appendDetail(model, request.rates.tier + ' tier' + (request.rates.tierAssumed ? ' (assumed)' : ''));
    const rateDate = document.createElement('time'); rateDate.dateTime = request.rates.date; rateDate.textContent = 'Rates ' + request.rates.date; rateDate.title = request.rates.source; cost.append(rateDate);
    if (anthropic) {
      appendDetail(model, anthropic.speed + ' speed · ' + anthropic.inferenceGeo + ' inference');
      for (const assumption of anthropic.assumptions) appendDetail(cost, assumption);
      const verifiedSource = request.rates.source === 'https://platform.claude.com/docs/en/about-claude/pricing';
      if (verifiedSource) {
        const source = document.createElement('a'); source.href = request.rates.source; source.target = '_blank'; source.rel = 'noopener noreferrer'; source.textContent = 'Anthropic rates ' + request.rates.date; source.title = 'Anthropic pricing source'; rateDate.replaceChildren(source);
      }
      if (!verifiedSource) appendDetail(cost, 'Pricing source: ' + request.rates.source);
    }
    if (request.rates.longContext) appendDetail(cost, anthropic ? 'Recorded rates for prompts over 100,000 input tokens' : 'Recorded long-context rates');
  }
  if (request.serviceTier) appendDetail(model, 'Recorded service tier: ' + request.serviceTier + (request.tierSource ? ' · ' + request.tierSource : ''));
  if (request.rateCorrection) appendDetail(cost, 'Rate correction: ' + request.rateCorrection.reason + ' Previous ' + request.rateCorrection.previousRates.tier + ' rates ' + request.rateCorrection.previousRates.date + ': ' + oldCost(request.rateCorrection.previousCostUsd) + '. Original rate date retained.');
  if (request.amountCorrection) {
    const previous = request.amountCorrection;
    appendDetail(cost, 'Amount correction: ' + previous.reason + ' Previous ' + count(previous.previousTokens.input) + ' input, ' + count(previous.previousTokens.output) + ' output; ' + oldCost(previous.previousCostUsd) + (previous.previousPartial ? ' (partial)' : '') + (previous.previousUnpricedReason ? ' · ' + previous.previousUnpricedReason : '') + '.');
  }
  for (const correction of request.anthropicCorrections ?? []) appendDetail(cost, 'Billing metadata correction: ' + correction.reason + ' Previous model: ' + (correction.previousModel ?? 'unknown') + '; ' + oldCost(correction.previousCostUsd) + (correction.previousRates ? '; rates ' + correction.previousRates.date : '') + '.');
  if (request.source) {
    const source = request.source;
    model.title = [source.guestNamespace, source.modelProvider, source.originThreadId, source.originTurnId ?? 'Source turn unassigned', source.rootTurnId ?? 'Root prompt unassigned', source.responseId, request.key].join('\n');
  }
}

function requestLink(ref: RequestRef | TurnRef, agents: AgentUsageDetails[]): HTMLAnchorElement {
  const isRequest = 'requestKey' in ref;
  const id = isRequest ? requestIdentity(ref) : turnIdentity(ref);
  const link = document.createElement('a'); link.href = '#' + encodeURIComponent(id); link.dataset.usageFocusKey = identity('source', id);
  const agent = agents.find(agent => agent.sessionId === ref.sessionId)!;
  const turnIndex = agent.turns.findIndex(turn => turn.id === ref.turnId);
  const requestIndex = isRequest ? agent.turns[turnIndex].requests.findIndex(request => request.key === ref.requestKey) : -1;
  link.textContent = (agent.lineage.kind === 'primary' ? 'Main agent' : agentName(agent)) + ' · agent turn ' + (turnIndex + 1) + (isRequest ? ' · request ' + (requestIndex + 1) : ' · unmeasured');
  link.title = ref.turnId + (isRequest ? '\n' + ref.requestKey : ' · unmeasured turn') + '\n' + agentName(agent);
  link.addEventListener('click', event => {
    const target = document.getElementById(id); if (!target) return;
    event.preventDefault();
    let disclosure = target.closest('details');
    while (disclosure) { disclosure.open = true; disclosure = disclosure.parentElement?.closest('details') ?? null; }
    target.focus({ preventScroll: true }); target.scrollIntoView?.({ block: 'nearest' });
  });
  return link;
}
function appendReferences(section: HTMLElement, refs: RequestRef[], turns: TurnRef[], agents: AgentUsageDetails[]): void {
  if (!refs.length && !turns.length) return;
  section.append(note('Source references', 'support-text usage-source-heading'));
  const list = document.createElement('ul'); list.className = 'usage-source-list';
  for (const ref of [...refs, ...turns]) { const item = document.createElement('li'); item.append(requestLink(ref, agents)); list.append(item); }
  section.append(list);
}
function appendPromptAttribution(section: HTMLElement, prompt: PromptUsage, details: ConversationUsageDetails): void {
  if (details.agents.length > 1) {
    section.append(note('Subagents: ' + summaryText(prompt.descendantsUsage), 'support-text prompt-attribution'));
    section.append(note('Combined: ' + summaryText(prompt.combinedUsage), 'support-text prompt-attribution'));
  }
  appendReferences(section, prompt.requestRefs, prompt.unmeasuredTurnRefs, details.agents);
}

function turnSection(turn: TurnUsage, index: number, agent: AgentUsageDetails, promptLabel: boolean, headingLevel: 'h3' | 'h4' = 'h3'): HTMLElement {
  const template = document.querySelector<HTMLTemplateElement>('#prompt-usage-template')!;
  const fragment = document.importNode(template.content, true);
  const section = fragment.querySelector('section')!;
  section.id = turnIdentity({ sessionId: agent.sessionId, turnId: turn.id }); section.tabIndex = -1; section.dataset.agentTurn = agent.sessionId; section.dataset.usageFocusKey = section.id;
  const oldHeading = section.querySelector('h3')!;
  const heading = document.createElement(headingLevel); heading.id = identity('heading', agent.sessionId, turn.id);
  heading.textContent = (promptLabel ? 'Prompt ' : 'Agent turn ') + (index + 1) + ' · ' + turn.status; oldHeading.replaceWith(heading);
  if (!turn.requests.length) {
    section.querySelector('.table-scroll')!.remove();
    section.append(note('No request usage was recorded for this ' + (promptLabel ? 'prompt' : 'agent turn') + '. Token counts and API cost are unknown.'));
    return section;
  }
  const table = section.querySelector('table')!; table.setAttribute('aria-labelledby', heading.id);
  const scroll = section.querySelector<HTMLElement>('.table-scroll')!; scroll.setAttribute('aria-labelledby', heading.id); scroll.dataset.usageFocusKey = section.id + '-table';
  const usage = { ...emptyUsage, requests: turn.requests.length };
  for (const request of turn.requests) {
    appendRequestRow(table.tBodies[0], request, agent.sessionId, turn.id);
    if (request.costUsd === null) usage.unpricedRequests++;
    else usage.costUsd += request.costUsd;
  }
  table.tFoot!.querySelector('td')!.textContent = money(usage);
  return section;
}

function scopeTable(details: ConversationUsageDetails, selected: AgentUsageDetails): HTMLElement {
  const group = document.createElement('div'); group.className = 'table-scroll'; group.tabIndex = 0; group.setAttribute('role', 'group'); group.setAttribute('aria-label', 'Recorded usage by agent scope'); group.dataset.usageFocusKey = 'scope-table';
  const table = document.createElement('table'); table.className = 'data-table usage-scope-table';
  const caption = table.createCaption(); caption.className = 'visually-hidden'; caption.textContent = 'Recorded usage by agent scope';
  const head = table.createTHead().insertRow();
  for (const label of ['Scope', 'Input', 'Cached input', 'Cache writes', 'Output', 'Total tokens', 'API cost']) { const cell = document.createElement('th'); cell.scope = 'col'; cell.textContent = label; head.append(cell); }
  const body = table.createTBody();
  const scopes: [string, UsageSummary][] = [[selected.lineage.kind === 'subagent' ? 'Selected agent' : 'Main agent', details.ownUsage], ['Subagents', details.descendantsUsage], ['Combined', details.combinedUsage]];
  for (const [label, usage] of scopes) {
    const row = body.insertRow(); const heading = document.createElement('th'); heading.scope = 'row'; heading.textContent = label; row.append(heading);
    for (const value of [usageCount(usage, 'input'), cachedCount(usage), cacheWrites(usage), usageCount(usage, 'output'), usageCount(usage, 'total'), money(usage)]) row.insertCell().textContent = value;
  }
  group.append(table); return group;
}

export function usageDetailsStatus(details: ConversationUsageDetails): string {
  const selected = details.agents.find(agent => agent.sessionId === details.sessionId)!;
  const agentTurns = selected.lineage.kind === 'subagent' || selected.agentKind === 'claude-code';
  const base = !details.agentTurnCount ? (agentTurns ? 'No agent turns have been recorded for this session yet.' : 'No prompts have been recorded for this session yet.') : details.combinedUsage.requests ? 'Recorded after each response. Available prices are retained with each request.' : (agentTurns ? 'Agent turns are recorded, but no request usage is available. Token counts and API cost are unknown.' : 'Prompts are recorded, but no request usage is available. Token counts and API cost are unknown.');
  const messages = [base];
  if (agentTurns || details.agents.length > 1 || details.userPromptCount !== null) {
    messages.push(details.userPromptCount === null ? 'User prompt count unknown.' : count(details.userPromptCount) + ' user prompt(s).');
    messages.push(count(details.agentTurnCount) + ' agent turn(s).');
    messages.push('Recorded source coverage: ' + details.coverage + '.');
  }
  if (selected.lineage.kind === 'subagent') messages.push('Selected agent subtree only' + (details.rootSessionId ? '; ancestor main session is excluded.' : '; main session could not be established.'));
  if (details.combinedUsage.unmeasuredTurns) messages.push(count(details.combinedUsage.unmeasuredTurns) + ' agent turn(s) have unknown token counts and API cost.');
  const claude = details.agents.some(agent => agent.agentKind === 'claude-code');
  messages.push(claude ? 'Token API equivalent in USD. Estimates use saved rates and labelled assumptions. Subscription charges, account discounts, and server-tool charges are separate.' : 'API equivalent in USD. ChatGPT subscription billing is separate.');
  messages.push(...details.warnings);
  return messages.join(' ');
}

function preserveUsageView(content: HTMLElement): () => void {
  const disclosures = new Map([...content.querySelectorAll<HTMLDetailsElement>('[data-usage-disclosure-key]')].map(disclosure => [disclosure.dataset.usageDisclosureKey!, disclosure.open]));
  const focus = document.hasFocus() && content.contains(document.activeElement) ? document.activeElement as HTMLElement : null;
  const focusedKey = focus?.dataset.usageFocusKey;
  const panel = content.closest<HTMLElement>('.dialog-content');
  const scrollTop = panel?.scrollTop ?? 0;
  const scrolls = new Map([...content.querySelectorAll<HTMLElement>('.table-scroll[data-usage-focus-key]')].map(group => [group.dataset.usageFocusKey!, [group.scrollLeft, group.scrollTop]]));
  return () => {
    for (const disclosure of content.querySelectorAll<HTMLDetailsElement>('[data-usage-disclosure-key]')) disclosure.open = disclosures.get(disclosure.dataset.usageDisclosureKey!) ?? false;
    if (focusedKey && document.hasFocus() && document.activeElement === document.body) {
      const target = [...content.querySelectorAll<HTMLElement>('[data-usage-focus-key]')].find(element => element.dataset.usageFocusKey === focusedKey) ?? document.getElementById('close-session-usage');
      target?.focus({ preventScroll: true });
    }
    for (const group of content.querySelectorAll<HTMLElement>('.table-scroll[data-usage-focus-key]')) {
      const position = scrolls.get(group.dataset.usageFocusKey!); if (position) { group.scrollLeft = position[0]; group.scrollTop = position[1]; }
    }
    if (panel) panel.scrollTop = scrollTop;
  };
}

export function renderUsageDetails(details: ConversationUsageDetails): void {
  const content = document.getElementById('session-usage-details')!;
  const restore = content.dataset.usageSessionId === details.sessionId ? preserveUsageView(content) : () => {};
  const selected = details.agents.find(agent => agent.sessionId === details.sessionId)!;
  const report = document.createDocumentFragment();
  if (details.agents.length > 1) report.append(scopeTable(details, selected));
  const promptLabel = selected.lineage.kind !== 'subagent' && selected.agentKind !== 'claude-code';
  for (const [index, turn] of selected.turns.entries()) {
    const section = turnSection(turn, index, selected, promptLabel);
    const prompt = details.prompts.find(prompt => prompt.turnId === turn.id);
    if (promptLabel && prompt) appendPromptAttribution(section, prompt, details);
    report.append(section);
  }
  if (!promptLabel && details.prompts.length) {
    const section = document.createElement('section'); section.className = 'turn-usage';
    const heading = document.createElement('h3'); heading.textContent = 'User prompt attribution'; section.append(heading);
    for (const prompt of details.prompts) {
      const group = document.createElement('section'); group.className = 'turn-usage';
      const title = document.createElement('h4'); title.textContent = 'User prompt · ' + prompt.turnId + ' · ' + prompt.status; group.append(title);
      group.append(note('Selected subtree: ' + summaryText(prompt.combinedUsage)));
      appendReferences(group, prompt.requestRefs, prompt.unmeasuredTurnRefs, details.agents); section.append(group);
    }
    report.append(section);
  }
  if (details.agents.length > 1) {
    const section = document.createElement('section'); section.className = 'turn-usage';
    const heading = document.createElement('h3'); heading.textContent = 'Subagents'; section.append(heading);
    for (const agent of details.agents.filter(agent => agent.sessionId !== details.sessionId)) {
      const disclosure = document.createElement('details'); disclosure.className = 'advanced-actions usage-agent'; disclosure.id = identity('agent', agent.sessionId); disclosure.dataset.usageDisclosureKey = agent.sessionId;
      const summary = document.createElement('summary'); summary.textContent = agentName(agent); summary.dataset.usageFocusKey = identity('agent-summary', agent.sessionId); disclosure.append(summary);
      disclosure.append(note(summaryText(agent.ownUsage)));
      disclosure.append(note('Parent: ' + (agent.lineage.parentSessionId ?? 'unknown') + ' · Relationship: ' + agent.relationStatus + ' · Recorded source coverage: ' + agent.coverage + (agent.available ? '' : ' · Session file unavailable')));
      for (const [index, turn] of agent.turns.entries()) disclosure.append(turnSection(turn, index, agent, false, 'h4'));
      if (!agent.turns.length) disclosure.append(note('No agent turns have been recorded.'));
      section.append(disclosure);
    }
    report.append(section);
  }
  if (details.unassigned.requestRefs.length || details.unassigned.unmeasuredTurnRefs.length) {
    const section = document.createElement('section'); section.className = 'turn-usage usage-unassigned';
    const heading = document.createElement('h3'); heading.textContent = 'Unassigned to a user prompt'; section.append(heading);
    section.append(note('These recorded requests or unmeasured agent turns have no proven user prompt attribution. They remain included in the selected scope.'));
    section.append(note(summaryText(details.unassigned.usage)));
    appendReferences(section, details.unassigned.requestRefs, details.unassigned.unmeasuredTurnRefs, details.agents); report.append(section);
  }
  content.replaceChildren(report); content.dataset.usageSessionId = details.sessionId; restore();
}

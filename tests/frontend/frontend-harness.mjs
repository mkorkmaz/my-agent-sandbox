import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { stripTypeScriptTypes } from 'node:module';
import { fileURLToPath } from 'node:url';
import { JSDOM } from 'jsdom';
import { createStore } from 'zustand/vanilla';

const root = fileURLToPath(new URL('../../', import.meta.url));
const terminalStatuses = JSON.parse(fs.readFileSync(root + 'tests/fixtures/terminal-status.json', 'utf8'));
const flush = async () => { for (let count = 0; count < 6; count++) await new Promise(resolve => setImmediate(resolve)); };
function deferred() {
  let resolve, reject;
  const promise = new Promise((complete, fail) => { resolve = complete; reject = fail; });
  return { promise, resolve, reject };
}
const project = { id: 'project-a', environmentId: 'a'.repeat(32), agentKind: 'codex', name: 'örnek proje', path: '/fixture/örnek proje', profile: 'codex', label: 'sample-a', stage: 'environment-ready', ownsBinding: true, ownsSession: true, policy: { name: 'sample', owned: true, fingerprint: 'policy' } };
const plan = { data: { profile: 'codex', session: 'sample-a', runtime: { kind: 'hull', backend: 'hvi' }, isolation: 'microVM', project: { host: project.path, guest: '/work/örnek proje', mode: 'read-write' }, home: { host: '/fixture/guest-home', guest: '/root' }, limits: { cpus: 4, mem: 4096 }, posture: 'isolated', image: { ref: 'fixture' }, verify: { mode: 'require' }, policies: ['sample'], egress: { default: 'deny', allow: [] }, credentials: [] } };
const emptyUsage = { input: 0, cached: 0, cacheWrite: 0, output: 0, reasoning: 0, total: 0, costUsd: 0, unpricedRequests: 0, requests: 0, completedTurns: 0, assumedTierRequests: 0, missingCacheRequests: 0, missingCachedRequests: 0, missingCacheWriteRequests: 0, unmeasuredTurns: 0 };
const unknownLineage = { kind: 'unknown', parentSessionId: null, agentPath: null, nickname: null, sourceVersion: null, lineageVersion: 1 };
const conversationFixture = conversation => ({ state: 'active', available: true, usage: { ...emptyUsage }, warning: null, lineage: { ...unknownLineage }, relationStatus: 'unknown', rootSessionId: null, subagentCount: 0, coverage: 'unknown', usageRevision: 'fixture-v1', ...conversation, familyUsage: conversation.familyUsage ?? conversation.usage ?? { ...emptyUsage } });
const sessionDashboard = (conversations, overrides = {}) => ({ conversations: conversations.map(conversationFixture), usage: { ...emptyUsage }, coverage: 'unknown', pricingDate: '2026-11-10', warnings: [], ...overrides });
const rateFixture = rates => rates ? { source: 'fixture', input: 1, cached: 0.1, write30m: 1, write12h: 1, output: 2, longContext: false, ...rates } : null;
const requestFixture = (request, key) => ({ key, partial: false, serviceTier: null, tierSource: null, rateCorrection: null, source: null, amountCorrection: null, ...request, tokens: { reasoning: 0, write12h: null, cacheKnown: true, cachedKnown: null, cacheWriteKnown: null, ...request.tokens }, rates: rateFixture(request.rates) });
function sessionDetails(sessionId, sourceTurns, overrides = {}) {
  const turns = sourceTurns.map(turn => ({ originThreadId: null, rootTurnId: null, parentTurnId: null, ...turn, requests: turn.requests.map((request, index) => requestFixture(request, sessionId + '/' + turn.id + '/' + index)) }));
  const usage = { ...emptyUsage, completedTurns: turns.filter(turn => turn.status === 'complete').length, unmeasuredTurns: turns.filter(turn => !turn.requests.length).length };
  const requestRefs = [], unmeasuredTurnRefs = [];
  for (const turn of turns) {
    if (!turn.requests.length) unmeasuredTurnRefs.push({ sessionId, turnId: turn.id });
    for (const request of turn.requests) {
      const tokens = request.tokens;
      for (const field of ['input', 'cached', 'cacheWrite', 'output', 'reasoning']) usage[field] += tokens[field];
      usage.total += tokens.input + tokens.output;
      usage.requests++;
      if (request.costUsd === null) usage.unpricedRequests++;
      else usage.costUsd += request.costUsd;
      if (request.rates?.tierAssumed) usage.assumedTierRequests++;
      if ((tokens.cachedKnown ?? tokens.cacheKnown) === false) usage.missingCachedRequests++;
      if ((tokens.cacheWriteKnown ?? tokens.cacheKnown) === false) usage.missingCacheWriteRequests++;
      if ((tokens.cachedKnown ?? tokens.cacheKnown) === false || (tokens.cacheWriteKnown ?? tokens.cacheKnown) === false) usage.missingCacheRequests++;
      requestRefs.push({ sessionId, turnId: turn.id, requestKey: request.key });
    }
  }
  return { sessionId, rootSessionId: null, ownUsage: usage, descendantsUsage: { ...emptyUsage }, combinedUsage: usage, coverage: 'unknown', warnings: [], agents: [{ sessionId, title: 'Legacy single', available: true, lineage: { ...unknownLineage }, relationStatus: 'unknown', ownUsage: usage, coverage: 'unknown', turns }], userPromptCount: null, agentTurnCount: turns.length, prompts: [], unassigned: { usage, requestRefs, unmeasuredTurnRefs }, ...overrides };
}
const settings = { schemaVersion: 2, cliPath: null, codex: { profile: 'codex', allowedHosts: [] }, claudeCode: { profile: 'tugboat-claude', allowedHosts: ['api.anthropic.com'] }, cpus: 4, memoryMb: 4096, verification: 'require', network: 'isolated', telemetry: false };
const recordsLocation = { directory: '/fixture/Application Support/Tugboat', defaultDirectory: '/fixture/Application Support/Tugboat', pendingDirectory: null, available: true, canChange: true, message: null };
const sharedAccount = { generation: 1, pendingProjectId: null, pendingEnvironmentId: null, state: 'signed-in', email: 'fixture@example.invalid', message: 'Shared ChatGPT account ready.', candidates: [] };
const sharedClaudeAccount = { generation: 1, state: 'signed-in', email: 'claude@example.invalid', message: 'Shared Claude account ready.' };
const providerLogin = (provider, phase = 'idle', revision = 0) => ({ provider, phase, revision, authorizationUrl: phase === 'waiting' ? provider === 'chatgpt' ? 'https://auth.openai.com/codex/device' : 'https://claude.com/cai/oauth/authorize' : null, userCode: provider === 'chatgpt' && phase === 'waiting' ? 'ABCD-EFGH' : null, message: phase === 'waiting' ? 'Complete provider sign-in in your browser.' : '' });

function captureProjectRescans(dom) {
  const pending = new Map();
  const set = dom.window.setTimeout.bind(dom.window), clear = dom.window.clearTimeout.bind(dom.window);
  let next = -1;
  dom.window.setTimeout = (callback, delay, ...args) => {
    if (delay !== 8000) return set(callback, delay, ...args);
    const id = next--; pending.set(id, callback); return id;
  };
  dom.window.clearTimeout = id => { if (!pending.delete(id)) clear(id); };
  return { pending, run: () => { const entry = pending.entries().next().value; assert.ok(entry, 'Expected an automatic folder check'); pending.delete(entry[0]); entry[1](); } };
}

async function interfaceHarness(overrides = {}, configureDom = () => {}) {
  const dom = new JSDOM(fs.readFileSync(root + 'index.html', 'utf8'), { url: 'https://tauri.localhost/' });
  configureDom(dom);
  dom.window.HTMLDialogElement.prototype.showModal = function() { this.open = true; };
  dom.window.HTMLDialogElement.prototype.close = function() { this.open = false; this.dispatchEvent(new dom.window.Event('close')); };
  const calls = [], listeners = new Map(), modules = new Map();
  const terminals = new Map();
  let resizeTerminal;
  let selectedFixture = project;
  let nextTerminal = 1;
  const handlers = {
    frontend_ready: () => null, update_menu_state: () => null,
    application_version: () => '0.3.0',
    storage_location: () => recordsLocation,
    shared_chatgpt_status: () => sharedAccount,
    sync_project_chatgpt: () => sharedAccount,
    retry_shared_chatgpt: () => sharedAccount,
    shared_claude_status: () => sharedClaudeAccount,
    retry_shared_claude: () => sharedClaudeAccount,
    disconnect_shared_claude: () => ({ ...sharedClaudeAccount, generation: 2, state: 'signed-out', email: null }),
    provider_login_status: ({ provider }) => providerLogin(provider),
    start_provider_login: ({ provider }) => providerLogin(provider, 'waiting', 1),
    cancel_provider_login: ({ provider }) => providerLogin(provider, 'idle', 2),
    open_provider_login: () => null,
    load_interface_preferences: () => ({ sidebarWidth: null }),
    save_interface_preferences: () => null,
    load_settings: () => settings,
    telemetry_status: () => ({ recorded: 'off', effective: 'off', message: null }),
    recent_projects: () => [], choose_project_folder: () => project.path,
    rescan_projects: () => handlers.recent_projects(),
    open_project: () => project, saved_project_dashboard: () => sessionDashboard([]), recorded_project_dashboard: () => sessionDashboard([]), project_dashboard: () => sessionDashboard([]),
    read_project_preferences: () => ({ project, options: { cpus: 4, memoryMb: 4096, verification: 'require', network: 'isolated' } }),
    register_project: () => project,
    prepare_project: () => ({ project, plan, fingerprint: 'plan-a', message: null, candidates: [] }),
    preview_project: () => ({ project: selectedFixture, plan, fingerprint: 'plan-a' }),
    inspect_host: () => ({ architecture: 'arm64', osVersion: '27.2', tools: {}, brigStatus: 'installed', version: null, doctor: null, message: null }),
    terminal_sessions: ({ projectId }) => [...terminals.values()].filter(status => status.projectId === projectId),
    terminal_snapshot: ({ identifier }) => terminals.get(identifier) ?? null,
    terminal_layout: () => null, terminal_focus: () => null,
    terminal_close: ({ identifier }) => { terminals.delete(identifier); },
    ensure_project_environment: ({ agentKind }) => agentKind === 'codex' ? project : { ...project, agentKind, environmentId: 'b'.repeat(32), profile: 'tugboat-claude' },
    agent_account_status: ({ identifier, environmentId }) => ({ projectId: identifier, environmentId, state: 'signed-in', message: 'Claude ready.', terminalIdentifier: null }),
    start_agent_session: ({ identifier: projectId, environmentId, action, sessionId }) => {
      const identifier = nextTerminal++;
      const provider = environmentId === 'b'.repeat(32) || (selectedFixture.environmentId === environmentId && selectedFixture.agentKind === 'claude-code') ? 'claude' : 'codex';
      const status = terminalStatuses[provider + '-' + action] ?? { purpose: action, running: true, exitCode: null, busy: false };
      const conversationId = action === 'new-chat' && provider === 'claude' ? '00000000-0000-4000-8000-' + String(identifier).padStart(12, '0') : sessionId ?? null;
      terminals.set(identifier, { ...status, identifier, projectId, environmentId, agentKind: provider === 'claude' ? 'claude-code' : 'codex', conversationId });
      return identifier;
    },
    ...overrides,
  };
  const invoke = async (command, args) => {
    calls.push({ command, args });
    assert.ok(handlers[command], 'Unhandled command: ' + command);
    const result = await handlers[command](args);
    if (['open_project', 'register_project', 'ensure_project_environment'].includes(command) && result) selectedFixture = result;
    return result;
  };
  const context = vm.createContext({ document: dom.window.document, console, ResizeObserver: class { constructor(callback) { resizeTerminal = callback; } observe() {} }, setTimeout, clearTimeout });
  const core = new vm.SyntheticModule(['invoke'], function() { this.setExport('invoke', invoke); }, { context });
  const events = new vm.SyntheticModule(['listen'], function() { this.setExport('listen', async (event, callback) => {
    const previous = listeners.get(event);
    listeners.set(event, previous ? async payload => { await previous(payload); await callback(payload); } : callback);
    return () => listeners.delete(event);
  }); }, { context });
  const css = new vm.SyntheticModule([], function() {}, { context });
  const zustand = new vm.SyntheticModule(['createStore'], function() { this.setExport('createStore', createStore); }, { context });
  function sourceModule(file) {
    if (modules.has(file)) return modules.get(file);
    const module = new vm.SourceTextModule(stripTypeScriptTypes(fs.readFileSync(file, 'utf8')), { context, identifier: file });
    modules.set(file, module);
    return module;
  }
  const main = path.join(root, 'src/main.ts');
  const module = sourceModule(main);
  await module.link((name, parent) => {
    if (name.endsWith('.css')) return css;
    if (name === '@tauri-apps/api/core') return core;
    if (name === '@tauri-apps/api/event') return events;
    if (name === 'zustand/vanilla') return zustand;
    return sourceModule(path.resolve(path.dirname(parent.identifier), name + '.ts'));
  });
  const ready = module.evaluate();
  await flush();
  return { dom, calls, handlers, ready, listeners, terminals, resizeTerminal: () => resizeTerminal(), renderUsage: details => modules.get(path.join(root, 'src/dashboard/dashboard.ts')).namespace.renderUsageDetails(details), element: id => dom.window.document.getElementById(id), setTerminal: value => { terminals.set(value.identifier, value); } };
}

export { root, flush, deferred, project, plan, emptyUsage, unknownLineage, conversationFixture, sessionDashboard, sessionDetails, settings, recordsLocation, sharedAccount, sharedClaudeAccount, providerLogin, captureProjectRescans, interfaceHarness };

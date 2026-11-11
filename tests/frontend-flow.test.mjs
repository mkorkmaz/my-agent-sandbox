import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { stripTypeScriptTypes } from 'node:module';
import { fileURLToPath } from 'node:url';
import { JSDOM } from 'jsdom';

const root = fileURLToPath(new URL('../', import.meta.url));
const flush = async () => { for (let count = 0; count < 6; count++) await new Promise(resolve => setImmediate(resolve)); };
const project = { id: 'project-a', name: 'örnek proje', path: '/fixture/örnek proje', profile: 'codex', label: 'sample-a', stage: 'environment-ready', ownsBinding: true, ownsSession: true, policy: { name: 'sample', owned: true, fingerprint: 'policy' } };
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
const settings = { schemaVersion: 1, cliPath: null, profile: 'codex', cpus: 4, memoryMb: 4096, verification: 'require', network: 'isolated', allowedHosts: [], telemetry: false };

async function interfaceHarness(overrides = {}) {
  const dom = new JSDOM(fs.readFileSync(root + 'index.html', 'utf8'), { url: 'https://tauri.localhost/' });
  dom.window.HTMLDialogElement.prototype.showModal = function() { this.open = true; };
  dom.window.HTMLDialogElement.prototype.close = function() { this.open = false; this.dispatchEvent(new dom.window.Event('close')); };
  const calls = [], listeners = new Map(), modules = new Map();
  const terminals = new Map();
  let nextTerminal = 1;
  const handlers = {
    frontend_ready: () => null, update_menu_state: () => null,
    application_version: () => '0.1.0',
    load_settings: () => settings,
    telemetry_status: () => ({ recorded: 'off', effective: 'off', message: null }),
    recent_projects: () => [], choose_project_folder: () => project.path,
    open_project: () => project, project_dashboard: () => sessionDashboard([]),
    read_project_preferences: () => ({ project, options: { cpus: 4, memoryMb: 4096, verification: 'require', network: 'isolated' } }),
    register_project: () => project,
    prepare_project: () => ({ project, plan, fingerprint: 'plan-a', message: null, candidates: [] }),
    preview_project: () => ({ project, plan, fingerprint: 'plan-a' }),
    inspect_host: () => ({ architecture: 'arm64', osVersion: '27.2', tools: {}, brigStatus: 'installed', version: null, doctor: null, message: null }),
    terminal_sessions: ({ projectId }) => [...terminals.values()].filter(status => status.projectId === projectId),
    terminal_snapshot: ({ identifier }) => terminals.get(identifier) ?? null,
    terminal_layout: () => null, terminal_focus: () => null,
    terminal_close: ({ identifier }) => { terminals.delete(identifier); },
    start_codex_session: ({ identifier: projectId, action, sessionId }) => {
      const identifier = nextTerminal++;
      terminals.set(identifier, { identifier, projectId, purpose: action, running: true, exitCode: null, conversationId: sessionId ?? null, busy: false });
      return identifier;
    },
    ...overrides,
  };
  const invoke = async (command, args) => {
    calls.push({ command, args });
    assert.ok(handlers[command], 'Unhandled command: ' + command);
    return handlers[command](args);
  };
  const context = vm.createContext({ document: dom.window.document, console, ResizeObserver: class { observe() {} }, setTimeout, clearTimeout });
  const core = new vm.SyntheticModule(['invoke'], function() { this.setExport('invoke', invoke); }, { context });
  const events = new vm.SyntheticModule(['listen'], function() { this.setExport('listen', async (event, callback) => { listeners.set(event, callback); return () => listeners.delete(event); }); }, { context });
  const css = new vm.SyntheticModule([], function() {}, { context });
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
    return sourceModule(path.resolve(path.dirname(parent.identifier), name + '.ts'));
  });
  const ready = module.evaluate();
  await flush();
  return { dom, calls, handlers, ready, listeners, terminals, renderUsage: details => modules.get(path.join(root, 'src/dashboard.ts')).namespace.renderUsageDetails(details), element: id => dom.window.document.getElementById(id), setTerminal: value => { terminals.set(value.identifier, value); } };
}

test('settings stay disabled until initialization installs the save handler', async () => {
  let release;
  const harness = await interfaceHarness({ telemetry_status: () => new Promise(resolve => { release = resolve; }) });
  assert.equal(harness.element('settings-fields').disabled, true);
  release({ recorded: 'off', effective: 'off', message: null });
  await harness.ready;
  assert.equal(harness.element('settings-fields').disabled, false);
  harness.dom.window.close();
});

test('cancelled folder selection does not register a project', async () => {
  const harness = await interfaceHarness({ choose_project_folder: () => null });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'register_project').length, 0);
  assert.equal(harness.element('choose-folder').disabled, false);
  harness.dom.window.close();
});

test('double launch creates one Codex process and navigation returns to that process', async () => {
  let finishStart;
  const terminal = { identifier: 42, projectId: project.id, purpose: 'codex', running: true, exitCode: null };
  const harness = await interfaceHarness({ start_codex_session: () => new Promise(resolve => { finishStart = () => { harness.setTerminal(terminal); resolve(42); }; }) });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'prepare_project').length, 1);
  harness.element('start-codex').click(); harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'start_codex_session').length, 1);
  finishStart(); await flush();
  harness.element('settings-navigation').click(); await flush();
  assert.equal(harness.element('project-view').hidden, true);
  assert.ok(harness.calls.some(call => call.command === 'terminal_layout' && call.args.identifier === 42 && call.args.layout.visible === false));
  harness.element('sidebar-current-project').click(); await flush();
  assert.ok(harness.calls.some(call => call.command === 'terminal_focus' && call.args.identifier === 42));
  assert.equal(harness.calls.filter(call => call.command === 'start_codex_session').length, 1);
  harness.dom.window.close();
});

test('failed auxiliary launch hides the previous native surface and preserves Codex', async () => {
  const harness = await interfaceHarness({ start_project_operation: () => { throw new Error('Simulated process failure'); } });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('hide-terminal').click(); await flush();
  harness.element('git-diff').click(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.element('return-to-terminal').hidden, false);
  assert.match(harness.element('folder-error').textContent, /Simulated process failure/);
  assert.ok(harness.calls.some(call => call.command === 'terminal_layout' && call.args.identifier === 1 && !call.args.layout.visible));
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 0);
  harness.dom.window.close();
});

test('an auxiliary process can remain open while another Codex chat starts', async () => {
  const harness = await interfaceHarness();
  harness.handlers.start_project_operation = () => {
    harness.setTerminal({ identifier: 20, projectId: project.id, purpose: 'tests', running: true, exitCode: null, conversationId: null, busy: false });
    return 20;
  };
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('hide-terminal').click(); await flush();
  harness.element('git-diff').click(); await flush();
  harness.element('hide-terminal').click(); await flush();
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.terminals.size, 3);
  assert.equal(harness.calls.filter(call => call.command === 'start_codex_session').length, 2);
  assert.equal(harness.element('folder-error').hidden, true);
  harness.dom.window.close();
});

test('ready removal preview clears its loading message before opening the dialog', async () => {
  const harness = await interfaceHarness({ preview_removal: () => ({ reference: 'codex@sample-a', workspace: '/fixture/guest-home', description: 'Own test session', fingerprint: 'remove-a', external: false, hasEnvironment: false }) });
  await harness.ready;
  const dialog = harness.element('removal-dialog');
  dialog.showModal = () => { dialog.open = true; };
  harness.element('choose-folder').click(); await flush();
  harness.element('preview-removal').click(); await flush();
  assert.equal(dialog.open, true);
  assert.equal(harness.element('operation-status').textContent, '');
  assert.equal(harness.element('logout-on-removal').disabled, true);
  harness.dom.window.close();
});

test('the project library sorts by last opening and selected projects replace the sidebar list', async () => {
  const older = { ...project, id: 'older', name: 'Older project', path: '/fixture/older', lastOpenedAt: 1000, updatedAt: 9000, gitBranch: 'feature/older' };
  const newer = { ...project, id: 'newer', name: 'Newer project', path: '/fixture/newer', lastOpenedAt: 2000, updatedAt: 1, gitBranch: 'main' };
  const projects = [older, newer];
  const harness = await interfaceHarness({
    recent_projects: () => projects,
    open_project: ({ identifier }) => projects.find(project => project.id === identifier),
    prepare_project: ({ identifier }) => ({ project: projects.find(project => project.id === identifier), plan, fingerprint: 'plan-a', message: null, candidates: [] }),
  });
  await harness.ready;
  const rows = [...harness.element('recent-project-list').querySelectorAll('button')];
  assert.deepEqual(rows.map(row => row.dataset.projectId), ['newer', 'older']);
  assert.match(rows[1].closest('tr').textContent, /feature\/older/);
  assert.doesNotMatch(rows[1].closest('tr').textContent, /fixture/);
  assert.equal(rows[1].title, older.name + '\n' + older.path);
  assert.equal(harness.element('sidebar-project-list').hidden, false);
  rows[0].click(); await flush();
  assert.equal(harness.element('sidebar-project-list').hidden, true);
  assert.equal(harness.element('sidebar-project-context').hidden, false);
  assert.match(harness.element('sidebar-current-project').textContent, /Newer project/);
  assert.doesNotMatch(harness.element('sidebar-project-context').textContent, /Older project/);
  assert.equal(harness.element('recent-projects').hidden, true);
  harness.element('project-navigation').click(); await flush();
  assert.equal(harness.element('sidebar-project-list').hidden, false);
  assert.equal(harness.element('sidebar-project-context').hidden, true);
  assert.equal(harness.element('recent-projects').hidden, false);
  harness.dom.window.close();
});

test('Add new project registers a folder and makes it available in both project lists', async () => {
  let projects = [];
  const harness = await interfaceHarness({
    recent_projects: () => projects,
    register_project: () => { projects = [{ ...project, lastOpenedAt: 1000, gitBranch: 'main' }]; return projects[0]; },
  });
  await harness.ready;
  assert.equal(harness.element('choose-folder').textContent, 'Add new project');
  harness.element('choose-folder').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'register_project').length, 1);
  assert.match(harness.element('sidebar-current-project').textContent, /örnek proje/);
  harness.element('project-navigation').click(); await flush();
  assert.equal(harness.element('sidebar-project-list').querySelectorAll('button').length, 1);
  assert.equal(harness.element('recent-project-list').querySelectorAll('button').length, 1);
  harness.dom.window.close();
});

test('saved session titles keep their full tooltip and resume the exact conversation', async () => {
  const conversation = { id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7', title: 'A long Türkçe and 日本語 session title that must remain complete in the tooltip and accessible name', updatedAt: 2000 };
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard([conversation]) });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  const row = harness.element('sidebar-session-list').querySelector('button');
  assert.equal(row.title, conversation.title);
  assert.equal(row.getAttribute('aria-label'), conversation.title);
  assert.equal(row.textContent, conversation.title);
  row.click(); await flush();
  const launch = harness.calls.find(call => call.command === 'start_codex_session');
  assert.equal(launch.args.action, 'resume');
  assert.equal(launch.args.sessionId, conversation.id);
  assert.equal(launch.args.identifier, project.id);
  assert.equal(launch.args.fingerprint, 'plan-a');
  harness.dom.window.close();
});

test('a delayed history response cannot replace another project session list', async () => {
  const second = { ...project, id: 'project-b', name: 'Second project', path: '/fixture/second' };
  const projects = [project, second];
  let releaseFirst;
  const harness = await interfaceHarness({
    recent_projects: () => projects,
    open_project: ({ identifier }) => projects.find(project => project.id === identifier),
    prepare_project: ({ identifier }) => ({ project: projects.find(project => project.id === identifier), plan, fingerprint: 'plan-a', message: null, candidates: [] }),
    project_dashboard: ({ identifier }) => identifier === project.id
      ? new Promise(resolve => { releaseFirst = resolve; })
      : sessionDashboard([{ id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e8', title: 'Second project history', updatedAt: 2000 }]),
  });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  harness.element('project-navigation').click(); await flush();
  harness.element('recent-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
  releaseFirst(sessionDashboard([{ id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7', title: 'First project history', updatedAt: 1000 }])); await flush();
  assert.match(harness.element('sidebar-session-list').textContent, /Second project history/);
  assert.doesNotMatch(harness.element('sidebar-session-list').textContent, /First project history/);
  harness.dom.window.close();
});

test('a saved session opens alongside a new chat and each sidebar row returns to its own terminal', async () => {
  const conversation = { id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7', title: 'Earlier session', updatedAt: 1000 };
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard([conversation]) });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('sidebar-session-list').querySelector('[data-session-id="' + conversation.id + '"]').click(); await flush();
  assert.equal(harness.element('folder-error').hidden, true);
  assert.equal(harness.calls.filter(call => call.command === 'start_codex_session').length, 2);
  assert.equal(harness.terminals.size, 2);
  for (const [session, identifier] of [['terminal-1', 1], [conversation.id, 2], ['terminal-1', 1]]) {
    harness.element('sidebar-session-list').querySelector('[data-session-id="' + session + '"]').click(); await flush();
    assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, identifier);
    assert.equal(harness.element('sidebar-session-list').querySelector('[aria-current="true"]').dataset.sessionId, session);
  }
  assert.equal(harness.calls.filter(call => call.command === 'start_codex_session').length, 2);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 0);
  harness.dom.window.close();
});

test('the current saved session returns to its existing terminal after settings and project navigation', async () => {
  const conversation = { id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7', title: 'Current saved session', updatedAt: 1000 };
  const harness = await interfaceHarness({ recent_projects: () => [project], project_dashboard: () => sessionDashboard([conversation]) });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  harness.element('sidebar-session-list').querySelector('button').click(); await flush();
  harness.element('settings-navigation').click(); await flush();
  harness.element('sidebar-session-list').querySelector('button').click(); await flush();
  assert.equal(harness.element('project-view').hidden, false);
  assert.equal(harness.element('folder-error').hidden, true);
  harness.element('project-navigation').click(); await flush();
  harness.element('recent-project-list').querySelector('button').click(); await flush();
  harness.element('sidebar-session-list').querySelector('button').click(); await flush();
  assert.equal(harness.element('folder-error').hidden, true);
  assert.equal(harness.element('terminal-status').textContent, conversation.title);
  assert.equal(harness.calls.filter(call => call.command === 'start_codex_session').length, 1);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 0);
  harness.dom.window.close();
});

test('parallel new chats keep their identities, report agent activity, and close independently', async () => {
  const harness = await interfaceHarness();
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('hide-terminal').click(); await flush();
  assert.equal(harness.element('start-codex').disabled, false);
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.terminals.size, 2);
  const first = { id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7', title: 'First agent', updatedAt: 1000, available: false };
  const second = { id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e8', title: 'Second agent', updatedAt: 2000 };
  harness.handlers.project_dashboard = () => sessionDashboard([second, first]);
  harness.setTerminal({ ...harness.terminals.get(2), conversationId: second.id, busy: true });
  await harness.listeners.get('terminal-sessions-changed')({ payload: project.id }); await flush();
  assert.equal(harness.element('sidebar-session-list').querySelector('[aria-current="true"]').dataset.sessionId, second.id);
  harness.setTerminal({ ...harness.terminals.get(1), conversationId: first.id, busy: false });
  harness.listeners.get('project-usage-updated')({ payload: [project.id, sessionDashboard([second, first])] });
  await harness.listeners.get('terminal-sessions-changed')({ payload: project.id }); await flush();
  const row = id => harness.element('sidebar-session-list').querySelector('[data-session-id="' + id + '"]');
  assert.equal(harness.element('sidebar-session-list').children.length, 2);
  assert.ok(row(second.id).querySelector('.session-activity'));
  assert.match(row(second.id).getAttribute('aria-label'), /agent working/);
  assert.equal(row(first.id).querySelector('.session-activity'), null);
  row(first.id).click(); await flush();
  harness.element('close-terminal').click(); await flush();
  assert.deepEqual([...harness.terminals.keys()], [2]);
  row(second.id).click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, 2);
  assert.equal(harness.calls.filter(call => call.command === 'start_codex_session').length, 2);
  harness.setTerminal({ ...harness.terminals.get(2), running: false, exitCode: 0 });
  await harness.listeners.get('terminal-exit')({ payload: { identifier: 2, projectId: project.id, purpose: 'codex', code: 0 } });
  await flush();
  assert.equal(row(second.id).querySelector('.session-activity'), null);
  harness.dom.window.close();
});

test('project navigation remembers the selected chat while other projects run', async () => {
  const other = { ...project, id: 'project-b', name: 'Second project', path: '/fixture/second' };
  const projects = [project, other];
  const selected = ({ identifier }) => ({ project: projects.find(item => item.id === identifier), plan, fingerprint: 'plan-a', candidates: [] });
  const harness = await interfaceHarness({ recent_projects: () => projects, open_project: args => selected(args).project, prepare_project: selected, preview_project: selected });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('hide-terminal').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('sidebar-session-list').querySelector('[data-session-id="terminal-1"]').click(); await flush();
  harness.element('project-navigation').click(); await flush();
  harness.element('recent-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('project-navigation').click(); await flush();
  harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').click(); await flush();
  harness.element('return-to-terminal').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, 1);
  assert.equal(harness.terminals.size, 3);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 0);
  harness.dom.window.close();
});

test('a delayed terminal response cannot reverse the latest sidebar selection', async () => {
  const first = { id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7', title: 'First chat', updatedAt: 1000 };
  const second = { id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e8', title: 'Second chat', updatedAt: 2000 };
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard([first, second]) });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  const select = id => harness.element('sidebar-session-list').querySelector('[data-session-id="' + id + '"]').click();
  select(first.id); await flush(); select(second.id); await flush();
  const list = harness.handlers.terminal_sessions;
  let release;
  harness.handlers.terminal_sessions = () => new Promise(resolve => { release = () => resolve([...harness.terminals.values()]); });
  select(first.id); await flush();
  harness.handlers.terminal_sessions = list;
  select(second.id); await flush();
  release(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, 2);
  assert.equal(harness.element('sidebar-session-list').querySelector('[aria-current="true"]').dataset.sessionId, second.id);
  harness.dom.window.close();
});

test('ending one chat cannot hide a different chat selected while closing', async () => {
  const first = { id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7', title: 'First chat', updatedAt: 1000 };
  const second = { id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e8', title: 'Second chat', updatedAt: 2000 };
  let finishClose;
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard([first, second]), terminal_close: ({ identifier }) => new Promise(resolve => { finishClose = () => { harness.terminals.delete(identifier); resolve(); }; }) });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  const select = id => harness.element('sidebar-session-list').querySelector('[data-session-id="' + id + '"]').click();
  select(first.id); await flush(); select(second.id); await flush(); select(first.id); await flush();
  harness.element('close-terminal').click(); await flush();
  select(second.id); await flush();
  finishClose(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.deepEqual([...harness.terminals.keys()], [2]);
  assert.equal(harness.element('sidebar-session-list').querySelector('[aria-current="true"]').dataset.sessionId, second.id);
  harness.dom.window.close();
});

test('sidebar resizing clamps pointer and keyboard input, saves the width and preserves workspace space', async () => {
  const harness = await interfaceHarness();
  await harness.ready;
  const handle = harness.element('sidebar-resizer');
  const window = harness.dom.window;
  handle.setPointerCapture = () => {};
  handle.releasePointerCapture = () => handle.dispatchEvent(new window.Event('lostpointercapture'));
  const key = value => handle.dispatchEvent(new window.KeyboardEvent('keydown', { key: value, bubbles: true }));
  key('ArrowLeft');
  assert.equal(handle.getAttribute('aria-valuenow'), '280');
  key('ArrowRight');
  assert.equal(window.localStorage.getItem('tugboat.sidebarWidth'), '296');
  handle.dispatchEvent(new window.PointerEvent('pointerdown', { button: 0, pointerId: 1 }));
  handle.dispatchEvent(new window.PointerEvent('pointermove', { pointerId: 1, clientX: 400 }));
  handle.dispatchEvent(new window.PointerEvent('pointerup', { pointerId: 1 }));
  assert.equal(window.localStorage.getItem('tugboat.sidebarWidth'), '400');
  assert.equal(harness.dom.window.document.querySelector('.app-shell').classList.contains('resizing-sidebar'), false);
  Object.defineProperty(window, 'innerWidth', { value: 760, configurable: true });
  window.dispatchEvent(new window.Event('resize'));
  assert.equal(handle.getAttribute('aria-valuenow'), '280');
  Object.defineProperty(window, 'innerWidth', { value: 1024, configurable: true });
  window.dispatchEvent(new window.Event('resize'));
  assert.equal(handle.getAttribute('aria-valuenow'), '400');
  key('End');
  assert.equal(handle.getAttribute('aria-valuenow'), '544');
  key('Home');
  assert.equal(handle.getAttribute('aria-valuenow'), '280');
  harness.dom.window.close();
});

test('session refresh stays disabled while the selected project is being prepared', async () => {
  let finishPreparation;
  const harness = await interfaceHarness({ prepare_project: () => new Promise(resolve => { finishPreparation = resolve; }) });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  assert.equal(harness.element('refresh-sessions').disabled, true);
  finishPreparation({ project, plan, fingerprint: 'plan-a', message: null, candidates: [] }); await flush();
  assert.equal(harness.element('refresh-sessions').disabled, false);
  harness.dom.window.close();
});

test('project dashboard keeps project identity, session costs and creation actions in their scopes', async () => {
  const conversation = { id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7', title: 'Existing chat', updatedAt: 2000, state: 'active', usage: { ...emptyUsage, total: 1050, input: 1000, cached: 100, output: 50, costUsd: 0.00231, requests: 1 } };
  const data = sessionDashboard([conversation], { usage: conversation.usage });
  const harness = await interfaceHarness({ recent_projects: () => [project], project_dashboard: () => data });
  await harness.ready;
  assert.equal(harness.element('create-demo-project'), null);
  assert.equal(harness.element('sidebar-add-project'), null);
  harness.element('choose-folder').click(); await flush();
  assert.equal(harness.element('choose-folder').hidden, true);
  assert.equal(harness.element('project-settings-button').hidden, false);
  assert.equal(harness.element('project-overview').hidden, false);
  assert.equal(harness.element('project-reference').textContent, 'codex@sample-a');
  assert.equal(harness.element('selected-folder').textContent, project.path);
  assert.match(harness.element('session-table-body').textContent, /Existing chat/);
  assert.match(harness.element('project-usage').textContent, /1,050/);
  assert.match(harness.element('project-usage').textContent, /100 \(10%\)/);
  assert.match(harness.element('session-table-body').textContent, /100 \(10%\)/);
  harness.element('session-table-body').querySelector('.session-title').click(); await flush();
  assert.equal(harness.calls.find(call => call.command === 'start_codex_session').args.sessionId, conversation.id);
  harness.element('project-navigation').click(); await flush();
  assert.equal(harness.element('project-heading').textContent, 'Projects');
  assert.equal(harness.element('choose-folder').hidden, false);
  assert.equal(harness.element('project-dashboard').hidden, true);
  harness.dom.window.close();
});

test('archive, Trash and restore move a session between views without changing project totals', async () => {
  const conversation = { id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7', title: 'Keep usage history', updatedAt: 2000, state: 'active', usage: { ...emptyUsage, input: 1000, output: 50, total: 1050, costUsd: 0.01, requests: 1 } };
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard([conversation], { usage: conversation.usage }), change_conversation_state: ({ state, identifier, sessionId }) => { assert.equal(identifier, project.id); assert.equal(sessionId, conversation.id); conversation.state = state; } });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const button = label => [...harness.element('session-table-body').querySelectorAll('button')].find(button => button.getAttribute('aria-label') === label + ' ' + conversation.title);
  button('Archive').click(); await flush();
  assert.equal(harness.element('sidebar-session-list').children.length, 0);
  harness.dom.window.document.querySelector('[data-session-state="archived"]').click();
  assert.match(harness.element('session-table-body').textContent, /Keep usage history/);
  button('Delete').click(); await flush();
  harness.dom.window.document.querySelector('[data-session-state="deleted"]').click();
  assert.equal(harness.element('session-table-body').querySelector('.session-title').disabled, true);
  assert.match(harness.element('project-usage').textContent, /1,050/);
  button('Restore').click(); await flush();
  harness.dom.window.document.querySelector('[data-session-state="active"]').click();
  assert.equal(harness.element('sidebar-session-list').children.length, 1);
  assert.match(harness.element('project-usage').textContent, /1,050/);
  harness.dom.window.close();
});

test('project settings use a modal, save scoped values and hide a native terminal while open', async () => {
  let current = { ...project };
  const harness = await interfaceHarness({ prepare_project: () => ({ project: current, plan, fingerprint: 'plan-a', candidates: [], message: null }), save_project_preferences: ({ identifier, name, options }) => { assert.equal(identifier, project.id); assert.equal(options.cpus, 2); assert.equal(options.memoryMb, 2048); current = { ...project, name, stage: 'ready' }; return current; } });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  assert.equal(harness.element('project-settings-dialog').open, true);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_layout').at(-1).args.layout.visible, false);
  harness.element('close-project-settings').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'terminal_layout').at(-1).args.layout.visible, true);
  harness.element('hide-terminal').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  harness.element('project-name').value = 'Renamed project'; harness.element('project-cpus').value = '2'; harness.element('project-memory').value = '2048';
  harness.element('project-preferences').dispatchEvent(new harness.dom.window.Event('submit', { bubbles: true, cancelable: true })); await flush();
  assert.equal(harness.element('project-heading').textContent, 'Renamed project');
  assert.equal(harness.element('project-overview').hidden, true);
  assert.equal(harness.element('project-setup').hidden, false);
  harness.dom.window.close();
});

test('usage notifications update cumulative totals and prompt details retain unpriced requests', async () => {
  const conversation = { id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7', title: 'Tracked chat', updatedAt: 2000, state: 'active' };
  const unpriced = { at: '2026-11-10T20:00:00Z', model: null, tokens: { input: 1000, cached: 0, cacheWrite: 0, output: 50 }, costUsd: null, unpricedReason: 'Model price is unavailable', rates: null };
  const priced = { ...unpriced, model: 'gpt-6.1-sol', tokens: { input: 12000, cached: 3000, cacheWrite: 200, output: 80 }, costUsd: 0.012345, unpricedReason: null, rates: { tier: 'standard', tierAssumed: true, date: '2026-11-10' } };
  const turns = [
    { id: 'one', status: 'complete', requests: [unpriced, priced] },
    { id: 'two', status: 'complete', requests: [] },
    { id: 'three', status: 'interrupted', requests: [{ ...unpriced, tokens: { ...unpriced.tokens, cacheKnown: false } }] },
  ];
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard([conversation]), conversation_usage: ({ identifier, sessionId }) => { assert.equal(identifier, project.id); assert.equal(sessionId, conversation.id); return sessionDetails(sessionId, turns); } });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, sessionDashboard([conversation], { usage: { ...emptyUsage, input: 1000, output: 50, total: 1050, requests: 1, unpricedRequests: 1 } })] });
  assert.match(harness.element('project-usage').textContent, /1,050/);
  assert.match(harness.element('project-usage').textContent, /Unavailable/);
  assert.doesNotMatch(harness.element('project-usage').textContent, /\$0/);
  harness.element('session-table-body').querySelector('[aria-label="Usage for Tracked chat"]').click(); await flush();
  assert.equal(harness.element('session-usage-dialog').open, true);
  const details = harness.element('session-usage-details');
  const tables = [...details.querySelectorAll('table')];
  assert.deepEqual(tables.map(table => harness.element(table.getAttribute('aria-labelledby')).textContent), ['Prompt 1 · complete', 'Prompt 3 · interrupted']);
  const [first, last] = tables;
  assert.deepEqual([...first.tHead.querySelectorAll('th[scope="col"]')].map(th => th.textContent), ['Model', 'Input', 'Cached input', 'Cache writes', 'Output', 'API cost']);
  const [unknown, known] = first.tBodies[0].rows;
  assert.equal(unknown.querySelector('th[scope="row"]').textContent, 'Unknown model');
  assert.equal(unknown.cells[2].textContent, '0 (0%)');
  assert.equal(unknown.cells[5].querySelector('small').textContent, 'Model price is unavailable');
  assert.deepEqual([...known.cells].map(cell => cell.firstChild.textContent), ['gpt-6.1-sol', '12,000', '3,000 (25%)', '200', '80', '$0.012345']);
  assert.equal(known.cells[0].querySelector('small').textContent, 'standard tier (assumed)');
  assert.equal(known.cells[5].querySelector('time').dateTime, '2026-11-10');
  assert.equal(first.tFoot.querySelector('td').textContent, '$0.0123 + unpriced');
  assert.equal(last.tBodies[0].rows[0].cells[2].textContent, 'Unknown');
  assert.equal(last.tFoot.querySelector('td').textContent, 'Unavailable');

  harness.element('close-session-usage').click();
  harness.handlers.conversation_usage = ({ sessionId }) => sessionDetails(sessionId, []);
  harness.element('session-table-body').querySelector('[aria-label="Usage for Tracked chat"]').click(); await flush();
  assert.equal(details.children.length, 0);
  assert.match(harness.element('session-usage-status').textContent, /No prompts have been recorded/);
  harness.dom.window.close();
});

test('a partially readable session shows its own warning while healthy chats remain available', async () => {
  const healthy = { id: 'healthy', title: 'Healthy chat', updatedAt: 2000, available: true };
  const damaged = { id: 'damaged', title: 'Damaged chat', updatedAt: 1000, available: true, warning: 'Usage partially read: Missing input token count.' };
  const harness = await interfaceHarness({ project_dashboard: () => ({ ...sessionDashboard([healthy, damaged]), warnings: ['Some saved session names could not be read.'] }) });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const rows = [...harness.element('session-table-body').rows];
  assert.equal(rows[0].querySelector('.session-title').disabled, false);
  assert.doesNotMatch(rows[0].textContent, /partially read/);
  assert.match(rows[1].textContent, /Usage partially read/);
  assert.equal(harness.element('dashboard-session-status').hidden, false);
  assert.match(harness.element('dashboard-session-status').textContent, /saved session names/);
  harness.dom.window.close();
});

test('environment completion can update its project while a diagnostic terminal remains selected', async () => {
  let environmentProject = { ...project };
  const harness = await interfaceHarness({ preview_project: () => ({ project: environmentProject, plan, fingerprint: 'plan-a' }) });
  harness.setTerminal({ identifier: 70, projectId: project.id, purpose: 'environment', running: false, exitCode: 0 });
  harness.setTerminal({ identifier: 80, projectId: project.id, purpose: 'doctor', running: true, exitCode: null });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('return-to-terminal').click(); await flush();
  const caption = harness.element('terminal-status').textContent;
  await harness.listeners.get('environment-preparation-finished')({ payload: { project: environmentProject, terminalIdentifier: 70, message: null } });
  await flush();
  assert.equal(harness.element('project-overview').hidden, false);
  assert.equal(harness.element('terminal-status').textContent, caption);
  assert.equal(harness.calls.filter(call => call.command === 'finish_environment').length, 0);
  environmentProject = { ...project, stage: 'ready' };
  await harness.listeners.get('environment-preparation-finished')({ payload: { project: environmentProject, terminalIdentifier: 90, message: 'Codex version could not be verified. Try again.' } });
  await flush();
  assert.equal(harness.element('project-overview').hidden, true);
  assert.equal(harness.element('prepare-environment').hidden, false);
  assert.match(harness.element('preparation-status').textContent, /Try again/);
  await harness.listeners.get('environment-preparation-finished')({ payload: { project, terminalIdentifier: 70, message: null } });
  assert.match(harness.element('preparation-status').textContent, /Try again/);
  harness.dom.window.close();
});

test('a failed initial plan disables New Chat and offers a visible retry using the new fingerprint', async () => {
  let attempts = 0;
  const harness = await interfaceHarness({ prepare_project: () => {
    if (++attempts === 1) throw new Error('The project plan could not be read.');
    return { project, plan, fingerprint: 'recovered-plan', message: null, candidates: [] };
  } });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.equal(harness.element('project-plan-feedback').hidden, false);
  assert.match(harness.element('project-plan-status').textContent, /could not be read/);
  assert.equal(harness.element('start-codex').disabled, true);
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'start_codex_session').length, 0);
  harness.element('retry-project-plan').click(); await flush();
  assert.equal(harness.element('project-plan-feedback').hidden, true);
  assert.equal(harness.element('start-codex').disabled, false);
  harness.element('start-codex').click(); harness.element('start-codex').click(); await flush();
  const starts = harness.calls.filter(call => call.command === 'start_codex_session');
  assert.equal(starts.length, 1);
  assert.equal(starts[0].args.fingerprint, 'recovered-plan');
  harness.dom.window.close();
});

test('a failed plan refresh cannot retain the old launch fingerprint or be reenabled by terminal updates', async () => {
  let fail;
  const harness = await interfaceHarness();
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.handlers.preview_project = () => new Promise((resolve, reject) => { fail = reject; });
  harness.element('refresh-plan').click(); await flush();
  assert.equal(harness.element('start-codex').disabled, true);
  assert.equal(harness.element('sharing-plan').hidden, true);
  fail(new Error('Current plan is unavailable.')); await flush();
  await harness.listeners.get('terminal-sessions-changed')({ payload: project.id });
  assert.equal(harness.element('start-codex').disabled, true);
  assert.equal(harness.element('retry-project-plan').hidden, false);
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'start_codex_session').length, 0);
  harness.handlers.preview_project = () => ({ project, plan, fingerprint: 'new-plan' });
  harness.element('retry-project-plan').click(); await flush();
  assert.equal(harness.element('start-codex').disabled, false);
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.find(call => call.command === 'start_codex_session').args.fingerprint, 'new-plan');
  harness.dom.window.close();
});

test('selecting another project clears sign-in, policy, boundary, operation and preference fields', async () => {
  const second = { ...project, id: 'project-b', name: 'Second project', path: '/fixture/second' };
  const projects = [project, second];
  let releasePreferences;
  const preferences = current => ({ project: current, options: { cpus: 4, memoryMb: 4096, verification: 'require', network: 'isolated' } });
  const harness = await interfaceHarness({
    recent_projects: () => projects,
    open_project: ({ identifier }) => projects.find(current => current.id === identifier),
    prepare_project: ({ identifier }) => ({ project: projects.find(current => current.id === identifier), plan, fingerprint: 'plan-a', message: null, candidates: [] }),
    read_project_preferences: ({ identifier }) => identifier === second.id ? new Promise(resolve => { releasePreferences = resolve; }) : preferences(project),
    login_status: () => ({ method: 'chatgpt', message: 'Account for project A' }),
    read_project_policy: () => ({ document: { project: 'A' }, fingerprint: 'policy-a', editable: true, attached: true, allowedHosts: ['a.example.com'] }),
    run_boundary_tests: () => ({ projectId: project.id, planFingerprint: 'plan-a', passed: true, cleanup: true, results: [{ name: 'A network', target: 'a.example.com', expected: 'Allowed', observed: 'Allowed', outcome: 'pass' }] }),
    preview_removal: () => { throw new Error('Project A removal preview failed'); },
  });
  await harness.ready;
  harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  for (const id of ['check-login', 'read-project-policy', 'run-boundary-tests', 'preview-removal']) harness.element(id).click();
  await flush();
  assert.match(harness.element('login-status').textContent, /project A/);
  assert.match(harness.element('project-policy-document').textContent, /"A"/);
  assert.equal(harness.element('use-brig-boundary').disabled, false);
  assert.match(harness.element('operation-status').textContent, /Project A/);
  harness.element('close-project-settings').click(); harness.element('project-navigation').click(); await flush();
  harness.element('recent-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  assert.equal(harness.element('project-preference-fields').disabled, true);
  assert.equal(harness.element('project-name').value, '');
  assert.doesNotMatch(harness.element('login-status').textContent, /project A/);
  assert.equal(harness.element('project-policy-document').textContent, '');
  assert.equal(harness.element('project-policy-hosts').value, '');
  assert.equal(harness.element('project-policy-fields').disabled, true);
  assert.equal(harness.element('boundary-results').children.length, 0);
  assert.equal(harness.element('boundary-status').textContent, '');
  assert.equal(harness.element('use-brig-boundary').disabled, true);
  assert.equal(harness.element('use-brig-boundary').checked, false);
  assert.equal(harness.element('operation-status').textContent, '');
  releasePreferences(preferences(second)); await flush();
  assert.equal(harness.element('project-name').value, second.name);
  harness.dom.window.close();
});

test('late settings successes and errors cannot overwrite a reopened view or an A to B to A selection', async () => {
  for (const transition of ['reopen', 'project-switch']) for (const outcome of ['success', 'error']) {
    const second = { ...project, id: 'project-b', name: 'Second project', path: '/fixture/second' };
    const projects = [project, second];
    const pending = new Map();
    const defer = name => new Promise((resolve, reject) => pending.set(name, { resolve, reject }));
    const preferences = current => ({ project: current, options: { cpus: 4, memoryMb: 4096, verification: 'require', network: 'isolated' } });
    const policy = label => ({ document: { label }, fingerprint: label, editable: true, attached: true, allowedHosts: [label.toLowerCase() + '.example.com'] });
    const boundary = label => ({ projectId: project.id, planFingerprint: 'plan-a', passed: true, cleanup: true, results: [{ name: label, target: label, expected: 'Allowed', observed: 'Allowed', outcome: 'pass' }] });
    const harness = await interfaceHarness({
      recent_projects: () => projects,
      open_project: ({ identifier }) => projects.find(current => current.id === identifier),
      prepare_project: ({ identifier }) => ({ project: projects.find(current => current.id === identifier), plan, fingerprint: 'plan-a', message: null, candidates: [] }),
      read_project_preferences: () => defer('preferences'), login_status: () => defer('login'),
      read_project_policy: () => defer('policy'), run_boundary_tests: () => defer('boundary'),
      preview_removal: () => defer('preview'), stop_project: () => defer('stop'),
    });
    await harness.ready;
    harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').click(); await flush();
    harness.element('project-settings-button').click(); await flush();
    for (const id of ['check-login', 'read-project-policy', 'run-boundary-tests', 'stop-project', 'preview-removal']) harness.element(id).click();
    await flush();
    assert.equal(pending.size, 6);
    harness.element('close-project-settings').click();
    harness.handlers.read_project_preferences = ({ identifier }) => preferences(projects.find(current => current.id === identifier));
    harness.handlers.login_status = () => ({ method: 'chatgpt', message: 'Fresh account' });
    harness.handlers.read_project_policy = () => policy('Fresh');
    harness.handlers.run_boundary_tests = () => boundary('Fresh');
    if (transition === 'project-switch') {
      harness.element('project-navigation').click(); await flush();
      harness.element('recent-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
      harness.element('project-navigation').click(); await flush();
      harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').click(); await flush();
    }
    harness.element('project-settings-button').click(); await flush();
    for (const id of ['check-login', 'read-project-policy', 'run-boundary-tests']) harness.element(id).click();
    await flush();
    const oldResults = {
      preferences: preferences({ ...project, name: 'OLD project' }),
      login: { method: 'chatgpt', message: 'OLD account' }, policy: policy('OLD'), boundary: boundary('OLD'),
      preview: { reference: 'OLD', workspace: 'OLD', description: 'OLD', fingerprint: 'OLD', external: false, hasEnvironment: true },
      stop: { ...project, stage: 'ready', name: 'OLD project' },
    };
    for (const [name, promise] of pending) {
      if (outcome === 'success') promise.resolve(oldResults[name]);
      else promise.reject(new Error('OLD ' + name + ' failed'));
    }
    await flush();
    assert.equal(harness.element('project-name').value, project.name);
    assert.equal(harness.element('project-preference-fields').disabled, false);
    assert.equal(harness.element('login-status').textContent, 'Fresh account');
    assert.match(harness.element('project-policy-document').textContent, /Fresh/);
    assert.doesNotMatch(harness.element('project-policy-document').textContent, /OLD/);
    assert.match(harness.element('boundary-results').textContent, /Fresh/);
    assert.doesNotMatch(harness.element('boundary-results').textContent, /OLD/);
    assert.equal(harness.element('operation-status').textContent, '');
    assert.equal(harness.element('removal-dialog').open, false);
    assert.equal(harness.element('project-heading').textContent, project.name);
    harness.dom.window.close();
  }
});

test('a delayed usage response or error cannot replace another conversation or a newer opening of the same one', async () => {
  for (const sameSession of [false, true]) for (const outcome of ['success', 'error']) {
    const first = { id: 'chat-a', title: 'Chat A', updatedAt: 1000 };
    const second = sameSession ? first : { id: 'chat-b', title: 'Chat B', updatedAt: 2000 };
    let resolveOld, rejectOld, requests = 0;
    const request = model => [{ id: 'turn', status: 'complete', requests: [{ at: 'now', model, tokens: { input: 1000, cached: 0, cacheWrite: 0, output: 100 }, costUsd: 0.006, unpricedReason: null, rates: { date: '2026-11-10', tier: 'priority', tierAssumed: false } }] }];
    const harness = await interfaceHarness({
      project_dashboard: () => sessionDashboard(sameSession ? [first] : [first, second]),
      conversation_usage: ({ sessionId }) => ++requests === 1 ? new Promise((resolve, reject) => { resolveOld = value => resolve(sessionDetails(sessionId, value)); rejectOld = reject; }) : sessionDetails(sessionId, request('new-model')),
    });
    await harness.ready; harness.element('choose-folder').click(); await flush();
    harness.element('session-table-body').querySelector('[aria-label="Usage for Chat A"]').click(); await flush();
    harness.element('close-session-usage').click();
    harness.element('session-table-body').querySelector('[aria-label="Usage for ' + second.title + '"]').click(); await flush();
    const status = harness.element('session-usage-status').textContent;
    if (outcome === 'success') resolveOld(request('old-model'));
    else rejectOld(new Error('Old usage failed'));
    await flush();
    assert.equal(harness.element('session-usage-title').textContent, second.title);
    assert.match(harness.element('session-usage-details').textContent, /new-model/);
    assert.doesNotMatch(harness.element('session-usage-details').textContent, /old-model/);
    assert.equal(harness.element('session-usage-status').textContent, status);
    harness.dom.window.close();
  }
});

test('cache reads and writes display their independent known, zero and unknown values', async () => {
  const conversation = { id: 'chat', title: 'Cache accounting', updatedAt: 1000 };
  const base = { at: 'now', model: 'gpt-6.1-sol', costUsd: null, unpricedReason: 'Cache usage is not recorded', rates: null };
  const tokens = { input: 300, cached: 75, cacheWrite: 0, output: 30, cacheKnown: false, cachedKnown: true, cacheWriteKnown: false };
  const usage = { ...emptyUsage, input: 300, cached: 75, cacheWrite: 0, output: 30, total: 330, requests: 1, unpricedRequests: 1, missingCacheRequests: 1, missingCachedRequests: 0, missingCacheWriteRequests: 1 };
  const harness = await interfaceHarness({
    project_dashboard: () => ({ ...sessionDashboard([{ ...conversation, usage }]), usage }),
    conversation_usage: ({ sessionId }) => sessionDetails(sessionId, [{ id: 'one', status: 'complete', requests: [
      { ...base, tokens },
      { ...base, tokens: { ...tokens, cached: 0, cacheKnown: true, cachedKnown: true, cacheWriteKnown: true } },
      { ...base, tokens: { ...tokens, cached: 0, cacheWrite: 30, cachedKnown: false, cacheWriteKnown: true } },
      { ...base, tokens: { input: 300, cached: 75, cacheWrite: 0, output: 30, cacheKnown: false } },
    ] }]),
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.match(harness.element('project-usage').textContent, /75 \(25%\)/);
  assert.match(harness.element('usage-note').textContent, /Cache writes Unknown/);
  harness.element('session-table-body').querySelector('[aria-label="Usage for Cache accounting"]').click(); await flush();
  const rows = [...harness.element('session-usage-details').querySelectorAll('tbody tr')];
  assert.deepEqual(rows.map(row => [row.cells[2].textContent, row.cells[3].textContent]), [
    ['75 (25%)', 'Unknown'], ['0 (0%)', '0'], ['Unknown', '30'], ['Unknown', 'Unknown'],
  ]);
  assert.equal(rows[0].cells[5].firstChild.textContent, 'Unpriced');
  harness.dom.window.close();
});

test('source availability notifications update resume controls while keeping historical totals', async () => {
  const usage = { ...emptyUsage, input: 1000, output: 100, total: 1100, requests: 1, costUsd: 0.006 };
  const conversation = { id: 'chat', title: 'Available chat', updatedAt: 1000, available: true, usage };
  const dashboard = available => ({ ...sessionDashboard([{ ...conversation, available }]), usage });
  const harness = await interfaceHarness({ project_dashboard: () => dashboard(true) });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const original = harness.element('project-usage').textContent;
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, dashboard(false)] });
  assert.equal(harness.element('session-table-body').querySelector('.session-title').disabled, true);
  assert.equal(harness.element('sidebar-session-list').querySelector('button').disabled, true);
  assert.match(harness.element('session-table-body').textContent, /Session file unavailable/);
  assert.equal(harness.element('project-usage').textContent, original);
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, dashboard(true)] });
  assert.equal(harness.element('session-table-body').querySelector('.session-title').disabled, false);
  assert.equal(harness.element('sidebar-session-list').querySelector('button').disabled, false);
  assert.equal(harness.element('project-usage').textContent, original);
  assert.equal(harness.calls.filter(call => call.command === 'project_dashboard').length, 1);
  harness.dom.window.close();
});

test('network settings and execution plans distinguish guest traffic from hosted Codex tools', async () => {
  const harness = await interfaceHarness();
  await harness.ready;
  assert.match(harness.element('network-hint').textContent, /guest command traffic/);
  assert.match(harness.element('network-hint').textContent, /Hosted Codex tools/);
  assert.match(harness.element('network-hint').textContent, /does not read or set the web search mode/);
  harness.element('network-preference').value = 'offline';
  harness.element('network-preference').dispatchEvent(new harness.dom.window.Event('change'));
  assert.match(harness.element('network-hint').textContent, /Offline blocks guest command traffic/);
  assert.match(harness.element('network-hint').textContent, /separate scope/);
  harness.element('choose-folder').click(); await flush();
  assert.match(harness.element('sharing-plan-values').textContent, /Allowed guest destinations/);
  assert.match(harness.element('sharing-plan-values').textContent, /Mode not read or managed by Tugboat/);
  harness.dom.window.close();
});

test('inconclusive or incomplete boundary reports cannot enable broader guest access', async () => {
  const report = { projectId: project.id, planFingerprint: 'plan-a', passed: true, cleanup: true, results: [{ name: 'Network', target: 'example.com', expected: 'Denied', observed: 'Unknown', outcome: 'inconclusive' }] };
  const harness = await interfaceHarness({ run_boundary_tests: () => report });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  for (const [results, cleanup] of [[report.results, true], [[], true], [[{ ...report.results[0], outcome: 'pass' }], false]]) {
    report.results = results; report.cleanup = cleanup;
    harness.element('run-boundary-tests').click(); await flush();
    assert.equal(harness.element('use-brig-boundary').disabled, true);
    assert.match(harness.element('boundary-status').textContent, /could not be verified/);
  }
  harness.dom.window.close();
});

test('a failed process close keeps the terminal visible and permits a retry', async () => {
  let fail = true;
  const harness = await interfaceHarness({ terminal_close: ({ identifier }) => {
    if (fail) throw new Error('Still running');
    harness.terminals.delete(identifier);
  } });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('close-terminal').click(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.equal(harness.terminals.size, 1);
  assert.match(harness.element('terminal-status').textContent, /Could not end the process.*Still running/);
  fail = false; harness.element('close-terminal').click(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.terminals.size, 0);
  harness.dom.window.close();
});

test('local, guest and login actions explain a conflicting process without entering the chat', async () => {
  for (const [button, status] of [['start-terminal', 'local-terminal-status'], ['guest-shell', 'login-status'], ['codex-login', 'login-status']]) {
    const harness = await interfaceHarness();
    await harness.ready; harness.element('choose-folder').click(); await flush();
    harness.element('start-codex').click(); await flush();
    harness.element('project-settings-button').click(); await flush();
    const focusCalls = harness.calls.filter(call => call.command === 'terminal_focus').length;
    harness.element(button).focus(); harness.element(button).click(); await flush();
    assert.equal(harness.element('project-settings-dialog').open, true);
    assert.match(harness.element(status).textContent, /End the running Codex chat before opening/);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').length, focusCalls);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_start').length, 0);
    assert.equal(harness.calls.filter(call => call.command === 'start_codex_session').length, 1);
    assert.equal(harness.dom.window.document.activeElement, harness.element(button));
    harness.dom.window.close();
  }
});

test('matching local, guest and login actions restore their own terminal only', async () => {
  for (const [button, purpose] of [['start-terminal', 'local'], ['guest-shell', 'shell'], ['codex-login', 'login']]) {
    const harness = await interfaceHarness();
    await harness.ready; harness.element('choose-folder').click(); await flush();
    harness.setTerminal({ identifier: 70, projectId: project.id, purpose, running: true, exitCode: null, conversationId: null, busy: false });
    await harness.listeners.get('terminal-sessions-changed')({ payload: project.id });
    harness.element('project-settings-button').click(); await flush();
    harness.element(button).click(); await flush();
    assert.equal(harness.element('project-settings-dialog').open, false);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, 70);
    assert.equal(harness.calls.filter(call => ['terminal_start', 'start_codex_session'].includes(call.command)).length, 0);
    harness.dom.window.close();
  }
});

test('completed test output stays reachable after a new chat opens and closes explicitly', async () => {
  const harness = await interfaceHarness();
  harness.handlers.start_project_operation = () => {
    harness.setTerminal({ identifier: 20, projectId: project.id, purpose: 'tests', running: true, exitCode: null, conversationId: null, busy: false });
    return 20;
  };
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('project-tests').click(); await flush();
  harness.setTerminal({ ...harness.terminals.get(20), running: false, exitCode: 2 });
  await harness.listeners.get('terminal-exit')({ payload: { identifier: 20, projectId: project.id, purpose: 'tests', code: 2 } }); await flush();
  harness.element('hide-terminal').click(); await flush();
  harness.element('start-codex').click(); await flush();
  const output = harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="20"]');
  assert.match(output.textContent, /Project tests.*exit 2/);
  output.click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, 20);
  assert.equal(harness.calls.filter(call => call.command === 'start_project_operation').length, 1);
  assert.match(harness.element('terminal-status').textContent, /Project tests.*exit 2/);
  assert.equal(harness.element('close-terminal').textContent, 'Close output');
  harness.element('close-terminal').click(); await flush();
  assert.equal(harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="20"]'), null);
  assert.equal(harness.terminals.size, 2);
  harness.dom.window.close();
});

test('an unavailable library record can be forgotten without opening or removing its Brig environment', async () => {
  const missing = { ...project, available: false };
  const other = { ...project, id: 'other', name: 'Available', available: true };
  let records = [missing, other];
  const harness = await interfaceHarness({ recent_projects: () => records, forget_project: ({ identifier }) => { records = records.filter(record => record.id !== identifier); } });
  await harness.ready;
  const forget = () => harness.element('recent-project-list').querySelector('.project-record-forget');
  assert.equal(harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').disabled, true);
  assert.match(harness.element('recent-project-list').textContent, /Folder unavailable/);
  assert.equal(records.length, 2);
  forget().click(); await flush();
  assert.match(harness.element('forget-project-dialog').textContent, /Project files, Brig environments, guest homes and saved usage stay on disk/);
  harness.element('cancel-forget-project').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'forget_project').length, 0);
  forget().click(); harness.element('confirm-forget-project').click(); await flush();
  assert.equal(harness.element('forget-project-dialog').open, false);
  assert.equal(forget(), null);
  assert.deepEqual(harness.calls.find(call => call.command === 'forget_project').args.identifier, project.id);
  assert.deepEqual(records.map(record => record.id), ['other']);
  assert.equal(harness.calls.filter(call => ['open_project', 'preview_removal', 'remove_project', 'prepare_project', 'terminal_close'].includes(call.command)).length, 0);
  harness.dom.window.close();
});

test('incomplete removal opens recovery with its saved sign-out choice and keeps chat disabled', async () => {
  const pending = { ...project, stage: 'removing' };
  const harness = await interfaceHarness({
    register_project: () => pending,
    prepare_project: () => ({ project: pending, plan: null, fingerprint: null, candidates: [], message: 'Removal is incomplete. Review the removal preview to continue.' }),
    preview_removal: () => ({ reference: 'codex@sample-a', workspace: '/fixture/guest-home', description: 'Continue incomplete removal. Environment removal complete; policy cleanup pending.', fingerprint: 'recovery', external: false, hasEnvironment: false, logout: true }),
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.equal(harness.element('resume-removal').hidden, false);
  assert.equal(harness.element('start-codex').disabled, true);
  assert.match(harness.element('project-plan-status').textContent, /Removal is incomplete/);
  harness.element('resume-removal').click(); await flush();
  assert.equal(harness.element('removal-dialog').open, true);
  assert.match(harness.element('removal-description').textContent, /policy cleanup pending/);
  assert.equal(harness.element('logout-on-removal').checked, true);
  assert.equal(harness.element('logout-on-removal').disabled, true);
  assert.equal(harness.calls.filter(call => ['remove_project', 'start_codex_session', 'prepare_environment'].includes(call.command)).length, 0);
  harness.dom.window.close();
});

test('every recorded prompt remains visible when request measurements are missing', async () => {
  const conversation = { id: 'chat', title: 'Interrupted prompt', updatedAt: 1000 };
  let turns = [
    { id: 'first', status: 'interrupted', requests: [] },
    { id: 'second', status: 'complete', requests: [{ at: 'now', model: 'gpt-6.1-sol', tokens: { input: 100, cached: 0, cacheWrite: 0, output: 10, cacheKnown: true }, costUsd: 0.0003, unpricedReason: null, rates: null }] },
  ];
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard([conversation]), conversation_usage: ({ sessionId }) => sessionDetails(sessionId, turns) });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const open = async () => { harness.element('session-table-body').querySelector('[aria-label="Usage for Interrupted prompt"]').click(); await flush(); };
  await open();
  const sections = harness.element('session-usage-details').querySelectorAll('section[data-agent-turn]');
  assert.equal(sections.length, 2);
  assert.match(sections[0].textContent, /Prompt 1 · interrupted.*No request usage.*API cost are unknown/s);
  assert.doesNotMatch(sections[0].textContent, /\$0|Total API cost/);
  assert.match(sections[1].textContent, /Prompt 2 · complete/);
  assert.match(sections[1].textContent, /\$0\.000300/);
  harness.element('close-session-usage').click(); turns = turns.slice(0, 1); await open();
  assert.match(harness.element('session-usage-status').textContent, /Prompts are recorded, but no request usage/);
  harness.element('close-session-usage').click(); turns = []; await open();
  assert.match(harness.element('session-usage-status').textContent, /No prompts have been recorded/);
  harness.dom.window.close();
});

test('sidebar focus survives usage and terminal refreshes without taking focus from project settings', async () => {
  const first = { id: 'first', title: 'First', updatedAt: 2 };
  const second = { id: 'second', title: 'Second', updatedAt: 1 };
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard([first, second]) });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const row = id => harness.element('sidebar-session-list').querySelector('[data-session-id="' + id + '"]');
  row('first').focus();
  assert.equal(harness.dom.window.document.activeElement, row('first'), 'Initial focus: ' + harness.dom.window.document.activeElement.outerHTML);
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, sessionDashboard([{ ...first, title: 'First renamed' }, second])] });
  assert.equal(harness.dom.window.document.activeElement, row('first'), 'Updated focus: ' + harness.dom.window.document.activeElement.outerHTML);
  row('first').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'start_codex_session').at(-1).args.sessionId, 'first');
  harness.setTerminal({ identifier: 90, projectId: project.id, purpose: 'tests', running: false, exitCode: 0, conversationId: null, busy: false });
  await harness.listeners.get('terminal-sessions-changed')({ payload: project.id });
  const output = () => harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="90"]');
  output().focus(); await harness.listeners.get('terminal-sessions-changed')({ payload: project.id });
  assert.equal(harness.dom.window.document.activeElement, output());
  harness.element('project-settings-button').click(); await flush();
  harness.element('project-name').focus();
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, sessionDashboard([first, second])] });
  assert.equal(harness.dom.window.document.activeElement, harness.element('project-name'));
  harness.element('close-project-settings').click(); await flush();
  row('second').focus();
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, sessionDashboard([first, { ...second, state: 'archived' }])] });
  assert.equal(harness.dom.window.document.activeElement, row('first'));
  harness.terminals.clear();
  await harness.listeners.get('terminal-sessions-changed')({ payload: project.id });
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, sessionDashboard([])] });
  assert.equal(harness.dom.window.document.activeElement, harness.element('refresh-sessions'));
  harness.dom.window.close();
});

test('cancelled Mac checks keep the labelled previous result and a second check can succeed', async () => {
  const valid = { architecture: 'arm64', osVersion: '27.2', tools: { brig: '/fixture/brig', hull: '/fixture/hull', homebrew: '/fixture/brew', git: '/usr/bin/git' }, brigStatus: 'installed', version: { data: { version: 'v0.5.2' } }, doctor: { data: [{ name: 'runtime', state: 'ok', finding: 'Available' }] }, message: null };
  let count = 0, cancelCheck;
  const harness = await interfaceHarness({
    inspect_host: () => ++count === 2 ? new Promise(resolve => { cancelCheck = resolve; }) : ({ ...valid, osVersion: count === 1 ? '27.2' : '27.3' }),
    cancel_inspection: () => cancelCheck({ ...valid, brigStatus: 'cancelled', version: null, doctor: null, tools: {}, message: 'Cancelled' }),
  });
  await harness.ready;
  const tools = harness.element('host-tools').textContent, doctor = harness.element('host-doctor').textContent;
  harness.element('check-host').click(); await flush();
  assert.equal(harness.element('host-status').textContent, 'Checking this Mac…');
  harness.element('cancel-host').click(); await flush();
  assert.match(harness.element('host-status').textContent, /Mac check cancelled.*previous completed check.*v0\.5\.2/s);
  assert.doesNotMatch(harness.element('host-status').textContent, /Unsupported/);
  assert.equal(harness.element('host-tools').textContent, tools);
  assert.equal(harness.element('host-doctor').textContent, doctor);
  assert.equal(harness.element('check-host').disabled, false);
  harness.element('check-host').click(); await flush();
  assert.match(harness.element('host-status').textContent, /Brig installed.*27\.3/);
  assert.doesNotMatch(harness.element('host-status').textContent, /previous completed/);
  harness.dom.window.close();
});

test('Settings displays the native package version without a version in the HTML', async () => {
  const harness = await interfaceHarness({ application_version: () => '9.8.7' });
  await harness.ready;
  assert.equal(harness.element('application-version').textContent, '9.8.7');
  assert.match(fs.readFileSync(root + 'index.html', 'utf8'), /id="application-version">Loading…/);
  harness.dom.window.close();
});

test('working tree and staged diff controls launch separately labelled guest operations', async () => {
  const harness = await interfaceHarness();
  let identifier = 60;
  harness.handlers.start_project_operation = ({ operation }) => {
    harness.setTerminal({ identifier: ++identifier, projectId: project.id, purpose: operation, running: false, exitCode: 0, conversationId: null, busy: false });
    return identifier;
  };
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('git-diff').click(); await flush();
  harness.element('git-staged-diff').click(); await flush();
  const calls = harness.calls.filter(call => call.command === 'start_project_operation');
  assert.deepEqual(calls.map(call => call.args.operation), ['git-diff', 'git-staged-diff']);
  for (const call of calls) { assert.equal(call.args.identifier, project.id); assert.equal(call.args.fingerprint, 'plan-a'); }
  assert.match(harness.element('sidebar-terminal-list').textContent, /Working tree diff.*Staged diff/);
  harness.dom.window.close();
});

test('the frozen legacy detail DTO preserves the selected own prompt view', async () => {
  const example = JSON.parse(fs.readFileSync(root + 'tests/fixtures/codex-subagents/contracts/v2-examples.json', 'utf8')).examples.find(example => example.name === 'legacy-single');
  const conversation = example.conversations[0];
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(example.conversations), conversation_usage: ({ identifier, sessionId }) => {
    assert.equal(identifier, project.id); assert.equal(sessionId, conversation.id);
    return structuredClone(example.details);
  } });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('session-table-body').querySelector('[aria-label="Usage for ' + conversation.title + '"]').click(); await flush();
  assert.equal(harness.element('session-usage-dialog').open, true);
  assert.match(harness.element('session-usage-details').textContent, /Prompt 1 · complete/);
  assert.equal(harness.element('session-usage-details').querySelectorAll('tbody tr').length, 1);
  assert.match(harness.element('session-usage-details').textContent, /1,000.*500 \(50%\).*100/s);
  harness.dom.window.close();
});

test('the usage boundary rejects old arrays and a mismatched selected session', async () => {
  const conversation = conversationFixture({ id: 'strict-dto', title: 'Strict DTO', updatedAt: 1000 });
  for (const response of [[], sessionDetails('other-session', [])]) {
    const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard([conversation]), conversation_usage: () => response });
    await harness.ready; harness.element('choose-folder').click(); await flush();
    harness.element('session-table-body').querySelector('[aria-label="Usage for Strict DTO"]').click(); await flush();
    assert.match(harness.element('session-usage-status').textContent, /does not match this session/);
    assert.equal(harness.element('session-usage-details').children.length, 0);
    harness.dom.window.close();
  }
});

const subagentContractExamples = JSON.parse(fs.readFileSync(root + 'tests/fixtures/codex-subagents/contracts/v2-examples.json', 'utf8')).examples;
const contractExample = name => structuredClone(subagentContractExamples.find(example => example.name === name));

test('family grouping uses one main row and a read-only unlinked recovery area', async () => {
  const family = contractExample('root-two-children-grandchild-followup');
  const orphan = contractExample('orphan');
  const legacy = conversationFixture({ id: 'legacy', title: 'Legacy chat', updatedAt: 1000 });
  const invalid = conversationFixture({ ...orphan.conversations[0], id: 'invalid', title: 'Invalid child', relationStatus: 'invalid', warning: 'Parent relationship conflicts.' });
  family.conversations[1].state = 'deleted';
  const records = [...family.conversations, ...orphan.conversations, legacy, invalid];
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(records, { usage: family.projectUsage, coverage: 'partial' }), conversation_usage: () => orphan.details });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const rows = [...harness.element('session-table-body').rows];
  assert.deepEqual(rows.map(row => row.dataset.sessionId), [family.details.sessionId, legacy.id]);
  assert.deepEqual([...harness.element('sidebar-session-list').querySelectorAll('button')].map(button => button.dataset.sessionId), [family.details.sessionId, legacy.id]);
  assert.match(rows[0].textContent, /Includes 3 subagents/);
  assert.match(rows[0].cells[4].textContent, /24\.2K/);
  assert.match(rows[0].cells[2].textContent, /63\.6%/);
  assert.doesNotMatch(rows[1].textContent, /subagent|relationship/i);
  assert.match(harness.element('project-usage').textContent, /24,200/);
  assert.equal(harness.element('unlinked-subagents').hidden, false);
  const unlinked = [...harness.element('unlinked-subagent-body').rows];
  assert.equal(unlinked.length, 2);
  assert.match(unlinked[0].textContent, /Parent session unavailable/);
  assert.match(unlinked[1].textContent, /Invalid session relationship.*Parent relationship conflicts/s);
  for (const row of unlinked) {
    assert.equal(row.querySelector('.session-title').tagName, 'SPAN');
    assert.deepEqual([...row.querySelectorAll('button')].map(button => button.getAttribute('aria-label')), ['Usage for ' + row.querySelector('.session-title').textContent]);
  }
  unlinked[0].querySelector('button').click(); await flush();
  assert.equal(harness.calls.find(call => call.command === 'conversation_usage').args.sessionId, orphan.details.sessionId);
  assert.equal(harness.calls.filter(call => call.command === 'start_codex_session').length, 0);
  harness.dom.window.close();
});

test('main session counts use singular and plural subagent labels in the table and sidebar tooltip', async () => {
  const conversations = [0, 1, 3].map(subagentCount => conversationFixture({ id: 'main-' + subagentCount, title: 'Main with ' + subagentCount, updatedAt: 1000, subagentCount }));
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(conversations) });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  for (const conversation of conversations) {
    const date = harness.element('session-table-body').querySelector('[data-session-id="' + conversation.id + '"] .session-date');
    const button = harness.element('sidebar-session-list').querySelector('[data-session-id="' + conversation.id + '"]');
    const suffix = conversation.subagentCount === 1 ? 'Includes 1 subagent' : 'Includes 3 subagents';
    if (!conversation.subagentCount) {
      assert.doesNotMatch(date.textContent, /Includes/); assert.equal(button.title, conversation.title);
      continue;
    }
    assert.ok(date.textContent.endsWith(suffix)); assert.equal(button.title, conversation.title + '\n' + suffix);
  }
  harness.dom.window.close();
});

test('root archive, Trash and restore preserve descendant raw states and unique project totals', async () => {
  const family = contractExample('root-two-children-grandchild-followup');
  const main = family.conversations[0];
  family.conversations[1].state = 'deleted'; family.conversations[2].state = 'archived';
  const childStates = family.conversations.slice(1).map(conversation => conversation.state);
  const changedIds = [];
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(family.conversations, { usage: family.projectUsage }), change_conversation_state: ({ sessionId, state }) => {
    changedIds.push(sessionId); family.conversations.find(conversation => conversation.id === sessionId).state = state;
  } });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const projectTotal = harness.element('project-usage').textContent;
  const action = label => [...harness.element('session-table-body').querySelectorAll('button')].find(button => button.getAttribute('aria-label') === label + ' ' + main.title);
  action('Archive').click(); await flush();
  assert.equal(harness.element('sidebar-session-list').children.length, 0);
  harness.dom.window.document.querySelector('[data-session-state="archived"]').click();
  assert.equal(harness.element('session-table-body').rows.length, 1);
  action('Delete').click(); await flush();
  harness.dom.window.document.querySelector('[data-session-state="deleted"]').click();
  assert.equal(harness.element('session-table-body').rows.length, 1);
  action('Restore').click(); await flush();
  harness.dom.window.document.querySelector('[data-session-state="active"]').click();
  assert.equal(harness.element('sidebar-session-list').children.length, 1);
  assert.deepEqual(changedIds, [main.id, main.id, main.id]);
  assert.deepEqual(family.conversations.slice(1).map(conversation => conversation.state), childStates);
  assert.equal(harness.element('project-usage').textContent, projectTotal);
  harness.dom.window.close();
});

test('a resolved child terminal identity restores the running root chat and keeps its activity state', async () => {
  const family = contractExample('root-two-children-grandchild-followup');
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(family.conversations) });
  harness.setTerminal({ identifier: 77, projectId: project.id, purpose: 'resume', running: true, exitCode: null, conversationId: family.conversations[1].id, busy: true });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const chat = harness.element('sidebar-session-list').querySelector('button');
  assert.equal(harness.element('sidebar-session-list').children.length, 1);
  assert.equal(chat.dataset.sessionId, family.conversations[0].id);
  assert.ok(chat.querySelector('.session-activity'));
  chat.click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'start_codex_session').length, 0);
  assert.ok(harness.calls.some(call => call.command === 'terminal_focus' && call.args.identifier === 77));
  assert.equal(harness.element('sidebar-session-list').querySelector('[aria-current="true"]').dataset.sessionId, family.conversations[0].id);
  harness.dom.window.close();
});

test('family detail scopes and prompt source links retain each agent request exactly once', async () => {
  const family = contractExample('root-two-children-grandchild-followup');
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(family.conversations), conversation_usage: () => family.details });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('session-table-body').querySelector('[aria-label="Usage for Primary R"]').click(); await flush();
  const details = harness.element('session-usage-details');
  const scopes = [...details.querySelector('.usage-scope-table').tBodies[0].rows];
  assert.deepEqual(scopes.map(row => [row.cells[0].textContent, row.cells[5].textContent]), [['Main agent', '11,000'], ['Subagents', '13,200'], ['Combined', '24,200']]);
  assert.equal(scopes[2].cells[2].textContent, '14,000 (63.6%)');
  assert.match(harness.element('session-usage-status').textContent, /2 user prompt\(s\).*6 agent turn\(s\)/);
  const own = [...details.querySelectorAll('section[data-agent-turn="' + family.details.sessionId + '"]')];
  assert.deepEqual(own.map(section => section.querySelector('h3').textContent), ['Prompt 1 · complete', 'Prompt 2 · complete']);
  assert.match(own[0].textContent, /Subagents: 11,000 tokens.*Combined: 16,500 tokens/s);
  assert.match(own[1].textContent, /Subagents: 2,200 tokens.*Combined: 7,700 tokens/s);
  const requests = [...details.querySelectorAll('.request-usage tbody tr')];
  assert.equal(requests.length, 6); assert.equal(new Set(requests.map(row => row.id)).size, 6);
  const request = family.details.agents[0].turns[0].requests[0];
  const model = requests[0].cells[0];
  for (const value of Object.values(request.source)) if (value !== null) assert.ok(model.title.includes(value));
  assert.ok(model.title.includes(request.key));
  assert.doesNotMatch(model.textContent, /Source turn:|Response:/);
  assert.ok(!model.textContent.includes(request.source.originTurnId));
  assert.ok(!model.textContent.includes(request.source.responseId));
  assert.equal(details.querySelectorAll('details.usage-agent').length, 3);
  assert.equal(details.querySelectorAll('.usage-agent h3').length, 0);
  assert.ok([...details.querySelectorAll('.usage-agent h4')].every(heading => heading.textContent.startsWith('Agent turn ')));
  const refs = [...details.querySelectorAll('.usage-source-list a')];
  assert.equal(refs.length, 6);
  for (const link of refs) assert.ok(harness.element(decodeURIComponent(new URL(link.href).hash.slice(1))));
  const mainLink = refs.find(link => link.textContent === 'Main agent · agent turn 1 · request 1');
  assert.ok(mainLink); assert.ok(mainLink.title.includes(family.details.agents[0].title)); assert.ok(mainLink.title.includes(request.key));
  const childLink = refs.find(link => link.textContent.startsWith('Grandchild C'));
  childLink.click(); await flush();
  const target = harness.element(decodeURIComponent(new URL(childLink.href).hash.slice(1)));
  assert.equal(target.closest('details').open, true);
  assert.equal(harness.dom.window.document.activeElement, target);
  const summary = target.closest('details').querySelector('summary');
  summary.click(); await flush(); assert.equal(target.closest('details').open, false);
  assert.match(harness.element('session-usage-status').textContent, /API equivalent.*subscription billing is separate/);
  harness.dom.window.close();
});

test('selected-child detail keeps its subtree and labels local turns separately from user prompts', async () => {
  const child = contractExample('selected-child-subtree');
  const harness = await interfaceHarness(); await harness.ready;
  harness.renderUsage(child.details);
  const content = harness.element('session-usage-details');
  const scopes = [...content.querySelector('.usage-scope-table').tBodies[0].rows];
  assert.deepEqual(scopes.map(row => [row.cells[0].textContent, row.cells[5].textContent]), [['Selected agent', '4,400'], ['Subagents', '2,200'], ['Combined', '6,600']]);
  assert.equal(content.querySelectorAll('.request-usage tbody tr').length, 3);
  assert.equal(content.querySelectorAll('.usage-agent').length, 1);
  assert.doesNotMatch(content.textContent, /Primary R|Child B|24,200|Prompt 1 ·/);
  assert.match(content.textContent, /Agent turn 1 · complete.*User prompt attribution.*User prompt ·/s);
  assert.ok(content.textContent.includes(child.details.prompts[0].turnId));
  harness.dom.window.close();
});

test('orphan details preserve unknown user prompt count and explicitly unassigned own work', async () => {
  const orphan = contractExample('orphan');
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(orphan.conversations), conversation_usage: () => orphan.details });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('unlinked-subagent-body').querySelector('button').click(); await flush();
  assert.match(harness.element('session-usage-status').textContent, /User prompt count unknown.*1 agent turn\(s\).*main session could not be established/s);
  const content = harness.element('session-usage-details');
  assert.match(content.textContent, /Agent turn 1 · complete/);
  assert.doesNotMatch(content.textContent, /Prompt 1 ·/);
  assert.match(content.querySelector('.usage-unassigned').textContent, /Unassigned to a user prompt.*1,100 tokens.*500 \(50%\) cached input/s);
  assert.equal(content.querySelectorAll('.request-usage tbody tr').length, 1);
  assert.equal(content.querySelectorAll('.usage-unassigned a').length, 1);
  harness.dom.window.close();
});

test('unmeasured work, an exact zero request and an empty history have different amounts', async () => {
  for (const name of ['genuine-unmeasured-own-turn', 'valid-zero-token-exact-request', 'empty-history']) {
    const example = contractExample(name);
    const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(example.conversations, { usage: example.details.combinedUsage }), conversation_usage: () => example.details });
    await harness.ready; harness.element('choose-folder').click(); await flush();
    const main = example.conversations[0];
    harness.element('session-table-body').querySelector('[aria-label="Usage for ' + main.title + '"]').click(); await flush();
    const report = harness.element('session-usage-details');
    if (name === 'genuine-unmeasured-own-turn') {
      assert.match(harness.element('project-usage').textContent, /Total tokensUnknown.*InputUnknown.*Total API costUnknown/);
      assert.doesNotMatch(harness.element('session-table-body').textContent, /\$0/);
      assert.match(report.textContent, /Token counts and API cost are unknown/);
      assert.equal(report.querySelectorAll('.request-usage').length, 0);
    }
    if (name === 'valid-zero-token-exact-request') {
      assert.match(harness.element('project-usage').textContent, /Total tokens0.*Total API cost\$0\.00/);
      assert.match(report.textContent, /0 \(0%\).*\$0\.000000/s);
      assert.equal(report.querySelectorAll('.request-usage tbody tr').length, 1);
    }
    if (name === 'empty-history') {
      assert.equal(report.children.length, 0);
      assert.match(harness.element('session-usage-status').textContent, /No prompts have been recorded/);
      assert.match(harness.element('project-usage').textContent, /Total API cost\$0\.00/);
    }
    harness.dom.window.close();
  }
});

test('partial family summaries suppress complete cache percentages and retain safe Unicode agent metadata', async () => {
  const family = contractExample('root-two-children-grandchild-followup');
  const title = '<img src=x onerror=alert(1)> 日本語 📚 ' + 'Uzun Türkçe görev '.repeat(12);
  family.details.agents[1].title = title;
  family.details.combinedUsage = { ...family.details.combinedUsage, unmeasuredTurns: 1 };
  family.details.coverage = 'partial';
  const harness = await interfaceHarness(); await harness.ready; harness.renderUsage(family.details);
  const content = harness.element('session-usage-details');
  const combined = content.querySelector('.usage-scope-table').tBodies[0].rows[2];
  assert.equal(combined.cells[1].textContent, '22,000 + unknown');
  assert.equal(combined.cells[2].textContent, '14,000 + unknown');
  assert.equal(combined.cells[5].textContent, '24,200 + unknown');
  assert.equal(combined.cells[6].textContent, 'Unavailable');
  assert.equal(content.querySelector('img'), null);
  assert.ok(content.querySelector('.usage-agent summary').textContent.includes(title));
  assert.ok(content.querySelector('.usage-source-list a').textContent.startsWith('Main agent'));
  assert.ok(content.querySelector('.usage-source-list a').title.includes('Primary R'));
  harness.dom.window.close();
});

test('request rows retain mixed models, independent cache knownness and both dated correction histories', async () => {
  const fork = contractExample('normal-fork-inherited-amount-correction');
  const corrected = fork.details.agents[0].turns[0].requests[0];
  const edge = JSON.parse(fs.readFileSync(root + 'tests/fixtures/codex-subagents/contracts/v2-examples.json', 'utf8')).correctionEdgeExamples[0].request;
  const mixed = { ...corrected, key: 'other-model', model: 'second-model', tokens: { ...corrected.tokens, input: 500, cached: 0, cacheWrite: 25, cachedKnown: false, cacheWriteKnown: true }, rates: { ...corrected.rates, date: '2025-10-01', tier: 'priority' }, amountCorrection: null, costUsd: 0.123 };
  const conversation = conversationFixture({ id: 'mixed', title: 'Mixed snapshots', updatedAt: 1000 });
  const details = sessionDetails(conversation.id, [{ id: 'one', status: 'complete', requests: [corrected, edge, mixed] }]);
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard([conversation]), conversation_usage: () => details });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('session-table-body').querySelector('[aria-label="Usage for Mixed snapshots"]').click(); await flush();
  const rows = [...harness.element('session-usage-details').querySelectorAll('.request-usage tbody tr')];
  assert.equal(rows.length, 3);
  assert.match(rows[0].textContent, /Amount correction:.*139,/s);
  assert.match(rows[1].cells[5].textContent, /Unpriced.*compatible dated rate is unavailable.*Rates 2026-11-10.*Earlier independent recorded-tier correction retained.*priority rates 2026-11-10.*Amount correction:/s);
  assert.equal(rows[2].cells[2].textContent, 'Unknown'); assert.equal(rows[2].cells[3].textContent, '25');
  assert.match(rows[2].cells[0].textContent, /second-model.*priority tier/s);
  assert.equal(rows[2].cells[5].querySelector('time').dateTime, '2025-10-01');
  assert.match(harness.element('session-usage-details').querySelector('tfoot').textContent, /unpriced/);
  harness.dom.window.close();
});

test('equal local turn IDs in different agents retain separate tables and source targets', async () => {
  const family = contractExample('root-two-children-grandchild-followup');
  const children = [family.details.agents[1], family.details.agents[3]];
  for (const agent of children) {
    const oldId = agent.turns[0].id; agent.turns[0].id = 'shared-local-turn';
    for (const request of agent.turns[0].requests) if (request.source) request.source.originTurnId = 'shared-local-turn';
    for (const prompt of family.details.prompts) for (const ref of prompt.requestRefs) if (ref.sessionId === agent.sessionId && ref.turnId === oldId) ref.turnId = 'shared-local-turn';
  }
  const harness = await interfaceHarness(); await harness.ready; harness.renderUsage(family.details);
  const content = harness.element('session-usage-details');
  const sections = [...content.querySelectorAll('section[data-agent-turn]')].filter(section => section.id.includes('shared-local-turn'));
  assert.equal(sections.length, 2); assert.notEqual(sections[0].id, sections[1].id);
  assert.notEqual(sections[0].querySelector('h4').id, sections[1].querySelector('h4').id);
  const sourceTargets = [...content.querySelectorAll('.usage-source-list a')].filter(link => link.title.startsWith('shared-local-turn')).map(link => harness.element(decodeURIComponent(new URL(link.href).hash.slice(1))));
  assert.equal(sourceTargets.length, 2); assert.notEqual(sourceTargets[0], sourceTargets[1]);
  assert.deepEqual(new Set(sourceTargets.map(row => row.closest('section').dataset.agentTurn)), new Set(children.map(agent => agent.sessionId)));
  harness.dom.window.close();
});


test('a running unlinked child stays reachable as a process without a child resume action', async () => {
  const orphan = contractExample('orphan');
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(orphan.conversations) });
  harness.setTerminal({ identifier: 78, projectId: project.id, purpose: 'codex', running: true, exitCode: null, conversationId: orphan.conversations[0].id, busy: true });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const process = harness.element('sidebar-session-list').querySelector('button');
  assert.equal(harness.element('sidebar-session-list').children.length, 1);
  assert.equal(process.dataset.sessionId, 'terminal-78');
  assert.match(process.textContent, /Codex process 78/);
  assert.ok(process.querySelector('.session-activity'));
  assert.equal(harness.element('session-table-body').rows.length, 0);
  assert.equal(harness.element('unlinked-subagent-body').rows.length, 1);
  process.click(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.equal(harness.calls.filter(call => call.command === 'start_codex_session').length, 0);
  assert.ok(harness.calls.some(call => call.command === 'terminal_focus' && call.args.identifier === 78));
  assert.equal(harness.element('sidebar-session-list').querySelector('[aria-current="true"]').dataset.sessionId, 'terminal-78');
  harness.dom.window.close();
});

const publishUsage = async (harness, conversations, overrides = {}) => {
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, sessionDashboard(conversations, overrides)] }); await flush();
};
const openUsage = async (harness, title) => { harness.element('session-table-body').querySelector('[aria-label="Usage for ' + title + '"]').click(); await flush(); };

test('open detail refreshes only for sorted selected-subtree revisions, metadata and membership', async () => {
  const family = contractExample('root-two-children-grandchild-followup');
  const unrelated = conversationFixture({ id: 'unrelated', title: 'Other chat', updatedAt: 1000 });
  let records = [...family.conversations, unrelated];
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(records), conversation_usage: () => structuredClone(family.details) });
  await harness.ready; harness.element('choose-folder').click(); await flush(); await openUsage(harness, 'Primary R');
  const requests = () => harness.calls.filter(call => call.command === 'conversation_usage').length;
  assert.equal(requests(), 1);
  await publishUsage(harness, [...records].reverse()); assert.equal(requests(), 1);
  unrelated.usageRevision = 'other-changed'; unrelated.title = 'Other changed'; unrelated.familyUsage = { ...emptyUsage, input: 55, total: 55 };
  await publishUsage(harness, records, { usage: { ...emptyUsage, total: 999 } }); assert.equal(requests(), 1);
  family.conversations[1].lineage.nickname = '更新したエージェント 📚'; family.details.agents[1].lineage.nickname = '更新したエージェント 📚';
  await publishUsage(harness, records); assert.equal(requests(), 2);
  assert.match(harness.element('session-usage-details').querySelector('.usage-agent summary').textContent, /更新したエージェント 📚/);
  family.conversations[0].title = 'Updated main title'; family.details.agents[0].title = 'Updated main title';
  await publishUsage(harness, records); assert.equal(requests(), 3); assert.equal(harness.element('session-usage-title').textContent, 'Updated main title');
  family.conversations[1].usageRevision = 'child-only-source-revision';
  await publishUsage(harness, records); assert.equal(requests(), 4);
  const added = conversationFixture({ id: 'new-link', title: 'New agent', updatedAt: 1000, lineage: { ...unknownLineage, kind: 'subagent', parentSessionId: 'missing-parent', nickname: 'new', sourceVersion: '0.167.0' }, relationStatus: 'orphan' });
  records = [...records, added]; await publishUsage(harness, records); assert.equal(requests(), 4);
  added.lineage.parentSessionId = family.details.sessionId; added.relationStatus = 'resolved'; added.rootSessionId = family.details.sessionId;
  family.conversations[0].subagentCount++;
  family.details.agents.push({ sessionId: added.id, title: added.title, available: true, lineage: added.lineage, relationStatus: 'resolved', ownUsage: { ...emptyUsage }, coverage: 'complete', turns: [] });
  await publishUsage(harness, records); assert.equal(requests(), 5);
  assert.equal(harness.element('session-usage-details').querySelectorAll('.usage-agent').length, 4);
  harness.dom.window.close();
});

test('an unresolved child in another guest does not widen a proven root refresh scope', async () => {
  const family = contractExample('root-two-children-grandchild-followup');
  const detached = contractExample('orphan').conversations[0];
  detached.lineage.parentSessionId = family.details.sessionId;
  const records = [...family.conversations, detached];
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(records), conversation_usage: () => structuredClone(family.details) });
  await harness.ready; harness.element('choose-folder').click(); await flush(); await openUsage(harness, 'Primary R');
  const requests = () => harness.calls.filter(call => call.command === 'conversation_usage').length;
  assert.equal(requests(), 1);
  detached.usageRevision = 'other-guest-child-changed';
  await publishUsage(harness, records);
  assert.equal(requests(), 1);
  assert.equal(harness.element('session-usage-details').querySelectorAll('.usage-agent').length, 3);
  harness.dom.window.close();
});

test('pending live refreshes coalesce, discard stale data and preserve disclosure, focus and scroll', async () => {
  const family = contractExample('root-two-children-grandchild-followup');
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(family.conversations), conversation_usage: () => structuredClone(family.details) });
  await harness.ready; harness.element('choose-folder').click(); await flush(); await openUsage(harness, 'Primary R');
  Object.defineProperty(harness.dom.window.document, 'hasFocus', { value: () => true, configurable: true });
  const content = harness.element('session-usage-details'); const panel = content.closest('.dialog-content');
  const disclosure = content.querySelector('.usage-agent'); disclosure.open = true; disclosure.querySelector('summary').focus();
  panel.scrollTop = 140;
  content.querySelector('section[data-agent-turn] .table-scroll').scrollLeft = 55;
  disclosure.querySelector('.table-scroll').scrollLeft = 31;
  const pending = [];
  harness.handlers.conversation_usage = () => new Promise((resolve, reject) => pending.push({ resolve, reject }));
  family.conversations[1].usageRevision = 'child-2'; await publishUsage(harness, family.conversations);
  family.conversations[1].usageRevision = 'child-3'; await publishUsage(harness, family.conversations);
  family.conversations[1].usageRevision = 'child-4'; await publishUsage(harness, family.conversations);
  assert.equal(pending.length, 1);
  const stale = structuredClone(family.details); stale.agents[1].title = 'STALE CHILD'; pending[0].resolve(stale); await flush();
  assert.equal(pending.length, 2); assert.doesNotMatch(content.textContent, /STALE CHILD/);
  family.details.agents[1].title = 'Current child · Türkçe 日本語'; pending[1].resolve(structuredClone(family.details)); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'conversation_usage').length, 3);
  const restored = content.querySelector('.usage-agent');
  assert.equal(restored.open, true); assert.equal(harness.dom.window.document.activeElement, restored.querySelector('summary'));
  assert.equal(panel.scrollTop, 140); assert.equal(content.querySelector('section[data-agent-turn] .table-scroll').scrollLeft, 55); assert.equal(restored.querySelector('.table-scroll').scrollLeft, 31);
  assert.match(restored.querySelector('summary').textContent, /Current child · Türkçe 日本語/);
  const lastGood = content.textContent;
  harness.handlers.conversation_usage = () => { throw new Error('Local history temporarily unavailable'); };
  family.conversations[1].usageRevision = 'child-5'; await publishUsage(harness, family.conversations);
  assert.equal(content.textContent, lastGood); assert.equal(restored.open, true);
  assert.match(harness.element('session-usage-status').textContent, /Showing the last recorded data.*Local history temporarily unavailable/);
  harness.element('close-session-usage').focus();
  harness.handlers.conversation_usage = () => structuredClone(family.details);
  family.conversations[1].usageRevision = 'child-6'; await publishUsage(harness, family.conversations);
  assert.equal(harness.dom.window.document.activeElement, harness.element('close-session-usage'));
  assert.doesNotMatch(harness.element('session-usage-status').textContent, /Could not refresh/);
  assert.equal(content.querySelector('.usage-agent').open, true);
  harness.dom.window.close();
});

test('a late live refresh success or error cannot affect a closed and reopened usage selection', async () => {
  for (const sameSession of [true, false]) for (const outcome of ['success', 'error']) {
    const family = contractExample('root-two-children-grandchild-followup');
    const other = conversationFixture({ id: 'other-chat', title: 'Other chat', updatedAt: 1000 });
    const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard([...family.conversations, other]), conversation_usage: () => structuredClone(family.details) });
    await harness.ready; harness.element('choose-folder').click(); await flush(); await openUsage(harness, 'Primary R');
    let resolveOld, rejectOld;
    harness.handlers.conversation_usage = () => new Promise((resolve, reject) => { resolveOld = resolve; rejectOld = reject; });
    family.conversations[1].usageRevision = 'live-pending'; await publishUsage(harness, [...family.conversations, other]);
    harness.element('close-session-usage').click();
    const fresh = structuredClone(family.details); fresh.agents[1].title = 'FRESH CHILD';
    harness.handlers.conversation_usage = ({ sessionId }) => sameSession ? fresh : sessionDetails(sessionId, [{ id: 'new', status: 'complete', requests: [{ at: 'now', model: 'FRESH MODEL', tokens: { input: 1, cached: 0, cacheWrite: 0, output: 1 }, costUsd: 0.001, unpricedReason: null, rates: null }] }]);
    await openUsage(harness, sameSession ? 'Primary R' : other.title);
    const good = harness.element('session-usage-details').textContent;
    const status = harness.element('session-usage-status').textContent;
    if (outcome === 'success') resolveOld({ ...family.details, warnings: ['OLD WARNING'] });
    else rejectOld(new Error('OLD LIVE ERROR'));
    await flush();
    assert.equal(harness.element('session-usage-details').textContent, good);
    assert.equal(harness.element('session-usage-status').textContent, status);
    assert.match(good, sameSession ? /FRESH CHILD/ : /FRESH MODEL/);
    assert.equal(harness.calls.filter(call => call.command === 'conversation_usage').length, 3);
    harness.dom.window.close();
  }
});

test('a child provenance-only update changes prompt partition without duplicating spending rows', async () => {
  const family = contractExample('root-two-children-grandchild-followup');
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(family.conversations), conversation_usage: () => structuredClone(family.details) });
  await harness.ready; harness.element('choose-folder').click(); await flush(); await openUsage(harness, 'Primary R');
  const agent = family.details.agents[1]; const turn = agent.turns[0];
  const removed = family.details.prompts[0].requestRefs.find(ref => ref.sessionId === agent.sessionId && ref.turnId === turn.id);
  family.details.prompts[0].requestRefs = family.details.prompts[0].requestRefs.filter(ref => ref !== removed);
  const usage = sessionDetails(agent.sessionId, [turn]).ownUsage;
  for (const field of Object.keys(emptyUsage)) {
    family.details.prompts[0].descendantsUsage[field] -= usage[field]; family.details.prompts[0].combinedUsage[field] -= usage[field];
  }
  family.details.unassigned = { usage, requestRefs: [removed], unmeasuredTurnRefs: [] };
  turn.rootTurnId = null; turn.parentTurnId = null; turn.requests[0].source.rootTurnId = null;
  family.conversations[1].usageRevision = 'provenance-only';
  await publishUsage(harness, family.conversations);
  const content = harness.element('session-usage-details');
  assert.equal(content.querySelectorAll('.request-usage tbody tr').length, 6);
  assert.match(content.querySelector('.usage-unassigned').textContent, /Unassigned to a user prompt.*2,200 tokens/s);
  assert.equal(content.querySelectorAll('.usage-source-list a').length, 6);
  assert.equal(content.querySelector('.usage-scope-table').tBodies[0].rows[2].cells[5].textContent, '24,200');
  assert.match(harness.element('session-usage-status').textContent, /2 user prompt\(s\).*6 agent turn\(s\)/);
  harness.dom.window.close();
});

test('an outdated live refresh error yields one queued refresh without replacing current data', async () => {
  const family = contractExample('root-two-children-grandchild-followup');
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(family.conversations), conversation_usage: () => structuredClone(family.details) });
  await harness.ready; harness.element('choose-folder').click(); await flush(); await openUsage(harness, 'Primary R');
  let rejectOld;
  harness.handlers.conversation_usage = () => new Promise((resolve, reject) => { rejectOld = reject; });
  family.conversations[1].usageRevision = 'queued-error-1'; await publishUsage(harness, family.conversations);
  family.conversations[1].usageRevision = 'queued-error-2'; await publishUsage(harness, family.conversations);
  family.details.agents[1].title = 'Latest after queued failure';
  harness.handlers.conversation_usage = () => structuredClone(family.details);
  rejectOld(new Error('OUTDATED FAILURE')); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'conversation_usage').length, 3);
  assert.match(harness.element('session-usage-details').textContent, /Latest after queued failure/);
  assert.doesNotMatch(harness.element('session-usage-status').textContent, /OUTDATED FAILURE|Could not refresh/);
  harness.dom.window.close();
});

test('project switches reject a prior live result and ignore that project dashboard events', async () => {
  for (const outcome of ['success', 'error']) {
    const family = contractExample('root-two-children-grandchild-followup');
    const second = { ...project, id: 'project-b', name: 'Project B', path: '/fixture/b' };
    const secondConversation = conversationFixture({ id: 'chat-b', title: 'B usage', updatedAt: 1000 });
    const projects = [project, second];
    const harness = await interfaceHarness({ recent_projects: () => projects, open_project: ({ identifier }) => projects.find(current => current.id === identifier), prepare_project: ({ identifier }) => ({ project: projects.find(current => current.id === identifier), plan, fingerprint: 'plan-a', candidates: [], message: null }), project_dashboard: ({ identifier }) => sessionDashboard(identifier === project.id ? family.conversations : [secondConversation]), conversation_usage: () => structuredClone(family.details) });
    await harness.ready; harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').click(); await flush(); await openUsage(harness, 'Primary R');
    let resolveOld, rejectOld;
    harness.handlers.conversation_usage = () => new Promise((resolve, reject) => { resolveOld = resolve; rejectOld = reject; });
    family.conversations[1].usageRevision = 'project-a-pending'; await publishUsage(harness, family.conversations);
    harness.element('project-navigation').click(); await flush();
    harness.element('recent-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
    harness.handlers.conversation_usage = ({ sessionId }) => sessionDetails(sessionId, []);
    await openUsage(harness, secondConversation.title);
    const content = harness.element('session-usage-details').textContent;
    const status = harness.element('session-usage-status').textContent;
    const calls = harness.calls.filter(call => call.command === 'conversation_usage').length;
    await publishUsage(harness, family.conversations);
    if (outcome === 'success') resolveOld({ ...family.details, warnings: ['OLD PROJECT WARNING'] });
    else rejectOld(new Error('OLD PROJECT FAILURE'));
    await flush();
    assert.equal(harness.element('project-heading').textContent, second.name);
    assert.equal(harness.element('session-usage-title').textContent, secondConversation.title);
    assert.equal(harness.element('session-usage-details').textContent, content);
    assert.equal(harness.element('session-usage-status').textContent, status);
    assert.equal(harness.calls.filter(call => call.command === 'conversation_usage').length, calls);
    harness.dom.window.close();
  }
});

test('a supported unknown-relation child refreshes its legacy parent detail while unsupported and invalid edges stay outside scope', async () => {
  const legacy = contractExample('legacy-single');
  const family = contractExample('root-two-children-grandchild-followup');
  const parent = legacy.conversations[0];
  const child = family.conversations[1];
  const childAgent = family.details.agents[1];
  child.lineage.parentSessionId = parent.id; child.relationStatus = 'unknown'; child.rootSessionId = null; child.subagentCount = 0; child.familyUsage = child.usage; child.coverage = 'unknown';
  childAgent.lineage = structuredClone(child.lineage); childAgent.relationStatus = 'unknown'; childAgent.coverage = 'unknown';
  for (const turn of childAgent.turns) {
    turn.rootTurnId = null; turn.parentTurnId = null;
    for (const request of turn.requests) request.source.rootTurnId = null;
  }
  const combined = Object.fromEntries(Object.keys(emptyUsage).map(field => [field, parent.usage[field] + child.usage[field]]));
  parent.subagentCount = 1; parent.familyUsage = combined;
  const agents = [...legacy.details.agents, childAgent];
  const requestRefs = agents.flatMap(agent => agent.turns.flatMap(turn => turn.requests.map(request => ({ sessionId: agent.sessionId, turnId: turn.id, requestKey: request.key }))));
  const details = { ...legacy.details, descendantsUsage: child.usage, combinedUsage: combined, agents, agentTurnCount: 3, unassigned: { usage: combined, requestRefs, unmeasuredTurnRefs: [] } };
  const unsupported = conversationFixture({ id: 'unsupported-version', title: 'Unsupported child', updatedAt: 1000, lineage: { ...child.lineage, sourceVersion: '99.0.0' }, relationStatus: 'unknown' });
  const unsupportedLineage = conversationFixture({ ...unsupported, id: 'unsupported-lineage', lineage: { ...child.lineage, lineageVersion: 2 } });
  const invalid = conversationFixture({ id: 'invalid-edge', title: 'Invalid child', updatedAt: 1000, lineage: { ...child.lineage }, relationStatus: 'invalid' });
  const records = [parent, child, unsupported, unsupportedLineage, invalid];
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard(records), conversation_usage: () => structuredClone(details) });
  await harness.ready; harness.element('choose-folder').click(); await flush(); await openUsage(harness, parent.title);
  const requests = () => harness.calls.filter(call => call.command === 'conversation_usage').length;
  assert.equal(requests(), 1);
  for (const excluded of [unsupported, unsupportedLineage, invalid]) {
    excluded.usageRevision = 'excluded-edge-update';
    await publishUsage(harness, records); assert.equal(requests(), 1);
  }
  child.usageRevision = 'trusted-child-only-update'; childAgent.turns[0].requests[0].model = 'Updated unknown-relation child model';
  await publishUsage(harness, records);
  assert.equal(requests(), 2);
  assert.match(harness.element('session-usage-details').textContent, /Updated unknown-relation child model/);
  assert.equal(harness.element('session-usage-details').querySelector('.usage-scope-table').tBodies[0].rows[2].cells[5].textContent, '5,500');
  assert.equal(harness.element('session-usage-details').querySelectorAll('.request-usage tbody tr').length, 3);
  assert.match(harness.element('session-usage-status').textContent, /User prompt count unknown.*3 agent turn\(s\)/);
  harness.dom.window.close();
});

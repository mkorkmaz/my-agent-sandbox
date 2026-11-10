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
const emptyUsage = { input: 0, cached: 0, cacheWrite: 0, output: 0, reasoning: 0, total: 0, costUsd: 0, unpricedRequests: 0, requests: 0, completedTurns: 0, assumedTierRequests: 0 };
const sessionDashboard = conversations => ({ conversations, usage: { ...emptyUsage }, pricingDate: '2026-11-10' });
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
  return { dom, calls, handlers, ready, listeners, terminals, element: id => dom.window.document.getElementById(id), setTerminal: value => { terminals.set(value.identifier, value); } };
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
  const data = { conversations: [conversation], usage: conversation.usage, pricingDate: '2026-11-10' };
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
  const harness = await interfaceHarness({ project_dashboard: () => ({ conversations: [conversation], usage: conversation.usage, pricingDate: '2026-11-10' }), change_conversation_state: ({ state, identifier, sessionId }) => { assert.equal(identifier, project.id); assert.equal(sessionId, conversation.id); conversation.state = state; } });
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
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard([conversation]), conversation_usage: ({ identifier, sessionId }) => { assert.equal(identifier, project.id); assert.equal(sessionId, conversation.id); return turns; } });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, { conversations: [conversation], usage: { ...emptyUsage, input: 1000, output: 50, total: 1050, requests: 1, unpricedRequests: 1 }, pricingDate: '2026-11-10' }] });
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
  harness.handlers.conversation_usage = () => [];
  harness.element('session-table-body').querySelector('[aria-label="Usage for Tracked chat"]').click(); await flush();
  assert.equal(details.children.length, 0);
  assert.match(harness.element('session-usage-status').textContent, /No token usage/);
  harness.dom.window.close();
});

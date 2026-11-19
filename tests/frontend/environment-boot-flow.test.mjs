import test from 'node:test';
import assert from 'node:assert/strict';
import { flush, deferred, project, plan, interfaceHarness } from './frontend-harness.mjs';

function bootGeometry(dom) {
  const rectangles = {
    'terminal-surface': [100, 160, 900, 560],
    'environment-boot-card': [360, 300, 380, 240],
  };
  for (const [identifier, rectangle] of Object.entries(rectangles)) {
    const element = dom.window.document.getElementById(identifier);
    if (element) element.getBoundingClientRect = () => new dom.window.DOMRect(...rectangle);
  }
}

async function stoppedEnvironment() {
  let current = { ...project, stage: 'ready', approvedPlan: 'saved-plan' };
  const prepared = () => ({ project: current, plan, fingerprint: 'saved-plan', message: null, candidates: [] });
  const harness = await interfaceHarness({
    register_project: () => current, prepare_project: prepared, preview_project: prepared,
    prepare_environment: () => {
      current = { ...current, stage: 'environment-starting' };
      harness.setTerminal({ identifier: 70, projectId: project.id, environmentId: project.environmentId, agentKind: 'codex', purpose: 'environment', running: true, exitCode: null, conversationId: null, busy: false });
      return 70;
    },
  }, bootGeometry);
  await harness.ready;
  return {
    harness, prepared,
    start: async () => { harness.element('choose-folder').click(); await flush(); },
    finish: async (message = null) => {
      current = { ...current, stage: message ? 'ready' : 'environment-ready', environmentAttempt: { completed: true, error: message, terminalIdentifier: 70 } };
      const terminal = harness.terminals.get(70);
      if (terminal) harness.setTerminal({ ...terminal, exitCode: message ? 1 : 0 });
      await harness.listeners.get('environment-preparation-finished')({ payload: { project: current, terminalIdentifier: 70, message } });
      const finished = harness.terminals.get(70);
      if (finished) harness.setTerminal({ ...finished, running: false, exitCode: message ? 1 : 0 });
      await harness.listeners.get('terminal-exit')({ payload: { identifier: 70, projectId: project.id, environmentId: project.environmentId, agentKind: 'codex', purpose: 'environment', code: message ? 1 : 0 } }); await flush();
    },
  };
}

test('environment startup shows setup logs and the matching agent boot overlay', async t => {
  for (const [agentKind, name] of [['codex', 'Codex'], ['claude-code', 'Claude Code']]) await t.test(name, async t => {
    let current = { ...project, agentKind, stage: 'ready', approvedPlan: 'saved-plan' };
    const prepared = () => ({ project: current, plan, fingerprint: 'saved-plan', message: null, candidates: [] });
    const harness = await interfaceHarness({
      register_project: () => current, prepare_project: prepared, preview_project: prepared,
      prepare_environment: () => {
        current = { ...current, stage: 'environment-starting' };
        harness.setTerminal({ identifier: 70, projectId: current.id, environmentId: current.environmentId, agentKind, purpose: 'environment', running: true, exitCode: null, conversationId: null, busy: false });
        return 70;
      },
    }, bootGeometry);
    t.after(() => harness.dom.window.close());
    await harness.ready;
    const overlay = harness.element('environment-boot-overlay');
    assert.ok(overlay, 'The workspace needs a boot overlay');
    assert.equal(overlay.hidden, true);
    harness.element('choose-folder').click(); await flush();
    assert.equal(harness.element('terminal-panel').hidden, false);
    assert.equal(overlay.hidden, false);
    assert.equal(harness.element('environment-boot-message').textContent, 'Brig environment booting');
    assert.equal(harness.element('environment-boot-canvas').tagName, 'CANVAS');
    const request = harness.calls.find(call => call.command === 'prepare_environment');
    assert.equal(request.args.environmentId, current.environmentId);
    assert.equal(request.args.layout.visible, false);
    const runningLayout = harness.calls.filter(call => call.command === 'terminal_layout').at(-1).args.layout;
    assert.equal(runningLayout.visible, true);
    assert.deepEqual({ ...runningLayout.overlay }, { x: 260, y: 140, width: 380, height: 240 });
    assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').length, 0);
    current = { ...current, stage: 'environment-ready' };
    harness.setTerminal({ ...harness.terminals.get(70), exitCode: 0 });
    await harness.listeners.get('environment-preparation-finished')({ payload: { project: current, terminalIdentifier: 70, message: null } }); await flush();
    assert.equal(overlay.hidden, true);
    assert.equal(harness.terminals.get(70).running, true);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 0);
    harness.setTerminal({ ...harness.terminals.get(70), running: false });
    await harness.listeners.get('terminal-exit')({ payload: { identifier: 70, projectId: current.id, environmentId: current.environmentId, agentKind, purpose: 'environment', code: 0 } }); await flush();
    assert.equal(harness.terminals.has(70), false);
    assert.equal(harness.element('terminal-panel').hidden, true);
    assert.equal(harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="70"]'), null);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_close' && call.args.identifier === 70).length, 1);
    assert.equal(harness.element('start-codex').disabled, false);
  });
});

test('environment verification and completion errors close setup and retain retry feedback', async t => {
  for (const event of ['environment-preparation-finished', 'environment-preparation-error']) await t.test(event, async t => {
    const fixture = await stoppedEnvironment(), { harness } = fixture;
    t.after(() => harness.dom.window.close());
    await fixture.start();
    assert.equal(harness.element('environment-boot-overlay').hidden, false);
    if (event === 'environment-preparation-finished') await fixture.finish('Guest version check failed.');
    if (event === 'environment-preparation-error') {
      await harness.listeners.get(event)({ payload: { projectId: project.id, environmentId: project.environmentId, terminalIdentifier: 70, message: 'Could not verify the environment.' } }); await flush();
      assert.equal(harness.terminals.get(70).running, true);
      assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 0);
      harness.setTerminal({ ...harness.terminals.get(70), running: false, exitCode: 1 });
      await harness.listeners.get('terminal-exit')({ payload: { identifier: 70, projectId: project.id, environmentId: project.environmentId, agentKind: 'codex', purpose: 'environment', code: 1 } }); await flush();
    }
    assert.equal(harness.element('environment-boot-overlay').hidden, true);
    assert.equal(harness.element('terminal-panel').hidden, true);
    assert.equal(harness.terminals.has(70), false);
    assert.equal(harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="70"]'), null);
    assert.equal(harness.element('start-codex').disabled, true);
    assert.equal(harness.element('retry-project-plan').hidden, false);
    assert.match(harness.element('project-plan-status').textContent, /failed|Could not verify/);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_close' && call.args.identifier === 70).length, 1);
  });
});

test('duplicate environment completion events share one native terminal close', async t => {
  const fixture = await stoppedEnvironment(), { harness } = fixture, closing = deferred();
  t.after(() => harness.dom.window.close());
  harness.handlers.terminal_close = async ({ identifier }) => { await closing.promise; harness.terminals.delete(identifier); };
  await fixture.start();
  const first = fixture.finish(); await flush();
  const duplicate = fixture.finish(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close' && call.args.identifier === 70).length, 1);
  closing.resolve();
  await Promise.all([first, duplicate]);
  await harness.listeners.get('terminal-exit')({ payload: { identifier: 70, projectId: project.id, environmentId: project.environmentId, agentKind: 'codex', purpose: 'environment', code: 0 } }); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close' && call.args.identifier === 70).length, 1);
  assert.equal(harness.terminals.has(70), false);
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="70"]'), null);
  assert.equal(harness.element('folder-error').hidden, true);
});

test('completion before a delayed environment start response closes setup without reopening it', async t => {
  const fixture = await stoppedEnvironment(), { harness } = fixture, response = deferred();
  t.after(() => harness.dom.window.close());
  const start = harness.handlers.prepare_environment;
  harness.handlers.prepare_environment = () => { start(); return response.promise; };
  harness.handlers.finish_environment = () => fixture.prepared().project;
  await fixture.start();
  const completion = fixture.finish(); await flush();
  response.resolve(70);
  await completion; await flush();
  assert.equal(harness.terminals.has(70), false);
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.element('environment-boot-overlay').hidden, true);
  assert.equal(harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="70"]'), null);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close' && call.args.identifier === 70).length, 1);
  assert.equal(harness.element('folder-error').hidden, true);
  assert.equal(harness.element('workspace-status-bar').classList.contains('is-error'), false);
});

test('a completed environment snapshot closes setup without a completion event', async t => {
  let current = { ...project, stage: 'ready', approvedPlan: 'saved-plan' };
  const prepared = () => ({ project: current, plan, fingerprint: 'saved-plan', message: null, candidates: [] });
  const harness = await interfaceHarness({
    register_project: () => current, prepare_project: prepared, preview_project: prepared,
    prepare_environment: () => {
      current = { ...current, stage: 'environment-ready' };
      harness.setTerminal({ identifier: 70, projectId: project.id, environmentId: project.environmentId, purpose: 'environment', running: false, exitCode: 0, conversationId: null, busy: false });
      return 70;
    },
    finish_environment: () => current,
  }, bootGeometry);
  t.after(() => harness.dom.window.close());
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'finish_environment').length, 1);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close' && call.args.identifier === 70).length, 1);
  assert.equal(harness.terminals.has(70), false);
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.element('environment-boot-overlay').hidden, true);
  assert.equal(harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="70"]'), null);
  assert.equal(harness.element('start-codex').disabled, false);
});

test('foreign and superseded environment events cannot stop the active boot overlay', async t => {
  const fixture = await stoppedEnvironment(), { harness } = fixture;
  t.after(() => harness.dom.window.close());
  await fixture.start();
  const foreign = { ...fixture.prepared().project, environmentId: 'b'.repeat(32), agentKind: 'claude-code', stage: 'environment-ready' };
  await harness.listeners.get('environment-preparation-finished')({ payload: { project: foreign, terminalIdentifier: 80, message: null } });
  await harness.listeners.get('environment-preparation-error')({ payload: { projectId: project.id, environmentId: foreign.environmentId, terminalIdentifier: 80, message: 'Other environment failed.' } });
  await harness.listeners.get('environment-preparation-finished')({ payload: { project: { ...fixture.prepared().project, stage: 'environment-ready' }, terminalIdentifier: 69, message: null } });
  await harness.listeners.get('environment-preparation-error')({ payload: { projectId: project.id, environmentId: project.environmentId, terminalIdentifier: 69, message: 'Old attempt failed.' } }); await flush();
  assert.equal(harness.element('environment-boot-overlay').hidden, false);
  assert.equal(harness.element('environment-boot-message').textContent, 'Brig environment booting');
  assert.equal(harness.element('folder-error').hidden, true);
  await fixture.finish();
  assert.equal(harness.element('environment-boot-overlay').hidden, true);
});

test('a late environment start response preserves Help and Settings navigation', async t => {
  for (const view of ['help', 'settings']) await t.test(view, async t => {
    const fixture = await stoppedEnvironment(), { harness } = fixture, response = deferred();
    t.after(() => harness.dom.window.close());
    const start = harness.handlers.prepare_environment;
    harness.handlers.prepare_environment = () => { start(); return response.promise; };
    await fixture.start();
    assert.equal(harness.element('environment-boot-overlay').hidden, false);
    harness.element(view + '-navigation').click(); await flush();
    const heading = harness.element(view + '-heading');
    assert.equal(harness.dom.window.document.activeElement, heading);
    response.resolve(70); await flush();
    assert.equal(harness.element(view + '-view').hidden, false);
    assert.equal(harness.element('project-view').hidden, true);
    assert.equal(harness.element('environment-boot-overlay').hidden, true);
    assert.equal(harness.dom.window.document.activeElement, heading);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').length, 0);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_layout').at(-1).args.layout.visible, false);
    await fixture.finish();
    assert.equal(harness.element(view + '-view').hidden, false);
    assert.equal(harness.dom.window.document.activeElement, heading);
    assert.equal(harness.element('environment-boot-overlay').hidden, true);
    assert.equal(harness.terminals.has(70), false);
    assert.equal(harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="70"]'), null);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_close' && call.args.identifier === 70).length, 1);
  });
});

test('returning from Help to a running setup resumes its boot overlay without another process', async t => {
  const fixture = await stoppedEnvironment(), { harness } = fixture;
  t.after(() => harness.dom.window.close());
  await fixture.start();
  harness.element('help-navigation').click(); await flush();
  assert.equal(harness.element('environment-boot-overlay').hidden, true);
  harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="70"]').click(); await flush();
  assert.equal(harness.element('help-view').hidden, true);
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.equal(harness.element('environment-boot-overlay').hidden, false);
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 1);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').length, 0);
});

test('boot completion preserves an open project settings dialog and its field focus', async t => {
  const fixture = await stoppedEnvironment(), { harness } = fixture;
  t.after(() => harness.dom.window.close());
  await fixture.start();
  harness.element('project-settings-button').click(); await flush();
  harness.element('project-name').focus();
  const focused = harness.dom.window.document.activeElement;
  assert.equal(harness.calls.filter(call => call.command === 'terminal_layout').at(-1).args.layout.visible, false);
  await fixture.finish();
  assert.equal(harness.element('environment-boot-overlay').hidden, true);
  assert.equal(harness.element('project-settings-dialog').open, true);
  assert.equal(harness.dom.window.document.activeElement, focused);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').length, 0);
  harness.element('close-project-settings').click(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.terminals.has(70), false);
  assert.equal(harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="70"]'), null);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close' && call.args.identifier === 70).length, 1);
});

test('selecting a live local shell prevents a late setup response from taking the workspace', async t => {
  const fixture = await stoppedEnvironment(), { harness } = fixture, response = deferred();
  t.after(() => harness.dom.window.close());
  const start = harness.handlers.prepare_environment;
  harness.handlers.prepare_environment = () => { start(); return response.promise; };
  harness.setTerminal({ identifier: 80, projectId: project.id, purpose: 'local', running: true, exitCode: null, conversationId: null, busy: false });
  await fixture.start();
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 1);
  assert.equal(harness.element('environment-boot-overlay').hidden, false);
  const local = harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="80"]');
  assert.equal(local.disabled, false);
  local.click(); await flush();
  assert.equal(harness.element('environment-boot-overlay').hidden, true);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, 80);
  response.resolve(70); await flush();
  assert.equal(harness.element('environment-boot-overlay').hidden, true);
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.match(harness.element('terminal-status').textContent, /Local terminal/);
  await fixture.finish();
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, 80);
  assert.equal(harness.terminals.get(80).running, true);
  assert.equal(harness.terminals.has(70), false);
  assert.equal(harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="70"]'), null);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 1);
  assert.equal(harness.calls.find(call => call.command === 'terminal_close').args.identifier, 70);
});

test('choosing the next chat agent preserves the shared boot overlay', async t => {
  const fixture = await stoppedEnvironment(), { harness } = fixture;
  t.after(() => harness.dom.window.close());
  const choose = async kind => {
    harness.element('agent-choice').value = kind;
    harness.element('agent-choice').dispatchEvent(new harness.dom.window.Event('change')); await flush();
  };
  await fixture.start();
  await choose('claude-code');
  assert.equal(harness.element('environment-boot-overlay').hidden, false);
  assert.equal(harness.element('agent-choice').value, 'claude-code');
  await choose('codex');
  assert.equal(harness.element('environment-boot-overlay').hidden, false);
  assert.equal(harness.element('environment-boot-message').textContent, 'Brig environment booting');
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 1);
});

test('setup completed while another agent is selected closes only its own terminal', async t => {
  const fixture = await stoppedEnvironment(), { harness } = fixture;
  t.after(() => harness.dom.window.close());
  const choose = async kind => {
    harness.element('agent-choice').value = kind;
    harness.element('agent-choice').dispatchEvent(new harness.dom.window.Event('change')); await flush();
  };
  await fixture.start();
  await choose('claude-code');
  harness.setTerminal({ identifier: 80, projectId: project.id, environmentId: project.environmentId, agentKind: 'claude-code', purpose: 'shell', running: true, exitCode: null, conversationId: null, busy: false });
  await harness.listeners.get('terminal-sessions-changed')({ payload: project.id }); await flush();
  harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="80"]').click(); await flush();
  await fixture.finish();
  await harness.listeners.get('terminal-exit')({ payload: { identifier: 70, projectId: project.id, environmentId: project.environmentId, agentKind: 'codex', purpose: 'environment', code: 0 } }); await flush();
  assert.equal(harness.element('agent-choice').value, 'claude-code');
  assert.equal(harness.element('project-reference').textContent, project.profile + '@' + project.label);
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, 80);
  assert.equal(harness.terminals.get(80).running, true);
  assert.equal(harness.terminals.has(70), false);
  await choose('codex');
  assert.equal(harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="70"]'), null);
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.equal(harness.element('environment-boot-overlay').hidden, true);
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 1);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 1);
  assert.equal(harness.calls.find(call => call.command === 'terminal_close').args.identifier, 70);
});

test('a failed setup disappears and keeps retry feedback after Help navigation', async t => {
  const fixture = await stoppedEnvironment(), { harness } = fixture;
  t.after(() => harness.dom.window.close());
  await fixture.start();
  await harness.listeners.get('environment-preparation-error')({ payload: { projectId: project.id, environmentId: project.environmentId, terminalIdentifier: 70, message: 'Could not verify the environment.' } }); await flush();
  assert.equal(harness.element('environment-boot-overlay').hidden, true);
  assert.equal(harness.element('retry-project-plan').hidden, false);
  assert.equal(fixture.prepared().project.stage, 'environment-starting');
  assert.equal(harness.terminals.get(70).running, true);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 0);
  harness.setTerminal({ ...harness.terminals.get(70), running: false, exitCode: 1 });
  await harness.listeners.get('terminal-exit')({ payload: { identifier: 70, projectId: project.id, environmentId: project.environmentId, agentKind: 'codex', purpose: 'environment', code: 1 } }); await flush();
  assert.equal(harness.terminals.has(70), false);
  harness.element('help-navigation').click(); await flush();
  harness.element('sidebar-current-project').click(); await flush();
  assert.equal(harness.element('help-view').hidden, true);
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="70"]'), null);
  assert.equal(harness.element('environment-boot-overlay').hidden, true);
  assert.equal(harness.element('retry-project-plan').hidden, false);
  assert.match(harness.element('project-plan-status').textContent, /Could not verify the environment/);
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 1);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close' && call.args.identifier === 70).length, 1);
});

test('a previous project cannot replace the new workspace when environment startup finishes', async t => {
  const fixture = await stoppedEnvironment(), { harness } = fixture;
  t.after(() => harness.dom.window.close());
  const other = { ...project, id: 'project-b', environmentId: 'c'.repeat(32), name: 'Other project', path: '/fixture/other-project' };
  harness.handlers.recent_projects = () => [fixture.prepared().project, other];
  harness.handlers.open_project = ({ identifier }) => identifier === other.id ? other : fixture.prepared().project;
  const prepared = ({ identifier }) => identifier === other.id ? { project: other, plan, fingerprint: 'saved-plan', message: null, candidates: [] } : fixture.prepared();
  harness.handlers.prepare_project = prepared; harness.handlers.preview_project = prepared;
  await harness.listeners.get('records-changed')({ payload: ['projects'] }); await flush();
  await fixture.start();
  harness.element('project-navigation').click(); await flush();
  harness.element('recent-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
  assert.equal(harness.element('project-heading').textContent, other.name);
  assert.equal(harness.element('environment-boot-overlay').hidden, true);
  await fixture.finish();
  await harness.listeners.get('environment-preparation-error')({ payload: { projectId: project.id, environmentId: project.environmentId, terminalIdentifier: 70, message: 'Previous project failed.' } }); await flush();
  assert.equal(harness.element('project-heading').textContent, other.name);
  assert.equal(harness.element('environment-boot-overlay').hidden, true);
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.element('folder-error').hidden, true);
  assert.equal(harness.terminals.has(70), false);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close' && call.args.identifier === 70).length, 1);
});

test('completed setup from another project waits for an unrelated launch and then closes', async t => {
  const fixture = await stoppedEnvironment(), { harness } = fixture, launch = deferred();
  t.after(() => harness.dom.window.close());
  const other = { ...project, id: 'project-b', environmentId: 'c'.repeat(32), name: 'Other project', path: '/fixture/other-project' };
  harness.handlers.recent_projects = () => [fixture.prepared().project, other];
  harness.handlers.open_project = ({ identifier }) => identifier === other.id ? other : fixture.prepared().project;
  const prepared = ({ identifier }) => identifier === other.id ? { project: other, plan, fingerprint: 'saved-plan', message: null, candidates: [] } : fixture.prepared();
  harness.handlers.prepare_project = prepared; harness.handlers.preview_project = prepared;
  harness.handlers.terminal_start = () => launch.promise;
  await harness.listeners.get('records-changed')({ payload: ['projects'] }); await flush();
  await fixture.start();
  harness.element('project-navigation').click(); await flush();
  harness.element('recent-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  harness.element('start-terminal').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'terminal_start').length, 1);
  await fixture.finish();
  assert.equal(harness.terminals.get(70).running, false);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 0);
  assert.equal(harness.element('project-heading').textContent, other.name);
  harness.setTerminal({ identifier: 80, projectId: other.id, purpose: 'local', running: true, exitCode: null, conversationId: null, busy: false });
  launch.resolve(80); await flush();
  assert.equal(harness.terminals.has(70), false);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 1);
  assert.equal(harness.calls.find(call => call.command === 'terminal_close').args.identifier, 70);
  assert.equal(harness.terminals.get(80).running, true);
  assert.equal(harness.element('project-heading').textContent, other.name);
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.match(harness.element('terminal-status').textContent, /Local terminal/);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, 80);
  assert.equal(harness.element('folder-error').hidden, true);
});

test('terminal checking releases completed setup cleanup when it reuses a shell or fails', async t => {
  for (const outcome of ['reuse', 'failure']) await t.test(outcome, async t => {
    const fixture = await stoppedEnvironment(), { harness } = fixture, checking = deferred();
    t.after(() => harness.dom.window.close());
    const other = { ...project, id: 'project-b', environmentId: 'c'.repeat(32), name: 'Other project', path: '/fixture/other-project' };
    harness.handlers.recent_projects = () => [fixture.prepared().project, other];
    harness.handlers.open_project = ({ identifier }) => identifier === other.id ? other : fixture.prepared().project;
    const prepared = ({ identifier }) => identifier === other.id ? { project: other, plan, fingerprint: 'saved-plan', message: null, candidates: [] } : fixture.prepared();
    harness.handlers.prepare_project = prepared; harness.handlers.preview_project = prepared;
    await harness.listeners.get('records-changed')({ payload: ['projects'] }); await flush();
    await fixture.start();
    harness.element('project-navigation').click(); await flush();
    harness.element('recent-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
    harness.setTerminal({ identifier: 80, projectId: other.id, purpose: 'local', running: true, exitCode: null, conversationId: null, busy: false });
    const sessions = harness.handlers.terminal_sessions;
    let delayed = true;
    harness.handlers.terminal_sessions = ({ projectId }) => {
      if (projectId === other.id && delayed) { delayed = false; return checking.promise; }
      return sessions({ projectId });
    };
    harness.element('project-settings-button').click(); await flush();
    harness.element('start-terminal').click(); await flush();
    assert.equal(delayed, false);
    await fixture.finish();
    assert.equal(harness.terminals.get(70).running, false);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 0);
    if (outcome === 'reuse') checking.resolve([harness.terminals.get(80)]);
    if (outcome === 'failure') checking.reject(new Error('Could not inspect the local terminal.'));
    await flush();
    assert.equal(harness.terminals.has(70), false);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 1);
    assert.equal(harness.calls.find(call => call.command === 'terminal_close').args.identifier, 70);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_start').length, 0);
    assert.equal(harness.terminals.get(80).running, true);
    assert.equal(harness.element('project-heading').textContent, other.name);
    if (outcome === 'reuse') {
      assert.equal(harness.element('terminal-panel').hidden, false);
      assert.match(harness.element('terminal-status').textContent, /Local terminal/);
      assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, 80);
      assert.equal(harness.element('folder-error').hidden, true);
    }
    if (outcome === 'failure') assert.match(harness.element('folder-error').textContent, /Could not inspect the local terminal/);
  });
});

test('closing setup output hides the boot overlay and late failure cannot reopen it', async t => {
  const fixture = await stoppedEnvironment(), { harness } = fixture;
  t.after(() => harness.dom.window.close());
  await fixture.start();
  harness.element('close-terminal').click(); await flush();
  assert.equal(harness.element('environment-boot-overlay').hidden, true);
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.terminals.has(70), false);
  await harness.listeners.get('environment-preparation-error')({ payload: { projectId: project.id, environmentId: project.environmentId, terminalIdentifier: 70, message: 'Environment setup was cancelled.' } }); await flush();
  assert.equal(harness.element('environment-boot-overlay').hidden, true);
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').length, 0);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close' && call.args.identifier === 70).length, 1);
});

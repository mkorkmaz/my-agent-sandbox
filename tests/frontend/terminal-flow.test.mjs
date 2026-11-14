import test from 'node:test';
import assert from 'node:assert/strict';
import { flush, deferred, project, plan, sessionDashboard, providerLogin, interfaceHarness } from './frontend-harness.mjs';

test('a missing terminal hides its native surface and shows a timed status error', async () => {
  let expire;
  const harness = await interfaceHarness({}, dom => {
    const schedule = dom.window.setTimeout.bind(dom.window);
    dom.window.setTimeout = (callback, delay, ...args) => {
      if (delay !== 10000) return schedule(callback, delay, ...args);
      expire = () => callback(...args);
      return -100;
    };
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, false);
  harness.handlers.terminal_sessions = () => [];
  await harness.listeners.get('terminal-sessions-changed')({ payload: project.id }); await flush();
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_layout').at(-1).args.layout.visible, false);
  assert.equal(harness.element('folder-error').hidden, true);
  const bar = harness.element('workspace-status-bar');
  assert.equal(bar.hidden, false);
  assert.equal(bar.classList.contains('is-error'), true);
  assert.match(bar.textContent, /This terminal output is no longer retained/);
  await harness.listeners.get('provider-login-changed')({ payload: providerLogin('chatgpt', 'preparing', 1) });
  assert.equal(bar.classList.contains('is-error'), true);
  assert.match(bar.textContent, /This terminal output is no longer retained/);
  assert.equal(typeof expire, 'function');
  expire();
  assert.equal(bar.hidden, true);
  harness.dom.window.close();
});

test('an expired terminal warning cannot hide a newer workspace status', async () => {
  let expire;
  const refresh = deferred();
  const harness = await interfaceHarness({}, dom => {
    const schedule = dom.window.setTimeout.bind(dom.window);
    dom.window.setTimeout = (callback, delay, ...args) => {
      if (delay !== 10000) return schedule(callback, delay, ...args);
      expire = () => callback(...args);
      return -100;
    };
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.handlers.terminal_sessions = () => [];
  await harness.listeners.get('terminal-sessions-changed')({ payload: project.id }); await flush();
  harness.handlers.project_dashboard = () => refresh.promise;
  harness.element('refresh-sessions').click(); await flush();
  const bar = harness.element('workspace-status-bar');
  assert.equal(bar.classList.contains('is-error'), false);
  assert.match(bar.textContent, /Refreshing sessions/);
  expire();
  assert.equal(bar.hidden, false);
  assert.match(bar.textContent, /Refreshing sessions/);
  refresh.resolve(sessionDashboard([])); await flush();
  assert.equal(bar.hidden, true);
  harness.dom.window.close();
});

test('hiding an already removed native terminal keeps the timed warning as the only error', async () => {
  const harness = await interfaceHarness();
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.handlers.terminal_sessions = () => [];
  harness.handlers.terminal_layout = ({ layout }) => { if (!layout.visible) throw 'Terminal closed.'; };
  await harness.listeners.get('terminal-sessions-changed')({ payload: project.id }); await flush();
  assert.equal(harness.element('folder-error').hidden, true);
  assert.equal(harness.element('workspace-status-bar').classList.contains('is-error'), true);
  assert.match(harness.element('workspace-status-text').textContent, /This terminal output is no longer retained/);
  harness.dom.window.close();
});

test('an earlier warning timer cannot dismiss a newer terminal warning', async () => {
  const timers = [];
  const harness = await interfaceHarness({}, dom => {
    const schedule = dom.window.setTimeout.bind(dom.window);
    dom.window.setTimeout = (callback, delay, ...args) => {
      if (delay !== 10000) return schedule(callback, delay, ...args);
      timers.push(() => callback(...args));
      return -100 - timers.length;
    };
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.handlers.terminal_sessions = () => [];
  await harness.listeners.get('terminal-sessions-changed')({ payload: project.id }); await flush();
  harness.handlers.terminal_sessions = () => [...harness.terminals.values()];
  await harness.listeners.get('terminal-sessions-changed')({ payload: project.id }); await flush();
  harness.element('sidebar-session-list').querySelector('button').click(); await flush();
  harness.handlers.terminal_sessions = () => [];
  await harness.listeners.get('terminal-sessions-changed')({ payload: project.id }); await flush();
  assert.equal(timers.length, 2);
  timers[0]();
  assert.equal(harness.element('workspace-status-bar').hidden, false);
  assert.equal(harness.element('workspace-status-bar').classList.contains('is-error'), true);
  timers[1]();
  assert.equal(harness.element('workspace-status-bar').hidden, true);
  harness.dom.window.close();
});

test('a live chat stays reachable while the first history read is pending or fails', async () => {
  const pending = deferred();
  const harness = await interfaceHarness({ project_dashboard: () => pending.promise });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.element('sidebar-session-list').querySelector('.skeleton'), null);
  assert.ok(harness.element('sidebar-session-list').querySelector('.sidebar-session'));
  harness.element('hide-terminal').click(); await flush();
  pending.reject(new Error('History unavailable')); await flush();
  const chat = harness.element('sidebar-session-list').querySelector('.sidebar-session');
  assert.ok(chat); assert.equal(chat.disabled, false);
  chat.click(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 1);
  harness.dom.window.close();
});

test('ending a terminal shows a loader and restores the control after failure', async () => {
  const close = deferred();
  const harness = await interfaceHarness({ terminal_close: () => close.promise });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('close-terminal').click(); harness.element('close-terminal').click(); await flush();
  assert.equal(harness.element('close-terminal').getAttribute('aria-busy'), 'true');
  assert.equal(harness.element('close-terminal').disabled, true);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 1);
  close.reject(new Error('Process still running')); await flush();
  assert.equal(harness.element('close-terminal').getAttribute('aria-busy'), 'false');
  assert.equal(harness.element('close-terminal').disabled, false);
  assert.match(harness.element('terminal-status').textContent, /Process still running/);
  harness.dom.window.close();
});

test('double launch creates one Codex process and navigation returns to that process', async () => {
  let finishStart;
  const terminal = { identifier: 42, projectId: project.id, purpose: 'codex', running: true, exitCode: null };
  const harness = await interfaceHarness({ start_agent_session: () => new Promise(resolve => { finishStart = () => { harness.setTerminal(terminal); resolve(42); }; }) });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'preview_project').length, 1);
  harness.element('start-codex').click(); harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 1);
  finishStart(); await flush();
  harness.element('settings-navigation').click(); await flush();
  assert.equal(harness.element('project-view').hidden, true);
  assert.ok(harness.calls.some(call => call.command === 'terminal_layout' && call.args.identifier === 42 && call.args.layout.visible === false));
  harness.element('sidebar-current-project').click(); await flush();
  assert.ok(harness.calls.some(call => call.command === 'terminal_focus' && call.args.identifier === 42));
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 1);
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
  assert.match(harness.element('folder-error').textContent, /Simulated process failure/);
  assert.ok(harness.calls.some(call => call.command === 'terminal_layout' && call.args.identifier === 1 && !call.args.layout.visible));
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 0);
  harness.element('sidebar-session-list').querySelector('[data-session-id="' + project.environmentId + '/terminal-1"]').click(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, 1);
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 1);
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
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 2);
  assert.equal(harness.element('folder-error').hidden, true);
  harness.dom.window.close();
});

test('saved session titles keep their full tooltip and resume the exact conversation', async () => {
  const conversation = { id: '01a1c5fd-894f-7e42-b36c-bd1a0dfef8e7', title: 'A long Türkçe and 日本語 session title that must remain complete in the tooltip and accessible name', updatedAt: 2000 };
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard([conversation]) });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  const row = harness.element('sidebar-session-list').querySelector('button');
  assert.equal(row.title, conversation.title);
  assert.equal(row.getAttribute('aria-label'), conversation.title + ', Codex');
  assert.equal(row.textContent, conversation.title);
  row.click(); await flush();
  const launch = harness.calls.find(call => call.command === 'start_agent_session');
  assert.equal(launch.args.action, 'resume');
  assert.equal(launch.args.sessionId, conversation.id);
  assert.equal(launch.args.identifier, project.id);
  assert.equal(launch.args.fingerprint, 'plan-a');
  assert.equal(launch.args.useBrigBoundary, true);
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
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 2);
  assert.equal(harness.terminals.size, 2);
  for (const [session, identifier] of [[project.environmentId + '/terminal-1', 1], [conversation.id, 2], [project.environmentId + '/terminal-1', 1]]) {
    harness.element('sidebar-session-list').querySelector('[data-session-id="' + session + '"]').click(); await flush();
    assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, identifier);
    assert.equal(harness.element('sidebar-session-list').querySelector('[aria-current="true"]').dataset.sessionId, session);
  }
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 2);
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
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 1);
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
  assert.equal(harness.element('sidebar-session-list').querySelector('[aria-current="true"]').dataset.sessionId, project.environmentId + '/' + second.id);
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
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 2);
  harness.setTerminal({ ...harness.terminals.get(2), running: false, exitCode: 0 });
  await harness.listeners.get('terminal-exit')({ payload: { identifier: 2, projectId: project.id, purpose: 'codex', code: 0 } });
  await flush();
  assert.equal(row(second.id).querySelector('.session-activity'), null);
  harness.dom.window.close();
});

test('project navigation keeps chats reachable from the sidebar while other projects run', async () => {
  const other = { ...project, id: 'project-b', name: 'Second project', path: '/fixture/second' };
  const projects = [project, other];
  const selected = ({ identifier }) => ({ project: projects.find(item => item.id === identifier), plan, fingerprint: 'plan-a', candidates: [] });
  const harness = await interfaceHarness({ recent_projects: () => projects, open_project: args => selected(args).project, prepare_project: selected, preview_project: selected });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('hide-terminal').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('sidebar-session-list').querySelector('[data-session-id="' + project.environmentId + '/terminal-1"]').click(); await flush();
  harness.element('project-navigation').click(); await flush();
  harness.element('recent-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('project-navigation').click(); await flush();
  harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').click(); await flush();
  harness.element('sidebar-session-list').querySelector('[data-session-id="' + project.environmentId + '/terminal-1"]').click(); await flush();
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

test('ending a terminal skips resize requests and missing-output warnings while closure is pending', async () => {
  const close = deferred();
  const harness = await interfaceHarness();
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  harness.handlers.terminal_close = ({ identifier }) => {
    harness.terminals.delete(identifier);
    return close.promise;
  };
  harness.handlers.terminal_layout = ({ identifier }) => {
    if (!harness.terminals.has(identifier)) throw 'Terminal closed.';
  };
  const layoutCount = harness.calls.filter(call => call.command === 'terminal_layout').length;
  harness.element('close-terminal').click(); await flush();
  harness.resizeTerminal(); await flush();
  await harness.listeners.get('terminal-sessions-changed')({ payload: project.id }); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'terminal_layout').length, layoutCount);
  assert.equal(harness.element('folder-error').hidden, true);
  assert.equal(harness.element('workspace-status-bar').classList.contains('is-error'), false);
  close.resolve(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.element('folder-error').hidden, true);
  harness.dom.window.close();
});

test('a layout failure received after Close terminal ignores only the expected closed-terminal result', async t => {
  for (const [error, expectedHidden] of [['Terminal closed.', true], ['Native layout failed.', false]]) await t.test(error, async () => {
    const layout = deferred();
    const harness = await interfaceHarness();
    await harness.ready;
    harness.element('choose-folder').click(); await flush();
    harness.element('start-codex').click(); await flush();
    harness.handlers.terminal_layout = () => layout.promise;
    harness.resizeTerminal(); await flush();
    harness.element('close-terminal').click(); await flush();
    layout.reject(error); await flush();
    assert.equal(harness.element('terminal-panel').hidden, true);
    assert.equal(harness.element('folder-error').hidden, expectedHidden);
    if (!expectedHidden) assert.equal(harness.element('folder-error').textContent, error);
    harness.dom.window.close();
  });
});

test('a focus failure received after Close terminal cannot report that opening the terminal failed', async () => {
  const focus = deferred();
  const harness = await interfaceHarness({ terminal_focus: () => focus.promise });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').length, 1);
  harness.element('close-terminal').click(); await flush();
  focus.reject('Terminal closed.'); await flush();
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.element('folder-error').hidden, true);
  assert.equal(harness.element('start-codex').disabled, false);
  harness.dom.window.close();
});

test('an already running chat stays reachable while the plan is checked', async () => {
  const checking = deferred();
  const conversation = { id: 'live-chat', title: 'Running chat', updatedAt: 1000, environmentId: project.environmentId, available: false };
  const harness = await interfaceHarness({
    recent_projects: () => [project],
    saved_project_dashboard: () => sessionDashboard([conversation]),
    preview_project: () => checking.promise,
  });
  await harness.ready;
  harness.setTerminal({ identifier: 42, projectId: project.id, environmentId: project.environmentId, purpose: 'codex', running: true, exitCode: null, conversationId: conversation.id, busy: false });
  harness.element('recent-project-list').querySelector('.recent-project').click(); await flush();
  const live = harness.element('sidebar-session-list').querySelector('button');
  assert.equal(live.disabled, false);
  live.click(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.equal(harness.element('workspace-status-bar').hidden, false);
  assert.match(harness.element('workspace-status-text').textContent, /execution plan/i);
  assert.equal(harness.element('hide-terminal').closest('[hidden]'), null);
  assert.equal(harness.element('close-terminal').closest('[hidden]'), null);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_layout').at(-1).args.layout.visible, true);
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 0);
  assert.equal(harness.element('start-codex').disabled, true);
  harness.element('settings-navigation').click(); await flush();
  assert.equal(harness.element('workspace-status-bar').hidden, true);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_layout').at(-1).args.layout.visible, false);
  harness.element('sidebar-session-list').querySelector('button').click(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.equal(harness.element('workspace-status-bar').hidden, false);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_layout').at(-1).args.layout.visible, true);
  checking.resolve({ project, plan, fingerprint: 'plan-a' }); await flush();
  assert.equal(harness.element('workspace-status-bar').hidden, true);
  assert.equal(harness.element('hide-terminal').closest('[hidden]'), null);
  assert.equal(harness.element('close-terminal').closest('[hidden]'), null);
  harness.element('hide-terminal').click(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_layout').at(-1).args.layout.visible, false);
  harness.dom.window.close();
});

test('a failed process close keeps the terminal visible and permits a retry', async () => {
  const close = deferred();
  let fail = true;
  const harness = await interfaceHarness({ terminal_close: ({ identifier }) => {
    if (fail) return close.promise;
    harness.terminals.delete(identifier);
  } });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  const layoutCount = harness.calls.filter(call => call.command === 'terminal_layout').length;
  harness.element('close-terminal').click(); await flush();
  harness.resizeTerminal(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'terminal_layout').length, layoutCount);
  close.reject(new Error('Still running')); await flush();
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.equal(harness.terminals.size, 1);
  assert.match(harness.element('terminal-status').textContent, /Could not close the terminal.*Still running/);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_layout').length, layoutCount + 1);
  assert.equal(harness.element('close-terminal').disabled, false);
  fail = false; harness.element('close-terminal').click(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.terminals.size, 0);
  harness.dom.window.close();
});

test('local and guest actions explain a conflicting process without entering the chat', async () => {
  for (const [button, status] of [['start-terminal', 'local-terminal-status'], ['guest-shell', 'guest-terminal-status']]) {
    const harness = await interfaceHarness();
    await harness.ready; harness.element('choose-folder').click(); await flush();
    harness.element('start-codex').click(); await flush();
    harness.element('project-settings-button').click(); await flush();
    const focusCalls = harness.calls.filter(call => call.command === 'terminal_focus').length;
    harness.element(button).focus(); harness.element(button).click(); await flush();
    assert.equal(harness.element('project-settings-dialog').open, true);
    assert.match(harness.element(status).textContent, /End the running chat before opening/);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').length, focusCalls);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_start').length, 0);
    assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 1);
    assert.equal(harness.dom.window.document.activeElement, harness.element(button));
    harness.dom.window.close();
  }
});

test('matching local and guest actions restore their own terminal only', async () => {
  for (const [button, purpose] of [['start-terminal', 'local'], ['guest-shell', 'shell']]) {
    const harness = await interfaceHarness();
    await harness.ready; harness.element('choose-folder').click(); await flush();
    harness.setTerminal({ identifier: 70, projectId: project.id, purpose, running: true, exitCode: null, conversationId: null, busy: false });
    await harness.listeners.get('terminal-sessions-changed')({ payload: project.id });
    harness.element('project-settings-button').click(); await flush();
    harness.element(button).click(); await flush();
    assert.equal(harness.element('project-settings-dialog').open, false);
    assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, 70);
    assert.equal(harness.calls.filter(call => ['terminal_start', 'start_agent_session'].includes(call.command)).length, 0);
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

test('stopping Brig refreshes the surviving local shell and completed output', async () => {
  let current = { ...project };
  const h = await interfaceHarness({
    stop_project: () => { current = { ...project, stage: 'ready' }; return current; },
    preview_project: () => ({ project: current, plan, fingerprint: 'plan-a' }),
  });
  h.element('choose-folder').click();
  await flush();
  h.setTerminal({ identifier: 81, projectId: project.id, purpose: 'local', running: true, exitCode: null });
  h.setTerminal({ identifier: 82, projectId: project.id, purpose: 'tests', running: false, exitCode: 0 });
  h.element('project-settings-button').click();
  await flush();
  const before = h.calls.length;
  h.element('stop-project').click();
  await flush();
  assert.ok(h.calls.slice(before).some(call => call.command === 'terminal_sessions'));
  assert.equal(h.element('sidebar-terminal-list').children.length, 2);
  assert.equal(h.calls.filter(call => call.command === 'prepare_environment').length, 0);
  assert.equal(h.element('start-codex').disabled, true);
  assert.equal(h.element('retry-project-plan').textContent, 'Start environment');
  assert.match(h.element('project-plan-status').textContent, /environment is stopped/);
  h.dom.window.close();
});

test('new chats default to the Brig boundary and preserve an explicit sandbox choice without boundary checks', async () => {
  for (const fullAccess of [true, false]) {
    const h = await interfaceHarness();
    await h.ready;
    h.element('choose-folder').click(); await flush();
    h.element('project-settings-button').click(); await flush();
    const choice = h.element('use-brig-boundary');
    assert.equal(choice.disabled, false);
    assert.equal(choice.checked, true);
    if (!fullAccess) choice.click();
    h.element('close-project-settings').click(); await flush();
    h.element('start-codex').click(); await flush();
    assert.equal(h.calls.find(call => call.command === 'start_agent_session').args.useBrigBoundary, fullAccess);
    assert.equal(h.calls.filter(call => call.command === 'run_boundary_tests').length, 0);
    h.dom.window.close();
  }
});


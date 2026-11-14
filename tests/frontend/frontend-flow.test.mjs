import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { root, flush, deferred, project, plan, emptyUsage, unknownLineage, conversationFixture, sessionDashboard, sessionDetails, settings, recordsLocation, sharedAccount, sharedClaudeAccount, providerLogin, captureProjectRescans, interfaceHarness } from './frontend-harness.mjs';

test('initial project reads show skeletons and then show records or a real empty state', async () => {
  for (const records of [[], [project]]) {
    const pending = deferred();
    const harness = await interfaceHarness({ rescan_projects: () => pending.promise });
    assert.equal(harness.element('recent-projects').getAttribute('aria-busy'), 'true');
    assert.equal(harness.element('empty-projects').hidden, true);
    assert.equal(harness.element('recent-project-list').querySelectorAll('.skeleton-row').length, 3);
    assert.equal(harness.element('sidebar-project-list').querySelectorAll('.skeleton-row').length, 3);
    pending.resolve(records); await harness.ready;
    assert.equal(harness.element('recent-projects').getAttribute('aria-busy'), 'false');
    assert.equal(harness.element('recent-project-list').querySelector('.skeleton'), null);
    assert.equal(harness.element('empty-projects').hidden, records.length > 0);
    assert.equal(harness.element('recent-projects').hidden, records.length === 0);
    harness.dom.window.close();
  }
});

test('project folder loading uses the workspace status bar without shifting the library', async () => {
  const scan = deferred();
  const harness = await interfaceHarness({ rescan_projects: () => scan.promise });
  const bar = harness.element('workspace-status-bar');
  assert.equal(bar.hidden, false);
  assert.match(bar.textContent, /Checking project folders/);
  assert.equal(bar.getAttribute('role'), 'status');
  assert.equal(bar.getAttribute('aria-live'), 'polite');
  assert.equal(bar.parentElement.className, 'workspace');
  assert.equal(bar.previousElementSibling.tagName, 'MAIN');
  assert.equal(harness.element('workspace-status-text').title, harness.element('workspace-status-text').textContent);
  assert.equal(harness.element('project-rescan-status').hidden, true);
  scan.resolve([]); await harness.ready; await flush();
  assert.equal(bar.hidden, true);
  harness.dom.window.close();
});

test('overlapping project loads use one fixed status bar and clear it after both finish', async () => {
  const planCheck = deferred(), saved = deferred(), current = deferred();
  const harness = await interfaceHarness({
    recent_projects: () => [project],
    preview_project: () => planCheck.promise,
    saved_project_dashboard: () => saved.promise,
    project_dashboard: () => current.promise,
  });
  await harness.ready;
  const bar = harness.element('workspace-status-bar');
  assert.equal(bar.hidden, true);
  harness.element('recent-project-list').querySelector('.recent-project').click(); await flush();
  assert.equal(bar.hidden, false);
  assert.match(bar.textContent, /execution plan/i);
  assert.equal(harness.element('project-plan-feedback').hidden, true);
  assert.equal(harness.element('dashboard-session-status').hidden, true);
  assert.equal(harness.element('session-list-status').hidden, true);
  saved.resolve(sessionDashboard([{ id: 'saved-chat', title: 'Saved chat', updatedAt: 1000, available: false }], { warnings: ['Showing saved sessions while checking for updates.'] })); await flush();
  assert.match(harness.element('session-table-body').textContent, /Saved chat/);
  assert.equal(harness.element('dashboard-session-status').hidden, true);
  assert.match(bar.textContent, /execution plan/i);
  planCheck.resolve({ project, plan, fingerprint: 'plan-a' }); await flush();
  assert.equal(bar.hidden, false);
  assert.match(bar.textContent, /sessions/i);
  current.resolve(sessionDashboard([{ id: 'saved-chat', title: 'Current chat', updatedAt: 2000 }])); await flush();
  assert.equal(bar.hidden, true);
  harness.dom.window.close();
});

test('status bar follows the visible view and keeps plan errors and retry actions inline', async () => {
  const checking = deferred();
  const harness = await interfaceHarness({ recent_projects: () => [project], preview_project: () => checking.promise });
  await harness.ready; harness.element('recent-project-list').querySelector('.recent-project').click(); await flush();
  const bar = harness.element('workspace-status-bar');
  assert.match(bar.textContent, /execution plan/i);
  harness.element('settings-navigation').click(); await flush();
  assert.equal(bar.hidden, true);
  harness.element('sidebar-current-project').click(); await flush();
  assert.equal(bar.hidden, false);
  checking.reject(new Error('Plan check failed.')); await flush();
  assert.equal(bar.hidden, true);
  assert.equal(harness.element('project-plan-feedback').hidden, false);
  assert.equal(harness.element('retry-project-plan').hidden, false);
  assert.match(harness.element('project-plan-status').textContent, /Plan check failed/);
  harness.dom.window.close();
});

test('a late plan result cannot clear the status for another project', async () => {
  const first = deferred(), second = deferred();
  const other = { ...project, id: 'project-b', environmentId: 'b'.repeat(32), name: 'Second project' };
  const harness = await interfaceHarness({
    recent_projects: () => [project, other],
    open_project: ({ identifier }) => identifier === project.id ? project : other,
    preview_project: ({ identifier }) => identifier === project.id ? first.promise : second.promise,
  });
  await harness.ready;
  harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').click(); await flush();
  harness.element('sidebar-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
  const bar = harness.element('workspace-status-bar');
  assert.match(bar.textContent, /execution plan/i);
  first.resolve({ project, plan, fingerprint: 'plan-a' }); await flush();
  assert.equal(bar.hidden, false);
  assert.match(bar.textContent, /execution plan/i);
  assert.equal(harness.element('start-codex').disabled, true);
  second.resolve({ project: other, plan, fingerprint: 'plan-b' }); await flush();
  assert.equal(bar.hidden, true);
  harness.dom.window.close();
});

test('settings progress uses the status bar only on the relevant settings page', async () => {
  const check = deferred(), save = deferred();
  const harness = await interfaceHarness({ inspect_host: () => check.promise, save_settings: () => save.promise });
  await harness.ready;
  const bar = harness.element('workspace-status-bar');
  assert.equal(bar.hidden, true);
  harness.element('settings-navigation').click(); await flush();
  assert.equal(bar.hidden, true);
  harness.element('settings-tab-system-checks').click(); await flush();
  assert.equal(bar.hidden, false);
  assert.match(bar.textContent, /Checking this Mac/);
  assert.equal(harness.element('host-status').hidden, true);
  check.resolve({ architecture: 'arm64', osVersion: '27.2', tools: {}, brigSource: 'bundled', brigStatus: 'installed', version: null, doctor: null, message: null }); await flush();
  assert.equal(bar.hidden, true);
  assert.equal(harness.element('host-status').hidden, false);
  harness.element('settings-tab-general').click();
  harness.element('save-settings').click(); await flush();
  assert.match(bar.textContent, /Saving settings/);
  assert.equal(harness.element('settings-status').hidden, true);
  save.resolve({ recorded: 'off', effective: 'off', message: null }); await flush();
  assert.equal(bar.hidden, true);
  assert.equal(harness.element('settings-status').hidden, false);
  assert.match(harness.element('settings-status').textContent, /Settings saved/);
  harness.dom.window.close();
});

test('API cost information opens beside the total and stays open when usage updates', async () => {
  const saved = deferred(), current = deferred(), planCheck = deferred();
  const chat = { id: 'cost-chat', title: 'Cost chat', updatedAt: 1000 };
  const harness = await interfaceHarness({ saved_project_dashboard: () => saved.promise, project_dashboard: () => current.promise, preview_project: () => planCheck.promise });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const details = harness.element('usage-info');
  assert.equal(details.hidden, true);
  saved.resolve(sessionDashboard([chat])); await flush();
  assert.equal(details.hidden, false);
  assert.equal(details.open, false);
  const control = details.querySelector('summary');
  assert.equal(control.getAttribute('aria-label'), 'About total API cost');
  assert.equal(details.closest('dt').textContent.includes('Total API cost'), true);
  assert.match(harness.element('usage-note').textContent, /Token API equivalent in USD.*Includes archived and deleted sessions.*OpenAI catalogue checked 2026-11-10.*Each request retains its dated rates.*Cache writes 0.*ChatGPT subscription billing is separate/s);
  control.focus(); control.click();
  assert.equal(details.open, true);
  const updated = sessionDashboard([chat], { usage: { ...emptyUsage, unpricedRequests: 1 } });
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, updated] });
  assert.equal(harness.element('usage-info'), details);
  assert.equal(details.open, true);
  assert.equal(harness.dom.window.document.activeElement, control);
  assert.match(harness.element('usage-note').textContent, /1 request\(s\) unpriced/);
  harness.dom.window.document.body.click();
  assert.equal(details.open, false);
  control.click();
  harness.dom.window.document.dispatchEvent(new harness.dom.window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  assert.equal(details.open, false);
  assert.equal(harness.dom.window.document.activeElement, control);
  planCheck.resolve({ project, plan, fingerprint: 'plan-a' }); current.resolve(updated); await flush();
  harness.dom.window.close();
});

test('switching projects closes the API cost information', async () => {
  const second = { ...project, id: 'project-b', environmentId: 'b'.repeat(32), name: 'Second project' };
  const harness = await interfaceHarness({ recent_projects: () => [project, second], open_project: ({ identifier }) => identifier === project.id ? project : second });
  await harness.ready;
  harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').click(); await flush();
  const details = harness.element('usage-info');
  details.querySelector('summary').click();
  assert.equal(details.open, true);
  harness.element('sidebar-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
  assert.equal(details.open, false);
  harness.dom.window.close();
});

test('failed initial project reads remove skeletons and show an error without an empty state', async () => {
  const pending = deferred();
  const harness = await interfaceHarness({ rescan_projects: () => pending.promise });
  pending.reject(new Error('Records drive unavailable')); await harness.ready;
  assert.equal(harness.element('recent-project-list').querySelector('.skeleton'), null);
  assert.equal(harness.element('recent-projects').getAttribute('aria-busy'), 'false');
  assert.equal(harness.element('empty-projects').hidden, true);
  assert.match(harness.element('project-rescan-status').textContent, /Records drive unavailable/);
  harness.dom.window.close();
});

test('agent profile edits survive tab changes and save both agents with their host defaults', async () => {
  const saved = { ...settings, codex: { profile: 'custom-codex', allowedHosts: ['private.example'] }, claudeCode: { profile: 'custom-claude', allowedHosts: ['api.anthropic.com', 'platform.claude.com'] } };
  const harness = await interfaceHarness({ load_settings: () => saved, save_settings: () => ({ recorded: 'off', effective: 'off', message: null }) });
  await harness.ready;
  harness.element('settings-navigation').click();
  harness.element('settings-tab-agent-profile').click();
  assert.equal(harness.element('codex-profile').value, 'custom-codex');
  assert.equal(harness.element('claude-code-profile').value, 'custom-claude');
  harness.element('codex-profile').value = 'changed-codex';
  harness.element('claude-code-profile').value = 'changed-claude';
  harness.element('settings-tab-general').click();
  harness.element('cpu-count').value = '6';
  harness.element('settings-tab-general').dispatchEvent(new harness.dom.window.KeyboardEvent('keydown', { key: 'ArrowRight', cancelable: true }));
  assert.equal(harness.dom.window.document.activeElement, harness.element('settings-tab-agent-profile'));
  assert.equal(harness.element('settings-panel-agent-profile').hidden, false);
  assert.equal(harness.element('codex-profile').value, 'changed-codex');
  assert.equal(harness.element('claude-code-profile').value, 'changed-claude');
  harness.element('save-agent-profile').click();
  await flush();
  const request = harness.calls.find(call => call.command === 'save_settings');
  assert.equal(request.args.settings.schemaVersion, 2);
  assert.equal(request.args.settings.codex.profile, 'changed-codex');
  assert.deepEqual(Array.from(request.args.settings.codex.allowedHosts), saved.codex.allowedHosts);
  assert.deepEqual(Array.from(request.args.settings.claudeCode.allowedHosts), saved.claudeCode.allowedHosts);
  assert.equal(request.args.settings.claudeCode.profile, 'changed-claude');
  assert.equal(request.args.settings.cpus, 6);
  harness.dom.window.close();
});

test('external settings changes update untouched fields and preserve local edits', async () => {
  const changed = { ...settings, cpus: 8, memoryMb: 8192, codex: { ...settings.codex, profile: 'external-profile' } };
  let saved;
  const harness = await interfaceHarness({ save_settings: ({ settings: value }) => { saved = value; return { recorded: 'off', effective: 'off', message: null }; } });
  await harness.ready;
  harness.element('settings-navigation').click();
  harness.element('cpu-count').value = '6';
  harness.element('cpu-count').dispatchEvent(new harness.dom.window.Event('input', { bubbles: true }));
  harness.handlers.load_settings = () => changed;
  await harness.listeners.get('records-changed')({ payload: ['settings'] }); await flush();
  assert.equal(harness.element('cpu-count').value, '6');
  assert.equal(harness.element('memory-mb').value, '8192');
  assert.equal(harness.element('codex-profile').value, 'external-profile');
  harness.element('save-settings').click(); await flush();
  assert.equal(saved.cpus, 6);
  assert.equal(saved.memoryMb, 8192);
  assert.equal(saved.codex.profile, 'external-profile');
  harness.dom.window.close();
});

test('a changed project record refreshes the project list without reopening the page', async () => {
  let records = [project];
  const harness = await interfaceHarness({ recent_projects: () => records });
  await harness.ready;
  assert.match(harness.element('recent-project-list').textContent, /örnek proje/);
  records = [{ ...project, name: 'Renamed externally' }];
  await harness.listeners.get('records-changed')({ payload: ['projects'] }); await flush();
  assert.match(harness.element('recent-project-list').textContent, /Renamed externally/);
  harness.dom.window.close();
});

test('external project changes update the open project and untouched settings fields', async () => {
  let current = project;
  let options = { cpus: 4, memoryMb: 4096, verification: 'require', network: 'isolated' };
  let saved;
  const harness = await interfaceHarness({
    recent_projects: () => [current],
    read_project_preferences: () => ({ project: current, options }),
    preview_project: () => ({ project: current, plan, fingerprint: 'plan-a' }),
    prepare_project: () => ({ project: current, plan, fingerprint: 'plan-a', candidates: [], message: null }),
    save_project_preferences: value => { saved = value; current = { ...current, name: value.name }; return current; },
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  const reads = () => harness.calls.filter(call => call.command === 'read_project_preferences').length;
  assert.equal(reads(), 1);
  harness.element('close-project-settings').click();
  harness.element('project-settings-button').click(); await flush();
  assert.equal(reads(), 1);
  harness.element('project-cpus').value = '6';
  current = { ...current, name: 'External name' };
  options = { ...options, cpus: 8, memoryMb: 8192 };
  await harness.listeners.get('records-changed')({ payload: ['projects'] }); await flush();
  assert.equal(harness.element('project-heading').textContent, 'External name');
  assert.equal(harness.element('project-name').value, 'External name');
  assert.equal(harness.element('project-cpus').value, '6');
  assert.equal(harness.element('project-memory').value, '8192');
  harness.element('project-preferences').dispatchEvent(new harness.dom.window.Event('submit', { bubbles: true, cancelable: true })); await flush();
  assert.equal(saved.options.cpus, 6);
  assert.equal(saved.options.memoryMb, 8192);
  harness.dom.window.close();
});

test('renaming a project does not reload its execution plan', async () => {
  let current = project;
  const harness = await interfaceHarness({ recent_projects: () => [current], read_project_preferences: () => ({ project: current, options: settings }) });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const count = harness.calls.filter(call => call.command === 'preview_project').length;
  current = { ...project, name: 'External name' };
  await harness.listeners.get('records-changed')({ payload: ['projects'] }); await flush();
  assert.equal(harness.element('project-heading').textContent, 'External name');
  assert.equal(harness.calls.filter(call => call.command === 'preview_project').length, count);
  harness.dom.window.close();
});

test('a project change during a plan check schedules a new check before session start', async () => {
  const oldPlan = deferred(), newPlan = deferred();
  let checks = 0, current = project;
  let options = { cpus: 4, memoryMb: 4096, verification: 'require', network: 'isolated' };
  const harness = await interfaceHarness({
    recent_projects: () => [current], read_project_preferences: () => ({ project: current, options }),
    preview_project: () => {
      checks++;
      if (checks === 1) return { project: current, plan, fingerprint: 'initial' };
      return checks === 2 ? oldPlan.promise : newPlan.promise;
    },
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('project-settings-button').click(); await flush(); harness.element('close-project-settings').click();
  harness.element('refresh-plan').click(); await flush();
  options = { ...options, memoryMb: 8192 };
  await harness.listeners.get('records-changed')({ payload: ['projects'] }); await flush();
  oldPlan.resolve({ project: current, plan, fingerprint: 'outdated' }); await flush();
  assert.equal(checks, 3);
  assert.equal(harness.element('start-codex').disabled, true);
  newPlan.resolve({ project: current, plan, fingerprint: 'updated' }); await flush();
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.find(call => call.command === 'start_agent_session').args.fingerprint, 'updated');
  harness.dom.window.close();
});

test('a project change during save is applied after the save finishes', async () => {
  const saving = deferred();
  let current = project;
  let options = { cpus: 4, memoryMb: 4096, verification: 'require', network: 'isolated' };
  const harness = await interfaceHarness({
    recent_projects: () => [current], read_project_preferences: () => ({ project: current, options }),
    save_project_preferences: () => saving.promise,
    prepare_project: () => ({ project: current, plan, fingerprint: 'plan-a', candidates: [], message: null }),
    preview_project: () => ({ project: current, plan, fingerprint: 'plan-a' }),
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  harness.element('project-cpus').value = '6';
  harness.element('project-preferences').dispatchEvent(new harness.dom.window.Event('submit', { bubbles: true, cancelable: true })); await flush();
  current = { ...project, name: 'Changed during save' };
  options = { ...options, cpus: 6, memoryMb: 8192 };
  await harness.listeners.get('records-changed')({ payload: ['projects'] }); await flush();
  saving.resolve(project); await flush();
  assert.equal(harness.element('project-heading').textContent, 'Changed during save');
  assert.equal(harness.element('project-name').value, 'Changed during save');
  assert.equal(harness.element('project-cpus').value, '6');
  assert.equal(harness.element('project-memory').value, '8192');
  harness.dom.window.close();
});

test('a delayed project record refresh cannot replace a newly selected project', async () => {
  const pending = deferred();
  const other = { ...project, id: 'project-b', name: 'Project B', path: '/fixture/b' };
  const records = [project, other];
  const harness = await interfaceHarness({
    recent_projects: () => records,
    open_project: ({ identifier }) => records.find(item => item.id === identifier),
    read_project_preferences: () => pending.promise,
  });
  await harness.ready; harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').click(); await flush();
  const refresh = harness.listeners.get('records-changed')({ payload: ['projects'] }); await flush();
  harness.element('project-navigation').click(); await flush();
  harness.element('recent-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
  pending.resolve({ project: { ...project, name: 'Stale A' }, options: settings });
  await refresh; await flush();
  assert.equal(harness.element('project-heading').textContent, 'Project B');
  harness.dom.window.close();
});

test('removing a project record clears its cached sessions and blocks session start', async () => {
  let records = [project];
  const harness = await interfaceHarness({ recent_projects: () => records, project_dashboard: () => sessionDashboard([{ id: 'chat', title: 'Removed project chat', updatedAt: 1000 }]) });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.match(harness.element('session-table-body').textContent, /Removed project chat/);
  records = [];
  await harness.listeners.get('records-changed')({ payload: ['projects'] }); await flush();
  assert.doesNotMatch(harness.element('session-table-body').textContent, /Removed project chat/);
  assert.equal(harness.element('start-codex').disabled, true);
  assert.match(harness.element('folder-error').textContent, /no longer in the saved records/);
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, sessionDashboard([{ id: 'chat', title: 'Late removed chat', updatedAt: 2000 }])] });
  assert.doesNotMatch(harness.element('session-table-body').textContent, /Late removed chat/);
  records = [project];
  await harness.listeners.get('records-changed')({ payload: ['projects'] }); await flush();
  assert.match(harness.element('session-table-body').textContent, /Removed project chat/);
  assert.equal(harness.element('folder-error').hidden, true);
  assert.equal(harness.element('start-codex').disabled, false);
  harness.dom.window.close();
});

test('a delayed project-list refresh cannot remove a project registered after the read began', async () => {
  const pending = deferred();
  const harness = await interfaceHarness();
  await harness.ready;
  harness.handlers.recent_projects = () => pending.promise;
  const refresh = harness.listeners.get('records-changed')({ payload: ['projects'] }); await flush();
  harness.element('choose-folder').click(); await flush();
  pending.resolve([]); await refresh; await flush();
  assert.equal(harness.element('project-heading').textContent, project.name);
  assert.doesNotMatch(harness.element('folder-error').textContent, /no longer in the saved records/);
  assert.equal(harness.element('start-codex').disabled, false);
  harness.dom.window.close();
});

test('cached settings for other projects update without being discarded', async () => {
  let first = project;
  const second = { ...project, id: 'project-b', environmentId: 'b'.repeat(32), name: 'Project B', path: '/fixture/b' };
  let firstOptions = { cpus: 4, memoryMb: 4096, verification: 'require', network: 'isolated' };
  const records = () => [first, second];
  const harness = await interfaceHarness({
    recent_projects: records,
    open_project: ({ identifier }) => records().find(item => item.id === identifier),
    read_project_preferences: ({ identifier }) => ({ project: records().find(item => item.id === identifier), options: identifier === first.id ? firstOptions : settings }),
  });
  await harness.ready;
  harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  harness.element('close-project-settings').click(); harness.element('project-navigation').click(); await flush();
  harness.element('recent-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
  first = { ...first, name: 'Updated project A' };
  firstOptions = { ...firstOptions, memoryMb: 8192 };
  await harness.listeners.get('records-changed')({ payload: ['projects'] }); await flush();
  const reads = () => harness.calls.filter(call => call.command === 'read_project_preferences' && call.args.identifier === first.id).length;
  assert.equal(reads(), 2);
  harness.element('project-navigation').click(); await flush();
  harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  assert.equal(reads(), 2);
  assert.equal(harness.element('project-name').value, 'Updated project A');
  assert.equal(harness.element('project-memory').value, '8192');
  harness.dom.window.close();
});

test('reopening settings uses the stored provider statuses', async () => {
  const harness = await interfaceHarness();
  await harness.ready;
  const initialChatgpt = harness.calls.filter(call => call.command === 'shared_chatgpt_status').length;
  const initialClaude = harness.calls.filter(call => call.command === 'shared_claude_status').length;
  harness.element('settings-navigation').click(); await flush();
  harness.element('project-navigation').click(); await flush();
  harness.element('settings-navigation').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'shared_chatgpt_status').length, initialChatgpt);
  assert.equal(harness.calls.filter(call => call.command === 'shared_claude_status').length, initialClaude);
  await harness.listeners.get('shared-claude-changed')({ payload: { ...sharedClaudeAccount, generation: 2, email: 'updated@example.invalid' } });
  assert.equal(harness.element('claude-account-email').textContent, 'updated@example.invalid');
  harness.dom.window.close();
});

test('a usage database change updates the visible dashboard and cannot replace a newer event', async () => {
  const read = deferred();
  const harness = await interfaceHarness({
    project_dashboard: () => sessionDashboard([{ id: 'chat', title: 'Initial', updatedAt: 1000 }], { sequence: 1 }),
    recorded_project_dashboard: () => read.promise,
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const pending = harness.listeners.get('records-changed')({ payload: ['usage'] }); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'recorded_project_dashboard').length, 1);
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, sessionDashboard([{ id: 'chat', title: 'Newer event', updatedAt: 3000 }], { sequence: 3 })] });
  read.resolve(sessionDashboard([{ id: 'chat', title: 'Older read', updatedAt: 2000 }], { sequence: 2 }));
  await pending; await flush();
  assert.match(harness.element('session-table-body').textContent, /Newer event/);
  assert.equal(harness.calls.filter(call => call.command === 'preview_project').length, 1);
  harness.dom.window.close();
});

test('usage details reopen from memory and refresh after a database change', async () => {
  const conversation = { id: 'chat', title: 'Cached usage', updatedAt: 1000 };
  let model = 'First model';
  const dashboard = () => sessionDashboard([conversation]);
  const harness = await interfaceHarness({
    project_dashboard: dashboard,
    recorded_project_dashboard: dashboard,
    conversation_usage: ({ sessionId }) => sessionDetails(sessionId, [{ id: 'turn', status: 'complete', requests: [{ at: 'now', model, tokens: { input: 1, cached: 0, cacheWrite: 0, output: 1 }, costUsd: 0, unpricedReason: null, rates: null }] }]),
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  await openUsage(harness, conversation.title);
  assert.match(harness.element('session-usage-details').textContent, /First model/);
  harness.element('close-session-usage').click();
  await openUsage(harness, conversation.title);
  assert.equal(harness.calls.filter(call => call.command === 'conversation_usage').length, 1);
  model = 'Changed model';
  await harness.listeners.get('records-changed')({ payload: ['usage'] }); await flush();
  assert.match(harness.element('session-usage-details').textContent, /Changed model/);
  assert.equal(harness.calls.filter(call => call.command === 'conversation_usage').length, 2);
  harness.dom.window.close();
});

test('a database refresh does not mark saved sessions as verified', async () => {
  const checking = deferred();
  const conversation = { id: 'chat', title: 'Saved chat', updatedAt: 1000 };
  const harness = await interfaceHarness({
    saved_project_dashboard: () => sessionDashboard([{ ...conversation, available: false }]),
    recorded_project_dashboard: () => sessionDashboard([conversation], { sequence: 1 }),
    preview_project: () => checking.promise,
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  await harness.listeners.get('records-changed')({ payload: ['usage'] }); await flush();
  assert.equal(harness.element('session-table-body').querySelector('.session-title').disabled, true);
  assert.match(harness.element('session-table-body').textContent, /Session availability not verified/);
  assert.equal(harness.element('start-codex').disabled, true);
  checking.resolve({ project, plan, fingerprint: 'verified-plan' }); await flush();
  harness.dom.window.close();
});

test('a newer database snapshot keeps backend session verification when the earlier response is late', async () => {
  const verified = deferred();
  const conversation = { id: 'chat', title: 'Saved chat', updatedAt: 1000 };
  const harness = await interfaceHarness({
    saved_project_dashboard: () => sessionDashboard([{ ...conversation, available: false }]),
    project_dashboard: () => verified.promise,
    recorded_project_dashboard: () => sessionDashboard([{ ...conversation, title: 'Latest saved usage' }], { sequence: 2, availabilityVerified: true }),
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.equal(harness.element('session-table-body').querySelector('.session-title').disabled, true);
  await harness.listeners.get('records-changed')({ payload: ['usage'] }); await flush();
  assert.equal(harness.element('session-table-body').querySelector('.session-title').disabled, false);
  verified.resolve(sessionDashboard([conversation], { sequence: 1 })); await flush();
  assert.match(harness.element('session-table-body').textContent, /Latest saved usage/);
  assert.equal(harness.element('session-table-body').querySelector('.session-title').disabled, false);
  harness.dom.window.close();
});

test('General settings load and save each agent host list independently', async () => {
  const saved = { ...settings, codex: { profile: 'custom-codex', allowedHosts: ['auth.openai.com', 'chatgpt.com', 'api.openai.com'] }, claudeCode: { profile: 'custom-claude', allowedHosts: ['api.anthropic.com', 'platform.claude.com', 'claude.ai'] } };
  const harness = await interfaceHarness({ load_settings: () => saved, save_settings: () => ({ recorded: 'off', effective: 'off', message: null }) });
  await harness.ready;
  harness.element('settings-navigation').click();
  for (const [id, agent, hosts] of [['allowed-hosts', 'Codex', saved.codex.allowedHosts], ['claude-code-allowed-hosts', 'Claude Code', saved.claudeCode.allowedHosts]]) {
    const control = harness.element(id);
    assert.equal(control.closest('[role="tabpanel"]').id, 'settings-panel-general');
    assert.match(control.labels[0].textContent, new RegExp('new ' + agent + ' policies'));
    assert.equal(control.value, hosts.join('\n'));
  }
  harness.element('allowed-hosts').value = ' codex.example\n\ncodex.example\napi.openai.com ';
  harness.element('claude-code-allowed-hosts').value = ' claude.example\n\nclaude.example\napi.anthropic.com ';
  harness.element('settings-tab-agent-profile').click();
  harness.element('settings-tab-general').click();
  harness.element('save-settings').click(); await flush();
  const request = harness.calls.find(call => call.command === 'save_settings');
  assert.equal(request.args.settings.schemaVersion, 2);
  assert.equal(request.args.settings.codex.profile, saved.codex.profile);
  assert.equal(request.args.settings.claudeCode.profile, saved.claudeCode.profile);
  assert.deepEqual(Array.from(request.args.settings.codex.allowedHosts), ['codex.example', 'api.openai.com']);
  assert.deepEqual(Array.from(request.args.settings.claudeCode.allowedHosts), ['claude.example', 'api.anthropic.com']);
  assert.deepEqual(harness.calls.filter(call => /policy/.test(call.command)), []);
  harness.dom.window.close();
});

test('empty agent host lists remain empty and can be saved after clearing existing hosts', async () => {
  for (const [codexHosts, claudeHosts, clearHosts] of [[[], ['claude.example'], false], [['codex.example'], [], false], [[], [], false], [['codex.example'], ['claude.example'], true]]) {
    const saved = { ...settings, codex: { ...settings.codex, allowedHosts: codexHosts }, claudeCode: { ...settings.claudeCode, allowedHosts: claudeHosts } };
    const harness = await interfaceHarness({ load_settings: () => saved, save_settings: () => ({ recorded: 'off', effective: 'off', message: null }) });
    await harness.ready;
    harness.element('settings-navigation').click();
    for (const [id, allowedHosts] of [['allowed-hosts', codexHosts], ['claude-code-allowed-hosts', claudeHosts]]) {
      assert.equal(harness.element(id).value, allowedHosts.join('\n'));
      if (clearHosts) harness.element(id).value = '';
    }
    harness.element('save-settings').click(); await flush();
    const request = harness.calls.find(call => call.command === 'save_settings');
    assert.deepEqual(Array.from(request.args.settings.codex.allowedHosts), clearHosts ? [] : codexHosts);
    assert.deepEqual(Array.from(request.args.settings.claudeCode.allowedHosts), clearHosts ? [] : claudeHosts);
    harness.dom.window.close();
  }
});

test('settings validation opens and focuses the first invalid field across tabs', async () => {
  const harness = await interfaceHarness();
  await harness.ready;
  harness.element('settings-navigation').click();
  harness.element('codex-profile').value = '';
  harness.element('cpu-count').value = '0';
  harness.element('settings-tab-agent-profile').click();
  harness.element('save-agent-profile').click();
  assert.equal(harness.element('settings-panel-general').hidden, false);
  assert.equal(harness.dom.window.document.activeElement, harness.element('cpu-count'));
  harness.element('cpu-count').value = '4';
  harness.element('save-settings').click();
  assert.equal(harness.element('settings-panel-agent-profile').hidden, false);
  assert.equal(harness.dom.window.document.activeElement, harness.element('codex-profile'));
  harness.element('codex-profile').value = 'codex';
  harness.element('claude-code-profile').value = '';
  harness.element('settings-tab-general').click();
  harness.element('save-settings').click();
  assert.equal(harness.element('settings-panel-agent-profile').hidden, false);
  assert.equal(harness.dom.window.document.activeElement, harness.element('claude-code-profile'));
  assert.equal(harness.calls.filter(call => call.command === 'save_settings').length, 0);
  await flush();
  harness.dom.window.close();
});

test('settings use skeletons for reads and a button loader for saves with error recovery', async () => {
  const read = deferred(), save = deferred();
  const harness = await interfaceHarness({ load_settings: () => read.promise, save_settings: () => save.promise });
  assert.equal(harness.element('settings-fields').getAttribute('aria-busy'), 'true');
  assert.equal(harness.element('settings-fields').disabled, true);
  for (const id of ['codex-profile', 'claude-code-profile', 'allowed-hosts', 'claude-code-allowed-hosts', 'save-agent-profile']) assert.equal(harness.element(id).matches(':disabled'), true);
  assert.equal(harness.element('agent-profile-fields').getAttribute('aria-busy'), 'true');
  read.resolve(settings); await harness.ready;
  assert.equal(harness.element('settings-fields').getAttribute('aria-busy'), 'false');
  assert.equal(harness.element('agent-profile-fields').getAttribute('aria-busy'), 'false');
  assert.equal(harness.element('allowed-hosts').value, '');
  assert.equal(harness.element('claude-code-allowed-hosts').value, 'api.anthropic.com');
  harness.element('allowed-hosts').value = 'codex.example';
  harness.element('claude-code-allowed-hosts').value = 'claude.example';
  harness.element('settings-navigation').click();
  harness.element('settings-tab-agent-profile').click();
  harness.element('save-agent-profile').click(); await flush();
  assert.equal(harness.element('save-settings').getAttribute('aria-busy'), 'true');
  assert.equal(harness.element('save-settings').disabled, true);
  assert.equal(harness.element('save-agent-profile').getAttribute('aria-busy'), 'true');
  assert.equal(harness.element('save-agent-profile').disabled, true);
  assert.equal(harness.element('settings-fields').classList.contains('is-loading'), false);
  save.reject(new Error('Could not write settings')); await flush();
  assert.equal(harness.element('save-settings').getAttribute('aria-busy'), 'false');
  assert.equal(harness.element('save-settings').disabled, false);
  assert.match(harness.element('settings-status').textContent, /Could not write settings/);
  assert.match(harness.element('agent-profile-status').textContent, /Could not write settings/);
  assert.equal(harness.element('allowed-hosts').value, 'codex.example');
  assert.equal(harness.element('claude-code-allowed-hosts').value, 'claude.example');
  assert.equal(harness.dom.window.document.activeElement, harness.element('save-agent-profile'));
  assert.equal(harness.element('settings-panel-agent-profile').hidden, false);
  harness.dom.window.close();
});

test('pending settings saves retain the current view and visible focus after navigation', async () => {
  for (const destination of ['settings-tab-providers', 'project-navigation']) {
    const save = deferred();
    const harness = await interfaceHarness({ save_settings: () => save.promise });
    await harness.ready;
    harness.element('settings-navigation').click();
    harness.element('settings-tab-agent-profile').click();
    harness.element('save-agent-profile').click(); await flush();
    const navigation = harness.element(destination);
    navigation.click(); navigation.focus(); await flush();
    const focusedControl = harness.dom.window.document.activeElement;
    save.resolve({ recorded: 'off', effective: 'off', message: null }); await flush();
    assert.equal(harness.element('settings-view').hidden, destination === 'project-navigation');
    assert.equal(harness.dom.window.document.activeElement, focusedControl);
    if (destination === 'settings-tab-providers') assert.equal(harness.element('settings-panel-providers').hidden, false);
    harness.element('settings-tab-general').click();
    assert.match(harness.element('settings-status').textContent, /Settings saved/);
    assert.equal(harness.element('save-settings').disabled, false);
    assert.equal(harness.element('save-agent-profile').disabled, false);
    harness.dom.window.close();
  }
});

test('dashboard skeletons become data and a failed refresh preserves the recorded data', async () => {
  const read = deferred(), refresh = deferred();
  const harness = await interfaceHarness({ saved_project_dashboard: () => read.promise, project_dashboard: () => read.promise });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.equal(harness.element('project-usage').getAttribute('aria-busy'), 'true');
  assert.doesNotMatch(harness.element('project-usage').textContent, /\$0|\(0%\)/);
  assert.equal(harness.element('session-table').hidden, false);
  assert.equal(harness.element('session-table-body').querySelectorAll('.skeleton-row').length, 3);
  assert.equal(harness.element('sidebar-session-list').querySelectorAll('.skeleton-row').length, 3);
  read.resolve(sessionDashboard([{ id: 'loaded-chat', title: 'Recorded chat', updatedAt: 1000 }])); await flush();
  assert.equal(harness.element('project-usage').getAttribute('aria-busy'), 'false');
  assert.equal(harness.element('session-table-body').querySelector('.skeleton'), null);
  harness.handlers.project_dashboard = () => refresh.promise;
  harness.element('refresh-sessions').click(); await flush();
  assert.equal(harness.element('refresh-sessions').getAttribute('aria-busy'), 'true');
  assert.equal(harness.element('project-usage').classList.contains('is-loading'), false);
  assert.match(harness.element('session-table-body').textContent, /Recorded chat/);
  refresh.reject(new Error('Refresh unavailable')); await flush();
  assert.equal(harness.element('refresh-sessions').getAttribute('aria-busy'), 'false');
  assert.match(harness.element('session-table-body').textContent, /Recorded chat/);
  assert.match(harness.element('dashboard-session-status').textContent, /Refresh unavailable/);
  harness.dom.window.close();
});

test('usage skeletons clear after a read error and a closed read cannot clear a new skeleton', async () => {
  const first = deferred(), second = deferred();
  const harness = await interfaceHarness({ project_dashboard: () => sessionDashboard([{ id: 'chat', title: 'Usage chat', updatedAt: 1000 }]), conversation_usage: () => first.promise });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('usage-for-chat').click(); await flush();
  assert.equal(harness.element('session-usage-details').getAttribute('aria-busy'), 'true');
  assert.ok(harness.element('session-usage-details').querySelector('.skeleton'));
  harness.element('close-session-usage').click();
  harness.handlers.conversation_usage = () => second.promise;
  harness.element('usage-for-chat').click(); await flush();
  first.resolve(sessionDetails('chat', [])); await flush();
  assert.equal(harness.element('session-usage-details').getAttribute('aria-busy'), 'true');
  assert.ok(harness.element('session-usage-details').querySelector('.skeleton'));
  second.reject(new Error('Usage read failed')); await flush();
  assert.equal(harness.element('session-usage-details').getAttribute('aria-busy'), 'false');
  assert.equal(harness.element('session-usage-details').querySelector('.skeleton'), null);
  assert.match(harness.element('session-usage-status').textContent, /Usage read failed/);
  harness.dom.window.close();
});

test('project settings discard stale reads without clearing the current skeleton', async () => {
  const first = deferred(), second = deferred(), save = deferred();
  const harness = await interfaceHarness({ read_project_preferences: () => first.promise, save_project_preferences: () => save.promise });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  assert.equal(harness.element('project-preference-fields').getAttribute('aria-busy'), 'true');
  harness.element('close-project-settings').click();
  harness.handlers.read_project_preferences = () => second.promise;
  harness.element('project-settings-button').click(); await flush();
  first.resolve({ project: { ...project, name: 'Stale name' }, options: settings }); await flush();
  assert.equal(harness.element('project-preference-fields').getAttribute('aria-busy'), 'true');
  second.resolve({ project, options: settings }); await flush();
  assert.equal(harness.element('project-preference-fields').getAttribute('aria-busy'), 'false');
  assert.equal(harness.element('project-name').value, project.name);
  harness.element('project-preferences').dispatchEvent(new harness.dom.window.Event('submit', { cancelable: true })); await flush();
  assert.equal(harness.element('save-project-preferences').getAttribute('aria-busy'), 'true');
  assert.equal(harness.element('project-preference-fields').classList.contains('is-loading'), false);
  save.reject(new Error('Project save failed')); await flush();
  assert.equal(harness.element('save-project-preferences').getAttribute('aria-busy'), 'false');
  assert.equal(harness.element('project-preference-fields').disabled, false);
  harness.dom.window.close();
});

test('policy reads show a skeleton and recover from errors before a later successful read', async () => {
  const first = deferred();
  const harness = await interfaceHarness({ read_project_policy: () => first.promise });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  harness.element('read-project-policy').click(); await flush();
  assert.equal(harness.element('read-project-policy').getAttribute('aria-busy'), 'true');
  assert.ok(harness.element('project-policy-document').querySelector('.skeleton'));
  first.reject(new Error('Policy unavailable')); await flush();
  assert.equal(harness.element('read-project-policy').getAttribute('aria-busy'), 'false');
  assert.equal(harness.element('project-policy-document').querySelector('.skeleton'), null);
  harness.handlers.read_project_policy = () => ({ document: { name: 'Recovered policy' }, fingerprint: 'policy', editable: true, attached: true, allowedHosts: [] });
  harness.element('read-project-policy').click(); await flush();
  assert.match(harness.element('project-policy-document').textContent, /Recovered policy/);
  assert.equal(harness.element('project-policy-document').getAttribute('aria-busy'), 'false');
  harness.dom.window.close();
});

test('settings stay disabled until initialization installs the save handler', async () => {
  let release;
  const harness = await interfaceHarness({ telemetry_status: () => new Promise(resolve => { release = resolve; }) });
  assert.equal(harness.element('settings-fields').disabled, true);
  assert.equal(harness.element('agent-profile-fields').disabled, true);
  release({ recorded: 'off', effective: 'off', message: null });
  await harness.ready;
  assert.equal(harness.element('settings-fields').disabled, false);
  assert.equal(harness.element('agent-profile-fields').disabled, false);
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

test('ready removal preview clears its loading message before opening the dialog', async () => {
  const harness = await interfaceHarness({ preview_removal: () => ({ reference: 'codex@sample-a', workspace: '/fixture/guest-home', description: 'Own test session', fingerprint: 'remove-a', external: false, hasEnvironment: false }) });
  await harness.ready;
  const dialog = harness.element('removal-dialog');
  dialog.showModal = () => { dialog.open = true; };
  harness.element('choose-folder').click(); await flush();
  harness.element('preview-removal').click(); await flush();
  assert.equal(dialog.open, true);
  assert.equal(harness.element('operation-status').textContent, '');
  assert.equal(harness.element('logout-on-removal'), null);
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
  assert.equal(harness.element('choose-folder').parentElement.querySelector('select'), null);
  assert.equal(harness.element('empty-projects').hidden, false);
  assert.equal(harness.element('project-rescan-status').hidden, true);
  harness.element('choose-folder').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'register_project').length, 1);
  assert.deepEqual({ ...harness.calls.find(call => call.command === 'register_project').args }, { folder: project.path });
  assert.equal(harness.element('agent-choice').parentElement.nextElementSibling, harness.element('start-codex'));
  assert.match(harness.element('sidebar-current-project').textContent, /örnek proje/);
  harness.element('project-navigation').click(); await flush();
  assert.equal(harness.element('sidebar-project-list').querySelectorAll('button').length, 1);
  assert.equal(harness.element('recent-project-list').querySelectorAll('button').length, 1);
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

test('sidebar resizing clamps pointer and keyboard input, saves the width and preserves workspace space', async () => {
  const harness = await interfaceHarness();
  await harness.ready;
  const handle = harness.element('sidebar-resizer');
  const window = harness.dom.window;
  handle.setPointerCapture = () => {};
  handle.releasePointerCapture = pointerId => handle.dispatchEvent(new window.PointerEvent('lostpointercapture', { pointerId }));
  const key = value => handle.dispatchEvent(new window.KeyboardEvent('keydown', { key: value, bubbles: true }));
  key('ArrowLeft');
  assert.equal(handle.getAttribute('aria-valuenow'), '280');
  key('ArrowRight');
  await flush();
  assert.equal(harness.calls.filter(call => call.command === 'save_interface_preferences').at(-1).args.sidebarWidth, 296);
  handle.dispatchEvent(new window.PointerEvent('pointerdown', { button: 0, pointerId: 1 }));
  handle.dispatchEvent(new window.PointerEvent('pointermove', { pointerId: 1, clientX: 400 }));
  handle.dispatchEvent(new window.PointerEvent('pointerup', { pointerId: 1 }));
  await flush();
  assert.equal(harness.calls.filter(call => call.command === 'save_interface_preferences').at(-1).args.sidebarWidth, 400);
  assert.equal(window.localStorage.getItem('tugboat.sidebarWidth'), null);
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
  await flush();
  harness.dom.window.close();
});

test('sidebar preferences load from the records folder and migrate a legacy browser preference once', async () => {
  const harness = await interfaceHarness({}, dom => { dom.window.localStorage.setItem('tugboat.sidebarWidth', '400'); });
  await harness.ready; await flush();
  assert.equal(harness.element('sidebar-resizer').getAttribute('aria-valuenow'), '400');
  assert.equal(harness.calls.find(call => call.command === 'save_interface_preferences').args.sidebarWidth, 400);
  assert.equal(harness.dom.window.localStorage.getItem('tugboat.sidebarWidth'), null);
  harness.dom.window.close();
  const saved = await interfaceHarness({ load_interface_preferences: () => ({ sidebarWidth: 360 }) });
  await saved.ready;
  assert.equal(saved.element('sidebar-resizer').getAttribute('aria-valuenow'), '360');
  assert.equal(saved.calls.filter(call => call.command === 'save_interface_preferences').length, 0);
  saved.dom.window.close();
});

test('external sidebar changes update the stored width without interrupting a drag', async () => {
  const harness = await interfaceHarness({ load_interface_preferences: () => ({ sidebarWidth: 320 }) });
  await harness.ready; await flush();
  const handle = harness.element('sidebar-resizer');
  assert.equal(handle.getAttribute('aria-valuenow'), '320');
  harness.handlers.load_interface_preferences = () => ({ sidebarWidth: 400 });
  await harness.listeners.get('records-changed')({ payload: ['interface'] }); await flush();
  assert.equal(handle.getAttribute('aria-valuenow'), '400');
  handle.setPointerCapture = () => {};
  handle.dispatchEvent(new harness.dom.window.PointerEvent('pointerdown', { button: 0, pointerId: 1 }));
  handle.dispatchEvent(new harness.dom.window.PointerEvent('pointermove', { pointerId: 1, clientX: 450 }));
  harness.handlers.load_interface_preferences = () => ({ sidebarWidth: 500 });
  await harness.listeners.get('records-changed')({ payload: ['interface'] }); await flush();
  assert.equal(handle.getAttribute('aria-valuenow'), '450');
  harness.dom.window.close();
});

test('a delayed initial sidebar read cannot replace a newer external change', async () => {
  const initial = deferred();
  const harness = await interfaceHarness({ load_interface_preferences: () => initial.promise });
  await harness.ready;
  harness.handlers.load_interface_preferences = () => ({ sidebarWidth: 400 });
  await harness.listeners.get('records-changed')({ payload: ['interface'] }); await flush();
  initial.resolve({ sidebarWidth: 320 }); await flush();
  assert.equal(harness.element('sidebar-resizer').getAttribute('aria-valuenow'), '400');
  harness.dom.window.close();
});

test('choosing a records folder schedules a move and waits for an explicit restart', async () => {
  let finishChoice;
  let current = { ...recordsLocation };
  const harness = await interfaceHarness({
    storage_location: () => current,
    choose_storage_folder: () => new Promise(resolve => { finishChoice = () => resolve('/Volumes/Records/Tugboat'); }),
    change_storage_location: ({ folder, locate }) => { assert.equal(locate, false); current = { ...current, pendingDirectory: folder }; return current; },
    cancel_storage_move: () => { current = { ...current, pendingDirectory: null }; return current; },
    restart_for_storage_change: () => null,
  });
  await harness.ready; harness.element('settings-navigation').click(); await flush();
  assert.equal(harness.element('records-folder').textContent, recordsLocation.directory);
  const choose = harness.element('change-records-folder');
  choose.click(); choose.click(); await flush();
  assert.equal(choose.disabled, true);
  assert.match(harness.element('workspace-status-text').textContent, /Choosing a records folder/);
  assert.equal(harness.element('records-folder-status').hidden, true);
  assert.equal(harness.calls.filter(call => call.command === 'choose_storage_folder').length, 1);
  finishChoice(); await flush();
  assert.equal(harness.element('records-folder').textContent, recordsLocation.directory);
  assert.equal(harness.element('workspace-status-bar').hidden, true);
  assert.equal(harness.element('records-folder-status').hidden, false);
  assert.match(harness.element('records-folder-status').textContent, /Restart Tugboat.*\/Volumes\/Records\/Tugboat/);
  assert.equal(harness.element('restart-for-records').hidden, false);
  assert.equal(harness.calls.filter(call => call.command === 'restart_for_storage_change').length, 0);
  harness.element('restart-for-records').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'restart_for_storage_change').length, 1);
  harness.element('cancel-records-move').click(); await flush();
  assert.equal(harness.element('restart-for-records').hidden, true);
  assert.equal(harness.element('cancel-records-move').hidden, true);
  assert.equal(harness.element('records-folder-status').textContent, '');
  assert.equal(harness.calls.filter(call => ['terminal_close', 'forget_project', 'remove_project'].includes(call.command)).length, 0);
  harness.dom.window.close();
});

test('a cancelled or rejected records folder preserves the current location and permits retry', async () => {
  let folder = null;
  const harness = await interfaceHarness({
    choose_storage_folder: () => folder,
    change_storage_location: () => { throw new Error('Choose an empty folder for Tugboat records.'); },
  });
  await harness.ready;
  const choose = harness.element('change-records-folder');
  choose.click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'change_storage_location').length, 0);
  folder = '/fixture/not-empty'; choose.click(); await flush();
  assert.match(harness.element('records-folder-status').textContent, /Choose an empty folder/);
  assert.equal(harness.element('records-folder').textContent, recordsLocation.directory);
  assert.equal(choose.disabled, false);
  assert.equal(harness.element('restart-for-records').hidden, true);
  harness.dom.window.close();
});

test('unavailable records keep location recovery enabled while ordinary settings stay disabled', async () => {
  const unavailable = { ...recordsLocation, directory: '/Volumes/Offline/Tugboat', available: false, message: 'The records folder is unavailable.' };
  const harness = await interfaceHarness({
    storage_location: () => unavailable,
    load_settings: () => { throw new Error('The records folder is unavailable.'); },
    choose_storage_folder: () => '/Volumes/Reconnected/Tugboat',
    change_storage_location: ({ folder, locate }) => { assert.equal(locate, true); return { ...unavailable, pendingDirectory: folder }; },
  });
  await harness.ready;
  assert.equal(harness.element('settings-fields').disabled, true);
  assert.equal(harness.element('agent-profile-fields').disabled, true);
  for (const id of ['codex-profile', 'claude-code-profile', 'save-agent-profile']) assert.equal(harness.element(id).matches(':disabled'), true);
  assert.match(harness.element('agent-profile-status').textContent, /records folder is unavailable/);
  const locate = harness.element('change-records-folder');
  assert.equal(locate.disabled, false);
  assert.equal(locate.textContent, 'Locate records folder…');
  assert.equal(harness.element('restart-for-records').hidden, false);
  locate.click(); await flush();
  assert.equal(locate.disabled, false);
  assert.equal(harness.element('restart-for-records').hidden, false);
  assert.equal(harness.calls.find(call => call.command === 'change_storage_location').args.folder, '/Volumes/Reconnected/Tugboat');
  assert.match(harness.element('records-folder-status').textContent, /\/Volumes\/Reconnected\/Tugboat/);
  harness.dom.window.close();
});

test('session refresh stays disabled while the selected project is being prepared', async () => {
  let finishPreparation;
  const harness = await interfaceHarness({ register_project: () => ({ ...project, stage: 'selected' }), prepare_project: () => new Promise(resolve => { finishPreparation = resolve; }) });
  await harness.ready;
  harness.element('choose-folder').click(); await flush();
  assert.equal(harness.element('refresh-sessions').disabled, true);
  finishPreparation({ project, plan, fingerprint: 'plan-a', message: null, candidates: [] }); await flush();
  assert.equal(harness.element('refresh-sessions').disabled, false);
  harness.dom.window.close();
});

test('opening a saved project shows recorded sessions while its plan is checked', async () => {
  const checking = deferred(), refreshing = deferred();
  const saved = sessionDashboard([{ id: 'saved-chat', title: 'Saved chat', updatedAt: 1000, available: false }], { warnings: ['Showing saved sessions while checking for updates.'] });
  const current = sessionDashboard([{ id: 'saved-chat', title: 'Current chat', updatedAt: 2000 }]);
  const harness = await interfaceHarness({
    recent_projects: () => [project],
    saved_project_dashboard: () => saved,
    preview_project: () => checking.promise,
    project_dashboard: () => refreshing.promise,
  });
  await harness.ready;
  harness.element('recent-project-list').querySelector('.recent-project').click(); await flush();
  assert.equal(harness.element('project-dashboard').hidden, false);
  assert.match(harness.element('session-table-body').textContent, /Saved chat/);
  assert.equal(harness.element('dashboard-session-status').hidden, true);
  assert.equal(harness.element('workspace-status-bar').hidden, false);
  assert.match(harness.element('session-table-body').textContent, /Session availability not verified/);
  assert.doesNotMatch(harness.element('session-table-body').textContent, /Session file unavailable/);
  assert.equal(harness.element('start-codex').disabled, true);
  assert.equal(harness.element('session-table-body').querySelector('.session-title').disabled, true);
  assert.ok(harness.element('session-table').querySelector('.is-checking-sessions .session-title:disabled'));
  assert.ok(harness.element('sidebar-session-list').querySelector('.is-checking-sessions .sidebar-session:disabled > .sidebar-session-title'));
  assert.equal(harness.calls.filter(call => call.command === 'preview_project').length, 1);
  assert.equal(harness.calls.filter(call => call.command === 'prepare_project').length, 0);
  assert.equal(harness.calls.filter(call => call.command === 'project_dashboard').length, 0);
  checking.resolve({ project, plan, fingerprint: 'verified-plan' }); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'project_dashboard').length, 1);
  assert.match(harness.element('session-table-body').textContent, /Saved chat/);
  assert.equal(harness.element('session-table').classList.contains('is-checking-sessions'), true);
  assert.equal(harness.element('sidebar-session-list').classList.contains('is-checking-sessions'), true);
  refreshing.resolve(current); await flush();
  assert.match(harness.element('session-table-body').textContent, /Current chat/);
  assert.equal(harness.element('session-table').classList.contains('is-checking-sessions'), false);
  assert.equal(harness.element('sidebar-session-list').classList.contains('is-checking-sessions'), false);
  assert.equal(harness.element('session-table-body').querySelector('.session-title').disabled, false);
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.find(call => call.command === 'start_agent_session').args.fingerprint, 'verified-plan');
  harness.dom.window.close();
});

test('reopening a project uses its in-memory dashboard and includes background updates', async () => {
  const second = { ...project, id: 'project-b', environmentId: 'b'.repeat(32), name: 'Second project' };
  const projects = [project, second];
  const harness = await interfaceHarness({
    recent_projects: () => projects,
    open_project: ({ identifier }) => projects.find(current => current.id === identifier),
    project_dashboard: ({ identifier }) => sessionDashboard([{ id: identifier + '-chat', title: 'Current ' + identifier, updatedAt: 1000 }], { sequence: 1 }),
  });
  await harness.ready;
  const open = id => harness.element('recent-project-list').querySelector('[data-project-id="' + id + '"]').click();
  open(project.id); await flush();
  assert.match(harness.element('session-table-body').textContent, /Current project-a/);
  harness.element('project-navigation').click(); await flush();
  open(second.id); await flush();
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, sessionDashboard([{ id: 'project-a-chat', title: 'Updated in background', updatedAt: 2000 }], { sequence: 4 })] });
  assert.match(harness.element('session-table-body').textContent, /Current project-b/);
  harness.element('project-navigation').click(); await flush();
  open(project.id); await flush();
  assert.match(harness.element('session-table-body').textContent, /Updated in background/);
  assert.equal(harness.calls.filter(call => call.command === 'saved_project_dashboard' && call.args.identifier === project.id).length, 1);
  assert.equal(harness.calls.filter(call => call.command === 'project_dashboard' && call.args.identifier === project.id).length, 1);
  assert.equal(harness.calls.filter(call => call.command === 'preview_project' && call.args.identifier === project.id).length, 2);
  harness.dom.window.close();
});

test('A to B to A keeps the dashboard sidebar and usage on the accepted recorded snapshot', async t => {
  const second = { ...project, id: 'project-b', environmentId: 'b'.repeat(32), name: 'Second project' };
  const projects = [project, second], oldRead = deferred();
  const snapshot = (identifier, title, sequence) => sessionDashboard([{ id: identifier + '-chat', title, updatedAt: sequence }], { sequence, availabilityVerified: true });
  const harness = await interfaceHarness({
    recent_projects: () => projects,
    open_project: ({ identifier }) => projects.find(current => current.id === identifier),
    project_dashboard: ({ identifier }) => snapshot(identifier, 'Current ' + identifier, 1),
    recorded_project_dashboard: ({ identifier }) => snapshot(identifier, identifier === project.id ? 'Recorded A update' : 'Current project-b', 4),
    conversation_usage: ({ identifier, sessionId }) => sessionDetails(sessionId, [], { warnings: ['Recorded for ' + identifier] }),
  });
  t.after(() => harness.dom.window.close());
  await harness.ready;
  const open = id => harness.element('recent-project-list').querySelector('[data-project-id="' + id + '"]').click();
  open(project.id); await flush();
  harness.handlers.project_dashboard = () => oldRead.promise;
  harness.element('refresh-sessions').click(); await flush();
  harness.element('project-navigation').click(); await flush();
  harness.handlers.project_dashboard = ({ identifier }) => snapshot(identifier, 'Current ' + identifier, 1);
  open(second.id); await flush();
  const focus = harness.element('project-settings-button'); focus.focus();
  await harness.listeners.get('records-changed')({ payload: ['usage'] }); await flush();
  assert.equal(harness.element('project-heading').textContent, second.name);
  assert.equal(harness.dom.window.document.activeElement, focus);
  assert.match(harness.element('session-table-body').textContent, /Current project-b/);
  assert.doesNotMatch(harness.element('sidebar-session-list').textContent, /Recorded A update/);
  harness.element('project-navigation').click(); await flush();
  open(project.id); await flush();
  oldRead.resolve(snapshot(project.id, 'Obsolete first A read', 99)); await flush();
  assert.equal(harness.element('session-table-body').querySelector('.session-title').textContent, 'Recorded A update');
  assert.match(harness.element('sidebar-session-list').textContent, /Recorded A update/);
  assert.doesNotMatch(harness.element('sidebar-session-list').textContent, /Obsolete|Current project-b/);
  harness.element('usage-for-project-a-chat').click(); await flush();
  assert.equal(harness.element('session-usage-title').textContent, 'Recorded A update');
  assert.match(harness.element('session-usage-status').textContent, /Recorded for project-a/);
});

test('project navigation uses loaded records until a record change is reported', async () => {
  const harness = await interfaceHarness({ recent_projects: () => [project] });
  await harness.ready;
  const initial = harness.calls.filter(call => call.command === 'recent_projects').length;
  harness.element('recent-project-list').querySelector('button').click(); await flush();
  harness.element('project-navigation').click(); await flush();
  harness.element('recent-project-list').querySelector('button').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'recent_projects').length, initial);
  harness.dom.window.close();
});

test('terminal state for another project updates in memory before reopening it', async () => {
  const second = { ...project, id: 'project-b', name: 'Second project', path: '/fixture/b' };
  const projects = [project, second];
  const harness = await interfaceHarness({ recent_projects: () => projects, open_project: ({ identifier }) => projects.find(item => item.id === identifier) });
  await harness.ready;
  const open = id => harness.element('recent-project-list').querySelector('[data-project-id="' + id + '"]').click();
  open(project.id); await flush();
  harness.element('project-navigation').click(); await flush(); open(second.id); await flush();
  harness.setTerminal({ identifier: 87, projectId: project.id, purpose: 'local', running: false, exitCode: 0, conversationId: null, busy: false });
  await harness.listeners.get('terminal-sessions-changed')({ payload: project.id }); await flush();
  const reads = () => harness.calls.filter(call => call.command === 'terminal_sessions' && call.args.projectId === project.id).length;
  const before = reads();
  harness.element('project-navigation').click(); await flush(); open(project.id); await flush();
  assert.equal(reads(), before);
  assert.match(harness.element('sidebar-terminal-list').textContent, /Local terminal.*exit 0/);
  harness.dom.window.close();
});

test('late plan checks cannot replace a newer project selection', async () => {
  const firstCheck = deferred();
  const second = { ...project, id: 'project-b', name: 'Second project', path: '/fixture/second' };
  const projects = [project, second];
  let firstAttempts = 0;
  const harness = await interfaceHarness({
    recent_projects: () => projects,
    open_project: ({ identifier }) => projects.find(current => current.id === identifier),
    saved_project_dashboard: ({ identifier }) => sessionDashboard([{ id: 'saved-' + identifier, title: 'Saved ' + identifier, updatedAt: 1000, available: false }]),
    preview_project: ({ identifier }) => {
      const current = projects.find(item => item.id === identifier);
      if (identifier === project.id && firstAttempts++ === 0) return firstCheck.promise;
      return { project: current, plan, fingerprint: identifier + '-current' };
    },
    project_dashboard: ({ identifier }) => sessionDashboard([{ id: 'current-' + identifier, title: 'Current ' + identifier, updatedAt: 2000 }]),
  });
  await harness.ready;
  const open = id => harness.element('recent-project-list').querySelector('[data-project-id="' + id + '"]').click();
  open(project.id); await flush();
  assert.equal(harness.element('project-navigation').disabled, false);
  harness.element('project-navigation').click(); await flush();
  open(second.id); await flush();
  assert.equal(harness.element('project-heading').textContent, second.name);
  harness.element('project-navigation').click(); await flush();
  open(project.id); await flush();
  firstCheck.resolve({ project, plan, fingerprint: 'stale-plan' }); await flush();
  assert.equal(harness.element('project-heading').textContent, project.name);
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').at(-1).args.fingerprint, 'project-a-current');
  harness.dom.window.close();
});

test('a delayed saved dashboard cannot replace a newer live update', async () => {
  const saved = deferred(), checking = deferred();
  const harness = await interfaceHarness({
    saved_project_dashboard: () => saved.promise,
    preview_project: () => checking.promise,
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, { ...sessionDashboard([{ id: 'live', title: 'Live update', updatedAt: 2000 }]), sequence: 1 }] });
  saved.resolve(sessionDashboard([{ id: 'old', title: 'Old saved data', updatedAt: 1000, available: false }])); await flush();
  assert.match(harness.element('session-table-body').textContent, /Live update/);
  assert.doesNotMatch(harness.element('session-table-body').textContent, /Old saved data/);
  checking.resolve({ project, plan, fingerprint: 'plan-a' }); await flush();
  harness.dom.window.close();
});

test('retrying a failed background plan refreshes saved session availability', async () => {
  let attempts = 0;
  const saved = sessionDashboard([{ id: 'chat', title: 'Saved chat', updatedAt: 1000, available: false }], { warnings: ['Showing saved sessions while checking for updates.'] });
  const current = sessionDashboard([{ id: 'chat', title: 'Current chat', updatedAt: 2000 }]);
  const harness = await interfaceHarness({
    saved_project_dashboard: () => saved,
    preview_project: () => {
      if (++attempts === 1) throw new Error('Plan check failed.');
      return { project, plan, fingerprint: 'recovered-plan' };
    },
    project_dashboard: () => attempts === 1 ? saved : current,
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.match(harness.element('project-plan-status').textContent, /Plan check failed/);
  assert.equal(harness.element('start-codex').disabled, true);
  assert.match(harness.element('session-table-body').textContent, /Saved chat/);
  assert.equal(harness.element('session-table').classList.contains('is-checking-sessions'), false);
  assert.equal(harness.element('sidebar-session-list').classList.contains('is-checking-sessions'), false);
  harness.element('retry-project-plan').click(); await flush();
  assert.match(harness.element('session-table-body').textContent, /Current chat/);
  assert.equal(harness.element('session-table-body').querySelector('.session-title').disabled, false);
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.find(call => call.command === 'start_agent_session').args.fingerprint, 'recovered-plan');
  harness.dom.window.close();
});

test('accepting a changed policy refreshes saved session availability', async () => {
  let accepted = false;
  const saved = sessionDashboard([{ id: 'chat', title: 'Saved chat', updatedAt: 1000, available: false }], { warnings: ['Showing saved sessions while checking for updates.'] });
  const current = sessionDashboard([{ id: 'chat', title: 'Current chat', updatedAt: 2000 }]);
  const harness = await interfaceHarness({
    saved_project_dashboard: () => saved,
    preview_project: () => {
      if (!accepted) throw new Error('Policy changed.');
      return { project, plan, fingerprint: 'accepted-plan' };
    },
    project_dashboard: () => accepted ? current : saved,
    read_project_policy: () => ({ document: {}, fingerprint: 'new-policy', editable: true, attached: true, allowedHosts: [] }),
    change_project_policy: () => { accepted = true; return project; },
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.match(harness.element('session-table-body').textContent, /Saved chat/);
  harness.element('project-settings-button').click(); await flush();
  harness.element('read-project-policy').click(); await flush();
  harness.element('accept-project-policy').click(); await flush();
  assert.match(harness.element('session-table-body').textContent, /Current chat/);
  assert.equal(harness.element('session-table-body').querySelector('.session-title').disabled, false);
  harness.dom.window.close();
});

test('a plan for another project cannot enable a saved project', async () => {
  const other = { ...project, id: 'project-b', name: 'Other project' };
  const harness = await interfaceHarness({ preview_project: () => ({ project: other, plan, fingerprint: 'wrong-plan' }) });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.match(harness.element('project-plan-status').textContent, /belongs to another environment/);
  assert.equal(harness.element('start-codex').disabled, true);
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 0);
  harness.dom.window.close();
});

test('preparation for another project cannot enable a new project', async () => {
  const other = { ...project, id: 'project-b', name: 'Other project' };
  const harness = await interfaceHarness({
    register_project: () => ({ ...project, stage: 'selected' }),
    prepare_project: () => ({ project: other, plan, fingerprint: 'wrong-plan', message: null, candidates: [] }),
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.match(harness.element('project-plan-status').textContent, /belongs to another environment/);
  assert.equal(harness.element('start-codex').disabled, true);
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 0);
  harness.dom.window.close();
});

test('a saved project with an incomplete policy receipt resumes preparation', async () => {
  const incomplete = { ...project, stage: 'ready', policy: { ...project.policy, fingerprint: null } };
  const harness = await interfaceHarness({
    recent_projects: () => [incomplete],
    open_project: () => incomplete,
    prepare_project: () => ({ project, plan, fingerprint: 'recovered-plan', message: null, candidates: [] }),
  });
  await harness.ready; harness.element('recent-project-list').querySelector('.recent-project').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'prepare_project').length, 1);
  assert.equal(harness.calls.filter(call => call.command === 'preview_project').length, 0);
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.find(call => call.command === 'start_agent_session').args.fingerprint, 'recovered-plan');
  harness.dom.window.close();
});

test('a plan result does not start an environment while an agent switch is pending', async () => {
  const checking = deferred(), switching = deferred();
  const stopped = { ...project, stage: 'ready' };
  const claude = { ...project, agentKind: 'claude-code', environmentId: 'b'.repeat(32), profile: 'tugboat-claude' };
  const harness = await interfaceHarness({
    register_project: () => stopped,
    preview_project: () => checking.promise,
    ensure_project_environment: () => switching.promise,
    prepare_project: () => ({ project: claude, plan, fingerprint: 'claude-plan', message: null, candidates: [] }),
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('agent-choice').value = 'claude-code';
  harness.element('agent-choice').dispatchEvent(new harness.dom.window.Event('change')); await flush();
  checking.resolve({ project: stopped, plan, fingerprint: 'codex-plan' }); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 0);
  switching.resolve(claude); await flush();
  assert.equal(harness.element('agent-choice').value, 'claude-code');
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 0);
  harness.dom.window.close();
});

test('a retried plan does not start an environment while an agent switch is pending', async () => {
  const checking = deferred(), switching = deferred();
  const stopped = { ...project, stage: 'ready' };
  const claude = { ...project, agentKind: 'claude-code', environmentId: 'b'.repeat(32), profile: 'tugboat-claude' };
  let attempts = 0;
  const harness = await interfaceHarness({
    register_project: () => stopped,
    preview_project: () => {
      if (++attempts === 1) throw new Error('Plan check failed.');
      return checking.promise;
    },
    ensure_project_environment: () => switching.promise,
    prepare_project: () => ({ project: claude, plan, fingerprint: 'claude-plan', message: null, candidates: [] }),
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('retry-project-plan').click(); await flush();
  harness.element('agent-choice').value = 'claude-code';
  harness.element('agent-choice').dispatchEvent(new harness.dom.window.Event('change')); await flush();
  checking.resolve({ project: stopped, plan, fingerprint: 'codex-plan' }); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 0);
  switching.resolve(claude); await flush();
  assert.equal(harness.element('agent-choice').value, 'claude-code');
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 0);
  harness.dom.window.close();
});

test('a failed agent switch resumes automatic startup for the selected project', async () => {
  const checking = deferred(), switching = deferred();
  const stopped = { ...project, stage: 'ready' };
  let started = false;
  const harness = await interfaceHarness({
    register_project: () => stopped,
    preview_project: () => started ? { project: { ...stopped, stage: 'environment-starting' }, plan, fingerprint: 'codex-plan' } : checking.promise,
    ensure_project_environment: () => switching.promise,
    prepare_environment: () => {
      started = true;
      harness.setTerminal({ identifier: 70, projectId: project.id, environmentId: project.environmentId, purpose: 'environment', running: true, exitCode: null, conversationId: null, busy: false });
      return 70;
    },
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('agent-choice').value = 'claude-code';
  harness.element('agent-choice').dispatchEvent(new harness.dom.window.Event('change')); await flush();
  checking.resolve({ project: stopped, plan, fingerprint: 'codex-plan' }); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 0);
  switching.reject(new Error('Agent switch failed.')); await flush();
  assert.equal(harness.element('agent-choice').value, 'codex');
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 1);
  assert.equal(harness.element('folder-error').hidden, false);
  assert.match(harness.element('folder-error').textContent, /Agent switch failed/);
  harness.dom.window.close();
});

test('opening a stopped project starts its saved environment in the background and reuses it on reopening', async () => {
  const preferences = { cpus: 2, memoryMb: 2048, verification: 'require', network: 'isolated' };
  let current = { ...project, stage: 'ready', options: preferences };
  const prepared = () => ({ project: current, plan, fingerprint: 'saved-plan', message: null, candidates: [] });
  const harness = await interfaceHarness({
    recent_projects: () => [current], open_project: () => current,
    prepare_project: prepared, preview_project: prepared,
    prepare_environment: () => {
      current = { ...current, stage: 'environment-starting' };
      harness.setTerminal({ identifier: 70, projectId: project.id, purpose: 'environment', running: true, exitCode: null, conversationId: null, busy: false });
      return 70;
    },
  });
  await harness.ready;
  const open = () => harness.element('recent-project-list').querySelector('.recent-project').click();
  open(); await flush();
  const start = harness.calls.find(call => call.command === 'prepare_environment');
  assert.equal(start.args.environmentId, project.environmentId);
  assert.equal(start.args.identifier, project.id);
  assert.equal(start.args.fingerprint, 'saved-plan');
  assert.equal(start.args.layout.visible, false);
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.element('project-settings-dialog').open, false);
  assert.equal(harness.element('project-setup').hidden, true);
  assert.equal(harness.element('start-codex').disabled, true);
  assert.equal(harness.element('project-overview').hidden, false);
  assert.match(harness.element('project-plan-status').textContent, /Starting.*saved settings/);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').length, 0);
  current = { ...current, stage: 'environment-ready' };
  harness.setTerminal({ ...harness.terminals.get(70), running: false, exitCode: 0 });
  await harness.listeners.get('environment-preparation-finished')({ payload: { project: current, terminalIdentifier: 70, message: null } }); await flush();
  assert.equal(harness.element('start-codex').disabled, false);
  assert.equal(harness.element('project-overview').hidden, false);
  assert.equal(harness.element('project-plan-feedback').hidden, true);
  assert.deepEqual(current.options, preferences);
  harness.element('project-navigation').click(); await flush(); open(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 1);
  assert.equal(harness.calls.filter(call => ['save_project_preferences', 'save_settings', 'remove_project', 'terminal_close'].includes(call.command)).length, 0);
  harness.dom.window.close();
});

test('an existing environment start is reused without a second process or terminal focus', async () => {
  let current = { ...project, stage: 'environment-starting' };
  const harness = await interfaceHarness({
    recent_projects: () => [current], open_project: () => current,
    preview_project: () => ({ project: current, plan, fingerprint: 'saved-plan' }),
  });
  await harness.ready;
  harness.setTerminal({ identifier: 70, projectId: project.id, purpose: 'environment', running: true, exitCode: null, conversationId: null, busy: false });
  harness.element('recent-project-list').querySelector('.recent-project').click(); await flush();
  assert.equal(harness.calls.filter(call => ['prepare_project', 'prepare_environment', 'terminal_focus'].includes(call.command)).length, 0);
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.element('project-setup').hidden, true);
  assert.match(harness.element('project-plan-status').textContent, /Starting/);
  current = { ...current, stage: 'environment-ready' };
  harness.setTerminal({ ...harness.terminals.get(70), running: false, exitCode: 0 });
  await harness.listeners.get('environment-preparation-finished')({ payload: { project: current, terminalIdentifier: 70, message: null } }); await flush();
  assert.equal(harness.element('start-codex').disabled, false);
  harness.dom.window.close();
});

test('an environment that exits before its start response is verified without waiting for another event', async () => {
  let current = { ...project, stage: 'ready' };
  const prepared = () => ({ project: current, plan, fingerprint: 'saved-plan', message: null, candidates: [] });
  const harness = await interfaceHarness({
    register_project: () => current, prepare_project: prepared, preview_project: prepared,
    prepare_environment: async () => {
      current = { ...current, stage: 'environment-ready' };
      harness.setTerminal({ identifier: 70, projectId: project.id, purpose: 'environment', running: false, exitCode: 0, conversationId: null, busy: false });
      await harness.listeners.get('environment-preparation-finished')({ payload: { project: current, terminalIdentifier: 70, message: null } });
      return 70;
    },
    finish_environment: ({ identifier, environmentId, terminalIdentifier }) => { assert.equal(identifier, project.id); assert.equal(environmentId, project.environmentId); assert.equal(terminalIdentifier, 70); return current; },
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'finish_environment').length, 1);
  assert.equal(harness.element('start-codex').disabled, false);
  assert.equal(harness.element('project-setup').hidden, true);
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.element('project-plan-feedback').hidden, true);
  harness.dom.window.close();
});

test('a failed automatic environment start offers a retry without repeating project configuration', async () => {
  let current = { ...project, stage: 'ready' }, attempts = 0;
  const prepared = () => ({ project: current, plan, fingerprint: 'saved-plan', message: null, candidates: [] });
  const harness = await interfaceHarness({
    register_project: () => current, prepare_project: prepared, preview_project: prepared,
    prepare_environment: () => {
      if (++attempts === 1) throw new Error('The runner is temporarily unavailable.');
      current = { ...current, stage: 'environment-starting' };
      harness.setTerminal({ identifier: 70, projectId: project.id, purpose: 'environment', running: true, exitCode: null, conversationId: null, busy: false });
      return 70;
    },
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.equal(harness.element('retry-project-plan').hidden, false);
  assert.equal(harness.element('retry-project-plan').textContent, 'Retry environment');
  assert.match(harness.element('project-plan-status').textContent, /temporarily unavailable/);
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.element('start-codex').disabled, true);
  harness.element('retry-project-plan').click(); harness.element('retry-project-plan').click(); await flush();
  assert.equal(attempts, 2);
  assert.equal(harness.calls.filter(call => call.command === 'prepare_project').length, 0);
  assert.equal(harness.calls.filter(call => ['save_project_preferences', 'save_settings', 'terminal_close'].includes(call.command)).length, 0);
  assert.match(harness.element('project-plan-status').textContent, /Starting/);
  assert.equal(harness.element('retry-project-plan').hidden, true);
  harness.dom.window.close();
});

test('a failed background environment check reports the failure and leaves New Chat disabled', async () => {
  let current = { ...project, stage: 'ready' };
  const prepared = () => ({ project: current, plan, fingerprint: 'saved-plan', message: null, candidates: [] });
  const harness = await interfaceHarness({
    register_project: () => current, prepare_project: prepared, preview_project: prepared,
    prepare_environment: () => {
      current = { ...current, stage: 'environment-starting' };
      harness.setTerminal({ identifier: 70, projectId: project.id, purpose: 'environment', running: true, exitCode: null, conversationId: null, busy: false });
      return 70;
    },
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  current = { ...current, stage: 'ready', environmentAttempt: { completed: true, error: 'Guest version check failed.', terminalIdentifier: 70 } };
  harness.setTerminal({ ...harness.terminals.get(70), running: false, exitCode: 2 });
  await harness.listeners.get('environment-preparation-finished')({ payload: { project: current, terminalIdentifier: 70, message: 'Guest version check failed.' } }); await flush();
  assert.equal(harness.element('retry-project-plan').textContent, 'Retry environment');
  assert.equal(harness.element('retry-project-plan').hidden, false);
  assert.match(harness.element('project-plan-status').textContent, /Guest version check failed/);
  assert.equal(harness.element('start-codex').disabled, true);
  assert.equal(harness.element('project-settings-dialog').open, false);
  assert.equal(harness.element('project-setup').hidden, true);
  harness.dom.window.close();
});

test('automatic environment startup preserves a project settings dialog opened during launch', async () => {
  let finishStart, current = { ...project, stage: 'ready' };
  const prepared = () => ({ project: current, plan, fingerprint: 'saved-plan', message: null, candidates: [] });
  const harness = await interfaceHarness({
    register_project: () => current, prepare_project: prepared, preview_project: prepared,
    read_project_preferences: () => ({ project: current, options: { cpus: 2, memoryMb: 2048, verification: 'require', network: 'isolated' } }),
    prepare_environment: () => new Promise(resolve => { finishStart = () => {
      current = { ...current, stage: 'environment-starting' };
      harness.setTerminal({ identifier: 70, projectId: project.id, purpose: 'environment', running: true, exitCode: null, conversationId: null, busy: false });
      resolve(70);
    }; }),
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  harness.element('project-name').focus();
  finishStart(); await flush();
  assert.equal(harness.element('project-settings-dialog').open, true);
  assert.equal(harness.dom.window.document.activeElement, harness.element('project-name'));
  assert.equal(harness.element('project-cpus').value, '2');
  assert.equal(harness.element('terminal-panel').hidden, true);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').length, 0);
  harness.dom.window.close();
});

test('automatic environment startup keeps a running local terminal and returns to it on request', async () => {
  let current = { ...project, stage: 'ready' };
  const prepared = () => ({ project: current, plan, fingerprint: 'saved-plan', message: null, candidates: [] });
  const harness = await interfaceHarness({
    register_project: () => current, prepare_project: prepared, preview_project: prepared,
    prepare_environment: () => {
      current = { ...current, stage: 'environment-starting' };
      harness.setTerminal({ identifier: 71, projectId: project.id, purpose: 'environment', running: true, exitCode: null, conversationId: null, busy: false });
      return 71;
    },
  });
  await harness.ready;
  harness.setTerminal({ identifier: 70, projectId: project.id, purpose: 'local', running: true, exitCode: null, conversationId: null, busy: false });
  harness.element('choose-folder').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 1);
  assert.equal(harness.calls.filter(call => ['terminal_close', 'terminal_focus'].includes(call.command)).length, 0);
  current = { ...current, stage: 'environment-ready' };
  harness.setTerminal({ ...harness.terminals.get(71), running: false, exitCode: 0 });
  await harness.listeners.get('environment-preparation-finished')({ payload: { project: current, terminalIdentifier: 71, message: null } }); await flush();
  assert.equal(harness.terminals.get(70).running, true);
  harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="70"]').click(); await flush();
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, 70);
  harness.dom.window.close();
});

test('a stopped environment after completion never enables New Chat from a stale success event', async () => {
  let current = { ...project, stage: 'ready' };
  const prepared = () => ({ project: current, plan, fingerprint: 'saved-plan', message: null, candidates: [] });
  const harness = await interfaceHarness({
    register_project: () => current, prepare_project: prepared, preview_project: prepared,
    prepare_environment: () => {
      current = { ...current, stage: 'environment-starting' };
      harness.setTerminal({ identifier: 70, projectId: project.id, purpose: 'environment', running: true, exitCode: null, conversationId: null, busy: false });
      return 70;
    },
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  current = { ...current, stage: 'ready' };
  harness.setTerminal({ ...harness.terminals.get(70), running: false, exitCode: 0 });
  await harness.listeners.get('environment-preparation-finished')({ payload: { project: { ...current, stage: 'environment-ready' }, terminalIdentifier: 70, message: null } }); await flush();
  assert.equal(harness.element('start-codex').disabled, true);
  assert.match(harness.element('project-plan-status').textContent, /environment is not running/);
  assert.equal(harness.element('retry-project-plan').textContent, 'Retry environment');
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 1);
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
  assert.equal(harness.calls.find(call => call.command === 'start_agent_session').args.sessionId, conversation.id);
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
  assert.equal(harness.element('project-overview').hidden, false);
  assert.equal(harness.element('project-setup').hidden, true);
  assert.match(harness.element('project-plan-status').textContent, /End running sessions/);
  assert.equal(harness.calls.filter(call => call.command === 'prepare_environment').length, 0);
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
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, sessionDashboard([{ ...conversation, usageRevision: 'cleared' }])] });
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
  harness.element('sidebar-terminal-list').querySelector('[data-terminal-id="80"]').click(); await flush();
  const caption = harness.element('terminal-status').textContent;
  await harness.listeners.get('environment-preparation-finished')({ payload: { project: environmentProject, terminalIdentifier: 70, message: null } });
  await flush();
  assert.equal(harness.element('project-overview').hidden, false);
  assert.equal(harness.element('terminal-status').textContent, caption);
  assert.equal(harness.calls.filter(call => call.command === 'finish_environment').length, 0);
  environmentProject = { ...project, stage: 'ready' };
  await harness.listeners.get('environment-preparation-finished')({ payload: { project: environmentProject, terminalIdentifier: 90, message: 'Codex version could not be verified. Try again.' } });
  await flush();
  assert.equal(harness.element('project-overview').hidden, false);
  assert.equal(harness.element('prepare-environment').hidden, false);
  assert.match(harness.element('preparation-status').textContent, /Try again/);
  await harness.listeners.get('environment-preparation-finished')({ payload: { project, terminalIdentifier: 70, message: null } });
  assert.match(harness.element('preparation-status').textContent, /Try again/);
  harness.dom.window.close();
});

test('a failed initial plan disables New Chat and offers a visible retry using the new fingerprint', async () => {
  let attempts = 0;
  const harness = await interfaceHarness({ register_project: () => ({ ...project, stage: 'selected' }), prepare_project: () => {
    if (++attempts === 1) throw new Error('The project plan could not be read.');
    return { project, plan, fingerprint: 'recovered-plan', message: null, candidates: [] };
  } });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.equal(harness.element('project-plan-feedback').hidden, false);
  assert.match(harness.element('project-plan-status').textContent, /could not be read/);
  assert.equal(harness.element('start-codex').disabled, true);
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 0);
  harness.element('retry-project-plan').click(); await flush();
  assert.equal(harness.element('project-plan-feedback').hidden, true);
  assert.equal(harness.element('start-codex').disabled, false);
  harness.element('start-codex').click(); harness.element('start-codex').click(); await flush();
  const starts = harness.calls.filter(call => call.command === 'start_agent_session');
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
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 0);
  harness.handlers.preview_project = () => ({ project, plan, fingerprint: 'new-plan' });
  harness.element('retry-project-plan').click(); await flush();
  assert.equal(harness.element('start-codex').disabled, false);
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.find(call => call.command === 'start_agent_session').args.fingerprint, 'new-plan');
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
    read_project_policy: () => ({ document: { project: 'A' }, fingerprint: 'policy-a', editable: true, attached: true, allowedHosts: ['a.example.com'] }),
    run_boundary_tests: () => ({ projectId: project.id, environmentId: project.environmentId, planFingerprint: 'plan-a', checkedAt: Math.floor(Date.now() / 1000), passed: true, cleanup: true, results: [{ name: 'A network', target: 'a.example.com', expected: 'Allowed', observed: 'Allowed', outcome: 'pass' }] }),
    preview_removal: () => { throw new Error('Project A removal preview failed'); },
  });
  await harness.ready;
  harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  for (const id of ['read-project-policy', 'run-boundary-tests', 'preview-removal']) harness.element(id).click();
  await flush();
  assert.match(harness.element('project-policy-document').textContent, /"A"/);
  assert.equal(harness.element('use-brig-boundary').disabled, false);
  harness.element('use-brig-boundary').click();
  assert.match(harness.element('operation-status').textContent, /Project A/);
  harness.element('close-project-settings').click(); harness.element('project-navigation').click(); await flush();
  harness.element('recent-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  assert.equal(harness.element('project-preference-fields').disabled, true);
  assert.equal(harness.element('project-name').value, '');
  assert.equal(harness.element('guest-terminal-status').textContent, '');
  assert.equal(harness.element('project-policy-document').textContent, '');
  assert.equal(harness.element('project-policy-hosts').value, '');
  assert.equal(harness.element('project-policy-fields').disabled, true);
  assert.equal(harness.element('boundary-results').children.length, 0);
  assert.equal(harness.element('boundary-status').textContent, '');
  assert.equal(harness.element('use-brig-boundary').disabled, false);
  assert.equal(harness.element('use-brig-boundary').checked, true);
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
    const boundary = label => ({ projectId: project.id, environmentId: project.environmentId, planFingerprint: 'plan-a', checkedAt: Math.floor(Date.now() / 1000), passed: true, cleanup: true, results: [{ name: label, target: label, expected: 'Allowed', observed: 'Allowed', outcome: 'pass' }] });
    const harness = await interfaceHarness({
      recent_projects: () => projects,
      open_project: ({ identifier }) => projects.find(current => current.id === identifier),
      prepare_project: ({ identifier }) => ({ project: projects.find(current => current.id === identifier), plan, fingerprint: 'plan-a', message: null, candidates: [] }),
      read_project_preferences: () => defer('preferences'),
      read_project_policy: () => defer('policy'), run_boundary_tests: () => defer('boundary'),
      preview_removal: () => defer('preview'), stop_project: () => defer('stop'),
    });
    await harness.ready;
    harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').click(); await flush();
    harness.element('project-settings-button').click(); await flush();
    for (const id of ['read-project-policy', 'run-boundary-tests', 'stop-project', 'preview-removal']) harness.element(id).click();
    await flush();
    assert.equal(pending.size, 5);
    harness.element('close-project-settings').click();
    harness.handlers.read_project_preferences = ({ identifier }) => preferences(projects.find(current => current.id === identifier));
    harness.handlers.read_project_policy = () => policy('Fresh');
    harness.handlers.run_boundary_tests = () => boundary('Fresh');
    if (transition === 'project-switch') {
      harness.element('project-navigation').click(); await flush();
      harness.element('recent-project-list').querySelector('[data-project-id="project-b"]').click(); await flush();
      harness.element('project-navigation').click(); await flush();
      harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').click(); await flush();
    }
    harness.element('project-settings-button').click(); await flush();
    for (const id of ['read-project-policy', 'run-boundary-tests']) harness.element(id).click();
    await flush();
    const oldResults = {
      preferences: preferences({ ...project, name: 'OLD project' }),
      policy: policy('OLD'), boundary: boundary('OLD'),
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
    assert.equal(harness.element('guest-terminal-status').textContent, '');
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
  assert.match(harness.element('network-hint').textContent, /online Codex and Claude Code sessions are unavailable/);
  assert.match(harness.element('network-hint').textContent, /separate scope/);
  harness.element('choose-folder').click(); await flush();
  assert.match(harness.element('sharing-plan-values').textContent, /Allowed guest destinations/);
  assert.match(harness.element('sharing-plan-values').textContent, /Mode not read or managed by Tugboat/);
  harness.dom.window.close();
});

test('inconclusive or incomplete boundary diagnostics preserve the selected Codex sandbox mode', async () => {
  const report = { projectId: project.id, environmentId: project.environmentId, planFingerprint: 'plan-a', checkedAt: Math.floor(Date.now() / 1000), passed: true, cleanup: true, results: [{ name: 'Network', target: 'example.com', expected: 'Denied', observed: 'Unknown', outcome: 'inconclusive' }] };
  const harness = await interfaceHarness({ run_boundary_tests: () => report });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('use-brig-boundary').click();
  for (const [results, cleanup] of [[report.results, true], [[], true], [[{ ...report.results[0], outcome: 'pass' }], false]]) {
    report.results = results; report.cleanup = cleanup;
    harness.element('run-boundary-tests').click(); await flush();
    assert.equal(harness.element('use-brig-boundary').disabled, false);
    assert.equal(harness.element('use-brig-boundary').checked, false);
    assert.match(harness.element('boundary-status').textContent, /could not be verified/);
  }
  harness.dom.window.close();
});

test('a new project connects the shared account without another login', async () => {
  const harness = await interfaceHarness(); await harness.ready;
  harness.element('choose-folder').click(); await flush();
  assert.equal(harness.element('start-codex').disabled, false);
  assert.equal(harness.calls.filter(call => call.command === 'sync_project_chatgpt').length, 1);
  assert.equal(harness.element('project-account-feedback').hidden, true);
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 0);
  harness.element('settings-navigation').click(); await flush();
  assert.equal(harness.element('project-settings-dialog').open, false);
  assert.equal(harness.element('settings-view').hidden, false);
  assert.equal(harness.element('shared-chatgpt-email').textContent, sharedAccount.email);
  assert.equal(harness.element('shared-chatgpt-heading').textContent, 'ChatGPT');
  harness.dom.window.close();
});

test('provider details and account controls belong only to application settings', async () => {
  let account = sharedAccount;
  const harness = await interfaceHarness({ shared_chatgpt_status: () => account, sync_project_chatgpt: () => account }); await harness.ready;
  harness.element('choose-folder').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  const projectSettings = harness.element('project-settings-dialog');
  for (const id of ['codex-login', 'check-login', 'login-status', 'login-hint', 'logout-on-removal']) assert.equal(harness.element(id), null);
  assert.doesNotMatch(projectSettings.textContent, /ChatGPT|provider|sign[- ]in|sign[- ]out/i);
  assert.equal(projectSettings.contains(harness.element('shared-chatgpt-section')), false);
  account = { ...sharedAccount, generation: 2, state: 'needs-sign-in', message: 'ChatGPT sign-in required.' };
  await harness.listeners.get('shared-chatgpt-changed')({ payload: account }); await flush();
  assert.doesNotMatch(projectSettings.textContent, /ChatGPT|provider|sign[- ]in|sign[- ]out/i);
  assert.equal(harness.element('guest-terminal-status').textContent, '');
  harness.element('settings-navigation').click(); await flush();
  assert.equal(harness.element('project-settings-dialog').open, false);
  assert.equal(harness.element('settings-view').contains(harness.element('shared-chatgpt-section')), true);
  assert.equal(harness.element('shared-chatgpt-heading').textContent, 'ChatGPT');
  assert.equal(harness.element('shared-chatgpt-sign-in').hidden, false);
  assert.equal(harness.calls.filter(call => call.command === 'login_status').length, 0);
  harness.dom.window.close();
});

test('the project account warning opens the provider section in application settings', async () => {
  const account = { ...sharedAccount, state: 'signed-out', email: null };
  const harness = await interfaceHarness({ shared_chatgpt_status: () => account, sync_project_chatgpt: () => account });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.equal(harness.element('project-account-feedback').hidden, false);
  harness.element('project-provider-settings').click(); await flush();
  assert.equal(harness.element('settings-view').hidden, false);
  assert.equal(harness.dom.window.document.activeElement, harness.element('shared-chatgpt-heading'));
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 0);
  harness.dom.window.close();
});

test('removal offers no account action and keeps a previous removal choice unchanged', async () => {
  for (const logout of [undefined, false, true]) {
    const harness = await interfaceHarness({
      preview_removal: () => ({ reference: 'codex@sample-a', workspace: '/fixture/guest-home', description: 'Remove this environment.', fingerprint: 'remove-a', external: false, logout }),
      remove_project: () => 'Project removed.',
    });
    await harness.ready; harness.element('choose-folder').click(); await flush();
    harness.element('project-settings-button').click(); await flush();
    harness.element('preview-removal').click(); await flush();
    assert.equal(harness.element('logout-on-removal'), null);
    assert.doesNotMatch(harness.element('removal-dialog').textContent, /ChatGPT|sign[- ]in|sign[- ]out/i);
    const confirmation = harness.element('removal-confirmation');
    confirmation.value = 'codex@sample-a'; confirmation.dispatchEvent(new harness.dom.window.Event('input'));
    harness.element('confirm-removal').click(); await flush();
    assert.equal(harness.calls.find(call => call.command === 'remove_project').args.logout, logout ?? false);
    assert.equal(harness.calls.filter(call => call.command === 'disconnect_shared_chatgpt').length, 0);
    assert.equal(harness.element('shared-chatgpt-email').textContent, sharedAccount.email);
    harness.dom.window.close();
  }
});

test('checking a connected account renews once and preserves its running chat', async () => {
  let account = sharedAccount, completeRenewal;
  const harness = await interfaceHarness({ shared_chatgpt_status: () => account, sync_project_chatgpt: () => account,
    retry_shared_chatgpt: () => new Promise(resolve => { completeRenewal = () => { account = { ...sharedAccount, generation: 2 }; resolve(account); }; }) });
  await harness.ready; harness.element('choose-folder').click(); await flush(); harness.element('start-codex').click(); await flush();
  harness.element('settings-navigation').click(); await flush();
  const check = harness.element('shared-chatgpt-retry'); assert.equal(check.hidden, false); assert.equal(check.textContent, 'Check connection');
  check.click(); check.click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'retry_shared_chatgpt').length, 1);
  completeRenewal(); await flush();
  assert.equal(harness.terminals.size, 1); assert.equal([...harness.terminals.values()][0].running, true);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 0);
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 1); harness.dom.window.close();
});

test('one provider sign-in starts once and returns to the browser without starting a project terminal', async () => {
  const start = deferred();
  const harness = await interfaceHarness({ start_provider_login: ({ provider }) => { assert.equal(provider, 'chatgpt'); return start.promise; } });
  await harness.ready; harness.element('settings-navigation').click(); harness.element('settings-tab-providers').click(); await flush();
  harness.element('shared-chatgpt-sign-in').click(); harness.element('shared-chatgpt-sign-in').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'start_provider_login').length, 1);
  assert.match(harness.element('workspace-status-text').textContent, /Connecting ChatGPT/);
  assert.equal(harness.element('shared-chatgpt-hint').hidden, true);
  start.resolve(providerLogin('chatgpt', 'waiting', 1)); await flush();
  assert.equal(harness.element('workspace-status-bar').hidden, true);
  assert.equal(harness.element('shared-chatgpt-hint').hidden, false);
  assert.match(harness.element('shared-chatgpt-hint').textContent, /Complete provider sign-in/);
  assert.equal(harness.element('shared-chatgpt-sign-in').textContent, 'Return to browser');
  assert.equal(harness.element('chatgpt-device-code').value, 'ABCD-EFGH');
  harness.element('shared-chatgpt-sign-in').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'open_provider_login').length, 1);
  assert.equal(harness.calls.filter(call => ['terminal_start', 'prepare_environment', 'start_agent_session', 'start_project_operation'].includes(call.command)).length, 0);
  assert.equal(harness.terminals.size, 0);
  assert.equal(harness.element('settings-view').hidden, false);
  harness.dom.window.close();
});

test('saved account choices remain explicit and unchanged updates preserve selection and focus', async () => {
  let account = { ...sharedAccount, state: 'signed-out', email: null, candidates: [
    { projectId: 'source-a', name: 'First', email: 'first@example.invalid' },
    { projectId: 'source-b', name: 'Second', email: 'second@example.invalid' },
  ] };
  let completeAdoption;
  const harness = await interfaceHarness({ shared_chatgpt_status: () => account, sync_project_chatgpt: () => account,
    use_project_chatgpt: ({ identifier }) => { assert.equal(identifier, 'source-b'); return new Promise(resolve => { completeAdoption = () => { account = { ...sharedAccount, generation: 2, email: 'second@example.invalid' }; resolve(account); }; }); } });
  await harness.ready; harness.element('settings-navigation').click(); await flush();
  harness.element('settings-tab-providers').click(); await flush();
  const choices = harness.element('saved-chatgpt-project'); choices.value = 'source-b'; choices.focus();
  const option = choices.options[1]; await harness.listeners.get('shared-chatgpt-changed')({ payload: account });
  assert.equal(choices.options[1], option); assert.equal(choices.value, 'source-b'); assert.equal(harness.dom.window.document.activeElement, choices);
  harness.element('use-saved-chatgpt').click(); harness.element('use-saved-chatgpt').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'use_project_chatgpt').length, 1);
  completeAdoption(); await flush(); assert.equal(harness.element('shared-chatgpt-email').textContent, 'second@example.invalid');
  assert.equal(harness.element('saved-chatgpt-accounts').hidden, true); harness.dom.window.close();
});

test('a rejected sign-out preserves the account and leaves active chats running', async () => {
  const harness = await interfaceHarness({ disconnect_shared_chatgpt: () => { throw new Error('End running chats before changing the shared ChatGPT account.'); } });
  await harness.ready; harness.element('choose-folder').click(); await flush(); harness.element('start-codex').click(); await flush();
  harness.element('settings-navigation').click(); await flush(); harness.element('shared-chatgpt-disconnect').click(); await flush();
  assert.equal(harness.element('shared-chatgpt-email').textContent, sharedAccount.email);
  assert.match(harness.element('shared-chatgpt-status').textContent, /End running chats/);
  assert.equal(harness.element('shared-chatgpt-disconnect').disabled, false);
  assert.equal(harness.terminals.size, 1); assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 0);
  harness.dom.window.close();
});

test('old account generations and background errors do not change focus or navigation', async () => {
  const harness = await interfaceHarness(); await harness.ready; harness.element('settings-navigation').click(); await flush();
  harness.element('settings-tab-providers').click(); await flush();
  harness.element('shared-chatgpt-disconnect').focus();
  await harness.listeners.get('shared-chatgpt-changed')({ payload: { ...sharedAccount, generation: 8, email: 'new@example.invalid' } });
  await harness.listeners.get('shared-chatgpt-changed')({ payload: { ...sharedAccount, generation: 2, state: 'signed-out', email: null } });
  await harness.listeners.get('shared-chatgpt-error')({ payload: 'Network temporarily unavailable.' });
  assert.equal(harness.element('shared-chatgpt-email').textContent, 'new@example.invalid');
  assert.equal(harness.element('settings-view').hidden, false);
  assert.equal(harness.dom.window.document.activeElement, harness.element('shared-chatgpt-disconnect'));
  assert.match(harness.element('shared-chatgpt-status').textContent, /Network temporarily/); harness.dom.window.close();
});

test('a failed project connection blocks new chat until shared renewal and reconnect succeed', async () => {
  let failed = true, account = sharedAccount;
  const harness = await interfaceHarness({ shared_chatgpt_status: () => account,
    sync_project_chatgpt: () => { if (failed) throw new Error('Connect the guest home and retry.'); return account; },
    retry_shared_chatgpt: () => { failed = false; account = { ...sharedAccount, generation: 2 }; return account; } });
  await harness.ready; harness.element('choose-folder').click(); await flush(); assert.equal(harness.element('start-codex').disabled, true);
  harness.element('settings-navigation').click(); await flush();
  assert.match(harness.element('shared-chatgpt-status').textContent, /Connect the guest home and retry/);
  assert.equal(harness.element('shared-chatgpt-retry').hidden, false);
  harness.element('shared-chatgpt-retry').click(); await flush(); assert.equal(harness.element('start-codex').disabled, false);
  assert.equal(harness.calls.filter(call => call.command === 'sync_project_chatgpt').length, 2); harness.dom.window.close();
});

test('cancelling provider sign-in preserves the prior account and rejects a late browser update', async () => {
  const harness = await interfaceHarness({ provider_login_status: ({ provider }) => providerLogin(provider, provider === 'chatgpt' ? 'waiting' : 'idle', 1) });
  await harness.ready; harness.element('settings-navigation').click(); await flush();
  harness.element('settings-tab-providers').click(); await flush();
  assert.equal(harness.element('shared-chatgpt-cancel').hidden, false);
  harness.element('shared-chatgpt-cancel').focus();
  harness.element('shared-chatgpt-cancel').click(); await flush();
  await harness.listeners.get('provider-login-changed')({ payload: providerLogin('chatgpt', 'waiting', 1) });
  assert.equal(harness.calls.filter(call => call.command === 'cancel_provider_login').length, 1);
  assert.equal(harness.element('shared-chatgpt-cancel').hidden, true);
  assert.equal(harness.element('shared-chatgpt-email').textContent, sharedAccount.email);
  assert.equal(harness.dom.window.document.activeElement, harness.element('shared-chatgpt-sign-in'));
  assert.equal(harness.calls.filter(call => call.command === 'disconnect_shared_chatgpt' || call.command === 'start_agent_session').length, 0);
  harness.dom.window.close();
});

test('a committed login retry finishes the saved connection without another device login', async () => {
  let account = { ...sharedAccount, generation: 2, state: 'signing-in', pendingProjectId: project.id, pendingFinalizing: true };
  const harness = await interfaceHarness({ shared_chatgpt_status: () => account,
    retry_shared_chatgpt: () => { account = { ...sharedAccount, generation: 3 }; return account; } });
  await harness.ready; harness.element('settings-navigation').click(); await flush();
  assert.equal(harness.element('shared-chatgpt-cancel').hidden, true); assert.equal(harness.element('shared-chatgpt-retry').textContent, 'Finish connection');
  harness.element('shared-chatgpt-retry').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'retry_shared_chatgpt').length, 1);
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 0); harness.dom.window.close();
});

test('both providers connect from Settings with an empty project registry', async () => {
  const harness = await interfaceHarness({ shared_chatgpt_status: () => ({ ...sharedAccount, state: 'signed-out', email: null }), shared_claude_status: () => ({ ...sharedClaudeAccount, state: 'signed-out', email: null }) });
  await harness.ready; harness.element('settings-navigation').click(); await flush();
  assert.equal(harness.element('shared-chatgpt-sign-in').disabled, false);
  assert.equal(harness.element('claude-sign-in').disabled, false);
  harness.element('shared-chatgpt-sign-in').click(); harness.element('claude-sign-in').click(); await flush();
  assert.deepEqual(harness.calls.filter(call => call.command === 'start_provider_login').map(call => call.args.provider).sort(), ['chatgpt', 'claude']);
  assert.equal(harness.element('settings-view').hidden, false);
  assert.equal(harness.calls.filter(call => ['choose_project_folder', 'register_project', 'prepare_project', 'terminal_start', 'prepare_environment', 'start_agent_session', 'start_project_operation'].includes(call.command)).length, 0);
  assert.equal(harness.terminals.size, 0);
  assert.equal(harness.element('claude-cancel').hidden, false);
  harness.dom.window.close();
});

test('an offline project keeps the shared account and explains its network setting without login', async () => {
  const harness = await interfaceHarness({ sync_project_chatgpt: () => { throw new Error('This project is offline. Enable isolated networking in project settings to connect ChatGPT.'); } });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.equal(harness.element('start-codex').disabled, true);
  assert.match(harness.element('project-account-status').textContent, /project is offline/);
  assert.equal(harness.element('shared-chatgpt-email').textContent, sharedAccount.email);
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 0); harness.dom.window.close();
});

test('a project switch discards the old account connection before enabling the new chat', async () => {
  let releaseFirst;
  const second = { ...project, id: 'project-b', name: 'Second', path: '/fixture/second' };
  const harness = await interfaceHarness({ sync_project_chatgpt: ({ identifier }) => identifier === project.id
    ? new Promise(resolve => { releaseFirst = resolve; }) : sharedAccount });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('project-navigation').click(); await flush();
  harness.handlers.register_project = () => second;
  harness.handlers.prepare_project = () => ({ project:second, plan:{data:{...plan.data,project:{...plan.data.project,host:second.path}}}, fingerprint:'plan-b',message:null,candidates:[] });
  harness.handlers.read_project_preferences = () => ({project:second,options:settings});
  harness.element('choose-folder').click(); await flush(); assert.equal(harness.element('start-codex').disabled,true);
  releaseFirst({ ...sharedAccount, state:'signed-out',email:null }); await flush();
  assert.equal(harness.element('project-heading').textContent,'Second'); assert.equal(harness.element('start-codex').disabled,false);
  assert.equal(harness.calls.filter(call => call.command==='sync_project_chatgpt').at(-1).args.identifier,second.id);
  assert.equal(harness.element('shared-chatgpt-email').textContent,sharedAccount.email); harness.dom.window.close();
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

test('startup checks folders automatically without displaying a rescan button or opening a project', async () => {
  let projects = [{ ...project, available: false }];
  let finishScan;
  const harness = await interfaceHarness({
    recent_projects: () => projects,
    rescan_projects: () => new Promise(resolve => { finishScan = () => { projects = [{ ...project, available: true }]; resolve(projects); }; }),
  });
  assert.equal(harness.element('rescan-projects'), null);
  assert.equal(harness.element('project-rescan-status').hidden, true);
  assert.match(harness.element('workspace-status-bar').textContent, /Checking project folders/);
  assert.equal(harness.element('project-rescan-status').textContent, 'Checking project folders…');
  assert.equal(harness.element('empty-projects').hidden, true);
  harness.dom.window.dispatchEvent(new harness.dom.window.Event('focus'));
  harness.dom.window.dispatchEvent(new harness.dom.window.Event('focus'));
  await flush();
  assert.equal(harness.calls.filter(call => call.command === 'rescan_projects').length, 1);
  assert.equal(harness.element('choose-folder').disabled, false);
  finishScan(); await harness.ready;
  const button = harness.element('recent-project-list').querySelector('.recent-project');
  assert.equal(button.disabled, false);
  assert.equal(button.dataset.projectId, project.id);
  assert.equal(harness.element('sidebar-project-list').querySelector('button').disabled, false);
  assert.equal(harness.element('recent-project-list').querySelector('.project-record-forget'), null);
  assert.equal(harness.element('project-rescan-status').hidden, true);
  assert.equal(harness.calls.filter(call => ['register_project', 'open_project', 'prepare_project', 'prepare_environment', 'forget_project', 'remove_project', 'terminal_close'].includes(call.command)).length, 0);
  button.click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'open_project').at(-1).args.identifier, project.id);
  assert.match(harness.element('project-heading').textContent, /örnek proje/);
  harness.dom.window.close();
});

test('unavailable folders and failed automatic checks retry until the drive reconnects', async () => {
  let fail = false, clock;
  let projects = [{ ...project, available: false }];
  const harness = await interfaceHarness({
    recent_projects: () => projects,
    rescan_projects: () => { if (fail) throw new Error('Registry temporarily locked'); return projects; },
  }, dom => { clock = captureProjectRescans(dom); });
  await harness.ready;
  assert.equal(clock.pending.size, 1);
  assert.match(harness.element('project-rescan-status').textContent, /Connect the drive.*automatically/);
  assert.equal(harness.element('recent-project-list').querySelector('.recent-project').disabled, true);
  fail = true; clock.run(); await flush();
  assert.match(harness.element('project-rescan-status').textContent, /try again automatically.*Registry temporarily locked/);
  assert.equal(harness.element('recent-project-list').querySelectorAll('tr').length, 1);
  assert.equal(harness.element('choose-folder').disabled, false);
  assert.equal(harness.element('recent-project-list').querySelector('.project-record-forget').disabled, false);
  fail = false; clock.run(); await flush();
  assert.match(harness.element('project-rescan-status').textContent, /unavailable.*automatically/);
  projects = [{ ...project, available: true }];
  clock.run(); await flush();
  assert.equal(harness.element('recent-project-list').querySelector('.recent-project').disabled, false);
  assert.equal(harness.element('project-rescan-status').hidden, true);
  assert.equal(clock.pending.size, 0);
  assert.equal(harness.calls.filter(call => call.command === 'rescan_projects').length, 4);
  harness.dom.window.close();
});

test('an automatic check that finishes after navigation keeps the selected project', async () => {
  const other = { ...project, id: 'other', name: 'Other project', available: true };
  let projects = [{ ...project, available: false }, other];
  let finishScan, first = true;
  const harness = await interfaceHarness({
    recent_projects: () => projects,
    open_project: () => other,
    prepare_project: () => ({ project: other, plan, fingerprint: 'other-plan', message: null, candidates: [] }),
    rescan_projects: () => { if (first) { first = false; return projects; } return new Promise(resolve => { finishScan = () => { projects = [{ ...project, available: true }, other]; resolve(projects); }; }); },
  });
  await harness.ready;
  harness.dom.window.dispatchEvent(new harness.dom.window.Event('focus')); await flush();
  const otherButton = harness.element('recent-project-list').querySelector('[data-project-id="other"]');
  assert.equal(otherButton.disabled, false);
  otherButton.click(); await flush();
  finishScan(); await flush();
  assert.equal(harness.element('project-heading').textContent, 'Other project');
  assert.equal(harness.element('project-rescan-status').hidden, true);
  assert.equal(harness.calls.filter(call => call.command === 'open_project').length, 1);
  harness.element('project-navigation').click(); await flush();
  assert.equal(harness.element('recent-project-list').querySelector('[data-project-id="project-a"]').disabled, false);
  harness.dom.window.close();
});

test('automatic folder checks retain keyboard focus and stop when all folders become available', async () => {
  let clock;
  const other = { ...project, id: 'other', name: 'Other project', available: true };
  let projects = [{ ...project, available: false }, other];
  const harness = await interfaceHarness({ recent_projects: () => projects }, dom => { clock = captureProjectRescans(dom); });
  await harness.ready;
  const otherButton = () => harness.element('recent-project-list').querySelector('[data-project-id="other"]');
  otherButton().focus();
  const original = otherButton();
  clock.run(); await flush();
  assert.equal(otherButton(), original);
  assert.equal(harness.dom.window.document.activeElement, original);
  projects = [{ ...project, available: true }, other];
  clock.run(); await flush();
  assert.equal(harness.dom.window.document.activeElement, otherButton());
  assert.equal(clock.pending.size, 0);
  harness.dom.window.close();
});

test('automatic checks pause in Settings and resume when the project library is shown', async () => {
  let clock;
  const harness = await interfaceHarness({ recent_projects: () => [{ ...project, available: false }] }, dom => { clock = captureProjectRescans(dom); });
  await harness.ready;
  assert.equal(clock.pending.size, 1);
  harness.element('settings-navigation').click(); await flush();
  assert.equal(clock.pending.size, 0);
  harness.dom.window.dispatchEvent(new harness.dom.window.Event('focus')); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'rescan_projects').length, 1);
  harness.element('project-navigation').click(); await flush();
  assert.equal(clock.pending.size, 1);
  clock.run(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'rescan_projects').length, 2);
  harness.dom.window.close();
});

test('incomplete removal preserves its saved operation without showing account controls', async () => {
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
  assert.equal(harness.element('logout-on-removal'), null);
  assert.equal(harness.calls.filter(call => ['remove_project', 'start_agent_session', 'prepare_environment'].includes(call.command)).length, 0);
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
  harness.element('close-session-usage').click(); turns = turns.slice(0, 1);
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, sessionDashboard([{ ...conversation, usageRevision: 'only-interrupted' }])] });
  await open();
  assert.match(harness.element('session-usage-status').textContent, /Prompts are recorded, but no request usage/);
  harness.element('close-session-usage').click(); turns = [];
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, sessionDashboard([{ ...conversation, usageRevision: 'no-turns' }])] });
  await open();
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
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').at(-1).args.sessionId, 'first');
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
  assert.match(fs.readFileSync(root + 'index.html', 'utf8'), /id="application-version"[^>]*>Loading…/);
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
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 0);
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
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 0);
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
      assert.deepEqual([...harness.element('project-usage').querySelectorAll('dd')].map(value => value.textContent), ['Unknown', 'Unknown', 'Unknown', 'Unknown', 'Unknown']);
      assert.doesNotMatch(harness.element('session-table-body').textContent, /\$0/);
      assert.match(report.textContent, /Token counts and API cost are unknown/);
      assert.equal(report.querySelectorAll('.request-usage').length, 0);
    }
    if (name === 'valid-zero-token-exact-request') {
      assert.equal(harness.element('project-usage').querySelectorAll('dd')[0].textContent, '0');
      assert.equal(harness.element('project-usage').querySelectorAll('dd')[4].textContent, '$0.00');
      assert.match(report.textContent, /0 \(0%\).*\$0\.000000/s);
      assert.equal(report.querySelectorAll('.request-usage tbody tr').length, 1);
    }
    if (name === 'empty-history') {
      assert.equal(report.children.length, 0);
      assert.match(harness.element('session-usage-status').textContent, /No prompts have been recorded/);
      assert.equal(harness.element('project-usage').querySelectorAll('dd')[4].textContent, '$0.00');
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
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 0);
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

test('a delayed usage close event and old result cannot clear the next dialog', async t => {
  const first = deferred(), second = deferred();
  const harness = await interfaceHarness({
    project_dashboard: () => sessionDashboard([{ id: 'first', title: 'First usage', updatedAt: 1 }, { id: 'second', title: 'Second usage', updatedAt: 2 }]),
    conversation_usage: ({ sessionId }) => sessionId === 'first' ? first.promise : second.promise,
  });
  t.after(() => harness.dom.window.close());
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('usage-for-first').click(); await flush();
  const dialog = harness.element('session-usage-dialog');
  let deliverClose;
  dialog.close = () => { dialog.open = false; deliverClose = () => dialog.dispatchEvent(new harness.dom.window.Event('close')); };
  harness.element('close-session-usage').click();
  harness.element('usage-for-second').click(); await flush();
  deliverClose();
  first.reject(new Error('Old usage failure')); await flush();
  assert.equal(dialog.open, true);
  assert.equal(harness.element('session-usage-title').textContent, 'Second usage');
  assert.equal(harness.element('session-usage-details').getAttribute('aria-busy'), 'true');
  assert.ok(harness.element('session-usage-details').querySelector('.skeleton'));
  assert.doesNotMatch(harness.element('session-usage-status').textContent, /Old usage failure/);
  second.resolve(sessionDetails('second', [])); await flush();
  assert.equal(harness.element('session-usage-details').dataset.usageSessionId, 'second');
  assert.equal(harness.element('session-usage-details').getAttribute('aria-busy'), 'false');
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

test('manual snapshots and usage events respect the backend read order', async () => {
  const snapshot = (title, sequence) => sessionDashboard([{ id: 'one', title, updatedAt: 1 }], { sequence });
  const h = await interfaceHarness({ project_dashboard: () => snapshot('Initial', 1) });
  h.element('choose-folder').click(); await flush();
  let finish;
  h.handlers.project_dashboard = () => new Promise(resolve => { finish = resolve; });
  h.element('refresh-sessions').click(); await flush();
  const event = h.listeners.get('project-usage-updated');
  await event({ payload: [project.id, snapshot('New event', 3)] });
  finish(snapshot('Old read', 2)); await flush();
  const title = () => h.element('session-table-body').querySelector('.session-title').textContent;
  assert.equal(title(), 'New event');
  assert.equal(h.element('refresh-sessions').disabled, false);
  h.handlers.project_dashboard = () => snapshot('New read', 5);
  h.element('refresh-sessions').click(); await flush();
  await event({ payload: [project.id, snapshot('Old event', 4)] });
  assert.equal(title(), 'New read');
  const error = h.listeners.get('project-usage-error');
  await error({ payload: [project.id, 'Current failure', 7] });
  await event({ payload: [project.id, snapshot('Delayed success', 6)] });
  assert.equal(title(), 'New read');
  assert.match(h.element('dashboard-session-status').textContent, /Current failure/);
  await event({ payload: [project.id, snapshot('Recovered', 8)] });
  await error({ payload: [project.id, 'Stale failure', 7] });
  assert.equal(title(), 'Recovered');
  assert.doesNotMatch(h.element('dashboard-session-status').textContent, /failure/);
  h.dom.window.close();
});

test('a usage read failure survives terminal refreshes and recovers on retry', async () => {
  const h = await interfaceHarness({ project_dashboard: () => { throw Error('Unreadable history'); } });
  h.element('choose-folder').click(); await flush();
  await h.listeners.get('terminal-sessions-changed')({ payload: project.id }); await flush();
  const status = h.element('dashboard-session-status');
  assert.match(status.textContent, /Unreadable history.*Refresh/);
  assert.equal(h.element('project-overview').getAttribute('aria-busy'), 'false');
  h.handlers.project_dashboard = () => sessionDashboard([{ id: 'one', title: 'Recovered chat', updatedAt: 1 }]);
  h.element('refresh-sessions').click(); await flush();
  assert.match(h.element('session-table-body').textContent, /Recovered chat/);
  assert.doesNotMatch(status.textContent, /Unreadable/);
  h.handlers.project_dashboard = () => { throw Error('Transient error'); };
  h.element('refresh-sessions').click(); await flush();
  assert.match(h.element('session-table-body').textContent, /Recovered chat/);
  assert.equal(h.element('session-table').hidden, false);
  assert.match(status.textContent, /Transient error/);
  h.dom.window.close();
});

test('interrupted Mac checks use one completed snapshot for the selected CLI path', async () => {
  const result = brigStatus => ({ architecture:'arm64', osVersion:'27.2', tools:{brig:brigStatus === 'installed' ? '/tool/brig' : null, homebrew:'/tool/brew'}, brigStatus,
    version:brigStatus === 'installed' ? {data:{version:'v0.5.2'}} : null, doctor:null, message:null });
  let next = 'installed';
  const h = await interfaceHarness({inspect_host: () => result(next)});
  await h.ready;
  for (next of ['missing','cancelled','timed-out']) {
    h.element('check-host').click(); await flush();
    assert.match(h.element('host-status').textContent, /Brig not found/);
    assert.doesNotMatch(h.element('host-status').textContent,/v0.5.2/);
    assert.match(h.element('host-tools').textContent,/Not found/);
    assert.equal(h.dom.window.document.getElementById('install-brig'),null);
  }
  h.element('cli-path').value = '/other/brig';
  h.element('check-host').click(); await flush();
  assert.doesNotMatch(h.element('host-status').textContent,/previous completed/);
  assert.equal(h.dom.window.document.getElementById('install-brig'),null);
  h.dom.window.close();
});

test('System Checks names the effective bundled CLI source without a Homebrew installation action', async () => {
  const h = await interfaceHarness({ inspect_host: () => ({ architecture:'arm64', osVersion:'27.2',
    tools:{brig:'/App/Tugboat.app/Contents/MacOS/brig',hull:'/App/Tugboat.app/Contents/MacOS/hull'},
    brigSource:'bundled', brigStatus:'installed', version:{data:{version:'v0.5.2'}}, doctor:null, message:null }) });
  await h.ready;
  assert.match(h.element('host-status').textContent,/Bundled Brig.*v0\.5\.2/);
  assert.equal(h.dom.window.document.getElementById('install-brig'),null);
  assert.equal(h.dom.window.document.getElementById('homebrew-help'),null);
  assert.match(h.element('host-tools').textContent,/Contents\/MacOS\/brig/);
  h.dom.window.close();
});

test('only the top dialog is accessible and closing it returns focus to the current trigger', async t => {
  const conversation = { id: 'chat-a', title: 'QA chat', updatedAt: 1000 };
  const h = await interfaceHarness({
    preview_removal: () => ({ reference: 'codex@sample-a', workspace: '/fixture/guest-home', description: 'Test session', fingerprint: 'remove-a', external: false, hasEnvironment: true }),
    project_dashboard: () => sessionDashboard([conversation]),
    conversation_usage: () => sessionDetails(conversation.id, []),
  });
  t.after(() => h.dom.window.close());
  const doc = h.dom.window.document;
  function checkModal(id) {
    for (const dialog of doc.querySelectorAll('dialog')) {
      const active = dialog.id === id;
      assert.equal(dialog.getAttribute('aria-modal'), active ? 'true' : null, dialog.id);
      assert.equal(dialog.getAttribute('aria-hidden'), active ? null : 'true', dialog.id);
      assert.equal(dialog.inert, !active, dialog.id);
    }
  }
  await h.ready;
  h.element('choose-folder').click(); await flush();
  checkModal(null);
  h.element('project-settings-button').click(); await flush();
  checkModal('project-settings-dialog');
  const preview = h.element('preview-removal');
  preview.closest('details').open = true;
  preview.click(); await flush();
  checkModal('removal-dialog');
  assert.equal(doc.activeElement, h.element('removal-confirmation'));
  h.element('removal-dialog').close();
  checkModal('project-settings-dialog');
  assert.equal(doc.activeElement, preview);
  h.element('close-project-settings').click();
  const usageButton = () => h.element('session-table-body').querySelector('[aria-label="Usage for QA chat"]');
  const original = usageButton();
  original.click(); await flush();
  checkModal('session-usage-dialog');
  assert.equal(doc.activeElement, h.element('close-session-usage'));
  await publishUsage(h, [conversation]);
  assert.notEqual(usageButton(), original);
  h.element('close-session-usage').click();
  checkModal(null);
  assert.equal(doc.activeElement, usageButton());
});


test('preparation policy and boundary requests select the recorded agent environment', async () => {
  const harness = await interfaceHarness({
    register_project: () => ({ ...project, stage: 'selected' }),
    read_project_policy: () => ({ document: {}, fingerprint: 'policy-a', editable: true, attached: true, allowedHosts: ['api.openai.com'] }),
    change_project_policy: () => project,
    run_boundary_tests: () => ({ projectId: project.id, environmentId: project.environmentId, planFingerprint: 'plan-a', checkedAt: 1, passed: true, cleanup: true, results: [{ name: 'Network', target: 'example.com', expected: 'Denied', observed: 'Denied', outcome: 'pass' }] }),
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('project-settings-button').click(); await flush();
  harness.element('read-project-policy').click(); await flush();
  harness.element('run-boundary-tests').click(); await flush();
  harness.element('save-project-policy').click(); await flush();
  for (const command of ['prepare_project', 'preview_project', 'read_project_policy', 'change_project_policy', 'run_boundary_tests']) {
    const request = harness.calls.find(call => call.command === command);
    assert.ok(request, command);
    assert.equal(request.args.identifier, project.id, command);
    assert.equal(request.args.environmentId, project.environmentId, command);
  }
  harness.dom.window.close();
});

test('preparation events from another agent environment cannot change the selected Codex environment', async () => {
  const harness = await interfaceHarness();
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const count = harness.calls.filter(call => call.command === 'preview_project').length;
  const foreign = { ...project, environmentId: 'b'.repeat(32), agentKind: 'claude-code', name: 'Other agent', stage: 'ready' };
  await harness.listeners.get('environment-preparation-finished')({ payload: { project: foreign, terminalIdentifier: 999, message: 'Other agent finished.' } });
  await harness.listeners.get('environment-preparation-error')({ payload: { projectId: project.id, environmentId: foreign.environmentId, terminalIdentifier: 999, message: 'Other agent failed.' } });
  await flush();
  assert.equal(harness.calls.filter(call => call.command === 'preview_project').length, count);
  assert.equal(harness.element('project-heading').textContent, project.name);
  assert.doesNotMatch(harness.element('preparation-status').textContent, /Other agent/);
  assert.equal(harness.element('start-codex').disabled, false);
  harness.dom.window.close();
});

test('foreign environment or stale plan boundary reports cannot appear as a passed selected check', async () => {
  const report = { projectId: project.id, environmentId: project.environmentId, planFingerprint: 'plan-a', checkedAt: 1, passed: true, cleanup: true, results: [{ name: 'Network', target: 'example.com', expected: 'Denied', observed: 'Denied', outcome: 'pass' }] };
  const harness = await interfaceHarness({ run_boundary_tests: () => report });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  for (const [environmentId, fingerprint] of [['b'.repeat(32), 'plan-a'], [project.environmentId, 'old-plan']]) {
    report.environmentId = environmentId; report.planFingerprint = fingerprint;
    harness.element('run-boundary-tests').click(); await flush();
    assert.match(harness.element('boundary-status').textContent, /another environment or plan/);
    assert.equal(harness.element('boundary-results').children.length, 0);
  }
  report.environmentId = project.environmentId; report.planFingerprint = 'plan-a';
  harness.element('run-boundary-tests').click(); await flush();
  assert.match(harness.element('boundary-status').textContent, /checks passed/);
  assert.equal(harness.element('boundary-results').children.length, 1);
  harness.dom.window.close();
});

const claudeProject = { ...project, environmentId: 'b'.repeat(32), agentKind: 'claude-code', profile: 'tugboat-claude', label: 'claude-fixture' };
const environmentPlan = selected => ({ ...plan, data: { ...plan.data, profile: selected.profile, session: selected.label, home: { host: '/fixture/home-' + selected.agentKind, guest: '/root' } } });
async function twoAgentHarness(overrides = {}) {
  const selected = args => args?.environmentId === claudeProject.environmentId ? claudeProject : project;
  return interfaceHarness({
    ensure_project_environment: ({ agentKind }) => agentKind === 'claude-code' ? claudeProject : project,
    prepare_project: args => ({ project: selected(args), plan: environmentPlan(selected(args)), fingerprint: selected(args).agentKind + '-plan', message: null, candidates: [] }),
    preview_project: args => ({ project: selected(args), plan: environmentPlan(selected(args)), fingerprint: selected(args).agentKind + '-plan' }),
    ...overrides,
  });
}
function chooseAgent(harness, kind) {
  harness.element('agent-choice').value = kind;
  harness.element('agent-choice').dispatchEvent(new harness.dom.window.Event('change'));
}

function recordedClaudeDetails(sessionId) {
  const captured = fs.readFileSync(root + 'tests/fixtures/claude-code/v2.1.318/priced-details.json', 'utf8');
  const original = JSON.parse(captured).sessionId;
  return JSON.parse(captured.replaceAll(original, sessionId));
}

test('usage caches separate equal session IDs and survive an agent switch within the project dashboard', async t => {
  const identifier = '11111111-1111-4111-8111-111111111111';
  const conversations = [project, claudeProject].map(selected => conversationFixture({
    id: identifier, environmentId: selected.environmentId, agentKind: selected.agentKind,
    title: selected.agentKind + ' stored usage', updatedAt: 1, usageAvailable: true,
  }));
  const harness = await twoAgentHarness({
    project_dashboard: () => sessionDashboard(conversations),
    conversation_usage: ({ sessionId, environmentId }) => sessionDetails(sessionId, [{ id: 'turn', status: 'complete', requests: [{
      at: 'now', model: environmentId === project.environmentId ? 'Codex stored model' : 'Claude stored model',
      tokens: { input: 1, cached: 0, cacheWrite: 0, output: 1 }, costUsd: 0, unpricedReason: null, rates: null,
    }] }]),
  });
  t.after(() => harness.dom.window.close());
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const open = async environmentId => { harness.element('usage-for-' + environmentId + '/' + identifier).click(); await flush(); };
  await open(project.environmentId);
  assert.match(harness.element('session-usage-details').textContent, /Codex stored model/);
  harness.element('close-session-usage').click();
  await open(claudeProject.environmentId);
  assert.match(harness.element('session-usage-details').textContent, /Claude stored model/);
  harness.element('close-session-usage').click();
  chooseAgent(harness, 'claude-code'); await flush();
  assert.equal(harness.element('session-table-body').rows.length, 2);
  assert.match(harness.element('sidebar-session-list').textContent, /codex stored usage.*claude-code stored usage/s);
  await open(project.environmentId);
  assert.match(harness.element('session-usage-details').textContent, /Codex stored model/);
  assert.doesNotMatch(harness.element('session-usage-details').textContent, /Claude stored model/);
  assert.equal(harness.calls.filter(call => call.command === 'conversation_usage').length, 2);
});

test('a pending chat launch does not reuse the previous agent logo', async () => {
  const harness = await twoAgentHarness();
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.element('terminal-status').querySelector('.agent-logo').dataset.agentKind, 'codex');
  const pending = deferred();
  const start = harness.handlers.start_agent_session;
  harness.handlers.start_agent_session = async args => { await pending.promise; return start(args); };
  harness.element('hide-terminal').click(); await flush();
  chooseAgent(harness, 'claude-code'); await flush();
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.element('terminal-status').textContent, 'Opening New Chat…');
  assert.equal(harness.element('terminal-status').querySelector('.agent-logo'), null);
  assert.equal(harness.element('terminal-status').getAttribute('aria-label'), null);
  pending.resolve(); await flush();
  assert.equal(harness.element('terminal-status').querySelector('.agent-logo').dataset.agentKind, 'claude-code');
  for (const terminal of harness.terminals.values()) terminal.running = false;
  harness.handlers.terminal_start = () => {
    harness.setTerminal({ identifier: 80, projectId: project.id, purpose: 'local', running: true, exitCode: null });
    return 80;
  };
  harness.element('hide-terminal').click(); await flush();
  harness.element('start-terminal').click(); await flush();
  assert.equal(harness.element('terminal-status').textContent, 'Local terminal');
  assert.equal(harness.element('terminal-status').querySelector('.agent-logo'), null);
  assert.equal(harness.element('terminal-status').getAttribute('aria-label'), null);
  harness.dom.window.close();
});

test('shared Claude sign-in and two chats keep the Codex terminal and environment selection', async () => {
  let account = { ...sharedClaudeAccount, state: 'signed-out', email: null };
  const harness = await twoAgentHarness({ shared_claude_status: () => account });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  const codexTerminal = [...harness.terminals.values()][0];
  chooseAgent(harness, 'claude-code'); await flush();
  assert.equal(harness.element('start-codex').disabled, true);
  harness.element('project-provider-settings').click(); await flush();
  harness.element('claude-sign-in').click(); await flush();
  assert.equal(harness.element('settings-view').hidden, false);
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session' && call.args.action === 'login').length, 0);
  account = { ...sharedClaudeAccount, generation: 2 };
  await harness.listeners.get('shared-claude-changed')({ payload: account });
  await harness.listeners.get('provider-login-changed')({ payload: providerLogin('claude', 'idle', 2) }); await flush();
  assert.equal(harness.element('claude-account-email').textContent, account.email);
  harness.element('project-navigation').click(); await flush();
  harness.element('choose-folder').click(); await flush(); chooseAgent(harness, 'claude-code'); await flush();
  harness.element('start-codex').click(); await flush();
  harness.element('hide-terminal').click(); await flush(); harness.element('start-codex').click(); await flush();
  const claudeChats = [...harness.terminals.values()].filter(terminal => terminal.purpose === 'claude');
  assert.equal(claudeChats.length, 2);
  assert.equal(harness.terminals.get(codexTerminal.identifier).running, true);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 0);
  harness.dom.window.close();
});

test('equal native session IDs keep distinct agent rows, resumes and archive targets', async () => {
  const identifier = '11111111-1111-4111-8111-111111111111';
  const codex = conversationFixture({ id: identifier, title: 'Same title', updatedAt: 1000, environmentId: project.environmentId, agentKind: 'codex', usageAvailable: true });
  const details = recordedClaudeDetails(identifier);
  const claude = { ...codex, environmentId: claudeProject.environmentId, agentKind: 'claude-code', usageAvailable: true, usage: details.combinedUsage, familyUsage: details.combinedUsage, coverage: 'partial' };
  const changes = [];
  const harness = await twoAgentHarness({ project_dashboard: () => sessionDashboard([codex, claude], { usage: details.combinedUsage, usageAvailable: true, coverage: 'partial' }), conversation_usage: args => { assert.equal(args.environmentId, claudeProject.environmentId); assert.equal(args.sessionId, identifier); return details; }, change_conversation_state: args => { changes.push(args); } });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const rows = [...harness.element('session-table-body').rows];
  assert.equal(rows.length, 2); assert.match(rows[0].textContent, /Codex/); assert.match(rows[1].textContent, /Claude Code/);
  assert.equal(rows[0].querySelector('.session-agent-tag').textContent, 'Codex');
  assert.equal(rows[1].querySelector('.session-agent-tag').textContent, 'Claude Code');
  assert.equal(rows[1].cells[5].textContent, '$0.0557');
  assert.match(rows[1].cells[4].title, /82,948 total tokens/);
  assert.equal(rows[0].querySelector('[aria-label="Archive Same title, Codex"]').title, 'Archive Same title, Codex');
  assert.ok(rows[1].querySelector('[aria-label="Delete Same title, Claude Code"]'));
  rows[1].querySelector('[aria-label="Usage for Same title, Claude Code"]').click(); await flush();
  assert.equal(harness.element('session-usage-dialog').open, true);
  assert.match(harness.element('session-usage-details').textContent, /Agent turn 1 · complete/);
  assert.doesNotMatch(harness.element('session-usage-details').textContent, /Prompt 1/);
  assert.match(harness.element('session-usage-status').textContent, /User prompt count unknown.*Recorded source coverage: partial.*discovered subagent messages/s);
  assert.match(harness.element('session-usage-details').textContent, /\$0\.034285 \(estimate\).*Anthropic rates 2026-11-13.*5-minute cache writes assumed/s);
  assert.equal(harness.element('session-usage-details').querySelector('time a').href, 'https://platform.claude.com/docs/en/about-claude/pricing');
  assert.match(harness.element('usage-note').textContent, /labelled standard\/global estimates/);
  harness.element('close-session-usage').click(); await flush();
  rows[1].querySelector('[aria-label="Archive Same title, Claude Code"]').click(); await flush();
  assert.equal(changes[0].environmentId, claudeProject.environmentId);
  for (const environmentId of [project.environmentId, claudeProject.environmentId]) {
    const kind = environmentId === project.environmentId ? 'codex' : 'claude-code';
    const sidebarRow = harness.element('sidebar-session-list').querySelector('[data-session-id="' + environmentId + '/' + identifier + '"]');
    assert.equal(sidebarRow.querySelector('.agent-logo').dataset.agentKind, kind);
    assert.equal(sidebarRow.querySelector('.agent-logo').getAttribute('aria-hidden'), 'true');
    sidebarRow.click(); await flush();
    assert.equal(harness.element('terminal-status').querySelector('.agent-logo').dataset.agentKind, kind);
    assert.equal(harness.element('terminal-status').getAttribute('aria-label'), (kind === 'codex' ? 'Codex' : 'Claude Code') + ': Same title');
  }
  const launches = harness.calls.filter(call => call.command === 'start_agent_session');
  assert.equal(launches.length, 2);
  assert.deepEqual(launches.map(call => call.args.environmentId), [project.environmentId, claudeProject.environmentId]);
  assert.deepEqual([...harness.terminals.values()].map(status => status.purpose), ['resume', 'claude-resume']);
  assert.ok(launches.every(call => call.args.sessionId === identifier));
  harness.element('sidebar-session-list').querySelector('[data-session-id="' + claudeProject.environmentId + '/' + identifier + '"]').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 2);
  assert.match(harness.element('usage-note').textContent, /Claude totals include recorded main-session and discovered subagent messages/);
  assert.match(harness.element('project-usage').textContent, /82,948/);
  assert.doesNotMatch(harness.element('project-usage').textContent, /Usage unavailable/);
  assert.equal(harness.element('project-usage').querySelectorAll('dd')[4].textContent, '$0.0557');
  harness.dom.window.close();
});

test('Claude missing usage shows known subtotals plus unknown and keeps details available', async () => {
  const identifier = '1a5bc5b5-af21-432d-bf07-e2218dd9f9bf';
  const details = recordedClaudeDetails(identifier);
  const usage = { ...details.combinedUsage, unmeasuredTurns: 1 };
  details.combinedUsage = usage; details.ownUsage = usage; details.agents[0].ownUsage = usage;
  details.agents[0].turns.push({ id: 'msg_missing_cache', status: 'complete', requests: [], originThreadId: identifier, rootTurnId: null, parentTurnId: null });
  details.agentTurnCount++;
  const claude = conversationFixture({ id: identifier, environmentId: claudeProject.environmentId, agentKind: 'claude-code', title: 'Partial Claude chat', usageAvailable: true, available: false, usage, coverage: 'partial' });
  const harness = await twoAgentHarness({ project_dashboard: () => sessionDashboard([claude], { usage, coverage: 'partial', usageAvailable: true }), conversation_usage: () => details });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const row = harness.element('session-table-body').rows[0];
  assert.equal(row.cells[1].textContent, '82.5K + unknown');
  assert.equal(row.cells[2].textContent, '76.1K + unknown');
  assert.doesNotMatch(row.cells[2].textContent, /%/);
  assert.match(harness.element('project-usage').textContent, /82,948 \+ unknown.*82,539 \+ unknown.*76,065 \+ unknown/s);
  assert.doesNotMatch(harness.element('project-usage').textContent, /%/);
  assert.equal(harness.element('project-usage').querySelectorAll('dd')[4].textContent, '$0.0557 + unknown');
  row.querySelector('[aria-label="Usage for Partial Claude chat, Claude Code"]').click(); await flush();
  assert.match(harness.element('session-usage-details').textContent, /Agent turn 6.*No request usage was recorded.*Token counts and API cost are unknown/s);
  assert.match(harness.element('session-usage-status').textContent, /1 agent turn\(s\) have unknown token counts/);
  harness.element('close-session-usage').click(); await flush();
  const unknown = { ...emptyUsage, unmeasuredTurns: 1 };
  await harness.listeners.get('project-usage-updated')({ payload: [project.id, sessionDashboard([{ ...claude, usage: unknown, familyUsage: unknown, usageRevision: 'unknown-only' }], { usage: unknown, usageAvailable: true, coverage: 'unknown' })] });
  const unknownRow = harness.element('session-table-body').rows[0];
  for (const cell of [...unknownRow.cells].slice(1, 6)) assert.equal(cell.textContent, 'Unknown');
  assert.ok(unknownRow.querySelector('[aria-label="Usage for Partial Claude chat, Claude Code"]'));
  assert.doesNotMatch(harness.element('project-usage').textContent, /\$0|0%/);
  harness.dom.window.close();
});

test('native priced Claude family exposes separate main and nested costs with saved source metadata', async () => {
  const contract = JSON.parse(fs.readFileSync(root + 'tests/fixtures/claude-code/priced-subagents/native-contract.json', 'utf8'));
  const harness = await twoAgentHarness({ project_dashboard: () => structuredClone(contract.dashboard), conversation_usage: () => structuredClone(contract.details) });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const rows = harness.element('session-table-body').rows;
  assert.equal(rows.length, 1);
  assert.equal(rows[0].cells[5].textContent, '$0.0599');
  assert.match(harness.element('usage-note').textContent, /Anthropic catalogue checked 2026-11-13.*Each request retains its dated rates/);
  assert.doesNotMatch(harness.element('usage-note').textContent, /API prices are unavailable/);
  rows[0].querySelector('[aria-label^="Usage for "]').click(); await flush();
  const content = harness.element('session-usage-details');
  assert.deepEqual([...content.querySelectorAll('.usage-scope-table tbody tr')].map(row => [row.cells[0].textContent, row.cells[6].textContent]), [['Main agent', '$0.0014'], ['Subagents', '$0.0585'], ['Combined', '$0.0599']]);
  assert.match(content.textContent, /fast speed · us inference/);
  assert.match(content.textContent, /3 at 5 minutes · 2 at 1 hour/);
  assert.match(content.textContent, /batch tier/);
  assert.match(content.textContent, /\$0\.055060.*Recorded rates for prompts over 100,000 input tokens/s);
  assert.equal(content.querySelectorAll('time a[href="https://platform.claude.com/docs/en/about-claude/pricing"]').length, 4);
  assert.doesNotMatch(content.textContent, /\(estimate\)|cache writes assumed/);
  assert.match(harness.element('session-usage-status').textContent, /Subscription charges, account discounts, and server-tool charges are separate/);
  harness.element('close-session-usage').click(); await flush();
  contract.details.agents[0].turns[0].requests[0].rates.source = 'javascript:alert("fixture")';
  harness.handlers.recorded_project_dashboard = () => structuredClone(contract.dashboard);
  await harness.listeners.get('records-changed')({ payload: ['usage'] }); await flush();
  harness.element('session-table-body').querySelector('[aria-label^="Usage for "]').click(); await flush();
  assert.equal(content.querySelector('a[href^="javascript:"]'), null);
  assert.match(content.textContent, /Pricing source: javascript:alert\("fixture"\)/);
  assert.equal(content.querySelectorAll('time a').length, 3);
  harness.dom.window.close();
});

test('native Claude family output keeps one root row and refreshes nested usage after child-only growth', async () => {
  const contract = JSON.parse(fs.readFileSync(root + 'tests/fixtures/claude-code/subagents/native-contract.json', 'utf8'));
  const dashboard = structuredClone(contract.dashboard), details = structuredClone(contract.details);
  const rootSession = dashboard.conversations.find(conversation => conversation.id === details.sessionId);
  const harness = await twoAgentHarness({ project_dashboard: () => dashboard, conversation_usage: () => structuredClone(details) });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  assert.equal(harness.element('session-table-body').rows.length, 1);
  assert.equal(harness.element('sidebar-session-list').children.length, 1);
  const row = harness.element('session-table-body').rows[0];
  assert.match(row.textContent, /Claude Code.*Includes 3 subagents/s);
  assert.match(row.cells[4].title, /1,350 total tokens/);
  row.querySelector('[aria-label^="Usage for "]').click(); await flush();
  const content = harness.element('session-usage-details');
  assert.deepEqual([...content.querySelectorAll('.usage-scope-table tbody tr')].map(row => [row.cells[0].textContent, row.cells[5].textContent]), [['Main agent', '135'], ['Subagents', '1,215'], ['Combined', '1,350']]);
  assert.equal(content.querySelectorAll('.usage-agent').length, 3);
  assert.match(content.textContent, /Agent turn 1/);
  assert.doesNotMatch(content.textContent, /Prompt 1/);
  assert.match(harness.element('session-usage-status').textContent, /User prompt count unknown/);
  const nested = dashboard.conversations.find(conversation => conversation.lineage.agentPath === '/nested');
  const grow = usage => ({ ...usage, output: usage.output + 1, total: usage.total + 1 });
  nested.usageRevision += '-growth'; nested.usage = grow(nested.usage); nested.familyUsage = grow(nested.familyUsage);
  rootSession.familyUsage = grow(rootSession.familyUsage); dashboard.usage = grow(dashboard.usage);
  details.descendantsUsage = grow(details.descendantsUsage); details.combinedUsage = grow(details.combinedUsage); details.unassigned.usage = grow(details.unassigned.usage);
  const agent = details.agents.find(agent => agent.sessionId === nested.id);
  agent.ownUsage = grow(agent.ownUsage); agent.turns[0].requests[0].tokens.output++;
  await publishUsage(harness, dashboard.conversations, { usage: dashboard.usage });
  assert.equal(harness.calls.filter(call => call.command === 'conversation_usage').length, 2);
  assert.match(content.querySelector('.usage-scope-table').textContent, /1,351/);
  assert.equal(harness.element('session-table-body').rows.length, 1);
  assert.equal(harness.element('sidebar-session-list').children.length, 1);
  harness.dom.window.close();
});

test('late Claude account and preparation results keep the selected Codex environment', async () => {
  const preparation = deferred(), account = deferred();
  let deferPreparation = true;
  const harness = await twoAgentHarness({
    prepare_project: args => args.environmentId === claudeProject.environmentId && deferPreparation ? preparation.promise : ({ project, plan, fingerprint: 'codex-plan', message: null, candidates: [] }),
    agent_account_status: () => account.promise,
  });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  chooseAgent(harness, 'claude-code'); await flush();
  assert.equal(harness.element('agent-choice').disabled, false);
  const liveCodex = harness.element('sidebar-session-list').querySelector('button');
  assert.equal(liveCodex.disabled, false);
  liveCodex.click(); await flush();
  assert.equal(harness.element('start-codex').disabled, false);
  deferPreparation = false;
  preparation.resolve({ project: claudeProject, plan: environmentPlan(claudeProject), fingerprint: 'claude-plan', message: null, candidates: [] });
  account.resolve({ projectId: project.id, environmentId: claudeProject.environmentId, state: 'signed-out', message: 'Late Claude sign-out.', terminalIdentifier: null });
  await flush();
  assert.equal(harness.element('agent-choice').value, 'codex');
  assert.equal(harness.element('project-reference').textContent, project.profile + '@' + project.label);
  assert.equal(harness.element('start-codex').disabled, false);
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').at(-1).args.environmentId, project.environmentId);
  harness.dom.window.close();
});

test('a new project can select Claude beside New Chat', async () => {
  const harness = await twoAgentHarness();
  await harness.ready; harness.element('choose-folder').click(); await flush();
  const accountSyncCount = harness.calls.filter(call => call.command === 'sync_project_chatgpt').length;
  chooseAgent(harness, 'claude-code'); await flush();
  assert.equal(harness.element('agent-choice').value, 'claude-code');
  assert.equal(harness.calls.filter(call => call.command === 'sync_project_chatgpt').length, accountSyncCount);
  assert.equal(harness.calls.filter(call => call.command === 'prepare_project').at(-1).args.environmentId, claudeProject.environmentId);
  harness.element('start-codex').click(); await flush();
  const launch = harness.calls.find(call => call.command === 'start_agent_session');
  assert.equal(launch.args.environmentId, claudeProject.environmentId);
  assert.equal(launch.args.action, 'new-chat');
  assert.equal(launch.args.useBrigBoundary, true);
  harness.dom.window.close();
});

test('an unavailable shared Claude account blocks its chats and preserves the Codex account', async () => {
  const harness = await twoAgentHarness({ shared_claude_status: () => { throw Error('Claude account is unavailable.'); } });
  await harness.ready; harness.element('choose-folder').click(); await flush(); chooseAgent(harness, 'claude-code'); await flush();
  assert.equal(harness.element('start-codex').disabled, true);
  assert.match(harness.element('claude-account-status').textContent, /unavailable/);
  chooseAgent(harness, 'codex'); await flush();
  assert.equal(harness.element('start-codex').disabled, false);
  harness.dom.window.close();
});

test('selected and all-environment removal keep distinct preview and confirmation scopes', async () => {
  for (const all of [false, true]) {
    const previews = [], removals = [];
    const reference = all ? project.name : claudeProject.profile + '@' + claudeProject.label;
    const harness = await twoAgentHarness({
      preview_removal: args => { previews.push(args); return { reference, workspace: '/fixture/state', description: all ? 'Codex and Claude resources' : 'Claude resources', fingerprint: 'removal-proof', external: false, logout: false }; },
      remove_project: args => { removals.push(args); return 'Removed selected environments.'; },
    });
    await harness.ready; harness.element('choose-folder').click(); await flush(); chooseAgent(harness, 'claude-code'); await flush();
    harness.element('project-settings-button').click(); await flush();
    harness.element(all ? 'preview-project-removal' : 'preview-removal').click(); await flush();
    assert.equal(previews.length, 1);
    assert.equal(previews[0].environmentId, all ? undefined : claudeProject.environmentId);
    harness.element('removal-confirmation').value = reference;
    harness.element('removal-confirmation').dispatchEvent(new harness.dom.window.Event('input'));
    harness.element('confirm-removal').click(); await flush();
    assert.equal(removals.length, 1);
    assert.equal(removals[0].environmentId, all ? undefined : claudeProject.environmentId);
    assert.equal(removals[0].confirmation, reference);
    harness.dom.window.close();
  }
});


test('a background shared Claude update preserves the current Codex account feedback and focus', async () => {
  const harness = await twoAgentHarness({ shared_claude_status: () => ({ ...sharedClaudeAccount, state: 'signed-out', email: null }) });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  chooseAgent(harness, 'claude-code'); await flush();
  assert.equal(harness.element('start-codex').disabled, true);
  chooseAgent(harness, 'codex'); await flush();
  const focused = harness.dom.window.document.activeElement;
  await harness.listeners.get('shared-claude-changed')({ payload: { ...sharedClaudeAccount, generation: 2, message: 'Late Claude update.' } });
  assert.equal(harness.element('agent-choice').value, 'codex');
  assert.equal(harness.element('start-codex').disabled, false);
  assert.doesNotMatch(harness.element('project-account-status').textContent, /Late Claude/);
  assert.equal(harness.dom.window.document.activeElement, focused);
  harness.dom.window.close();
});

test('shared Claude checks retain Sign out and recover keyboard focus after status changes', async () => {
  const checking = deferred();
  const harness = await twoAgentHarness({ retry_shared_claude: () => checking.promise });
  await harness.ready; harness.element('settings-navigation').click(); await flush();
  harness.element('settings-tab-providers').click(); await flush(); harness.element('claude-sign-out').focus();
  harness.element('claude-check-account').click(); await flush();
  assert.equal(harness.element('claude-sign-out').hidden, false);
  assert.equal(harness.element('claude-sign-out').disabled, true);
  assert.equal(harness.element('claude-account-status').hidden, true);
  assert.match(harness.element('workspace-status-text').textContent, /Checking the shared Claude account/);
  checking.reject(Error('The check failed.')); await flush();
  assert.equal(harness.element('workspace-status-bar').hidden, true);
  assert.equal(harness.element('claude-account-status').hidden, false);
  assert.equal(harness.element('claude-sign-out').hidden, false);
  assert.equal(harness.element('claude-sign-out').disabled, false);
  assert.equal(harness.dom.window.document.activeElement.id, 'claude-sign-out');
  assert.match(harness.element('claude-account-status').textContent, /check failed/);
  harness.handlers.shared_claude_status = () => ({ ...sharedClaudeAccount, generation: 2, state: 'signed-out', email: null });
  harness.element('claude-check-account').click(); await flush();
  assert.equal(harness.element('claude-sign-out').hidden, true);
  assert.equal(harness.dom.window.document.activeElement.id, 'claude-sign-in');
  harness.dom.window.close();
});

test('a delayed shared Claude sign-out cannot replace a newer account or browser sign-in', async () => {
  const logout = deferred();
  const harness = await twoAgentHarness({ disconnect_shared_claude: () => logout.promise });
  await harness.ready; harness.element('choose-folder').click(); await flush(); chooseAgent(harness, 'claude-code'); await flush();
  harness.element('settings-navigation').click(); await flush(); harness.element('claude-sign-out').click(); await flush();
  await harness.listeners.get('shared-claude-changed')({ payload: { ...sharedClaudeAccount, generation: 8, email: 'new@example.invalid' } });
  await harness.listeners.get('provider-login-changed')({ payload: providerLogin('claude', 'waiting', 9) });
  logout.resolve({ ...sharedClaudeAccount, generation: 2, state: 'signed-out', email: null }); await flush();
  assert.equal(harness.element('claude-sign-in').textContent, 'Return to browser');
  assert.equal(harness.element('claude-account-email').textContent, 'new@example.invalid');
  assert.equal(harness.element('start-codex').disabled, true);
  harness.dom.window.close();
});

test('returning to an environment with pending preparation applies that one result to its new view', async () => {
  const pending = deferred();
  const harness = await twoAgentHarness({ prepare_project: args => args.environmentId === claudeProject.environmentId ? pending.promise : ({ project, plan, fingerprint: 'codex-plan', message: null, candidates: [] }) });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  chooseAgent(harness, 'claude-code'); await flush();
  chooseAgent(harness, 'codex'); await flush();
  chooseAgent(harness, 'claude-code'); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'prepare_project' && call.args.environmentId === claudeProject.environmentId).length, 1);
  pending.resolve({ project: claudeProject, plan: environmentPlan(claudeProject), fingerprint: 'claude-plan', message: null, candidates: [] });
  await flush();
  assert.equal(harness.element('agent-choice').value, 'claude-code');
  assert.equal(harness.element('start-codex').disabled, false);
  harness.element('start-codex').click(); await flush();
  const launch = harness.calls.filter(call => call.command === 'start_agent_session').at(-1);
  assert.equal(launch.args.environmentId, claudeProject.environmentId);
  assert.equal(launch.args.fingerprint, 'claude-plan');
  harness.dom.window.close();
});


test('recheck a saved width arriving during a drag cannot replace the new width', async () => {
  const restored = deferred();
  const harness = await interfaceHarness({ load_interface_preferences: () => restored.promise });
  try {
    const handle = harness.element('sidebar-resizer');
    const window = harness.dom.window;
    handle.setPointerCapture = () => {};
    handle.dispatchEvent(new window.PointerEvent('pointerdown', { button:0, pointerId:1 }));
    handle.dispatchEvent(new window.PointerEvent('pointermove', { pointerId:1, clientX:417 }));
    assert.equal(handle.getAttribute('aria-valuenow'), '417');
    restored.resolve({sidebarWidth:300}); await flush();
    assert.equal(handle.getAttribute('aria-valuenow'), '417');
  } finally { harness.dom.window.close(); }
});

test('recheck a cancelled removal confirmation cannot issue another command', async () => {
  const harness = await interfaceHarness({
    preview_removal: () => ({reference:'codex@sample-a',workspace:'/fixture',description:'Review fixture',fingerprint:'fixture-preview',external:false}),
    remove_project: () => 'Fixture only',
  });
  try {
    await harness.ready;
    harness.element('choose-folder').click(); await flush();
    harness.element('preview-removal').click(); await flush();
    const field = harness.element('removal-confirmation');
    field.value = 'codex@sample-a'; field.dispatchEvent(new harness.dom.window.Event('input'));
    harness.element('cancel-removal').click();
    assert.equal(harness.element('removal-dialog').open, false);
    harness.element('confirm-removal').click(); await flush();
    assert.equal(harness.calls.filter(call => call.command === 'remove_project').length, 0);
  } finally { harness.dom.window.close(); }
});

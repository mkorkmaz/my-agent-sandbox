import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { root, flush, deferred, project, plan, interfaceHarness } from './frontend-harness.mjs';

test('Help opens the complete bundled user guide above Settings and supports view navigation', async () => {
  const harness = await interfaceHarness();
  await harness.ready;
  const document = harness.dom.window.document;
  const help = harness.element('help-view');
  const footer = document.querySelector('.sidebar-footer');
  assert.deepEqual([...footer.querySelectorAll('.navigation-item')].map(button => button.id), ['help-navigation', 'settings-navigation']);
  assert.equal(help.hidden, true);
  harness.element('help-navigation').click(); await flush();
  assert.equal(help.hidden, false);
  assert.equal(harness.element('project-view').hidden, true);
  assert.equal(harness.element('settings-view').hidden, true);
  assert.equal(harness.element('help-navigation').getAttribute('aria-current'), 'page');
  assert.equal(harness.element('project-navigation').hasAttribute('aria-current'), false);
  assert.equal(document.activeElement, harness.element('help-heading'));
  const guide = new harness.dom.window.DOMParser().parseFromString(fs.readFileSync(root + 'docs/user-guide.html', 'utf8'), 'text/html');
  assert.equal(help.textContent, guide.querySelector('main').textContent);
  const links = [...help.querySelectorAll('a')];
  assert.ok(links.length > 0);
  for (const link of links) {
    assert.ok(link.getAttribute('href').startsWith('#'));
    assert.ok(help.contains(document.getElementById(link.hash.slice(1))), 'Missing guide section: ' + link.hash);
  }
  const identifiers = [...document.querySelectorAll('[id]')].map(element => element.id);
  assert.equal(new Set(identifiers).size, identifiers.length);
  harness.element('settings-navigation').click(); await flush();
  assert.equal(help.hidden, true);
  assert.equal(harness.element('settings-view').hidden, false);
  assert.equal(harness.element('help-navigation').hasAttribute('aria-current'), false);
  assert.equal(document.activeElement, harness.element('settings-heading'));
  harness.element('project-navigation').click(); await flush();
  assert.equal(help.hidden, true);
  assert.equal(harness.element('project-view').hidden, false);
  assert.equal(harness.element('project-navigation').getAttribute('aria-current'), 'page');
  harness.dom.window.close();
});

test('a late project check keeps Help visible and retains the selected project', async () => {
  const checking = deferred();
  const harness = await interfaceHarness({ preview_project: () => checking.promise });
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('help-navigation').click(); await flush();
  assert.equal(harness.element('workspace-status-bar').hidden, true);
  checking.resolve({ project, plan, fingerprint: 'plan-a' }); await flush();
  assert.equal(harness.element('help-view').hidden, false);
  assert.equal(harness.element('help-navigation').getAttribute('aria-current'), 'page');
  assert.equal(harness.dom.window.document.activeElement, harness.element('help-heading'));
  harness.element('sidebar-current-project').click(); await flush();
  assert.equal(harness.element('help-view').hidden, true);
  assert.equal(harness.element('project-view').hidden, false);
  assert.equal(harness.element('project-heading').textContent, project.name);
  assert.equal(harness.calls.filter(call => call.command === 'register_project').length, 1);
  harness.dom.window.close();
});

test('Help hides the native terminal surface and returns to the same running chat', async () => {
  const harness = await interfaceHarness();
  await harness.ready; harness.element('choose-folder').click(); await flush();
  harness.element('start-codex').click(); await flush();
  const identifier = harness.calls.find(call => call.command === 'terminal_focus').args.identifier;
  harness.element('help-navigation').click(); await flush();
  assert.equal(harness.element('help-view').hidden, false);
  assert.equal(harness.terminals.get(identifier).running, true);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_layout').at(-1).args.layout.visible, false);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_close').length, 0);
  harness.element('sidebar-session-list').querySelector('.sidebar-session').click(); await flush();
  assert.equal(harness.element('help-view').hidden, true);
  assert.equal(harness.element('terminal-panel').hidden, false);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_layout').at(-1).args.layout.visible, true);
  assert.equal(harness.calls.filter(call => call.command === 'terminal_focus').at(-1).args.identifier, identifier);
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').length, 1);
  harness.dom.window.close();
});

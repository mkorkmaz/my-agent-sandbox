import assert from 'node:assert/strict';
import test from 'node:test';
import { deferred, flush, interfaceHarness, project } from './frontend-harness.mjs';

const secondProject = { ...project, id: 'project-b', environmentId: 'b'.repeat(32), label: 'sample-b', path: '/fixture/second', name: 'Second project' };
const firstText = '2026-11-19T21:54:17+03:00 info network-gateway: published 127.0.0.1:12345=192.168.64.2:23331';
const secondText = '2026-11-19T21:54:17+03:00 info network-gateway: published 127.0.0.1:23456=192.168.64.2:23331';

async function openSettings(harness, selected) {
  harness.element('sidebar-project-list').querySelector(`[data-project-id="${selected.id}"]`).click();
  await flush();
  harness.element('project-settings-button').click();
  await flush();
  assert.equal(harness.element('project-settings-dialog').hidden, false);
}

test('reads two registered gateway refs and rejects results after a project switch', async t => {
  const first = deferred();
  const registered = { [project.id]: project, [secondProject.id]: secondProject };
  const harness = await interfaceHarness({
    recent_projects: () => [project, secondProject],
    open_project: ({ identifier }) => registered[identifier],
    read_project_preferences: ({ identifier }) => ({ project: registered[identifier], options: { cpus: 4, memoryMb: 4096, verification: 'require', network: 'isolated' } }),
    read_project_gateway_logs: ({ identifier }) => identifier === project.id ? first.promise : { status: 'available', text: secondText },
    copy_project_gateway_logs: () => null,
  });
  t.after(() => harness.dom.window.close());
  await harness.ready;
  const read = harness.element('read-project-gateway-logs');
  const view = harness.element('project-gateway-log');
  const copy = harness.element('copy-project-gateway-logs');
  await openSettings(harness, project);
  assert.equal(view.hidden, true);
  assert.equal(copy.hidden, true);
  read.click();
  assert.equal(read.disabled, true);
  assert.match(harness.element('project-gateway-log-status').textContent, /Reading/);
  harness.element('close-project-settings').click();
  await openSettings(harness, secondProject);
  assert.equal(read.disabled, false);
  read.click();
  await flush();
  assert.equal(view.textContent, secondText);
  first.resolve({ status: 'available', text: firstText });
  await flush();
  assert.equal(view.textContent, secondText);
  assert.equal(view.hidden, false);
  const reads = harness.calls.filter(call => call.command === 'read_project_gateway_logs');
  assert.deepEqual(reads.map(({ args }) => ({ ...args })), [project, secondProject].map(item => ({
    identifier: item.id, environmentId: item.environmentId, reference: `${item.profile}@${item.label}`,
  })));
  copy.click();
  await flush();
  assert.equal(harness.calls.find(call => call.command === 'copy_project_gateway_logs').args.text, secondText);
  assert.match(harness.element('project-gateway-log-status').textContent, /copied/);
});

test('only displays and copies plain text, and resets on every read and reopened settings view', async t => {
  const stale = deferred();
  const literal = '<img src=x onerror="invalid()">';
  const results = [
    { status: 'available', text: literal },
    { status: 'empty' }, { status: 'unavailable' }, { status: 'offline' }, { status: 'timeout' }, { status: 'output-limit' },
    new Error('private Brig stderr must stay hidden'),
    stale.promise,
    { status: 'available', text: firstText },
  ];
  const harness = await interfaceHarness({
    recent_projects: () => [project],
    read_project_gateway_logs: () => {
      const result = results.shift();
      if (result instanceof Error) throw result;
      return result;
    },
    copy_project_gateway_logs: () => null,
  });
  t.after(() => harness.dom.window.close());
  await harness.ready;
  await openSettings(harness, project);
  const read = harness.element('read-project-gateway-logs');
  const view = harness.element('project-gateway-log');
  const copy = harness.element('copy-project-gateway-logs');
  const status = harness.element('project-gateway-log-status');
  read.click();
  await flush();
  assert.equal(view.textContent, literal);
  assert.equal(view.children.length, 0);
  for (const expected of [/is empty/, /unavailable/, /Brig reports Offline/, /timed out/, /size limit/, /Brig could not read/]) {
    read.click();
    assert.equal(view.hidden, true);
    assert.equal(copy.hidden, true);
    assert.equal(view.textContent, '');
    await flush();
    assert.match(status.textContent, expected);
    assert.doesNotMatch(status.textContent, /private Brig stderr/);
  }
  read.click();
  harness.element('close-project-settings').click();
  harness.element('project-settings-button').click();
  await flush();
  read.click();
  await flush();
  stale.resolve({ status: 'available', text: secondText });
  await flush();
  assert.equal(view.textContent, firstText);
});

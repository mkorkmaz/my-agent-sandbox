import assert from 'node:assert/strict';
import test from 'node:test';
import { deferred, flush, interfaceHarness, project } from './frontend-harness.mjs';

const sibling = { ...project, id: 'preview-sibling', environmentId: 'b'.repeat(32), label: 'preview-sibling', path: '/fixture/sibling', name: 'Sibling project' };
const firstPort = { hostPort: 49157, guestPort: 38123 };
const siblingPort = { hostPort: 49158, guestPort: 38124 };
const view = (identifier, status, mapping = null) => ({ projectId: identifier, status, mapping });
const options = { cpus: 4, memoryMb: 4096, verification: 'require', network: 'isolated' };

async function openSettings(harness, selected) {
  harness.element('sidebar-project-list').querySelector(`[data-project-id="${selected.id}"]`).click();
  await flush();
  harness.element('project-settings-button').click();
  await flush();
  assert.equal(harness.element('project-settings-dialog').hidden, false);
  harness.element('project-preview-section').open = true;
  await flush();
}

function setup(t, observe) {
  const registered = { [project.id]: project, [sibling.id]: sibling };
  return interfaceHarness({
    recent_projects: () => [project, sibling],
    open_project: ({ identifier }) => registered[identifier],
    read_project_preferences: ({ identifier }) => ({ project: registered[identifier], options }),
    project_preview: observe,
    open_project_preview: () => undefined,
  }).then(harness => {
    t.after(() => harness.dom.window.close());
    return harness;
  });
}

function browserCalls(harness) {
  return harness.calls.filter(call => call.command === 'open_project_preview').map(({ args }) => ({ ...args, mapping: { ...args.mapping } }));
}

function submit(harness) {
  const event = new harness.dom.window.Event('submit', { bubbles: true, cancelable: true });
  harness.element('publish-project-preview').dispatchEvent(event);
}

test('validates the ports, displays server readiness, and recovers an interrupted close', async t => {
  let observation = view(project.id, 'closed');
  const harness = await setup(t, ({ identifier, action }) => {
    assert.equal(identifier, project.id);
    if (action === 'publish') observation = view(identifier, 'waiting-for-server', firstPort);
    if (action === 'close' && observation.status !== 'closing') {
      observation = view(identifier, 'closing', firstPort);
      throw new Error('Brig could not confirm the preview change.');
    }
    if (action === 'close') observation = view(identifier, 'closed');
    return observation;
  });
  await harness.ready;
  await openSettings(harness, project);
  const host = harness.element('preview-host-port');
  const guest = harness.element('preview-guest-port');
  const status = harness.element('project-preview-status');
  const open = harness.element('open-preview-browser');
  const close = harness.element('close-preview-port');
  const publish = harness.element('publish-preview-port');
  assert.match(status.textContent, /No local preview/);
  host.value = '71344';
  guest.value = String(firstPort.guestPort);
  submit(harness);
  assert.match(status.textContent, /1 to 65535/);
  assert.equal(harness.calls.some(call => call.command === 'project_preview' && call.args.action === 'publish'), false);
  host.value = String(firstPort.hostPort);
  submit(harness);
  await flush();
  assert.match(status.textContent, /127\.0\.0\.1:49157.*guest web server has not responded/);
  assert.equal(publish.disabled, true);
  assert.equal(host.readOnly, true);
  open.click();
  await flush();
  assert.deepEqual(browserCalls(harness), []);
  observation = view(project.id, 'ready', firstPort);
  open.click();
  await flush();
  assert.deepEqual(browserCalls(harness), [{ identifier: project.id, environmentId: project.environmentId, mapping: firstPort }]);
  close.click();
  await flush();
  assert.match(status.textContent, /could not confirm/);
  harness.element('check-preview-port').click();
  await flush();
  assert.match(status.textContent, /Retry closing the preview/);
  assert.equal(open.disabled, true);
  close.click();
  await flush();
  assert.match(status.textContent, /No local preview/);
  assert.equal(publish.disabled, false);
  assert.equal(host.readOnly, false);
});

test('does not open a stale project or a different approval from the one displayed', async t => {
  const late = deferred();
  let firstReads = 0;
  let activeSiblingPort = siblingPort;
  const harness = await setup(t, ({ identifier }) => {
    if (identifier === project.id) {
      firstReads++;
      if (firstReads === 2) return late.promise;
      return view(project.id, 'ready', firstPort);
    }
    return view(sibling.id, 'ready', activeSiblingPort);
  });
  await harness.ready;
  await openSettings(harness, project);
  const open = harness.element('open-preview-browser');
  const status = harness.element('project-preview-status');
  open.click();
  await flush();
  harness.element('close-project-settings').click();
  await openSettings(harness, sibling);
  assert.match(status.textContent, /127\.0\.0\.1:49158/);
  late.resolve(view(project.id, 'ready', firstPort));
  await flush();
  assert.deepEqual(browserCalls(harness), []);
  assert.match(status.textContent, /127\.0\.0\.1:49158/);
  activeSiblingPort = { hostPort: 49159, guestPort: 38125 };
  open.click();
  await flush();
  assert.match(status.textContent, /approved preview has changed/);
  assert.deepEqual(browserCalls(harness), []);
  open.click();
  await flush();
  assert.deepEqual(browserCalls(harness), [{ identifier: sibling.id, environmentId: sibling.environmentId, mapping: activeSiblingPort }]);
});

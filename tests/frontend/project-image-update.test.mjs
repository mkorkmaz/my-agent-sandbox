import assert from 'node:assert/strict';
import test from 'node:test';
import { deferred, flush, interfaceHarness, plan, project } from './frontend-harness.mjs';

const sibling = { ...project, id: 'image-sibling', environmentId: 'b'.repeat(32), label: 'image-sibling', name: 'Sibling project' };
const candidate = 'ghcr.io/brig-sh/claude-code-stock@sha256:f4a80cbac021d6c8cdbad51d3b52f66a2a6e2f0ede2c1144aa73453fae60430a';
const previousVersions = { codex: '0.167.0', claudeCode: '2.1.318' };
const candidateVersions = { codex: '0.167.0', claudeCode: '2.1.326' };
const images = { previous: project.imageRef, candidate, previousVersions, candidateVersions };
const available = { status: 'available', ...images };
const reviewed = { status: 'review', ...images, token: 'review-token', changes: [
  { path: '/etc/test config', kind: 'file' },
  { path: '/usr/local/<svg>', kind: 'link' },
  { path: '/tmp/removed', kind: 'device-or-deletion' },
] };
const options = { cpus: 4, memoryMb: 4096, verification: 'require', network: 'isolated' };
const bind = (owner, response) => ({ projectId: owner.id, environmentId: owner.environmentId, ...response });

async function setup(t, handler, initial = project) {
  const records = new Map([[project.id, initial], [sibling.id, sibling]]);
  const harness = await interfaceHarness({
    recent_projects: () => [...records.values()],
    open_project: ({ identifier }) => records.get(identifier),
    read_project_preferences: ({ identifier }) => ({ project: records.get(identifier), options }),
    preview_project: ({ identifier }) => {
      const owner = records.get(identifier);
      return { project: owner, plan, fingerprint: owner.imageRef === candidate ? 'candidate-plan' : 'plan-a' };
    },
    project_image_update: ({ identifier, environmentId, action }) => {
      const owner = records.get(identifier);
      assert.equal(environmentId, owner.environmentId);
      return handler(owner, action, updated => records.set(identifier, updated));
    },
  });
  t.after(() => harness.dom.window.close());
  await harness.ready;
  return harness;
}

async function open(harness, owner) {
  harness.element('sidebar-project-list').querySelector(`[data-project-id="${owner.id}"]`).click();
  await flush();
  await openPanel(harness);
}

async function openPanel(harness) {
  harness.element('project-settings-button').click();
  await flush();
  assert.equal(harness.element('project-settings-dialog').open, true);
  harness.element('project-image-update').open = true;
  await flush();
}

const calls = harness => harness.calls.filter(call => call.command === 'project_image_update');
const status = harness => harness.element('project-image-update-status').textContent;

test('reviews exact guest paths and verifies the selected candidate before launching another chat', async t => {
  const harness = await setup(t, (owner, action, save) => {
    if (action.kind === 'read') return bind(owner, available);
    if (action.kind === 'review') {
      save({ ...owner, stage: 'updating-image' });
      return bind(owner, reviewed);
    }
    assert.deepEqual({ ...action }, { kind: 'apply', token: reviewed.token });
    save({ ...owner, stage: 'environment-ready', imageRef: candidate, approvedPlan: 'candidate-plan' });
    return bind(owner, { status: 'updated', ...images, versions: candidateVersions });
  });
  await open(harness, project);
  assert.equal(calls(harness).length, 1);
  assert.equal(calls(harness)[0].args.action.kind, 'read');
  const imageRows = harness.element('project-image-update-images');
  assert.ok(imageRows.textContent.includes(project.imageRef) && imageRows.textContent.includes(candidate));
  assert.match(imageRows.textContent, /Codex 0\.167\.0 · Claude Code 2\.1\.318.*Codex 0\.167\.0 · Claude Code 2\.1\.326/s);
  assert.equal(harness.element('apply-project-image').disabled, true);
  harness.element('review-project-image').click(); await flush();
  assert.match(status(harness), /guest is stopped/);
  assert.equal(harness.element('start-codex').disabled, true);
  assert.match(harness.element('project-plan-status').textContent, /Continue the guest image update/);
  const paths = harness.element('project-image-update-files');
  assert.deepEqual([...paths.children].map(item => item.textContent), [
    'File · /etc/test config', 'Link · /usr/local/<svg>', 'Device or deleted entry · /tmp/removed',
  ]);
  assert.equal(paths.querySelector('svg'), null);
  assert.match(harness.element('project-image-update-impact').textContent, /same project folder, persistent home, network policy, and approved preview/);
  harness.element('apply-project-image').click(); await flush();
  assert.match(status(harness), /updated image is running.*both agent versions/);
  assert.equal(harness.element('project-image-update-impact').hidden, true);
  assert.equal(harness.element('apply-project-image').hidden, true);
  assert.ok(imageRows.textContent.includes(candidate));
  harness.element('close-project-settings').click(); await flush();
  harness.element('start-codex').click(); await flush();
  assert.equal(harness.calls.filter(call => call.command === 'start_agent_session').at(-1)?.args.fingerprint, 'candidate-plan');
  assert.deepEqual(calls(harness).map(call => call.args.action.kind), ['read', 'review', 'apply']);
});

test('opens a durable review after restart and Keep current restores the reviewed system disk', async t => {
  const harness = await setup(t, (owner, action, save) => {
    if (action.kind === 'read') return bind(owner, { ...reviewed, changes: [] });
    assert.deepEqual({ ...action }, { kind: 'keep-current', token: reviewed.token });
    save({ ...owner, stage: 'environment-ready' });
    return bind(owner, { status: 'restored', image: project.imageRef, versions: previousVersions, message: 'Kept the previous guest image and its system files.' });
  }, { ...project, stage: 'updating-image' });
  await open(harness, project);
  assert.equal(harness.element('project-image-update-empty').hidden, false);
  assert.equal(harness.element('project-image-update-files').hidden, true);
  assert.equal(harness.element('start-codex').disabled, true);
  assert.equal(harness.calls.filter(call => ['prepare_project', 'prepare_environment'].includes(call.command)).length, 0);
  harness.element('keep-project-image').click(); await flush();
  assert.match(status(harness), /previous image and system disk are running/);
  assert.equal(harness.element('keep-project-image').hidden, true);
  assert.equal(harness.element('start-codex').disabled, false);
  assert.ok(harness.element('project-image-update-images').textContent.includes(project.imageRef));
  assert.equal(calls(harness).length, 2);
});

test('active project terminals prevent review while a sibling terminal leaves the project eligible', async t => {
  const harness = await setup(t, (owner, action) => {
    if (action.kind === 'read') return bind(owner, available);
    throw new Error('Close the project’s active chats and terminals before updating the guest image.');
  });
  const terminal = { identifier: 41, projectId: project.id, purpose: 'local', running: true, exitCode: null, conversationId: null, busy: false };
  harness.setTerminal(terminal);
  harness.setTerminal({ ...terminal, identifier: 42, projectId: sibling.id });
  await open(harness, project);
  const review = harness.element('review-project-image');
  assert.equal(review.disabled, true);
  review.click(); await flush();
  assert.equal(calls(harness).length, 1);
  harness.setTerminal({ ...terminal, running: false, exitCode: 0 });
  await harness.listeners.get('terminal-exit')({ payload: { identifier: 41, projectId: project.id, purpose: 'local', code: 0 } });
  assert.equal(review.disabled, false);
  review.click(); await flush();
  assert.match(status(harness), /active chats and terminals/);
  assert.equal(harness.element('apply-project-image').disabled, true);
  assert.equal(harness.terminals.get(42).running, true);
  assert.equal(calls(harness).length, 2);
});

test('rechecks an interrupted update and offers only the native recovery action', async t => {
  let recovery = false;
  const harness = await setup(t, (owner, action, save) => {
    if (action.kind === 'read') return bind(owner, recovery ? { status: 'recovery', ...images, message: 'The image update stopped. Recover the previous guest state.' } : reviewed);
    if (action.kind === 'apply') {
      recovery = true;
      throw new Error('Candidate executable verification failed.');
    }
    assert.equal(action.kind, 'recover');
    save({ ...owner, stage: 'environment-ready' });
    return bind(owner, { status: 'restored', image: project.imageRef, versions: previousVersions, message: 'Recovered the interrupted guest image change.' });
  }, { ...project, stage: 'updating-image' });
  await open(harness, project);
  harness.element('apply-project-image').click(); await flush();
  assert.match(status(harness), /executable verification failed/);
  assert.equal(harness.element('apply-project-image').disabled, true);
  assert.equal(harness.element('start-codex').disabled, true);
  harness.element('check-project-image').click(); await flush();
  assert.match(status(harness), /Recover the previous guest state/);
  assert.equal(harness.element('recover-project-image').disabled, false);
  assert.equal(harness.element('keep-project-image').disabled, true);
  harness.element('recover-project-image').click(); await flush();
  assert.match(status(harness), /Recovered the interrupted guest image change/);
  assert.match(harness.element('project-image-update-images').textContent, /Claude Code 2\.1\.318/);
  assert.deepEqual(calls(harness).map(call => call.args.action.kind), ['read', 'apply', 'read', 'recover']);
});

test('a candidate verification failure leaves the current plan available and requires a new review', async t => {
  const harness = await setup(t, (owner, action) => {
    if (action.kind === 'read') return bind(owner, available);
    throw new Error('Candidate signature verification failed.');
  });
  await open(harness, project);
  harness.element('review-project-image').click(); await flush();
  assert.match(status(harness), /signature verification failed/);
  assert.equal(harness.element('apply-project-image').disabled, true);
  assert.equal(harness.element('start-codex').disabled, false);
  harness.element('check-project-image').click(); await flush();
  assert.equal(harness.element('review-project-image').disabled, false);
});

test('a late review cannot change the sibling project or enable an apply action there', async t => {
  const late = deferred();
  const harness = await setup(t, (owner, action) => {
    if (owner.id === sibling.id) return bind(owner, { status: 'current', image: candidate, versions: candidateVersions });
    return action.kind === 'read' ? bind(owner, available) : late.promise;
  });
  await open(harness, project);
  harness.element('review-project-image').click(); await flush();
  harness.element('close-project-settings').click(); await flush();
  harness.element('project-navigation').click(); await flush();
  await open(harness, sibling);
  assert.match(status(harness), /Already up to date/);
  late.resolve(bind(project, reviewed)); await flush();
  assert.match(status(harness), /Already up to date/);
  assert.equal(harness.element('apply-project-image').disabled, true);
  assert.equal(harness.element('project-image-update-impact').hidden, true);
  assert.equal(calls(harness).at(-1).args.identifier, sibling.id);
  assert.equal(harness.calls.filter(call => call.command === 'read_project_preferences').at(-1).args.identifier, sibling.id);
});

test('mismatched environment responses cannot supply a review token', async t => {
  const harness = await setup(t, owner => bind(owner, { ...reviewed, environmentId: sibling.environmentId }));
  await open(harness, project);
  assert.match(status(harness), /does not match the requested project/);
  assert.equal(harness.element('apply-project-image').disabled, true);
  assert.equal(harness.element('project-image-update-images').hidden, true);
  assert.equal(harness.element('project-image-update-impact').hidden, true);
});

test('an unsupported recovery runtime leaves only the status check available', async t => {
  const harness = await setup(t, (_owner, action) => {
    assert.equal(action.kind, 'read');
    throw 'Guest image updates require the bundled Hull recovery build.';
  });
  await open(harness, project);
  assert.match(status(harness), /require the bundled Hull recovery build/);
  assert.equal(harness.element('check-project-image').disabled, false);
  for (const id of ['review-project-image', 'apply-project-image', 'keep-project-image', 'recover-project-image']) {
    assert.equal(harness.element(id).disabled, true);
  }
  assert.equal(harness.element('start-codex').disabled, false);
});

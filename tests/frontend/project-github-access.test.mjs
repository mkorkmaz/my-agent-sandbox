import assert from 'node:assert/strict';
import test from 'node:test';
import { deferred, flush, interfaceHarness, project } from './frontend-harness.mjs';

const synthetic = 'github_pat_synthetic_fixture_12345678';
const sibling = { ...project, id: 'github-sibling', environmentId: 'a'.repeat(32), label: 'github-sibling', path: '/fixture/sibling' };
const view = (identifier, status, repository = null, running = false) => ({ projectId: identifier, status, repository, running });

async function openSettings(harness, selected) {
  harness.element('sidebar-project-list').querySelector(`[data-project-id="${selected.id}"]`).click();
  await flush();
  harness.element('project-settings-button').click();
  await flush();
  assert.equal(harness.element('project-settings-dialog').hidden, false);
  harness.element('project-github-access').open = true;
  await flush();
}

async function setup(t, commands) {
  const registered = { [project.id]: project, [sibling.id]: sibling };
  const harness = await interfaceHarness({
    recent_projects: () => [project, sibling],
    open_project: ({ identifier }) => registered[identifier],
    read_project_preferences: ({ identifier }) => ({ project: registered[identifier], options: { cpus: 4, memoryMb: 4096, verification: 'require', network: 'isolated' } }),
    ...commands,
  });
  t.after(() => harness.dom.window.close());
  await harness.ready;
  return harness;
}

function enter(harness) {
  const password = harness.element('project-github-token');
  harness.element('project-github-repository').value = 'demo/private';
  password.value = synthetic;
  harness.element('save-project-github').click();
  assert.equal(password.value, '');
}

test('masks and clears the token, reports Keychain recovery, and revokes a running grant', async t => {
  let status = 'none';
  const harness = await setup(t, {
    project_github_access: ({ identifier, action, repository, token }) => {
      assert.equal(identifier, project.id);
      if (action === 'save') assert.equal(token, synthetic);
      if (action === 'save' && status === 'none') {
        status = 'saving';
        throw 'Brig could not access the Mac Keychain. Allow its prompt, then retry.';
      }
      if (action === 'save') {
        assert.equal(repository, 'demo/private');
        status = 'saved';
      }
      if (action === 'revoke') status = 'none';
      return view(identifier, status, status === 'none' ? null : 'demo/private', status === 'saved');
    },
    stop_project: () => { throw new Error('Revoke must not stop the environment.'); },
  });
  await openSettings(harness, project);
  assert.equal(harness.element('project-github-token').type, 'password');
  enter(harness);
  await flush();
  assert.match(harness.element('project-github-status').textContent, /Mac Keychain.*retry/);
  harness.element('check-project-github').click();
  await flush();
  assert.match(harness.element('project-github-status').textContent, /Saving GitHub access is incomplete/);
  enter(harness);
  await flush();
  assert.match(harness.element('project-github-status').textContent, /Fetch-only.*permission is unverified/);
  assert.equal(harness.element('project-github-repository').readOnly, true);
  harness.element('revoke-project-github').click();
  await flush();
  assert.match(harness.element('project-github-status').textContent, /No GitHub fetch access/);
  assert.equal(harness.calls.some(call => call.command === 'stop_project'), false);
  assert.equal(harness.dom.window.document.body.textContent.includes(synthetic), false);
});

test('drops a saved token handoff when the selected project changes during stop', async t => {
  const stop = deferred();
  const harness = await setup(t, {
    project_github_access: ({ identifier }) => view(identifier, 'none', null, identifier === project.id),
    stop_project: () => stop.promise,
  });
  await openSettings(harness, project);
  assert.match(harness.element('save-project-github').textContent, /Stop environment and save/);
  enter(harness);
  await flush();
  harness.element('close-project-settings').click();
  await openSettings(harness, sibling);
  stop.resolve(project);
  await flush();
  assert.equal(harness.calls.some(call => call.command === 'project_github_access' && call.args.action === 'save'), false);
  assert.equal(harness.element('project-github-token').value, '');
});

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const directory = process.env.BRIG_FIXTURES ?? fileURLToPath(new URL('../fixtures/brig/v0.5.2/', import.meta.url));
const read = name => JSON.parse(fs.readFileSync(path.join(directory, name + '.json'), 'utf8'));
const hosts = ['api.anthropic.com', 'api.openai.com', 'auth.openai.com', 'chatgpt.com', 'claude.ai', 'platform.claude.com'];
const image = 'ghcr.io/brig-sh/claude-code-stock@sha256:f5b733fbce5aaf38edcdcfe7021993e65625c0f064334a96089e9f1fb5e8d0ce';

function inspect(result) {
  if (result.errorCode === 'ENOENT') return 'missing-brig';
  assert.equal(result.errorCode, null);
  assert.equal(result.exitCode, 0);
  const envelope = JSON.parse(result.stdout);
  assert.equal(envelope.apiVersion, 'brig.sh/v1alpha1');
  assert.equal(envelope.kind, 'Plan');
  const plan = envelope.data;
  if (plan.refused?.reason) return 'refused';
  if (plan.runtime.available === false) return 'missing-runtime';
  assert.equal(plan.runtime.available, true);
  assert.equal(plan.runtime.kind, 'hull');
  assert.equal(plan.runtime.backend, 'hvi');
  assert.equal(plan.profile, 'tugboat-project');
  assert.equal(plan.session, 'bd01-fixture');
  assert.deepEqual(plan.project, { host: '/fixture/örnek proje', guest: '/work/örnek proje', mode: 'read-write' });
  assert.equal(plan.posture, 'isolated');
  assert.deepEqual(plan.policies, ['bd01-fixture']);
  assert.equal(plan.noPolicy, false);
  assert.equal(plan.egress.default, 'deny');
  assert.deepEqual(plan.egress.allow.map(rule => rule.host).sort(), hosts);
  assert.deepEqual(plan.credentials, []);
  assert.equal(plan.image.ref, image);
  assert.deepEqual(plan.verify, { mode: 'require', policy: 'brig' });
  assert.deepEqual(plan.home, { host: '/fixture/project-home-bd01-fixture', guest: '/root', mode: 'read-write' });
  return 'ready';
}

test('version envelope records the captured Brig build', () => {
  const result = read('version');
  const envelope = JSON.parse(result.stdout);
  const manifest = read('manifest');
  assert.equal(result.exitCode, 0);
  assert.equal(envelope.kind, 'Version');
  assert.equal(envelope.apiVersion, manifest.apiVersion);
  assert.equal(envelope.data.version, manifest.brigVersion);
  assert.equal(envelope.data.commit, manifest.brigCommit);
  assert.equal(envelope.data.arch, 'arm64');
});

test('shared project plan capture records the exact profile source and separate argument values', () => {
  const manifest = read('manifest');
  const source = fileURLToPath(new URL('../../src-tauri/profiles/tugboat-project.yaml', import.meta.url));
  assert.equal(manifest.profileSource, 'src-tauri/profiles/tugboat-project.yaml');
  assert.equal(manifest.profileSha256, createHash('sha256').update(fs.readFileSync(source)).digest('hex'));
  assert.deepEqual(read('ready').command.slice(1), [
    '--json', 'plan', '--network', 'isolated', '--cpus', '4', '--mem', '4096',
    '--image', image, '--home', '/fixture/project-home', 'tugboat-project@bd01-fixture', '/fixture/örnek proje',
  ]);
  assert.match(manifest.planScope, /No VM or account operation/);
});

test('the shared profile has one neutral executable and keeps Claude access on a memory mount', () => {
  const result = read('profile');
  assert.equal(result.exitCode, 0);
  const profile = JSON.parse(result.stdout);
  assert.equal(profile.name, 'tugboat-project');
  assert.equal(profile.kind, 'agent');
  assert.equal(profile.binary, 'bash');
  assert.deepEqual(profile.secrets ?? [], []);
  assert.deepEqual(profile.files ?? [], []);
  assert.deepEqual(profile.env, [
    { name: 'IS_SANDBOX', value: '1' }, { name: 'CLAUDE_CONFIG_DIR', value: '/brig/claude' },
    { name: 'DISABLE_UPDATES', value: '1' },
  ]);
  assert.ok(profile.volumes.some(volume => volume.kind === 'tmpfs' && volume.at === '/brig/claude'));
  assert.ok(profile.volumes.some(volume => volume.kind === 'hostmount' && volume.path === '.claude/projects'));
  assert.ok(profile.staleCredentialFiles.includes('.claude/.credentials.json'));
});

test('shared plans reject changed identity, image, policy, or host credentials', () => {
  for (const mutate of [
    plan => { plan.home.host = '/fixture/another-project-home'; },
    plan => { plan.project.host = '/fixture/another-project'; },
    plan => { plan.image.ref = 'ghcr.io/brig-sh/claude-code-stock:root'; },
    plan => { plan.profile = 'codex'; },
    plan => { plan.session = 'another-project'; },
    plan => { plan.policies.push('unexpected'); },
    plan => { plan.credentials = [{ name: 'ANTHROPIC_API_KEY', state: 'forwarded' }]; },
    plan => { plan.credentials = [{ name: 'ANTHROPIC_API_KEY', state: 'unresolved' }]; },
    plan => { plan.verify.mode = 'off'; },
  ]) {
    const result = read('ready');
    const envelope = JSON.parse(result.stdout);
    mutate(envelope.data);
    result.stdout = JSON.stringify(envelope);
    assert.throws(() => inspect(result));
  }
});

for (const name of ['ready', 'refused', 'missing-runtime', 'missing-brig']) {
  test('classify the captured ' + name + ' case', () => assert.equal(inspect(read(name)), name));
}

test('exit code zero can represent an unavailable runtime or a refused plan', () => {
  const refused = read('refused');
  const missing = read('missing-runtime');
  assert.equal(refused.exitCode, 0);
  assert.equal(missing.exitCode, 0);
  assert.equal(JSON.parse(refused.stdout).data.runtime.backend, 'vz');
  assert.ok(JSON.parse(refused.stdout).data.refused.reason.trim());
  assert.equal(JSON.parse(missing.stdout).data.refused, undefined);
});

test('additional fields preserve the contract with the same API version', () => {
  const result = read('ready');
  const envelope = JSON.parse(result.stdout);
  envelope.data.futureField = { supported: true };
  result.stdout = JSON.stringify(envelope);
  assert.equal(inspect(result), 'ready');
  envelope.apiVersion = 'brig.sh/v2';
  result.stdout = JSON.stringify(envelope);
  assert.throws(() => inspect(result));
});

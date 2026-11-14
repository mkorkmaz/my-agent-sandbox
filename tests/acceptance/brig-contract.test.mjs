import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const directory = process.env.BRIG_FIXTURES ?? fileURLToPath(new URL('../fixtures/brig/v0.5.2/', import.meta.url));
const read = (name, agentKind = 'codex') => JSON.parse(fs.readFileSync(path.join(directory, agentKind === 'codex' ? '' : agentKind, name + '.json'), 'utf8'));
const agents = {
  codex: { profile: 'codex', session: 'bd01-fixture', hosts: ['api.openai.com', 'auth.openai.com', 'chatgpt.com'] },
  'claude-code': { profile: 'tugboat-claude', session: 'bd01-claude', hosts: ['api.anthropic.com', 'claude.ai', 'platform.claude.com'] },
};

function inspect(result, agentKind = 'codex') {
  const agent = agents[agentKind];
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
  assert.equal(plan.profile, agent.profile);
  assert.equal(plan.session, agent.session);
  assert.equal(plan.project.host, '/fixture/örnek proje');
  assert.equal(plan.project.guest, '/work/örnek proje');
  assert.equal(plan.project.mode, 'read-write');
  assert.equal(plan.posture, 'isolated');
  assert.ok(plan.policies.includes(agent.session));
  assert.equal(plan.noPolicy, false);
  assert.equal(plan.egress.default, 'deny');
  assert.deepEqual(plan.egress.allow.map(rule => rule.host).sort(), agent.hosts);
  assert.ok(Array.isArray(plan.credentials));
  assert.ok(plan.credentials.every(credential => ['unresolved', 'withheld'].includes(credential.state)));
  if (agentKind === 'claude-code') {
    assert.deepEqual(plan.credentials, []);
    assert.equal(plan.image.ref, 'ghcr.io/brig-sh/claude-code-stock:root');
    assert.equal(plan.verify.mode, 'require');
    assert.equal(plan.home.host, '/fixture/guest-claude-bd01-claude');
    assert.equal(plan.home.guest, '/root');
  }
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

for (const name of ['ready', 'refused', 'missing-runtime']) {
  test('classify the captured Claude ' + name + ' case', () => assert.equal(inspect(read(name, 'claude-code'), 'claude-code'), name));
}

test('Claude plan capture pins the managed profile and preserves separate argument values', () => {
  const manifest = read('manifest', 'claude-code');
  assert.equal(manifest.agentKind, 'claude-code');
  assert.equal(manifest.brigVersion, read('manifest').brigVersion);
  assert.equal(manifest.brigCommit, read('manifest').brigCommit);
  const source = fileURLToPath(new URL('../../src-tauri/profiles/tugboat-claude.yaml', import.meta.url));
  assert.equal(manifest.profileSha256, createHash('sha256').update(fs.readFileSync(source)).digest('hex'));
  assert.deepEqual(read('ready', 'claude-code').command.slice(1), [
    '--json', 'plan', '--network', 'isolated', '--home', '/fixture/guest-claude',
    'tugboat-claude@bd01-claude', '/fixture/örnek proje',
  ]);
  assert.match(manifest.planScope, /No VM or account operation/);
});

test('captured Claude profile has no host credential sources and keeps transcript storage separate from credentials', () => {
  const result = read('profile', 'claude-code');
  assert.equal(result.exitCode, 0);
  const profile = JSON.parse(result.stdout);
  assert.equal(profile.name, 'tugboat-claude');
  assert.equal(profile.kind, 'agent');
  assert.equal(profile.binary, 'claude');
  assert.deepEqual(profile.secrets ?? [], []);
  assert.deepEqual(profile.files ?? [], []);
  assert.deepEqual(profile.env, [
    { name: 'IS_SANDBOX', value: '1' }, { name: 'CLAUDE_CONFIG_DIR', value: '/brig/claude' },
  ]);
  assert.ok(profile.volumes.some(volume => volume.kind === 'tmpfs' && volume.at === '/brig/claude'));
  assert.ok(profile.volumes.some(volume => volume.kind === 'hostmount' && volume.path === '.claude/projects'));
  assert.ok(profile.staleCredentialFiles.includes('.claude/.credentials.json'));
});

test('Claude plans reject another agent home, image, profile, or forwarded credential', () => {
  const codexHome = JSON.parse(read('ready').stdout).data.home;
  for (const mutate of [
    plan => { plan.home = codexHome; },
    plan => { plan.image.ref = 'ghcr.io/brig-sh/codex-stock:root'; },
    plan => { plan.profile = 'codex'; },
    plan => { plan.session = 'bd01-fixture'; },
    plan => { plan.credentials = [{ name: 'ANTHROPIC_API_KEY', state: 'forwarded' }]; },
    plan => { plan.credentials = [{ name: 'ANTHROPIC_API_KEY', state: 'unknown' }]; },
    plan => { plan.verify.mode = 'off'; },
  ]) {
    const result = read('ready', 'claude-code');
    const envelope = JSON.parse(result.stdout);
    mutate(envelope.data);
    result.stdout = JSON.stringify(envelope);
    assert.throws(() => inspect(result, 'claude-code'));
  }
});

for (const name of ['ready', 'refused', 'missing-runtime', 'missing-brig']) {
  test('classify the captured ' + name + ' case', () => assert.equal(inspect(read(name)), name));
}

test('exit code zero also occurs when a policy refuses the plan', () => {
  const result = read('refused');
  assert.equal(result.exitCode, 0);
  const plan = JSON.parse(result.stdout).data;
  assert.equal(plan.runtime.backend, 'vz');
  assert.ok(plan.refused.reason.trim());
  assert.equal(inspect(result), 'refused');
});

test('a missing runtime can have exit code zero and no refused field', () => {
  const result = read('missing-runtime');
  assert.equal(result.exitCode, 0);
  assert.equal(JSON.parse(result.stdout).data.refused, undefined);
  assert.equal(inspect(result), 'missing-runtime');
});

test('additional fields in the same API version remain compatible', () => {
  const result = read('ready');
  const envelope = JSON.parse(result.stdout);
  envelope.data.futureField = { supported: true };
  result.stdout = JSON.stringify(envelope);
  assert.equal(inspect(result), 'ready');
});

test('another project or API version fails the contract', () => {
  for (const mutate of [envelope => { envelope.data.project.host = '/fixture/other'; }, envelope => { envelope.apiVersion = 'brig.sh/v2'; }]) {
    const result = read('ready');
    const envelope = JSON.parse(result.stdout);
    mutate(envelope);
    result.stdout = JSON.stringify(envelope);
    assert.throws(() => inspect(result));
  }
});

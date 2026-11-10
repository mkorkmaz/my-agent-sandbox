import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const directory = process.env.BRIG_FIXTURES ?? fileURLToPath(new URL('./fixtures/brig/v0.5.2/', import.meta.url));
const read = name => JSON.parse(fs.readFileSync(path.join(directory, name + '.json'), 'utf8'));

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
  assert.equal(plan.profile, 'codex');
  assert.equal(plan.session, 'bd01-fixture');
  assert.equal(plan.project.host, '/fixture/örnek proje');
  assert.equal(plan.project.guest, '/work/örnek proje');
  assert.equal(plan.project.mode, 'read-write');
  assert.equal(plan.posture, 'isolated');
  assert.ok(plan.policies.includes('bd01-fixture'));
  assert.equal(plan.noPolicy, false);
  assert.equal(plan.egress.default, 'deny');
  assert.deepEqual(plan.egress.allow.map(rule => rule.host).sort(), ['api.openai.com', 'auth.openai.com', 'chatgpt.com']);
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

import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { cleanSharedProject } from '../../scripts/acceptance/shared-project-cleanup.mjs';

const scripts = fileURLToPath(new URL('../../scripts/acceptance', import.meta.url));
const darwinOnly = { skip: process.platform !== 'darwin' };

function fixture(t, script, stage = 'version', overrideTools = false) {
  const temporary = prefix => {
    const dir = fs.mkdtempSync(prefix);
    t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
    return dir;
  };
  const workspace = temporary(path.join(os.tmpdir(), 'tugboat-acceptance-'));
  const withSpaces = script === 'check-space-paths.mjs';
  const root = temporary(withSpaces ? '/private/tmp/brig-space-' : '/private/tmp/brig-bd15-');
  const args = withSpaces ? [temporary('/private/tmp/brig-cache-'), root] : [root];
  if (overrideTools) args.push(path.join(workspace, 'custom tools/brig'), path.join(workspace, 'custom tools/hull'));
  const runtime = path.join(workspace, '.build/brig-runtime/bin/hull');
  fs.mkdirSync(path.dirname(runtime), { recursive: true });
  fs.writeFileSync(runtime, '');
  fs.mkdirSync(path.join(workspace, 'scripts/acceptance'), { recursive: true });
  const entry = path.join(workspace, 'scripts/acceptance', script);
  fs.copyFileSync(path.join(scripts, script), entry);
  fs.copyFileSync(path.join(scripts, 'acceptance-runtime.mjs'), path.join(workspace, 'scripts/acceptance/acceptance-runtime.mjs'));
  const stub = fileURLToPath(new URL('../fixtures/acceptance-commands.cjs', import.meta.url));
  const result = spawnSync(process.execPath, ['--require', stub, entry, ...args], {
    encoding: 'utf8', env: { ...process.env, TEST_BRIG_STAGE: stage }, timeout: 10_000,
  });
  assert.ifError(result.error);
  const evidence = path.join(workspace, 'artifacts', withSpaces ? 'space-paths' : 'bd-15', 'real-brig.json');
  return { status: result.status, report: JSON.parse(fs.readFileSync(evidence, 'utf8')) };
}

test('acceptance scripts record the command failure in a fresh checkout', darwinOnly, t => {
  for (const script of ['check-real-brig.mjs', 'check-space-paths.mjs']) {
    const { report } = fixture(t, script);
    assert.match(report.blocker ?? report.failure, /Brig fixture unavailable/);
    assert.equal(report.commands[0].stderr, 'Brig fixture unavailable');
    assert.ok(report.completedAt);
  }
});

test('real Brig acceptance exit code agrees with the final evidence, including cleanup', darwinOnly, t => {
  for (const [stage, expectedError] of [
    ['success', null], ['version', /Brig fixture unavailable/], ['detach', /detach/],
    ['inventory', /inventory offline/], ['remaining', /Guest remains/],
  ]) {
    const { status, report } = fixture(t, 'check-real-brig.mjs', stage);
    assert.equal(status, expectedError ? 1 : 0, stage);
    assert.equal(report.outcome, expectedError ? 'blocked' : 'passed-with-native-and-account-acceptance-pending', stage);
    if (expectedError) assert.match(report.blocker ?? report.cleanupError, expectedError, stage);
    else assert.deepEqual(report.remaining, []);
  }
});

test('real Brig acceptance defaults to the bundled tools and preserves explicit tool overrides', darwinOnly, t => {
  for (const overrideTools of [false, true]) {
    const { status, report } = fixture(t, 'check-real-brig.mjs', 'success', overrideTools);
    assert.equal(status, 0);
    const commands = report.commands.filter(command => ['brig', 'hull'].includes(path.basename(command.binary)));
    assert.ok(commands.length > 0);
    assert.ok(commands.every(command => command.binary.includes(overrideTools ? '/custom tools/' : '/.build/brig-runtime/bin/')));
  }
});

const sharedReference = 'tugboat-project-proof@proof';

function sharedCleanupFixture(stage) {
  const calls = [];
  let detachAttempts = 0;
  const inventory = async () => {
    if (stage === 'inventory') throw new Error('inventory unavailable');
    if (stage === 'foreign') return [{ ref: 'another-project@review', state: 'running' }];
    if (calls.some(args => args[0] === 'rm' && args.length === 2) && stage !== 'remaining') return [];
    return [{ ref: sharedReference, state: stage === 'stopped' ? 'stopped' : 'running' }];
  };
  const brig = async args => {
    calls.push(args);
    if (stage === args[0]) throw new Error('guest command failed');
    return { stdout: stage === 'preview' ? 'another-project@review' : 'Remove ' + sharedReference };
  };
  const detach = async () => {
    calls.push(['detach']);
    detachAttempts++;
    if (stage === 'busy' && detachAttempts === 1 || stage === 'busy-exhausted') {
      return { status: 1, stderr: 'Resource busy' };
    }
    if (stage === 'detach') return { status: 1, stderr: 'Store unavailable' };
    return { status: 0, stderr: '' };
  };
  return { calls, run: () => cleanSharedProject(sharedReference, inventory, brig, detach) };
}

test('shared project cleanup uses one exact guest and verifies its removal before detaching', async () => {
  const running = sharedCleanupFixture('success');
  assert.equal(await running.run(), 1);
  assert.deepEqual(running.calls, [
    ['stop', sharedReference], ['rm', sharedReference, '--dry-run'], ['rm', sharedReference], ['detach'],
  ]);
  const stopped = sharedCleanupFixture('stopped');
  await stopped.run();
  assert.deepEqual(stopped.calls, running.calls.slice(1));
});

test('shared project cleanup preserves unexpected guests and refuses incomplete removal', async () => {
  for (const stage of ['foreign', 'inventory', 'stop', 'preview', 'rm', 'remaining']) {
    const fixture = sharedCleanupFixture(stage);
    await assert.rejects(fixture.run(), stage);
    assert.ok(!fixture.calls.some(args => args[0] === 'detach'), stage);
    if (stage === 'foreign' || stage === 'inventory') assert.deepEqual(fixture.calls, []);
  }
});

test('shared project cleanup retries only a transient busy detach', async () => {
  const recovered = sharedCleanupFixture('busy');
  assert.equal(await recovered.run(), 2);
  for (const [stage, count] of [['detach', 1], ['busy-exhausted', 3]]) {
    const fixture = sharedCleanupFixture(stage);
    await assert.rejects(fixture.run(), /detach/);
    assert.equal(fixture.calls.filter(args => args[0] === 'detach').length, count);
  }
});

import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const scripts = fileURLToPath(new URL('../scripts', import.meta.url));
const darwinOnly = { skip: process.platform !== 'darwin' };

function fixture(t, script, stage = 'version') {
  const temporary = prefix => {
    const dir = fs.mkdtempSync(prefix);
    t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
    return dir;
  };
  const workspace = temporary(path.join(os.tmpdir(), 'tugboat-acceptance-'));
  const withSpaces = script === 'check-space-paths.mjs';
  const root = temporary(withSpaces ? '/private/tmp/brig-space-' : '/private/tmp/brig-bd15-');
  const args = withSpaces ? [temporary('/private/tmp/brig-cache-'), root] : [root];
  const runtime = path.join(workspace, '.build/hull-compat/bin/hull');
  fs.mkdirSync(path.dirname(runtime), { recursive: true });
  fs.writeFileSync(runtime, '');
  fs.mkdirSync(path.join(workspace, 'scripts'));
  const entry = path.join(workspace, 'scripts', script);
  fs.copyFileSync(path.join(scripts, script), entry);
  const stub = fileURLToPath(new URL('./fixtures/acceptance-commands.cjs', import.meta.url));
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

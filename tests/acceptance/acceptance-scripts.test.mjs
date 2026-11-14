import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

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

function agentFixture(t, stage = 'success') {
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-agents-'));
  const root = fs.mkdtempSync('/private/tmp/tga-');
  t.after(() => { fs.rmSync(workspace, { recursive: true, force: true }); fs.rmSync(root, { recursive: true, force: true }); });
  for (const name of ['scripts/acceptance', 'src-tauri/profiles', '.build/brig-runtime/bin']) fs.mkdirSync(path.join(workspace, name), { recursive: true });
  const entry = path.join(workspace, 'scripts/acceptance/check-agent-environments.mjs');
  fs.copyFileSync(path.join(scripts, 'check-agent-environments.mjs'), entry);
  fs.copyFileSync(path.join(scripts, 'acceptance-process.mjs'), path.join(workspace, 'scripts/acceptance/acceptance-process.mjs'));
  fs.copyFileSync(path.join(scripts, 'acceptance-runtime.mjs'), path.join(workspace, 'scripts/acceptance/acceptance-runtime.mjs'));
  fs.copyFileSync(fileURLToPath(new URL('../../src-tauri/profiles/tugboat-claude.yaml', import.meta.url)), path.join(workspace, 'src-tauri/profiles/tugboat-claude.yaml'));
  fs.writeFileSync(path.join(workspace, '.build/brig-runtime/bin/hull'), '');
  const stub = fileURLToPath(new URL('../fixtures/brig/agent-environments-stub.cjs', import.meta.url));
  const result = spawnSync(process.execPath, ['--require', stub, entry, root], {
    encoding: 'utf8', timeout: 10_000,
    env: { ...process.env, TEST_BRIG_STAGE: stage, ANTHROPIC_API_KEY: 'synthetic-host-value', OPENAI_API_KEY: 'synthetic-host-value' },
  });
  assert.ifError(result.error);
  const report = JSON.parse(fs.readFileSync(path.join(workspace, 'artifacts/claude-code/step-9-cli-environments.json'), 'utf8'));
  return { status: result.status, report };
}

test('dual-agent acceptance uses separate homes and policies with exact guest arguments', darwinOnly, t => {
  const { status, report } = agentFixture(t);
  assert.equal(status, 0);
  assert.equal(report.outcome, 'passed-cli-with-native-and-account-acceptance-pending');
  assert.deepEqual(report.remaining, []);
  assert.equal(report.testRootRemoved, true);
  assert.equal(report.checks.length, 6);
  assert.ok(report.checks.every(check => check.passed));
  const [codex, claude] = report.agents;
  assert.notEqual(codex.home, claude.home);
  assert.notEqual(codex.policy, claude.policy);
  assert.deepEqual(report.commands.filter(command => command.args[0] === 'run').map(command => command.args.slice(-4)), [
    ['tugboat-test-codex@co', report.project, '--', '--version'], ['tugboat-claude@cl', report.project, '--', '--version'],
  ]);
  assert.ok(report.commands.filter(command => command.args[0] === 'run').every(command => command.timeoutMs === 900_000));
  assert.ok(report.agents.every(agent => agent.storeCalls > 0));
  assert.equal(report.hostHomePreserved, true);
  assert.equal(report.isolatedHome, false);
  assert.equal(report.isolatedGuestHomes, true);
  assert.ok(report.runtimeStores.length >= 2);
  assert.ok(report.runtimeStores.every(store => store === report.store));
  assert.deepEqual(report.codexProfile, { name: 'tugboat-test-codex', derivedFrom: 'codex', credentialSourcesRemoved: true });
  assert.equal(report.userInterfaceTest, false);
  assert.equal(report.accountTest, false);
  assert.equal(report.modelRequestTest, false);
});

test('dual-agent acceptance rejects failed boots, forwarded credentials, and another environment home', darwinOnly, t => {
  for (const stage of ['version', 'claude-boot', 'credentials', 'wrong-home', 'wrong-store', 'wrong-runtime']) {
    const { status, report } = agentFixture(t, stage);
    assert.equal(status, 1, stage);
    assert.equal(report.outcome, 'blocked', stage);
    assert.ok(report.blocker, stage);
    assert.deepEqual(report.remaining, [], stage);
    assert.ok(report.completedAt, stage);
    if (stage === 'wrong-runtime') assert.ok(report.commands.every(command => command.args[0] !== 'run'));
  }
});

test('dual-agent acceptance retries transient busy detach errors and fails after three attempts', darwinOnly, t => {
  const recovered = agentFixture(t, 'detach-busy');
  assert.equal(recovered.status, 0);
  assert.equal(recovered.report.detachAttempts, 2);
  const exhausted = agentFixture(t, 'detach-exhausted');
  assert.equal(exhausted.status, 1);
  assert.equal(exhausted.report.outcome, 'blocked');
  assert.equal(exhausted.report.detachAttempts, 3);
  assert.match(exhausted.report.cleanupError, /Resource busy/);
  assert.deepEqual(exhausted.report.remaining, []);
  assert.notEqual(exhausted.report.testRootRemoved, true);
});

test('dual-agent acceptance fails when cleanup cannot stop, preview, remove, verify, detach, or delete', darwinOnly, t => {
  for (const stage of ['stop', 'preview', 'remove', 'remaining', 'inventory', 'detach', 'root-cleanup']) {
    const { status, report } = agentFixture(t, stage);
    assert.equal(status, 1, stage);
    assert.equal(report.outcome, 'blocked', stage);
    assert.ok(report.cleanupError, stage);
    assert.ok(report.completedAt, stage);
    assert.notEqual(report.testRootRemoved, true, stage);
  }
});

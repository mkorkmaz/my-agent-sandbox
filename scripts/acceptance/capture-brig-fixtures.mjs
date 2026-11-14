import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const projectRoot = fileURLToPath(new URL('../../', import.meta.url));
const brigPath = process.argv[2] ?? path.join(projectRoot, '.build/brig-runtime/bin/brig');
const fixtureRoot = fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()), 'brig-contract-'));
const selectedProject = path.join(fixtureRoot, 'örnek proje');
const editorPath = path.join(fixtureRoot, 'policy-editor');
const policyPath = path.join(fixtureRoot, 'policy.yaml');
fs.mkdirSync(selectedProject);
const runtimeDirectory = path.join(fixtureRoot, 'bin');
const profileDirectory = path.join(fixtureRoot, 'profiles');
fs.mkdirSync(runtimeDirectory);
fs.mkdirSync(profileDirectory);
// Plan capture needs an available executable. This fixture never starts a VM or a hull store.
fs.writeFileSync(path.join(runtimeDirectory, 'hull'), '#!/bin/sh\nexit 1\n', { mode: 0o700 });
const claudeProfile = path.join(projectRoot, 'src-tauri/profiles/tugboat-claude.yaml');
fs.copyFileSync(claudeProfile, path.join(profileDirectory, 'tugboat-claude.yaml'));
fs.writeFileSync(editorPath, '#!/bin/sh\nexec /bin/cp "$BRIG_FIXTURE_POLICY" "$1"\n', { mode: 0o700 });
fs.writeFileSync(policyPath, `apiVersion: brig.sh/v1alpha1
kind: Policy
metadata:
  name: bd01-fixture
spec:
  egress:
    default: deny
    allow:
      - host: auth.openai.com
      - host: chatgpt.com
      - host: api.openai.com
`);

// Only these values reach the fixture processes. Each directory belongs to this run.
const childEnvironment = {
  HOME: fixtureRoot,
  PATH: runtimeDirectory + ':/usr/bin:/bin',
  XDG_CONFIG_HOME: path.join(fixtureRoot, 'config'),
  BRIG_STATE_DIR: path.join(fixtureRoot, 'state'),
  BRIG_POLICY_DIR: path.join(fixtureRoot, 'policies'),
  BRIG_PROFILE_DIR: profileDirectory,
  BRIG_HYPERVISOR: 'hvi',
  BRIG_RUNTIME: 'hull',
  BRIG_VERIFY: 'require',
  DO_NOT_TRACK: '1',
  TERM: 'dumb',
  EDITOR: editorPath,
  VISUAL: editorPath,
  BRIG_FIXTURE_POLICY: policyPath,
};

function invoke(args, environment = childEnvironment, binary = brigPath) {
  const result = spawnSync(binary, args, {
    cwd: selectedProject,
    env: environment,
    encoding: 'utf8',
    timeout: 30_000,
    maxBuffer: 1024 * 1024,
  });
  return {
    command: [binary, ...args],
    exitCode: result.status,
    signal: result.signal,
    errorCode: result.error?.code ?? null,
    stdout: result.stdout ?? '',
    stderr: result.stderr ?? '',
  };
}

function successful(args) {
  const result = invoke(args);
  assert.equal(result.errorCode, null, result.stderr);
  assert.equal(result.exitCode, 0, result.stderr);
  return result;
}

try {
  const version = successful(['--json', 'version']);
  const release = JSON.parse(version.stdout).data;
  assert.match(release.version, /^v\d+\.\d+\.\d+$/);
  const outputDirectory = path.resolve(process.argv[3] ?? path.join(projectRoot, 'tests/fixtures/brig', release.version));
  successful(['policy', 'create', 'bd01-fixture']);
  successful(['policy', 'attach', 'bd01-fixture', 'codex', '-n', 'bd01-fixture']);
  successful(['policy', 'check', 'codex', '-n', 'bd01-fixture']);

  const args = ['--json', 'plan', '--network', 'isolated', 'codex@bd01-fixture', selectedProject];
  const ready = invoke(args);
  const refused = invoke(args, { ...childEnvironment, BRIG_HYPERVISOR: 'vz' });
  const missingRuntime = invoke(args, { ...childEnvironment, PATH: '/usr/bin:/bin' });
  const missingBrig = invoke(['--json', 'version'], childEnvironment, path.join(fixtureRoot, 'missing-brig'));

  for (const result of [ready, refused, missingRuntime]) {
    assert.equal(result.exitCode, 0, result.stderr);
    assert.equal(result.errorCode, null);
    assert.equal(JSON.parse(result.stdout).kind, 'Plan');
  }
  assert.equal(JSON.parse(ready.stdout).data.refused, undefined);
  assert.equal(JSON.parse(ready.stdout).data.runtime.available, true);
  assert.ok(JSON.parse(refused.stdout).data.refused.reason);
  assert.equal(JSON.parse(missingRuntime.stdout).data.runtime.available, false);
  assert.equal(missingBrig.errorCode, 'ENOENT');

  const cases = { version, ready, refused, 'missing-runtime': missingRuntime, 'missing-brig': missingBrig };
  fs.mkdirSync(outputDirectory, { recursive: true });
  for (const [name, result] of Object.entries(cases)) {
    const sanitized = JSON.stringify(result, null, 2).replaceAll(fixtureRoot, '/fixture');
    fs.writeFileSync(path.join(outputDirectory, name + '.json'), sanitized + '\n');
  }
  fs.writeFileSync(path.join(outputDirectory, 'manifest.json'), JSON.stringify({
    capturedAt: new Date().toISOString(),
    brigVersion: release.version,
    brigCommit: release.commit,
    apiVersion: 'brig.sh/v1alpha1',
    host: 'darwin/arm64',
    rootReplacement: '/fixture',
    cases: Object.keys(cases),
    planScope: 'No-boot CLI plan. The runtime is an executable fixture. Native, VM, and account acceptance are separate.',
  }, null, 2) + '\n');
  fs.writeFileSync(policyPath, `apiVersion: brig.sh/v1alpha1
kind: Policy
metadata:
  name: bd01-claude
spec:
  egress:
    default: deny
    allow:
      - host: api.anthropic.com
      - host: platform.claude.com
      - host: claude.ai
`);
  successful(['policy', 'create', 'bd01-claude']);
  successful(['policy', 'attach', 'bd01-claude', 'tugboat-claude', '-n', 'bd01-claude']);
  successful(['policy', 'check', 'tugboat-claude', '-n', 'bd01-claude']);
  const claudeArguments = ['--json', 'plan', '--network', 'isolated', '--home', path.join(fixtureRoot, 'guest-claude'), 'tugboat-claude@bd01-claude', selectedProject];
  const claudeCases = {
    profile: successful(['agent', 'show', 'tugboat-claude', '--json']),
    ready: invoke(claudeArguments),
    refused: invoke(claudeArguments, { ...childEnvironment, BRIG_HYPERVISOR: 'vz' }),
    'missing-runtime': invoke(claudeArguments, { ...childEnvironment, PATH: '/usr/bin:/bin' }),
  };
  const claudePlan = JSON.parse(claudeCases.ready.stdout).data;
  assert.equal(claudePlan.runtime.available, true);
  assert.equal(claudePlan.runtime.backend, 'hvi');
  assert.equal(claudePlan.home.host, path.join(fixtureRoot, 'guest-claude-bd01-claude'));
  assert.deepEqual(claudePlan.credentials, []);
  assert.equal(claudePlan.verify.mode, 'require');
  assert.ok(JSON.parse(claudeCases.refused.stdout).data.refused.reason);
  assert.equal(JSON.parse(claudeCases['missing-runtime'].stdout).data.runtime.available, false);
  const claudeDirectory = path.join(outputDirectory, 'claude-code');
  fs.mkdirSync(claudeDirectory, { recursive: true });
  for (const [name, result] of Object.entries(claudeCases)) {
    assert.equal(result.errorCode, null);
    assert.equal(result.exitCode, 0, result.stderr);
    fs.writeFileSync(path.join(claudeDirectory, name + '.json'), JSON.stringify(result, null, 2).replaceAll(fixtureRoot, '/fixture') + '\n');
  }
  fs.writeFileSync(path.join(claudeDirectory, 'manifest.json'), JSON.stringify({
    capturedAt: new Date().toISOString(), brigVersion: release.version, brigCommit: release.commit,
    apiVersion: 'brig.sh/v1alpha1', agentKind: 'claude-code', rootReplacement: '/fixture',
    profileSource: 'src-tauri/profiles/tugboat-claude.yaml',
    profileSha256: createHash('sha256').update(fs.readFileSync(claudeProfile)).digest('hex'),
    cases: Object.keys(claudeCases),
    planScope: 'Real Brig CLI with a temporary runtime executable, profile, policy, home, and state. No VM or account operation.',
  }, null, 2) + '\n');
  console.log('Captured ' + Object.keys(cases).length + ' cases in ' + outputDirectory);
  console.log('Captured ' + Object.keys(claudeCases).length + ' Claude cases in ' + claudeDirectory);
} finally {
  fs.rmSync(fixtureRoot, { recursive: true, force: true });
}

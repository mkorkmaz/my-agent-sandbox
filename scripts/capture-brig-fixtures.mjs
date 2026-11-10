import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const projectRoot = fileURLToPath(new URL('../', import.meta.url));
const brigPath = process.argv[2] ?? '/opt/homebrew/bin/brig';
const fixtureRoot = fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()), 'brig-contract-'));
const selectedProject = path.join(fixtureRoot, 'örnek proje');
const editorPath = path.join(fixtureRoot, 'policy-editor');
const policyPath = path.join(fixtureRoot, 'policy.yaml');
fs.mkdirSync(selectedProject);
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
  PATH: '/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin',
  XDG_CONFIG_HOME: path.join(fixtureRoot, 'config'),
  BRIG_STATE_DIR: path.join(fixtureRoot, 'state'),
  BRIG_POLICY_DIR: path.join(fixtureRoot, 'policies'),
  BRIG_HYPERVISOR: 'hvi',
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
    planScope: 'No-boot CLI plan. A running VM and interactive Codex are verified in later plan items.',
  }, null, 2) + '\n');
  console.log('Captured ' + Object.keys(cases).length + ' cases in ' + outputDirectory);
} finally {
  fs.rmSync(fixtureRoot, { recursive: true, force: true });
}

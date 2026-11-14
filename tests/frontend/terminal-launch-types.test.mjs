import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const compiler = path.join(root, 'node_modules/.bin/tsc');
const arguments_ = ['--ignoreConfig', '--noEmit', '--strict', '--target', 'ES2022', '--module', 'ESNext', '--moduleResolution', 'Bundler'];

test('terminal launch arguments reject missing and cross-command fields at build time', () => {
  const result = spawnSync(compiler, [...arguments_, 'tests/fixtures/terminal-launch-types.ts'], { cwd: root, encoding: 'utf8', timeout: 30000 });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, result.stdout + result.stderr);
});

test('a malformed terminal resume fails the TypeScript build', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-terminal-types-'));
  try {
    const file = path.join(directory, 'invalid.ts');
    fs.writeFileSync(file, `import type { OpenTerminal } from ${JSON.stringify(path.join(root, 'src/terminal/terminal-launch'))};\ndeclare const open: OpenTerminal;\nvoid open('start_agent_session', { identifier: 'project-a', environmentId: 'environment', fingerprint: 'reviewed-plan', action: 'resume', useBrigBoundary: true }, 'Resume');\n`);
    const result = spawnSync(compiler, [...arguments_, file], { cwd: root, encoding: 'utf8', timeout: 30000 });
    assert.equal(result.error, undefined);
    assert.notEqual(result.status, 0);
    assert.match(result.stdout + result.stderr, /sessionId.*missing/);
  } finally { fs.rmSync(directory, { recursive: true, force: true }); }
});

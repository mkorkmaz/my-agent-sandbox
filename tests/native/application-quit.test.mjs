import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

test('macOS keeps the app open after a cancelled quit and exits after approval', { skip: process.platform !== 'darwin' }, context => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-quit-'));
  context.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const program = path.join(directory, 'test-quit');
  const native = fileURLToPath(new URL('../../src-tauri/native/ApplicationQuit.m', import.meta.url));
  const fixture = fileURLToPath(new URL('./application-quit.m', import.meta.url));
  execFileSync('/usr/bin/clang', ['-fobjc-arc', '-framework', 'AppKit', native, fixture, '-o', program], { timeout: 60_000 });
  const output = execFileSync(program, [], { encoding: 'utf8', timeout: 10_000 });
  assert.match(output, /Confirmed two quit requests/);
});

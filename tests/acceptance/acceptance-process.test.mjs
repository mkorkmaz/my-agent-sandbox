import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { runAcceptanceProcess } from '../../scripts/acceptance/acceptance-process.mjs';

const unixOnly = { skip: process.platform === 'win32' };

test('acceptance process keeps argument values separate and reports a missing executable', unixOnly, async () => {
  const result = await runAcceptanceProcess(process.execPath, ['-e', 'process.stdout.write(process.argv[1]); process.stderr.write("detail");', 'a value; $(literal)'], { timeout: 2_000 });
  assert.equal(result.status, 0);
  assert.equal(result.error, null);
  assert.equal(result.stdout, 'a value; $(literal)');
  assert.equal(result.stderr, 'detail');
  const missing = await runAcceptanceProcess('/missing/tugboat-acceptance-command', [], { timeout: 2_000 });
  assert.equal(missing.error, 'ENOENT');
  assert.notEqual(missing.status, 0);
});

test('acceptance process kills the process group when output exceeds the limit', unixOnly, async () => {
  const result = await runAcceptanceProcess(process.execPath, ['-e', 'process.stdout.write("x".repeat(4096)); setInterval(() => {}, 1000);'], { timeout: 2_000, maxBuffer: 128 });
  assert.equal(result.error, 'ENOBUFS');
  assert.equal(result.signal, 'SIGKILL');
  assert.ok(Buffer.byteLength(result.stdout) <= 128);
});

test('acceptance timeout terminates descendants instead of leaving a pull process running', unixOnly, async t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-command-group-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const childFile = path.join(root, 'child-pid');
  const heartbeat = path.join(root, 'heartbeat');
  const descendantSource = 'const fs=require("node:fs"); let count=0; setInterval(()=>fs.writeFileSync(process.argv[1],String(++count)),20);';
  const source = `const {spawn}=require('node:child_process'); const fs=require('node:fs'); const child=spawn(process.execPath,['-e',${JSON.stringify(descendantSource)},process.argv[2]],{stdio:'ignore'}); fs.writeFileSync(process.argv[1],String(child.pid)); setInterval(()=>{},1000);`;
  const result = await runAcceptanceProcess(process.execPath, ['-e', source, childFile, heartbeat], { timeout: 500 });
  assert.equal(result.error, 'ETIMEDOUT');
  assert.equal(result.signal, 'SIGKILL');
  const descendant = Number(fs.readFileSync(childFile, 'utf8'));
  const before = fs.readFileSync(heartbeat, 'utf8');
  assert.ok(Number(before) > 0, 'The descendant must run before the timeout.');
  await new Promise(resolve => setTimeout(resolve, 150));
  const after = fs.readFileSync(heartbeat, 'utf8');
  if (after !== before) {
    try { process.kill(descendant, 'SIGKILL'); }
    catch (error) { if (error.code !== 'ESRCH') throw error; }
  }
  assert.equal(after, before, 'The descendant must stop after the timeout.');
});

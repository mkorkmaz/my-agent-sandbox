import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { runtimeTools } from '../../scripts/runtime/runtime-bundle.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const macOnly = { skip: process.platform !== 'darwin' || process.arch !== 'arm64' ? 'requires an Apple silicon Mac' : false };

function fixture(context, noSign) {
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-runtime-prepare-'));
  context.after(() => fs.rmSync(workspace, { recursive: true, force: true }));
  const write = (relative, contents) => {
    const file = path.join(workspace, relative);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, contents);
  };
  const payload = 'synthetic runtime archive';
  const sha256 = createHash('sha256').update(payload).digest('hex');
  const pins = { cosign: { url: 'https://fixture.invalid/cosign', sha256 } };
  for (const name of ['brig', 'hull']) {
    pins[name] = { url: `https://fixture.invalid/${name}.tar.gz`, sha256, signer: 'fixture identity', sbomSha256: sha256, version: name === 'brig' ? 'v0.5.2' : 'v0.1.0' };
    for (const suffix of ['tar.gz', 'pem', 'sig']) write(`.build/brig-runtime-downloads/${name}.${suffix}`, payload);
    write(`.build/brig-runtime-downloads/${name}-sbom.json`, payload);
    write(`.build/brig-runtime-downloads/${name}-checksums.txt`, `${sha256}  ${name}.tar.gz\n`);
    write(`.build/${name}-compat/bin/build.json`, '{}');
  }
  write('.build/brig-runtime-downloads/cosign', payload);
  write('config/brig-runtime.json', JSON.stringify(pins));
  write('config/disk-tools.json', '{}');
  write('resources/runtime/registry/config.json', '{}');
  write('.build/disk-tools/etc/mke2fs.conf', 'fixture configuration');
  for (const tool of runtimeTools.filter(tool => tool !== 'cosign')) {
    let directory = 'brig-compat/bin';
    if (tool === 'hull') directory = 'hull-compat/bin';
    if (['hvi', 'vz-runner'].includes(tool)) directory = 'brig-runtime-inputs/hull';
    if (['mke2fs', 'debugfs', 'e2fsck'].includes(tool)) directory = 'disk-tools/bin';
    write(`.build/${directory}/${tool}`, 'fixture executable');
  }
  for (const script of ['prepare-bundled-runtime.mjs', 'runtime-bundle.mjs']) write(`scripts/runtime/${script}`, fs.readFileSync(path.join(root, 'scripts/runtime', script)));
  const commandsFile = path.join(workspace, 'commands.jsonl');
  const result = spawnSync(process.execPath, [
    '--require', path.join(root, 'tests/fixtures/native-preparation.cjs'),
    path.join(workspace, 'scripts/runtime/prepare-bundled-runtime.mjs'), ...(noSign ? ['--no-sign'] : []),
  ], {
    cwd: workspace, encoding: 'utf8', timeout: 10_000,
    env: { ...process.env, TUGBOAT_NATIVE_COMMANDS: commandsFile, APPLE_SIGNING_IDENTITY: 'fixture release identity' },
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  return fs.readFileSync(commandsFile, 'utf8').trim().split('\n').map(line => JSON.parse(line));
}

for (const noSign of [false, true]) {
  test(`runtime preparation ${noSign ? 'bypasses release signing' : 'preserves default release signing'} and retains vendor verification`, macOnly, context => {
    const commands = fixture(context, noSign);
    const preparers = commands.filter(command => command.binary === process.execPath);
    assert.equal(preparers.length, 3);
    for (const command of preparers.slice(0, 2)) assert.deepEqual(command.argumentsList.slice(1), noSign ? ['--no-sign'] : []);
    const signatures = commands.filter(command => command.binary === '/usr/bin/codesign' && command.argumentsList.includes('--sign'));
    assert.equal(signatures.length, noSign ? 0 : runtimeTools.length - 2);
    assert.equal(commands.filter(command => command.argumentsList.includes('verify-blob')).length, 2);
    assert.equal(commands.filter(command => command.binary === '/usr/bin/codesign' && command.argumentsList.includes('-R')).length, 2);
  });
}

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const macOnly = { skip: process.platform !== 'darwin' || process.arch !== 'arm64' ? 'requires an Apple silicon Mac' : false };

function fixture(context, tool, { testMode = true, goAvailable = true, failTest = false, noSign = false } = {}) {
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-native-prepare-'));
  context.after(() => fs.rmSync(workspace, { recursive: true, force: true }));
  const write = (relative, contents) => {
    const file = path.join(workspace, relative);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, contents);
  };
  const archive = 'synthetic native archive';
  const sha256 = createHash('sha256').update(archive).digest('hex');
  const pin = { version: 'v0.1.0', commit: 'fixture-commit', variant: 'fixture', source: { url: 'https://fixture.invalid/source.tar.gz', sha256 } };
  write('config/hull-compat.json', JSON.stringify({ ...pin, go: { url: 'https://fixture.invalid/go.tar.gz', sha256 } }));
  write('config/brig-compat.json', JSON.stringify(pin));
  for (const name of ['hull', 'brig']) {
    write(`scripts/runtime/prepare-${name}.mjs`, fs.readFileSync(path.join(root, `scripts/runtime/prepare-${name}.mjs`)));
    write(`.build/${name}-compat/downloads/${name}.tar.gz`, archive);
    write(`resources/runtime/${name}-disk-tools.patch`, 'fixture patch');
  }
  write('.build/hull-compat/downloads/go.tar.gz', archive);
  write('resources/runtime/hull-spaces.patch', 'fixture patch');
  for (const name of ['hull_spaces_test.go', 'hull_disk_tools_test.go', 'brig_disk_tools_test.go']) write(`tests/native/${name}`, 'fixture test');
  if (goAvailable) write('.build/hull-compat/go/bin/go', 'fixture Go');
  const commands = path.join(workspace, 'commands.jsonl');
  const run = (argumentsList = [...(testMode ? ['--test'] : []), ...(noSign ? ['--no-sign'] : [])]) => spawnSync(process.execPath, [
    '--require', path.join(root, 'tests/fixtures/native-preparation.cjs'),
    path.join(workspace, `scripts/runtime/prepare-${tool}.mjs`),
    ...argumentsList,
  ], {
    cwd: workspace, encoding: 'utf8', timeout: 10_000,
    env: { ...process.env, TUGBOAT_NATIVE_COMMANDS: commands, TUGBOAT_FAIL_NATIVE_TEST: String(failTest) },
  });
  const result = run();
  assert.ifError(result.error);
  return { result, run, workspace, commands: () => fs.readFileSync(commands, 'utf8').trim().split('\n').map(line => JSON.parse(line)) };
}

for (const tool of ['hull', 'brig']) {
  test(`cold ${tool} test mode runs Go tests without building or signing executables`, macOnly, context => {
    const { result, commands, workspace } = fixture(context, tool);
    assert.equal(result.status, 0, result.stderr);
    assert.ok(commands().some(command => command.argumentsList[0] === 'test'));
    assert.ok(commands().every(command => command.argumentsList[0] !== 'build' && command.binary !== '/usr/bin/codesign'));
    assert.equal(fs.existsSync(path.join(workspace, `.build/${tool}-compat/prepared.json`)), false);
  });

  test(`default ${tool} preparation still builds, signs, and caches its executables`, macOnly, context => {
    const { result, run, commands, workspace } = fixture(context, tool, { testMode: false });
    assert.equal(result.status, 0, result.stderr);
    const count = tool === 'hull' ? 1 : 3;
    assert.equal(commands().filter(command => command.argumentsList[0] === 'build').length, count);
    assert.equal(commands().filter(command => command.binary === '/usr/bin/codesign').length, count);
    assert.equal(fs.existsSync(path.join(workspace, `.build/${tool}-compat/prepared.json`)), true);
    const before = commands();
    const cached = run();
    assert.equal(cached.status, 0, cached.stderr);
    assert.deepEqual(commands(), before);
  });

  test(`${tool} no-sign preparation has a separate cache from default preparation`, macOnly, context => {
    const { result, run, commands, workspace } = fixture(context, tool, { testMode: false, noSign: true });
    assert.equal(result.status, 0, result.stderr);
    assert.ok(commands().some(command => command.argumentsList[0] === 'build'));
    assert.ok(commands().every(command => command.binary !== '/usr/bin/codesign'));
    const marker = path.join(workspace, `.build/${tool}-compat/prepared.json`);
    assert.equal(JSON.parse(fs.readFileSync(marker)).signing, 'none');
    const before = commands();
    assert.equal(run().status, 0);
    assert.deepEqual(commands(), before);
    const standard = run([]);
    assert.equal(standard.status, 0, standard.stderr);
    assert.ok(commands().some(command => command.binary === '/usr/bin/codesign'));
    assert.equal(JSON.parse(fs.readFileSync(marker)).signing, 'ad-hoc');
  });
}

test('cold Brig test mode requests test-only hull preparation when Go is absent', macOnly, context => {
  const { result, commands } = fixture(context, 'brig', { goAvailable: false });
  assert.equal(result.status, 0, result.stderr);
  const prepare = commands().find(command => command.binary === process.execPath);
  assert.deepEqual(prepare.argumentsList.slice(1), ['--test']);
  assert.ok(commands().every(command => command.binary !== '/usr/bin/codesign'));
});

test('Brig no-sign preparation forwards its mode when Go requires hull preparation', macOnly, context => {
  const { result, commands } = fixture(context, 'brig', { testMode: false, goAvailable: false, noSign: true });
  assert.equal(result.status, 0, result.stderr);
  const prepare = commands().find(command => command.binary === process.execPath);
  assert.deepEqual(prepare.argumentsList.slice(1), ['--no-sign']);
  assert.ok(commands().every(command => command.binary !== '/usr/bin/codesign'));
});

test('a failing native test stops preparation before executables are built or signed', macOnly, context => {
  for (const tool of ['hull', 'brig']) {
    const { result, commands } = fixture(context, tool, { failTest: true });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /synthetic Go test failure/);
    assert.ok(commands().every(command => command.argumentsList[0] !== 'build' && command.binary !== '/usr/bin/codesign'));
  }
});

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const macOnly = { skip: process.platform !== 'darwin' || process.arch !== 'arm64' ? 'requires an Apple silicon Mac' : false };

async function fixture(context, response) {
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-ghostty-prepare-'));
  context.after(() => fs.rmSync(workspace, { recursive: true, force: true }));
  const archive = 'synthetic Ghostty archive';
  const sha256 = createHash('sha256').update(archive).digest('hex');
  const requests = [];
  const server = http.createServer((request, result) => {
    requests.push(request.url);
    const { status, body = archive } = response(request.url, requests.length);
    result.writeHead(status); result.end(body);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  context.after(() => new Promise(resolve => server.close(resolve)));
  const base = 'http://127.0.0.1:' + server.address().port;
  fs.mkdirSync(path.join(workspace, 'config'));
  fs.writeFileSync(path.join(workspace, 'config/development-versions.json'), JSON.stringify({ ghostty: {
    version: 'fixture', commit: 'fixture-commit', source: { url: base + '/source.tar.gz', sha256 },
    dependencies: { url: base + '/dependencies.tar.gz', sha256 },
    zig: { version: 'fixture', tarball: base + '/zig.tar.xz', shasum: sha256 },
  } }));
  fs.mkdirSync(path.join(workspace, 'scripts/runtime'), { recursive: true });
  fs.copyFileSync(path.join(root, 'scripts/runtime/prepare-ghostty.mjs'), path.join(workspace, 'scripts/runtime/prepare-ghostty.mjs'));
  const commandsFile = path.join(workspace, 'commands.jsonl');
  const run = () => new Promise(resolve => execFile(process.execPath, [
    '--require', path.join(root, 'tests/fixtures/ghostty-preparation.cjs'),
    path.join(workspace, 'scripts/runtime/prepare-ghostty.mjs'),
  ], {
    cwd: workspace, timeout: 20_000, env: { ...process.env, TUGBOAT_GHOSTTY_COMMANDS: commandsFile },
  }, (error, stdout, stderr) => resolve({ error, stdout, stderr })));
  const work = path.join(workspace, '.build/libghostty');
  return { run, requests, work, archive, commands: () => fs.existsSync(commandsFile) ? fs.readFileSync(commandsFile, 'utf8').trim().split('\n').map(line => JSON.parse(line)) : [] };
}

test('Ghostty preparation retries a transient download and caches only verified archives', macOnly, async context => {
  const { run, requests, work, archive, commands } = await fixture(context, (_url, count) => ({ status: count === 1 ? 503 : 200 }));
  const result = await run();
  assert.ifError(result.error);
  assert.deepEqual(requests, ['/source.tar.gz', '/source.tar.gz', '/dependencies.tar.gz', '/zig.tar.xz']);
  assert.equal(fs.readFileSync(path.join(work, 'source.tar.gz'), 'utf8'), archive);
  assert.ok(fs.existsSync(path.join(work, 'install/pin.json')));
  const downloads = commands().filter(command => command.binary === '/usr/bin/curl');
  assert.equal(downloads.length, 3);
  for (const command of downloads) {
    assert.equal(command.argumentsList[command.argumentsList.indexOf('--retry') + 1], '2');
    assert.equal(command.argumentsList[command.argumentsList.indexOf('--connect-timeout') + 1], '15');
    assert.equal(command.argumentsList[command.argumentsList.indexOf('--max-time') + 1], '180');
    assert.ok(command.timeout > 3 * 180_000);
  }
  assert.ok(fs.readdirSync(work).every(name => !name.endsWith('.partial')));
});

test('Ghostty preparation stops after bounded download retries without extracting or building', macOnly, async context => {
  const { run, requests, work, commands } = await fixture(context, () => ({ status: 503 }));
  const result = await run();
  assert.ok(result.error);
  assert.deepEqual(requests, ['/source.tar.gz', '/source.tar.gz', '/source.tar.gz']);
  assert.ok(commands().every(command => command.binary === '/usr/bin/curl'));
  assert.equal(fs.existsSync(path.join(work, 'source.tar.gz')), false);
  assert.equal(fs.existsSync(path.join(work, 'source.tar.gz.partial')), false);
  assert.equal(fs.existsSync(path.join(work, 'install/pin.json')), false);
});

test('Ghostty preparation rejects a bad checksum and can recover on the next run', macOnly, async context => {
  const { run, requests, work, archive, commands } = await fixture(context, (_url, count) => ({ status: 200, body: count === 1 ? 'corrupt archive' : undefined }));
  const rejected = await run();
  assert.ok(rejected.error);
  assert.match(rejected.stderr, /Archive checksum mismatch/);
  assert.equal(fs.existsSync(path.join(work, 'source.tar.gz')), false);
  assert.equal(fs.existsSync(path.join(work, 'source.tar.gz.partial')), false);
  assert.ok(commands().every(command => command.binary === '/usr/bin/curl'));
  const recovered = await run();
  assert.ifError(recovered.error);
  assert.equal(fs.readFileSync(path.join(work, 'source.tar.gz'), 'utf8'), archive);
  assert.equal(requests.length, 4);
  assert.ok(fs.existsSync(path.join(work, 'install/pin.json')));
});

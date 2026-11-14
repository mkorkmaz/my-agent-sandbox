import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const workspace = fileURLToPath(new URL('../../', import.meta.url));
const configuration = JSON.parse(fs.readFileSync(path.join(workspace, 'config/clean-mac-test.json'), 'utf8'));
assert.equal(configuration.schemaVersion, 1);
assert.match(configuration.node.version, /^\d+\.\d+\.\d+$/);
assert.equal(configuration.node.url, 'https://nodejs.org/dist/v' + configuration.node.version + '/node-v' + configuration.node.version + '-darwin-arm64.tar.gz');
assert.match(configuration.node.sha256, /^[a-f0-9]{64}$/);
const outputDirectory = path.join(workspace, 'artifacts/bundled-runtime');
fs.mkdirSync(outputDirectory, { recursive: true });
const staging = fs.mkdtempSync(path.join(outputDirectory, '.clean-mac-kit-'));
const kit = path.join(staging, 'Tugboat Clean Mac Test');
const archive = path.join(outputDirectory, 'clean-mac-test-kit.zip');
const temporaryArchive = path.join(staging, 'kit.zip');
try {
  for (const directory of ['scripts/acceptance', 'config']) fs.mkdirSync(path.join(kit, directory), { recursive: true });
  for (const name of ['check-bundled-runtime.mjs', 'acceptance-process.mjs']) fs.copyFileSync(path.join(workspace, 'scripts/acceptance', name), path.join(kit, 'scripts/acceptance', name));
  fs.copyFileSync(path.join(workspace, 'config/clean-mac-test.json'), path.join(kit, 'config/clean-mac-test.json'));
  for (const name of ['Run Clean Mac Check.command', 'README.txt']) fs.copyFileSync(path.join(workspace, 'resources/clean-mac-test', name), path.join(kit, name));
  fs.chmodSync(path.join(kit, 'Run Clean Mac Check.command'), 0o755);
  execFileSync('/usr/bin/ditto', ['-c', '-k', '--norsrc', '--keepParent', kit, temporaryArchive], { timeout: 30_000 });
  fs.renameSync(temporaryArchive, archive);
  console.log(JSON.stringify({ archive, bytes: fs.statSync(archive).size, sha256: createHash('sha256').update(fs.readFileSync(archive)).digest('hex') }, null, 2));
} finally {
  fs.rmSync(staging, { recursive: true, force: true });
}

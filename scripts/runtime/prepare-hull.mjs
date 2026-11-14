import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const pin = JSON.parse(fs.readFileSync(path.join(root, 'config/hull-compat.json')));
const work = path.join(root, '.build/hull-compat');
const source = path.join(work, 'hull-' + pin.commit);
const patch = path.join(root, 'resources/runtime/hull-spaces.patch');
const diskToolsPatch = path.join(root, 'resources/runtime/hull-disk-tools.patch');
const hash = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const noSign = process.argv.includes('--no-sign');
const identity = JSON.stringify({ ...pin, patch: hash(patch), diskToolsPatch: hash(diskToolsPatch), signing: noSign ? 'none' : 'ad-hoc' });
const binary = path.join(work, 'bin/hull');
const buildFile = path.join(work, 'bin/build.json');
const marker = path.join(work, 'prepared.json');
const test = process.argv.includes('--test');
if (process.platform !== 'darwin' || process.arch !== 'arm64') throw new Error('This build requires an Apple silicon Mac.');
function savedBuild() {
  if (!fs.existsSync(buildFile)) return null;
  try { return JSON.parse(fs.readFileSync(buildFile, 'utf8')); }
  catch (error) {
    if (error instanceof SyntaxError) return null;
    throw error;
  }
}
const build = savedBuild();
const cached = fs.existsSync(marker) && fs.readFileSync(marker, 'utf8') === identity
  && fs.existsSync(binary) && build?.binarySha256 === hash(binary)
  && build.patchSha256 === hash(patch) && build.diskToolsPatchSha256 === hash(diskToolsPatch);
if (cached && !test) process.exit(0);
fs.mkdirSync(path.join(work, 'downloads'), { recursive: true });
async function archive(url, checksum, name) {
  const file = path.join(work, 'downloads', name);
  if (!fs.existsSync(file)) {
    const response = await fetch(url, { signal: AbortSignal.timeout(180_000) });
    if (!response.ok) throw new Error('Download failed: ' + response.status);
    const temporary = file + '.partial';
    fs.writeFileSync(temporary, Buffer.from(await response.arrayBuffer()));
    fs.renameSync(temporary, file);
  }
  if (hash(file) !== checksum) throw new Error('Archive checksum mismatch: ' + name);
  execFileSync('/usr/bin/tar', ['-xf', file, '-C', work]);
}
if (!cached) {
  fs.rmSync(source, { recursive: true, force: true });
  await archive(pin.source.url, pin.source.sha256, 'hull.tar.gz');
  await archive(pin.go.url, pin.go.sha256, 'go.tar.gz');
  execFileSync('/usr/bin/patch', ['--batch', '-p1', '-i', patch], { cwd: source, stdio: 'inherit' });
  execFileSync('/usr/bin/patch', ['--batch', '-p1', '-i', diskToolsPatch], { cwd: source, stdio: 'inherit' });
}
fs.copyFileSync(path.join(root, 'tests/native/hull_spaces_test.go'), path.join(source, 'cmd/hull/desktop_spaces_test.go'));
fs.copyFileSync(path.join(root, 'tests/native/hull_disk_tools_test.go'), path.join(source, 'cmd/hull/desktop_disk_tools_test.go'));
const go = path.join(work, 'go/bin/go');
const environment = { ...process.env, GOTOOLCHAIN: 'local', GOCACHE: path.join(work, 'go-cache'), GOPATH: path.join(work, 'go-path'), CGO_ENABLED: '0', GOOS: 'darwin', GOARCH: 'arm64' };
const options = { cwd: source, env: environment, stdio: 'inherit' };
if (test) {
  execFileSync(go, ['test', '-mod=readonly', './cmd/hull', '-run', 'Test(Desktop|ParseShares|SplitShare)', '-count=1', '-v'], options);
  process.exit(0);
}
if (!cached) {
  fs.mkdirSync(path.dirname(binary), { recursive: true });
  const info = 'github.com/brig-sh/hull/internal/buildinfo';
  const flags = ['-s', '-w', '-X', info + '.gitCommit=' + pin.commit, '-X', info + '.gitCommitTime=' + pin.commitTime, '-X', info + '.gitDescribe=v0.1.0-0-g7e50ac3', '-X', info + '.gitModified=true'].join(' ');
  execFileSync(go, ['build', '-mod=readonly', '-buildvcs=false', '-trimpath', '-ldflags', flags, '-o', binary, './cmd/hull'], options);
  if (!noSign) execFileSync('/usr/bin/codesign', ['--force', '--sign', '-', binary], { stdio: 'inherit' });
  fs.writeFileSync(buildFile, JSON.stringify({ ...pin, modified: true, patchSha256: hash(patch), diskToolsPatchSha256: hash(diskToolsPatch), binarySha256: hash(binary) }, null, 2) + '\n');
  fs.writeFileSync(marker, identity);
  console.log('Prepared hull ' + pin.version + ' with ' + pin.variant + '.');
}

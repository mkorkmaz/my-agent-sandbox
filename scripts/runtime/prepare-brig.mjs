import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const pin = JSON.parse(fs.readFileSync(path.join(root, 'config/brig-compat.json'), 'utf8'));
const hull = JSON.parse(fs.readFileSync(path.join(root, 'config/hull-compat.json'), 'utf8'));
const work = path.join(root, '.build/brig-compat');
const source = path.join(work, 'brig-' + pin.commit);
const patch = path.join(root, 'resources/runtime/brig-disk-tools.patch');
const binaries = ['brig', 'brigd', 'brig-broker'];
const hash = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const noSign = process.argv.includes('--no-sign');
const identity = JSON.stringify({...pin, go:hull.go, patch:hash(patch), script:hash(fileURLToPath(import.meta.url)), signing:noSign ? 'none' : 'ad-hoc'});
const manifestFile = path.join(work, 'bin/build.json');
const marker = path.join(work, 'prepared.json');
const test = process.argv.includes('--test');
if (process.platform !== 'darwin' || process.arch !== 'arm64') throw new Error('Brig requires an Apple silicon build machine.');
const saved = fs.existsSync(manifestFile) ? JSON.parse(fs.readFileSync(manifestFile, 'utf8')) : null;
const cached = fs.existsSync(marker) && fs.readFileSync(marker, 'utf8') === identity
  && binaries.every(name => fs.existsSync(path.join(work, 'bin', name)) && saved?.tools?.[name]?.sha256 === hash(path.join(work, 'bin', name)));
if (cached && !test) process.exit(0);
const run = (binary, arguments_, options={}) => execFileSync(binary, arguments_, {timeout:300_000, stdio:'inherit', ...options});
const go = path.join(root, '.build/hull-compat/go/bin/go');
if (!fs.existsSync(go)) run(process.execPath, [path.join(root, 'scripts/runtime/prepare-hull.mjs'), ...(test ? ['--test'] : []), ...(noSign ? ['--no-sign'] : [])]);
fs.mkdirSync(path.join(work, 'downloads'), {recursive:true});
const archive = path.join(work, 'downloads/brig.tar.gz');
if (!fs.existsSync(archive) || hash(archive) !== pin.source.sha256) {
  const temporary = archive + '.partial';
  run('/usr/bin/curl', ['-fsSL', '--retry', '2', '--connect-timeout', '15', '--max-time', '180', pin.source.url, '-o', temporary]);
  if (hash(temporary) !== pin.source.sha256) throw new Error('Brig source archive checksum mismatch.');
  fs.renameSync(temporary, archive);
}
if (!cached) {
  fs.rmSync(source, {recursive:true, force:true});
  run('/usr/bin/tar', ['-xzf', archive, '-C', work]);
  run('/usr/bin/patch', ['--batch', '-p1', '-i', patch], {cwd:source});
}
fs.copyFileSync(path.join(root, 'tests/native/brig_disk_tools_test.go'), path.join(source, 'internal/runtime/desktop_disk_tools_test.go'));
const environment = {
  ...process.env, GOTOOLCHAIN:'local', CGO_ENABLED:'0', GOOS:'darwin', GOARCH:'arm64',
  GOCACHE:path.join(root, '.build/hull-compat/go-cache'), GOPATH:path.join(root, '.build/hull-compat/go-path'),
};
const options = {cwd:source, env:environment};
if (test) {
  run(go, ['test', '-mod=readonly', './internal/runtime', './internal/buildinfo', '-count=1'], options);
  process.exit(0);
}
if (cached) process.exit(0);
const temporary = fs.mkdtempSync(path.join(work, 'stage-'));
try {
  const info = 'github.com/brig-sh/brig/internal/buildinfo';
  const flags = ['-s', '-w', '-X', info + '.gitCommit=' + pin.commit, '-X', info + '.gitCommitTime=' + pin.commitTime,
    '-X', info + '.gitDescribe=' + pin.version + '-0-g' + pin.commit.slice(0,7), '-X', info + '.gitModified=true'].join(' ');
  for (const name of binaries) {
    const binary = path.join(temporary, name);
    run(go, ['build', '-mod=readonly', '-buildvcs=false', '-trimpath', '-ldflags', flags, '-o', binary, './cmd/' + name], options);
    if (!noSign) run('/usr/bin/codesign', ['--force', '--sign', '-', binary]);
  }
  const verificationHome = path.join(work, 'verification-home');
  fs.mkdirSync(verificationHome, {recursive:true});
  const version = JSON.parse(execFileSync(path.join(temporary, 'brig'), ['--json', 'version'], {
    encoding:'utf8', timeout:10_000, env:{HOME:verificationHome, PATH:'/usr/bin:/bin:/usr/sbin:/sbin', DO_NOT_TRACK:'1'},
  }));
  if (version.data.version !== pin.version || version.data.modified !== true || version.data.commit !== pin.commit) {
    throw new Error('Patched Brig reports an unexpected build identity.');
  }
  const tools = Object.fromEntries(binaries.map(name => [name, {sha256:hash(path.join(temporary, name))}]));
  fs.writeFileSync(path.join(temporary, 'build.json'), JSON.stringify({...pin, go:hull.go, modified:true, patchSha256:hash(patch), tools}, null, 2) + '\n');
  const destination = path.join(work, 'bin');
  const previous = path.join(work, 'bin.previous');
  fs.rmSync(previous, {recursive:true, force:true});
  if (fs.existsSync(destination)) fs.renameSync(destination, previous);
  try { fs.renameSync(temporary, destination); }
  catch (error) {
    if (fs.existsSync(previous)) fs.renameSync(previous, destination);
    throw error;
  }
  fs.writeFileSync(marker, identity);
  fs.rmSync(previous, {recursive:true, force:true});
  console.log('Prepared Brig ' + pin.version + ' with ' + pin.variant + '.');
} finally {
  fs.rmSync(temporary, {recursive:true, force:true});
}

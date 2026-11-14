import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const pin = JSON.parse(fs.readFileSync(path.join(root, 'config/disk-tools.json')));
const work = path.join(root, '.build/disk-tools');
const source = path.join(work, 'e2fsprogs-' + pin.version);
const compile = path.join(work, 'compile');
const hash = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const toolSources = { mke2fs: 'misc/mke2fs', debugfs: 'debugfs/debugfs', e2fsck: 'e2fsck/e2fsck' };
const configuration = path.join(work, 'etc/mke2fs.conf');
const manifestFile = path.join(work, 'build.json');
const identity = JSON.stringify({ ...pin, script: hash(fileURLToPath(import.meta.url)) });
if (process.platform !== 'darwin' || process.arch !== 'arm64') throw new Error('Disk tools require an Apple silicon Mac.');
const manifest = fs.existsSync(manifestFile) ? JSON.parse(fs.readFileSync(manifestFile)) : null;
const cached = manifest?.identity === identity
  && Object.keys(toolSources).every(name => fs.existsSync(path.join(work, 'bin', name)) && hash(path.join(work, 'bin', name)) === manifest.tools[name])
  && fs.existsSync(configuration) && hash(configuration) === manifest.configurationSha256;
if (cached) process.exit(0);

fs.mkdirSync(path.join(work, 'downloads'), { recursive: true });
const archive = path.join(work, 'downloads', 'e2fsprogs-' + pin.version + '.tar.gz');
if (!fs.existsSync(archive)) {
  const response = await fetch(pin.source.url, { signal: AbortSignal.timeout(180_000) });
  if (!response.ok) throw new Error('Disk tools source download failed: ' + response.status);
  const partial = archive + '.partial';
  fs.writeFileSync(partial, Buffer.from(await response.arrayBuffer()));
  if (hash(partial) !== pin.source.sha256) throw new Error('Disk tools source checksum mismatch.');
  fs.renameSync(partial, archive);
}
if (hash(archive) !== pin.source.sha256) throw new Error('Disk tools source checksum mismatch: ' + archive);
execFileSync('/usr/bin/tar', ['-xzf', archive, '-C', work], { timeout: 30_000 });
fs.rmSync(compile, { recursive: true, force: true });
fs.mkdirSync(compile, { recursive: true });
const log = path.join(work, 'build.log');
const logFile = fs.openSync(log, 'w');
const environment = {
  PATH: '/usr/bin:/bin:/usr/sbin:/sbin',
  HOME: process.env.HOME,
  TMPDIR: process.env.TMPDIR ?? '/tmp',
  LANG: 'en_US.UTF-8',
  MACOSX_DEPLOYMENT_TARGET: pin.minimumMacosVersion,
};
const options = { cwd: compile, env: environment, stdio: ['ignore', logFile, logFile], timeout: 300_000 };
try {
  execFileSync(path.join(source, 'configure'), [
    '--prefix=/usr/local', '--sysconfdir=/etc', '--disable-nls', '--disable-rpath',
    '--disable-bsd-shlibs', '--disable-elf-shlibs', '--disable-fuse2fs', '--disable-fsck',
    '--disable-uuidd', '--disable-e2initrd-helper', '--without-libarchive',
    '--with-udev-rules-dir=no', '--with-systemd-unit-dir=no', '--without-crond-dir',
    'MKDIR_P=mkdir -p', 'CC=/usr/bin/clang', 'PKG_CONFIG=/usr/bin/false',
    'CFLAGS=-O2 -arch arm64 -mmacosx-version-min=' + pin.minimumMacosVersion,
    'LDFLAGS=-arch arm64 -mmacosx-version-min=' + pin.minimumMacosVersion,
  ], options);
  execFileSync('/usr/bin/make', ['-j8', 'libs'], options);
  execFileSync('/usr/bin/make', ['-C', 'misc', '-j8', 'mke2fs', 'mke2fs.conf'], options);
  execFileSync('/usr/bin/make', ['-C', 'debugfs', '-j8', 'debugfs'], options);
  execFileSync('/usr/bin/make', ['-C', 'e2fsck', '-j8', 'e2fsck'], options);
} catch (error) {
  throw new Error('Disk tools build failed. See ' + log, { cause: error });
} finally {
  fs.closeSync(logFile);
}

const tools = {};
fs.mkdirSync(path.join(work, 'bin'), { recursive: true });
fs.mkdirSync(path.dirname(configuration), { recursive: true });
for (const [name, relative] of Object.entries(toolSources)) {
  const input = path.join(compile, relative);
  const libraries = execFileSync('/usr/bin/otool', ['-L', input], { encoding: 'utf8', timeout: 10_000 });
  const nonSystem = libraries.split('\n').slice(1).filter(line => line.trim() && !line.trim().startsWith('/usr/lib/') && !line.trim().startsWith('/System/Library/'));
  if (nonSystem.length) throw new Error(name + ' links to non-system libraries: ' + nonSystem.join(', '));
  const build = execFileSync('/usr/bin/vtool', ['-show-build', input], { encoding: 'utf8', timeout: 10_000 });
  if (!build.includes('minos ' + pin.minimumMacosVersion)) throw new Error(name + ' has an unexpected minimum macOS version.');
  const output = path.join(work, 'bin', name);
  fs.copyFileSync(input, output);
  fs.chmodSync(output, 0o755);
  tools[name] = hash(output);
}
fs.copyFileSync(path.join(compile, 'misc/mke2fs.conf'), configuration);
fs.writeFileSync(manifestFile, JSON.stringify({ ...pin, identity, tools, configurationSha256: hash(configuration), libraries: [] }, null, 2) + '\n');
console.log('Prepared e2fsprogs ' + pin.version + ' for macOS ' + pin.minimumMacosVersion + ' with system libraries only.');

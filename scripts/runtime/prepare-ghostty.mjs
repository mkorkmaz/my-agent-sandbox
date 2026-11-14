import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const pin = JSON.parse(fs.readFileSync(path.join(root, 'config/development-versions.json'))).ghostty;
const work = path.join(root, '.build/libghostty');
const install = path.join(work, 'install');
const marker = path.join(install, 'pin.json');
const identity = JSON.stringify({ commit: pin.commit, zig: pin.zig.version, target: 'aarch64-macos.15.0' });
if (process.platform !== 'darwin' || process.arch !== 'arm64') throw new Error('This build requires an Apple silicon Mac.');
const installed = fs.existsSync(marker) && fs.readFileSync(marker, 'utf8') === identity && fs.existsSync(path.join(install, 'lib/libghostty-vt.a'));
fs.mkdirSync(work, { recursive: true });
async function archive(url, expected, directory) {
  if (installed && fs.existsSync(path.join(work, directory))) return;
  const file = path.join(work, new URL(url).pathname.split('/').at(-1));
  if (!fs.existsSync(file)) {
    const response = await fetch(url, { signal: AbortSignal.timeout(180_000) });
    if (!response.ok) throw new Error('Download failed: ' + response.status);
    fs.writeFileSync(file, Buffer.from(await response.arrayBuffer()));
  }
  if (createHash('sha256').update(fs.readFileSync(file)).digest('hex') !== expected) throw new Error('Archive checksum mismatch: ' + path.basename(file));
  execFileSync('/usr/bin/tar', ['-xf', file, '-C', work], { timeout: 180_000 });
}
await archive(pin.source.url, pin.source.sha256, 'libghostty-vt-' + pin.version + '/LICENSE');
await archive(pin.dependencies.url, pin.dependencies.sha256, 'libghostty-vt-' + pin.version + '-deps');
await archive(pin.zig.tarball, pin.zig.shasum, 'zig-aarch64-macos-' + pin.zig.version + '/LICENSE');
if (installed) process.exit(0);
const zig = path.join(work, 'zig-aarch64-macos-' + pin.zig.version, 'zig');
execFileSync(zig, ['build', '--system', '../libghostty-vt-' + pin.version + '-deps', '--prefix', install, '-Demit-lib-vt', '-Demit-xcframework=false', '-Dtarget=aarch64-macos.15.0', '-Dcpu=baseline', '-Doptimize=ReleaseSafe'], {
  cwd: path.join(work, 'libghostty-vt-' + pin.version), stdio: 'inherit',
  env: { ...process.env, GIT_CEILING_DIRECTORIES: fs.realpathSync(work), ZIG_GLOBAL_CACHE_DIR: path.join(work, 'zig-global-cache'), ZIG_LOCAL_CACHE_DIR: path.join(work, 'zig-local-cache') },
});
fs.writeFileSync(marker, identity);
console.log('libghostty-vt ' + pin.version + ' prepared.');

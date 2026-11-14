import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { hashFile, runtimeTools, verifyRuntime } from './runtime-bundle.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const pins = JSON.parse(fs.readFileSync(path.join(root, 'config/brig-runtime.json'), 'utf8'));
const downloads = path.join(root, '.build/brig-runtime-downloads');
const inputs = path.join(root, '.build/brig-runtime-inputs');
const destination = path.join(root, '.build/brig-runtime');
const noSign = process.argv.includes('--no-sign');
const environment = {...process.env, HOME:path.join(inputs, 'verification-home')};
const run = (binary, args, options={}) => execFileSync(binary, args, {timeout:180_000,stdio:'inherit',...options});
if (process.platform !== 'darwin' || process.arch !== 'arm64') throw new Error('The bundled runtime requires an Apple silicon Mac.');
fs.mkdirSync(downloads, {recursive:true});
const lock = path.join(root,'.build/brig-runtime.lock');
run('/usr/bin/shlock',['-p',String(process.pid),'-f',lock]);
process.once('exit', () => {
  if (fs.existsSync(lock) && fs.readFileSync(lock,'utf8').trim() === String(process.pid)) fs.unlinkSync(lock);
});
fs.mkdirSync(environment.HOME, {recursive:true});

function download(url, name, checksum) {
  const file = path.join(downloads, name);
  if (fs.existsSync(file) && (!checksum || hashFile(file) === checksum)) return file;
  const temporary = file + '.partial';
  run('/usr/bin/curl', ['-fsSL', '--retry', '2', '--connect-timeout', '15', '--max-time', '180', url, '-o', temporary]);
  if (checksum && hashFile(temporary) !== checksum) throw new Error('Downloaded SHA-256 mismatch: ' + name);
  fs.renameSync(temporary, file);
  return file;
}

function extract(name) {
  const pin = pins[name];
  const archive = download(pin.url, name + '.tar.gz', pin.sha256);
  const directory = path.join(inputs, name);
  fs.mkdirSync(directory, {recursive:true});
  const entries = execFileSync('/usr/bin/tar', ['-tzf', archive], {encoding:'utf8',timeout:30_000}).trim().split('\n');
  if (entries.some(entry => path.isAbsolute(entry) || entry.split('/').includes('..'))) throw new Error('Unsafe release archive: ' + name);
  run('/usr/bin/tar', ['-xzf', archive, '-C', directory]);
  const url = pin.url.slice(0, pin.url.lastIndexOf('/'));
  const certificate = download(url + '/checksums.txt.pem', name + '.pem');
  const signature = download(url + '/checksums.txt.sig', name + '.sig');
  const checksums = download(url + '/checksums.txt', name + '-checksums.txt');
  run(path.join(downloads, 'cosign'), ['verify-blob', '--certificate', certificate, '--signature', signature,
    '--certificate-identity', pin.signer, '--certificate-oidc-issuer', 'https://token.actions.githubusercontent.com', checksums],
    {env:environment});
  const filename = pin.url.slice(pin.url.lastIndexOf('/') + 1);
  if (!fs.readFileSync(checksums, 'utf8').split('\n').some(line => line.trim() === pin.sha256 + '  ' + filename)) {
    throw new Error('Signed checksums do not cover pinned ' + name + ' archive.');
  }
  download(pin.url + '.sbom.json', name + '-sbom.json', pin.sbomSha256);
  return directory;
}

download(pins.cosign.url, 'cosign', pins.cosign.sha256);
fs.chmodSync(path.join(downloads, 'cosign'), 0o755);
extract('brig');
const hull = extract('hull');
run(process.execPath, [path.join(root, 'scripts/runtime/prepare-hull.mjs'), ...(noSign ? ['--no-sign'] : [])]);
run(process.execPath, [path.join(root, 'scripts/runtime/prepare-brig.mjs'), ...(noSign ? ['--no-sign'] : [])]);
run(process.execPath, [path.join(root, 'scripts/runtime/prepare-disk-tools.mjs')]);
const temporary = fs.mkdtempSync(path.join(root, '.build/brig-runtime-stage-'));
try {
  const bin = path.join(temporary, 'bin');
  fs.mkdirSync(bin);
  for (const name of runtimeTools) {
    let source = path.join(root, '.build/brig-compat/bin', name);
    if (name === 'hull') source = path.join(root, '.build/hull-compat/bin/hull');
    if (['hvi', 'vz-runner'].includes(name)) source = path.join(hull, name);
    if (name === 'cosign') source = path.join(downloads, name);
    if (['mke2fs', 'debugfs', 'e2fsck'].includes(name)) source = path.join(root, '.build/disk-tools/bin', name);
    fs.copyFileSync(source, path.join(bin, name));
    fs.chmodSync(path.join(bin, name), 0o755);
    if (!noSign && process.env.APPLE_SIGNING_IDENTITY && !['hvi','vz-runner'].includes(name)) {
      run('/usr/bin/codesign', ['--force','--timestamp','--options','runtime','--sign',process.env.APPLE_SIGNING_IDENTITY,path.join(bin,name)]);
    }
  }
  fs.copyFileSync(path.join(root, '.build/disk-tools/etc/mke2fs.conf'), path.join(temporary, 'mke2fs.conf'));
  fs.mkdirSync(path.join(temporary, 'registry'));
  fs.copyFileSync(path.join(root, 'resources/runtime/registry/config.json'), path.join(temporary, 'registry/config.json'));
  const manifest = {
    schemaVersion:1, brigVersion:pins.brig.version, sources:pins,
    hull:JSON.parse(fs.readFileSync(path.join(root, '.build/hull-compat/bin/build.json'), 'utf8')),
    brig:JSON.parse(fs.readFileSync(path.join(root, '.build/brig-compat/bin/build.json'), 'utf8')),
    diskTools:JSON.parse(fs.readFileSync(path.join(root, 'config/disk-tools.json'), 'utf8')),
    tools:Object.fromEntries(runtimeTools.map(name => [name, {sha256:hashFile(path.join(bin, name))}])),
    libraries:[], configuration:Object.fromEntries(['mke2fs.conf', 'registry/config.json'].map(name => [name, {sha256:hashFile(path.join(temporary, name))}])),
  };
  fs.writeFileSync(path.join(temporary, 'build.json'), JSON.stringify(manifest, null, 2) + '\n');
  verifyRuntime(bin, temporary);
  const previous = destination + '.previous';
  fs.rmSync(previous, {recursive:true,force:true});
  if (fs.existsSync(destination)) fs.renameSync(destination, previous);
  try { fs.renameSync(temporary, destination); }
  catch (error) {
    if (fs.existsSync(previous)) fs.renameSync(previous, destination);
    throw error;
  }
  fs.rmSync(previous, {recursive:true,force:true});
  console.log('Prepared the complete Brig runtime in ' + destination);
} finally {
  fs.rmSync(temporary, {recursive:true,force:true});
}

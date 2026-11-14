import fs from 'node:fs';
import os from 'node:os';
import { createHash } from 'node:crypto';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import { verifyRuntime } from '../runtime/runtime-bundle.mjs';
import { verifyPackagedLicenses } from '../licenses/license-notices.mjs';

const run = (program, args) => execFileSync(program, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });

export function verifyDeveloperId(file) {
  const requirement = 'anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists';
  run('/usr/bin/codesign', ['--verify', '--strict', '-R', '=' + requirement, file]);
}

export function verifyPackagedHull(app) {
  const bin = path.join(app, 'Contents/MacOS');
  const runtime = path.join(app, 'Contents/Resources/runtime');
  const manifest = JSON.parse(fs.readFileSync(path.join(runtime, 'build.json'), 'utf8'));
  const digest = createHash('sha256').update(fs.readFileSync(path.join(bin, 'hull'))).digest('hex');
  if (digest !== manifest.tools.hull.sha256) throw new Error('Packaged hull does not match the SHA-256 in runtime/build.json.');
}

export function verifyPackagedVersion(app, version) {
  const plist = path.join(app, 'Contents/Info.plist');
  const actual = run('/usr/bin/plutil', ['-extract', 'CFBundleShortVersionString', 'raw', plist]).trim();
  if (actual !== version) throw new Error(`Bundle version ${actual} does not match config version ${version}.`);
  const executable = run('/usr/bin/plutil', ['-extract', 'CFBundleExecutable', 'raw', plist]).trim();
  if (!executable || path.basename(executable) !== executable) throw new Error('Invalid bundle executable.');
  const metadata = execFileSync(path.join(app, 'Contents/MacOS', executable), ['--package-version'], { encoding: 'utf8', timeout: 10_000 }).trim();
  if (metadata !== version) throw new Error(`Settings/About metadata ${metadata} does not match config version ${version}.`);
  return metadata;
}

function main() {
  const [installerPath, version] = process.argv.slice(2);
  if (!installerPath || !version) throw new Error('Usage: node scripts/release/verify-macos-release.mjs <dmg> <version>');
  const installer = fs.realpathSync(installerPath);
  run('/usr/bin/hdiutil', ['verify', installer]);
  verifyDeveloperId(installer);
  run('/usr/bin/xcrun', ['stapler', 'validate', installer]);
  run('/usr/sbin/spctl', ['--assess', '--type', 'open', '--context', 'context:primary-signature', '--verbose=4', installer]);
  const mount = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-release-'));
  let mounted = false;
  try {
    run('/usr/bin/hdiutil', ['attach', '-readonly', '-nobrowse', '-mountpoint', mount, installer]);
    mounted = true;
    const app = path.join(mount, 'Tugboat.app');
    run('/usr/bin/codesign', ['--verify', '--deep', '--strict', app]);
    verifyDeveloperId(app);
    const manifest = verifyRuntime(path.join(app, 'Contents/MacOS'), path.join(app, 'Contents/Resources/runtime'));
    verifyPackagedLicenses(app, manifest.diskTools);
    for (const name of ['brig','brigd','brig-broker','hull','cosign','mke2fs','debugfs','e2fsck']) verifyDeveloperId(path.join(app, 'Contents/MacOS', name));
    run('/usr/bin/xcrun', ['stapler', 'validate', app]);
    run('/usr/sbin/spctl', ['--assess', '--type', 'execute', '--verbose=4', app]);
    verifyPackagedHull(app);
    verifyPackagedVersion(app, version);
    console.log(`Verified Tugboat ${version}: Developer ID, notarization and Gatekeeper passed on the mounted installer.`);
  } finally {
    if (mounted) run('/usr/bin/hdiutil', ['detach', mount]);
    fs.rmSync(mount, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) main();

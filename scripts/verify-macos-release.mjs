import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const run = (program, args) => execFileSync(program, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });

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
  if (!installerPath || !version) throw new Error('Usage: node scripts/verify-macos-release.mjs <dmg> <version>');
  const installer = fs.realpathSync(installerPath);
  run('/usr/bin/hdiutil', ['verify', installer]);
  run('/usr/bin/xcrun', ['stapler', 'validate', installer]);
  const mount = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-release-'));
  let mounted = false;
  try {
    run('/usr/bin/hdiutil', ['attach', '-readonly', '-nobrowse', '-mountpoint', mount, installer]);
    mounted = true;
    const app = path.join(mount, 'Tugboat.app');
    run('/usr/bin/codesign', ['--verify', '--deep', '--strict', app]);
    for (const binary of [app, path.join(app, 'Contents/Resources/runtime/hull')]) {
      run('/usr/bin/codesign', ['--verify', '--strict', '-R', 'anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists', binary]);
    }
    run('/usr/bin/xcrun', ['stapler', 'validate', app]);
    run('/usr/sbin/spctl', ['--assess', '--type', 'execute', '--verbose=4', app]);
    verifyPackagedVersion(app, version);
    console.log(`Verified Tugboat ${version}: Developer ID, notarization and Gatekeeper passed on the mounted installer.`);
  } finally {
    if (mounted) run('/usr/bin/hdiutil', ['detach', mount]);
    fs.rmSync(mount, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) main();

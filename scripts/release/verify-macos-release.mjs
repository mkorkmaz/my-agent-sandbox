import fs from 'node:fs';
import os from 'node:os';
import { createHash } from 'node:crypto';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import { verifyRuntime } from '../runtime/runtime-bundle.mjs';
import { verifyPackagedLicenses } from '../licenses/license-notices.mjs';
import { fileHash, verifyUpdaterArchive } from './macos-release-assets.mjs';

const run = (program, args) => execFileSync(program, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 180_000 });

export function verifyDeveloperId(file, team) {
  const developerId = 'anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists';
  const requirement = team ? `${developerId} and certificate leaf[subject.OU] = "${team}"` : developerId;
  run('/usr/bin/codesign', ['--verify', '--strict', '-R', '=' + requirement, file]);
}

export function verifyPackagedHull(app) {
  const bin = path.join(app, 'Contents/MacOS');
  const runtime = path.join(app, 'Contents/Resources/runtime');
  const manifest = JSON.parse(fs.readFileSync(path.join(runtime, 'build.json'), 'utf8'));
  const digest = createHash('sha256').update(fs.readFileSync(path.join(bin, 'hull'))).digest('hex');
  if (digest !== manifest.tools.hull.sha256) throw new Error('Packaged hull does not match the SHA-256 in runtime/build.json.');
}

export function verifyPackagedVersion(app, version, publicKey, buildNumber) {
  const plist = path.join(app, 'Contents/Info.plist');
  const actual = run('/usr/bin/plutil', ['-extract', 'CFBundleShortVersionString', 'raw', plist]).trim();
  if (actual !== version) throw new Error(`Bundle version ${actual} does not match config version ${version}.`);
  const executable = run('/usr/bin/plutil', ['-extract', 'CFBundleExecutable', 'raw', plist]).trim();
  if (!executable || path.basename(executable) !== executable) throw new Error('Invalid bundle executable.');
  const binary = path.join(app, 'Contents/MacOS', executable);
  if (run('/usr/bin/lipo', ['-archs', binary]).trim() !== 'arm64') throw new Error('The Tugboat executable must be arm64.');
  const metadata = execFileSync(binary, ['--package-version'], { encoding: 'utf8', timeout: 10_000 }).trim();
  if (metadata !== version) throw new Error(`Settings/About metadata ${metadata} does not match config version ${version}.`);
  if (publicKey || buildNumber !== undefined) {
    const updater = JSON.parse(run(binary, ['--package-update-config']));
    const endpoint = 'https://github.com/reformo/tugboat/releases/latest/download/latest.json';
    if (publicKey && (updater.pubkey !== publicKey || updater.requireSignedVersion !== true ||
        !Array.isArray(updater.endpoints) || updater.endpoints.length !== 1 || updater.endpoints[0] !== endpoint)) {
      throw new Error('The app must embed the release updater key, signed-version check, and production feed.');
    }
    if (buildNumber !== undefined && updater.buildNumber !== buildNumber) {
      throw new Error(`The app must embed release build number ${buildNumber}.`);
    }
  }
  return metadata;
}

function verifyApp(app, version, team, publicKey, buildNumber) {
  const plist = path.join(app, 'Contents/Info.plist');
  if (run('/usr/bin/plutil', ['-extract', 'CFBundleIdentifier', 'raw', plist]).trim() !== 'sh.brig.desktop') {
    throw new Error('Expected the production Tugboat bundle identifier.');
  }
  if (run('/usr/bin/plutil', ['-extract', 'LSMinimumSystemVersion', 'raw', plist]).trim() !== '15.0') {
    throw new Error('Expected a macOS 15.0 minimum version.');
  }
  run('/usr/bin/codesign', ['--verify', '--deep', '--strict', app]);
  verifyDeveloperId(app, team);
  const manifest = verifyRuntime(path.join(app, 'Contents/MacOS'), path.join(app, 'Contents/Resources/runtime'));
  verifyPackagedLicenses(app, manifest.diskTools);
  for (const name of ['brig','brigd','brig-broker','hull','cosign','mke2fs','debugfs','e2fsck']) verifyDeveloperId(path.join(app, 'Contents/MacOS', name), team);
  run('/usr/bin/xcrun', ['stapler', 'validate', app]);
  run('/usr/sbin/spctl', ['--assess', '--type', 'execute', '--verbose=4', app]);
  verifyPackagedHull(app);
  verifyPackagedVersion(app, version, publicKey, buildNumber);
}

async function bundleHash(app) {
  const hash = createHash('sha256');
  async function addEntry(relative) {
    const entry = path.join(app, relative);
    const status = fs.lstatSync(entry);
    if (status.isDirectory()) {
      hash.update(JSON.stringify([relative, 'directory']));
      for (const name of fs.readdirSync(entry).sort()) await addEntry(path.join(relative, name));
      return;
    }
    if (status.isSymbolicLink()) {
      hash.update(JSON.stringify([relative, 'link', fs.readlinkSync(entry)]));
      return;
    }
    if (!status.isFile()) throw new Error(`Unexpected file type in Tugboat.app: ${relative}.`);
    const digest = (await fileHash(entry, 'sha256')).toString('hex');
    hash.update(JSON.stringify([relative, 'file', status.mode & 0o111, digest]));
  }
  await addEntry('');
  return hash.digest('hex');
}

export async function verifyMatchingBundles(installedApp, updatedApp) {
  // macOS allows nested code with the same designated requirement under an unchanged outer seal.
  if (await bundleHash(installedApp) !== await bundleHash(updatedApp)) {
    throw new Error('The disk image and updater archive must contain the same Tugboat app build.');
  }
}

export async function verifyMacosRelease(installerPath, archivePath, version, buildNumber) {
  const team = /^Developer ID Application: .+ \(([A-Z0-9]{10})\)$/.exec(process.env.APPLE_SIGNING_IDENTITY ?? '')?.[1];
  if (!team) throw new Error('Set APPLE_SIGNING_IDENTITY to a Developer ID Application identity with its Team ID.');
  const publicKey = process.env.TAURI_UPDATER_PUBLIC_KEY?.trim();
  if (!publicKey) throw new Error('Set TAURI_UPDATER_PUBLIC_KEY to the Tauri updater public key.');
  const archive = fs.realpathSync(archivePath);
  await verifyUpdaterArchive(archive, version, publicKey);
  const installer = fs.realpathSync(installerPath);
  run('/usr/bin/hdiutil', ['verify', installer]);
  verifyDeveloperId(installer, team);
  run('/usr/bin/xcrun', ['stapler', 'validate', installer]);
  run('/usr/sbin/spctl', ['--assess', '--type', 'open', '--context', 'context:primary-signature', '--verbose=4', installer]);
  const mount = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-release-'));
  let mounted = false;
  try {
    run('/usr/bin/hdiutil', ['attach', '-readonly', '-nobrowse', '-mountpoint', mount, installer]);
    mounted = true;
    const installedApp = path.join(mount, 'Tugboat.app');
    verifyApp(installedApp, version, team, publicKey, buildNumber);
    const extracted = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-updater-'));
    try {
      run('/usr/bin/tar', ['-xpzf', archive, '-C', extracted]);
      const updatedApp = path.join(extracted, 'Tugboat.app');
      verifyApp(updatedApp, version, team, publicKey, buildNumber);
      await verifyMatchingBundles(installedApp, updatedApp);
    } finally {
      fs.rmSync(extracted, { recursive: true, force: true });
    }
  } finally {
    if (mounted) run('/usr/bin/hdiutil', ['detach', mount]);
    fs.rmSync(mount, { recursive: true, force: true });
  }
  console.log(`Verified Tugboat ${version} in the disk image and the signed updater archive.`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  const [installer, archive, version, buildNumber] = process.argv.slice(2);
  if (!installer || !archive || !version) throw new Error('Usage: node scripts/release/verify-macos-release.mjs <dmg> <app.tar.gz> <version> [build number]');
  await verifyMacosRelease(installer, archive, version, buildNumber);
}

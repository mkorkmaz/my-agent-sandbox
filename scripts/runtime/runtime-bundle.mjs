import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';

export const runtimeTools = ['brig', 'brigd', 'brig-broker', 'hull', 'hvi', 'vz-runner', 'cosign', 'mke2fs', 'debugfs', 'e2fsck'];
export const hashFile = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const run = (binary, args) => execFileSync(binary, args, {encoding:'utf8', timeout:30_000, stdio:['ignore','pipe','pipe']});

export function verifyNativeTool(file) {
  if (!fs.lstatSync(file).isFile() || !(fs.statSync(file).mode & 0o111)) throw new Error('Runtime tool is not executable: ' + file);
  const architecture = run('/usr/bin/lipo', ['-archs', file]).trim();
  if (architecture !== 'arm64') throw new Error('Runtime tool must be arm64: ' + file);
  const build = run('/usr/bin/vtool', ['-show-build', file]);
  const minimum = build.match(/\bminos\s+(\d+)(?:\.(\d+))?/);
  if (!minimum || Number(minimum[1]) > 15 || (Number(minimum[1]) === 15 && Number(minimum[2]) > 0)) {
    throw new Error('Runtime tool requires a newer macOS version: ' + file);
  }
  const libraries = run('/usr/bin/otool', ['-L', file]).trim().split('\n').slice(1).map(line => line.trim().split(' (')[0]);
  for (const library of libraries) {
    if (!library.startsWith('/usr/lib/') && !library.startsWith('/System/Library/')) throw new Error('Runtime tool has an external library: ' + library);
  }
}

export function verifyRunner(file, entitlement) {
  run('/usr/bin/codesign', ['--verify', '--strict', file]);
  run('/usr/bin/codesign', ['--verify', '--strict', '-R', '=anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists', file]);
  const xml = run('/usr/bin/codesign', ['-d', '--entitlements', '-', '--xml', file]);
  if (!new RegExp('<key>' + entitlement.replaceAll('.', '\\.') + '</key>\\s*<true\\s*/>').test(xml)) {
    throw new Error('Runner entitlement is missing or disabled: ' + file);
  }
}

export function verifyRuntime(bin, resources) {
  const manifest = JSON.parse(fs.readFileSync(path.join(resources, 'build.json'), 'utf8'));
  if (manifest.schemaVersion !== 1 || manifest.brigVersion !== 'v0.5.2') throw new Error('Unsupported bundled runtime manifest.');
  if (JSON.stringify(Object.keys(manifest.tools).sort()) !== JSON.stringify([...runtimeTools].sort())) throw new Error('Incomplete runtime inventory.');
  for (const name of runtimeTools) {
    const file = path.join(bin, name);
    if (hashFile(file) !== manifest.tools[name].sha256) throw new Error('Runtime SHA-256 mismatch: ' + name);
    verifyNativeTool(file);
  }
  for (const name of ['mke2fs.conf', 'registry/config.json']) {
    if (hashFile(path.join(resources, name)) !== manifest.configuration[name].sha256) throw new Error('Runtime SHA-256 mismatch: ' + name);
  }
  verifyRunner(path.join(bin, 'hvi'), 'com.apple.security.hypervisor');
  verifyRunner(path.join(bin, 'vz-runner'), 'com.apple.security.virtualization');
  return manifest;
}

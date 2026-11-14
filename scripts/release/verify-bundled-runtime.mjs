import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { verifyRuntime } from '../runtime/runtime-bundle.mjs';
import { verifyPackagedLicenses } from '../licenses/license-notices.mjs';

const [artifact] = process.argv.slice(2);
if (!artifact) throw new Error('Usage: node scripts/release/verify-bundled-runtime.mjs <Tugboat.app or DMG>');
const file = fs.realpathSync(artifact);
const run = args => execFileSync('/usr/bin/hdiutil',args,{timeout:120_000,stdio:'inherit'});
const mount = fs.statSync(file).isFile() ? fs.mkdtempSync(path.join(os.tmpdir(),'tugboat-package-')) : null;
let mounted = false;
try {
  if (mount) {
    run(['verify',file]);
    run(['attach','-readonly','-nobrowse','-mountpoint',mount,file]);
    mounted = true;
  }
  const app = mount ? path.join(mount,'Tugboat.app') : file;
  const manifest = verifyRuntime(path.join(app,'Contents/MacOS'),path.join(app,'Contents/Resources/runtime'));
  verifyPackagedLicenses(app, manifest.diskTools);
  console.log('Verified '+Object.keys(manifest.tools).length+' bundled tools, their hashes, macOS deployment targets and signed runners.');
} finally {
  if (mounted) run(['detach',mount]);
  if (mount) fs.rmdirSync(mount);
}

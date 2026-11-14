import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { verifyDeveloperId, verifyPackagedHull } from '../../scripts/release/verify-macos-release.mjs';
import { execFileSync, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const root = fileURLToPath(new URL('../../', import.meta.url));

test('codesign evaluates the Developer ID requirement against an ad hoc signature', {skip: process.platform !== 'darwin'}, t => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-signature-test-'));
  t.after(() => fs.rmSync(temp, {recursive: true, force: true}));
  const binary = path.join(temp, 'fixture');
  fs.copyFileSync('/usr/bin/true', binary);
  fs.chmodSync(binary, 0o700);
  execFileSync('/usr/bin/codesign', ['--force', '--sign', '-', binary], {stdio: 'pipe'});
  execFileSync('/usr/bin/codesign', ['--verify', '--strict', binary]);
  assert.throws(() => verifyDeveloperId(binary), error => error.status === 3);
});

test('release restores the original keychain list on success and at each failure stage', {skip: process.platform !== 'darwin'}, t => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-release-test-'));
  t.after(() => fs.rmSync(temp, {recursive:true, force:true}));
  const bin = path.join(temp, 'bin');
  fs.mkdirSync(bin);
  const stub = (name, script) => fs.writeFileSync(path.join(bin, name), '#!/bin/bash\nset -eu\n'+script, { mode: 0o755 });
  stub('security', `case "$1" in
    list-keychains)
      if [[ "$#" == 3 ]]; then
        while IFS= read -r item; do printf '    "%s"\\n' "$item"; done < "$KEYCHAIN_STATE"
      else
        shift 4
        printf '%s\\n' "$@" > "$KEYCHAIN_STATE"
      fi ;;
    create-keychain) echo "$4" >> "$KEYCHAIN_STATE" ;;
    import) [[ "$FAIL_STAGE" != import ]] ;;
  esac`);
  stub('npm', `[[ "$FAIL_STAGE" != "$2" ]] || exit 1
    if [[ "$2" == tauri ]]; then
      [[ -z "\${APPLE_CERTIFICATE+x}" && -z "\${APPLE_CERTIFICATE_PASSWORD+x}" && -z "\${APPLE_API_KEY_CONTENT+x}" ]] || { echo 'Tauri would import the certificate again.' >&2; exit 1; }
      [[ "$APPLE_SIGNING_IDENTITY" == 'Developer ID Application: Test' && -s "$APPLE_API_KEY_PATH" ]] || exit 1
    fi`);
  stub('xcrun', '[[ "$FAIL_STAGE" != "$1" ]] && echo \'{"status":"Accepted"}\'');
  fs.mkdirSync(path.join(temp, 'src-tauri'));
  fs.writeFileSync(path.join(temp, 'src-tauri/tauri.conf.json'), '{"version":"1"}');
  const runtime = path.join(temp,'.build/brig-runtime');
  fs.mkdirSync(runtime,{recursive:true});
  fs.writeFileSync(path.join(runtime,'hull'),'hull');
  const hullSha256 = createHash('sha256').update('hull').digest('hex');
  fs.writeFileSync(path.join(runtime,'build.json'),JSON.stringify({tools:{hull:{sha256:hullSha256}},patchSha256:'original-patch'}));
  const state = path.join(temp, 'original');
  const original = '/fixtures/Team Signing.keychain-db\n/fixtures/personal.keychain-db\n';
  for (const stage of ['import','native:prepare','tauri','notarytool','success']) {
    fs.writeFileSync(state, original);
    const result = spawnSync('/bin/bash', [path.join(root,'scripts/build-macos-release.sh')], {cwd:temp,encoding:'utf8',env:{
      PATH:bin+':'+process.env.PATH, TMPDIR:temp, KEYCHAIN_STATE:state, FAIL_STAGE:stage,
      APPLE_CERTIFICATE:'Zml4dHVyZQ==', APPLE_CERTIFICATE_PASSWORD:'fixture', APPLE_SIGNING_IDENTITY:'Developer ID Application: Test',
      APPLE_API_KEY:'fixture', APPLE_API_ISSUER:'fixture', APPLE_API_KEY_CONTENT:'fixture',
    }});
    assert.equal(result.status === 0, stage === 'success', stage + ': ' + result.stderr);
    assert.equal(fs.readFileSync(state,'utf8'),original,stage);
  }
  const resources = path.join(temp, 'Tugboat.app/Contents/Resources');
  fs.mkdirSync(resources,{recursive:true});
  fs.cpSync(runtime,path.join(resources,'runtime'),{recursive:true});
  const app = path.join(temp,'Tugboat.app');
  fs.mkdirSync(path.join(app,'Contents/MacOS'),{recursive:true});
  fs.copyFileSync(path.join(runtime,'hull'),path.join(app,'Contents/MacOS/hull'));
  assert.equal(JSON.parse(fs.readFileSync(path.join(runtime,'build.json'))).patchSha256,'original-patch');
  verifyPackagedHull(app);
  fs.appendFileSync(path.join(app,'Contents/MacOS/hull'),'changed');
  assert.throws(() => verifyPackagedHull(app), /SHA-256/);
});

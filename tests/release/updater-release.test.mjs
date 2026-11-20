import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { createHash, generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import {
  assetNames, assertNewVersion, releaseManifest, verifyReleaseManifest, verifyUpdaterArchive,
} from '../../scripts/release/macos-release-assets.mjs';
import { publishMacosRelease } from '../../scripts/release/publish-macos-release.mjs';

const fixtureVersion = '0.4.2';
const fixtureTag = 'v0.4.2-build.18';
const root = fileURLToPath(new URL('../../', import.meta.url));

function temporaryDirectory(context) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-updater-test-'));
  context.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  return directory;
}

test('the verifier checks the updater archive bytes, signing key and trusted version', async context => {
  const archive = path.join(temporaryDirectory(context), 'Tugboat.app.tar.gz');
  const payload = Buffer.from('archive fixture');
  const keyId = randomBytes(8);
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const rawKey = publicKey.export({format: 'der', type: 'spki'}).subarray(-32);
  const minisignKey = Buffer.concat([Buffer.from('Ed'), keyId, rawKey]).toString('base64');
  const encodedKey = Buffer.from('untrusted comment: fixture\n' + minisignKey + '\n').toString('base64');
  const signature = sign(null, createHash('blake2b512').update(payload).digest(), privateKey);
  const minisignSignature = Buffer.concat([Buffer.from('ED'), keyId, signature]).toString('base64');
  const trustedComment = 'timestamp:0\tfile:Tugboat.app.tar.gz\tversion:0.4.2';
  const signedComment = sign(null, Buffer.concat([signature, Buffer.from(trustedComment)]), privateKey).toString('base64');
  const signatureFile = comment =>
    Buffer.from(['untrusted comment: fixture', minisignSignature, 'trusted comment: ' + comment, signedComment, ''].join('\n')).toString('base64');
  fs.writeFileSync(archive, payload);
  fs.writeFileSync(archive + '.sig', signatureFile(trustedComment));
  await verifyUpdaterArchive(archive, fixtureVersion, encodedKey);
  const wrongKey = generateKeyPairSync('ed25519').publicKey.export({format: 'der', type: 'spki'}).subarray(-32);
  const wrongMinisignKey = Buffer.concat([Buffer.from('Ed'), keyId, wrongKey]).toString('base64');
  const wrongEncodedKey = Buffer.from('untrusted comment: fixture\n' + wrongMinisignKey + '\n').toString('base64');
  await assert.rejects(verifyUpdaterArchive(archive, fixtureVersion, wrongEncodedKey), /Invalid Tauri updater archive signature/);
  await assert.rejects(verifyUpdaterArchive(archive, '0.4.3', encodedKey), /signature does not bind/);
  fs.writeFileSync(archive + '.sig', signatureFile(trustedComment.replace('0.4.2', '0.4.3')));
  await assert.rejects(verifyUpdaterArchive(archive, '0.4.3', encodedKey), /Invalid Tauri updater archive signature/);
  fs.writeFileSync(archive + '.sig', signatureFile(trustedComment));
  fs.truncateSync(archive, payload.length - 1);
  await assert.rejects(verifyUpdaterArchive(archive, fixtureVersion, encodedKey), /Invalid Tauri updater archive signature/);
});

test('the verifier accepts versioned Tauri signatures with and without a key password', { skip: process.platform !== 'darwin' }, async context => {
  const tauri = (args, password) => execFileSync(process.execPath, [path.join(root, 'node_modules/@tauri-apps/cli/tauri.js'), 'signer', ...args], {
    cwd: root, env: { PATH: process.env.PATH, CI: 'true', TAURI_SIGNING_PRIVATE_KEY_PASSWORD: password }, stdio: 'pipe', timeout: 30_000,
  });
  for (const password of ['', 'disposable-fixture']) {
    const directory = temporaryDirectory(context);
    const keyFile = path.join(directory, 'updater.key');
    const archive = path.join(directory, 'Tugboat.app.tar.gz');
    fs.writeFileSync(archive, 'archive fixture');
    const options = password ? ['--password', password] : ['--ci'];
    tauri(['generate', '--write-keys', keyFile, ...options]);
    tauri(['sign', '--private-key-path', keyFile, '--app-version', fixtureVersion, archive], password);
    await verifyUpdaterArchive(archive, fixtureVersion, fs.readFileSync(keyFile + '.pub', 'utf8').trim());
  }
});

test('new public versions advance stable bundle version independently of build number', () => {
  assert.throws(() => assetNames('0.4.2-rc.1'), /stable version/);
  assert.throws(() => assetNames('0.4.2+build.18'), /stable version/);
  assert.doesNotThrow(() => assertNewVersion('0.4.10', ['v0.4.2-build.20', 'v0.3.0']));
  assert.throws(() => assertNewVersion('0.4.2', ['v0.4.2-build.17']), /must be higher/);
  assert.throws(() => assertNewVersion('0.4.2', ['v0.4.3-build.17', 'v0.4.1-build.18']), /must be higher/);
});

test('the feed matches the tag-specific archive and exact signature file contents', context => {
  const directory = temporaryDirectory(context);
  const signature = 'encoded-signature';
  fs.writeFileSync(path.join(directory, 'Tugboat.app.tar.gz.sig'), signature);
  const file = path.join(directory, 'latest.json');
  fs.writeFileSync(file, JSON.stringify(releaseManifest(fixtureVersion, fixtureTag, 'Fixture notes', signature)));
  verifyReleaseManifest(directory, fixtureVersion, fixtureTag);
  assert.throws(() => verifyReleaseManifest(directory, fixtureVersion, 'v0.4.2-build.19'), /archive URL/);
  fs.writeFileSync(file, JSON.stringify(releaseManifest(fixtureVersion, fixtureTag, 'Fixture notes', 'different-signature')));
  assert.throws(() => verifyReleaseManifest(directory, fixtureVersion, fixtureTag), /signature file/);
});

test('publication checks the commit and complete draft assets, and verifies a public retry without replacing assets', async context => {
  const directory = temporaryDirectory(context);
  const commit = 'a'.repeat(40);
  const names = assetNames(fixtureVersion);
  for (const name of names.slice(0, 3)) fs.writeFileSync(path.join(directory, name), name);
  const args = { repository: 'reformo/tugboat', version: fixtureVersion, buildNumber: '18', commit, directory };
  const oldRelease = { tag_name: 'v0.4.0-build.16', draft: false, prerelease: false };
  const release = { id: 18, tag_name: fixtureTag, target_commitish: commit, draft: true, prerelease: false, body: 'Fixture notes' };
  let draftCreated = false;
  let assets = [];
  let publishCount = 0;
  let uploadCount = 0;
  let conflict = true;
  let omitAsset = true;
  const github = command => {
    if (command.includes('repos/reformo/tugboat/releases?per_page=100')) return JSON.stringify([draftCreated ? [oldRelease, release] : [oldRelease]]);
    if (command.includes('repos/reformo/tugboat/releases/18/assets?per_page=100')) return JSON.stringify([omitAsset ? assets.slice(0, 3) : assets]);
    if (command.includes('repos/reformo/tugboat/commits/' + commit)) return JSON.stringify({ sha: commit });
    if (command[1]?.includes('/git/matching-refs/')) return JSON.stringify(release.draft && !conflict ? [] : [
      { ref: 'refs/tags/' + fixtureTag, object: { type: 'commit', sha: conflict ? 'b'.repeat(40) : commit } },
    ]);
    if (command[1] === 'repos/reformo/tugboat/releases/18') return JSON.stringify(release);
    if (command[1] === 'upload') {
      uploadCount++;
      assets = names.map(name => ({name, state: 'uploaded', digest: 'sha256:' + createHash('sha256').update(fs.readFileSync(path.join(directory, name))).digest('hex')}));
      return '';
    }
    if (command[1] === 'download') {
      for (const name of names) fs.copyFileSync(path.join(directory, name), path.join(command[6], name));
      return '';
    }
    if (command[2] === 'POST') {
      assert.equal(draftCreated, false);
      assert.ok(command.includes('target_commitish=' + commit));
      assert.ok(command.includes('draft=true'));
      draftCreated = true;
      return JSON.stringify(release);
    }
    if (command[2] === 'PATCH') {
      assert.ok(command.includes('make_latest=true'));
      release.draft = false;
      publishCount++;
      return JSON.stringify(release);
    }
    assert.fail('Unexpected GitHub command ' + command.join(' '));
  };
  const verifyPackage = (installer, archive, version) => {
    assert.equal(version, fixtureVersion);
    assert.equal(fs.readFileSync(installer, 'utf8'), names[0]);
    assert.equal(fs.readFileSync(archive, 'utf8'), names[1]);
  };
  await assert.rejects(publishMacosRelease(args, github, verifyPackage), /does not identify commit/);
  assert.equal(uploadCount, 0);
  conflict = false;
  await assert.rejects(publishMacosRelease(args, github, verifyPackage), /exactly four release assets/);
  assert.equal(publishCount, 0);
  omitAsset = false;
  await publishMacosRelease(args, github, verifyPackage);
  await publishMacosRelease(args, github, verifyPackage);
  assert.equal(publishCount, 1);
  assert.equal(uploadCount, 2);
  assets[1].digest = 'sha256:invalid';
  await assert.rejects(publishMacosRelease(args, github, verifyPackage), /does not match the verified local file/);
  assert.equal(publishCount, 1);
  assert.equal(uploadCount, 2);
});

import test from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateAppleApiKeyContent } from '../../scripts/release/validate-apple-api-key.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const { privateKey } = generateKeyPairSync('ec', {
  namedCurve: 'prime256v1',
  privateKeyEncoding: { type: 'pkcs8', format: 'pem' },
  publicKeyEncoding: { type: 'spki', format: 'pem' },
});

test('notarization accepts raw P-256 PKCS#8 keys with Unix or Windows line breaks', () => {
  for (const keyContent of [privateKey, privateKey.trimEnd(), privateKey.replaceAll('\n', '\r\n')]) {
    assert.doesNotThrow(() => validateAppleApiKeyContent(keyContent));
  }
});

test('notarization rejects keys with another algorithm or curve', () => {
  for (const [algorithm, options] of [['ed25519', {}], ['ec', { namedCurve: 'secp384r1' }]]) {
    const keyPair = generateKeyPairSync(algorithm, { ...options, privateKeyEncoding: { type: 'pkcs8', format: 'pem' } });
    assert.throws(() => validateAppleApiKeyContent(keyPair.privateKey), /App Store Connect P-256 private key/);
  }
});

test('missing notarization keys fail without exposing key contents', () => {
  for (const keyContent of [undefined, null, '']) {
    assert.throws(() => validateAppleApiKeyContent(keyContent), /APPLE_API_KEY_CONTENT/);
  }
});

test('notarization validation reports malformed keys without logging secret contents', () => {
  const keyContent = '-----BEGIN PRIVATE KEY-----\nSecretMarkerInvalidBase64\n-----END PRIVATE KEY-----\n';
  const result = spawnSync(process.execPath, [path.join(root, 'scripts/release/validate-apple-api-key.mjs')], {
    encoding: 'utf8', timeout: 5000, env: { APPLE_API_KEY_CONTENT: keyContent },
  });
  assert.ifError(result.error);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /APPLE_API_KEY_CONTENT/);
  assert.equal(result.stderr.includes('SecretMarkerInvalidBase64'), false);
  assert.equal(result.stdout, '');
});

test('invalid notarization keys stop release before keychain access', context => {
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-invalid-api-key-'));
  context.after(() => fs.rmSync(workspace, { recursive: true, force: true }));
  const commands = path.join(workspace, 'commands');
  fs.mkdirSync(commands);
  const accessed = path.join(workspace, 'keychain-accessed');
  fs.writeFileSync(path.join(commands, 'security'), '#!/bin/bash\nprintf accessed > "$KEYCHAIN_ACCESSED"\nexit 1\n', { mode: 0o755 });
  const invalidKeys = [
    'fixture-invalid-private-key',
    Buffer.from(privateKey).toString('base64'),
    privateKey.replaceAll('\n', '\\n'),
    privateKey.replace('-----BEGIN PRIVATE KEY-----\n', ''),
    '-----BEGIN PRIVATE KEY-----\ninvalid\n-----END PRIVATE KEY-----\n',
  ];
  for (const keyContent of invalidKeys) {
    const result = spawnSync('/bin/bash', [path.join(root, 'scripts/build-macos-release.sh')], {
      cwd: workspace, encoding: 'utf8', timeout: 5000,
      env: {
        PATH: commands + ':' + process.env.PATH, KEYCHAIN_ACCESSED: accessed,
        APPLE_CERTIFICATE: 'Zml4dHVyZQ==', APPLE_CERTIFICATE_PASSWORD: 'fixture',
        APPLE_SIGNING_IDENTITY: 'Developer ID Application: Fixture',
        APPLE_API_KEY: 'fixture', APPLE_API_ISSUER: 'fixture', APPLE_API_KEY_CONTENT: keyContent,
      },
    });
    assert.ifError(result.error);
    assert.notEqual(result.status, 0);
    assert.equal(fs.existsSync(accessed), false, 'Invalid key reached the keychain command.');
    assert.match(result.stderr, /APPLE_API_KEY_CONTENT/);
    assert.equal(result.stderr.includes(keyContent), false, 'The error exposed the invalid key.');
  }
});

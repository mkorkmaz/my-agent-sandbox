import { createPrivateKey } from 'node:crypto';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

export function validateAppleApiKeyContent(privateKeyContent) {
  const rawPrivateKeyPattern = /^-----BEGIN PRIVATE KEY-----\r?\n(?:[A-Za-z0-9+/=]+\r?\n)+-----END PRIVATE KEY-----(?:\r?\n)?$/;
  if (typeof privateKeyContent !== 'string' || !rawPrivateKeyPattern.test(privateKeyContent)) {
    throw new Error('APPLE_API_KEY_CONTENT must contain the complete raw AuthKey_<KEY_ID>.p8 text with real line breaks. Do not use Base64, quotes, or literal \\n.');
  }
  let privateKey;
  try {
    privateKey = createPrivateKey({ key: privateKeyContent, format: 'pem', type: 'pkcs8' });
  } catch (error) {
    throw new Error('APPLE_API_KEY_CONTENT is not a valid PKCS#8 private key. Copy the original AuthKey_<KEY_ID>.p8 file again.', { cause: error });
  }
  if (privateKey.asymmetricKeyType !== 'ec' || privateKey.asymmetricKeyDetails?.namedCurve !== 'prime256v1') {
    throw new Error('APPLE_API_KEY_CONTENT must contain the App Store Connect P-256 private key from AuthKey_<KEY_ID>.p8.');
  }
}

function main() {
  try {
    validateAppleApiKeyContent(process.env.APPLE_API_KEY_CONTENT);
    console.log('Apple notarization private key format is valid.');
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) main();

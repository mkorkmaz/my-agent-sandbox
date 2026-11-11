#!/bin/bash
set -euo pipefail

: "${APPLE_CERTIFICATE:?Set the base64-encoded Developer ID Application certificate.}"
: "${APPLE_CERTIFICATE_PASSWORD:?Set the certificate password.}"
: "${APPLE_SIGNING_IDENTITY:?Set the Developer ID Application signing identity.}"
: "${APPLE_API_KEY:?Set the App Store Connect API key ID.}"
: "${APPLE_API_ISSUER:?Set the App Store Connect API issuer.}"
: "${APPLE_API_KEY_CONTENT:?Set the App Store Connect private key contents.}"
[[ "$APPLE_SIGNING_IDENTITY" == 'Developer ID Application: '* ]] || { echo 'A Developer ID Application identity is required.' >&2; exit 1; }

work=$(mktemp -d)
keychain="$work/release.keychain-db"
cleanup() {
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
umask 077
printf '%s' "$APPLE_CERTIFICATE" | /usr/bin/base64 -D > "$work/certificate.p12"
printf '%s' "$APPLE_API_KEY_CONTENT" > "$work/AuthKey.p8"
export APPLE_API_KEY_PATH="$work/AuthKey.p8"
keychain_password=$(uuidgen)
security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$work/certificate.p12" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null
security list-keychains -d user -s "$keychain" "$HOME/Library/Keychains/login.keychain-db"

npm run native:prepare
codesign --force --timestamp --options runtime --keychain "$keychain" --sign "$APPLE_SIGNING_IDENTITY" .build/hull-compat/bin/hull
node --input-type=module - "$work/config.json" <<'JS'
import fs from 'node:fs';
fs.writeFileSync(process.argv[2], JSON.stringify({ bundle: { macOS: { signingIdentity: process.env.APPLE_SIGNING_IDENTITY, hardenedRuntime: true } } }));
JS
npm run tauri -- build --ci --target aarch64-apple-darwin --bundles app,dmg --config "$work/config.json" -- --locked
version=$(node -p "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json')).version")
installer="src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/Tugboat_${version}_aarch64.dmg"
xcrun notarytool submit "$installer" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER" --wait --timeout 30m --output-format json > "$work/notarization.json"
node --input-type=module - "$work/notarization.json" <<'JS'
import fs from 'node:fs';
const result = JSON.parse(fs.readFileSync(process.argv[2]));
if (result.status !== 'Accepted') throw new Error('DMG notarization failed: ' + result.status);
JS
xcrun stapler staple "$installer"

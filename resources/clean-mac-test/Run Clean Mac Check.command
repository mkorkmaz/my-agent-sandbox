#!/bin/bash
set -euo pipefail

kitDirectory=$(cd -- "$(dirname -- "$0")" && pwd -P)
logFile="$kitDirectory/check-$(/bin/date -u +%Y%m%dT%H%M%SZ).log"
exec > >(/usr/bin/tee "$logFile") 2>&1
nodeWorkDirectory=''
cleanup() {
  local result=$?
  if [[ -n "$nodeWorkDirectory" ]]; then /bin/rm -rf -- "$nodeWorkDirectory" || result=1; fi
  printf '\n%s\n' "Test log is in $logFile"
  printf '%s\n' "Evidence is in $kitDirectory/artifacts/bundled-runtime"
  if [[ -t 0 ]]; then read -r -p 'Press Return to close this window. ' reply || true; fi
  exit "$result"
}
trap cleanup EXIT

application=${1:-/Applications/Tugboat.app}
if [[ "$application" != /* || ! -x "$application/Contents/MacOS/tugboat" ]]; then
  printf '%s\n' "Install the supplied Tugboat app in /Applications or pass its absolute path." >&2
  exit 1
fi
if [[ "$(/usr/bin/uname -m)" != arm64 ]]; then
  printf '%s\n' 'Run this test on an Apple silicon Mac.' >&2
  exit 1
fi
macOSVersion=$(/usr/bin/sw_vers -productVersion)
if [[ "${macOSVersion%%.*}" -lt 15 ]]; then
  printf '%s\n' 'This test requires macOS 15 or later.' >&2
  exit 1
fi
homebrewPresent=false
for prefix in /opt/homebrew /usr/local; do
  for relative in bin/brew Homebrew/bin/brew; do
    if [[ -e "$prefix/$relative" ]]; then
      homebrewPresent=true
    fi
  done
done
acceptanceArguments=("$application")
if ! "$homebrewPresent"; then acceptanceArguments+=(--clean-mac); fi
printf '%s\n' "Homebrew detected on this Mac: $homebrewPresent"

configuration="$kitDirectory/config/clean-mac-test.json"
nodeVersion=$(/usr/bin/plutil -extract node.version raw -o - "$configuration")
nodeUrl=$(/usr/bin/plutil -extract node.url raw -o - "$configuration")
nodeChecksum=$(/usr/bin/plutil -extract node.sha256 raw -o - "$configuration")
nodeWorkDirectory=$(/usr/bin/mktemp -d /private/tmp/tugboat-test-node.XXXXXXXX)
archive="$nodeWorkDirectory/node.tar.gz"
printf '%s\n' "Downloading the private Node.js $nodeVersion test runtime."
/usr/bin/curl -fsSL --retry 2 --connect-timeout 15 --max-time 300 "$nodeUrl" -o "$archive"
actualChecksum=$(/usr/bin/shasum -a 256 "$archive")
if [[ "${actualChecksum%% *}" != "$nodeChecksum" ]]; then
  printf '%s\n' 'The Node.js archive failed its SHA-256 check.' >&2
  exit 1
fi
/usr/bin/tar -xzf "$archive" -C "$nodeWorkDirectory"
node="$nodeWorkDirectory/node-v$nodeVersion-darwin-arm64/bin/node"
printf '%s\n' 'Checking the supplied app and starting two isolated guest environments.'
printf '%s\n' 'The first boot downloads guest images. A cold boot can take up to 30 minutes per agent.'
"$node" "$kitDirectory/scripts/acceptance/check-bundled-runtime.mjs" "${acceptanceArguments[@]}"

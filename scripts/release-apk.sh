#!/bin/bash
# Publish the phone APK (arm64) as GitHub release assets.
#   scripts/release-apk.sh 0.2.0
# Creates release `apk-v<version>` (assets: hohmann-reach.apk, hohmann-reach.apk.sha256) and refreshes the moving
# release `apk-latest`. The game-server VM copies apk-latest into <data>/public/ so the server's own
# http://<server>:7777/ download page serves it.
set -euo pipefail
cd "$(dirname "$0")/.."
VER="${1:?usage: release-apk.sh <version>}"
REPO=JonasMerrell/space-rts
DIST="$(mktemp -d)"
# the app compares this with the version the server announces and asks to update when older
HOHMANN_VERSION="$VER" ABIS=arm64-v8a OUT="$DIST/hohmann-reach.apk" ./build-apk.sh
(cd "$DIST" && sha256sum hohmann-reach.apk > hohmann-reach.apk.sha256)
SHA="$(git rev-parse HEAD)"
NOTES="Android app (arm64) at ${SHA:0:12}. Verify: sha256sum -c hohmann-reach.apk.sha256"
gh release create "apk-v$VER" "$DIST/hohmann-reach.apk" "$DIST/hohmann-reach.apk.sha256" -R "$REPO" --target "$SHA" \
  --title "App v$VER" --notes "$NOTES"
gh release delete apk-latest -R "$REPO" --yes --cleanup-tag 2>/dev/null || true
gh release create apk-latest "$DIST/hohmann-reach.apk" "$DIST/hohmann-reach.apk.sha256" -R "$REPO" --target "$SHA" \
  --title "App (latest)" --notes "$NOTES (same as apk-v$VER)" --latest=false
cp "$DIST/hohmann-reach.apk" HohmannsReach-phone.apk 2>/dev/null || true
echo "released apk-v$VER and apk-latest from ${SHA:0:12}"

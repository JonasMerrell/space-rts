#!/bin/bash
# Publish the browser build as GitHub release assets.
#   scripts/release-web.sh 0.1.0
# Creates release `web-v<version>` (assets: hohmann-web.tar.gz + .sha256) at the current commit and
# refreshes the moving release `web-latest`. The game-server VM unpacks it into <data>/public/web/
# and restarts the server (the file list is read at startup); it is then served at /play/:
#   https://github.com/JonasMerrell/space-rts/releases/download/web-latest/hohmann-web.tar.gz
#   https://github.com/JonasMerrell/space-rts/releases/download/web-latest/hohmann-web.tar.gz.sha256
# The tarball's top-level directory is `web/`.
set -euo pipefail
cd "$(dirname "$0")/.."
VER="${1:?usage: release-web.sh <version>}"
REPO=JonasMerrell/space-rts
DIST="$(mktemp -d)"
scripts/build-web.sh "$DIST/web"
tar -C "$DIST" --owner=0 --group=0 -czf "$DIST/hohmann-web.tar.gz" web
(cd "$DIST" && sha256sum hohmann-web.tar.gz > hohmann-web.tar.gz.sha256)
SHA="$(git rev-parse HEAD)"
NOTES="Browser build (wasm) of crates/client at ${SHA:0:12}; unpack into <data>/public/web/. Verify: sha256sum -c hohmann-web.tar.gz.sha256"
gh release create "web-v$VER" "$DIST/hohmann-web.tar.gz" "$DIST/hohmann-web.tar.gz.sha256" -R "$REPO" --target "$SHA" \
  --title "Browser build v$VER" --notes "$NOTES"
gh release delete web-latest -R "$REPO" --yes --cleanup-tag 2>/dev/null || true
gh release create web-latest "$DIST/hohmann-web.tar.gz" "$DIST/hohmann-web.tar.gz.sha256" -R "$REPO" --target "$SHA" \
  --title "Browser build (latest)" --notes "$NOTES (same as web-v$VER)" --latest=false
echo "released web-v$VER and web-latest from ${SHA:0:12}"

#!/bin/bash
# Publish a static (musl) game-server build as GitHub release assets.
#   scripts/release-server.sh 0.1.0
# Creates release `server-v<version>` (assets: server, server.sha256) at the current commit and
# refreshes the moving release `server-latest` with the same assets. The game-server VM
# (hohmann-reach-server) downloads from:
#   https://github.com/JonasMerrell/space-rts/releases/download/server-latest/server
#   https://github.com/JonasMerrell/space-rts/releases/download/server-latest/server.sha256
set -euo pipefail
cd "$(dirname "$0")/.."
VER="${1:?usage: release-server.sh <version>}"
export PATH="$HOME/.cargo/bin:$PATH"
REPO=JonasMerrell/space-rts
TARGET=x86_64-unknown-linux-musl
# ring (TLS) has C code; the host gcc builds it fine for musl
export CC_x86_64_unknown_linux_musl="${CC_x86_64_unknown_linux_musl:-gcc}"
rustup target add "$TARGET" >/dev/null
cargo build --release -p server --target "$TARGET"
OUT="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')/$TARGET/release"
DIST="$(mktemp -d)"
cp "$OUT/server" "$DIST/server"
strip "$DIST/server"
(cd "$DIST" && sha256sum server > server.sha256)
SHA="$(git rev-parse HEAD)"
NOTES="Static x86_64 musl build of crates/server at ${SHA:0:12}. Verify: sha256sum -c server.sha256"
gh release create "server-v$VER" "$DIST/server" "$DIST/server.sha256" -R "$REPO" --target "$SHA" \
  --title "Game server v$VER" --notes "$NOTES"
# moving "latest" release: retarget the tag and replace the assets
gh release delete server-latest -R "$REPO" --yes --cleanup-tag 2>/dev/null || true
gh release create server-latest "$DIST/server" "$DIST/server.sha256" -R "$REPO" --target "$SHA" \
  --title "Game server (latest)" --notes "$NOTES (same as server-v$VER)" --latest=false
echo "released server-v$VER and server-latest from ${SHA:0:12}"

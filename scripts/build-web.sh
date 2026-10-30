#!/bin/bash
# Browser build: crates/client for wasm32 + the JS glue in web/ + game assets → dist/web/
# (served by the game server under /play/ from <data>/public/web/). Usage: scripts/build-web.sh [outdir]
set -euo pipefail
cd "$(dirname "$0")/.."
OUT="${1:-dist/web}"
export PATH="$HOME/.cargo/bin:$PATH"
# miniquad / quad-snd / web/hohmann.js functions are imported from JS at load time
RUSTFLAGS="-C link-arg=--allow-undefined" cargo build -p client --bin space-rts --release --target wasm32-unknown-unknown
WASM="${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release/space-rts.wasm"
rm -rf "$OUT"
mkdir -p "$OUT/assets"
cp web/index.html web/pre.js web/style.css web/boot.js web/hohmann.js web/mq_js_bundle.js web/manifest.webmanifest web/sw.js "$OUT/"
cp assets/icon/ic_launcher_512.png "$OUT/icon.png"
cp "$WASM" "$OUT/hohmann.wasm"
cp -r assets/sprites assets/audio assets/manifest.json "$OUT/assets/"
if [ -n "${HOHMANN_VERSION:-}" ]; then
  printf '{"version":"%s"}\n' "$HOHMANN_VERSION" > "$OUT/version.json"
fi
du -sh "$OUT"

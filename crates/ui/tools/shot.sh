#!/bin/bash
# shot.sh SCENE... -> crates/ui/shots/SCENE.png, rendering the demo on Xvfb (Mesa llvmpipe) in the
# localhost/rts-ui-shot container (build once: podman build -t localhost/rts-ui-shot crates/ui/tools/img).
# Builds with a separate target dir (host lacks the libasound.so dev link).
set -e
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
UI=$ROOT/crates/ui
export PATH=$HOME/.cargo/bin:$PATH
mkdir -p "$ROOT/target-ui/lib" && ln -sf /usr/lib64/libasound.so.2 "$ROOT/target-ui/lib/libasound.so"
CARGO_TARGET_DIR=$ROOT/target-ui RUSTFLAGS="-L $ROOT/target-ui/lib" cargo build -q -p ui --example demo --release
mkdir -p "${SHOTS:-$UI/shots}"
for scene in "$@"; do
  podman run --rm -v "$ROOT":"$ROOT":Z -w "$ROOT" \
    -e UI_SCENE=$scene -e UI_SHOT=${SHOTS:-$UI/shots}/$scene.png -e UI_HOVER=${UI_HOVER:-} -e UI_W=${UI_W:-540} -e UI_H=${UI_H:-1170} \
    localhost/rts-ui-shot bash -c "Xvfb :1 -screen 0 2000x1400x24 -nolisten tcp >/dev/null 2>&1 & sleep 1; DISPLAY=:1 timeout 30 $ROOT/target-ui/release/examples/demo" \
    && echo "${SHOTS:-shots}/$scene.png"
done

#!/bin/bash
# shot.sh SCENE... -> crates/ui/shots/SCENE.png, rendering the demo on Xvfb (Mesa llvmpipe) in the
# localhost/rts-ui-shot container. Builds with a separate target dir (host lacks libasound.so dev link).
set -e
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
UI=$ROOT/crates/ui
export PATH=$HOME/.cargo/bin:$PATH
CARGO_TARGET_DIR=$ROOT/target-ui RUSTFLAGS="-L $UI/tools/lib" cargo build -q -p ui --example demo --release
mkdir -p "$UI/shots"
for scene in "$@"; do
  podman run --rm -v "$ROOT":"$ROOT":Z \
    -e UI_SCENE=$scene -e UI_SHOT=$UI/shots/$scene.png -e UI_W=${UI_W:-540} -e UI_H=${UI_H:-1170} \
    localhost/rts-ui-shot bash -c "Xvfb :1 -screen 0 1600x1300x24 -nolisten tcp >/dev/null 2>&1 & sleep 1; DISPLAY=:1 timeout 30 $ROOT/target-ui/release/examples/demo" \
    && echo "shots/$scene.png"
done

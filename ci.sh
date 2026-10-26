#!/usr/bin/env bash
# Workspace checks: build, tests, clippy. Owned by the tests thread (see COORD.md).
#   ./ci.sh            build + all tests + clippy (warnings shown, not fatal)
#   ./ci.sh --strict   clippy warnings fail the run
#   ./ci.sh --quality  also run the ignored quality-gate / known-bug tests
#   ./ci.sh --quick    simtests only (fast loop while editing sim)
# SIMTESTS_CASES=n changes the number of random cases per property test (default 300).
set -uo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"

strict=0 quality=0 quick=0
for a in "$@"; do
  case "$a" in
    --strict) strict=1 ;;
    --quality) quality=1 ;;
    --quick) quick=1 ;;
    *) echo "unknown option $a" >&2; exit 2 ;;
  esac
done

fail=0
step() { echo; echo "== $*"; }
run() { "$@" || { echo "!! failed: $*"; fail=1; }; }

if [ $quick = 1 ]; then
  step "simtests"
  run cargo test -p simtests --release
  exit $fail
fi

step "build (all targets)"
run cargo build --workspace --all-targets

step "tests"
# release: the property tests and planner scans are ~20× slower in debug
run cargo test --workspace --release

step "ui flow (Xvfb, release)"
if command -v Xvfb >/dev/null; then
  disp=:$((90 + RANDOM % 100))
  Xvfb $disp -screen 0 1200x2500x24 >/dev/null 2>&1 &
  xpid=$!
  sleep 1
  # no sound card on netdash: point ALSA at a null device so macroquad's audio init doesn't panic
  shots="${UITESTS_SHOTS:-${CARGO_TARGET_DIR:-target}/uitests-shots}"
  run env DISPLAY=$disp ALSA_CONFIG_PATH="$PWD/crates/uitests/asound-null.conf" UITESTS_SHOTS="$shots" \
    timeout 300 cargo test --release -p uitests --test ui_flow
  kill $xpid 2>/dev/null
  echo "screenshots: $shots"
else
  echo "Xvfb not installed, skipping (sudo zypper install xorg-x11-server-Xvfb)"
fi

if [ $quality = 1 ]; then
  step "quality gates + known bugs (ignored tests)"
  run cargo test --workspace --release -- --ignored
fi

step "clippy"
if [ $strict = 1 ]; then
  run cargo clippy --workspace --all-targets -- -D warnings
else
  cargo clippy --workspace --all-targets 2>&1 | tee /dev/stderr | grep -c '^warning' | xargs -I{} echo "clippy: {} warning lines"
fi

echo
[ $fail = 0 ] && echo "CI OK" || echo "CI FAILED"
exit $fail

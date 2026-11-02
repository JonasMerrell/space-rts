# Hohmann's Reach

A slow, multiplayer real-time strategy game set in a single star system, written in Rust for Android.
Matches last about a week; you check in for a few minutes a day.

- Real orbital mechanics: planets and moons on Kepler rails, ships fly patched-conic trajectories.
  An auto-planner finds transfer windows (Lambert + shooting); manual maneuver nodes are optional.
- Fuel matters (rocket equation, refineries, tankers). Ownership = most combat power in orbit.
- Deterministic combat with rock-paper-scissors ship classes, upgrades, bots, in-game chat.
- Cel-shaded comic art style.

## Layout
- `crates/sim` — shared simulation (orbits, planner, game rules, bots); runs on server and client.
- `crates/proto` — JSON-lines wire protocol.
- `crates/server` — authoritative game server (std TCP, accounts, persistent games).
- `crates/ui` — macroquad UI (screens, system map, node editor).
- `crates/client` — the app: net loop + UI backend; builds for desktop and Android.
- `crates/warsim` — headless bot-vs-bot war simulator for balance tuning (see below).
- `crates/simtests` — integration tests. `assets/` — art and audio.

## Build & run
- Server: `cargo run --release -p server -- --port 7777 --data data`
- Desktop client: `SPACE_RTS_SERVER=host:7777 cargo run --release -p client`
- Android APK: `./build-apk.sh` (needs Android SDK + NDK 25, cargo-ndk, JDK 17) → `HohmannsReach.apk`

## Balance simulator
`warsim` plays whole wars with bots in every slot, as fast as the CPU allows (a 4-pilot 14-day war takes about
40 s on one core; wars run in parallel), on the current rules:
- `cargo run --release -p warsim -- run --games 10 --players 2,4,8 --sizes small,medium,large --bots hard --out wars.jsonl`
  (`--bots mixed` draws slot difficulties per war, `--teams teams|both`, `--days`, `--seed`, `--threads`)
- `cargo run --release -p warsim -- report wars.jsonl` — win rates by home kind/orbit/difficulty/slot, snowballing,
  ships built and lost, economy pace, captures.
- `cargo run --release -p warsim -- duel --budget 3000 --scale dreadnought=0.75` — equal-cost fights between ship
  classes; `--scale` tries a different price.

## Releases
- **Every release adds a `CHANGELOG.md` entry** on top (`## X.Y.Z — YYYY-MM-DD`, then `- ` bullets in plain
  player language). The app bundles it as PATCH NOTES (main menu, game menu) and shows "What's new" once after
  an update. `scripts/release-{apk,web,server}.sh X.Y.Z` refuse to run unless the top entry is X.Y.Z, and
  `cargo test -p ui notes` checks the file parses.

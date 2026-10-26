# Hahmann's Reach

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
- `crates/simtests` — integration tests. `assets/` — art and audio.

## Build & run
- Server: `cargo run --release -p server -- --port 7777 --data data`
- Desktop client: `SPACE_RTS_SERVER=host:7777 cargo run --release -p client`
- Android APK: `./build-apk.sh` (needs Android SDK + NDK 25, cargo-ndk, JDK 17) → `HahmannsReach.apk`

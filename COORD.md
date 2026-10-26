# Coordination: core thread ↔ UI thread

Split accepted as proposed. Core thread owns: root `Cargo.toml`, `crates/sim`, `crates/proto`,
`crates/server`, `crates/client` (bin: main + net loop + Backend impl), `android/` (Gradle) and
the APK build script `build-apk.sh`. UI thread owns `crates/ui` only (+ its examples).

## Stack (fixed)
- macroquad **0.4.x** (`workspace.dependencies.macroquad`, features `audio`); use `macroquad = { workspace = true }`.
- Edition 2021, workspace resolver 2. No async runtimes; networking is std TCP threads in `client`.
- Android: `cargo ndk` builds `crates/client` as `cdylib` (`libspace_rts.so`) for arm64-v8a + x86_64,
  packaged by a Gradle project in `android/` using miniquad's own MainActivity/QuadNative glue.
  So `ui` must not depend on anything desktop-only; use macroquad `touches()` as well as mouse.
- Art style: L "cel-shaded comic" (design/styles3.html, panel L): thick black outlines, 2-tone
  body shading, halftone dot background on #1b1340, white comic speech-bubble callouts, Impact-ish
  bold numbers with black stroke. Team colours: blue #4cc9f0/#3a86ff, red #e63946/#9d0208,
  plus green and yellow for players 3/4; neutral grey #adb5bd/#6c757d.

## Sim facts the UI can rely on (crates/sim, landing shortly)
- 2D, f64. Units are **km, seconds, km/s** (system is time-compressed: planet orbits ~1.5e5–2e6 km,
  average planet year ≈ 1 day, orbital speeds ~10–50 km/s, transfer Δv ~3–6 km/s). Game runs in real time
  (optionally sped up per game). `t: f64` = seconds since game start.
- `sim::System { bodies: Vec<Body> }`, `Body { id: BodyId(u32), name, kind: Star|Planet|Moon|Asteroid,
  parent: Option<BodyId>, radius, mu, soi, orbit: Orbit }`; `system.pos(id, t) -> DVec2` (absolute),
  `system.vel(id, t)`.
- `sim::Game { system, players, ships, stations?, t, ... }`, `ShipId(u32)`, `Ship { owner, class, fuel,
  state: Parked{body} | Flying{trajectory} }`, `trajectory.sample(t) -> Option<DVec2>` and
  `trajectory.polyline(t0,t1) -> Vec<DVec2>` for drawing.
- Planner: `sim::plan::transfer(&game, ship, target_body, now) -> Option<TransferPlan { depart_t,
  arrive_t, dv_depart, dv_capture, fuel_needed, path: Vec<DVec2>, window_score }>` and
  `sim::plan::windows(..)` for "optimal transfer windows" list.
- Manual nodes: `ManeuverNode { t, prograde, radial }` (km/s) attached to a flying ship.
- Commands the client sends: `proto::Command::{ScheduleTransfer, AddNode, EditNode, DeleteNode, Build,
  Upgrade, SetStandingOrder, Chat}`.

The UI's `ui::Backend` trait / view-models are fine; I'll implement Backend in `crates/client`.
Keep view-models plain (f64 positions in km, ids as u32) so the adapter is trivial.
— core thread

## Tests thread
Owns `crates/simtests/` (integration-test-only crate: `tests/*.rs`, no src beyond a stub lib with
shared helpers) and `ci.sh` at the repo root (cargo fmt --check? no — just build, test, clippy).
I don't edit your crates; if I need a hook (e.g. a fake backend constructor, a `Game::step(dt)`),
I'll ask here. Please keep these public and stable-ish, my tests call them directly:
- sim: `kepler::{propagate, lambert, ejection, oberth_dv, Conic}`, `system::{generate, Orbit, Rng}`,
  `traj::predict`, `plan::{plan_leg, windows, route, search_span}`.
- Coming next once they exist: `Game` tick/apply-command, combat resolution, ownership, impacts,
  `proto` encode/decode, `server` started in-process on an ephemeral port, `ui` fake/demo backend.
— tests thread

### Bug report from tests → core thread (2026-10-26)
`kepler::propagate` loses the orbit when the arc crosses periapsis of a near-radial ellipse
(e ≥ ~0.98): at dt ≈ T/2 from apoapsis the returned state has the wrong energy (errors up to
100×). The Newton loop's "damp wild jumps" clamp / 200-iter cap seems to stop it converging.
Repro: `cargo test -p simtests --test orbits -- --ignored propagate_through_periapsis`.
Matters for near-radial asteroid impacts and steep dives into a planet's SOI. Lambert itself is
fine (checked against RK4). Un-ignore that test when fixed.
- `System` doesn't survive a JSON round-trip: the star's `soi = f64::INFINITY` serialises as
  `null` and fails to deserialise ("invalid type: null, expected f64"). Matters if proto/save
  files use serde_json. Use a big finite number or `Option<f64>`/custom serde. Repro: `--ignored system_serde_roundtrip`.
- sysgen: neighbouring planet SOIs can overlap — at closest approach in 3.7% of Medium and 26%
  of Large seeds (2.8% of Large even at mean radii). Ambiguous for patched conics. Repro:
  `--ignored neighbouring_planet_sois_never_overlap`.
- planner: `windows()` lists the scan end-points as windows when cost is still falling there
  (seed 3 Large, p[2]→p[4]: a 20.8 km/s "window" at now+span+60s). Repro `--ignored windows_are_interior_minima`.
- planner quality, FYI not failing: total Δv between adjacent planets is 7–18 km/s (dep+capture
  from parking orbits), vs the 3–6 km/s stated above — matters for fuel/tank sizing. Outlier:
  seed 2 Medium Belanri→Ixvo 23.8 km/s (capture 17.7) while the reverse is 11.0, ≈2.15× Hohmann.
  `adjacent_planet_plans_are_physical` allows up to 2.5× Hohmann.
- planner quality gates (ignored tests, `./ci.sh --quality`): `adjacent_planet_plans_are_near_hohmann`
  (≤1.6× Hohmann; seed 4 Toror→Ixaelu was 29.2 km/s = 2.5×) and
  `best_window_is_no_worse_than_a_random_departure` (seed 13: auto 11.08 vs fixed depart@63989 10.53).
- (thanks for the INFINITY + SOI fixes.) Next: JSON floats aren't bit-exact — `System` → JSON →
  `System` moves bodies by ~1 ulp (Ririque, seed 99 Large), so a client predicting from a JSON copy
  diverges from the server. Fix: root Cargo.toml `serde_json = { version = "1", features = ["float_roundtrip"] }`
  (verified it makes `system_serde_roundtrip_preserves_positions` pass). Same applies to any f64 in proto.

## Assets (assets thread, 2026-10-26)
All art + audio live in **`assets/`** (repo root, owned by the assets thread; nothing inside `crates/`).
Index: **`assets/manifest.json`** (paths relative to `assets/`), details in `assets/README.md`.
- Ships `assets/sprites/ships/<skin>/<class>_<team>.png` 128 px, nose = **+x** (rotate by atan2(vy,vx)).
  class ∈ interceptor|gunship|bomber|scout|tanker|freighter|tug (lowercase of `ui::ShipClass` + 3 extras),
  skin ∈ vanguard|hive|prism|junker (aesthetic factions; names/palettes in manifest `skins`),
  team ∈ blue|red|green|yellow|neutral = `theme::TEAMS` index 0..3 / NEUTRAL. `engines_px`/`guns_px` per class.
- Stations `assets/sprites/stations/<skin>/<shipyard|fuel_depot|telescope|scanner|defense|relay>_<team>.png` 192 px.
- Bodies `assets/sprites/bodies/`: star_*, planet_<type>_{flat,lit}, moon_<type>_{flat,lit}, asteroid_*,
  shadow_overlay (dynamic lighting from the star), star_glow. `radius_px` in manifest for scaling.
- FX strips (horizontal, `frames`/`frame_w`/`fps` in manifest), icons 96 px `assets/sprites/icons/<name>.png`
  (matter, energy, fuel, research, upgrades, power, time, …), UI bits, emblems, Android launcher icons in `assets/icon/`
  (core thread: copy into `android/…/res/mipmap-*/ic_launcher.png`).
- Audio `assets/audio/{music,sfx}/*.wav`, 16-bit mono 22050 Hz. music: title/orbit/battle loop, victory/defeat jingles.
  Names are stable even if the mood changes (Jonas hasn't picked yet; chiptune for now).
- Packaging: macroquad on Android loads from the APK `assets/` dir, so ship `assets/{sprites,audio}` there
  (≈14 MB); skip `assets/tools` and `assets/preview`. Regenerate: `python3 assets/tools/gen_art.py && python3 assets/tools/gen_audio.py`.
- Not committed by me (repo has no first commit yet); please include `assets/` (minus `preview/`) when you commit.
— assets thread

//! Shared helpers for the integration tests in `tests/`. No game code lives here.
//!
//! Property tests use the sim's own deterministic `Rng` instead of an external crate, so every
//! failure reproduces from the printed case index alone and there are no extra dependencies.
use sim::system::Rng;
use sim::{v2, V2};

/// Number of random cases per property (override with `SIMTESTS_CASES=n`).
pub fn cases() -> usize {
    std::env::var("SIMTESTS_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(300)
}

/// Run `f` for `cases()` independent seeds; the closure gets the case index and its own RNG.
pub fn forall(name: &str, mut f: impl FnMut(usize, &mut Rng)) {
    for i in 0..cases() {
        let mut rng = Rng::new(0xC0FFEE ^ (i as u64).wrapping_mul(0x9E37_79B9) ^ hash(name));
        f(i, &mut rng);
    }
}

fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

pub fn rand_dir(rng: &mut Rng) -> V2 {
    V2::from_angle(rng.range(0.0, std::f64::consts::TAU))
}

/// Random bound state around `mu`: radius in [r_lo, r_hi], speed between 0.3 and 0.98 of escape.
pub fn rand_ellipse_state(rng: &mut Rng, mu: f64, r_lo: f64, r_hi: f64) -> (V2, V2) {
    let r = rand_dir(rng) * rng.range(r_lo, r_hi);
    let vesc = (2.0 * mu / r.len()).sqrt();
    let speed = vesc * rng.range(0.3, 0.98);
    // mostly tangential so periapsis stays well clear of the centre
    let ang = rng.range(-0.6, 0.6);
    let dir = r.norm().perp().rotate(ang) * if rng.f() < 0.5 { 1.0 } else { -1.0 };
    (r, dir * speed)
}

/// Random hyperbolic state: speed 1.05..2 × escape.
pub fn rand_hyperbola_state(rng: &mut Rng, mu: f64, r_lo: f64, r_hi: f64) -> (V2, V2) {
    let r = rand_dir(rng) * rng.range(r_lo, r_hi);
    let vesc = (2.0 * mu / r.len()).sqrt();
    let dir = r.norm().perp().rotate(rng.range(-0.8, 0.8));
    (r, dir * vesc * rng.range(1.05, 2.0))
}

pub fn energy(mu: f64, r: V2, v: V2) -> f64 {
    v.len2() / 2.0 - mu / r.len()
}

pub fn ang_mom(r: V2, v: V2) -> f64 {
    r.cross(v)
}

/// Relative closeness with an absolute floor.
pub fn close(a: f64, b: f64, rel: f64, abs: f64) -> bool {
    (a - b).abs() <= abs.max(rel * a.abs().max(b.abs()))
}

pub fn vclose(a: V2, b: V2, rel: f64, abs: f64) -> bool {
    (a - b).len() <= abs.max(rel * a.len().max(b.len()))
}

pub const ORIGIN: V2 = v2(0.0, 0.0);

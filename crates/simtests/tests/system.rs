//! Star-system generation and determinism (`sim::system`).
use sim::system::{generate, Rng, SystemSize, STAR_MU};
use sim::{BodyKind, System};

const SIZES: [SystemSize; 3] = [SystemSize::Small, SystemSize::Medium, SystemSize::Large];

fn planets(s: &System) -> Vec<&sim::Body> {
    s.bodies.iter().filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant)).collect()
}

#[test]
fn rng_golden_values() {
    // Pinned output: if this changes, every saved game and every client/server pair disagrees.
    let mut r = Rng::new(1);
    let got: Vec<u64> = (0..4).map(|_| r.next_u64()).collect();
    let mut r2 = Rng::new(1);
    let again: Vec<u64> = (0..4).map(|_| r2.next_u64()).collect();
    assert_eq!(got, again);
    assert_eq!(got, GOLDEN_RNG, "Rng output changed: {got:?}");
}
const GOLDEN_RNG: [u64; 4] = [973819730272012410, 6108091081255984487, 12125365036566318712, 9038174178950858617];

#[test]
fn rng_ranges() {
    let mut r = Rng::new(7);
    for _ in 0..10_000 {
        let f = r.f();
        assert!((0.0..1.0).contains(&f));
        let x = r.range(-3.0, 5.0);
        assert!((-3.0..5.0).contains(&x));
        assert!(r.below(6) < 6);
    }
}

#[test]
fn generation_is_bit_identical_for_a_seed() {
    for size in SIZES {
        for seed in [0u64, 1, 42, 0xDEADBEEF, u64::MAX] {
            let a = serde_json::to_string(&generate(seed, size)).unwrap();
            let b = serde_json::to_string(&generate(seed, size)).unwrap();
            assert_eq!(a, b, "seed {seed} {size:?}");
        }
    }
}

#[test]
fn different_seeds_give_different_systems() {
    let a = serde_json::to_string(&generate(1, SystemSize::Medium)).unwrap();
    let b = serde_json::to_string(&generate(2, SystemSize::Medium)).unwrap();
    assert_ne!(a, b);
}

/// A system sent to clients (or saved) as JSON must predict *bit-identical* positions, or client
/// prediction drifts from the server. Needs serde_json's `float_roundtrip` feature.
#[test]
fn system_serde_roundtrip_preserves_positions() {
    let s = generate(99, SystemSize::Large);
    let back: System = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    for b in &s.bodies {
        for t in [0.0, 1234.5, 86_400.0 * 3.3] {
            assert_eq!(s.pos(b.id, t), back.pos(b.id, t), "{} at {t}", b.name);
        }
    }
}

#[test]
fn structure_is_sane_for_many_seeds() {
    for size in SIZES {
        let want = match size {
            SystemSize::Small => 5,
            SystemSize::Medium => 7,
            SystemSize::Large => 9,
        };
        for seed in 0..200u64 {
            let s = generate(seed, size);
            let ctx = format!("seed {seed} {size:?}");
            assert_eq!(s.star().kind, BodyKind::Star, "{ctx}");
            assert!(s.star().parent.is_none());
            assert_eq!(s.bodies.iter().filter(|b| b.is_star()).count(), 1, "{ctx}");
            for (i, b) in s.bodies.iter().enumerate() {
                assert_eq!(b.id.0 as usize, i, "{ctx}: ids are indices");
                assert!(b.radius > 0.0 && b.mu > 0.0, "{ctx}: {}", b.name);
                assert!(!b.name.is_empty());
            }
            let ps = planets(&s);
            assert_eq!(ps.len(), want, "{ctx}");
            // planets ordered outward, no overlapping SOIs, orbits nowhere near the star
            for w in ps.windows(2) {
                assert!(w[1].orbit.unwrap().a > w[0].orbit.unwrap().a, "{ctx}: planets not ordered");
            }
            for p in &ps {
                let o = p.orbit.unwrap();
                assert!(o.a * (1.0 - o.e) > s.star().radius * 3.0, "{ctx}: {} grazes star", p.name);
                // room to capture into the parking orbit (inner rocky planets of the new generator have
                // SOIs of only ~1.6× their parking radius)
                assert!(p.soi > p.park_radius() * 1.5, "{ctx}: {} SOI too small to park in (soi {:.0}, park {:.0})", p.name, p.soi, p.park_radius());
                assert!(p.park_radius() < p.soi, "{ctx}");
            }
            for b in &s.bodies {
                if b.kind == BodyKind::Moon {
                    let par = s.body(b.parent.unwrap());
                    assert!(matches!(par.kind, BodyKind::Planet | BodyKind::GasGiant), "{ctx}: moon of a moon");
                    let o = b.orbit.unwrap();
                    assert!(o.a + b.soi < par.soi, "{ctx}: moon {} leaks out of parent SOI", b.name);
                    assert!(o.a - b.radius > par.park_radius(), "{ctx}: moon {} cuts parent parking orbit", b.name);
                }
                if b.kind == BodyKind::Asteroid {
                    assert_eq!(b.parent, Some(s.star().id), "{ctx}");
                    assert!(b.park_radius() < b.soi, "{ctx}: can't park at asteroid {}", b.name);
                }
            }
        }
    }
}

/// Known sim issue (COORD.md, 2026-10-26): neighbouring planets' SOIs can overlap — ~3.7% of
/// Medium and ~26% of Large systems at closest approach (2.8% of Large even on mean radii), which
/// makes the patched-conic "which SOI am I in" question ambiguous. Un-ignore once sysgen spaces
/// planets by SOI.
#[test]

fn neighbouring_planet_sois_never_overlap() {
    for size in SIZES {
        for seed in 0..1000u64 {
            let s = generate(seed, size);
            for w in planets(&s).windows(2) {
                let (a0, a1) = (w[0].orbit.unwrap(), w[1].orbit.unwrap());
                let gap = a1.a * (1.0 - a1.e) - a0.a * (1.0 + a0.e);
                assert!(
                    gap > w[0].soi + w[1].soi,
                    "seed {seed} {size:?}: SOIs of {} and {} can overlap (gap {gap:.0} km, SOIs {:.0} + {:.0})",
                    w[0].name, w[1].name, w[0].soi, w[1].soi
                );
            }
        }
    }
}

#[test]
fn planet_years_average_about_a_day() {
    // design: average planet year ≈ 1 day; inner hours, outer ~a week
    let mut all = vec![];
    for seed in 0..100u64 {
        let s = generate(seed, SystemSize::Medium);
        for p in planets(&s) {
            all.push(p.orbit.unwrap().period() / 86_400.0);
        }
    }
    all.sort_by(f64::total_cmp);
    let median = all[all.len() / 2];
    assert!((0.4..2.5).contains(&median), "median year {median} days");
    assert!(*all.last().unwrap() < 8.0, "outer year {} days", all.last().unwrap());
}

#[test]
fn star_mu_and_body_motion() {
    let s = generate(5, SystemSize::Small);
    assert_eq!(s.star().mu, STAR_MU);
    assert_eq!(s.pos(s.star().id, 1e6), sim::V2::ZERO);
    for b in &s.bodies[1..] {
        // bodies actually move and their absolute velocity is d(pos)/dt
        let t = 5000.0;
        let fd = (s.pos(b.id, t + 0.5) - s.pos(b.id, t - 0.5)) / 1.0;
        let v = s.vel(b.id, t);
        assert!((fd - v).len() < 1e-4 * v.len().max(1.0), "{}: fd {fd:?} v {v:?}", b.name);
        // relative-frame helpers are consistent
        if let Some(p) = b.parent {
            assert!((s.pos_in(b.id, p, t) - s.rel_state(b.id, t).0).len() < 1e-6);
            assert_eq!(*s.lineage(b.id).last().unwrap(), s.star().id);
        }
        // by_name is case-insensitive (first match wins for duplicate names)
        let found = s.by_name(&b.name.to_uppercase()).unwrap();
        assert!(s.body(found).name.eq_ignore_ascii_case(&b.name));
    }
}

//! The star system: bodies on Keplerian rails and random generation.
use crate::kepler::ecc_anomaly;
use crate::v2::{v2, V2};
use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BodyId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyKind {
    Star,
    Planet,
    GasGiant,
    Moon,
    Asteroid,
    /// a free-floating station's spot, founded by a construction ship in orbit of the sun (outpost.rs)
    Outpost,
}

/// Elliptic orbit around the parent, fixed for the whole game.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Orbit {
    pub a: f64,
    pub e: f64,
    /// argument of periapsis
    pub w: f64,
    /// mean anomaly at t = 0
    pub m0: f64,
    /// mean motion (rad/s) — derived from parent mu, stored for speed
    pub n: f64,
}

impl Orbit {
    pub fn new(parent_mu: f64, a: f64, e: f64, w: f64, m0: f64) -> Orbit {
        Orbit { a, e, w, m0, n: (parent_mu / (a * a * a)).sqrt() }
    }
    pub fn period(&self) -> f64 {
        TAU / self.n
    }
    /// Position and velocity relative to the parent at time t.
    pub fn state(&self, t: f64) -> (V2, V2) {
        let m = self.m0 + self.n * t;
        let ea = ecc_anomaly(m, self.e);
        let (s, c) = ea.sin_cos();
        let b = self.a * (1.0 - self.e * self.e).sqrt();
        let p = v2(self.a * (c - self.e), b * s);
        let k = self.n / (1.0 - self.e * c);
        let v = v2(-self.a * s * k, b * c * k);
        (p.rotate(self.w), v.rotate(self.w))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Body {
    pub id: BodyId,
    pub name: String,
    pub kind: BodyKind,
    pub parent: Option<BodyId>,
    pub radius: f64,
    pub mu: f64,
    /// sphere of influence radius (`STAR_SOI` for the star)
    pub soi: f64,
    pub orbit: Option<Orbit>,
    /// cosmetic: palette index / surface style
    pub look: u32,
}

impl Body {
    /// Radius of the standard parking orbit used for parked ships.
    pub fn park_radius(&self) -> f64 {
        match self.kind {
            BodyKind::Asteroid | BodyKind::Outpost => self.radius * 3.0,
            _ => self.radius * 1.6,
        }
    }
    pub fn is_star(&self) -> bool {
        self.kind == BodyKind::Star
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct System {
    pub bodies: Vec<Body>,
    pub seed: u64,
}

impl System {
    pub fn body(&self, id: BodyId) -> &Body {
        &self.bodies[id.0 as usize]
    }
    pub fn star(&self) -> &Body {
        &self.bodies[0]
    }
    pub fn children(&self, id: BodyId) -> impl Iterator<Item = &Body> {
        self.bodies.iter().filter(move |b| b.parent == Some(id))
    }
    /// Absolute position (star at origin).
    pub fn pos(&self, id: BodyId, t: f64) -> V2 {
        let b = self.body(id);
        match (b.parent, &b.orbit) {
            (Some(p), Some(o)) => self.pos(p, t) + o.state(t).0,
            _ => V2::ZERO,
        }
    }
    pub fn vel(&self, id: BodyId, t: f64) -> V2 {
        let b = self.body(id);
        match (b.parent, &b.orbit) {
            (Some(p), Some(o)) => self.vel(p, t) + o.state(t).1,
            _ => V2::ZERO,
        }
    }
    /// Position/velocity of `id` relative to its parent.
    pub fn rel_state(&self, id: BodyId, t: f64) -> (V2, V2) {
        self.body(id).orbit.map(|o| o.state(t)).unwrap_or((V2::ZERO, V2::ZERO))
    }
    /// Position of `id` relative to `frame` (which must be an ancestor or the body itself).
    pub fn pos_in(&self, id: BodyId, frame: BodyId, t: f64) -> V2 {
        self.pos(id, t) - self.pos(frame, t)
    }
    pub fn vel_in(&self, id: BodyId, frame: BodyId, t: f64) -> V2 {
        self.vel(id, t) - self.vel(frame, t)
    }
    /// Chain of ancestors from the body up to the star (inclusive both ends).
    pub fn lineage(&self, id: BodyId) -> Vec<BodyId> {
        let mut v = vec![id];
        let mut cur = id;
        while let Some(p) = self.body(cur).parent {
            v.push(p);
            cur = p;
        }
        v
    }
    pub fn by_name(&self, name: &str) -> Option<BodyId> {
        self.bodies.iter().find(|b| b.name.eq_ignore_ascii_case(name)).map(|b| b.id)
    }
}

/// Small deterministic RNG (xorshift64*), identical on every platform.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rng(pub u64);
impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    pub fn f(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f()
    }
    pub fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % n as u64) as u32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SystemSize {
    Small,
    Medium,
    Large,
}

/// Gravitational parameter of the star (km³/s²). Chosen so a planet at ~4e5 km has a ~1 day year.
pub const STAR_MU: f64 = 3.65e8;
/// The star's SOI is everything (finite so the system round-trips through JSON).
pub const STAR_SOI: f64 = 1e30;

const SYLLABLES: &[&str] = &[
    "ka", "ri", "zo", "ul", "ven", "tor", "ix", "an", "dra", "mo", "sel", "que", "bel", "nyx", "or", "ta", "phe", "lu",
    "gar", "est", "vo", "rh", "ae", "kor",
];

fn name(rng: &mut Rng) -> String {
    let n = 2 + rng.below(2);
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(SYLLABLES[rng.below(SYLLABLES.len() as u32) as usize]);
    }
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

const GREEK: &[&str] = &["a", "b", "c", "d"];

/// Generate a random system. Planets are spaced geometrically ("spread out"); planet years range
/// from a few hours (inner) to about a week (outer), averaging about a day.
pub fn generate(seed: u64, size: SystemSize) -> System {
    generate_for(seed, size, 4)
}

/// `generate` for a war with `slots` pilots. Up to four it is exactly `generate`; each slot past
/// four adds a planet and stretches the outer orbit by 15%, so there are homes for everyone at
/// about the same spacing, half rocky inner worlds and half gas giants (rules 6).
pub fn generate_for(seed: u64, size: SystemSize, slots: usize) -> System {
    let mut rng = Rng::new(seed);
    let (base, outer) = match size {
        SystemSize::Small => (5, 9.0e5),
        SystemSize::Medium => (7, 1.4e6),
        SystemSize::Large => (9, 2.0e6),
    };
    let extra = slots.saturating_sub(4) as i32;
    let n_planets = base + extra;
    let outer = outer * (1.0 + 0.15 * extra as f64);
    let mut bodies = vec![Body {
        id: BodyId(0),
        name: name(&mut rng),
        kind: BodyKind::Star,
        parent: None,
        radius: 25_000.0,
        mu: STAR_MU,
        soi: STAR_SOI,
        orbit: None,
        look: rng.below(4),
    }];
    let inner = 1.4e5;
    let ratio = (outer / inner as f64).powf(1.0 / (n_planets - 1) as f64);
    let belt_after = (n_planets / 2) as usize; // asteroid belt between two middle planets
    // first pass: planet orbits and masses, so SOIs can be clamped against both neighbours
    struct P {
        a: f64,
        e: f64,
        giant: bool,
        radius: f64,
        mu: f64,
        soi: f64,
    }
    let mut ps: Vec<P> = (0..n_planets)
        .map(|i| {
            // inner system: small rocky worlds close together; outer system: gas giants far apart
            let giant = i > n_planets / 2;
            // balance: outer years were very long, so pull planets in until the outermost one
            // orbits 2x faster (T ∝ a^1.5, so a shrinks by k^-2/3), ramping from 1x at the innermost
            let k = 1.0 + i as f64 / (n_planets - 1) as f64;
            let a = inner * ratio.powi(i) * k.powf(-2.0 / 3.0) * rng.range(0.93, 1.07);
            // The system is time-compressed (years of hours/days), so bodies are small and dense to
            // keep their spheres of influence comfortably larger than the bodies themselves.
            // rocky planets grow outward (bigger = richer: see Game::base_yield), offsetting the slower,
            // costlier outer orbits
            let grow = 1.0 + 0.6 * i as f64 / (n_planets - 1) as f64;
            let radius = if giant { rng.range(2800.0, 4500.0) } else { rng.range(700.0, 1500.0) * grow };
            let mu = if giant { 3.0e4 * (radius / 1000.0).powi(3) } else { 1.6e5 * (radius / 1000.0).powi(3) };
            let e = rng.range(0.0, 0.06);
            P { a, e, giant, radius, mu, soi: a * (mu / STAR_MU).powf(0.4) }
        })
        .collect();
    for i in 0..ps.len() {
        // neighbouring SOIs must never touch, even at closest approach (apoapsis vs periapsis)
        let mut cap = f64::INFINITY;
        if i > 0 {
            cap = cap.min(0.45 * (ps[i].a * (1.0 - ps[i].e) - ps[i - 1].a * (1.0 + ps[i - 1].e)));
        }
        if i + 1 < ps.len() {
            cap = cap.min(0.45 * (ps[i + 1].a * (1.0 - ps[i + 1].e) - ps[i].a * (1.0 + ps[i].e)));
        }
        ps[i].soi = ps[i].soi.min(cap);
        // a tightly packed planet must still leave room to park inside its SOI: shrink it (same density)
        let k = if ps[i].giant { 3.0e4 } else { 1.6e5 };
        while ps[i].soi < ps[i].radius * 1.6 * 1.6 && ps[i].radius > 300.0 {
            ps[i].radius *= 0.9;
            ps[i].mu = k * (ps[i].radius / 1000.0).powi(3);
            ps[i].soi = (ps[i].a * (ps[i].mu / STAR_MU).powf(0.4)).min(cap);
        }
    }
    for (i, p) in ps.iter().enumerate() {
        let (a_i, giant, radius, mu, soi) = (p.a, p.giant, p.radius, p.mu, p.soi);
        let orbit = Orbit::new(STAR_MU, a_i, p.e, rng.range(0.0, TAU), rng.range(0.0, TAU));
        let pid = BodyId(bodies.len() as u32);
        let pname = name(&mut rng);
        bodies.push(Body {
            id: pid,
            name: pname.clone(),
            kind: if giant { BodyKind::GasGiant } else { BodyKind::Planet },
            parent: Some(BodyId(0)),
            radius,
            mu,
            soi,
            orbit: Some(orbit),
            look: rng.below(8),
        });
        // moons
        // gas giants carry big moon systems; inner rocky planets have at most one moon
        let n_moons = if giant { 2 + rng.below(3) } else { rng.below(2) };
        let mut mr = radius * if giant { 2.4 } else { 3.5 };
        let mut prev_outer = radius * 1.6;
        for k in 0..n_moons {
            let r_m = mr * rng.range(1.0, 1.3);
            let mrad = radius * if giant { rng.range(0.07, 0.15) } else { rng.range(0.12, 0.3) };
            let mmu = 1.2e5 * (mrad / 1000.0).powi(3);
            let msoi = r_m * (mmu / mu).powf(0.4);
            // must fit inside the planet's SOI, and leave room for a parking orbit inside its own
            // ...and must not touch the previous moon's SOI
            if r_m + msoi > soi * 0.7 || msoi < mrad * 1.6 * 1.6 || r_m - msoi < prev_outer * 1.05 {
                break;
            }
            prev_outer = r_m + msoi;
            let morbit = Orbit::new(mu, r_m, 0.0, 0.0, rng.range(0.0, TAU));
            bodies.push(Body {
                id: BodyId(bodies.len() as u32),
                name: format!("{pname} {}", GREEK[k as usize]),
                kind: BodyKind::Moon,
                parent: Some(pid),
                radius: mrad,
                mu: mmu,
                soi: msoi,
                orbit: Some(morbit),
                look: rng.below(8),
            });
            mr = (r_m + msoi) * if giant { 1.2 } else { 1.5 };
        }
        if i + 1 == belt_after {
            // belt sits in the middle of the gap between this planet's SOI and the next one's
            let lo = a_i * (1.0 + p.e) + soi;
            let hi = ps[i + 1].a * (1.0 - ps[i + 1].e) - ps[i + 1].soi;
            let belt_a = 0.5 * (lo + hi);
            let half = 0.3 * (hi - lo);
            // a dense belt between the inner and outer system: lots of small, cheap-to-reach claims
            for _ in 0..(8 + n_planets) {
                let e = rng.range(0.0, 0.5) * half / belt_a;
                let ar = (belt_a + rng.range(-0.5, 0.5) * half).max(lo + belt_a * e * 1.05);
                let rad = rng.range(15.0, 60.0);
                let amu = 0.8e5 * (rad / 1000.0).powi(3);
                let o = Orbit::new(STAR_MU, ar, e, rng.range(0.0, TAU), rng.range(0.0, TAU));
                bodies.push(Body {
                    id: BodyId(bodies.len() as u32),
                    name: format!("{}-{}", (b'A' + rng.below(26) as u8) as char, 100 + rng.below(900)),
                    kind: BodyKind::Asteroid,
                    parent: Some(BodyId(0)),
                    radius: rad,
                    mu: amu,
                    soi: (ar * (amu / STAR_MU).powf(0.4)).max(rad * 8.0),
                    orbit: Some(o),
                    look: rng.below(8),
                });
            }
        }
    }
    System { bodies, seed }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generation_is_sane() {
        let s = generate(42, SystemSize::Medium);
        let planets: Vec<_> = s.bodies.iter().filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant)).collect();
        assert_eq!(planets.len(), 7);
        let periods: Vec<f64> = planets.iter().map(|p| p.orbit.unwrap().period() / 3600.0).collect();
        println!("planet periods (h): {periods:?}");
        assert!(periods[0] < 12.0 && *periods.last().unwrap() > 48.0);
        for b in &s.bodies {
            if let Some(p) = b.parent {
                let par = s.body(p);
                assert!(b.orbit.unwrap().a > par.radius * 2.0, "{} too close", b.name);
                if b.kind != BodyKind::Asteroid {
                    assert!(b.soi > b.park_radius() * 1.5, "{} soi {} < park {}", b.name, b.soi, b.park_radius());
                }
                if b.kind == BodyKind::Moon {
                    assert!(b.orbit.unwrap().a + b.soi < par.soi, "moon {} soi leaks", b.name);
                }
            }
        }
        // determinism
        let s2 = generate(42, SystemSize::Medium);
        assert_eq!(s.bodies.len(), s2.bodies.len());
        assert_eq!(s.bodies[3].name, s2.bodies[3].name);
    }

    fn sane(s: &System) {
        for b in &s.bodies {
            if let Some(p) = b.parent {
                let par = s.body(p);
                assert!(b.orbit.unwrap().a > par.radius * 2.0, "{} too close", b.name);
                if b.kind != BodyKind::Asteroid {
                    assert!(b.soi > b.park_radius() * 1.5, "{} soi {} < park {}", b.name, b.soi, b.park_radius());
                }
                if b.kind == BodyKind::Moon {
                    assert!(b.orbit.unwrap().a + b.soi < par.soi, "moon {} soi leaks", b.name);
                }
            }
        }
    }

    #[test]
    fn four_slots_or_fewer_generate_the_old_system() {
        for size in [SystemSize::Small, SystemSize::Medium, SystemSize::Large] {
            for slots in 2..=4 {
                let (a, b) = (generate(9, size), generate_for(9, size, slots));
                assert_eq!(a.bodies.len(), b.bodies.len());
                assert!(a.bodies.iter().zip(&b.bodies).all(|(x, y)| x.name == y.name && x.radius == y.radius));
            }
        }
    }

    #[test]
    fn eight_slot_systems_have_room_for_everyone() {
        for size in [SystemSize::Small, SystemSize::Medium, SystemSize::Large] {
            for seed in 0..40 {
                let s = generate_for(seed, size, 8);
                sane(&s);
                let rocky = s.bodies.iter().filter(|b| b.kind == BodyKind::Planet).count();
                let giants = s.bodies.iter().filter(|b| b.kind == BodyKind::GasGiant).count();
                assert!(rocky + giants >= 9, "{size:?} seed {seed}: {rocky} rocky + {giants} giants");
                assert!(rocky >= giants, "inner/outer balance: {rocky} rocky vs {giants} giants");
                // the outermost year stays playable (a few days, not weeks)
                let last = s.bodies.iter().rfind(|b| b.parent == Some(BodyId(0)) && b.kind == BodyKind::GasGiant).unwrap();
                let days = last.orbit.unwrap().period() / 86_400.0;
                assert!(days < 21.0, "{size:?} seed {seed}: outer year {days:.1} d");
            }
        }
    }
}

//! Property tests for the two-body / Kepler layer (`sim::kepler`, `sim::system::Orbit`).
use sim::kepler::{circular_speed, ecc_anomaly, ejection, lambert, oberth_dv, propagate, stumpff, Conic};
use sim::system::{Orbit, STAR_MU};
use sim::{v2, V2};
use simtests::*;
use std::f64::consts::TAU;

const PLANET_MU: f64 = 4.0e5;

#[test]
fn stumpff_is_continuous_across_zero() {
    for &z in &[-1e-5, -1e-6, -9e-7, 0.0, 9e-7, 1e-6, 1e-5] {
        let (c, s) = stumpff(z);
        assert!(close(c, 0.5, 1e-5, 0.0), "C({z}) = {c}");
        assert!(close(s, 1.0 / 6.0, 1e-5, 0.0), "S({z}) = {s}");
    }
    // series branch vs closed form agree at the switch-over
    let (c1, s1) = stumpff(1.0001e-6);
    let (c0, s0) = stumpff(0.9999e-6);
    assert!((c1 - c0).abs() < 1e-9 && (s1 - s0).abs() < 1e-9);
}

#[test]
fn propagate_conserves_energy_and_angular_momentum_ellipse() {
    forall("ellipse-conserve", |i, rng| {
        let (r0, v0) = rand_ellipse_state(rng, STAR_MU, 1.0e5, 2.0e6);
        let dt = rng.range(-3.0e5, 3.0e5);
        let (r1, v1) = propagate(STAR_MU, r0, v0, dt);
        let (e0, e1) = (energy(STAR_MU, r0, v0), energy(STAR_MU, r1, v1));
        assert!(close(e0, e1, 1e-7, 1e-9), "case {i}: energy {e0} -> {e1}");
        let (h0, h1) = (ang_mom(r0, v0), ang_mom(r1, v1));
        assert!(close(h0, h1, 1e-7, 1e-6), "case {i}: h {h0} -> {h1}");
    });
}

#[test]
fn propagate_conserves_energy_and_angular_momentum_hyperbola() {
    forall("hyper-conserve", |i, rng| {
        let (r0, v0) = rand_hyperbola_state(rng, PLANET_MU, 4_000.0, 60_000.0);
        let dt = rng.range(-2.0e4, 2.0e4);
        let (r1, v1) = propagate(PLANET_MU, r0, v0, dt);
        assert!(r1.x.is_finite() && v1.x.is_finite(), "case {i}: non-finite");
        let (e0, e1) = (energy(PLANET_MU, r0, v0), energy(PLANET_MU, r1, v1));
        assert!(close(e0, e1, 1e-6, 1e-9), "case {i}: energy {e0} -> {e1}");
        let (h0, h1) = (ang_mom(r0, v0), ang_mom(r1, v1));
        assert!(close(h0, h1, 1e-6, 1e-6), "case {i}: h {h0} -> {h1}");
    });
}

#[test]
fn propagate_roundtrip_returns_to_start() {
    forall("roundtrip", |i, rng| {
        let (r0, v0) = if rng.f() < 0.5 {
            rand_ellipse_state(rng, STAR_MU, 1.0e5, 2.0e6)
        } else {
            rand_hyperbola_state(rng, STAR_MU, 1.0e5, 2.0e6)
        };
        let dt = rng.range(1.0, 2.0e5);
        let (r1, v1) = propagate(STAR_MU, r0, v0, dt);
        let (r2, v2_) = propagate(STAR_MU, r1, v1, -dt);
        assert!(vclose(r2, r0, 1e-7, 1e-2), "case {i}: r {r0:?} -> {r2:?}");
        assert!(vclose(v2_, v0, 1e-7, 1e-7), "case {i}: v {v0:?} -> {v2_:?}");
    });
}

#[test]
fn propagate_composes() {
    forall("compose", |i, rng| {
        let (r0, v0) = rand_ellipse_state(rng, STAR_MU, 1.0e5, 2.0e6);
        let (a, b) = (rng.range(-1e5, 1e5), rng.range(-1e5, 1e5));
        let (ra, va) = propagate(STAR_MU, r0, v0, a);
        let (rab, vab) = propagate(STAR_MU, ra, va, b);
        let (rd, vd) = propagate(STAR_MU, r0, v0, a + b);
        assert!(vclose(rab, rd, 1e-7, 1e-2), "case {i}: {rab:?} vs {rd:?}");
        assert!(vclose(vab, vd, 1e-7, 1e-7), "case {i}");
    });
}

#[test]
fn propagate_whole_period_is_identity() {
    forall("period", |i, rng| {
        let (r0, v0) = rand_ellipse_state(rng, STAR_MU, 1.0e5, 2.0e6);
        let c = Conic::from_state(STAR_MU, r0, v0);
        let p = c.period().expect("bound orbit has a period");
        let k = 1.0 + rng.below(5) as f64;
        let (r1, v1) = propagate(STAR_MU, r0, v0, p * k);
        assert!(vclose(r1, r0, 1e-6, 1.0), "case {i}: after {k} periods {r0:?} -> {r1:?}");
        assert!(vclose(v1, v0, 1e-6, 1e-6), "case {i}");
    });
}

/// Known sim bug (reported in COORD.md, 2026-10-26): crossing periapsis on an orbit with
/// e ≳ 0.98 gives a state with the wrong energy. Near-radial asteroid impacts and steep dives
/// take this path. Remove the `ignore` once `kepler::propagate` is fixed.
#[test]

fn propagate_through_periapsis_of_near_radial_orbit() {
    for &e in &[0.9, 0.95, 0.98, 0.99, 0.995, 0.999] {
        let a: f64 = 1.0e5;
        // start at apoapsis, so dt ≈ T/2 crosses periapsis
        let r0 = v2(a * (1.0 + e), 0.0);
        let v0 = v2(0.0, (STAR_MU * (1.0 - e) / (a * (1.0 + e))).sqrt());
        let period = TAU * (a.powi(3) / STAR_MU).sqrt();
        let e0 = -STAR_MU / (2.0 * a);
        for k in 0..2000 {
            let dt = period * k as f64 / 2000.0;
            let (r, v) = propagate(STAR_MU, r0, v0, dt);
            let en = energy(STAR_MU, r, v);
            assert!(close(en, e0, 1e-6, 0.0), "e={e} dt/T={}: energy {en} vs {e0}", k as f64 / 2000.0);
        }
    }
}

#[test]
fn propagate_zero_dt_is_identity() {
    let (r, v) = (v2(4e5, 1.0), v2(0.2, 30.0));
    assert_eq!(propagate(STAR_MU, r, v, 0.0), (r, v));
}

#[test]
fn propagate_matches_newtonian_integration_short_arc() {
    // independent check: RK4 integrate a = -mu r/|r|³ and compare
    forall("rk4", |i, rng| {
        let (r0, v0) = rand_ellipse_state(rng, STAR_MU, 2.0e5, 1.0e6);
        let dt = 3600.0;
        let acc = |r: V2| r * (-STAR_MU / r.len().powi(3));
        let (mut r, mut v) = (r0, v0);
        let n = 2000;
        let h = dt / n as f64;
        for _ in 0..n {
            let (k1r, k1v) = (v, acc(r));
            let (k2r, k2v) = (v + k1v * (h / 2.0), acc(r + k1r * (h / 2.0)));
            let (k3r, k3v) = (v + k2v * (h / 2.0), acc(r + k2r * (h / 2.0)));
            let (k4r, k4v) = (v + k3v * h, acc(r + k3r * h));
            r += (k1r + k2r * 2.0 + k3r * 2.0 + k4r) * (h / 6.0);
            v += (k1v + k2v * 2.0 + k3v * 2.0 + k4v) * (h / 6.0);
        }
        let (rk, vk) = propagate(STAR_MU, r0, v0, dt);
        assert!((rk - r).len() < 1e-3, "case {i}: kepler {rk:?} rk4 {r:?}");
        assert!((vk - v).len() < 1e-8 * v.len().max(1.0) + 1e-7, "case {i}");
    });
}

#[test]
fn ecc_anomaly_solves_keplers_equation() {
    forall("kepler-eq", |i, rng| {
        let e = rng.range(0.0, 0.97);
        let m = rng.range(-20.0, 20.0);
        let ea = ecc_anomaly(m, e);
        let resid = (ea - e * ea.sin() - m.rem_euclid(TAU)).abs();
        assert!(resid < 1e-10, "case {i}: e={e} M={m} E={ea} resid={resid}");
    });
}

#[test]
fn orbit_state_matches_propagation_and_derivative() {
    forall("orbit-rails", |i, rng| {
        let a = rng.range(1.0e5, 2.0e6);
        let e = rng.range(0.0, 0.3);
        let o = Orbit::new(STAR_MU, a, e, rng.range(0.0, TAU), rng.range(0.0, TAU));
        let t = rng.range(0.0, 7.0 * 86_400.0);
        let dt = rng.range(-5e4, 5e4);
        let (r0, v0) = o.state(t);
        let (r1, v1) = o.state(t + dt);
        let (rp, vp) = propagate(STAR_MU, r0, v0, dt);
        assert!(vclose(r1, rp, 1e-7, 1e-2), "case {i}: rails {r1:?} vs propagate {rp:?}");
        assert!(vclose(v1, vp, 1e-7, 1e-7), "case {i}");
        // velocity is the time derivative of position
        let h = 1.0;
        let fd = (o.state(t + h).0 - o.state(t - h).0) / (2.0 * h);
        assert!(vclose(fd, v0, 1e-6, 1e-6), "case {i}: fd {fd:?} vs v {v0:?}");
        // shape round-trips through the state vector
        let c = Conic::from_state(STAR_MU, r0, v0);
        assert!(close(c.a, a, 1e-9, 1e-6), "case {i}: a {a} -> {}", c.a);
        assert!((c.e - e).abs() < 1e-9, "case {i}: e {e} -> {}", c.e);
        assert!(close(o.period(), c.period().unwrap(), 1e-9, 0.0));
        // and the orbits run counter-clockwise
        assert!(c.h > 0.0, "case {i}: rails orbit is clockwise");
    });
}

#[test]
fn keplers_third_law() {
    let year = |a: f64| Orbit::new(STAR_MU, a, 0.0, 0.0, 0.0).period();
    // ~1 day at 4e5 km, per the design notes
    let d = year(4.0e5) / 86_400.0;
    assert!((0.8..1.2).contains(&d), "planet at 4e5 km has a {d} day year");
    // T² ∝ a³
    let ratio = (year(8.0e5) / year(4.0e5)).powi(2);
    assert!(close(ratio, 8.0, 1e-12, 0.0));
}

#[test]
fn conic_apsides_and_periapsis_timing() {
    forall("conic", |i, rng| {
        let (r0, v0) = rand_ellipse_state(rng, STAR_MU, 1.0e5, 2.0e6);
        let c = Conic::from_state(STAR_MU, r0, v0);
        assert!(c.rp <= r0.len() * (1.0 + 1e-9) && r0.len() <= c.apoapsis() * (1.0 + 1e-9), "case {i}");
        let tp = c.time_to_periapsis(r0, v0).unwrap();
        assert!(tp >= 0.0 && tp <= c.period().unwrap() + 1e-6, "case {i}: tp {tp}");
        let (rp, vp) = propagate(STAR_MU, r0, v0, tp);
        assert!(close(rp.len(), c.rp, 1e-6, 1e-2), "case {i}: |r| at periapsis {} vs rp {}", rp.len(), c.rp);
        assert!(rp.dot(vp).abs() / (rp.len() * vp.len()) < 1e-5, "case {i}: not perpendicular at periapsis");
    });
}

#[test]
fn lambert_solutions_hit_the_target() {
    let mut solved = 0;
    forall("lambert", |i, rng| {
        let r1 = rand_dir(rng) * rng.range(1.4e5, 2.0e6);
        let r2 = rand_dir(rng) * rng.range(1.4e5, 2.0e6);
        let tof = rng.range(2.0e3, 4.0e5);
        let ccw = rng.f() < 0.7;
        if let Some((va, vb)) = lambert(STAR_MU, r1, r2, tof, ccw) {
            solved += 1;
            // arcs that dive through the star are unphysical (and hit the e≈1 propagate issue,
            // covered separately by `propagate_through_periapsis_of_near_radial_orbit`)
            if Conic::from_state(STAR_MU, r1, va).rp < 25_000.0 {
                return;
            }
            let (rr, vv) = propagate(STAR_MU, r1, va, tof);
            assert!(vclose(rr, r2, 1e-6, 5.0), "case {i}: missed by {} km (r1={r1:?} r2={r2:?} tof={tof} ccw={ccw} v1={va:?})", (rr - r2).len());
            assert!(vclose(vv, vb, 1e-5, 1e-4), "case {i}");
            // direction of travel respected
            assert_eq!(r1.cross(va) > 0.0, ccw, "case {i}: wrong winding");
        }
    });
    assert!(solved * 2 > cases(), "lambert solved only {solved}/{}", cases());
}

#[test]
fn lambert_rejects_degenerate_input() {
    let r = v2(4e5, 0.0);
    assert!(lambert(STAR_MU, r, v2(-3e5, 1e5), 0.0, true).is_none());
    assert!(lambert(STAR_MU, r, v2(-3e5, 1e5), -10.0, true).is_none());
    assert!(lambert(STAR_MU, r, r * 1.5, 1e4, true).is_none(), "collinear same direction");
}

#[test]
fn lambert_hohmann_limit() {
    // 179.9° transfer in exactly half the Hohmann period ≈ Hohmann departure speed
    let (ra, rb): (f64, f64) = (4.0e5, 8.0e5);
    let at = (ra + rb) / 2.0;
    let tof = std::f64::consts::PI * (at.powi(3) / STAR_MU).sqrt();
    let r2 = V2::from_angle(179.9f64.to_radians()) * rb;
    let (v1, _) = lambert(STAR_MU, v2(ra, 0.0), r2, tof, true).unwrap();
    let hohmann = (STAR_MU * (2.0 / ra - 1.0 / at)).sqrt();
    assert!(close(v1.len(), hohmann, 5e-3, 0.0), "{} vs {hohmann}", v1.len());
}

#[test]
fn ejection_reaches_requested_asymptote() {
    forall("ejection", |i, rng| {
        let rp = rng.range(4_000.0, 20_000.0);
        let vinf = rand_dir(rng) * rng.range(0.5, 8.0);
        let (p, v) = ejection(PLANET_MU, rp, vinf);
        assert!(close(p.len(), rp, 1e-9, 0.0), "case {i}");
        assert!(p.dot(v).abs() < 1e-6 * p.len() * v.len(), "case {i}: not at periapsis");
        assert!(p.cross(v) > 0.0, "case {i}: should leave CCW");
        // speed from vis-viva matches oberth_dv
        let dv = v.len() - circular_speed(PLANET_MU, rp);
        assert!(close(dv, oberth_dv(PLANET_MU, rp, vinf.len()), 1e-9, 1e-12), "case {i}");
        let (_, vfar) = propagate(PLANET_MU, p, v, 5e6);
        let ang = (vfar.norm().cross(vinf.norm())).asin().abs();
        assert!(ang < 5e-3, "case {i}: asymptote off by {ang} rad");
    });
}

#[test]
fn oberth_dv_properties() {
    // zero excess speed = escape burn; monotonic in vinf; cheaper deeper in the well per unit vinf²
    let rp = 8000.0;
    let esc = (2.0f64).sqrt() * circular_speed(PLANET_MU, rp) - circular_speed(PLANET_MU, rp);
    assert!(close(oberth_dv(PLANET_MU, rp, 0.0), esc, 1e-12, 0.0));
    let mut prev = 0.0;
    for k in 1..50 {
        let d = oberth_dv(PLANET_MU, rp, k as f64 * 0.2);
        assert!(d > prev);
        prev = d;
    }
    assert!(oberth_dv(PLANET_MU, rp, 3.0) - oberth_dv(PLANET_MU, rp, 0.0) < 3.0, "Oberth effect");
}

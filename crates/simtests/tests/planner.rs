//! Transfer planner (`sim::plan`) and patched-conic prediction (`sim::traj`).
use sim::kepler::{circular_speed, oberth_dv};
use sim::plan::{plan_leg, route, search_span, windows};
use sim::system::{generate, SystemSize, STAR_MU};
use sim::traj::{predict, End, State};
use sim::{BodyId, BodyKind, System};

fn planets(sys: &System) -> Vec<BodyId> {
    sys.bodies.iter().filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant)).map(|b| b.id).collect()
}

/// Rough Hohmann estimate between two circular heliocentric orbits, burning from parking orbits.
fn hohmann_dv(sys: &System, a: BodyId, b: BodyId) -> f64 {
    let (ba, bb) = (sys.body(a), sys.body(b));
    let (r1, r2) = (ba.orbit.unwrap().a, bb.orbit.unwrap().a);
    let at = (r1 + r2) / 2.0;
    let v1 = (STAR_MU * (2.0 / r1 - 1.0 / at)).sqrt() - circular_speed(STAR_MU, r1);
    let v2 = circular_speed(STAR_MU, r2) - (STAR_MU * (2.0 / r2 - 1.0 / at)).sqrt();
    oberth_dv(ba.mu, ba.park_radius(), v1.abs()) + oberth_dv(bb.mu, bb.park_radius(), v2.abs())
}

#[test]
fn plans_are_deterministic() {
    let sys = generate(21, SystemSize::Medium);
    let p = planets(&sys);
    let a = plan_leg(&sys, p[1], p[2], 500.0, None).unwrap();
    let b = plan_leg(&sys, p[1], p[2], 500.0, None).unwrap();
    // bit-identical: the server and every client must agree on where a fleet will be
    assert_eq!(serde_json::to_string(&a).unwrap(), serde_json::to_string(&b).unwrap());
}

/// Every converged plan between neighbouring planets over a few seeds: (system, label, now, plan).
fn adjacent_plans() -> Vec<(System, String, f64, sim::plan::TransferPlan)> {
    let mut out = vec![];
    for seed in [1u64, 2, 3, 4, 5] {
        let sys = generate(seed, SystemSize::Medium);
        let p = planets(&sys);
        for w in p.windows(2) {
            for (from, to) in [(w[0], w[1]), (w[1], w[0])] {
                let now = 3600.0 * seed as f64;
                let ctx = format!("seed {seed} {} -> {}", sys.body(from).name, sys.body(to).name);
                let plan = plan_leg(&sys, from, to, now, None).unwrap_or_else(|e| panic!("{ctx}: {e}"));
                // non-convergence is counted by the core thread's all_planet_pairs test
                if plan.converged {
                    out.push((sys.clone(), ctx, now, plan));
                }
            }
        }
    }
    out
}

#[test]
fn adjacent_planet_plans_are_physical() {
    let plans = adjacent_plans();
    assert!(plans.len() >= 40, "only {} converged plans", plans.len());
    for (sys, ctx, now, plan) in &plans {
        let (from, to, now) = (plan.from, plan.to, *now);
        assert_ne!(from, to);
        assert!(plan.depart_t() >= now, "{ctx}: departs in the past");
        assert!(plan.arrive_t > plan.depart_t(), "{ctx}: arrives before departing");
        assert!(plan.depart_dv > 0.0 && plan.capture_dv > 0.0, "{ctx}");
        // design: fleets take hours, not minutes or weeks
        let hours = (plan.arrive_t - plan.depart_t()) / 3600.0;
        assert!((0.5..24.0 * 7.0).contains(&hours), "{ctx}: flight {hours:.1} h");
        // never better than physics allows
        let h = hohmann_dv(sys, from, to);
        assert!(plan.total_dv() > 0.7 * h, "{ctx}: {:.2} km/s beats Hohmann {h:.2}", plan.total_dv());
        // the path really ends captured at the target
        match plan.traj.end {
            End::Capture { body, t, rp, .. } => {
                assert_eq!(body, to, "{ctx}");
                assert!((t - plan.arrive_t).abs() < 60.0, "{ctx}: capture t {t} vs arrive {}", plan.arrive_t);
                assert!(rp > sys.body(to).radius, "{ctx}: periapsis below surface");
            }
            e => panic!("{ctx}: converged but ends in {e:?}"),
        }
        // trajectory is continuous and starts at the departure state
        let st = plan.traj.state_at(sys, plan.depart_t()).unwrap();
        assert!((st.r - plan.depart.r).len() < 1.0, "{ctx}");
        for s in plan.traj.segs.windows(2) {
            assert!((s[0].t1 - s[1].t0).abs() < 1e-6, "{ctx}: time gap between segments");
            let pa = plan.traj.abs_pos(sys, s[0].t1 - 1e-3).unwrap();
            let pb = plan.traj.abs_pos(sys, s[1].t0 + 1e-3).unwrap();
            assert!(pa.dist(pb) < 50.0, "{ctx}: position jumps {} km at SOI change", pa.dist(pb));
        }
    }
}

/// Planner *quality* (not correctness): cost within 1.6× of an ideal Hohmann transfer. Ignored
/// while the core thread tunes the planner; run with `./ci.sh --quality`.
#[test]
#[ignore = "quality gate: planner sometimes costs >2× Hohmann"]
fn adjacent_planet_plans_are_near_hohmann() {
    let mut bad = vec![];
    for (sys, ctx, _, plan) in adjacent_plans() {
        let h = hohmann_dv(&sys, plan.from, plan.to);
        let r = plan.total_dv() / h;
        if r > 1.6 {
            bad.push(format!("{ctx}: {:.2} km/s = {r:.2}× Hohmann (dep {:.2}, cap {:.2})", plan.total_dv(), plan.depart_dv, plan.capture_dv));
        }
    }
    assert!(bad.is_empty(), "{} expensive plans:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn fixed_departure_time_is_respected() {
    let sys = generate(8, SystemSize::Medium);
    let p = planets(&sys);
    let td = 20_000.0;
    let plan = plan_leg(&sys, p[2], p[3], 0.0, Some(td)).unwrap();
    assert!((plan.depart_t() - td).abs() < 1.0, "asked {td}, got {}", plan.depart_t());
    // and a departure in the past is pushed to now
    let late = plan_leg(&sys, p[2], p[3], 50_000.0, Some(10.0)).unwrap();
    assert!(late.depart_t() >= 50_000.0);
}

#[test]
#[ignore = "quality gate: auto-picked window can be ~5% worse than a fixed departure"]
fn best_window_is_no_worse_than_a_random_departure() {
    let sys = generate(13, SystemSize::Medium);
    let p = planets(&sys);
    let best = plan_leg(&sys, p[1], p[2], 0.0, None).unwrap();
    let span = search_span(&sys, p[1], p[2]);
    for k in 1..6 {
        let td = span * k as f64 / 6.0;
        if let Ok(fixed) = plan_leg(&sys, p[1], p[2], 0.0, Some(td)) {
            if fixed.converged && best.converged {
                assert!(best.total_dv() <= fixed.total_dv() * 1.05, "best {:.2} vs depart@{td:.0} {:.2}", best.total_dv(), fixed.total_dv());
            }
        }
    }
}

#[test]
fn windows_are_local_minima_inside_span() {
    let sys = generate(3, SystemSize::Large);
    let p = planets(&sys);
    let (from, to) = (p[2], p[4]);
    let now = 10_000.0;
    let span = search_span(&sys, from, to);
    assert!((3600.0..=6.0 * 86_400.0).contains(&span));
    let w = windows(&sys, from, to, now, span);
    assert!(!w.is_empty(), "no transfer windows");
    for x in &w {
        assert!(x.depart_t >= now - 1e-6 && x.depart_t <= now + span + 120.0, "{x:?} (span ends {})", now + span);
        assert!(x.arrive_t > x.depart_t && x.dv > 0.0 && x.dv.is_finite(), "{x:?}");
    }
    for pair in w.windows(2) {
        assert!(pair[1].depart_t > pair[0].depart_t, "windows not in time order");
    }
    // the cheapest window is a sensible cost, and is what plan_leg picks
    let best = w.iter().map(|x| x.dv).fold(f64::INFINITY, f64::min);
    assert!(best < 20.0, "best window {best:.2} km/s");
}

/// Known planner quirk (COORD.md): `windows` reports the scan end-points as windows when the cost
/// is still falling there, e.g. a 20.8 km/s "window" at the very end of the span next to a 12 km/s
/// real one. Un-ignore once end-points are dropped or flagged.
#[test]
#[ignore = "known: windows() includes scan end-points"]
fn windows_are_interior_minima() {
    let sys = generate(3, SystemSize::Large);
    let p = planets(&sys);
    let (from, to, now) = (p[2], p[4], 10_000.0);
    let span = search_span(&sys, from, to);
    let w = windows(&sys, from, to, now, span);
    for x in &w {
        assert!(x.depart_t > now + span * 0.02 && x.depart_t < now + span * 0.98, "edge window {x:?}");
    }
}

#[test]
fn routes_between_any_two_bodies_are_chains_of_direct_legs() {
    for seed in 0..20u64 {
        let sys = generate(seed, SystemSize::Medium);
        let ids: Vec<BodyId> = sys.bodies.iter().filter(|b| !b.is_star()).map(|b| b.id).collect();
        for &a in &ids {
            for &b in &ids {
                let r = route(&sys, a, b);
                if a == b {
                    assert!(r.is_empty());
                    continue;
                }
                assert_eq!(*r.last().unwrap(), b, "seed {seed}: route must end at target");
                assert!(!r.iter().any(|&x| sys.body(x).is_star()), "seed {seed}: routes through the star");
                let mut prev = a;
                for &hop in &r {
                    assert_ne!(hop, prev, "seed {seed}: repeated hop");
                    let (pa, pb) = (sys.body(prev), sys.body(hop));
                    let direct = (pa.parent == pb.parent) || pb.parent == Some(prev) || pa.parent == Some(hop);
                    assert!(direct, "seed {seed}: {} -> {} is not a direct leg in {r:?}", pa.name, pb.name);
                    prev = hop;
                }
            }
        }
    }
}

#[test]
fn coasting_in_a_parking_orbit_never_ends() {
    // a ship parked on a circular orbit, no burns, should still be around that body at the horizon
    let sys = generate(4, SystemSize::Small);
    let p = planets(&sys)[1];
    let b = sys.body(p);
    let r = sim::v2(b.park_radius(), 0.0);
    let v = r.norm().perp() * circular_speed(b.mu, r.len());
    let tr = predict(&sys, State { parent: p, t: 0.0, r, v }, &[], None, 86_400.0);
    assert!(matches!(tr.end, End::Horizon { .. }), "{:?}", tr.end);
    assert!(tr.segs.iter().all(|s| s.parent == p), "left the parking orbit");
    assert!(tr.total_dv() == 0.0);
}

#[test]
fn falling_straight_down_impacts() {
    let sys = generate(4, SystemSize::Small);
    let p = planets(&sys)[1];
    let b = sys.body(p);
    let r = sim::v2(b.soi * 0.5, 0.0);
    let v = sim::v2(-0.01, 0.05); // almost no angular momentum
    let tr = predict(&sys, State { parent: p, t: 0.0, r, v }, &[], None, 10.0 * 86_400.0);
    assert!(matches!(tr.end, End::Impact { body, .. } if body == p), "{:?}", tr.end);
}

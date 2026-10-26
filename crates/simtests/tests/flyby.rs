//! Gravity assists (`sim::assist`, `plan::plan_route` / `options` / `plan_hinted`) and flying them in a game.
use sim::game::*;
use sim::plan::{self, Choice, PlanHint, TransferPlan};
use sim::system::{generate, SystemSize};
use sim::traj::End;
use sim::{BodyId, System};

fn body(sys: &System, name: &str) -> BodyId {
    sys.by_name(name).unwrap_or_else(|| panic!("no body {name}"))
}

/// Seed-1 pairs where the survey (`cargo run --release -p sim --example flybycheck`) finds an assist.
const PAIRS: [(&str, &str); 3] = [("Votorgar", "Garae"), ("Votorgar", "Tanyxvo"), ("Votorgar", "Orkaix")];

/// First seed-1 pair that plans with a flyby at `now`.
fn assisted(sys: &System, now: f64) -> (BodyId, BodyId, TransferPlan) {
    for (a, b) in PAIRS {
        let (a, b) = (body(sys, a), body(sys, b));
        let p = plan::plan_route(sys, a, b, now, None).unwrap();
        if !p.flybys.is_empty() {
            return (a, b, p);
        }
    }
    panic!("no assisted route among {PAIRS:?}");
}

fn same(a: &TransferPlan, b: &TransferPlan) -> bool {
    serde_json::to_string(a).unwrap() == serde_json::to_string(b).unwrap()
}

#[test]
fn assist_route_saves_dv_and_really_swings_by() {
    let sys = generate(1, SystemSize::Medium);
    let (a, b, p) = assisted(&sys, 3600.0);
    let direct = plan::plan_leg(&sys, a, b, 3600.0, None).unwrap();
    println!("{} -> {} via {}: {:.2} km/s vs direct {:.2}", sys.body(a).name, sys.body(b).name, sys.body(p.flybys[0]).name, p.total_dv(), direct.total_dv());
    assert!(p.converged);
    assert!(p.total_dv() < direct.total_dv() * 0.95);
    assert!(matches!(p.traj.end, End::Capture { body, .. } if body == b));
    let fb = p.traj.flybys(&sys);
    let f = fb.iter().find(|f| f.body == p.flybys[0]).expect("path passes the assist body");
    assert!(f.rp > sys.body(f.body).radius, "flyby below the surface");
    assert!(f.turn > 0.05, "the pass should bend the path (turn {:.3} rad)", f.turn);
    // the single correction burn comes after the pass
    assert_eq!(p.burns.len(), 1);
    assert!(p.burns[0].t > f.t_out && p.burns[0].t < p.arrive_t);
}

#[test]
fn plans_are_deterministic_and_hints_reproduce_them() {
    let sys = generate(1, SystemSize::Medium);
    let (a, b, p) = assisted(&sys, 3600.0);
    let (_, _, q) = assisted(&sys, 3600.0);
    assert!(same(&p, &q), "plan_route is not deterministic");
    // the server re-plans from the hint (at a later `now`) and must get the identical course
    let hint = p.hint.expect("assist plans carry a hint");
    assert!(matches!(hint, PlanHint::Assist { .. }));
    let r = plan::plan_hinted(&sys, a, b, p.depart_t(), hint).unwrap();
    assert!(same(&p, &r), "hinted re-plan differs:\n{:?}\n{:?}", p.depart, r.depart);
    let d = plan::plan_leg(&sys, a, b, 3600.0, None).unwrap();
    let rd = plan::plan_hinted(&sys, a, b, d.depart_t(), d.hint.unwrap()).unwrap();
    assert!(same(&d, &rd), "hinted direct re-plan differs");
}

#[test]
fn options_run_from_cheapest_to_fastest() {
    let sys = generate(1, SystemSize::Medium);
    let (a, b) = (body(&sys, "Anseldra"), body(&sys, "Orkaix"));
    let opts = plan::options(&sys, a, b, 3600.0, None, 60.0);
    for (c, p) in &opts {
        println!("{c:?}: {:.2} km/s, {:.1} h, via {:?}", p.total_dv(), (p.arrive_t - 3600.0) / 3600.0, p.flybys);
    }
    assert!(opts.len() >= 2, "expected a faster option too");
    assert_eq!(opts[0].0, Choice::Cheapest);
    for w in opts.windows(2) {
        assert!(w[1].1.total_dv() >= w[0].1.total_dv(), "sorted by Δv");
        assert!(w[1].1.arrive_t < w[0].1.arrive_t, "each dearer option arrives sooner");
    }
    for (_, p) in &opts {
        assert!(p.converged && matches!(p.traj.end, End::Capture { body, .. } if body == b));
        assert!(p.total_dv() <= 60.0 * 1.1);
    }
}

#[test]
fn ship_flies_an_assist_route_in_the_game() {
    let mut g = Game::new("FLYBY".into(), Settings { name: "t".into(), size: SystemSize::Medium, max_players: 2, length: 14.0 * 86_400.0, speed: 1.0 }, 1, "ann");
    g.add_player("bob", None).unwrap();
    g.start();
    let (a, b, p) = assisted(&g.system.clone(), g.t);
    let ship = g.ships.iter().find(|s| s.owner == 0).unwrap().id;
    {
        let s = g.ships.iter_mut().find(|s| s.id == ship).unwrap();
        s.state = ShipState::Parked { body: a, departure: None };
        s.route.clear();
        s.fuel = s.class.stats().fuel_cap * 4.0;
    }
    // other ships could fight at the destination; this test is about the flight
    g.ships.retain(|s| s.id == ship);
    let hint = p.hint.unwrap();
    g.apply(0, Command::TransferPlanned { ships: vec![ship], to: b, depart_t: p.depart_t(), hint }).unwrap();
    g.advance_to(p.depart_t() + 60.0);
    let s = g.ship(ship).unwrap();
    let ShipState::Flying(f) = &s.state else { panic!("not departed: {:?}", s.state) };
    assert_eq!(f.burns.len(), 1, "correction burn is a maneuver node on the flight");
    g.advance_to(p.arrive_t + 120.0);
    let s = g.ship(ship).unwrap();
    assert_eq!(s.parked_at(), Some(b), "{:?}", s.state);
    // a bad hint is an error, not a panic
    let bad = PlanHint::Assist { via: BodyId(9999), flyby_t: 1.0, arrive_t: 2.0 };
    assert!(g.apply(0, Command::TransferPlanned { ships: vec![ship], to: a, depart_t: g.t + 100.0, hint: bad }).is_err());
}

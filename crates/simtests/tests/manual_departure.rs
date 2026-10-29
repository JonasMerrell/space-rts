//! Manual maneuver nodes from orbit (`Command::SetBurns` on a parked ship → `Departure::manual`).
use sim::game::*;
use sim::kepler::circular_speed;
use sim::system::SystemSize;
use sim::traj::Burn;

fn game() -> Game {
    let s = Settings { name: "test".into(), size: SystemSize::Medium, max_players: 2, length: 7.0 * 86_400.0, speed: 1.0 };
    let mut g = Game::new("NODE".into(), s, 11, "ann");
    g.add_player("bob", None).unwrap();
    g.start();
    g
}

fn parked_ship(g: &Game) -> (ShipId, sim::BodyId) {
    g.ships.iter().filter(|s| s.owner == 0 && g.fleet_of(s.id).is_none()).find_map(|s| s.parked_at().map(|b| (s.id, b))).expect("a parked ship")
}

#[test]
fn nodes_from_orbit_schedule_a_departure_that_flies() {
    let mut g = game();
    let (ship, home) = parked_ship(&g);
    let b = g.system.body(home);
    let vc = circular_speed(b.mu, b.park_radius());
    let t0 = g.t + 1800.0;
    // well past escape speed
    let burn = Burn { t: t0, prograde: vc * 0.6, radial: 0.0 };
    g.apply(0, Command::SetBurns { ship, burns: vec![burn] }).unwrap();
    let ShipState::Parked { departure: Some(d), .. } = &g.ship(ship).unwrap().state else { panic!("no departure") };
    assert_eq!(d.node, Some(burn));
    assert!((d.dv - burn.dv()).abs() < 1e-9);
    let park = Departure::parking_state(&g.system, home, t0);
    assert!((d.state.r - park.r).len() < 1e-6, "burns where the parking orbit is at that time");
    assert!(((d.state.v - park.v).len() - burn.prograde).abs() < 1e-6);
    let to = d.to;
    let fuel = g.ship(ship).unwrap().fuel;
    g.advance_to(t0 + 1.0);
    let s = g.ship(ship).unwrap();
    let ShipState::Flying(f) = &s.state else { panic!("didn't depart") };
    assert_eq!(f.dest, Some(to));
    assert!(s.fuel < fuel, "the departure burn costs fuel");
}

#[test]
fn a_burn_too_small_to_escape_comes_back_into_orbit() {
    let mut g = game();
    let (ship, home) = parked_ship(&g);
    let t0 = g.t + 600.0;
    g.apply(0, Command::SetBurns { ship, burns: vec![Burn { t: t0, prograde: 0.05, radial: 0.0 }] }).unwrap();
    let ShipState::Parked { departure: Some(d), .. } = &g.ship(ship).unwrap().state else { panic!() };
    assert_eq!(d.to, home);
    g.advance_to(t0 + 1.0);
    assert!(matches!(g.ship(ship).unwrap().state, ShipState::Flying(_)));
    // one (slightly longer) lap later it's parked again
    let b = g.system.body(home);
    let lap = std::f64::consts::TAU * (b.park_radius().powi(3) / b.mu).sqrt();
    g.advance_to(t0 + 1.5 * lap);
    assert_eq!(g.ship(ship).unwrap().parked_at(), Some(home));
}

#[test]
fn clearing_nodes_cancels_only_a_hand_flown_departure() {
    let mut g = game();
    let (ship, _) = parked_ship(&g);
    let t0 = g.t + 1800.0;
    g.apply(0, Command::SetBurns { ship, burns: vec![Burn { t: t0, prograde: 1.0, radial: 0.2 }] }).unwrap();
    g.apply(0, Command::SetBurns { ship, burns: vec![] }).unwrap();
    assert!(matches!(g.ship(ship).unwrap().state, ShipState::Parked { departure: None, .. }));
    // no nodes on a ship without a manual departure is a no-op, not an error
    g.apply(0, Command::SetBurns { ship, burns: vec![] }).unwrap();
}

#[test]
fn the_first_burn_must_fit_the_tank() {
    let mut g = game();
    let (ship, _) = parked_ship(&g);
    let dv = g.ship(ship).unwrap().dv_left() + 1.0;
    assert!(dv < 1000.0, "fits under the per-burn sanity cap");
    let t0 = g.t + 1800.0;
    assert!(g.apply(0, Command::SetBurns { ship, burns: vec![Burn { t: t0, prograde: dv, radial: 0.0 }] }).is_err());
    assert!(matches!(g.ship(ship).unwrap().state, ShipState::Parked { departure: None, .. }));
}

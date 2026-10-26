//! Time-scrubber forecast (`Game::forecast_to` on a fog-filtered view) and orders for unbuilt ships.
use sim::game::*;
use sim::system::SystemSize;
use sim::BodyId;
use std::time::Instant;

const HOUR: f64 = 3600.0;
const DAY: f64 = 86_400.0;

fn game(bots: bool) -> Game {
    let s = Settings { name: "test".into(), size: SystemSize::Medium, max_players: 4, length: 14.0 * DAY, speed: 1.0 };
    let mut g = Game::new("TEST".into(), s, 5, "ann");
    for n in ["bob", "cat", "dan"] {
        g.add_player(n, if bots { Some("hard".into()) } else { None }).unwrap();
    }
    g.start();
    g
}

fn home(g: &Game, p: PlayerId) -> BodyId {
    g.players[p as usize].home.unwrap()
}

fn rich(g: &mut Game, p: PlayerId) {
    g.players[p as usize].matter = 1e6;
    g.players[p as usize].energy = 1e6;
}

#[test]
fn forecast_shows_queued_ship_when_it_is_built() {
    let mut g = game(false);
    rich(&mut g, 0);
    let h = home(&g, 0);
    g.apply(0, Command::Build { body: h, class: ShipClass::Scout }).unwrap();
    let done = g.bodies[h.0 as usize].queue[0].done_t;
    let mut f = g.view_for(0);
    let before = f.ships.len();
    f.forecast_to(done - 61.0);
    assert_eq!(f.ships.len(), before, "not built yet");
    f.forecast_to(done + 61.0);
    assert_eq!(f.ships.len(), before + 1, "built in the forecast");
    assert_eq!(f.ships.last().unwrap().parked_at(), Some(h));
}

#[test]
fn queued_order_sends_the_new_ship_on_its_way() {
    let mut g = game(false);
    rich(&mut g, 0);
    let h = home(&g, 0);
    let to = home(&g, 1);
    g.apply(0, Command::Build { body: h, class: ShipClass::Scout }).unwrap();
    let done = g.bodies[h.0 as usize].queue[0].done_t;
    g.apply(0, Command::QueueOrder { body: h, done_t: done, to: Some(to), depart_t: None }).unwrap();
    assert!(g.apply(1, Command::QueueOrder { body: h, done_t: done, to: Some(to), depart_t: None }).is_err(), "not bob's job");
    g.advance_to(done + 61.0);
    let s = g.ships.iter().max_by_key(|s| s.id.0).unwrap();
    let going = match &s.state {
        ShipState::Parked { departure, .. } => departure.as_ref().map(|d| d.to),
        ShipState::Flying(f) => f.dest,
    };
    assert!(going.is_some(), "new ship has no transfer: {:?}", s.state);
}

#[test]
fn forecast_does_not_run_bots() {
    let mut g = game(true);
    g.advance_to(6.0 * HOUR);
    let mut f = g.view_for(0);
    let mut real = g.clone();
    let t = g.t + 2.0 * DAY;
    real.advance_to(t);
    let t0 = Instant::now();
    f.forecast_to(t);
    eprintln!("forecast 2 days: {:?}, {} ships (real game {} ships)", t0.elapsed(), f.ships.len(), real.ships.len());
    let enemy_now = g.view_for(0).ships.iter().filter(|s| s.owner != 0).count();
    let enemy_f = f.ships.iter().filter(|s| s.owner != 0).count();
    assert!(enemy_f <= enemy_now, "forecast invented enemy ships: {enemy_f} > {enemy_now}");
}

#[test]
fn forecast_a_week_is_fast_enough() {
    let mut g = game(true);
    g.advance_to(6.0 * HOUR);
    let mut f = g.view_for(0);
    let t0 = Instant::now();
    f.forecast_to(g.t + 7.0 * DAY);
    let el = t0.elapsed();
    eprintln!("forecast 7 days: {el:?} ({} ships at start)", g.ships.len());
    assert!(el.as_secs_f64() < 20.0);
}

#[test]
fn host_can_change_speed_without_a_time_jump() {
    let mut g = game(false);
    g.started_real = 1_000_000.0;
    let real = g.started_real + 3600.0; // one real hour in at 1×
    g.advance_to(g.time_at(real));
    let t = g.t;
    assert!(g.apply(1, Command::SetSpeed { speed: 4.0 }).is_err(), "only the host");
    g.apply(0, Command::SetSpeed { speed: 4.0 }).unwrap();
    assert!((g.time_at(real) - t).abs() < 1e-6, "game time continuous");
    assert!((g.time_at(real + 60.0) - (t + 240.0)).abs() < 1e-6, "4× from now on");
}

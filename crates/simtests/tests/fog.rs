//! Fog of war (`sim::fog`): detection by signature, engine flares, scanners, fleet glow, ghost
//! contacts, and that `Game::view_for` never leaks what a player can't see.
use sim::fog::{self, detects, signature, Sensor, SensorKind};
use sim::game::*;
use sim::system::SystemSize;
use sim::{v2, BodyId, BodyKind};

fn game4(seed: u64) -> Game {
    let s = Settings { name: "fog".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0 };
    let mut g = Game::new("FOG1".into(), s, seed, "ann");
    for n in ["bob", "cat", "dan"] {
        g.add_player(n, None).unwrap();
    }
    g.start();
    g
}

fn put_ship(g: &mut Game, owner: PlayerId, class: ShipClass, at: BodyId) -> ShipId {
    let id = ShipId(g.next_ship);
    g.next_ship += 1;
    let st = class.stats();
    g.ships.push(Ship {
        id,
        owner,
        class,
        name: format!("{} t{}", st.name, id.0),
        hp: st.hp,
        fuel: st.fuel_cap,
        order: StandingOrder::Hold,
        route: vec![],
        state: ShipState::Parked { body: at, departure: None },
        tech: Default::default(),
        last_burn_t: no_burn(),
    });
    id
}

/// Advance exactly one 60 s tick so vision is recomputed.
fn tick(g: &mut Game) {
    let t = g.last_tick + TICK;
    g.advance_to(t);
}

fn passive(range: f64) -> Vec<Sensor> {
    vec![Sensor { pos: v2(0.0, 0.0), range, kind: SensorKind::Passive }]
}

#[test]
fn big_ships_are_seen_farther_than_small_ones() {
    let s = passive(100_000.0);
    let sig = |c: ShipClass| c.stats().signature;
    // range scales with sqrt(signature)
    assert!(detects(&s, v2(190_000.0, 0.0), sig(ShipClass::Dreadnought)).0, "dreadnought (sig 4) at 1.9 R");
    assert!(!detects(&s, v2(210_000.0, 0.0), sig(ShipClass::Dreadnought)).0);
    assert!(!detects(&s, v2(60_000.0, 0.0), sig(ShipClass::Scout)).0, "scout (sig 0.3) at 0.6 R");
    assert!(detects(&s, v2(50_000.0, 0.0), sig(ShipClass::Scout)).0);
    let mut by_sig = ShipClass::ALL.to_vec();
    by_sig.sort_by(|a, b| sig(*a).total_cmp(&sig(*b)));
    assert_eq!(by_sig.first(), Some(&ShipClass::Scout));
    assert_eq!(by_sig.last(), Some(&ShipClass::Dreadnought));
}

#[test]
fn engine_burns_flare_for_a_while() {
    let mut g = game4(1);
    let id = put_ship(&mut g, 1, ShipClass::Interceptor, BodyId(1));
    let base = ShipClass::Interceptor.stats().signature;
    let s = g.ship(id).unwrap().clone();
    assert_eq!(signature(&s, 1000.0), base);
    let mut s2 = s.clone();
    s2.last_burn_t = 1000.0;
    assert_eq!(signature(&s2, 1000.0), base * fog::BURN_FLARE);
    assert_eq!(signature(&s2, 1000.0 + fog::FLARE_TIME - 1.0), base * fog::BURN_FLARE);
    assert_eq!(signature(&s2, 1000.0 + fog::FLARE_TIME + 1.0), base);
}

#[test]
fn departures_set_the_burn_time() {
    let mut g = game4(2);
    let home = g.players[0].home.unwrap();
    let target = g.players[1].home.unwrap();
    let ship = g.ships.iter().find(|s| s.owner == 0 && s.class == ShipClass::Gunship).unwrap().id;
    g.apply(0, Command::Transfer { ships: vec![ship], to: target, depart_t: None }).unwrap();
    let ShipState::Parked { departure: Some(d), .. } = &g.ship(ship).unwrap().state else { panic!("no departure") };
    let dep = d.state.t;
    g.advance_to(dep + 120.0);
    let s = g.ship(ship).unwrap();
    assert!(matches!(s.state, ShipState::Flying(_)), "left {home:?}");
    assert_eq!(s.last_burn_t, dep);
}

#[test]
fn scanners_see_every_ship_in_range_and_its_plans() {
    let s = vec![Sensor { pos: v2(0.0, 0.0), range: 50_000.0, kind: SensorKind::Scanner }];
    assert_eq!(detects(&s, v2(49_000.0, 0.0), 0.01), (true, true));
    assert_eq!(detects(&s, v2(51_000.0, 0.0), 100.0), (false, false), "scanner range is hard, not signature-scaled");
    let relay = vec![Sensor { pos: v2(0.0, 0.0), range: 1e9, kind: SensorKind::Relay }];
    assert!(!detects(&relay, v2(10.0, 0.0), 100.0).0, "a relay is not a sensor by itself");
}

#[test]
fn fleets_parked_together_are_easier_to_spot() {
    // player 0's only sensor that matters is its home telescope; find a body just out of its
    // reach for one scout, then stack scouts there until the group (sig × sqrt(n)) shows up.
    for seed in 1..40 {
        let mut g = game4(seed);
        let home = g.players[0].home.unwrap();
        let t_next = g.last_tick + TICK;
        let r = fog::body_sensor(&g.bodies[home.0 as usize]);
        let scout = ShipClass::Scout.stats().signature;
        let Some((b, need)) = g
            .system
            .bodies
            .iter()
            .filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::Moon | BodyKind::Asteroid))
            .filter(|b| g.bodies[b.id.0 as usize].owner.is_none())
            .map(|b| {
                let d = g.system.pos(b.id, t_next).dist(g.system.pos(home, t_next));
                (b.id, ((d / r).powi(2) / scout).powi(2).ceil() as usize)
            })
            .find(|&(_, n)| (3..=12).contains(&n))
        else {
            continue;
        };
        for _ in 0..need - 1 {
            put_ship(&mut g, 1, ShipClass::Scout, b);
        }
        tick(&mut g);
        let seen = |g: &Game| g.view_for(0).ships.iter().filter(|s| s.owner == 1 && s.parked_at() == Some(b)).count();
        assert_eq!(seen(&g), 0, "seed {seed}: {} scouts at {b:?} should stay hidden", need - 1);
        put_ship(&mut g, 1, ShipClass::Scout, b);
        tick(&mut g);
        assert_eq!(seen(&g), need, "seed {seed}: {need} scouts glow enough to be seen");
        return;
    }
    panic!("no seed had a suitable body");
}

#[test]
fn view_never_leaks_undetected_ships_or_enemy_plans() {
    for seed in [1, 5, 9] {
        let mut g = game4(seed);
        for p in 0..4u32 {
            let mine: Vec<ShipId> = g.ships.iter().filter(|s| s.owner == p && s.class != ShipClass::Tanker).map(|s| s.id).collect();
            let target = g.players[((p + 1) % 4) as usize].home.unwrap();
            let _ = g.apply(p, Command::Transfer { ships: mine[..3].to_vec(), to: target, depart_t: None });
        }
        for _ in 0..6 {
            let t = g.t + 6.0 * 3600.0;
            g.advance_to(t);
            for p in 0..4u32 {
                let v = g.view_for(p);
                let vis = &g.players[p as usize].vision;
                assert_eq!(v.ships.iter().filter(|s| s.owner == p).count(), g.ships.iter().filter(|s| s.owner == p).count());
                for s in v.ships.iter().filter(|s| s.owner != p) {
                    assert!(vis.ships.binary_search(&s.id).is_ok(), "seed {seed}: p{p} got undetected {:?}", s.id);
                    let scanned = vis.scanned.binary_search(&s.id).is_ok();
                    assert!(scanned || s.route.is_empty(), "enemy route leaked");
                    match &s.state {
                        ShipState::Parked { departure, .. } => assert!(scanned || departure.is_none(), "enemy departure leaked"),
                        ShipState::Flying(f) => assert!(scanned || f.burns.is_empty(), "enemy burns leaked"),
                    }
                    assert_eq!(s.name, s.class.stats().name, "enemy ship names reveal fleet sizes");
                }
                for (i, pl) in v.players.iter().enumerate() {
                    if i as u32 != p {
                        assert!(pl.matter == 0.0 && pl.energy == 0.0 && pl.intel.is_empty() && pl.contacts.is_empty() && pl.home.is_none());
                    }
                }
                for b in &v.bodies {
                    assert!(b.queue.iter().all(|j| j.owner == p), "enemy build queue leaked");
                }
                for c in &v.players[p as usize].contacts {
                    assert!(v.ships.iter().all(|s| s.id != c.ship.id), "a ghost duplicates a live ship");
                }
                assert!(v.events.iter().all(|e| e.to.is_none() || e.to == Some(p)));
            }
        }
    }
}

#[test]
fn lost_contacts_become_ghosts_on_their_last_known_arc() {
    let mut g = game4(3);
    let home = g.players[0].home.unwrap();
    // an enemy dreadnought parked at our home is certainly seen...
    let id = put_ship(&mut g, 2, ShipClass::Dreadnought, home);
    tick(&mut g);
    assert!(g.view_for(0).ships.iter().any(|s| s.id == id));
    assert!(g.view_for(0).players[0].contacts.iter().all(|c| c.ship.id != id), "no ghost while in sight");
    // ...then it slips away to the farthest body (teleported so the test doesn't depend on flight)
    let t = g.last_tick + TICK;
    let far = g
        .system
        .bodies
        .iter()
        .max_by(|a, b| g.system.pos(a.id, t).dist(g.system.pos(home, t)).total_cmp(&g.system.pos(b.id, t).dist(g.system.pos(home, t))))
        .unwrap()
        .id;
    let seen_t = g.t;
    let s = g.ships.iter_mut().find(|s| s.id == id).unwrap();
    s.state = ShipState::Parked { body: far, departure: None };
    tick(&mut g);
    let v = g.view_for(0);
    assert!(v.ships.iter().all(|s| s.id != id), "out of range now");
    let c = v.players[0].contacts.iter().find(|c| c.ship.id == id).expect("ghost contact");
    assert_eq!(c.seen_t, seen_t);
    assert_eq!(c.ship.parked_at(), Some(home), "ghost stays where it was last seen");
    // a destroyed ship whose last position we can see is forgotten
    g.ships.retain(|s| s.id != id);
    tick(&mut g);
    assert!(g.view_for(0).players[0].contacts.iter().all(|c| c.ship.id != id));
}

#[test]
fn intel_power_counts_only_detected_ships() {
    let mut g = game4(4);
    tick(&mut g);
    for p in 0..4u32 {
        let pl = &g.players[p as usize];
        for (i, intel) in pl.intel.iter().enumerate() {
            let Some(intel) = intel else { continue };
            for &(q, pow) in &intel.power {
                if q == p || g.bodies[i].owner == Some(q) {
                    continue;
                }
                assert!(
                    g.ships.iter().any(|s| s.owner == q && s.parked_at() == Some(BodyId(i as u32)) && pl.vision.ships.binary_search(&s.id).is_ok()),
                    "p{p} intel shows {pow} power of p{q} at body {i} without seeing a ship"
                );
            }
        }
    }
}

#[test]
fn view_round_trips_json_and_is_smaller_than_the_full_state() {
    let mut g = game4(6);
    g.advance_to(4.0 * 3600.0);
    let full = serde_json::to_string(&g).unwrap().len();
    for p in 0..4 {
        let v = g.view_for(p);
        let js = serde_json::to_string(&v).unwrap();
        assert!(js.len() < full, "view {} >= full {full}", js.len());
        let back: Game = serde_json::from_str(&js).unwrap();
        assert_eq!(back.ships.len(), v.ships.len());
        // the client can keep predicting from its view without panicking
        let mut c = back.clone();
        c.advance_to(c.t + 6.0 * 3600.0);
    }
}


/// `cargo test --release -p simtests --test fog -- --ignored --nocapture fog_cost`
#[test]
#[ignore = "benchmark"]
fn fog_cost() {
    let mut g = game4(5);
    for p in 0..4u32 {
        let mine: Vec<ShipId> = g.ships.iter().filter(|s| s.owner == p && s.class != ShipClass::Tanker).map(|s| s.id).collect();
        let target = g.players[((p + 1) % 4) as usize].home.unwrap();
        let _ = g.apply(p, Command::Transfer { ships: mine[..3].to_vec(), to: target, depart_t: None });
    }
    let t0 = std::time::Instant::now();
    g.advance_to(86_400.0);
    let adv = t0.elapsed();
    let t1 = std::time::Instant::now();
    for _ in 0..100 {
        for p in 0..4 {
            std::hint::black_box(g.view_for(p));
        }
    }
    println!("advance 1 day: {adv:?}; view_for x400: {:?}; ships {}", t1.elapsed(), g.ships.len());
}

#[test]
fn ghost_contacts_are_capped_and_expire() {
    let mut g = game4(7);
    let home = g.players[0].home.unwrap();
    for _ in 0..(fog::MAX_CONTACTS + 20) {
        put_ship(&mut g, 1, ShipClass::Dreadnought, home);
    }
    tick(&mut g);
    assert!(g.players[0].contacts.len() <= fog::MAX_CONTACTS);
    // move them all far away out of sight, then let the ghosts age out
    let far = g.players.iter().map(|p| p.home.unwrap()).max_by(|a, b| {
        let d = |x: &BodyId| g.system.pos(*x, g.t).dist(g.system.pos(home, g.t));
        d(a).total_cmp(&d(b))
    }).unwrap();
    for s in g.ships.iter_mut().filter(|s| s.owner == 1 && s.class == ShipClass::Dreadnought) {
        s.state = ShipState::Parked { body: far, departure: None };
    }
    let t = g.t + fog::CONTACT_TTL + 2.0 * TICK;
    g.advance_to(t);
    assert!(g.players[0].contacts.iter().all(|c| g.t - c.seen_t < fog::CONTACT_TTL), "stale ghosts kept");
}

#[test]
fn vision_is_not_saved_or_sent() {
    let mut g = game4(8);
    tick(&mut g);
    assert!(!g.players[0].vision.ships.is_empty());
    let js = serde_json::to_string(&g).unwrap();
    assert!(!js.contains("\"vision\""));
}

fn put_station(g: &mut Game, owner: PlayerId, kind: sim::station::StationKind, body: BodyId) {
    let id = sim::station::StationId(g.stations.len() as u32 + 1000);
    g.stations.push(sim::station::Station { id, owner, kind, body, hp: kind.max_hp(), fuel: 0.0, done_t: 0.0 });
}

/// An unowned body with an enemy fleet out of player 0's sight, for station tests.
fn hidden_enemy_at(g: &mut Game) -> (BodyId, ShipId) {
    let home = g.players[0].home.unwrap();
    let t = g.last_tick + TICK;
    let far = g
        .system
        .bodies
        .iter()
        .filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::Moon) && g.bodies[b.id.0 as usize].owner.is_none())
        .max_by(|a, b| g.system.pos(a.id, t).dist(g.system.pos(home, t)).total_cmp(&g.system.pos(b.id, t).dist(g.system.pos(home, t))))
        .unwrap()
        .id;
    let id = put_ship(g, 1, ShipClass::Interceptor, far);
    (far, id)
}

#[test]
fn telescope_stations_extend_sight_and_scanners_reveal_plans() {
    use sim::station::StationKind;
    let mut g = game4(11);
    let (far, id) = hidden_enemy_at(&mut g);
    put_station(&mut g, 1, StationKind::Defense, far);
    tick(&mut g);
    let v = g.view_for(0);
    assert!(v.ships.iter().all(|s| s.id != id), "enemy at {far:?} is out of sight");
    assert!(v.stations.iter().all(|s| s.owner == 0), "enemy station at an unseen body leaked");
    // a telescope station of ours at the same body sees the interceptor and the station
    put_station(&mut g, 0, StationKind::Telescope, far);
    // give the enemy a departure plan to check plan stripping
    let target = g.players[0].home.unwrap();
    let _ = g.apply(1, Command::Transfer { ships: vec![id], to: target, depart_t: None });
    tick(&mut g);
    let v = g.view_for(0);
    let s = v.ships.iter().find(|s| s.id == id).expect("telescope station sees the interceptor");
    assert!(matches!(s.state, ShipState::Parked { departure: None, .. }), "telescope alone doesn't reveal plans");
    assert!(v.stations.iter().any(|s| s.owner == 1 && s.kind == StationKind::Defense));
    // a scanner reveals the departure plan
    put_station(&mut g, 0, StationKind::Scanner, far);
    tick(&mut g);
    let v = g.view_for(0);
    let s = v.ships.iter().find(|s| s.id == id).unwrap();
    if g.ship(id).is_some_and(|s| matches!(s.state, ShipState::Parked { departure: Some(_), .. })) {
        assert!(matches!(s.state, ShipState::Parked { departure: Some(_), .. }), "scanner should reveal the departure");
    }
    assert!(g.players[0].vision.scanned_bodies.binary_search(&far).is_ok());
}

#[test]
fn relays_boost_nearby_sensors() {
    use sim::station::StationKind;
    let mut g = game4(12);
    tick(&mut g);
    let home = g.players[0].home.unwrap();
    let before = g.sensors(0);
    put_station(&mut g, 0, StationKind::Relay, home);
    let after = g.sensors(0);
    let tel = |v: &[Sensor]| v.iter().filter(|s| s.kind == SensorKind::Passive).map(|s| s.range).fold(0.0, f64::max);
    assert!((tel(&after) / tel(&before) - fog::RELAY_BOOST).abs() < 1e-9);
}

#[test]
fn other_players_research_is_hidden() {
    let mut g = game4(13);
    g.players[1].techs.push(sim::tech::Tech::BetterOptics);
    tick(&mut g);
    let v = g.view_for(0);
    assert!(v.players[1].techs.is_empty() && v.players[1].research.is_empty());
    assert_eq!(g.view_for(1).players[1].techs.len(), 1);
}

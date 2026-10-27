//! Game rules (`sim::game`): determinism, commands, fuel, combat, ownership, game end.
use sim::game::*;
use sim::system::{Rng, SystemSize};
use sim::{BodyId, BodyKind};

const HOUR: f64 = 3600.0;

fn settings(players: usize) -> Settings {
    Settings { name: "test".into(), size: SystemSize::Medium, max_players: players, length: 7.0 * 86_400.0, speed: 1.0 }
}

/// A started 4-player game.
fn game4(seed: u64) -> Game {
    let mut g = Game::new("TEST".into(), settings(4), seed, "ann");
    for n in ["bob", "cat", "dan"] {
        g.add_player(n, None).unwrap();
    }
    g.start();
    g
}

fn json(g: &Game) -> String {
    serde_json::to_string(g).unwrap()
}

/// A neutral, undefended rocky body nobody starts on.
fn neutral_body(g: &Game) -> BodyId {
    g.system
        .bodies
        .iter()
        .find(|b| matches!(b.kind, BodyKind::Planet | BodyKind::Moon) && g.bodies[b.id.0 as usize].owner.is_none())
        .map(|b| b.id)
        .expect("a neutral body")
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
        last_burn_t: sim::game::no_burn(),
        intercept: None,
    });
    id
}

/// Some transfer orders so a determinism run has flights, captures and combat in it.
fn busy_commands(g: &mut Game) {
    for p in 0..4u32 {
        let mine: Vec<ShipId> = g.ships.iter().filter(|s| s.owner == p && s.class != ShipClass::Tanker).map(|s| s.id).collect();
        let target = g.players[((p + 1) % 4) as usize].home.unwrap();
        let _ = g.apply(p, Command::Transfer { ships: mine[..3].to_vec(), to: target, depart_t: None });
        let _ = g.apply(p, Command::Chat { text: format!("hi from {p}") });
    }
}

// ------------------------------------------------------------------ lobby & commands

#[test]
fn lobby_caps_players_and_dedupes_names() {
    let mut g = Game::new("ABCD".into(), settings(4), 1, "ann");
    assert_eq!(g.status, Status::Lobby);
    assert_eq!(g.add_player("bob", None), Ok(1));
    assert_eq!(g.add_player("bob", None), Ok(1), "rejoining returns the same id");
    g.add_player("cat", Some("easy".into())).unwrap();
    g.add_player("dan", None).unwrap();
    assert!(g.add_player("eve", None).is_err(), "5th player");
    assert!(g.apply(0, Command::Transfer { ships: vec![], to: BodyId(1), depart_t: None }).is_err(), "commands before start");
    assert!(g.apply(0, Command::Chat { text: "lobby chat ok".into() }).is_ok());
    g.start();
    assert_eq!(g.status, Status::Running);
    assert!(g.add_player("late", None).is_err(), "can't join a running game");
}

#[test]
fn start_gives_each_player_a_distinct_home_and_fleet() {
    let g = game4(3);
    let homes: Vec<BodyId> = g.players.iter().map(|p| p.home.expect("home")).collect();
    let mut dedup = homes.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), 4, "homes {homes:?}");
    for p in &g.players {
        let home = p.home.unwrap();
        assert_eq!(g.bodies[home.0 as usize].owner, Some(p.id));
        let fleet: Vec<_> = g.ships.iter().filter(|s| s.owner == p.id).collect();
        assert!(!fleet.is_empty());
        assert!(fleet.iter().all(|s| s.parked_at() == Some(home)));
        // equal start: same fleet composition for everyone
        let comp = |id| { let mut v: Vec<_> = g.ships.iter().filter(|s| s.owner == id).map(|s| format!("{:?}", s.class)).collect(); v.sort(); v };
        assert_eq!(comp(p.id), comp(0));
    }
}

#[test]
fn commands_on_other_players_ships_are_rejected() {
    let mut g = game4(4);
    let bobs = g.ships.iter().find(|s| s.owner == 1).unwrap().id;
    let before = json(&g);
    assert!(g.apply(0, Command::SetOrder { ship: bobs, order: StandingOrder::Retreat }).is_err());
    assert!(g.apply(0, Command::CancelDeparture { ship: bobs }).is_err());
    assert!(g.apply(0, Command::Transfer { ships: vec![bobs], to: BodyId(1), depart_t: None }).is_err());
    assert!(g.apply(0, Command::Build { body: g.players[1].home.unwrap(), class: ShipClass::Scout }).is_err());
    assert!(g.apply(1, Command::Extend { seconds: 86_400.0 }).is_err(), "only the host extends");
    assert!(g.apply(0, Command::Transfer { ships: vec![ShipId(99_999)], to: BodyId(1), depart_t: None }).is_err());
    assert_eq!(before, json(&g), "rejected commands must not change state");
}

#[test]
fn unaffordable_build_changes_nothing() {
    let mut g = game4(5);
    let home = g.players[0].home.unwrap();
    g.players[0].matter = 1.0;
    let before = json(&g);
    assert!(g.apply(0, Command::Build { body: home, class: ShipClass::Gunship }).is_err());
    assert_eq!(before, json(&g));
}

#[test]
fn build_completes_after_build_time_and_costs_resources() {
    let mut g = game4(5);
    let home = g.players[0].home.unwrap();
    let (m0, n0) = (g.players[0].matter, g.ships.iter().filter(|s| s.owner == 0).count());
    let st = ShipClass::Interceptor.stats();
    g.apply(0, Command::Build { body: home, class: ShipClass::Interceptor }).unwrap();
    assert!((g.players[0].matter - (m0 - st.matter)).abs() < 1e-9);
    g.advance_to(st.build_time - TICK);
    assert_eq!(g.ships.iter().filter(|s| s.owner == 0).count(), n0, "built too early");
    g.advance_to(st.build_time + 2.0 * TICK);
    let n1 = g.ships.iter().filter(|s| s.owner == 0 && s.parked_at() == Some(home)).count();
    assert!(n1 > n0, "interceptor not built");
}

#[test]
fn extend_moves_the_end_and_game_ends_with_a_winner() {
    let mut g = game4(6);
    let end = g.end_t;
    g.apply(0, Command::Extend { seconds: 86_400.0 }).unwrap();
    assert_eq!(g.end_t, end + 86_400.0);
    // give dan a big lead in territory
    for b in g.bodies.iter_mut().filter(|b| b.owner.is_none()).take(3) {
        b.owner = Some(3);
    }
    // keep the run short: pull the end in
    g.end_t = g.t + 2.0 * TICK;
    g.advance_to(g.t + 10.0 * TICK);
    assert_eq!(g.status, Status::Ended);
    assert_eq!(g.winner, Some(3));
    assert!(g.apply(0, Command::Build { body: g.players[0].home.unwrap(), class: ShipClass::Scout }).is_err());
    assert!(g.apply(0, Command::Chat { text: "gg".into() }).is_ok(), "chat still works after the end");
}

// ------------------------------------------------------------------ determinism

#[test]
fn same_seed_same_commands_same_game() {
    let run = || {
        let mut g = game4(11);
        busy_commands(&mut g);
        g.advance_to(30.0 * HOUR);
        g
    };
    let (a, b) = (run(), run());
    assert_eq!(json(&a), json(&b));
}

#[test]
fn advancing_in_chunks_equals_advancing_at_once() {
    // the server advances whenever it wakes up, at irregular times; the result must not depend on it
    let mut a = game4(12);
    busy_commands(&mut a);
    let mut b = a.clone();
    a.advance_to(20.0 * HOUR);
    let mut rng = Rng::new(5);
    let mut t = 0.0;
    while t < 20.0 * HOUR {
        t = (t + rng.range(1.0, 5000.0)).min(20.0 * HOUR);
        b.advance_to(t);
    }
    assert_eq!(a.t, b.t);
    assert_eq!(json(&a), json(&b), "state depends on how time was chunked");
}

#[test]
fn save_and_reload_mid_flight_continues_identically() {
    let mut a = game4(13);
    busy_commands(&mut a);
    // run until someone is mid-flight (departures wait for their transfer window)
    let flying = |g: &Game| g.ships.iter().any(|s| matches!(s.state, ShipState::Flying(_)));
    while !flying(&a) && a.t < 72.0 * HOUR {
        a.advance_to(a.t + HOUR);
    }
    assert!(flying(&a), "want ships in flight");
    let mut b: Game = serde_json::from_str(&json(&a)).unwrap();
    let t_end = a.t + 20.0 * HOUR;
    a.advance_to(t_end);
    b.advance_to(t_end);
    assert_eq!(json(&a), json(&b), "reloaded game diverged");
}

// ------------------------------------------------------------------ fuel

#[test]
fn rocket_equation() {
    for c in ShipClass::ALL {
        let st = c.stats();
        assert!(st.ve > 0.0 && st.dry_mass > 0.0);
        assert_eq!(fuel_needed(c, st.fuel_cap, 0.0), 0.0);
        // burning dv_left uses exactly all the fuel
        let full = Ship {
            id: ShipId(1), owner: 0, class: c, name: String::new(), hp: st.hp, fuel: st.fuel_cap,
            order: StandingOrder::Hold, route: vec![], state: ShipState::Parked { body: BodyId(1), departure: None },
            tech: Default::default(),
            last_burn_t: sim::game::no_burn(),
        intercept: None,
        };
        let dv = full.dv_left();
        assert!((full.fuel_for(dv) - st.fuel_cap).abs() < 1e-9 * st.fuel_cap, "{c:?}");
        // two half burns cost the same as one full burn (Tsiolkovsky is path-independent)
        let f1 = fuel_needed(c, st.fuel_cap, dv / 2.0);
        let f2 = fuel_needed(c, st.fuel_cap - f1, dv / 2.0);
        assert!((f1 + f2 - st.fuel_cap).abs() < 1e-9 * st.fuel_cap, "{c:?}");
        // a heavier (fuller) ship pays more for the same dv
        assert!(fuel_needed(c, st.fuel_cap, 1.0) > fuel_needed(c, st.fuel_cap * 0.5, 1.0));
    }
}

#[test]
fn ships_can_reach_a_neighbour_planet_on_a_full_tank() {
    // design: fuel matters, but a full combat ship should manage one adjacent-planet hop
    let g = game4(14);
    let p = &g.players[0];
    let home = p.home.unwrap();
    let ship = g.ships.iter().find(|s| s.owner == 0 && s.class == ShipClass::Gunship).unwrap();
    let mut ok = 0;
    let mut tried = vec![];
    for b in g.system.bodies.iter().filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant) && b.id != home) {
        if let Ok((plan, _)) = g.plan_for(ship, b.id, None) {
            tried.push(format!("{} {:.1}", b.name, plan.total_dv()));
            if plan.total_dv() <= ship.dv_left() {
                ok += 1;
            }
        }
    }
    println!("gunship dv_left {:.1} km/s; plans: {tried:?}", ship.dv_left());
    assert!(ok >= 1, "a full gunship can't reach any planet (dv_left {:.1}, plans {tried:?})", ship.dv_left());
}

#[test]
fn transfer_spends_departure_fuel_then_arrives_parked() {
    let mut g = game4(15);
    let ship = g.ships.iter().find(|s| s.owner == 0 && s.class == ShipClass::Gunship).unwrap().clone();
    let to = g.players[1].home.unwrap();
    let (plan, rest) = g.plan_for(&ship, to, None).unwrap();
    g.apply(0, Command::Transfer { ships: vec![ship.id], to, depart_t: None }).unwrap();
    let fuel0 = g.ship(ship.id).unwrap().fuel;
    g.advance_to(plan.depart_t() + 1.0);
    let s = g.ship(ship.id).unwrap();
    assert!(matches!(s.state, ShipState::Flying(_)), "should have departed");
    let spent = fuel0 - s.fuel;
    assert!((spent - fuel_needed(ShipClass::Gunship, fuel0, plan.depart_dv)).abs() < 1e-6, "spent {spent}");
    if rest.is_empty() {
        g.advance_to(plan.arrive_t + 2.0 * TICK);
        if let Some(s) = g.ship(ship.id) {
            // it may have died in combat at bob's home; if alive it must be parked there
            assert_eq!(s.parked_at(), Some(to), "{:?}", s.state);
        }
    }
}

#[test]
fn departure_without_fuel_is_refused() {
    let mut g = game4(16);
    let ship = g.ships.iter().find(|s| s.owner == 0 && s.class == ShipClass::Gunship).unwrap().id;
    g.ships.iter_mut().find(|s| s.id == ship).unwrap().fuel = 0.01;
    let to = g.players[1].home.unwrap();
    assert!(g.apply(0, Command::Transfer { ships: vec![ship], to, depart_t: None }).is_err());
}

#[test]
fn tanker_refuel_conserves_fuel() {
    let mut g = game4(17);
    let x = neutral_body(&g);
    g.ships.clear();
    let tk = put_ship(&mut g, 0, ShipClass::Tanker, x);
    let gs = put_ship(&mut g, 0, ShipClass::Gunship, x);
    g.ships.iter_mut().find(|s| s.id == gs).unwrap().fuel = 0.0;
    let total0: f64 = g.ships.iter().map(|s| s.fuel).sum();
    g.advance_to(g.t + 2.0 * HOUR);
    let (t, s) = (g.ship(tk).unwrap(), g.ship(gs).unwrap());
    assert!(s.fuel > 0.0, "tanker didn't refuel the gunship");
    assert!(t.fuel >= 0.3 * ShipClass::Tanker.stats().fuel_cap - 1e-9, "tanker gave away its reserve");
    assert!(s.fuel <= ShipClass::Gunship.stats().fuel_cap + 1e-9, "overfilled");
    let total1: f64 = g.ships.iter().map(|s| s.fuel).sum();
    assert!((total0 - total1).abs() < 1e-6, "fuel created/destroyed: {total0} -> {total1}");
}

// ------------------------------------------------------------------ combat & ownership

/// Fight fleet `a` (player 0) against fleet `b` (player 1) at a neutral body for up to `hours`.
/// Returns surviving (hp, count) for each side.
fn duel(a: &[(ShipClass, usize)], b: &[(ShipClass, usize)], hours: f64, shuffle: Option<u64>) -> (Game, BodyId) {
    let mut g = game4(21);
    let x = neutral_body(&g);
    g.ships.clear();
    for &(c, n) in a {
        for _ in 0..n {
            put_ship(&mut g, 0, c, x);
        }
    }
    for &(c, n) in b {
        for _ in 0..n {
            put_ship(&mut g, 1, c, x);
        }
    }
    if let Some(seed) = shuffle {
        let mut r = Rng::new(seed);
        for i in (1..g.ships.len()).rev() {
            let j = r.below(i as u32 + 1) as usize;
            g.ships.swap(i, j);
        }
    }
    g.advance_to(g.t + hours * HOUR);
    (g, x)
}

fn side(g: &Game, p: PlayerId) -> (usize, f64) {
    let v: Vec<_> = g.ships.iter().filter(|s| s.owner == p).collect();
    (v.len(), v.iter().map(|s| s.hp).sum())
}

fn cost(f: &[(ShipClass, usize)]) -> f64 {
    f.iter().map(|&(c, n)| (c.stats().matter + c.stats().energy) * n as f64).sum()
}

#[test]
fn combat_outcome_does_not_depend_on_ship_list_order() {
    let a = [(ShipClass::Gunship, 2), (ShipClass::Interceptor, 3), (ShipClass::Bomber, 1)];
    let b = [(ShipClass::Bomber, 2), (ShipClass::Interceptor, 2), (ShipClass::Dreadnought, 1)];
    let (g0, _) = duel(&a, &b, 3.0, None);
    let mut base: Vec<(u32, u64)> = g0.ships.iter().map(|s| (s.id.0, s.hp.to_bits())).collect();
    base.sort();
    for seed in 1..6 {
        let (g, _) = duel(&a, &b, 3.0, Some(seed));
        let mut got: Vec<(u32, u64)> = g.ships.iter().map(|s| (s.id.0, s.hp.to_bits())).collect();
        got.sort();
        assert_eq!(base, got, "shuffle {seed} changed the battle");
    }
}

#[test]
fn rock_paper_scissors_at_equal_cost() {
    use ShipClass::*;
    // (winner, loser): interceptors beat bombers, gunships beat interceptors, bombers beat gunships
    for (w, l) in [(Interceptor, Bomber), (Gunship, Interceptor), (Bomber, Gunship)] {
        let budget = 1200.0;
        let n = |c: ShipClass| (budget / (c.stats().matter + c.stats().energy)).round().max(1.0) as usize;
        let (fa, fb) = ([(w, n(w))], [(l, n(l))]);
        let (g, _) = duel(&fa, &fb, 24.0, None);
        let (na, _) = side(&g, 0);
        let (nb, _) = side(&g, 1);
        assert!(
            na > 0 && nb == 0,
            "{w:?}×{} (cost {}) vs {l:?}×{} (cost {}): survivors {na} vs {nb}",
            n(w), cost(&fa), n(l), cost(&fb)
        );
    }
}

#[test]
fn unarmed_ships_never_fight() {
    use ShipClass::*;
    let (g, _) = duel(&[(Scout, 2), (Tanker, 1)], &[(Tanker, 2)], 12.0, None);
    assert_eq!(g.ships.len(), 5);
    assert!(g.ships.iter().all(|s| s.hp == s.class.stats().hp));
}

#[test]
fn ships_at_different_bodies_never_fight() {
    let mut g = game4(22);
    let bodies: Vec<BodyId> = g.system.bodies.iter().filter(|b| b.kind == BodyKind::Moon && g.bodies[b.id.0 as usize].owner.is_none()).map(|b| b.id).take(2).collect();
    g.ships.clear();
    put_ship(&mut g, 0, ShipClass::Gunship, bodies[0]);
    put_ship(&mut g, 1, ShipClass::Gunship, bodies[1]);
    g.advance_to(g.t + 6.0 * HOUR);
    assert!(g.ships.iter().all(|s| s.hp == s.class.stats().hp));
}

#[test]
fn sole_power_in_orbit_takes_the_body() {
    let mut g = game4(23);
    let x = neutral_body(&g);
    put_ship(&mut g, 2, ShipClass::Interceptor, x);
    g.advance_to(g.t + 2.0 * TICK);
    assert_eq!(g.bodies[x.0 as usize].owner, Some(2));
    assert!(g.events_for(2).any(|e| e.title == "Territory gained" && e.body_ref == Some(x)));
    // unarmed ships alone don't claim anything
    let y = g.system.bodies.iter().find(|b| b.kind == BodyKind::Moon && g.bodies[b.id.0 as usize].owner.is_none()).unwrap().id;
    put_ship(&mut g, 3, ShipClass::Scout, y);
    g.advance_to(g.t + 2.0 * TICK);
    assert_eq!(g.bodies[y.0 as usize].owner, None);
}

#[test]
fn winner_of_a_battle_ends_up_owning_the_body() {
    use ShipClass::*;
    let (g, x) = duel(&[(Gunship, 4)], &[(Interceptor, 3)], 24.0, None);
    assert_eq!(side(&g, 1).0, 0);
    assert_eq!(g.bodies[x.0 as usize].owner, Some(0));
}

#[test]
fn planetary_defense_fights_invaders() {
    let mut g = game4(24);
    let home = g.players[1].home.unwrap();
    g.ships.retain(|s| s.owner != 1);
    let def0 = g.bodies[home.0 as usize].def_hp;
    assert!(def0 > 0.0);
    let inv = put_ship(&mut g, 0, ShipClass::Bomber, home);
    g.advance_to(g.t + 2.0 * HOUR);
    let hurt = g.ship(inv).map(|s| s.hp < s.class.stats().hp).unwrap_or(true);
    assert!(hurt, "defenses didn't shoot");
    assert!(g.bodies[home.0 as usize].def_hp < def0 || g.ship(inv).is_none());
}

/// Design (round 4): weapons account for relative velocity — energy is instant, kinetic must lead
/// and does more damage.
#[test]
fn kinetic_accuracy_falls_with_relative_velocity() {
    use sim::combat::{evasion, shot, size, KINETIC_RANGE};
    use sim::v2;
    let c = ShipClass::Gunship;
    let r = v2(800.0, 0.0);
    let mut last = 2.0;
    for v in [0.0, 1.0, 3.0, 6.0, 10.0, 15.0, 20.0] {
        let k = shot(Weapon::Kinetic, r, v2(0.0, v), size(c), evasion(c)).expect("in range, catchable");
        assert!(k.hit <= last, "kinetic hit {} at {v} km/s should not rise", k.hit);
        last = k.hit;
        // beams arrive at light speed: the target's motion doesn't matter
        let e = shot(Weapon::Energy, r, v2(0.0, v), size(c), evasion(c)).unwrap();
        assert!((e.hit - 1.0).abs() < 1e-12 && e.tof < 0.01);
    }
    assert!(last < 0.5, "fast crossers should mostly dodge slugs");
    // kinetic out-damages energy against a slow target
    let k = shot(Weapon::Kinetic, r, sim::V2::ZERO, size(c), evasion(c)).unwrap();
    let e = shot(Weapon::Energy, r, sim::V2::ZERO, size(c), evasion(c)).unwrap();
    assert!(k.hit * k.factor > e.hit * e.factor);
    // a target crossing faster than the slugs can't be hit at all; nothing fires outside range
    assert!(shot(Weapon::Kinetic, r, v2(0.0, 40.0), size(c), evasion(c)).is_none());
    assert!(shot(Weapon::Kinetic, v2(KINETIC_RANGE * 1.01, 0.0), sim::V2::ZERO, size(c), evasion(c)).is_none());
}

/// A ship flying past an enemy fleet gets shot during the pass: beams always connect, slugs miss a
/// fast flyby.
#[test]
fn flybys_are_shot_by_energy_but_outrun_kinetic() {
    use sim::traj::State;
    use sim::v2;
    let run = |class: ShipClass, speed: f64| -> f64 {
        let mut g = game4(31);
        let x = neutral_body(&g);
        g.ships.clear();
        for _ in 0..3 {
            put_ship(&mut g, 1, class, x);
        }
        let id = put_ship(&mut g, 0, ShipClass::Dreadnought, x);
        let rp = g.system.body(x).park_radius() * 1.2;
        let vesc = (2.0 * g.system.body(x).mu / rp).sqrt();
        let s = g.ships.iter_mut().find(|s| s.id == id).unwrap();
        let start = State { parent: x, t: g.t, r: v2(rp, 0.0), v: v2(0.0, (vesc * vesc + speed * speed).sqrt()) };
        s.state = ShipState::Flying(Flight { start, burns: vec![], dest: None, cache: None });
        g.advance_to(g.t + 20.0 * 60.0);
        let hp = g.ship(id).map(|s| s.hp).unwrap_or(0.0);
        ShipClass::Dreadnought.stats().hp - hp
    };
    let laser_slow = run(ShipClass::Gunship, 2.0);
    let laser_fast = run(ShipClass::Gunship, 30.0);
    let slug_slow = run(ShipClass::Bomber, 2.0);
    let slug_fast = run(ShipClass::Bomber, 30.0);
    println!("laser {laser_slow:.1}/{laser_fast:.1}  slug {slug_slow:.1}/{slug_fast:.1}");
    assert!(laser_slow > 0.0 && laser_fast > 0.0, "beams should hit any flyby");
    assert!(slug_slow > 0.0, "slugs should hit a slow flyby");
    assert!(slug_fast < 0.25 * slug_slow, "a fast flyby should mostly outrun slugs");
    assert!(laser_fast < laser_slow, "a fast flyby spends less time in range");
}

/// Design (rounds 4–6): asteroids with drives can be crashed into planets; damage = ½ m v², and a
/// large enough impact wipes upgrades and everything in orbit.
#[test]
fn asteroid_impact_energy_scales_with_mass_and_velocity_squared() {
    use sim::asteroid::{impact_energy, plan_impact, strength, WIPE};
    assert_eq!(impact_energy(1000.0, 10.0), 50_000.0);
    assert_eq!(impact_energy(2000.0, 10.0), 2.0 * impact_energy(1000.0, 10.0));
    assert_eq!(impact_energy(1000.0, 30.0), 9.0 * impact_energy(1000.0, 10.0));

    // crash the heaviest asteroid with a course into player 1's home
    let mut g = game4(6);
    let home = g.players[1].home.unwrap();
    let mut asts: Vec<BodyId> = g.system.bodies.iter().filter(|b| b.kind == BodyKind::Asteroid).map(|b| b.id).collect();
    asts.sort_by(|a, b| g.system.body(*b).radius.total_cmp(&g.system.body(*a).radius));
    let (ast, plan) = asts.iter().find_map(|&a| plan_impact(&g, a, home, g.t).map(|p| (a, p))).expect("an impact course");
    let imp = plan.impact.unwrap();
    assert_eq!(imp.body, home);
    assert!((imp.energy - impact_energy(sim::asteroid::mass(&g.system, ast), imp.v)).abs() < 1e-6 * imp.energy);
    assert!((imp.severity - imp.energy / strength(&g.system, home)).abs() < 1e-9);
    // nobody owns it yet: rejected; then give it to player 0 with enough fuel
    assert!(g.apply(0, Command::AimAsteroid { body: ast, target: home }).is_err());
    g.bodies[ast.0 as usize].owner = Some(0);
    g.bodies[ast.0 as usize].fuel = plan.fuel + 1.0;
    g.players[0].matter = 1e5;
    g.players[0].energy = 1e5;
    assert!(g.apply(0, Command::AimAsteroid { body: ast, target: home }).is_err(), "needs Asteroid Drives research");
    g.players[0].techs.push(sim::tech::Tech::AsteroidDrives);
    g.apply(0, Command::AimAsteroid { body: ast, target: home }).unwrap();
    assert!(g.apply(0, Command::AimAsteroid { body: ast, target: home }).is_err(), "one drive at a time");
    let before = g.ships.iter().filter(|s| s.parked_at() == Some(home)).count();
    assert!(before > 0);
    g.advance_to(imp.t + 2.0 * TICK);
    assert!(g.bodies[ast.0 as usize].gone);
    assert!(g.events.iter().any(|e| e.title == "Impact" || e.title == "Planet shattered"));
    if imp.severity >= WIPE {
        assert_eq!(g.bodies[home.0 as usize].owner, None, "a wiping impact leaves nobody in charge");
        assert!(g.bodies[home.0 as usize].levels.iter().all(|&l| l == 0));
        assert_eq!(g.ships.iter().filter(|s| s.parked_at() == Some(home)).count(), 0, "everything in orbit dies");
    } else {
        panic!("expected the biggest asteroid to wipe a home planet, severity {}", imp.severity);
    }
}

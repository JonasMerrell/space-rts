//! Wars of up to eight pilots: slots, colours, homes, team modes, and old 4-player saves.
use sim::game::*;
use sim::system::SystemSize;
use sim::{BodyId, BodyKind};

const HOUR: f64 = 3600.0;

fn settings(size: SystemSize, players: usize, teams: usize) -> Settings {
    Settings { name: "eight".into(), size, max_players: players, length: 7.0 * 86_400.0, speed: 1.0, teams }
}

/// A started war with `n` pilots: "p0" hosts, the rest are hard bots.
fn war(size: SystemSize, n: usize, teams: usize, seed: u64) -> Game {
    let mut g = Game::new("EIGHT".into(), settings(size, n, teams), seed, "p0");
    for i in 1..n {
        g.add_player(&format!("Bot {i} (hard)"), Some("hard".into())).unwrap();
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
        last_burn_t: sim::game::no_burn(),
        intercept: None,
        cloak: None,
        mimic: None,
    });
    id
}

fn neutral_moon(g: &Game) -> BodyId {
    g.system.bodies.iter().find(|b| b.kind == BodyKind::Moon && g.bodies[b.id.0 as usize].owner.is_none()).map(|b| b.id).expect("a neutral moon")
}

#[test]
fn eight_pilots_get_distinct_homes_and_colours_in_every_size() {
    for size in [SystemSize::Small, SystemSize::Medium, SystemSize::Large] {
        for seed in 1..6 {
            let g = war(size, 8, 0, seed);
            assert_eq!(g.rules, RULES);
            let mut homes: Vec<BodyId> = g.players.iter().map(|p| p.home.unwrap_or_else(|| panic!("{size:?} seed {seed}: {} has no home", p.name))).collect();
            homes.sort();
            homes.dedup();
            assert_eq!(homes.len(), 8, "{size:?} seed {seed}: shared homes");
            let mut colors: Vec<usize> = g.players.iter().map(|p| p.color).collect();
            colors.dedup();
            assert_eq!(colors, (0..8).collect::<Vec<_>>());
            for p in &g.players {
                assert_eq!(g.bodies[p.home.unwrap().0 as usize].owner, Some(p.id));
            }
        }
    }
}

#[test]
fn eight_bot_war_runs_and_round_trips() {
    let mut g = war(SystemSize::Medium, 8, 0, 3);
    g.advance_to(12.0 * HOUR);
    assert_eq!(g.status, Status::Running);
    let back: Game = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
    assert_eq!(back.players.len(), 8);
    assert_eq!(back.ships.len(), g.ships.len());
    // every pilot still sees the system through fog
    for p in 0..8 {
        let v = g.view_for(p);
        assert!(v.ships.iter().any(|s| s.owner == p));
    }
}

#[test]
fn ninth_pilot_is_refused_and_slots_cap_joins() {
    let mut g = Game::new("FULL".into(), settings(SystemSize::Medium, 8, 0), 1, "p0");
    for i in 1..8 {
        g.add_player(&format!("p{i}"), None).unwrap();
    }
    assert!(g.add_player("p8", None).is_err());
    let mut g = Game::new("SIX".into(), settings(SystemSize::Medium, 6, 0), 1, "p0");
    for i in 1..6 {
        g.add_player(&format!("p{i}"), None).unwrap();
    }
    assert!(g.add_player("p6", None).is_err());
}

#[test]
fn team_options_split_slots_evenly() {
    assert_eq!(team_options(2), vec![0]);
    assert_eq!(team_options(4), vec![0, 2]);
    assert_eq!(team_options(5), vec![0]);
    assert_eq!(team_options(6), vec![0, 2, 3]);
    assert_eq!(team_options(8), vec![0, 2, 4]);
}

#[test]
fn joiners_fill_the_smallest_team_and_can_switch_while_there_is_room() {
    let mut g = Game::new("TEAM".into(), settings(SystemSize::Medium, 8, 4), 1, "p0");
    for i in 1..8 {
        g.add_player(&format!("p{i}"), None).unwrap();
    }
    for t in 0..4u8 {
        assert_eq!(g.players.iter().filter(|p| p.team == t).count(), 2, "team {t}");
    }
    assert!(g.set_team(0, 1).is_err(), "team 1 is full");
    assert!(g.set_team(0, 9).is_err(), "no team 9");
    g.remove_player(1).unwrap();
    let open = (0..4u8).find(|&t| g.players.iter().filter(|p| p.team == t).count() < 2).unwrap();
    let mover = g.players.iter().find(|p| p.team != open).unwrap().id;
    g.set_team(mover, open).unwrap();
    assert_eq!(g.players[mover as usize].team, open);
    // allies are teammates, nobody else
    for a in &g.players {
        for b in &g.players {
            assert_eq!(g.allied(a.id, b.id), a.team == b.team);
        }
    }
    g.start();
    assert!(g.set_team(0, open).is_err(), "teams lock at the start");
}

#[test]
fn free_for_all_has_no_allies() {
    let g = war(SystemSize::Medium, 4, 0, 2);
    assert!(!g.team_game());
    assert!(g.allied(1, 1));
    assert!(!g.allied(0, 1) && !g.allied(2, 3));
}

/// 2v2: ann + cat against bob + dan.
fn two_v_two() -> Game {
    let mut g = Game::new("2V2".into(), settings(SystemSize::Medium, 4, 2), 4, "ann");
    for n in ["bob", "cat", "dan"] {
        g.add_player(n, None).unwrap();
    }
    g.start();
    g
}

#[test]
fn teammates_hold_an_orbit_together_without_fighting() {
    let mut g = two_v_two();
    assert!(g.allied(0, 2) && g.allied(1, 3) && !g.allied(0, 1));
    let m = neutral_moon(&g);
    let a = put_ship(&mut g, 0, ShipClass::Gunship, m);
    let c = put_ship(&mut g, 2, ShipClass::Gunship, m);
    let t = g.t;
    g.advance_to(t + 2.0 * HOUR);
    for id in [a, c] {
        let s = g.ship(id).expect("teammates don't shoot each other");
        assert_eq!(s.hp, s.stats().hp);
    }
    let owner = g.bodies[m.0 as usize].owner.expect("the team claims it");
    assert!(g.allied(owner, 0));
    // an enemy arriving does fight
    let b = put_ship(&mut g, 1, ShipClass::Gunship, m);
    let t = g.t;
    g.advance_to(t + 2.0 * HOUR);
    let hurt = |g: &Game, id| g.ship(id).is_none_or(|s| s.hp < s.stats().hp);
    assert!(hurt(&g, b), "the enemy gunship takes fire");
    assert!(hurt(&g, a) || hurt(&g, c), "and shoots back");
}

#[test]
fn teammates_share_sensors_and_see_each_others_plans() {
    let mut g = two_v_two();
    // cat's scout parks far from ann's sensors, at bob's home
    let far = g.players[1].home.unwrap();
    let scout = put_ship(&mut g, 2, ShipClass::Scout, far);
    let t = g.t;
    g.advance_to(t + 120.0);
    assert!(g.players[0].vision.ships.binary_search(&scout).is_ok(), "ann sees cat's scout");
    let v = g.view_for(0);
    assert!(v.ships.iter().any(|s| s.id == scout));
    // and whatever cat's scout sees at bob's home, ann sees too
    let bobs: Vec<ShipId> = g.ships.iter().filter(|s| s.owner == 1 && s.parked_at() == Some(far)).map(|s| s.id).collect();
    assert!(!bobs.is_empty());
    assert!(bobs.iter().all(|id| g.players[0].vision.ships.binary_search(id).is_ok()));
    // the enemy team gets no such sharing: dan doesn't automatically see ann's ships
    assert!(!g.allied(3, 0));
}

#[test]
fn intercepting_a_teammate_is_refused() {
    let g = two_v_two();
    assert!(g.allied(0, 2));
    let mut g = g;
    let mine = g.ships.iter().find(|s| s.owner == 0).unwrap().id;
    let theirs = g.ships.iter().find(|s| s.owner == 2).unwrap().id;
    let t = g.t;
    let e = g.apply(0, Command::Intercept { ships: vec![mine], target: theirs, depart_t: t + 60.0, arrive_t: t + 4.0 * HOUR, matched: true });
    assert_eq!(e, Err("that's one of ours".into()));
}

#[test]
fn the_team_with_most_territory_wins() {
    let mut g = two_v_two();
    // hand bob + dan nearly everything, but make ann the single best pilot
    let free: Vec<usize> = (1..g.bodies.len()).filter(|&i| g.bodies[i].owner.is_none() && g.system.bodies[i].kind == BodyKind::Moon).collect();
    for (k, &i) in free.iter().enumerate() {
        g.bodies[i].owner = Some(if k < 3 { 0 } else if k % 2 == 0 { 1 } else { 3 });
    }
    let (s0, s1) = (g.team_score(0), g.team_score(1));
    assert!(s1 > s0, "{s1} > {s0}");
    let end = g.end_t;
    g.advance_to(end + 60.0);
    assert_eq!(g.status, Status::Ended);
    assert_eq!(g.winning_team, Some(g.players[1].team));
    assert!(g.winner.is_some_and(|w| g.allied(w, 1)));
}

/// A war saved by the server before 8-player support (rules 3, no teams): it must keep loading
/// and running exactly as a free-for-all, with its old system untouched.
#[test]
fn old_four_player_save_loads_and_keeps_running() {
    let json = include_str!("data/old4_rules3.json");
    let mut g: Game = serde_json::from_str(json).expect("old save loads");
    assert_eq!(g.rules, 3);
    assert_eq!(g.settings.teams, 0);
    assert_eq!(g.players.len(), 4);
    assert!(g.players.iter().all(|p| p.team == 0));
    assert!(!g.team_game() && !g.allied(0, 1));
    assert_eq!(g.winning_team, None);
    let bodies = g.system.bodies.len();
    let planets = g.system.bodies.iter().filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant)).count();
    assert_eq!(planets, 7, "the saved Medium system, not a regenerated one");
    let t = g.t;
    g.advance_to(t + 6.0 * HOUR);
    assert_eq!(g.status, Status::Running);
    assert_eq!(g.system.bodies.len(), bodies);
    assert!(g.players.iter().all(|p| p.home.is_some()));
    // and it round-trips with the new fields
    let back: Game = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
    assert_eq!(back.rules, 3);
}

//! Fog of war (`sim::fog`, `Game::view_for`): what the server sends each player.
use sim::fog::{body_sensor, ship_sensor};
use sim::game::*;
use sim::system::SystemSize;
use sim::{BodyId, V2};

fn game4(seed: u64) -> Game {
    let s = Settings { name: "fog".into(), size: SystemSize::Large, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0 };
    let mut g = Game::new("FOGG".into(), s, seed, "ann");
    for n in ["bob", "cat", "dan"] {
        g.add_player(n, None).unwrap();
    }
    g.start();
    g.advance_to(120.0);
    g
}

fn ship_at(g: &mut Game, owner: PlayerId, class: ShipClass, at: BodyId) -> ShipId {
    let id = ShipId(g.next_ship);
    g.next_ship += 1;
    let st = class.stats();
    g.ships.push(Ship {
        id, owner, class, name: format!("t{}", id.0), hp: st.hp, fuel: st.fuel_cap,
        order: StandingOrder::Hold, route: vec![], state: ShipState::Parked { body: at, departure: None }, tech: Default::default(), last_burn_t: sim::game::no_burn(),
        intercept: None,
        cloak: None,
        mimic: None,
    });
    id
}

#[test]
fn a_player_always_sees_all_of_their_own_stuff() {
    let g = game4(1);
    for p in 0..4 {
        let v = g.view_for(p);
        let mine = |g: &Game| { let mut v: Vec<_> = g.ships.iter().filter(|s| s.owner == p).map(|s| s.id).collect(); v.sort(); v };
        assert_eq!(mine(&v), mine(&g), "player {p} lost own ships");
        for (i, b) in g.bodies.iter().enumerate() {
            if b.owner == Some(p) {
                assert_eq!(serde_json::to_string(&v.bodies[i]).unwrap(), serde_json::to_string(b).unwrap());
            }
        }
        assert_eq!(v.players[p as usize].matter, g.players[p as usize].matter);
    }
}

/// `view_for` must agree with the vision the server computed at the last tick (`Player.vision`):
/// nothing outside it, and no enemy plans unless a scanner covers that ship.
#[test]
fn hidden_things_stay_hidden() {
    let mut g = game4(2);
    g.advance_to(g.t + 3.0 * TICK); // a few ticks so vision/intel are current
    for p in 0..4 {
        let v = g.view_for(p);
        let vis = &g.players[p as usize].vision;
        for s in &v.ships {
            if s.owner != p {
                assert!(vis.ships.binary_search(&s.id).is_ok(), "player {p} got enemy ship {} outside its vision", s.id.0);
                if vis.scanned.binary_search(&s.id).is_err() {
                    assert!(s.route.is_empty(), "enemy route leaked");
                    assert_eq!(s.fuel, 0.0, "enemy fuel leaked");
                    match &s.state {
                        ShipState::Parked { departure, .. } => assert!(departure.is_none(), "enemy departure plan leaked"),
                        ShipState::Flying(f) => assert!(f.burns.is_empty(), "enemy burns leaked"),
                    }
                }
            }
        }
        // every ship in vision does reach the client (no silent drops either)
        for id in &vis.ships {
            if g.ship(*id).is_some() {
                assert!(v.ship(*id).is_some(), "player {p} lost ship {} that it can see", id.0);
            }
        }
        for pl in &v.players {
            if pl.id != p {
                assert_eq!((pl.matter, pl.energy), (0.0, 0.0), "player {p} sees {}'s stockpile", pl.name);
                assert!(pl.intel.is_empty(), "player {p} gets {}'s intel", pl.name);
            }
        }
        assert!(v.events.iter().all(|e| e.to.is_none() || e.to == Some(p)), "someone else's events leaked");
        // other players' build queues never leak, seen or not
        for (i, b) in v.bodies.iter().enumerate() {
            assert!(b.queue.iter().all(|j| j.owner == p), "player {p} sees someone else's build queue at body {i}");
            if g.bodies[i].owner.is_some() && g.bodies[i].owner != Some(p) && vis.bodies.binary_search(&BodyId(i as u32)).is_err() {
                // unseen enemy body: whatever the client has comes from old intel, not live state
                let live = serde_json::to_string(&g.bodies[i]).unwrap();
                let shown = serde_json::to_string(b).unwrap();
                let intel = g.players[p as usize].intel.get(i).cloned().flatten().map(|x| serde_json::to_string(&x.state).unwrap());
                assert!(shown != live || intel.as_deref() == Some(&shown[..]), "player {p} gets live state of unseen body {i}");
            }
        }
    }
}

#[test]
fn big_ships_are_seen_from_further_than_small_ones() {
    let mut g = game4(3);
    // find a body that player 0 can't see at all, and put enemy ships there
    let sensors = g.sensors(0);
    let far = (0..g.system.bodies.len() as u32).map(BodyId).find(|&b| !g.system.body(b).is_star() && !g.sees_body(&sensors, b)).expect("an unseen body");
    let scout = ship_at(&mut g, 1, ShipClass::Scout, far);
    let dread = ship_at(&mut g, 1, ShipClass::Dreadnought, far);
    let at = g.system.pos(far, g.t);
    let d_sig = ShipClass::Dreadnought.stats().signature.sqrt();
    let s_sig = ShipClass::Scout.stats().signature.sqrt();
    assert!(d_sig > s_sig);
    // a single observing scout of player 0 at a distance between the two detection ranges
    let r = ship_sensor(ShipClass::Scout);
    let dist = r * (s_sig + d_sig) / 2.0;
    let sensors = vec![sim::fog::Sensor { pos: at + V2::from_angle(0.3) * dist, range: r, kind: sim::fog::SensorKind::Passive }];
    assert!(g.sees_ship(&sensors, g.ship(dread).unwrap()), "dreadnought not seen at {dist:.0} km");
    assert!(!g.sees_ship(&sensors, g.ship(scout).unwrap()), "scout seen at {dist:.0} km");
    // and in the real view (no observer there) neither is visible
    let v = g.view_for(0);
    assert!(v.ship(scout).is_none() && v.ship(dread).is_none());
}

#[test]
fn telescopes_extend_body_sensors() {
    let mut b = BodyState::default();
    let r0 = body_sensor(&b);
    b.levels[Upgrade::Telescope.index()] = 2;
    assert!(body_sensor(&b) > r0);
}

#[test]
fn intel_remembers_what_was_last_seen() {
    let mut g = game4(4);
    // an unowned body outside player 0's home sensors
    let home_sensors = g.sensors(0);
    let x = (0..g.system.bodies.len() as u32)
        .map(BodyId)
        .find(|&b| !g.system.body(b).is_star() && g.bodies[b.0 as usize].owner.is_none() && !g.sees_body(&home_sensors, b))
        .expect("a body outside home sensor range");
    // player 0 parks a tanker at x (sees it; any ship claims an empty body now), then leaves
    let sc = ship_at(&mut g, 0, ShipClass::Tanker, x);
    g.advance_to(g.t + 2.0 * TICK);
    assert!(g.players[0].intel[x.0 as usize].is_some(), "no intel recorded for a body we orbited");
    assert_eq!(g.bodies[x.0 as usize].owner, Some(0), "a tanker should claim an empty body");
    g.ships.retain(|s| s.id != sc);
    // the claim lapses while we're away: nobody is watching any more
    g.bodies[x.0 as usize].owner = None;
    g.advance_to(g.t + TICK);
    let seen_t = g.players[0].intel[x.0 as usize].as_ref().unwrap().seen_t;
    // bob takes it while we're not looking
    ship_at(&mut g, 1, ShipClass::Gunship, x);
    g.advance_to(g.t + 3.0 * TICK);
    assert_eq!(g.bodies[x.0 as usize].owner, Some(1));
    assert!(!g.sees_body(&g.sensors(0), x), "player 0 still has eyes on the body");
    let v = g.view_for(0);
    assert_ne!(v.bodies[x.0 as usize].owner, Some(1), "player 0 learned about the capture without seeing it");
    assert_eq!(g.players[0].intel[x.0 as usize].as_ref().unwrap().seen_t, seen_t);
}

#[test]
fn view_contains_no_secret_strings() {
    // cheap leak canary: another player's chat-free secrets (their stockpile numbers) don't appear
    let mut g = game4(5);
    g.players[2].matter = 123_456.789;
    let j = serde_json::to_string(&g.view_for(0)).unwrap();
    assert!(!j.contains("123456.789"), "player 2's matter leaked to player 0");
}

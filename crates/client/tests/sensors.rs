//! Sensors report (client/sensors.rs): built from the fogged view, so it shows threats and contacts
//! the player's sensors see and nothing they don't.
use space_rts::sensors::report;
use sim::game::*;
use sim::system::SystemSize;
use sim::{BodyId, BodyKind};
use ui::model::{Focus, ReportSection as Sec, When};

fn game4(seed: u64) -> Game {
    let s = Settings { name: "sens".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0 };
    let mut g = Game::new("SENS".into(), s, seed, "ann");
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
        name: st.name.into(),
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

fn tick(g: &mut Game) {
    let t = g.last_tick + TICK;
    g.advance_to(t);
}

/// The body farthest from `from` that player `p` can't see.
fn hidden_body(g: &Game, p: PlayerId, from: BodyId) -> BodyId {
    let t = g.t;
    let v = &g.players[p as usize].vision;
    g.system
        .bodies
        .iter()
        .filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::Moon) && v.bodies.binary_search(&b.id).is_err())
        .max_by(|a, b| g.system.pos(a.id, t).dist(g.system.pos(from, t)).total_cmp(&g.system.pos(b.id, t).dist(g.system.pos(from, t))))
        .expect("some body out of sensor range")
        .id
}

#[test]
fn enemies_over_home_are_threats_and_hidden_ones_stay_hidden() {
    let mut g = game4(3);
    tick(&mut g);
    let home = g.players[0].home.unwrap();
    let far = hidden_body(&g, 0, home);
    let near = put_ship(&mut g, 2, ShipClass::Dreadnought, home);
    let hidden = put_ship(&mut g, 1, ShipClass::Gunship, far);
    tick(&mut g);
    let view = g.view_for(0);
    let r = report(&view, 0, view.t, 0.0);
    let home_name = &g.system.body(home).name;
    let threat = r.items.iter().find(|i| i.section == Sec::Threats && i.text.contains(home_name)).expect("enemy in orbit of home is a threat");
    assert_eq!(threat.when, When::Live);
    assert!(threat.text.contains("cat"), "{}", threat.text);
    assert!(r.items.iter().any(|i| i.section == Sec::Ships && i.focus == Some(Focus::Ship(near.0))));
    // nothing anywhere points at, or describes, the gunship we can't see
    assert!(r.items.iter().all(|i| i.focus != Some(Focus::Ship(hidden.0))));
    assert!(!r.items.iter().any(|i| matches!(i.section, Sec::Ships | Sec::Ghosts | Sec::Threats) && i.title == "bob"));
    // the log has the sighting of the dreadnought, and it is new after time 0
    assert!(r.items.iter().any(|i| i.section == Sec::Log && i.title == "Contact"));
    assert!(r.unread() > 0);
    assert_eq!(report(&view, 0, view.t, view.t + 1.0).unread(), 0);
    // bob's own report doesn't list his gunship as a contact
    let vb = g.view_for(1);
    let rb = report(&vb, 1, vb.t, 0.0);
    assert!(rb.items.iter().all(|i| i.section != Sec::Ships || i.focus != Some(Focus::Ship(hidden.0))));
}

#[test]
fn own_transfer_shows_with_eta_and_unknown_bodies_are_listed() {
    let mut g = game4(5);
    tick(&mut g);
    let home = g.players[0].home.unwrap();
    let ship = g.ships.iter().find(|s| s.owner == 0 && s.parked_at() == Some(home)).map(|s| s.id);
    let ship = ship.unwrap_or_else(|| put_ship(&mut g, 0, ShipClass::Scout, home));
    let target = g.system.bodies.iter().find(|b| b.kind == BodyKind::Planet && b.id != home && b.parent == g.system.body(home).parent).unwrap().id;
    g.apply(0, Command::Transfer { ships: vec![ship], to: target, depart_t: None }).unwrap();
    tick(&mut g);
    let view = g.view_for(0);
    let r = report(&view, 0, view.t, view.t);
    let tr = r.items.iter().find(|i| i.section == Sec::Transfers).expect("our transfer is listed");
    assert!(tr.text.contains(&g.system.body(target).name), "{}", tr.text);
    assert_eq!(tr.when, When::Eta);
    assert!(tr.t > view.t);
    // bodies: ours aren't listed, every other one is either described or in "never scanned"
    let bodies: Vec<_> = r.items.iter().filter(|i| i.section == Sec::Bodies).collect();
    assert!(bodies.iter().all(|i| i.focus != Some(Focus::Body(home.0))));
    assert!(bodies.iter().any(|i| i.title == "Never scanned"));
}

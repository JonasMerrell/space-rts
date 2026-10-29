use sim::game::*;
use sim::station::{Station, StationId, StationKind};
use sim::{BodyKind, SystemSize};
fn main() {
    let mut g = Game::new("T".into(), Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0, teams: 0 }, 5, "alice");
    g.add_player("bob", None).unwrap();
    g.start();
    let ast = g.system.bodies.iter().find(|b| b.kind == BodyKind::Asteroid).unwrap().id;
    for tanker in [false, true] {
        let mut h = g.clone();
        h.stations.push(Station { id: StationId(0), owner: 0, kind: StationKind::FuelDepot, body: ast, hp: 150.0, fuel: 0.0, done_t: 0.0, work_left: 0.0 });
        let a = h.spawn(0, ShipClass::Gunship, ast);
        let b = h.spawn(0, ShipClass::Gunship, ast);
        let mut ids = vec![a, b];
        if tanker { ids.push(h.spawn(0, ShipClass::Tanker, ast)); }
        for &i in &ids { h.ships.iter_mut().find(|s| s.id == i).unwrap().fuel = 10.0; }
        let r = h.apply(0, Command::Fleet(sim::fleet::FleetCommand::Form { ships: ids.clone(), name: "F".into() }));
        println!("form fleet: {r:?}");
        let before: Vec<f64> = ids.iter().map(|i| h.ship(*i).unwrap().fuel).collect();
        h.advance_to(h.t + 3.0 * 3600.0);
        let after: Vec<f64> = ids.iter().map(|i| h.ship(*i).unwrap().fuel).collect();
        println!("tanker={tanker}: fuel {before:?} -> {after:?}; depot stock {:.0}, energy {:.0}", h.stations[0].fuel, h.players[0].energy);
    }
}

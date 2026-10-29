use sim::game::*;
use sim::{BodyKind, SystemSize};
fn main() {
    let mut g = Game::new("T".into(), Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0, teams: 0 }, 5, "alice");
    g.add_player("bob", None).unwrap();
    g.start();
    let ast = g.system.bodies.iter().find(|b| b.kind == BodyKind::Asteroid).unwrap().id;
    for (a, b) in [(ShipClass::Interceptor, ShipClass::Gunship), (ShipClass::Scout, ShipClass::Gunship), (ShipClass::Scout, ShipClass::Tanker), (ShipClass::Constructor, ShipClass::Scout)] {
        let mut h = g.clone();
        h.bodies[ast.0 as usize].owner = Some(1);
        let j = h.spawn(1, b, ast);
        h.advance_to(h.t + 120.0);
        let i = h.spawn(0, a, ast);
        h.advance_to(h.t + 3.0 * 3600.0);
        println!("alice {a:?} vs bob {b:?} (bob owns): hp {:?}/{:?} owner {:?}", h.ship(i).map(|s| s.hp as i32), h.ship(j).map(|s| s.hp as i32), h.bodies[ast.0 as usize].owner);
    }
}

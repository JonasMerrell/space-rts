use sim::game::*;
use sim::{BodyKind, SystemSize};
fn main() {
    let mut g = Game::new("T".into(), Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0, teams: 0 }, 5, "alice");
    g.add_player("bob", None).unwrap();
    g.start();
    let giant = g.system.bodies.iter().find(|b| b.kind == BodyKind::GasGiant && g.bodies[b.id.0 as usize].owner.is_none()).unwrap().id;
    for (a, b) in [(ShipClass::Interceptor, ShipClass::Scout), (ShipClass::Interceptor, ShipClass::Constructor), (ShipClass::Interceptor, ShipClass::Interceptor), (ShipClass::Gunship, ShipClass::Scout)] {
        let mut h = g.clone();
        let i = h.spawn(0, a, giant);
        let j = h.spawn(1, b, giant);
        h.advance_to(h.t + 3.0 * 3600.0);
        println!("{a:?} vs {b:?}: power {:?} -> hp {:?} / {:?}, owner {:?}", g.power_at(giant), h.ship(i).map(|s| s.hp as i32), h.ship(j).map(|s| s.hp as i32), h.bodies[giant.0 as usize].owner);
    }
}

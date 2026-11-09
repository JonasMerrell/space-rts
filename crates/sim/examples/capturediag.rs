//! Diagnose a body that won't change hands: `cargo run -p sim --example capturediag -- <game.json> <body name>`
use sim::game::Game;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let text = std::fs::read_to_string(&a[1]).unwrap().replace(":null", ":1e300");
    let mut g: Game = serde_json::from_str(&text).unwrap();
    let name = a[2].to_lowercase();
    let body = g.system.bodies.iter().find(|b| b.name.to_lowercase() == name).unwrap().id;
    let bi = body.0 as usize;
    let report = |g: &Game, tag: &str| {
        let b = &g.bodies[bi];
        let powers = g.power_at(body);
        println!("[{tag}] t {:.0} last_tick {:.0} status {:?} owner {:?} def_hp {} shattered {} gone {} in_combat {}", g.t, g.last_tick, g.status, b.owner, b.def_hp, b.shattered, b.gone, b.in_combat);
        println!("  power_at {:?} sides {}", powers, g.sides_at(&powers.iter().copied().filter(|p| p.1 > 0.0).collect::<Vec<_>>()));
        let here: Vec<_> = g.ships.iter().filter(|s| s.parked_at() == Some(body)).collect();
        println!("  parked {} (dark {}), nan hp {}", here.len(), here.iter().filter(|s| s.dark(g.t)).count(), here.iter().filter(|s| !s.hp.is_finite()).count());
        println!("  battles {}", g.battles.len());
    };
    report(&g, "loaded");
    let t = g.t;
    for (i, dt) in [60.0, 600.0, 3600.0, 6.0 * 3600.0].iter().enumerate() {
        g.advance_to(t + dt);
        report(&g, &format!("+{dt}s #{i}"));
    }
}

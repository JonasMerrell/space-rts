//! Survey asteroid impact plans: for each seed, every asteroid aimed at each player home.
use sim::asteroid::{fuel_available, mass, plan_impact, strength};
use sim::game::{Game, Settings};
use sim::system::SystemSize;
use sim::BodyKind;

fn main() {
    let mut found = 0;
    let mut tried = 0;
    for seed in 1..=6u64 {
        let settings = Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 30.0 * 86_400.0, speed: 1.0, teams: 0 };
        let mut g = Game::new("T".into(), settings, seed, "a");
        g.add_player("b", None).unwrap();
        g.start();
        let home = g.players[1].home.unwrap();
        for a in g.system.bodies.iter().filter(|b| b.kind == BodyKind::Asteroid).map(|b| b.id).collect::<Vec<_>>() {
            tried += 1;
            let t0 = std::time::Instant::now();
            let p = plan_impact(&g, a, home, 0.0);
            let ms = t0.elapsed().as_millis();
            match p {
                Some(p) => {
                    found += 1;
                    let i = p.impact.unwrap();
                    println!("seed {seed} {:>6} m {:>7.0}t dv {:.2} fuel {:>6.0}t burn {:>5.1}h hit {} in {:.1}d at {:.1} km/s E {:.2e} sev {:.2} (str {:.1e}) [{ms} ms] avail {:.0}",
                        g.system.body(a).name, mass(&g.system, a), p.dv, p.fuel, p.burn_s / 3600.0, g.system.body(i.body).name, (i.t) / 86400.0, i.v, i.energy, i.severity, strength(&g.system, home), fuel_available(&g, 0, a));
                }
                None => println!("seed {seed} {:>6} none [{ms} ms]", g.system.body(a).name),
            }
        }
    }
    println!("{found}/{tried}");
}

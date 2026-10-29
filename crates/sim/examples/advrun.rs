//! Balance check for the advanced tech tier: the same all-bot week with and without it.
//! `cargo run --release -p sim --example advrun [seed] [days]`
use sim::game::*;
use sim::station::StationKind;
use sim::SystemSize;

fn main() {
    let mut args = std::env::args().skip(1);
    let seed: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(9);
    let days: f64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(7.0);
    for rules in [1, 2] {
        let mut g = Game::new("BOTS".into(), Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: days * 86_400.0 + 3600.0, speed: 1.0, teams: 0 }, seed, "host");
        g.rules = rules;
        for (n, d) in [("Hard A", "hard"), ("Hard B", "hard"), ("Normal C", "normal")] {
            g.add_player(n, Some(d.into())).unwrap();
        }
        // the host idles: an all-bot war
        g.start();
        let t0 = std::time::Instant::now();
        for day in 1..=days as u32 {
            g.advance_to(day as f64 * 86_400.0);
            println!("rules {rules} day {day} ({:.0?}):", t0.elapsed());
            for p in 1..g.players.len() as PlayerId {
                let pl = &g.players[p as usize];
                let adv: Vec<String> = pl.techs.iter().filter(|t| t.advanced()).map(|t| format!("{t:?}")).collect();
                let st: Vec<String> = g.stations.iter().filter(|s| s.owner == p && s.active(g.t)).map(|s| s.kind).filter(|k| (*k as usize) >= StationKind::Gravimetric as usize).map(|k| format!("{k:?}")).collect();
                println!(
                    "  {:<9} bodies {:>2} score {:>4} ships {:>3} techs {:>2} adv {:?} stations {:?} mines {}",
                    pl.name,
                    g.bodies.iter().filter(|b| b.owner == Some(p)).count(),
                    g.score(p),
                    g.ships.iter().filter(|s| s.owner == p).count(),
                    pl.techs.len(),
                    adv,
                    st,
                    g.minefields.iter().filter(|m| m.owner == p).map(|m| m.charges).sum::<u32>(),
                );
            }
        }
    }
}

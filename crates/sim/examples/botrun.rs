use sim::game::*;
use sim::SystemSize;
fn main() {
    let mut g = Game::new("BOTS".into(), Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0, teams: 0 }, 9, "alice");
    g.add_player("Bot 1", Some("hard".into())).unwrap();
    g.add_player("Bot 2", Some("normal".into())).unwrap();
    g.start();
    for h in 1..=48 {
        let t0 = std::time::Instant::now();
        g.advance_to(h as f64 * 3600.0);
        let flying = g.ships.iter().filter(|s| matches!(s.state, ShipState::Flying(_))).count();
        println!("h{h}: {:?} ships {} flying {flying} owned {:?}", t0.elapsed(), g.ships.len(), (0..3).map(|p| g.bodies.iter().filter(|b| b.owner == Some(p)).count()).collect::<Vec<_>>());
    }
}

//! Intercept survey on a saved game: every (own ship, visible enemy ship in flight) pair, as each
//! player would plan it on their fogged view.
//!   cargo run --release -p sim --example interceptcheck <game.json>
use sim::game::{Game, ShipState};

fn main() {
    let path = std::env::args().nth(1).expect("game json");
    let mut g: Game = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    // vision is not saved: one tick recomputes it
    let t = g.t + 60.0;
    g.advance_to(t);
    println!("t = {:.0}", g.t);
    for p in 0..g.players.len() as u32 {
        let v = g.view_for(p);
        for s in v.ships.iter().filter(|s| s.owner == p) {
            for t in v.ships.iter().filter(|t| t.owner != p && matches!(t.state, ShipState::Flying(_))) {
                for matched in [true, false] {
                    let t0 = std::time::Instant::now();
                    let o = v.intercept_options(s.id, t.id, &v.view_path(t.id).unwrap(), v.t, matched);
                    println!("p{p} {} ({:.1} km/s left) -> {} matched={matched}: {} options in {:?}", s.name, s.dv_left(), t.name, o.len(), t0.elapsed());
                    for x in &o {
                        println!("    {:.2}+{:.2} km/s meet +{:.1} h, miss {:.0} km, closing {:.2}", x.depart_dv, x.match_dv, (x.arrive_t() - v.t) / 3600.0, x.miss, x.closing);
                    }
                }
            }
        }
    }
}

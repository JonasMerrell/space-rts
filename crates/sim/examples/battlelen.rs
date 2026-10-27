use sim::game::*;
use sim::{BodyKind, SystemSize};
fn main() {
    let mut g = Game::new("T".into(), Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 30.0 * 86_400.0, speed: 1.0 }, 5, "alice");
    g.add_player("bob", None).unwrap();
    g.start();
    let neutral = g.system.bodies.iter().find(|b| b.kind == BodyKind::Planet && g.bodies[b.id.0 as usize].owner.is_none()).unwrap().id;
    use ShipClass::*;
    let cases: Vec<(&str, Vec<ShipClass>, Vec<ShipClass>, bool)> = vec![
        ("Interceptor vs Interceptor", vec![Interceptor], vec![Interceptor], false),
        ("Gunship vs Interceptor", vec![Gunship], vec![Interceptor], false),
        ("Gunship vs Gunship", vec![Gunship], vec![Gunship], false),
        ("Bomber vs Gunship", vec![Bomber], vec![Gunship], false),
        ("3 Gunships vs 3 Gunships", vec![Gunship; 3], vec![Gunship; 3], false),
        ("Dreadnought vs 3 Gunships", vec![Dreadnought], vec![Gunship; 3], false),
        ("2 Bombers vs home guns (lvl1)", vec![Bomber; 2], vec![], true),
        ("Interceptor vs home guns (lvl1)", vec![Interceptor], vec![], true),
    ];
    for (name, a, b, home) in cases {
        let mut h = g.clone();
        h.ships.retain(|s| s.owner != 1 || !home);
        let at = if home { h.players[1].home.unwrap() } else { neutral };
        if home { h.ships.retain(|s| s.parked_at() != Some(at)); }
        let ia: Vec<ShipId> = a.iter().map(|&c| h.spawn(0, c, at)).collect();
        let ib: Vec<ShipId> = b.iter().map(|&c| h.spawn(1, c, at)).collect();
        let t0 = h.t;
        let mut end = None;
        for m in 1..=(48 * 60) {
            h.advance_to(t0 + m as f64 * 60.0);
            let alive = |ids: &Vec<ShipId>, h: &Game| ids.iter().filter(|i| h.ship(**i).is_some()).count();
            let (na, nb) = (alive(&ia, &h), alive(&ib, &h));
            let guns = home && h.bodies[at.0 as usize].def_hp > 0.0;
            if na == 0 || (nb == 0 && !guns && !(home && h.bodies[at.0 as usize].owner == Some(1))) || (home && h.bodies[at.0 as usize].owner == Some(0)) {
                end = Some((m as f64 / 60.0, na, nb));
                break;
            }
        }
        match end {
            Some((hrs, na, nb)) => println!("{name}: {hrs:.1} h (left: {na} vs {nb})"),
            None => println!("{name}: >48 h"),
        }
    }
}

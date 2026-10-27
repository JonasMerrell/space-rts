use sim::*;
fn main() {
    let s = system::generate(7, SystemSize::Medium);
    for b in &s.bodies {
        if matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant) {
            let moons = s.children(b.id).count();
            println!("{:<10} {:?} a={:.0} R={:.0} soi={:.0} mu={:.0} T={:.1}h moons={}", b.name, b.kind, b.orbit.unwrap().a, b.radius, b.soi, b.mu, b.orbit.unwrap().period() / 3600.0, moons);
        }
    }
    println!("asteroids {}", s.bodies.iter().filter(|b| b.kind == BodyKind::Asteroid).count());
}

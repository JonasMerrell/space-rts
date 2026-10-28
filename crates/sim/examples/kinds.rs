use sim::*;
fn main() {
    for size in [SystemSize::Small, SystemSize::Medium, SystemSize::Large] {
        for seed in [1u64, 2, 3] {
            let s = system::generate(seed, size);
            let k: String = s.bodies.iter().filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant)).map(|b| if b.kind == BodyKind::GasGiant { 'G' } else { 'r' }).collect();
            println!("{size:?} seed {seed}: {k}");
        }
    }
}

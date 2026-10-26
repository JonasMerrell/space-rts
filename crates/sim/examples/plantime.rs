use sim::*;
fn main() {
    let sys = system::generate(9, SystemSize::Medium);
    let p: Vec<_> = sys.bodies.iter().filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant)).map(|b| b.id).collect();
    for (a, b) in [(p[1], p[2]), (p[2], p[3]), (p[3], p[1]), (p[4], p[5])] {
        let t0 = std::time::Instant::now();
        let n = std::cell::Cell::new(0);
        let pl = plan::plan_leg(&sys, a, b, 0.0, None).unwrap();
        println!("{:?} conv {} {:?}", t0.elapsed(), pl.converged, n);
        let t0 = std::time::Instant::now();
        let tr = traj::predict(&sys, pl.depart, &[], Some(b), pl.arrive_t + 3600.0);
        println!("  one predict {:?} segs {}", t0.elapsed(), tr.segs.len());
    }
}

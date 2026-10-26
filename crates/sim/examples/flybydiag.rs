//! Per-pair breakdown of the assist screen and flight (cost and timing), for tuning.
use sim::*;
use std::time::Instant;
fn main() {
    let seed: u64 = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(1);
    let sys = system::generate(seed, SystemSize::Medium);
    let p: Vec<_> = sys.bodies.iter().filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant)).map(|b| b.id).collect();
    for &a in &p {
        for &b in &p {
            if a == b {
                continue;
            }
            let now = 3600.0 * seed as f64;
            let t0 = Instant::now();
            let d = plan::plan_leg(&sys, a, b, now, None).unwrap();
            let td = t0.elapsed();
            let span = plan::search_span(&sys, a, b);
            let dest = plan::dv_curve(&sys, a, b, now, span, 96).into_iter().map(|x| x.1).fold(f64::INFINITY, f64::min);
            let t0 = Instant::now();
            let c = assist::screen(&sys, a, b, now, plan::search_span(&sys, a, b), None);
            let ts = t0.elapsed();
            let best = c.first();
            let fl = best.map(|c| {
                let t = Instant::now();
                let r = assist::fly(&sys, a, b, c, f64::INFINITY);
                (r.map(|p| (p.total_dv() * 100.0).round() / 100.0), t.elapsed())
            });
            println!(
                "{}->{} direct {:.2} (est {dest:.2}) conv {} {td:.2?} | screen {ts:.2?} best {:?} | fly {:?}",
                a.0,
                b.0,
                d.total_dv(),
                d.converged,
                best.map(|c| (sys.body(c.via).name.clone(), (c.total() * 100.0).round() / 100.0, (c.fix_dv * 100.0).round() / 100.0)),
                fl
            );
        }
    }
}

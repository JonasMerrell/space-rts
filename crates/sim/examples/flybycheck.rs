//! Gravity-assist survey: every planet pair, direct leg vs `plan_route` (Δv, time, planning cost).
use sim::*;
use std::time::Instant;
fn main() {
    let seeds: Vec<u64> = std::env::args().nth(1).map(|s| s.split(',').map(|x| x.parse().unwrap()).collect()).unwrap_or((1..=6).collect());
    let (mut td, mut tr, mut n, mut used, mut saved) = (0.0, 0.0, 0, 0, 0.0);
    for seed in seeds {
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
                let t1 = Instant::now();
                let r = plan::plan_route(&sys, a, b, now, None).unwrap();
                let t2 = Instant::now();
                td += (t1 - t0).as_secs_f64();
                tr += (t2 - t1).as_secs_f64();
                n += 1;
                if !r.flybys.is_empty() {
                    used += 1;
                    saved += 1.0 - r.total_dv() / d.total_dv();
                    let fb = r.traj.flybys(&sys);
                    println!(
                        "seed {seed} {} -> {}: direct {:.2} km/s {:.1} h | via {} {:.2} km/s (dep {:.2} fix {:.2} cap {:.2}) {:.1} h, rp {:.0} gain {:+.2} | plan {:.2}s vs {:.2}s",
                        sys.body(a).name, sys.body(b).name, d.total_dv(), (d.arrive_t - d.depart_t()) / 3600.0,
                        sys.body(r.flybys[0]).name, r.total_dv(), r.depart_dv, r.mid_dv(), r.capture_dv, (r.arrive_t - r.depart_t()) / 3600.0,
                        fb.first().map(|f| f.rp).unwrap_or(0.0), fb.first().map(|f| f.gain).unwrap_or(0.0),
                        (t2 - t1).as_secs_f64(), (t1 - t0).as_secs_f64()
                    );
                }
            }
        }
    }
    println!("pairs {n}, assist used {used} (mean saving {:.0}%), planning time route/direct = {:.2}", 100.0 * saved / used.max(1) as f64, tr / td);
}

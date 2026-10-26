//! Planner quality survey: adjacent planet pairs vs a Hohmann estimate.
use sim::kepler::{circular_speed, oberth_dv};
use sim::system::{generate, SystemSize, STAR_MU};
use sim::*;
fn main() {
    let mut worst: f64 = 0.0;
    let mut nc = 0;
    let mut n = 0;
    for seed in 1u64..=10 {
        let sys = generate(seed, SystemSize::Medium);
        let p: Vec<_> = sys.bodies.iter().filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant)).map(|b| b.id).collect();
        for w in p.windows(2) {
            for (a, b) in [(w[0], w[1]), (w[1], w[0])] {
                let (ba, bb) = (sys.body(a), sys.body(b));
                let (r1, r2) = (ba.orbit.unwrap().a, bb.orbit.unwrap().a);
                let at = (r1 + r2) / 2.0;
                let v1 = (STAR_MU * (2.0 / r1 - 1.0 / at)).sqrt() - circular_speed(STAR_MU, r1);
                let v2 = circular_speed(STAR_MU, r2) - (STAR_MU * (2.0 / r2 - 1.0 / at)).sqrt();
                let h = oberth_dv(ba.mu, ba.park_radius(), v1.abs()) + oberth_dv(bb.mu, bb.park_radius(), v2.abs());
                let now = 3600.0 * seed as f64;
                let plan = plan::plan_leg(&sys, a, b, now, None).unwrap();
                n += 1;
                if !plan.converged { nc += 1; println!("seed {seed} {}->{} NOT converged {:?}", ba.name, bb.name, plan.traj.end); continue; }
                let r = plan.total_dv() / h;
                worst = worst.max(r);
                if r > 1.5 { println!("seed {seed} {}->{}: {:.2} (dep {:.2} cap {:.2}) vs hohmann {h:.2} x{r:.2}", ba.name, bb.name, plan.total_dv(), plan.depart_dv, plan.capture_dv); }
            }
        }
    }
    println!("pairs {n}, not converged {nc}, worst ratio {worst:.2}");
}

use sim::*;
fn main() {
    let sys = system::generate(7, SystemSize::Medium);
    let moon = sys.bodies.iter().find(|b| b.kind == BodyKind::Moon).unwrap();
    let pl = sys.body(moon.parent.unwrap());
    println!("planet R {:.0} mu {:.0} soi {:.0} park {:.0}; moon a {:.0} R {:.0} soi {:.0} park {:.0} T {:.0}s", pl.radius, pl.mu, pl.soi, pl.park_radius(), moon.orbit.unwrap().a, moon.radius, moon.soi, moon.park_radius(), moon.orbit.unwrap().period());
    for _ in 0..1 {
    let plan = plan::plan_leg(&sys, pl.id, moon.id, 0.0, None).unwrap();
    println!("dep {:.2} cap {:.2} conv {} end {:?} depart {:?}", plan.depart_dv, plan.capture_dv, plan.converged, plan.traj.end, plan.depart);
    for s in &plan.traj.segs { 
        let mu = sys.body(s.parent).mu;
        let c = kepler::Conic::from_state(mu, s.r0, s.v0);
        println!("seg parent {} t {:.0}..{:.0} r0 {:.0} v0 {:.2} e {:.3} rp {:.0} h {:.0}", sys.body(s.parent).name, s.t0, s.t1, s.r0.len(), s.v0.len(), c.e, c.rp, c.h); }
    }
}

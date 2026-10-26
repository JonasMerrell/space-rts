//! Gravity-assist routes: "fly past F on the way from A to B" when the slingshot saves Δv.
//!
//! Screening is patched-conic Lambert (A→F, F→B in the common parent's frame) with a cost for the
//! v∞ mismatch / turn F can't provide. The winner is then flown for real against `predict`: a
//! Nelder–Mead on the departure aims the pass at the wanted periapsis on the wanted side of F,
//! and a correction burn shortly after leaving F's SOI (a normal maneuver node on the flight)
//! fixes whatever the flyby left over and targets the capture at B.
use crate::kepler::{circular_speed, ejection, lambert, oberth_dv, Conic};
use crate::plan::{dips_below, park_dv, PlanHint, TransferPlan};
use crate::system::{BodyId, BodyKind, System};
use crate::traj::{predict, Burn, End, State, Trajectory};
use crate::v2::V2;

/// A screened single-flyby route (Lambert estimate).
#[derive(Clone, Copy, Debug)]
pub struct Candidate {
    pub via: BodyId,
    pub depart_t: f64,
    pub flyby_t: f64,
    pub arrive_t: f64,
    pub dep_dv: f64,
    /// correction needed after the flyby (v∞ mismatch + turn F can't give)
    pub fix_dv: f64,
    pub cap_dv: f64,
    /// v∞ relative to F before / after the pass (parent frame)
    pub vin: V2,
    pub vout: V2,
}

impl Candidate {
    pub fn total(&self) -> f64 {
        self.dep_dv + self.fix_dv + self.cap_dv
    }
}

/// Bodies worth swinging past between siblings `from` and `to`: the other planets around the same
/// parent (or the other moons of the same planet). Asteroids are too light to bend anything.
pub fn assist_bodies(sys: &System, from: BodyId, to: BodyId) -> Vec<BodyId> {
    let parent = sys.body(from).parent;
    if parent.is_none() || parent != sys.body(to).parent {
        return vec![];
    }
    sys.bodies
        .iter()
        .filter(|b| b.parent == parent && b.id != from && b.id != to)
        .filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant | BodyKind::Moon))
        .map(|b| b.id)
        .collect()
}

/// Lowest periapsis we'll aim a flyby at (clear of the surface with margin).
fn min_rp(sys: &System, f: BodyId) -> f64 {
    sys.body(f).radius * 1.25
}

/// Estimated correction Δv after an unpowered pass of `f` that needs to turn `vin` into `vout`,
/// and the periapsis (signed: + = counter-clockwise pass) that best does it.
pub fn flyby_cost(sys: &System, f: BodyId, vin: V2, vout: V2) -> (f64, f64) {
    let b = sys.body(f);
    let (a, c) = (vin.len(), vout.len());
    let v = a.max(1e-3);
    let delta = vin.cross(vout).atan2(vin.dot(vout));
    let turn = delta.abs();
    let turn_at = |rp: f64| 2.0 * (1.0 / (1.0 + rp * v * v / b.mu)).asin();
    let (rmin, rmax) = (min_rp(sys, f), b.soi * 0.5);
    let rp = if turn >= turn_at(rmin) {
        rmin
    } else {
        (b.mu / (v * v) * (1.0 / (turn / 2.0).sin().max(1e-9) - 1.0)).clamp(rmin, rmax)
    };
    let miss = (turn - turn_at(rp)).abs();
    // the outgoing speed stays |vin|; the burn after the pass makes up length and leftover angle
    let fix = (c - a).abs() + 2.0 * a.min(c) * (miss / 2.0).sin();
    (fix, rp * if delta >= 0.0 { 1.0 } else { -1.0 })
}

struct LegEst {
    dep: f64,
    v_end: V2,
}

/// Screen every assist body for the cheapest A → F → B within the departure span (or at a fixed
/// departure time). Returns the best candidate per assist body, cheapest first.
pub fn screen(sys: &System, from: BodyId, to: BodyId, now: f64, span: f64, depart_t: Option<f64>) -> Vec<Candidate> {
    let Some(frame) = sys.body(from).parent else { return vec![] };
    if sys.body(to).parent != Some(frame) {
        return vec![];
    }
    let fr = sys.body(frame);
    let mu = fr.mu;
    let (bf, bt) = (sys.body(from), sys.body(to));
    let rel_h = |a: BodyId, b: BodyId| {
        let (ra, rb) = (sys.body(a).orbit.unwrap().a, sys.body(b).orbit.unwrap().a);
        let s = 0.5 * (ra + rb);
        std::f64::consts::PI * (s * s * s / mu).sqrt()
    };
    let tof_grid = |h: f64| (0..10).map(move |i| h * (0.4 + 0.13 * i as f64));
    let (t_lo, t_hi) = match depart_t {
        Some(td) => (td, td),
        None => (now + 60.0, now + 60.0 + span),
    };
    let mut out = vec![];
    for via in assist_bodies(sys, from, to) {
        let (h1, h2) = (rel_h(from, via), rel_h(via, to));
        let n = if depart_t.is_some() { 1 } else { 56 };
        let mut best: Option<Candidate> = None;
        // flyby times: either swept with the departure span, or fixed departure + each tof
        let flyby_times: Vec<f64> = match depart_t {
            Some(td) => tof_grid(h1).map(|t| td + t).collect(),
            None => (0..n).map(|i| t_lo + 0.4 * h1 + (span + 0.9 * h1) * i as f64 / (n - 1) as f64).collect(),
        };
        for t1 in flyby_times {
            let rf = sys.pos_in(via, frame, t1);
            let vf = sys.vel_in(via, frame, t1);
            // legs into F arriving at t1
            let ins: Vec<(f64, LegEst)> = tof_grid(h1)
                .filter_map(|tof| {
                    let td = t1 - tof;
                    if td < t_lo - 1.0 || td > t_hi + 1.0 {
                        return None;
                    }
                    let r1 = sys.pos_in(from, frame, td);
                    let (v1, v2) = lambert(mu, r1, rf, tof, true)?;
                    if dips_below(mu, r1, v1, tof, fr.radius * 1.2) {
                        return None;
                    }
                    let vinf = v1 - sys.vel_in(from, frame, td);
                    Some((td, LegEst { dep: oberth_dv(bf.mu, bf.park_radius(), vinf.len()), v_end: v2 - vf }))
                })
                .collect();
            if ins.is_empty() {
                continue;
            }
            // legs out of F leaving at t1
            let outs: Vec<(f64, f64, V2)> = tof_grid(h2)
                .filter_map(|tof| {
                    let ta = t1 + tof;
                    let r2 = sys.pos_in(to, frame, ta);
                    let (v1, v2) = lambert(mu, rf, r2, tof, true)?;
                    if dips_below(mu, rf, v1, tof, fr.radius * 1.2) {
                        return None;
                    }
                    let vinf = v2 - sys.vel_in(to, frame, ta);
                    Some((ta, oberth_dv(bt.mu, bt.park_radius(), vinf.len()), v1 - vf))
                })
                .collect();
            for (td, li) in &ins {
                for &(ta, cap, vout) in &outs {
                    let (fix, _) = flyby_cost(sys, via, li.v_end, vout);
                    let c = Candidate { via, depart_t: *td, flyby_t: t1, arrive_t: ta, dep_dv: li.dep, fix_dv: fix, cap_dv: cap, vin: li.v_end, vout };
                    if best.is_none_or(|b| c.total() < b.total()) {
                        best = Some(c);
                    }
                }
            }
        }
        if let Some(b) = best {
            out.push(refine(sys, from, to, b, depart_t.is_some(), now).unwrap_or(b));
        }
    }
    out.sort_by(|a, b| a.total().total_cmp(&b.total()));
    out
}

/// Evaluate one (depart, flyby, arrive) triple on the Lambert estimate.
pub(crate) fn estimate(sys: &System, from: BodyId, to: BodyId, via: BodyId, td: f64, t1: f64, ta: f64) -> Option<Candidate> {
    let frame = sys.body(from).parent?;
    let fr = sys.body(frame);
    let (bf, bt) = (sys.body(from), sys.body(to));
    if t1 <= td + 60.0 || ta <= t1 + 60.0 {
        return None;
    }
    let r1 = sys.pos_in(from, frame, td);
    let rf = sys.pos_in(via, frame, t1);
    let r2 = sys.pos_in(to, frame, ta);
    let vf = sys.vel_in(via, frame, t1);
    let (a1, a2) = lambert(fr.mu, r1, rf, t1 - td, true)?;
    let (b1, b2) = lambert(fr.mu, rf, r2, ta - t1, true)?;
    if dips_below(fr.mu, r1, a1, t1 - td, fr.radius * 1.2) || dips_below(fr.mu, rf, b1, ta - t1, fr.radius * 1.2) {
        return None;
    }
    let dep = oberth_dv(bf.mu, bf.park_radius(), (a1 - sys.vel_in(from, frame, td)).len());
    let cap = oberth_dv(bt.mu, bt.park_radius(), (b2 - sys.vel_in(to, frame, ta)).len());
    let (vin, vout) = (a2 - vf, b1 - vf);
    let (fix, _) = flyby_cost(sys, via, vin, vout);
    Some(Candidate { via, depart_t: td, flyby_t: t1, arrive_t: ta, dep_dv: dep, fix_dv: fix, cap_dv: cap, vin, vout })
}

/// Pattern search on (depart, flyby, arrive) times around a screened candidate.
fn refine(sys: &System, from: BodyId, to: BodyId, c: Candidate, fixed_depart: bool, now: f64) -> Option<Candidate> {
    let mut cur = estimate(sys, from, to, c.via, c.depart_t, c.flyby_t, c.arrive_t)?;
    let mut step = 0.04 * (c.arrive_t - c.depart_t);
    for _ in 0..60 {
        let mut improved = false;
        for (d0, d1, d2) in [(1.0, 0.0, 0.0), (-1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, -1.0, 0.0), (0.0, 0.0, 1.0), (0.0, 0.0, -1.0), (1.0, 1.0, 1.0), (-1.0, -1.0, -1.0)] {
            if fixed_depart && d0 != 0.0 {
                continue;
            }
            let td = cur.depart_t + d0 * step;
            if td < now + 30.0 {
                continue;
            }
            if let Some(n) = estimate(sys, from, to, c.via, td, cur.flyby_t + d1 * step, cur.arrive_t + d2 * step) {
                if n.total() < cur.total() {
                    cur = n;
                    improved = true;
                    break;
                }
            }
        }
        if !improved {
            step *= 0.5;
            if step < 5.0 {
                break;
            }
        }
    }
    Some(cur)
}

/// Downhill simplex over `p0` with initial step sizes `d`. Deterministic.
fn nelder_mead<const N: usize>(p0: [f64; N], d: [f64; N], iters: usize, mut f: impl FnMut(&[f64; N]) -> f64) -> ([f64; N], f64) {
    let mut pts: Vec<([f64; N], f64)> = vec![(p0, f(&p0))];
    for k in 0..N {
        let mut p = p0;
        p[k] += d[k];
        pts.push((p, f(&p)));
    }
    let lerp = |a: &[f64; N], b: &[f64; N], t: f64| {
        let mut o = *a;
        for k in 0..N {
            o[k] = a[k] + (b[k] - a[k]) * t;
        }
        o
    };
    for _ in 0..iters {
        pts.sort_by(|a, b| a.1.total_cmp(&b.1));
        if (pts[N].1 - pts[0].1).abs() < 1e-7 {
            break;
        }
        let mut c = [0.0; N];
        for (p, _) in &pts[..N] {
            for k in 0..N {
                c[k] += p[k] / N as f64;
            }
        }
        let worst = pts[N];
        let refl = lerp(&c, &worst.0, -1.0);
        let fr = f(&refl);
        if fr < pts[0].1 {
            let exp = lerp(&c, &worst.0, -2.0);
            let fe = f(&exp);
            pts[N] = if fe < fr { (exp, fe) } else { (refl, fr) };
        } else if fr < pts[N - 1].1 {
            pts[N] = (refl, fr);
        } else {
            let con = lerp(&c, &worst.0, 0.5);
            let fc = f(&con);
            if fc < worst.1 {
                pts[N] = (con, fc);
            } else {
                let best = pts[0].0;
                for i in 1..=N {
                    let p = lerp(&best, &pts[i].0, 0.5);
                    pts[i] = (p, f(&p));
                }
            }
        }
    }
    pts.sort_by(|a, b| a.1.total_cmp(&b.1));
    pts[0]
}

/// Signed periapsis of the first pass through `via`'s SOI (None if the path never enters it).
fn pass_rp(sys: &System, tr: &Trajectory, via: BodyId) -> Option<f64> {
    let s = tr.segs.iter().find(|s| s.parent == via)?;
    let c = Conic::from_state(sys.body(via).mu, s.r0, s.v0);
    Some(c.rp * c.h.signum())
}

/// Fly a screened candidate for real. Returns a plan whose departure leads through `via` and
/// whose single correction burn after the pass ends in a capture at `to`. Gives up early once the
/// estimate can't beat `bar` (km/s).
pub fn fly(sys: &System, from: BodyId, to: BodyId, c: &Candidate, bar: f64) -> Option<TransferPlan> {
    let frame = sys.body(from).parent?;
    let (bf, bv, bt) = (sys.body(from), sys.body(c.via), sys.body(to));
    let (_, rp_t) = flyby_cost(sys, c.via, c.vin, c.vout);
    // stage 1: aim the departure so the pass goes in on the wanted side at the wanted height
    let vinf_d = {
        let r1 = sys.pos_in(from, frame, c.depart_t);
        let rf = sys.pos_in(c.via, frame, c.flyby_t);
        let (a1, _) = lambert(sys.body(frame).mu, r1, rf, c.flyby_t - c.depart_t, true)?;
        a1 - sys.vel_in(from, frame, c.depart_t)
    };
    let (r0, v0) = ejection(bf.mu, bf.park_radius(), vinf_d);
    let start = |p: &[f64; 3]| -> State {
        let r = r0.rotate(p[0]);
        State { parent: from, t: c.depart_t, r, v: v0.rotate(p[0]) * (1.0 + p[1]) + r.norm() * p[2] }
    };
    let h1 = c.flyby_t + 0.5 * (c.flyby_t - c.depart_t) + 600.0;
    let est = c.total().max(0.5);
    let aim = |p: &[f64; 3]| -> f64 {
        let st = start(p);
        let tr = predict(sys, st, &[], None, h1);
        let dv = park_dv(sys, from, &st);
        match pass_rp(sys, &tr, c.via) {
            Some(rp) => ((rp - rp_t) / bv.soi).abs() + 0.002 * dv / est,
            None => 1.0 + tr.closest_approach(sys, c.via).0 / bv.soi,
        }
    };
    let (p, err) = nelder_mead([0.0; 3], [0.03, 0.01, 0.02 * v0.len()], 40, aim);
    if err > 0.1 {
        return None;
    }
    // stage 2: from that pass, let the departure slide to whatever pass gives the cheapest total
    // (departure + Lambert estimate of the correction after the SOI + capture)
    let fmu = sys.body(frame).mu;
    let correction = |tr: &Trajectory| -> Option<(f64, f64, State, V2)> {
        let fb = tr.flybys(sys).into_iter().find(|f| f.body == c.via)?;
        let tc = fb.t_out + 120.0;
        let s = tr.state_at(sys, tc).filter(|s| s.parent == frame)?;
        let mut best: Option<(f64, f64, State, V2)> = None;
        for k in 0..12 {
            let tof = (c.arrive_t - tc).max(3600.0) * (0.7 + 0.06 * k as f64);
            let r2 = sys.pos_in(to, frame, tc + tof);
            let Some((v1, v2)) = lambert(fmu, s.r, r2, tof, true) else { continue };
            if dips_below(fmu, s.r, v1, tof, sys.body(frame).radius * 1.2) {
                continue;
            }
            let cost = (v1 - s.v).len() + oberth_dv(bt.mu, bt.park_radius(), (v2 - sys.vel_in(to, frame, tc + tof)).len());
            if best.as_ref().is_none_or(|b| cost < b.0) {
                best = Some((cost, tc, s, v1 - s.v));
            }
        }
        best
    };
    let total = |p: &[f64; 3]| -> f64 {
        let st = start(p);
        let tr = predict(sys, st, &[], None, h1);
        match correction(&tr) {
            Some((cost, ..)) => park_dv(sys, from, &st) + cost,
            None => 1e3 + aim(p),
        }
    };
    let (p, est_total) = nelder_mead(p, [0.01, 0.004, 0.005 * v0.len()], 50, total);
    if est_total > bar {
        return None;
    }
    let dep = start(&p);
    let tr = predict(sys, dep, &[], None, h1);
    let (_, tc, s, g) = correction(&tr)?;
    // stage 3: polish the correction burn against the real path into the capture
    let horizon = c.arrive_t + 0.6 * (c.arrive_t - c.depart_t) + 600.0;
    let burn = |q: &[f64; 2]| Burn::from_vector(tc, s.r, s.v, g + V2 { x: q[0], y: q[1] });
    let target = bt.park_radius();
    let hit = |q: &[f64; 2]| -> f64 {
        let b = burn(q);
        let tr = predict(sys, State { v: s.v + b.vector(s.r, s.v), ..s }, &[], Some(to), horizon);
        match tr.end {
            End::Capture { body, rp, vp, .. } if body == to => {
                ((rp - target) / target).abs() * 0.05 + 0.01 * (b.dv() + vp - circular_speed(bt.mu, rp)) / est
            }
            End::Impact { body, .. } if body == to => {
                let l = tr.segs.last().unwrap();
                0.2 + 0.2 * (1.0 - Conic::from_state(bt.mu, l.r0, l.v0).rp / target).clamp(0.0, 1.0)
            }
            _ => 1.0 + tr.closest_approach(sys, to).0 / bt.soi,
        }
    };
    let step = 0.02 * g.len().max(0.5);
    let (q, err) = nelder_mead([0.0; 2], [step, step], 60, hit);
    let b = burn(&q);
    let traj = predict(sys, dep, &[b], Some(to), horizon);
    let End::Capture { body, t, rp, vp } = traj.end else { return None };
    if body != to || err >= 0.08 || !traj.flybys(sys).iter().any(|f| f.body == c.via) {
        return None;
    }
    Some(TransferPlan {
        from,
        to,
        depart: dep,
        depart_dv: park_dv(sys, from, &dep),
        capture_dv: vp - circular_speed(bt.mu, rp),
        arrive_t: t,
        traj,
        converged: true,
        burns: vec![b],
        flybys: vec![c.via],
        hint: Some(PlanHint::Assist { via: c.via, flyby_t: c.flyby_t, arrive_t: c.arrive_t }),
    })
}

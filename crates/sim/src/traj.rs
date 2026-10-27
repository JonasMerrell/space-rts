//! Patched-conic trajectory prediction: a ship coasts on a two-body conic around its current
//! parent, switching frames at sphere-of-influence boundaries and applying scheduled burns.
use crate::kepler::{propagate, Conic};
use crate::system::{BodyId, System};
use crate::v2::V2;
use serde::{Deserialize, Serialize};

/// Kinematic state relative to `parent`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct State {
    pub parent: BodyId,
    pub t: f64,
    pub r: V2,
    pub v: V2,
}

/// Impulsive burn. `prograde` along the velocity, `radial` perpendicular to it pointing away
/// from the parent body. km/s.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Burn {
    pub t: f64,
    pub prograde: f64,
    pub radial: f64,
}

impl Burn {
    pub fn dv(&self) -> f64 {
        self.prograde.hypot(self.radial)
    }
    /// Convert to a velocity change vector for a ship at (r, v).
    pub fn vector(&self, r: V2, v: V2) -> V2 {
        let p = v.norm();
        let mut q = p.perp();
        if q.dot(r) < 0.0 {
            q = -q;
        }
        p * self.prograde + q * self.radial
    }
    /// Inverse of `vector`.
    pub fn from_vector(t: f64, r: V2, v: V2, dv: V2) -> Burn {
        let p = v.norm();
        let mut q = p.perp();
        if q.dot(r) < 0.0 {
            q = -q;
        }
        Burn { t, prograde: dv.dot(p), radial: dv.dot(q) }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Segment {
    pub parent: BodyId,
    pub t0: f64,
    pub r0: V2,
    pub v0: V2,
    pub t1: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum End {
    /// Reaches periapsis inside the destination's SOI: the ship can brake into a parking orbit.
    Capture { body: BodyId, t: f64, rp: f64, vp: f64 },
    Impact { body: BodyId, t: f64 },
    Horizon { t: f64 },
}

impl End {
    pub fn t(&self) -> f64 {
        match *self {
            End::Capture { t, .. } | End::Impact { t, .. } | End::Horizon { t } => t,
        }
    }
}

/// An unpowered pass through a body's sphere of influence that isn't the end of the trajectory: a
/// gravity assist. Speeds are km/s; `gain` is the change of speed in the frame of the body's parent
/// (positive = slingshot boost, negative = braking flyby).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Flyby {
    pub body: BodyId,
    /// time of closest approach (or of SOI exit if the pass was cut short by a burn)
    pub t: f64,
    pub t_in: f64,
    pub t_out: f64,
    /// closest approach to the body's centre, km
    pub rp: f64,
    /// hyperbolic excess speed relative to the body
    pub vinf: f64,
    /// angle the body turned the velocity by (radians, unsigned)
    pub turn: f64,
    pub gain: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trajectory {
    pub segs: Vec<Segment>,
    pub end: End,
    /// Burns that were actually applied during prediction (in time order).
    pub burns: Vec<Burn>,
}

impl Trajectory {
    pub fn seg_at(&self, t: f64) -> Option<&Segment> {
        self.segs.iter().find(|s| t >= s.t0 && t <= s.t1)
    }
    /// State relative to the segment parent at time t.
    pub fn state_at(&self, sys: &System, t: f64) -> Option<State> {
        let s = self.seg_at(t)?;
        let (r, v) = propagate(sys.body(s.parent).mu, s.r0, s.v0, t - s.t0);
        Some(State { parent: s.parent, t, r, v })
    }
    pub fn abs_pos(&self, sys: &System, t: f64) -> Option<V2> {
        let st = self.state_at(sys, t)?;
        Some(sys.pos(st.parent, t) + st.r)
    }
    /// Sample a segment as points relative to its parent (draw them around the parent's
    /// position at whatever display time you use, like a KSP map view).
    pub fn sample_rel(&self, sys: &System, seg: &Segment, n: usize) -> Vec<V2> {
        let mu = sys.body(seg.parent).mu;
        let n = n.max(2);
        (0..n)
            .map(|i| {
                let t = seg.t0 + (seg.t1 - seg.t0) * i as f64 / (n - 1) as f64;
                propagate(mu, seg.r0, seg.v0, t - seg.t0).0
            })
            .collect()
    }
    /// Gravity assists along the path: every complete pass (entered from the parent's frame and left
    /// again) through a body's SOI. Burns inside the SOI split the pass into several segments; they are
    /// merged here.
    pub fn flybys(&self, sys: &System) -> Vec<Flyby> {
        let mut out = vec![];
        let mut i = 1;
        while i < self.segs.len() {
            let b = self.segs[i].parent;
            let up = sys.body(b).parent;
            if up.is_none() || Some(self.segs[i - 1].parent) != up {
                i += 1;
                continue;
            }
            let mut j = i;
            while j + 1 < self.segs.len() && self.segs[j + 1].parent == b {
                j += 1;
            }
            if j + 1 >= self.segs.len() || Some(self.segs[j + 1].parent) != up {
                i = j + 1;
                continue; // still inside at the end (capture, impact or horizon), or went down into a moon
            }
            let mu = sys.body(b).mu;
            let (s0, s1) = (&self.segs[i], &self.segs[j]);
            let (_, v_out) = propagate(mu, s1.r0, s1.v0, s1.t1 - s1.t0);
            let mut best = (s0.r0.len(), s0.t0);
            for s in &self.segs[i..=j] {
                let c = Conic::from_state(mu, s.r0, s.v0);
                if let Some(tp) = c.time_to_periapsis(s.r0, s.v0).filter(|&tp| s.t0 + tp <= s.t1) {
                    if c.rp < best.0 {
                        best = (c.rp, s.t0 + tp);
                    }
                }
            }
            let energy = s0.v0.len2() / 2.0 - mu / s0.r0.len();
            let (_, vb_in) = sys.rel_state(b, s0.t0);
            let (_, vb_out) = sys.rel_state(b, s1.t1);
            out.push(Flyby {
                body: b,
                t: best.1,
                t_in: s0.t0,
                t_out: s1.t1,
                rp: best.0,
                vinf: (2.0 * energy).max(0.0).sqrt(),
                turn: s0.v0.cross(v_out).atan2(s0.v0.dot(v_out)).abs(),
                gain: (v_out + vb_out).len() - (s0.v0 + vb_in).len(),
            });
            i = j + 1;
        }
        out
    }
    pub fn total_dv(&self) -> f64 {
        self.burns.iter().map(|b| b.dv()).sum()
    }
    /// Closest approach of the path to `body` (distance, time), sampled.
    pub fn closest_approach(&self, sys: &System, body: BodyId) -> (f64, f64) {
        let mut best = (f64::INFINITY, 0.0);
        for s in &self.segs {
            let n = 400;
            let mu = sys.body(s.parent).mu;
            for i in 0..=n {
                let t = s.t0 + (s.t1 - s.t0) * i as f64 / n as f64;
                let p = sys.pos(s.parent, t) + propagate(mu, s.r0, s.v0, t - s.t0).0;
                let d = p.dist(sys.pos(body, t));
                if d < best.0 {
                    best = (d, t);
                }
            }
        }
        best
    }
}

const MAX_SEGMENTS: usize = 16;
const MAX_STEPS: usize = 60_000;

/// Predict a path from `start` until `horizon` (absolute time), applying `burns` (sorted by t,
/// those before start.t are ignored). Capture is only detected for `dest`.
pub fn predict(sys: &System, start: State, burns: &[Burn], dest: Option<BodyId>, horizon: f64) -> Trajectory {
    let mut segs = Vec::new();
    let mut applied = Vec::new();
    let mut st = start;
    let mut bi = burns.iter().position(|b| b.t >= start.t).unwrap_or(burns.len());
    let mut steps = 0usize;
    loop {
        let next_burn = burns.get(bi).map(|b| b.t).filter(|&t| t < horizon);
        let t_lim = next_burn.unwrap_or(horizon);
        let (ev, t_ev) = scan_segment(sys, &st, dest, t_lim, &mut steps);
        segs.push(Segment { parent: st.parent, t0: st.t, r0: st.r, v0: st.v, t1: t_ev });
        let mu = sys.body(st.parent).mu;
        match ev {
            SegEvent::Capture { rp, vp } => {
                return Trajectory { segs, end: End::Capture { body: st.parent, t: t_ev, rp, vp }, burns: applied };
            }
            SegEvent::Impact => {
                return Trajectory { segs, end: End::Impact { body: st.parent, t: t_ev }, burns: applied };
            }
            SegEvent::Limit => {
                if next_burn.is_some() && t_ev < horizon {
                    let b = burns[bi];
                    bi += 1;
                    let (r, v) = propagate(mu, st.r, st.v, b.t - st.t);
                    let dv = b.vector(r, v);
                    applied.push(b);
                    st = State { parent: st.parent, t: b.t, r, v: v + dv };
                } else {
                    return Trajectory { segs, end: End::Horizon { t: horizon }, burns: applied };
                }
            }
            SegEvent::Exit => {
                let (r, v) = propagate(mu, st.r, st.v, t_ev - st.t);
                let parent = sys.body(st.parent);
                let gp = parent.parent.expect("exit from star");
                let (pr, pv) = sys.rel_state(parent.id, t_ev);
                st = State { parent: gp, t: t_ev, r: r + pr, v: v + pv };
            }
            SegEvent::Enter(child) => {
                let (r, v) = propagate(mu, st.r, st.v, t_ev - st.t);
                let (cr, cv) = sys.rel_state(child, t_ev);
                st = State { parent: child, t: t_ev, r: r - cr, v: v - cv };
            }
        }
        if segs.len() >= MAX_SEGMENTS || steps >= MAX_STEPS {
            let t = st.t;
            return Trajectory { segs, end: End::Horizon { t }, burns: applied };
        }
    }
}

enum SegEvent {
    Capture { rp: f64, vp: f64 },
    Impact,
    Exit,
    Enter(BodyId),
    Limit,
}

fn scan_segment(sys: &System, st: &State, dest: Option<BodyId>, t_lim: f64, steps: &mut usize) -> (SegEvent, f64) {
    let body = sys.body(st.parent);
    let mu = body.mu;
    let conic = Conic::from_state(mu, st.r, st.v);
    let mut t_end = t_lim;
    let mut ev = SegEvent::Limit;
    // analytic periapsis events
    if let Some(tp) = conic.time_to_periapsis(st.r, st.v) {
        let t_p = st.t + tp;
        if conic.rp < body.radius {
            // impact before periapsis: bisection on |r| = radius over [t, t_p]
            if t_p <= t_end || st.r.len() > body.radius {
                let (mut lo, mut hi) = (st.t, t_p);
                if st.r.len() <= body.radius {
                    return (SegEvent::Impact, st.t);
                }
                for _ in 0..60 {
                    let m = 0.5 * (lo + hi);
                    if propagate(mu, st.r, st.v, m - st.t).0.len() > body.radius {
                        lo = m
                    } else {
                        hi = m
                    }
                }
                if hi <= t_end {
                    t_end = hi;
                    ev = SegEvent::Impact;
                }
            }
        } else if Some(st.parent) == dest && t_p <= t_end && tp > 1.0 {
            let (rr, vv) = propagate(mu, st.r, st.v, tp);
            t_end = t_p;
            ev = SegEvent::Capture { rp: rr.len(), vp: vv.len() };
        }
    }
    // marching search for SOI exit / child entry before t_end
    let can_exit = body.parent.is_some() && conic.apoapsis() > body.soi;
    let children: Vec<_> = sys.children(st.parent).collect();
    if !can_exit && children.is_empty() {
        return (ev, t_end);
    }
    let own_period = conic.period().unwrap_or(f64::INFINITY);
    let mut t = st.t;
    let (mut r, mut v) = (st.r, st.v);
    let inside = |r: V2, t: f64| -> Option<SegEvent> {
        if can_exit && r.len() > body.soi {
            return Some(SegEvent::Exit);
        }
        for c in &children {
            let (cr, _) = sys.rel_state(c.id, t);
            if r.dist(cr) < c.soi {
                return Some(SegEvent::Enter(c.id));
            }
        }
        None
    };
    // started inside a child's SOI (e.g. right after exiting it) — ignore that child until we leave
    let mut ignore: Option<BodyId> = None;
    for c in &children {
        if r.dist(sys.rel_state(c.id, t).0) <= c.soi * 1.0001 {
            ignore = Some(c.id);
        }
    }
    while t < t_end {
        *steps += 1;
        if *steps > MAX_STEPS {
            return (SegEvent::Limit, t);
        }
        let mut h = (own_period / 90.0).min(4000.0);
        let vm = v.len().max(1e-6);
        if can_exit {
            h = h.min((body.soi - r.len()).max(0.0) / vm * 0.9);
        }
        for c in &children {
            let (cr, cv) = sys.rel_state(c.id, t);
            let d = r.dist(cr) - c.soi;
            let vrel = (v - cv).len().max(1e-6);
            if Some(c.id) == ignore {
                if d > c.soi * 0.01 {
                    ignore = None;
                }
                continue;
            }
            h = h.min((d.max(0.0) / vrel * 0.8).max(0.0)).min(c.orbit.unwrap().period() / 60.0);
        }
        let h = h.max(2.0).min(t_end - t);
        let tn = t + h;
        let (rn, vn) = propagate(mu, st.r, st.v, tn - st.t);
        let hit = inside(rn, tn).filter(|e| !matches!((e, ignore), (SegEvent::Enter(c), Some(i)) if *c == i));
        if let Some(e) = hit {
            // bisect for the crossing time
            let (mut lo, mut hi) = (t, tn);
            for _ in 0..50 {
                if hi - lo < 0.25 {
                    break;
                }
                let m = 0.5 * (lo + hi);
                let (rm, _) = propagate(mu, st.r, st.v, m - st.t);
                let still = inside(rm, m).filter(|e| !matches!((e, ignore), (SegEvent::Enter(c), Some(i)) if *c == i));
                if still.is_some() {
                    hi = m
                } else {
                    lo = m
                }
            }
            let (rh, _) = propagate(mu, st.r, st.v, hi - st.t);
            let e2 = inside(rh, hi).unwrap_or(e);
            return (e2, hi);
        }
        t = tn;
        r = rn;
        v = vn;
    }
    (ev, t_end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::{generate, BodyKind, Rng, SystemSize};
    use crate::v2::V2;

    /// A pass through `b`'s SOI built in b's frame: periapsis `rp` at time `tp` with excess speed
    /// `vinf`, started just outside the SOI in the parent's frame.
    fn pass(sys: &System, b: BodyId, rp: f64, vinf: f64, ang: f64, ccw: bool, tp: f64) -> State {
        let body = sys.body(b);
        let vp = (vinf * vinf + 2.0 * body.mu / rp).sqrt();
        let dir = V2::from_angle(ang);
        let (r, v) = (dir * rp, dir.perp() * if ccw { vp } else { -vp });
        // back up to just outside the SOI (two-body in b's frame; outside the SOI this is the patched-conic approximation)
        let (mut lo, mut hi) = (0.0, 1.0);
        while propagate(body.mu, r, v, -hi).0.len() < body.soi * 1.002 {
            hi *= 2.0;
        }
        for _ in 0..80 {
            let m = 0.5 * (lo + hi);
            if propagate(body.mu, r, v, -m).0.len() < body.soi * 1.002 {
                lo = m
            } else {
                hi = m
            }
        }
        let (r0, v0) = propagate(body.mu, r, v, -hi);
        let t0 = tp - hi;
        let (br, bv) = sys.rel_state(b, t0);
        State { parent: body.parent.unwrap(), t: t0, r: r0 + br, v: v0 + bv }
    }

    fn abs_state(sys: &System, s: &Segment, t: f64) -> (V2, V2) {
        let (r, v) = propagate(sys.body(s.parent).mu, s.r0, s.v0, t - s.t0);
        (sys.pos(s.parent, t) + r, sys.vel(s.parent, t) + v)
    }

    #[test]
    fn unplanned_flyby_is_a_proper_hyperbola() {
        let sys = generate(7, SystemSize::Medium);
        let mut rng = Rng::new(99);
        // bodies without moons: a moon system makes the pass a different (multi-body) path
        let targets: Vec<BodyId> = sys
            .bodies
            .iter()
            .filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant | BodyKind::Moon) && sys.children(b.id).next().is_none())
            // gas-giant moons orbit deep in the giant's well: the giant's pull over the approach arc
            // shifts v∞ by more than this test's tolerance (patched conics are still continuous there)
            .filter(|b| b.parent.is_none_or(|p| sys.body(p).kind != BodyKind::GasGiant))
            .map(|b| b.id)
            .collect();
        let mut n = 0;
        for k in 0..200 {
            let b = targets[k % targets.len()];
            let body = sys.body(b);
            let rp = rng.range(body.radius * 1.1, (body.radius * 4.0).min(body.soi * 0.7).max(body.radius * 1.2));
            let vesc = (2.0 * body.mu / rp).sqrt();
            let vinf = vesc * rng.range(0.1, 1.5);
            let tp = rng.range(0.0, 3.0 * 86_400.0);
            let st = pass(&sys, b, rp, vinf, rng.range(0.0, 6.3), rng.f() < 0.5, tp);
            let tr = predict(&sys, st, &[], None, tp + 86_400.0);
            if std::env::var("FBDBG").is_ok() {
                let (cr, cv) = sys.rel_state(b, st.t);
                println!("case {k} {} soi {} rp {rp} vinf {vinf} d0 {} vrel {} segs {:?}", body.name, body.soi, st.r.dist(cr), (st.v - cv).len(), tr.segs);
            }
            if tr.segs.iter().any(|s| sys.body(s.parent).parent == Some(b)) {
                continue; // ran into one of b's moons on the way: a different (also valid) path
            }
            let fb = tr.flybys(&sys);
            let Some(f) = fb.iter().find(|f| f.body == b) else {
                panic!("case {k}: no flyby of {} in {:?} / {:?}", body.name, tr.segs.iter().map(|s| s.parent).collect::<Vec<_>>(), tr.end);
            };
            n += 1;
            // closest approach and timing match the hyperbola we built
            // (loosely: the short approach arc outside the SOI is in the star frame)
            assert!((f.rp - rp).abs() < rp * 0.05, "case {k}: rp {} vs {rp}", f.rp);
            assert!((f.t - tp).abs() < 60.0, "case {k}: tp {} vs {tp}", f.t);
            assert!((f.vinf - vinf).abs() < 0.02 * vinf + 0.1, "case {k}: vinf {} vs {vinf}", f.vinf);
            // the turn between SOI entry and exit is what the conic predicts at r = SOI:
            // 2·atan(sin ν / (e + cos ν)), approaching the asymptotic 2·asin(1/e) for a big SOI
            let e = 1.0 + f.rp * f.vinf * f.vinf / body.mu;
            let cnu = ((f.rp * (1.0 + e) / body.soi - 1.0) / e).clamp(-1.0, 1.0);
            let turn = 2.0 * (1.0 - cnu * cnu).sqrt().atan2(e + cnu);
            assert!(f.turn <= 2.0 * (1.0 / e).asin() + 1e-9, "case {k}");
            assert!((f.turn - turn).abs() < 2e-3, "case {k}: turn {} vs {turn}", f.turn);
            // energy in the body frame is the same entering and leaving
            let i = tr.segs.iter().position(|s| s.parent == b).unwrap();
            let s_in = &tr.segs[i];
            let s_out = tr.segs[i..].iter().take_while(|s| s.parent == b).last().unwrap();
            let (r_out, v_out) = propagate(body.mu, s_out.r0, s_out.v0, s_out.t1 - s_out.t0);
            let en = |r: V2, v: V2| v.len2() / 2.0 - body.mu / r.len();
            assert!((en(s_in.r0, s_in.v0) - en(r_out, v_out)).abs() < 1e-6 * en(r_out, v_out).abs().max(1.0), "case {k}: energy");
            assert!((s_in.r0.len() / body.soi - 1.0).abs() < 1e-3 && (r_out.len() / body.soi - 1.0).abs() < 1e-3, "case {k}: SOI crossing");
            // position and velocity are continuous across every frame change
            for w in tr.segs.windows(2) {
                let (pa, va) = abs_state(&sys, &w[0], w[0].t1);
                let (pb, vb) = abs_state(&sys, &w[1], w[1].t0);
                assert!((w[0].t1 - w[1].t0).abs() < 1e-9, "case {k}: gap");
                assert!(pa.dist(pb) < 1e-3 && va.dist(vb) < 1e-6, "case {k}: jump {} km {} km/s", pa.dist(pb), va.dist(vb));
            }
        }
        assert!(n >= 150, "only {n} clean passes");
    }

    #[test]
    fn trailing_pass_boosts_leading_pass_brakes() {
        let sys = generate(7, SystemSize::Medium);
        let b = sys
            .bodies
            .iter()
            .filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant) && sys.children(b.id).next().is_none())
            .max_by(|a, c| a.mu.total_cmp(&c.mu))
            .unwrap();
        let tp = 5000.0;
        let (rb, vb) = sys.rel_state(b.id, tp);
        let rp = b.radius * 1.5;
        let vinf = 6.0;
        // periapsis behind the planet (opposite its velocity), moving outward from the star: CCW turn
        // bends the velocity forward → boost. The mirrored pass (in front of the planet) brakes.
        let behind = (-vb).angle();
        let gains: Vec<f64> = [(behind, rb.cross(vb) > 0.0), (behind + std::f64::consts::PI, rb.cross(vb) > 0.0)]
            .into_iter()
            .map(|(a, ccw)| {
                let tr = predict(&sys, pass(&sys, b.id, rp, vinf, a, ccw, tp), &[], None, tp + 86_400.0);
                tr.flybys(&sys)[0].gain
            })
            .collect();
        println!("gains {gains:?}");
        assert!(gains[0] > 1.0, "{gains:?}");
        assert!(gains[1] < -1.0, "{gains:?}");
    }
}

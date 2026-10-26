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

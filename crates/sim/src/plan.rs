//! Transfer planner: porkchop search over Lambert solutions for the best windows, then a
//! patched-conic shooting refinement so the real predicted path is captured at the target.
use crate::assist;
use crate::kepler::{circular_speed, Conic, ejection, lambert, oberth_dv};
use crate::system::{BodyId, System};
use crate::traj::{predict, Burn, End, State, Trajectory};
use crate::v2::V2;
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Window {
    pub depart_t: f64,
    pub arrive_t: f64,
    /// estimated total Δv (departure + capture), km/s
    pub dv: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferPlan {
    pub from: BodyId,
    pub to: BodyId,
    /// state relative to `from` immediately after the departure burn
    pub depart: State,
    pub depart_dv: f64,
    pub capture_dv: f64,
    pub arrive_t: f64,
    /// predicted path (ends in Capture at `to` when the plan converged)
    pub traj: Trajectory,
    pub converged: bool,
    /// mid-course burns (e.g. the correction after a gravity assist), flown as maneuver nodes
    #[serde(default)]
    pub burns: Vec<Burn>,
    /// bodies the path swings past on the way (gravity assists), in order
    #[serde(default)]
    pub flybys: Vec<BodyId>,
    /// what to hand `plan_hinted` (with `depart_t()`) to get exactly this plan again
    #[serde(default)]
    pub hint: Option<PlanHint>,
}

/// Enough to re-plan a chosen course deterministically (the server re-plans every order).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlanHint {
    Direct { tof: f64 },
    Assist { via: BodyId, flyby_t: f64, arrive_t: f64 },
}

/// Labels for the course options offered to the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Choice {
    Cheapest,
    Balanced,
    Fastest,
}

impl TransferPlan {
    pub fn total_dv(&self) -> f64 {
        self.depart_dv + self.mid_dv() + self.capture_dv
    }
    /// Δv of the mid-course burns.
    pub fn mid_dv(&self) -> f64 {
        self.burns.iter().map(|b| b.dv()).sum()
    }
    pub fn depart_t(&self) -> f64 {
        self.depart.t
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Rel {
    /// both orbit the same parent; Lambert leg in the parent's frame
    Sibling(BodyId),
    /// `to` orbits `from`
    Child,
    /// `from` orbits `to`
    Parent,
}

fn relation(sys: &System, from: BodyId, to: BodyId) -> Option<Rel> {
    let (bf, bt) = (sys.body(from), sys.body(to));
    if bf.parent.is_some() && bf.parent == bt.parent {
        Some(Rel::Sibling(bf.parent.unwrap()))
    } else if bt.parent == Some(from) {
        Some(Rel::Child)
    } else if bf.parent == Some(to) {
        Some(Rel::Parent)
    } else {
        None
    }
}

/// Bodies to visit (excluding `from`) to get from one body to another, one direct leg per hop.
pub fn route(sys: &System, from: BodyId, to: BodyId) -> Vec<BodyId> {
    if from == to {
        return vec![];
    }
    if relation(sys, from, to).is_some() {
        return vec![to];
    }
    let la = sys.lineage(from);
    let lb = sys.lineage(to);
    // walk up from `from` until an ancestor is related to something on `to`'s lineage
    let mut up = vec![];
    for &a in &la {
        for (j, &b) in lb.iter().enumerate() {
            if a == b || relation(sys, a, b).is_some() {
                let mut path = up.clone();
                if a != from && !path.contains(&a) {
                    path.push(a);
                }
                let mut down: Vec<BodyId> = lb[..=j].to_vec();
                down.reverse();
                for d in down {
                    if d != a && !path.contains(&d) {
                        path.push(d);
                    }
                }
                path.retain(|&x| !sys.body(x).is_star());
                return path;
            }
        }
        if a != from {
            up.push(a);
        }
    }
    vec![to]
}

fn ccw_circ(mu: f64, r: V2) -> V2 {
    r.norm().perp() * circular_speed(mu, r.len())
}

/// Δv of the departure burn from the standard (counter-clockwise, circular) parking orbit.
pub(crate) fn park_dv(sys: &System, from: BodyId, st: &State) -> f64 {
    (st.v - ccw_circ(sys.body(from).mu, st.r)).len()
}

struct Leg {
    depart: State,
    dep_dv: f64,
    cap_dv: f64,
    arrive_t: f64,
}

/// True when the coast from (r, v) passes below `min_r` within `tof` seconds.
pub(crate) fn dips_below(mu: f64, r: V2, v: V2, tof: f64, min_r: f64) -> bool {
    let c = Conic::from_state(mu, r, v);
    c.rp < min_r && c.time_to_periapsis(r, v).is_some_and(|tp| tp < tof)
}

/// Lambert estimate of a direct leg departing at td with time of flight tof.
fn leg(sys: &System, from: BodyId, to: BodyId, rel: Rel, td: f64, tof: f64) -> Option<Leg> {
    let (bf, bt) = (sys.body(from), sys.body(to));
    let ta = td + tof;
    match rel {
        Rel::Sibling(f) => {
            let mu = sys.body(f).mu;
            let r1 = sys.pos_in(from, f, td);
            let r2 = sys.pos_in(to, f, ta);
            let (v1, v2) = lambert(mu, r1, r2, tof, true)?;
            if dips_below(mu, r1, v1, tof, sys.body(f).radius * 1.2) {
                return None;
            }
            let vinf_d = v1 - sys.vel_in(from, f, td);
            let vinf_a = v2 - sys.vel_in(to, f, ta);
            let (r, v) = ejection(bf.mu, bf.park_radius(), vinf_d);
            Some(Leg {
                depart: State { parent: from, t: td, r, v },
                dep_dv: oberth_dv(bf.mu, bf.park_radius(), vinf_d.len()),
                cap_dv: oberth_dv(bt.mu, bt.park_radius(), vinf_a.len()),
                arrive_t: ta,
            })
        }
        Rel::Child => {
            let r2 = sys.pos_in(to, from, ta);
            let r1 = (-r2).norm() * bf.park_radius();
            let (v1, v2) = lambert(bf.mu, r1, r2, tof, true)?;
            if dips_below(bf.mu, r1, v1, tof, bf.radius * 1.1) {
                return None;
            }
            let vinf_a = v2 - sys.vel_in(to, from, ta);
            Some(Leg {
                depart: State { parent: from, t: td, r: r1, v: v1 },
                dep_dv: (v1 - ccw_circ(bf.mu, r1)).len(),
                cap_dv: oberth_dv(bt.mu, bt.park_radius(), vinf_a.len()),
                arrive_t: ta,
            })
        }
        Rel::Parent => {
            let r1 = sys.pos_in(from, to, td);
            let r2 = (-r1).norm().rotate(-0.15) * bt.park_radius();
            let (v1, v2) = lambert(bt.mu, r1, r2, tof, true)?;
            if dips_below(bt.mu, r1, v1, tof * 0.98, bt.radius * 1.1) {
                return None;
            }
            let vinf_d = v1 - sys.vel_in(from, to, td);
            let (r, v) = ejection(bf.mu, bf.park_radius(), vinf_d);
            Some(Leg {
                depart: State { parent: from, t: td, r, v },
                dep_dv: oberth_dv(bf.mu, bf.park_radius(), vinf_d.len()),
                cap_dv: (v2 - ccw_circ(bt.mu, r2)).len(),
                arrive_t: ta,
            })
        }
    }
}

pub(crate) fn hohmann_tof(sys: &System, from: BodyId, to: BodyId, rel: Rel) -> f64 {
    let (frame, r1, r2) = match rel {
        Rel::Sibling(f) => (f, sys.body(from).orbit.unwrap().a, sys.body(to).orbit.unwrap().a),
        Rel::Child => (from, sys.body(from).park_radius(), sys.body(to).orbit.unwrap().a),
        Rel::Parent => (to, sys.body(from).orbit.unwrap().a, sys.body(to).park_radius()),
    };
    let a = 0.5 * (r1 + r2);
    PI * (a * a * a / sys.body(frame).mu).sqrt()
}

fn tof_grid(h: f64) -> impl Iterator<Item = f64> {
    (0..14).map(move |i| h * (0.45 + 0.1 * i as f64))
}

/// Departure-time samples to search and the estimated cost of the best tof for each.
fn scan(sys: &System, from: BodyId, to: BodyId, now: f64, span: f64, n: usize) -> Vec<(f64, f64, f64)> {
    let Some(rel) = relation(sys, from, to) else { return vec![] };
    let h = hohmann_tof(sys, from, to, rel);
    (0..n)
        .filter_map(|i| {
            let td = now + 60.0 + span * i as f64 / (n - 1) as f64;
            tof_grid(h)
                .filter_map(|tof| leg(sys, from, to, rel, td, tof).map(|l| (td, tof, l.dep_dv + l.cap_dv)))
                .min_by(|a, b| a.2.total_cmp(&b.2))
        })
        .collect()
}

/// Synodic-ish search span: long enough to contain at least one good window.
pub fn search_span(sys: &System, from: BodyId, to: BodyId) -> f64 {
    let p = |b: BodyId| sys.body(b).orbit.map(|o| o.period()).unwrap_or(f64::INFINITY);
    let span = match relation(sys, from, to) {
        Some(Rel::Sibling(_)) => {
            let (a, b) = (p(from), p(to));
            (1.0 / (1.0 / a - 1.0 / b).abs()).min(a.max(b) * 1.5)
        }
        Some(Rel::Child) => p(to),
        Some(Rel::Parent) => p(from),
        None => 86_400.0,
    };
    span.clamp(3600.0, 6.0 * 86_400.0)
}

/// The best transfer windows (local minima of Δv over departure time) within `span` seconds.
pub fn windows(sys: &System, from: BodyId, to: BodyId, now: f64, span: f64) -> Vec<Window> {
    let s = scan(sys, from, to, now, span, 96);
    let mut out = vec![];
    // interior minima only: an end of the scan is just where we stopped looking
    for i in 1..s.len().saturating_sub(1) {
        let c = s[i].2;
        let l = if i > 0 { s[i - 1].2 } else { f64::INFINITY };
        let r = if i + 1 < s.len() { s[i + 1].2 } else { f64::INFINITY };
        if c <= l && c <= r {
            out.push(Window { depart_t: s[i].0, arrive_t: s[i].0 + s[i].1, dv: c });
        }
    }
    out
}

/// Cost curve for the time-scrubber / porkchop strip: (depart time, best Δv) samples.
pub fn dv_curve(sys: &System, from: BodyId, to: BodyId, now: f64, span: f64, n: usize) -> Vec<(f64, f64)> {
    scan(sys, from, to, now, span, n.max(2)).into_iter().map(|(t, _, c)| (t, c)).collect()
}

/// Plan a single direct leg. With `depart_t = None` the best window within the search span is
/// used. The result's trajectory is a real patched-conic prediction ending in a capture.
pub fn plan_leg(sys: &System, from: BodyId, to: BodyId, now: f64, depart_t: Option<f64>) -> Result<TransferPlan, String> {
    let rel = relation(sys, from, to).ok_or("no direct transfer between these bodies")?;
    let h = hohmann_tof(sys, from, to, rel);
    let starts: Vec<(f64, f64)> = match depart_t {
        Some(td) => {
            let td = td.max(now + 30.0);
            let best = tof_grid(h)
                .filter_map(|tof| leg(sys, from, to, rel, td, tof).map(|l| (tof, l.dep_dv + l.cap_dv)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .ok_or("no transfer solution for that departure time")?;
            vec![(td, best.0)]
        }
        None => {
            // the Lambert estimate ignores the planets' own gravity wells, so the cheapest estimate
            // is not always the cheapest real path: refine the best few windows and keep the winner
            let span = search_span(sys, from, to);
            let s = scan(sys, from, to, now, span, 96);
            let mut minima: Vec<(f64, f64, f64)> = (0..s.len())
                .filter(|&i| {
                    let l = if i > 0 { s[i - 1].2 } else { f64::INFINITY };
                    let r = if i + 1 < s.len() { s[i + 1].2 } else { f64::INFINITY };
                    s[i].2 <= l && s[i].2 <= r
                })
                .map(|i| s[i])
                .collect();
            minima.sort_by(|a, b| a.2.total_cmp(&b.2));
            minima.truncate(3);
            if minima.is_empty() {
                return Err("no transfer solution found".into());
            }
            minima.into_iter().map(|m| (m.0, m.1)).collect()
        }
    };
    // local refinement of (td, tof) on the Lambert estimate (skip td when it is user-fixed)
    let cost = |td: f64, tof: f64| leg(sys, from, to, rel, td, tof).map(|l| l.dep_dv + l.cap_dv).unwrap_or(1e9);
    let mut best: Option<(f64, TransferPlan)> = None;
    for (td0, tof0) in starts {
        let (mut td, mut tof) = (td0, tof0);
        let mut step_t = h * 0.05;
        let mut step_d = h * 0.05;
        for _ in 0..40 {
            let base = cost(td, tof);
            let mut improved = false;
            for (dtd, dtf) in [(step_d, 0.0), (-step_d, 0.0), (0.0, step_t), (0.0, -step_t)] {
                if depart_t.is_some() && dtd != 0.0 {
                    continue;
                }
                let (a, b) = (td + dtd, tof + dtf);
                if a < now + 30.0 || b < h * 0.2 {
                    continue;
                }
                if cost(a, b) < base {
                    td = a;
                    tof = b;
                    improved = true;
                    break;
                }
            }
            if !improved {
                step_t *= 0.5;
                step_d *= 0.5;
            }
        }
        let Some(l) = leg(sys, from, to, rel, td, tof) else { continue };
        for scale in [1.0, 0.25] {
            if scale < 1.0 && best.as_ref().is_some_and(|b| b.1.converged) {
                break; // only retry with a finer simplex when the first attempt missed
            }
            let plan = shoot(sys, from, to, &l, tof, scale);
            // converged plans rank by fuel; otherwise by how close they got
            let score = if plan.converged { plan.total_dv() } else { 1e6 + plan.traj.closest_approach(sys, to).0 };
            if best.as_ref().is_none_or(|b| score < b.0) {
                best = Some((score, plan));
            }
        }
    }
    let best = best.map(|b| b.1).ok_or_else(|| "no transfer solution".to_string())?;
    if depart_t.is_some() || !best.converged {
        return Ok(best);
    }
    Ok(slide_departure(sys, from, to, now, best))
}

/// The Lambert estimate ignores the planets' own wells, so the real optimum can sit a little off
/// the estimated window: probe the real cost 5% of the span either side and keep the cheapest.
fn slide_departure(sys: &System, from: BodyId, to: BodyId, now: f64, best: TransferPlan) -> TransferPlan {
    let step = search_span(sys, from, to) * 0.05;
    let td0 = best.depart_t();
    let mut cur = best;
    for d in [-1.0, 1.0] {
        let td = td0 + d * step;
        if td < now + 30.0 {
            continue;
        }
        if let Ok(p) = plan_leg(sys, from, to, now, Some(td)) {
            if p.converged && p.total_dv() < cur.total_dv() {
                cur = p;
            }
        }
    }
    cur
}

/// Like `plan_leg`, but also considers swinging past another body on the way (gravity assist) and
/// takes that route when it saves at least 5% Δv. Assist routes carry one correction burn after
/// the pass (`TransferPlan::burns`).
pub fn plan_route(sys: &System, from: BodyId, to: BodyId, now: f64, depart_t: Option<f64>) -> Result<TransferPlan, String> {
    let direct = plan_leg(sys, from, to, now, depart_t);
    let bar = direct.as_ref().ok().filter(|p| p.converged).map(|p| p.total_dv()).unwrap_or(f64::INFINITY);
    let span = search_span(sys, from, to);
    // the Lambert screen is cheap; only the best one or two survivors are flown for real
    for c in assist::screen(sys, from, to, now, span, depart_t).iter().take(2) {
        if c.total() > bar * 0.9 {
            break;
        }
        if let Some(p) = assist::fly(sys, from, to, c, bar * 0.95) {
            if p.total_dv() < bar * 0.95 {
                return Ok(p);
            }
        }
    }
    direct
}

/// Re-plan a course from its hint: the same departure time and hint give the same plan.
pub fn plan_hinted(sys: &System, from: BodyId, to: BodyId, depart_t: f64, hint: PlanHint) -> Result<TransferPlan, String> {
    let fail = || "couldn't find that course any more, plan again".to_string();
    match hint {
        PlanHint::Direct { tof } => {
            let rel = relation(sys, from, to).ok_or("no direct transfer between these bodies")?;
            if !(tof.is_finite() && tof > 60.0) {
                return Err(fail());
            }
            let l = leg(sys, from, to, rel, depart_t, tof).ok_or_else(fail)?;
            let plan = shoot(sys, from, to, &l, tof, 1.0);
            if plan.converged {
                return Ok(plan);
            }
            let fine = shoot(sys, from, to, &l, tof, 0.25);
            if fine.converged {
                Ok(fine)
            } else {
                Err(fail())
            }
        }
        PlanHint::Assist { via, flyby_t, arrive_t } => {
            if !assist::assist_bodies(sys, from, to).contains(&via) {
                return Err(fail());
            }
            let c = assist::estimate(sys, from, to, via, depart_t, flyby_t, arrive_t).ok_or_else(fail)?;
            assist::fly(sys, from, to, &c, f64::INFINITY).ok_or_else(fail)
        }
    }
}

/// Course options from cheapest to fastest: the cheapest plan (gravity assists included), and
/// direct courses that leave sooner / fly shorter for more Δv, up to `max_dv` (the ship's budget).
/// At most three, each a different arrival time, sorted by Δv.
pub fn options(sys: &System, from: BodyId, to: BodyId, now: f64, depart_t: Option<f64>, max_dv: f64) -> Vec<(Choice, TransferPlan)> {
    let mut out = vec![];
    let Ok(cheap) = plan_route(sys, from, to, now, depart_t) else { return out };
    if !cheap.converged {
        return out;
    }
    let Some(rel) = relation(sys, from, to) else { return vec![(Choice::Cheapest, cheap)] };
    let h = hohmann_tof(sys, from, to, rel);
    let base = cheap.total_dv();
    // leave as soon as possible (or at the chosen time) and shorten the flight
    let td = depart_t.unwrap_or(now + 60.0).max(now + 30.0);
    let est: Vec<(f64, f64)> = (0..24)
        .map(|i| h * (0.2 + 0.05 * i as f64))
        .filter_map(|tof| leg(sys, from, to, rel, td, tof).map(|l| (tof, l.dep_dv + l.cap_dv)))
        .collect();
    let mut tried: Vec<f64> = vec![];
    for (choice, cap) in [(Choice::Fastest, (max_dv * 0.9).min(base * 3.0)), (Choice::Balanced, base * 1.4)] {
        // shortest flight whose estimate fits the budget; one retry a little longer if it misses
        let fits: Vec<f64> = est.iter().filter(|e| e.1 <= cap).map(|e| e.0).collect();
        for &tof in fits.iter().take(2) {
            if tried.contains(&tof) {
                break;
            }
            tried.push(tof);
            if let Ok(p) = plan_hinted(sys, from, to, td, PlanHint::Direct { tof }) {
                if p.total_dv() <= cap.max(base) * 1.1 && p.arrive_t < cheap.arrive_t - 600.0 {
                    out.push((choice, p));
                }
                break;
            }
        }
    }
    out.push((Choice::Cheapest, cheap));
    // drop options that arrive within the same hour as a cheaper one
    out.sort_by(|a, b| a.1.total_dv().total_cmp(&b.1.total_dv()));
    let mut kept: Vec<(Choice, TransferPlan)> = vec![];
    for o in out {
        if !kept.iter().any(|k| (k.1.arrive_t - o.1.arrive_t).abs() < 3600.0 || k.1.arrive_t <= o.1.arrive_t) {
            kept.push(o);
        }
    }
    kept
}

/// Correct the Lambert estimate against the real patched-conic path with a Nelder–Mead search over
/// (rotation of the departure point, speed scale, radial kick).
fn shoot(sys: &System, from: BodyId, to: BodyId, l: &Leg, tof: f64, scale: f64) -> TransferPlan {
    let bt = sys.body(to);
    let horizon = l.arrive_t + tof * 0.6 + 600.0;
    let mu_f = sys.body(from).mu;
    let est = (l.dep_dv + l.cap_dv).max(0.5);
    let make = |p: &[f64; 3]| -> State {
        let r = l.depart.r.rotate(p[0]);
        let v = l.depart.v.rotate(p[0]) * (1.0 + p[1]) + r.norm() * p[2];
        State { r, v, ..l.depart }
    };
    let eval = |p: &[f64; 3]| -> (f64, Trajectory) {
        let st = make(p);
        let tr = predict(sys, st, &[], Some(to), horizon);
        let err = match tr.end {
            End::Capture { body, rp, vp, .. } if body == to => {
                let target = bt.park_radius();
                let dv = (st.v - ccw_circ(mu_f, st.r)).len() + vp - circular_speed(bt.mu, rp);
                ((rp - target) / target).abs() * 0.05 + 0.01 * dv / est
            }
            End::Impact { body, .. } if body == to => {
                let s = tr.segs.last().unwrap();
                let rp = Conic::from_state(bt.mu, s.r0, s.v0).rp;
                0.2 + 0.2 * (1.0 - rp / bt.park_radius()).clamp(0.0, 1.0)
            }
            _ => {
                let (d, _) = tr.closest_approach(sys, to);
                1.0 + d / bt.soi
            }
        };
        (err, tr)
    };
    let mut pts: Vec<([f64; 3], f64)> = Vec::new();
    for p in [[0.0, 0.0, 0.0], [0.03 * scale, 0.0, 0.0], [0.0, 0.01 * scale, 0.0], [0.0, 0.0, 0.02 * scale * l.depart.v.len()]] {
        pts.push((p, eval(&p).0));
    }
    for _ in 0..140 {
        pts.sort_by(|a, b| a.1.total_cmp(&b.1));
        if (pts[pts.len() - 1].1 - pts[0].1).abs() < 1e-6 {
            break;
        }
        let n = pts.len() - 1;
        let mut c = [0.0; 3];
        for (p, _) in &pts[..n] {
            for k in 0..3 {
                c[k] += p[k] / n as f64;
            }
        }
        let lerp = |a: &[f64; 3], b: &[f64; 3], t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        let worst = pts[n];
        let refl = lerp(&c, &worst.0, -1.0);
        let fr = eval(&refl).0;
        if fr < pts[0].1 {
            let exp = lerp(&c, &worst.0, -2.0);
            let fe = eval(&exp).0;
            pts[n] = if fe < fr { (exp, fe) } else { (refl, fr) };
        } else if fr < pts[n - 1].1 {
            pts[n] = (refl, fr);
        } else {
            let con = lerp(&c, &worst.0, 0.5);
            let fc = eval(&con).0;
            if fc < worst.1 {
                pts[n] = (con, fc);
            } else {
                let best = pts[0].0;
                for i in 1..=n {
                    let p = lerp(&best, &pts[i].0, 0.5);
                    pts[i] = (p, eval(&p).0);
                }
            }
        }
    }
    pts.sort_by(|a, b| a.1.total_cmp(&b.1));
    let (err, traj) = eval(&pts[0].0);
    let depart = make(&pts[0].0);
    let depart_dv = (depart.v - ccw_circ(mu_f, depart.r)).len();
    let (converged, capture_dv, arrive_t) = match traj.end {
        End::Capture { body, t, rp, vp } if body == to && err < 0.08 => (true, vp - circular_speed(bt.mu, rp), t),
        _ => (false, l.cap_dv, l.arrive_t),
    };
    TransferPlan { from, to, depart, depart_dv, capture_dv, arrive_t, traj, converged, burns: vec![], flybys: vec![], hint: Some(PlanHint::Direct { tof }) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::{generate, BodyKind, SystemSize};

    fn planets(sys: &System) -> Vec<BodyId> {
        sys.bodies.iter().filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant)).map(|b| b.id).collect()
    }

    #[test]
    fn planet_to_planet_converges() {
        let sys = generate(7, SystemSize::Medium);
        let p = planets(&sys);
        let t0 = std::time::Instant::now();
        let plan = plan_leg(&sys, p[2], p[3], 1000.0, None).unwrap();
        println!(
            "{} -> {}: dep {:.2} km/s cap {:.2} km/s, depart +{:.1}h arrive +{:.1}h, converged {} ({:?})",
            sys.body(p[2]).name,
            sys.body(p[3]).name,
            plan.depart_dv,
            plan.capture_dv,
            (plan.depart_t() - 1000.0) / 3600.0,
            (plan.arrive_t - 1000.0) / 3600.0,
            plan.converged,
            t0.elapsed()
        );
        assert!(plan.converged, "end = {:?}", plan.traj.end);
        assert!(plan.total_dv() < 15.0);
        let w = windows(&sys, p[2], p[3], 0.0, search_span(&sys, p[2], p[3]));
        assert!(!w.is_empty());
    }

    #[test]
    fn planet_to_moon_and_back() {
        let sys = generate(7, SystemSize::Medium);
        let moon = sys.bodies.iter().find(|b| b.kind == BodyKind::Moon).unwrap();
        let planet = moon.parent.unwrap();
        let out = plan_leg(&sys, planet, moon.id, 0.0, None).unwrap();
        println!("to moon: {:.2}+{:.2} km/s conv {} {:?}", out.depart_dv, out.capture_dv, out.converged, out.traj.end);
        assert!(out.converged);
        let back = plan_leg(&sys, moon.id, planet, 0.0, None).unwrap();
        println!("to planet: {:.2}+{:.2} km/s conv {} {:?}", back.depart_dv, back.capture_dv, back.converged, back.traj.end);
        assert!(back.converged);
    }

    #[test]
    fn all_planet_pairs() {
        let sys = generate(11, SystemSize::Medium);
        let p = planets(&sys);
        let mut fails = 0;
        for i in 0..p.len() {
            for j in 0..p.len() {
                if i == j {
                    continue;
                }
                let plan = plan_leg(&sys, p[i], p[j], 0.0, None).unwrap();
                println!("{i}->{j}: {:.2}+{:.2} conv {}", plan.depart_dv, plan.capture_dv, plan.converged);
                if !plan.converged {
                    fails += 1;
                }
            }
        }
        assert!(fails <= 2, "{fails} pairs failed");
    }

    #[test]
    fn routes() {
        let sys = generate(7, SystemSize::Medium);
        let moons: Vec<_> = sys.bodies.iter().filter(|b| b.kind == BodyKind::Moon).collect();
        let a = moons[0];
        let b = moons.iter().find(|m| m.parent != a.parent).unwrap();
        let r = route(&sys, a.id, b.id);
        assert_eq!(r, vec![a.parent.unwrap(), b.parent.unwrap(), b.id]);
    }
}

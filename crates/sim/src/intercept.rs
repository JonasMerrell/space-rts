//! Intercept planner: a course that meets an enemy ship where our sensors predict it will be.
//!
//! The target's path is only what the player knows (`Game::known_path`: the fogged copy of the
//! ship, so an unscanned target is assumed to keep coasting from its last burn). For a departure
//! time and a meeting time a Lambert arc in the common frame gives the estimate; Nelder–Mead on
//! the real patched-conic prediction then removes the miss. An optional match burn at the meeting
//! point cancels the relative velocity, so the chaser rides along with the target and the fight
//! lasts (damage per pass is higher slow than fast, see combat.rs). Without it the chaser flies
//! through at the closing speed.
use crate::game::{Flight, Game, Ship, ShipId, ShipState};
use crate::kepler::{circular_speed, ejection, lambert, oberth_dv};
use crate::system::{BodyId, System};
use crate::traj::{predict, Burn, End, State, Trajectory};
use crate::v2::V2;
use serde::{Deserialize, Serialize};

/// A miss below this counts as an intercept (kinetic weapons reach 4000 km).
pub const HIT_KM: f64 = 1_500.0;
/// Shortest flight we plan (and the earliest meeting after now).
const MIN_TOF: f64 = 900.0;
/// How soon after `now` the departure burn can be.
pub const LEAD: f64 = 120.0;

/// Everything needed to re-plan an intercept exactly (the server re-plans every order).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hint {
    pub target: ShipId,
    pub depart_t: f64,
    pub arrive_t: f64,
    /// match the target's velocity on arrival
    pub matched: bool,
}

/// Where the chaser starts from: parked at a body, or coasting on a path (its burns dropped).
#[derive(Clone, Debug)]
pub enum Chaser {
    Parked(BodyId),
    Flying(Trajectory),
}

impl Chaser {
    pub fn of(sys: &System, s: &Ship) -> Chaser {
        match &s.state {
            ShipState::Parked { body, .. } => Chaser::Parked(*body),
            ShipState::Flying(f) => Chaser::Flying(coast(sys, f.start)),
        }
    }
    fn state_at(&self, sys: &System, t: f64) -> Option<State> {
        match self {
            Chaser::Parked(_) => None,
            Chaser::Flying(tr) => tr.state_at(sys, t),
        }
    }
}

fn coast(sys: &System, st: State) -> Trajectory {
    predict(sys, st, &[], None, st.t + Flight::HORIZON)
}

#[derive(Clone, Debug)]
pub struct InterceptPlan {
    pub hint: Hint,
    /// chaser state right after the departure burn (relative to the parking body, or to the
    /// segment parent when it was already flying)
    pub depart: State,
    pub depart_dv: f64,
    /// the departure as a burn on the current path (flying chasers only)
    pub depart_burn: Option<Burn>,
    /// velocity match at the meeting point (None = fly through)
    pub match_burn: Option<Burn>,
    pub match_dv: f64,
    /// relative speed at the meeting point before any match burn, km/s
    pub closing: f64,
    /// distance to the target at the meeting time, km
    pub miss: f64,
    /// chaser path through the meeting point (match burn applied)
    pub traj: Trajectory,
    pub converged: bool,
}

impl InterceptPlan {
    pub fn total_dv(&self) -> f64 {
        self.depart_dv + self.match_dv
    }
    pub fn arrive_t(&self) -> f64 {
        self.hint.arrive_t
    }
    pub fn depart_t(&self) -> f64 {
        self.hint.depart_t
    }
    /// Burns the flight carries after departure (the match burn, if any).
    pub fn burns(&self) -> Vec<Burn> {
        self.match_burn.into_iter().collect()
    }
}

/// Absolute state on a trajectory.
fn abs_state(sys: &System, tr: &Trajectory, t: f64) -> Option<(V2, V2)> {
    let st = tr.state_at(sys, t)?;
    Some((sys.pos(st.parent, t) + st.r, sys.vel(st.parent, t) + st.v))
}

/// Last time the target's known path can be met: a captured target is parked after that (plan a
/// transfer instead), a crash ends it.
pub fn reach_end(tgt: &Trajectory) -> f64 {
    match tgt.end {
        End::Capture { t, .. } | End::Impact { t, .. } => t - 60.0,
        End::Horizon { t } => t,
    }
}

/// Lowest common ancestor of two bodies.
fn common(sys: &System, a: BodyId, b: BodyId) -> BodyId {
    let lb = sys.lineage(b);
    *sys.lineage(a).iter().find(|x| lb.contains(x)).unwrap_or(&sys.star().id)
}

/// Lambert estimate: chaser state after the departure burn and its Δv, plus the closing velocity
/// (target minus chaser, absolute) at the meeting point.
fn estimate(sys: &System, ch: &Chaser, tgt: &Trajectory, td: f64, ta: f64) -> Option<(State, f64, V2)> {
    let tof = ta - td;
    if tof < MIN_TOF * 0.5 {
        return None;
    }
    let ts = tgt.state_at(sys, ta)?;
    let (pt, vt) = (sys.pos(ts.parent, ta) + ts.r, sys.vel(ts.parent, ta) + ts.v);
    match ch {
        Chaser::Parked(a) => {
            let a = *a;
            let ba = sys.body(a);
            let mu_a = ba.mu;
            if sys.lineage(ts.parent).contains(&a) {
                // the target is inside our own SOI: leave the parking orbit on the far side
                let r2 = pt - sys.pos(a, ta);
                let r1 = (-r2).norm() * ba.park_radius();
                let (v1, v2) = lambert(mu_a, r1, r2, tof, true)?;
                let dv = (v1 - r1.norm().perp() * circular_speed(mu_a, r1.len())).len();
                return Some((State { parent: a, t: td, r: r1, v: v1 }, dv, vt - (v2 + sys.vel(a, ta))));
            }
            let f = common(sys, ba.parent?, ts.parent);
            // climb to the ancestor of A that orbits F (A itself for planets → planets)
            let x = *sys.lineage(a).iter().find(|&&x| sys.body(x).parent == Some(f))?;
            let mu = sys.body(f).mu;
            let r1 = sys.pos_in(x, f, td);
            let r2 = pt - sys.pos(f, ta);
            let (v1, v2) = lambert(mu, r1, r2, tof, true)?;
            if crate::plan::dips_below(mu, r1, v1, tof, sys.body(f).radius * 1.2) {
                return None;
            }
            let vinf = v1 + sys.vel(f, td) - sys.vel(a, td);
            let (r, v) = ejection(mu_a, ba.park_radius(), vinf);
            Some((State { parent: a, t: td, r, v }, oberth_dv(mu_a, ba.park_radius(), vinf.len()), vt - (v2 + sys.vel(f, ta))))
        }
        Chaser::Flying(_) => {
            let cs = ch.state_at(sys, td)?;
            let f = common(sys, cs.parent, ts.parent);
            let mu = sys.body(f).mu;
            let r1 = sys.pos(cs.parent, td) + cs.r - sys.pos(f, td);
            let vc = sys.vel(cs.parent, td) + cs.v - sys.vel(f, td);
            let r2 = pt - sys.pos(f, ta);
            let (v1, v2) = lambert(mu, r1, r2, tof, true)?;
            if crate::plan::dips_below(mu, r1, v1, tof, sys.body(f).radius * 1.2) {
                return None;
            }
            let dv = v1 - vc;
            Some((State { v: cs.v + dv, ..cs }, dv.len(), vt - (v2 + sys.vel(f, ta))))
        }
    }
}

/// Estimated Δv of an intercept (departure, plus the match burn when `matched`).
fn est_cost(sys: &System, ch: &Chaser, tgt: &Trajectory, td: f64, ta: f64, matched: bool) -> Option<f64> {
    let (_, dv, close) = estimate(sys, ch, tgt, td, ta)?;
    Some(dv + if matched { close.len() } else { 0.0 })
}

fn nelder_mead(f: &dyn Fn(&[f64]) -> f64, start: &[f64], step: &[f64], iters: usize, tol: f64) -> (Vec<f64>, f64) {
    let n = start.len();
    let mut pts: Vec<(Vec<f64>, f64)> = vec![(start.to_vec(), f(start))];
    for k in 0..n {
        let mut p = start.to_vec();
        p[k] += step[k];
        let fp = f(&p);
        pts.push((p, fp));
    }
    let lerp = |a: &[f64], b: &[f64], t: f64| -> Vec<f64> { a.iter().zip(b).map(|(x, y)| x + (y - x) * t).collect() };
    for _ in 0..iters {
        pts.sort_by(|a, b| a.1.total_cmp(&b.1));
        if pts[0].1 < tol || (pts[n].1 - pts[0].1).abs() < tol * 0.01 {
            break;
        }
        let mut c = vec![0.0; n];
        for (p, _) in &pts[..n] {
            for k in 0..n {
                c[k] += p[k] / n as f64;
            }
        }
        let worst = pts[n].clone();
        let refl = lerp(&c, &worst.0, -1.0);
        let fr = f(&refl);
        if fr < pts[0].1 {
            let exp = lerp(&c, &worst.0, -2.0);
            let fe = f(&exp);
            pts[n] = if fe < fr { (exp, fe) } else { (refl, fr) };
        } else if fr < pts[n - 1].1 {
            pts[n] = (refl, fr);
        } else {
            let con = lerp(&c, &worst.0, 0.5);
            let fc = f(&con);
            if fc < worst.1 {
                pts[n] = (con, fc);
            } else {
                let best = pts[0].0.clone();
                for p in pts.iter_mut().skip(1) {
                    let q = lerp(&best, &p.0, 0.5);
                    let fq = f(&q);
                    *p = (q, fq);
                }
            }
        }
    }
    pts.sort_by(|a, b| a.1.total_cmp(&b.1));
    pts.swap_remove(0)
}

/// Damped Newton on a 2-parameter aim whose result is a 2D miss vector (finite-difference
/// Jacobian). Returns the parameters and the miss distance (1e12 when it never flew).
fn newton(f: &dyn Fn(&[f64]) -> Option<V2>, eps: f64) -> (Vec<f64>, f64) {
    let mut p = vec![0.0, 0.0];
    let Some(mut m) = f(&p) else { return (p, 1e12) };
    for _ in 0..14 {
        if m.len() < 50.0 {
            break;
        }
        let (Some(a), Some(b)) = (f(&[p[0] + eps, p[1]]), f(&[p[0], p[1] + eps])) else { break };
        let (ja, jb) = ((a - m) * (1.0 / eps), (b - m) * (1.0 / eps));
        let det = ja.x * jb.y - jb.x * ja.y;
        if det.abs() < 1e-12 {
            break;
        }
        // J d = -m
        let d0 = (-m.x * jb.y + jb.x * m.y) / det;
        let d1 = (-ja.x * m.y + m.x * ja.y) / det;
        let mut k = 1.0;
        let mut moved = false;
        for _ in 0..6 {
            let q = [p[0] + d0 * k, p[1] + d1 * k];
            // keep the aim sane: a few tenths of a radian / of the speed
            if q[0].abs() < 1.0 && q[1].abs() < 1.0 {
                if let Some(mq) = f(&q) {
                    if mq.len() < m.len() {
                        p = q.to_vec();
                        m = mq;
                        moved = true;
                        break;
                    }
                }
            }
            k *= 0.5;
        }
        if !moved {
            break;
        }
    }
    (p, m.len())
}

/// Plan exactly the intercept `hint` describes. Same inputs, same plan (the server re-plans
/// orders with it). Err when there is no course for those times.
pub fn plan_hinted(sys: &System, ch: &Chaser, tgt: &Trajectory, hint: Hint) -> Result<InterceptPlan, String> {
    let fail = || "no intercept course for those times".to_string();
    let (td, ta) = (hint.depart_t, hint.arrive_t);
    if !(td.is_finite() && ta.is_finite()) || ta > reach_end(tgt) {
        return Err(fail());
    }
    let (est, _, _) = estimate(sys, ch, tgt, td, ta).ok_or_else(fail)?;
    let (pt, _) = abs_state(sys, tgt, ta).ok_or_else(fail)?;
    let horizon = ta + 1.0;
    let base_v = est.v.len().max(1e-3);
    let make = |p: &[f64]| -> State {
        match ch {
            // rotate the departure point around the parking orbit, scale the speed, add a radial kick
            Chaser::Parked(_) => {
                let r = est.r.rotate(p[0]);
                State { r, v: est.v.rotate(p[0]) * (1.0 + p[1]) + r.norm() * p.get(2).copied().unwrap_or(0.0), ..est }
            }
            // nudge the burn vector
            Chaser::Flying(_) => State { v: est.v + V2 { x: p[0], y: p[1] } * base_v, ..est },
        }
    };
    // miss vector (chaser minus target at the meeting time); None = crashed or lost on the way
    let miss_v = |p: &[f64]| -> Option<V2> {
        let tr = predict(sys, make(p), &[], None, horizon);
        match tr.end {
            End::Horizon { .. } => abs_state(sys, &tr, ta).map(|(pc, _)| pc - pt),
            _ => None,
        }
    };
    let obj = |p: &[f64]| miss_v(p).map_or(1e12, |m| m.len());
    // Newton on the two aim parameters (a dozen predictions when it works), Nelder–Mead otherwise
    let (mut best, mut err) = newton(&miss_v, 2.0e-4);
    let step: Vec<f64> = match ch {
        Chaser::Parked(_) => vec![0.02, 0.01, 0.01 * base_v],
        Chaser::Flying(_) => vec![0.01, 0.01],
    };
    let pad = |p: &[f64]| -> Vec<f64> { p.iter().copied().chain(std::iter::repeat(0.0)).take(step.len()).collect() };
    // where Newton got stuck, then from scratch (the robust, slow way)
    for (start, iters) in [(pad(&best), 120), (pad(&[]), 220)] {
        if err <= HIT_KM {
            break;
        }
        let (b2, e2) = nelder_mead(&obj, &start, &step, iters, 100.0);
        if e2 < err {
            best = b2;
            err = e2;
        }
    }
    let best: Vec<f64> = best.into_iter().chain(std::iter::repeat(0.0)).take(3).collect();
    let depart = make(&best);
    let (depart_dv, depart_burn) = match ch {
        Chaser::Parked(a) => {
            let mu = sys.body(*a).mu;
            ((depart.v - depart.r.norm().perp() * circular_speed(mu, depart.r.len())).len(), None)
        }
        Chaser::Flying(_) => {
            let before = ch.state_at(sys, td).ok_or_else(fail)?;
            let dv = depart.v - before.v;
            (dv.len(), Some(Burn::from_vector(td, before.r, before.v, dv)))
        }
    };
    let coast_tr = predict(sys, depart, &[], None, horizon);
    let st = coast_tr.state_at(sys, ta).ok_or_else(fail)?;
    let (_, vt) = abs_state(sys, tgt, ta).ok_or_else(fail)?;
    let rel = vt - (sys.vel(st.parent, ta) + st.v);
    let match_burn = hint.matched.then(|| Burn::from_vector(ta, st.r, st.v, rel));
    let burns: Vec<Burn> = match_burn.into_iter().collect();
    // a little past the meeting so the map shows where the chaser goes on
    let traj = predict(sys, depart, &burns, None, ta + ((ta - td) * 0.25).max(3600.0));
    Ok(InterceptPlan {
        hint,
        depart,
        depart_dv,
        depart_burn,
        match_burn,
        match_dv: if hint.matched { rel.len() } else { 0.0 },
        closing: rel.len(),
        miss: err,
        traj,
        converged: err <= HIT_KM,
    })
}

/// Cheapest departure time and its estimated cost for meeting the target at `ta`.
fn best_departure(sys: &System, ch: &Chaser, tgt: &Trajectory, now: f64, ta: f64, matched: bool) -> Option<(f64, f64)> {
    let t0 = now + LEAD;
    let latest = ta - MIN_TOF;
    if latest < t0 {
        return None;
    }
    let n = 12;
    let mut best: Option<(f64, f64)> = None;
    for j in 0..n {
        let td = t0 + (latest - t0) * j as f64 / (n - 1) as f64;
        if let Some(c) = est_cost(sys, ch, tgt, td, ta, matched) {
            if best.is_none_or(|b| c < b.1) {
                best = Some((td, c));
            }
        }
    }
    // a few rounds of golden-section-ish refinement around the winner
    let (mut td, mut c) = best?;
    let mut step = (latest - t0) / (n - 1) as f64 * 0.5;
    for _ in 0..8 {
        for cand in [td - step, td + step] {
            if cand < t0 || cand > latest {
                continue;
            }
            if let Some(cc) = est_cost(sys, ch, tgt, cand, ta, matched) {
                if cc < c {
                    td = cand;
                    c = cc;
                }
            }
        }
        step *= 0.5;
    }
    Some((td, c))
}

/// Intercept courses from cheapest to fastest (the fuel ↔ time slider's stops): each arrives
/// sooner and costs more than the one before, all within `max_dv`. Empty when the target can't be
/// reached (out of its known path, or out of fuel).
pub fn options(sys: &System, ch: &Chaser, tgt: &Trajectory, target: ShipId, now: f64, max_dv: f64, matched: bool) -> Vec<InterceptPlan> {
    let end = reach_end(tgt).min(now + 4.0 * 86_400.0);
    let first = now + LEAD + MIN_TOF;
    if end <= first {
        return vec![];
    }
    // meeting-time grid, denser near now (quadratic spacing), and the Pareto front of it
    let n = 60;
    let span = end - first;
    let mut front: Vec<(f64, f64, f64)> = vec![]; // (arrive, depart, est cost)
    for i in 0..n {
        let ta = first + span * (i as f64 / (n - 1) as f64).powi(2);
        if let Some((td, c)) = best_departure(sys, ch, tgt, now, ta, matched) {
            if c <= max_dv && front.iter().all(|f| f.2 > c) {
                front.push((ta, td, c));
            }
        }
    }
    // front is in arrival order with falling cost: the cheapest is the latest
    front.reverse();
    let (Some(&(.., lo)), Some(&(.., hi))) = (front.first(), front.last()) else { return vec![] };
    const STEPS: usize = 6;
    let mut out: Vec<InterceptPlan> = vec![];
    let mut tried: Vec<usize> = vec![];
    for k in 0..STEPS {
        let want = lo + (hi - lo) * k as f64 / (STEPS - 1) as f64;
        let mut near: Vec<usize> = (0..front.len()).collect();
        near.sort_by(|&a, &b| (front[a].2 - want).abs().total_cmp(&(front[b].2 - want).abs()));
        for i in near.into_iter().take(3) {
            if tried.contains(&i) {
                continue;
            }
            tried.push(i);
            let (ta, td, _) = front[i];
            let Ok(p) = plan_hinted(sys, ch, tgt, Hint { target, depart_t: td, arrive_t: ta, matched }) else { continue };
            if !p.converged || p.total_dv() > max_dv {
                continue;
            }
            if out.last().is_none_or(|l| p.total_dv() > l.total_dv() && p.arrive_t() < l.arrive_t() - 300.0) {
                out.push(p);
                break;
            }
        }
    }
    out
}

/// An intercept part way (`f` = 0..1) between two neighbouring `options` (departure and meeting
/// times interpolated, then planned exactly). None when it doesn't close.
pub fn between(sys: &System, ch: &Chaser, tgt: &Trajectory, a: Hint, b: Hint, f: f64) -> Option<InterceptPlan> {
    if !(0.0..=1.0).contains(&f) || a.target != b.target || a.matched != b.matched {
        return None;
    }
    let l = |x: f64, y: f64| x + (y - x) * f;
    let p = plan_hinted(sys, ch, tgt, Hint { depart_t: l(a.depart_t, b.depart_t), arrive_t: l(a.arrive_t, b.arrive_t), ..a }).ok()?;
    p.converged.then_some(p)
}

/// A ship's standing intercept order.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub target: ShipId,
    pub matched: bool,
    pub arrive_t: f64,
    /// fingerprint of the target's course as the owner knew it when this was planned
    pub known: u64,
    /// the target is out of sensor range (the chaser coasts on)
    pub lost: bool,
}

/// Fingerprint of what a (redacted) ship's course looks like: changes when it burns or parks.
fn course_sig(s: &Ship) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |x: u64| h = (h ^ x).wrapping_mul(0x0100_0000_01b3);
    match &s.state {
        ShipState::Parked { body, .. } => mix(body.0 as u64),
        ShipState::Flying(f) => {
            for x in [f.start.t, f.start.v.x, f.start.v.y] {
                mix(x.to_bits());
            }
            mix(f.dest.map_or(u64::MAX, |b| b.0 as u64));
            for b in &f.burns {
                mix(b.t.to_bits());
                mix(b.prograde.to_bits());
            }
        }
    }
    h
}

/// Arrival times the server tries when it re-plans a chase on its own (cheap: estimates only).
fn replan_times(now: f64, old: f64, end: f64) -> Vec<f64> {
    let first = now + LEAD + MIN_TOF;
    let mut v: Vec<f64> = vec![];
    if old >= first && old <= end {
        v.push(old);
    }
    for h in [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0] {
        let t = now + LEAD + h * 3600.0;
        if t >= first && t <= end {
            v.push(t);
        }
    }
    v
}

impl Game {
    /// The path player `p` knows for ship `id`: the fogged copy (burns only when scanned).
    /// None when `p` can't see it now.
    pub fn known_path(&self, p: crate::game::PlayerId, id: ShipId) -> Option<Trajectory> {
        let s = self.ship(id)?;
        let v = &self.players.get(p as usize)?.vision;
        if s.owner != p && v.ships.binary_search(&id).is_err() {
            return None;
        }
        let scanned = s.owner == p || v.scanned.binary_search(&id).is_ok();
        match self.redact(s, scanned).state {
            ShipState::Flying(f) => Some(f.compute(&self.system)),
            ShipState::Parked { .. } => None,
        }
    }

    /// The path of ship `id` in a fogged view (`view_for`, as the client holds it: what's in it is
    /// what the player knows; vision itself isn't sent).
    pub fn view_path(&self, id: ShipId) -> Option<Trajectory> {
        match &self.ship(id)?.state {
            ShipState::Flying(f) => Some(f.compute(&self.system)),
            ShipState::Parked { .. } => None,
        }
    }

    /// Plan an intercept of `target` (whose known path is `tgt`) for `ship` and its fleet: the
    /// fleet's thirstiest ship sets the Δv budget.
    pub fn intercept_options(&self, ship: ShipId, target: ShipId, tgt: &Trajectory, now: f64, matched: bool) -> Vec<InterceptPlan> {
        let Some(s) = self.ship(ship) else { return vec![] };
        let budget = self.with_fleetmates(ship).iter().filter_map(|&id| self.ship(id)).map(|m| m.dv_left()).fold(s.dv_left(), f64::min);
        options(&self.system, &Chaser::of(&self.system, s), tgt, target, now, budget, matched)
    }
}

impl Game {
    /// What player `p` knows of ship `id` (None when out of sight).
    fn known_ship(&self, p: crate::game::PlayerId, id: ShipId) -> Option<Ship> {
        let s = self.ship(id)?;
        let v = &self.players.get(p as usize)?.vision;
        if s.owner != p && v.ships.binary_search(&id).is_err() {
            return None;
        }
        Some(self.redact(s, s.owner == p || v.scanned.binary_search(&id).is_ok()))
    }

    /// `Command::Intercept`: re-plan the course from its hint on what `player` knows, check fuel,
    /// and send the ships (whole fleets) on it.
    pub(crate) fn order_intercept(&mut self, player: crate::game::PlayerId, ships: &[ShipId], hint: Hint) -> Result<(), String> {
        let ships = self.expand_fleets(ships);
        let lead = *ships.first().ok_or("no ships")?;
        for &id in &ships {
            if self.ship(id).ok_or("no such ship")?.owner != player {
                return Err("not your ship".into());
            }
        }
        let t = self.ship(hint.target).ok_or("that ship is gone")?;
        if t.owner == player {
            return Err("that's one of ours".into());
        }
        let known = self.known_ship(player, hint.target).ok_or("target is out of sensor range")?;
        if let Some(b) = known.parked_at() {
            return Err(format!("target is in orbit of {}: plan a transfer there", self.system.body(b).name));
        }
        // one course for everyone: all parked at the same body, or one fleet in flight
        let s0 = self.ship(lead).unwrap();
        let mates = self.with_fleetmates(lead);
        let together = ships.iter().all(|&id| {
            let s = self.ship(id).unwrap();
            match (s0.parked_at(), s.parked_at()) {
                (Some(a), Some(b)) => a == b,
                (None, None) => mates.contains(&id),
                _ => false,
            }
        });
        if !together {
            return Err("ships must be in the same orbit or flying as one fleet".into());
        }
        // at high game speeds the planned departure may already have slipped by: burn right away
        let hint = Hint { depart_t: hint.depart_t.max(self.t + 30.0), ..hint };
        let tgt = self.known_path(player, hint.target).ok_or("target is out of sensor range")?;
        let plan = plan_hinted(&self.system, &Chaser::of(&self.system, s0), &tgt, hint)?;
        if !plan.converged {
            return Err("couldn't find that intercept any more, plan again".into());
        }
        self.fleet_fuel_check(&ships, plan.depart_dv, plan.match_dv)?;
        let sig = course_sig(&known);
        self.fly_intercept(&ships, &plan, sig);
        Ok(())
    }

    fn fly_intercept(&mut self, ships: &[ShipId], plan: &InterceptPlan, known: u64) {
        let order = Order { target: plan.hint.target, matched: plan.hint.matched, arrive_t: plan.arrive_t(), known, lost: false };
        for &id in ships {
            let Some(s) = self.ship_mut(id) else { continue };
            s.route.clear();
            s.intercept = Some(order);
            match &mut s.state {
                ShipState::Parked { body, departure } => {
                    *departure = Some(crate::game::Departure { to: *body, state: plan.depart, dv: plan.depart_dv, burns: plan.burns() });
                }
                ShipState::Flying(f) => {
                    f.burns = plan.depart_burn.into_iter().chain(plan.match_burn).collect();
                    f.dest = None;
                    f.cache = None;
                }
            }
        }
    }

    fn clear_intercept(&mut self, ships: &[ShipId]) {
        for &id in ships {
            if let Some(s) = self.ship_mut(id) {
                s.intercept = None;
            }
        }
    }

    /// Every tick: follow up on intercept orders. A target that changed course (burned, parked)
    /// while still in sight gets a fresh course; out of sight the chaser coasts on.
    pub(crate) fn intercept_tick(&mut self) {
        use crate::game::EventKind;
        let mut seen: Vec<ShipId> = vec![];
        let leads: Vec<ShipId> = self.ships.iter().filter(|s| s.intercept.is_some()).map(|s| s.id).collect();
        for lead in leads {
            if seen.contains(&lead) {
                continue;
            }
            let group: Vec<ShipId> = self.with_fleetmates(lead).into_iter().filter(|&id| self.ship(id).is_some_and(|s| s.intercept.is_some())).collect();
            seen.extend(&group);
            let Some(s) = self.ship(lead) else { continue };
            let (owner, name, o) = (s.owner, s.name.clone(), s.intercept.unwrap());
            let set = |g: &mut Game, o: Order| {
                for &id in &group {
                    if let Some(s) = g.ship_mut(id) {
                        s.intercept = Some(o);
                    }
                }
            };
            if self.ship(o.target).is_none() {
                self.clear_intercept(&group);
                self.event(Some(owner), EventKind::Info, "Target gone", &format!("{name}'s target is no longer out there"), None, Some(lead));
                continue;
            }
            let Some(known) = self.known_ship(owner, o.target) else {
                if !o.lost {
                    set(self, Order { lost: true, ..o });
                    self.event(Some(owner), EventKind::Info, "Contact lost", &format!("{name} lost sight of its target and coasts on"), None, Some(lead));
                }
                continue;
            };
            let sig = course_sig(&known);
            if sig == o.known {
                if o.lost {
                    set(self, Order { lost: false, ..o });
                }
                continue;
            }
            let o = Order { known: sig, lost: false, ..o };
            set(self, o);
            let tname = known.class.stats().name;
            if let Some(b) = known.parked_at() {
                let bname = self.system.body(b).name.clone();
                let chaser_parked = self.ship(lead).unwrap().parked_at().is_some();
                if chaser_parked {
                    // head for its orbit instead
                    self.clear_intercept(&group);
                    match self.transfer(owner, &group, b, None, None) {
                        Ok(()) => self.event(Some(owner), EventKind::Info, "Target in orbit", &format!("The {tname} parked at {bname}; {name} will follow it there"), Some(b), Some(lead)),
                        Err(e) => self.event(Some(owner), EventKind::Bad, "Target in orbit", &format!("The {tname} parked at {bname}; {name} can't follow ({e})"), Some(b), Some(lead)),
                    }
                } else {
                    // a chaser whose path enters that SOI brakes into orbit there too
                    let sys = &self.system;
                    let enters = match &self.ship(lead).unwrap().state {
                        ShipState::Flying(f) => {
                            let tr = predict(sys, f.start, &f.burns, None, f.start.t + Flight::HORIZON);
                            tr.segs.iter().any(|sg| sg.parent == b && sg.t1 > self.t)
                        }
                        _ => false,
                    };
                    for &id in &group {
                        if let Some(ShipState::Flying(f)) = self.ship_mut(id).map(|s| &mut s.state) {
                            if enters {
                                f.dest = Some(b);
                                f.cache = None;
                            }
                        }
                    }
                    self.clear_intercept(&group);
                    let msg = if enters { format!("The {tname} parked at {bname}; {name} will brake into orbit there") } else { format!("The {tname} parked at {bname}; plan a transfer once {name} arrives somewhere") };
                    self.event(Some(owner), EventKind::Info, "Target in orbit", &msg, Some(b), Some(lead));
                }
                continue;
            }
            // it changed course in flight: burn as soon as possible for a new meeting
            let Some(tgt) = self.known_path(owner, o.target) else { continue };
            let s = self.ship(lead).unwrap();
            let ch = Chaser::of(&self.system, s);
            let budget = group.iter().filter_map(|&id| self.ship(id)).map(|m| m.dv_left()).fold(f64::INFINITY, f64::min);
            let td = self.t + LEAD;
            let mut best: Option<(f64, f64)> = None;
            for (k, ta) in replan_times(self.t, o.arrive_t, reach_end(&tgt)).into_iter().enumerate() {
                let Some(c) = est_cost(&self.system, &ch, &tgt, td, ta, o.matched) else { continue };
                // keep the old meeting time if it's affordable, else the cheapest
                if c <= budget * 0.9 && (k == 0 && ta == o.arrive_t || best.is_none_or(|b| c < b.1)) {
                    best = Some((ta, c));
                    if ta == o.arrive_t {
                        break;
                    }
                }
            }
            let plan = best.and_then(|(ta, _)| plan_hinted(&self.system, &ch, &tgt, Hint { target: o.target, depart_t: td, arrive_t: ta, matched: o.matched }).ok());
            match plan.filter(|p| p.converged) {
                Some(p) if self.fleet_fuel_check(&group, p.depart_dv, p.match_dv).is_ok() => {
                    let hrs = (p.arrive_t() - self.t) / 3600.0;
                    self.fly_intercept(&group, &p, sig);
                    self.event(Some(owner), EventKind::Info, "Intercept re-plotted", &format!("The {tname} changed course; {name} burns {:.1} km/s to meet it in {hrs:.1} h", p.total_dv()), None, Some(lead));
                }
                _ => {
                    self.event(Some(owner), EventKind::Bad, "Can't catch it", &format!("The {tname} changed course and {name} can't reach it now"), None, Some(lead));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Command, Settings, ShipClass};
    use crate::system::{BodyKind, SystemSize};

    /// A two-player game with an enemy gunship already flying between two planets.
    pub(crate) fn chase_game() -> (Game, ShipId, ShipId) {
        chase_game_with(ShipClass::Interceptor, ShipClass::Gunship)
    }

    pub(crate) fn chase_game_with(mine: ShipClass, theirs: ShipClass) -> (Game, ShipId, ShipId) {
        let mut g = Game::new("T".into(), Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0 }, 7, "a");
        g.add_player("b", None).unwrap();
        g.start();
        let planets: Vec<BodyId> = g.system.bodies.iter().filter(|b| matches!(b.kind, BodyKind::Planet)).map(|b| b.id).collect();
        let (a, b) = (planets[1], planets[2]);
        let me = g.spawn(0, mine, a);
        let foe = g.spawn(1, theirs, b);
        g.apply(1, Command::Transfer { ships: vec![foe], to: a, depart_t: None }).unwrap();
        let dep = match &g.ship(foe).unwrap().state {
            ShipState::Parked { departure: Some(d), .. } => d.state.t,
            _ => panic!("no departure"),
        };
        g.advance_to(dep + 3600.0);
        // make sure we see it (the test is about planning, not sensors)
        g.update_intel();
        let v = &mut g.players[0].vision;
        if v.ships.binary_search(&foe).is_err() {
            v.ships.push(foe);
            v.ships.sort();
        }
        (g, me, foe)
    }

    #[test]
    fn intercept_ladder_meets_the_target_and_replays() {
        let (g, me, foe) = chase_game();
        let t0 = std::time::Instant::now();
        let opts = g.intercept_options(me, foe, &g.known_path(0, foe).unwrap(), g.t, true);
        println!("{} options in {:?}", opts.len(), t0.elapsed());
        for o in &opts {
            println!("  dv {:.2}+{:.2} km/s, meet in {:.1} h, miss {:.0} km, closing {:.2} km/s", o.depart_dv, o.match_dv, (o.arrive_t() - g.t) / 3600.0, o.miss, o.closing);
        }
        assert!(opts.len() >= 2, "only {} options", opts.len());
        for w in opts.windows(2) {
            assert!(w[1].total_dv() > w[0].total_dv() && w[1].arrive_t() < w[0].arrive_t());
        }
        // the chaser really is next to the target at the meeting time, with the target's velocity
        let tgt = g.known_path(0, foe).unwrap();
        for o in &opts {
            let (pc, vc) = abs_state(&g.system, &o.traj, o.arrive_t() + 1.0).unwrap();
            let (pt, vt) = abs_state(&g.system, &tgt, o.arrive_t() + 1.0).unwrap();
            assert!(pc.dist(pt) < HIT_KM, "miss {}", pc.dist(pt));
            assert!((vc - vt).len() < 0.05, "rel speed {}", (vc - vt).len());
        }
        // re-planning from the hint gives the same course bit for bit
        let o = &opts[0];
        let again = plan_hinted(&g.system, &Chaser::of(&g.system, g.ship(me).unwrap()), &tgt, o.hint).unwrap();
        assert_eq!(again.depart.v.x.to_bits(), o.depart.v.x.to_bits());
        assert_eq!(again.miss.to_bits(), o.miss.to_bits());
    }

    fn abs_ship(g: &Game, id: ShipId) -> (V2, V2) {
        match &g.ship(id).unwrap().state {
            ShipState::Flying(f) => abs_state(&g.system, &f.compute(&g.system), g.t).unwrap(),
            ShipState::Parked { body, .. } => (g.system.pos(*body, g.t), g.system.vel(*body, g.t)),
        }
    }

    fn see(g: &mut Game, id: ShipId) {
        let v = &mut g.players[0].vision;
        if v.ships.binary_search(&id).is_err() {
            v.ships.push(id);
            v.ships.sort();
        }
    }

    #[test]
    fn ordered_intercept_flies_to_the_target_and_follows_a_course_change() {
        // unarmed on both sides, so nobody dies before the course change
        let (mut g, me, foe) = chase_game_with(ShipClass::Scout, ShipClass::Tanker);
        let opts = g.intercept_options(me, foe, &g.known_path(0, foe).unwrap(), g.t, true);
        let o = opts.last().expect("an option").clone();
        let h = o.hint;
        g.apply(0, Command::Intercept { ships: vec![me], target: foe, depart_t: h.depart_t, arrive_t: h.arrive_t, matched: true }).unwrap();
        assert!(g.ship(me).unwrap().intercept.is_some());
        // fly to the meeting point (keep the foe visible: this test is about the order, not sensors)
        while g.t < h.arrive_t + 120.0 {
            let t = (g.t + 600.0).min(h.arrive_t + 120.0);
            g.advance_to(t);
            see(&mut g, foe);
        }
        let ((pc, vc), (pt, vt)) = (abs_ship(&g, me), abs_ship(&g, foe));
        println!("after meeting: {:.0} km apart, {:.3} km/s relative", pc.dist(pt), (vc - vt).len());
        assert!(pc.dist(pt) < HIT_KM * 2.0, "{} km apart", pc.dist(pt));
        assert!((vc - vt).len() < 0.2, "{} km/s apart", (vc - vt).len());
        // the foe burns: the chaser re-plans on its own
        let st = match &g.ship(foe).unwrap().state {
            ShipState::Flying(f) => f.compute(&g.system).state_at(&g.system, g.t + 300.0).unwrap(),
            _ => panic!("foe parked"),
        };
        let burn = Burn { t: st.t, prograde: 1.5, radial: 0.5 };
        g.apply(1, Command::SetBurns { ship: foe, burns: vec![burn] }).unwrap();
        let before = g.events.len();
        let t1 = g.t + 1200.0;
        while g.t < t1 {
            let t = (g.t + 60.0).min(t1);
            g.advance_to(t);
            see(&mut g, foe);
        }
        let titles: Vec<&str> = g.events[before..].iter().map(|e| e.title.as_str()).collect();
        println!("events: {titles:?}");
        assert!(titles.contains(&"Intercept re-plotted"), "{titles:?}");
        let o2 = g.ship(me).unwrap().intercept.unwrap();
        while g.t < o2.arrive_t + 120.0 {
            let t = (g.t + 600.0).min(o2.arrive_t + 120.0);
            g.advance_to(t);
            see(&mut g, foe);
        }
        let ((pc, _), (pt, _)) = (abs_ship(&g, me), abs_ship(&g, foe));
        println!("after re-plan: {:.0} km apart", pc.dist(pt));
        assert!(pc.dist(pt) < HIT_KM * 2.0);
    }
}

#[cfg(test)]
mod view_tests {
    use super::tests::chase_game;
    #[test]
    fn plans_on_the_fogged_view() {
        let (g, me, foe) = chase_game();
        let v = g.view_for(0);
        println!("in view: {}, vision has it: {}", v.ship(foe).is_some(), v.players[0].vision.ships.binary_search(&foe).is_ok());
        let p = v.known_path(0, foe);
        println!("known path: {:?}", p.as_ref().map(|t| t.end));
        let o = v.intercept_options(me, foe, &v.view_path(foe).unwrap(), v.t, true);
        println!("{} options", o.len());
        assert!(!o.is_empty());
    }
}

#[cfg(test)]
mod saved_tests {
    use super::*;
    /// INTERCEPT_GAME=path cargo test -p sim --release saved_game_front -- --ignored --nocapture
    #[test]
    #[ignore]
    fn saved_game_front() {
        let Ok(path) = std::env::var("INTERCEPT_GAME") else { return };
        let mut g: Game = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let t = g.t + 60.0;
        g.advance_to(t);
        let v = g.view_for(0);
        let me = v.ships.iter().find(|s| s.owner == 0 && s.class == crate::game::ShipClass::Bomber).unwrap().id;
        let foe = v.ships.iter().find(|s| s.owner == 1 && matches!(s.state, ShipState::Flying(_))).unwrap().id;
        let tgt = v.known_path(0, foe).unwrap();
        println!("target path end {:?} (+{:.1} h)", tgt.end, (tgt.end.t() - v.t) / 3600.0);
        let ch = Chaser::of(&v.system, v.ship(me).unwrap());
        for matched in [true, false] {
            let end = reach_end(&tgt).min(v.t + 4.0 * 86_400.0);
            let first = v.t + LEAD + MIN_TOF;
            for i in 0..40 {
                let ta = first + (end - first) * ((1.0 + 99.0 * i as f64 / 39.0).ln() / 100f64.ln());
                if let Some((td, c)) = best_departure(&v.system, &ch, &tgt, v.t, ta, matched) {
                    let t0 = std::time::Instant::now();
                    let p = plan_hinted(&v.system, &ch, &tgt, Hint { target: foe, depart_t: td, arrive_t: ta, matched });
                    let r = p.map(|p| format!("{:.2} miss {:.0}", p.total_dv(), p.miss)).unwrap_or_else(|e| e);
                    println!("m={matched} ta +{:.1}h td +{:.1}h est {c:.2} -> {r} ({:?})", (ta - v.t) / 3600.0, (td - v.t) / 3600.0, t0.elapsed());
                }
            }
        }
    }
}

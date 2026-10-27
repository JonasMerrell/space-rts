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
        if pts[0].1 < tol * 0.1 || (pts[n].1 - pts[0].1).abs() < tol * 0.01 {
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
                State { r, v: est.v.rotate(p[0]) * (1.0 + p[1]) + r.norm() * p[2], ..est }
            }
            // nudge the burn vector
            Chaser::Flying(_) => State { v: est.v + V2 { x: p[0], y: p[1] } * base_v, ..est },
        }
    };
    let miss = |st: &State| -> f64 {
        let tr = predict(sys, *st, &[], None, horizon);
        match (tr.end, abs_state(sys, &tr, ta)) {
            (End::Horizon { .. }, Some((pc, _))) => pc.dist(pt),
            // crashed or lost before the meeting
            _ => 1e12,
        }
    };
    let step: Vec<f64> = match ch {
        Chaser::Parked(_) => vec![0.02, 0.01, 0.01 * base_v],
        Chaser::Flying(_) => vec![0.01, 0.01],
    };
    let obj = |p: &[f64]| miss(&make(p));
    let (mut best, mut err) = nelder_mead(&obj, &vec![0.0; step.len()], &step, 220, 50.0);
    if err > HIT_KM {
        // retry from the best point with a finer simplex
        let fine: Vec<f64> = step.iter().map(|s| s * 0.1).collect();
        let (b2, e2) = nelder_mead(&obj, &best, &fine, 220, 50.0);
        if e2 < err {
            best = b2;
            err = e2;
        }
    }
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
    let end = reach_end(tgt).min(now + 6.0 * 86_400.0);
    let first = now + LEAD + MIN_TOF;
    if end <= first {
        return vec![];
    }
    // meeting-time grid, denser near now (log spacing), and the Pareto front of it
    let n = 40;
    let span = end - first;
    let mut front: Vec<(f64, f64, f64)> = vec![]; // (arrive, depart, est cost)
    for i in 0..n {
        let ta = first + span * ((1.0 + 99.0 * i as f64 / (n - 1) as f64).ln() / 100f64.ln());
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

    /// Plan an intercept of `target` for `ship` (and its fleet) as player `p` would: the fleet's
    /// thirstiest ship sets the Δv budget.
    pub fn intercept_options(&self, p: crate::game::PlayerId, ship: ShipId, target: ShipId, now: f64, matched: bool) -> Vec<InterceptPlan> {
        let (Some(s), Some(tgt)) = (self.ship(ship), self.known_path(p, target)) else { return vec![] };
        let budget = self.with_fleetmates(ship).iter().filter_map(|&id| self.ship(id)).map(|m| m.dv_left()).fold(s.dv_left(), f64::min);
        options(&self.system, &Chaser::of(&self.system, s), &tgt, target, now, budget, matched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Command, Settings, ShipClass};
    use crate::system::{BodyKind, SystemSize};

    /// A two-player game with an enemy gunship already flying between two planets.
    pub(crate) fn chase_game() -> (Game, ShipId, ShipId) {
        let mut g = Game::new("T".into(), Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0 }, 7, "a");
        g.add_player("b", None).unwrap();
        g.start();
        let planets: Vec<BodyId> = g.system.bodies.iter().filter(|b| matches!(b.kind, BodyKind::Planet)).map(|b| b.id).collect();
        let (a, b) = (planets[1], planets[2]);
        let me = g.spawn(0, ShipClass::Interceptor, a);
        let foe = g.spawn(1, ShipClass::Gunship, b);
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
        let opts = g.intercept_options(0, me, foe, g.t, true);
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
}

//! New course for a ship that is already flying: one burn on its current path (instead of a
//! departure from a parking orbit) onto a path that is captured at a planet or moon. Same fuel ↔
//! time slider as transfers: `options` gives the stops, `between` the courses between them, and
//! the server re-plans the picked course from its `Hint`.
use crate::game::{Game, PlayerId, ShipId, ShipState};
use crate::intercept::{coast, common, ladder, nelder_mead};
use crate::kepler::{circular_speed, lambert, oberth_dv};
use crate::system::{BodyId, System};
use crate::traj::{predict, Burn, End, State, Trajectory};
use crate::v2::V2;
use serde::{Deserialize, Serialize};

/// Everything needed to re-plan a course exactly: burn at `depart_t`, aimed (Lambert) at `to` as
/// it will be at `aim_t` (the real capture time differs a little, see `CoursePlan::arrive_t`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hint {
    pub to: BodyId,
    pub depart_t: f64,
    pub aim_t: f64,
}

#[derive(Clone, Debug)]
pub struct CoursePlan {
    pub hint: Hint,
    /// the course change, on the ship's current path
    pub burn: Burn,
    pub depart_dv: f64,
    pub capture_dv: f64,
    /// periapsis at `to`, where the capture burn happens
    pub arrive_t: f64,
    /// state right after the burn
    pub start: State,
    pub traj: Trajectory,
    pub converged: bool,
}

impl CoursePlan {
    pub fn total_dv(&self) -> f64 {
        self.depart_dv + self.capture_dv
    }
}

/// Lambert estimate from the coasting path at `td` to `to` at `ta`: post-burn state and (burn Δv,
/// capture Δv).
fn estimate(sys: &System, path: &Trajectory, to: BodyId, td: f64, ta: f64) -> Option<(State, f64, f64)> {
    let cs = path.state_at(sys, td)?;
    let tof = ta - td;
    let bt = sys.body(to);
    let inside = sys.lineage(cs.parent).contains(&to);
    // inside the target's SOI: aim at the parking orbit, else at the body (its well bends us in)
    let f = if inside { to } else { common(sys, cs.parent, bt.parent?) };
    let mu = sys.body(f).mu;
    let r1 = sys.pos(cs.parent, td) + cs.r - sys.pos(f, td);
    let vc = sys.vel(cs.parent, td) + cs.v - sys.vel(f, td);
    let r2 = if inside { (-r1).norm().rotate(-0.15) * bt.park_radius() } else { sys.pos(to, ta) - sys.pos(f, ta) };
    let (v1, v2) = lambert(mu, r1, r2, tof, true)?;
    if !inside && crate::plan::dips_below(mu, r1, v1, tof, sys.body(f).radius * 1.2) {
        return None;
    }
    let cap = if inside {
        (v2 - r2.norm().perp() * circular_speed(mu, r2.len())).len()
    } else {
        oberth_dv(bt.mu, bt.park_radius(), (v2 - sys.vel_in(to, f, ta)).len())
    };
    let dv = v1 - vc;
    Some((State { v: cs.v + dv, ..cs }, dv.len(), cap))
}

/// Plan exactly the course `hint` describes from the coasting `path`.
pub fn plan_hinted(sys: &System, path: &Trajectory, hint: Hint) -> Result<CoursePlan, String> {
    let fail = || "no course to there for those times".to_string();
    let (td, ta, to) = (hint.depart_t, hint.aim_t, hint.to);
    if !(td.is_finite() && ta.is_finite()) || ta <= td {
        return Err(fail());
    }
    let before = path.state_at(sys, td).ok_or_else(fail)?;
    let (est, est_dv, _) = estimate(sys, path, to, td, ta).ok_or_else(fail)?;
    let bt = sys.body(to);
    let horizon = ta + (ta - td) * 0.6 + 600.0;
    let scale = est.v.len().max(1e-3);
    let make = |p: &[f64]| State { v: est.v + V2 { x: p[0], y: p[1] } * scale, ..est };
    let fly = |st: State| predict(sys, st, &[], Some(to), horizon);
    let est_cost = est_dv.max(0.5);
    // like the transfer shooter: periapsis at the parking radius, cheap
    let err = |p: &[f64]| -> f64 {
        let st = make(p);
        let tr = fly(st);
        match tr.end {
            End::Capture { body, rp, vp, .. } if body == to => {
                let dv = (st.v - before.v).len() + vp - circular_speed(bt.mu, rp);
                ((rp - bt.park_radius()) / bt.park_radius()).abs() * 0.05 + 0.01 * dv / est_cost
            }
            End::Impact { body, .. } if body == to => 0.3,
            _ => 1.0 + tr.closest_approach(sys, to).0 / bt.soi,
        }
    };
    let (mut best, mut e) = nelder_mead(&err, &[0.0, 0.0], &[0.01, 0.01], 160, 1e-9);
    if e >= 0.08 {
        let (b2, e2) = nelder_mead(&err, &best, &[0.002, 0.002], 160, 1e-9);
        if e2 < e {
            best = b2;
            e = e2;
        }
    }
    let start = make(&best);
    let traj = fly(start);
    let dv = start.v - before.v;
    let (converged, capture_dv, arrive_t) = match traj.end {
        End::Capture { body, t, rp, vp } if body == to && e < 0.08 => (true, vp - circular_speed(bt.mu, rp), t),
        _ => (false, 0.0, ta),
    };
    Ok(CoursePlan { hint, burn: Burn::from_vector(td, before.r, before.v, dv), depart_dv: dv.len(), capture_dv, arrive_t, start, traj, converged })
}

/// Courses from cheapest to fastest within `max_dv` (the slider's stops). `earliest` = no burn
/// before then.
pub fn options(sys: &System, path: &Trajectory, to: BodyId, now: f64, earliest: f64, max_dv: f64) -> Vec<CoursePlan> {
    let start = earliest.max(now);
    // burns only on the known path; arrivals up to four days out
    let last_burn = match path.end {
        End::Horizon { t } => t,
        e => e.t() - 60.0,
    };
    let cost = |td: f64, ta: f64| if td <= last_burn { estimate(sys, path, to, td, ta).map(|e| e.1 + e.2) } else { None };
    let exact = |td: f64, ta: f64| {
        let p = plan_hinted(sys, path, Hint { to, depart_t: td, aim_t: ta }).ok().filter(|p| p.converged)?;
        let (dv, at) = (p.total_dv(), p.arrive_t);
        Some((p, dv, at))
    };
    ladder(start, start + 4.0 * 86_400.0, max_dv, &cost, &exact)
}

/// A course part way (`f` = 0..1) between two neighbouring `options`.
pub fn between(sys: &System, path: &Trajectory, a: Hint, b: Hint, f: f64) -> Option<CoursePlan> {
    if !(0.0..=1.0).contains(&f) || a.to != b.to {
        return None;
    }
    let l = |x: f64, y: f64| x + (y - x) * f;
    let p = plan_hinted(sys, path, Hint { to: a.to, depart_t: l(a.depart_t, b.depart_t), aim_t: l(a.aim_t, b.aim_t) }).ok()?;
    p.converged.then_some(p)
}

impl Game {
    /// The path a flying ship coasts on if its planned burns are dropped (what a new course
    /// starts from). None when parked.
    pub fn coasting_path(&self, id: ShipId) -> Option<Trajectory> {
        match &self.ship(id)?.state {
            ShipState::Flying(f) => Some(coast(&self.system, f.start)),
            ShipState::Parked { .. } => None,
        }
    }

    /// Slider stops for a new course of flying `ship` (and its fleet) to `to`.
    pub fn reroute_options(&self, ship: ShipId, to: BodyId, now: f64, earliest: f64) -> Vec<CoursePlan> {
        let (Some(s), Some(path)) = (self.ship(ship), self.coasting_path(ship)) else { return vec![] };
        let budget = self.with_fleetmates(ship).iter().filter_map(|&id| self.ship(id)).map(|m| m.dv_left()).fold(s.dv_left(), f64::min);
        options(&self.system, &path, to, now, earliest, budget)
    }

    /// `Command::Reroute`: re-plan from the hint, check fuel, and replace the flight's burns.
    pub(crate) fn reroute(&mut self, player: PlayerId, ships: &[ShipId], hint: Hint) -> Result<(), String> {
        let ships = self.expand_fleets(ships);
        let lead = *ships.first().ok_or("no ships")?;
        let mates = self.with_fleetmates(lead);
        for &id in &ships {
            let s = self.ship(id).ok_or("no such ship")?;
            if s.owner != player {
                return Err("not your ship".into());
            }
            if !matches!(s.state, ShipState::Flying(_)) {
                return Err("ship must be in flight for a new course".into());
            }
            if !mates.contains(&id) {
                return Err("ships must be flying as one fleet".into());
            }
        }
        if hint.to.0 as usize >= self.system.bodies.len() || self.system.body(hint.to).is_star() {
            return Err("no such body".into());
        }
        let path = self.coasting_path(lead).ok_or("ship must be in flight")?;
        // at high game speeds the planned burn time may have slipped by: burn right away
        let hint = Hint { depart_t: hint.depart_t.max(self.t + 30.0), ..hint };
        let p = plan_hinted(&self.system, &path, hint)?;
        if !p.converged {
            return Err("couldn't find that course any more, plan again".into());
        }
        self.fleet_fuel_check(&ships, p.depart_dv, p.capture_dv)?;
        for &id in &ships {
            let Some(s) = self.ship_mut(id) else { continue };
            s.route.clear();
            s.intercept = None;
            if let ShipState::Flying(f) = &mut s.state {
                f.burns = vec![p.burn];
                f.dest = Some(hint.to);
                f.cache = None;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Command, Settings, ShipClass};
    use crate::system::{BodyKind, SystemSize};

    #[test]
    fn flying_ship_gets_a_new_course_and_parks_there() {
        let mut g = Game::new("T".into(), Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0 }, 7, "a");
        g.start();
        let planets: Vec<BodyId> = g.system.bodies.iter().filter(|b| matches!(b.kind, BodyKind::Planet)).map(|b| b.id).collect();
        let me = g.spawn(0, ShipClass::Scout, planets[1]);
        g.apply(0, Command::Transfer { ships: vec![me], to: planets[2], depart_t: None }).unwrap();
        let dep = match &g.ship(me).unwrap().state {
            ShipState::Parked { departure: Some(d), .. } => d.state.t,
            _ => panic!("no departure"),
        };
        g.advance_to(dep + 3600.0);
        for &x in &planets {
            let o = g.reroute_options(me, x, g.t, g.t);
            println!("to {}: {} options {:?}", g.system.body(x).name, o.len(), o.iter().map(|p| (p.total_dv() as i32, ((p.arrive_t - g.t) / 3600.0) as i32)).collect::<Vec<_>>());
        }
        let to = planets[0];
        let t0 = std::time::Instant::now();
        let opts = g.reroute_options(me, to, g.t, g.t);
        println!("{} options in {:?}", opts.len(), t0.elapsed());
        for o in &opts {
            println!("  {:.2}+{:.2} km/s, burn +{:.1} h, arrive +{:.1} h", o.depart_dv, o.capture_dv, (o.hint.depart_t - g.t) / 3600.0, (o.arrive_t - g.t) / 3600.0);
        }
        assert!(!opts.is_empty());
        for w in opts.windows(2) {
            assert!(w[1].total_dv() > w[0].total_dv() && w[1].arrive_t < w[0].arrive_t);
        }
        let o = opts.last().unwrap().clone();
        g.apply(0, Command::Reroute { ships: vec![me], to, depart_t: o.hint.depart_t, aim_t: o.hint.aim_t }).unwrap();
        g.advance_to(o.arrive_t + 120.0);
        assert_eq!(g.ship(me).unwrap().parked_at(), Some(to), "{:?}", g.events.iter().map(|e| &e.body).collect::<Vec<_>>());
    }
}

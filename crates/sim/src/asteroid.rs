//! Asteroid drives and impacts. A player who controls an asteroid (owns it, or has a ship
//! parked there) can bolt a drive onto it and push it onto a new orbit. The drive is low
//! thrust, so a push takes hours, and it burns fuel from the asteroid's stock and from tankers
//! parked there. A pushed asteroid that hits a planet or moon releases ½ m v² of kinetic
//! energy: small impacts damage defenses, fleets and upgrades; big ones wipe the body clean;
//! huge ones shatter it along with everything in orbit around it and its moons.
//!
//! A pushed asteroid stays on Keplerian rails around the star (its `Body.orbit` is rewritten at
//! every push), so `System::pos/vel` keep working everywhere. Its patched-conic future is
//! predicted with `traj::predict`; inside a planet's sphere of influence that prediction
//! decides whether it hits, and after a flyby the rails are rebased onto the deflected orbit.
use crate::game::{EventKind, Game, PlayerId, ShipClass, ShipState, Upgrade};
use crate::kepler::{lambert, Conic};
use crate::system::{BodyId, BodyKind, Orbit, System};
use crate::traj::{predict, Burn, End, State, Trajectory};
use crate::v2::V2;
use serde::{Deserialize, Serialize};

/// Exhaust velocity of an asteroid drive, km/s.
pub const DRIVE_VE: f64 = 100.0;
/// Drive thrust in t·km/s² (so burn seconds = Δv · mass / thrust).
pub const DRIVE_THRUST: f64 = 4.0;
/// Time to bolt a drive on before the burn can start.
pub const INSTALL_S: f64 = 2.0 * 3600.0;
pub const DRIVE_MATTER: f64 = 200.0;
pub const DRIVE_ENERGY: f64 = 150.0;
/// Biggest push a drive can deliver in one burn, km/s.
pub const MAX_DV: f64 = 8.0;
/// Severity at which a body loses everything; at `SHATTER` it is destroyed.
pub const WIPE: f64 = 1.0;
pub const SHATTER: f64 = 4.0;
/// How far ahead a pushed asteroid's path is predicted.
const HORIZON: f64 = 20.0 * 86_400.0;

/// Game mass of an asteroid in tonnes (by radius; 30 km ≈ 10 kt).
pub fn mass(sys: &System, ast: BodyId) -> f64 {
    1.0e4 * (sys.body(ast).radius / 30.0).powi(3)
}

/// Kinetic energy of an impact, t·km²/s² (= GJ).
pub fn impact_energy(mass_t: f64, v_rel: f64) -> f64 {
    0.5 * mass_t * v_rel * v_rel
}

/// Energy (GJ) at which an impact wipes `body` (severity 1).
pub fn strength(sys: &System, body: BodyId) -> f64 {
    let b = sys.body(body);
    match b.kind {
        BodyKind::Star => f64::INFINITY,
        _ => (2.5e6 * (b.radius / 1000.0).powi(3)).max(1.0e6),
    }
}

/// Fuel (t) and burn duration (s) for pushing `ast` by `dv` km/s.
pub fn push_cost(sys: &System, ast: BodyId, dv: f64) -> (f64, f64) {
    let m = mass(sys, ast);
    (m * (1.0 - (-dv / DRIVE_VE).exp()), dv * m / DRIVE_THRUST)
}

/// A drive bolted to an asteroid, waiting for its burn.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Drive {
    pub body: BodyId,
    pub owner: PlayerId,
    /// the burn, applied as one impulse at its midpoint `burn.t`
    pub burn: Burn,
    /// burn duration, s (the drive is busy from `burn.t - burn_s/2`)
    pub burn_s: f64,
    pub target: Option<BodyId>,
}

/// A pushed asteroid on its way somewhere.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Drift {
    pub body: BodyId,
    pub owner: PlayerId,
    pub traj: Trajectory,
    /// index of the next trajectory segment whose start rebases the rails
    pub next_seg: usize,
}

impl Drift {
    /// Time of this drift's next event (rebase after a flyby, impact, or re-prediction).
    pub fn next_t(&self) -> f64 {
        self.traj.segs.get(self.next_seg).map(|s| s.t0).unwrap_or(self.traj.end.t())
    }
}

/// What the UI shows before committing to a push.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImpactPlan {
    pub body: BodyId,
    pub target: Option<BodyId>,
    pub burn: Burn,
    pub dv: f64,
    pub fuel: f64,
    pub burn_s: f64,
    /// predicted impact (None = misses everything within the prediction horizon)
    pub impact: Option<Impact>,
    pub traj: Trajectory,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Impact {
    pub body: BodyId,
    pub t: f64,
    /// speed relative to the body at impact, km/s
    pub v: f64,
    /// GJ
    pub energy: f64,
    /// energy / strength (≥ WIPE wipes, ≥ SHATTER destroys)
    pub severity: f64,
}

/// Elliptic, prograde Orbit through (r, v) at time t around a body of `mu`.
pub fn orbit_from_state(mu: f64, r: V2, v: V2, t: f64) -> Option<Orbit> {
    let c = Conic::from_state(mu, r, v);
    if !(c.e < 0.98) || c.h <= 0.0 || !c.a.is_finite() || c.a <= 0.0 {
        return None;
    }
    let w = if c.e > 1e-9 { c.evec.angle() } else { 0.0 };
    let nu = c.true_anomaly(r);
    let ea = 2.0 * (((1.0 - c.e) / (1.0 + c.e)).sqrt() * (nu / 2.0).tan()).atan();
    let m = ea - c.e * ea.sin();
    let mut o = Orbit::new(mu, c.a, c.e, w, 0.0);
    o.m0 = m - o.n * t;
    Some(o)
}

/// A copy of the system in which `ast` is nobody's child (so its own SOI isn't in the way).
fn detached(sys: &System, ast: BodyId) -> System {
    let mut s = sys.clone();
    s.bodies[ast.0 as usize].parent = None;
    s
}

/// Predict where `ast` goes after being given `dv` (absolute vector) at time t.
fn path_after(sys: &System, ast: BodyId, t: f64, dv: V2) -> Trajectory {
    let star = sys.star().id;
    let r = sys.pos(ast, t);
    let v = sys.vel(ast, t) + dv;
    predict(&detached(sys, ast), State { parent: star, t, r, v }, &[], None, t + HORIZON)
}

fn impact_of(sys: &System, ast: BodyId, tr: &Trajectory) -> Option<Impact> {
    let End::Impact { body, t } = tr.end else { return None };
    let st = tr.state_at(sys, t)?;
    let v = st.v.len();
    let energy = impact_energy(mass(sys, ast), v);
    Some(Impact { body, t, v, energy, severity: energy / strength(sys, body) })
}

/// Fuel the player can spend on a push of `ast`: the asteroid's stock (if theirs) + their tankers there.
pub fn fuel_available(g: &Game, p: PlayerId, ast: BodyId) -> f64 {
    let bs = &g.bodies[ast.0 as usize];
    let stock = if bs.owner == Some(p) { bs.fuel } else { 0.0 };
    stock + g.ships.iter().filter(|s| s.owner == p && s.class == ShipClass::Tanker && s.parked_at() == Some(ast)).map(|s| s.fuel).sum::<f64>()
}

/// Earliest time a burn of `burn_s` seconds can be centred on.
fn earliest(now: f64, burn_s: f64) -> f64 {
    now + INSTALL_S + burn_s / 2.0
}

/// Preview a manual push.
pub fn plan_push(g: &Game, ast: BodyId, burn: Burn) -> ImpactPlan {
    let sys = &g.system;
    let (r, v) = (sys.pos(ast, burn.t), sys.vel(ast, burn.t));
    let dvv = burn.vector(r, v);
    let (fuel, burn_s) = push_cost(sys, ast, burn.dv());
    let traj = path_after(sys, ast, burn.t, dvv);
    let impact = impact_of(sys, ast, &traj);
    ImpactPlan { body: ast, target: impact.map(|i| i.body), burn, dv: burn.dv(), fuel, burn_s, impact, traj }
}

/// Find the cheapest push that crashes `ast` into `target`, burning no earlier than possible
/// from `now`. Scans departure × flight time with Lambert, then checks each candidate with the
/// patched-conic predictor. None if nothing within `MAX_DV` hits.
pub fn plan_impact(g: &Game, ast: BodyId, target: BodyId, now: f64) -> Option<ImpactPlan> {
    let sys = &g.system;
    if sys.body(ast).kind != BodyKind::Asteroid || target == ast || sys.body(target).is_star() {
        return None;
    }
    let mu = sys.star().mu;
    let t_min = earliest(now, push_cost(sys, ast, 0.5).1);
    let mut cands: Vec<(f64, f64, V2)> = vec![];
    for i in 0..36 {
        let t = t_min + i as f64 * 7200.0;
        let (r1, va) = (sys.pos(ast, t), sys.vel(ast, t));
        for j in 0..20 {
            let tof = 0.4 * 86_400.0 * 1.25f64.powi(j);
            let r2 = sys.pos(target, t + tof);
            let Some((v1, _)) = lambert(mu, r1, r2, tof, true) else { continue };
            let dv = v1 - va;
            let d = dv.len();
            if d.is_finite() && d <= MAX_DV && t >= earliest(now, push_cost(sys, ast, d).1) {
                cands.push((d, t, dv));
            }
        }
    }
    cands.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    for &(d, t, dv) in cands.iter().take(12) {
        let traj = path_after(sys, ast, t, dv);
        let Some(imp) = impact_of(sys, ast, &traj) else { continue };
        if imp.body != target && sys.body(imp.body).parent != Some(target) {
            continue;
        }
        let burn = Burn::from_vector(t, sys.pos(ast, t), sys.vel(ast, t), dv);
        let (fuel, burn_s) = push_cost(sys, ast, d);
        return Some(ImpactPlan { body: ast, target: Some(target), burn, dv: d, fuel, burn_s, impact: Some(imp), traj });
    }
    None
}

impl Game {
    /// Whether `p` may bolt drives onto asteroids. The research thread gates this on
    /// `Tech::AsteroidDrives`.
    pub fn can_drive_asteroids(&self, _p: PlayerId) -> bool {
        true
    }

    /// Validate and schedule a push (from `Command::PushAsteroid` / `AimAsteroid`).
    pub(crate) fn order_push(&mut self, p: PlayerId, ast: BodyId, plan: Result<ImpactPlan, String>) -> Result<(), String> {
        if ast.0 as usize >= self.bodies.len() || self.system.body(ast).kind != BodyKind::Asteroid {
            return Err("not an asteroid".into());
        }
        if self.bodies[ast.0 as usize].gone {
            return Err("that asteroid is gone".into());
        }
        if !self.can_drive_asteroids(p) {
            return Err("needs Asteroid Drives research".into());
        }
        let here = self.ships.iter().any(|s| s.owner == p && s.parked_at() == Some(ast));
        if self.bodies[ast.0 as usize].owner != Some(p) && !here {
            return Err("you need to own the asteroid or have a ship parked there".into());
        }
        if self.drives.iter().any(|d| d.body == ast) {
            return Err("a drive is already burning on that asteroid".into());
        }
        let plan = plan?;
        if !(plan.dv > 0.0 && plan.dv <= MAX_DV) {
            return Err(format!("a drive can push at most {MAX_DV} km/s"));
        }
        if plan.burn.t < earliest(self.t, plan.burn_s) - 1.0 {
            return Err("too soon: the drive needs 2 h to install plus half the burn".into());
        }
        if fuel_available(self, p, ast) < plan.fuel {
            return Err(format!("needs {:.0} t of fuel at the asteroid", plan.fuel));
        }
        self.pay(p, DRIVE_MATTER, DRIVE_ENERGY)?;
        self.drives.push(Drive { body: ast, owner: p, burn: plan.burn, burn_s: plan.burn_s, target: plan.target });
        Ok(())
    }

    /// Drive burns, flyby rebases and impacts up to `until`, in time order.
    pub(crate) fn asteroid_events(&mut self, until: f64) {
        loop {
            let d = self.drives.iter().enumerate().map(|(i, d)| (d.burn.t, 0, i)).filter(|e| e.0 <= until);
            let f = self.drifts.iter().enumerate().map(|(i, d)| (d.next_t(), 1, i)).filter(|e| e.0 <= until);
            let Some((t, kind, i)) = d.chain(f).min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1))) else { return };
            if kind == 0 {
                let d = self.drives.remove(i);
                self.burn_drive(d);
            } else {
                self.drift_event(i, t);
            }
        }
    }

    fn burn_drive(&mut self, d: Drive) {
        let sys = &self.system;
        let ast = d.body;
        let name = sys.body(ast).name.clone();
        let (r, v) = (sys.pos(ast, d.burn.t), sys.vel(ast, d.burn.t));
        // pay fuel: asteroid stock first, then tankers parked there; short → partial burn
        let (need, _) = push_cost(sys, ast, d.burn.dv());
        let have = fuel_available(self, d.owner, ast);
        let k = if need > have { (DRIVE_VE * (mass(sys, ast) / (mass(sys, ast) - have)).ln() / d.burn.dv()).min(1.0) } else { 1.0 };
        let mut owed = need.min(have);
        if self.bodies[ast.0 as usize].owner == Some(d.owner) {
            let x = owed.min(self.bodies[ast.0 as usize].fuel);
            self.bodies[ast.0 as usize].fuel -= x;
            owed -= x;
        }
        for s in self.ships.iter_mut().filter(|s| s.owner == d.owner && s.class == ShipClass::Tanker && s.parked_at() == Some(ast)) {
            let x = owed.min(s.fuel);
            s.fuel -= x;
            owed -= x;
        }
        let dv = d.burn.vector(r, v) * k;
        if k < 1.0 {
            self.event(Some(d.owner), EventKind::Bad, "Drive ran dry", &format!("{name} got only {:.0}% of its push", k * 100.0), Some(ast), None);
        }
        self.drifts.retain(|x| x.body != ast);
        let traj = path_after(&self.system, ast, d.burn.t, dv);
        self.set_rails(ast, State { parent: self.system.star().id, t: d.burn.t, r, v: v + dv });
        let tgt = impact_of(&self.system, ast, &traj).map(|i| format!(" — on course for {}", self.system.body(i.body).name)).unwrap_or_default();
        self.event(Some(d.owner), EventKind::Info, "Asteroid pushed", &format!("{name} burned {:.2} km/s{tgt}", dv.len()), Some(ast), None);
        self.drifts.push(Drift { body: ast, owner: d.owner, traj, next_seg: 1 });
    }

    /// Put `ast` on heliocentric rails through `st`, and reset anything planned around it.
    fn set_rails(&mut self, ast: BodyId, st: State) {
        let mu = self.system.body(st.parent).mu;
        let star = self.system.star().id;
        if st.parent == star {
            if let Some(o) = orbit_from_state(mu, st.r, st.v, st.t) {
                self.system.bodies[ast.0 as usize].orbit = Some(o);
            }
        }
        let name = self.system.body(ast).name.clone();
        let mut reset = vec![];
        for s in self.ships.iter_mut() {
            match &mut s.state {
                ShipState::Parked { body, departure } if *body == ast && departure.is_some() => {
                    *departure = None;
                    s.route.clear();
                    reset.push((s.owner, s.id));
                }
                ShipState::Flying(f) => {
                    if f.dest == Some(ast) || f.cache.as_ref().is_some_and(|tr| tr.segs.iter().any(|sg| sg.parent == ast)) {
                        f.cache = None;
                    }
                }
                _ => {}
            }
        }
        for (o, id) in reset {
            self.event(Some(o), EventKind::Bad, "Departure cancelled", &format!("{name} changed course; replan"), Some(ast), Some(id));
        }
    }

    fn drift_event(&mut self, i: usize, t: f64) {
        let d = &mut self.drifts[i];
        let ast = d.body;
        if let Some(seg) = d.traj.segs.get(d.next_seg).copied() {
            d.next_seg += 1;
            // back in the star's frame after a flyby: rebase the rails onto the bent orbit
            if seg.parent == self.system.star().id {
                self.set_rails(ast, State { parent: seg.parent, t: seg.t0, r: seg.r0, v: seg.v0 });
            }
            return;
        }
        let d = self.drifts.remove(i);
        match d.traj.end {
            End::Impact { .. } => {
                if let Some(imp) = impact_of(&self.system, ast, &d.traj) {
                    self.impact(ast, d.owner, imp);
                }
            }
            _ => {
                // still coasting: keep predicting from the current rails
                let star = self.system.star().id;
                let r = self.system.pos(ast, t);
                let v = self.system.vel(ast, t);
                let traj = predict(&detached(&self.system, ast), State { parent: star, t, r, v }, &[], None, t + HORIZON);
                if matches!(traj.end, End::Impact { .. }) || traj.segs.len() > 1 {
                    self.drifts.push(Drift { body: ast, owner: d.owner, traj, next_seg: 1 });
                }
            }
        }
    }

    /// Resolve an asteroid hitting a body.
    pub(crate) fn impact(&mut self, ast: BodyId, owner: PlayerId, imp: Impact) {
        let aname = self.system.body(ast).name.clone();
        let tname = self.system.body(imp.body).name.clone();
        // the asteroid and whatever was parked on it are gone
        self.bodies[ast.0 as usize] = Default::default();
        self.bodies[ast.0 as usize].gone = true;
        let b = &mut self.system.bodies[ast.0 as usize];
        b.parent = None;
        b.orbit = None;
        b.soi = 0.0;
        self.drives.retain(|d| d.body != ast);
        let s = imp.severity;
        let mut doomed: Vec<BodyId> = vec![ast];
        let target = imp.body;
        let bi = target.0 as usize;
        if !self.system.body(target).is_star() {
            let bs = &mut self.bodies[bi];
            if s >= WIPE {
                let shattered = bs.shattered || s >= SHATTER;
                *bs = Default::default();
                bs.shattered = shattered;
                doomed.push(target);
                if shattered {
                    doomed.extend(self.system.children(target).map(|c| c.id));
                }
            } else {
                bs.def_hp *= 1.0 - s;
                for u in Upgrade::ALL {
                    let l = bs.levels[u.index()];
                    bs.levels[u.index()] = l - (l as f64 * s).round() as u8;
                }
                for sh in self.ships.iter_mut().filter(|sh| sh.parked_at() == Some(target)) {
                    sh.hp -= s * sh.class.stats().hp;
                }
            }
        }
        let lost: Vec<(PlayerId, String, BodyId)> =
            self.ships.iter().filter_map(|sh| sh.parked_at().filter(|b| doomed.contains(b) || sh.hp <= 0.0).map(|b| (sh.owner, sh.name.clone(), b))).collect();
        self.ships.retain(|sh| !sh.parked_at().is_some_and(|b| doomed.contains(&b)) && sh.hp > 0.0);
        for (o, n, b) in lost {
            let bn = self.system.body(b).name.clone();
            self.event(Some(o), EventKind::Battle, "Ship destroyed", &format!("{n} was destroyed by the impact at {bn}"), Some(b), None);
        }
        let pj = imp.energy / 1e6;
        let (title, what) = if self.bodies[bi].shattered && s >= SHATTER {
            ("Planet shattered", format!("{aname} hit {tname} at {:.1} km/s ({pj:.1} PJ) and shattered it", imp.v))
        } else if s >= WIPE {
            ("Impact", format!("{aname} hit {tname} at {:.1} km/s ({pj:.1} PJ) and wiped it clean", imp.v))
        } else {
            ("Impact", format!("{aname} hit {tname} at {:.1} km/s ({pj:.1} PJ): {:.0}% damage", imp.v, s * 100.0))
        };
        let who = self.players.get(owner as usize).map(|p| p.name.clone()).unwrap_or_default();
        self.event(None, EventKind::Battle, title, &format!("{what} (pushed by {who})"), Some(target), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Command, Settings};
    use crate::system::SystemSize;

    #[test]
    fn orbit_from_state_round_trips() {
        let sys = crate::system::generate(5, SystemSize::Medium);
        for b in sys.bodies.iter().filter(|b| b.kind == BodyKind::Asteroid) {
            let t = 12_345.0;
            let (r, v) = b.orbit.unwrap().state(t);
            let o = orbit_from_state(sys.star().mu, r, v, t).unwrap();
            let (r2, v2) = o.state(t);
            assert!(r.dist(r2) < 1e-3 * r.len() * 1e-3, "{} {:?} vs {:?}", b.name, r, r2);
            assert!(v.dist(v2) < 1e-6 * v.len());
            let (r3, _) = o.state(t + 86_400.0);
            assert!(r3.dist(b.orbit.unwrap().state(t + 86_400.0).0) < 1.0);
        }
    }

    #[test]
    fn energy_is_half_m_v_squared() {
        assert_eq!(impact_energy(2.0, 3.0), 9.0);
        assert_eq!(impact_energy(4.0, 3.0), 2.0 * impact_energy(2.0, 3.0));
        assert_eq!(impact_energy(2.0, 6.0), 4.0 * impact_energy(2.0, 3.0));
    }

    pub(crate) fn running(seed: u64) -> Game {
        let settings = Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 30.0 * 86_400.0, speed: 1.0 };
        let mut g = Game::new("T".into(), settings, seed, "alice");
        g.add_player("bob", None).unwrap();
        g.start();
        g
    }

    #[test]
    fn aimed_asteroid_hits_its_target() {
        let mut g = running(7);
        let ast = g.system.bodies.iter().find(|b| b.kind == BodyKind::Asteroid).unwrap().id;
        let home1 = g.players[1].home.unwrap();
        g.bodies[ast.0 as usize].owner = Some(0);
        g.bodies[ast.0 as usize].fuel = 1e6;
        g.players[0].matter = 1e4;
        g.players[0].energy = 1e4;
        let plan = plan_impact(&g, ast, home1, g.t).expect("no impact course found");
        assert_eq!(plan.impact.unwrap().body, home1);
        g.apply(0, Command::AimAsteroid { body: ast, target: home1 }).unwrap();
        let t_hit = plan.impact.unwrap().t;
        g.advance_to(t_hit + 120.0);
        assert!(g.bodies[ast.0 as usize].gone, "asteroid still there");
        assert!(g.events.iter().any(|e| e.title == "Impact" || e.title == "Planet shattered"), "no impact event");
    }
}

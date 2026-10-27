//! Heat-sink cloaks (advanced tier, Shadow branch). A hot ship can't be hidden, but it can
//! store its waste heat for a while: a dark ship's signature drops to `DARK_SIG` until its heat
//! sink fills, then it has to vent (signature × `VENT_SIG`) for as long as it was dark.
//! Any engine burn or weapon fire ends the dark spell at once. Dark ships don't count toward
//! orbit control, can't be shot by anyone whose sensors don't see them, and hold their fire;
//! a ship that decloaks on purpose gets a first-strike bonus for `STRIKE_TIME`.
use crate::game::{EventKind, Game, PlayerId, Ship, ShipClass, ShipId};
use crate::tech::Tech;
use serde::{Deserialize, Serialize};

/// How long a heat sink lasts (× `Mods::cloak_time`).
pub const DARK_TIME: f64 = 12.0 * 3600.0;
/// Signature multiplier while dark.
/// Signature multiplier while dark: 100× fainter, so a dark gunship shows to a ship's sensors
/// only within ~2,000 km, but a planet with a telescope still sees its own orbit.
pub const DARK_SIG: f64 = 0.01;
/// Signature multiplier while venting (× `Mods::vent_sig`).
pub const VENT_SIG: f64 = 3.0;
/// First-strike window after a chosen decloak, and its damage multiplier.
pub const STRIKE_TIME: f64 = 3600.0;
pub const STRIKE_BONUS: f64 = 1.5;
/// Energy to chill a ship's heat sink, per ship.
pub const CLOAK_ENERGY: f64 = 30.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cloak {
    /// went dark at
    pub since: f64,
    /// dark until then (the heat sink fills), unless broken earlier
    pub dark_until: f64,
    /// venting until
    pub vent_until: f64,
    /// first-strike bonus until (after a chosen decloak)
    #[serde(default)]
    pub strike_until: f64,
}

impl Cloak {
    pub fn dark(&self, t: f64) -> bool {
        t >= self.since && t < self.dark_until
    }
    pub fn venting(&self, t: f64) -> bool {
        t >= self.dark_until && t < self.vent_until
    }
}

/// Dreadnoughts make too much heat to hide.
pub fn can_cloak(class: ShipClass) -> bool {
    !matches!(class, ShipClass::Dreadnought)
}

impl Ship {
    pub fn dark(&self, t: f64) -> bool {
        self.cloak.is_some_and(|c| c.dark(t))
    }
}

impl Game {
    /// Signature multiplier a ship's cloak applies now (`vent_sig` = the owner's research mod).
    pub fn cloak_sig(s: &Ship, t: f64, vent_sig: f64) -> f64 {
        match s.cloak {
            Some(c) if c.dark(t) => DARK_SIG,
            Some(c) if c.venting(t) => VENT_SIG * vent_sig,
            _ => 1.0,
        }
    }

    /// Damage multiplier from a first strike out of the dark.
    pub fn strike_mult(s: &Ship, t: f64) -> f64 {
        if s.cloak.is_some_and(|c| t < c.strike_until) {
            STRIKE_BONUS
        } else {
            1.0
        }
    }

    /// Why `s` can't go dark now (None = it can).
    pub fn cloak_blocker(&self, p: PlayerId, s: &Ship) -> Option<String> {
        if !self.has_tech(p, Tech::HeatSinkCloaks) {
            return Some(format!("needs {}", Tech::HeatSinkCloaks.label()));
        }
        if !can_cloak(s.class) {
            return Some(format!("a {} runs too hot to hide", s.class.stats().name));
        }
        match s.cloak {
            Some(c) if c.dark(self.t) => Some("already dark".into()),
            Some(c) if c.venting(self.t) => Some("still venting heat".into()),
            _ => None,
        }
    }

    /// `Command::Cloak`: take ships dark, or bring them out of the dark (with a first strike).
    pub(crate) fn set_cloak(&mut self, p: PlayerId, ships: &[ShipId], on: bool) -> Result<(), String> {
        let idx: Vec<usize> = ships.iter().map(|id| self.ships.iter().position(|s| s.id == *id && s.owner == p).ok_or("not your ship")).collect::<Result<_, _>>()?;
        let t = self.t;
        if on {
            for &i in &idx {
                if let Some(why) = self.cloak_blocker(p, &self.ships[i]) {
                    return Err(format!("{}: {why}", self.ships[i].name));
                }
            }
            self.pay(p, 0.0, CLOAK_ENERGY * idx.len() as f64)?;
            let len = DARK_TIME * self.mods(p).cloak_time;
            for i in idx {
                self.ships[i].cloak = Some(Cloak { since: t, dark_until: t + len, vent_until: t + len + len / self.mods(p).cloak_time, strike_until: 0.0 });
            }
        } else {
            let k = self.mods(p).cloak_time;
            for i in idx {
                if let Some(c) = self.ships[i].cloak.as_mut().filter(|c| c.dark(t)) {
                    end_dark(c, t, k, true);
                }
            }
        }
        Ok(())
    }

    /// Burns break the dark; full heat sinks start venting (once per tick).
    pub(crate) fn cloak_tick(&mut self) {
        let t = self.t;
        let mut news: Vec<(PlayerId, String, &'static str)> = vec![];
        for pi in 0..self.players.len() {
            let k = self.mods(pi as PlayerId).cloak_time;
            for s in self.ships.iter_mut().filter(|s| s.owner == pi as PlayerId) {
                let Some(c) = s.cloak.as_mut() else { continue };
                if c.dark(t) && s.last_burn_t >= c.since && s.last_burn_t <= t {
                    end_dark(c, t, k, false);
                    news.push((s.owner, s.name.clone(), "engine burn: out of the dark"));
                } else if c.dark_until > t - crate::game::TICK && c.dark_until <= t && c.since < c.dark_until {
                    news.push((s.owner, s.name.clone(), "heat sink full, venting"));
                }
                if t >= c.vent_until && t >= c.strike_until {
                    s.cloak = None;
                }
            }
        }
        for (o, name, what) in news {
            self.event(Some(o), EventKind::Info, "Cloak", &format!("{name}: {what}"), None, None);
        }
    }

    /// A dark ship that got into a fight (fired or was fired on) is dark no longer.
    pub(crate) fn break_dark(&mut self, ship: usize) {
        let t = self.t;
        let k = self.mods(self.ships[ship].owner).cloak_time;
        if let Some(c) = self.ships[ship].cloak.as_mut().filter(|c| c.dark(t)) {
            end_dark(c, t, k, false);
        }
    }
}

/// Leave the dark at t: vent for as long as the sink was filling (research shortens it with
/// `k` = cloak_time), and optionally open the first-strike window.
fn end_dark(c: &mut Cloak, t: f64, k: f64, strike: bool) {
    let used = (t - c.since).max(0.0);
    c.dark_until = t;
    c.vent_until = t + used / k.max(1.0);
    if strike {
        c.strike_until = t + STRIKE_TIME;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Command;

    fn cloaky() -> (Game, ShipId) {
        let mut g = crate::tech::tests::game();
        g.players[0].techs.push(Tech::HeatSinkCloaks);
        let id = g.ships.iter().find(|s| s.owner == 0 && can_cloak(s.class)).unwrap().id;
        (g, id)
    }

    #[test]
    fn dark_then_vent() {
        let (mut g, id) = cloaky();
        let sig0 = g.signatures()[g.ships.iter().position(|s| s.id == id).unwrap()];
        g.apply(0, Command::Cloak { ships: vec![id], on: true }).unwrap();
        let i = g.ships.iter().position(|s| s.id == id).unwrap();
        assert!(g.signatures()[i] / sig0 <= DARK_SIG + 1e-12);
        assert!(g.apply(0, Command::Cloak { ships: vec![id], on: true }).unwrap_err().contains("already dark"));
        g.advance_to(g.t + DARK_TIME + 120.0);
        let s = g.ship(id).unwrap();
        assert!(!s.dark(g.t) && s.cloak.unwrap().venting(g.t));
        assert!(g.apply(0, Command::Cloak { ships: vec![id], on: true }).unwrap_err().contains("venting"));
        g.advance_to(g.t + DARK_TIME);
        assert!(g.ship(id).unwrap().cloak.is_none());
    }

    #[test]
    fn decloak_strikes_and_needs_research() {
        let (mut g, id) = cloaky();
        assert!(g.apply(1, Command::Cloak { ships: vec![id], on: true }).is_err());
        g.players[0].techs.clear();
        assert!(g.apply(0, Command::Cloak { ships: vec![id], on: true }).unwrap_err().contains("Heat-sink"));
        g.players[0].techs.push(Tech::HeatSinkCloaks);
        g.apply(0, Command::Cloak { ships: vec![id], on: true }).unwrap();
        g.advance_to(g.t + 2.0 * 3600.0);
        g.apply(0, Command::Cloak { ships: vec![id], on: false }).unwrap();
        let s = g.ship(id).unwrap().clone();
        assert!(!s.dark(g.t));
        assert_eq!(Game::strike_mult(&s, g.t), STRIKE_BONUS);
        // vented for as long as it was dark
        assert!((s.cloak.unwrap().vent_until - g.t - 2.0 * 3600.0).abs() < 61.0);
    }

    #[test]
    fn dark_ships_lurk_unseen_and_unshot_in_a_neutral_orbit() {
        let (mut g, _) = cloaky();
        // an empty body: alice's interceptor goes dark there, bob's gunship arrives
        let b = (0..g.bodies.len() as u32).map(crate::BodyId).find(|&b| g.bodies[b.0 as usize].owner.is_none() && !g.system.body(b).is_star()).unwrap();
        let a = g.spawn(0, ShipClass::Interceptor, b);
        g.apply(0, Command::Cloak { ships: vec![a], on: true }).unwrap();
        let e = g.spawn(1, ShipClass::Gunship, b);
        g.advance_to(g.t + 600.0);
        assert!(g.players[1].vision.ships.binary_search(&a).is_err(), "bob saw the dark ship");
        assert!(g.power_at(b).iter().all(|&(p, _)| p != 0), "dark ships don't claim orbits");
        let hp = (g.ship(a).unwrap().hp, g.ship(e).unwrap().hp);
        g.advance_to(g.t + 3600.0);
        assert_eq!((g.ship(a).unwrap().hp, g.ship(e).unwrap().hp), hp, "nobody fights while one side is dark and unseen");
        // decloak: first strike, and the fight is on
        g.apply(0, Command::Cloak { ships: vec![a], on: false }).unwrap();
        g.advance_to(g.t + 600.0);
        assert!(g.ship(e).map_or(true, |s| s.hp < hp.1));
    }
}

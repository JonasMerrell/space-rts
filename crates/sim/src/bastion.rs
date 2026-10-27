//! Interplanetary defenses (advanced tier, Bastion branch).
//!
//! - Orbital minefields: a constructor seeds `MINE_CHARGES` mines in orbit of a body. Each enemy
//!   ship that captures into that orbit sets one off (`MINE_DAMAGE`, more the harder it brakes).
//!   Mines are only visible to their owner and to scanning stations; enemy gunships parked there
//!   sweep one charge per hour each.
//! - Interdiction grid (station): shoots down incoming ordnance (ordnance.rs) and halves the
//!   severity of an asteroid impact on its body by fragmenting it.
//! - Phased laser battery (station on an owned body): a long-range beam (`Weapon::Beam`, see
//!   combat.rs) that hits ships approaching or leaving, out to 40,000 km.
use crate::game::{EventKind, Game, PlayerId, ShipClass, ShipId};
use crate::station::StationKind;
use crate::system::BodyId;
use crate::tech::Tech;
use serde::{Deserialize, Serialize};

pub const MINE_CHARGES: u32 = 10;
pub const MINE_DAMAGE: f64 = 40.0;
/// Capture burn (km/s) at which a mine hits twice as hard (capped there).
pub const MINE_DV_REF: f64 = 5.0;
pub const MINE_COST: (f64, f64) = (300.0, 200.0);
/// Charges one parked enemy gunship clears per hour.
pub const SWEEP_RATE: f64 = 1.0;
/// Asteroid impact severity multiplier at a body with an interdiction grid.
pub const GRID_IMPACT: f64 = 0.5;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Minefield {
    pub owner: PlayerId,
    pub body: BodyId,
    pub charges: u32,
    /// sweeping progress towards the next charge cleared
    #[serde(default)]
    pub swept: f64,
}

impl Game {
    /// Why `p` can't seed mines at `body` now (None = it can).
    pub fn mines_blocker(&self, p: PlayerId, body: BodyId) -> Option<String> {
        if !self.has_tech(p, Tech::Minefields) {
            return Some(format!("needs {}", Tech::Minefields.label()));
        }
        if self.system.body(body).is_star() {
            return Some("can't mine the star's orbit".into());
        }
        if self.constructors_at(p, body) == 0 {
            return Some("needs a construction ship in orbit here".into());
        }
        if self.minefields.iter().any(|m| m.owner == p && m.body == body && m.charges >= MINE_CHARGES) {
            return Some("the field here is full".into());
        }
        None
    }

    /// `Command::LayMines`: seed (or top up) a field.
    pub(crate) fn lay_mines(&mut self, p: PlayerId, body: BodyId) -> Result<(), String> {
        if let Some(why) = self.mines_blocker(p, body) {
            return Err(why);
        }
        self.pay(p, MINE_COST.0, MINE_COST.1)?;
        match self.minefields.iter_mut().find(|m| m.owner == p && m.body == body) {
            Some(m) => m.charges = MINE_CHARGES,
            None => self.minefields.push(Minefield { owner: p, body, charges: MINE_CHARGES, swept: 0.0 }),
        }
        Ok(())
    }

    /// Mine charges `p` has at `body`.
    pub fn mines_at(&self, p: PlayerId, body: BodyId) -> u32 {
        self.minefields.iter().filter(|m| m.owner == p && m.body == body).map(|m| m.charges).sum()
    }

    /// A ship just braked into orbit of `body` with a `dv` km/s capture burn: enemy mines go off.
    pub(crate) fn trip_mines(&mut self, ship: ShipId, body: BodyId, dv: f64) {
        let Some(si) = self.ships.iter().position(|s| s.id == ship) else { return };
        let owner = self.ships[si].owner;
        let mut hits = vec![];
        for m in self.minefields.iter_mut().filter(|m| m.body == body && m.owner != owner && m.charges > 0) {
            m.charges -= 1;
            hits.push(m.owner);
        }
        if hits.is_empty() {
            return;
        }
        let k = 1.0 + (dv.abs() / MINE_DV_REF).min(1.0);
        let dmg = MINE_DAMAGE * k * hits.len() as f64;
        self.ships[si].hp -= dmg;
        let name = self.ships[si].name.clone();
        let bname = self.system.body(body).name.clone();
        let dead = self.ships[si].hp <= 0.0;
        if dead {
            self.ships.remove(si);
            self.event(Some(owner), EventKind::Battle, "Ship destroyed", &format!("{name} hit a minefield at {bname}"), Some(body), None);
        } else {
            self.event(Some(owner), EventKind::Battle, "Mine hit", &format!("{name} hit a mine at {bname} ({dmg:.0} damage)"), Some(body), Some(ship));
        }
        for o in hits {
            let what = if dead { "destroyed" } else { "damaged" };
            self.event(Some(o), EventKind::Good, "Mine hit", &format!("Our mines at {bname} {what} an enemy ship"), Some(body), None);
        }
        self.minefields.retain(|m| m.charges > 0);
    }

    /// Enemy gunships parked in a field sweep it.
    pub(crate) fn mines_tick(&mut self, dt: f64) {
        if self.minefields.is_empty() {
            return;
        }
        let h = dt / 3600.0;
        for i in 0..self.minefields.len() {
            let (o, b) = (self.minefields[i].owner, self.minefields[i].body);
            let sweepers = self.ships.iter().filter(|s| s.owner != o && s.class == ShipClass::Gunship && s.parked_at() == Some(b)).count();
            let m = &mut self.minefields[i];
            m.swept += sweepers as f64 * SWEEP_RATE * h;
            while m.swept >= 1.0 && m.charges > 0 {
                m.swept -= 1.0;
                m.charges -= 1;
            }
            if sweepers == 0 {
                m.swept = 0.0;
            }
        }
        self.minefields.retain(|m| m.charges > 0);
    }

    /// Impact severity multiplier for `body` (an interdiction grid there fragments asteroids).
    pub fn impact_shield(&self, body: BodyId) -> f64 {
        if self.stations_at(body).any(|s| s.kind == StationKind::Interdiction) {
            GRID_IMPACT
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Command, Upgrade};

    #[test]
    fn mines_hit_arrivals_and_get_swept() {
        let mut g = crate::tech::tests::game();
        let home = g.players[0].home.unwrap();
        assert!(g.apply(0, Command::LayMines { body: home }).unwrap_err().contains("Orbital"));
        g.players[0].techs.push(Tech::Minefields);
        assert!(g.apply(0, Command::LayMines { body: home }).unwrap_err().contains("construction ship"));
        g.spawn(0, ShipClass::Constructor, home);
        g.apply(0, Command::LayMines { body: home }).unwrap();
        assert_eq!(g.mines_at(0, home), MINE_CHARGES);
        // an enemy bomber brakes into the orbit
        let e = g.spawn(1, ShipClass::Bomber, home);
        let hp = g.ship(e).unwrap().hp;
        g.trip_mines(e, home, 0.0);
        assert!((g.ship(e).unwrap().hp - (hp - MINE_DAMAGE)).abs() < 1e-9);
        assert_eq!(g.mines_at(0, home), MINE_CHARGES - 1);
        // our own ships don't set them off
        let own = g.spawn(0, ShipClass::Scout, home);
        g.trip_mines(own, home, 0.0);
        assert_eq!(g.mines_at(0, home), MINE_CHARGES - 1);
        // two enemy gunships sweep 2 charges an hour
        g.ships.retain(|s| s.id != e);
        g.bodies[home.0 as usize].levels[Upgrade::Defense.index()] = 0;
        g.bodies[home.0 as usize].def_hp = 0.0;
        g.spawn(1, ShipClass::Gunship, home);
        g.spawn(1, ShipClass::Gunship, home);
        g.mines_tick(3600.0);
        assert_eq!(g.mines_at(0, home), MINE_CHARGES - 3);
    }

    #[test]
    fn battery_reaches_across_the_soi_but_weakens() {
        use crate::combat::{shot, BEAM_RANGE};
        use crate::game::Weapon;
        use crate::v2::{v2, V2};
        let at = |d: f64, w| shot(w, v2(d, 0.0), V2::ZERO, 1.0, 0.0);
        assert!(at(5_000.0, Weapon::Beam).unwrap().factor > 0.85);
        let far = at(35_000.0, Weapon::Beam).unwrap().factor;
        assert!(far > 0.1 && far < 0.2, "{far}");
        assert!(at(BEAM_RANGE + 1.0, Weapon::Beam).is_none());
        assert!(at(10_000.0, Weapon::Energy).is_none(), "ship lasers stay short-ranged");
    }

    #[test]
    fn battery_needs_an_owned_body() {
        let mut g = crate::tech::tests::game();
        g.players[0].techs.push(Tech::PhasedLasers);
        let b = (0..g.bodies.len() as u32).map(BodyId).find(|&b| g.bodies[b.0 as usize].owner.is_none() && !g.system.body(b).is_star()).unwrap();
        g.spawn(0, ShipClass::Constructor, b);
        assert!(g.station_blocker(0, b, StationKind::LaserBattery).unwrap().contains("own"));
        let home = g.players[0].home.unwrap();
        g.spawn(0, ShipClass::Constructor, home);
        assert_eq!(g.station_blocker(0, home, StationKind::LaserBattery), None);
    }
}

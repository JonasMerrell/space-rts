//! Fog of war. Every body is always visible on the map, but what is *on* it (owner, upgrades,
//! fleets) is only known from sensors: telescopes at owned bodies, and ships' own sensors.
//! Ships are seen at a range scaled by their signature, so small ships are hard to spot and
//! dreadnoughts are visible from far away. The server sends each player `Game::view_for(p)`.
use crate::game::{BodyState, Game, PlayerId, Ship, ShipClass, ShipState, Upgrade};
use crate::system::BodyId;
use crate::v2::V2;
use serde::{Deserialize, Serialize};

/// Last thing a player's sensors saw at a body.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Intel {
    pub seen_t: f64,
    pub state: BodyState,
    /// armed power in orbit per player at that time
    pub power: Vec<(PlayerId, f64)>,
}

/// Sensor range (km) of an owned body.
pub fn body_sensor(b: &BodyState) -> f64 {
    60_000.0 + 250_000.0 * b.level(Upgrade::Telescope) as f64
}

/// Sensor range (km) of a ship.
pub fn ship_sensor(c: ShipClass) -> f64 {
    match c {
        ShipClass::Scout => 150_000.0,
        ShipClass::Dreadnought => 40_000.0,
        _ => 20_000.0,
    }
}

impl Game {
    /// (position, range) of every sensor a player has at time t.
    pub fn sensors(&self, p: PlayerId) -> Vec<(V2, f64)> {
        let t = self.t;
        let mut out: Vec<(V2, f64)> = self
            .bodies
            .iter()
            .enumerate()
            .filter(|(_, b)| b.owner == Some(p))
            .map(|(i, b)| (self.system.pos(BodyId(i as u32), t), body_sensor(b)))
            .collect();
        for s in self.ships.iter().filter(|s| s.owner == p) {
            out.push((self.ship_pos(s, t), ship_sensor(s.class)));
        }
        out
    }

    pub fn sees_ship(&self, sensors: &[(V2, f64)], s: &Ship) -> bool {
        let k = s.class.stats().signature.sqrt();
        let pos = self.ship_pos(s, self.t);
        sensors.iter().any(|(q, r)| q.dist(pos) <= r * k)
    }

    pub fn sees_body(&self, sensors: &[(V2, f64)], b: BodyId) -> bool {
        let pos = self.system.pos(b, self.t);
        sensors.iter().any(|(q, r)| q.dist(pos) <= *r)
    }

    /// Refresh every human/bot player's intel about bodies in sensor range (once per tick).
    pub(crate) fn update_intel(&mut self) {
        for p in 0..self.players.len() {
            let sensors = self.sensors(p as PlayerId);
            if self.players[p].intel.len() != self.bodies.len() {
                self.players[p].intel = vec![None; self.bodies.len()];
            }
            for i in 0..self.bodies.len() {
                let b = BodyId(i as u32);
                if self.sees_body(&sensors, b) {
                    let power = self.power_at(b);
                    self.players[p].intel[i] = Some(Intel { seen_t: self.t, state: self.bodies[i].clone(), power });
                }
            }
        }
    }

    /// What player `p` is allowed to know: hidden enemy ships removed, enemy plans stripped,
    /// bodies outside sensor range replaced by the last intel (or unknown), other players'
    /// stockpiles and intel blanked, and only `p`'s events kept.
    pub fn view_for(&self, p: PlayerId) -> Game {
        let sensors = self.sensors(p);
        let mut g = self.clone();
        g.ships = self
            .ships
            .iter()
            .filter(|s| s.owner == p || self.sees_ship(&sensors, s))
            .cloned()
            .map(|mut s| {
                if s.owner != p {
                    // you can see where a ship is going, not what it plans to do next
                    s.route.clear();
                    match &mut s.state {
                        ShipState::Parked { departure, .. } => *departure = None,
                        ShipState::Flying(f) => {
                            f.burns.clear();
                            f.cache = None;
                        }
                    }
                }
                s
            })
            .collect();
        let intel = self.players.get(p as usize).map(|pl| pl.intel.clone()).unwrap_or_default();
        for i in 0..g.bodies.len() {
            if self.bodies[i].owner == Some(p) || self.sees_body(&sensors, BodyId(i as u32)) {
                continue;
            }
            g.bodies[i] = intel.get(i).cloned().flatten().map(|x| x.state).unwrap_or_default();
            g.bodies[i].queue.clear();
        }
        for pl in g.players.iter_mut() {
            if pl.id != p {
                pl.matter = 0.0;
                pl.energy = 0.0;
                pl.intel.clear();
                pl.bot_avoid.clear();
            }
        }
        g.events.retain(|e| e.to.is_none() || e.to == Some(p));
        g
    }
}

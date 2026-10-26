//! Fog of war. Every body is always visible on the map, but what is *on* it (owner, upgrades,
//! fleets) is only known from sensors: telescopes at owned bodies, sensor stations, and ships'
//! own sensors. A ship is seen when it is within `range * sqrt(signature)` of a sensor, so small
//! ships are hard to spot, dreadnoughts are visible from far away, a fleet parked together glows
//! brighter than one ship (× sqrt(n)), and an engine burn lights a ship up for a while.
//!
//! Visibility is evaluated once per tick (`update_intel`) and cached per player, so the server
//! can cut `Game::view_for(p)` cheaply for every State it sends.
use crate::game::{BodyState, Game, PlayerId, Ship, ShipClass, ShipId, ShipState, Upgrade};
use crate::system::BodyId;
use crate::v2::V2;
use serde::{Deserialize, Serialize};

/// Last thing a player's sensors saw at a body.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Intel {
    pub seen_t: f64,
    pub state: BodyState,
    /// armed power in orbit per player at that time (only ships that were detected)
    pub power: Vec<(PlayerId, f64)>,
}

/// Last sighting of an enemy ship that is no longer in sensor range. `ship` is the ship as it was
/// seen (plans stripped), so its coast arc can be extrapolated for the time scrubber.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Contact {
    pub seen_t: f64,
    pub ship: Ship,
}

/// What a player currently detects. Recomputed every tick; `ships` and `bodies` are sorted.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Vision {
    pub t: f64,
    pub ships: Vec<ShipId>,
    /// enemy ships inside a scanning station's range: their burns and routes are visible too
    pub scanned: Vec<ShipId>,
    pub bodies: Vec<BodyId>,
    /// bodies inside a scanning station's range (plans made there, e.g. asteroid drives, are visible)
    pub scanned_bodies: Vec<BodyId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SensorKind {
    /// telescopes and ship sensors: detection range scales with the target's signature
    Passive,
    /// scanning station: sees every ship in range regardless of signature, and their plans
    Scanner,
    /// sensor relay: owned sensors inside its range get `RELAY_BOOST` × range
    Relay,
}

#[derive(Clone, Copy, Debug)]
pub struct Sensor {
    pub pos: V2,
    pub range: f64,
    pub kind: SensorKind,
}

/// Engine plumes multiply a ship's signature by this while burning and for `FLARE_TIME` after.
pub const BURN_FLARE: f64 = 10.0;
pub const FLARE_TIME: f64 = 1800.0;
pub const RELAY_BOOST: f64 = 1.5;
/// Ghost contacts older than this are forgotten.
pub const CONTACT_TTL: f64 = 2.0 * 86_400.0;
/// At most this many ghost contacts per player (the most recent are kept).
pub const MAX_CONTACTS: usize = 64;

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

/// Signature of a ship at time t, including the engine flare after a burn.
pub fn signature(s: &Ship, t: f64) -> f64 {
    let mut k = s.class.stats().signature;
    if t - s.last_burn_t < FLARE_TIME && t >= s.last_burn_t {
        k *= BURN_FLARE;
    }
    k
}

/// Is a target with signature `sig` at `pos` detected by these sensors? Returns (seen, scanned).
pub fn detects(sensors: &[Sensor], pos: V2, sig: f64) -> (bool, bool) {
    let k = sig.max(0.0).sqrt();
    let mut seen = false;
    for s in sensors {
        let d = s.pos.dist(pos);
        match s.kind {
            SensorKind::Scanner if d <= s.range => return (true, true),
            SensorKind::Passive if d <= s.range * k => seen = true,
            _ => {}
        }
    }
    (seen, false)
}

/// Can these sensors make out what is on a body at `pos`?
pub fn sees_point(sensors: &[Sensor], pos: V2) -> bool {
    sensors.iter().any(|s| s.kind != SensorKind::Relay && s.pos.dist(pos) <= s.range)
}

impl Game {
    /// Every sensor player `p` has at the current time, with relay boosts applied.
    /// `ship_pos[i]` is the position of `self.ships[i]` now.
    pub fn sensors_with(&self, p: PlayerId, ship_pos: &[V2]) -> Vec<Sensor> {
        let t = self.t;
        let mut out: Vec<Sensor> = self
            .bodies
            .iter()
            .enumerate()
            .filter(|(_, b)| b.owner == Some(p))
            .map(|(i, b)| Sensor { pos: self.system.pos(BodyId(i as u32), t), range: body_sensor(b), kind: SensorKind::Passive })
            .collect();
        for (s, &pos) in self.ships.iter().zip(ship_pos) {
            if s.owner == p {
                out.push(Sensor { pos, range: ship_sensor(s.class), kind: SensorKind::Passive });
            }
        }
        let relays: Vec<Sensor> = out.iter().filter(|s| s.kind == SensorKind::Relay).copied().collect();
        for s in out.iter_mut().filter(|s| s.kind != SensorKind::Relay) {
            if relays.iter().any(|r| r.pos.dist(s.pos) <= r.range) {
                s.range *= RELAY_BOOST;
            }
        }
        out
    }

    /// (position, range) of every sensor a player has now.
    pub fn sensors(&self, p: PlayerId) -> Vec<Sensor> {
        let pos: Vec<V2> = self.ships.iter().map(|s| self.ship_pos(s, self.t)).collect();
        self.sensors_with(p, &pos)
    }

    /// Current positions of all ships (fills the trajectory caches).
    fn ship_positions(&mut self) -> Vec<V2> {
        let t = self.t;
        let sys = &self.system;
        self.ships
            .iter_mut()
            .map(|s| match &mut s.state {
                ShipState::Parked { body, .. } => sys.pos(*body, t),
                ShipState::Flying(f) => f.trajectory(sys).abs_pos(sys, t).unwrap_or(V2::ZERO),
            })
            .collect()
    }

    /// Recompute every player's vision, refresh intel about bodies in sensor range and ghost
    /// contacts of enemy ships (once per tick).
    pub(crate) fn update_intel(&mut self) {
        let pos = self.ship_positions();
        let t = self.t;
        // parked ships of one owner at one body glow together: each is seen at its own
        // signature × sqrt(group size), so a fleet is easier to spot but its scouts still less so
        let mut groups: Vec<(BodyId, PlayerId, f64)> = vec![];
        for s in &self.ships {
            if let Some(b) = s.parked_at() {
                match groups.iter_mut().find(|g| g.0 == b && g.1 == s.owner) {
                    Some(g) => g.2 += 1.0,
                    None => groups.push((b, s.owner, 1.0)),
                }
            }
        }
        let sig: Vec<f64> = self
            .ships
            .iter()
            .map(|s| {
                let n = s.parked_at().and_then(|b| groups.iter().find(|g| g.0 == b && g.1 == s.owner)).map(|g| g.2).unwrap_or(1.0);
                signature(s, t) * n.sqrt()
            })
            .collect();
        for p in 0..self.players.len() {
            let me = p as PlayerId;
            let sensors = self.sensors_with(me, &pos);
            let mut v = Vision { t, ..Default::default() };
            for (i, s) in self.ships.iter().enumerate() {
                if s.owner == me {
                    v.ships.push(s.id);
                    continue;
                }
                let (seen, scanned) = detects(&sensors, pos[i], sig[i]);
                if seen {
                    v.ships.push(s.id);
                }
                if scanned {
                    v.scanned.push(s.id);
                }
            }
            for i in 0..self.bodies.len() {
                let b = BodyId(i as u32);
                let pos = self.system.pos(b, t);
                if self.bodies[i].owner == Some(me) || sees_point(&sensors, pos) {
                    v.bodies.push(b);
                }
                if sensors.iter().any(|s| s.kind == SensorKind::Scanner && s.pos.dist(pos) <= s.range) {
                    v.scanned_bodies.push(b);
                }
            }
            v.ships.sort();
            v.scanned.sort();

            if self.players[p].intel.len() != self.bodies.len() {
                self.players[p].intel = vec![None; self.bodies.len()];
            }
            for &b in &v.bodies {
                let power = self.power_at(b).into_iter().filter(|&(q, _)| q == me || self.bodies[b.0 as usize].owner == Some(q) || self.power_seen(&v, b, q)).collect();
                let mut state = self.bodies[b.0 as usize].clone();
                state.queue.retain(|j| j.owner == me);
                self.players[p].intel[b.0 as usize] = Some(Intel { seen_t: t, state, power });
            }

            // ghosts: remember enemy ships we see now; forget the ones we see again, lost ones
            // we were watching, and stale ones
            let mut contacts = std::mem::take(&mut self.players[p].contacts);
            contacts.retain(|c| {
                t - c.seen_t < CONTACT_TTL
                    && v.ships.binary_search(&c.ship.id).is_err()
                    && (self.ship(c.ship.id).is_some() || !sees_point(&sensors, self.ghost_pos(c, t)))
            });
                for s in &self.ships {
                if s.owner != me && v.ships.binary_search(&s.id).is_ok() {
                    contacts.push(Contact { seen_t: t, ship: self.redact(s, false) });
                }
            }
            if contacts.len() > MAX_CONTACTS {
                contacts.sort_by(|a, b| b.seen_t.total_cmp(&a.seen_t));
                contacts.truncate(MAX_CONTACTS);
            }
            self.players[p].contacts = contacts;
            self.players[p].vision = v;
        }
    }

    /// Did vision `v` detect the armed ships of player `q` at body `b`?
    fn power_seen(&self, v: &Vision, b: BodyId, q: PlayerId) -> bool {
        self.ships.iter().any(|s| s.owner == q && s.parked_at() == Some(b) && v.ships.binary_search(&s.id).is_ok())
    }

    /// Where a ghost contact would be at time t if it kept coasting (or stayed parked).
    pub fn ghost_pos(&self, c: &Contact, t: f64) -> V2 {
        self.ship_pos(&c.ship, t)
    }

    /// An enemy ship as another player may see it: no plans (unless `scanned`), no exact fuel,
    /// a generic name and no standing order.
    pub fn redact(&self, s: &Ship, scanned: bool) -> Ship {
        let mut s = s.clone();
        s.name = s.class.stats().name.to_string();
        s.order = crate::game::StandingOrder::Hold;
        if !scanned {
            s.fuel = 0.0;
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
    }

    /// What player `p` is allowed to know: undetected enemy ships removed (kept as ghost
    /// contacts), enemy plans stripped unless scanned, bodies outside sensor range replaced by
    /// the last intel (or unknown), other players' stockpiles, homes and intel blanked, and only
    /// `p`'s events kept. Uses the vision computed at the last tick.
    pub fn view_for(&self, p: PlayerId) -> Game {
        let Some(me) = self.players.get(p as usize) else {
            // not a player (spectator): only the map
            let mut g = self.clone();
            g.ships.clear();
            g.bodies = vec![BodyState::default(); g.bodies.len()];
            for pl in g.players.iter_mut() {
                blank_player(pl);
            }
            g.events.retain(|e| e.to.is_none());
            return g;
        };
        let v = &me.vision;
        let mut g = self.clone();
        g.ships = self
            .ships
            .iter()
            .filter(|s| s.owner == p || v.ships.binary_search(&s.id).is_ok())
            .map(|s| if s.owner == p { s.clone() } else { self.redact(s, v.scanned.binary_search(&s.id).is_ok()) })
            .collect();
        g.next_ship = g.ships.iter().map(|s| s.id.0 + 1).max().unwrap_or(0);
        g.bodies = (0..self.bodies.len())
            .map(|i| {
                let mut b = if self.bodies[i].owner == Some(p) {
                    self.bodies[i].clone()
                } else if v.bodies.binary_search(&BodyId(i as u32)).is_ok() {
                    self.bodies[i].clone()
                } else {
                    me.intel.get(i).cloned().flatten().map(|x| x.state).unwrap_or_default()
                };
                b.queue.retain(|j| j.owner == p);
                b
            })
            .collect();
        for pl in g.players.iter_mut() {
            if pl.id != p {
                blank_player(pl);
            } else {
                // ghosts are only for ships that are out of sight now
                pl.contacts.retain(|c| g.ships.iter().all(|s| s.id != c.ship.id));
            }
        }
        g.events.retain(|e| e.to.is_none() || e.to == Some(p));
        g
    }
}

fn blank_player(pl: &mut crate::game::Player) {
    pl.matter = 0.0;
    pl.energy = 0.0;
    pl.home = None;
    pl.intel.clear();
    pl.contacts.clear();
    pl.vision = Vision::default();
    pl.bot_avoid.clear();
}

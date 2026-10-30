//! Fog of war. Every body is always visible on the map, but what is *on* it (owner, upgrades,
//! fleets) is only known from sensors: telescopes at owned bodies, sensor stations, and ships'
//! own sensors. A ship is seen when it is within `range * sqrt(signature)` of a sensor, so small
//! ships are hard to spot, dreadnoughts are visible from far away, a fleet parked together glows
//! brighter than one ship (× sqrt(n)), and an engine burn lights a ship up for a while.
//!
//! Visibility is evaluated once per tick (`update_intel`) and cached per player, so the server
//! can cut `Game::view_for(p)` cheaply for every State it sends.
use crate::game::{Event, EventKind, BodyState, Game, PlayerId, Ship, ShipClass, ShipId, ShipState, Upgrade};
use crate::station::StationKind;
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
    /// gravimetric array: ignores signature; `range` is for a `GRAV_REF_MASS` target and scales
    /// with sqrt(mass)
    Gravimetric,
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
/// Ships parked at one body are spread around its orbit: no passive sensor is ever closer to a
/// target than this (only matters for very faint targets, i.e. cloaked ships).
pub const MIN_DETECT_DIST: f64 = 5_000.0;
/// Target mass (t) at which a gravimetric array reaches its nominal range.
pub const GRAV_REF_MASS: f64 = 1000.0;
/// Lidar sweep: range multiplier, how long it lasts, and the cooldown between sweeps.
pub const LIDAR_RANGE: f64 = 3.0;
pub const LIDAR_TIME: f64 = 3600.0;
pub const LIDAR_COOLDOWN: f64 = 6.0 * 3600.0;
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

/// Sensor a station provides (before research mods), if any.
pub fn station_sensor(kind: StationKind) -> Option<(f64, SensorKind)> {
    match kind {
        StationKind::Telescope => Some((500_000.0, SensorKind::Passive)),
        StationKind::Scanner => Some((150_000.0, SensorKind::Scanner)),
        StationKind::Relay => Some((400_000.0, SensorKind::Relay)),
        StationKind::Gravimetric => Some((400_000.0, SensorKind::Gravimetric)),
        _ => None,
    }
}

/// Signature of a ship at time t, including the engine flare after a burn.
pub fn signature(s: &Ship, t: f64) -> f64 {
    let mut k = crate::cloak::apparent_class(s).stats().signature;
    if t - s.last_burn_t < FLARE_TIME && t >= s.last_burn_t {
        k *= BURN_FLARE;
    }
    k
}

/// Is a target with signature `sig` at `pos` detected by these sensors? Returns (seen, scanned).
pub fn detects(sensors: &[Sensor], pos: V2, sig: f64) -> (bool, bool) {
    detects_mass(sensors, pos, sig, 0.0)
}

/// `detects` for a target of `mass` tonnes (gravimetric arrays see mass, not signature).
pub fn detects_mass(sensors: &[Sensor], pos: V2, sig: f64, mass: f64) -> (bool, bool) {
    let k = sig.max(0.0).sqrt();
    let km = (mass.max(0.0) / GRAV_REF_MASS).sqrt();
    let mut seen = false;
    for s in sensors {
        let d = s.pos.dist(pos);
        match s.kind {
            SensorKind::Scanner if d <= s.range => return (true, true),
            SensorKind::Passive if d.max(MIN_DETECT_DIST) <= s.range * k => seen = true,
            SensorKind::Gravimetric if d <= s.range * km => seen = true,
            _ => {}
        }
    }
    (seen, false)
}

/// Can these sensors make out what is on a body at `pos`?
pub fn sees_point(sensors: &[Sensor], pos: V2) -> bool {
    sensors.iter().any(|s| matches!(s.kind, SensorKind::Passive | SensorKind::Scanner) && s.pos.dist(pos) <= s.range)
}

impl Game {
    /// Does this sensor set see ship `s` right now? (Same rule as the per-tick vision.)
    pub fn sees_ship(&self, sensors: &[Sensor], s: &Ship) -> bool {
        let sig = match self.ships.iter().position(|x| x.id == s.id) {
            Some(i) => self.signatures()[i],
            None => signature(s, self.t),
        };
        detects_mass(sensors, self.ship_pos(s, self.t), sig, s.mass()).0
    }

    /// Can this sensor set make out what is on body `b` right now?
    pub fn sees_body(&self, sensors: &[Sensor], b: BodyId) -> bool {
        sees_point(sensors, self.system.pos(b, self.t))
    }

    /// Effective signature of every ship now (index = `self.ships`): class signature, engine
    /// flare, the owner's signature research, and parked fleets glowing together.
    pub fn signatures(&self) -> Vec<f64> {
        let t = self.t;
        // parked ships of one owner at one body glow together: each is seen at its own
        // signature × sqrt(group size), so a fleet is easier to spot but its scouts still less so
        let mut groups: Vec<(BodyId, PlayerId, f64)> = vec![];
        for s in self.ships.iter().filter(|s| !s.dark(t)) {
            if let Some(b) = s.parked_at() {
                match groups.iter_mut().find(|g| g.0 == b && g.1 == s.owner) {
                    Some(g) => g.2 += 1.0,
                    None => groups.push((b, s.owner, 1.0)),
                }
            }
        }
        let mods: Vec<crate::tech::Mods> = (0..self.players.len()).map(|p| self.mods(p as PlayerId)).collect();
        self
            .ships
            .iter()
            .map(|s| {
                let n = s.parked_at().filter(|_| !s.dark(t)).and_then(|b| groups.iter().find(|g| g.0 == b && g.1 == s.owner)).map(|g| g.2).unwrap_or(1.0);
                let m = &mods[s.owner as usize];
                signature(s, t) * n.sqrt() * m.signature * Game::cloak_sig(s, t, m.vent_sig)
            })
            .collect()
    }

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
        for st in self.stations.iter().filter(|st| st.owner == p && st.active(t)) {
            if let Some((range, kind)) = station_sensor(st.kind) {
                // a sweeping lidar reaches farther and, like any scanner, ignores signature
                let range = if kind == SensorKind::Scanner && self.sweeping(st.id) { range * LIDAR_RANGE } else { range };
                out.push(Sensor { pos: self.system.pos(st.body, t), range, kind });
            }
        }
        let k = self.mods(p).sensor;
        for s in out.iter_mut() {
            s.range *= k;
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
        let sig = self.signatures();
        // teammates share their sensor picture
        let own: Vec<Vec<Sensor>> = (0..self.players.len()).map(|p| self.sensors_with(p as PlayerId, &pos)).collect();
        for p in 0..self.players.len() {
            let me = p as PlayerId;
            let sensors: Vec<Sensor> = if self.team_game() {
                self.allies(me).into_iter().flat_map(|q| own[q as usize].iter().copied()).collect()
            } else {
                own[p].clone()
            };
            let mut v = Vision { t, ..Default::default() };
            for (i, s) in self.ships.iter().enumerate() {
                if self.allied(me, s.owner) {
                    v.ships.push(s.id);
                    continue;
                }
                let (seen, scanned) = detects_mass(&sensors, pos[i], sig[i], s.mass());
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
                if self.bodies[i].owner.is_some_and(|o| self.allied(me, o)) || sees_point(&sensors, pos) {
                    v.bodies.push(b);
                }
                if sensors.iter().any(|s| s.kind == SensorKind::Scanner && s.pos.dist(pos) <= s.range) {
                    v.scanned_bodies.push(b);
                }
            }
            // a sweeping lidar lights itself up for everyone it illuminates; gates glow for all
            for b in self.pinged_by_others(me, &sensors).into_iter().chain(self.gate_bodies()) {
                if !v.bodies.contains(&b) {
                    v.bodies.push(b);
                }
            }
            v.bodies.sort();
            v.ships.sort();
            v.scanned.sort();

            // outposts append bodies: keep what we know about the others
            self.players[p].intel.resize(self.bodies.len(), None);
            // vision isn't saved, so after a restart (or at game start) there's nothing to diff against
            if !self.players[p].vision.ships.is_empty() {
                let news = self.sensor_news(me, &self.players[p].vision, &v, &pos);
                log_sensor_items(&mut self.players[p].sensor_log, news);
            }
            for &b in &v.bodies {
                let mut power: Vec<(PlayerId, f64)> = self.power_at(b).into_iter().filter(|&(q, _)| q == me || self.bodies[b.0 as usize].owner == Some(q) || self.power_seen(&v, b, q)).collect();
                // decoys we haven't seen through count as the warship they pretend to be
                let see_decoys = self.sees_decoys(me);
                for d in self.ships.iter().filter(|d| d.class == ShipClass::Decoy && d.owner != me && d.parked_at() == Some(b)) {
                    if see_decoys || v.scanned.binary_search(&d.id).is_ok() || v.ships.binary_search(&d.id).is_err() {
                        continue;
                    }
                    let fake = crate::cloak::apparent_class(d).stats().attack;
                    match power.iter_mut().find(|e| e.0 == d.owner) {
                        Some(e) => e.1 += fake,
                        None => power.push((d.owner, fake)),
                    }
                }
                let mut state = self.bodies[b.0 as usize].clone();
                state.queue.retain(|j| j.owner == me);
                self.players[p].intel[b.0 as usize] = Some(Intel { seen_t: t, state, power });
            }

            // ghosts: remember enemy ships we see now; forget the ones we see again, lost ones
            // we were watching, and stale ones
            let mut contacts = std::mem::take(&mut self.players[p].contacts);
            contacts.retain_mut(|c| {
                if t - c.seen_t >= CONTACT_TTL || v.ships.binary_search(&c.ship.id).is_ok() {
                    return false;
                }
                if self.ship(c.ship.id).is_some() {
                    return true;
                }
                // a ghost's coast arc never changes: predict it once (an 8-day coast is expensive)
                if let ShipState::Flying(f) = &mut c.ship.state {
                    f.trajectory(&self.system);
                }
                !sees_point(&sensors, self.ghost_pos(c, t))
            });
                for s in &self.ships {
                if !self.allied(me, s.owner) && v.ships.binary_search(&s.id).is_ok() {
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

    /// What changed in `me`'s sensor picture since the last tick: contacts spotted and lost,
    /// engine burns, other players' bodies changing hands and battles sighted. Only from `me`'s
    /// own vision (`prev` → `v`), grouped per owner and nearest body.
    fn sensor_news(&self, me: PlayerId, prev: &Vision, v: &Vision, pos: &[V2]) -> Vec<SensorItem> {
        let t = v.t;
        let mut out: Vec<SensorItem> = vec![];
        let mut add = |kind: ReportKind, owner: PlayerId, body: Option<BodyId>, s: &Ship, dest: Option<BodyId>| {
            match out.iter_mut().find(|x| x.kind == kind && x.owner == Some(owner) && x.body == body) {
                Some(x) => {
                    x.ships.push((s.id, s.class));
                    x.dest = x.dest.or(dest);
                }
                None => out.push(SensorItem { t, kind, owner: Some(owner), body, ships: vec![(s.id, s.class)], dest, other: None }),
            }
        };
        for (i, s) in self.ships.iter().enumerate() {
            if self.allied(me, s.owner) {
                continue;
            }
            let now = v.ships.binary_search(&s.id).is_ok();
            let before = prev.ships.binary_search(&s.id).is_ok();
            let at = s.parked_at().or_else(|| self.nearest_body(pos[i]));
            let dest = match &s.state {
                ShipState::Flying(f) if v.scanned.binary_search(&s.id).is_ok() => f.dest,
                _ => None,
            };
            if now && !before {
                add(ReportKind::Spotted, s.owner, at, s, dest);
            } else if !now && before {
                add(ReportKind::Lost, s.owner, at, s, None);
            }
            if now && s.last_burn_t > prev.t && s.last_burn_t <= t {
                add(ReportKind::Burn, s.owner, at, s, dest);
            }
        }
        let intel = self.players.get(me as usize).map(|p| &p.intel);
        for &b in &v.bodies {
            let now = &self.bodies[b.0 as usize];
            let Some(Some(old)) = intel.and_then(|x| x.get(b.0 as usize)) else { continue };
            // our own wins and losses already have their own events
            if now.owner != old.state.owner && now.owner != Some(me) && old.state.owner != Some(me) {
                out.push(SensorItem { t, kind: ReportKind::Territory, owner: now.owner, body: Some(b), ships: vec![], dest: None, other: old.state.owner });
            }
            let mine_there = self.ships.iter().any(|s| s.owner == me && s.parked_at() == Some(b));
            if now.in_combat && !old.state.in_combat && now.owner != Some(me) && !mine_there {
                out.push(SensorItem { t, kind: ReportKind::Battle, owner: now.owner, body: Some(b), ships: vec![], dest: None, other: None });
            }
        }
        out
    }

    /// A sensor log item as a report line (title, text).
    pub fn describe_sensor_item(&self, x: &SensorItem) -> (String, String) {
        let name = |q: Option<PlayerId>| q.and_then(|q| self.players.get(q as usize)).map(|p| p.name.clone()).unwrap_or_else(|| "nobody".into());
        let bname = |b: Option<BodyId>| b.map(|b| self.system.body(b).name.clone()).unwrap_or_else(|| "deep space".into());
        let mut classes: Vec<(&str, usize)> = vec![];
        for (_, c) in &x.ships {
            let c = c.stats().name;
            match classes.iter_mut().find(|y| y.0 == c) {
                Some(y) => y.1 += 1,
                None => classes.push((c, 1)),
            }
        }
        let list = classes.iter().map(|(c, k)| if *k > 1 { format!("{k} {c}s") } else { c.to_string() }).collect::<Vec<_>>().join(", ");
        let to = x.dest.map(|d| format!(", heading for {}", bname(Some(d)))).unwrap_or_default();
        let who = name(x.owner);
        match x.kind {
            ReportKind::Spotted => ("Contact".into(), format!("{who}: {list} near {}{to}", bname(x.body))),
            ReportKind::Lost => ("Contact lost".into(), format!("Lost track of {who}'s {list} near {}", bname(x.body))),
            ReportKind::Burn => ("Engine burn".into(), format!("{who}'s {list} burned near {}{to}", bname(x.body))),
            ReportKind::Territory => ("Territory".into(), format!("{} changed hands from {} to {who}", bname(x.body), name(x.other))),
            ReportKind::Battle => ("Battle sighted".into(), format!("Fighting at {}", bname(x.body))),
        }
    }

    /// Everything player `p` should hear about after game time `since`: their events (battles,
    /// territory, builds, research…) plus their sensor log as `EventKind::Sensor` events, oldest
    /// first, at most `max` (the most recent are kept).
    pub fn away_report(&self, p: PlayerId, since: f64, max: usize) -> Vec<Event> {
        let mut v: Vec<Event> = self.events_for(p).filter(|e| e.t > since).cloned().collect();
        if let Some(pl) = self.players.get(p as usize) {
            for x in pl.sensor_log.iter().filter(|x| x.t > since) {
                let (title, body) = self.describe_sensor_item(x);
                v.push(Event {
                    seq: 0,
                    t: x.t,
                    to: Some(p),
                    kind: EventKind::Sensor,
                    title,
                    body,
                    body_ref: x.body,
                    ship_ref: x.ships.first().map(|s| s.0),
                    group: None,
                });
            }
        }
        v.sort_by(|a, b| a.t.total_cmp(&b.t));
        let n = v.len();
        v.drain(..n.saturating_sub(max));
        v
    }

    /// Planet or moon nearest to a point (for "near X" in sensor reports).
    fn nearest_body(&self, pos: V2) -> Option<BodyId> {
        self.system
            .bodies
            .iter()
            .filter(|b| !b.is_star())
            .min_by(|a, b| self.system.pos(a.id, self.t).dist(pos).total_cmp(&self.system.pos(b.id, self.t).dist(pos)))
            .map(|b| b.id)
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
    /// A decoy shows as its mimic class unless `scanned` (callers pass scanned || the viewer's
    /// `Mods::see_decoys`).
    pub fn redact(&self, s: &Ship, scanned: bool) -> Ship {
        let mut s = s.clone();
        if !scanned && s.class == ShipClass::Decoy {
            s.class = crate::cloak::apparent_class(&s);
            s.hp = s.class.stats().hp;
            s.tech = Default::default();
        }
        s.mimic = None;
        s.cloak = None;
        s.name = s.class.stats().name.to_string();
        s.order = crate::game::StandingOrder::Hold;
        if !scanned {
            s.fuel = 0.0;
            s.route.clear();
            s.intercept = None;
            match &mut s.state {
                ShipState::Parked { departure, .. } => *departure = None,
                ShipState::Flying(f) => {
                    // where it's headed (and whether it will brake there) is a plan, not something
                    // a telescope sees: only the coast arc is known
                    f.burns.clear();
                    f.dest = None;
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
            g.stations.clear();
            g.minefields.clear();
            g.fleets.clear();
            g.drives.clear();
            g.battles.clear();
            g.bodies = vec![BodyState::default(); g.bodies.len()];
            for pl in g.players.iter_mut() {
                blank_player(pl);
            }
            g.events.retain(|e| e.to.is_none());
            return g;
        };
        let v = &me.vision;
        let see_decoys = self.sees_decoys(p);
        let mut g = self.clone();
        g.ships = self
            .ships
            .iter()
            .filter(|s| s.owner == p || v.ships.binary_search(&s.id).is_ok())
            .map(|s| match () {
                _ if s.owner == p => s.clone(),
                // teammates' plans are shared, like a scanner reading
                _ if self.allied(p, s.owner) => self.redact(s, true),
                _ => self.redact(s, see_decoys && s.class == ShipClass::Decoy || v.scanned.binary_search(&s.id).is_ok()),
            })
            .collect();
        g.next_ship = g.ships.iter().map(|s| s.id.0 + 1).max().unwrap_or(0);
        // live battles: only the ones you're in, without the ships you can't see. Enemy ships fighting
        // you keep their fuel: their autopilot depends on it, and your screen runs the same battle.
        g.battles.retain(|b| b.sides.contains(&p));
        for s in g.ships.iter_mut().filter(|s| s.owner != p) {
            if g.battles.iter().any(|b| b.fighters.iter().any(|f| f.unit == crate::battle::Unit::Ship(s.id))) {
                if let Some(real) = self.ship(s.id) {
                    s.fuel = real.fuel;
                }
            }
        }
        for b in g.battles.iter_mut() {
            b.fighters.retain(|f| match f.unit {
                crate::battle::Unit::Ship(id) => g.ships.iter().any(|s| s.id == id),
                _ => true,
            });
        }
        // stations are big: seen whenever their body is
        g.stations.retain(|st| st.owner == p || v.bodies.binary_search(&st.body).is_ok());
        // mines are only seen by their owner and by scanning stations
        g.minefields.retain(|m| self.allied(p, m.owner) || v.scanned_bodies.contains(&m.body));
        for st in g.stations.iter_mut().filter(|st| st.owner != p) {
            st.fuel = 0.0;
        }
        g.drives = self.visible_drives(p, v);
        g.bodies = (0..self.bodies.len())
            .map(|i| {
                let mut b = if self.bodies[i].owner.is_some_and(|o| self.allied(p, o)) {
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
        // outposts nobody told us about: drawn as gone. The orbit stays so prediction matches the server.
        for i in 0..g.bodies.len() {
            let b = BodyId(i as u32);
            if self.outpost_hidden(p, b) && !g.ships.iter().any(|s| s.parked_at() == Some(b)) {
                g.bodies[i] = BodyState { gone: true, ..Default::default() };
                g.system.bodies[i].name = "?".into();
                g.stations.retain(|st| st.body != b);
            }
        }
        g.events.retain(|e| e.to.is_none() || e.to == Some(p));
        g.fleets.retain(|f| f.owner == p);
        g.next_fleet = 0;
        g
    }
}

fn blank_player(pl: &mut crate::game::Player) {
    pl.matter = 0.0;
    pl.energy = 0.0;
    pl.home = None;
    pl.intel.clear();
    pl.contacts.clear();
    pl.sensor_log.clear();
    pl.last_active = 0.0;
    pl.research.clear();
    pl.waiting.clear();
    pl.techs.clear();
    pl.vision = Vision::default();
    pl.bot_avoid.clear();
}


#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReportKind {
    Spotted,
    Lost,
    Burn,
    Territory,
    Battle,
}

/// One line of a player's sensor log (the "while you were away" report). Repeated sightings of
/// the same kind, owner and body within `LOG_MERGE` are merged into one item.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SensorItem {
    pub t: f64,
    pub kind: ReportKind,
    pub owner: Option<PlayerId>,
    pub body: Option<BodyId>,
    pub ships: Vec<(ShipId, ShipClass)>,
    /// where the ships are heading (only if a scanner saw their plan)
    pub dest: Option<BodyId>,
    /// previous owner (territory changes)
    pub other: Option<PlayerId>,
}

/// Sensor log items of the same kind/owner/body this close together are merged.
pub const LOG_MERGE: f64 = 3600.0;
/// Sensor log entries kept per player.
pub const LOG_CAP: usize = 200;

fn log_sensor_items(log: &mut Vec<SensorItem>, news: Vec<SensorItem>) {
    for x in news {
        let merge = log.iter_mut().rev().take_while(|y| x.t - y.t < LOG_MERGE).find(|y| y.kind == x.kind && y.owner == x.owner && y.body == x.body && y.other == x.other);
        match merge {
            Some(y) => {
                for s in x.ships {
                    if !y.ships.iter().any(|z| z.0 == s.0) {
                        y.ships.push(s);
                    }
                }
                y.dest = x.dest.or(y.dest);
            }
            None => log.push(x),
        }
    }
    let n = log.len();
    log.drain(..n.saturating_sub(LOG_CAP));
}

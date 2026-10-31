//! War history for rewinding the map and replaying battles (Jonas 2026-10-31: "add a rewind feature
//! and then show battle replays").
//!
//! The server's game carries a `Recorder` (clients' copies never do: a cloned game records nothing).
//! Every `SNAP_EVERY` it writes a `Snap` of where every ship is and who owns what, and every
//! `FRAME_EVERY` of a live battle a `Frame` of the arena. Each entry carries a bit mask of the players
//! whose sensors saw it right then, so `Snap::for_player` / `BattleRec::for_player` hand a player only
//! what they saw at the time (fog of war). The server drains the recorder into a file per war.
use serde::{Deserialize, Serialize};

use crate::battle::{Battle, Unit};
use crate::game::{Game, PlayerId, ShipClass, Weapon};
use crate::system::BodyId;
use crate::v2::V2;

/// Game seconds between map snapshots.
pub const SNAP_EVERY: f64 = 600.0;
/// Game seconds between battle frames (after `LONG_FIGHT` frames, every `FRAME_EVERY_LONG`).
pub const FRAME_EVERY: f64 = 2.0;
pub const FRAME_EVERY_LONG: f64 = 10.0;
pub const LONG_FIGHT: usize = 1800;
/// Hits stay on a frame's list this long after they happen, so a replay draws each one once.
const KEEP_HITS: f64 = FRAME_EVERY_LONG;

fn bit(p: PlayerId) -> u32 {
    if p < 32 { 1 << p } else { 0 }
}

/// A ship on the map at a snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapShip {
    pub id: u32,
    pub owner: PlayerId,
    pub class: ShipClass,
    /// parked at this body (drawn at the body), else flying at `pos`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<BodyId>,
    /// absolute position, km (rounded)
    #[serde(default)]
    pub pos: [i64; 2],
    /// a decoy: the class enemies took it for
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub looks: Option<ShipClass>,
    /// players who saw it (server side; 0 once filtered)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub seen: u32,
}

fn is_zero(x: &u32) -> bool {
    *x == 0
}

/// The map at one moment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snap {
    pub t: f64,
    pub ships: Vec<SnapShip>,
    /// owner of each body (filtered: as the player last knew it)
    pub owners: Vec<Option<PlayerId>>,
    /// players who saw each body (server side; empty once filtered)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owner_seen: Vec<u32>,
    /// battles being fought (ids into the war's battle records)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub battles: Vec<u32>,
}

/// Something that fought in a recorded battle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecUnit {
    pub unit: Unit,
    pub owner: PlayerId,
    pub label: String,
    pub class: Option<ShipClass>,
    pub hp_max: f32,
    pub weapon: Weapon,
    /// players who saw it at some point (server side)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub seen: u32,
}

/// One fighter in a frame: unit index, arena x, y (km), hull, who saw it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecPos(pub u16, pub f32, pub f32, pub f32, #[serde(default)] pub u32);

/// A salvo in flight: firing unit index, owner, x, y, vx, vy.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecSalvo(pub u16, pub PlayerId, pub f32, pub f32, pub f32, pub f32);

/// A hit since the last frame: game time, from unit, to unit, laser beam?, damage.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecHit(pub f64, pub u16, pub u16, pub bool, pub f32);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub t: f64,
    pub u: Vec<RecPos>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub s: Vec<RecSalvo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub h: Vec<RecHit>,
}

/// A battle from start to end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BattleRec {
    pub id: u32,
    pub body: Option<BodyId>,
    /// "Vesper" / "deep space"
    pub place: String,
    pub body_radius: f64,
    /// where it was fought on the map (absolute km at the start)
    pub pos: [i64; 2],
    pub start_t: f64,
    pub end_t: f64,
    pub sides: Vec<PlayerId>,
    /// ships each side lost (by player id)
    pub lost: Vec<u32>,
    pub units: Vec<RecUnit>,
    pub frames: Vec<Frame>,
}

/// What the war list of battles shows (no frames).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BattleInfo {
    pub id: u32,
    pub body: Option<BodyId>,
    pub place: String,
    pub pos: [i64; 2],
    pub start_t: f64,
    pub end_t: f64,
    pub sides: Vec<PlayerId>,
    pub lost: Vec<u32>,
}

/// One line of a war's history file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Hist {
    S(Snap),
    B(BattleRec),
}

/// What the server's game has recorded since the last drain.
#[derive(Default, Debug)]
pub struct Recording {
    pub snaps: Vec<Snap>,
    pub battles: Vec<BattleRec>,
    open: Vec<BattleRec>,
    last_snap: Option<f64>,
}

/// `Game::recorder`: on only in the server's own game. Cloning (views, forecasts, bot what-ifs) gives
/// an empty one, so nothing but the real game ever records.
#[derive(Default)]
pub struct Recorder(pub Option<Box<Recording>>);

impl Clone for Recorder {
    fn clone(&self) -> Self {
        Recorder(None)
    }
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "Recorder(on)" } else { "Recorder(off)" })
    }
}

impl Recorder {
    pub fn on() -> Recorder {
        Recorder(Some(Box::default()))
    }

    /// Snapshots and finished battles recorded since the last call.
    pub fn drain(&mut self) -> Vec<Hist> {
        let Some(r) = self.0.as_mut() else { return vec![] };
        let mut out: Vec<Hist> = r.snaps.drain(..).map(Hist::S).collect();
        out.extend(r.battles.drain(..).map(Hist::B));
        out
    }
}

fn round(v: V2) -> [i64; 2] {
    [v.x.round() as i64, v.y.round() as i64]
}

impl Game {
    /// Players whose sensors see ship `id` now (allies always do).
    fn ship_seen_mask(&self, id: crate::game::ShipId, owner: PlayerId) -> u32 {
        self.players.iter().filter(|p| self.allied(p.id, owner) || p.vision.ships.binary_search(&id).is_ok()).fold(0, |m, p| m | bit(p.id))
    }

    fn body_seen_mask(&self, b: BodyId) -> u32 {
        self.players.iter().filter(|p| p.vision.bodies.binary_search(&b).is_ok() || self.bodies[b.0 as usize].owner.is_some_and(|o| self.allied(p.id, o))).fold(0, |m, p| m | bit(p.id))
    }

    /// After each tick's intel update: a map snapshot every `SNAP_EVERY`.
    pub(crate) fn record_snap(&mut self) {
        let Some(mut rec) = self.recorder.0.take() else { return };
        let t = self.t;
        if rec.last_snap.is_none_or(|l| t >= l + SNAP_EVERY - 1e-6) {
            rec.last_snap = Some((t / SNAP_EVERY).floor() * SNAP_EVERY);
            let ships = (0..self.ships.len())
                .map(|i| {
                    let s = &self.ships[i];
                    let at = s.parked_at();
                    let pos = if at.is_some() { [0, 0] } else { round(self.ship_pos(s, t)) };
                    let looks = (s.class == ShipClass::Decoy).then(|| crate::cloak::apparent_class(s));
                    SnapShip { id: s.id.0, owner: s.owner, class: s.class, at, pos, looks, seen: self.ship_seen_mask(s.id, s.owner) }
                })
                .collect();
            let owners = self.bodies.iter().map(|b| b.owner).collect();
            let owner_seen = (0..self.bodies.len()).map(|i| self.body_seen_mask(BodyId(i as u32))).collect();
            let battles = self.battles.iter().map(|b| b.id).collect();
            rec.snaps.push(Snap { t, ships, owners, owner_seen, battles });
        }
        self.recorder.0 = Some(rec);
    }

    /// Arena position and hull of a fighter now (None = it's gone), plus its static description.
    fn rec_fighter(&mut self, b: &Battle, i: usize, fp: V2) -> Option<(V2, f64, RecUnit)> {
        let f = &b.fighters[i];
        let t = self.t;
        let (pos, hp, unit) = match f.unit {
            Unit::Ship(id) => {
                let k = self.ships.iter().position(|s| s.id == id)?;
                let (p, _, _) = Self::ship_state_abs(&self.system, &mut self.ships[k], t)?;
                let s = &self.ships[k];
                let st = s.stats();
                let u = RecUnit { unit: f.unit, owner: f.owner, label: st.name.to_string(), class: Some(s.class), hp_max: st.hp as f32, weapon: st.weapon, seen: 0 };
                (p, s.hp, u)
            }
            Unit::Defense(body) => {
                let bs = &self.bodies[body.0 as usize];
                let u = RecUnit { unit: f.unit, owner: f.owner, label: "Defenses".into(), class: None, hp_max: self.def_max(body) as f32, weapon: Weapon::Energy, seen: 0 };
                (self.system.pos(body, t), bs.def_hp, u)
            }
            Unit::Station(id) => {
                let st = self.stations.iter().find(|x| x.id == id)?;
                let weapon = match st.kind {
                    crate::station::StationKind::LaserBattery => Weapon::Beam,
                    k if k.attack() > 0.0 => Weapon::Energy,
                    _ => Weapon::None,
                };
                let u = RecUnit { unit: f.unit, owner: f.owner, label: st.kind.label().to_string(), class: None, hp_max: st.kind.max_hp() as f32, weapon, seen: 0 };
                (self.system.pos(st.body, t), st.hp, u)
            }
        };
        Some((pos + f.dp - fp, hp, unit))
    }

    fn unit_seen_mask(&self, u: Unit, owner: PlayerId) -> u32 {
        match u {
            Unit::Ship(id) => self.ship_seen_mask(id, owner),
            Unit::Defense(b) => self.body_seen_mask(b),
            Unit::Station(id) => self.stations.iter().find(|s| s.id == id).map_or(0, |s| self.body_seen_mask(s.body)),
        }
    }

    /// After each battle step: a frame of every live battle every `FRAME_EVERY`; battles that ended
    /// are finished and queued for the server.
    pub(crate) fn record_battles(&mut self) {
        let Some(mut rec) = self.recorder.0.take() else { return };
        let t = self.t;
        // ended
        let live: Vec<u32> = self.battles.iter().map(|b| b.id).collect();
        let (gone, open): (Vec<BattleRec>, Vec<BattleRec>) = rec.open.drain(..).partition(|r| !live.contains(&r.id));
        rec.open = open;
        for mut r in gone {
            r.end_t = r.frames.last().map_or(t, |f| f.t);
            if !r.frames.is_empty() {
                rec.battles.push(r);
            }
        }
        let battles = std::mem::take(&mut self.battles);
        for b in &battles {
            let k = match rec.open.iter().position(|r| r.id == b.id) {
                Some(k) => k,
                None => {
                    let sys = &self.system;
                    let place = b.body.map(|x| sys.body(x).name.clone()).unwrap_or_else(|| "deep space".into());
                    let pos = round(b.frame(sys, t).0);
                    let body_radius = b.body.map_or(0.0, |x| sys.body(x).radius);
                    rec.open.push(BattleRec { id: b.id, body: b.body, place, body_radius, pos, start_t: b.start_t, end_t: t, sides: vec![], lost: vec![], units: vec![], frames: vec![] });
                    rec.open.len() - 1
                }
            };
            let r = &mut rec.open[k];
            r.sides = b.sides.clone();
            r.lost = b.lost.clone();
            r.end_t = t;
            let every = if r.frames.len() < LONG_FIGHT { FRAME_EVERY } else { FRAME_EVERY_LONG };
            if r.frames.last().is_some_and(|f| t < f.t + every - 1e-6) {
                continue;
            }
            let (fp, _) = b.frame(&self.system, t);
            let mut u = vec![];
            let index = |r: &mut BattleRec, unit: RecUnit| -> u16 {
                match r.units.iter().position(|x| x.unit == unit.unit) {
                    Some(i) => i as u16,
                    None => {
                        r.units.push(unit);
                        (r.units.len() - 1) as u16
                    }
                }
            };
            for i in 0..b.fighters.len() {
                let Some((p, hp, unit)) = self.rec_fighter(b, i, fp) else { continue };
                let seen = self.unit_seen_mask(unit.unit, unit.owner);
                let r = &mut rec.open[k];
                let ix = index(r, unit);
                r.units[ix as usize].seen |= seen;
                u.push(RecPos(ix, p.x as f32, p.y as f32, hp.max(0.0) as f32, seen));
            }
            let r = &mut rec.open[k];
            let ix = |r: &BattleRec, unit: Unit| r.units.iter().position(|x| x.unit == unit).map(|i| i as u16);
            let s = b.salvos.iter().filter_map(|sv| Some(RecSalvo(ix(r, sv.from)?, sv.owner, sv.pos.x as f32, sv.pos.y as f32, sv.vel.x as f32, sv.vel.y as f32))).collect();
            let since = r.frames.last().map_or(f64::NEG_INFINITY, |f| f.t);
            let h = b.hits.iter().filter(|h| h.t > since && h.t <= t && t - h.t < KEEP_HITS).filter_map(|h| Some(RecHit(h.t, ix(r, h.from).or(ix(r, h.to))?, ix(r, h.to)?, h.beam, h.dmg as f32))).collect();
            r.frames.push(Frame { t, u, s, h });
        }
        self.battles = battles;
        self.recorder.0 = Some(rec);
    }

    /// The live battles' records so far (a server restart loses them; `drain` returns finished ones).
    pub fn recording_open(&self) -> usize {
        self.recorder.0.as_ref().map_or(0, |r| r.open.len())
    }
}

impl Snap {
    /// What player `p` saw: ships they saw (decoys as what they pretended to be, unless allied), and
    /// body owners as last seen (`known` carries that across snapshots; start it empty).
    pub fn for_player(&self, game: &Game, p: PlayerId, known: &mut Vec<Option<PlayerId>>) -> Snap {
        let b = bit(p);
        let ships = self
            .ships
            .iter()
            .filter(|s| s.seen & b != 0 || game.allied(p, s.owner))
            .map(|s| {
                let mut s = s.clone();
                if !game.allied(p, s.owner) {
                    if let Some(l) = s.looks {
                        s.class = l;
                    }
                }
                s.looks = None;
                s.seen = 0;
                s
            })
            .collect();
        known.resize(self.owners.len(), None);
        for (i, o) in self.owners.iter().enumerate() {
            let seen = self.owner_seen.get(i).is_some_and(|m| m & b != 0) || o.is_some_and(|o| game.allied(p, o));
            if seen {
                known[i] = *o;
            }
        }
        Snap { t: self.t, ships, owners: known.clone(), owner_seen: vec![], battles: self.battles.clone() }
    }
}

impl BattleRec {
    /// Can `p` watch this? They fought in it, or saw some of it.
    pub fn visible_to(&self, game: &Game, p: PlayerId) -> bool {
        self.sides.iter().any(|&s| game.allied(p, s)) || self.units.iter().any(|u| u.seen & bit(p) != 0)
    }

    pub fn info(&self) -> BattleInfo {
        BattleInfo { id: self.id, body: self.body, place: self.place.clone(), pos: self.pos, start_t: self.start_t, end_t: self.end_t, sides: self.sides.clone(), lost: self.lost.clone() }
    }

    /// What `p` saw of it: only units seen at that moment (allies always), salvos from units they
    /// saw, hits between units they saw. Decoys stay what they pretended to be to enemies.
    pub fn for_player(&self, game: &Game, p: PlayerId) -> BattleRec {
        let b = bit(p);
        let ally = |o: PlayerId| game.allied(p, o);
        let mut r = self.clone();
        for u in r.units.iter_mut() {
            u.seen = 0;
        }
        for f in r.frames.iter_mut() {
            f.u.retain(|x| x.4 & b != 0 || self.units.get(x.0 as usize).is_some_and(|u| ally(u.owner)));
            let shown: Vec<u16> = f.u.iter().map(|x| x.0).collect();
            for x in f.u.iter_mut() {
                x.4 = 0;
            }
            f.s.retain(|s| shown.contains(&s.0) || ally(s.1));
            f.h.retain(|h| shown.contains(&h.2));
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cloned_game_does_not_record() {
        let r = Recorder::on();
        assert!(r.clone().0.is_none());
    }

    #[test]
    fn snapshots_hide_unseen_ships_and_keep_last_known_owners() {
        let mut g = Game::new("T".into(), crate::game::Settings { name: "t".into(), size: crate::system::SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0, teams: 0 }, 7, "a");
        g.add_player("b", None).unwrap();
        let snap = Snap {
            t: 0.0,
            ships: vec![
                SnapShip { id: 1, owner: 0, class: ShipClass::Scout, at: None, pos: [0, 0], looks: None, seen: 0b01 },
                SnapShip { id: 2, owner: 1, class: ShipClass::Decoy, at: None, pos: [0, 0], looks: Some(ShipClass::Dreadnought), seen: 0b10 },
                SnapShip { id: 3, owner: 1, class: ShipClass::Decoy, at: None, pos: [0, 0], looks: Some(ShipClass::Dreadnought), seen: 0b11 },
            ],
            owners: vec![Some(1), Some(0)],
            owner_seen: vec![0b10, 0b11],
            battles: vec![],
        };
        let mut known = vec![];
        let v = snap.for_player(&g, 0, &mut known);
        assert_eq!(v.ships.iter().map(|s| s.id).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(v.ships[1].class, ShipClass::Dreadnought, "a decoy fools the enemy");
        assert_eq!(v.owners, vec![None, Some(0)]);
        let later = Snap { owners: vec![None, Some(0)], owner_seen: vec![0b10, 0b11], ..snap.clone() };
        let snap2 = Snap { owners: vec![Some(1), Some(1)], owner_seen: vec![0b11, 0b10], ..snap };
        let mut known = vec![];
        snap2.for_player(&g, 0, &mut known);
        assert_eq!(later.for_player(&g, 0, &mut known).owners, vec![Some(1), Some(0)], "unseen body keeps what we last saw");
        let v1 = snap2.for_player(&g, 1, &mut vec![]);
        assert_eq!(v1.ships.iter().find(|s| s.id == 2).unwrap().class, ShipClass::Decoy, "the owner knows its decoy");
    }

    #[test]
    fn a_battle_is_recorded_and_only_its_sides_and_watchers_get_it() {
        let (mut g, x) = crate::battle::tests::arena();
        g.recorder = Recorder::on();
        for _ in 0..3 {
            g.spawn(0, ShipClass::Interceptor, x);
            g.spawn(1, ShipClass::Interceptor, x);
        }
        let mut hist = vec![];
        let end = g.t + 4.0 * 3600.0;
        while g.t < end {
            let t = g.t + 30.0;
            g.advance_to(t);
            hist.extend(g.recorder.drain());
        }
        let snaps: Vec<&Snap> = hist.iter().filter_map(|h| if let Hist::S(s) = h { Some(s) } else { None }).collect();
        assert!(snaps.len() >= 20, "{} snapshots in 4 h", snaps.len());
        assert!(snaps.windows(2).skip(1).all(|w| w[1].t - w[0].t >= SNAP_EVERY - 1.0), "{:?}", snaps.iter().map(|s| s.t).collect::<Vec<_>>());
        assert!(snaps.iter().any(|s| !s.battles.is_empty()), "a snapshot during the fight lists it");
        let recs: Vec<&BattleRec> = hist.iter().filter_map(|h| if let Hist::B(b) = h { Some(b) } else { None }).collect();
        assert_eq!(recs.len(), 1);
        let r = recs[0];
        assert!(r.end_t - r.start_t > 300.0 && r.frames.len() > 100, "{} s, {} frames", r.end_t - r.start_t, r.frames.len());
        assert_eq!(r.units.len(), 6);
        assert!(r.frames.iter().any(|f| !f.h.is_empty()), "hits recorded");
        assert!(r.frames.iter().any(|f| !f.s.is_empty()), "salvos recorded");
        // hull only goes down
        let first = &r.frames[0];
        let last = r.frames.last().unwrap();
        assert!(last.u.iter().map(|u| u.3).sum::<f32>() < first.u.iter().map(|u| u.3).sum::<f32>());
        assert!(r.visible_to(&g, 0) && r.visible_to(&g, 1));
        // "c" had nothing there and no sensor on it (the arena's moon is unowned)
        let c_saw = r.units.iter().any(|u| u.seen & 0b100 != 0);
        assert_eq!(r.visible_to(&g, 2), c_saw);
        let mine = r.for_player(&g, 0);
        assert!(mine.frames.iter().all(|f| f.u.iter().all(|x| x.4 == 0)));
        assert!(serde_json::to_string(&Hist::B(r.clone())).unwrap().len() < 2_000_000);
    }
}

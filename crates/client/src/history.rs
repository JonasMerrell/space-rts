//! Rewind + battle replays on the client: the war's history as the server sent it (already cut to
//! what we saw), turned into past map views and replay frames for the UI.
use crate::backend::{model_unit, ship_class};
use sim::game::Weapon;
use sim::replay::{BattleInfo, BattleRec, Frame, Snap};
use sim::{BodyId as SimBody, System};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use ui::model as m;

/// Snapshots either side of a flight that its drawn past course spans.
const PATH_SNAPS: usize = 36;
/// Hits stay drawn this long in a replay (s).
const HIT_SHOWN: f64 = 3.0;

#[derive(Default)]
pub struct Past {
    pub code: String,
    pub snaps: Vec<Snap>,
    pub battles: Vec<BattleInfo>,
    /// a History request is out
    pub waiting: bool,
    pub replays: HashMap<u32, BattleRec>,
    pub replay_wait: HashSet<u32>,
    /// last `view` answer: (snapshot index, view)
    cache: RefCell<Option<(usize, Rc<m::GameView>)>>,
}

impl Past {
    pub fn for_code(code: &str) -> Past {
        Past { code: code.to_string(), ..Default::default() }
    }

    /// Where to ask from next (after the newest snapshot we have).
    pub fn since(&self) -> f64 {
        // (JSON has no -inf; game time starts at 0)
        self.snaps.last().map_or(-1.0, |s| s.t)
    }

    /// A page of history arrived. True = ask for the next page.
    pub fn add(&mut self, snaps: Vec<Snap>, battles: Vec<BattleInfo>, more: bool) -> bool {
        let since = self.since();
        self.snaps.extend(snaps.into_iter().filter(|s| s.t > since));
        self.battles = battles;
        self.waiting = more;
        more
    }

    pub fn info(&self) -> m::HistoryView {
        m::HistoryView {
            start: self.snaps.first().map(|s| s.t),
            end: self.snaps.last().map(|s| s.t),
            loading: self.waiting,
            battles: self
                .battles
                .iter()
                .map(|b| m::PastBattle { id: b.id, place: b.place.clone(), body: b.body.map(|x| x.0), pos: [b.pos[0] as f64, b.pos[1] as f64], start_t: b.start_t, end_t: b.end_t, sides: b.sides.clone(), lost: b.lost.clone() })
                .collect(),
        }
    }

    /// The last snapshot at or before `t`.
    fn index(&self, t: f64) -> Option<usize> {
        let n = self.snaps.partition_point(|s| s.t <= t);
        n.checked_sub(1)
    }

    fn pos_in(&self, sys: &System, k: usize, id: u32, t: f64) -> Option<[f64; 2]> {
        let s = self.snaps.get(k)?.ships.iter().find(|x| x.id == id)?;
        Some(match s.at {
            Some(b) => crate::backend::arr(sys.pos(b, t)),
            None => [s.pos[0] as f64, s.pos[1] as f64],
        })
    }

    /// Where ship `id` was at `t` (flying ships move in a straight line between snapshots).
    pub fn ship_pos(&self, sys: &System, id: u32, t: f64) -> Option<[f64; 2]> {
        let k = self.index(t)?;
        let a = self.snaps[k].ships.iter().find(|x| x.id == id)?;
        if a.at.is_some() {
            return self.pos_in(sys, k, id, t);
        }
        let p = [a.pos[0] as f64, a.pos[1] as f64];
        let Some(next) = self.snaps.get(k + 1) else { return Some(p) };
        match next.ships.iter().find(|x| x.id == id) {
            Some(b) if b.at.is_none() => {
                let f = ((t - self.snaps[k].t) / (next.t - self.snaps[k].t).max(1e-9)).clamp(0.0, 1.0);
                Some([p[0] + (b.pos[0] as f64 - p[0]) * f, p[1] + (b.pos[1] as f64 - p[1]) * f])
            }
            _ => Some(p),
        }
    }

    /// The course a flying ship took around snapshot `k`, as far as we saw it.
    fn path(&self, sys: &System, k: usize, id: u32) -> m::Trajectory {
        let flying = |j: usize| self.snaps[j].ships.iter().any(|x| x.id == id && x.at.is_none());
        let mut lo = k;
        while lo > 0 && k - lo < PATH_SNAPS && flying(lo - 1) {
            lo -= 1;
        }
        let mut hi = k;
        while hi + 1 < self.snaps.len() && hi - k < PATH_SNAPS && flying(hi + 1) {
            hi += 1;
        }
        let points = (lo..=hi).filter_map(|j| Some((self.snaps[j].t, self.pos_in(sys, j, id, self.snaps[j].t)?))).collect();
        m::Trajectory { points, flybys: vec![] }
    }

    /// The map at `t` as we saw it: today's view with the ships and owners of then.
    pub fn view(&self, live: &m::GameView, sys: &System, t: f64) -> Option<Rc<m::GameView>> {
        let k = self.index(t)?;
        if let Some((k0, v)) = &*self.cache.borrow() {
            if *k0 == k {
                return Some(v.clone());
            }
        }
        let snap = &self.snaps[k];
        let mut v = live.clone();
        v.ships = snap
            .ships
            .iter()
            .map(|s| {
                let class = ship_class(s.class);
                let st = s.class.stats();
                m::Ship {
                    id: s.id,
                    name: st.name.to_string(),
                    class,
                    owner: s.owner,
                    count: 1,
                    fuel: 0.0,
                    fuel_max: 0.0,
                    hp: 0.0,
                    hp_max: 0.0,
                    parked_at: s.at.map(|b| b.0),
                    trajectory: s.at.is_none().then(|| self.path(sys, k, s.id)).filter(|p| p.points.len() >= 2),
                    nodes: vec![],
                    destination: None,
                    retreat: false,
                    seen_at: None,
                    planned: None,
                    members: vec![],
                }
            })
            .collect();
        for b in v.bodies.iter_mut() {
            b.owner = snap.owners.get(b.id as usize).copied().flatten();
        }
        let v = Rc::new(v);
        *self.cache.borrow_mut() = Some((k, v.clone()));
        Some(v)
    }

    /// Replay of battle `id` at `t`: positions eased between frames, salvos flown on from theirs,
    /// hits of the last few seconds.
    pub fn replay(&self, id: u32, t: f64) -> Option<m::BattleView> {
        let r = self.replays.get(&id)?;
        let k = r.frames.partition_point(|f| f.t <= t).saturating_sub(1);
        let f = r.frames.get(k)?;
        let next = r.frames.get(k + 1);
        let at = |fr: &Frame, i: u16| fr.u.iter().find(|x| x.0 == i).map(|x| [x.1 as f64, x.2 as f64]);
        let mut units = vec![];
        for x in &f.u {
            let Some(u) = r.units.get(x.0 as usize) else { continue };
            let p0 = [x.1 as f64, x.2 as f64];
            let (pos, vel) = match next.and_then(|n| at(n, x.0).map(|p| (n.t, p))) {
                Some((t1, p1)) if t1 > f.t => {
                    let dt = t1 - f.t;
                    let k = ((t - f.t) / dt).clamp(0.0, 1.0);
                    let v = [(p1[0] - p0[0]) / dt, (p1[1] - p0[1]) / dt];
                    ([p0[0] + (p1[0] - p0[0]) * k, p0[1] + (p1[1] - p0[1]) * k], v)
                }
                _ => (p0, [0.0, 0.0]),
            };
            units.push(m::BattleUnit {
                unit: model_unit(u.unit),
                owner: u.owner,
                label: u.label.clone(),
                class: u.class.map(ship_class),
                fleet: None,
                hp: x.3,
                hp_max: u.hp_max,
                fuel: 0.0,
                fuel_max: 0.0,
                dv_left: 0.0,
                pos,
                vel,
                weapon: match u.weapon {
                    Weapon::None => m::WeaponKind::None,
                    Weapon::Kinetic => m::WeaponKind::Slugs,
                    _ => m::WeaponKind::Laser,
                },
                range: sim::combat::range(u.weapon),
                thrust: 0.0,
                stance: m::Stance::Auto,
                manual: false,
                burn_left: [0.0, 0.0],
                target: None,
                reload_in: 0.0,
            });
        }
        let dt = (t - f.t).max(0.0);
        let salvos = f
            .s
            .iter()
            .map(|s| m::BattleSalvo {
                owner: s.1,
                pos: [s.2 as f64 + s.4 as f64 * dt, s.3 as f64 + s.5 as f64 * dt],
                vel: [s.4 as f64, s.5 as f64],
                aimed_at: model_unit(r.units[s.0 as usize].unit),
                threat: None,
            })
            .collect();
        let mut hits = vec![];
        for fr in r.frames[..=k.min(r.frames.len() - 1)].iter().rev().take_while(|fr| t - fr.t < HIT_SHOWN + sim::replay::FRAME_EVERY_LONG) {
            for h in fr.h.iter().filter(|h| h.0 <= t && t - h.0 < HIT_SHOWN) {
                let (Some(from), Some(to)) = (at(fr, h.1), at(fr, h.2)) else { continue };
                hits.push(m::BattleHit { from, to, beam: h.3, dmg: h.4, age: t - h.0 });
            }
        }
        Some(m::BattleView {
            id,
            body: r.body.map(|b: SimBody| b.0),
            place: r.place.clone(),
            body_radius: r.body_radius,
            start_t: r.start_t,
            now: t,
            units,
            salvos,
            hits,
            burst: sim::battle::BURST,
            lost: r.lost.clone(),
        })
    }
}

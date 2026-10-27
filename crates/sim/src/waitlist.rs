//! Waiting for funds: ship builds, upgrades, stations and research ordered before the player can pay.
//! Each tick (after income) the list is walked in order: an item whose other requirements aren't
//! met yet (no constructor there, lab busy...) is skipped and reserves nothing; the first item the
//! player can't afford stops the walk, so it saves up and nothing behind it spends the money.
//! The same walk with the current income extrapolated gives the projected start times the UI shows.
use crate::game::{Command, EventKind, Game, PlayerId, ShipClass, Upgrade, TICK};
use crate::station::StationKind;
use crate::system::BodyId;
use crate::tech::Tech;
use serde::{Deserialize, Serialize};

/// Most items one player can have waiting.
pub const MAX_WAITING: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Wish {
    Build { body: BodyId, class: ShipClass },
    Upgrade { body: BodyId, upgrade: Upgrade },
    Station { body: BodyId, kind: StationKind },
    Research { body: BodyId, tech: Tech },
}

impl Wish {
    pub fn body(self) -> BodyId {
        match self {
            Wish::Build { body, .. } | Wish::Upgrade { body, .. } | Wish::Station { body, .. } | Wish::Research { body, .. } => body,
        }
    }

    pub fn label(self) -> String {
        match self {
            Wish::Build { class, .. } => class.stats().name.to_string(),
            Wish::Upgrade { upgrade, .. } => upgrade.label().to_string(),
            Wish::Station { kind, .. } => kind.label().to_string(),
            Wish::Research { tech, .. } => tech.label().to_string(),
        }
    }

    /// The command that starts it right away.
    pub fn command(self) -> Command {
        match self {
            Wish::Build { body, class } => Command::Build { body, class },
            Wish::Upgrade { body, upgrade } => Command::Upgrade { body, upgrade },
            Wish::Station { body, kind } => Command::BuildStation { body, kind },
            Wish::Research { body, tech } => Command::Research { body, tech },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Waiting {
    pub id: u32,
    pub wish: Wish,
    /// game time it was queued
    pub since: f64,
}

/// Projection for one waiting item.
#[derive(Clone, Debug, PartialEq)]
pub struct WaitEta {
    pub id: u32,
    pub matter: f64,
    pub energy: f64,
    /// projected start from the current income; None = not while it's blocked / no income
    pub start_t: Option<f64>,
    /// why it can't start regardless of money (it's skipped until that changes)
    pub blocked: Option<String>,
}

impl Game {
    /// Queue `wish` to start once it's affordable. Starts right away if nothing is waiting ahead
    /// of it and it can start now.
    pub(crate) fn wait_for_funds(&mut self, p: PlayerId, wish: Wish) -> Result<(), String> {
        let n = self.bodies.len() as u32;
        if wish.body().0 >= n {
            return Err("no such body".into());
        }
        if let Some(why) = self.wish_obsolete(p, wish, self.waiting_upgrades(p, wish)) {
            return Err(why);
        }
        let pl = &self.players[p as usize];
        if pl.waiting.len() >= MAX_WAITING {
            return Err(format!("at most {MAX_WAITING} items can wait for funds"));
        }
        if pl.waiting.is_empty() && self.wish_blocker(p, wish).is_none() {
            let (m, e) = self.wish_cost(p, wish, 0);
            if pl.matter >= m && pl.energy >= e {
                return self.apply(p, wish.command());
            }
        }
        let pl = &mut self.players[p as usize];
        let id = pl.next_wait;
        pl.next_wait += 1;
        pl.waiting.push(Waiting { id, wish, since: self.t });
        Ok(())
    }

    pub(crate) fn unwait(&mut self, p: PlayerId, id: u32) -> Result<(), String> {
        let w = &mut self.players[p as usize].waiting;
        let i = w.iter().position(|x| x.id == id).ok_or("that item is no longer waiting")?;
        w.remove(i);
        Ok(())
    }

    pub(crate) fn move_wait(&mut self, p: PlayerId, id: u32, to: usize) -> Result<(), String> {
        let w = &mut self.players[p as usize].waiting;
        let i = w.iter().position(|x| x.id == id).ok_or("that item is no longer waiting")?;
        let x = w.remove(i);
        w.insert(to.min(w.len()), x);
        Ok(())
    }

    /// Upgrades of the same kind at the same body already waiting (they raise the level this one buys).
    fn waiting_upgrades(&self, p: PlayerId, wish: Wish) -> u8 {
        self.players[p as usize].waiting.iter().filter(|w| w.wish == wish && matches!(wish, Wish::Upgrade { .. })).count() as u8
    }

    /// Matter and energy `wish` costs, with `ahead` more of the same upgrade bought before it.
    pub fn wish_cost(&self, _p: PlayerId, wish: Wish, ahead: u8) -> (f64, f64) {
        match wish {
            Wish::Build { class, .. } => (class.stats().matter, class.stats().energy),
            Wish::Upgrade { body, upgrade } => {
                let next = self.upgrade_level_after_queue(body, upgrade) + ahead + 1;
                let (m, e, _) = upgrade.cost(next.min(Upgrade::MAX_LEVEL));
                (m, e)
            }
            Wish::Station { kind, .. } => {
                let (m, e, _) = kind.cost();
                (m, e)
            }
            Wish::Research { tech, .. } => {
                let (m, e, _) = tech.cost();
                (m, e)
            }
        }
    }

    fn upgrade_level_after_queue(&self, body: BodyId, u: Upgrade) -> u8 {
        let b = &self.bodies[body.0 as usize];
        let pending = b.queue.iter().filter(|j| matches!(j.kind, crate::game::JobKind::Upgrade(x) if x == u)).count() as u8;
        b.level(u) + pending
    }

    /// Why `wish` can't start right now, money aside (it waits, skipped, until this clears).
    pub fn wish_blocker(&self, p: PlayerId, wish: Wish) -> Option<String> {
        match wish {
            Wish::Build { body, class } => {
                let yard = self.yard_level(p, body);
                if yard == 0 {
                    Some("needs the body or a shipyard station there".into())
                } else if yard < class.stats().min_shipyard {
                    Some(format!("needs shipyard level {}", class.stats().min_shipyard))
                } else {
                    None
                }
            }
            Wish::Upgrade { body, .. } => (self.bodies[body.0 as usize].owner != Some(p)).then(|| "you don't own that body".into()),
            Wish::Station { body, kind } => self.station_blocker(p, body, kind),
            Wish::Research { body, tech } => self.research_blocker(p, body, tech),
        }
    }

    /// `wish` can never start (or is pointless): it's dropped from the list. `ahead` = same upgrades before it.
    pub fn wish_obsolete(&self, p: PlayerId, wish: Wish, ahead: u8) -> Option<String> {
        match wish {
            Wish::Upgrade { body, upgrade } if self.upgrade_level_after_queue(body, upgrade) + ahead >= Upgrade::MAX_LEVEL => {
                Some("already at max level".into())
            }
            Wish::Research { tech, .. } => {
                let pl = &self.players[p as usize];
                if pl.techs.contains(&tech) {
                    Some("already researched".into())
                } else if pl.research.iter().any(|r| r.tech == tech) {
                    Some("already being researched".into())
                } else {
                    None
                }
            }
            Wish::Station { body, .. } if self.system.body(body).is_star() => Some("can't build in orbit of the star".into()),
            Wish::Station { body, kind } if self.stations.iter().any(|s| s.owner == p && s.body == body && s.kind == kind) => {
                Some("you already have one there".into())
            }
            _ => None,
        }
    }

    /// Every tick after income: start what the players can now pay for, in list order.
    pub(crate) fn fund_waiting(&mut self) {
        for p in 0..self.players.len() as PlayerId {
            let mut i = 0;
            while i < self.players[p as usize].waiting.len() {
                let w = self.players[p as usize].waiting[i].clone();
                let name = format!("{} at {}", w.wish.label(), self.system.body(w.wish.body()).name);
                if let Some(why) = self.wish_obsolete(p, w.wish, 0) {
                    self.players[p as usize].waiting.remove(i);
                    self.event(Some(p), EventKind::Info, "Dropped from queue", &format!("{name}: {why}"), Some(w.wish.body()), None);
                    continue;
                }
                if self.wish_blocker(p, w.wish).is_some() {
                    i += 1;
                    continue;
                }
                let (m, e) = self.wish_cost(p, w.wish, 0);
                let pl = &self.players[p as usize];
                if pl.matter < m || pl.energy < e {
                    break; // saving up for this one
                }
                if self.apply(p, w.wish.command()).is_ok() {
                    self.players[p as usize].waiting.remove(i);
                    self.event(Some(p), EventKind::Info, "Queued item started", &name, Some(w.wish.body()), None);
                } else {
                    i += 1;
                }
            }
        }
    }

    /// Net matter and energy income per second from `p`'s bodies right now.
    pub fn income(&self, p: PlayerId) -> (f64, f64) {
        let (mut m, mut e) = (0.0, 0.0);
        for (i, b) in self.bodies.iter().enumerate() {
            if b.owner != Some(p) {
                continue;
            }
            let (pm, pe, pf) = self.production(BodyId(i as u32));
            m += pm;
            e += pe;
            // the refinery burns 0.5 energy per tonne while the tanks aren't full
            if b.fuel < b.fuel_cap() {
                e -= 0.5 * pf;
            }
        }
        (m / 3600.0, e / 3600.0)
    }

    /// Projected start of each waiting item if income stays as it is now (same walk as `fund_waiting`).
    pub fn waiting_eta(&self, p: PlayerId) -> Vec<WaitEta> {
        let Some(pl) = self.players.get(p as usize) else { return vec![] };
        let (rm, re) = self.income(p);
        let (mut m, mut e, mut t) = (pl.matter, pl.energy, self.t);
        let mut stuck = false;
        let mut out = Vec::with_capacity(pl.waiting.len());
        for (k, w) in pl.waiting.iter().enumerate() {
            let ahead = pl.waiting[..k].iter().filter(|x| x.wish == w.wish && matches!(w.wish, Wish::Upgrade { .. })).count() as u8;
            let (cm, ce) = self.wish_cost(p, w.wish, ahead);
            let blocked = self.wish_obsolete(p, w.wish, ahead).or_else(|| self.wish_blocker(p, w.wish));
            let mut eta = WaitEta { id: w.id, matter: cm, energy: ce, start_t: None, blocked };
            if eta.blocked.is_none() && !stuck {
                let wait = |have: f64, need: f64, rate: f64| if have >= need { 0.0 } else if rate > 0.0 { (need - have) / rate } else { f64::INFINITY };
                let dt = wait(m, cm, rm).max(wait(e, ce, re));
                if dt.is_finite() {
                    // money arrives on ticks; the item starts on the first tick it's covered
                    let start = self.last_tick + ((t + dt - self.last_tick) / TICK).ceil().max(1.0) * TICK;
                    m += rm * (start - t) - cm;
                    e += re * (start - t) - ce;
                    t = start;
                    eta.start_t = Some(start);
                } else {
                    stuck = true;
                }
            }
            out.push(eta);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Settings, ShipState};
    use crate::system::SystemSize;

    fn game() -> Game {
        let mut g = Game::new(
            "ABCD".into(),
            Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0 },
            5,
            "alice",
        );
        g.add_player("bob", None).unwrap();
        g.start();
        g
    }

    fn broke(g: &mut Game) {
        g.players[0].matter = 0.0;
        g.players[0].energy = 0.0;
    }

    #[test]
    fn affordable_wish_starts_now() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        g.apply(0, Command::Wait { wish: Wish::Build { body: home, class: ShipClass::Scout } }).unwrap();
        assert!(g.players[0].waiting.is_empty());
        assert_eq!(g.bodies[home.0 as usize].queue.len(), 1);
    }

    #[test]
    fn waits_until_income_covers_it_and_starts_near_the_projection() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        broke(&mut g);
        let wish = Wish::Upgrade { body: home, upgrade: Upgrade::Mining };
        g.apply(0, Command::Wait { wish }).unwrap();
        assert_eq!(g.players[0].waiting.len(), 1);
        assert!(g.bodies[home.0 as usize].queue.is_empty());
        let eta = g.waiting_eta(0);
        let start = eta[0].start_t.expect("home has income");
        assert!(start > g.t);
        g.advance_to(start - 1.0);
        assert_eq!(g.players[0].waiting.len(), 1, "started before it was affordable");
        g.advance_to(start + 1.0);
        assert!(g.players[0].waiting.is_empty(), "didn't start at the projected tick");
        assert_eq!(g.bodies[home.0 as usize].queue.len(), 1);
        assert!(g.players[0].matter >= 0.0 && g.players[0].energy >= 0.0);
    }

    #[test]
    fn the_head_saves_up_and_cheaper_items_behind_it_wait() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        broke(&mut g);
        let big = Wish::Build { body: home, class: ShipClass::Gunship };
        let small = Wish::Build { body: home, class: ShipClass::Scout };
        g.apply(0, Command::Wait { wish: big }).unwrap();
        g.apply(0, Command::Wait { wish: small }).unwrap();
        let eta = g.waiting_eta(0);
        let (a, b) = (eta[0].start_t.unwrap(), eta[1].start_t.unwrap());
        assert!(a <= b);
        g.advance_to(a + 1.0);
        let left: Vec<Wish> = g.players[0].waiting.iter().map(|w| w.wish).collect();
        assert_eq!(left, if a == b { vec![] } else { vec![small] });
        // reorder: the scout first, then cancel it
        let mut g2 = game();
        broke(&mut g2);
        g2.apply(0, Command::Wait { wish: big }).unwrap();
        g2.apply(0, Command::Wait { wish: small }).unwrap();
        let id = g2.players[0].waiting[1].id;
        g2.apply(0, Command::MoveWait { id, to: 0 }).unwrap();
        assert_eq!(g2.players[0].waiting[0].wish, small);
        assert!(g2.waiting_eta(0)[0].start_t.unwrap() <= a);
        g2.apply(0, Command::Unwait { id }).unwrap();
        assert_eq!(g2.players[0].waiting.len(), 1);
        assert!(g2.apply(0, Command::Unwait { id }).is_err());
    }

    #[test]
    fn blocked_items_are_skipped_without_reserving() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        let away = g.system.bodies.iter().find(|b| b.kind == crate::system::BodyKind::Asteroid && g.bodies[b.id.0 as usize].owner.is_none()).unwrap().id;
        broke(&mut g);
        // no constructor at the asteroid: skipped; the scout behind it still gets built
        g.apply(0, Command::Wait { wish: Wish::Station { body: away, kind: StationKind::FuelDepot } }).unwrap();
        g.apply(0, Command::Wait { wish: Wish::Build { body: home, class: ShipClass::Scout } }).unwrap();
        let eta = g.waiting_eta(0);
        assert!(eta[0].blocked.is_some() && eta[0].start_t.is_none());
        g.advance_to(eta[1].start_t.unwrap() + 1.0);
        assert_eq!(g.players[0].waiting.len(), 1);
        assert!(matches!(g.players[0].waiting[0].wish, Wish::Station { .. }));
    }

    #[test]
    fn stacked_upgrades_cost_more_and_drop_past_max() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        broke(&mut g);
        let wish = Wish::Upgrade { body: home, upgrade: Upgrade::Solar };
        let lvl = g.bodies[home.0 as usize].level(Upgrade::Solar);
        for _ in lvl..Upgrade::MAX_LEVEL {
            g.apply(0, Command::Wait { wish }).unwrap();
        }
        assert!(g.apply(0, Command::Wait { wish }).is_err(), "past max level");
        let eta = g.waiting_eta(0);
        for w in eta.windows(2) {
            assert!(w[1].matter >= w[0].matter && w[1].start_t >= w[0].start_t);
        }
    }

    #[test]
    fn deterministic_and_survives_json() {
        let run = || {
            let mut g = game();
            let home = g.players[0].home.unwrap();
            broke(&mut g);
            for class in [ShipClass::Gunship, ShipClass::Scout, ShipClass::Bomber] {
                g.apply(0, Command::Wait { wish: Wish::Build { body: home, class } }).unwrap();
            }
            let g: Game = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
            let mut g = g;
            g.advance_to(g.t + 8.0 * 3600.0);
            (g.players[0].waiting.len(), g.ships.iter().filter(|s| s.owner == 0 && matches!(s.state, ShipState::Parked { .. })).count(), g.players[0].matter.to_bits())
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn other_players_dont_see_my_list() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        broke(&mut g);
        g.apply(0, Command::Wait { wish: Wish::Build { body: home, class: ShipClass::Gunship } }).unwrap();
        assert!(g.view_for(1).players[0].waiting.is_empty());
        assert_eq!(g.view_for(0).players[0].waiting.len(), 1);
    }
}

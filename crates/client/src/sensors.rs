//! Sensors report: built from the player's fogged view (`Game::view_for`), so it only knows what
//! the map shows. NOW = threats, battles in sight, our ships under way; CONTACTS = enemy ships in
//! range, last known positions, what we know about each planet and moon; LOG = events + sensor log.

use crate::backend::fleet_token;
use sim::fog::Contact;
use sim::game::{self as g, Game, ShipState, Upgrade};
use sim::traj::End;
use sim::{BodyId, V2};
use ui::model::{self as m, ReportSection as Sec, When};

/// Build the report for player `me` at game time `now`; log items after `seen_until` count as new.
pub fn report(game: &Game, me: g::PlayerId, now: f64, seen_until: f64) -> m::SensorsReport {
    let mut r = Builder { game, me, now, items: vec![] };
    r.threats_and_battles();
    r.transfers();
    r.contacts();
    r.bodies();
    r.log();
    m::SensorsReport { seen_until, items: r.items }
}

struct Builder<'a> {
    game: &'a Game,
    me: g::PlayerId,
    now: f64,
    items: Vec<m::ReportItem>,
}

/// "3 Gunships, Scout"
fn class_list<'a>(classes: impl Iterator<Item = &'a g::ShipClass>) -> String {
    let mut n: Vec<(&str, usize)> = vec![];
    for c in classes {
        let c = c.stats().name;
        match n.iter_mut().find(|x| x.0 == c) {
            Some(x) => x.1 += 1,
            None => n.push((c, 1)),
        }
    }
    n.iter().map(|(c, k)| if *k > 1 { format!("{k} {c}s") } else { c.to_string() }).collect::<Vec<_>>().join(", ")
}

/// Where a flight ends, if it ends at a body (capture or impact), and when.
fn flight_end(game: &Game, f: &g::Flight) -> Option<(BodyId, f64)> {
    let tr = match &f.cache {
        Some(t) => std::borrow::Cow::Borrowed(t),
        None => std::borrow::Cow::Owned(f.compute(&game.system)),
    };
    match tr.end {
        End::Capture { body, t, .. } | End::Impact { body, t } => Some((body, t)),
        End::Horizon { .. } => None,
    }
}

impl Builder<'_> {
    #[allow(clippy::too_many_arguments)]
    fn push(&mut self, section: Sec, when: When, t: f64, title: impl Into<String>, text: impl Into<String>, focus: Option<m::Focus>, fallback: Option<BodyId>, tone: i8) {
        self.items.push(m::ReportItem {
            t,
            when,
            section,
            title: title.into(),
            text: text.into(),
            focus,
            fallback: fallback.map(|b| m::Focus::Body(b.0)),
            tone,
        });
    }

    fn bname(&self, b: BodyId) -> String {
        self.game.system.body(b).name.clone()
    }

    fn pname(&self, p: g::PlayerId) -> String {
        self.game.players.get(p as usize).map(|p| p.name.clone()).unwrap_or_else(|| "someone".into())
    }

    fn vision(&self) -> Option<&sim::fog::Vision> {
        self.game.players.get(self.me as usize).map(|p| &p.vision)
    }

    fn sees_body(&self, b: BodyId) -> bool {
        self.game.bodies[b.0 as usize].owner == Some(self.me) || self.vision().is_some_and(|v| v.bodies.binary_search(&b).is_ok())
    }

    fn scanned(&self, s: g::ShipId) -> bool {
        self.vision().is_some_and(|v| v.scanned.binary_search(&s).is_ok())
    }

    /// Bodies we'd care about something arriving at: ours, and where our ships are parked.
    fn ours(&self, b: BodyId) -> bool {
        self.game.bodies[b.0 as usize].owner == Some(self.me) || self.game.ships.iter().any(|s| s.owner == self.me && s.parked_at() == Some(b))
    }

    fn nearest_body(&self, p: V2) -> Option<BodyId> {
        let sys = &self.game.system;
        sys.bodies
            .iter()
            .filter(|b| !b.is_star() && !self.game.bodies[b.id.0 as usize].gone)
            .min_by(|a, b| sys.pos(a.id, self.now).dist(p).total_cmp(&sys.pos(b.id, self.now).dist(p)))
            .map(|b| b.id)
    }

    /// Where an enemy flight is going as far as we can tell: its plan if a scanner sees it, else
    /// where its current coast arc ends (the same arc the map draws).
    fn heading(&self, s: &g::Ship) -> Option<(BodyId, f64)> {
        let ShipState::Flying(f) = &s.state else { return None };
        let end = flight_end(self.game, f);
        if self.scanned(s.id) {
            if let Some(d) = f.dest {
                return Some((d, end.filter(|e| e.0 == d).map(|e| e.1).unwrap_or(f64::NAN)));
            }
        }
        end
    }

    fn threats_and_battles(&mut self) {
        let game = self.game;
        let me = self.me;
        // enemy ships on their way to us, grouped per owner and target
        let mut incoming: Vec<(g::PlayerId, BodyId, f64, Vec<&g::Ship>)> = vec![];
        for s in game.ships.iter().filter(|s| s.owner != me) {
            if let Some((b, t)) = self.heading(s).filter(|(b, _)| self.ours(*b)) {
                match incoming.iter_mut().find(|x| x.0 == s.owner && x.1 == b) {
                    Some(x) => {
                        x.2 = x.2.min(t);
                        x.3.push(s);
                    }
                    None => incoming.push((s.owner, b, t, vec![s])),
                }
            }
        }
        for (owner, b, t, ships) in incoming {
            let text = format!("{}'s {} heading for {}", self.pname(owner), class_list(ships.iter().map(|s| &s.class)), self.bname(b));
            let (when, t) = if t.is_finite() { (When::Eta, t) } else { (When::Live, self.now) };
            self.push(Sec::Threats, when, t, "Incoming", text, Some(m::Focus::Ship(ships[0].id.0)), Some(b), -1);
        }
        // last seen coming our way
        let ghosts: Vec<&Contact> = game.players.get(me as usize).map(|p| p.contacts.iter().filter(|c| game.ship(c.ship.id).is_none()).collect()).unwrap_or_default();
        for c in ghosts {
            let ShipState::Flying(f) = &c.ship.state else { continue };
            if let Some((b, t)) = flight_end(game, f).filter(|(b, t)| self.ours(*b) && *t > self.now) {
                let text = format!("{}'s {} was last seen on course for {}", self.pname(c.ship.owner), c.ship.class.stats().name, self.bname(b));
                self.push(Sec::Threats, When::Eta, t, "Possibly incoming", text, Some(m::Focus::Ship(c.ship.id.0)), Some(b), -1);
            }
        }
        // enemy ships sitting in orbit of our bodies
        for b in game.system.bodies.iter().map(|b| b.id).filter(|&b| game.bodies[b.0 as usize].owner == Some(me)) {
            let mut by: Vec<(g::PlayerId, Vec<&g::Ship>)> = vec![];
            for s in game.ships.iter().filter(|s| s.owner != me && s.parked_at() == Some(b)) {
                match by.iter_mut().find(|x| x.0 == s.owner) {
                    Some(x) => x.1.push(s),
                    None => by.push((s.owner, vec![s])),
                }
            }
            for (owner, ships) in by {
                let text = format!("{}'s {} in orbit of {}", self.pname(owner), class_list(ships.iter().map(|s| &s.class)), self.bname(b));
                self.push(Sec::Threats, When::Live, self.now, "In orbit", text, Some(m::Focus::Body(b.0)), Some(b), -1);
            }
        }
        // asteroids pushed onto our bodies, and drives being fitted for it (only those we can see)
        for d in &game.drifts {
            if let End::Impact { body, t } = d.traj.end {
                if self.ours(body) && t > self.now {
                    let text = format!("{} is on course to hit {}", self.bname(d.body), self.bname(body));
                    self.push(Sec::Threats, When::Eta, t, "Asteroid strike", text, Some(m::Focus::Body(d.body.0)), Some(body), -1);
                }
            }
        }
        for d in game.drives.iter().filter(|d| d.owner != me) {
            if let Some(target) = d.target.filter(|&b| self.ours(b)) {
                let text = format!("{} is fitting a drive to {} aimed at {}", self.pname(d.owner), self.bname(d.body), self.bname(target));
                self.push(Sec::Threats, When::Eta, d.burn.t - d.burn_s / 2.0, "Asteroid drive", text, Some(m::Focus::Body(d.body.0)), Some(target), -1);
            }
        }
        // fighting we can see right now
        for b in game.system.bodies.iter().map(|b| b.id) {
            let st = &game.bodies[b.0 as usize];
            if !st.in_combat || !self.sees_body(b) {
                continue;
            }
            let mut sides: Vec<g::PlayerId> = st.owner.into_iter().collect();
            for s in game.ships.iter().filter(|s| s.parked_at() == Some(b)) {
                if !sides.contains(&s.owner) {
                    sides.push(s.owner);
                }
            }
            let who = sides.iter().map(|&p| if p == me { "you".to_string() } else { self.pname(p) }).collect::<Vec<_>>().join(", ");
            let text = if who.is_empty() { format!("Fighting at {}", self.bname(b)) } else { format!("Fighting at {} ({who})", self.bname(b)) };
            let tone = if sides.contains(&me) { -1 } else { 0 };
            self.push(Sec::Battles, When::Live, self.now, "Fighting", text, Some(m::Focus::Body(b.0)), Some(b), tone);
        }
    }

    fn transfers(&mut self) {
        let game = self.game;
        let me = self.me;
        let mut done_fleets = vec![];
        for s in game.ships.iter().filter(|s| s.owner == me) {
            let fleet = game.fleets.iter().find(|f| f.owner == me && f.ships.contains(&s.id));
            if let Some(f) = fleet {
                if done_fleets.contains(&f.id) {
                    continue;
                }
                done_fleets.push(f.id);
            }
            let (name, focus) = match fleet {
                Some(f) => (f.name.clone(), fleet_token(f)),
                None => (s.name.clone(), s.id.0),
            };
            match &s.state {
                ShipState::Flying(f) => {
                    let end = flight_end(game, f);
                    let (text, when, t) = match (f.dest, end) {
                        (Some(d), Some((b, t))) if b == d => (format!("{name} heading for {}", self.bname(d)), When::Eta, t),
                        (Some(d), _) => (format!("{name} heading for {}", self.bname(d)), When::Live, self.now),
                        (None, Some((b, t))) => (format!("{name} coasting toward {}", self.bname(b)), When::Eta, t),
                        (None, None) => (format!("{name} coasting through space"), When::Live, self.now),
                    };
                    self.push(Sec::Transfers, when, t, "Under way", text, Some(m::Focus::Ship(focus)), f.dest, 1);
                }
                ShipState::Parked { body, departure: Some(d) } => {
                    let text = format!("{name} leaves {} for {}", self.bname(*body), self.bname(d.to));
                    self.push(Sec::Transfers, When::Eta, d.state.t, "Departs", text, Some(m::Focus::Ship(focus)), Some(*body), 1);
                }
                _ => {}
            }
        }
    }

    fn contacts(&mut self) {
        let game = self.game;
        let me = self.me;
        // live: per owner and place (in orbit of X / in flight near X, heading for Y)
        let mut groups: Vec<((g::PlayerId, Option<BodyId>, Option<BodyId>, Option<BodyId>), Vec<&g::Ship>)> = vec![];
        for s in game.ships.iter().filter(|s| s.owner != me) {
            let parked = s.parked_at();
            let (near, to) = match parked {
                Some(_) => (None, None),
                None => (self.nearest_body(game.ship_pos(s, self.now)), self.heading(s).map(|h| h.0)),
            };
            let key = (s.owner, parked, near, to);
            match groups.iter_mut().find(|x| x.0 == key) {
                Some(x) => x.1.push(s),
                None => groups.push((key, vec![s])),
            }
        }
        for ((owner, parked, near, to), ships) in groups {
            let list = class_list(ships.iter().map(|s| &s.class));
            let text = match (parked, near, to) {
                (Some(b), _, _) => format!("{list} in orbit of {}", self.bname(b)),
                (None, Some(n), Some(t)) => format!("{list} in flight near {}, heading for {}", self.bname(n), self.bname(t)),
                (None, Some(n), None) => format!("{list} in flight near {}", self.bname(n)),
                _ => format!("{list} in deep space"),
            };
            let scanned = ships.iter().any(|s| self.scanned(s.id));
            let text = if scanned { format!("{text} · plans scanned") } else { text };
            self.push(Sec::Ships, When::Live, self.now, self.pname(owner), text, Some(m::Focus::Ship(ships[0].id.0)), parked.or(near), -1);
        }
        // ghosts: last sightings of ships out of range now
        let ghosts: Vec<&Contact> = game.players.get(me as usize).map(|p| p.contacts.iter().filter(|c| game.ship(c.ship.id).is_none()).collect()).unwrap_or_default();
        let mut groups: Vec<((g::PlayerId, Option<BodyId>), Vec<&Contact>)> = vec![];
        for c in ghosts {
            let near = c.ship.parked_at().or_else(|| self.nearest_body(game.ship_pos(&c.ship, c.seen_t)));
            match groups.iter_mut().find(|x| x.0 == (c.ship.owner, near)) {
                Some(x) => x.1.push(c),
                None => groups.push(((c.ship.owner, near), vec![c])),
            }
        }
        groups.sort_by(|a, b| b.1.iter().map(|c| c.seen_t).fold(0.0, f64::max).total_cmp(&a.1.iter().map(|c| c.seen_t).fold(0.0, f64::max)));
        for ((owner, near), cs) in groups {
            let seen = cs.iter().map(|c| c.seen_t).fold(0.0, f64::max);
            let place = near.map(|b| format!("near {}", self.bname(b))).unwrap_or_else(|| "in deep space".into());
            let text = format!("{}, last seen {place}", class_list(cs.iter().map(|c| &c.ship.class)));
            self.push(Sec::Ghosts, When::Seen, seen, self.pname(owner), text, Some(m::Focus::Ship(cs[0].ship.id.0)), near, -1);
        }
    }

    fn bodies(&mut self) {
        let game = self.game;
        let me = self.me;
        let intel = game.players.get(me as usize).map(|p| p.intel.as_slice()).unwrap_or_default();
        let mut rows: Vec<(u8, String, m::ReportItem)> = vec![];
        let mut unknown = vec![];
        for b in game.system.bodies.iter().filter(|b| !b.is_star()) {
            let i = b.id.0 as usize;
            let st = &game.bodies[i];
            if st.gone || st.owner == Some(me) {
                continue;
            }
            let live = self.sees_body(b.id);
            let known = intel.get(i).and_then(|x| x.as_ref());
            if !live && known.is_none() {
                unknown.push(b.name.clone());
                continue;
            }
            let mut parts = vec![match st.owner {
                Some(o) => format!("{}'s", self.pname(o)),
                None if st.shattered => "Shattered".into(),
                None => "Unclaimed".into(),
            }];
            let levels: Vec<String> = Upgrade::ALL.iter().filter(|&&u| st.level(u) > 0).map(|&u| format!("{u:?} {}", st.level(u))).collect();
            if !levels.is_empty() {
                parts.push(levels.join(", "));
            }
            let stations = game.stations.iter().filter(|s| s.body == b.id && s.owner != me).count();
            if stations > 0 {
                parts.push(if stations == 1 { "1 station".into() } else { format!("{stations} stations") });
            }
            if live {
                let ships: Vec<&g::Ship> = game.ships.iter().filter(|s| s.owner != me && s.parked_at() == Some(b.id)).collect();
                parts.push(if ships.is_empty() { "nothing in orbit".into() } else { format!("{} in orbit", class_list(ships.iter().map(|s| &s.class))) });
            } else if let Some(k) = known {
                let power: f64 = k.power.iter().filter(|p| p.0 != me).map(|p| p.1).sum();
                parts.push(if power > 0.0 { format!("armed ships in orbit (power {power:.0})") } else { "nothing seen in orbit".into() });
            }
            let hostile = st.owner.is_some();
            let (when, t) = if live { (When::Live, self.now) } else { (When::Seen, known.map(|k| k.seen_t).unwrap_or(self.now)) };
            let item = m::ReportItem {
                t,
                when,
                section: Sec::Bodies,
                title: b.name.clone(),
                text: parts.join(" · "),
                focus: Some(m::Focus::Body(b.id.0)),
                fallback: None,
                tone: if hostile { -1 } else { 0 },
            };
            // others' bodies first, then unclaimed; live before stale
            rows.push(((!hostile as u8) * 2 + (!live as u8), b.name.clone(), item));
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        self.items.extend(rows.into_iter().map(|r| r.2));
        if !unknown.is_empty() {
            self.push(Sec::Bodies, When::Never, self.now, "Never scanned", unknown.join(", "), None, None, 0);
        }
    }

    fn log(&mut self) {
        for e in self.game.away_report(self.me, f64::NEG_INFINITY, 300) {
            let tone = match e.kind {
                g::EventKind::Good => 1,
                g::EventKind::Bad | g::EventKind::Battle => -1,
                g::EventKind::Sensor if e.title == "Contact" || e.title == "Engine burn" => -1,
                _ => 0,
            };
            let focus = e.ship_ref.map(|s| m::Focus::Ship(s.0)).or(e.body_ref.map(|b| m::Focus::Body(b.0)));
            self.push(Sec::Log, When::At, e.t, e.title, e.body, focus, e.body_ref, tone);
        }
    }
}

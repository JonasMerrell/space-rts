//! Fleets: named groups of a player's ships that plan, burn and retreat as one.
//!
//! A fleet always has at least two members, all parked at the same body or flying the same
//! course. Per-ship commands (`Transfer`, `SetBurns`, `CancelDeparture`, `SetOrder`) on any member
//! apply to the whole fleet; `fleet_tick` drops members that got separated (e.g. one couldn't brake).
use serde::{Deserialize, Serialize};

use crate::game::{EventKind, Game, PlayerId, ShipClass, ShipId, ShipState};
use crate::BodyId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FleetId(pub u32);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fleet {
    pub id: FleetId,
    pub owner: PlayerId,
    pub name: String,
    pub ships: Vec<ShipId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FleetCommand {
    /// Group parked ships at one body into a new fleet (members leave their old fleets).
    Form { ships: Vec<ShipId>, name: String },
    /// Add parked ships to a fleet parked at the same body.
    Join { fleet: FleetId, ships: Vec<ShipId> },
    /// Take ships out of their fleets; with `split`, two or more leavers form a new fleet of that name.
    Leave { ships: Vec<ShipId>, split: Option<String> },
    /// Move every ship of `from` into `into` (both parked at the same body).
    Merge { into: FleetId, from: FleetId },
    Rename { fleet: FleetId, name: String },
    Disband { fleet: FleetId },
}

/// Fuel tanker reserve, same rule as tankers refuelling parked ships.
pub const TANKER_RESERVE: f64 = 0.3;
/// Tonnes per hour a tanker can pump into one fleetmate in flight.
const PUMP_RATE: f64 = 300.0;

/// Where a ship is, for "are these ships together" checks.
#[derive(PartialEq)]
enum Place {
    Parked(BodyId),
    Flying { t: u64, x: u64, y: u64, dest: Option<BodyId> },
}

fn clean_name(name: &str, fallback: String) -> String {
    let n: String = name.trim().chars().filter(|c| !c.is_control()).take(32).collect();
    if n.is_empty() {
        fallback
    } else {
        n
    }
}

impl Game {
    pub fn fleet(&self, id: FleetId) -> Option<&Fleet> {
        self.fleets.iter().find(|f| f.id == id)
    }

    pub fn fleet_of(&self, ship: ShipId) -> Option<&Fleet> {
        self.fleets.iter().find(|f| f.ships.contains(&ship))
    }

    /// `ship` and its fleetmates that still exist (just `ship` when it isn't in a fleet).
    pub fn with_fleetmates(&self, ship: ShipId) -> Vec<ShipId> {
        match self.fleet_of(ship) {
            Some(f) => f.ships.iter().copied().filter(|&s| self.ship(s).is_some()).collect(),
            None => vec![ship],
        }
    }

    /// Expand a ship list to whole fleets, keeping order and dropping duplicates.
    pub fn expand_fleets(&self, ships: &[ShipId]) -> Vec<ShipId> {
        let mut out: Vec<ShipId> = vec![];
        for &s in ships {
            for m in self.with_fleetmates(s) {
                if !out.contains(&m) {
                    out.push(m);
                }
            }
        }
        out
    }

    /// Δv the fleet can still fly (its weakest ship's).
    pub fn fleet_dv_left(&self, id: FleetId) -> f64 {
        let Some(f) = self.fleet(id) else { return 0.0 };
        f.ships.iter().filter_map(|&s| self.ship(s)).map(|s| s.dv_left()).fold(f64::INFINITY, f64::min)
    }

    fn place(&self, ship: ShipId) -> Option<Place> {
        let s = self.ship(ship)?;
        Some(match &s.state {
            ShipState::Parked { body, .. } => Place::Parked(*body),
            ShipState::Flying(f) => Place::Flying { t: f.start.t.to_bits(), x: f.start.r.x.to_bits(), y: f.start.r.y.to_bits(), dest: f.dest },
        })
    }

    /// All ships owned by `player`, existing and parked at one body; returns that body.
    fn parked_together(&self, player: PlayerId, ships: &[ShipId]) -> Result<BodyId, String> {
        let mut at = None;
        for &id in ships {
            let s = self.ship(id).ok_or("no such ship")?;
            if s.owner != player {
                return Err("not your ship".into());
            }
            let b = s.parked_at().ok_or_else(|| format!("{} must be parked in orbit to change fleets", s.name))?;
            match at {
                None => at = Some(b),
                Some(a) if a != b => return Err("ships must be parked at the same body to form a fleet".into()),
                _ => {}
            }
        }
        at.ok_or_else(|| "no ships given".into())
    }

    /// Members of a newly joined group must share one plan: keep it if they all already have the
    /// same scheduled departure, otherwise cancel everyone's.
    fn unify_departures(&mut self, ships: &[ShipId]) {
        let key = |g: &Game, id: ShipId| match &g.ship(id).map(|s| &s.state) {
            Some(ShipState::Parked { departure: Some(d), .. }) => Some((d.to, d.state.t.to_bits(), d.state.v.x.to_bits(), d.state.v.y.to_bits(), g.ship(id).unwrap().route.clone())),
            _ => None,
        };
        let first = key(self, ships[0]);
        if ships.iter().all(|&s| key(self, s) == first) {
            return;
        }
        for &id in ships {
            if let Some(s) = self.ship_mut(id) {
                if let ShipState::Parked { departure, .. } = &mut s.state {
                    *departure = None;
                }
                s.route.clear();
            }
        }
    }

    fn remove_from_fleets(&mut self, ships: &[ShipId]) {
        for f in self.fleets.iter_mut() {
            f.ships.retain(|s| !ships.contains(s));
        }
        self.fleets.retain(|f| f.ships.len() >= 2);
    }

    fn new_fleet(&mut self, player: PlayerId, ships: Vec<ShipId>, name: &str) -> FleetId {
        let id = FleetId(self.next_fleet);
        self.next_fleet += 1;
        let name = clean_name(name, format!("Fleet {}", id.0 + 1));
        // standing orders are fleet-wide: everyone takes the first ship's
        let order = self.ship(ships[0]).map(|s| s.order);
        for &s in &ships {
            if let (Some(o), Some(sh)) = (order, self.ship_mut(s)) {
                sh.order = o;
            }
        }
        self.fleets.push(Fleet { id, owner: player, name, ships });
        id
    }

    fn own_fleet(&self, player: PlayerId, id: FleetId) -> Result<&Fleet, String> {
        let f = self.fleet(id).ok_or("no such fleet")?;
        if f.owner != player {
            return Err("not your fleet".into());
        }
        Ok(f)
    }

    pub(crate) fn fleet_command(&mut self, player: PlayerId, cmd: FleetCommand) -> Result<(), String> {
        match cmd {
            FleetCommand::Form { ships, name } => {
                let mut ships = ships;
                ships.dedup();
                let ships = self.expand_fleets(&ships);
                if ships.len() < 2 {
                    return Err("a fleet needs at least two ships".into());
                }
                self.parked_together(player, &ships)?;
                self.remove_from_fleets(&ships);
                self.unify_departures(&ships);
                self.new_fleet(player, ships, &name);
                Ok(())
            }
            FleetCommand::Join { fleet, ships } => {
                let f = self.own_fleet(player, fleet)?;
                let name = f.name.clone();
                let mut all = f.ships.clone();
                for s in ships {
                    if !all.contains(&s) {
                        all.push(s);
                    }
                }
                self.parked_together(player, &all)?;
                let order = self.ship(all[0]).map(|s| s.order);
                // joiners leave their old fleets; the target fleet keeps its id and name
                self.fleets.retain(|f| f.id != fleet);
                self.remove_from_fleets(&all);
                self.unify_departures(&all);
                for &s in &all {
                    if let (Some(o), Some(sh)) = (order, self.ship_mut(s)) {
                        sh.order = o;
                    }
                }
                self.fleets.push(Fleet { id: fleet, owner: player, name, ships: all });
                self.fleets.sort_by_key(|f| f.id);
                Ok(())
            }
            FleetCommand::Leave { ships, split } => {
                for &s in &ships {
                    let sh = self.ship(s).ok_or("no such ship")?;
                    if sh.owner != player {
                        return Err("not your ship".into());
                    }
                }
                self.remove_from_fleets(&ships);
                if let Some(name) = split {
                    let mut ships = ships;
                    ships.dedup();
                    // leavers fly the same course already, so no plan needs unifying in flight
                    if ships.len() >= 2 && ships.iter().all(|&s| self.place(s) == self.place(ships[0])) {
                        self.new_fleet(player, ships, &name);
                    }
                }
                Ok(())
            }
            FleetCommand::Merge { into, from } => {
                if into == from {
                    return Err("that's the same fleet".into());
                }
                let mut all = self.own_fleet(player, into)?.ships.clone();
                all.extend(self.own_fleet(player, from)?.ships.iter().copied());
                self.parked_together(player, &all)?;
                self.unify_departures(&all);
                let order = self.ship(all[0]).map(|s| s.order);
                for &s in &all {
                    if let (Some(o), Some(sh)) = (order, self.ship_mut(s)) {
                        sh.order = o;
                    }
                }
                self.fleets.retain(|f| f.id != from);
                if let Some(f) = self.fleets.iter_mut().find(|f| f.id == into) {
                    f.ships = all;
                }
                Ok(())
            }
            FleetCommand::Rename { fleet, name } => {
                let fallback = self.own_fleet(player, fleet)?.name.clone();
                let name = clean_name(&name, fallback);
                if let Some(f) = self.fleets.iter_mut().find(|f| f.id == fleet) {
                    f.name = name;
                }
                Ok(())
            }
            FleetCommand::Disband { fleet } => {
                self.own_fleet(player, fleet)?;
                self.fleets.retain(|f| f.id != fleet);
                Ok(())
            }
        }
    }

    /// Fleet-wide fuel check for a transfer: every member must afford the departure itself; the
    /// capture burn may be covered by the fleet's tankers in flight. The weakest ship decides.
    pub fn fleet_fuel_check(&self, ships: &[ShipId], depart_dv: f64, capture_dv: f64) -> Result<(), String> {
        let mut deficit = 0.0;
        let mut spare = 0.0;
        let mut worst: Option<(f64, String)> = None;
        for &id in ships {
            let Some(s) = self.ship(id) else { continue };
            let dep = s.fuel_for(depart_dv);
            if dep > s.fuel {
                return Err(format!("not enough fuel: departure needs {:.1} km/s, {} has {:.1} km/s", depart_dv, s.name, s.dv_left()));
            }
            let cap = s.fuel_for_with(s.fuel - dep, capture_dv);
            let left = s.fuel - dep - cap;
            if s.class == ShipClass::Tanker {
                spare += (left - TANKER_RESERVE * s.stats().fuel_cap).max(0.0);
            } else if left < 0.0 {
                deficit += -left;
                if worst.as_ref().map_or(true, |w| -left > w.0) {
                    worst = Some((-left, s.name.clone()));
                }
            }
        }
        if deficit > spare + 1e-9 {
            let (short, name) = worst.unwrap();
            return Err(format!("not enough fuel: {name} is {short:.0} t short for the capture burn (fleet tankers can spare {spare:.0} t)"));
        }
        Ok(())
    }

    /// Once a tick: forget dead ships, split off separated members, and let tankers in flying
    /// fleets refuel their fleetmates.
    pub(crate) fn fleet_tick(&mut self, dt: f64) {
        let mut separated: Vec<(PlayerId, ShipId, String)> = vec![];
        for fi in 0..self.fleets.len() {
            let ships: Vec<ShipId> = self.fleets[fi].ships.iter().copied().filter(|&s| self.ship(s).is_some()).collect();
            // keep the biggest group of members that are together (ties: the one with the first ship)
            let mut best: Vec<ShipId> = vec![];
            for &a in &ships {
                let pa = self.place(a);
                let group: Vec<ShipId> = ships.iter().copied().filter(|&b| self.place(b) == pa).collect();
                if group.len() > best.len() {
                    best = group;
                }
            }
            for &s in ships.iter().filter(|s| !best.contains(s)) {
                separated.push((self.fleets[fi].owner, s, self.fleets[fi].name.clone()));
            }
            self.fleets[fi].ships = best;
        }
        for (owner, s, fname) in &separated {
            let name = self.ship(*s).map(|s| s.name.clone()).unwrap_or_default();
            self.event(Some(*owner), EventKind::Bad, "Fleet split", &format!("{name} lost contact with {fname}"), None, Some(*s));
        }
        self.fleets.retain(|f| f.ships.len() >= 2);

        let h = dt / 3600.0;
        for fi in 0..self.fleets.len() {
            let members: Vec<usize> = self.fleets[fi]
                .ships
                .iter()
                .filter_map(|&id| self.ships.iter().position(|s| s.id == id))
                .filter(|&i| matches!(self.ships[i].state, ShipState::Flying(_)))
                .collect();
            let (tankers, others): (Vec<usize>, Vec<usize>) = members.iter().partition(|&&i| self.ships[i].class == ShipClass::Tanker);
            for &t in &tankers {
                for &j in &others {
                    let room = self.ships[j].stats().fuel_cap - self.ships[j].fuel;
                    let spare = (self.ships[t].fuel - TANKER_RESERVE * self.ships[t].stats().fuel_cap).max(0.0);
                    let x = room.min(spare).min(PUMP_RATE * h).max(0.0);
                    self.ships[t].fuel -= x;
                    self.ships[j].fuel += x;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Command, Settings, StandingOrder};
    use crate::{BodyKind, SystemSize};

    fn game() -> Game {
        let mut g = Game::new(
            "FLT1".into(),
            Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0 },
            5,
            "alice",
        );
        g.add_player("bob", None).unwrap();
        g.start();
        g
    }

    fn near_planet(g: &Game) -> BodyId {
        let home = g.players[0].home.unwrap();
        let ha = g.system.body(home).orbit.unwrap().a;
        g.system
            .bodies
            .iter()
            .filter(|b| b.kind == BodyKind::Planet && g.bodies[b.id.0 as usize].owner.is_none())
            .min_by(|a, b| (a.orbit.unwrap().a - ha).abs().total_cmp(&(b.orbit.unwrap().a - ha).abs()))
            .unwrap()
            .id
    }

    fn mine(g: &Game, class: ShipClass) -> Vec<ShipId> {
        g.ships.iter().filter(|s| s.owner == 0 && s.class == class).map(|s| s.id).collect()
    }

    fn form(g: &mut Game, ships: Vec<ShipId>) -> FleetId {
        g.apply(0, Command::Fleet(FleetCommand::Form { ships, name: " Red Five ".into() })).unwrap();
        g.fleets.last().unwrap().id
    }

    #[test]
    fn form_split_merge_rename_disband() {
        let mut g = game();
        let gs = mine(&g, ShipClass::Gunship);
        let ic = mine(&g, ShipClass::Interceptor);
        let a = form(&mut g, vec![gs[0], gs[1], ic[0]]);
        assert_eq!(g.fleet(a).unwrap().name, "Red Five");
        assert_eq!(g.with_fleetmates(ic[0]).len(), 3);
        // one ship can't be a fleet; enemy ships can't be grouped
        assert!(g.apply(0, Command::Fleet(FleetCommand::Form { ships: vec![ic[1]], name: String::new() })).is_err());
        let enemy = g.ships.iter().find(|s| s.owner == 1).unwrap().id;
        assert!(g.apply(0, Command::Fleet(FleetCommand::Join { fleet: a, ships: vec![enemy] })).is_err());
        // split two off into a new fleet
        g.apply(0, Command::Fleet(FleetCommand::Leave { ships: vec![gs[1], ic[0]], split: Some("B".into()) })).unwrap();
        assert_eq!(g.fleets.len(), 1, "a one-ship fleet dissolves");
        let b = g.fleet_of(gs[1]).unwrap().id;
        assert!(g.fleet_of(gs[0]).is_none());
        g.apply(0, Command::Fleet(FleetCommand::Join { fleet: b, ships: vec![gs[0], ic[1]] })).unwrap();
        assert_eq!(g.fleet(b).unwrap().ships.len(), 4);
        let rest: Vec<ShipId> = mine(&g, ShipClass::Bomber).into_iter().chain(mine(&g, ShipClass::Scout)).collect();
        let c = form(&mut g, rest);
        g.apply(0, Command::Fleet(FleetCommand::Merge { into: b, from: c })).unwrap();
        assert!(g.fleet(c).is_none());
        assert_eq!(g.fleet(b).unwrap().ships.len(), 6);
        g.apply(0, Command::Fleet(FleetCommand::Rename { fleet: b, name: "Home Guard".into() })).unwrap();
        assert_eq!(g.fleet(b).unwrap().name, "Home Guard");
        assert!(g.apply(1, Command::Fleet(FleetCommand::Disband { fleet: b })).is_err());
        g.apply(0, Command::Fleet(FleetCommand::Disband { fleet: b })).unwrap();
        assert!(g.fleets.is_empty());
    }

    #[test]
    fn fleet_flies_as_one_and_arrives_together() {
        let mut g = game();
        let target = near_planet(&g);
        let gs = mine(&g, ShipClass::Gunship);
        let tk = mine(&g, ShipClass::Tanker);
        let f = form(&mut g, vec![gs[0], gs[1], tk[0]]);
        g.apply(0, Command::SetOrder { ship: gs[0], order: StandingOrder::Retreat }).unwrap();
        assert!(g.with_fleetmates(gs[0]).iter().all(|&s| g.ship(s).unwrap().order == StandingOrder::Retreat));
        // ordering one member sends the whole fleet
        g.apply(0, Command::Transfer { ships: vec![gs[1]], to: target, depart_t: None }).unwrap();
        let dep = match &g.ship(gs[0]).unwrap().state {
            ShipState::Parked { departure: Some(d), .. } => d.state.t,
            _ => panic!("fleetmate not scheduled"),
        };
        g.advance_to(dep + 600.0);
        assert_eq!(g.fleet(f).unwrap().ships.len(), 3, "still together in flight");
        g.advance_to(dep + 6.0 * 86_400.0);
        for &s in &[gs[0], gs[1], tk[0]] {
            assert_eq!(g.ship(s).unwrap().parked_at(), Some(target));
        }
        assert_eq!(g.fleet(f).unwrap().ships.len(), 3);
        // others don't see our fleets
        assert!(g.view_for(1).fleets.is_empty());
        assert_eq!(g.view_for(0).fleets.len(), 1);
    }

    #[test]
    fn weakest_ship_limits_the_fleet_and_rejection_schedules_nothing() {
        let mut g = game();
        let target = near_planet(&g);
        let gs = mine(&g, ShipClass::Gunship);
        form(&mut g, vec![gs[0], gs[1]]);
        g.ship_mut(gs[1]).unwrap().fuel = 1.0;
        let err = g.apply(0, Command::Transfer { ships: vec![gs[0]], to: target, depart_t: None }).unwrap_err();
        assert!(err.contains("fuel"), "{err}");
        assert!(g.ships.iter().all(|s| matches!(s.state, ShipState::Parked { departure: None, .. })));
        // plain multi-ship transfers are all-or-nothing too
        let ic = mine(&g, ShipClass::Interceptor);
        g.ship_mut(ic[1]).unwrap().fuel = 0.0;
        assert!(g.apply(0, Command::Transfer { ships: vec![ic[0], ic[1]], to: target, depart_t: None }).is_err());
        assert!(matches!(g.ship(ic[0]).unwrap().state, ShipState::Parked { departure: None, .. }));
    }

    #[test]
    fn tankers_cover_the_capture_burn_and_refuel_in_flight() {
        let mut g = game();
        let target = near_planet(&g);
        let gs = mine(&g, ShipClass::Gunship)[0];
        let tk = mine(&g, ShipClass::Tanker)[0];
        let (p, _) = g.plan_for(&g.ship(gs).unwrap().clone(), target, None).unwrap();
        // enough for the departure but not the capture
        let s = g.ship(gs).unwrap().clone();
        let fuel = s.fuel_for_with(0.0, p.depart_dv) * 1.6 + 5.0;
        g.ship_mut(gs).unwrap().fuel = fuel;
        assert!(g.fleet_fuel_check(&[gs], p.depart_dv, p.capture_dv).is_err());
        assert!(g.fleet_fuel_check(&[gs, tk], p.depart_dv, p.capture_dv).is_ok());
        form(&mut g, vec![gs, tk]);
        g.apply(0, Command::Transfer { ships: vec![gs], to: target, depart_t: None }).unwrap();
        g.advance_to(p.depart_t() + 3.0 * 3600.0);
        let after_burn = fuel - s.fuel_for_with(fuel, p.depart_dv);
        assert!(g.ship(gs).unwrap().fuel > after_burn + 100.0, "tanker pumped fuel in flight");
    }

    #[test]
    fn diverged_member_leaves_the_fleet() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        let other = near_planet(&g);
        let mut gs = mine(&g, ShipClass::Gunship);
        gs.push(mine(&g, ShipClass::Interceptor)[0]);
        let f = form(&mut g, vec![gs[0], gs[1], gs[2]]);
        // teleport one (as if it failed to brake / was moved by other means)
        if let ShipState::Parked { body, .. } = &mut g.ship_mut(gs[2]).unwrap().state {
            assert_eq!(*body, home);
            *body = other;
        }
        g.advance_to(120.0);
        assert_eq!(g.fleet(f).unwrap().ships, vec![gs[0], gs[1]]);
        assert!(g.events.iter().any(|e| e.title == "Fleet split"));
    }
}

//! The shared research tree: 5 branches × 4 tiers. Everyone researches from the same tree
//! (factions are only aesthetic). A tech is researched at an owned body with a Research lab;
//! each lab body runs one project at a time, separately from its ship/upgrade queue.
//! The whole tree is ~130 lab-hours, so one lab finishes it in about a week.
use crate::game::{Game, PlayerId, Upgrade};
use crate::system::BodyId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tech {
    // weapons
    HardenedSlugs,
    FocusedLasers,
    FireControl,
    SpinalLance,
    // engines / range
    EfficientNozzles,
    ExpandedTanks,
    IonDrives,
    AsteroidDrives,
    // industry
    DeepCoreMining,
    FusionPlants,
    ModularYards,
    AutoRefineries,
    // defense
    ArmorPlating,
    PlanetaryShields,
    PointDefense,
    DefensePlatforms,
    // scanning
    BetterOptics,
    ScanningStations,
    SignatureDamping,
    DeepSpaceArray,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Branch {
    Weapons,
    Engines,
    Industry,
    Defense,
    Scanning,
}

impl Branch {
    pub const ALL: [Branch; 5] = [Branch::Weapons, Branch::Engines, Branch::Industry, Branch::Defense, Branch::Scanning];
    pub fn label(self) -> &'static str {
        match self {
            Branch::Weapons => "Weapons",
            Branch::Engines => "Engines",
            Branch::Industry => "Industry",
            Branch::Defense => "Defense",
            Branch::Scanning => "Scanning",
        }
    }
}

/// One research project in progress at a lab.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Research {
    pub tech: Tech,
    pub body: BodyId,
    pub done_t: f64,
}

/// Research effects as multipliers (1.0 = no change). Other systems read these through `Game::mods`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mods {
    pub kinetic_dmg: f64,
    pub energy_dmg: f64,
    /// kinetic hit chance at high relative velocity
    pub accuracy: f64,
    /// ship exhaust velocity
    pub ve: f64,
    pub fuel_cap: f64,
    pub hp: f64,
    pub matter: f64,
    pub energy: f64,
    pub fuel: f64,
    /// ship build speed
    pub build_speed: f64,
    pub def_hp: f64,
    pub def_attack: f64,
    pub sensor: f64,
    /// own ships' signature (lower = harder to see)
    pub signature: f64,
}

impl Default for Mods {
    fn default() -> Mods {
        Mods {
            kinetic_dmg: 1.0,
            energy_dmg: 1.0,
            accuracy: 1.0,
            ve: 1.0,
            fuel_cap: 1.0,
            hp: 1.0,
            matter: 1.0,
            energy: 1.0,
            fuel: 1.0,
            build_speed: 1.0,
            def_hp: 1.0,
            def_attack: 1.0,
            sensor: 1.0,
            signature: 1.0,
        }
    }
}

impl Tech {
    pub const ALL: [Tech; 20] = [
        Tech::HardenedSlugs,
        Tech::FocusedLasers,
        Tech::FireControl,
        Tech::SpinalLance,
        Tech::EfficientNozzles,
        Tech::ExpandedTanks,
        Tech::IonDrives,
        Tech::AsteroidDrives,
        Tech::DeepCoreMining,
        Tech::FusionPlants,
        Tech::ModularYards,
        Tech::AutoRefineries,
        Tech::ArmorPlating,
        Tech::PlanetaryShields,
        Tech::PointDefense,
        Tech::DefensePlatforms,
        Tech::BetterOptics,
        Tech::ScanningStations,
        Tech::SignatureDamping,
        Tech::DeepSpaceArray,
    ];

    pub fn index(self) -> usize {
        Tech::ALL.iter().position(|&t| t == self).unwrap()
    }
    pub fn branch(self) -> Branch {
        Branch::ALL[self.index() / 4]
    }
    /// 1..=4 within the branch.
    pub fn tier(self) -> u8 {
        (self.index() % 4) as u8 + 1
    }
    /// The tech before this one in its branch.
    pub fn prereq(self) -> Option<Tech> {
        let i = self.index();
        (!i.is_multiple_of(4)).then(|| Tech::ALL[i - 1])
    }
    /// Research lab level a body needs to research this.
    pub fn min_lab(self) -> u8 {
        match self.tier() {
            1 | 2 => 1,
            3 => 2,
            _ => 3,
        }
    }
    /// (matter, energy, seconds at a level-1 lab).
    pub fn cost(self) -> (f64, f64, f64) {
        let k = self.tier() as f64;
        let hours = [2.0, 4.0, 8.0, 12.0][self.tier() as usize - 1];
        (150.0 * k, 200.0 * k, hours * 3600.0)
    }
    pub fn label(self) -> &'static str {
        match self {
            Tech::HardenedSlugs => "Hardened slugs",
            Tech::FocusedLasers => "Focused lasers",
            Tech::FireControl => "Fire control",
            Tech::SpinalLance => "Spinal lance",
            Tech::EfficientNozzles => "Efficient nozzles",
            Tech::ExpandedTanks => "Expanded tanks",
            Tech::IonDrives => "Ion drives",
            Tech::AsteroidDrives => "Asteroid drives",
            Tech::DeepCoreMining => "Deep-core mining",
            Tech::FusionPlants => "Fusion plants",
            Tech::ModularYards => "Modular yards",
            Tech::AutoRefineries => "Auto refineries",
            Tech::ArmorPlating => "Armor plating",
            Tech::PlanetaryShields => "Planetary shields",
            Tech::PointDefense => "Point defense",
            Tech::DefensePlatforms => "Defense platforms",
            Tech::BetterOptics => "Better optics",
            Tech::ScanningStations => "Scanning stations",
            Tech::SignatureDamping => "Signature damping",
            Tech::DeepSpaceArray => "Deep-space array",
        }
    }
    pub fn describe(self) -> &'static str {
        match self {
            Tech::HardenedSlugs => "Kinetic weapons deal 15% more damage.",
            Tech::FocusedLasers => "Energy weapons deal 15% more damage.",
            Tech::FireControl => "Kinetic slugs lead fast targets better: +25% accuracy.",
            Tech::SpinalLance => "All weapons deal 20% more damage.",
            Tech::EfficientNozzles => "Engine exhaust velocity +10%: more Δv per tonne of fuel.",
            Tech::ExpandedTanks => "Ship fuel tanks hold 25% more.",
            Tech::IonDrives => "Exhaust velocity +20% more.",
            Tech::AsteroidDrives => "Attach drives to asteroids to push them onto new orbits.",
            Tech::DeepCoreMining => "Matter production +25%.",
            Tech::FusionPlants => "Energy production +25%.",
            Tech::ModularYards => "Ships build 30% faster.",
            Tech::AutoRefineries => "Fuel refineries produce 50% more.",
            Tech::ArmorPlating => "Ships have 20% more hull.",
            Tech::PlanetaryShields => "Planetary defenses have 50% more hit points.",
            Tech::PointDefense => "Planetary defenses fire 50% harder.",
            Tech::DefensePlatforms => "Unlocks armed defense platform stations.",
            Tech::BetterOptics => "Telescope and sensor range +30%.",
            Tech::ScanningStations => "Unlocks scanning stations and sensor relays.",
            Tech::SignatureDamping => "Your ships are 30% harder to see.",
            Tech::DeepSpaceArray => "Sensor range +40% more.",
        }
    }
    fn apply(self, m: &mut Mods) {
        match self {
            Tech::HardenedSlugs => m.kinetic_dmg *= 1.15,
            Tech::FocusedLasers => m.energy_dmg *= 1.15,
            Tech::FireControl => m.accuracy *= 1.25,
            Tech::SpinalLance => {
                m.kinetic_dmg *= 1.2;
                m.energy_dmg *= 1.2;
            }
            Tech::EfficientNozzles => m.ve *= 1.1,
            Tech::ExpandedTanks => m.fuel_cap *= 1.25,
            Tech::IonDrives => m.ve *= 1.2,
            Tech::DeepCoreMining => m.matter *= 1.25,
            Tech::FusionPlants => m.energy *= 1.25,
            Tech::ModularYards => m.build_speed *= 1.3,
            Tech::AutoRefineries => m.fuel *= 1.5,
            Tech::ArmorPlating => m.hp *= 1.2,
            Tech::PlanetaryShields => m.def_hp *= 1.5,
            Tech::PointDefense => m.def_attack *= 1.5,
            Tech::BetterOptics => m.sensor *= 1.3,
            Tech::SignatureDamping => m.signature *= 0.7,
            Tech::DeepSpaceArray => m.sensor *= 1.4,
            Tech::AsteroidDrives | Tech::DefensePlatforms | Tech::ScanningStations => {}
        }
    }
}

/// Lab speed multiplier for a research lab level.
pub fn lab_speed(level: u8) -> f64 {
    1.0 + 0.5 * (level.max(1) as f64 - 1.0)
}

impl Game {
    pub fn has_tech(&self, p: PlayerId, t: Tech) -> bool {
        self.players.get(p as usize).is_some_and(|pl| pl.techs.contains(&t))
    }

    pub fn mods(&self, p: PlayerId) -> Mods {
        let mut m = Mods::default();
        if let Some(pl) = self.players.get(p as usize) {
            for t in &pl.techs {
                t.apply(&mut m);
            }
        }
        m
    }

    /// Why `p` can't start researching `tech` at `body` right now (None = it can).
    pub fn research_blocker(&self, p: PlayerId, body: BodyId, tech: Tech) -> Option<String> {
        let pl = self.players.get(p as usize)?;
        let b = self.bodies.get(body.0 as usize)?;
        if pl.techs.contains(&tech) {
            return Some("already researched".into());
        }
        if pl.research.iter().any(|r| r.tech == tech) {
            return Some("already being researched".into());
        }
        if let Some(pre) = tech.prereq() {
            if !pl.techs.contains(&pre) {
                return Some(format!("needs {}", pre.label()));
            }
        }
        if b.owner != Some(p) {
            return Some("you don't own that body".into());
        }
        if b.level(Upgrade::Research) < tech.min_lab() {
            return Some(format!("needs research lab level {}", tech.min_lab()));
        }
        if pl.research.iter().any(|r| r.body == body) {
            return Some("this lab is busy".into());
        }
        None
    }

    pub(crate) fn start_research(&mut self, p: PlayerId, body: BodyId, tech: Tech) -> Result<(), String> {
        if let Some(why) = self.research_blocker(p, body, tech) {
            return Err(why);
        }
        let (m, e, secs) = tech.cost();
        self.pay(p, m, e)?;
        let speed = lab_speed(self.bodies[body.0 as usize].level(Upgrade::Research));
        let done_t = self.t + secs / speed;
        self.players[p as usize].research.push(Research { tech, body, done_t });
        Ok(())
    }

    /// Finish research projects that are due; a lab that changed hands loses its project.
    pub(crate) fn research_tick(&mut self) {
        for pi in 0..self.players.len() {
            let now = self.t;
            let p = pi as PlayerId;
            let (done, keep): (Vec<Research>, Vec<Research>) = std::mem::take(&mut self.players[pi].research).into_iter().partition(|r| r.done_t <= now);
            self.players[pi].research = keep;
            let lost: Vec<Research> = self.players[pi].research.iter().filter(|r| self.bodies[r.body.0 as usize].owner != Some(p)).cloned().collect();
            self.players[pi].research.retain(|r| self.bodies[r.body.0 as usize].owner == Some(p));
            for r in lost {
                let bname = self.system.body(r.body).name.clone();
                self.event(Some(p), crate::game::EventKind::Bad, "Research lost", &format!("{} stopped: we lost {bname}", r.tech.label()), Some(r.body), None);
            }
            for r in done {
                if self.bodies[r.body.0 as usize].owner != Some(p) || self.players[pi].techs.contains(&r.tech) {
                    continue;
                }
                self.players[pi].techs.push(r.tech);
                self.refit(p);
                self.event(Some(p), crate::game::EventKind::Good, "Research done", &format!("{}: {}", r.tech.label(), r.tech.describe()), Some(r.body), None);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::game::{Command, Settings};
    use crate::system::SystemSize;

    pub(crate) fn game() -> Game {
        let mut g = Game::new(
            "ABCD".into(),
            Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0 },
            5,
            "alice",
        );
        g.add_player("bob", None).unwrap();
        g.start();
        g.players[0].matter = 1e5;
        g.players[0].energy = 1e5;
        g
    }

    #[test]
    fn research_needs_a_lab_and_prereqs() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        let r = |g: &mut Game, t| g.apply(0, Command::Research { body: home, tech: t });
        assert!(r(&mut g, Tech::HardenedSlugs).unwrap_err().contains("lab"));
        g.bodies[home.0 as usize].levels[Upgrade::Research.index()] = 1;
        assert!(r(&mut g, Tech::FocusedLasers).unwrap_err().contains("Hardened"));
        r(&mut g, Tech::HardenedSlugs).unwrap();
        assert!(r(&mut g, Tech::DeepCoreMining).unwrap_err().contains("busy"));
        // bob can't research at alice's lab
        assert!(g.apply(1, Command::Research { body: home, tech: Tech::DeepCoreMining }).is_err());
        let (_, _, secs) = Tech::HardenedSlugs.cost();
        g.advance_to(secs - 120.0);
        assert!(!g.has_tech(0, Tech::HardenedSlugs));
        g.advance_to(secs + 120.0);
        assert!(g.has_tech(0, Tech::HardenedSlugs));
        assert!((g.mods(0).kinetic_dmg - 1.15).abs() < 1e-9);
        assert_eq!(g.mods(1), Mods::default());
        // tier 3 needs lab level 2
        r(&mut g, Tech::FocusedLasers).unwrap();
        g.advance_to(g.t + 5.0 * 3600.0);
        assert!(r(&mut g, Tech::FireControl).unwrap_err().contains("level 2"));
    }

    #[test]
    fn engine_research_refits_existing_ships_and_industry_boosts_output() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        let ship = g.ships.iter().find(|s| s.owner == 0).unwrap().id;
        let dv0 = g.ship(ship).unwrap().dv_left();
        let (m0, _, _) = g.production(home);
        g.players[0].techs = vec![Tech::EfficientNozzles, Tech::DeepCoreMining];
        g.refit(0);
        assert!((g.ship(ship).unwrap().dv_left() / dv0 - 1.1).abs() < 1e-9);
        assert!((g.production(home).0 / m0 - 1.25).abs() < 1e-9);
    }

    #[test]
    #[ignore = "slow: bots plan transfers for two game days (~30 s release)"]
    fn normal_bots_research() {
        let mut g = Game::new(
            "BOTS".into(),
            Settings { name: "t".into(), size: SystemSize::Small, max_players: 2, length: 7.0 * 86_400.0, speed: 1.0 },
            3,
            "alice",
        );
        g.add_player("bot", Some("normal".into())).unwrap();
        g.start();
        g.advance_to(2.0 * 86_400.0);
        assert!(!g.players[1].techs.is_empty() || !g.players[1].research.is_empty(), "bot never started research");
    }

    #[test]
    fn losing_the_lab_cancels_research() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        g.bodies[home.0 as usize].levels[Upgrade::Research.index()] = 1;
        g.apply(0, Command::Research { body: home, tech: Tech::BetterOptics }).unwrap();
        g.bodies[home.0 as usize].owner = Some(1);
        g.advance_to(3.0 * 3600.0);
        assert!(!g.has_tech(0, Tech::BetterOptics));
        assert!(g.players[0].research.is_empty());
    }

    #[test]
    fn tree_shape() {
        assert_eq!(Tech::ALL.len(), 20);
        for b in Branch::ALL {
            let techs: Vec<Tech> = Tech::ALL.iter().copied().filter(|t| t.branch() == b).collect();
            assert_eq!(techs.len(), 4);
            assert_eq!(techs[0].prereq(), None);
            for w in techs.windows(2) {
                assert_eq!(w[1].prereq(), Some(w[0]));
                assert!(w[1].min_lab() >= w[0].min_lab());
            }
        }
        // ≈ a week of lab time at a level-1 lab
        let hours: f64 = Tech::ALL.iter().map(|t| t.cost().2 / 3600.0).sum();
        assert!((100.0..=170.0).contains(&hours), "{hours}");
    }
}

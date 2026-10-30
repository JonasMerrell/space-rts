//! The shared research tree: 5 branches × 4 tiers. Everyone researches from the same tree
//! (factions are only aesthetic). A tech is researched at an owned body with a Research lab;
//! each lab body runs one project per lab level at a time (Jonas: more research at once as labs
//! are upgraded; labs on different bodies run in parallel too), separately from its ship/upgrade queue.
//! The whole tree is ~130 lab-hours, so one lab finishes it in about a week.
//!
//! Advanced tier (rules >= 2, Jonas 2026-10-27): 5 more branches × 3 tiers that open once
//! `ADVANCED_UNLOCK` base techs are done. They unlock new mechanics (cloaks, ordnance, gates...)
//! rather than plain multipliers, and cost about 500 lab-hours, so few players finish it.
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
    // ---- advanced tier (rules >= 2); appended so old saves still parse
    // transit
    LaserLaunch,
    AntimatterCatalysis,
    Stargates,
    // strike
    Torpedoes,
    MassDrivers,
    FusionWarheads,
    // bastion
    Minefields,
    InterdictionGrid,
    PhasedLasers,
    // shadow
    DecoyDrones,
    HeatSinkCloaks,
    ThermalBaffles,
    // sight
    GravimetricArray,
    LidarSweep,
    DeepFieldInterferometry,
}

/// Base techs a player needs before the advanced tier opens.
pub const ADVANCED_UNLOCK: usize = 15;
/// First rules revision with the advanced tier.
pub const ADVANCED_RULES: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Branch {
    Weapons,
    Engines,
    Industry,
    Defense,
    Scanning,
    // advanced tier
    Transit,
    Strike,
    Bastion,
    Shadow,
    Sight,
}

impl Branch {
    /// The base tree's branches.
    pub const ALL: [Branch; 5] = [Branch::Weapons, Branch::Engines, Branch::Industry, Branch::Defense, Branch::Scanning];
    pub const ADVANCED: [Branch; 5] = [Branch::Transit, Branch::Strike, Branch::Bastion, Branch::Shadow, Branch::Sight];
    pub fn label(self) -> &'static str {
        match self {
            Branch::Weapons => "Weapons",
            Branch::Engines => "Engines",
            Branch::Industry => "Industry",
            Branch::Defense => "Defense",
            Branch::Scanning => "Scanning",
            Branch::Transit => "Transit",
            Branch::Strike => "Strike",
            Branch::Bastion => "Bastion",
            Branch::Shadow => "Shadow",
            Branch::Sight => "Sight",
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
    /// torpedo / mass-driver slug damage
    pub ordnance_dmg: f64,
    /// how long a heat-sink cloak lasts
    pub cloak_time: f64,
    /// signature while venting a heat sink
    pub vent_sig: f64,
    /// passive sensors tell decoys from ships
    pub see_decoys: bool,
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
            ordnance_dmg: 1.0,
            cloak_time: 1.0,
            vent_sig: 1.0,
            see_decoys: false,
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

    /// The advanced tier, branch-major (see `Branch::ADVANCED`).
    pub const ADVANCED: [Tech; 15] = [
        Tech::LaserLaunch,
        Tech::AntimatterCatalysis,
        Tech::Stargates,
        Tech::Torpedoes,
        Tech::MassDrivers,
        Tech::FusionWarheads,
        Tech::Minefields,
        Tech::InterdictionGrid,
        Tech::PhasedLasers,
        Tech::DecoyDrones,
        Tech::HeatSinkCloaks,
        Tech::ThermalBaffles,
        Tech::GravimetricArray,
        Tech::LidarSweep,
        Tech::DeepFieldInterferometry,
    ];

    /// Base tree, then the advanced tier.
    pub fn every() -> impl Iterator<Item = Tech> {
        Tech::ALL.into_iter().chain(Tech::ADVANCED)
    }

    /// 0..20 base, 20..35 advanced (declaration order = `every()` order, checked in tests).
    pub fn index(self) -> usize {
        self as usize
    }
    pub fn advanced(self) -> bool {
        self.index() >= Tech::ALL.len()
    }
    /// Index within the advanced tier.
    fn adv_index(self) -> Option<usize> {
        self.index().checked_sub(Tech::ALL.len())
    }
    pub fn branch(self) -> Branch {
        match self.adv_index() {
            Some(i) => Branch::ADVANCED[i / 3],
            None => Branch::ALL[self.index() / 4],
        }
    }
    /// 1..=4 within a base branch, 1..=3 within an advanced one.
    pub fn tier(self) -> u8 {
        match self.adv_index() {
            Some(i) => (i % 3) as u8 + 1,
            None => (self.index() % 4) as u8 + 1,
        }
    }
    /// The tech before this one in its branch.
    pub fn prereq(self) -> Option<Tech> {
        match self.adv_index() {
            Some(i) => (!i.is_multiple_of(3)).then(|| Tech::ADVANCED[i - 1]),
            None => {
                let i = self.index();
                (!i.is_multiple_of(4)).then(|| Tech::ALL[i - 1])
            }
        }
    }
    /// Research lab level a body needs to research this.
    pub fn min_lab(self) -> u8 {
        match (self.advanced(), self.tier()) {
            (true, 1) => 2,
            (true, _) => 3,
            (false, 1 | 2) => 1,
            (false, 3) => 2,
            _ => 3,
        }
    }
    /// (matter, energy, seconds at a level-1 lab). Base research costs energy only (Jonas 2026-10-29): the old
    /// matter price is folded into the energy price. The advanced tier still needs matter.
    pub fn cost(self) -> (f64, f64, f64) {
        let (m, e, secs) = self.old_cost();
        if self.advanced() {
            return (m, e, secs);
        }
        (0.0, m + e, secs)
    }
    fn old_cost(self) -> (f64, f64, f64) {
        let k = self.tier() as f64;
        if self.advanced() {
            // a full week of bot wars showed hard bots finishing the tier at half this: it should force choices
            let hours = [20.0, 32.0, 48.0][self.tier() as usize - 1];
            return (500.0 + 500.0 * k, 600.0 + 600.0 * k, hours * 3600.0);
        }
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
            Tech::LaserLaunch => "Laser launch array",
            Tech::AntimatterCatalysis => "Antimatter catalysis",
            Tech::Stargates => "Stargates",
            Tech::Torpedoes => "Hunter-killer torpedoes",
            Tech::MassDrivers => "Mass drivers",
            Tech::FusionWarheads => "Fusion warheads",
            Tech::Minefields => "Orbital minefields",
            Tech::InterdictionGrid => "Interdiction grid",
            Tech::PhasedLasers => "Phased laser battery",
            Tech::DecoyDrones => "Decoy drones",
            Tech::HeatSinkCloaks => "Heat-sink cloaking",
            Tech::ThermalBaffles => "Thermal baffles",
            Tech::GravimetricArray => "Gravimetric array",
            Tech::LidarSweep => "Lidar sweep",
            Tech::DeepFieldInterferometry => "Deep-field interferometry",
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
            Tech::LaserLaunch => "Station that beams departing ships: the first 3 km/s of a departure burn costs no fuel.",
            Tech::AntimatterCatalysis => "Engine exhaust velocity +50%.",
            Tech::Stargates => "Paired gate stations: ships parked at one gate jump to the other in 10 minutes.",
            Tech::Torpedoes => "Uncrewed torpedoes that chase one enemy ship across the system and detonate.",
            Tech::MassDrivers => "Station that fires guided slugs on transfer orbits at another body's orbit.",
            Tech::FusionWarheads => "Torpedo and slug damage +60%; slugs also hit stations.",
            Tech::Minefields => "Constructors seed mines that hit enemy ships capturing into that orbit.",
            Tech::InterdictionGrid => "Station that shoots down incoming torpedoes and slugs and fragments asteroids.",
            Tech::PhasedLasers => "Planet-based laser battery that hits ships out to 40,000 km.",
            Tech::DecoyDrones => "Cheap drones that look like a warship to enemy sensors.",
            Tech::HeatSinkCloaks => "Ships can go dark for 12 h by storing their heat, then must vent.",
            Tech::ThermalBaffles => "Cloaks last twice as long; venting is half as bright.",
            Tech::GravimetricArray => "Station that detects ships by mass, cloaked or not.",
            Tech::LidarSweep => "Scanning stations can ping: everything in 3× range shows for 1 h, the pinger too.",
            Tech::DeepFieldInterferometry => "Sensor range +50%, and passive sensors see through decoys.",
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
            Tech::AntimatterCatalysis => m.ve *= 1.5,
            Tech::FusionWarheads => m.ordnance_dmg *= 1.6,
            Tech::ThermalBaffles => {
                m.cloak_time *= 2.0;
                m.vent_sig *= 0.5;
            }
            Tech::DeepFieldInterferometry => {
                m.sensor *= 1.5;
                m.see_decoys = true;
            }
            // unlocks: the systems that use them check `has_tech`
            Tech::AsteroidDrives
            | Tech::DefensePlatforms
            | Tech::ScanningStations
            | Tech::LaserLaunch
            | Tech::Stargates
            | Tech::Torpedoes
            | Tech::MassDrivers
            | Tech::Minefields
            | Tech::InterdictionGrid
            | Tech::PhasedLasers
            | Tech::DecoyDrones
            | Tech::HeatSinkCloaks
            | Tech::GravimetricArray
            | Tech::LidarSweep => {}
        }
    }
}

/// Projects a lab can run at once: one per level.
pub fn lab_slots(level: u8) -> usize {
    level.max(1) as usize
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
        if tech.advanced() {
            if self.rules < ADVANCED_RULES {
                return Some("not in this war (started before the advanced tier)".into());
            }
            let base = pl.techs.iter().filter(|t| !t.advanced()).count();
            if base < ADVANCED_UNLOCK {
                return Some(format!("needs {ADVANCED_UNLOCK} base techs ({base} so far)"));
            }
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
        let slots = lab_slots(b.level(Upgrade::Research));
        if pl.research.iter().filter(|r| r.body == body).count() >= slots {
            return Some(if slots == 1 { "this lab is busy".into() } else { format!("all {slots} lab slots here are busy") });
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
            Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0, teams: 0 },
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
    fn upgraded_labs_and_more_labs_research_in_parallel() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        let r = |g: &mut Game, body, t| g.apply(0, Command::Research { body, tech: t });
        g.bodies[home.0 as usize].levels[Upgrade::Research.index()] = 2;
        r(&mut g, home, Tech::HardenedSlugs).unwrap();
        r(&mut g, home, Tech::DeepCoreMining).unwrap();
        let third = Tech::ALL.into_iter().find(|&t| g.research_blocker(0, home, t).is_some_and(|w| w.contains("busy"))).unwrap();
        // a second planet with its own lab takes a third project
        let other = (0..g.bodies.len() as u32).map(BodyId).find(|&b| b != home && g.bodies[b.0 as usize].owner.is_none() && !g.system.body(b).is_star()).unwrap();
        g.bodies[other.0 as usize].owner = Some(0);
        g.bodies[other.0 as usize].levels[Upgrade::Research.index()] = 1;
        r(&mut g, other, third).unwrap();
        assert_eq!(g.players[0].research.len(), 3);
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
            Settings { name: "t".into(), size: SystemSize::Small, max_players: 2, length: 7.0 * 86_400.0, speed: 1.0, teams: 0 },
            3,
            "alice",
        );
        g.add_player("bot", Some("normal".into())).unwrap();
        g.start();
        g.advance_to(2.0 * 86_400.0);
        assert!(!g.players[1].techs.is_empty() || !g.players[1].research.is_empty(), "bot never started research");
        assert!(g.ships.iter().any(|s| s.owner == 1 && s.class == crate::game::ShipClass::Constructor), "bot built no constructor");
        assert!(g.stations.iter().any(|s| s.owner == 1), "bot built no station");
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
    fn advanced_tier_opens_after_enough_base_research() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        g.bodies[home.0 as usize].levels[Upgrade::Research.index()] = 3;
        let r = |g: &mut Game, t| g.apply(0, Command::Research { body: home, tech: t });
        g.players[0].techs = Tech::ALL[..ADVANCED_UNLOCK - 1].to_vec();
        assert!(r(&mut g, Tech::Torpedoes).unwrap_err().contains("base techs"));
        g.players[0].techs = Tech::ALL[..ADVANCED_UNLOCK].to_vec();
        assert!(r(&mut g, Tech::MassDrivers).unwrap_err().contains("Hunter-killer"));
        r(&mut g, Tech::Torpedoes).unwrap();
        // a war started before the tier never gets it
        g.rules = 1;
        assert!(g.research_blocker(0, home, Tech::DecoyDrones).unwrap().contains("not in this war"));
        // stat techs feed Mods
        g.players[0].techs = vec![Tech::AntimatterCatalysis, Tech::ThermalBaffles, Tech::DeepFieldInterferometry];
        let m = g.mods(0);
        assert!((m.ve - 1.5).abs() < 1e-9 && (m.cloak_time - 2.0).abs() < 1e-9 && m.see_decoys);
    }

    #[test]
    fn advanced_tree_shape() {
        assert_eq!(Tech::every().count(), 35);
        assert!(Tech::every().enumerate().all(|(i, t)| t.index() == i));
        for b in Branch::ADVANCED {
            let techs: Vec<Tech> = Tech::ADVANCED.iter().copied().filter(|t| t.branch() == b).collect();
            assert_eq!(techs.len(), 3, "{b:?}");
            assert_eq!(techs[0].prereq(), None);
            assert_eq!(techs.iter().map(|t| t.tier()).collect::<Vec<_>>(), [1, 2, 3]);
            for w in techs.windows(2) {
                assert_eq!(w[1].prereq(), Some(w[0]));
            }
            assert!(techs.iter().all(|t| t.advanced() && t.min_lab() >= 2));
        }
        // too much for one lab to finish in a war: you pick
        let hours: f64 = Tech::ADVANCED.iter().map(|t| t.cost().2 / 3600.0).sum();
        assert!((450.0..=550.0).contains(&hours), "{hours}");
        // every advanced tech parses back from its serde name (the client sends Debug names)
        for t in Tech::every() {
            let json = serde_json::to_string(&t).unwrap();
            assert_eq!(serde_json::from_str::<Tech>(&json).unwrap(), t);
        }
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

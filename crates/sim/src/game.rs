//! Game rules: players, ships, economy, upgrades, deterministic combat and ownership by power
//! in orbit. `Game::advance_to` is the authoritative real-time step the server runs.
use crate::kepler::circular_speed;
use crate::plan::{self, PlanHint, TransferPlan};
use crate::system::{generate, BodyId, BodyKind, Rng, System, SystemSize};
use crate::traj::{predict, Burn, End, State, Trajectory};
use crate::station::{Station, StationKind};
use crate::tech::{Research, Tech};
use crate::v2::V2;
use serde::{Deserialize, Serialize};

pub type PlayerId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ShipId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ShipClass {
    Scout,
    Interceptor,
    Gunship,
    Bomber,
    Dreadnought,
    Tanker,
    /// builds space stations where it is parked; more of them build faster
    Constructor,
    /// advanced tier: unarmed drone that shows to enemy sensors as `Ship::mimic` (see cloak.rs)
    Decoy,
    /// advanced tier: uncrewed hunter-killer, detonates near enemy ships (see ordnance.rs)
    Torpedo,
    /// advanced tier: mass-driver slug, hits the body it's sent to instead of braking
    Slug,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Weapon {
    None,
    /// slugs/missiles: hits harder, but the projectile is slow
    Kinetic,
    /// lasers: light-speed, lower damage
    Energy,
    /// phased laser battery (stations only): light-speed, very long range, falls off with distance
    Beam,
}

#[derive(Clone, Copy, Debug)]
pub struct ClassStats {
    pub name: &'static str,
    pub hp: f64,
    /// damage per hour of combat against a neutral target
    pub attack: f64,
    pub weapon: Weapon,
    /// tonnes
    pub dry_mass: f64,
    pub fuel_cap: f64,
    /// exhaust velocity, km/s
    pub ve: f64,
    pub matter: f64,
    pub energy: f64,
    /// seconds
    pub build_time: f64,
    /// how easily telescopes see it (1 = baseline)
    pub signature: f64,
    pub min_shipyard: u8,
}

impl ShipClass {
    pub const ALL: [ShipClass; 10] = [
        ShipClass::Scout,
        ShipClass::Interceptor,
        ShipClass::Gunship,
        ShipClass::Bomber,
        ShipClass::Dreadnought,
        ShipClass::Tanker,
        ShipClass::Constructor,
        ShipClass::Decoy,
        ShipClass::Torpedo,
        ShipClass::Slug,
    ];

    /// Research needed before a shipyard can build this class.
    pub fn needs(self) -> Option<Tech> {
        match self {
            ShipClass::Decoy => Some(Tech::DecoyDrones),
            ShipClass::Torpedo => Some(Tech::Torpedoes),
            ShipClass::Slug => Some(Tech::MassDrivers),
            _ => None,
        }
    }

    pub fn stats(self) -> ClassStats {
        use ShipClass::*;
        let s = |name, hp, attack, weapon, dry_mass, fuel_cap, matter, energy, build_min: f64, signature, min_shipyard| ClassStats {
            name,
            hp,
            attack,
            weapon,
            dry_mass,
            fuel_cap,
            ve: 15.0,
            matter,
            energy,
            build_time: build_min * 60.0,
            signature,
            min_shipyard,
        };
        match self {
            Scout => s("Scout", 20.0, 0.0, Weapon::None, 8.0, 30.0, 30.0, 20.0, 10.0, 0.3, 1),
            Interceptor => s("Interceptor", 40.0, 20.0, Weapon::Kinetic, 20.0, 60.0, 60.0, 20.0, 15.0, 0.6, 1),
            Gunship => s("Gunship", 120.0, 30.0, Weapon::Energy, 60.0, 160.0, 150.0, 60.0, 40.0, 1.0, 1),
            Bomber => s("Bomber", 80.0, 40.0, Weapon::Kinetic, 50.0, 130.0, 130.0, 90.0, 40.0, 1.0, 1),
            Dreadnought => s("Dreadnought", 500.0, 90.0, Weapon::Energy, 300.0, 700.0, 600.0, 300.0, 180.0, 4.0, 2),
            Tanker => s("Tanker", 60.0, 0.0, Weapon::None, 40.0, 600.0, 80.0, 30.0, 20.0, 1.5, 1),
            Constructor => s("Constructor", 50.0, 0.0, Weapon::None, 30.0, 120.0, 100.0, 50.0, 30.0, 0.8, 1),
            // signature is the mimicked class's (see fog::signature); this is the default look
            Decoy => s("Decoy", 10.0, 0.0, Weapon::None, 4.0, 40.0, 40.0, 30.0, 10.0, 1.0, 1),
            Torpedo => s("Torpedo", 15.0, 0.0, Weapon::None, 4.0, 60.0, 120.0, 80.0, 20.0, 0.3, 1),
            // the driver's kick is its "fuel": ve 1000 km/s gives it Δv to reach anything
            Slug => ClassStats { ve: 1000.0, ..s("Slug", 5.0, 0.0, Weapon::None, 20.0, 20.0, 0.0, 0.0, 0.0, 0.2, 1) },
        }
    }

    /// Rock-paper-scissors damage multiplier of `self` attacking `target`.
    pub fn vs(self, target: ShipClass) -> f64 {
        use ShipClass::*;
        match (self, target) {
            (Interceptor, Bomber) | (Interceptor, Scout) | (Interceptor, Torpedo) => 2.0,
            (Interceptor, Gunship) | (Interceptor, Dreadnought) => 0.5,
            (Gunship, Interceptor) => 2.0,
            (Gunship, Dreadnought) => 0.75,
            (Bomber, Gunship) | (Bomber, Dreadnought) => 2.0,
            (Bomber, Interceptor) => 0.5,
            (Dreadnought, Interceptor) => 0.75,
            (Dreadnought, Gunship) | (Dreadnought, Bomber) => 1.25,
            (_, Tanker) | (_, Constructor) => 1.5,
            _ => 1.0,
        }
    }
    /// Multiplier against planetary defenses / stations.
    pub fn vs_defense(self) -> f64 {
        match self {
            ShipClass::Bomber => 2.0,
            ShipClass::Interceptor => 0.5,
            _ => 1.0,
        }
    }

    /// Player-facing description generated from the live stats, so UI text follows balance changes.
    pub fn describe(self) -> String {
        let s = self.stats();
        let strong: Vec<&str> = ShipClass::ALL.iter().filter(|&&c| self.vs(c) > 1.0 && c != ShipClass::Tanker && c != ShipClass::Constructor).map(|c| c.stats().name).collect();
        let weak: Vec<&str> = ShipClass::ALL.iter().filter(|&&c| self.vs(c) < 1.0).map(|c| c.stats().name).collect();
        let weapon = match s.weapon {
            Weapon::Kinetic => "kinetic slugs (hard-hitting, slow projectiles)",
            Weapon::Energy => "lasers (light-speed, lighter hits)",
            Weapon::Beam => "a long-range laser battery",
            Weapon::None => "no weapons",
        };
        let dv = s.ve * ((s.dry_mass + s.fuel_cap) / s.dry_mass).ln();
        let mut out = format!(
            "{} — {:.0} hull, {:.0} damage/h, {weapon}. Δv {:.1} km/s on full tanks ({:.0} t fuel). Signature {:.1}×.",
            s.name, s.hp, s.attack, dv, s.fuel_cap, s.signature
        );
        if !strong.is_empty() {
            out += &format!(" Strong vs {}.", strong.join(", "));
        }
        if !weak.is_empty() {
            out += &format!(" Weak vs {}.", weak.join(", "));
        }
        if self.vs_defense() != 1.0 {
            out += &format!(" {:.1}× vs planetary defenses.", self.vs_defense());
        }
        if self == ShipClass::Tanker {
            out += " Refuels friendly ships parked with it.";
        }
        if self == ShipClass::Torpedo {
            out += &format!(" Send it on an intercept: it detonates within {:.0} km of an enemy ship for {:.0} damage (2× vs Dreadnoughts).", crate::ordnance::DETONATE_RANGE, crate::ordnance::TORPEDO_DAMAGE);
        }
        if self == ShipClass::Decoy {
            out += " Shows to enemy sensors as the warship class you pick, burns and all, until a scanning station, Deep-field interferometry or a fight sees through it.";
        }
        if self == ShipClass::Constructor {
            out += &format!(" Builds space stations where it is parked; each one adds a worker (up to {}).", crate::station::MAX_WORKERS);
        }
        if s.min_shipyard > 1 {
            out += &format!(" Needs shipyard level {}.", s.min_shipyard);
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StandingOrder {
    /// stay and fight whatever arrives
    Hold,
    /// leave for the nearest friendly body when clearly outgunned
    Retreat,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Departure {
    pub to: BodyId,
    /// state relative to the departure body right after the burn
    pub state: State,
    pub dv: f64,
    /// mid-course burns planned with the departure (gravity-assist corrections)
    #[serde(default)]
    pub burns: Vec<Burn>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Flight {
    pub start: State,
    pub burns: Vec<Burn>,
    pub dest: Option<BodyId>,
    #[serde(skip)]
    pub cache: Option<Trajectory>,
}

impl Flight {
    pub const HORIZON: f64 = 8.0 * 86_400.0;
    pub fn trajectory(&mut self, sys: &System) -> &Trajectory {
        if self.cache.is_none() {
            self.cache = Some(predict(sys, self.start, &self.burns, self.dest, self.start.t + Self::HORIZON));
        }
        self.cache.as_ref().unwrap()
    }
    /// Non-caching variant for read-only callers.
    pub fn compute(&self, sys: &System) -> Trajectory {
        self.cache.clone().unwrap_or_else(|| predict(sys, self.start, &self.burns, self.dest, self.start.t + Self::HORIZON))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ShipState {
    Parked { body: BodyId, departure: Option<Departure> },
    Flying(Flight),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ship {
    pub id: ShipId,
    pub owner: PlayerId,
    pub class: ShipClass,
    pub name: String,
    pub hp: f64,
    /// tonnes of propellant
    pub fuel: f64,
    pub order: StandingOrder,
    /// remaining hops after the current leg (multi-leg routes)
    pub route: Vec<BodyId>,
    pub state: ShipState,
    /// research refits (engines, tanks, armor)
    #[serde(default)]
    pub tech: ShipTech,
    /// game time of the last engine burn (plumes make the ship easier to detect, see fog.rs)
    #[serde(default = "no_burn")]
    pub last_burn_t: f64,
    /// standing intercept order (re-planned when the target changes course, see intercept.rs)
    #[serde(default)]
    pub intercept: Option<crate::intercept::Order>,
    /// heat-sink cloak state (advanced tier, see cloak.rs)
    #[serde(default)]
    pub cloak: Option<crate::cloak::Cloak>,
    /// decoys: the class enemy sensors see
    #[serde(default)]
    pub mimic: Option<ShipClass>,
}

/// `last_burn_t` of a ship that never burned (finite so it survives JSON).
pub fn no_burn() -> f64 {
    -1e12
}

/// Multipliers research applies to a ship's class stats.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShipTech {
    pub ve: f64,
    pub fuel_cap: f64,
    pub hp: f64,
}

impl Default for ShipTech {
    fn default() -> ShipTech {
        ShipTech { ve: 1.0, fuel_cap: 1.0, hp: 1.0 }
    }
}

impl Ship {
    /// Class stats with research refits applied.
    pub fn stats(&self) -> ClassStats {
        let mut s = self.class.stats();
        s.ve *= self.tech.ve;
        s.fuel_cap *= self.tech.fuel_cap;
        s.hp *= self.tech.hp;
        s
    }
    pub fn mass(&self) -> f64 {
        self.stats().dry_mass + self.fuel
    }
    /// Δv available with the current fuel, km/s.
    pub fn dv_left(&self) -> f64 {
        let s = self.stats();
        s.ve * (self.mass() / s.dry_mass).ln()
    }
    /// Fuel needed for a burn of `dv` km/s with the current mass.
    pub fn fuel_for(&self, dv: f64) -> f64 {
        self.fuel_for_with(self.fuel, dv)
    }
    /// Fuel needed for a burn of `dv` km/s if the ship carried `fuel` tonnes.
    pub fn fuel_for_with(&self, fuel: f64, dv: f64) -> f64 {
        let s = self.stats();
        (s.dry_mass + fuel) * (1.0 - (-dv / s.ve).exp())
    }
    /// Where the ship is going: its scheduled departure's target, or its flight's destination.
    pub fn dest(&self) -> Option<BodyId> {
        match &self.state {
            ShipState::Parked { departure: Some(d), .. } => Some(d.to),
            ShipState::Parked { .. } => None,
            ShipState::Flying(f) => f.dest,
        }
    }
    pub fn parked_at(&self) -> Option<BodyId> {
        match self.state {
            ShipState::Parked { body, .. } => Some(body),
            _ => None,
        }
    }
}

/// Rocket equation: propellant for `dv` km/s for a ship of `class` carrying `fuel` tonnes.
pub fn fuel_needed(class: ShipClass, fuel: f64, dv: f64) -> f64 {
    let s = class.stats();
    (s.dry_mass + fuel) * (1.0 - (-dv / s.ve).exp())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Upgrade {
    Mining,
    Solar,
    Reactor,
    Refinery,
    Shipyard,
    Defense,
    Telescope,
    Research,
}

impl Upgrade {
    pub const ALL: [Upgrade; 8] = [
        Upgrade::Mining,
        Upgrade::Solar,
        Upgrade::Reactor,
        Upgrade::Refinery,
        Upgrade::Shipyard,
        Upgrade::Defense,
        Upgrade::Telescope,
        Upgrade::Research,
    ];
    pub const MAX_LEVEL: u8 = 3;
    pub fn index(self) -> usize {
        Upgrade::ALL.iter().position(|&u| u == self).unwrap()
    }
    /// (matter, energy, seconds) to reach `level`.
    pub fn cost(self, level: u8) -> (f64, f64, f64) {
        let (m, e) = match self {
            Upgrade::Mining => (120.0, 40.0),
            // solar and reactor cost the same (solar output depends on distance from the star)
            Upgrade::Solar | Upgrade::Reactor => (100.0, 20.0),
            Upgrade::Refinery => (120.0, 60.0),
            Upgrade::Shipyard => (200.0, 80.0),
            Upgrade::Defense => (150.0, 80.0),
            Upgrade::Telescope => (100.0, 100.0),
            Upgrade::Research => (120.0, 120.0),
        };
        let k = level as f64;
        (m * k, e * k, 1800.0 * k)
    }
    /// Player-facing effect of reaching `level`, generated from the same numbers the rules use.
    pub fn describe(self, level: u8) -> String {
        let l = level as f64;
        match self {
            Upgrade::Mining => format!("Matter output ×{:.0} (60/h base per body).", 1.0 + l),
            Upgrade::Solar => format!("+{:.0} energy/h at 1 AU-equivalent; falls off with distance² from the star.", 60.0 * l),
            Upgrade::Reactor => format!("+{:.0} energy/h anywhere. Costs the same as solar.", 60.0 * l),
            Upgrade::Refinery => format!("Refines {:.0} t fuel/h (0.5 energy per t); fuel stock cap {:.0} t.", 120.0 * l, 500.0 + 1500.0 * l),
            Upgrade::Shipyard => format!("Builds ships ×{:.1} speed{}.", 1.0 + 0.5 * (l - 1.0).max(0.0), if level >= 2 { "; can build Dreadnoughts" } else { "" }),
            Upgrade::Defense => format!("Planetary guns: {:.0} damage/h, {:.0} hp, regenerate 10%/h.", 40.0 * l, 250.0 * l),
            Upgrade::Telescope => format!("Sensor range {:.0} Mm from this body.", (60_000.0 + 250_000.0 * l) / 1000.0),
            Upgrade::Research => format!("Research lab level {level}: runs one project; higher tiers need higher labs."),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Upgrade::Mining => "Mining",
            Upgrade::Solar => "Solar",
            Upgrade::Reactor => "Reactor",
            Upgrade::Refinery => "Fuel refinery",
            Upgrade::Shipyard => "Shipyard",
            Upgrade::Defense => "Defense guns",
            Upgrade::Telescope => "Telescope",
            Upgrade::Research => "Research lab",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JobKind {
    Ship(ShipClass),
    Upgrade(Upgrade),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub kind: JobKind,
    pub owner: PlayerId,
    pub done_t: f64,
    /// order the ship gets as soon as it is built (planned on the time scrubber)
    #[serde(default)]
    pub then: Option<QueuedOrder>,
}

/// A body's queue has two lanes that run side by side: ship builds (one per builder, since others
/// can build at your shipyard station) and upgrades (Jonas: shipyard and upgrades don't share a queue).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Ships(PlayerId),
    Upgrades,
}

impl Job {
    pub fn lane(&self) -> Lane {
        match self.kind {
            JobKind::Ship(_) => Lane::Ships(self.owner),
            JobKind::Upgrade(_) => Lane::Upgrades,
        }
    }
}

impl BodyState {
    /// When the last job of `lane` finishes (or `now` if the lane is idle).
    pub fn lane_free_at(&self, lane: Lane, now: f64) -> f64 {
        self.queue.iter().filter(|j| j.lane() == lane).map(|j| j.done_t).fold(now, f64::max)
    }
}

/// Transfer for a ship that is still in a build queue.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueuedOrder {
    pub to: BodyId,
    pub depart_t: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BodyState {
    pub owner: Option<PlayerId>,
    /// level per `Upgrade::ALL` index
    pub levels: [u8; 8],
    /// fuel stockpile (tonnes)
    pub fuel: f64,
    /// remaining hit points of the planetary defenses
    pub def_hp: f64,
    pub queue: Vec<Job>,
    /// a battle was going on here last tick (so "Under attack" fires once per battle)
    #[serde(default)]
    pub in_combat: bool,
    /// destroyed by an asteroid impact: produces nothing and can't be owned
    #[serde(default)]
    pub shattered: bool,
    /// an asteroid that crashed into something; no longer exists (UI: don't draw it)
    #[serde(default)]
    pub gone: bool,
}

impl BodyState {
    pub fn level(&self, u: Upgrade) -> u8 {
        self.levels[u.index()]
    }
    pub fn def_max(&self) -> f64 {
        250.0 * self.level(Upgrade::Defense) as f64
    }
    pub fn def_attack(&self) -> f64 {
        40.0 * self.level(Upgrade::Defense) as f64
    }
    pub fn fuel_cap(&self) -> f64 {
        500.0 + 1500.0 * self.level(Upgrade::Refinery) as f64
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Player {
    pub id: PlayerId,
    pub name: String,
    /// aesthetic only: "vanguard" | "hive" | "prism" | "junker"
    pub faction: String,
    pub color: usize,
    pub matter: f64,
    pub energy: f64,
    pub bot: Option<String>,
    pub home: Option<BodyId>,
    /// bots: (from, to, until) routes that recently failed to plan, so they aren't retried every tick
    #[serde(default)]
    pub bot_avoid: Vec<(BodyId, BodyId, f64)>,
    /// fog of war: last sensor reading per body (index = body id)
    #[serde(default)]
    pub intel: Vec<Option<crate::fog::Intel>>,
    /// researched techs, in the order they finished
    #[serde(default)]
    pub techs: Vec<Tech>,
    /// projects running at labs (one per lab body)
    #[serde(default)]
    pub research: Vec<Research>,
    /// game time this player was last connected (the server stamps it; drives the away report)
    #[serde(default)]
    pub last_active: f64,
    /// fog of war: what this player's sensors picked up, for the away report (capped, see fog.rs)
    #[serde(default)]
    pub sensor_log: Vec<crate::fog::SensorItem>,
    /// fog of war: last sightings of enemy ships
    #[serde(default)]
    pub contacts: Vec<crate::fog::Contact>,
    /// fog of war: what this player detects right now (recomputed every tick, so not saved or sent)
    #[serde(skip)]
    pub vision: crate::fog::Vision,
    /// builds/upgrades/stations/research queued before they could be paid for (see waitlist.rs)
    #[serde(default)]
    pub waiting: Vec<crate::waitlist::Waiting>,
    #[serde(default)]
    pub next_wait: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMsg {
    pub from: PlayerId,
    pub t: f64,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    Info,
    Good,
    Bad,
    Battle,
    /// fog of war: what your sensors picked up (the "while you were away" report; not pushed to phones)
    Sensor,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub t: f64,
    /// None = everyone
    pub to: Option<PlayerId>,
    pub kind: EventKind,
    pub title: String,
    pub body: String,
    pub body_ref: Option<BodyId>,
    pub ship_ref: Option<ShipId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Lobby,
    Running,
    Ended,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub name: String,
    pub size: SystemSize,
    pub max_players: usize,
    /// game length in game seconds
    pub length: f64,
    /// game seconds per real second (1 = real time)
    pub speed: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Game {
    /// rules revision the match was created with (old saves load as 0 and keep their balance)
    #[serde(default)]
    pub rules: u32,
    pub code: String,
    pub settings: Settings,
    pub status: Status,
    pub host: PlayerId,
    /// game time, seconds since start
    pub t: f64,
    pub end_t: f64,
    pub system: System,
    pub players: Vec<Player>,
    pub ships: Vec<Ship>,
    pub bodies: Vec<BodyState>,
    pub chat: Vec<ChatMsg>,
    pub events: Vec<Event>,
    pub next_ship: u32,
    pub next_event: u64,
    /// last whole-minute economy/combat tick
    pub last_tick: f64,
    pub winner: Option<PlayerId>,
    /// unix time (s) at which game time 0 happened, set when the game starts
    #[serde(default)]
    pub started_real: f64,
    #[serde(default)]
    pub stations: Vec<Station>,
    #[serde(default)]
    pub next_station: u32,
    #[serde(default)]
    pub fleets: Vec<crate::fleet::Fleet>,
    #[serde(default)]
    pub next_fleet: u32,
    /// asteroid drives waiting to burn
    #[serde(default)]
    pub drives: Vec<crate::asteroid::Drive>,
    /// pushed asteroids being tracked for flybys and impacts
    #[serde(default)]
    pub drifts: Vec<crate::asteroid::Drift>,
    /// lidar sweeps running or cooling down (see cloak.rs)
    #[serde(default)]
    pub sweeps: Vec<crate::cloak::Sweep>,
    /// orbital minefields (see bastion.rs)
    #[serde(default)]
    pub minefields: Vec<crate::bastion::Minefield>,
    /// mass driver reloads and interdiction grid kills (see ordnance.rs)
    #[serde(default)]
    pub station_shots: Vec<crate::ordnance::Shot>,
    /// stargate throughput log (see transit.rs)
    #[serde(default)]
    pub gate_uses: Vec<crate::transit::GateUse>,
}

/// Commands players send. Applied at the current game time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Command {
    /// Auto-planned transfer (multi-leg routes are planned leg by leg on arrival).
    Transfer { ships: Vec<ShipId>, to: BodyId, depart_t: Option<f64> },
    /// A course picked from `plan::options` (gravity assist, faster flight...), re-planned from its hint.
    TransferPlanned { ships: Vec<ShipId>, to: BodyId, depart_t: f64, hint: PlanHint },
    CancelDeparture { ship: ShipId },
    /// Replace the manual burn list of a flying ship.
    SetBurns { ship: ShipId, burns: Vec<Burn> },
    Build { body: BodyId, class: ShipClass },
    Upgrade { body: BodyId, upgrade: Upgrade },
    SetOrder { ship: ShipId, order: StandingOrder },
    /// Start researching `tech` at a body with a research lab.
    Research { body: BodyId, tech: Tech },
    /// Take ships dark (heat-sink cloak) or bring them out of the dark.
    Cloak { ships: Vec<ShipId>, on: bool },
    /// Change what class a decoy shows as to enemy sensors.
    SetDecoy { ship: ShipId, looks_like: ShipClass },
    /// Lidar sweep from one of your scanning stations.
    LidarSweep { station: crate::station::StationId },
    /// Seed (or top up) an orbital minefield with a construction ship parked there.
    LayMines { body: BodyId },
    /// Load a slug at your mass driver there (it appears as a ship: send it at a body).
    LoadSlug { body: BodyId },
    /// Jump ships parked at one of your stargates to another of your gates.
    Jump { ships: Vec<ShipId>, to: BodyId },
    BuildStation { body: BodyId, kind: StationKind },
    /// A construction ship coasting around the sun stops where it is and founds an outpost there
    /// (a new body on its orbit, see outpost.rs).
    FoundOutpost { ship: ShipId },
    Chat { text: String },
    Extend { seconds: f64 },
    /// Form, split, merge, rename or disband fleets (see fleet.rs).
    Fleet(crate::fleet::FleetCommand),
    /// Bolt a drive onto an asteroid and push it (burn.t = middle of the low-thrust burn).
    PushAsteroid { body: BodyId, burn: Burn },
    /// Auto-plan the cheapest push that crashes `body` into `target`.
    AimAsteroid { body: BodyId, target: BodyId },
    /// Give the ship from the shipyard job at `body` finishing at `done_t` a transfer (`to: None` clears it).
    QueueOrder { body: BodyId, done_t: f64, to: Option<BodyId>, depart_t: Option<f64> },
    /// Cancel entry `index` of a body's build queue (yours only); refunds its cost, later jobs move up.
    CancelJob { body: BodyId, index: usize },
    /// Move entry `from` of a body's build queue to position `to` (both yours); finish times are recomputed.
    MoveJob { body: BodyId, from: usize, to: usize },
    /// Host only: game seconds per real second from now on.
    SetSpeed { speed: f64 },
    /// Queue a build/upgrade/station/research that starts by itself once it's affordable (now, if it is).
    Wait { wish: crate::waitlist::Wish },
    /// Drop an item from the waiting-for-funds list.
    Unwait { id: u32 },
    /// Move a waiting item to position `to` of the list (earlier = funded first).
    MoveWait { id: u32, to: usize },
    /// Meet enemy ship `target` at `arrive_t` (a course from `intercept::options`, re-planned from
    /// these numbers); `matched` = match its velocity there. Ships parked together or one fleet.
    Intercept { ships: Vec<ShipId>, target: ShipId, depart_t: f64, arrive_t: f64, matched: bool },
    /// New course for ships already in flight (one fleet): burn at `depart_t` towards `to`,
    /// re-planned from these numbers (`reroute::options`).
    Reroute { ships: Vec<ShipId>, to: BodyId, depart_t: f64, aim_t: f64 },
}

pub const TICK: f64 = 60.0;
/// Most ships one player can have in orbit of one body, counting ships on their way there and
/// ships in its build queue (keeps huge stacks, and the lag they cause, out of the game).
pub const ORBIT_CAP: usize = 12;

/// A game time that may be "never" (INFINITY). JSON has no infinity (serde_json writes `null`, which
/// then fails to load), so it is stored as a huge finite number and read back as INFINITY.
pub fn ser_time<S: serde::Serializer>(t: &f64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_f64(if t.is_finite() { *t } else { NEVER })
}

/// Reads `ser_time` values; also accepts `null` (files written before the fix) as INFINITY.
pub fn de_time<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let v: Option<f64> = Deserialize::deserialize(d)?;
    Ok(match v {
        Some(t) if t < NEVER * 0.5 => t,
        _ => f64::INFINITY,
    })
}

const NEVER: f64 = 1e300;
/// Power a parked construction ship or scout adds in orbit (Jonas: they can claim bodies).
pub const CONSTRUCTOR_POWER: f64 = 1.0;
/// Current rules revision for new matches. 1: matter yield by body kind and planet size. 2: advanced tech tier.
pub const RULES: u32 = 2;
const FACTIONS: [&str; 4] = ["vanguard", "hive", "prism", "junker"];

impl Game {
    pub fn new(code: String, settings: Settings, seed: u64, host_name: &str) -> Game {
        let system = generate(seed, settings.size);
        let bodies = vec![BodyState::default(); system.bodies.len()];
        let end_t = settings.length;
        let mut g = Game {
            rules: RULES,
            code,
            settings,
            status: Status::Lobby,
            host: 0,
            t: 0.0,
            end_t,
            system,
            players: vec![],
            ships: vec![],
            bodies,
            chat: vec![],
            events: vec![],
            next_ship: 1,
            next_event: 1,
            last_tick: 0.0,
            winner: None,
            started_real: 0.0,
            stations: vec![],
            next_station: 0,
            fleets: vec![],
            next_fleet: 0,
            drives: vec![],
            drifts: vec![],
            sweeps: vec![],
            minefields: vec![],
            station_shots: vec![],
            gate_uses: vec![],
        };
        let _ = g.add_player(host_name, None);
        g
    }

    pub fn player_by_name(&self, name: &str) -> Option<&Player> {
        self.players.iter().find(|p| p.name == name)
    }

    pub fn add_player(&mut self, name: &str, bot: Option<String>) -> Result<PlayerId, String> {
        if self.status != Status::Lobby {
            return Err("game already started".into());
        }
        if self.players.len() >= self.settings.max_players {
            return Err("game is full".into());
        }
        if let Some(p) = self.player_by_name(name) {
            return Ok(p.id);
        }
        let id = self.players.len() as PlayerId;
        self.players.push(Player {
            id,
            name: name.into(),
            faction: FACTIONS[id as usize % 4].into(),
            color: id as usize % 4,
            matter: 0.0,
            energy: 0.0,
            bot,
            home: None,
            bot_avoid: vec![],
            intel: vec![],
            techs: vec![],
            research: vec![],
            last_active: 0.0,
            sensor_log: vec![],
            contacts: vec![],
            vision: Default::default(),
            waiting: vec![],
            next_wait: 0,
        });
        self.preview_homes();
        Ok(id)
    }

    /// Lobby only: take a player (never the host) out again. Ids and colours are re-numbered,
    /// which is safe because nothing but chat refers to players before the start.
    pub fn remove_player(&mut self, id: PlayerId) -> Result<(), String> {
        if self.status != Status::Lobby {
            return Err("game already started".into());
        }
        if id == self.host {
            return Err("the host can't be removed".into());
        }
        if id as usize >= self.players.len() {
            return Err("no such player".into());
        }
        self.players.remove(id as usize);
        for (i, p) in self.players.iter_mut().enumerate() {
            p.id = i as PlayerId;
            p.color = i % 4;
        }
        self.chat.retain(|c| c.from != id);
        for c in &mut self.chat {
            if c.from > id {
                c.from -= 1;
            }
        }
        if self.host > id {
            self.host -= 1;
        }
        self.preview_homes();
        Ok(())
    }

    /// While waiting in the lobby, show everyone where they'll start (no owner or fleet yet).
    /// Same deterministic pick as `start`, so the preview is what you get if nobody else joins.
    fn preview_homes(&mut self) {
        if self.status != Status::Lobby {
            return;
        }
        let homes = pick_homes(&self.system, self.players.len());
        for (i, p) in self.players.iter_mut().enumerate() {
            p.home = homes.get(i).copied();
        }
    }

    /// Assign homes and starting fleets, then start the clock.
    pub fn start(&mut self) {
        if self.status != Status::Lobby {
            return;
        }
        let n = self.players.len();
        let homes = pick_homes(&self.system, n);
        for (i, home) in homes.into_iter().enumerate() {
            let p = &mut self.players[i];
            p.home = Some(home);
            p.matter = 400.0;
            p.energy = 200.0;
            let b = &mut self.bodies[home.0 as usize];
            b.owner = Some(i as PlayerId);
            for (u, l) in [(Upgrade::Mining, 1), (Upgrade::Solar, 1), (Upgrade::Refinery, 1), (Upgrade::Shipyard, 1), (Upgrade::Defense, 1), (Upgrade::Telescope, 1)] {
                b.levels[u.index()] = l;
            }
            b.fuel = 1500.0;
            b.def_hp = b.def_max();
            for class in [ShipClass::Gunship, ShipClass::Gunship, ShipClass::Interceptor, ShipClass::Interceptor, ShipClass::Bomber, ShipClass::Tanker, ShipClass::Scout] {
                self.spawn(i as PlayerId, class, home);
            }
        }
        self.status = Status::Running;
        self.update_intel();
        self.event(None, EventKind::Info, "Game started", "Fleets are fuelled. Good hunting.", None, None);
    }

    /// Game time corresponding to a unix time.
    pub fn time_at(&self, real: f64) -> f64 {
        ((real - self.started_real) * self.settings.speed).max(0.0)
    }

    pub fn spawn(&mut self, owner: PlayerId, class: ShipClass, at: BodyId) -> ShipId {
        let id = ShipId(self.next_ship);
        self.next_ship += 1;
        let s = class.stats();
        let n = self.ships.iter().filter(|x| x.owner == owner && x.class == class).count() + 1;
        self.ships.push(Ship {
            id,
            owner,
            class,
            name: format!("{} {}", s.name, n),
            hp: s.hp,
            fuel: s.fuel_cap,
            order: StandingOrder::Hold,
            route: vec![],
            state: ShipState::Parked { body: at, departure: None },
            tech: ShipTech::default(),
            last_burn_t: no_burn(),
            intercept: None,
            cloak: None,
            mimic: (class == ShipClass::Decoy).then_some(ShipClass::Gunship),
        });
        self.refit(owner);
        let s = self.ships.last_mut().unwrap();
        s.hp = s.stats().hp;
        s.fuel = s.stats().fuel_cap;
        id
    }

    /// Apply the owner's ship research to all their ships (hull and fuel scale with the new caps).
    pub(crate) fn refit(&mut self, owner: PlayerId) {
        let m = self.mods(owner);
        let tech = ShipTech { ve: m.ve, fuel_cap: m.fuel_cap, hp: m.hp };
        for s in self.ships.iter_mut().filter(|s| s.owner == owner && s.tech != tech) {
            s.hp *= tech.hp / s.tech.hp;
            s.tech = tech;
        }
    }

    /// Damage per hour of a body's planetary defenses, with the owner's research.
    pub fn def_attack(&self, body: BodyId) -> f64 {
        let b = &self.bodies[body.0 as usize];
        b.def_attack() * b.owner.map(|o| self.mods(o).def_attack).unwrap_or(1.0)
    }
    /// Max hit points of a body's planetary defenses, with the owner's research.
    pub fn def_max(&self, body: BodyId) -> f64 {
        let b = &self.bodies[body.0 as usize];
        b.def_max() * b.owner.map(|o| self.mods(o).def_hp).unwrap_or(1.0)
    }

    pub fn ship(&self, id: ShipId) -> Option<&Ship> {
        self.ships.iter().find(|s| s.id == id)
    }
    pub(crate) fn ship_mut(&mut self, id: ShipId) -> Option<&mut Ship> {
        self.ships.iter_mut().find(|s| s.id == id)
    }

    pub(crate) fn event(&mut self, to: Option<PlayerId>, kind: EventKind, title: &str, body: &str, body_ref: Option<BodyId>, ship_ref: Option<ShipId>) {
        let seq = self.next_event;
        self.next_event += 1;
        self.events.push(Event { seq, t: self.t, to, kind, title: title.into(), body: body.into(), body_ref, ship_ref });
        if self.events.len() > 400 {
            self.events.drain(..100);
        }
    }

    /// Absolute ship position at time t (parked ships sit on their body).
    pub fn ship_pos(&self, ship: &Ship, t: f64) -> V2 {
        match &ship.state {
            ShipState::Parked { body, .. } => self.system.pos(*body, t),
            ShipState::Flying(f) => {
                let tr = f.compute(&self.system);
                tr.abs_pos(&self.system, t).unwrap_or_else(|| {
                    let s = tr.segs.last().unwrap();
                    self.system.pos(s.parent, t) + crate::kepler::propagate(self.system.body(s.parent).mu, s.r0, s.v0, s.t1 - s.t0).0
                })
            }
        }
    }

    // ------------------------------------------------------------------ commands

    pub fn apply(&mut self, player: PlayerId, cmd: Command) -> Result<(), String> {
        if self.status == Status::Ended && !matches!(cmd, Command::Chat { .. }) {
            return Err("game over".into());
        }
        if self.status == Status::Lobby && !matches!(cmd, Command::Chat { .. }) {
            return Err("game has not started".into());
        }
        let n = self.system.bodies.len() as u32;
        let bad = |b: &BodyId| b.0 >= n;
        match &cmd {
            Command::Transfer { to, .. } | Command::TransferPlanned { to, .. } if bad(to) => return Err("no such body".into()),
            Command::TransferPlanned { depart_t, hint, .. } if !depart_t.is_finite() || !hint_ok(hint) => return Err("invalid course".into()),
            Command::Build { body, .. }
            | Command::Upgrade { body, .. }
            | Command::Research { body, .. }
            | Command::BuildStation { body, .. }
            | Command::PushAsteroid { body, .. }
            | Command::QueueOrder { body, .. }
            | Command::CancelJob { body, .. }
            | Command::MoveJob { body, .. }
            | Command::LayMines { body }
            | Command::LoadSlug { body }
            | Command::Jump { to: body, .. }
                if bad(body) =>
            {
                return Err("no such body".into())
            }
            Command::SetBurns { burns, .. } if burns.iter().any(|b| !b.t.is_finite() || !b.prograde.is_finite() || !b.radial.is_finite() || b.dv() > 1000.0) => {
                return Err("invalid burn".into())
            }
            Command::AimAsteroid { body, target } if bad(body) || bad(target) => return Err("no such body".into()),
            Command::PushAsteroid { burn, .. } if !burn.t.is_finite() || !burn.prograde.is_finite() || !burn.radial.is_finite() => return Err("invalid burn".into()),
            Command::Transfer { depart_t: Some(t), .. } | Command::QueueOrder { depart_t: Some(t), .. } if !t.is_finite() => {
                return Err("invalid departure time".into())
            }
            Command::QueueOrder { to: Some(to), .. } if bad(to) => return Err("no such body".into()),
            Command::SetSpeed { speed } if !speed.is_finite() || *speed <= 0.0 => return Err("invalid speed".into()),
            Command::Extend { seconds } if !seconds.is_finite() => return Err("invalid duration".into()),
            Command::Intercept { depart_t, arrive_t, .. } if !depart_t.is_finite() || !arrive_t.is_finite() => return Err("invalid course".into()),
            Command::Reroute { to, .. } if bad(to) => return Err("no such body".into()),
            Command::Reroute { depart_t, aim_t, .. } if !depart_t.is_finite() || !aim_t.is_finite() => return Err("invalid course".into()),
            _ => {}
        }
        // outposts the player doesn't know about don't exist for them; abandoned ones take no new arrivals
        let target = match &cmd {
            Command::Transfer { to, .. } | Command::TransferPlanned { to, .. } | Command::Reroute { to, .. } | Command::QueueOrder { to: Some(to), .. } | Command::Jump { to, .. } => Some(*to),
            Command::AimAsteroid { target, .. } => Some(*target),
            Command::Build { body, .. } | Command::Upgrade { body, .. } | Command::Research { body, .. } | Command::BuildStation { body, .. } | Command::LayMines { body } | Command::LoadSlug { body } => Some(*body),
            _ => None,
        };
        if let Some(b) = target {
            if self.outpost_hidden(player, b) {
                return Err("no such body".into());
            }
            if self.abandoned(b) {
                return Err(format!("{} is abandoned", self.system.body(b).name));
            }
            if self.is_outpost(b) && matches!(cmd, Command::Upgrade { .. } | Command::Research { .. }) {
                return Err("an outpost has no surface to build on: use stations".into());
            }
        }
        match cmd {
            Command::Fleet(c) => self.fleet_command(player, c),
            Command::Wait { wish } => self.wait_for_funds(player, wish),
            Command::Unwait { id } => self.unwait(player, id),
            Command::MoveWait { id, to } => self.move_wait(player, id, to),
            Command::Chat { text } => {
                let text: String = text.chars().take(500).collect();
                self.chat.push(ChatMsg { from: player, t: self.t, text });
                Ok(())
            }
            Command::MoveJob { body, from, to } => {
                let now = self.t;
                let q = &mut self.bodies[body.0 as usize].queue;
                if from >= q.len() || to >= q.len() || q[from].owner != player || q[to].owner != player || q[from].lane() != q[to].lane() {
                    return Err("can only reorder your own queued jobs within one queue".into());
                }
                let lane = q[from].lane();
                // remaining time of each job in this lane, in the current order
                let mut prev = now;
                let mut dur = vec![0.0; q.len()];
                for i in (0..q.len()).filter(|&i| q[i].lane() == lane) {
                    dur[i] = (q[i].done_t - prev.max(now)).max(0.0);
                    prev = q[i].done_t;
                }
                let job = q.remove(from);
                let d = dur.remove(from);
                q.insert(to, job);
                dur.insert(to, d);
                let mut end = now;
                for i in 0..q.len() {
                    if q[i].lane() == lane {
                        end += dur[i];
                        q[i].done_t = end;
                    }
                }
                Ok(())
            }
            Command::CancelJob { body, index } => {
                let now = self.t;
                let q = &mut self.bodies[body.0 as usize].queue;
                let job = q.get(index).ok_or("that job is no longer queued")?;
                if job.owner != player {
                    return Err("not your job".into());
                }
                // how long this job occupies its lane: from the previous job in the lane (or now)
                let lane = job.lane();
                let start = q[..index].iter().filter(|j| j.lane() == lane).map(|j| j.done_t).fold(now, f64::max);
                let span = (job.done_t - start).max(0.0);
                let job = q.remove(index);
                for j in q.iter_mut().skip(index).filter(|j| j.lane() == lane) {
                    j.done_t -= span;
                }
                let (m, e) = match job.kind {
                    JobKind::Ship(c) => (c.stats().matter, c.stats().energy),
                    JobKind::Upgrade(u) => {
                        let b = &self.bodies[body.0 as usize];
                        let pending = b.queue.iter().filter(|j| matches!(j.kind, JobKind::Upgrade(x) if x == u)).count() as u8;
                        let (m, e, _) = u.cost(b.level(u) + pending + 1);
                        (m, e)
                    }
                };
                let p = &mut self.players[player as usize];
                p.matter += m;
                p.energy += e;
                Ok(())
            }
            Command::QueueOrder { body, done_t, to, depart_t } => {
                let job = self.bodies[body.0 as usize]
                    .queue
                    .iter_mut()
                    .find(|j| j.owner == player && matches!(j.kind, JobKind::Ship(_)) && (j.done_t - done_t).abs() < 1.0)
                    .ok_or("that ship is no longer in the build queue")?;
                job.then = to.filter(|&to| to != body).map(|to| QueuedOrder { to, depart_t });
                Ok(())
            }
            Command::SetSpeed { speed } => {
                if player != self.host {
                    return Err("only the host can change the game speed".into());
                }
                let speed = speed.clamp(1.0, 1000.0);
                // keep game time continuous: real_now = started_real + t / speed for old and new speed
                if self.started_real > 0.0 {
                    self.started_real += self.t / self.settings.speed - self.t / speed;
                }
                self.settings.speed = speed;
                self.event(None, EventKind::Info, "Game speed", &format!("{speed}× from now on"), None, None);
                Ok(())
            }
            Command::Extend { seconds } => {
                if player != self.host {
                    return Err("only the host can extend the game".into());
                }
                self.end_t += seconds.clamp(0.0, 30.0 * 86_400.0);
                self.event(None, EventKind::Info, "Game extended", &format!("+{:.1} days", seconds / 86_400.0), None, None);
                Ok(())
            }
            Command::Transfer { ships, to, depart_t } => self.transfer(player, &ships, to, depart_t, None),
            Command::PushAsteroid { body, burn } => {
                let plan = Ok(crate::asteroid::plan_push(self, body, burn));
                self.order_push(player, body, plan)
            }
            Command::AimAsteroid { body, target } => {
                let plan = crate::asteroid::plan_impact(self, body, target, self.t).ok_or_else(|| "no collision course within reach".to_string());
                self.order_push(player, body, plan)
            }
            Command::TransferPlanned { ships, to, depart_t, hint } => self.transfer(player, &ships, to, Some(depart_t), Some(hint)),
            Command::Reroute { ships, to, depart_t, aim_t } => self.reroute(player, &ships, crate::reroute::Hint { to, depart_t, aim_t }),
            Command::Intercept { ships, target, depart_t, arrive_t, matched } => {
                self.no_slug(&ships)?;
                self.order_intercept(player, &ships, crate::intercept::Hint { target, depart_t, arrive_t, matched })
            }
            Command::CancelDeparture { ship } => {
                self.own_ship(player, ship)?;
                if !matches!(self.ship(ship).unwrap().state, ShipState::Parked { departure: Some(_), .. }) {
                    return Err("ship is not waiting to depart".into());
                }
                for id in self.with_fleetmates(ship) {
                    let s = self.ship_mut(id).unwrap();
                    if let ShipState::Parked { departure, .. } = &mut s.state {
                        *departure = None;
                        s.route.clear();
                        s.intercept = None;
                    }
                }
                Ok(())
            }
            Command::SetBurns { ship, mut burns } => {
                let now = self.t;
                let s = self.own_ship(player, ship)?;
                if !matches!(s.state, ShipState::Flying(_)) {
                    return Err("ship is not in flight".into());
                }
                burns.retain(|b| b.t > now && b.dv().is_finite());
                burns.sort_by(|a, b| a.t.total_cmp(&b.t));
                // a fleet flies one course
                for id in self.with_fleetmates(ship) {
                    let s = self.ship_mut(id).unwrap();
                    s.intercept = None;
                    if let ShipState::Flying(f) = &mut s.state {
                        f.burns = burns.clone();
                        f.cache = None;
                    }
                }
                Ok(())
            }
            Command::Research { body, tech } => self.start_research(player, body, tech),
            Command::Cloak { ships, on } => self.set_cloak(player, &ships, on),
            Command::SetDecoy { ship, looks_like } => self.set_decoy(player, ship, looks_like),
            Command::LidarSweep { station } => self.lidar_sweep(player, station),
            Command::LayMines { body } => self.lay_mines(player, body),
            Command::LoadSlug { body } => self.load_slug(player, body).map(|_| ()),
            Command::Jump { ships, to } => self.jump(player, &ships, to),
            Command::BuildStation { body, kind } => self.build_station(player, body, kind).map(|_| ()),
            Command::FoundOutpost { ship } => self.found_outpost(player, ship).map(|_| ()),
            Command::SetOrder { ship, order } => {
                self.own_ship(player, ship)?;
                for id in self.with_fleetmates(ship) {
                    self.ship_mut(id).unwrap().order = order;
                }
                Ok(())
            }
            Command::Build { body, class } => {
                let st = class.stats();
                if self.orbit_load(player, body) >= ORBIT_CAP {
                    return Err(format!("orbit full: at most {ORBIT_CAP} of your ships per body (counting ships on the way and in the build queue)"));
                }
                let yard = self.yard_level(player, body);
                if yard == 0 {
                    return Err("you need the body or a shipyard station there".into());
                }
                if yard < st.min_shipyard {
                    return Err(format!("needs shipyard level {}", st.min_shipyard));
                }
                if class == ShipClass::Slug {
                    return Err("slugs are loaded at a mass driver".into());
                }
                if let Some(t) = class.needs().filter(|&t| !self.has_tech(player, t)) {
                    return Err(format!("needs {}", t.label()));
                }
                let b = &self.bodies[body.0 as usize];
                let busy_until = b.lane_free_at(Lane::Ships(player), self.t);
                self.pay(player, st.matter, st.energy)?;
                let speed = (1.0 + 0.5 * (yard as f64 - 1.0)) * self.mods(player).build_speed;
                self.bodies[body.0 as usize].queue.push(Job { kind: JobKind::Ship(class), owner: player, done_t: busy_until + st.build_time / speed, then: None });
                Ok(())
            }
            Command::Upgrade { body, upgrade } => {
                let b = &self.bodies[body.0 as usize];
                if b.owner != Some(player) {
                    return Err("you don't own that body".into());
                }
                let pending = b.queue.iter().filter(|j| matches!(j.kind, JobKind::Upgrade(u) if u == upgrade)).count() as u8;
                let next = b.level(upgrade) + pending + 1;
                if next > Upgrade::MAX_LEVEL {
                    return Err("already at max level".into());
                }
                let (m, e, secs) = upgrade.cost(next);
                let busy_until = b.lane_free_at(Lane::Upgrades, self.t);
                self.pay(player, m, e)?;
                self.bodies[body.0 as usize].queue.push(Job { kind: JobKind::Upgrade(upgrade), owner: player, done_t: busy_until + secs, then: None });
                Ok(())
            }
        }
    }

    /// Ships `p` has at, heading to, or being built at `body` (see `ORBIT_CAP`).
    pub fn orbit_load(&self, p: PlayerId, body: BodyId) -> usize {
        let ships = self.ships.iter().filter(|s| s.owner == p && (s.parked_at() == Some(body) && s.dest().is_none_or(|d| d == body) || s.dest() == Some(body))).count();
        let queued = self.bodies[body.0 as usize].queue.iter().filter(|j| j.owner == p && matches!(j.kind, JobKind::Ship(_))).count();
        ships + queued
    }

    /// Shipyard level `p` can build at: the body's own shipyard if theirs, or a shipyard station.
    pub fn yard_level(&self, p: PlayerId, body: BodyId) -> u8 {
        let b = &self.bodies[body.0 as usize];
        let own = if b.owner == Some(p) { b.level(Upgrade::Shipyard) } else { 0 };
        own.max(self.station_yard_level(p, body))
    }

    fn own_ship(&mut self, player: PlayerId, id: ShipId) -> Result<&mut Ship, String> {
        let s = self.ship_mut(id).ok_or("no such ship")?;
        if s.owner != player {
            return Err("not your ship".into());
        }
        Ok(s)
    }

    pub(crate) fn pay(&mut self, player: PlayerId, m: f64, e: f64) -> Result<(), String> {
        let p = &mut self.players[player as usize];
        if p.matter < m || p.energy < e {
            return Err(format!("needs {m:.0} matter and {e:.0} energy"));
        }
        p.matter -= m;
        p.energy -= e;
        Ok(())
    }

    /// The plan a transfer order would use (same code on client and server).
    pub fn plan_for(&self, ship: &Ship, to: BodyId, depart_t: Option<f64>) -> Result<(TransferPlan, Vec<BodyId>), String> {
        let from = ship.parked_at().ok_or("ship must be parked in orbit to plan a transfer")?;
        let hops = plan::route(&self.system, from, to);
        let first = *hops.first().ok_or("already there")?;
        // direct legs only: this runs inside the server lock (bots, route hops). Gravity assists and
        // fast courses are searched on the client (`plan::options`) and ordered with a hint.
        // bots keep the cheaper planner (no departure probe): they plan inside the server lock
        let bot = self.players.get(ship.owner as usize).is_some_and(|p| p.bot.is_some());
        let plan_fn = if bot { plan::plan_leg_quick } else { plan::plan_leg };
        let p = plan_fn(&self.system, from, first, self.t, depart_t)?;
        if !p.converged {
            return Err("couldn't find a working trajectory, try another departure time".into());
        }
        Ok((p, hops[1..].to_vec()))
    }

    /// Re-plan a course the player picked from `plan::options` (cheap: one shooting run).
    pub fn plan_for_hinted(&self, ship: &Ship, to: BodyId, depart_t: f64, hint: PlanHint) -> Result<(TransferPlan, Vec<BodyId>), String> {
        let from = ship.parked_at().ok_or("ship must be parked in orbit to plan a transfer")?;
        let hops = plan::route(&self.system, from, to);
        let first = *hops.first().ok_or("already there")?;
        let p = plan::plan_hinted(&self.system, from, first, depart_t.max(self.t + 30.0), hint)?;
        Ok((p, hops[1..].to_vec()))
    }

    /// Transfer order for a set of ships (expanded to whole fleets). All or nothing: every ship is
    /// checked before any is scheduled. Ships leaving the same body share one plan; fleets also
    /// need fuel for the capture burn (their tankers can cover it in flight).
    pub(crate) fn transfer(&mut self, player: PlayerId, ships: &[ShipId], to: BodyId, depart_t: Option<f64>, hint: Option<PlanHint>) -> Result<(), String> {
        let ships = self.expand_fleets(ships);
        // ships already headed there (re-planning) don't count twice
        let new = ships.iter().filter(|&&id| self.ship(id).is_some_and(|s| s.dest() != Some(to))).count();
        if new > 0 && self.orbit_load(player, to) + new > ORBIT_CAP {
            let room = ORBIT_CAP.saturating_sub(self.orbit_load(player, to));
            return Err(format!("orbit full there: at most {ORBIT_CAP} of your ships per body, room for {room} more"));
        }
        let mut plans: Vec<(BodyId, (TransferPlan, Vec<BodyId>))> = vec![];
        let mut todo: Vec<(ShipId, usize)> = vec![];
        for &id in &ships {
            let ship = self.ship(id).ok_or("no such ship")?;
            if ship.owner != player {
                return Err("not your ship".into());
            }
            let from = ship.parked_at().ok_or("ship must be parked in orbit to plan a transfer")?;
            let k = match plans.iter().position(|p| p.0 == from) {
                Some(k) => k,
                None => {
                    let p = match (hint, depart_t) {
                        (Some(h), Some(td)) => self.plan_for_hinted(&ship.clone(), to, td, h)?,
                        _ => self.plan_for(&ship.clone(), to, depart_t)?,
                    };
                    plans.push((from, p));
                    plans.len() - 1
                }
            };
            let p = &plans[k].1 .0;
            let assist = self.launch_assist(player, from);
            if ship.fuel_for((p.depart_dv - assist).max(0.0) + p.mid_dv()) > ship.fuel {
                return Err(format!("not enough fuel: departure needs {:.1} km/s, {} has {:.1} km/s", p.depart_dv, ship.name, ship.dv_left()));
            }
            todo.push((id, k));
        }
        for f in self.fleets.iter().filter(|f| f.ships.iter().any(|s| ships.contains(s))) {
            let k = todo.iter().find(|t| f.ships.contains(&t.0)).unwrap().1;
            let p = &plans[k].1 .0;
            let assist = self.launch_assist(player, plans[k].0);
            self.fleet_fuel_check(&f.ships, (p.depart_dv - assist).max(0.0) + p.mid_dv(), p.capture_dv)?;
        }
        for (id, k) in todo {
            self.schedule(player, id, plans[k].1.clone())?;
        }
        Ok(())
    }

    fn schedule(&mut self, player: PlayerId, id: ShipId, (p, rest): (TransferPlan, Vec<BodyId>)) -> Result<(), String> {
        let ship = self.ship(id).ok_or("no such ship")?.clone();
        if ship.owner != player {
            return Err("not your ship".into());
        }
        let assist = ship.parked_at().map_or(0.0, |b| self.launch_assist(player, b));
        if ship.fuel_for((p.depart_dv - assist).max(0.0) + p.mid_dv()) > ship.fuel {
            return Err(format!("not enough fuel: departure needs {:.1} km/s, {} has {:.1} km/s", p.depart_dv, ship.name, ship.dv_left()));
        }
        let s = self.ship_mut(id).unwrap();
        s.route = rest;
        s.intercept = None;
        if let ShipState::Parked { departure, .. } = &mut s.state {
            *departure = Some(Departure { to: p.to, state: p.depart, dv: p.depart_dv, burns: p.burns.clone() });
        }
        Ok(())
    }

    // ------------------------------------------------------------------ simulation

    /// Advance the authoritative state to game time `t`.
    pub fn advance_to(&mut self, t: f64) {
        self.advance(t, true)
    }

    /// Prediction for the time scrubber: the same sim, but bots don't think (a player can't know their plans).
    pub fn forecast_to(&mut self, t: f64) {
        self.advance(t, false)
    }

    fn advance(&mut self, t: f64, bots: bool) {
        if self.status != Status::Running {
            return;
        }
        while self.t < t {
            let next_tick = self.last_tick + TICK;
            let step_end = next_tick.min(t);
            self.asteroid_events(step_end);
            self.ship_events(step_end);
            self.t = step_end;
            if step_end >= next_tick {
                self.last_tick = next_tick;
                self.economy(TICK);
                self.fund_waiting();
                self.jobs();
                self.research_tick();
                self.cloak_tick();
                self.stations_tick(TICK);
                self.mines_tick(TICK);
                self.combat_tick(TICK);
                self.ordnance_tick();
                self.fleet_tick(TICK);
                self.ownership();
                self.outposts_tick();
                self.update_intel();
                self.intercept_tick();
                if bots {
                    crate::bot::think_all(self);
                }
                if self.t >= self.end_t {
                    self.finish();
                    return;
                }
            }
        }
    }

    /// Departures, burns, captures and impacts up to time `until`, in order.
    fn ship_events(&mut self, until: f64) {
        loop {
            // earliest pending event across all ships
            let mut best: Option<(f64, usize)> = None;
            for (i, s) in self.ships.iter_mut().enumerate() {
                let te = match &mut s.state {
                    ShipState::Parked { departure: Some(d), .. } => d.state.t,
                    ShipState::Parked { .. } => continue,
                    ShipState::Flying(f) => {
                        let nb = f.burns.first().map(|b| b.t).unwrap_or(f64::INFINITY);
                        let end = match f.trajectory(&self.system).end {
                            End::Horizon { t } => t,
                            e => e.t(),
                        };
                        nb.min(end)
                    }
                };
                if te <= until && best.is_none_or(|b| te < b.0) {
                    best = Some((te, i));
                }
            }
            let Some((te, i)) = best else { return };
            self.t = self.t.max(te);
            self.ship_event(i, te);
        }
    }

    fn ship_event(&mut self, i: usize, te: f64) {
        // a laser launch array at the departure body pays for the first part of the burn
        let assist = match self.ships[i].state {
            ShipState::Parked { body, .. } => self.launch_assist(self.ships[i].owner, body),
            _ => 0.0,
        };
        let sys = &self.system;
        let ship = &mut self.ships[i];
        let (id, owner, name) = (ship.id, ship.owner, ship.name.clone());
        let cs = ship.stats();
        let rocket = |fuel: f64, dv: f64| (cs.dry_mass + fuel) * (1.0 - (-dv / cs.ve).exp());
        match &mut ship.state {
            ShipState::Parked { body, departure } => {
                let d = departure.take().unwrap();
                let need = rocket(ship.fuel, (d.dv - assist).max(0.0));
                if need > ship.fuel {
                    let body = *body;
                    ship.route.clear();
                    self.event(Some(owner), EventKind::Bad, "Departure aborted", &format!("{name} lacks fuel for the burn"), Some(body), Some(id));
                    return;
                }
                ship.fuel -= need;
                ship.last_burn_t = d.state.t;
                // an intercept course flies free (no capture) towards the meeting point
                let chasing = ship.intercept.is_some();
                ship.state = ShipState::Flying(Flight { start: d.state, burns: d.burns, dest: (!chasing).then_some(d.to), cache: None });
                let to = if chasing { "the intercept".to_string() } else { sys.body(d.to).name.clone() };
                self.event(Some(owner), EventKind::Info, "Departed", &format!("{name} burned {:.1} km/s for {to}", d.dv), None, Some(id));
            }
            ShipState::Flying(f) => {
                let tr = f.trajectory(sys).clone();
                let burn_due = f.burns.first().is_some_and(|b| b.t <= te && b.t <= tr.end.t());
                if burn_due {
                    let mut b = f.burns.remove(0);
                    let st = tr.state_at(sys, b.t).unwrap();
                    let mass = cs.dry_mass + ship.fuel;
                    let ve = cs.ve;
                    let need = mass * (1.0 - (-b.dv() / ve).exp());
                    if need > ship.fuel {
                        // burn what we have
                        let dv_av = ve * (mass / cs.dry_mass).ln();
                        let k = if b.dv() > 0.0 { dv_av / b.dv() } else { 0.0 };
                        b.prograde *= k;
                        b.radial *= k;
                        ship.fuel = 0.0;
                    } else {
                        ship.fuel -= need;
                    }
                    let dv = b.vector(st.r, st.v);
                    ship.last_burn_t = b.t;
                    f.start = State { v: st.v + dv, ..st };
                    f.cache = None;
                    return;
                }
                match tr.end {
                    End::Capture { body, rp, vp, .. } if ship.class == ShipClass::Slug && ship.route.is_empty() => {
                        let _ = rp;
                        self.slug_impact(id, body, vp);
                    }
                    End::Capture { body, rp, vp, .. } => {
                        let dv = vp - circular_speed(sys.body(body).mu, rp);
                        let need = rocket(ship.fuel, dv);
                        let bname = sys.body(body).name.clone();
                        if need <= ship.fuel {
                            ship.fuel -= need;
                            ship.last_burn_t = te;
                            ship.state = ShipState::Parked { body, departure: None };
                            self.event(Some(owner), EventKind::Good, "Arrived", &format!("{name} is in orbit of {bname}"), Some(body), Some(id));
                            self.continue_route(i);
                            self.trip_mines(id, body, dv);
                        } else {
                            // can't brake: fly past
                            let st = tr.state_at(sys, te).unwrap();
                            f.start = st;
                            f.dest = None;
                            f.cache = None;
                            ship.route.clear();
                            self.event(Some(owner), EventKind::Bad, "Flyby", &format!("{name} lacks fuel to brake at {bname}"), Some(body), Some(id));
                        }
                    }
                    End::Impact { body, .. } => {
                        let bname = sys.body(body).name.clone();
                        self.ships.remove(i);
                        self.event(Some(owner), EventKind::Bad, "Ship lost", &format!("{name} crashed into {bname}"), Some(body), None);
                    }
                    End::Horizon { t } => {
                        // keep coasting: rebase so the prediction window moves forward
                        match tr.state_at(sys, t.min(te)) {
                            Some(st) if st.t > f.start.t + 1.0 => {
                                f.start = st;
                                f.cache = None;
                            }
                            _ => {
                                // prediction can't make progress (pathological path): the ship is lost
                                self.ships.remove(i);
                                self.event(Some(owner), EventKind::Bad, "Ship lost", &format!("{name} was lost in deep space"), None, None);
                            }
                        }
                    }
                }
            }
        }
    }

    fn continue_route(&mut self, i: usize) {
        let ship = self.ships[i].clone();
        if let Some(&next) = ship.route.first() {
            let mut rest = ship.route.clone();
            rest.remove(0);
            // a fleetmate that arrived with us already planned this leg
            let here = ship.parked_at();
            let mate = self.with_fleetmates(ship.id).into_iter().filter(|&m| m != ship.id).find_map(|m| {
                let s = self.ship(m)?;
                match &s.state {
                    ShipState::Parked { body, departure: Some(d) } if Some(*body) == here && d.to == next && s.route == rest && d.state.t > self.t => Some(d.clone()),
                    _ => None,
                }
            });
            if let Some(d) = mate {
                if ship.fuel_for(d.dv) <= ship.fuel {
                    let s = &mut self.ships[i];
                    s.route = rest;
                    if let ShipState::Parked { departure, .. } = &mut s.state {
                        *departure = Some(d);
                    }
                    return;
                }
            }
            match self.plan_for(&ship, next, None) {
                Ok((p, _)) if ship.fuel_for(p.depart_dv + p.mid_dv()) <= ship.fuel => {
                    let s = &mut self.ships[i];
                    s.route = rest;
                    if let ShipState::Parked { departure, .. } = &mut s.state {
                        *departure = Some(Departure { to: p.to, state: p.depart, dv: p.depart_dv, burns: p.burns.clone() });
                    }
                }
                _ => {
                    self.ships[i].route.clear();
                    let body = ship.parked_at();
                    self.event(Some(ship.owner), EventKind::Bad, "Route stopped", &format!("{} can't plan the next leg", ship.name), body, Some(ship.id));
                }
            }
        }
    }

    /// Energy output multiplier for solar at a body (inverse square of distance from the star).
    pub fn solar_factor(&self, body: BodyId) -> f64 {
        let lin = self.system.lineage(body);
        let planet = lin[lin.len().saturating_sub(2)];
        let a = self.system.body(planet).orbit.map(|o| o.a).unwrap_or(4e5);
        (4e5 / a).powi(2).clamp(0.05, 8.0)
    }

    /// Base (matter, energy) per hour a body yields to its owner before upgrades.
    /// Rules 1: planets 90 matter scaled by size (radius / 1100 km, 0.6–1.8×), gas giants 60 matter +
    /// 40 energy, moons 30, asteroids 15; every body also gives 20 energy.
    pub fn base_yield(&self, body: BodyId) -> (f64, f64) {
        let b = self.system.body(body);
        if self.rules == 0 {
            return (if b.kind == BodyKind::Star { 0.0 } else { 60.0 }, 20.0);
        }
        match b.kind {
            BodyKind::Star => (0.0, 20.0),
            BodyKind::Planet => (90.0 * (b.radius / 1100.0).clamp(0.6, 1.8), 20.0),
            BodyKind::GasGiant => (60.0, 60.0),
            BodyKind::Moon => (30.0, 20.0),
            BodyKind::Asteroid => (15.0, 20.0),
            BodyKind::Outpost => (0.0, 0.0),
        }
    }

    /// Production per hour for a body (matter, energy, fuel) given its upgrades.
    pub fn production(&self, body: BodyId) -> (f64, f64, f64) {
        let b = &self.bodies[body.0 as usize];
        if b.owner.is_none() || b.shattered || b.gone {
            return (0.0, 0.0, 0.0);
        }
        let (base_m, base_e) = self.base_yield(body);
        let matter = base_m * (1.0 + b.level(Upgrade::Mining) as f64);
        let energy = base_e + 60.0 * b.level(Upgrade::Solar) as f64 * self.solar_factor(body) + 60.0 * b.level(Upgrade::Reactor) as f64;
        let fuel = 120.0 * b.level(Upgrade::Refinery) as f64;
        let m = self.mods(b.owner.unwrap());
        (matter * m.matter, energy * m.energy, fuel * m.fuel)
    }

    fn economy(&mut self, dt: f64) {
        let h = dt / 3600.0;
        for i in 0..self.bodies.len() {
            let Some(o) = self.bodies[i].owner else { continue };
            let (m, e, f) = self.production(BodyId(i as u32));
            let def_k = self.mods(o).def_hp;
            let p = &mut self.players[o as usize];
            p.matter += m * h;
            p.energy += e * h;
            // refining costs energy: 0.5 per tonne
            let want = (f * h).min(self.bodies[i].fuel_cap() - self.bodies[i].fuel).max(0.0);
            let made = want.min(p.energy / 0.5);
            p.energy -= made * 0.5;
            let b = &mut self.bodies[i];
            b.fuel += made;
            let max = b.def_max() * def_k;
            // guns are repaired only while nobody is shooting at them
            if !b.in_combat {
                b.def_hp = (b.def_hp + max * 0.1 * h).min(max);
            }
        }
        // refuelling: ships parked at a friendly body draw from its stock; tankers top up friends
        for i in 0..self.ships.len() {
            let Some(body) = self.ships[i].parked_at() else { continue };
            let owner = self.ships[i].owner;
            let bs = &mut self.bodies[body.0 as usize];
            let s = &mut self.ships[i];
            let room = s.stats().fuel_cap - s.fuel;
            if bs.owner == Some(owner) && room > 0.0 {
                let take = room.min(bs.fuel).min(200.0 * h);
                s.fuel += take;
                bs.fuel -= take;
            }
        }
        let tankers: Vec<usize> = (0..self.ships.len()).filter(|&i| self.ships[i].class == ShipClass::Tanker && self.ships[i].parked_at().is_some()).collect();
        for t in tankers {
            let (body, owner) = (self.ships[t].parked_at().unwrap(), self.ships[t].owner);
            for j in 0..self.ships.len() {
                if j == t || self.ships[j].owner != owner || self.ships[j].parked_at() != Some(body) || self.ships[j].class == ShipClass::Tanker {
                    continue;
                }
                let room = self.ships[j].stats().fuel_cap - self.ships[j].fuel;
                // a tanker keeps a reserve for its own trip home
                let spare = (self.ships[t].fuel - 0.3 * ShipClass::Tanker.stats().fuel_cap).max(0.0);
                let x = room.min(spare).min(300.0 * h);
                self.ships[t].fuel -= x;
                self.ships[j].fuel += x;
            }
        }
    }

    fn jobs(&mut self) {
        for i in 0..self.bodies.len() {
            let now = self.t;
            let (done, keep): (Vec<Job>, Vec<Job>) = std::mem::take(&mut self.bodies[i].queue).into_iter().partition(|j| j.done_t <= now);
            self.bodies[i].queue = keep;
            for j in done {
                if self.bodies[i].owner != Some(j.owner) && self.station_yard_level(j.owner, BodyId(i as u32)) == 0 {
                    continue; // lost the body (and has no shipyard station) while building
                }
                let body = BodyId(i as u32);
                let bname = self.system.body(body).name.clone();
                match j.kind {
                    JobKind::Ship(c) => {
                        let id = self.spawn(j.owner, c, body);
                        self.event(Some(j.owner), EventKind::Good, "Ship built", &format!("{} ready at {bname}", c.stats().name), Some(body), Some(id));
                        if let Some(o) = j.then {
                            let depart = o.depart_t.filter(|&t| t > self.t);
                            if let Err(e) = self.transfer(j.owner, &[id], o.to, depart, None) {
                                self.event(Some(j.owner), EventKind::Bad, "Order failed", &format!("{}: {e}", c.stats().name), Some(body), Some(id));
                            }
                        }
                    }
                    JobKind::Upgrade(_) if self.bodies[i].owner != Some(j.owner) => {}
                    JobKind::Upgrade(u) => {
                        let def_k = self.mods(j.owner).def_hp;
                        let b = &mut self.bodies[i];
                        b.levels[u.index()] = (b.levels[u.index()] + 1).min(Upgrade::MAX_LEVEL);
                        if u == Upgrade::Defense {
                            b.def_hp = b.def_max() * def_k;
                        }
                        let lvl = b.level(u);
                        self.event(Some(j.owner), EventKind::Good, "Upgrade done", &format!("{} level {lvl} at {bname}", u.label()), Some(body), None);
                    }
                }
            }
        }
    }

    /// Combat power of each player in orbit of a body (ships' attack + defenses for the owner).
    pub fn power_at(&self, body: BodyId) -> Vec<(PlayerId, f64)> {
        let mut out: Vec<(PlayerId, f64)> = vec![];
        let mut add = |p: PlayerId, x: f64| match out.iter_mut().find(|e| e.0 == p) {
            Some(e) => e.1 += x,
            None => out.push((p, x)),
        };
        for s in self.ships.iter().filter(|s| !s.dark(self.t)) {
            if s.parked_at() == Some(body) && s.class.stats().attack > 0.0 {
                add(s.owner, s.class.stats().attack * (s.hp / s.stats().hp).max(0.2));
            } else if s.parked_at() == Some(body) && matches!(s.class, ShipClass::Constructor | ShipClass::Scout) {
                // unarmed, but enough of a presence to claim an empty body
                add(s.owner, CONSTRUCTOR_POWER * (s.hp / s.stats().hp).max(0.2));
            }
        }
        for st in self.stations_at(body) {
            if st.kind.attack() > 0.0 {
                add(st.owner, st.kind.attack() * (st.hp / st.kind.max_hp()).max(0.2));
            }
        }
        let b = &self.bodies[body.0 as usize];
        if let Some(o) = b.owner {
            if b.def_hp > 0.0 {
                add(o, self.def_attack(body));
            }
        }
        out
    }

    /// Ships with the Retreat order leave a body where their side is clearly outgunned.
    pub(crate) fn retreats(&mut self, body: BodyId) {
        let powers = self.power_at(body);
        let total: f64 = powers.iter().map(|p| p.1).sum();
        let candidates: Vec<ShipId> = self
            .ships
            .iter()
            .filter(|s| s.parked_at() == Some(body) && s.order == StandingOrder::Retreat)
            .filter(|s| matches!(s.state, ShipState::Parked { departure: None, .. }))
            .filter(|s| {
                let mine = powers.iter().find(|p| p.0 == s.owner).map(|p| p.1).unwrap_or(0.0);
                total - mine > 2.0 * mine
            })
            .map(|s| s.id)
            .collect();
        let mut done: Vec<ShipId> = vec![];
        for id in candidates {
            if done.contains(&id) {
                continue;
            }
            let group = self.with_fleetmates(id);
            done.extend(group.iter().copied());
            let ship = self.ship(id).unwrap().clone();
            let safe = self
                .bodies
                .iter()
                .enumerate()
                .filter(|(i, b)| b.owner == Some(ship.owner) && *i as u32 != body.0)
                .map(|(i, _)| BodyId(i as u32))
                .min_by(|a, b| {
                    let pa = self.system.pos(*a, self.t).dist(self.system.pos(body, self.t));
                    let pb = self.system.pos(*b, self.t).dist(self.system.pos(body, self.t));
                    pa.total_cmp(&pb)
                });
            if let Some(to) = safe {
                if self.transfer(ship.owner, &group, to, Some(self.t + 60.0), None).is_ok() {
                    let who = self.fleet_of(id).map(|f| f.name.clone()).unwrap_or(ship.name.clone());
                    self.event(Some(ship.owner), EventKind::Bad, "Retreating", &format!("{who} is pulling back"), Some(body), Some(id));
                }
            }
        }
    }

    fn ownership(&mut self) {
        for bi in 0..self.bodies.len() {
            let body = BodyId(bi as u32);
            if self.system.body(body).is_star() || self.bodies[bi].shattered || self.bodies[bi].gone {
                continue;
            }
            let powers: Vec<(PlayerId, f64)> = self.power_at(body).into_iter().filter(|p| p.1 > 0.0).collect();
            if powers.len() == 1 {
                let p = powers[0].0;
                let b = &mut self.bodies[bi];
                if b.owner != Some(p) {
                    let prev = b.owner.replace(p);
                    b.queue.clear();
                    b.def_hp = 0.0;
                    let bname = self.system.body(body).name.clone();
                    self.event(Some(p), EventKind::Good, "Territory gained", &format!("You control {bname}"), Some(body), None);
                    if let Some(prev) = prev {
                        self.event(Some(prev), EventKind::Bad, "Territory lost", &format!("{bname} has fallen"), Some(body), None);
                    }
                }
            }
        }
    }

    /// Territory score: planets 3, moons and asteroids 1.
    pub fn score(&self, p: PlayerId) -> u32 {
        self.bodies
            .iter()
            .enumerate()
            .filter(|(_, b)| b.owner == Some(p))
            .map(|(i, _)| match self.system.bodies[i].kind {
                BodyKind::Planet | BodyKind::GasGiant => 3,
                _ => 1,
            })
            .sum()
    }

    fn finish(&mut self) {
        self.status = Status::Ended;
        let best = self.players.iter().max_by_key(|p| (self.score(p.id), std::cmp::Reverse(p.id))).map(|p| p.id);
        self.winner = best;
        if let Some(w) = best {
            let name = self.players[w as usize].name.clone();
            self.event(None, EventKind::Info, "Game over", &format!("{name} wins with {} territory", self.score(w)), None, None);
        }
    }

    /// Events visible to a player.
    pub fn events_for(&self, p: PlayerId) -> impl Iterator<Item = &Event> {
        self.events.iter().filter(move |e| e.to.is_none() || e.to == Some(p))
    }
}

/// Spread players over rocky planets, skipping the scorched innermost one when possible.
fn hint_ok(h: &PlanHint) -> bool {
    match *h {
        PlanHint::Direct { tof } => tof.is_finite() && tof > 0.0 && tof < 30.0 * 86_400.0,
        PlanHint::Assist { flyby_t, arrive_t, .. } => flyby_t.is_finite() && arrive_t.is_finite() && arrive_t - flyby_t < 30.0 * 86_400.0,
    }
}

fn pick_homes(sys: &System, n: usize) -> Vec<BodyId> {
    let mut rocky: Vec<BodyId> = sys.bodies.iter().filter(|b| b.kind == BodyKind::Planet).map(|b| b.id).collect();
    let giants: Vec<BodyId> = sys.bodies.iter().filter(|b| b.kind == BodyKind::GasGiant).map(|b| b.id).collect();
    if rocky.len() > n {
        rocky.remove(0);
    }
    rocky.extend(giants);
    let mut rng = Rng::new(sys.seed ^ 0xF00D);
    let mut out = vec![];
    let m = rocky.len();
    for i in 0..n.min(m) {
        out.push(rocky[(i * m) / n.max(1)]);
    }
    // shuffle who gets which
    for i in (1..out.len()).rev() {
        let j = rng.below(i as u32 + 1) as usize;
        out.swap(i, j);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn lobby_previews_homes_and_start_keeps_them() {
        let settings = Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 86_400.0, speed: 1.0 };
        let mut g = Game::new("ABCD".into(), settings, 5, "alice");
        assert!(g.players[0].home.is_some());
        g.add_player("bob", None).unwrap();
        g.add_player("Bot 1 (normal)", Some("normal".into())).unwrap();
        let homes: Vec<_> = g.players.iter().map(|p| p.home.unwrap()).collect();
        assert!(homes.iter().all(|h| g.bodies[h.0 as usize].owner.is_none()) && g.ships.is_empty());
        let mut uniq = homes.clone();
        uniq.dedup();
        assert_eq!(uniq.len(), 3);
        // removing the bot re-numbers and re-picks for two players
        assert!(g.remove_player(0).is_err());
        g.remove_player(2).unwrap();
        assert_eq!(g.players.len(), 2);
        let two: Vec<_> = g.players.iter().map(|p| p.home.unwrap()).collect();
        g.start();
        assert_eq!(g.players.iter().map(|p| p.home.unwrap()).collect::<Vec<_>>(), two);
        assert!(g.add_player("carol", None).is_err());
        assert!(g.remove_player(1).is_err());
    }

    #[test]
    fn remove_player_renumbers() {
        let settings = Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 86_400.0, speed: 1.0 };
        let mut g = Game::new("ABCD".into(), settings, 5, "alice");
        g.add_player("Bot 1 (easy)", Some("easy".into())).unwrap();
        g.add_player("bob", None).unwrap();
        g.apply(2, Command::Chat { text: "hi".into() }).unwrap();
        g.remove_player(1).unwrap();
        assert_eq!(g.players[1].name, "bob");
        assert_eq!((g.players[1].id, g.players[1].color), (1, 1));
        assert_eq!(g.chat.last().unwrap().from, 1);
    }

    #[test]
    fn start_gives_homes_and_fleets() {
        let g = game();
        assert_eq!(g.status, Status::Running);
        let h0 = g.players[0].home.unwrap();
        let h1 = g.players[1].home.unwrap();
        assert_ne!(h0, h1);
        assert_eq!(g.bodies[h0.0 as usize].owner, Some(0));
        assert_eq!(g.ships.iter().filter(|s| s.owner == 1).count(), 7);
        for s in &g.ships {
            assert!(s.dv_left() > 15.0, "{} has {:.1} km/s", s.name, s.dv_left());
        }
    }

    #[test]
    fn economy_and_building() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        let m0 = g.players[0].matter;
        g.apply(0, Command::Build { body: home, class: ShipClass::Interceptor }).unwrap();
        assert!(g.players[0].matter < m0);
        let n0 = g.ships.len();
        g.advance_to(3600.0);
        assert_eq!(g.ships.len(), n0 + 1);
        assert!(g.players[0].matter > m0 - 60.0);
    }

    #[test]
    fn transfer_arrives_and_captures_territory() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        let target = g
            .system
            .bodies
            .iter()
            .filter(|b| b.kind == BodyKind::Planet && g.bodies[b.id.0 as usize].owner.is_none())
            .min_by(|a, b| {
                let ha = g.system.body(home).orbit.unwrap().a;
                (a.orbit.unwrap().a - ha).abs().total_cmp(&(b.orbit.unwrap().a - ha).abs())
            })
            .unwrap()
            .id;
        let gunship = g.ships.iter().find(|s| s.owner == 0 && s.class == ShipClass::Gunship).unwrap().id;
        g.apply(0, Command::Transfer { ships: vec![gunship], to: target, depart_t: None }).unwrap();
        let ShipState::Parked { departure: Some(d), .. } = &g.ship(gunship).unwrap().state else { panic!() };
        let dep = d.state.t;
        g.advance_to(dep + 10.0);
        assert!(matches!(g.ship(gunship).unwrap().state, ShipState::Flying(_)));
        g.advance_to(dep + 6.0 * 86_400.0);
        let s = g.ship(gunship).unwrap();
        assert_eq!(s.parked_at(), Some(target), "{:?}", g.events.iter().rev().take(5).collect::<Vec<_>>());
        assert_eq!(g.bodies[target.0 as usize].owner, Some(0));
    }

    #[test]
    fn ships_and_upgrades_build_side_by_side() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        g.players[0].matter = 1e5;
        g.players[0].energy = 1e5;
        g.apply(0, Command::Build { body: home, class: ShipClass::Gunship }).unwrap();
        g.apply(0, Command::Upgrade { body: home, upgrade: Upgrade::Mining }).unwrap();
        g.apply(0, Command::Build { body: home, class: ShipClass::Scout }).unwrap();
        let q = g.bodies[home.0 as usize].queue.clone();
        let lvl = g.bodies[home.0 as usize].level(Upgrade::Mining);
        // the upgrade doesn't wait for the gunship, and the scout doesn't wait for the upgrade
        assert!((q[1].done_t - (g.t + Upgrade::Mining.cost(lvl + 1).2)).abs() < 1e-6);
        assert!(q[2].done_t > q[0].done_t, "ships still queue behind ships");
        let scout = q[2].done_t;
        assert!(g.apply(0, Command::MoveJob { body: home, from: 1, to: 0 }).is_err(), "lanes don't mix");
        let up = q[1].done_t;
        g.apply(0, Command::CancelJob { body: home, index: 0 }).unwrap();
        let q = &g.bodies[home.0 as usize].queue;
        assert_eq!(q[0].done_t, up, "cancelling a ship doesn't move the upgrade");
        assert!(q[1].done_t < scout, "the scout moves up");
    }

    #[test]
    fn cancel_job_refunds_and_moves_queue_up() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        let m0 = g.players[0].matter;
        g.apply(0, Command::Build { body: home, class: ShipClass::Interceptor }).unwrap();
        g.apply(0, Command::Build { body: home, class: ShipClass::Scout }).unwrap();
        let second = g.bodies[home.0 as usize].queue[1].done_t;
        g.apply(0, Command::CancelJob { body: home, index: 0 }).unwrap();
        let q = &g.bodies[home.0 as usize].queue;
        assert_eq!(q.len(), 1);
        assert!(q[0].done_t < second);
        assert!((g.players[0].matter - (m0 - ShipClass::Scout.stats().matter)).abs() < 1e-9);
        assert!(g.apply(1, Command::CancelJob { body: home, index: 0 }).is_err());
    }

    #[test]
    fn orbit_cap_limits_builds_and_arrivals() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        g.players[0].matter = 1e9;
        g.players[0].energy = 1e9;
        while g.orbit_load(0, home) < ORBIT_CAP {
            g.apply(0, Command::Build { body: home, class: ShipClass::Scout }).unwrap();
        }
        let e = g.apply(0, Command::Build { body: home, class: ShipClass::Scout }).unwrap_err();
        assert!(e.contains("orbit full"), "{e}");
        // the other player can still build at their own home
        let h1 = g.players[1].home.unwrap();
        g.players[1].matter = 1e9;
        g.players[1].energy = 1e9;
        assert!(g.apply(1, Command::Build { body: h1, class: ShipClass::Scout }).is_ok());
    }

    #[test]
    fn a_scout_claims_an_empty_body() {
        let mut g = game();
        let target = g.system.bodies.iter().find(|b| b.kind == BodyKind::Asteroid && g.bodies[b.id.0 as usize].owner.is_none()).unwrap().id;
        g.spawn(0, ShipClass::Scout, target);
        g.advance_to(g.t + 120.0);
        assert_eq!(g.bodies[target.0 as usize].owner, Some(0));
    }

    #[test]
    fn a_constructor_claims_an_empty_body() {
        let mut g = game();
        let target = g.system.bodies.iter().find(|b| b.kind == BodyKind::Asteroid && g.bodies[b.id.0 as usize].owner.is_none()).unwrap().id;
        let c = g.spawn(0, ShipClass::Constructor, target);
        g.advance_to(g.t + 120.0);
        assert_eq!(g.bodies[target.0 as usize].owner, Some(0));
        assert!(g.ship(c).is_some());
    }

    #[test]
    fn bots_expand() {
        let mut g = Game::new(
            "BOTS".into(),
            Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0 },
            9,
            "alice",
        );
        g.add_player("Bot 1", Some("hard".into())).unwrap();
        g.add_player("Bot 2", Some("normal".into())).unwrap();
        g.start();
        g.advance_to(1.0 * 86_400.0);
        for p in 1..2 {
            let owned = g.bodies.iter().filter(|b| b.owner == Some(p)).count();
            println!("bot {p}: {owned} bodies, {} ships", g.ships.iter().filter(|s| s.owner == p).count());
            assert!(owned >= 2, "bot {p} never expanded");
        }
    }

    #[test]
    fn under_attack_fires_once_per_battle() {
        let mut g = game();
        let home1 = g.players[1].home.unwrap();
        for _ in 0..3 {
            g.spawn(0, ShipClass::Bomber, home1);
        }
        g.advance_to(4.0 * 3600.0);
        let alerts = |g: &Game, p: PlayerId, title: &str| g.events_for(p).filter(|e| e.title == title && e.body_ref == Some(home1)).count();
        assert_eq!(alerts(&g, 1, "Under attack"), 1);
        assert_eq!(alerts(&g, 0, "Battle"), 1);
        assert_eq!(alerts(&g, 0, "Under attack"), 0);
    }

    #[test]
    fn combat_is_deterministic_and_rps() {
        let mut g = game();
        let home1 = g.players[1].home.unwrap();
        // drop three of alice's bombers into bob's home orbit by fiat
        for _ in 0..3 {
            g.spawn(0, ShipClass::Bomber, home1);
        }
        let mut g2 = g.clone();
        g.advance_to(4.0 * 3600.0);
        g2.advance_to(4.0 * 3600.0);
        assert_eq!(serde_json::to_string(&g.ships).unwrap(), serde_json::to_string(&g2.ships).unwrap());
        assert!(g.events.iter().any(|e| e.kind == EventKind::Battle));
        // bombers excel against gunships: bob's gunships should be the first to die
        assert!(g.ships.iter().filter(|s| s.owner == 1 && s.class == ShipClass::Gunship).count() < 2);
    }
}

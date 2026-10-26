//! Game rules: players, ships, economy, upgrades, deterministic combat and ownership by power
//! in orbit. `Game::advance_to` is the authoritative real-time step the server runs.
use crate::kepler::circular_speed;
use crate::plan::{self, TransferPlan};
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Weapon {
    None,
    /// slugs/missiles: hits harder, but the projectile is slow
    Kinetic,
    /// lasers: light-speed, lower damage
    Energy,
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
    pub const ALL: [ShipClass; 7] = [
        ShipClass::Scout,
        ShipClass::Interceptor,
        ShipClass::Gunship,
        ShipClass::Bomber,
        ShipClass::Dreadnought,
        ShipClass::Tanker,
        ShipClass::Constructor,
    ];

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
        }
    }

    /// Rock-paper-scissors damage multiplier of `self` attacking `target`.
    pub fn vs(self, target: ShipClass) -> f64 {
        use ShipClass::*;
        match (self, target) {
            (Interceptor, Bomber) | (Interceptor, Scout) => 2.0,
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
        let strong: Vec<&str> = ShipClass::ALL.iter().filter(|&&c| self.vs(c) > 1.0 && c != ShipClass::Tanker).map(|c| c.stats().name).collect();
        let weak: Vec<&str> = ShipClass::ALL.iter().filter(|&&c| self.vs(c) < 1.0).map(|c| c.stats().name).collect();
        let weapon = match s.weapon {
            Weapon::Kinetic => "kinetic slugs (hard-hitting, slow projectiles)",
            Weapon::Energy => "lasers (light-speed, lighter hits)",
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
    /// fog of war: last sightings of enemy ships
    #[serde(default)]
    pub contacts: Vec<crate::fog::Contact>,
    /// fog of war: what this player detects right now (recomputed every tick, so not saved or sent)
    #[serde(skip)]
    pub vision: crate::fog::Vision,
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
}

/// Commands players send. Applied at the current game time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Command {
    /// Auto-planned transfer (multi-leg routes are planned leg by leg on arrival).
    Transfer { ships: Vec<ShipId>, to: BodyId, depart_t: Option<f64> },
    CancelDeparture { ship: ShipId },
    /// Replace the manual burn list of a flying ship.
    SetBurns { ship: ShipId, burns: Vec<Burn> },
    Build { body: BodyId, class: ShipClass },
    Upgrade { body: BodyId, upgrade: Upgrade },
    SetOrder { ship: ShipId, order: StandingOrder },
    /// Start researching `tech` at a body with a research lab.
    Research { body: BodyId, tech: Tech },
    BuildStation { body: BodyId, kind: StationKind },
    Chat { text: String },
    Extend { seconds: f64 },
}

pub const TICK: f64 = 60.0;
const FACTIONS: [&str; 4] = ["vanguard", "hive", "prism", "junker"];

impl Game {
    pub fn new(code: String, settings: Settings, seed: u64, host_name: &str) -> Game {
        let system = generate(seed, settings.size);
        let bodies = vec![BodyState::default(); system.bodies.len()];
        let end_t = settings.length;
        let mut g = Game {
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
            contacts: vec![],
            vision: Default::default(),
        });
        Ok(id)
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

    pub(crate) fn spawn(&mut self, owner: PlayerId, class: ShipClass, at: BodyId) -> ShipId {
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
    fn ship_mut(&mut self, id: ShipId) -> Option<&mut Ship> {
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
            Command::Transfer { to, .. } if bad(to) => return Err("no such body".into()),
            Command::Build { body, .. } | Command::Upgrade { body, .. } | Command::Research { body, .. } | Command::BuildStation { body, .. } if bad(body) => {
                return Err("no such body".into())
            }
            Command::SetBurns { burns, .. } if burns.iter().any(|b| !b.t.is_finite() || !b.prograde.is_finite() || !b.radial.is_finite() || b.dv() > 1000.0) => {
                return Err("invalid burn".into())
            }
            Command::Transfer { depart_t: Some(t), .. } if !t.is_finite() => return Err("invalid departure time".into()),
            Command::Extend { seconds } if !seconds.is_finite() => return Err("invalid duration".into()),
            _ => {}
        }
        match cmd {
            Command::Chat { text } => {
                let text: String = text.chars().take(500).collect();
                self.chat.push(ChatMsg { from: player, t: self.t, text });
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
            Command::Transfer { ships, to, depart_t } => {
                // ships leaving the same body share one plan
                let mut plans: Vec<(BodyId, TransferPlan, Vec<BodyId>)> = vec![];
                for id in ships {
                    let ship = self.ship(id).ok_or("no such ship")?.clone();
                    let from = ship.parked_at().ok_or("ship must be parked in orbit to plan a transfer")?;
                    let cached = plans.iter().find(|p| p.0 == from).map(|p| (p.1.clone(), p.2.clone()));
                    let plan = match cached {
                        Some(p) => p,
                        None => {
                            let p = self.plan_for(&ship, to, depart_t)?;
                            plans.push((from, p.0.clone(), p.1.clone()));
                            p
                        }
                    };
                    self.schedule(player, id, plan)?;
                }
                Ok(())
            }
            Command::CancelDeparture { ship } => {
                let s = self.own_ship(player, ship)?;
                if let ShipState::Parked { departure, .. } = &mut s.state {
                    *departure = None;
                    s.route.clear();
                    Ok(())
                } else {
                    Err("ship is not waiting to depart".into())
                }
            }
            Command::SetBurns { ship, mut burns } => {
                let now = self.t;
                let s = self.own_ship(player, ship)?;
                let ShipState::Flying(f) = &mut s.state else { return Err("ship is not in flight".into()) };
                burns.retain(|b| b.t > now && b.dv().is_finite());
                burns.sort_by(|a, b| a.t.total_cmp(&b.t));
                f.burns = burns;
                f.cache = None;
                Ok(())
            }
            Command::Research { body, tech } => self.start_research(player, body, tech),
            Command::BuildStation { body, kind } => self.build_station(player, body, kind).map(|_| ()),
            Command::SetOrder { ship, order } => {
                self.own_ship(player, ship)?.order = order;
                Ok(())
            }
            Command::Build { body, class } => {
                let st = class.stats();
                let yard = self.yard_level(player, body);
                if yard == 0 {
                    return Err("you need the body or a shipyard station there".into());
                }
                if yard < st.min_shipyard {
                    return Err(format!("needs shipyard level {}", st.min_shipyard));
                }
                let b = &self.bodies[body.0 as usize];
                let busy_until = b.queue.iter().filter(|j| j.owner == player).map(|j| j.done_t).fold(self.t, f64::max);
                self.pay(player, st.matter, st.energy)?;
                let speed = (1.0 + 0.5 * (yard as f64 - 1.0)) * self.mods(player).build_speed;
                self.bodies[body.0 as usize].queue.push(Job { kind: JobKind::Ship(class), owner: player, done_t: busy_until + st.build_time / speed });
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
                let busy_until = b.queue.iter().map(|j| j.done_t).fold(self.t, f64::max);
                self.pay(player, m, e)?;
                self.bodies[body.0 as usize].queue.push(Job { kind: JobKind::Upgrade(upgrade), owner: player, done_t: busy_until + secs });
                Ok(())
            }
        }
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
        let p = plan::plan_leg(&self.system, from, first, self.t, depart_t)?;
        if !p.converged {
            return Err("couldn't find a working trajectory, try another departure time".into());
        }
        Ok((p, hops[1..].to_vec()))
    }

    pub(crate) fn order_transfer(&mut self, player: PlayerId, id: ShipId, to: BodyId, depart_t: Option<f64>) -> Result<(), String> {
        let ship = self.ship(id).ok_or("no such ship")?.clone();
        let plan = self.plan_for(&ship, to, depart_t)?;
        self.schedule(player, id, plan)
    }

    fn schedule(&mut self, player: PlayerId, id: ShipId, (p, rest): (TransferPlan, Vec<BodyId>)) -> Result<(), String> {
        let ship = self.ship(id).ok_or("no such ship")?.clone();
        if ship.owner != player {
            return Err("not your ship".into());
        }
        if ship.fuel_for(p.depart_dv) > ship.fuel {
            return Err(format!("not enough fuel: departure needs {:.1} km/s, {} has {:.1} km/s", p.depart_dv, ship.name, ship.dv_left()));
        }
        let s = self.ship_mut(id).unwrap();
        s.route = rest;
        if let ShipState::Parked { departure, .. } = &mut s.state {
            *departure = Some(Departure { to: p.to, state: p.depart, dv: p.depart_dv });
        }
        Ok(())
    }

    // ------------------------------------------------------------------ simulation

    /// Advance the authoritative state to game time `t`.
    pub fn advance_to(&mut self, t: f64) {
        if self.status != Status::Running {
            return;
        }
        while self.t < t {
            let next_tick = self.last_tick + TICK;
            let step_end = next_tick.min(t);
            self.ship_events(step_end);
            self.t = step_end;
            if step_end >= next_tick {
                self.last_tick = next_tick;
                self.economy(TICK);
                self.jobs();
                self.research_tick();
                self.stations_tick(TICK);
                self.combat(TICK);
                self.ownership();
                self.update_intel();
                crate::bot::think_all(self);
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
        let sys = &self.system;
        let ship = &mut self.ships[i];
        let (id, owner, name) = (ship.id, ship.owner, ship.name.clone());
        let cs = ship.stats();
        let rocket = |fuel: f64, dv: f64| (cs.dry_mass + fuel) * (1.0 - (-dv / cs.ve).exp());
        match &mut ship.state {
            ShipState::Parked { body, departure } => {
                let d = departure.take().unwrap();
                let need = rocket(ship.fuel, d.dv);
                if need > ship.fuel {
                    let body = *body;
                    ship.route.clear();
                    self.event(Some(owner), EventKind::Bad, "Departure aborted", &format!("{name} lacks fuel for the burn"), Some(body), Some(id));
                    return;
                }
                ship.fuel -= need;
                ship.last_burn_t = d.state.t;
                ship.state = ShipState::Flying(Flight { start: d.state, burns: vec![], dest: Some(d.to), cache: None });
                let to = sys.body(d.to).name.clone();
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
            match self.plan_for(&ship, next, None) {
                Ok((p, _)) if ship.fuel_for(p.depart_dv) <= ship.fuel => {
                    let s = &mut self.ships[i];
                    s.route = rest;
                    if let ShipState::Parked { departure, .. } = &mut s.state {
                        *departure = Some(Departure { to: p.to, state: p.depart, dv: p.depart_dv });
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

    /// Production per hour for a body (matter, energy, fuel) given its upgrades.
    pub fn production(&self, body: BodyId) -> (f64, f64, f64) {
        let b = &self.bodies[body.0 as usize];
        if b.owner.is_none() {
            return (0.0, 0.0, 0.0);
        }
        let kind = self.system.body(body).kind;
        let base_m = if kind == BodyKind::Star { 0.0 } else { 60.0 };
        let matter = base_m * (1.0 + b.level(Upgrade::Mining) as f64);
        let energy = 20.0 + 60.0 * b.level(Upgrade::Solar) as f64 * self.solar_factor(body) + 60.0 * b.level(Upgrade::Reactor) as f64;
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
            b.def_hp = (b.def_hp + max * 0.1 * h).min(max);
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
        for s in &self.ships {
            if s.parked_at() == Some(body) && s.class.stats().attack > 0.0 {
                add(s.owner, s.class.stats().attack * (s.hp / s.stats().hp).max(0.2));
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

    fn combat(&mut self, dt: f64) {
        let h = dt / 3600.0;
        for bi in 0..self.bodies.len() {
            let body = BodyId(bi as u32);
            let powers = self.power_at(body);
            if powers.iter().filter(|p| p.1 > 0.0).count() < 2 {
                self.bodies[bi].in_combat = false;
                continue;
            }
            if !self.bodies[bi].in_combat {
                self.bodies[bi].in_combat = true;
                let bname = self.system.body(body).name.clone();
                let owner = self.bodies[bi].owner;
                if let Some(o) = owner {
                    self.event(Some(o), EventKind::Battle, "Under attack", &format!("Enemy ships are attacking {bname}"), Some(body), None);
                }
                for (p, _) in powers.iter().filter(|p| p.1 > 0.0 && Some(p.0) != owner) {
                    self.event(Some(*p), EventKind::Battle, "Battle", &format!("Your ships are fighting at {bname}"), Some(body), None);
                }
            }
            let here: Vec<usize> = (0..self.ships.len()).filter(|&i| self.ships[i].parked_at() == Some(body)).collect();
            let owner = self.bodies[bi].owner;
            let mut dmg = vec![0.0; self.ships.len()];
            let mut def_dmg = 0.0;
            // each armed ship picks the enemy class it is best against, focusing the weakest
            let attackers: Vec<(PlayerId, Option<ShipClass>, f64)> = here
                .iter()
                .map(|&i| {
                    let s = &self.ships[i];
                    let st = s.class.stats();
                    let kin = if st.weapon == Weapon::Kinetic { 1.25 } else { 1.0 };
                    (s.owner, Some(s.class), st.attack * kin * h)
                })
                .chain(owner.filter(|_| self.bodies[bi].def_hp > 0.0).map(|o| (o, None, self.def_attack(body) * h)))
                .filter(|a| a.2 > 0.0)
                .collect();
            for (who, class, dmg_amount) in attackers {
                let enemies: Vec<usize> = here.iter().copied().filter(|&j| self.ships[j].owner != who).collect();
                let mult = |c: ShipClass| class.map(|a| a.vs(c)).unwrap_or(1.0);
                let target = enemies.iter().copied().max_by(|&a, &b| {
                    let (ca, cb) = (self.ships[a].class, self.ships[b].class);
                    mult(ca)
                        .total_cmp(&mult(cb))
                        .then((self.ships[b].hp - dmg[b]).total_cmp(&(self.ships[a].hp - dmg[a])))
                        .then(self.ships[b].id.cmp(&self.ships[a].id))
                });
                let def_target = owner.is_some_and(|o| o != who) && self.bodies[bi].def_hp - def_dmg > 0.0;
                match target {
                    Some(j) => dmg[j] += dmg_amount * mult(self.ships[j].class),
                    None if def_target => def_dmg += dmg_amount * class.map(|c| c.vs_defense()).unwrap_or(1.0),
                    None => {}
                }
            }
            for &i in &here {
                self.ships[i].hp -= dmg[i];
            }
            self.bodies[bi].def_hp = (self.bodies[bi].def_hp - def_dmg).max(0.0);
            let bname = self.system.body(body).name.clone();
            let lost: Vec<Ship> = self.ships.iter().filter(|s| s.hp <= 0.0).cloned().collect();
            self.ships.retain(|s| s.hp > 0.0);
            for s in lost {
                self.event(Some(s.owner), EventKind::Battle, "Ship destroyed", &format!("{} was destroyed at {bname}", s.name), Some(body), None);
            }
            self.retreats(body);
        }
    }

    /// Ships with the Retreat order leave a body where their side is clearly outgunned.
    fn retreats(&mut self, body: BodyId) {
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
        for id in candidates {
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
                if self.order_transfer(ship.owner, id, to, Some(self.t + 60.0)).is_ok() {
                    self.event(Some(ship.owner), EventKind::Bad, "Retreating", &format!("{} is pulling back", ship.name), Some(body), Some(id));
                }
            }
        }
    }

    fn ownership(&mut self) {
        for bi in 0..self.bodies.len() {
            let body = BodyId(bi as u32);
            if self.system.body(body).is_star() {
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

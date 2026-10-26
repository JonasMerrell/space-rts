//! Plain view-models the UI draws from. The client crate fills these from `sim`/`proto`;
//! positions are absolute (star-centred) in km, speeds/Δv in km/s, times are seconds since game start.

pub type BodyId = u32;
pub type ShipId = u32;
pub type PlayerId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    Star,
    Planet,
    Moon,
    Asteroid,
}

#[derive(Clone, Debug)]
pub struct Body {
    pub id: BodyId,
    pub name: String,
    pub kind: BodyKind,
    pub parent: Option<BodyId>,
    /// Physical radius in km (drawn with a minimum on-screen size).
    pub radius: f64,
    /// Sphere of influence radius in km (0 for the star).
    pub soi: f64,
    /// Owner = whoever has the most power in orbit; `None` = neutral/unknown.
    pub owner: Option<PlayerId>,
    /// What our telescopes know about the contents (None = never scanned).
    pub intel: Option<Intel>,
    /// Art key from assets/manifest.json `bodies` (e.g. "planet_ocean"); None = pick by id.
    /// "rubble" = shattered by an asteroid impact (drawn as debris). Destroyed bodies aren't sent.
    pub art: Option<String>,
    /// Points of the orbit around the parent, relative to the parent (closed loop), for drawing.
    pub orbit_path: Vec<[f64; 2]>,
}

#[derive(Clone, Debug, Default)]
pub struct Intel {
    /// Seconds since game start when this was observed.
    pub seen_at: f64,
    pub power: f32,
    pub upgrades: Vec<String>,
    pub stations: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShipClass {
    Scout,
    Interceptor,
    Gunship,
    Bomber,
    Dreadnought,
    Tanker,
}

impl ShipClass {
    pub const ALL: [ShipClass; 6] =
        [ShipClass::Scout, ShipClass::Interceptor, ShipClass::Gunship, ShipClass::Bomber, ShipClass::Dreadnought, ShipClass::Tanker];
    pub fn label(self) -> &'static str {
        match self {
            ShipClass::Scout => "Scout",
            ShipClass::Interceptor => "Interceptor",
            ShipClass::Gunship => "Gunship",
            ShipClass::Bomber => "Bomber",
            ShipClass::Dreadnought => "Dreadnought",
            ShipClass::Tanker => "Tanker",
        }
    }
}

/// Planet upgrade names, in the order the body panel lists them (= sim `Upgrade::ALL`).
pub const UPGRADES: [&str; 8] = ["Mining", "Solar", "Reactor", "Refinery", "Shipyard", "Defense", "Telescope", "Research"];

#[derive(Clone, Debug, Default)]
pub struct Cost {
    pub matter: f64,
    pub energy: f64,
    /// Build time in seconds.
    pub seconds: f64,
}

#[derive(Clone, Debug)]
pub struct UpgradeInfo {
    /// One of `UPGRADES`.
    pub name: String,
    pub level: u8,
    pub max: u8,
    /// Cost of the next level, None when maxed.
    pub next: Option<Cost>,
}

/// Owner-side detail for the body panel (`Backend::body_info`).
#[derive(Clone, Debug, Default)]
pub struct BodyInfo {
    pub upgrades: Vec<UpgradeInfo>,
    /// Ship classes this body's shipyard can build now, with costs.
    pub buildable: Vec<(ShipClass, Cost)>,
    /// (label, finishes at t).
    pub queue: Vec<(String, f64)>,
    pub fuel: f64,
    /// Military power in orbit per player.
    pub power: Vec<(PlayerId, f64)>,
    /// Per hour.
    pub matter_rate: f64,
    pub energy_rate: f64,
    pub fuel_rate: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ManeuverNode {
    pub t: f64,
    /// km/s along velocity (negative = retrograde).
    pub prograde: f64,
    /// km/s outward from the parent body (negative = radial in).
    pub radial: f64,
}

impl ManeuverNode {
    pub fn dv(&self) -> f64 {
        (self.prograde * self.prograde + self.radial * self.radial).sqrt()
    }
}

/// A timed polyline in absolute coordinates.
#[derive(Clone, Debug, Default)]
pub struct Trajectory {
    pub points: Vec<(f64, [f64; 2])>,
}

impl Trajectory {
    pub fn sample(&self, t: f64) -> Option<[f64; 2]> {
        let p = &self.points;
        if p.is_empty() || t < p[0].0 || t > p[p.len() - 1].0 {
            return None;
        }
        let i = p.partition_point(|q| q.0 <= t).clamp(1, p.len() - 1);
        let (t0, a) = p[i - 1];
        let (t1, b) = p[i];
        let k = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
        Some([a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k])
    }
    pub fn start(&self) -> Option<f64> {
        self.points.first().map(|p| p.0)
    }
    pub fn end(&self) -> Option<f64> {
        self.points.last().map(|p| p.0)
    }
}

#[derive(Clone, Debug)]
pub struct Ship {
    pub id: ShipId,
    pub name: String,
    pub class: ShipClass,
    pub owner: PlayerId,
    /// Ships in this group (a fleet token).
    pub count: u32,
    pub fuel: f32,
    pub fuel_max: f32,
    /// Parked in orbit of a body until `trajectory` (if any) departs.
    pub parked_at: Option<BodyId>,
    pub trajectory: Option<Trajectory>,
    pub nodes: Vec<ManeuverNode>,
    /// Where the scheduled flight ends up.
    pub destination: Option<BodyId>,
    /// Standing order while the player is away: retreat when outgunned (else hold and fight).
    pub retreat: bool,
    /// Fog of war: `Some(t)` = an enemy ship last seen at game time t, drawn as a fading ghost on
    /// its last known course; `None` = live.
    pub seen_at: Option<f64>,
    /// Only on the time scrubber: a ship that is still in a build queue now, ready at this time.
    pub planned: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct Player {
    pub id: PlayerId,
    pub name: String,
    /// Index into the team palette (0 blue, 1 red, 2 green, 3 yellow).
    pub color: usize,
    pub faction: String,
    pub online: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Resources {
    pub matter: f64,
    pub energy: f64,
    pub fuel: f64,
    /// Per hour.
    pub matter_rate: f64,
    pub energy_rate: f64,
    pub fuel_rate: f64,
}

#[derive(Clone, Debug)]
pub struct ChatMsg {
    pub from: PlayerId,
    pub t: f64,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Good,
    Bad,
    Battle,
}

#[derive(Clone, Debug)]
pub struct Notice {
    pub kind: NoticeKind,
    pub title: String,
    pub body: String,
    /// Optional body or ship to jump to when tapped.
    pub focus: Option<Focus>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Focus {
    Body(BodyId),
    Ship(ShipId),
}

/// Preview of pushing an owned asteroid into a planet (`Backend::plan_impact_async`).
#[derive(Clone, Debug)]
pub struct ImpactPreview {
    pub asteroid: BodyId,
    pub target: BodyId,
    /// Drive burn: start time and length (s).
    pub burn_t: f64,
    pub burn_s: f64,
    pub dv: f64,
    /// Fuel the burn needs at the asteroid, and what's available there.
    pub fuel: f64,
    pub fuel_available: f64,
    /// One-off cost of installing a drive (0 if one is already fitted).
    pub drive_cost: Cost,
    /// None = the push misses (no impact within the prediction horizon).
    pub impact_t: Option<f64>,
    /// km/s at impact.
    pub impact_speed: f64,
    /// Joules.
    pub impact_energy: f64,
    /// <1 damages (≈ fraction of upgrades lost), ≥1 wipes upgrades + everything in orbit, ≥4 shatters.
    pub severity: f64,
    pub path: Trajectory,
}

/// Result of the auto-planner for "ship → target".
#[derive(Clone, Debug)]
pub struct TransferPlan {
    pub ship: ShipId,
    pub target: BodyId,
    pub depart_t: f64,
    pub arrive_t: f64,
    pub dv_depart: f64,
    pub dv_capture: f64,
    pub fuel_needed: f32,
    pub path: Trajectory,
    /// Upcoming good departure times (seconds), best first is not required.
    pub windows: Vec<f64>,
}

#[derive(Clone, Debug)]
pub struct LobbyInfo {
    pub code: String,
    pub name: String,
    pub players: Vec<Player>,
    pub max_players: usize,
    pub host: PlayerId,
    pub length_days: u32,
    /// Game seconds per real second.
    pub speed: f64,
    pub bots: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct GameSummary {
    pub code: String,
    pub name: String,
    pub day: u32,
    pub length_days: u32,
    pub players: usize,
    pub unread: usize,
}

/// Everything the in-game screens draw.
#[derive(Clone, Debug, Default)]
pub struct GameView {
    pub me: PlayerId,
    pub host: PlayerId,
    pub now: f64,
    pub ends_at: f64,
    /// Game seconds per real second (0 = unknown, treat as 1).
    pub speed: f64,
    pub bodies: Vec<Body>,
    pub ships: Vec<Ship>,
    pub players: Vec<Player>,
    pub resources: Resources,
    pub chat: Vec<ChatMsg>,
}

impl GameView {
    pub fn body(&self, id: BodyId) -> Option<&Body> {
        self.bodies.iter().find(|b| b.id == id)
    }
    pub fn ship(&self, id: ShipId) -> Option<&Ship> {
        self.ships.iter().find(|s| s.id == id)
    }
    pub fn player(&self, id: PlayerId) -> Option<&Player> {
        self.players.iter().find(|p| p.id == id)
    }
    pub fn color_of(&self, owner: Option<PlayerId>) -> Option<usize> {
        owner.and_then(|o| self.player(o)).map(|p| p.color)
    }
}

// ---------------------------------------------------------------- research + stations

#[derive(Clone, Debug, PartialEq)]
pub enum TechState {
    Done,
    /// Being researched at a lab, finishes at t.
    Running { lab: BodyId, done_t: f64 },
    /// Can start now at this lab (the best idle one that qualifies).
    Ready { lab: BodyId },
    /// Can't start yet; human-readable reason ("needs Hardened slugs", "needs lab level 2", ...).
    Locked(String),
}

#[derive(Clone, Debug)]
pub struct TechInfo {
    /// Stable id (sim enum name), sent back in `UiAction::Research`.
    pub id: String,
    pub name: String,
    /// One of `TECH_BRANCHES`.
    pub branch: String,
    /// 1..=4
    pub tier: u8,
    pub desc: String,
    pub cost: Cost,
    pub min_lab: u8,
    pub state: TechState,
}

#[derive(Clone, Debug)]
pub struct LabInfo {
    pub body: BodyId,
    pub name: String,
    pub level: u8,
    /// (tech name, finishes at t)
    pub busy: Option<(String, f64)>,
}

/// Everything the research screen shows (`Backend::research`).
#[derive(Clone, Debug, Default)]
pub struct ResearchView {
    /// All techs, branch-major, tier order.
    pub techs: Vec<TechInfo>,
    pub labs: Vec<LabInfo>,
}

pub const TECH_BRANCHES: [&str; 5] = ["Weapons", "Engines", "Industry", "Defense", "Scanning"];

#[derive(Clone, Debug)]
pub struct StationInfo {
    /// One of `STATION_KINDS`.
    pub kind: String,
    pub owner: PlayerId,
    /// Under construction until then.
    pub building_until: Option<f64>,
    /// 0..1
    pub hp: f32,
}

#[derive(Clone, Debug)]
pub struct StationOption {
    /// One of `STATION_KINDS`, sent back in `UiAction::BuildStation`.
    pub kind: String,
    pub name: String,
    pub desc: String,
    pub cost: Cost,
    /// Why it can't be built here now; None = can build.
    pub blocker: Option<String>,
}

/// Stations at a body and what we could build there (`Backend::stations`).
#[derive(Clone, Debug, Default)]
pub struct StationsView {
    /// Stations we know of in orbit here (ours always, others when sensors see them).
    pub here: Vec<StationInfo>,
    pub options: Vec<StationOption>,
    /// We own the body or have a ship parked there, so building is possible at all.
    pub present: bool,
}

/// Station kinds (= sim `StationKind`), also the icon names `station_<lowercase>`.
pub const STATION_KINDS: [&str; 6] = ["Shipyard", "FuelDepot", "Telescope", "Scanner", "Defense", "Relay"];

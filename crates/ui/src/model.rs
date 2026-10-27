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
    /// Builds space stations where it's parked; more of them build faster.
    Constructor,
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
    pub fn label(self) -> &'static str {
        match self {
            ShipClass::Scout => "Scout",
            ShipClass::Interceptor => "Interceptor",
            ShipClass::Gunship => "Gunship",
            ShipClass::Bomber => "Bomber",
            ShipClass::Dreadnought => "Dreadnought",
            ShipClass::Tanker => "Tanker",
            ShipClass::Constructor => "Constructor",
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
    /// Ours waiting for funds at this body (they start by themselves once income covers them).
    pub waiting: Vec<WaitingItem>,
    pub fuel: f64,
    /// Military power in orbit per player.
    pub power: Vec<(PlayerId, f64)>,
    /// Per hour.
    pub matter_rate: f64,
    pub energy_rate: f64,
    pub fuel_rate: f64,
}

/// A build/upgrade/station/research queued before we could pay for it.
#[derive(Clone, Debug)]
pub struct WaitingItem {
    /// Sent back in `UiAction::Unwait` / `MoveWait`.
    pub id: u32,
    pub label: String,
    pub cost: Cost,
    /// Projected start from the current income; None = blocked or no income.
    pub start_t: Option<f64>,
    /// Why it can't start whatever the money ("needs a construction ship..."); it's skipped meanwhile.
    pub blocked: Option<String>,
    /// Position in our whole waiting list (all bodies; earlier = funded first) and its length.
    pub index: usize,
    pub of: usize,
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
    /// Gravity assists along the path.
    pub flybys: Vec<FlybyMark>,
}

/// A pass close by a body that bends the path (slingshot).
#[derive(Clone, Debug, Default)]
pub struct FlybyMark {
    pub body: BodyId,
    /// time and absolute position of closest approach
    pub t: f64,
    pub pos: [f64; 2],
    /// km above the body's centre at closest approach
    pub rp: f64,
    /// change of speed relative to the body's parent, km/s (+ = boost)
    pub gain: f64,
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
    /// A path `k` (0..1) of the way from `self` to `b`, matched by fraction of flight time (for the
    /// slider preview while the exact course is being planned).
    pub fn blend(&self, b: &Trajectory, k: f64) -> Trajectory {
        let (Some(a0), Some(a1), Some(b0), Some(b1)) = (self.start(), self.end(), b.start(), b.end()) else {
            return if k < 0.5 { self.clone() } else { b.clone() };
        };
        let n = self.points.len().max(b.points.len()).clamp(2, 240);
        let points = (0..n)
            .filter_map(|i| {
                let u = i as f64 / (n - 1) as f64;
                let (ta, tb) = (a0 + (a1 - a0) * u, b0 + (b1 - b0) * u);
                let (pa, pb) = (self.sample(ta)?, b.sample(tb)?);
                Some((ta + (tb - ta) * k, [pa[0] + (pb[0] - pa[0]) * k, pa[1] + (pb[1] - pa[1]) * k]))
            })
            .collect();
        Trajectory { points, flybys: if k < 0.5 { self.flybys.clone() } else { b.flybys.clone() } }
    }
}

impl TransferPlan {
    pub fn total_dv(&self) -> f64 {
        self.dv_depart + self.dv_mid + self.dv_capture
    }
    /// Preview `k` of the way to `b` (numbers and path interpolated). Not schedulable: its token is
    /// empty until the backend plans the exact course.
    pub fn blend(&self, b: &TransferPlan, k: f64) -> TransferPlan {
        let l = |x: f64, y: f64| x + (y - x) * k;
        let near = if k < 0.5 { self } else { b };
        let fuel_needed = self.fuel_needed + (b.fuel_needed - self.fuel_needed) * k as f32;
        TransferPlan {
            depart_t: l(self.depart_t, b.depart_t),
            arrive_t: l(self.arrive_t, b.arrive_t),
            dv_depart: l(self.dv_depart, b.dv_depart),
            dv_mid: l(self.dv_mid, b.dv_mid),
            dv_capture: l(self.dv_capture, b.dv_capture),
            fuel_needed,
            fuel_problem: if fuel_needed > near.fuel_have {
                Some(format!("not enough fuel: needs {fuel_needed:.0} t, has {:.0} t", near.fuel_have))
            } else if self.fuel_problem.is_some() && b.fuel_problem.is_some() {
                near.fuel_problem.clone()
            } else {
                None
            },
            path: self.path.blend(&b.path, k),
            token: String::new(),
            label: String::new(),
            ..near.clone()
        }
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
    /// Non-empty when this token is one of our fleets: its ships (real ship ids, not the token's).
    pub members: Vec<FleetMember>,
}

impl Ship {
    pub fn is_fleet(&self) -> bool {
        !self.members.is_empty()
    }
}

/// One ship inside a fleet token.
#[derive(Clone, Debug)]
pub struct FleetMember {
    pub id: ShipId,
    pub name: String,
    pub class: ShipClass,
    pub fuel: f32,
    pub fuel_max: f32,
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

/// Where a ship ends up with a set of manual nodes (node editor projection).
#[derive(Clone, Debug)]
pub struct NodeOutcome {
    /// End of the predicted path (horizon, capture or impact).
    pub end_t: f64,
    pub end_pos: [f64; 2],
    /// Captured into orbit of this body at the end, if the prediction says so.
    pub captured: Option<BodyId>,
    /// Closest approach to the most relevant body along the path.
    pub encounter: Option<Encounter>,
    /// Fuel left after all burns (None = backend can't tell).
    pub fuel_left: Option<f64>,
}

#[derive(Clone, Copy, Debug)]
pub struct Encounter {
    pub body: BodyId,
    pub t: f64,
    /// km
    pub distance: f64,
    /// Passes inside the body's sphere of influence.
    pub inside_soi: bool,
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
    /// Mid-course correction burns (after a gravity assist).
    pub dv_mid: f64,
    pub dv_capture: f64,
    /// Fuel every ship that flies needs (the whole fleet for a fleet token).
    pub fuel_needed: f32,
    /// Fuel those ships carry between them.
    pub fuel_have: f32,
    /// Why the fuel doesn't work out (None = it does). For fleets this is the sim's own rule: every
    /// ship pays its departure, the weakest decides, fleet tankers can cover capture burns.
    pub fuel_problem: Option<String>,
    pub path: Trajectory,
    /// Bodies the course swings past (gravity assists), in order.
    pub via: Vec<BodyId>,
    /// Short name of this option ("CHEAPEST", "BALANCED", "FASTEST"); empty for a lone plan.
    pub label: String,
    /// Course options for this ship and target, cheapest first, each arriving sooner for more Δv
    /// (the fuel ↔ time slider's stops; this plan is one of them or lies between two).
    pub options: Vec<TransferPlan>,
    /// Backend-defined handle that re-creates exactly this course when scheduled.
    pub token: String,
    /// Upcoming good departure times (seconds), best first is not required.
    pub windows: Vec<f64>,
    /// Set when this course intercepts an enemy ship instead of going to `target`.
    pub intercept: Option<Intercept>,
}

/// What an intercept course does at the meeting point (`TransferPlan::arrive_t`).
#[derive(Clone, Debug, PartialEq)]
pub struct Intercept {
    /// the enemy ship
    pub target: ShipId,
    /// relative speed at the meeting point before any match burn, km/s
    pub closing: f64,
    /// a match burn (in `dv_capture`) cancels that speed so we stay with it; else we fly through
    pub matched: bool,
    /// where the target will be then (km)
    pub meet: [f64; 2],
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

/// Final standings of an ended war (`Backend::results`).
#[derive(Clone, Debug, Default)]
pub struct GameResults {
    pub winner: Option<PlayerId>,
    /// (player, territory score), best first
    pub scores: Vec<(PlayerId, u32)>,
    pub ended_at: f64,
}

/// Everything the in-game screens draw.
#[derive(Clone, Debug, Default)]
pub struct GameView {
    /// Join code of this war (for war-level actions like DeleteGame).
    pub code: String,
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
    /// Some while the game waits for the host to start it (clock stopped at 0).
    pub lobby: Option<LobbyInfo>,
    /// Our home planet (during the lobby: where we'll start, may change as pilots join).
    pub home: Option<BodyId>,
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
    /// Under construction: estimated finish (INFINITY = paused, no workers there).
    pub building_until: Option<f64>,
    /// Construction progress 0..1 (1 when finished).
    pub progress: f32,
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
    /// Can be queued anyway: it starts by itself once the blocker clears and it's affordable.
    pub queueable: bool,
}

/// Stations at a body and what we could build there (`Backend::stations`).
#[derive(Clone, Debug, Default)]
pub struct StationsView {
    /// Stations we know of in orbit here (ours always, others when sensors see them).
    pub here: Vec<StationInfo>,
    pub options: Vec<StationOption>,
    /// We have workers here (constructors, or our own shipyard), so building is possible at all.
    pub present: bool,
    /// Our construction ships parked here.
    pub constructors: usize,
    /// Workers on our sites here (constructors + our shipyard, capped); build speed multiplier.
    pub workers: usize,
}

/// Station kinds (= sim `StationKind`), also the icon names `station_<lowercase>`.
pub const STATION_KINDS: [&str; 6] = ["Shipyard", "FuelDepot", "Telescope", "Scanner", "Defense", "Relay"];

/// Tab of the sensors report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportTab {
    /// what is happening right now
    Now,
    /// what our telescopes and scanners see (or last saw)
    Contacts,
    /// what has happened
    Log,
}

/// Group of rows inside a sensors report tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportSection {
    /// enemy ships or asteroids on their way to our bodies, enemy ships in orbit of ours
    Threats,
    /// fighting we can see
    Battles,
    /// our ships in flight and scheduled departures
    Transfers,
    /// enemy ships in sensor range now
    Ships,
    /// enemy ships out of sensor range: where they were last seen
    Ghosts,
    /// planets and moons and what we know about them
    Bodies,
    /// events and sensor sightings, oldest first
    Log,
}

impl ReportSection {
    pub fn tab(self) -> ReportTab {
        match self {
            ReportSection::Threats | ReportSection::Battles | ReportSection::Transfers => ReportTab::Now,
            ReportSection::Ships | ReportSection::Ghosts | ReportSection::Bodies => ReportTab::Contacts,
            ReportSection::Log => ReportTab::Log,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ReportSection::Threats => "THREATS",
            ReportSection::Battles => "BATTLES IN SIGHT",
            ReportSection::Transfers => "YOUR SHIPS UNDER WAY",
            ReportSection::Ships => "CONTACTS IN RANGE",
            ReportSection::Ghosts => "LAST KNOWN POSITIONS",
            ReportSection::Bodies => "PLANETS & MOONS",
            ReportSection::Log => "",
        }
    }
}

/// How a report row's time reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    /// it happened at `t` (clock time)
    At,
    /// it happens at `t` (countdown)
    Eta,
    /// last seen at `t` ("3H AGO")
    Seen,
    /// seen right now
    Live,
    /// no time to show
    Never,
}

#[derive(Clone, Debug)]
pub struct ReportItem {
    /// game time (see `when`)
    pub t: f64,
    pub when: When,
    pub section: ReportSection,
    pub title: String,
    pub text: String,
    pub focus: Option<Focus>,
    /// Used when `focus` is a ship that's gone or out of sight now (its body).
    pub fallback: Option<Focus>,
    /// -1 hostile, 0 neutral, 1 ours/good
    pub tone: i8,
}

/// The sensors report: what is happening now, what our sensors see, and what has happened. Built
/// from the fogged view only, so it never knows more than the map.
#[derive(Clone, Debug, Default)]
pub struct SensorsReport {
    /// Log items after this game time are new (the player hasn't opened the report since).
    pub seen_until: f64,
    pub items: Vec<ReportItem>,
}

impl SensorsReport {
    pub fn unread(&self) -> usize {
        self.items.iter().filter(|i| i.section == ReportSection::Log && i.t > self.seen_until).count()
    }
    pub fn threats(&self) -> usize {
        self.items.iter().filter(|i| i.section == ReportSection::Threats).count()
    }
}

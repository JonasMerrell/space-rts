//! Plain view-models the UI draws from. The client crate fills these from `sim`/`proto`;
//! positions are absolute (star-centred) in Gm, times are seconds since game start.

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
    /// Physical radius in Gm (drawn with a minimum on-screen size).
    pub radius: f64,
    /// Sphere of influence radius in Gm (0 for the star).
    pub soi: f64,
    /// Owner = whoever has the most power in orbit; `None` = neutral/unknown.
    pub owner: Option<PlayerId>,
    /// What our telescopes know about the contents (None = never scanned).
    pub intel: Option<Intel>,
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
    Interceptor,
    Gunship,
    Bomber,
    Tanker,
}

impl ShipClass {
    pub fn label(self) -> &'static str {
        match self {
            ShipClass::Interceptor => "Interceptor",
            ShipClass::Gunship => "Gunship",
            ShipClass::Bomber => "Bomber",
            ShipClass::Tanker => "Tanker",
        }
    }
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
    pub now: f64,
    pub ends_at: f64,
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

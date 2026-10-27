//! The seam between the UI and the rest of the client. `crates/client` implements this over
//! `sim` + the network; `demo::DemoBackend` fakes it for screenshots and offline play.

use crate::model::*;

/// Async results the backend reports back (login done, lobby changed, ...).
#[derive(Clone, Debug)]
pub enum BackendEvent {
    LoggedIn { user: String },
    LoginFailed(String),
    LoggedOut,
    Lobby(LobbyInfo),
    LobbyFailed(String),
    GameStarted,
    Notice(Notice),
}

/// Commands from the UI. Fire-and-forget; results arrive as `BackendEvent`s or a changed view.
#[derive(Clone, Debug)]
pub enum UiAction {
    /// Close the "while you were away" report.
    DismissReport,
    /// `invite`: code for invite-only servers when registering ("" = none)
    Login { server: String, user: String, pass: String, register: bool, invite: String },
    Logout,
    /// `speed`: game seconds per real second (1 = real time; 4 = a 7-day game takes 42 real hours).
    CreateGame { name: String, length_days: u32, speed: f64, bots: Vec<String> },
    JoinGame { code: String },
    OpenGame { code: String },
    AddBot { difficulty: String },
    /// Host only, before the start.
    RemoveBot { player: PlayerId },
    SetFaction(String),
    StartGame,
    LeaveLobby,
    ScheduleTransfer(TransferPlan),
    SetNodes { ship: ShipId, nodes: Vec<ManeuverNode> },
    CancelFlight { ship: ShipId },
    Chat(String),
    BackToMenu,
    Build { body: BodyId, class: ShipClass },
    /// `upgrade` is one of `model::UPGRADES`.
    Upgrade { body: BodyId, upgrade: String },
    SetOrder { ship: ShipId, retreat: bool },
    /// Host only.
    ExtendGame { days: u32 },
    /// Host only: delete the war for every player (ask for confirmation first).
    DeleteGame { code: String },
    /// Host only: game seconds per real second from now on.
    SetSpeed { speed: f64 },
    /// Retry the server connection now instead of waiting for the backoff.
    Reconnect,
    /// Remove item `index` (0 = building now) from a body's build queue, refunding its cost.
    CancelJob { body: BodyId, index: usize },
    /// Move queue item `from` to position `to` (indices into `BodyInfo.queue`).
    MoveJob { body: BodyId, from: usize, to: usize },
    /// Start `tech` (a `TechInfo::id`) at a lab body.
    Research { body: BodyId, tech: String },
    /// `kind` is one of `model::STATION_KINDS`.
    BuildStation { body: BodyId, kind: String },
    /// Fit a drive (if needed) to our asteroid and push it onto a collision course with `target`.
    AimAsteroid { body: BodyId, target: BodyId },
    /// Group ships/fleet tokens parked at one body into a new fleet (tokens bring all their ships).
    FormFleet { ships: Vec<ShipId>, name: String },
    /// Add ships/fleet tokens parked with `fleet` (a fleet token) to it.
    JoinFleet { fleet: ShipId, ships: Vec<ShipId> },
    /// Take member ships out of their fleet; `split` = two or more leavers form a new fleet with that name.
    LeaveFleet { ships: Vec<ShipId>, split: Option<String> },
    MergeFleets { into: ShipId, from: ShipId },
    RenameFleet { fleet: ShipId, name: String },
    DisbandFleet { fleet: ShipId },
}

/// Result of polling `Backend::plan_transfer_async`.
#[derive(Clone, Debug)]
pub enum PlanPoll {
    Pending,
    Done(Option<TransferPlan>),
}

/// Result of polling `Backend::plan_impact_async`.
pub enum ImpactPoll {
    Pending,
    Done(Option<ImpactPreview>),
}

pub trait Backend {
    /// Drain events since the last frame.
    fn poll(&mut self) -> Vec<BackendEvent>;
    fn act(&mut self, action: UiAction);

    /// Games this account is in (main menu list).
    fn my_games(&self) -> Vec<GameSummary>;
    /// The current game, if one is open.
    fn view(&self) -> Option<&GameView>;

    /// Absolute position of a body at time `t` (predicted from what the player knows).
    fn body_pos(&self, id: BodyId, t: f64) -> [f64; 2];
    /// Absolute position of a ship at `t`, `None` if we don't know/it's gone.
    fn ship_pos(&self, id: ShipId, t: f64) -> Option<[f64; 2]>;

    /// Auto-planner: best transfer for `ship` to `target` departing at or after `after`.
    fn plan_transfer(&self, ship: ShipId, target: BodyId, after: f64) -> Option<TransferPlan>;
    /// Non-blocking planner: call every frame with the same arguments until it stops returning
    /// `Pending`. The default just runs `plan_transfer` synchronously.
    fn plan_transfer_async(&self, ship: ShipId, target: BodyId, after: f64) -> PlanPoll {
        PlanPoll::Done(self.plan_transfer(ship, target, after))
    }
    /// Asteroid impact planner, polled every frame like `plan_transfer_async`. None = can't aim it
    /// (not ours, no tech, no route). Default: unsupported.
    fn plan_impact_async(&self, _asteroid: BodyId, _target: BodyId) -> ImpactPoll {
        ImpactPoll::Done(None)
    }
    /// Summary of where the ship ends up with these nodes (capture, closest approach, fuel left).
    /// Default None: the UI derives end point + closest approach from `preview_nodes` itself.
    fn node_outcome(&self, _ship: ShipId, _nodes: &[ManeuverNode]) -> Option<NodeOutcome> {
        None
    }
    /// Trajectory the ship would fly with these manual nodes (for the node editor preview).
    fn preview_nodes(&self, ship: ShipId, nodes: &[ManeuverNode]) -> Option<Trajectory>;

    /// Owner-side detail (upgrades, shipyard, queue) for a body; None if not ours / unknown.
    /// Rules text from the sim for a ship class ("ship:Gunship") or upgrade level ("upgrade:Mining:2"),
    /// so detail sheets stay in sync with balance changes. None = use the UI's own text.
    fn describe(&self, _key: &str) -> Option<String> {
        None
    }
    /// Final standings once the war has ended (time limit reached); None while it runs.
    fn results(&self) -> Option<GameResults> {
        None
    }
    /// When your stations under construction at `body` should be finished (latest of them), if any.
    /// Infinity = paused (no constructor there). Use it to schedule a constructor to leave when done.
    fn construction_done(&self, _body: BodyId) -> Option<f64> {
        None
    }
    fn body_info(&self, _id: BodyId) -> Option<BodyInfo> {
        None
    }
    /// Tech tree state for the research screen.
    fn research(&self) -> Option<ResearchView> {
        None
    }
    /// Stations at a body and what we could build there.
    fn stations(&self, _id: BodyId) -> Option<StationsView> {
        None
    }
    /// "While you were away" report to show on entering a game (until `UiAction::DismissReport`).
    fn away_report(&self) -> Option<&AwayReport> {
        None
    }
    /// Signing back in from a saved session as this user (login screen shows a splash, not the form).
    fn auto_login(&self) -> Option<String> {
        None
    }
    /// Some(text) while logged in but not connected ("Reconnecting..."); None when online.
    fn connection_status(&self) -> Option<String> {
        None
    }
    /// Time scrubber: the game as predicted at a future time `t`, simulated forward from what the
    /// player knows (builds finish, scheduled burns fire, battles play out). `view().now` stays the
    /// real now. `None` = no forecast for `t`: draw `view()` and move things along their paths.
    fn predicted(&self, _t: f64) -> Option<std::rc::Rc<GameView>> {
        None
    }
    /// Game time up to which `predicted` is available (the forecast fills in in the background).
    fn forecast_until(&self) -> f64 {
        f64::INFINITY
    }
    /// Server address shown on the login screen.
    fn default_server(&self) -> String {
        String::new()
    }
}

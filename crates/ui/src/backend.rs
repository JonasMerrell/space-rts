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
    /// The sensors report was looked at: its log items so far are no longer new.
    SensorsSeen,
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
    /// Build/Upgrade/Research/BuildStation start now when affordable, otherwise they wait for funds
    /// and start by themselves once income covers them.
    Build { body: BodyId, class: ShipClass },
    /// `upgrade` is one of `model::UPGRADES`.
    Upgrade { body: BodyId, upgrade: String },
    SetOrder { ship: ShipId, retreat: bool },
    /// A ship's special action (`Ability::key` from `Backend::ship_abilities`).
    ShipAbility { ship: ShipId, key: String },
    /// A station action at a body (`Ability::key` from `StationsView::actions`).
    StationAbility { body: BodyId, key: String },
    /// Host only.
    ExtendGame { days: u32 },
    /// Host only: delete the war for every player (ask for confirmation first).
    DeleteGame { code: String },
    /// Bug report (`bug`) or feature request; the server logs it with the current war and client version.
    Feedback { bug: bool, text: String },
    /// Host only: game seconds per real second from now on.
    SetSpeed { speed: f64 },
    /// Retry the server connection now instead of waiting for the backoff.
    Reconnect,
    /// The "update available" card's button: open the app download page (phone/desktop) or
    /// reload the page (browser).
    GetUpdate,
    /// Remove item `index` (0 = building now) from a body's build queue, refunding its cost.
    CancelJob { body: BodyId, index: usize },
    /// Move queue item `from` to position `to` (indices into `BodyInfo.queue`).
    MoveJob { body: BodyId, from: usize, to: usize },
    /// Drop an item from the waiting-for-funds list (`WaitingItem::id`).
    Unwait { id: u32 },
    /// Move a waiting item to position `to` of the whole waiting list (`WaitingItem::index`).
    MoveWait { id: u32, to: usize },
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

/// Result of polling `Backend::windows_async`.
#[derive(Clone, Debug)]
pub enum WindowsPoll {
    Pending,
    Done(Option<WindowsView>),
}

impl WindowsPoll {
    pub fn is_pending(&self) -> bool {
        matches!(self, WindowsPoll::Pending)
    }
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
    /// Fuel ↔ time slider: the exact course `f` (0..1) of the way from course option `a` to its
    /// neighbour `b` (both from `TransferPlan::options`), polled every frame like
    /// `plan_transfer_async`. Done(None) = no course there (the UI snaps to the nearer option).
    /// Default: unsupported.
    fn plan_between_async(&self, _a: &TransferPlan, _b: &TransferPlan, _f: f32) -> PlanPoll {
        PlanPoll::Done(None)
    }
    /// Intercept courses for `ship` (or fleet token) on the enemy ship `target`, polled every frame
    /// like `plan_transfer_async`: a `TransferPlan` with `intercept` set and the fuel ↔ time slider's
    /// stops in `options`. `matched` = match the target's velocity on arrival. Default: unsupported.
    fn plan_intercept_async(&self, _ship: ShipId, _target: ShipId, _matched: bool) -> PlanPoll {
        PlanPoll::Done(None)
    }
    /// Transfer-window tool for a parked ship, polled every frame like `plan_transfer_async`.
    /// None = not parked / not ours. Default: unsupported.
    fn windows_async(&self, _ship: ShipId) -> WindowsPoll {
        WindowsPoll::Done(None)
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
    /// One line about who holds the orbit of `body` right now, when it isn't simply yours or empty:
    /// "Battle: …", or "Standoff: no armed ships, nobody can take it" when only unarmed ships of
    /// different players are parked there. None = nothing to say.
    fn orbit_status(&self, _body: BodyId) -> Option<String> {
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
    /// Special actions one of our ships (or fleet tokens) has, e.g. going dark.
    fn ship_abilities(&self, _ship: ShipId) -> Vec<Ability> {
        vec![]
    }
    /// Stations at a body and what we could build there.
    fn stations(&self, _id: BodyId) -> Option<StationsView> {
        None
    }
    /// Sensors report for the open game (what is happening, what sensors see, what happened).
    fn sensors(&self) -> Option<&SensorsReport> {
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
    /// A newer app / browser build than this one has been released (the server says so at login).
    fn update_available(&self) -> Option<AppUpdate> {
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
    /// Our own ship (or fleet token) is cloaked ("dark") at `t`: until when. Drawn faded on the map.
    fn dark_until(&self, _ship: ShipId, _t: f64) -> Option<f64> {
        None
    }
    /// Our orbit load at `body`: (ships counted against the cap, cap). Parked + inbound + queued.
    fn orbit_load(&self, _body: BodyId) -> Option<(usize, usize)> {
        None
    }
    /// Server address shown on the login screen.
    fn default_server(&self) -> String {
        String::new()
    }
}

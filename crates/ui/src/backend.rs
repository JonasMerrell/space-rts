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
    Login { user: String, pass: String, register: bool },
    Logout,
    CreateGame { name: String, length_days: u32, bots: Vec<String> },
    JoinGame { code: String },
    OpenGame { code: String },
    AddBot { difficulty: String },
    SetFaction(String),
    StartGame,
    LeaveLobby,
    ScheduleTransfer(TransferPlan),
    SetNodes { ship: ShipId, nodes: Vec<ManeuverNode> },
    CancelFlight { ship: ShipId },
    Chat(String),
    BackToMenu,
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
    /// Trajectory the ship would fly with these manual nodes (for the node editor preview).
    fn preview_nodes(&self, ship: ShipId, nodes: &[ManeuverNode]) -> Option<Trajectory>;
}

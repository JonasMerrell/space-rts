//! Wire protocol between the game client and server: one JSON object per line over TCP.
use serde::{Deserialize, Serialize};
use sim::game::{Command, Event, Game, PlayerId};
use sim::system::SystemSize;

pub const DEFAULT_PORT: u16 = 7777;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Log in (or create the account when `register`).
    Hello { user: String, pass: String, register: bool },
    ListGames,
    CreateGame { name: String, size: SystemSize, length_days: f64, speed: f64, max_players: usize, bots: Vec<String> },
    JoinGame { code: String },
    AddBot { code: String, difficulty: String },
    SetFaction { code: String, faction: String },
    StartGame { code: String },
    /// Subscribe to a game's state (unsubscribes from the previous one).
    Open { code: String },
    Cmd { code: String, cmd: Command },
    /// Notification poll: events for me that happened after unix time `since`, across all my games.
    Events { since: f64 },
    Ping,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameSummary {
    pub code: String,
    pub name: String,
    pub status: sim::game::Status,
    pub t: f64,
    pub length: f64,
    pub players: Vec<String>,
    pub max_players: usize,
    pub me: PlayerId,
    pub last_event: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome { user: String },
    Error { msg: String },
    Games(Vec<GameSummary>),
    /// Full authoritative game state. `real_time` = server unix time (s) when `game.t` was taken.
    State { game: Box<Game>, me: PlayerId, real_time: f64 },
    /// (game code, unix time of the event, event)
    Events(Vec<(String, f64, Event)>),
    /// Result of a `Cmd` (ok or the rule it broke).
    CmdResult { ok: bool, msg: String },
    Pong,
}

pub fn encode<T: Serialize>(m: &T) -> String {
    let mut s = serde_json::to_string(m).expect("serialize");
    s.push('\n');
    s
}

pub fn decode<'a, T: Deserialize<'a>>(line: &'a str) -> Result<T, String> {
    serde_json::from_str(line.trim_end()).map_err(|e| e.to_string())
}

pub fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

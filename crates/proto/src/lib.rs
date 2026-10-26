//! Wire protocol between the game client and server: one JSON object per line over TLS (TCP).
pub mod wire;

use serde::{Deserialize, Serialize};
use sim::game::{Command, Event, Game, PlayerId};
use sim::system::SystemSize;

pub const DEFAULT_PORT: u16 = 7777;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Log in (or create the account when `register`).
    Hello {
        user: String,
        pass: String,
        register: bool,
        /// Needed to register when the server is invite-only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invite: Option<String>,
    },
    ListGames,
    CreateGame { name: String, size: SystemSize, length_days: f64, speed: f64, max_players: usize, bots: Vec<String> },
    JoinGame { code: String },
    AddBot { code: String, difficulty: String },
    /// Host only, while waiting for the start.
    RemoveBot { code: String, player: PlayerId },
    SetFaction { code: String, faction: String },
    StartGame { code: String },
    /// Subscribe to a game's state (unsubscribes from the previous one).
    Open { code: String },
    Cmd { code: String, cmd: Command },
    /// Notification poll: events for me that happened after unix time `since`, across all my games.
    Events { since: f64 },
    Ping,
}

/// Longest password the server will hash.
pub const MAX_PASS: usize = 256;

impl ClientMsg {
    /// Size limits on everything a client controls, checked before the server acts on a message.
    /// (`sim::Game::apply` checks a command's numbers and ids itself.)
    pub fn check(&self) -> Result<(), &'static str> {
        let short = |s: &str| s.len() <= 64;
        let ok_f = |x: f64| x.is_finite();
        let ok = match self {
            ClientMsg::Hello { user, pass, invite, .. } => user.len() <= 64 && pass.len() <= MAX_PASS && invite.as_deref().is_none_or(short),
            ClientMsg::CreateGame { name, length_days, speed, bots, .. } => {
                name.len() <= 200 && ok_f(*length_days) && ok_f(*speed) && bots.len() <= 16 && bots.iter().all(|b| short(b))
            }
            ClientMsg::JoinGame { code } | ClientMsg::StartGame { code } | ClientMsg::Open { code } | ClientMsg::RemoveBot { code, .. } => short(code),
            ClientMsg::AddBot { code, difficulty } => short(code) && short(difficulty),
            ClientMsg::SetFaction { code, faction } => short(code) && short(faction),
            ClientMsg::Cmd { code, .. } => short(code),
            ClientMsg::Events { since } => !since.is_nan(),
            ClientMsg::ListGames | ClientMsg::Ping => true,
        };
        if ok {
            Ok(())
        } else {
            Err("message fields out of range")
        }
    }
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
    /// "While you were away": sent on `Open` (before the State) with everything for you since you
    /// were last connected to that game. `since`/`now` are game times.
    Report { code: String, since: f64, now: f64, events: Vec<Event> },
    /// Result of a `Cmd` (ok or the rule it broke).
    CmdResult { ok: bool, msg: String },
    Pong,
}

pub fn encode<T: Serialize>(m: &T) -> String {
    // only maps with non-string keys can fail, and the protocol has none
    let mut s = serde_json::to_string(m).unwrap_or_else(|_| "\"Ping\"".into());
    s.push('\n');
    s
}

pub fn decode<'a, T: Deserialize<'a>>(line: &'a str) -> Result<T, String> {
    serde_json::from_str(line.trim_end()).map_err(|e| e.to_string())
}

pub fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

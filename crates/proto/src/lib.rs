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
        /// Log in with a session token from an earlier Welcome instead of the password (`pass` empty).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        /// "Stay logged in": a password login asks for a session token in the Welcome.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        stay: bool,
    },
    /// Revoke the session token this device holds (logging out).
    Logout { token: String },
    ListGames,
    CreateGame {
        name: String,
        size: SystemSize,
        length_days: f64,
        speed: f64,
        max_players: usize,
        bots: Vec<String>,
        /// number of teams (0 = free-for-all), see `sim::game::team_options`
        #[serde(default)]
        teams: usize,
    },
    JoinGame { code: String },
    AddBot { code: String, difficulty: String },
    /// Host only, while waiting for the start.
    RemoveBot { code: String, player: PlayerId },
    SetFaction { code: String, faction: String },
    /// Lobby: move yourself (or, as host, a bot) to another team.
    SetTeam { code: String, player: PlayerId, team: u8 },
    StartGame { code: String },
    /// Subscribe to a game's state (unsubscribes from the previous one).
    Open { code: String },
    Cmd { code: String, cmd: Command },
    /// Notification poll: events for me that happened after unix time `since`, across all my games.
    Events { since: f64 },
    Ping,
    /// Bug report / feature request, appended to the server's feedback log.
    Feedback { bug: bool, text: String, game: Option<String>, client: String },
    /// Host only: delete the war for everyone (it disappears from every player's list).
    DeleteGame { code: String },
    /// Send push notifications (battles, enemies inbound) for this account here too.
    PushSubscribe { sub: PushSub },
    /// Stop pushing to the subscription with this endpoint / token (logging out).
    PushUnsubscribe { id: String },
    /// Rewind: the war's map history as I saw it, snapshots after game time `since` (a page at a time).
    History { code: String, since: f64 },
    /// A finished battle to replay, as I saw it.
    Replay { code: String, id: u32 },
}

/// Where the server pushes notifications to (see server/src/push.rs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PushSub {
    /// A browser's Web Push subscription (`PushSubscription.toJSON()`; keys in base64url).
    Web { endpoint: String, p256dh: String, auth: String },
    /// The Android app's Firebase Cloud Messaging registration token.
    Fcm { token: String },
}

impl PushSub {
    /// The endpoint or token: what `PushUnsubscribe` names.
    pub fn id(&self) -> &str {
        match self {
            PushSub::Web { endpoint, .. } => endpoint,
            PushSub::Fcm { token } => token,
        }
    }
}

/// Session tokens: 32 random bytes as 64 lowercase hex digits.
pub fn ok_token(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
            ClientMsg::Hello { user, pass, invite, token, .. } => {
                user.len() <= 64 && pass.len() <= MAX_PASS && invite.as_deref().is_none_or(short) && token.as_deref().is_none_or(ok_token)
            }
            ClientMsg::Logout { token } => ok_token(token),
            ClientMsg::CreateGame { name, length_days, speed, bots, .. } => {
                name.len() <= 200 && ok_f(*length_days) && ok_f(*speed) && bots.len() <= 16 && bots.iter().all(|b| short(b))
            }
            ClientMsg::JoinGame { code } | ClientMsg::StartGame { code } | ClientMsg::Open { code } | ClientMsg::RemoveBot { code, .. } | ClientMsg::DeleteGame { code } | ClientMsg::SetTeam { code, .. } => short(code),
            ClientMsg::AddBot { code, difficulty } => short(code) && short(difficulty),
            ClientMsg::SetFaction { code, faction } => short(code) && short(faction),
            ClientMsg::Cmd { code, .. } => short(code),
            ClientMsg::Events { since } => !since.is_nan(),
            ClientMsg::History { code, since } => short(code) && !since.is_nan(),
            ClientMsg::Replay { code, .. } => short(code),
            ClientMsg::Feedback { text, game, client, .. } => text.len() <= 16_000 && game.as_deref().is_none_or(short) && client.len() <= 200,
            ClientMsg::PushSubscribe { sub: PushSub::Web { endpoint, p256dh, auth } } => {
                endpoint.len() <= 1024 && endpoint.starts_with("https://") && p256dh.len() <= 128 && auth.len() <= 64
            }
            ClientMsg::PushSubscribe { sub: PushSub::Fcm { token } } => !token.is_empty() && token.len() <= 512,
            ClientMsg::PushUnsubscribe { id } => id.len() <= 1024,
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
    Welcome {
        user: String,
        /// A new session token, when the Hello asked to stay logged in (store it instead of the password).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        /// Newest released app / browser build ("0.8.3"), when the server knows it. Clients older
        /// than this tell the player to update (old servers don't send it, old clients ignore it).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        latest_client: Option<String>,
        /// Where the update card's UPDATE button takes phones: the server's own download page
        /// (`server --update-url`). Clients fall back to their built-in page without it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        update_url: Option<String>,
    },
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
    /// The war you had open was deleted by its host.
    GameDeleted { code: String, name: String },
    /// Answer to `History`: snapshots in time order, and every finished battle you may replay.
    /// `more` = ask again from the last snapshot's time for the rest.
    History { code: String, snaps: Vec<sim::replay::Snap>, battles: Vec<sim::replay::BattleInfo>, more: bool },
    /// Answer to `Replay`.
    Replay { code: String, rec: Box<sim::replay::BattleRec> },
}

/// "v0.8.2" / "0.8.2" / "0.8" → (0, 8, 2); a "-beta"/"+build" suffix is ignored. None if it isn't a version.
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim();
    let v = v.strip_prefix('v').unwrap_or(v);
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let mut num = |need: bool| match parts.next() {
        Some(p) => p.parse::<u32>().ok(),
        None if need => None,
        None => Some(0),
    };
    let ver = (num(true)?, num(false)?, num(false)?);
    parts.next().is_none().then_some(ver)
}

/// A web page a client may open for updating: http(s), short, nothing that could break out of a URL.
pub fn ok_update_url(u: &str) -> bool {
    (u.starts_with("https://") || u.starts_with("http://"))
        && u.len() <= 256
        && u.len() > "https://".len()
        && u.chars().all(|c| c.is_ascii_graphic() && !"\"'<>\\`{}|^".contains(c))
}

/// True when `latest` is a newer version than `mine` (false if either doesn't parse).
pub fn is_newer(latest: &str, mine: &str) -> bool {
    matches!((parse_version(latest), parse_version(mine)), (Some(l), Some(m)) if l > m)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v0.8.2"), Some((0, 8, 2)));
        assert_eq!(parse_version("0.10"), Some((0, 10, 0)));
        assert_eq!(parse_version("1.2.3-beta+x"), Some((1, 2, 3)));
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("0.8.2.1"), None);
        assert_eq!(parse_version("latest"), None);
        assert!(is_newer("0.8.10", "0.8.9"));
        assert!(is_newer("v0.9.0", "0.8.2"));
        assert!(is_newer("1.0", "0.99.99"));
        assert!(!is_newer("0.8.2", "v0.8.2"));
        assert!(!is_newer("0.8.1", "0.8.2"));
        assert!(!is_newer("garbage", "0.8.2"));
        assert!(!is_newer("0.9.0", "dev"));
    }

    #[test]
    fn update_urls_are_plain_web_links() {
        assert!(ok_update_url("https://hohmannsreach.duckdns.org:7777/"));
        assert!(ok_update_url("http://192.168.0.105:7777/hohmann-reach.apk"));
        assert!(!ok_update_url("https://"));
        assert!(!ok_update_url("intent://x#Intent;end"));
        assert!(!ok_update_url("javascript:alert(1)"));
        assert!(!ok_update_url("https://a b"));
        assert!(!ok_update_url("https://a\"><script>"));
        assert!(!ok_update_url(&format!("https://{}", "a".repeat(300))));
    }

    #[test]
    fn welcome_without_latest_client_still_decodes() {
        // an old server's Welcome
        let m: ServerMsg = decode(r#"{"Welcome":{"user":"jonas"}}"#).unwrap();
        assert!(matches!(m, ServerMsg::Welcome { latest_client: None, .. }));
        assert!(matches!(m, ServerMsg::Welcome { update_url: None, .. }));
        let line = encode(&ServerMsg::Welcome { user: "j".into(), token: None, latest_client: Some("0.9.0".into()), update_url: Some("https://h/".into()) });
        assert!(line.contains(r#""latest_client":"0.9.0""#));
        // an old client's view of it: unknown fields are ignored
        #[derive(Deserialize)]
        enum OldServerMsg {
            Welcome { user: String },
        }
        let OldServerMsg::Welcome { user } = decode::<OldServerMsg>(&line).unwrap();
        assert_eq!(user, "j");
    }
}

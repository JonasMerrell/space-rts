//! Authoritative game server: accounts, lobbies with join codes, persistent real-time games.
//! One JSON message per line over TLS (see `proto`), or over a WebSocket for the browser build (`ws`). Thread per connection, one ticker thread.
//! Internet-facing: see `guard` for the abuse limits and `tls` for the certificate.
pub mod guard;
pub mod http;
pub mod tls;
pub mod ws;
#[cfg(test)]
mod fuzz;

use guard::{Bucket, Guard, Limits};
use proto::{decode, encode, unix_now, wire, ClientMsg, GameSummary, ServerMsg};
use sha2::{Digest, Sha256};
use sim::game::{Game, PlayerId, Settings, Status};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
struct Account {
    /// argon2id PHC string. Accounts from before it have only `salt` + `hash` (sha256) and are
    /// upgraded on their next good login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    phc: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    salt: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    hash: String,
    /// "Stay logged in" sessions: sha256 of each token (the token itself is never stored).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tokens: Vec<Session>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
struct Session {
    sha256: String,
    /// unix time it was last used to log in
    last: f64,
}

/// Sessions per account (the least recently used goes first) and how long an unused one lasts.
const MAX_SESSIONS: usize = 10;
const SESSION_TTL: f64 = 90.0 * 86_400.0;

fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// 256 random bits as hex.
fn new_token() -> String {
    (0..4).map(|_| format!("{:016x}", random_u64())).collect()
}

impl Account {
    fn prune_sessions(&mut self, now: f64) {
        self.tokens.retain(|t| now - t.last < SESSION_TTL);
        self.tokens.sort_by(|a, b| b.last.total_cmp(&a.last));
        self.tokens.truncate(MAX_SESSIONS);
    }
    /// A new session token (returned once; only its hash is kept).
    fn mint(&mut self, now: f64) -> String {
        let t = new_token();
        self.tokens.push(Session { sha256: token_hash(&t), last: now });
        self.prune_sessions(now);
        t
    }
}

/// Lines queued for a slow client before we give up on it (a State is sent every 20 s at most).
const SEND_QUEUE: usize = 256;

struct Conn {
    user: Option<String>,
    open: Option<String>,
    tx: SyncSender<String>,
    ip: IpAddr,
    /// to cut off a client that stops reading
    sock: Option<TcpStream>,
}

impl Conn {
    fn push(&self, line: String) {
        if let Err(TrySendError::Full(_)) = self.tx.try_send(line) {
            if let Some(s) = &self.sock {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        }
    }
}

/// How the server listens. `Config::new(data)` = self-signed TLS, plaintext only from the LAN.
#[derive(Clone, Debug)]
pub struct Config {
    pub data: PathBuf,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub plain: Plain,
    /// Invite-only registration: one code per line, each used once (removed from the file).
    pub invites: Option<PathBuf>,
    pub limits: Limits,
    /// CA-signed cert + key for browsers (e.g. Let's Encrypt fullchain.pem/privkey.pem), served to
    /// TLS clients that offer ALPN http/1.1 or h2; the app offers none and keeps the pinned cert.
    pub web_cert: Option<PathBuf>,
    pub web_key: Option<PathBuf>,
    /// Public host names of the browser build (WebSocket origins https://name[:port]).
    pub web_hosts: Vec<String>,
    /// Port the server listens on (for the origins and https redirects).
    pub port: u16,
    /// Browser build files, scanned from `<data>/public/web/` when the server starts.
    pub site: Arc<http::Site>,
}

/// Who may connect without TLS (old app builds, LAN tools, tests).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plain {
    Never,
    /// loopback and private (RFC 1918 / ULA / link-local) addresses only
    Lan,
}

impl Config {
    pub fn new(data: PathBuf) -> Config {
        Config {
            data,
            tls_cert: None,
            tls_key: None,
            plain: Plain::Lan,
            invites: None,
            limits: Limits::default(),
            web_cert: None,
            web_key: None,
            web_hosts: vec![],
            port: proto::DEFAULT_PORT,
            site: Arc::default(),
        }
    }

    /// Origins a browser page of ours can have: https://host and https://host:port.
    fn origins(&self) -> Vec<String> {
        self.web_hosts.iter().flat_map(|h| [format!("https://{h}"), format!("https://{h}:{}", self.port)]).collect()
    }
}

pub(crate) fn is_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local(),
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return is_lan(IpAddr::V4(v4));
            }
            let seg = v.segments()[0];
            v.is_loopback() || (seg & 0xfe00) == 0xfc00 || (seg & 0xffc0) == 0xfe80
        }
    }
}

fn ok_username(u: &str) -> bool {
    !u.is_empty() && u.chars().count() <= 24 && u.chars().all(|c| !c.is_control() && c != '\u{200b}' && c != '\u{feff}')
}

pub struct Server {
    data: PathBuf,
    invites: Option<PathBuf>,
    guard: Option<Arc<Guard>>,
    accounts: HashMap<String, Account>,
    games: HashMap<String, Game>,
    conns: HashMap<u64, Conn>,
    next_conn: u64,
    dirty: HashSet<String>,
    /// games whose state changed since the last broadcast
    changed: HashSet<String>,
}

fn legacy_hash(salt: &str, pass: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(b":");
    h.update(pass.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// From the OS CSPRNG: join codes, game seeds and invite codes must not be guessable.
fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    if rustls::crypto::ring::default_provider().secure_random.fill(&mut b).is_err() {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        return h.finish();
    }
    u64::from_le_bytes(b)
}

/// `n` fresh invite codes like `K7QM-2XPA` (40 bits each), appended to `path`; returns them.
pub fn make_invites(path: &Path, n: usize) -> std::io::Result<Vec<String>> {
    const A: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let codes: Vec<String> = (0..n)
        .map(|_| {
            let mut x = random_u64();
            let mut c: String = (0..8)
                .map(|_| {
                    let ch = A[(x % 32) as usize] as char;
                    x /= 32;
                    ch
                })
                .collect();
            c.insert(4, '-');
            c
        })
        .collect();
    let mut text = std::fs::read_to_string(path).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for c in &codes {
        text.push_str(c);
        text.push('\n');
    }
    std::fs::write(path, text)?;
    Ok(codes)
}

fn write_atomic(path: &Path, data: &str) {
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, data).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

impl Server {
    pub fn load(data: PathBuf) -> Server {
        let _ = std::fs::create_dir_all(data.join("games"));
        let accounts = std::fs::read_to_string(data.join("accounts.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let mut games = HashMap::new();
        if let Ok(rd) = std::fs::read_dir(data.join("games")) {
            for e in rd.flatten() {
                if e.path().extension().is_some_and(|x| x == "json") {
                    match std::fs::read_to_string(e.path()).map_err(|e| e.to_string()).and_then(|s| serde_json::from_str::<Game>(&s).map_err(|e| e.to_string())) {
                        Ok(g) => {
                            games.insert(g.code.clone(), g);
                        }
                        Err(err) => eprintln!("skipping {}: {err}", e.path().display()),
                    }
                }
            }
        }
        eprintln!("loaded {} accounts, {} games from {}", accounts_len(&accounts), games.len(), data.display());
        Server { data, invites: None, guard: None, accounts, games, conns: HashMap::new(), next_conn: 1, dirty: HashSet::new(), changed: HashSet::new() }
    }

    fn save(&mut self) {
        for code in self.dirty.drain() {
            if let Some(g) = self.games.get(&code) {
                if let Ok(s) = serde_json::to_string(g) {
                    write_atomic(&self.data.join("games").join(format!("{code}.json")), &s);
                }
            }
        }
    }

    fn save_accounts(&self) {
        if let Ok(s) = serde_json::to_string_pretty(&self.accounts) {
            write_atomic(&self.data.join("accounts.json"), &s);
        }
    }

    fn new_code(&self) -> String {
        const A: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
        loop {
            let mut x = random_u64();
            let code: String = (0..5)
                .map(|_| {
                    let c = A[(x % A.len() as u64) as usize] as char;
                    x /= A.len() as u64;
                    c
                })
                .collect();
            if !self.games.contains_key(&code) {
                return code;
            }
        }
    }

    fn summary(&self, g: &Game, user: &str) -> Option<GameSummary> {
        let me = g.player_by_name(user)?.id;
        Some(GameSummary {
            code: g.code.clone(),
            name: g.settings.name.clone(),
            status: g.status,
            t: g.t,
            length: g.end_t,
            players: g.players.iter().map(|p| p.name.clone()).collect(),
            max_players: g.settings.max_players,
            me,
            last_event: g.events_for(me).last().map(|e| e.seq).unwrap_or(0),
        })
    }

    fn send(&self, conn: u64, msg: &ServerMsg) {
        if let Some(c) = self.conns.get(&conn) {
            c.push(encode(msg));
        }
    }

    fn state_line(g: &Game, me: PlayerId) -> String {
        // each player only gets what their sensors know (fog of war)
        let game = serde_json::to_string(&g.view_for(me)).unwrap_or_default();
        format!("{{\"State\":{{\"game\":{game},\"me\":{me},\"real_time\":{}}}}}\n", unix_now())
    }

    fn broadcast(&mut self, code: &str) {
        let Some(g) = self.games.get(code) else { return };
        let mut lines: HashMap<PlayerId, String> = HashMap::new();
        for c in self.conns.values() {
            if c.open.as_deref() == Some(code) {
                if let Some(me) = c.user.as_deref().and_then(|u| g.player_by_name(u)).map(|p| p.id) {
                    let line = lines.entry(me).or_insert_with(|| Self::state_line(g, me));
                    c.push(line.clone());
                }
            }
        }
    }

    fn handle(&mut self, conn: u64, msg: ClientMsg) {
        let user = self.conns.get(&conn).and_then(|c| c.user.clone());
        let reply = |s: &Server, m: ServerMsg| s.send(conn, &m);
        let err = |s: &Server, m: &str| s.send(conn, &ServerMsg::Error { msg: m.into() });
        if let ClientMsg::Ping = msg {
            return reply(self, ServerMsg::Pong);
        }
        let Some(user) = user else { return err(self, "log in first") };
        // a game this player is in; any other code gets the same answer so codes can't be probed
        let member = |s: &Server, code: &str| s.games.get(code).is_some_and(|g| g.player_by_name(&user).is_some());
        match msg {
            ClientMsg::Ping => {}
            // handled outside the lock by `hello` (password hashing is slow)
            ClientMsg::Hello { .. } => err(self, "already logged in"),
            ClientMsg::Logout { token } => {
                let h = token_hash(&token);
                if let Some(a) = self.accounts.get_mut(&user) {
                    let n = a.tokens.len();
                    a.tokens.retain(|t| t.sha256 != h);
                    if a.tokens.len() != n {
                        self.save_accounts();
                    }
                }
            }
            ClientMsg::ListGames => {
                let u = user.clone();
                let mut list: Vec<GameSummary> = self.games.values().filter_map(|g| self.summary(g, &u)).collect();
                list.sort_by(|a, b| a.code.cmp(&b.code));
                reply(self, ServerMsg::Games(list));
            }
            ClientMsg::CreateGame { name, size, length_days, speed, max_players, bots } => {
                let max = self.guard.as_ref().map_or(usize::MAX, |g| g.limits.max_hosted);
                let hosted = self.games.values().filter(|g| g.status != Status::Ended && g.player_by_name(&user).is_some_and(|p| p.id == g.host)).count();
                if hosted >= max {
                    return err(self, "you're hosting too many games already; finish one first");
                }
                let code = self.new_code();
                let settings = Settings {
                    name: if name.trim().is_empty() { format!("{user}'s game") } else { name.trim().chars().take(40).collect() },
                    size,
                    max_players: max_players.clamp(2, 8),
                    length: length_days.clamp(0.1, 60.0) * 86_400.0,
                    speed: speed.clamp(1.0, 1000.0),
                };
                let mut g = Game::new(code.clone(), settings, random_u64(), &user);
                for (i, b) in bots.iter().take(7).enumerate() {
                    let b = if ["easy", "normal", "hard"].contains(&b.as_str()) { b.as_str() } else { "normal" };
                    let _ = g.add_player(&format!("Bot {} ({b})", i + 1), Some(b.to_string()));
                }
                self.games.insert(code.clone(), g);
                self.dirty.insert(code.clone());
                self.open(conn, &code);
                self.handle(conn, ClientMsg::ListGames);
            }
            ClientMsg::JoinGame { code } => {
                let code = code.trim().to_uppercase();
                let Some(g) = self.games.get_mut(&code) else {
                    self.bad_code(conn);
                    return err(self, "no game with that code");
                };
                if g.player_by_name(&user).is_none() {
                    // joining is open only while the game waits for its host to press start
                    if g.status != Status::Lobby {
                        return err(self, "that game has already started, joining is closed");
                    }
                    if let Err(e) = g.add_player(&user, None) {
                        return err(self, &e);
                    }
                }
                self.dirty.insert(code.clone());
                self.changed.insert(code.clone());
                self.open(conn, &code);
                self.handle(conn, ClientMsg::ListGames);
            }
            ClientMsg::AddBot { code, .. }
            | ClientMsg::RemoveBot { code, .. }
            | ClientMsg::SetFaction { code, .. }
            | ClientMsg::StartGame { code }
            | ClientMsg::Open { code }
            | ClientMsg::Cmd { code, .. }
                if !member(self, &code) =>
            {
                err(self, "you're not in that game")
            }
            ClientMsg::AddBot { code, difficulty } => {
                let difficulty = if ["easy", "normal", "hard"].contains(&difficulty.as_str()) { difficulty } else { "normal".to_string() };
                let Some(g) = self.games.get_mut(&code) else { return err(self, "no such game") };
                if g.player_by_name(&user).map(|p| p.id) != Some(g.host) {
                    return err(self, "only the host can add bots");
                }
                let n = (1..).find(|n| !g.players.iter().any(|p| p.name.starts_with(&format!("Bot {n} ")))).unwrap_or(1);
                if let Err(e) = g.add_player(&format!("Bot {n} ({difficulty})"), Some(difficulty)) {
                    return err(self, &e);
                }
                self.dirty.insert(code.clone());
                self.changed.insert(code);
            }
            ClientMsg::RemoveBot { code, player } => {
                let Some(g) = self.games.get_mut(&code) else { return err(self, "no such game") };
                if g.player_by_name(&user).map(|p| p.id) != Some(g.host) {
                    return err(self, "only the host can remove bots");
                }
                if g.players.get(player as usize).is_none_or(|p| p.bot.is_none()) {
                    return err(self, "that pilot isn't a bot");
                }
                if let Err(e) = g.remove_player(player) {
                    return err(self, &e);
                }
                self.dirty.insert(code.clone());
                self.changed.insert(code);
            }
            ClientMsg::SetFaction { code, faction } => {
                let Some(g) = self.games.get_mut(&code) else { return err(self, "no such game") };
                if g.status != Status::Lobby {
                    return err(self, "factions are locked once the game starts");
                }
                if let Some(p) = g.players.iter_mut().find(|p| p.name == user) {
                    p.faction = faction.chars().take(20).collect();
                }
                self.dirty.insert(code.clone());
                self.changed.insert(code);
            }
            ClientMsg::StartGame { code } => {
                let Some(g) = self.games.get_mut(&code) else { return err(self, "no such game") };
                if g.player_by_name(&user).map(|p| p.id) != Some(g.host) {
                    return err(self, "only the host can start the game");
                }
                if g.status != Status::Lobby {
                    return err(self, "the game is already running");
                }
                g.start();
                g.started_real = unix_now();
                self.dirty.insert(code.clone());
                self.changed.insert(code);
            }
            ClientMsg::Open { code } => {
                if !self.games.get(&code).is_some_and(|g| g.player_by_name(&user).is_some()) {
                    return err(self, "you're not in that game");
                }
                self.open(conn, &code);
            }
            ClientMsg::Cmd { code, cmd } => {
                let Some(g) = self.games.get_mut(&code) else { return err(self, "no such game") };
                let Some(me) = g.player_by_name(&user).map(|p| p.id) else { return err(self, "you're not in that game") };
                if g.status == Status::Running {
                    let t = g.time_at(unix_now());
                    g.advance_to(t);
                }
                let res = g.apply(me, cmd);
                // chat is the one thing a player can grow without limit; keep the recent part
                if g.chat.len() > 400 {
                    let n = g.chat.len() - 300;
                    g.chat.drain(..n);
                }
                self.dirty.insert(code.clone());
                self.changed.insert(code);
                let (ok, msg) = match res {
                    Ok(()) => (true, String::new()),
                    Err(e) => (false, e),
                };
                reply(self, ServerMsg::CmdResult { ok, msg });
            }
            ClientMsg::Events { since } => {
                let u = user.clone();
                let mut out = vec![];
                for g in self.games.values() {
                    let Some(me) = g.player_by_name(&u).map(|p| p.id) else { continue };
                    for e in g.events_for(me).filter(|e| e.kind != sim::game::EventKind::Sensor) {
                        let real = g.started_real + e.t / g.settings.speed;
                        if real > since {
                            out.push((g.code.clone(), real, e.clone()));
                        }
                    }
                }
                // a phone that was offline for days only needs the latest news
                out.sort_by(|a, b| a.1.total_cmp(&b.1));
                let skip = out.len().saturating_sub(50);
                reply(self, ServerMsg::Events(out.split_off(skip)));
            }
        }
    }

    fn open(&mut self, conn: u64, code: &str) {
        let me = {
            let Some(c) = self.conns.get_mut(&conn) else { return };
            c.open = Some(code.to_string());
            let u = c.user.clone().unwrap_or_default();
            self.games.get(code).and_then(|g| g.player_by_name(&u)).map(|p| p.id)
        };
        if let (Some(g), Some(me)) = (self.games.get_mut(code), me) {
            if g.status == Status::Running {
                let t = g.time_at(unix_now());
                g.advance_to(t);
                let since = g.players[me as usize].last_active;
                let report = ServerMsg::Report { code: code.to_string(), since, now: g.t, events: g.away_report(me, since, 300) };
                g.players[me as usize].last_active = g.t;
                self.dirty.insert(code.to_string());
                self.send(conn, &report);
            }
            let g = &self.games[code];
            let line = Self::state_line(g, me);
            if let Some(c) = self.conns.get(&conn) {
                c.push(line);
            }
        }
    }

    /// Advance running games to the current real time; broadcast and save as needed.
    fn tick(&mut self, broadcast_all: bool) {
        let now = unix_now();
        let codes: Vec<String> = self.games.keys().cloned().collect();
        for code in codes {
            let Some(g) = self.games.get_mut(&code) else { continue };
            if g.status == Status::Running {
                let before = (g.next_event, g.ships.len(), g.status);
                let t = g.time_at(now);
                g.advance_to(t);
                self.dirty.insert(code.clone());
                if (g.next_event, g.ships.len(), g.status) != before {
                    self.changed.insert(code.clone());
                }
            }
            if broadcast_all && g.status == Status::Running {
                self.changed.insert(code.clone());
            }
        }
        // players with the game open are "here": the away report starts when they leave
        for c in self.conns.values() {
            let (Some(code), Some(u)) = (c.open.as_deref(), c.user.as_deref()) else { continue };
            if let Some(g) = self.games.get_mut(code) {
                if let Some(i) = g.players.iter().position(|p| p.name == u) {
                    g.players[i].last_active = g.t;
                }
            }
        }
        let changed: Vec<String> = self.changed.drain().collect();
        for code in changed {
            self.broadcast(&code);
        }
    }
}

fn accounts_len(a: &HashMap<String, Account>) -> usize {
    a.len()
}

fn lock(s: &Mutex<Server>) -> std::sync::MutexGuard<'_, Server> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn argon2_hash(pass: &str) -> Option<String> {
    use argon2::password_hash::PasswordHasher;
    argon2::Argon2::default().hash_password(pass.as_bytes()).ok().map(|h| h.to_string())
}

/// Does `pass` match `a`? (argon2id, or the old salted sha256)
fn verify(a: &Account, pass: &str) -> bool {
    use argon2::password_hash::PasswordVerifier;
    match &a.phc {
        Some(phc) => argon2::Argon2::default().verify_password(pass.as_bytes(), phc.as_str()).is_ok(),
        None => {
            let h = legacy_hash(&a.salt, pass);
            !a.hash.is_empty() && h.len() == a.hash.len() && h.bytes().zip(a.hash.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
        }
    }
}

impl Server {
    /// Count a bad join code against this connection's IP (codes are short; don't let them be swept).
    fn bad_code(&self, conn: u64) {
        if let (Some(g), Some(c)) = (&self.guard, self.conns.get(&conn)) {
            g.login_failed(c.ip, None);
        }
    }

    /// Take one invite code out of the invite file. Ok(()) when registration is open.
    fn use_invite(&self, code: Option<&str>) -> Result<(), &'static str> {
        let Some(path) = &self.invites else { return Ok(()) };
        let code = code.map(str::trim).filter(|c| !c.is_empty()).ok_or("this server is invite-only: enter an invite code")?;
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut found = false;
        let rest: Vec<&str> = text
            .lines()
            .filter(|l| {
                let hit = !found && !l.trim().is_empty() && l.trim().eq_ignore_ascii_case(code);
                found |= hit;
                !hit
            })
            .collect();
        if !found {
            return Err("that invite code isn't valid (or was already used)");
        }
        write_atomic(path, &(rest.join("\n") + "\n"));
        Ok(())
    }
}

/// Log in (password or session token) or register. The slow part (argon2) runs without the server lock.
fn hello(server: &Mutex<Server>, guard: &Guard, id: u64, ip: IpAddr, h: Hello) {
    let Hello { user, pass, register, invite, token, stay } = h;
    let err = |m: &str| lock(server).send(id, &ServerMsg::Error { msg: m.into() });
    let user = user.trim().to_string();
    if !ok_username(&user) || (pass.is_empty() && token.is_none()) {
        return err("enter a username (max 24 chars) and password");
    }
    if let Err(e) = guard.may_login(ip, &user) {
        return err(&e);
    }
    let now = unix_now();
    if let Some(t) = token.filter(|_| !register) {
        // a saved session: no password, no new token
        let h = token_hash(&t);
        let mut s = lock(server);
        let ok = s.accounts.get_mut(&user).is_some_and(|a| {
            a.prune_sessions(now);
            match a.tokens.iter_mut().find(|x| x.sha256 == h) {
                Some(x) => {
                    x.last = now;
                    true
                }
                None => false,
            }
        });
        if !ok {
            drop(s);
            guard.login_failed(ip, Some(&user));
            return err("your saved login has expired: log in again");
        }
        s.save_accounts();
        return welcome(&mut s, id, user, None);
    }
    let acct = lock(server).accounts.get(&user).cloned();
    match acct {
        Some(a) => {
            if !guard.hashing(|| verify(&a, &pass)) {
                guard.login_failed(ip, Some(&user));
                return err("wrong password");
            }
            let phc = if a.phc.is_none() { guard.hashing(|| argon2_hash(&pass)) } else { None };
            if phc.is_some() || stay {
                let mut s = lock(server);
                let mut token = None;
                if let Some(a) = s.accounts.get_mut(&user) {
                    if let Some(phc) = phc {
                        *a = Account { phc: Some(phc), tokens: std::mem::take(&mut a.tokens), ..Account::default() };
                    }
                    token = stay.then(|| a.mint(now));
                }
                s.save_accounts();
                return welcome(&mut s, id, user, token);
            }
        }
        None if register => {
            if user.to_lowercase().starts_with("bot ") {
                return err("names starting with \"Bot\" are kept for bots");
            }
            if let Err(e) = guard.may_register(ip) {
                return err(&e);
            }
            let Some(phc) = guard.hashing(|| argon2_hash(&pass)) else { return err("couldn't create the account") };
            let mut s = lock(server);
            if s.accounts.contains_key(&user) {
                drop(s);
                return err("that name was just taken");
            }
            if let Err(e) = s.use_invite(invite.as_deref()) {
                drop(s);
                guard.login_failed(ip, None);
                return err(e);
            }
            let mut a = Account { phc: Some(phc), ..Account::default() };
            let token = stay.then(|| a.mint(now));
            s.accounts.insert(user.clone(), a);
            s.save_accounts();
            guard.registered(ip);
            return welcome(&mut s, id, user, token);
        }
        None => {
            guard.login_failed(ip, None);
            return err("no such account (tap Register to create it)");
        }
    }
    welcome(&mut lock(server), id, user, None);
}

/// The fields of `ClientMsg::Hello`.
struct Hello {
    user: String,
    pass: String,
    register: bool,
    invite: Option<String>,
    token: Option<String>,
    stay: bool,
}

fn welcome(s: &mut Server, id: u64, user: String, token: Option<String>) {
    if let Some(c) = s.conns.get_mut(&id) {
        c.user = Some(user.clone());
    }
    s.send(id, &ServerMsg::Welcome { user, token });
    s.handle(id, ClientMsg::ListGames);
}

type Reader = Box<dyn BufRead + Send>;
type Writer = Box<dyn Write + Send>;

/// Sniff the first byte: 0x16 = TLS (then sniff again inside), 'G'/'H' = HTTP (download page, the
/// browser build, or a WebSocket upgrade), anything else = plaintext JSON lines (LAN only).
fn setup(cfg: &Config, tls: &Arc<rustls::ServerConfig>, stream: TcpStream, ip: IpAddr) -> Option<(Reader, Writer)> {
    let mut b = [0u8; 1];
    let n = stream.peek(&mut b).ok()?;
    if n == 0 {
        return None;
    }
    let plain_ok = cfg.plain == Plain::Lan && is_lan(ip);
    let secure = b[0] == 0x16;
    let (mut r, mut w): (Reader, Writer) = if secure {
        let conn = rustls::ServerConnection::new(tls.clone()).ok()?;
        let (r, w) = wire::split(conn, stream).ok()?;
        (Box::new(BufReader::with_capacity(16 * 1024, r)), Box::new(w))
    } else if http::looks_like_http(&b) || plain_ok {
        let w = stream.try_clone().ok()?;
        (Box::new(BufReader::new(stream)), Box::new(w))
    } else {
        // an old app build: tell it why instead of just hanging up
        let mut s = stream;
        let _ = s.write_all(encode(&ServerMsg::Error { msg: "this server needs the updated app (secure connection)".into() }).as_bytes());
        return None;
    };
    if !http::looks_like_http(r.fill_buf().ok()?) {
        return Some((r, w));
    }
    let origins = cfg.origins();
    let https_base = (cfg.web_cert.is_some()).then(|| cfg.web_hosts.first().map(|h| format!("https://{h}:{}", cfg.port))).flatten();
    let ctx = http::Ctx {
        ip,
        data: &cfg.data,
        site: &cfg.site,
        secure,
        game_ok: secure || plain_ok,
        origins: &origins,
        https_base: https_base.as_deref(),
        head_timeout: cfg.limits.handshake_timeout,
    };
    match http::serve(&mut r, &mut *w, &ctx) {
        http::Outcome::WebSocket => {
            let (r, w) = ws::split(r, w);
            Some((Box::new(r), Box::new(w)))
        }
        http::Outcome::Close => {
            let _ = w.flush();
            None
        }
    }
}

fn client_thread(server: Arc<Mutex<Server>>, guard: Arc<Guard>, cfg: Arc<Config>, tls: Arc<rustls::ServerConfig>, stream: TcpStream, ip: IpAddr) {
    let l = &guard.limits;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(l.handshake_timeout));
    let _ = stream.set_write_timeout(Some(l.write_timeout));
    let kill = stream.try_clone().ok();
    let Some((mut reader, mut w)) = setup(&cfg, &tls, stream, ip) else { return };
    if let Some(k) = &kill {
        let _ = k.set_read_timeout(Some(l.idle_timeout));
    }
    let (tx, rx): (SyncSender<String>, Receiver<String>) = sync_channel(SEND_QUEUE);
    let id = {
        let mut s = lock(&server);
        let id = s.next_conn;
        s.next_conn += 1;
        s.conns.insert(id, Conn { user: None, open: None, tx, sock: kill.as_ref().and_then(|k| k.try_clone().ok()), ip });
        id
    };
    let writer = thread::spawn(move || {
        for line in rx {
            if w.write_all(line.as_bytes()).is_err() {
                break;
            }
        }
    });
    let mut bucket = Bucket::new(l.msg_burst);
    let mut heavy = Bucket::new(l.heavy_burst);
    loop {
        let line = match wire::read_line(&mut reader, wire::MAX_CLIENT_LINE) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::InvalidData {
                    lock(&server).send(id, &ServerMsg::Error { msg: format!("bad message: {e}") });
                }
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if !bucket.take(l.msg_rate, l.msg_burst) {
            guard::log_limited(ip);
            lock(&server).send(id, &ServerMsg::Error { msg: "too many messages, slow down".into() });
            break;
        }
        let msg = decode::<ClientMsg>(&line).and_then(|m| m.check().map(|_| m).map_err(String::from));
        let is_heavy = matches!(
            &msg,
            Ok(ClientMsg::Open { .. } | ClientMsg::JoinGame { .. } | ClientMsg::CreateGame { .. } | ClientMsg::ListGames | ClientMsg::Events { .. })
                | Ok(ClientMsg::Cmd { cmd: sim::game::Command::Chat { .. }, .. })
        );
        if is_heavy && !heavy.take(l.heavy_rate, l.heavy_burst) {
            guard::log_limited(ip);
            lock(&server).send(id, &ServerMsg::Error { msg: "too many requests, wait a moment".into() });
            continue;
        }
        match msg {
            Ok(ClientMsg::Hello { user, pass, register, invite, token, stay }) => {
                let logged_in = lock(&server).conns.get(&id).is_some_and(|c| c.user.is_some());
                if logged_in {
                    lock(&server).send(id, &ServerMsg::Error { msg: "already logged in".into() });
                } else if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hello(&server, &guard, id, ip, Hello { user, pass, register, invite, token, stay }))).is_err() {
                    lock(&server).send(id, &ServerMsg::Error { msg: "internal error logging in".into() });
                }
            }
            Ok(m) => {
                let mut s = lock(&server);
                // a bug in one command must never take the whole server down
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.handle(id, m)));
                if r.is_err() {
                    s.send(id, &ServerMsg::Error { msg: "internal error handling that request".into() });
                }
            }
            Err(e) => lock(&server).send(id, &ServerMsg::Error { msg: format!("bad message: {}", e.chars().take(200).collect::<String>()) }),
        }
    }
    lock(&server).conns.remove(&id); // drops tx: the writer drains what's queued, then ends
    let _ = writer.join();
    if let Some(k) = kill {
        let _ = k.shutdown(std::net::Shutdown::Both);
    }
}

/// Run the server on `listener` forever, persisting to `data` (self-signed TLS, plaintext from the LAN).
pub fn serve(listener: TcpListener, data: PathBuf) {
    if let Err(e) = serve_with(listener, Config::new(data)) {
        eprintln!("server: {e}");
    }
}

/// Run the server with `cfg`. Returns only if the TLS setup fails.
pub fn serve_with(listener: TcpListener, mut cfg: Config) -> Result<(), String> {
    let web = match (cfg.web_cert.as_deref(), cfg.web_key.as_deref()) {
        (Some(c), Some(k)) => Some((c, k)),
        (None, None) => None,
        _ => return Err("--web-cert and --web-key go together".into()),
    };
    let (tls, fp) = tls::load_with_web(&cfg.data, cfg.tls_cert.as_deref(), cfg.tls_key.as_deref(), web)?;
    eprintln!("TLS cert sha256 {fp}");
    if let Ok(a) = listener.local_addr() {
        cfg.port = a.port();
    }
    let web_dir = cfg.data.join("public").join("web");
    cfg.site = Arc::new(http::Site::scan(&web_dir));
    if !cfg.site.is_empty() {
        eprintln!("browser build from {} at /play/", web_dir.display());
    }
    let guard = Guard::new(cfg.limits.clone());
    let mut srv = Server::load(cfg.data.clone());
    srv.invites = cfg.invites.clone();
    srv.guard = Some(guard.clone());
    let server = Arc::new(Mutex::new(srv));
    let cfg = Arc::new(cfg);
    {
        let server = server.clone();
        thread::spawn(move || {
            let mut last_save = Instant::now();
            let mut last_bcast = Instant::now();
            loop {
                thread::sleep(Duration::from_millis(1000));
                let mut s = lock(&server);
                let all = last_bcast.elapsed() > Duration::from_secs(20);
                if all {
                    last_bcast = Instant::now();
                }
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.tick(all))).is_err() {
                    eprintln!("panic during tick");
                }
                if last_save.elapsed() > Duration::from_secs(30) {
                    s.save();
                    last_save = Instant::now();
                }
            }
        });
    }
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(_) => {
                // e.g. out of file descriptors: don't spin
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        let Ok(peer) = stream.peer_addr() else { continue };
        let ip = peer.ip();
        let Some(ticket) = guard.admit(ip) else { continue };
        let (server, guard, cfg, tls) = (server.clone(), guard.clone(), cfg.clone(), tls.clone());
        thread::spawn(move || {
            let _ticket = ticket;
            client_thread(server, guard, cfg, tls, stream, ip);
        });
    }
    Ok(())
}

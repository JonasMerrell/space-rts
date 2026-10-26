//! Authoritative game server: accounts, lobbies with join codes, persistent real-time games.
//! One JSON message per line over TCP (see `proto`). Thread per connection, one ticker thread.
use proto::{decode, encode, unix_now, ClientMsg, GameSummary, ServerMsg};
use sha2::{Digest, Sha256};
use sim::game::{Game, PlayerId, Settings, Status};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Account {
    salt: String,
    hash: String,
}

struct Conn {
    user: Option<String>,
    open: Option<String>,
    tx: Sender<String>,
}

pub struct Server {
    data: PathBuf,
    accounts: HashMap<String, Account>,
    games: HashMap<String, Game>,
    conns: HashMap<u64, Conn>,
    next_conn: u64,
    dirty: HashSet<String>,
    /// games whose state changed since the last broadcast
    changed: HashSet<String>,
}

fn hash(salt: &str, pass: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(b":");
    h.update(pass.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    h.finish()
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
        Server { data, accounts, games, conns: HashMap::new(), next_conn: 1, dirty: HashSet::new(), changed: HashSet::new() }
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
            let _ = c.tx.send(encode(msg));
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
                    let _ = c.tx.send(line.clone());
                }
            }
        }
    }

    fn handle(&mut self, conn: u64, msg: ClientMsg) {
        let user = self.conns.get(&conn).and_then(|c| c.user.clone());
        let reply = |s: &Server, m: ServerMsg| s.send(conn, &m);
        let err = |s: &Server, m: &str| s.send(conn, &ServerMsg::Error { msg: m.into() });
        match msg {
            ClientMsg::Ping => reply(self, ServerMsg::Pong),
            ClientMsg::Hello { user, pass, register } => {
                let user = user.trim().to_string();
                if user.is_empty() || user.len() > 24 || pass.is_empty() {
                    return err(self, "enter a username (max 24 chars) and password");
                }
                match self.accounts.get(&user) {
                    Some(a) if a.hash == hash(&a.salt, &pass) => {}
                    Some(_) => return err(self, "wrong password"),
                    None if register => {
                        let salt = format!("{:016x}", random_u64());
                        let h = hash(&salt, &pass);
                        self.accounts.insert(user.clone(), Account { salt, hash: h });
                        self.save_accounts();
                    }
                    None => return err(self, "no such account (tap Register to create it)"),
                }
                if let Some(c) = self.conns.get_mut(&conn) {
                    c.user = Some(user.clone());
                }
                reply(self, ServerMsg::Welcome { user });
                self.handle(conn, ClientMsg::ListGames);
            }
            _ if user.is_none() => err(self, "log in first"),
            ClientMsg::ListGames => {
                let u = user.unwrap();
                let mut list: Vec<GameSummary> = self.games.values().filter_map(|g| self.summary(g, &u)).collect();
                list.sort_by(|a, b| a.code.cmp(&b.code));
                reply(self, ServerMsg::Games(list));
            }
            ClientMsg::CreateGame { name, size, length_days, speed, max_players, bots } => {
                let code = self.new_code();
                let settings = Settings {
                    name: if name.trim().is_empty() { format!("{}'s game", user.as_ref().unwrap()) } else { name.trim().chars().take(40).collect() },
                    size,
                    max_players: max_players.clamp(2, 8),
                    length: length_days.clamp(0.1, 60.0) * 86_400.0,
                    speed: speed.clamp(1.0, 1000.0),
                };
                let mut g = Game::new(code.clone(), settings, random_u64(), user.as_ref().unwrap());
                for (i, b) in bots.iter().enumerate() {
                    let _ = g.add_player(&format!("Bot {} ({b})", i + 1), Some(b.clone()));
                }
                self.games.insert(code.clone(), g);
                self.dirty.insert(code.clone());
                self.open(conn, &code);
                self.handle(conn, ClientMsg::ListGames);
            }
            ClientMsg::JoinGame { code } => {
                let code = code.trim().to_uppercase();
                let Some(g) = self.games.get_mut(&code) else { return err(self, "no game with that code") };
                if g.player_by_name(user.as_ref().unwrap()).is_none() {
                    if let Err(e) = g.add_player(user.as_ref().unwrap(), None) {
                        return err(self, &e);
                    }
                }
                self.dirty.insert(code.clone());
                self.changed.insert(code.clone());
                self.open(conn, &code);
                self.handle(conn, ClientMsg::ListGames);
            }
            ClientMsg::AddBot { code, difficulty } => {
                let Some(g) = self.games.get_mut(&code) else { return err(self, "no such game") };
                if g.player_by_name(user.as_ref().unwrap()).map(|p| p.id) != Some(g.host) {
                    return err(self, "only the host can add bots");
                }
                let n = g.players.iter().filter(|p| p.bot.is_some()).count() + 1;
                if let Err(e) = g.add_player(&format!("Bot {n} ({difficulty})"), Some(difficulty)) {
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
                if let Some(p) = g.players.iter_mut().find(|p| Some(&p.name) == user.as_ref()) {
                    p.faction = faction.chars().take(20).collect();
                }
                self.dirty.insert(code.clone());
                self.changed.insert(code);
            }
            ClientMsg::StartGame { code } => {
                let Some(g) = self.games.get_mut(&code) else { return err(self, "no such game") };
                if g.player_by_name(user.as_ref().unwrap()).map(|p| p.id) != Some(g.host) {
                    return err(self, "only the host can start the game");
                }
                g.start();
                g.started_real = unix_now();
                self.dirty.insert(code.clone());
                self.changed.insert(code);
            }
            ClientMsg::Open { code } => {
                if !self.games.get(&code).is_some_and(|g| g.player_by_name(user.as_ref().unwrap()).is_some()) {
                    return err(self, "you're not in that game");
                }
                self.open(conn, &code);
            }
            ClientMsg::Cmd { code, cmd } => {
                let Some(g) = self.games.get_mut(&code) else { return err(self, "no such game") };
                let Some(me) = g.player_by_name(user.as_ref().unwrap()).map(|p| p.id) else { return err(self, "you're not in that game") };
                if g.status == Status::Running {
                    let t = g.time_at(unix_now());
                    g.advance_to(t);
                }
                let res = g.apply(me, cmd);
                self.dirty.insert(code.clone());
                self.changed.insert(code);
                let (ok, msg) = match res {
                    Ok(()) => (true, String::new()),
                    Err(e) => (false, e),
                };
                reply(self, ServerMsg::CmdResult { ok, msg });
            }
            ClientMsg::Events { since } => {
                let u = user.unwrap();
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
                let _ = c.tx.send(line);
            }
        }
    }

    /// Advance running games to the current real time; broadcast and save as needed.
    fn tick(&mut self, broadcast_all: bool) {
        let now = unix_now();
        let codes: Vec<String> = self.games.keys().cloned().collect();
        for code in codes {
            let g = self.games.get_mut(&code).unwrap();
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

fn client_thread(server: Arc<Mutex<Server>>, stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let (tx, rx) = channel::<String>();
    let id = {
        let mut s = lock(&server);
        let id = s.next_conn;
        s.next_conn += 1;
        s.conns.insert(id, Conn { user: None, open: None, tx });
        id
    };
    let mut w = match stream.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    thread::spawn(move || {
        for line in rx {
            if w.write_all(line.as_bytes()).is_err() {
                break;
            }
        }
        let _ = w.shutdown(std::net::Shutdown::Both);
    });
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let mut s = lock(&server);
        match decode::<ClientMsg>(&line) {
            Ok(m) => {
                // a bug in one command must never take the whole server down
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.handle(id, m)));
                if r.is_err() {
                    s.send(id, &ServerMsg::Error { msg: "internal error handling that request".into() });
                }
            }
            Err(e) => s.send(id, &ServerMsg::Error { msg: format!("bad message: {e}") }),
        }
    }
    lock(&server).conns.remove(&id);
}

/// Run the server on `listener` forever, persisting to `data`.
pub fn serve(listener: TcpListener, data: PathBuf) {
    let server = Arc::new(Mutex::new(Server::load(data)));
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
    for stream in listener.incoming().flatten() {
        let server = server.clone();
        thread::spawn(move || client_thread(server, stream));
    }
}

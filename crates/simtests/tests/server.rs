//! Server integration: real `server::serve` on 127.0.0.1:0 with a temp data dir, real TCP clients.
use proto::{decode, encode, ClientMsg, ServerMsg};
use sim::game::{Command, Game, ShipClass, Status};
use sim::system::SystemSize;
use sim::BodyId;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("simtests-server-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn start(data: PathBuf) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || server::serve(l, data));
    addr
}

struct Client {
    w: TcpStream,
    r: BufReader<TcpStream>,
    name: String,
}

impl Client {
    fn connect(addr: SocketAddr, name: &str) -> Client {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        Client { w: s.try_clone().unwrap(), r: BufReader::new(s), name: name.into() }
    }
    fn send(&mut self, m: &ClientMsg) {
        self.w.write_all(encode(m).as_bytes()).unwrap();
    }
    fn raw(&mut self, line: &str) {
        self.w.write_all(line.as_bytes()).unwrap();
    }
    /// Read messages until `f` returns Some, or panic after `secs`.
    fn wait<T>(&mut self, secs: f64, mut f: impl FnMut(&ServerMsg) -> Option<T>) -> T {
        self.try_wait(secs, &mut f).unwrap_or_else(|| panic!("{}: timed out waiting", self.name))
    }
    fn try_wait<T>(&mut self, secs: f64, f: &mut impl FnMut(&ServerMsg) -> Option<T>) -> Option<T> {
        let end = Instant::now() + Duration::from_secs_f64(secs);
        let mut line = String::new();
        while Instant::now() < end {
            line.clear();
            match self.r.read_line(&mut line) {
                Ok(0) => panic!("{}: server closed the connection", self.name),
                Ok(_) => {
                    let m: ServerMsg = decode(&line).unwrap_or_else(|e| panic!("{}: bad server line {e}: {line}", self.name));
                    if let Some(x) = f(&m) {
                        return Some(x);
                    }
                }
                Err(_) => {} // read timeout, keep polling
            }
        }
        None
    }
    fn login(&mut self, pass: &str, register: bool) -> Result<String, String> {
        self.send(&ClientMsg::Hello { user: self.name.clone(), pass: pass.into(), register });
        self.wait(5.0, |m| match m {
            ServerMsg::Welcome { user } => Some(Ok(user.clone())),
            ServerMsg::Error { msg } => Some(Err(msg.clone())),
            _ => None,
        })
    }
    fn error(&mut self) -> String {
        self.wait(5.0, |m| if let ServerMsg::Error { msg } = m { Some(msg.clone()) } else { None })
    }
    fn state(&mut self, secs: f64, mut f: impl FnMut(&Game, u32) -> bool) -> (Game, u32) {
        self.wait(secs, |m| match m {
            ServerMsg::State { game, me, .. } if f(game, *me) => Some(((**game).clone(), *me)),
            _ => None,
        })
    }
    fn cmd(&mut self, code: &str, cmd: Command) -> (bool, String) {
        self.send(&ClientMsg::Cmd { code: code.into(), cmd });
        self.wait(10.0, |m| if let ServerMsg::CmdResult { ok, msg } = m { Some((*ok, msg.clone())) } else { None })
    }
    fn ping(&mut self) -> bool {
        self.send(&ClientMsg::Ping);
        self.try_wait(5.0, &mut |m| matches!(m, ServerMsg::Pong).then_some(())).is_some()
    }
}

fn create(c: &mut Client, max_players: usize) -> String {
    c.send(&ClientMsg::CreateGame { name: "Friday Night War".into(), size: SystemSize::Small, length_days: 7.0, speed: 1.0, max_players, bots: vec![] });
    let (g, _) = c.state(5.0, |g, _| g.status == Status::Lobby);
    g.code
}

#[test]
fn accounts_register_login_and_persist() {
    let dir = temp_dir("accounts");
    let addr = start(dir.clone());
    let mut a = Client::connect(addr, "jonas");
    assert!(a.login("pw1", false).is_err(), "login before register");
    assert_eq!(a.login("pw1", true), Ok("jonas".into()));
    let mut b = Client::connect(addr, "jonas");
    assert!(b.login("wrong", false).is_err(), "wrong password accepted");
    assert!(b.login("wrong", true).is_err(), "register can't overwrite an existing account");
    assert_eq!(b.login("pw1", false), Ok("jonas".into()));
    let mut c = Client::connect(addr, "");
    assert!(c.login("x", true).is_err(), "empty username");
    let mut d = Client::connect(addr, &"x".repeat(25));
    assert!(d.login("x", true).is_err(), "25-char username");
    // passwords are not stored in the clear
    let acc = std::fs::read_to_string(dir.join("accounts.json")).unwrap();
    assert!(!acc.contains("pw1"));
    // a fresh server on the same data dir knows the account
    let addr2 = start(dir);
    let mut e = Client::connect(addr2, "jonas");
    assert_eq!(e.login("pw1", false), Ok("jonas".into()));
}

#[test]
fn must_log_in_before_anything_else() {
    let addr = start(temp_dir("nologin"));
    let mut c = Client::connect(addr, "x");
    c.send(&ClientMsg::ListGames);
    assert!(c.error().contains("log in"));
    c.send(&ClientMsg::JoinGame { code: "ABCDE".into() });
    assert!(c.error().contains("log in"));
    assert!(c.ping(), "ping works without login");
}

#[test]
fn four_players_join_by_code_and_play() {
    let addr = start(temp_dir("4p"));
    let names = ["ann", "bob", "cat", "dan"];
    let mut cs: Vec<Client> = names.iter().map(|n| Client::connect(addr, n)).collect();
    for c in &mut cs {
        c.login("pw", true).unwrap();
    }
    let code = create(&mut cs[0], 4);
    assert!(code.len() >= 4 && code.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()), "code {code}");
    assert!(!code.contains(['0', 'O', '1', 'I']), "ambiguous characters in join code {code}");
    // join with sloppy typing: lowercase + spaces
    for c in cs.iter_mut().skip(1) {
        c.send(&ClientMsg::JoinGame { code: format!(" {} ", code.to_lowercase()) });
        c.state(5.0, |g, _| g.code == code);
    }
    // 5th player bounces
    let mut eve = Client::connect(addr, "eve");
    eve.login("pw", true).unwrap();
    eve.send(&ClientMsg::JoinGame { code: code.clone() });
    assert!(eve.error().contains("full"));
    eve.send(&ClientMsg::JoinGame { code: "ZZZZZ".into() });
    assert!(eve.error().contains("code"));
    // everybody sees 4 players in the lobby
    let (g, _) = cs[3].state(5.0, |g, _| g.players.len() == 4);
    assert_eq!(g.players.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), names);
    // only the host starts
    cs[1].send(&ClientMsg::StartGame { code: code.clone() });
    assert!(cs[1].error().contains("host"));
    cs[0].send(&ClientMsg::StartGame { code: code.clone() });
    let mut mes = vec![];
    for c in cs.iter_mut() {
        let (g, me) = c.state(30.0, |g, _| g.status == Status::Running);
        assert_eq!(g.players[me as usize].name, c.name, "State.me must be the receiver");
        mes.push(me);
    }
    mes.sort();
    assert_eq!(mes, vec![0, 1, 2, 3]);

    // commands round-trip and reach the other players
    let (ok, msg) = cs[2].cmd(&code, Command::Chat { text: "hello from cat".into() });
    assert!(ok, "{msg}");
    cs[0].state(30.0, |g, _| g.chat.iter().any(|m| m.text == "hello from cat"));
    // rule violations come back as CmdResult, not as a dropped connection
    // (take the id from ann's own view: fog may hide her ships from bob)
    let (g, _) = cs[0].state(30.0, |_, _| true);
    let anns = g.ships.iter().find(|s| s.owner == 0).unwrap().id;
    let (ok, msg) = cs[1].cmd(&code, Command::SetOrder { ship: anns, order: sim::game::StandingOrder::Retreat });
    assert!(!ok && msg.contains("not your ship"), "{msg}");
    // a non-member can't open or command the game
    eve.send(&ClientMsg::Open { code: code.clone() });
    assert!(eve.error().contains("not in"));
    // game list shows it for members only
    cs[3].send(&ClientMsg::ListGames);
    let n = cs[3].wait(5.0, |m| if let ServerMsg::Games(v) = m { Some(v.iter().filter(|s| s.code == code).count()) } else { None });
    assert_eq!(n, 1);
    eve.send(&ClientMsg::ListGames);
    let n = eve.wait(5.0, |m| if let ServerMsg::Games(v) = m { Some(v.len()) } else { None });
    assert_eq!(n, 0);
}

#[test]
fn reconnecting_player_gets_the_game_back() {
    let addr = start(temp_dir("reconnect"));
    let mut a = Client::connect(addr, "ann");
    a.login("pw", true).unwrap();
    let mut b = Client::connect(addr, "bob");
    b.login("pw", true).unwrap();
    let code = create(&mut a, 2);
    b.send(&ClientMsg::JoinGame { code: code.clone() });
    b.state(5.0, |g, _| g.players.len() == 2);
    a.send(&ClientMsg::StartGame { code: code.clone() });
    b.state(30.0, |g, _| g.status == Status::Running);
    drop(b);
    let mut b2 = Client::connect(addr, "bob");
    b2.login("pw", false).unwrap();
    let list = b2.wait(5.0, |m| if let ServerMsg::Games(v) = m { Some(v.clone()) } else { None });
    assert!(list.iter().any(|s| s.code == code), "rejoining player doesn't see their game");
    b2.send(&ClientMsg::Open { code: code.clone() });
    let (_, me) = b2.state(5.0, |g, _| g.status == Status::Running);
    assert_eq!(me, 1);
}

#[test]
fn malformed_lines_get_an_error_and_the_connection_survives() {
    let addr = start(temp_dir("garbage"));
    let mut c = Client::connect(addr, "x");
    c.raw("this is not json\n");
    assert!(c.error().contains("bad message"));
    c.raw("{\"Hello\":{\"user\":1}}\n");
    c.error();
    assert!(c.ping());
}

/// A client sending ids that don't exist must not be able to crash the server for everyone
/// (regression: this used to panic inside `Game::apply` and poison the server mutex).
#[test]
fn bad_ids_in_commands_dont_take_the_server_down() {
    let addr = start(temp_dir("badids"));
    let mut a = Client::connect(addr, "ann");
    a.login("pw", true).unwrap();
    let mut b = Client::connect(addr, "bob");
    b.login("pw", true).unwrap();
    let code = create(&mut a, 2);
    b.send(&ClientMsg::JoinGame { code: code.clone() });
    b.state(5.0, |g, _| g.players.len() == 2);
    a.send(&ClientMsg::StartGame { code: code.clone() });
    a.state(30.0, |g, _| g.status == Status::Running);
    a.send(&ClientMsg::Cmd { code: code.clone(), cmd: Command::Build { body: BodyId(60_000), class: ShipClass::Scout } });
    a.send(&ClientMsg::Cmd { code: code.clone(), cmd: Command::Upgrade { body: BodyId(60_000), upgrade: sim::game::Upgrade::ALL[0] } });
    let (g, _) = b.state(30.0, |_, _| true);
    let mine = g.ships.iter().find(|s| s.owner == 1).unwrap().id;
    b.send(&ClientMsg::Cmd { code: code.clone(), cmd: Command::Transfer { ships: vec![mine], to: BodyId(60_000), depart_t: None } });
    std::thread::sleep(Duration::from_millis(500));
    let mut c = Client::connect(addr, "cat");
    assert!(c.ping(), "server stopped answering after bad ids");
    assert!(c.login("pw", true).is_ok());
}

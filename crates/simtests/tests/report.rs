//! "While you were away" report: per-player sensor log (`fog.rs`), `Game::away_report`, and the
//! server sending `ServerMsg::Report` on `Open`.
use proto::{decode, encode, ClientMsg, ServerMsg};
use sim::fog::{ReportKind, LOG_CAP};
use sim::game::*;
use sim::system::SystemSize;
use sim::{BodyId, BodyKind};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

fn game4(seed: u64) -> Game {
    let s = Settings { name: "rep".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0 };
    let mut g = Game::new("REP1".into(), s, seed, "ann");
    for n in ["bob", "cat", "dan"] {
        g.add_player(n, None).unwrap();
    }
    g.start();
    g
}

fn put_ship(g: &mut Game, owner: PlayerId, class: ShipClass, at: BodyId) -> ShipId {
    let id = ShipId(g.next_ship);
    g.next_ship += 1;
    let st = class.stats();
    g.ships.push(Ship {
        id,
        owner,
        class,
        name: st.name.into(),
        hp: st.hp,
        fuel: st.fuel_cap,
        order: StandingOrder::Hold,
        route: vec![],
        state: ShipState::Parked { body: at, departure: None },
        tech: Default::default(),
        last_burn_t: no_burn(),
        intercept: None,
        cloak: None,
        mimic: None,
    });
    id
}

fn tick(g: &mut Game) {
    let t = g.last_tick + TICK;
    g.advance_to(t);
}

fn farthest_body(g: &Game, from: BodyId) -> BodyId {
    let t = g.last_tick + TICK;
    g.system
        .bodies
        .iter()
        .filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::Moon))
        .max_by(|a, b| g.system.pos(a.id, t).dist(g.system.pos(from, t)).total_cmp(&g.system.pos(b.id, t).dist(g.system.pos(from, t))))
        .unwrap()
        .id
}

#[test]
fn contacts_spotted_and_lost_are_logged_and_merged() {
    let mut g = game4(3);
    tick(&mut g);
    let home = g.players[0].home.unwrap();
    let far = farthest_body(&g, home);
    let log0 = g.players[0].sensor_log.len();
    // two enemy dreadnoughts show up over our home
    let a = put_ship(&mut g, 2, ShipClass::Dreadnought, home);
    let b = put_ship(&mut g, 2, ShipClass::Dreadnought, home);
    tick(&mut g);
    let log = &g.players[0].sensor_log[log0..];
    let spotted: Vec<_> = log.iter().filter(|x| x.kind == ReportKind::Spotted && x.owner == Some(2)).collect();
    assert_eq!(spotted.len(), 1, "one grouped item, got {log:?}");
    assert_eq!(spotted[0].body, Some(home));
    assert_eq!(spotted[0].ships.len(), 2);
    // they leave (teleported out of range) -> one "lost" item
    for s in g.ships.iter_mut().filter(|s| s.id == a || s.id == b) {
        s.state = ShipState::Parked { body: far, departure: None };
    }
    tick(&mut g);
    assert!(g.players[0].sensor_log.iter().any(|x| x.kind == ReportKind::Lost && x.ships.len() == 2));
    // back and forth within the hour merges into the same items instead of spamming
    let n = g.players[0].sensor_log.len();
    for k in 0..6 {
        let at = if k % 2 == 0 { home } else { far };
        for s in g.ships.iter_mut().filter(|s| s.id == a || s.id == b) {
            s.state = ShipState::Parked { body: at, departure: None };
        }
        tick(&mut g);
    }
    assert_eq!(g.players[0].sensor_log.len(), n, "flicker within an hour should merge");
    // only player 0 (and anyone else whose sensors saw it) logs it; nobody logs their own ships
    assert!(g.players[2].sensor_log.iter().all(|x| x.owner != Some(2)));
}

#[test]
fn sensor_log_is_capped_and_private() {
    let mut g = game4(4);
    tick(&mut g);
    let home = g.players[0].home.unwrap();
    let far = farthest_body(&g, home);
    // many separate sightings, each > 1 h apart so nothing merges
    // (a fresh ship each time: one parked over our home gets shot)
    for k in 0..(LOG_CAP + 20) {
        let at = if k % 2 == 0 { home } else { far };
        g.ships.retain(|s| s.owner != 1 || s.class != ShipClass::Dreadnought);
        put_ship(&mut g, 1, ShipClass::Dreadnought, at);
        let t = g.t + 3700.0;
        g.advance_to(t);
    }
    assert!(g.players[0].sensor_log.len() <= LOG_CAP);
    let v = g.view_for(0);
    for (i, p) in v.players.iter().enumerate() {
        if i != 0 {
            assert!(p.sensor_log.is_empty() && p.last_active == 0.0, "player {i}'s log leaked");
        }
    }
    // the json survives the round trip
    let back: Game = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
    assert_eq!(back.players[0].sensor_log.len(), g.players[0].sensor_log.len());
}

#[test]
fn no_sightings_logged_after_a_reload() {
    let mut g = game4(5);
    tick(&mut g);
    let home = g.players[0].home.unwrap();
    put_ship(&mut g, 1, ShipClass::Dreadnought, home);
    tick(&mut g);
    // reload: vision isn't saved, so the first tick must not report "new" contacts
    let mut g2: Game = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
    let n = g2.players[0].sensor_log.len();
    tick(&mut g2);
    assert_eq!(g2.players[0].sensor_log.len(), n);
}

#[test]
fn away_report_merges_events_and_sensor_log() {
    let mut g = game4(6);
    for p in 0..4u32 {
        let mine: Vec<ShipId> = g.ships.iter().filter(|s| s.owner == p && s.class != ShipClass::Tanker).map(|s| s.id).collect();
        let target = g.players[((p + 1) % 4) as usize].home.unwrap();
        let _ = g.apply(p, Command::Transfer { ships: mine[..3].to_vec(), to: target, depart_t: None });
    }
    let since = 3600.0;
    g.advance_to(30.0 * 3600.0);
    for p in 0..4u32 {
        let r = g.away_report(p, since, 300);
        assert!(r.windows(2).all(|w| w[0].t <= w[1].t), "oldest first");
        assert!(r.iter().all(|e| e.t > since && (e.to.is_none() || e.to == Some(p))));
        let sensors = r.iter().filter(|e| e.kind == EventKind::Sensor).count();
        let logged = g.players[p as usize].sensor_log.iter().filter(|x| x.t > since).count();
        assert_eq!(sensors, logged.min(r.len()));
        assert!(g.away_report(p, since, 5).len() <= 5);
        for e in &r {
            assert!(!e.body.contains('→'), "font lacks →: {}", e.body);
        }
    }
    // at least someone saw something in 30 h of four fleets crossing the system
    assert!((0..4).any(|p| g.away_report(p, since, 300).iter().any(|e| e.kind == EventKind::Sensor)));
}

// ------------------------------------------------------------------ server

struct Client {
    w: TcpStream,
    r: BufReader<TcpStream>,
}

impl Client {
    fn connect(addr: SocketAddr) -> Client {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        Client { w: s.try_clone().unwrap(), r: BufReader::new(s) }
    }
    fn send(&mut self, m: &ClientMsg) {
        self.w.write_all(encode(m).as_bytes()).unwrap();
    }
    fn wait<T>(&mut self, secs: f64, mut f: impl FnMut(&ServerMsg) -> Option<T>) -> T {
        let end = Instant::now() + Duration::from_secs_f64(secs);
        let mut line = String::new();
        while Instant::now() < end {
            line.clear();
            if let Ok(n) = self.r.read_line(&mut line) {
                assert!(n > 0, "server closed");
                if let Some(x) = f(&decode(&line).unwrap()) {
                    return x;
                }
            }
        }
        panic!("timed out")
    }
}

#[test]
fn server_sends_a_report_on_open_covering_the_time_away() {
    let dir = std::env::temp_dir().join(format!("simtests-report-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || server::serve(l, dir));

    let mut c = Client::connect(addr);
    c.send(&ClientMsg::Hello { user: "ann".into(), pass: "pw".into(), register: true, invite: None, token: None, stay: false });
    c.wait(5.0, |m| matches!(m, ServerMsg::Welcome { .. }).then_some(()));
    // 600 game seconds per real second
    c.send(&ClientMsg::CreateGame { name: "away".into(), size: SystemSize::Small, length_days: 7.0, speed: 600.0, max_players: 2, bots: vec!["easy".into()] });
    let code = c.wait(5.0, |m| match m {
        ServerMsg::State { game, .. } => Some(game.code.clone()),
        _ => None,
    });
    c.send(&ClientMsg::StartGame { code: code.clone() });
    c.wait(5.0, |m| matches!(m, ServerMsg::State { game, .. } if game.status == Status::Running).then_some(()));
    c.send(&ClientMsg::Open { code: code.clone() });
    let (_, first_now) = c.wait(5.0, |m| match m {
        ServerMsg::Report { since, now, .. } => Some((*since, *now)),
        _ => None,
    });
    drop(c);
    // away for ~2 s real = ~20 game minutes
    std::thread::sleep(Duration::from_millis(2000));
    let mut c = Client::connect(addr);
    c.send(&ClientMsg::Hello { user: "ann".into(), pass: "pw".into(), register: false, invite: None, token: None, stay: false });
    c.wait(5.0, |m| matches!(m, ServerMsg::Welcome { .. }).then_some(()));
    c.send(&ClientMsg::Open { code: code.clone() });
    // the Report comes before the State
    let (since, now) = c.wait(5.0, |m| match m {
        ServerMsg::Report { since, now, code: rc, .. } => {
            assert_eq!(rc, &code);
            Some((*since, *now))
        }
        ServerMsg::State { .. } => panic!("State before Report"),
        _ => None,
    });
    assert!(since >= first_now, "since {since} should be when we left (>= {first_now})");
    assert!(now - since > 600.0, "away {}s of game time", now - since);
}

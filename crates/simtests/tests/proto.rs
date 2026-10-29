//! Wire protocol (`proto`): framing and lossless round-trips, incl. full game state.
use proto::{decode, encode, ClientMsg, GameSummary, ServerMsg};
use sim::game::{Command, Game, Settings, ShipClass, Status};
use sim::system::SystemSize;
use sim::BodyId;

fn running_game() -> Game {
    let s = Settings { name: "wire".into(), size: SystemSize::Small, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0, teams: 0 };
    let mut g = Game::new("WXYZ".into(), s, 77, "ann");
    for n in ["bob", "cat", "dan"] {
        g.add_player(n, None).unwrap();
    }
    g.start();
    g.advance_to(600.0);
    g
}

fn client_msgs() -> Vec<ClientMsg> {
    vec![
        ClientMsg::Hello { user: "jonas".into(), pass: "pä$$ \"quoted\"\nnewline".into(), register: true, invite: None, token: None, stay: false },
        ClientMsg::ListGames,
        ClientMsg::CreateGame { name: "Friday".into(), size: SystemSize::Large, length_days: 7.0, speed: 1.0, max_players: 4, bots: vec!["easy".into(), "hard".into()], teams: 0 },
        ClientMsg::JoinGame { code: "K7QX".into() },
        ClientMsg::AddBot { code: "K7QX".into(), difficulty: "normal".into() },
        ClientMsg::SetFaction { code: "K7QX".into(), faction: "hive".into() },
        ClientMsg::StartGame { code: "K7QX".into() },
        ClientMsg::Open { code: "K7QX".into() },
        ClientMsg::Cmd { code: "K7QX".into(), cmd: Command::Build { body: BodyId(3), class: ShipClass::Bomber } },
        ClientMsg::Cmd { code: "K7QX".into(), cmd: Command::Chat { text: "line1\nline2 🚀".into() } },
        ClientMsg::Events { since: 1.7e9 },
        ClientMsg::Ping,
    ]
}

#[test]
fn every_client_message_is_one_line_and_round_trips() {
    for m in client_msgs() {
        let line = encode(&m);
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1, "embedded newline breaks line framing: {line:?}");
        let back: ClientMsg = decode(&line).unwrap();
        assert_eq!(encode(&back), line);
        // CRLF from a sloppy client is fine too
        let crlf = line.trim_end().to_string() + "\r\n";
        assert!(decode::<ClientMsg>(&crlf).is_ok(), "{crlf:?}");
    }
}

#[test]
fn server_messages_round_trip() {
    let g = running_game();
    let events: Vec<_> = g.events.iter().cloned().map(|e| ("WXYZ".to_string(), 1.7e9 + e.t, e)).collect();
    let msgs = vec![
        ServerMsg::Welcome { user: "jonas".into(), token: None, latest_client: None, update_url: None },
        ServerMsg::Error { msg: "bad password".into() },
        ServerMsg::Games(vec![GameSummary {
            code: "WXYZ".into(), name: "wire".into(), status: Status::Running, t: g.t, length: 7.0 * 86_400.0,
            players: vec!["ann".into(), "bob".into()], max_players: 4, me: 1, last_event: 9,
        }]),
        ServerMsg::Events(events),
        ServerMsg::CmdResult { ok: false, msg: "not your ship".into() },
        ServerMsg::Pong,
    ];
    for m in msgs {
        let line = encode(&m);
        assert_eq!(line.matches('\n').count(), 1);
        let back: ServerMsg = decode(&line).unwrap();
        assert_eq!(encode(&back), line);
    }
}

#[test]
fn state_message_lets_the_client_predict_exactly_what_the_server_will_do() {
    let mut server = running_game();
    let mine: Vec<_> = server.ships.iter().filter(|s| s.owner == 0).map(|s| s.id).take(2).collect();
    let to = server.players[1].home.unwrap();
    server.apply(0, Command::Transfer { ships: mine, to, depart_t: None }).unwrap();
    let line = encode(&ServerMsg::State { game: Box::new(server.clone()), me: 0, real_time: proto::unix_now() });
    let ServerMsg::State { game, me, .. } = decode::<ServerMsg>(&line).unwrap() else { panic!() };
    assert_eq!(me, 0);
    let mut client = *game;
    let t = server.t + 12.0 * 3600.0;
    server.advance_to(t);
    client.advance_to(t);
    assert_eq!(serde_json::to_string(&server).unwrap(), serde_json::to_string(&client).unwrap());
}

#[test]
fn garbage_is_an_error_not_a_panic() {
    for line in ["", "{", "null", "\"Ping\"x", "{\"Hello\":{}}", "{\"Nope\":1}", "\u{0}"] {
        assert!(decode::<ClientMsg>(line).is_err(), "{line:?} decoded");
    }
    assert!(decode::<ClientMsg>("\"Ping\"").is_ok());
}

#[test]
fn state_message_size_is_reasonable_for_a_phone() {
    let g = running_game();
    let n = encode(&ServerMsg::State { game: Box::new(g), me: 0, real_time: 0.0 }).len();
    println!("State message: {n} bytes");
    assert!(n < 512 * 1024, "State is {n} bytes");
}

/// Design: planet contents are hidden without telescopes and small ships are hard to see. The
/// server sends each player `Game::view_for(me)` (see `server::send_state`).
#[test]
fn state_hides_enemy_ships_outside_sensor_range() {
    let g = running_game();
    let line = encode(&ServerMsg::State { game: Box::new(g.view_for(0)), me: 0, real_time: 0.0 });
    let ServerMsg::State { game, .. } = decode::<ServerMsg>(&line).unwrap() else { panic!() };
    // (homes can be neighbours at conjunction — seed 77 has two 64 000 km apart — so only count
    // scouts whose home is well outside player 0's home telescope)
    let h0 = g.system.pos(g.players[0].home.unwrap(), g.t);
    let far = |b: Option<BodyId>| b.is_some_and(|b| g.system.pos(b, g.t).dist(h0) > 400_000.0);
    let enemy_far = game.ships.iter().filter(|s| s.owner != 0 && s.class == ShipClass::Scout && far(s.parked_at())).count();
    assert!(g.ships.iter().filter(|s| s.owner != 0 && s.class == ShipClass::Scout && far(s.parked_at())).count() > 0, "test needs far homes");
    assert_eq!(enemy_far, 0, "client can see {enemy_far} enemy scouts parked at their far-away homes");
}

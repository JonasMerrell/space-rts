//! Regression check: a server restart mid-game must not drop the player to the menu, and re-opening
//! the same game from the menu must land on the game screen again.
use space_rts::backend::ClientBackend;
use std::time::{Duration, Instant};
use ui::{Backend, BackendEvent, UiAction};

fn pump(be: &mut ClientBackend, secs: f64) -> Vec<BackendEvent> {
    let t0 = Instant::now();
    let mut all = vec![];
    while t0.elapsed().as_secs_f64() < secs {
        all.extend(be.poll());
        std::thread::sleep(Duration::from_millis(50));
    }
    all
}

fn main() {
    let server = std::env::args().nth(1).unwrap_or("127.0.0.1:7777".into());
    let restart = std::env::args().nth(2).unwrap_or_default();
    let mut be = ClientBackend::new(server.clone());
    be.act(UiAction::Login { server, user: "zz-reconnect".into(), pass: "pw-reconnect".into(), register: true, invite: String::new() });
    pump(&mut be, 1.5);
    be.act(UiAction::CreateGame { name: "reconnect test".into(), length_days: 1, bots: vec!["easy".into()], speed: 1.0, max_players: 4, teams: 0 });
    pump(&mut be, 1.5);
    be.act(UiAction::StartGame);
    let ev = pump(&mut be, 1.5);
    assert!(ev.iter().any(|e| matches!(e, BackendEvent::GameStarted)), "no GameStarted");
    let code = be.my_games().last().map(|g| g.code.clone());
    std::process::Command::new("sh").arg("-c").arg(&restart).status().unwrap();
    let ev = pump(&mut be, 12.0);
    let relogged = ev.iter().any(|e| matches!(e, BackendEvent::LoggedIn { .. }));
    println!("after restart: LoggedIn event = {relogged}, view = {}", be.view().is_some());
    assert!(!relogged && be.view().is_some(), "reconnect dropped the player out of the game");
    be.act(UiAction::BackToMenu);
    pump(&mut be, 1.0);
    be.act(UiAction::OpenGame { code: code.expect("game code") });
    let ev = pump(&mut be, 2.0);
    let started = ev.iter().any(|e| matches!(e, BackendEvent::GameStarted));
    println!("re-open same game: GameStarted = {started}");
    assert!(started);
    println!("OK");
}

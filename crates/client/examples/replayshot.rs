//! Screenshots of the rewind and a battle replay on the real client: in-process server with a war
//! where our 3 interceptors meet Mara's 3 at a moon, run sped up until the fight is over, then the
//! map rewound to it, the battle list, and the replay.
//!   SHOT=dir [SPEED=200] [RUN=60] [UI_W=540 UI_H=1170] cargo run -p client --example replayshot
//! Writes dir/{rewind,list,replay}.png.
use macroquad::prelude::*;
use sim::game::{Game, Settings, ShipClass};
use sim::system::{BodyKind, SystemSize};
use space_rts::backend::ClientBackend;
use std::time::{Duration, Instant};
use ui::game::GameScreen;
use ui::{Backend, BackendEvent, Screen, Ui, UiAction};

fn conf() -> Conf {
    let w = std::env::var("UI_W").ok().and_then(|v| v.parse().ok()).unwrap_or(540);
    let h = std::env::var("UI_H").ok().and_then(|v| v.parse().ok()).unwrap_or(1170);
    Conf { window_title: "replayshot".into(), window_width: w, window_height: h, high_dpi: true, ..Default::default() }
}

fn pump(be: &mut ClientBackend, secs: f64, until: impl Fn(&ClientBackend, &[BackendEvent]) -> bool) -> Vec<BackendEvent> {
    let t0 = Instant::now();
    let mut all = vec![];
    while t0.elapsed().as_secs_f64() < secs {
        all.extend(be.poll());
        if until(be, &all) {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    all
}

async fn frames(ui: &mut Ui, be: &mut ClientBackend, n: usize, shot: Option<String>) {
    for frame in 0..n {
        ui.frame(be);
        if frame + 1 == n {
            if let Some(p) = &shot {
                get_screen_data().export_png(p);
            }
        }
        next_frame().await;
    }
}

#[macroquad::main(conf)]
async fn main() {
    let dir = std::env::var("SHOT").unwrap_or(".".into());
    let speed: f64 = std::env::var("SPEED").ok().and_then(|s| s.parse().ok()).unwrap_or(200.0);
    let run: f64 = std::env::var("RUN").ok().and_then(|s| s.parse().ok()).unwrap_or(60.0);
    let data = std::env::temp_dir().join(format!("replayshot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(data.join("games")).unwrap();
    // the war, saved where the server will load it
    let settings = Settings { name: "Replay test".into(), size: SystemSize::Medium, max_players: 2, length: 14.0 * 86_400.0, speed, teams: 0 };
    let mut g = Game::new("RPLAY".into(), settings, 7, "Jonas");
    g.add_player("Mara", Some("normal".into())).unwrap();
    g.start();
    let moon = g.system.bodies.iter().find(|b| b.kind == BodyKind::Moon && g.bodies[b.id.0 as usize].owner.is_none()).unwrap().id;
    for _ in 0..3 {
        g.spawn(0, ShipClass::Interceptor, moon);
        g.spawn(1, ShipClass::Interceptor, moon);
    }
    g.spawn(0, ShipClass::Gunship, moon);
    g.started_real = proto::unix_now();
    std::fs::write(data.join("games").join("RPLAY.json"), serde_json::to_string(&g).unwrap()).unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let data2 = data.clone();
    std::thread::spawn(move || server::serve(listener, data));

    ui::art::load("assets").await;
    ui::art::set_muted(true);
    let mut ui = Ui::new();
    let mut be = ClientBackend::new(addr);
    be.act(UiAction::Login { server: String::new(), user: "Jonas".into(), pass: "pw".into(), register: true, invite: String::new() });
    pump(&mut be, 5.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::LoggedIn { .. })));
    be.act(UiAction::OpenGame { code: "RPLAY".into() });
    pump(&mut be, 10.0, |b, _| b.view().is_some());
    pump(&mut be, run, |_, _| false);
    eprintln!("game time {:.1} h", be.view().unwrap().now / 3600.0);

    ui.user = "Jonas".into();
    let mut g = GameScreen::new();
    ui::game::rewind::enter(&mut g);
    ui.screen = Screen::Game(g);
    frames(&mut ui, &mut be, 10, None).await;
    let ev = pump(&mut be, 10.0, |b, _| b.history().is_some_and(|h| !h.loading && h.start.is_some()));
    eprintln!("events {ev:?}");
    let hist = std::fs::read_to_string(data2.join("history").join("RPLAY.jsonl")).unwrap_or_default();
    eprintln!("history file: {} lines, {} bytes", hist.lines().count(), hist.len());
    let h = be.history().unwrap();
    eprintln!("history {:?}..{:?}, battles {:?}", h.start, h.end, h.battles.iter().map(|b| (b.id, &b.place, b.start_t, b.end_t, &b.lost)).collect::<Vec<_>>());
    let b = h.battles.first().cloned().expect("a recorded battle");
    let now = be.view().unwrap().now;
    if let Screen::Game(g) = &mut ui.screen {
        g.rewind.as_mut().unwrap().back = now - (b.start_t + b.end_t) / 2.0;
    }
    frames(&mut ui, &mut be, 30, Some(format!("{dir}/rewind.png"))).await;
    if let Screen::Game(g) = &mut ui.screen {
        g.rewind.as_mut().unwrap().list_open = true;
    }
    frames(&mut ui, &mut be, 20, Some(format!("{dir}/list.png"))).await;
    if let Screen::Game(g) = &mut ui.screen {
        g.rewind.as_mut().unwrap().list_open = false;
        ui::game::replay::open(g, b.id);
        let r = g.replay.as_mut().unwrap();
        r.t = Some(b.start_t + (b.end_t - b.start_t) * 0.3);
        r.playing = false;
    }
    frames(&mut ui, &mut be, 10, None).await;
    pump(&mut be, 10.0, |b2, _| b2.replay(b.id, b.start_t).is_some());
    let rv = be.replay(b.id, b.start_t + (b.end_t - b.start_t) * 0.3).expect("replay loaded");
    eprintln!("replay: {} units, {} salvos, {} hits", rv.units.len(), rv.salvos.len(), rv.hits.len());
    frames(&mut ui, &mut be, 30, Some(format!("{dir}/replay.png"))).await;
}

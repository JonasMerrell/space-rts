//! Screenshots of the sensors report on the real client: in-process server, real ClientBackend,
//! a sped-up game against bots run for a while, one of our ships sent off, then the report opened.
//!   SHOT=dir [SPEED=1000] [RUN=90] [UI_W=540 UI_H=1170] cargo run -p client --example sensorshot
//! Writes dir/{map,now,contacts,log}.png.
use macroquad::prelude::*;
use space_rts::backend::ClientBackend;
use std::time::{Duration, Instant};
use ui::game::GameScreen;
use ui::model::{BodyKind, ReportTab};
use ui::{Backend, BackendEvent, Screen, Ui, UiAction};

fn conf() -> Conf {
    let w = std::env::var("UI_W").ok().and_then(|v| v.parse().ok()).unwrap_or(540);
    let h = std::env::var("UI_H").ok().and_then(|v| v.parse().ok()).unwrap_or(1170);
    Conf { window_title: "sensorshot".into(), window_width: w, window_height: h, high_dpi: true, ..Default::default() }
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

#[macroquad::main(conf)]
async fn main() {
    let dir = std::env::var("SHOT").unwrap_or(".".into());
    let speed: f64 = std::env::var("SPEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1000.0);
    let run: f64 = std::env::var("RUN").ok().and_then(|s| s.parse().ok()).unwrap_or(90.0);
    let data = std::env::temp_dir().join(format!("sensorshot-{}", std::process::id()));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || server::serve(listener, data));

    ui::art::load("assets").await;
    ui::art::set_muted(true);
    let mut ui = Ui::new();
    let mut be = ClientBackend::new(addr);
    be.act(UiAction::Login { server: String::new(), user: "Jonas".into(), pass: "pw".into(), register: true, invite: String::new() });
    pump(&mut be, 5.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::LoggedIn { .. })));
    be.act(UiAction::CreateGame { name: "Sensors test".into(), length_days: 7, speed, bots: vec!["hard".into(), "hard".into(), "normal".into()] });
    pump(&mut be, 5.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::Lobby(_))));
    be.act(UiAction::StartGame);
    pump(&mut be, 10.0, |b, _| b.view().is_some_and(|v| v.lobby.is_none() && v.bodies.iter().any(|b| b.owner == Some(v.me))));
    let v = be.view().expect("game view").clone();
    let home = v.bodies.iter().find(|b| b.owner == Some(v.me) && b.kind == BodyKind::Planet).expect("home").id;
    pump(&mut be, run * 0.3, |_, _| false);
    // send one of our ships to the nearest other planet so something is under way
    let v = be.view().unwrap().clone();
    let now = v.now;
    let target = {
        let h = be.body_pos(home, now);
        v.bodies
            .iter()
            .filter(|b| b.kind == BodyKind::Planet && b.id != home)
            .min_by(|a, b| {
                let d = |id| {
                    let p = be.body_pos(id, now);
                    (p[0] - h[0]).hypot(p[1] - h[1])
                };
                d(a.id).total_cmp(&d(b.id))
            })
            .unwrap()
            .id
    };
    if let Some(s) = v.ships.iter().find(|s| s.owner == v.me && s.parked_at == Some(home)) {
        if let Some(p) = be.plan_transfer(s.id, target, now) {
            be.act(UiAction::ScheduleTransfer(p));
        }
    }
    pump(&mut be, run * 0.7, |_, _| false);
    let rep = be.sensors().cloned().unwrap_or_default();
    eprintln!("game time {:.1} h, {} items, {} unread, {} threats", be.view().unwrap().now / 3600.0, rep.items.len(), rep.unread(), rep.threats());
    for it in &rep.items {
        eprintln!("  {:?} {:?} {} | {}", it.section, it.when, it.title, it.text);
    }

    ui.user = "Jonas".into();
    for (name, tab) in [("map", None), ("now", Some(ReportTab::Now)), ("contacts", Some(ReportTab::Contacts)), ("log", Some(ReportTab::Log))] {
        let mut g = GameScreen::new();
        if let Some(tab) = tab {
            g.sensors_open = true;
            g.report.show(tab);
        }
        ui.screen = Screen::Game(g);
        for frame in 0..30 {
            ui.frame(&mut be);
            if frame == 29 {
                get_screen_data().export_png(&format!("{dir}/{name}.png"));
            }
            next_frame().await;
        }
    }
}

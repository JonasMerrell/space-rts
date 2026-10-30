//! Screenshot of the LOG with fleet events grouped: in-process server, real ClientBackend, our
//! starting ships formed into one fleet and sent to the nearest planet, run until it arrives.
//!   SHOT=dir [SPEED=3000] [RUN=120] cargo run --release -p client --example fleetlogshot
//! Writes dir/fleetlog.png.
use macroquad::prelude::*;
use space_rts::backend::ClientBackend;
use std::time::{Duration, Instant};
use ui::game::GameScreen;
use ui::model::{BodyKind, ReportTab};
use ui::{Backend, BackendEvent, Screen, Ui, UiAction};

fn conf() -> Conf {
    let w = std::env::var("UI_W").ok().and_then(|v| v.parse().ok()).unwrap_or(540);
    let h = std::env::var("UI_H").ok().and_then(|v| v.parse().ok()).unwrap_or(1170);
    Conf { window_title: "fleetlogshot".into(), window_width: w, window_height: h, high_dpi: true, ..Default::default() }
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
    let speed: f64 = std::env::var("SPEED").ok().and_then(|s| s.parse().ok()).unwrap_or(3000.0);
    let run: f64 = std::env::var("RUN").ok().and_then(|s| s.parse().ok()).unwrap_or(120.0);
    let data = std::env::temp_dir().join(format!("fleetlogshot-{}", std::process::id()));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || server::serve(listener, data));

    ui::art::load("assets").await;
    ui::art::set_muted(true);
    let mut ui = Ui::new();
    let mut be = ClientBackend::new(addr);
    be.act(UiAction::Login { server: String::new(), user: "Jonas".into(), pass: "pw".into(), register: true, invite: String::new() });
    pump(&mut be, 5.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::LoggedIn { .. })));
    be.act(UiAction::CreateGame { name: "Fleet log".into(), length_days: 7, speed, bots: vec!["easy".into()], max_players: 4, teams: 0 });
    pump(&mut be, 5.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::Lobby(_))));
    be.act(UiAction::StartGame);
    pump(&mut be, 10.0, |b, _| b.view().is_some_and(|v| v.lobby.is_none() && v.bodies.iter().any(|b| b.owner == Some(v.me))));
    let v = be.view().expect("game view").clone();
    let home = v.bodies.iter().find(|b| b.owner == Some(v.me) && b.kind == BodyKind::Planet).expect("home").id;
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
    let mine: Vec<_> = v.ships.iter().filter(|s| s.owner == v.me && s.parked_at == Some(home)).map(|s| s.id).collect();
    be.act(UiAction::FormFleet { ships: mine.clone(), name: "Alpha".into() });
    pump(&mut be, 3.0, |_, _| false);
    let now = be.view().unwrap().now;
    let p = be.plan_transfer(mine[0], target, now).expect("plan");
    be.act(UiAction::ScheduleTransfer(p));
    pump(&mut be, run, |b, _| b.sensors().is_some_and(|r| r.items.iter().any(|i| i.title == "Arrived")));
    let rep = be.sensors().cloned().unwrap_or_default();
    for it in &rep.items {
        eprintln!("  {:?} {:?} {} | {}", it.section, it.when, it.title, it.text);
    }

    ui.user = "Jonas".into();
    for name in ["fleetlog"] {
        let mut g = GameScreen::new();
        g.sensors_open = true;
        g.report.show(ReportTab::Log);
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

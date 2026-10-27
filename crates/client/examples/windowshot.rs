//! Transfer-window tool on the real client: in-process server, real ClientBackend, open WINDOWS
//! for a parked ship (screenshot per sort), tap the top row and check it plans that course.
//!   OUT=dir [UI_W=540 UI_H=1170] cargo run -p client --example windowshot
use macroquad::prelude::*;
use space_rts::backend::ClientBackend;
use std::time::{Duration, Instant};
use ui::game::windows::{Sort, WindowsSheet};
use ui::game::{GameScreen, Mode};
use ui::model::{BodyKind, Focus};
use ui::{Backend, BackendEvent, Screen, Ui, UiAction};

fn conf() -> Conf {
    let dim = |k: &str, d: i32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    Conf { window_title: "windowshot".into(), window_width: dim("UI_W", 540), window_height: dim("UI_H", 1170), high_dpi: true, ..Default::default() }
}

fn pump(be: &mut ClientBackend, secs: f64, until: impl Fn(&ClientBackend, &[BackendEvent]) -> bool) {
    let t0 = Instant::now();
    let mut all = vec![];
    while t0.elapsed().as_secs_f64() < secs {
        all.extend(be.poll());
        if until(be, &all) {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[macroquad::main(conf)]
async fn main() {
    let out = std::env::var("OUT").unwrap_or(".".into());
    let data = std::env::temp_dir().join(format!("windowshot-{}", std::process::id()));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || server::serve(listener, data));

    ui::art::load("assets").await;
    ui::art::set_muted(true);
    let mut ui = Ui::new();
    let mut be = ClientBackend::new(addr);
    be.act(UiAction::Login { server: String::new(), user: "Jonas".into(), pass: "pw".into(), register: true, invite: String::new() });
    pump(&mut be, 5.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::LoggedIn { .. })));
    // build a ship at full speed, then slow time down again for planning
    be.act(UiAction::CreateGame { name: "Windows test".into(), length_days: 7, speed: 168.0, bots: vec!["normal".into()] });
    pump(&mut be, 5.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::Lobby(_))));
    be.act(UiAction::StartGame);
    pump(&mut be, 10.0, |b, _| b.view().is_some_and(|v| v.lobby.is_none() && v.bodies.iter().any(|x| x.owner == Some(v.me))));
    let v = be.view().expect("game view").clone();
    let home = v.bodies.iter().find(|b| b.owner == Some(v.me) && b.kind == BodyKind::Planet).expect("home").id;
    be.act(UiAction::Build { body: home, class: ui::model::ShipClass::Interceptor });
    let me = v.me;
    pump(&mut be, 120.0, |b, _| b.view().is_some_and(|v| v.ships.iter().any(|s| s.owner == me && s.parked_at.is_some())));
    be.act(UiAction::SetSpeed { speed: 1.0 });
    pump(&mut be, 2.0, |_, _| false);
    let v = be.view().expect("game view").clone();
    let now = v.now;
    let ship = v.ships.iter().find(|s| s.owner == v.me && s.parked_at.is_some() && s.members.is_empty()).expect("a starting ship");
    let home = ship.parked_at.unwrap();
    let ship = ship.id;
    eprintln!("ship {ship} at {home}");

    let mut g = GameScreen::new();
    g.selected = Some(Focus::Ship(ship));
    g.mode = Mode::Windows(ui::game::windows::WindowsSheet::new(ship));
    ui.user = "Jonas".into();
    ui.screen = Screen::Game(g);
    let t0 = Instant::now();
    while be.windows_async(ship).is_pending() && t0.elapsed().as_secs() < 60 {
        be.poll();
        ui.frame(&mut be);
        next_frame().await;
    }
    eprintln!("windows in {:?}", t0.elapsed());
    let wv = match be.windows_async(ship) {
        ui::WindowsPoll::Done(Some(v)) => v,
        _ => panic!("no windows"),
    };
    for o in &wv.list {
        eprintln!("  {:>3} via {:?}: {} windows, fastest {:?}", o.target, o.via, o.windows.len(), o.fastest.map(|f| ((f.arrive_t - now) / 3600.0, f.dv)));
    }
    for (name, sort) in [("soonest", Sort::Soonest), ("quickest", Sort::Quickest), ("cheapest", Sort::Cheapest)] {
        if let Screen::Game(g) = &mut ui.screen {
            g.mode = Mode::Windows(WindowsSheet { ship, sort, scroll: 0.0 });
        }
        for i in 0..8 {
            be.poll();
            ui.frame(&mut be);
            if i == 7 {
                let f = format!("{out}/windows_{name}.png");
                get_screen_data().export_png(&f);
                eprintln!("{f}");
            }
            next_frame().await;
        }
    }
    // tap the first row's name: plans that destination
    let tap = std::env::var("TAP").ok().and_then(|v| {
        let mut it = v.split(',').map(|x| x.parse::<f32>().ok());
        Some(vec2(it.next()??, it.next()??))
    });
    if let Some(p) = tap {
        ui.input.inject_tap(p);
        let t0 = Instant::now();
        loop {
            be.poll();
            ui.frame(&mut be);
            let done = matches!(&ui.screen, Screen::Game(GameScreen { mode: Mode::Plan(_), .. }));
            if done || t0.elapsed().as_secs() > 60 {
                break;
            }
            next_frame().await;
        }
        for i in 0..8 {
            be.poll();
            ui.frame(&mut be);
            if i == 7 {
                let f = format!("{out}/windows_plan.png");
                get_screen_data().export_png(&f);
                eprintln!("{f}");
            }
            next_frame().await;
        }
        match &ui.screen {
            Screen::Game(GameScreen { mode: Mode::Plan(p), .. }) => eprintln!("planned to {} departing +{:.1}h, {} options", p.target, (p.depart_t - now) / 3600.0, p.options.len()),
            _ => panic!("tap didn't plan a course"),
        }
    }
    eprintln!("OK");
}

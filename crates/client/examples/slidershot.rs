//! Fuel ↔ time slider on the real client: in-process server, real ClientBackend, plan a course
//! for a starting ship, drag the slider (screenshot mid-drag), let go (screenshot of the exact
//! course), engage it and check the server flies exactly that course.
//!   OUT=dir [UI_W=540 UI_H=1170] cargo run -p client --example slidershot
use macroquad::prelude::*;
use space_rts::backend::ClientBackend;
use std::time::{Duration, Instant};
use ui::game::{GameScreen, Mode};
use ui::model::{BodyKind, Focus};
use ui::{Backend, BackendEvent, Screen, Ui, UiAction};

fn conf() -> Conf {
    let dim = |k: &str, d: i32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    Conf { window_title: "slidershot".into(), window_width: dim("UI_W", 540), window_height: dim("UI_H", 1170), high_dpi: true, ..Default::default() }
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

fn plan_of(ui: &Ui) -> Option<ui::model::TransferPlan> {
    match &ui.screen {
        Screen::Game(GameScreen { mode: Mode::Plan(p), .. }) => Some(p.clone()),
        _ => None,
    }
}

#[macroquad::main(conf)]
async fn main() {
    let out = std::env::var("OUT").unwrap_or(".".into());
    let data = std::env::temp_dir().join(format!("slidershot-{}", std::process::id()));
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
    be.act(UiAction::CreateGame { name: "Slider test".into(), length_days: 7, speed: 168.0, bots: vec!["normal".into()], max_players: 4, teams: 0 });
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
    let target = {
        let h = be.body_pos(home, now);
        let d = |id| {
            let p = be.body_pos(id, now);
            (p[0] - h[0]).hypot(p[1] - h[1])
        };
        let mut planets: Vec<_> = v.bodies.iter().filter(|b| b.kind == BodyKind::Planet && b.id != home).collect();
        planets.sort_by(|a, b| d(a.id).total_cmp(&d(b.id)));
        planets[std::env::var("NTH").ok().and_then(|n| n.parse().ok()).unwrap_or(0)].id
    };
    let ship = ship.id;
    eprintln!("ship {ship} {home} -> {target}");

    let mut g = GameScreen::new();
    g.selected = Some(Focus::Ship(ship));
    g.mode = Mode::Planning { ship, target, after: now };
    ui.user = "Jonas".into();
    ui.screen = Screen::Game(g);
    let shot = |name: &str| {
        let f = format!("{out}/{name}.png");
        get_screen_data().export_png(&f);
        eprintln!("{f}");
    };
    // wait for the ladder
    let t0 = Instant::now();
    while plan_of(&ui).is_none() && t0.elapsed().as_secs() < 60 {
        be.poll();
        ui.frame(&mut be);
        next_frame().await;
    }
    for i in 0..10 {
        be.poll();
        ui.frame(&mut be);
        if i == 9 {
            shot("slider_0_cheapest");
        }
        next_frame().await;
    }
    let p0 = plan_of(&ui).expect("plan");
    for o in &p0.options {
        eprintln!("  option {:>8} {:.2} km/s depart +{:.1}h flight {:.1}h", o.label, o.dv_depart + o.dv_mid + o.dv_capture, (o.depart_t - now) / 3600.0, (o.arrive_t - o.depart_t) / 3600.0);
    }
    assert!(p0.options.len() >= 2, "no slider");
    let Screen::Game(g) = &ui.screen else { panic!() };
    let tr = g.plan_track.expect("slider drawn");
    let y = tr.y + tr.h / 2.0;
    let frac: f32 = std::env::var("FRAC").ok().and_then(|f| f.parse().ok()).unwrap_or(0.55);
    // drag from the knob, take a shot while the finger is still down
    let (a, b) = (vec2(tr.x + 2.0, y), vec2(tr.x + tr.w * frac, y));
    ui.input.inject_drag(a, b, 24);
    for i in 0..26 {
        be.poll();
        ui.frame(&mut be);
        if i == 24 {
            shot("slider_1_dragging");
            let p = plan_of(&ui).unwrap();
            eprintln!("dragging: {:.2} km/s flight {:.1}h token {:?}", p.dv_depart + p.dv_mid + p.dv_capture, (p.arrive_t - p.depart_t) / 3600.0, p.token);
        }
        next_frame().await;
    }
    let t0 = Instant::now();
    while plan_of(&ui).is_some_and(|p| p.token.is_empty()) && t0.elapsed().as_secs() < 30 {
        be.poll();
        ui.frame(&mut be);
        next_frame().await;
    }
    for i in 0..5 {
        be.poll();
        ui.frame(&mut be);
        if i == 4 {
            shot("slider_2_exact");
        }
        next_frame().await;
    }
    let p = plan_of(&ui).expect("exact plan");
    eprintln!("exact: {:.2} km/s depart +{:.1}h flight {:.1}h token {}", p.dv_depart + p.dv_mid + p.dv_capture, (p.depart_t - now) / 3600.0, (p.arrive_t - p.depart_t) / 3600.0, p.token);
    assert!(!p.token.is_empty());
    be.act(UiAction::ScheduleTransfer(p.clone()));
    pump(&mut be, 5.0, |b, _| b.view().and_then(|v| v.ship(ship)).is_some_and(|s| s.trajectory.is_some()));
    let s = be.view().unwrap().ship(ship).unwrap().clone();
    let end = s.trajectory.as_ref().and_then(|t| t.end()).expect("server scheduled the course");
    eprintln!("server course ends {:.1}s after the slider's (arrive {:.0})", end - p.arrive_t, p.arrive_t);
    assert!((end - p.arrive_t).abs() < 1.0, "server flies a different course");
    eprintln!("OK");
}

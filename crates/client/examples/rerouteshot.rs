//! New course for a ship in flight on the real client: in-process server, real ClientBackend.
//! Send a ship to the nearest planet, and once it's flying plan a new course to another planet
//! (screenshot), engage it and check the server flies exactly that course.
//!   OUT=dir [UI_W=540 UI_H=1170] cargo run -p client --example rerouteshot
use macroquad::prelude::*;
use space_rts::backend::ClientBackend;
use std::time::{Duration, Instant};
use ui::game::{GameScreen, Mode};
use ui::model::{BodyKind, Focus};
use ui::{Backend, BackendEvent, Screen, Ui, UiAction};

fn conf() -> Conf {
    let dim = |k: &str, d: i32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    Conf { window_title: "rerouteshot".into(), window_width: dim("UI_W", 540), window_height: dim("UI_H", 1170), high_dpi: true, ..Default::default() }
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
    let data = std::env::temp_dir().join(format!("rerouteshot-{}", std::process::id()));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || server::serve(listener, data));

    ui::art::load("assets").await;
    ui::art::set_muted(true);
    let mut ui = Ui::new();
    let mut be = ClientBackend::new(addr);
    be.act(UiAction::Login { server: String::new(), user: "Jonas".into(), pass: "pw".into(), register: true, invite: String::new() });
    pump(&mut be, 5.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::LoggedIn { .. })));
    be.act(UiAction::CreateGame { name: "Reroute test".into(), length_days: 7, speed: 168.0, bots: vec![] });
    pump(&mut be, 5.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::Lobby(_))));
    be.act(UiAction::StartGame);
    pump(&mut be, 10.0, |b, _| b.view().is_some_and(|v| v.lobby.is_none() && v.bodies.iter().any(|x| x.owner == Some(v.me))));
    let v = be.view().expect("game view").clone();
    let now = v.now;
    let ship = v.ships.iter().find(|s| s.owner == v.me && s.parked_at.is_some() && s.members.is_empty()).expect("a starting ship");
    let home = ship.parked_at.unwrap();
    let ship = ship.id;
    let h = be.body_pos(home, now);
    let d = |id| {
        let p = be.body_pos(id, now);
        (p[0] - h[0]).hypot(p[1] - h[1])
    };
    let mut planets: Vec<_> = v.bodies.iter().filter(|b| b.kind == BodyKind::Planet && b.id != home).map(|b| b.id).collect();
    planets.sort_by(|a, b| d(*a).total_cmp(&d(*b)));
    let first = be.plan_transfer(ship, planets[0], now).expect("first course");
    be.act(UiAction::ScheduleTransfer(first.clone()));
    eprintln!("ship {ship} {home} -> {} departs in {:.1} h", planets[0], (first.depart_t - now) / 3600.0);
    pump(&mut be, 200.0, |b, _| b.view().is_some_and(|v| v.ship(ship).is_some_and(|s| s.trajectory.is_some() && s.parked_at.is_none())));
    be.act(UiAction::SetSpeed { speed: 1.0 });
    pump(&mut be, 2.0, |_, _| false);
    let now = be.view().unwrap().now;
    let target = planets[std::env::var("NTH").ok().and_then(|n| n.parse().ok()).unwrap_or(1)];
    eprintln!("in flight at {now:.0}: new course to {target}");

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
            shot("reroute_0_plan");
        }
        next_frame().await;
    }
    let p = plan_of(&ui).expect("new course (none found?)");
    for o in &p.options {
        eprintln!("  option {:>8} {:.2}+{:.2} km/s burn +{:.1}h arrive +{:.1}h", o.label, o.dv_depart, o.dv_capture, (o.depart_t - now) / 3600.0, (o.arrive_t - now) / 3600.0);
    }
    be.act(UiAction::ScheduleTransfer(p.clone()));
    pump(&mut be, 5.0, |b, _| b.view().and_then(|v| v.ship(ship)).and_then(|s| s.trajectory.as_ref()).and_then(|t| t.end()).is_some_and(|e| (e - p.arrive_t).abs() < 1.0));
    for i in 0..10 {
        be.poll();
        ui.frame(&mut be);
        if i == 9 {
            shot("reroute_1_engaged");
        }
        next_frame().await;
    }
    let s = be.view().unwrap().ship(ship).unwrap().clone();
    let end = s.trajectory.as_ref().and_then(|t| t.end()).expect("flying");
    eprintln!("server course ends {:.1}s after the planned arrival", end - p.arrive_t);
    assert!((end - p.arrive_t).abs() < 1.0, "server flies a different course");
    eprintln!("OK");
}

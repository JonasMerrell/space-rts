//! Intercept on the real client: in-process server, two real ClientBackends (Jonas + a rival).
//! The rival sends a gunship towards Jonas's home; once Jonas's sensors pick it up, tap our ship,
//! tap the enemy, screenshot the intercept sheet (cheapest, dragged to fastest, fly-through),
//! engage it and check the server's course meets the target.
//!   OUT=dir [UI_W=540 UI_H=1170] cargo run -p client --example interceptshot
use macroquad::prelude::*;
use space_rts::backend::ClientBackend;
use std::time::{Duration, Instant};
use ui::game::{GameScreen, Mode};
use ui::model::{BodyKind, Focus};
use ui::{Backend, BackendEvent, Screen, Ui, UiAction};

fn conf() -> Conf {
    let dim = |k: &str, d: i32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    Conf { window_title: "interceptshot".into(), window_width: dim("UI_W", 540), window_height: dim("UI_H", 1170), high_dpi: true, ..Default::default() }
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
    let data = std::env::temp_dir().join(format!("interceptshot-{}", std::process::id()));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || server::serve(listener, data));

    ui::art::load("assets").await;
    ui::art::set_muted(true);
    let mut ui = Ui::new();
    let mut be = ClientBackend::new(addr.clone());
    let mut rival = ClientBackend::new(addr);
    be.act(UiAction::Login { server: String::new(), user: "Jonas".into(), pass: "pw".into(), register: true, invite: String::new() });
    rival.act(UiAction::Login { server: String::new(), user: "Rival".into(), pass: "pw".into(), register: true, invite: String::new() });
    pump(&mut be, 5.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::LoggedIn { .. })));
    pump(&mut rival, 5.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::LoggedIn { .. })));
    be.act(UiAction::CreateGame { name: "Intercept test".into(), length_days: 7, speed: 168.0, bots: vec![] });
    let mut code = String::new();
    pump(&mut be, 5.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::Lobby(_))));
    if let Some(l) = be.view().and_then(|v| v.lobby.clone()) {
        code = l.code.clone();
    }
    eprintln!("code {code}");
    rival.act(UiAction::JoinGame { code });
    pump(&mut rival, 5.0, |b, _| b.view().is_some());
    pump(&mut be, 2.0, |_, _| false);
    be.act(UiAction::StartGame);
    let both = |be: &mut ClientBackend, rival: &mut ClientBackend, secs: f64, done: &dyn Fn(&ClientBackend, &ClientBackend) -> bool| {
        let t0 = Instant::now();
        while t0.elapsed().as_secs_f64() < secs {
            be.poll();
            rival.poll();
            if done(be, rival) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        false
    };
    both(&mut be, &mut rival, 10.0, &|a, b| [a, b].iter().all(|x| x.view().is_some_and(|v| v.lobby.is_none() && v.bodies.iter().any(|y| y.owner == Some(v.me)))));
    let v = be.view().expect("view").clone();
    let rv = rival.view().expect("rival view").clone();
    let home = v.bodies.iter().find(|b| b.owner == Some(v.me) && b.kind == BodyKind::Planet).expect("home").id;
    let rhome = rv.bodies.iter().find(|b| b.owner == Some(rv.me) && b.kind == BodyKind::Planet).expect("rival home").id;
    be.act(UiAction::Build { body: home, class: ui::model::ShipClass::Bomber });
    rival.act(UiAction::Build { body: rhome, class: ui::model::ShipClass::Gunship });
    let (me, them) = (v.me, rv.me);
    both(&mut be, &mut rival, 120.0, &|a, b| {
        a.view().is_some_and(|v| v.ships.iter().any(|s| s.owner == me && s.parked_at.is_some() && s.class == ui::model::ShipClass::Bomber))
            && b.view().is_some_and(|v| v.ships.iter().any(|s| s.owner == them && s.class == ui::model::ShipClass::Gunship && s.parked_at.is_some()))
    });
    // the rival sends its gunship at us
    let gun = rival.view().unwrap().ships.iter().find(|s| s.owner == them && s.class == ui::model::ShipClass::Gunship).expect("gunship").id;
    let now = rival.view().unwrap().now;
    let rp = rival.plan_transfer(gun, home, now).expect("rival course");
    eprintln!("rival departs in {:.1} h, arrives in {:.1} h", (rp.depart_t - now) / 3600.0, (rp.arrive_t - now) / 3600.0);
    rival.act(UiAction::ScheduleTransfer(rp.clone()));
    // until Jonas can see it flying
    let seen = both(&mut be, &mut rival, 900.0, &|a, _| a.view().is_some_and(|v| v.ship(gun).is_some_and(|s| s.trajectory.is_some() && s.seen_at.is_none())));
    assert!(seen, "never saw the gunship");
    be.act(UiAction::SetSpeed { speed: 1.0 });
    pump(&mut be, 2.0, |_, _| false);
    let v = be.view().expect("game view").clone();
    let ship = v.ships.iter().find(|s| s.owner == me && s.class == ui::model::ShipClass::Bomber).expect("our bomber").id;
    eprintln!("bomber {ship} intercepting gunship {gun}, now {:.0}", v.now);

    let mut g = GameScreen::new();
    g.selected = Some(Focus::Ship(ship));
    g.mode = Mode::Target(ship);
    ui.user = "Jonas".into();
    ui.screen = Screen::Game(g);
    let shot = |name: &str| {
        let f = format!("{out}/{name}.png");
        get_screen_data().export_png(&f);
        eprintln!("{f}");
    };
    // tap the enemy (as the map would): Target mode + tap on an enemy ship
    if let Screen::Game(g) = &mut ui.screen {
        let v = be.view().unwrap().clone();
        g.intercept(&v, ship, gun, true);
    }
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
            shot("intercept_0_cheapest");
        }
        next_frame().await;
    }
    let p0 = plan_of(&ui).expect("intercept plan");
    let now = be.view().unwrap().now;
    for o in &p0.options {
        eprintln!("  option {:>8} {:.2}+{:.2} km/s meet in {:.1}h closing {:.2}", o.label, o.dv_depart, o.dv_capture, (o.arrive_t - now) / 3600.0, o.intercept.as_ref().unwrap().closing);
    }
    assert!(p0.intercept.is_some());
    // drag the slider to the fast end
    if p0.options.len() > 1 {
        let Screen::Game(g) = &ui.screen else { panic!() };
        let tr = g.plan_track.expect("slider drawn");
        let y = tr.y + tr.h / 2.0;
        ui.input.inject_drag(vec2(tr.x + 2.0, y), vec2(tr.x + tr.w * 0.999, y), 24);
        for _ in 0..30 {
            be.poll();
            ui.frame(&mut be);
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
                shot("intercept_1_fastest");
            }
            next_frame().await;
        }
    }
    // fly-through instead of matching speed
    if let Screen::Game(g) = &mut ui.screen {
        let v = be.view().unwrap().clone();
        g.intercept(&v, ship, gun, false);
    }
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
            shot("intercept_2_flythrough");
        }
        next_frame().await;
    }
    // back to matched, engage the cheapest and check the server's course
    if let Screen::Game(g) = &mut ui.screen {
        let v = be.view().unwrap().clone();
        g.intercept(&v, ship, gun, true);
    }
    let t0 = Instant::now();
    while plan_of(&ui).is_none() && t0.elapsed().as_secs() < 60 {
        be.poll();
        ui.frame(&mut be);
        next_frame().await;
    }
    let p = plan_of(&ui).expect("plan");
    be.act(UiAction::ScheduleTransfer(p.clone()));
    let now = be.view().unwrap().now;
    // the departure is a couple of minutes out: let the clock run
    be.act(UiAction::SetSpeed { speed: 24.0 });
    pump(&mut be, 30.0, |b, _| b.view().and_then(|v| v.ship(ship)).is_some_and(|s| s.trajectory.is_some()));
    let s = be.view().unwrap().ship(ship).unwrap().clone();
    let tr = s.trajectory.clone().expect("server scheduled the intercept");
    let pc = tr.sample(p.arrive_t).expect("path through the meeting");
    let pt = be.ship_pos(gun, p.arrive_t).expect("target path");
    let miss = (pc[0] - pt[0]).hypot(pc[1] - pt[1]);
    eprintln!("engaged at {now:.0}: server course passes {miss:.0} km from the target at the meeting time");
    for _ in 0..10 {
        be.poll();
        ui.frame(&mut be);
        next_frame().await;
    }
    shot("intercept_3_engaged");
    assert!(miss < 3000.0, "server course misses by {miss} km");
    eprintln!("OK");
}

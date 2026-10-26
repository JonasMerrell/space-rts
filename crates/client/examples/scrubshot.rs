//! Screenshots of the time scrubber on the real client: in-process server, real ClientBackend,
//! a ship queued at the home shipyard, then the map scrubbed around the time it's built.
//!   SCENE=<before|built|plan|queued|ranges> SHOT=out.png [SPEED=1] cargo run -p client --example scrubshot
use macroquad::prelude::*;
use space_rts::backend::ClientBackend;
use std::time::{Duration, Instant};
use ui::game::{GameScreen, Mode};
use ui::model::{BodyKind, Focus, ShipClass};
use ui::{Backend, BackendEvent, Screen, Ui, UiAction};

fn conf() -> Conf {
    Conf { window_title: "scrubshot".into(), window_width: 540, window_height: 1170, high_dpi: true, ..Default::default() }
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

fn forecast_done(be: &ClientBackend) -> bool {
    be.view().is_some_and(|v| be.forecast_fresh_until() >= v.now + 7.0 * 86_400.0 - 120.0)
}

#[macroquad::main(conf)]
async fn main() {
    let scene = std::env::var("SCENE").unwrap_or("built".into());
    let shot = std::env::var("SHOT").unwrap_or(format!("{scene}.png"));
    let speed: f64 = std::env::var("SPEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let data = std::env::temp_dir().join(format!("scrubshot-{}", std::process::id()));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || server::serve(listener, data));

    ui::art::load("assets").await;
    ui::art::set_muted(true);
    let mut ui = Ui::new();
    let mut be = ClientBackend::new(addr);
    be.act(UiAction::Login { server: String::new(), user: "Jonas".into(), pass: "pw".into(), register: true, invite: String::new() });
    pump(&mut be, 5.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::LoggedIn { .. })));
    be.act(UiAction::CreateGame { name: "Scrub test".into(), length_days: 7, speed, bots: vec!["normal".into()] });
    pump(&mut be, 5.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::Lobby(_))));
    be.act(UiAction::StartGame);
    pump(&mut be, 5.0, |b, _| b.view().is_some());
    let v = be.view().expect("game view").clone();
    let home = v.bodies.iter().find(|b| b.owner == Some(v.me) && b.kind == BodyKind::Planet).expect("home").id;
    be.act(UiAction::Build { body: home, class: ShipClass::Interceptor });
    pump(&mut be, 3.0, |b, _| b.body_info(home).is_some_and(|i| !i.queue.is_empty()));
    pump(&mut be, 0.5, |_, _| false);
    let info = be.body_info(home).unwrap();
    let ready = info.queue.last().expect("queued").1;
    pump(&mut be, 20.0, |b, _| forecast_done(b));
    let now = be.view().unwrap().now;
    eprintln!("now {now:.0} ready {ready:.0} until {:.0}", be.forecast_until());
    if let Some(pv) = be.predicted(ready + 120.0) {
        eprintln!("predicted ships {} live {}", pv.ships.len(), v.ships.len());
    }
    let planned = be.predicted(ready + 120.0).and_then(|v| v.ships.iter().find(|s| s.planned.is_some()).map(|s| s.id)).expect("planned ship in forecast");
    // the nearest other planet as a target
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
    eprintln!("now {now:.0} ready {ready:.0} (+{:.1}h) planned id {planned:#x} target {target}", (ready - now) / 3600.0);

    let mut g = GameScreen::new();
    g.selected = Some(Focus::Ship(planned));
    match scene.as_str() {
        "before" => {
            g.selected = Some(Focus::Body(home));
            g.scrub = (ready - now) * 0.6;
        }
        "built" => {
            g.scrub = ready - now + 900.0;
            g.mode = Mode::Target(planned);
        }
        "plan" => {
            g.scrub = ready - now + 900.0;
            let p = be.plan_transfer(planned, target, now).expect("plan for planned ship");
            g.mode = Mode::Plan(p);
        }
        "queued" => {
            let p = be.plan_transfer(planned, target, now).expect("plan for planned ship");
            let mid = (p.depart_t + p.arrive_t) / 2.0;
            be.act(UiAction::ScheduleTransfer(p));
            pump(&mut be, 3.0, |_, _| false);
            pump(&mut be, 20.0, |b, _| forecast_done(b));
            let now = be.view().unwrap().now;
            g.scrub = mid - now;
            g.scrub_span = 3.0 * 86_400.0;
        }
        "ranges" => {
            g.scrub = ready - now + 900.0;
            g.ranges_open = true;
            g.scrub_span = 3.0 * 86_400.0;
        }
        _ => {}
    }
    g.scrub_span = g.scrub_span.max(ui::game::SCRUB_RANGES.iter().map(|r| r.0).find(|&r| r >= g.scrub).unwrap_or(g.scrub_span));
    ui.user = "Jonas".into();
    ui.screen = Screen::Game(g);
    for frame in 0..40 {
        ui.frame(&mut be);
        if frame == 39 {
            get_screen_data().export_png(&shot);
        }
        next_frame().await;
    }
    if let Screen::Game(g) = &ui.screen {
        eprintln!("scene {scene}: scrub +{:.1}h, mode {:?}", g.scrub / 3600.0, std::mem::discriminant(&g.mode));
    }
}

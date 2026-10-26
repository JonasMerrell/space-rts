//! End-to-end UI flow over the fake backend, in a real macroquad window at phone size.
//! Needs a display: `ci.sh` starts Xvfb. With no `DISPLAY` the test is skipped (exit 0).
//! Screenshots of every screen go to `$UITESTS_SHOTS` (default `target/uitests-shots/`).
use macroquad::prelude::*;
use ui::demo::DemoBackend;
use ui::model::*;
use ui::{Backend, Screen, Ui, UiAction};

fn conf() -> Conf {
    Conf { window_title: "uitests".into(), window_width: 540, window_height: 1170, ..Default::default() }
}

struct T {
    ui: Ui,
    be: DemoBackend,
    shots: std::path::PathBuf,
    passed: usize,
}

impl T {
    async fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.ui.frame(&mut self.be);
            next_frame().await;
        }
    }
    /// Render, save a screenshot and check something other than a flat background was drawn.
    async fn shot(&mut self, name: &str) {
        self.frames(3).await;
        self.ui.frame(&mut self.be);
        let img = get_screen_data();
        let path = self.shots.join(format!("{name}.png"));
        img.export_png(path.to_str().unwrap());
        let px = img.get_image_data();
        let first = px[0];
        let distinct = px.iter().step_by(97).filter(|p| **p != first).count();
        assert!(distinct > 200, "{name}: screen looks blank ({distinct} differing samples)");
        next_frame().await;
    }
    fn ok(&mut self, what: &str) {
        self.passed += 1;
        println!("ok   {what}");
    }
    fn screen(&self) -> &'static str {
        match self.ui.screen {
            Screen::Login(_) => "login",
            Screen::Menu(_) => "menu",
            // the waiting period is the game screen with the lobby overlay
            Screen::Game(_) if self.be.view().is_some_and(|v| v.lobby.is_some()) => "lobby",
            Screen::Game(_) => "game",
        }
    }
}

async fn run() {
    let shots = std::env::var("UITESTS_SHOTS").map(Into::into).unwrap_or_else(|_| {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/uitests-shots")
    });
    std::fs::create_dir_all(&shots).unwrap();
    let mut t = T { ui: Ui::new(), be: DemoBackend::new(), shots, passed: 0 };

    // -- login
    assert_eq!(t.screen(), "login");
    t.shot("01-login").await;
    t.ok("starts on the login screen");
    t.be.act(UiAction::Login { server: String::new(), user: "Jonas".into(), pass: "hunter2".into(), register: false });
    t.frames(2).await;
    assert_eq!(t.screen(), "menu", "LoggedIn should open the menu");
    assert_eq!(t.ui.user, "Jonas");
    t.shot("02-menu").await;
    t.ok("login -> menu");
    assert!(!t.be.my_games().is_empty(), "demo has games to list");
    t.ok("menu lists my games");

    // -- create a 4-player lobby (me + 1 bot, then add bots to the cap)
    t.be.act(UiAction::CreateGame { name: "Test War".into(), length_days: 7, speed: 1.0, bots: vec!["easy".into()] });
    t.frames(2).await;
    assert_eq!(t.screen(), "lobby");
    let players = |t: &T| t.be.view().and_then(|v| v.lobby.as_ref()).map(|i| (i.players.len(), i.max_players, i.code.clone()));
    assert_eq!(players(&t).map(|p| p.0), Some(2));
    {
        let v = t.be.view().unwrap();
        assert!(v.ships.is_empty() && v.now == 0.0, "clock stopped and no fleets while waiting");
        assert!(v.home.is_some(), "we see where we'll start");
    }
    t.ok("waiting period shows the system, paused");
    t.be.act(UiAction::AddBot { difficulty: "hard".into() });
    t.be.act(UiAction::AddBot { difficulty: "normal".into() });
    t.frames(2).await;
    let (n, max, code) = players(&t).unwrap();
    assert_eq!((n, max), (4, 4), "four players to start");
    assert_eq!(code.len(), 4, "join code is short: {code}");
    t.shot("03-lobby-4p").await;
    t.ok("create lobby, add bots up to 4 players");
    t.be.act(UiAction::SetFaction("HIVE BROOD".into()));
    t.frames(2).await;
    {
        let me = t.be.view().unwrap().lobby.as_ref().unwrap().players.iter().find(|p| p.id == 1).unwrap().clone();
        assert_eq!(me.faction, "HIVE BROOD");
    }
    let bot = t.be.view().unwrap().lobby.as_ref().unwrap().players.iter().find(|p| !p.online).unwrap().id;
    t.be.act(UiAction::RemoveBot { player: bot });
    t.frames(2).await;
    assert_eq!(players(&t).map(|p| p.0), Some(3), "host removes a bot");
    t.ok("remove a bot while waiting");
    t.ok("faction choice reaches the lobby view");

    // -- leave and join by code
    t.be.act(UiAction::LeaveLobby);
    t.frames(2).await;
    assert_eq!(t.screen(), "menu");
    t.be.act(UiAction::JoinGame { code: "ABCD".into() });
    t.frames(2).await;
    assert_eq!(t.screen(), "lobby", "joining an unknown code opens its lobby");
    t.ok("leave lobby, join by code");

    // -- start the game
    t.be.act(UiAction::StartGame);
    t.frames(2).await;
    assert_eq!(t.screen(), "game");
    assert!(t.be.view().unwrap().lobby.is_none());
    t.be.speed = 0.0; // freeze game time so the rest is deterministic
    t.frames(1).await;
    let view = t.be.view().expect("game view").clone();
    assert!(view.bodies.iter().any(|b| b.kind == BodyKind::Star));
    assert!(view.players.len() >= 2);
    assert!(view.ends_at > view.now, "game has an end time in the future");
    t.shot("04-map").await;
    t.ok("start game -> map");

    // body positions are continuous and planets actually orbit
    for b in view.bodies.iter().filter(|b| b.parent.is_some()) {
        let p0 = t.be.body_pos(b.id, view.now);
        let p1 = t.be.body_pos(b.id, view.now + 1.0);
        let p2 = t.be.body_pos(b.id, view.now + 3600.0);
        let d = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
        assert!(d(p0, p1) < d(p0, p2) && d(p0, p2) > 0.0, "{} doesn't move sensibly", b.name);
    }
    t.ok("bodies move continuously");

    // -- auto-plan and schedule a transfer for one of my parked ships
    let me = view.me;
    let ship = view.ships.iter().find(|s| s.owner == me && s.parked_at.is_some() && s.trajectory.is_none()).expect("a parked ship of mine").clone();
    let target = view.bodies.iter().find(|b| Some(b.id) != ship.parked_at && b.kind == BodyKind::Planet).unwrap().id;
    let plan = t.be.plan_transfer(ship.id, target, view.now).expect("demo planner finds a transfer");
    assert!(plan.depart_t >= view.now && plan.arrive_t > plan.depart_t, "{plan:?}");
    assert!(plan.dv_depart > 0.0 && plan.fuel_needed > 0.0);
    assert!(plan.path.points.len() >= 2);
    for w in plan.path.points.windows(2) {
        assert!(w[1].0 >= w[0].0, "path times go backwards");
    }
    if let Screen::Game(g) = &mut t.ui.screen {
        g.plan(&t.be, &view, ship.id, target, view.now);
    }
    t.shot("05-plan").await;
    let fuel_before = ship.fuel;
    t.be.act(UiAction::ScheduleTransfer(plan.clone()));
    t.frames(1).await;
    let s = t.be.view().unwrap().ship(ship.id).unwrap().clone();
    assert_eq!(s.destination, Some(target));
    assert!(s.trajectory.is_some());
    assert!((s.fuel - (fuel_before - plan.fuel_needed)).abs() < 1e-3, "fuel {fuel_before} -> {}", s.fuel);
    assert!(t.be.ship_pos(ship.id, (plan.depart_t + plan.arrive_t) / 2.0).is_some(), "ship has a mid-flight position");
    t.shot("06-scheduled").await;
    t.ok("auto-plan + schedule transfer spends fuel");

    // -- manual maneuver nodes preview
    let node = ManeuverNode { t: plan.depart_t + 600.0, prograde: 0.5, radial: 0.0 };
    let pv = t.be.preview_nodes(ship.id, std::slice::from_ref(&node));
    assert!(pv.is_some(), "node preview");
    t.be.act(UiAction::SetNodes { ship: ship.id, nodes: vec![node] });
    t.frames(1).await;
    assert_eq!(t.be.view().unwrap().ship(ship.id).unwrap().nodes.len(), 1);
    t.ok("maneuver node preview + set");

    // -- chat
    let n0 = t.be.view().unwrap().chat.len();
    t.be.act(UiAction::Chat("gg".into()));
    t.frames(1).await;
    let chat = &t.be.view().unwrap().chat;
    assert_eq!(chat.len(), n0 + 1);
    assert_eq!(chat.last().unwrap().text, "gg");
    if let Screen::Game(g) = &mut t.ui.screen {
        g.chat_open = true;
    }
    t.shot("07-chat").await;
    t.ok("chat message appears");

    // -- notices become toasts
    let notice = Notice { kind: NoticeKind::Bad, title: "UNDER ATTACK".into(), body: "test".into(), focus: None };
    t.ui.frame(&mut t.be); // nothing queued
    if let Screen::Game(g) = &mut t.ui.screen {
        g.chat_open = false;
        let before = g.toasts.len();
        g.push_notice(notice);
        assert_eq!(g.toasts.len(), before + 1);
    }
    t.shot("08-toast").await;
    t.ok("notice -> toast");

    // -- back to menu, then log out
    t.be.act(UiAction::BackToMenu);
    t.frames(2).await;
    assert_eq!(t.screen(), "menu", "closing the game returns to the menu");
    t.be.act(UiAction::Logout);
    t.frames(2).await;
    assert_eq!(t.screen(), "login");
    t.ok("back to menu, logout");

    // -- other phone sizes don't panic and still draw
    for (w, h, name) in [(360.0, 800.0, "small"), (1080.0, 2400.0, "large")] {
        request_new_screen_size(w, h);
        t.frames(3).await;
        t.shot(&format!("09-login-{name}")).await;
    }
    t.ok("renders at other phone sizes");

    println!("\nui_flow: {} checks passed, screenshots in {}", t.passed, t.shots.display());
}

fn main() {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        println!("ui_flow: no display, skipped (run via ./ci.sh, which starts Xvfb)");
        return;
    }
    // A panic inside the macroquad loop must fail the test process, not hang the window.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        hook(info);
        std::process::exit(101);
    }));
    macroquad::Window::from_config(conf(), async {
        run().await;
        std::process::exit(0);
    });
}

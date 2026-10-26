//! Offline UI demo on the fake backend.
//!   cargo run -p ui --example demo
//! Screenshot mode: UI_SCENE=<login|menu|create|lobby|map|body|target|plan|nodes|scrub|chat|toast> UI_SHOT=out.png
//! sets the scene up, renders a few frames, saves the PNG and exits.

use macroquad::prelude::*;
use ui::demo::DemoBackend;
use ui::game::{GameScreen, Mode, NodeEdit};
use ui::model::*;
use ui::screens::{Lobby, Menu};
use ui::{Backend, Screen, Ui, UiAction};

fn conf() -> Conf {
    let w = std::env::var("UI_W").ok().and_then(|v| v.parse().ok()).unwrap_or(540);
    let h = std::env::var("UI_H").ok().and_then(|v| v.parse().ok()).unwrap_or(1170);
    Conf { window_title: "space-rts ui demo".into(), window_width: w, window_height: h, high_dpi: true, ..Default::default() }
}

fn setup(scene: &str, ui: &mut Ui, be: &mut DemoBackend) {
    ui.user = "Jonas".into();
    let game = |ui: &mut Ui, be: &mut DemoBackend| {
        be.act(UiAction::Login { user: "Jonas".into(), pass: "x".into(), register: false });
        be.start_game();
        be.speed = 0.0;
        let _ = be.poll();
        ui.screen = Screen::Game(GameScreen::new());
    };
    match scene {
        "login" => {
            if let Screen::Login(l) = &mut ui.screen {
                l.user.text = "Jonas".into();
                l.pass.text = "hunter2".into();
            }
        }
        "menu" => ui.screen = Screen::Menu(Menu::new("Jonas".into())),
        "create" => {
            let mut m = Menu::new("Jonas".into());
            m.creating = true;
            m.bots = vec![1, 2];
            ui.screen = Screen::Menu(m);
        }
        "lobby" => {
            be.act(UiAction::Login { user: "Jonas".into(), pass: "x".into(), register: false });
            be.act(UiAction::CreateGame { name: "Friday Night War".into(), length_days: 7, bots: vec!["hard".into()] });
            let mut info = None;
            for ev in be.poll() {
                if let ui::BackendEvent::Lobby(i) = ev {
                    info = Some(i);
                }
            }
            let mut info = info.unwrap();
            info.players.insert(1, Player { id: 2, name: "Mara".into(), color: 1, faction: "HIVE BROOD".into(), online: true });
            info.players[2].color = 2;
            ui.screen = Screen::Lobby(Lobby { info: Some(info), me: 1, faction: 0 });
        }
        _ => {
            game(ui, be);
            let Screen::Game(g) = &mut ui.screen else { unreachable!() };
            let v = be.view().unwrap().clone();
            match scene {
                "body" => g.selected = Some(Focus::Body(2)),
                "target" => {
                    g.selected = Some(Focus::Ship(1));
                    g.mode = Mode::Target(1);
                }
                "plan" | "scrub" => {
                    let p = be.plan_transfer(1, 4, v.now).unwrap();
                    if scene == "scrub" {
                        g.scrub = (p.depart_t - v.now) + (p.arrive_t - p.depart_t) * 0.5;
                    }
                    g.selected = Some(Focus::Ship(1));
                    g.mode = Mode::Plan(p);
                }
                "nodes" => {
                    let s = v.ship(4).unwrap();
                    let t = s.trajectory.as_ref().unwrap().end().unwrap() * 0.5 + v.now * 0.5;
                    let nodes = vec![ManeuverNode { t, prograde: 1400.0, radial: -600.0 }];
                    let preview = be.preview_nodes(4, &nodes);
                    g.selected = Some(Focus::Ship(4));
                    g.mode = Mode::Nodes(NodeEdit { ship: 4, nodes, sel: 0, preview, dirty: false, drag: None });
                }
                "chat" => g.chat_open = true,
                "toast" => {
                    g.push_notice(Notice { kind: NoticeKind::Battle, title: "BATTLE AT TETHYS!".into(), body: "Corsair pack engaged Red Fang".into(), focus: Some(Focus::Body(2)) });
                    g.push_notice(Notice { kind: NoticeKind::Good, title: "GAIA NOVA IS YOURS!".into(), body: "Most power in orbit".into(), focus: Some(Focus::Body(3)) });
                }
                _ => {}
            }
        }
    }
}

#[macroquad::main(conf)]
async fn main() {
    let shot = std::env::var("UI_SHOT").ok();
    let mut be = DemoBackend::new();
    let mut ui = Ui::new();
    ui::art::load("assets").await;
    if shot.is_some() {
        ui::art::set_muted(true);
    }
    let scene = std::env::var("UI_SCENE").unwrap_or_default();
    if !scene.is_empty() {
        setup(&scene, &mut ui, &mut be);
    }
    let mut frame = 0;
    loop {
        ui.frame(&mut be);
        frame += 1;
        if let Some(path) = &shot {
            if frame == 20 {
                get_screen_data().export_png(path);
                break;
            }
        }
        next_frame().await;
    }
}

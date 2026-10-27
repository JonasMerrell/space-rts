//! Offline UI demo on the fake backend.
//!   cargo run -p ui --example demo
//! Screenshot mode: UI_SCENE=<login|menu|create|lobby|lobby_guest|map|body|owned|yard|ship|gmenu|queue|status|res_energy|detail_up|detail_ship|rock|impact|target|plan|nodes|scrub|chat|toast> UI_SHOT=out.png
//! sets the scene up, renders a few frames, saves the PNG and exits.

use macroquad::prelude::*;
use ui::demo::DemoBackend;
use ui::game::{GameScreen, Mode, NodeEdit};
use ui::model::*;
use ui::screens::Menu;
use ui::{Backend, Screen, Ui, UiAction};

fn conf() -> Conf {
    let w = std::env::var("UI_W").ok().and_then(|v| v.parse().ok()).unwrap_or(540);
    let h = std::env::var("UI_H").ok().and_then(|v| v.parse().ok()).unwrap_or(1170);
    Conf { window_title: "space-rts ui demo".into(), window_width: w, window_height: h, high_dpi: true, ..Default::default() }
}

fn setup(scene: &str, ui: &mut Ui, be: &mut DemoBackend) {
    ui.user = "Jonas".into();
    let game = |ui: &mut Ui, be: &mut DemoBackend| {
        be.act(UiAction::Login { server: String::new(), user: "Jonas".into(), pass: "x".into(), register: false, invite: String::new() });
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
            be.act(UiAction::Login { server: String::new(), user: "Jonas".into(), pass: "x".into(), register: false, invite: String::new() });
            be.act(UiAction::CreateGame { name: "Friday Night War".into(), length_days: 7, speed: 1.0, bots: vec!["hard".into()] });
            let mut info = None;
            for ev in be.poll() {
                if let ui::BackendEvent::Lobby(i) = ev {
                    info = Some(i);
                }
            }
            let mut info = info.unwrap();
            info.players.insert(1, Player { id: 2, name: "Mara".into(), color: 1, faction: "HIVE BROOD".into(), online: true });
            info.players[2].color = 2;
            info.players[2].online = false;
            be.enter_lobby(info);
            let _ = be.poll();
            ui.screen = Screen::Game(GameScreen::new());
        }
        "lobby_guest" => {
            be.act(UiAction::Login { server: String::new(), user: "Jonas".into(), pass: "x".into(), register: false, invite: String::new() });
            be.act(UiAction::JoinGame { code: "MX4PQ".into() });
            let _ = be.poll();
            ui.screen = Screen::Game(GameScreen::new());
        }
        _ => {
            game(ui, be);
            let Screen::Game(g) = &mut ui.screen else { unreachable!() };
            let v = be.view().unwrap().clone();
            match scene {
                "body" => g.selected = Some(Focus::Body(1)),
                "owned" => g.selected = Some(Focus::Body(2)),
                "yard" => {
                    g.selected = Some(Focus::Body(2));
                    g.body_tab = 1;
                }
                "gmenu" => g.menu_open = true,
                "research" | "research_w" => {
                    be.act(ui::UiAction::Research { body: 2, tech: "ModularYards".into() });
                    g.research_open = true;
                    g.research_branch = if scene == "research" { 2 } else { 0 };
                }
                "stations" => {
                    add_builders(be, 2, 2);
                    be.act(ui::UiAction::BuildStation { body: 2, kind: "Telescope".into() });
                    g.selected = Some(Focus::Body(2));
                    g.stations_for = Some(2);
                    g.station_pick = Some("FuelDepot".into());
                }
                "away" => {
                    add_builders(be, 3, 1);
                    g.selected = Some(Focus::Body(3));
                }
                "builder" => {
                    add_builders(be, 3, 3);
                    g.selected = Some(Focus::Ship(90));
                    g.mode = Mode::Target(90);
                }
                "report" => be.report = Some(ui::demo::demo_report(v.now)),
                "rock" => g.selected = Some(Focus::Body(9)),
                "queue" => {
                    for c in [ShipClass::Gunship, ShipClass::Scout, ShipClass::Bomber] {
                        be.act(UiAction::Build { body: 2, class: c });
                    }
                    be.act(UiAction::Upgrade { body: 2, upgrade: "Mining".into() });
                    g.selected = Some(Focus::Body(2));
                    g.detail = Some(ui::game::details::Detail::Queue { body: 2 });
                }
                "status" => {
                    // departing (Red Fang), burn ahead (Hammerhead), selected = Hammerhead
                    if let Some(p) = be.plan_transfer(1, 4, v.now) {
                        be.act(UiAction::ScheduleTransfer(p));
                    }
                    let end = v.ship(4).and_then(|s| s.trajectory.as_ref()).and_then(|t| t.end()).unwrap_or(v.now);
                    be.act(UiAction::SetNodes { ship: 4, nodes: vec![ManeuverNode { t: (v.now + end) / 2.0, prograde: 0.6, radial: 0.0 }] });
                    g.selected = Some(Focus::Ship(4));
                }
                "res_energy" => g.res_open = Some(1),
                "detail_up" => {
                    g.selected = Some(Focus::Body(2));
                    g.detail = Some(ui::game::details::Detail::Upgrade { body: 2, name: "Refinery".into() });
                }
                "detail_ship" => {
                    g.selected = Some(Focus::Body(2));
                    g.body_tab = 1;
                    g.detail = Some(ui::game::details::Detail::Ship { body: 2, class: ShipClass::Bomber });
                }
                "impact" => {
                    if let ui::ImpactPoll::Done(Some(ip)) = be.plan_impact_async(9, 4) {
                        g.selected = Some(Focus::Body(9));
                        g.mode = Mode::Impact(ip);
                    }
                }
                "ship" => g.selected = Some(Focus::Ship(1)),
                "fleet" | "fleet_new" => {
                    let (anchor, other) = if scene == "fleet" { (1, 5) } else { (5, 6) };
                    g.selected = Some(Focus::Ship(anchor));
                    let mut fs = ui::game::fleet::FleetSheet::new(anchor, Some(Focus::Ship(anchor)));
                    if scene == "fleet" {
                        fs.members.push(101);
                    }
                    fs.others.push(other);
                    g.fleet = Some(fs);
                }
                "fleet_many" => {
                    // lots of ships at Tethys: the list has to scroll
                    if let Some(v) = be.view.as_mut() {
                        let base = v.ships.iter().find(|s| s.id == 2).unwrap().clone();
                        for i in 0..12u32 {
                            let mut sh = base.clone();
                            sh.id = 500 + i;
                            sh.name = format!("Picket {}", i + 1);
                            sh.count = 1;
                            v.ships.push(sh);
                        }
                    }
                    g.selected = Some(Focus::Ship(1));
                    let mut fs = ui::game::fleet::FleetSheet::new(1, Some(Focus::Ship(1)));
                    fs.scroll = 140.0;
                    g.fleet = Some(fs);
                }
                "fleet_formed" => {
                    be.act(ui::UiAction::FormFleet { ships: vec![5, 6, 4], name: "Strike Group".into() });
                    be.act(ui::UiAction::LeaveFleet { ships: vec![101, 102], split: Some("Red Fang B".into()) });
                    let id = be.view.as_ref().unwrap().ships.iter().find(|s| s.name == "Strike Group").unwrap().id;
                    g.selected = Some(Focus::Ship(id));
                    g.fleet = Some(ui::game::fleet::FleetSheet::new(id, Some(Focus::Ship(id))));
                }
                "fleet_bar" => {
                    g.selected = Some(Focus::Ship(5));
                    g.mode = Mode::Target(5);
                }
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
                    let nodes = vec![ManeuverNode { t, prograde: 1.4, radial: -0.6 }];
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

/// Park `n` construction ships (one stack, id 90) at a body.
fn add_builders(be: &mut DemoBackend, body: BodyId, n: u32) {
    if let Some(v) = be.view.as_mut() {
        let me = v.me;
        v.ships.push(Ship {
            id: 90,
            name: "Hardhats".into(),
            class: ShipClass::Constructor,
            owner: me,
            count: n,
            fuel: 120.0,
            fuel_max: 120.0,
            parked_at: Some(body),
            trajectory: None,
            nodes: vec![],
            destination: None,
            retreat: false,
            seen_at: None,
            planned: None,
            members: vec![],
        });
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
    if let Some((x, y)) = std::env::var("UI_HOVER").ok().and_then(|v| v.split_once(',').map(|(a, b)| (a.parse().unwrap_or(0.0), b.parse().unwrap_or(0.0)))) {
        ui.input.script_hover = Some(vec2(x, y));
    }
    let mut frame = 0;
    loop {
        // UI_TAP=x,y : scripted tap after 5 frames (screenshot debugging)
        if frame == 5 {
            if let Some((x, y)) = std::env::var("UI_TAP").ok().and_then(|v| v.split_once(',').map(|(a, b)| (a.parse().unwrap_or(0.0), b.parse().unwrap_or(0.0)))) {
                ui.input.inject_tap(vec2(x, y));
            }
        }
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

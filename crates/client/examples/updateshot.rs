//! Screenshot of the "new version available" card on the real client: in-process server that
//! announces LATEST (default 0.9.0); build this with an older version baked in, e.g.
//!   HOHMANN_VERSION=0.8.2 SHOT=dir [LATEST=0.9.0] [UI_W=540 UI_H=1170] cargo run -p client --example updateshot
//! Writes dir/update.png (menu with the card) and dir/later.png (after LATER).
use macroquad::prelude::*;
use space_rts::backend::ClientBackend;
use std::time::{Duration, Instant};
use ui::{Backend, BackendEvent, Screen, Ui, UiAction};

fn conf() -> Conf {
    let w = std::env::var("UI_W").ok().and_then(|v| v.parse().ok()).unwrap_or(540);
    let h = std::env::var("UI_H").ok().and_then(|v| v.parse().ok()).unwrap_or(1170);
    Conf { window_title: "updateshot".into(), window_width: w, window_height: h, high_dpi: true, ..Default::default() }
}

#[macroquad::main(conf)]
async fn main() {
    let dir = std::env::var("SHOT").unwrap_or(".".into());
    let data = std::env::temp_dir().join(format!("updateshot-{}", std::process::id()));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let mut cfg = server::Config::new(data);
    cfg.latest_client = Some(std::env::var("LATEST").unwrap_or("0.9.0".into()));
    std::thread::spawn(move || server::serve_with(listener, cfg));

    ui::art::load("assets").await;
    ui::art::set_muted(true);
    let mut ui = Ui::new();
    let mut be = ClientBackend::new(addr);
    be.act(UiAction::Login { server: String::new(), user: "Jonas".into(), pass: "pw-update".into(), register: true, invite: String::new() });
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < 5.0 && !be.poll().iter().any(|e| matches!(e, BackendEvent::LoggedIn { .. })) {
        std::thread::sleep(Duration::from_millis(30));
    }
    eprintln!("this build: {:?}, update: {:?}", space_rts::update::APP_VERSION, be.update_available());
    ui.user = "Jonas".into();
    ui.screen = Screen::Menu(ui::screens::Menu::new("Jonas".into()));
    for frame in 0..30 {
        ui.frame(&mut be);
        if frame == 29 {
            get_screen_data().export_png(&format!("{dir}/update.png"));
        }
        next_frame().await;
    }
    ui.update_dismissed = be.update_available().map(|u| u.latest);
    for frame in 0..10 {
        ui.frame(&mut be);
        if frame == 9 {
            get_screen_data().export_png(&format!("{dir}/later.png"));
        }
        next_frame().await;
    }
}

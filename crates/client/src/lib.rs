//! The game app: macroquad window + `ui::Ui` driven by `ClientBackend` (network + local sim).
pub mod backend;
pub mod net;
pub mod notify;

use macroquad::prelude::*;

/// Default server: netdash on the home LAN. Override with SPACE_RTS_SERVER (desktop).
pub const DEFAULT_SERVER: &str = "192.168.0.247:7777";

fn window_conf() -> Conf {
    Conf {
        window_title: "Hahmann's Reach".to_owned(),
        window_width: 412,
        window_height: 880,
        high_dpi: true,
        // MSAA configs are missing on some GPUs (and the emulator), and EGL then finds no config at all
        sample_count: if cfg!(target_os = "android") { 0 } else { 4 },
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let server = std::env::var("SPACE_RTS_SERVER").ok().or_else(load_server).unwrap_or_else(|| DEFAULT_SERVER.to_string());
    ui::art::load("assets").await;
    let mut ui = ui::Ui::new();
    let mut be = backend::ClientBackend::new(server);
    be.resume();
    loop {
        ui.frame(&mut be);
        next_frame().await;
    }
}

fn server_file() -> std::path::PathBuf {
    // Android: the app's private files dir; desktop: the config dir
    if cfg!(target_os = "android") {
        std::path::PathBuf::from("/data/data/net.lego.hahmannsreach/files/server.txt")
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        std::path::PathBuf::from(home).join(".config/hahmanns-reach/server.txt")
    }
}

fn load_server() -> Option<String> {
    std::fs::read_to_string(server_file()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Remember the last server the player logged in to.
pub fn save_server(addr: &str) {
    let p = server_file();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, addr);
}

/// Desktop entry point (the macro-generated `main` is private to this module).
pub fn run() {
    main();
}

/// Android entry point called by miniquad's native activity glue.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn quad_main() {
    main();
}

//! The game app: macroquad window + `ui::Ui` driven by `ClientBackend` (network + local sim).
pub mod backend;
pub mod net;

use macroquad::prelude::*;

/// Default server: netdash on the home LAN. Override with SPACE_RTS_SERVER (desktop).
pub const DEFAULT_SERVER: &str = "192.168.0.247:7777";

fn window_conf() -> Conf {
    Conf {
        window_title: "Kepler Clash".to_owned(),
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
    let server = std::env::var("SPACE_RTS_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.to_string());
    ui::art::load("assets").await;
    let mut ui = ui::Ui::new();
    let mut be = backend::ClientBackend::new(server);
    loop {
        ui.frame(&mut be);
        next_frame().await;
    }
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

//! The game app: macroquad window + `ui::Ui` driven by `ClientBackend` (network + local sim).
pub mod backend;
pub mod forecast;
pub mod net;
pub mod notify;
pub mod sensors;
#[cfg(not(target_arch = "wasm32"))]
pub mod tls;
pub mod update;
#[cfg(target_arch = "wasm32")]
pub mod web;

use macroquad::prelude::*;

/// Default server: netdash on the home LAN. Override with SPACE_RTS_SERVER (desktop).
pub const DEFAULT_SERVER: &str = "hohmannsreach.tplinkdns.com:7777";
/// The same server on the home LAN (tried when the public name can't be reached from inside).
pub const LAN_SERVER: &str = "192.168.0.105:7777";

/// LAN address to try after `addr` fails, if `addr` is the public game server.
pub fn lan_fallback(addr: &str) -> Option<&'static str> {
    (addr.trim() == DEFAULT_SERVER).then_some(LAN_SERVER)
}

/// Servers the game used to live on; a remembered one is moved to `DEFAULT_SERVER`.
const OLD_SERVERS: &[&str] = &["192.168.0.247:7777", "192.168.0.247", "192.168.0.105:7777"];

/// Point a remembered address that is a retired server at the current default.
pub fn migrate_server(addr: String) -> String {
    if OLD_SERVERS.contains(&addr.trim()) {
        DEFAULT_SERVER.to_string()
    } else {
        addr
    }
}

fn window_conf() -> Conf {
    Conf {
        window_title: "Hohmann's Reach".to_owned(),
        window_width: 412,
        window_height: 880,
        high_dpi: true,
        // MSAA configs are missing on some GPUs (and the emulator), and EGL then finds no config at all
        sample_count: if cfg!(target_os = "android") { 0 } else { 4 },
        ..Default::default()
    }
}

/// SPACE_RTS_SERVER overrides the server (desktop; there is no environment in the browser).
pub(crate) fn env_server() -> Result<String, std::env::VarError> {
    if cfg!(target_arch = "wasm32") {
        Err(std::env::VarError::NotPresent)
    } else {
        std::env::var("SPACE_RTS_SERVER")
    }
}

/// Wall-clock unix time in seconds (`SystemTime` panics in the browser).
pub fn unix_now() -> f64 {
    if cfg!(target_arch = "wasm32") {
        macroquad::miniquad::date::now()
    } else {
        proto::unix_now()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    // the browser shows nothing for a panic unless it reaches the console
    #[cfg(target_arch = "wasm32")]
    std::panic::set_hook(Box::new(|p| {
        macroquad::logging::error!("{p}");
    }));
    let server = env_server().ok().or_else(|| load_server().map(migrate_server)).unwrap_or_else(|| DEFAULT_SERVER.to_string());
    ui::art::load("assets").await;
    #[cfg(target_arch = "wasm32")]
    web::loaded();
    let mut ui = ui::Ui::new();
    whats_new(&mut ui);
    let mut sound = load_sound_prefs();
    ui::art::set_music_on(sound.0);
    ui::art::set_effects_on(sound.1);
    ui::game::map::set_enemy_paths(sound.2);
    let mut be = backend::ClientBackend::new(server);
    be.resume();
    loop {
        ui.frame(&mut be);
        let now = (ui::art::music_on(), ui::art::effects_on(), ui::game::map::enemy_paths());
        if now != sound {
            sound = now;
            save_sound_prefs(sound);
        }
        #[cfg(target_arch = "wasm32")]
        web::text_fields(&mut ui);
        next_frame().await;
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn server_file() -> std::path::PathBuf {
    // Android: the app's private files dir; desktop: the config dir
    if cfg!(target_os = "android") {
        std::path::PathBuf::from("/data/data/net.lego.hohmannsreach/files/server.txt")
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        std::path::PathBuf::from(home).join(".config/hohmanns-reach/server.txt")
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn load_server() -> Option<String> {
    std::fs::read_to_string(server_file()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// In the browser the server is always the one that served the page.
#[cfg(target_arch = "wasm32")]
fn load_server() -> Option<String> {
    Some(web::page_host()).filter(|s| !s.is_empty())
}

/// Remember the last server the player logged in to.
#[cfg(target_arch = "wasm32")]
pub fn save_server(_addr: &str) {}

/// Remember the last server the player logged in to.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_server(addr: &str) {
    let p = server_file();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, addr);
}

/// A small remembered setting: `<name>.txt` next to server.txt natively, localStorage key `name` on the web.
fn load_pref(name: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    let text = web::store_get(name);
    #[cfg(not(target_arch = "wasm32"))]
    let text = std::fs::read_to_string(server_file().with_file_name(format!("{name}.txt"))).ok();
    text
}

fn save_pref(name: &str, v: &str) {
    #[cfg(target_arch = "wasm32")]
    web::store_set(name, v);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let p = server_file().with_file_name(format!("{name}.txt"));
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, v);
    }
}

/// First launch of a new release: open "What's new" (once the player is logged in). A fresh install
/// (no saved login, server or settings) has nothing to compare with and just remembers the version.
fn whats_new(ui: &mut ui::Ui) {
    let Some(ver) = update::APP_VERSION else { return };
    let seen = load_pref("notes_seen").map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    if seen.as_deref() == Some(ver) {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    let installed_before = load_pref("sound").is_some();
    #[cfg(not(target_arch = "wasm32"))]
    let installed_before = load_pref("sound").is_some() || load_pref("server").is_some();
    let returning = seen.is_some() || installed_before || notify::load().is_some();
    if returning && seen.as_deref().is_none_or(|s| ui::notes::newer(ver, s)) {
        ui.show_whats_new(ver, seen.as_deref());
    }
    save_pref("notes_seen", ver);
}

/// Music / effects / enemy-paths switches, remembered across launches ("music effects paths" as 0/1).
fn load_sound_prefs() -> (bool, bool, bool) {
    let text = load_pref("sound");
    let mut it = text.as_deref().unwrap_or("1 1 1").split_whitespace().map(|v| v != "0");
    (it.next().unwrap_or(true), it.next().unwrap_or(true), it.next().unwrap_or(ui::game::map::enemy_paths()))
}

fn save_sound_prefs((music, effects, paths): (bool, bool, bool)) {
    save_pref("sound", &format!("{} {} {}", music as u8, effects as u8, paths as u8));
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

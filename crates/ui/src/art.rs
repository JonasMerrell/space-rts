//! Sprites and sounds from the repo-level `assets/` dir (see assets/manifest.json).
//! Everything is optional: if a file is missing the UI falls back to vector cel drawing.

use crate::model::*;
use macroquad::audio::{load_sound, play_sound, play_sound_once, stop_sound, PlaySoundParams, Sound};
use macroquad::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

pub const SKINS: [&str; 4] = ["vanguard", "hive", "prism", "junker"];
const TEAM_NAMES: [&str; 4] = ["blue", "red", "green", "yellow"];
const CLASSES: [&str; 5] = ["scout", "interceptor", "gunship", "bomber", "tanker"];
const PLANETS: [&str; 11] = [
    "planet_terran", "planet_ocean", "planet_desert", "planet_ice", "planet_lava", "planet_barren", "planet_toxic",
    "planet_jungle", "planet_gas_orange", "planet_gas_blue", "planet_gas_ringed",
];
const MOONS: [&str; 5] = ["moon_grey", "moon_icy", "moon_red", "moon_sulfur", "moon_carbon"];
const ASTEROIDS: [&str; 8] = [
    "asteroid_rock", "asteroid_rock2", "asteroid_rock3", "asteroid_rock4", "asteroid_metal", "asteroid_metal2", "asteroid_ice",
    "asteroid_ice2",
];
const STARS: [&str; 5] = ["star_yellow", "star_orange", "star_red", "star_blue", "star_white"];
pub const ICONS: [&str; 24] = [
    "matter", "energy", "fuel", "chat", "menu", "power", "telescope", "time", "alert", "fleet", "maneuver_node", "close",
    // research + stations
    "research", "check", "lock", "shield", "weapon_kinetic", "extraction",
    "station_shipyard", "station_fuel_depot", "station_telescope", "station_scanner", "station_defense", "station_relay",
];
const SFX: &[&str] = &[
    "ui_click", "ui_tap", "ui_confirm", "ui_back", "ui_error", "notification", "chat_message", "transfer_scheduled", "alert_enemy",
    "capture", "scan_ping", "alert_ping", "menu_open", "menu_close", "menu_select", "menu_tab", "toggle_on", "toggle_off",
    "slider_tick", "start_game", "lobby_join", "purchase",
];
const MUSIC: [&str; 2] = ["title", "orbit"];

pub struct Art {
    tex: HashMap<String, Texture2D>,
    sfx: HashMap<String, Sound>,
}

thread_local! {
    // Leaked like theme::FONTS (GL objects must not drop in TLS destructors).
    static ART: Cell<Option<&'static Art>> = const { Cell::new(None) };
    static MUSIC_ON: RefCell<Option<&'static str>> = const { RefCell::new(None) };
    static MUTED: Cell<bool> = const { Cell::new(false) };
    static LAST_TICK: Cell<f64> = const { Cell::new(0.0) };
}

/// Load sprites + sounds from `root` (usually "assets"; on Android the APK assets dir).
pub async fn load(root: &str) {
    let mut tex = HashMap::new();
    let mut paths: Vec<String> = vec![];
    for skin in SKINS {
        for class in CLASSES {
            for team in TEAM_NAMES.iter().chain(["neutral"].iter()) {
                paths.push(format!("sprites/ships/{skin}/{class}_{team}.png"));
            }
        }
    }
    for b in PLANETS.iter().chain(MOONS.iter()) {
        paths.push(format!("sprites/bodies/{b}_flat.png"));
    }
    for b in ASTEROIDS.iter().chain(STARS.iter()).chain(["star_glow", "shadow_overlay"].iter()) {
        paths.push(format!("sprites/bodies/{b}.png"));
    }
    for i in ICONS {
        paths.push(format!("sprites/icons/{i}.png"));
    }
    for u in ["select_ring", "node_marker", "transfer_window"] {
        paths.push(format!("sprites/ui/{u}.png"));
    }
    for s in SKINS {
        paths.push(format!("sprites/emblems/{s}.png"));
    }
    for p in paths {
        if let Ok(t) = load_texture(&format!("{root}/{p}")).await {
            t.set_filter(FilterMode::Linear);
            tex.insert(p, t);
        }
    }
    let mut sfx = HashMap::new();
    for s in SFX {
        if let Ok(snd) = load_sound(&format!("{root}/audio/sfx/{s}.wav")).await {
            sfx.insert(s.to_string(), snd);
        }
    }
    for m in MUSIC {
        if let Ok(snd) = load_sound(&format!("{root}/audio/music/{m}.wav")).await {
            sfx.insert(format!("music/{m}"), snd);
        }
    }
    ART.with(|a| a.set(Some(Box::leak(Box::new(Art { tex, sfx })))));
}

fn get() -> Option<&'static Art> {
    ART.with(|a| a.get())
}

pub fn tex(path: &str) -> Option<Texture2D> {
    get().and_then(|a| a.tex.get(path).cloned())
}

/// Draw `t` centred at `c`, `size` px wide/high, rotated `rot` radians, tinted.
pub fn draw_centered(t: &Texture2D, c: Vec2, size: Vec2, rot: f32, tint: Color) {
    draw_texture_ex(t, c.x - size.x / 2.0, c.y - size.y / 2.0, tint, DrawTextureParams { dest_size: Some(size), rotation: rot, ..Default::default() });
}

pub fn ship_tex(class: ShipClass, skin: &str, team: Option<usize>) -> Option<Texture2D> {
    let class = match class {
        ShipClass::Scout => "scout",
        ShipClass::Interceptor => "interceptor",
        ShipClass::Gunship => "gunship",
        ShipClass::Bomber => "bomber",
        // No dreadnought sprite yet: a big gunship hull (drawn larger by the map).
        ShipClass::Dreadnought => "gunship",
        ShipClass::Tanker => "tanker",
    };
    let team = team.map(|t| TEAM_NAMES[t % 4]).unwrap_or("neutral");
    let skin = if SKINS.contains(&skin) { skin } else { "vanguard" };
    tex(&format!("sprites/ships/{skin}/{class}_{team}.png"))
}

/// (texture key, radius_px / px) for a body. Falls back to a stable pick by id.
pub fn body_art(b: &Body) -> (String, f32) {
    let pick = |list: &[&str]| -> String {
        match &b.art {
            Some(a) if list.contains(&a.as_str()) => a.clone(),
            // Stars default to the first entry (yellow).
            _ if b.kind == BodyKind::Star => list[0].to_string(),
            _ => list[(b.id as usize * 7 + 3) % list.len()].to_string(),
        }
    };
    match b.kind {
        BodyKind::Star => (format!("sprites/bodies/{}.png", pick(&STARS)), 140.0 / 512.0),
        BodyKind::Planet => {
            let k = pick(&PLANETS);
            let ratio = if k == "planet_gas_ringed" { 112.0 / 384.0 } else { 112.0 / 256.0 };
            (format!("sprites/bodies/{k}_flat.png"), ratio)
        }
        BodyKind::Moon => (format!("sprites/bodies/{}_flat.png", pick(&MOONS)), 54.0 / 128.0),
        BodyKind::Asteroid => (format!("sprites/bodies/{}.png", pick(&ASTEROIDS)), 32.0 / 96.0),
    }
}

/// Faction display name → skin key.
pub fn skin_of(faction: &str) -> &'static str {
    let f = faction.to_lowercase();
    SKINS.iter().copied().find(|s| f.contains(s)).unwrap_or(match f.as_str() {
        x if x.contains("solar") || x.contains("navy") => "vanguard",
        x if x.contains("void") || x.contains("brood") => "hive",
        x if x.contains("choir") || x.contains("covenant") => "prism",
        x if x.contains("miner") || x.contains("clan") => "junker",
        _ => "vanguard",
    })
}

pub fn sfx(name: &str) {
    if MUTED.with(|m| m.get()) {
        return;
    }
    // Rate-limit detent ticks to ~20/s.
    if name == "slider_tick" {
        let now = get_time();
        if LAST_TICK.with(|t| now - t.replace(now) < 0.05) {
            return;
        }
    }
    if let Some(s) = get().and_then(|a| a.sfx.get(name)) {
        play_sound_once(s);
    }
}

/// Switch the looping music bed (None = silence). Cheap to call every frame.
pub fn music(track: Option<&'static str>) {
    let track = if MUTED.with(|m| m.get()) { None } else { track };
    MUSIC_ON.with(|cur| {
        let mut cur = cur.borrow_mut();
        if *cur == track {
            return;
        }
        let Some(a) = get() else { return };
        if let Some(old) = cur.and_then(|o| a.sfx.get(&format!("music/{o}"))) {
            stop_sound(old);
        }
        if let Some(new) = track.and_then(|t| a.sfx.get(&format!("music/{t}"))) {
            play_sound(new, PlaySoundParams { looped: true, volume: 0.45 });
        }
        *cur = track;
    });
}

pub fn set_muted(m: bool) {
    MUTED.with(|x| x.set(m));
    if m {
        music(None);
    }
}

pub fn muted() -> bool {
    MUTED.with(|m| m.get())
}

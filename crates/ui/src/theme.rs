//! Palette, fonts, UI scale and text. Game art (ships, planets) is cel-shaded comic (style L);
//! the UI chrome around it is deliberately plainer: dark glass panels, thin borders, clean type.

use macroquad::prelude::*;
use std::cell::Cell;

pub const GAME_TITLE: &str = "KEPLER CLASH";

pub const BG: Color = hex(0x1b1340);
pub const INK: Color = hex(0x000000);
pub const PAPER: Color = hex(0xffffff);
pub const SUN: Color = hex(0xffd23f);
pub const SUN_SHADE: Color = hex(0xff9f1c);
pub const ACCENT: Color = hex(0xffd23f);
pub const DANGER: Color = hex(0xe63946);
pub const GOOD: Color = hex(0x52b788);
pub const MUTED: Color = Color::new(1.0, 1.0, 1.0, 0.55);
pub const PANEL: Color = Color::new(0.06, 0.05, 0.15, 0.88);
pub const PANEL_HI: Color = Color::new(0.14, 0.12, 0.30, 0.92);
pub const BORDER: Color = Color::new(1.0, 1.0, 1.0, 0.16);
pub const FIELD: Color = Color::new(0.03, 0.02, 0.09, 0.9);

/// Light/dark pairs for team colours (blue, red, green, yellow) and neutral grey.
pub const TEAMS: [(Color, Color); 4] = [
    (hex(0x4cc9f0), hex(0x3a86ff)),
    (hex(0xe63946), hex(0x9d0208)),
    (hex(0x80ed99), hex(0x38a169)),
    (hex(0xffd23f), hex(0xe09f00)),
];
pub const NEUTRAL: (Color, Color) = (hex(0xadb5bd), hex(0x6c757d));

pub const fn hex(v: u32) -> Color {
    Color::new(
        ((v >> 16) & 0xff) as f32 / 255.0,
        ((v >> 8) & 0xff) as f32 / 255.0,
        (v & 0xff) as f32 / 255.0,
        1.0,
    )
}

pub fn team(color: Option<usize>) -> (Color, Color) {
    color.map(|c| TEAMS[c % TEAMS.len()]).unwrap_or(NEUTRAL)
}

pub fn alpha(c: Color, a: f32) -> Color {
    Color::new(c.r, c.g, c.b, a)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FontKind {
    /// Rajdhani Bold: titles, numbers, buttons.
    Title,
    /// Barlow Medium: body text, chat.
    Body,
}

struct Fonts {
    title: Font,
    body: Font,
}

thread_local! {
    // Leaked on purpose: dropping GL textures in TLS destructors after the context is gone segfaults.
    static FONTS: Cell<Option<&'static Fonts>> = const { Cell::new(None) };
    static SCALE: Cell<f32> = const { Cell::new(1.0) };
}

pub fn init() {
    let mut title = load_ttf_font_from_bytes(include_bytes!("../assets/Rajdhani-Bold.ttf")).unwrap();
    let mut body = load_ttf_font_from_bytes(include_bytes!("../assets/Barlow-Medium.ttf")).unwrap();
    title.set_filter(FilterMode::Linear);
    body.set_filter(FilterMode::Linear);
    FONTS.with(|f| f.set(Some(Box::leak(Box::new(Fonts { title, body })))));
}

/// Recompute the dp→px scale; call once per frame. 1 dp ≈ 1/400 of the short screen side.
pub fn update_scale() {
    let short = screen_width().min(screen_height());
    SCALE.with(|s| s.set((short / 400.0).max(0.75)));
}

/// dp → px.
pub fn s() -> f32 {
    SCALE.with(|s| s.get())
}

fn with_font<R>(kind: FontKind, f: impl FnOnce(&Font) -> R) -> R {
    FONTS.with(|fs| {
        let fs = fs.get().expect("theme::init not called");
        f(match kind {
            FontKind::Title => &fs.title,
            FontKind::Body => &fs.body,
        })
    })
}

pub fn measure(text: &str, kind: FontKind, size_dp: f32) -> Vec2 {
    let px = (size_dp * s()).round() as u16;
    with_font(kind, |f| {
        let d = measure_text(text, Some(f), px, 1.0);
        vec2(d.width, px as f32)
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Plain text; `y` is the vertical centre of the line.
pub fn text(t: &str, x: f32, y: f32, size_dp: f32, color: Color, kind: FontKind, align: Align) {
    let px = (size_dp * s()).round() as u16;
    with_font(kind, |f| {
        let d = measure_text(t, Some(f), px, 1.0);
        let x = match align {
            Align::Left => x,
            Align::Center => x - d.width / 2.0,
            Align::Right => x - d.width,
        };
        let baseline = y - d.height / 2.0 + d.offset_y;
        draw_text_ex(t, x, baseline, TextParams { font: Some(f), font_size: px, color, ..Default::default() });
    });
}

/// Title-font text with a thin dark halo so it stays legible over the map.
pub fn inked(t: &str, x: f32, y: f32, size_dp: f32, color: Color, align: Align) {
    let o = s().max(1.0);
    let halo = Color::new(0.0, 0.0, 0.0, 0.55);
    for (dx, dy) in [(-o, 0.0), (o, 0.0), (0.0, -o), (0.0, o)] {
        text(t, x + dx, y + dy, size_dp, halo, FontKind::Title, align);
    }
    text(t, x, y, size_dp, color, FontKind::Title, align);
}

/// Word-wrap `t` to `width` px.
pub fn wrap(t: &str, kind: FontKind, size_dp: f32, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in t.split('\n') {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if measure(&cand, kind, size_dp).x > width && !cur.is_empty() {
                lines.push(std::mem::replace(&mut cur, word.to_string()));
            } else {
                cur = cand;
            }
        }
        lines.push(cur);
    }
    lines
}

/// "Day 3 · 14:05" from seconds since game start.
pub fn fmt_clock(t: f64) -> String {
    let t = t.max(0.0) as u64;
    format!("DAY {} · {:02}:{:02}", t / 86400 + 1, (t / 3600) % 24, (t / 60) % 60)
}

/// "6h 12m" / "2d 4h" / "45s".
pub fn fmt_dur(d: f64) -> String {
    let neg = d < 0.0;
    let d = d.abs() as u64;
    let s = if d >= 86400 {
        format!("{}d {}h", d / 86400, (d / 3600) % 24)
    } else if d >= 3600 {
        format!("{}h {:02}m", d / 3600, (d / 60) % 60)
    } else if d >= 60 {
        format!("{}m", d / 60)
    } else {
        format!("{}s", d)
    };
    if neg { format!("-{s}") } else { s }
}

pub fn fmt_amount(v: f64) -> String {
    if v.abs() >= 10_000.0 {
        format!("{:.1}k", v / 1000.0)
    } else {
        format!("{:.0}", v)
    }
}

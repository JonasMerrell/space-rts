//! Immediate-mode widgets. Each takes a rect in px and the shared `Input`.
//! Flat, quiet chrome: the cel-shaded art lives on the map, not in the buttons.

use crate::cel;
use crate::input::Input;
use crate::theme::*;
use macroquad::prelude::*;

#[derive(Clone, Copy, PartialEq)]
pub enum Style {
    Primary,
    Secondary,
    Danger,
    Ghost,
}

/// (fill, border, label colour)
fn style_colors(st: Style) -> (Color, Color, Color) {
    match st {
        Style::Primary => (ACCENT, ACCENT, hex(0x1a1430)),
        Style::Secondary => (Color::new(1.0, 1.0, 1.0, 0.08), Color::new(1.0, 1.0, 1.0, 0.35), PAPER),
        Style::Danger => (alpha(DANGER, 0.9), DANGER, PAPER),
        Style::Ghost => (Color::new(0.0, 0.0, 0.0, 0.25), BORDER, MUTED),
    }
}

fn lighten(c: Color, k: f32) -> Color {
    Color::new(c.r + (1.0 - c.r) * k, c.g + (1.0 - c.g) * k, c.b + (1.0 - c.b) * k, (c.a + k).min(1.0))
}

/// Flat rounded button; returns true when tapped.
pub fn button(inp: &mut Input, r: Rect, label: &str, st: Style) -> bool {
    inp.block(r);
    let down = inp.holding(r);
    let (fill, border, fg) = style_colors(st);
    let rad = 8.0 * s();
    let b = (1.2 * s()).max(1.0);
    let hot = inp.hovering(r);
    cel::round_rect(r, rad, if down { lighten(fill, 0.18) } else if hot { lighten(fill, 0.09) } else { fill });
    cel::round_rect_lines(r, rad, b, border);
    let size = (r.h / s() * 0.42).min(22.0);
    text(label, r.center().x, r.center().y + s(), size, fg, FontKind::Title, Align::Center);
    let hit = inp.tapped(r);
    if hit {
        inp.consume_tap();
        crate::art::sfx(if st == Style::Ghost { "ui_back" } else { "ui_tap" });
    }
    hit
}

/// Round button with a short label or an "@icon" name. `fill` tints the ring when not plain.
pub fn round_button(inp: &mut Input, c: Vec2, rad: f32, label: &str, fill: Color, badge: usize) -> bool {
    // Touch target is at least 48dp across even when the button is drawn smaller.
    let hit = rad.max(24.0 * s());
    let r = Rect::new(c.x - hit, c.y - hit, hit * 2.0, hit * 2.0);
    inp.block(r);
    let down = inp.holding(r);
    let ring = if fill == PAPER { Color::new(1.0, 1.0, 1.0, 0.3) } else { alpha(fill, 0.8) };
    draw_circle(c.x, c.y, rad + 1.2 * s(), ring);
    draw_circle(c.x, c.y, rad, if down || inp.hovering(r) { PANEL_HI } else { PANEL });
    // "@name" = icon from assets/sprites/icons, text fallback.
    match label.strip_prefix('@') {
        Some(name) => match crate::art::tex(&format!("sprites/icons/{name}.png")) {
            Some(t) => crate::art::draw_centered(&t, c, vec2(rad, rad) * 1.25, 0.0, WHITE),
            None => text(&name.to_uppercase(), c.x, c.y, rad / s() * 0.55, PAPER, FontKind::Title, Align::Center),
        },
        None => text(label, c.x, c.y, rad / s() * 0.8, PAPER, FontKind::Title, Align::Center),
    }
    if badge > 0 {
        let b = c + vec2(rad * 0.72, -rad * 0.72);
        draw_circle(b.x, b.y, 8.0 * s(), DANGER);
        text(&badge.min(99).to_string(), b.x, b.y, 11.0, PAPER, FontKind::Title, Align::Center);
    }
    let hit = inp.tapped(r);
    if hit {
        inp.consume_tap();
        crate::art::sfx("ui_tap");
    }
    hit
}

#[derive(Default, Clone)]
pub struct TextField {
    pub text: String,
    /// Mirrors `Input::focused_field == id` (read-only; focus follows taps).
    pub focused: bool,
    id: u32,
    pub password: bool,
    pub upper: bool,
    pub max_len: usize,
}

impl TextField {
    pub fn new(password: bool) -> Self {
        Self { password, max_len: 64, id: next_field_id(), ..Default::default() }
    }
    pub fn code() -> Self {
        Self { upper: true, max_len: 6, id: next_field_id(), ..Default::default() }
    }
    /// Invite code (`K7QM-2XPA`).
    pub fn invite() -> Self {
        Self { upper: true, max_len: 16, id: next_field_id(), ..Default::default() }
    }
}

fn next_field_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Single-line text input. Returns true if Enter was pressed while focused.
/// Focus is owned by `Input` so only one field can have it at a time.
pub fn text_field(inp: &mut Input, r: Rect, f: &mut TextField, placeholder: &str) -> bool {
    if f.id == 0 {
        f.id = next_field_id();
    }
    inp.field(f.id, r);
    if inp.tapped(r) {
        inp.consume_tap();
    }
    f.focused = inp.focused_field == Some(f.id);
    let mut submit = false;
    if f.focused {
        for &c in &inp.chars {
            if f.text.chars().count() < f.max_len.max(1) {
                f.text.push(if f.upper { c.to_ascii_uppercase() } else { c });
            }
        }
        if inp.backspace {
            f.text.pop();
        }
        if let Some(p) = inp.paste.take() {
            // one line, trimmed to the field's limit
            for c in p.chars().filter(|c| !c.is_control()) {
                if f.text.chars().count() >= f.max_len.max(1) {
                    break;
                }
                f.text.push(if f.upper { c.to_ascii_uppercase() } else { c });
            }
        }
        if (inp.copy || inp.cut) && !f.password && !f.text.is_empty() {
            macroquad::miniquad::window::clipboard_set(&f.text);
            if inp.cut {
                f.text.clear();
            }
        }
        if inp.enter {
            submit = true;
            f.focused = false;
            inp.focused_field = None;
            macroquad::miniquad::window::show_keyboard(false);
        }
    }
    let b = if f.focused { 1.6 * s() } else { 1.0 * s() }.max(1.0);
    cel::round_rect(r, 8.0 * s(), FIELD);
    cel::round_rect_lines(r, 8.0 * s(), b, if f.focused { ACCENT } else { BORDER });
    let size = (r.h / s() * 0.45).min(22.0);
    let pad = 12.0 * s();
    let shown: String = if f.password { "•".repeat(f.text.chars().count()) } else { f.text.clone() };
    if shown.is_empty() && !f.focused {
        text(placeholder, r.x + pad, r.center().y, size, Color::new(1.0, 1.0, 1.0, 0.35), FontKind::Body, Align::Left);
    } else {
        // long text (e.g. pasted) scrolls left so the end and the cursor stay visible; clipped to the field
        let w = measure(&shown, FontKind::Body, size).x;
        let room = r.w - 2.0 * pad - 4.0 * s();
        let x0 = r.x + pad - (w - room).max(0.0);
        cel::clip(Some(Rect::new(r.x + 4.0 * s(), r.y, r.w - 8.0 * s(), r.h)));
        text(&shown, x0, r.center().y, size, PAPER, FontKind::Body, Align::Left);
        if f.focused && (get_time() * 2.0) as i64 % 2 == 0 {
            draw_rectangle(x0 + w + 2.0 * s(), r.center().y - size * s() * 0.45, 2.0 * s(), size * s() * 0.9, ACCENT);
        }
        cel::clip(None);
    }
    // COPY / PASTE menu (long-press or right-click on the field)
    if inp.field_menu == Some(f.id) {
        let bw = 78.0 * s();
        let mr = Rect::new(r.x + r.w - 2.0 * bw - 12.0 * s(), r.y - 46.0 * s(), 2.0 * bw + 12.0 * s(), 40.0 * s());
        inp.block(mr);
        inp.field_menu_rect = Some(mr);
        cel::panel(mr, alpha(PANEL, 0.98));
        let b1 = Rect::new(mr.x + 4.0 * s(), mr.y + 4.0 * s(), bw, mr.h - 8.0 * s());
        let b2 = Rect::new(b1.x + bw + 4.0 * s(), b1.y, bw, b1.h);
        let can_copy = !f.password && !f.text.is_empty();
        if button(inp, b1, "COPY", if can_copy { Style::Secondary } else { Style::Ghost }) {
            if can_copy {
                macroquad::miniquad::window::clipboard_set(&f.text);
            }
            inp.field_menu = None;
        }
        if button(inp, b2, "PASTE", Style::Primary) {
            if let Some(p) = macroquad::miniquad::window::clipboard_get() {
                for c in p.chars().filter(|c| !c.is_control()) {
                    if f.text.chars().count() >= f.max_len.max(1) {
                        break;
                    }
                    f.text.push(if f.upper { c.to_ascii_uppercase() } else { c });
                }
            }
            inp.field_menu = None;
        }
    }
    submit
}

/// "‹ value ›" stepper; returns the new value.
pub fn stepper(inp: &mut Input, r: Rect, label: &str, v: u32, lo: u32, hi: u32) -> u32 {
    let bw = r.h;
    let mut v = v;
    if button(inp, Rect::new(r.x, r.y, bw, r.h), "-", Style::Secondary) && v > lo {
        v -= 1;
    }
    if button(inp, Rect::new(r.x + r.w - bw, r.y, bw, r.h), "+", Style::Secondary) && v < hi {
        v += 1;
    }
    text(label, r.center().x, r.center().y, (r.h / s() * 0.45).min(22.0), PAPER, FontKind::Title, Align::Center);
    v
}

/// Toggle chip; returns true when tapped.
pub fn chip(inp: &mut Input, r: Rect, label: &str, on: bool, color: Color) -> bool {
    inp.block(r);
    let b = (1.2 * s()).max(1.0);
    cel::round_rect(r, r.h / 2.0, if on { alpha(color, 0.22) } else if inp.hovering(r) { PANEL_HI } else { PANEL });
    cel::round_rect_lines(r, r.h / 2.0, b, if on { color } else { BORDER });
    let size = (r.h / s() * 0.42).min(18.0);
    if on {
        text(label, r.center().x, r.center().y, size, color, FontKind::Title, Align::Center);
    } else {
        text(label, r.center().x, r.center().y, size, MUTED, FontKind::Title, Align::Center);
    }
    let hit = inp.tapped(r);
    if hit {
        inp.consume_tap();
    }
    hit
}

/// Thin rounded progress bar.
pub fn bar(r: Rect, frac: f32, fill: Color) {
    cel::round_rect(r, r.h / 2.0, Color::new(1.0, 1.0, 1.0, 0.12));
    let f = frac.clamp(0.0, 1.0);
    if f > 0.0 {
        cel::round_rect(Rect::new(r.x, r.y, (r.w * f).max(r.h), r.h), r.h / 2.0, fill);
    }
}

/// Log-scale slider for a multiplier in `lo..=hi` that snaps to `snaps` (and to whole numbers in
/// between). Returns the new value and whether the finger was lifted this frame (commit point).
/// `labels` draws the snap values under their ticks.
pub fn log_slider(inp: &mut Input, r: Rect, v: f64, lo: f64, hi: f64, snaps: &[f64], labels: bool) -> (f64, bool) {
    inp.block(r);
    let track = Rect::new(r.x + 12.0 * s(), r.y + 14.0 * s(), r.w - 24.0 * s(), 6.0 * s());
    let frac_of = |x: f64| ((x / lo).ln() / (hi / lo).ln()).clamp(0.0, 1.0) as f32;
    let mut v = v.clamp(lo, hi);
    let mut released = false;
    if let Some(p) = inp.held {
        if inp.gesture_on_ui() && r.contains(p) {
            let f = ((p.x - track.x) / track.w).clamp(0.0, 1.0);
            let raw = lo * (hi / lo).powf(f as f64);
            let near = snaps.iter().copied().min_by(|a, b| ((frac_of(*a) - f).abs()).total_cmp(&(frac_of(*b) - f).abs()));
            let new = match near {
                Some(n) if (frac_of(n) - f).abs() * track.w < 12.0 * s() => n,
                _ => raw.round().clamp(lo, hi),
            };
            if new != v {
                crate::art::sfx("slider_tick");
            }
            v = new;
        }
    }
    if inp.released.is_some_and(|p| r.contains(p)) {
        released = true;
    }
    bar(track, frac_of(v), ACCENT);
    for &k in snaps {
        let x = track.x + track.w * frac_of(k);
        draw_line(x, track.y + track.h + 3.0 * s(), x, track.y + track.h + 8.0 * s(), 1.5 * s(), MUTED);
        if labels {
            text(&format!("{k}×"), x, track.y + track.h + 18.0 * s(), 11.0, MUTED, FontKind::Title, Align::Center);
        }
    }
    let kc = vec2(track.x + track.w * frac_of(v), track.y + track.h / 2.0);
    draw_circle(kc.x, kc.y, 12.0 * s(), alpha(ACCENT, 0.35));
    draw_circle(kc.x, kc.y, 8.0 * s(), PAPER);
    (v, released)
}

//! Immediate-mode comic widgets. Each takes a rect in px and the shared `Input`.

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

fn style_fill(st: Style) -> (Color, Color) {
    match st {
        Style::Primary => (ACCENT, INK),
        Style::Secondary => (PAPER, INK),
        Style::Danger => (DANGER, PAPER),
        Style::Ghost => (PANEL_HI, PAPER),
    }
}

/// Chunky comic button; returns true when tapped.
pub fn button(inp: &mut Input, r: Rect, label: &str, st: Style) -> bool {
    inp.block(r);
    let down = inp.holding(r);
    let (fill, fg) = style_fill(st);
    let o = if down { 2.5 * s() } else { 0.0 };
    let rr = Rect::new(r.x + o, r.y + o, r.w, r.h);
    cel::panel(rr, fill);
    let size = (r.h / s() * 0.5).min(26.0);
    if fg == PAPER {
        inked(label, rr.center().x, rr.center().y, size, PAPER, Align::Center);
    } else {
        text(label, rr.center().x, rr.center().y, size, fg, FontKind::Title, Align::Center);
    }
    let hit = inp.tapped(r);
    if hit {
        inp.consume_tap();
    }
    hit
}

/// Round icon-ish button with a short glyph label (e.g. "≡", "NOW").
pub fn round_button(inp: &mut Input, c: Vec2, rad: f32, label: &str, fill: Color, badge: usize) -> bool {
    let r = Rect::new(c.x - rad, c.y - rad, rad * 2.0, rad * 2.0);
    inp.block(r);
    let down = inp.holding(r);
    let c = if down { c + vec2(2.0, 2.0) * s() } else { c };
    draw_circle(c.x + 3.0 * s(), c.y + 3.0 * s(), rad + 3.0 * s(), Color::new(0.0, 0.0, 0.0, 0.5));
    draw_circle(c.x, c.y, rad + 3.0 * s(), INK);
    draw_circle(c.x, c.y, rad, fill);
    text(label, c.x, c.y, rad / s() * 0.9, INK, FontKind::Title, Align::Center);
    if badge > 0 {
        let b = c + vec2(rad * 0.75, -rad * 0.75);
        draw_circle(b.x, b.y, 10.0 * s(), INK);
        draw_circle(b.x, b.y, 8.0 * s(), DANGER);
        text(&badge.min(99).to_string(), b.x, b.y, 12.0, PAPER, FontKind::Title, Align::Center);
    }
    let hit = inp.tapped(r);
    if hit {
        inp.consume_tap();
    }
    hit
}

#[derive(Default, Clone)]
pub struct TextField {
    pub text: String,
    pub focused: bool,
    pub password: bool,
    pub upper: bool,
    pub max_len: usize,
}

impl TextField {
    pub fn new(password: bool) -> Self {
        Self { password, max_len: 64, ..Default::default() }
    }
    pub fn code() -> Self {
        Self { upper: true, max_len: 6, ..Default::default() }
    }
}

/// Single-line text input. Returns true if Enter was pressed while focused.
pub fn text_field(inp: &mut Input, r: Rect, f: &mut TextField, placeholder: &str) -> bool {
    inp.block(r);
    if let Some(p) = inp.tap {
        let inside = r.contains(p);
        if inside != f.focused {
            f.focused = inside;
            macroquad::miniquad::window::show_keyboard(inside);
        }
        if inside {
            inp.consume_tap();
        }
    }
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
        if inp.enter {
            submit = true;
            f.focused = false;
            macroquad::miniquad::window::show_keyboard(false);
        }
    }
    let o = 3.0 * s();
    cel::round_rect(Rect::new(r.x - o, r.y - o, r.w + 2.0 * o, r.h + 2.0 * o), 8.0 * s() + o, if f.focused { ACCENT } else { INK });
    cel::round_rect(r, 8.0 * s(), PAPER);
    let size = (r.h / s() * 0.45).min(22.0);
    let pad = 12.0 * s();
    let shown: String = if f.password { "•".repeat(f.text.chars().count()) } else { f.text.clone() };
    if shown.is_empty() && !f.focused {
        text(placeholder, r.x + pad, r.center().y, size, Color::new(0.0, 0.0, 0.0, 0.4), FontKind::Body, Align::Left);
    } else {
        text(&shown, r.x + pad, r.center().y, size, INK, FontKind::Body, Align::Left);
        if f.focused && (get_time() * 2.0) as i64 % 2 == 0 {
            let w = measure(&shown, FontKind::Body, size).x;
            draw_rectangle(r.x + pad + w + 2.0 * s(), r.center().y - size * s() * 0.45, 2.5 * s(), size * s() * 0.9, INK);
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
    inked(label, r.center().x, r.center().y, (r.h / s() * 0.5).min(24.0), PAPER, Align::Center);
    v
}

/// Toggle chip; returns true when tapped.
pub fn chip(inp: &mut Input, r: Rect, label: &str, on: bool, color: Color) -> bool {
    inp.block(r);
    let o = 2.5 * s();
    cel::round_rect(Rect::new(r.x - o, r.y - o, r.w + 2.0 * o, r.h + 2.0 * o), r.h / 2.0 + o, INK);
    cel::round_rect(r, r.h / 2.0, if on { color } else { PANEL });
    let size = (r.h / s() * 0.5).min(20.0);
    if on {
        text(label, r.center().x, r.center().y, size, INK, FontKind::Title, Align::Center);
    } else {
        text(label, r.center().x, r.center().y, size, MUTED, FontKind::Title, Align::Center);
    }
    let hit = inp.tapped(r);
    if hit {
        inp.consume_tap();
    }
    hit
}

/// Horizontal fill bar with ink border.
pub fn bar(r: Rect, frac: f32, fill: Color) {
    let o = 2.0 * s();
    draw_rectangle(r.x - o, r.y - o, r.w + 2.0 * o, r.h + 2.0 * o, INK);
    draw_rectangle(r.x, r.y, r.w, r.h, PANEL);
    draw_rectangle(r.x, r.y, r.w * frac.clamp(0.0, 1.0), r.h, fill);
}

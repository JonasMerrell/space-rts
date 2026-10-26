//! "While you were away": a sheet listing what happened since the player last had the game
//! open (battles and territory, sensor contacts, own empire). Tapping a row jumps to it.

use super::map;
use super::GameScreen;
use crate::art;
use crate::backend::{Backend, UiAction};
use crate::cel;
use crate::input::Input;
use crate::model::*;
use crate::screens::{safe_bottom, safe_top};
use crate::theme::*;
use crate::widgets::{self, Style};
use macroquad::prelude::*;

#[derive(Default)]
pub struct ReportState {
    /// Scroll offset of the list, px.
    scroll: f32,
    /// `since` of the report the open sound was played for.
    opened: Option<f64>,
}

const SECTIONS: [(ReportSection, &str); 3] =
    [(ReportSection::Battles, "BATTLES & TERRITORY"), (ReportSection::Contacts, "SENSOR CONTACTS"), (ReportSection::Empire, "YOUR EMPIRE")];

pub fn away_report(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) {
    let Some(rep) = be.away_report() else {
        g.report = ReportState::default();
        return;
    };
    if g.report.opened != Some(rep.since) {
        g.report = ReportState { scroll: 0.0, opened: Some(rep.since) };
        art::sfx("menu_open");
    }
    let m = 12.0 * s();
    let r = Rect::new(m, safe_top() + m, screen_width() - 2.0 * m, screen_height() - safe_top() - safe_bottom() - 2.0 * m);
    inp.block(r);
    cel::panel(r, alpha(PANEL, 0.98));
    cel::round_rect_lines(r, 14.0 * s(), 1.0 * s(), BORDER);
    text("WHILE YOU WERE AWAY", r.x + 18.0 * s(), r.y + 26.0 * s(), 22.0, PAPER, FontKind::Title, Align::Left);
    let away = rep.now - rep.since;
    text(
        &format!("{} · {} since {}", plural(rep.items.len(), "report"), fmt_dur(away), fmt_clock(rep.since)),
        r.x + 18.0 * s(),
        r.y + 50.0 * s(),
        14.0,
        MUTED,
        FontKind::Body,
        Align::Left,
    );
    let mut close = widgets::round_button(inp, vec2(r.x + r.w - 24.0 * s(), r.y + 24.0 * s()), 14.0 * s(), "X", PAPER, 0);
    let bh = 50.0 * s();
    let br = Rect::new(r.x + 16.0 * s(), r.y + r.h - bh - 14.0 * s(), r.w - 32.0 * s(), bh);
    close |= widgets::button(inp, br, "TO THE MAP", Style::Ghost);

    // list, scrolled by dragging inside it
    let list = Rect::new(r.x + 10.0 * s(), r.y + 66.0 * s(), r.w - 20.0 * s(), br.y - r.y - 76.0 * s());
    if let (Some(d), Some(p)) = (inp.drag, inp.drag_pos) {
        if list.contains(p) {
            g.report.scroll -= d.y;
        }
    }
    let row_text_w = list.w - 64.0 * s();
    // lay out first so the scroll can be clamped
    let mut rows: Vec<(f32, Option<&str>, Option<&ReportItem>, Vec<String>)> = vec![];
    let mut h = 0.0;
    for (sec, label) in SECTIONS {
        let items: Vec<&ReportItem> = rep.items.iter().filter(|i| i.section == sec).rev().collect();
        if items.is_empty() {
            continue;
        }
        rows.push((h, Some(label), None, vec![]));
        h += 30.0 * s();
        for it in items {
            let lines = wrap(&it.text, FontKind::Body, 14.0, row_text_w);
            let rh = (26.0 + 18.0 * lines.len() as f32 + 10.0) * s();
            rows.push((h, None, Some(it), lines));
            h += rh;
        }
    }
    g.report.scroll = g.report.scroll.clamp(0.0, (h - list.h).max(0.0));
    let y0 = list.y - g.report.scroll;
    let mut tapped: Option<Focus> = None;
    let time = get_time() as f32;
    for (i, (y, label, item, lines)) in rows.iter().enumerate() {
        let y = y0 + y;
        let next = rows.get(i + 1).map(|r| y0 + r.0).unwrap_or(y0 + h);
        if next < list.y || y > list.y + list.h {
            continue;
        }
        if let Some(label) = label {
            if y >= list.y - 4.0 * s() {
                text(label, list.x + 8.0 * s(), y + 18.0 * s(), 13.0, MUTED, FontKind::Title, Align::Left);
            }
            continue;
        }
        let Some(it) = item else { continue };
        let row = Rect::new(list.x, y, list.w, next - y - 4.0 * s());
        // rows are clipped by skipping the ones that stick out of the list
        if row.y < list.y - 1.0 || row.y + row.h > list.y + list.h + 1.0 {
            continue;
        }
        let tone = match it.tone {
            t if t < 0 => DANGER,
            t if t > 0 => GOOD,
            _ => MUTED,
        };
        cel::panel(row, alpha(PANEL_HI, 0.55));
        draw_rectangle(row.x, row.y + 6.0 * s(), 3.0 * s(), row.h - 12.0 * s(), tone);
        let icon = vec2(row.x + 30.0 * s(), row.y + 26.0 * s());
        row_icon(view, it.focus, icon, time);
        let tx = row.x + 56.0 * s();
        text(&it.title.to_uppercase(), tx, row.y + 18.0 * s(), 13.0, tone, FontKind::Title, Align::Left);
        text(&fmt_clock(it.t), row.x + row.w - 10.0 * s(), row.y + 18.0 * s(), 12.0, MUTED, FontKind::Title, Align::Right);
        for (k, l) in lines.iter().enumerate() {
            text(l, tx, row.y + (38.0 + 18.0 * k as f32) * s(), 14.0, PAPER, FontKind::Body, Align::Left);
        }
        if it.focus.is_some() && inp.tapped(row) {
            inp.consume_tap();
            tapped = match it.focus {
                Some(Focus::Ship(id)) if view.ships.iter().all(|x| x.id != id) => it.fallback,
                f => f,
            };
        }
    }
    if rows.is_empty() {
        text("Quiet. Nothing your sensors noticed.", list.x + 8.0 * s(), list.y + 24.0 * s(), 14.0, MUTED, FontKind::Body, Align::Left);
    }
    if let Some(f) = tapped {
        art::sfx("menu_select");
        g.focus(be, f, view.now + g.scrub);
        g.emit(UiAction::DismissReport);
    } else if close {
        art::sfx("menu_close");
        g.emit(UiAction::DismissReport);
    }
}

fn row_icon(view: &GameView, focus: Option<Focus>, c: Vec2, time: f32) {
    match focus {
        Some(Focus::Body(id)) => {
            if let Some(b) = view.body(id) {
                let col = b.owner.and_then(|o| view.player(o)).map(|p| p.color);
                map::draw_body(b, c, 13.0 * s(), c + vec2(-200.0, -80.0), col, time);
            }
        }
        Some(Focus::Ship(id)) => {
            if let Some(sh) = view.ships.iter().find(|x| x.id == id) {
                let player = view.player(sh.owner);
                let skin = player.map(|p| art::skin_of(&p.faction)).unwrap_or("vanguard");
                if let Some(t) = art::ship_tex(sh.class, skin, player.map(|p| p.color)) {
                    art::draw_centered(&t, c, vec2(40.0 * s(), 40.0 * s()), -0.6, WHITE);
                }
            }
        }
        None => draw_circle_lines(c.x, c.y, 10.0 * s(), 1.5 * s(), MUTED),
    }
}

fn plural(n: usize, w: &str) -> String {
    if n == 1 {
        format!("1 {w}")
    } else {
        format!("{n} {w}s")
    }
}

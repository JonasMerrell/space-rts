//! Transfer-window tool: where a parked ship can go and when. One row per planet / moon / gas
//! giant with its next good window, the quickest trip that fits the fuel, and a strip of the
//! estimated Δv over the next few days (a slice of a porkchop plot: green = cheap, amber = the
//! edge of the fuel, red = out of reach). Sort by soonest window,
//! quickest trip or least fuel; tap a row (or a spot on its strip) to plan that course.
//! Opened with WINDOWS on a parked ship's sheet.

use super::panels::{close_x, sheet_rect};
use super::{GameScreen, Mode};
use crate::art;
use crate::backend::Backend;
use crate::cel;
use crate::input::Input;
use crate::model::*;
use crate::theme::*;
use crate::widgets;
use crate::WindowsPoll;
use macroquad::prelude::*;

const ROW_H: f32 = 60.0;
const ROW_GAP: f32 = 6.0;
/// Rows visible before the list scrolls (phone); computer mode also stops at the panel height.
const VISIBLE_ROWS: f32 = 5.5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sort {
    /// next window that fits the fuel, soonest first
    Soonest,
    /// earliest arrival that fits the fuel
    Quickest,
    /// least Δv of any window
    Cheapest,
}

#[derive(Clone, Debug)]
pub struct WindowsSheet {
    pub ship: ShipId,
    pub sort: Sort,
    /// List scroll offset in px (drag or mouse wheel over the list).
    pub scroll: f32,
}

impl WindowsSheet {
    pub fn new(ship: ShipId) -> Self {
        WindowsSheet { ship, sort: Sort::Soonest, scroll: 0.0 }
    }
}

/// The window a row stands for under the current sort (None = nothing fits the fuel).
fn pick(o: &Opportunity, sort: Sort, budget: f64) -> Option<WindowEst> {
    let fits = |w: &&WindowEst| w.dv <= budget * 0.9;
    match sort {
        Sort::Soonest => o.windows.iter().find(fits).copied(),
        Sort::Quickest => o.fastest,
        Sort::Cheapest => o.windows.iter().filter(fits).min_by(|a, b| a.dv.total_cmp(&b.dv)).copied(),
    }
}

fn sort_key(o: &Opportunity, sort: Sort, budget: f64) -> f64 {
    match (pick(o, sort, budget), sort) {
        (None, _) => f64::INFINITY,
        (Some(w), Sort::Soonest) => w.depart_t,
        (Some(w), Sort::Quickest) => w.arrive_t,
        (Some(w), Sort::Cheapest) => w.dv,
    }
}

/// Colour for an estimated Δv: green at the row's best, shading to amber at the edge of the
/// ship's fuel, faded red beyond it.
fn cost_color(dv: f32, best: f32, budget: f64) -> Color {
    let cap = (budget * 0.9) as f32;
    if !dv.is_finite() || dv > cap {
        return alpha(DANGER, 0.35);
    }
    let k = if cap > best { ((dv - best) / (cap - best)).clamp(0.0, 1.0) } else { 0.0 };
    Color::new(GOOD.r + (ACCENT.r - GOOD.r) * k, GOOD.g + (ACCENT.g - GOOD.g) * k, GOOD.b + (ACCENT.b - GOOD.b) * k, 1.0)
}

pub fn windows_sheet(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) {
    let Mode::Windows(mut ws) = g.mode.clone() else { return };
    let poll = be.windows_async(ws.ship);
    let wv = match &poll {
        WindowsPoll::Done(Some(v)) => Some(v),
        _ => None,
    };
    let n = wv.map(|v| v.list.len()).unwrap_or(0);
    let row_px = (ROW_H + ROW_GAP) * s();
    let content_h = n as f32 * row_px;
    let fixed = (112.0 + 20.0) * s();
    let room = if wide() { (screen_height() - 260.0 * s() - fixed).max(row_px * 2.0) } else { f32::MAX };
    let list_h = if n == 0 { 40.0 * s() } else { content_h.min(VISIBLE_ROWS * row_px).min(room) };
    let r = sheet_rect((fixed + list_h) / s());
    inp.block(r);
    cel::panel(r, alpha(PANEL, 0.98));
    cel::round_rect_lines(r, 10.0 * s(), 1.0 * s(), BORDER);
    let pad = 16.0 * s();
    text("TRANSFER WINDOWS", r.x + pad, r.y + 24.0 * s(), 20.0, ACCENT, FontKind::Title, Align::Left);
    let ship_name = view.ship(ws.ship).map(|sh| sh.name.clone()).unwrap_or_default();
    let sub = match wv {
        Some(v) => format!(
            "{ship_name} at {} · next {} · {:.0} km/s of fuel",
            view.body(v.from).map(|b| b.name.as_str()).unwrap_or("?"),
            fmt_dur(v.horizon),
            v.budget
        ),
        None => format!("{ship_name} · scanning..."),
    };
    text(&sub, r.x + pad, r.y + 46.0 * s(), 13.0, MUTED, FontKind::Body, Align::Left);
    if close_x(inp, r) {
        g.mode = Mode::Idle;
        return;
    }

    // sort chips
    let cy = r.y + 60.0 * s();
    let cw = (r.w - 2.0 * pad - 2.0 * 8.0 * s()) / 3.0;
    for (i, (label, sort)) in [("SOONEST", Sort::Soonest), ("QUICKEST", Sort::Quickest), ("LEAST FUEL", Sort::Cheapest)].into_iter().enumerate() {
        let cr = Rect::new(r.x + pad + i as f32 * (cw + 8.0 * s()), cy, cw, 32.0 * s());
        if widgets::chip(inp, cr, label, ws.sort == sort, ACCENT) && ws.sort != sort {
            art::sfx("menu_tab");
            ws.sort = sort;
            ws.scroll = 0.0;
        }
    }

    let list = Rect::new(r.x + pad, cy + 52.0 * s(), r.w - 2.0 * pad, list_h);
    let Some(wv) = wv else {
        let msg = if matches!(poll, WindowsPoll::Pending) { "Looking for transfer windows..." } else { "Only a ship parked in orbit can plan transfers" };
        text(msg, list.x, list.y + 18.0 * s(), 14.0, MUTED, FontKind::Body, Align::Left);
        g.mode = Mode::Windows(ws);
        return;
    };
    // strip time axis, above the list
    let strip_x = |row: Rect| (row.x + row.w * 0.46, row.w * 0.54 - 10.0 * s());
    let (sx, sw) = strip_x(list);
    for (k, lbl) in [(0.0, "NOW".to_string()), (0.5, format!("+{}", fmt_dur(wv.horizon / 2.0))), (1.0, format!("+{}", fmt_dur(wv.horizon)))] {
        let al = if k == 0.0 { Align::Left } else if k == 1.0 { Align::Right } else { Align::Center };
        text(&lbl, sx + sw * k as f32, list.y - 9.0 * s(), 11.0, MUTED, FontKind::Title, al);
    }

    let mut rows: Vec<&Opportunity> = wv.list.iter().collect();
    rows.sort_by(|a, b| sort_key(a, ws.sort, wv.budget).total_cmp(&sort_key(b, ws.sort, wv.budget)));
    let max_scroll = (content_h - list_h).max(0.0);
    if let (Some(d), Some(p)) = (inp.drag, inp.drag_pos) {
        if list.contains(p) {
            ws.scroll -= d.y;
        }
    }
    if let (Some((_, k)), Some(p)) = (inp.zoom, inp.hover) {
        if list.contains(p) {
            ws.scroll -= k.ln() / 1.15f32.ln() * row_px;
        }
    }
    ws.scroll = ws.scroll.clamp(0.0, max_scroll);
    let tap = inp.tap.filter(|&p| list.contains(p) && !inp.occluded(p));
    let mut go: Option<(BodyId, f64)> = None;
    cel::clip(Some(list));
    let mut y = list.y - ws.scroll;
    for o in rows {
        let rr = Rect::new(list.x, y, list.w, ROW_H * s());
        y += row_px;
        if rr.y + rr.h < list.y || rr.y > list.y + list.h {
            continue;
        }
        let hot = inp.hovering(rr) || inp.holding(rr);
        cel::round_rect(rr, 8.0 * s(), alpha(PAPER, if hot { 0.10 } else { 0.05 }));
        let name = view.body(o.target).map(|b| b.name.to_uppercase()).unwrap_or_default();
        text(&name, rr.x + 10.0 * s(), rr.y + 17.0 * s(), 15.0, PAPER, FontKind::Title, Align::Left);
        let via: Vec<String> = o.via.iter().filter_map(|&b| view.body(b)).map(|b| b.name.clone()).collect();
        let w = pick(o, ws.sort, wv.budget);
        let (line1, line2, col) = match w {
            Some(w) => {
                let leave = if w.depart_t - view.now < 600.0 { "go now".to_string() } else { format!("go in {}", fmt_dur(w.depart_t - view.now)) };
                (format!("{leave} · {:.1} km/s", w.dv), format!("arrive in {}{}", fmt_dur(w.arrive_t - view.now), if via.is_empty() { String::new() } else { format!(" via {}", via.join(", ")) }), PAPER)
            }
            None => ("too far for this fuel".to_string(), if via.is_empty() { String::new() } else { format!("via {}", via.join(", ")) }, DANGER),
        };
        text(&line1, rr.x + 10.0 * s(), rr.y + 36.0 * s(), 12.0, col, FontKind::Body, Align::Left);
        text(&line2, rr.x + 10.0 * s(), rr.y + 51.0 * s(), 12.0, MUTED, FontKind::Body, Align::Left);
        // Δv strip: green cheap → amber at the fuel limit, red out of reach; diamonds = windows, dot = quickest
        let (sx, sw) = strip_x(rr);
        let bar = Rect::new(sx, rr.y + 24.0 * s(), sw, 12.0 * s());
        let n = o.strip.len().max(1);
        let best = o.strip.iter().copied().fold(f32::INFINITY, f32::min);
        for (i, &dv) in o.strip.iter().enumerate() {
            let x0 = bar.x + bar.w * i as f32 / n as f32;
            draw_rectangle(x0, bar.y, bar.w / n as f32 + 0.5, bar.h, cost_color(dv, best, wv.budget));
        }
        draw_rectangle_lines(bar.x, bar.y, bar.w, bar.h, 1.0 * s(), BORDER);
        let x_of = |t: f64| bar.x + bar.w * (((t - wv.start) / wv.horizon) as f32).clamp(0.0, 1.0);
        for win in &o.windows {
            let on = w.is_some_and(|p| p.depart_t == win.depart_t);
            cel::diamond(vec2(x_of(win.depart_t), bar.y - 4.0 * s()), if on { 6.0 } else { 4.5 } * s(), if win.dv <= wv.budget * 0.9 { if on { ACCENT } else { PAPER } } else { MUTED });
        }
        if let Some(f) = o.fastest {
            draw_circle(x_of(f.depart_t), bar.y + bar.h + 5.0 * s(), 3.5 * s(), if ws.sort == Sort::Quickest { ACCENT } else { PAPER });
        }
        if let Some(p) = tap.filter(|&p| rr.contains(p)) {
            // on the strip: leave at that time; elsewhere: the row's window
            let after = if bar.contains(p) || (p.x >= bar.x && p.y >= bar.y - 10.0 * s()) {
                wv.start + wv.horizon * ((p.x - bar.x) / bar.w).clamp(0.0, 1.0) as f64
            } else {
                w.map(|w| w.depart_t).unwrap_or(view.now)
            };
            go = Some((o.target, after));
        }
    }
    cel::clip(None);
    if max_scroll > 0.0 {
        let th = (list_h / content_h * list_h).max(24.0 * s());
        let ty = list.y + (list_h - th) * (ws.scroll / max_scroll);
        cel::round_rect(Rect::new(list.x + list.w + 4.0 * s(), ty, 3.0 * s(), th), 1.5 * s(), MUTED);
    }
    g.mode = Mode::Windows(ws);
    if let Some((target, after)) = go {
        inp.consume_tap();
        art::sfx("ui_click");
        let ship = match &g.mode {
            Mode::Windows(w) => w.ship,
            _ => return,
        };
        g.plan(be, view, ship, target, after.max(view.now));
    }
}

//! Rewind (Jonas 2026-10-31): the time scrubber's PAST mode drags the map back through the war as we
//! saw it then (`Backend::past`), with every battle we fought or watched on the bar and on the map.
//! Tapping one opens its replay (`replay.rs`).

use super::panels::{hud_bottom, margin, scrub_top, SCRUB_H};
use super::GameScreen;
use crate::art;
use crate::backend::{Backend, UiAction};
use crate::cel;
use crate::input::Input;
use crate::model::*;
use crate::theme::*;
use crate::widgets::{self, Style};
use macroquad::prelude::*;

/// The rewind's colour (the forecast is amber).
pub const REWIND: Color = hex(0x5bc0eb);
/// Battles show on the rewound map this long after they ended (game s).
const MARK_AFTER: f64 = 1800.0;
/// While rewound, newer history is fetched this often (wall s).
const REFRESH: f64 = 60.0;

#[derive(Default)]
pub struct RewindState {
    /// game seconds before now the map shows
    pub back: f64,
    /// the battle list sheet is open
    pub list_open: bool,
    pub list_scroll: f32,
    /// wall time of the last LoadHistory
    asked: f64,
}

pub fn enter(g: &mut GameScreen) {
    art::sfx("menu_open");
    g.scrub = 0.0;
    g.mode = super::Mode::Idle;
    g.detail = None;
    g.ranges_open = false;
    g.rewind = Some(RewindState { back: 3600.0, ..Default::default() });
    g.emit(UiAction::LoadHistory);
    if let Some(r) = g.rewind.as_mut() {
        r.asked = get_time();
    }
}

pub fn leave(g: &mut GameScreen) {
    art::sfx("menu_close");
    g.rewind = None;
}

/// Seconds back the bar reaches (to the first snapshot; at least an hour so the bar can be drawn).
fn span(h: &HistoryView, now: f64) -> f64 {
    h.start.map_or(3600.0, |s| (now - s).max(3600.0))
}

/// Keep the history fresh and the knob inside it.
pub fn update(g: &mut GameScreen, be: &dyn Backend, now: f64) {
    let Some(r) = g.rewind.as_mut() else { return };
    let h = be.history().unwrap_or_default();
    r.back = r.back.clamp(0.0, span(&h, now));
    if get_time() - r.asked > REFRESH && !h.loading {
        r.asked = get_time();
        g.emit(UiAction::LoadHistory);
    }
}

/// Battles to mark on the map at time `t`.
fn marked(h: &HistoryView, t: f64) -> impl Iterator<Item = &PastBattle> {
    h.battles.iter().filter(move |b| b.start_t - 60.0 <= t && t <= b.end_t + MARK_AFTER)
}

fn where_on_map(be: &dyn Backend, b: &PastBattle, t: f64) -> [f64; 2] {
    match b.body {
        Some(body) => be.body_pos(body, t.clamp(b.start_t, b.end_t)),
        None => b.pos,
    }
}

/// Map layer while rewound: a ring + REPLAY tag on every battle being fought then (or just over).
/// A tap on one opens its replay; returns true when it took the tap.
/// With `draw` false it only hit-tests (before the map takes the input).
pub fn map_marks(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, cam: &super::map::Camera, t: f64, draw: bool) -> bool {
    let Some(h) = be.history() else { return false };
    let k = 0.5 + 0.5 * (get_time() as f32 * 4.0).sin();
    let mut open = None;
    for b in marked(&h, t) {
        let p = cam.to_screen(where_on_map(be, b, t));
        let live = t <= b.end_t;
        let r = 26.0 * s();
        if draw {
            draw_circle_lines(p.x, p.y, r + 4.0 * s() * k, 2.5 * s(), alpha(DANGER, if live { 0.5 + 0.4 * k } else { 0.35 }));
        }
        if live && draw {
            cel::burst(p + vec2(r * 0.7, -r * 0.7), 9.0 * s(), 7, DANGER, get_time() as f32);
        }
        let tag = Rect::new(p.x - 34.0 * s(), p.y + r + 6.0 * s(), 68.0 * s(), 22.0 * s());
        if draw {
            cel::round_rect(tag, tag.h / 2.0, alpha(DANGER, 0.9));
            text("REPLAY", tag.center().x, tag.center().y + s(), 12.0, PAPER, FontKind::Title, Align::Center);
        } else if let Some(tp) = inp.tap.filter(|q| !inp.occluded(*q)) {
            if tag.contains(tp) || tp.distance(p) < r + 8.0 * s() {
                open = Some(b.id);
            }
        }
    }
    if let Some(id) = open {
        inp.consume_tap();
        super::replay::open(g, id);
        return true;
    }
    false
}

/// Frame + "REWIND -3h 20m" pill over the map.
pub fn banner(back: f64, at: f64, loading: bool, empty: bool) {
    let w = screen_width();
    let (t, b) = (hud_bottom(), scrub_top() - 8.0 * s());
    let k = 2.0 * s();
    let col = alpha(REWIND, 0.6);
    draw_rectangle(0.0, t, w, k, col);
    draw_rectangle(0.0, b - k, w, k, col);
    draw_rectangle(0.0, t, k, b - t, col);
    draw_rectangle(w - k, t, k, b - t, col);
    let label = if empty && loading {
        "LOADING HISTORY...".to_string()
    } else if empty {
        "NO HISTORY YET: IT IS RECORDED FROM NOW ON".to_string()
    } else {
        format!("-{}  ·  {}", fmt_dur(back), fmt_clock(at))
    };
    let lw = measure(&label, FontKind::Title, 15.0).x + 26.0 * s();
    let r = Rect::new((w - lw) / 2.0, t + 6.0 * s(), lw, 26.0 * s());
    cel::round_rect(r, r.h / 2.0, alpha(REWIND, 0.92));
    text(&label, r.center().x, r.center().y + s(), 15.0, hex(0x1a1430), FontKind::Title, Align::Center);
}

/// The scrubber in PAST mode: from the war's first snapshot to now, battles marked on it.
pub fn bar(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) {
    let now = view.now;
    let h = be.history().unwrap_or_default();
    let span = span(&h, now);
    let Some(st) = g.rewind.as_mut() else { return };
    let w = screen_width();
    let r = Rect::new(margin(), scrub_top(), w - 2.0 * margin(), SCRUB_H * s() - margin());
    inp.block(r);
    cel::panel(r, PANEL);
    let btn_w = 54.0 * s();
    let left = r.x + btn_w + 18.0 * s();
    let track = Rect::new(left + 8.0 * s(), r.y + r.h * 0.42, r.x + r.w - 2.0 * btn_w - 40.0 * s() - left, 8.0 * s());
    // the bar runs from the start (left) to now (right)
    let x_of = |back: f64| track.x + track.w * (1.0 - (back / span).clamp(0.0, 1.0)) as f32;

    let grab = Rect::new(track.x - 12.0 * s(), r.y, track.w + 24.0 * s(), r.h);
    let mut tapped_battle = None;
    if let Some(tp) = inp.tap.filter(|p| grab.contains(*p)) {
        // a tap right on a battle mark opens it
        tapped_battle = h.battles.iter().map(|b| (b.id, (x_of(now - b.start_t) - tp.x).abs())).filter(|x| x.1 < 10.0 * s() && tp.y < track.y + 4.0 * s()).min_by(|a, b| a.1.total_cmp(&b.1)).map(|x| x.0);
    }
    if tapped_battle.is_none() {
        if let Some(p) = inp.held {
            if inp.gesture_on_ui() && grab.contains(p) {
                let back = (1.0 - ((p.x - track.x) / track.w).clamp(0.0, 1.0) as f64) * span;
                if (back / 3600.0).floor() != (st.back / 3600.0).floor() {
                    art::sfx("slider_tick");
                }
                st.back = back;
            }
        }
    }
    widgets::bar(track, (1.0 - st.back / span) as f32, REWIND);
    // day ticks, counted back from now
    let (tick, label_every) = if span <= 12.0 * 3600.0 { (3600.0, 3.0 * 3600.0) } else if span <= 3.0 * 86_400.0 { (6.0 * 3600.0, 86_400.0) } else { (86_400.0, 2.0 * 86_400.0) };
    let mut k = 0.0;
    while k <= span + 1.0 {
        let x = x_of(k);
        let tall = ((k / label_every).round() - k / label_every).abs() < 1e-6;
        draw_line(x, track.y + track.h + 3.0 * s(), x, track.y + track.h + if tall { 10.0 } else { 6.0 } * s(), 2.0 * s(), MUTED);
        if tall {
            let l = if k == 0.0 { "NOW".to_string() } else { format!("-{}", fmt_dur(k).replace(" 0h", "")) };
            text(&l, x, track.y + track.h + 20.0 * s(), 12.0, MUTED, FontKind::Title, Align::Center);
        }
        k += tick;
    }
    for b in &h.battles {
        let x = x_of(now - b.start_t);
        cel::burst(vec2(x, track.y - 9.0 * s()), 6.0 * s(), 6, DANGER, 0.0);
    }
    let kc = vec2(x_of(st.back), track.y + track.h / 2.0);
    draw_circle(kc.x, kc.y, 11.0 * s(), alpha(REWIND, 0.35));
    draw_circle(kc.x, kc.y, 7.0 * s(), PAPER);
    if h.loading {
        text("loading...", track.x + track.w, track.y - 9.0 * s(), 11.0, MUTED, FontKind::Body, Align::Right);
    }
    // PAST (on: back to the forecast), BATTLES list, NOW
    let pr = Rect::new(r.x + 8.0 * s(), r.y + 9.0 * s(), btn_w, r.h - 18.0 * s());
    let mut leave_now = widgets::button(inp, pr, "PAST", Style::Primary);
    let br = Rect::new(r.x + r.w - 2.0 * btn_w - 14.0 * s(), r.y + 9.0 * s(), btn_w, r.h - 18.0 * s());
    let n = h.battles.len();
    if widgets::button(inp, br, &format!("{n}"), if st.list_open { Style::Primary } else { Style::Secondary }) {
        st.list_open = !st.list_open;
        art::sfx(if st.list_open { "menu_open" } else { "menu_close" });
    }
    text("BATTLES", br.center().x, br.y - 6.0 * s(), 10.0, MUTED, FontKind::Title, Align::Center);
    let nr = Rect::new(r.x + r.w - btn_w - 8.0 * s(), r.y + 9.0 * s(), btn_w, r.h - 18.0 * s());
    leave_now |= widgets::button(inp, nr, "NOW", Style::Ghost);
    if let Some(id) = tapped_battle {
        inp.consume_tap();
        super::replay::open(g, id);
    } else if leave_now {
        leave(g);
    }
}

/// The list of battles we can replay, newest first.
pub fn list(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) {
    let Some(st) = g.rewind.as_mut() else { return };
    if !st.list_open {
        return;
    }
    let h = be.history().unwrap_or_default();
    let w = screen_width();
    let sheet_w = if wide() { side_w() } else { w - 2.0 * margin() };
    let top = hud_bottom() + 40.0 * s();
    let r = Rect::new(w - margin() - sheet_w, top, sheet_w, (scrub_top() - 12.0 * s() - top).max(120.0 * s()));
    inp.block(r);
    cel::panel(r, PANEL);
    let pad = 14.0 * s();
    text("BATTLE REPLAYS", r.x + pad, r.y + 24.0 * s(), 18.0, PAPER, FontKind::Title, Align::Left);
    if widgets::round_button(inp, vec2(r.x + r.w - 24.0 * s(), r.y + 22.0 * s()), 14.0 * s(), "X", PAPER, 0) || inp.take_back() {
        st.list_open = false;
        art::sfx("menu_close");
        return;
    }
    let row_h = 58.0 * s();
    let body = Rect::new(r.x, r.y + 44.0 * s(), r.w, r.h - 50.0 * s());
    if h.battles.is_empty() {
        let msg = if h.loading { "Loading..." } else { "No battles you fought or saw yet" };
        text(msg, r.x + pad, body.y + 24.0 * s(), 14.0, MUTED, FontKind::Body, Align::Left);
        return;
    }
    if let Some(d) = inp.drag.filter(|_| inp.held.is_some_and(|p| body.contains(p))) {
        st.list_scroll -= d.y;
    }
    let total = h.battles.len() as f32 * row_h;
    st.list_scroll = st.list_scroll.clamp(0.0, (total - body.h).max(0.0));
    let me = view.me;
    let name = |p: PlayerId| view.player(p).map(|x| x.name.clone()).unwrap_or_else(|| "?".into());
    let mut open = None;
    for (i, b) in h.battles.iter().rev().enumerate() {
        let y = body.y + i as f32 * row_h - st.list_scroll;
        if y + row_h < body.y || y > body.y + body.h {
            continue;
        }
        let row = Rect::new(r.x + 8.0 * s(), y, r.w - 16.0 * s(), row_h - 6.0 * s());
        cel::round_rect(row, 10.0 * s(), if inp.hovering(row) { PANEL_HI } else { alpha(PANEL_HI, 0.5) });
        let sides: Vec<String> = b.sides.iter().map(|&p| name(p)).collect();
        text(&format!("{} · {}", b.place.to_uppercase(), sides.join(" vs ")), row.x + 12.0 * s(), row.y + 20.0 * s(), 14.0, PAPER, FontKind::Title, Align::Left);
        let lost_me = if b.sides.contains(&me) { b.lost.get(me as usize).copied().unwrap_or(0) } else { 0 };
        let lost_them: u32 = b.lost.iter().enumerate().filter(|(p, _)| *p != me as usize).map(|(_, l)| l).sum();
        let ago = fmt_dur(view.now - b.start_t);
        let sub = if b.sides.contains(&me) { format!("{ago} ago · {} long · lost {lost_me} / {lost_them}", fmt_dur(b.end_t - b.start_t)) } else { format!("{ago} ago · {} long · seen by your sensors", fmt_dur(b.end_t - b.start_t)) };
        text(&sub, row.x + 12.0 * s(), row.y + 40.0 * s(), 12.0, MUTED, FontKind::Body, Align::Left);
        inked("PLAY", row.x + row.w - 14.0 * s(), row.center().y, 14.0, DANGER, Align::Right);
        if inp.tap.is_some_and(|p| row.contains(p) && body.contains(p)) {
            open = Some((b.id, b.start_t));
        }
    }
    if let Some((id, start)) = open {
        inp.consume_tap();
        if let Some(st) = g.rewind.as_mut() {
            st.back = (view.now - start).max(0.0);
        }
        super::replay::open(g, id);
    }
}

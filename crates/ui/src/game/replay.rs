//! Battle replays: a finished battle played back on the battle screen's arena, as we saw it
//! (`Backend::replay`), with play/pause, speed and a timeline.

use super::battle::draw_arena;
use super::map::Camera;
use super::rewind::REWIND;
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

/// Playback speeds (game seconds per second).
pub const SPEEDS: [(f64, &str); 4] = [(1.0, "1×"), (4.0, "4×"), (16.0, "16×"), (64.0, "64×")];

pub struct ReplayScreen {
    pub id: u32,
    /// game time shown (None until the replay has loaded)
    pub t: Option<f64>,
    pub playing: bool,
    pub speed: f64,
    cam: Option<Camera>,
    asked: bool,
}

pub fn open(g: &mut GameScreen, id: u32) {
    art::sfx("menu_open");
    g.replay = Some(ReplayScreen { id, t: None, playing: true, speed: 4.0, cam: None, asked: false });
}

fn fit(b: &BattleView) -> Camera {
    super::battle::fit_view(b)
}

pub fn replay_screen(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) {
    let Some(mut st) = g.replay.take() else { return };
    if !st.asked {
        st.asked = true;
        g.emit(UiAction::LoadReplay { id: st.id });
    }
    let info = be.history().and_then(|h| h.battles.into_iter().find(|b| b.id == st.id));
    let top = safe_top() + 8.0 * s();
    let bar = Rect::new(8.0 * s(), top, screen_width() - 16.0 * s(), 58.0 * s());
    let sheet_h = 112.0 * s();
    let sheet = Rect::new(8.0 * s(), screen_height() - safe_bottom() - sheet_h - 8.0 * s(), screen_width() - 16.0 * s(), sheet_h);
    let (start, end) = info.as_ref().map_or((0.0, 0.0), |b| (b.start_t, b.end_t));
    let t = *st.t.get_or_insert(start);
    let b = be.replay(st.id, t);

    // arena: pan / zoom
    cel::backdrop();
    if let Some(b) = &b {
        let mut cam = *st.cam.get_or_insert_with(|| fit(b));
        if inp.for_map() {
            if let Some((c, k)) = inp.zoom {
                let f = fit(b).zoom;
                cam.zoom_at(c, k, f * 0.1, f * 50.0);
            }
            if let Some(d) = inp.drag {
                cam.pan_px(d);
            }
        }
        st.cam = Some(cam);
        draw_arena(b, view, &cam, &[]);
    } else {
        let msg = if info.is_none() { "That battle isn't in your history" } else { "Loading replay..." };
        text(msg, screen_width() / 2.0, screen_height() / 2.0, 18.0, MUTED, FontKind::Title, Align::Center);
    }

    // ---- top bar
    inp.block(bar);
    cel::panel(bar, alpha(PANEL, 0.95));
    let place = info.as_ref().map_or("battle".to_string(), |b| b.place.to_uppercase());
    text(&format!("REPLAY · {place}"), bar.x + 14.0 * s(), bar.y + 22.0 * s(), 19.0, REWIND, FontKind::Title, Align::Left);
    let me = view.me;
    let name = |p: PlayerId| view.player(p).map(|x| x.name.clone()).unwrap_or_else(|| "?".into());
    if let Some(i) = &info {
        let sides: Vec<String> = i.sides.iter().map(|&p| name(p)).collect();
        let lost = i.sides.iter().map(|&p| format!("{} lost {}", name(p), i.lost.get(p as usize).copied().unwrap_or(0))).collect::<Vec<_>>().join(" · ");
        let sub = if i.sides.len() <= 3 { lost } else { sides.join(" vs ") };
        text(&sub, bar.x + 14.0 * s(), bar.y + 44.0 * s(), 13.0, MUTED, FontKind::Body, Align::Left);
    }
    let mut shut = widgets::round_button(inp, vec2(bar.x + bar.w - 26.0 * s(), bar.center().y), 15.0 * s(), "X", PAPER, 0);
    shut |= inp.take_back();
    if let Some(b) = &b {
        if widgets::round_button(inp, vec2(bar.x + bar.w - 70.0 * s(), bar.center().y), 15.0 * s(), "[ ]", PAPER, 0) {
            st.cam = Some(fit(b));
        }
    }

    // ---- bottom sheet: play / pause, timeline, speed
    inp.block(sheet);
    cel::panel(sheet, alpha(PANEL, 0.96));
    let pad = 14.0 * s();
    let len = (end - start).max(1.0);
    let pb = Rect::new(sheet.x + pad, sheet.y + 12.0 * s(), 64.0 * s(), 40.0 * s());
    let at_end = t >= end - 1e-6;
    if widgets::button(inp, pb, if st.playing && !at_end { "PAUSE" } else if at_end { "AGAIN" } else { "PLAY" }, Style::Primary) {
        if at_end {
            st.t = Some(start);
            st.playing = true;
        } else {
            st.playing = !st.playing;
        }
        art::sfx("ui_click");
    }
    let track = Rect::new(pb.x + pb.w + 18.0 * s(), pb.center().y - 4.0 * s(), sheet.x + sheet.w - pad - (pb.x + pb.w + 18.0 * s()), 8.0 * s());
    let grab = Rect::new(track.x - 12.0 * s(), pb.y - 6.0 * s(), track.w + 24.0 * s(), pb.h + 12.0 * s());
    if let Some(p) = inp.held {
        if inp.gesture_on_ui() && grab.contains(p) {
            st.t = Some(start + ((p.x - track.x) / track.w).clamp(0.0, 1.0) as f64 * len);
        }
    }
    widgets::bar(track, ((t - start) / len) as f32, REWIND);
    let kx = track.x + track.w * ((t - start) / len).clamp(0.0, 1.0) as f32;
    draw_circle(kx, track.center().y, 9.0 * s(), PAPER);
    text(&format!("{} / {}", fmt_dur(t - start), fmt_dur(len)), track.x, track.y + 24.0 * s(), 12.0, MUTED, FontKind::Title, Align::Left);
    let mine = b.as_ref().map_or(0, |b| b.units.iter().filter(|u| u.owner == me && matches!(u.unit, BattleRef::Ship(_))).count());
    let theirs = b.as_ref().map_or(0, |b| b.units.iter().filter(|u| u.owner != me && matches!(u.unit, BattleRef::Ship(_))).count());
    text(&format!("{mine} vs {theirs} ships in sight"), track.x + track.w, track.y + 24.0 * s(), 12.0, MUTED, FontKind::Body, Align::Right);
    let cw = 52.0 * s();
    let cy = sheet.y + sheet.h - 42.0 * s();
    for (i, &(k, label)) in SPEEDS.iter().enumerate() {
        let r = Rect::new(sheet.x + pad + i as f32 * (cw + 8.0 * s()), cy, cw, 30.0 * s());
        if widgets::chip(inp, r, label, st.speed == k, REWIND) {
            st.speed = k;
            art::sfx("menu_tab");
        }
    }
    // the fog note where there's room for it after the chips
    let note = "Only what your sensors saw";
    let after = sheet.x + pad + SPEEDS.len() as f32 * (cw + 8.0 * s()) + 6.0 * s();
    if after + measure(note, FontKind::Body, 12.0).x < sheet.x + sheet.w - pad {
        text(note, sheet.x + sheet.w - pad, cy + 15.0 * s(), 12.0, MUTED, FontKind::Body, Align::Right);
    }

    if st.playing && b.is_some() {
        st.t = Some((t + get_frame_time() as f64 * st.speed).min(end));
    }
    if shut {
        art::sfx("menu_close");
    } else {
        g.replay = Some(st);
    }
}

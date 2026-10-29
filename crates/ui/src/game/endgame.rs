//! War-level overlays: the results screen when a war ends, the bug report / feature request form,
//! and the host's "delete war" confirmation.

use super::GameScreen;
use crate::art;
use crate::backend::{Backend, UiAction};
use crate::cel;
use crate::input::Input;
use crate::model::*;
use crate::screens::{safe_bottom, safe_top};
use crate::theme::*;
use crate::widgets::{self, Style, TextField};
use macroquad::prelude::*;

/// Centred sheet rect: full width on phones, capped on computers.
fn centre_sheet(h_dp: f32) -> Rect {
    let m = 12.0 * s();
    let w = (screen_width() - 2.0 * m).min(560.0 * s());
    let h = (h_dp * s()).min(screen_height() - safe_top() - safe_bottom() - 2.0 * m);
    Rect::new((screen_width() - w) / 2.0, safe_top() + (screen_height() - safe_top() - safe_bottom() - h) / 2.0, w, h)
}

/// Game over: winner + standings. Returns true while it covers the screen.
pub fn results(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) -> bool {
    let Some(res) = be.results() else {
        g.results_seen = false;
        return false;
    };
    if g.results_hidden {
        return false;
    }
    if !g.results_seen {
        g.results_seen = true;
        art::sfx(if res.winner == Some(view.me) { "capture" } else { "notification" });
    }
    let rows = res.scores.len().min(8);
    // eight pilots: tighter rows so the sheet still fits a phone
    let step = if rows > 4 { 42.0 } else { 50.0 };
    let team_of = |id: PlayerId| view.player(id).and_then(|p| p.team);
    let teams: Vec<(u8, u32)> = {
        let mut t: Vec<(u8, u32)> = vec![];
        for &(pid, score) in &res.scores {
            if let Some(k) = team_of(pid) {
                match t.iter_mut().find(|e| e.0 == k) {
                    Some(e) => e.1 += score,
                    None => t.push((k, score)),
                }
            }
        }
        t.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        t
    };
    let extra = if teams.is_empty() { 0.0 } else { 24.0 };
    let r = centre_sheet(200.0 + extra + rows as f32 * step);
    inp.block(r);
    cel::panel(r, alpha(PANEL, 0.98));
    let pad = 18.0 * s();
    let name = |id: PlayerId| view.player(id).map(|p| p.name.clone()).unwrap_or_else(|| format!("Pilot {id}"));
    let (title, col) = match (res.winning_team, res.winner) {
        (Some(t), _) if team_of(view.me) == Some(t) => ("VICTORY".to_string(), GOOD),
        (Some(t), _) => (format!("TEAM {} WINS", t + 1), ACCENT),
        (None, Some(w)) if w == view.me => ("VICTORY".to_string(), GOOD),
        (None, Some(w)) => (format!("{} WINS", name(w).to_uppercase()), ACCENT),
        (None, None) => ("DRAW".to_string(), MUTED),
    };
    text("THE WAR IS OVER", r.center().x, r.y + 28.0 * s(), 14.0, MUTED, FontKind::Title, Align::Center);
    inked(&title, r.center().x, r.y + 62.0 * s(), 34.0, col, Align::Center);
    let sub = if teams.is_empty() { "Most territory when time ran out" } else { "Most team territory when time ran out" };
    text(sub, r.center().x, r.y + 92.0 * s(), 13.0, MUTED, FontKind::Body, Align::Center);
    let mut y = r.y + 112.0 * s();
    if !teams.is_empty() {
        let line: Vec<String> = teams.iter().map(|(t, sc)| format!("TEAM {} · {sc}", t + 1)).collect();
        text(&line.join("   "), r.center().x, y + 6.0 * s(), 13.0, PAPER, FontKind::Title, Align::Center);
        y += extra * s();
    }
    let top = res.scores.first().map(|s| s.1).unwrap_or(1).max(1);
    for (i, (pid, score)) in res.scores.iter().take(rows).enumerate() {
        let rr = Rect::new(r.x + pad, y, r.w - 2.0 * pad, (step - 8.0) * s());
        let me = *pid == view.me;
        cel::round_rect(rr, 8.0 * s(), if me { alpha(ACCENT, 0.12) } else { Color::new(1.0, 1.0, 1.0, 0.04) });
        let (l, _) = team(view.color_of(Some(*pid)));
        text(&format!("{}", i + 1), rr.x + 16.0 * s(), rr.center().y, 18.0, if i == 0 { ACCENT } else { MUTED }, FontKind::Title, Align::Center);
        draw_circle(rr.x + 40.0 * s(), rr.center().y, 7.0 * s(), l);
        let who = if me { format!("{} (you)", name(*pid)) } else { name(*pid) };
        let who = match team_of(*pid) {
            Some(t) => format!("{who} · T{}", t + 1),
            None => who,
        };
        text(&who, rr.x + 56.0 * s(), rr.center().y, 15.0, PAPER, FontKind::Title, Align::Left);
        let bar = Rect::new(rr.x + rr.w - 150.0 * s(), rr.center().y - 3.0 * s(), 90.0 * s(), 6.0 * s());
        widgets::bar(bar, *score as f32 / top as f32, l);
        text(&score.to_string(), rr.x + rr.w - 12.0 * s(), rr.center().y, 15.0, PAPER, FontKind::Title, Align::Right);
        y += step * s();
    }
    let by = r.y + r.h - 60.0 * s();
    let bw = (r.w - 2.0 * pad - 12.0 * s()) / 2.0;
    if widgets::button(inp, Rect::new(r.x + pad, by, bw, 46.0 * s()), "LOOK AT THE MAP", Style::Secondary) || inp.take_back() {
        g.results_hidden = true;
    }
    if widgets::button(inp, Rect::new(r.x + pad + bw + 12.0 * s(), by, bw, 46.0 * s()), "BACK TO MENU", Style::Primary) {
        g.emit(UiAction::BackToMenu);
    }
    true
}

/// Bug report / feature request form.
pub struct FeedbackForm {
    pub bug: bool,
    pub text: TextField,
}

impl FeedbackForm {
    pub fn new() -> Self {
        let mut text = TextField::new(false);
        text.max_len = 600;
        FeedbackForm { bug: true, text }
    }
}

impl Default for FeedbackForm {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns true while the form is open (it covers the screen).
pub fn feedback(g: &mut GameScreen, inp: &mut Input) -> bool {
    let Some(mut f) = g.feedback.take() else { return false };
    let r = centre_sheet(250.0);
    inp.block(r);
    cel::panel(r, alpha(PANEL, 0.98));
    let pad = 16.0 * s();
    text("SEND FEEDBACK", r.x + pad, r.y + 24.0 * s(), 20.0, PAPER, FontKind::Title, Align::Left);
    if let Some(l) = wrap("Goes to the developer with this war and your app version.", FontKind::Body, 12.0, r.w - 2.0 * pad).first() {
        text(l, r.x + pad, r.y + 46.0 * s(), 12.0, MUTED, FontKind::Body, Align::Left);
    }
    let cw = (r.w - 2.0 * pad - 10.0 * s()) / 2.0;
    let cy = r.y + 64.0 * s();
    for (i, (label, bug)) in [("BUG REPORT", true), ("FEATURE IDEA", false)].iter().enumerate() {
        let cr = Rect::new(r.x + pad + i as f32 * (cw + 10.0 * s()), cy, cw, 34.0 * s());
        if widgets::chip(inp, cr, label, f.bug == *bug, if *bug { DANGER } else { GOOD }) && f.bug != *bug {
            f.bug = *bug;
            art::sfx("menu_tab");
        }
    }
    let hint = if f.bug { "What went wrong, and what were you doing?" } else { "What would you like the game to do?" };
    let fr = Rect::new(r.x + pad, cy + 50.0 * s(), r.w - 2.0 * pad, 48.0 * s());
    let mut send = widgets::text_field(inp, fr, &mut f.text, hint);
    text(&format!("{}/600", f.text.text.chars().count()), fr.x + fr.w, fr.y + fr.h + 12.0 * s(), 11.0, MUTED, FontKind::Body, Align::Right);
    let by = r.y + r.h - 58.0 * s();
    let bw = (r.w - 2.0 * pad - 12.0 * s()) / 2.0;
    let mut close = inp.take_back();
    if widgets::button(inp, Rect::new(r.x + pad, by, bw, 46.0 * s()), "CANCEL", Style::Ghost) {
        close = true;
    }
    let ok = f.text.text.trim().chars().count() >= 3;
    send |= widgets::button(inp, Rect::new(r.x + pad + bw + 12.0 * s(), by, bw, 46.0 * s()), "SEND", if ok { Style::Primary } else { Style::Ghost });
    if send && ok {
        g.emit(UiAction::Feedback { bug: f.bug, text: f.text.text.trim().to_string() });
        art::sfx("ui_confirm");
        g.push_notice(Notice { kind: NoticeKind::Good, title: "Thanks!".into(), body: if f.bug { "Bug report sent".into() } else { "Idea sent".into() }, focus: None });
        close = true;
    }
    if close {
        macroquad::miniquad::window::show_keyboard(false);
        inp.focused_field = None;
    } else {
        g.feedback = Some(f);
    }
    true
}

/// Host-only "DELETE WAR" menu row with an inline confirmation (second tap within the menu).
pub fn delete_row(g: &mut GameScreen, inp: &mut Input, view: &GameView, r: Rect) {
    if view.host != view.me {
        return;
    }
    let code = view.lobby.as_ref().map(|l| l.code.clone()).unwrap_or_else(|| view.code.clone());
    if !g.confirm_delete {
        if widgets::button(inp, r, "DELETE WAR", Style::Ghost) {
            g.confirm_delete = true;
            art::sfx("ui_error");
        }
        return;
    }
    let bw = (r.w - 8.0 * s()) / 2.0;
    if widgets::button(inp, Rect::new(r.x, r.y, bw, r.h), "KEEP IT", Style::Secondary) {
        g.confirm_delete = false;
    }
    if widgets::button(inp, Rect::new(r.x + bw + 8.0 * s(), r.y, bw, r.h), "DELETE FOR ALL", Style::Danger) && !code.is_empty() {
        g.confirm_delete = false;
        g.menu_open = false;
        g.emit(UiAction::DeleteGame { code });
    }
}

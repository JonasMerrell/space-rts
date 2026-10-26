//! Out-of-game screens: login, main menu (games list, new game, join by code), lobby.

use crate::backend::{Backend, UiAction};
use crate::cel;
use crate::input::Input;
use crate::model::*;
use crate::theme::*;
use crate::widgets::{self, Style, TextField};
use macroquad::prelude::*;

pub const FACTIONS: [&str; 4] = ["SOLAR UNION", "VOID CORSAIRS", "IRON CHOIR", "FREE MINERS"];
pub const BOT_LEVELS: [&str; 3] = ["EASY", "NORMAL", "HARD"];

/// A centred column layout helper: hands out rows top to bottom.
pub struct Column {
    pub x: f32,
    pub w: f32,
    pub y: f32,
}

impl Column {
    pub fn new(top: f32) -> Self {
        let w = (screen_width() - 40.0 * s()).min(460.0 * s());
        Column { x: (screen_width() - w) / 2.0, w, y: top }
    }
    pub fn row(&mut self, h_dp: f32) -> Rect {
        let r = Rect::new(self.x, self.y, self.w, h_dp * s());
        self.y += (h_dp + 14.0) * s();
        r
    }
    pub fn gap(&mut self, dp: f32) {
        self.y += dp * s();
    }
}

/// The title logo: a burst behind slanted inked letters.
pub fn logo(c: Vec2, size_dp: f32) {
    let t = get_time() as f32;
    cel::burst(c, size_dp * 2.6 * s(), 14, DANGER, t * 0.1);
    cel::burst(c, size_dp * 2.1 * s(), 14, ACCENT, -t * 0.1 + 0.2);
    inked(GAME_TITLE, c.x, c.y, size_dp, PAPER, Align::Center);
}

#[derive(Default)]
pub struct Login {
    pub user: TextField,
    pub pass: TextField,
    pub error: Option<String>,
    pub busy: bool,
}

impl Login {
    pub fn new() -> Self {
        Login { user: TextField::new(false), pass: TextField::new(true), ..Default::default() }
    }

    pub fn draw(&mut self, inp: &mut Input, be: &mut dyn Backend) {
        let h = screen_height();
        logo(vec2(screen_width() / 2.0, h * 0.2), 46.0);
        let mut col = Column::new(h * 0.38);
        text("PILOT NAME", col.x + 4.0 * s(), col.y, 18.0, MUTED, FontKind::Title, Align::Left);
        col.gap(14.0);
        let r = col.row(52.0);
        let mut go = widgets::text_field(inp, r, &mut self.user, "callsign");
        text("PASSWORD", col.x + 4.0 * s(), col.y, 18.0, MUTED, FontKind::Title, Align::Left);
        col.gap(14.0);
        let r = col.row(52.0);
        go |= widgets::text_field(inp, r, &mut self.pass, "secret");
        col.gap(6.0);
        let r = col.row(60.0);
        go |= widgets::button(inp, r, if self.busy { "LAUNCHING..." } else { "LOG IN!" }, Style::Primary);
        let r = col.row(50.0);
        let reg = widgets::button(inp, r, "NEW PILOT? REGISTER", Style::Secondary);
        if (go || reg) && !self.busy {
            if self.user.text.trim().is_empty() || self.pass.text.is_empty() {
                self.error = Some("Need a name and a password!".into());
            } else {
                self.busy = true;
                self.error = None;
                be.act(UiAction::Login { user: self.user.text.trim().to_string(), pass: self.pass.text.clone(), register: reg });
            }
        }
        if let Some(e) = &self.error {
            let r = col.row(56.0);
            cel::bubble(r, Some(vec2(r.center().x, r.y - 30.0 * s())), PAPER);
            text(e, r.center().x, r.center().y, 18.0, DANGER, FontKind::Body, Align::Center);
        }
    }
}

pub struct Menu {
    pub join: TextField,
    pub creating: bool,
    pub new_name: TextField,
    pub length_days: u32,
    pub bots: Vec<usize>,
    pub error: Option<String>,
    pub user: String,
    scroll: f32,
}

impl Menu {
    pub fn new(user: String) -> Self {
        let mut new_name = TextField::new(false);
        new_name.text = format!("{}'s war", user);
        Menu { join: TextField::code(), creating: false, new_name, length_days: 7, bots: vec![], error: None, user, scroll: 0.0 }
    }

    pub fn draw(&mut self, inp: &mut Input, be: &mut dyn Backend) {
        let w = screen_width();
        let top = 16.0 * s() + safe_top();
        inked(GAME_TITLE, w / 2.0, top + 22.0 * s(), 30.0, ACCENT, Align::Center);
        text(&format!("pilot: {}", self.user), w / 2.0, top + 52.0 * s(), 16.0, MUTED, FontKind::Body, Align::Center);
        if self.creating {
            return self.draw_create(inp, be, top + 80.0 * s());
        }
        let mut col = Column::new(top + 80.0 * s());
        inked("YOUR WARS", col.x, col.y + 12.0 * s(), 24.0, PAPER, Align::Left);
        col.gap(34.0);
        let games = be.my_games();
        if games.is_empty() {
            let r = col.row(70.0);
            cel::bubble(r, None, PAPER);
            text("No games yet. Start one or join a friend's!", r.center().x, r.center().y, 17.0, INK, FontKind::Body, Align::Center);
        }
        for g in games.iter().take(4) {
            let r = col.row(74.0);
            inp.block(r);
            cel::panel(r, if inp.holding(r) { PANEL_HI } else { PANEL });
            inked(&g.name.to_uppercase(), r.x + 16.0 * s(), r.y + 24.0 * s(), 22.0, PAPER, Align::Left);
            text(
                &format!("Day {}/{} · {} pilots · code {}", g.day, g.length_days, g.players, g.code),
                r.x + 16.0 * s(),
                r.y + 52.0 * s(),
                15.0,
                MUTED,
                FontKind::Body,
                Align::Left,
            );
            if g.unread > 0 {
                let c = vec2(r.x + r.w - 30.0 * s(), r.center().y);
                cel::burst(c, 22.0 * s(), 9, DANGER, 0.0);
                text(&g.unread.to_string(), c.x, c.y, 18.0, PAPER, FontKind::Title, Align::Center);
            }
            if inp.tapped(r) {
                inp.consume_tap();
                be.act(UiAction::OpenGame { code: g.code.clone() });
            }
        }
        col.gap(10.0);
        let r = col.row(60.0);
        if widgets::button(inp, r, "START A NEW WAR", Style::Primary) {
            self.creating = true;
        }
        col.gap(4.0);
        inked("JOIN WITH CODE", col.x, col.y + 12.0 * s(), 22.0, PAPER, Align::Left);
        col.gap(32.0);
        let r = col.row(56.0);
        let fw = r.w * 0.6;
        let mut go = widgets::text_field(inp, Rect::new(r.x, r.y, fw, r.h), &mut self.join, "e.g. K7QX");
        go |= widgets::button(inp, Rect::new(r.x + fw + 14.0 * s(), r.y, r.w - fw - 14.0 * s(), r.h), "JOIN!", Style::Secondary);
        if go {
            let code = self.join.text.trim().to_uppercase();
            if code.len() < 4 {
                self.error = Some("Codes are 4+ letters".into());
            } else {
                self.error = None;
                be.act(UiAction::JoinGame { code });
            }
        }
        if let Some(e) = &self.error {
            let r = col.row(50.0);
            cel::bubble(r, Some(vec2(r.x + 60.0 * s(), r.y - 26.0 * s())), PAPER);
            text(e, r.center().x, r.center().y, 17.0, DANGER, FontKind::Body, Align::Center);
        }
        let r = Rect::new(col.x, screen_height() - 70.0 * s() - safe_bottom(), col.w, 50.0 * s());
        if widgets::button(inp, r, "LOG OUT", Style::Ghost) {
            be.act(UiAction::Logout);
        }
        let _ = self.scroll;
    }

    fn draw_create(&mut self, inp: &mut Input, be: &mut dyn Backend, top: f32) {
        let mut col = Column::new(top);
        inked("NEW WAR", col.x, col.y + 12.0 * s(), 26.0, PAPER, Align::Left);
        col.gap(36.0);
        text("NAME", col.x + 4.0 * s(), col.y, 18.0, MUTED, FontKind::Title, Align::Left);
        col.gap(14.0);
        let r = col.row(52.0);
        widgets::text_field(inp, r, &mut self.new_name, "name your war");
        text("LENGTH (can be extended later)", col.x + 4.0 * s(), col.y, 18.0, MUTED, FontKind::Title, Align::Left);
        col.gap(14.0);
        let r = col.row(52.0);
        self.length_days = widgets::stepper(inp, r, &format!("{} DAYS", self.length_days), self.length_days, 2, 30);
        text("BOTS (fill empty seats)", col.x + 4.0 * s(), col.y, 18.0, MUTED, FontKind::Title, Align::Left);
        col.gap(14.0);
        let r = col.row(44.0);
        let cw = (r.w - 2.0 * 12.0 * s()) / 3.0;
        for (i, lvl) in BOT_LEVELS.iter().enumerate() {
            let n = self.bots.iter().filter(|&&b| b == i).count();
            let label = if n > 0 { format!("{lvl} ×{n}") } else { lvl.to_string() };
            let cr = Rect::new(r.x + i as f32 * (cw + 12.0 * s()), r.y, cw, r.h);
            if widgets::chip(inp, cr, &label, n > 0, [GOOD, ACCENT, DANGER][i]) {
                if self.bots.len() < 3 {
                    self.bots.push(i);
                } else {
                    self.bots.retain(|&b| b != i);
                }
            }
        }
        text("tap to add, tap again when full to clear", col.x + 4.0 * s(), col.y, 14.0, MUTED, FontKind::Body, Align::Left);
        col.gap(26.0);
        let r = col.row(60.0);
        if widgets::button(inp, r, "CREATE & GET CODE", Style::Primary) {
            be.act(UiAction::CreateGame {
                name: self.new_name.text.trim().to_string(),
                length_days: self.length_days,
                bots: self.bots.iter().map(|&b| BOT_LEVELS[b].to_lowercase()).collect(),
            });
            self.creating = false;
        }
        let r = col.row(50.0);
        if widgets::button(inp, r, "BACK", Style::Ghost) {
            self.creating = false;
        }
    }
}

#[derive(Default)]
pub struct Lobby {
    pub info: Option<LobbyInfo>,
    pub me: PlayerId,
    pub faction: usize,
}

impl Lobby {
    pub fn draw(&mut self, inp: &mut Input, be: &mut dyn Backend) {
        let Some(info) = self.info.clone() else {
            inked("CONNECTING...", screen_width() / 2.0, screen_height() / 2.0, 30.0, PAPER, Align::Center);
            return;
        };
        let top = 16.0 * s() + safe_top();
        let mut col = Column::new(top);
        inked(&info.name.to_uppercase(), screen_width() / 2.0, col.y + 20.0 * s(), 28.0, ACCENT, Align::Center);
        col.gap(48.0);
        // Join code in a big shout bubble.
        let r = col.row(110.0);
        cel::bubble(r, Some(vec2(r.x + r.w * 0.8, r.y + r.h + 26.0 * s())), PAPER);
        text("SHARE THIS CODE WITH FRIENDS", r.center().x, r.y + 24.0 * s(), 16.0, INK, FontKind::Title, Align::Center);
        let spaced: String = info.code.chars().map(|c| format!("{c} ")).collect();
        inked(spaced.trim_end(), r.center().x, r.y + 68.0 * s(), 54.0, ACCENT, Align::Center);
        col.gap(10.0);
        inked(&format!("PILOTS {}/{}", info.players.len(), info.max_players), col.x, col.y + 12.0 * s(), 22.0, PAPER, Align::Left);
        text(&format!("{} days", info.length_days), col.x + col.w, col.y + 12.0 * s(), 18.0, MUTED, FontKind::Body, Align::Right);
        col.gap(32.0);
        for i in 0..info.max_players {
            let r = col.row(56.0);
            match info.players.get(i) {
                Some(p) => {
                    cel::panel(r, PANEL);
                    let (l, d) = TEAMS[p.color % 4];
                    cel::body(vec2(r.x + 30.0 * s(), r.center().y), 14.0 * s(), l, d);
                    let me = if p.id == self.me { "  (you)" } else { "" };
                    let host = if p.id == info.host { "  ★ host" } else { "" };
                    inked(&format!("{}{}", p.name.to_uppercase(), me), r.x + 56.0 * s(), r.y + 20.0 * s(), 20.0, PAPER, Align::Left);
                    text(&format!("{}{}", p.faction, host), r.x + 56.0 * s(), r.y + 42.0 * s(), 14.0, MUTED, FontKind::Body, Align::Left);
                }
                None => {
                    cel::round_rect(r, 10.0 * s(), alpha(PAPER, 0.06));
                    text("waiting for a pilot...", r.center().x, r.center().y, 16.0, MUTED, FontKind::Body, Align::Center);
                }
            }
        }
        text("YOUR FACTION (looks only)", col.x + 4.0 * s(), col.y, 16.0, MUTED, FontKind::Title, Align::Left);
        col.gap(14.0);
        let r = col.row(40.0);
        let cw = (r.w - 12.0 * s()) / 2.0;
        let r2 = col.row(40.0);
        for (i, f) in FACTIONS.iter().enumerate() {
            let rr = if i < 2 { r } else { r2 };
            let cr = Rect::new(rr.x + (i % 2) as f32 * (cw + 12.0 * s()), rr.y, cw, rr.h);
            if widgets::chip(inp, cr, f, self.faction == i, ACCENT) {
                self.faction = i;
                be.act(UiAction::SetFaction(f.to_string()));
            }
        }
        let is_host = info.host == self.me;
        let bottom = screen_height() - safe_bottom() - 16.0 * s();
        let bw = (col.w - 14.0 * s()) / 2.0;
        let r = Rect::new(col.x, bottom - 60.0 * s(), col.w, 60.0 * s());
        if is_host {
            let full = info.players.len() >= info.max_players;
            if info.players.len() < info.max_players
                && widgets::button(inp, Rect::new(col.x, r.y - 64.0 * s(), bw, 50.0 * s()), "+ ADD BOT", Style::Secondary)
            {
                be.act(UiAction::AddBot { difficulty: "normal".into() });
            }
            if widgets::button(inp, Rect::new(col.x + bw + 14.0 * s(), r.y - 64.0 * s(), bw, 50.0 * s()), "LEAVE", Style::Ghost) {
                be.act(UiAction::LeaveLobby);
            }
            let label = if full { "LAUNCH!" } else { "LAUNCH NOW" };
            if widgets::button(inp, r, label, Style::Primary) {
                be.act(UiAction::StartGame);
            }
        } else {
            cel::bubble(Rect::new(col.x, r.y - 64.0 * s(), col.w, 50.0 * s()), None, PAPER);
            text("Waiting for the host to launch...", col.x + col.w / 2.0, r.y - 39.0 * s(), 17.0, INK, FontKind::Body, Align::Center);
            if widgets::button(inp, r, "LEAVE", Style::Ghost) {
                be.act(UiAction::LeaveLobby);
            }
        }
    }
}

/// Status-bar / notch allowance. Android draws us edge-to-edge under the status bar.
pub fn safe_top() -> f32 {
    if cfg!(target_os = "android") { 28.0 * s() } else { 0.0 }
}

pub fn safe_bottom() -> f32 {
    if cfg!(target_os = "android") { 20.0 * s() } else { 0.0 }
}

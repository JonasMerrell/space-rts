//! Out-of-game screens: login, main menu (games list, new game, join by code), lobby.

use crate::art;
use crate::backend::{Backend, UiAction};
use crate::cel;
use crate::input::Input;
#[allow(unused_imports)]
use crate::model::{Body, BodyKind};
use crate::theme::*;
use crate::widgets::{self, Style, TextField};
use macroquad::prelude::*;

/// Aesthetic factions = art skins (vanguard, hive, prism, junker).
pub const FACTIONS: [&str; 4] = ["VANGUARD NAVY", "HIVE BROOD", "PRISM COVENANT", "JUNKER CLANS"];
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

/// Title block: a lit planet from the game art over the wordmark.
pub fn logo(c: Vec2, size_dp: f32) {
    let t = get_time() as f32;
    let pc = c - vec2(0.0, size_dp * 1.9 * s());
    let body = Body {
        id: 3,
        name: String::new(),
        kind: BodyKind::Planet,
        parent: Some(0),
        radius: 0.0,
        soi: 0.0,
        owner: None,
        intel: None,
        art: Some("planet_terran".into()),
        orbit_path: vec![],
    };
    crate::game::map::draw_body(&body, pc, size_dp * 1.1 * s(), pc + vec2(-300.0, -200.0) * s(), None, t);
    text(GAME_TITLE, c.x, c.y, size_dp, PAPER, FontKind::Title, Align::Center);
    let w = measure(GAME_TITLE, FontKind::Title, size_dp).x;
    draw_rectangle(c.x - w / 2.0, c.y + size_dp * 0.62 * s(), w, 2.0 * s(), ACCENT);
    text("MULTIPLAYER ORBITAL STRATEGY", c.x, c.y + size_dp * 0.95 * s(), 13.0, MUTED, FontKind::Title, Align::Center);
}

#[derive(Default)]
pub struct Login {
    pub user: TextField,
    pub pass: TextField,
    pub server: TextField,
    pub error: Option<String>,
    pub busy: bool,
}

impl Login {
    pub fn new() -> Self {
        Login { user: TextField::new(false), pass: TextField::new(true), server: TextField::new(false), ..Default::default() }
    }

    pub fn draw(&mut self, inp: &mut Input, be: &mut dyn Backend) {
        let h = screen_height();
        if self.server.text.is_empty() && !self.server.focused {
            self.server.text = be.default_server();
        }
        if let Some(user) = be.auto_login() {
            // Staying logged in: no form, just a splash while the saved session reconnects.
            if self.user.text.is_empty() {
                self.user.text = user.clone();
            }
            logo(vec2(screen_width() / 2.0, h * 0.3), 44.0);
            let mut col = Column::new(h * 0.48);
            let r = col.row(56.0);
            cel::panel(r, PANEL);
            text(&format!("Signing in as {user}..."), r.center().x, r.center().y, 17.0, PAPER, FontKind::Body, Align::Center);
            if let Some(st) = be.connection_status() {
                let r = col.row(24.0);
                text(&st, r.center().x, r.center().y, 13.0, MUTED, FontKind::Body, Align::Center);
            }
            col.gap(10.0);
            let r = col.row(48.0);
            if widgets::button(inp, r, "USE ANOTHER ACCOUNT", Style::Ghost) {
                be.act(UiAction::Logout);
            }
            return;
        }
        logo(vec2(screen_width() / 2.0, h * 0.2), 44.0);
        let mut col = Column::new(h * 0.32);
        text("PILOT NAME", col.x + 4.0 * s(), col.y, 18.0, MUTED, FontKind::Title, Align::Left);
        col.gap(14.0);
        let r = col.row(52.0);
        let mut go = widgets::text_field(inp, r, &mut self.user, "callsign");
        text("PASSWORD", col.x + 4.0 * s(), col.y, 18.0, MUTED, FontKind::Title, Align::Left);
        col.gap(14.0);
        let r = col.row(52.0);
        go |= widgets::text_field(inp, r, &mut self.pass, "secret");
        text("SERVER", col.x + 4.0 * s(), col.y, 18.0, MUTED, FontKind::Title, Align::Left);
        col.gap(14.0);
        let r = col.row(44.0);
        go |= widgets::text_field(inp, r, &mut self.server, "host:port");
        col.gap(6.0);
        let r = col.row(60.0);
        go |= widgets::button(inp, r, if self.busy { "LAUNCHING..." } else { "LOG IN" }, Style::Primary);
        let r = col.row(50.0);
        let reg = widgets::button(inp, r, "NEW PILOT? REGISTER", Style::Secondary);
        if (go || reg) && !self.busy {
            if self.user.text.trim().is_empty() || self.pass.text.is_empty() {
                self.error = Some("Enter a name and a password".into());
            } else {
                self.busy = true;
                self.error = None;
                inp.focused_field = None;
                macroquad::miniquad::window::show_keyboard(false);
                be.act(UiAction::Login {
                    server: self.server.text.trim().to_string(),
                    user: self.user.text.trim().to_string(),
                    pass: self.pass.text.clone(),
                    register: reg,
                });
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
    /// index into `SPEEDS`
    pub speed: usize,
    pub bots: Vec<usize>,
    pub error: Option<String>,
    pub user: String,
    scroll: f32,
}

impl Menu {
    pub fn new(user: String) -> Self {
        let mut new_name = TextField::new(false);
        new_name.text = format!("{}'s war", user);
        Menu { join: TextField::code(), creating: false, new_name, length_days: 7, speed: 0, bots: vec![], error: None, user, scroll: 0.0 }
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
            cel::panel(r, PANEL);
            text("No games yet. Start one or join a friend's!", r.center().x, r.center().y, 16.0, MUTED, FontKind::Body, Align::Center);
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
                draw_circle(c.x, c.y, 13.0 * s(), DANGER);
                text(&g.unread.to_string(), c.x, c.y, 15.0, PAPER, FontKind::Title, Align::Center);
            }
            if inp.tapped(r) {
                inp.consume_tap();
                be.act(UiAction::OpenGame { code: g.code.clone() });
            }
        }
        col.gap(10.0);
        let r = col.row(60.0);
        if widgets::button(inp, r, "START A NEW WAR", Style::Primary) {
            art::sfx("menu_open");
            self.creating = true;
        }
        col.gap(4.0);
        inked("JOIN WITH CODE", col.x, col.y + 12.0 * s(), 22.0, PAPER, Align::Left);
        col.gap(32.0);
        let r = col.row(56.0);
        let fw = r.w * 0.6;
        let mut go = widgets::text_field(inp, Rect::new(r.x, r.y, fw, r.h), &mut self.join, "e.g. K7QX");
        go |= widgets::button(inp, Rect::new(r.x + fw + 14.0 * s(), r.y, r.w - fw - 14.0 * s(), r.h), "JOIN", Style::Secondary);
        if go {
            let code = self.join.text.trim().to_uppercase();
            if code.len() < 4 {
                self.error = Some("Codes are 4+ letters".into());
            } else {
                self.error = None;
                art::sfx("start_game");
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
        text("GAME SPEED", col.x + 4.0 * s(), col.y, 18.0, MUTED, FontKind::Title, Align::Left);
        col.gap(14.0);
        let r = col.row(44.0);
        let gap = 8.0 * s();
        let cw = (r.w - gap * (SPEEDS.len() - 1) as f32) / SPEEDS.len() as f32;
        for (i, &k) in SPEEDS.iter().enumerate() {
            let cr = Rect::new(r.x + i as f32 * (cw + gap), r.y, cw, r.h);
            if widgets::chip(inp, cr, &format!("{k}×"), self.speed == i, ACCENT) {
                art::sfx("toggle_on");
                self.speed = i;
            }
        }
        let real_h = self.length_days as f64 * 24.0 / SPEEDS[self.speed];
        let note = if SPEEDS[self.speed] == 1.0 { "real time".to_string() } else { format!("{} days of game time in {}", self.length_days, real_len(real_h)) };
        text(&note, col.x + 4.0 * s(), col.y, 14.0, MUTED, FontKind::Body, Align::Left);
        col.gap(26.0);
        text("BOTS (fill empty seats)", col.x + 4.0 * s(), col.y, 18.0, MUTED, FontKind::Title, Align::Left);
        col.gap(14.0);
        let r = col.row(44.0);
        let cw = (r.w - 2.0 * 12.0 * s()) / 3.0;
        for (i, lvl) in BOT_LEVELS.iter().enumerate() {
            let n = self.bots.iter().filter(|&&b| b == i).count();
            let label = if n > 0 { format!("{lvl} ×{n}") } else { lvl.to_string() };
            let cr = Rect::new(r.x + i as f32 * (cw + 12.0 * s()), r.y, cw, r.h);
            if widgets::chip(inp, cr, &label, n > 0, [GOOD, ACCENT, DANGER][i]) {
                art::sfx(if self.bots.len() < 3 { "toggle_on" } else { "toggle_off" });
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
                speed: SPEEDS[self.speed],
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

/// Status-bar / notch allowance. Android draws us edge-to-edge under the status bar.
pub fn safe_top() -> f32 {
    if cfg!(target_os = "android") { 28.0 * s() } else { 0.0 }
}

pub fn safe_bottom() -> f32 {
    if cfg!(target_os = "android") { 20.0 * s() } else { 0.0 }
}

/// Game-speed choices on the create screen (game seconds per real second).
pub const SPEEDS: [f64; 7] = [1.0, 2.0, 4.0, 8.0, 24.0, 72.0, 168.0];

/// "42 h" / "3.5 days" for a real-time duration in hours.
pub fn real_len(hours: f64) -> String {
    if hours < 48.0 {
        format!("{:.0} h", hours)
    } else {
        format!("{:.1} days", hours / 24.0).replace(".0 ", " ")
    }
}

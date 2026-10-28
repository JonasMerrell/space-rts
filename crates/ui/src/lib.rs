//! Space RTS client UI (style L, cel-shaded comic). The client crate owns the window/main loop
//! and calls `Ui::frame` once per frame with its `Backend`.

pub mod art;
pub mod backend;
pub mod cel;
pub mod demo;
pub mod game;
pub mod input;
pub mod model;
pub mod screens;
pub mod theme;
pub mod widgets;

pub use backend::{Backend, BackendEvent, ImpactPoll, PlanPoll, UiAction, WindowsPoll};

use game::GameScreen;
use input::Input;
use macroquad::prelude::*;
use screens::{Login, Menu};

pub enum Screen {
    Login(Login),
    Menu(Menu),
    Game(GameScreen),
}

pub struct Ui {
    pub screen: Screen,
    pub input: Input,
    pub user: String,
    pub me: model::PlayerId,
    /// The "update available" card was closed for this version.
    pub update_dismissed: Option<String>,
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

impl Ui {
    /// Loads fonts; call after the macroquad window exists.
    pub fn new() -> Self {
        theme::init();
        Ui { screen: Screen::Login(Login::new()), input: Input::new(), user: String::new(), me: 0, update_dismissed: None }
    }

    pub fn frame(&mut self, be: &mut dyn Backend) {
        theme::update_scale();
        for ev in be.poll() {
            self.on_event(ev, be);
        }
        self.input.begin_frame();
        clear_background(theme::BG);
        cel::backdrop();
        art::music(Some(if matches!(self.screen, Screen::Game(_)) { "orbit" } else { "title" }));
        match &mut self.screen {
            Screen::Login(l) => l.draw(&mut self.input, be),
            Screen::Menu(m) => m.draw(&mut self.input, be),
            Screen::Game(g) => {
                g.draw(&mut self.input, be);
            }
        }
        if !matches!(self.screen, Screen::Login(_)) {
            if let Some(st) = be.connection_status() {
                self.offline_banner(&st, be);
            }
        }
        if let Some(u) = be.update_available() {
            if self.update_dismissed.as_deref() != Some(u.latest.as_str()) {
                self.update_card(&u, be);
            }
        }
        self.input.draw_debug();
        // Screen changes driven directly by our own actions.
        if let Screen::Game(_) = self.screen {
            if be.view().is_none() {
                self.screen = Screen::Menu(Menu::new(self.user.clone()));
            }
        }
    }

    fn on_event(&mut self, ev: BackendEvent, _be: &mut dyn Backend) {
        match ev {
            BackendEvent::LoggedIn { user } => {
                self.user = user.clone();
                self.screen = Screen::Menu(Menu::new(user));
            }
            BackendEvent::LoginFailed(e) => {
                if let Screen::Login(l) = &mut self.screen {
                    l.busy = false;
                    l.error = Some(e);
                } else {
                    let mut l = Login::new();
                    l.error = Some(e);
                    self.screen = Screen::Login(l);
                }
            }
            // The waiting period is shown on the game screen (map + lobby overlay), see game::lobby.
            BackendEvent::Lobby(info) => match &mut self.screen {
                Screen::Game(g) => {
                    if info.players.len() > g.lobby_pilots && g.lobby_pilots > 0 {
                        art::sfx("lobby_join");
                    }
                    g.lobby_pilots = info.players.len();
                }
                _ => {
                    let mut g = GameScreen::new();
                    g.lobby_pilots = info.players.len();
                    self.screen = Screen::Game(g);
                }
            },
            BackendEvent::LoggedOut => self.to_login(),
            BackendEvent::LobbyFailed(e) => {
                let mut m = Menu::new(self.user.clone());
                m.error = Some(e);
                self.screen = Screen::Menu(m);
            }
            BackendEvent::GameStarted => {
                if !matches!(self.screen, Screen::Game(_)) {
                    self.screen = Screen::Game(GameScreen::new());
                }
            }
            BackendEvent::Notice(n) => {
                if let Screen::Game(g) = &mut self.screen {
                    g.push_notice(n);
                }
            }
        }
    }

    /// "Reconnecting..." strip at the bottom of the screen, with a retry button.
    fn offline_banner(&mut self, status: &str, be: &mut dyn Backend) {
        use theme::*;
        let h = 44.0 * s();
        let y = screen_height() - h - screens::safe_bottom() - 90.0 * s();
        let r = Rect::new(12.0 * s(), y, screen_width() - 24.0 * s(), h);
        self.input.block(r);
        cel::round_rect(r, 8.0 * s(), Color::new(0.35, 0.05, 0.08, 0.95));
        cel::round_rect_lines(r, 8.0 * s(), 1.2 * s(), DANGER);
        let dots = ".".repeat((get_time() * 2.0) as usize % 4);
        text(&format!("{}{}", status.trim_end_matches("..."), if status.ends_with("...") { dots.as_str() } else { "" }), r.x + 12.0 * s(), r.center().y, 14.0, PAPER, FontKind::Body, Align::Left);
        let br = Rect::new(r.x + r.w - 86.0 * s(), r.y + 6.0 * s(), 80.0 * s(), h - 12.0 * s());
        if widgets::button(&mut self.input, br, "RETRY", widgets::Style::Secondary) {
            be.act(UiAction::Reconnect);
        }
    }

    /// "A new version is available" over everything, until UPDATE/RELOAD or LATER (or back).
    /// Play isn't blocked: LATER closes it for this version until the app restarts.
    fn update_card(&mut self, u: &model::AppUpdate, be: &mut dyn Backend) {
        use theme::*;
        let (sw, sh) = (screen_width(), screen_height());
        self.input.block(Rect::new(0.0, 0.0, sw, sh));
        draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.0, 0.0, 0.0, 0.55));
        let w = (sw - 32.0 * s()).min(440.0 * s());
        let pad = 18.0 * s();
        let body = if u.reload {
            format!("Version {} of Hohmann's Reach is out (this page is {}). Reload to get it; your wars carry on.", u.latest, u.current)
        } else {
            format!("Version {} of Hohmann's Reach is out (you have {}). Download and install it to keep up; your wars carry on.", u.latest, u.current)
        };
        let lines = wrap(&body, FontKind::Body, 15.0, w - 2.0 * pad);
        let line_h = 21.0 * s();
        let h = pad + 30.0 * s() + lines.len() as f32 * line_h + 16.0 * s() + 44.0 * s() + pad;
        let r = Rect::new((sw - w) / 2.0, ((sh - h) / 2.0).max(screens::safe_top() + 8.0 * s()), w, h);
        cel::panel(r, PANEL);
        cel::round_rect_lines(r, 8.0 * s(), 1.5 * s(), ACCENT);
        let mut y = r.y + pad + 12.0 * s();
        text("NEW VERSION AVAILABLE", r.x + pad, y, 20.0, ACCENT, FontKind::Title, Align::Left);
        y += 18.0 * s() + line_h / 2.0;
        for l in &lines {
            text(l, r.x + pad, y, 15.0, PAPER, FontKind::Body, Align::Left);
            y += line_h;
        }
        let bh = 44.0 * s();
        let by = r.y + r.h - pad - bh;
        let bw = (w - 2.0 * pad - 12.0 * s()) / 2.0;
        let later = Rect::new(r.x + pad, by, bw, bh);
        let get = Rect::new(r.x + r.w - pad - bw, by, bw, bh);
        if widgets::button(&mut self.input, get, if u.reload { "RELOAD" } else { "UPDATE" }, widgets::Style::Primary) {
            be.act(UiAction::GetUpdate);
            if !u.reload {
                self.update_dismissed = Some(u.latest.clone());
            }
        }
        if widgets::button(&mut self.input, later, "LATER", widgets::Style::Secondary) || self.input.take_back() {
            self.update_dismissed = Some(u.latest.clone());
        }
    }

    /// Logged out / back to the title screen.
    pub fn to_login(&mut self) {
        self.screen = Screen::Login(Login::new());
    }
}

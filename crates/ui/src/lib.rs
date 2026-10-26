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

pub use backend::{Backend, BackendEvent, UiAction};

use game::GameScreen;
use input::Input;
use macroquad::prelude::*;
use screens::{Lobby, Login, Menu};

pub enum Screen {
    Login(Login),
    Menu(Menu),
    Lobby(Lobby),
    Game(GameScreen),
}

pub struct Ui {
    pub screen: Screen,
    pub input: Input,
    pub user: String,
    pub me: model::PlayerId,
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
        Ui { screen: Screen::Login(Login::new()), input: Input::new(), user: String::new(), me: 0 }
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
            Screen::Lobby(l) => l.draw(&mut self.input, be),
            Screen::Game(g) => {
                g.draw(&mut self.input, be);
            }
        }
        // Screen changes driven directly by our own actions.
        if let Screen::Game(_) = self.screen {
            if be.view().is_none() {
                self.screen = Screen::Menu(Menu::new(self.user.clone()));
            }
        }
    }

    fn on_event(&mut self, ev: BackendEvent, be: &mut dyn Backend) {
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
            BackendEvent::Lobby(info) => {
                if let Screen::Lobby(l) = &mut self.screen {
                    if l.info.as_ref().is_some_and(|old| info.players.len() > old.players.len()) {
                        art::sfx("lobby_join");
                    }
                    l.info = Some(info);
                } else {
                    let me = be.view().map(|v| v.me).unwrap_or(self.me);
                    self.screen = Screen::Lobby(Lobby { info: Some(info), me, faction: 0 });
                }
            }
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

    /// Logged out / back to the title screen.
    pub fn to_login(&mut self) {
        self.screen = Screen::Login(Login::new());
    }
}

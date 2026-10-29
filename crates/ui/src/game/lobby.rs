//! Waiting period before the host starts the clock: the system is already generated and
//! shown on the map, players join by code, the host adds/removes bots and presses START.

use super::GameScreen;
use crate::art;
use crate::backend::{Backend, UiAction};
use crate::cel;
use crate::input::Input;
use crate::model::*;
use crate::screens::{safe_bottom, safe_top, FACTIONS};
use crate::theme::*;
use crate::widgets::{self, Style};
use macroquad::prelude::*;

use super::map::{self, Camera};

fn margin() -> f32 {
    12.0 * s()
}

/// Ring + "YOUR HOME" tag on the planet we'll start on (it can move until the start, as pilots join).
pub fn draw_home(be: &dyn Backend, view: &GameView, cam: &Camera, t: f64) {
    let Some(home) = view.home.and_then(|h| view.body(h)) else { return };
    let c = cam.to_screen(be.body_pos(home.id, t));
    let r = map::body_radius_px(home, cam) + 10.0 * s();
    let (light, _) = TEAMS[view.players.iter().find(|p| p.id == view.me).map(|p| p.color).unwrap_or(0) % TEAMS.len()];
    let pulse = 1.0 + 0.06 * (get_time() as f32 * 3.0).sin();
    draw_circle_lines(c.x, c.y, r * pulse, 2.5 * s(), light);
    let label = "YOUR HOME";
    let w = measure(label, FontKind::Title, 13.0).x + 16.0 * s();
    // above the planet's name label, which sits just over the body
    let body_r = map::body_radius_px(home, cam);
    let tag = Rect::new(c.x - w / 2.0, c.y - body_r - 54.0 * s(), w, 22.0 * s());
    cel::round_rect(tag, 11.0 * s(), alpha(light, 0.25));
    cel::round_rect_lines(tag, 11.0 * s(), 1.2 * s(), light);
    text(label, tag.center().x, tag.center().y, 13.0, PAPER, FontKind::Title, Align::Center);
}

/// Top banner + pilot roster. Everything the lobby needs; the rest of the HUD is hidden.
pub fn overlay(g: &mut GameScreen, inp: &mut Input, view: &GameView, info: &LobbyInfo) {
    let host = info.host == view.me;
    let w = screen_width();
    let x = margin();
    let cw = w - 2.0 * margin();

    // -- banner
    let top = safe_top() + margin();
    let banner = Rect::new(x, top, if wide() { cw.min(640.0 * s()) } else { cw }, 112.0 * s());
    inp.block(banner);
    cel::panel(banner, PANEL);
    let status = if host { "WAITING TO START" } else { "WAITING FOR HOST" };
    let dots = ".".repeat((get_time() * 1.5) as usize % 4);
    text(&format!("{status}{dots}"), banner.x + 14.0 * s(), banner.y + 20.0 * s(), 16.0, ACCENT, FontKind::Title, Align::Left);
    text(&info.name, banner.x + 14.0 * s(), banner.y + 42.0 * s(), 14.0, MUTED, FontKind::Body, Align::Left);
    text("JOIN CODE", banner.x + 14.0 * s(), banner.y + 66.0 * s(), 12.0, MUTED, FontKind::Title, Align::Left);
    let spaced: String = info.code.chars().map(|c| format!("{c} ")).collect();
    text(spaced.trim_end(), banner.x + 14.0 * s(), banner.y + 92.0 * s(), 30.0, PAPER, FontKind::Title, Align::Left);
    let len = if info.speed > 1.0 {
        format!("{} DAYS · {}× ({} REAL)", info.length_days, info.speed, crate::screens::real_len(info.length_days as f64 * 24.0 / info.speed).to_uppercase())
    } else {
        format!("{} DAYS", info.length_days)
    };
    text(
        &format!("{}/{} PILOTS{} · {len}", info.players.len(), info.max_players, if info.teams >= 2 { format!(" · {}", crate::screens::team_label(info.max_players, info.teams)) } else { String::new() }),
        banner.x + banner.w - 14.0 * s(),
        banner.y + 92.0 * s(),
        13.0,
        MUTED,
        FontKind::Title,
        Align::Right,
    );
    let by = banner.y + 28.0 * s();
    let bx = banner.x + banner.w - 34.0 * s();
    let unread = view.chat.len().saturating_sub(g.chat_seen);
    if widgets::round_button(inp, vec2(bx - 50.0 * s(), by), 20.0 * s(), "@chat", PAPER, if g.chat_open { 0 } else { unread }) {
        g.chat_open = !g.chat_open;
        art::sfx(if g.chat_open { "menu_open" } else { "menu_close" });
        g.chat_seen = view.chat.len();
    }
    if widgets::round_button(inp, vec2(bx, by), 20.0 * s(), "@menu", ACCENT, 0) {
        g.menu_open = !g.menu_open;
        art::sfx(if g.menu_open { "menu_open" } else { "menu_close" });
    }
    if g.menu_open {
        let pad = 10.0 * s();
        let bh = 44.0 * s();
        let host = info.host == view.me;
        let n = if host { 3.0 } else { 2.0 };
        let r = Rect::new(w - margin() - 290.0 * s(), banner.y + banner.h + 8.0 * s(), 290.0 * s(), n * bh + (n + 1.0) * pad);
        inp.block(r);
        cel::panel(r, alpha(PANEL, 0.97));
        let row = |i: f32| Rect::new(r.x + pad, r.y + pad + i * (bh + pad), r.w - 2.0 * pad, bh);
        art::sound_toggles(inp, row(0.0));
        if host {
            super::endgame::delete_row(g, inp, view, row(1.0));
        }
        if widgets::button(inp, row(n - 1.0), "LEAVE TO MENU", Style::Ghost) {
            g.menu_open = false;
            g.emit(UiAction::BackToMenu);
        }
        if inp.tap.is_some_and(|p| !r.contains(p)) {
            g.confirm_delete = false;
        }
    }
    if g.chat_open {
        return;
    }

    // -- roster sheet (collapsible so the map can be looked at)
    // up to 8 pilots: compact one-line rows past four so the map stays visible on a phone
    let compact = info.max_players > 4;
    let row_h = if compact { 34.0 } else { 44.0 } * s();
    let gap = if compact { 5.0 } else { 8.0 } * s();
    let bottom = screen_height() - safe_bottom() - margin();
    let btn_h = 52.0 * s();
    let footer = btn_h + gap;
    let header = 40.0 * s();
    let body = if g.lobby_roster { info.max_players as f32 * (row_h + gap) + 26.0 * s() + 36.0 * s() + gap } else { 0.0 };
    // computer mode: roster in the right-hand column
    let (sx, sw) = if wide() { (w - margin() - side_w(), side_w()) } else { (x, cw) };
    let sheet = Rect::new(sx, bottom - footer - body - header - gap, sw, header + body + footer + gap);
    inp.block(sheet);
    cel::panel(sheet, PANEL);
    let hr = Rect::new(sheet.x, sheet.y, sheet.w, header);
    text(&format!("PILOTS {}/{}", info.players.len(), info.max_players), hr.x + 14.0 * s(), hr.center().y, 16.0, PAPER, FontKind::Title, Align::Left);
    text(if g.lobby_roster { "HIDE" } else { "SHOW" }, hr.x + hr.w - 14.0 * s(), hr.center().y, 13.0, MUTED, FontKind::Title, Align::Right);
    if inp.tapped(hr) {
        inp.consume_tap();
        g.lobby_roster = !g.lobby_roster;
        art::sfx(if g.lobby_roster { "menu_open" } else { "menu_close" });
    }
    let mut y = sheet.y + header;
    if g.lobby_roster {
        // team games list the pilots team by team
        let mut order: Vec<&Player> = info.players.iter().collect();
        order.sort_by_key(|p| (p.team, p.id));
        let team_cap = if info.teams >= 2 { info.max_players / info.teams } else { info.max_players };
        for i in 0..info.max_players {
            let r = Rect::new(sheet.x + 10.0 * s(), y, sheet.w - 20.0 * s(), row_h);
            y += row_h + gap;
            let Some(p) = order.get(i).copied() else {
                cel::round_rect(r, 8.0 * s(), alpha(PAPER, 0.05));
                text("open slot · share the code", r.center().x, r.center().y, 14.0, MUTED, FontKind::Body, Align::Center);
                continue;
            };
            cel::round_rect(r, 8.0 * s(), alpha(PAPER, 0.06));
            let c = vec2(r.x + 22.0 * s(), r.center().y);
            let (l, d) = TEAMS[p.color % TEAMS.len()];
            cel::body(c, if compact { 11.0 } else { 13.0 } * s(), l, d);
            if let Some(t) = art::tex(&format!("sprites/emblems/{}.png", art::skin_of(&p.faction))) {
                art::draw_centered(&t, c, vec2(20.0, 20.0) * s() * if compact { 0.85 } else { 1.0 }, 0.0, WHITE);
            }
            let me = if p.id == view.me { " (you)" } else { "" };
            let role = if p.id == info.host {
                "host"
            } else if !p.online {
                "bot"
            } else {
                "pilot"
            };
            if compact {
                text(&format!("{}{me}", p.name), r.x + 40.0 * s(), r.center().y, 14.0, PAPER, FontKind::Title, Align::Left);
            } else {
                text(&format!("{}{me}", p.name), r.x + 44.0 * s(), r.y + 15.0 * s(), 16.0, PAPER, FontKind::Title, Align::Left);
                text(&format!("{} · {role}", p.faction.to_lowercase()), r.x + 44.0 * s(), r.y + 32.0 * s(), 12.0, MUTED, FontKind::Body, Align::Left);
            }
            let inset = if compact { 4.0 } else { 7.0 } * s();
            let mut right = r.x + r.w - 6.0 * s();
            if host && !p.online && p.id != info.host {
                let br = Rect::new(right - 86.0 * s(), r.y + inset, 86.0 * s(), r.h - 2.0 * inset);
                right = br.x - 6.0 * s();
                if widgets::button(inp, br, "REMOVE", Style::Ghost) {
                    g.emit(UiAction::RemoveBot { player: p.id });
                }
            }
            // team chip: tap your own (or, as host, a bot's) to move to the next team with room
            if let Some(team) = p.team {
                let tr = Rect::new(right - 64.0 * s(), r.y + inset, 64.0 * s(), r.h - 2.0 * inset);
                let movable = p.id == view.me || (host && !p.online);
                if widgets::chip(inp, tr, &format!("TEAM {}", team + 1), true, if movable { ACCENT } else { MUTED }) && movable {
                    let n = info.teams.max(1) as u8;
                    let next = (1..n).map(|k| (team + k) % n).find(|&t| info.players.iter().filter(|q| q.team == Some(t)).count() < team_cap);
                    if let Some(t) = next {
                        art::sfx("menu_select");
                        g.emit(UiAction::SetTeam { player: p.id, team: t });
                    }
                }
            }
        }
        text("YOUR FACTION (looks only)", sheet.x + 14.0 * s(), y + 10.0 * s(), 12.0, MUTED, FontKind::Title, Align::Left);
        y += 26.0 * s();
        let mine = info.players.iter().find(|p| p.id == view.me).map(|p| art::skin_of(&p.faction)).unwrap_or("vanguard");
        let fw = (sheet.w - 20.0 * s() - 3.0 * 6.0 * s()) / 4.0;
        for (i, f) in FACTIONS.iter().enumerate() {
            let cr = Rect::new(sheet.x + 10.0 * s() + i as f32 * (fw + 6.0 * s()), y, fw, 34.0 * s());
            let short = f.split(' ').next().unwrap_or(f);
            if widgets::chip(inp, cr, short, art::skin_of(f) == mine, ACCENT) {
                art::sfx("menu_select");
                g.emit(UiAction::SetFaction(f.to_string()));
            }
        }
    }

    // -- footer: host starts the clock, everyone else waits
    let fr = Rect::new(sheet.x + 10.0 * s(), sheet.y + sheet.h - btn_h - gap, sheet.w - 20.0 * s(), btn_h);
    if host {
        let bw = (fr.w - gap) * 0.36;
        let full = info.players.len() >= info.max_players;
        let add = Rect::new(fr.x, fr.y, bw, fr.h);
        if full {
            cel::round_rect(add, 8.0 * s(), alpha(PAPER, 0.05));
            text("FULL", add.center().x, add.center().y, 16.0, MUTED, FontKind::Title, Align::Center);
        } else if widgets::button(inp, add, "+ BOT", Style::Secondary) {
            g.emit(UiAction::AddBot { difficulty: "normal".into() });
        }
        let start = Rect::new(fr.x + bw + gap, fr.y, fr.w - bw - gap, fr.h);
        if info.players.len() < 2 {
            cel::round_rect(start, 8.0 * s(), alpha(PAPER, 0.05));
            text("NEED 2 PILOTS", start.center().x, start.center().y, 16.0, MUTED, FontKind::Title, Align::Center);
        } else if widgets::button(inp, start, "START GAME", Style::Primary) {
            art::sfx("start_game");
            g.emit(UiAction::StartGame);
        }
    } else {
        let host_name = info.players.iter().find(|p| p.id == info.host).map(|p| p.name.as_str()).unwrap_or("the host");
        cel::round_rect(fr, 8.0 * s(), alpha(PAPER, 0.05));
        text(&format!("The clock starts when {host_name} presses START"), fr.center().x, fr.center().y, 14.0, MUTED, FontKind::Body, Align::Center);
    }
}

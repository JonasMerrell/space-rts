//! HUD, scrubber, bottom sheets, planner/node-editor overlays, chat and toasts.

use super::details::Detail;
use super::map::{self, Camera};
use super::{GameScreen, Handle, Mode, NodeEdit, SCRUB_RANGES};
use crate::art;
use crate::backend::{Backend, UiAction};
use crate::cel;
use crate::input::Input;
use crate::model::*;
use crate::screens::{safe_bottom, safe_top};
use crate::theme::*;
use crate::widgets::{self, Style};
use macroquad::prelude::*;

const SCRUB_H: f32 = 74.0;
const HUD_H: f32 = 86.0;

fn margin() -> f32 {
    12.0 * s()
}

fn hud_bottom() -> f32 {
    safe_top() + HUD_H * s()
}

fn scrub_top() -> f32 {
    screen_height() - safe_bottom() - SCRUB_H * s()
}

// ---------------------------------------------------------------- HUD

fn icon_matter(c: Vec2, r: f32) {
    // Little inked cube.
    let p = |x: f32, y: f32| c + vec2(x, y) * r;
    let o = 2.0 * s();
    cel::round_rect(Rect::new(c.x - r - o, c.y - r - o, 2.0 * (r + o), 2.0 * (r + o)), 3.0 * s(), INK);
    draw_rectangle(c.x - r, c.y - r, 2.0 * r, 2.0 * r, NEUTRAL.0);
    draw_triangle(p(1.0, -1.0), p(1.0, 1.0), p(-1.0, 1.0), NEUTRAL.1);
}

fn icon_energy(c: Vec2, r: f32) {
    let pts = [vec2(0.2, -1.2), vec2(-0.7, 0.15), vec2(0.0, 0.15), vec2(-0.25, 1.2), vec2(0.75, -0.25), vec2(0.05, -0.25)];
    let q: Vec<Vec2> = pts.iter().map(|p| c + *p * r).collect();
    let g: Vec<Vec2> = pts.iter().map(|p| c + *p * (r + 3.0 * s())).collect();
    for (poly, col) in [(&g, INK), (&q, ACCENT)] {
        draw_triangle(poly[0], poly[1], poly[2], col);
        draw_triangle(poly[0], poly[2], poly[5], col);
        draw_triangle(poly[2], poly[3], poly[4], col);
        draw_triangle(poly[2], poly[4], poly[5], col);
    }
}

fn icon_fuel(c: Vec2, r: f32) {
    let o = 2.5 * s();
    draw_circle(c.x, c.y + r * 0.3, r * 0.8 + o, INK);
    draw_triangle(c + vec2(0.0, -r * 1.3 - o * 1.5), c + vec2(-r * 0.8 - o, r * 0.3), c + vec2(r * 0.8 + o, r * 0.3), INK);
    draw_circle(c.x, c.y + r * 0.3, r * 0.8, SUN_SHADE);
    draw_triangle(c + vec2(0.0, -r * 1.3), c + vec2(-r * 0.8, r * 0.3), c + vec2(r * 0.8, r * 0.3), SUN_SHADE);
    draw_circle(c.x - r * 0.3, c.y + r * 0.1, r * 0.2, Color::new(1.0, 1.0, 1.0, 0.6));
}

pub fn hud(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) {
    let w = screen_width();
    let top = safe_top() + margin();
    let res = view.resources;
    let btn_space = 3.0 * 52.0 * s();
    let avail = w - 2.0 * margin() - btn_space;
    let cw = ((avail - 2.0 * 8.0 * s()) / 3.0).min(if wide() { 150.0 * s() } else { f32::MAX });
    let ch = 44.0 * s();
    type Item<'a> = (&'a str, f64, f64, fn(Vec2, f32));
    let items: [Item; 3] =
        [("MATTER", res.matter, res.matter_rate, icon_matter), ("ENERGY", res.energy, res.energy_rate, icon_energy), ("FUEL", res.fuel, res.fuel_rate, icon_fuel)];
    for (i, (label, v, rate, icon)) in items.iter().enumerate() {
        let r = Rect::new(margin() + i as f32 * (cw + 8.0 * s()), top, cw, ch);
        inp.block(r);
        let open = g.res_open == Some(i);
        cel::panel(r, if open { PANEL_HI } else { PANEL });
        if inp.tapped(r) {
            inp.consume_tap();
            g.res_open = if open { None } else { Some(i) };
            art::sfx(if open { "menu_close" } else { "menu_open" });
        }
        let ic = vec2(r.x + 18.0 * s(), r.center().y);
        match art::tex(&format!("sprites/icons/{}.png", label.to_lowercase())) {
            Some(t) => art::draw_centered(&t, ic, vec2(26.0, 26.0) * s(), 0.0, WHITE),
            None => icon(ic, 8.0 * s()),
        }
        inked(&fmt_amount(*v), r.x + 34.0 * s(), r.y + 17.0 * s(), 19.0, PAPER, Align::Left);
        let rate_s = format!("{}{}/h", if *rate >= 0.0 { "+" } else { "" }, fmt_amount(*rate));
        let col = if *rate >= 0.0 { GOOD } else { DANGER };
        text(&rate_s, r.x + 34.0 * s(), r.y + 34.0 * s(), 12.0, col, FontKind::Title, Align::Left);
    }
    let bx = w - margin() - 22.0 * s();
    let unread = view.chat.len().saturating_sub(g.chat_seen);
    if widgets::round_button(inp, vec2(bx - 50.0 * s(), top + ch / 2.0), 21.0 * s(), "@chat", PAPER, if g.chat_open { 0 } else { unread }) {
        g.chat_open = !g.chat_open;
        art::sfx(if g.chat_open { "menu_open" } else { "menu_close" });
        g.chat_seen = view.chat.len();
    }
    super::report::hud_button(g, inp, be, vec2(bx - 100.0 * s(), top + ch / 2.0));
    if widgets::round_button(inp, vec2(bx, top + ch / 2.0), 21.0 * s(), "@menu", ACCENT, 0) {
        g.menu_open = !g.menu_open;
        art::sfx(if g.menu_open { "menu_open" } else { "menu_close" });
    }
    if g.menu_open {
        game_menu(g, inp, view, Rect::new(w - margin() - 290.0 * s(), top + ch + 30.0 * s(), 290.0 * s(), 0.0));
    }
    // Clock line.
    let y = top + ch + 16.0 * s();
    let clock = if view.speed > 1.0 { format!("{} · {}×", fmt_clock(view.now), view.speed) } else { fmt_clock(view.now) };
    inked(&clock, margin(), y, 17.0, PAPER, Align::Left);
    let left = view.ends_at - view.now;
    text(&format!("WAR ENDS IN {}", fmt_dur(left)), w - margin(), y, 14.0, MUTED, FontKind::Title, Align::Right);
}

fn game_menu(g: &mut GameScreen, inp: &mut Input, view: &GameView, at: Rect) {
    let host = view.host == view.me;
    let items = if host { 9 } else { 5 };
    let bh = 44.0 * s();
    let pad = 10.0 * s();
    let r = Rect::new(at.x, at.y, at.w, pad + items as f32 * (bh + pad));
    inp.block(r);
    cel::panel(r, alpha(PANEL, 0.97));
    let row = |i: usize| Rect::new(r.x + pad, r.y + pad + i as f32 * (bh + pad), r.w - 2.0 * pad, bh);
    art::sound_toggles(inp, row(0));
    let on = map::enemy_paths();
    if widgets::button(inp, row(1), if on { "ENEMY PATHS: SHOWN" } else { "ENEMY PATHS: HIDDEN" }, Style::Secondary) {
        map::set_enemy_paths(!on);
        art::sfx(if on { "toggle_off" } else { "toggle_on" });
    }
    if widgets::button(inp, row(2), "RESEARCH", Style::Secondary) {
        g.research_open = true;
        g.menu_open = false;
    }
    if host && widgets::button(inp, row(3), "EXTEND WAR +1 DAY", Style::Secondary) {
        g.emit(UiAction::ExtendGame { days: 1 });
        g.menu_open = false;
    }
    // Host: game speed slider (time multiplier) for a shorter war; sent when the finger lifts.
    if host {
        use crate::screens::{real_len, SPEEDS};
        let live = if view.speed > 0.0 { view.speed } else { 1.0 };
        let speed = g.speed_drag.unwrap_or(live);
        let lr = row(4);
        text(&format!("GAME SPEED {speed}×"), lr.x + 4.0 * s(), lr.y + 14.0 * s(), 14.0, PAPER, FontKind::Title, Align::Left);
        let real_h = (view.ends_at - view.now).max(0.0) / 3600.0 / speed;
        text(&format!("ends in {} real", real_len(real_h)), lr.x + 4.0 * s(), lr.y + 34.0 * s(), 12.0, MUTED, FontKind::Body, Align::Left);
        let (v, released) = widgets::log_slider(inp, row(5), speed, SPEEDS[0], SPEEDS[SPEEDS.len() - 1], &SPEEDS, false);
        g.speed_drag = Some(v).filter(|&v| v != live || inp.held.is_some());
        if released && v != live {
            g.emit(UiAction::SetSpeed { speed: v });
        }
    }
    if widgets::button(inp, row(items - if host { 3 } else { 2 }), "BUG REPORT / IDEA", Style::Secondary) {
        g.feedback = Some(super::endgame::FeedbackForm::new());
        g.menu_open = false;
        art::sfx("menu_open");
    }
    if host {
        super::endgame::delete_row(g, inp, view, row(items - 2));
    }
    if widgets::button(inp, row(items - 1), "LEAVE TO MENU", Style::Ghost) {
        g.menu_open = false;
        g.emit(UiAction::BackToMenu);
    }
    // Tap elsewhere closes it.
    if inp.tap.is_some_and(|p| !r.contains(p)) {
        g.menu_open = false;
    }
    if !g.menu_open {
        g.confirm_delete = false;
    }
}

// ---------------------------------------------------------------- Scrubber

/// RE-CENTRE pill under the HUD when the camera stopped following the selection.
/// Camera pills under the HUD (top left): RE-CENTRE when the camera stopped following the
/// selection, ZOOM TO when a body is selected, SYSTEM VIEW when zoomed in past the whole system.
pub fn camera_buttons(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) {
    let Some(cam) = g.cam else { return };
    let fit = map::Camera::fit(view);
    let mut x = margin();
    let y = hud_bottom() + 6.0 * s();
    let mut pill = |inp: &mut Input, label: &str| {
        let w = measure(label, FontKind::Title, 14.0).x + 28.0 * s();
        let r = Rect::new(x, y, w, 32.0 * s());
        x += w + 8.0 * s();
        widgets::button(inp, r, label, Style::Secondary)
    };
    if !g.follow && g.selected.is_some() && pill(inp, "RE-CENTRE") {
        g.follow = true;
        g.system_view = false;
    }
    if let Some(Focus::Body(id)) = g.selected {
        if let Some(b) = view.body(id) {
            let goal = zoom_to_body(view, b);
            // only offer it when it would actually zoom (not already about there)
            if (cam.zoom / goal - 1.0).abs() > 0.15 && pill(inp, &format!("ZOOM TO {}", b.name.to_uppercase())) {
                g.zoom_goal = Some(goal);
                g.follow = true;
                g.system_view = false;
                art::sfx("menu_select");
            }
        }
    }
    if cam.zoom > fit.zoom * 1.6 && pill(inp, "SYSTEM VIEW") {
        g.zoom_goal = Some(fit.zoom);
        g.system_view = true;
        g.follow = false;
        art::sfx("menu_select");
    }
    let _ = be;
}

/// px per km that frames `b` with its moons (or its sphere of influence) in the free map area.
fn zoom_to_body(view: &GameView, b: &Body) -> f64 {
    let moons = view
        .bodies
        .iter()
        .filter(|m| m.parent == Some(b.id))
        .flat_map(|m| m.orbit_path.iter())
        .map(|p| p[0].hypot(p[1]))
        .fold(0.0f64, f64::max);
    let reach = if moons > 0.0 { moons * 1.25 } else { b.soi.max(b.radius * 20.0) * 0.8 };
    // room: the map band not covered by panels (phone: above the bottom sheet; computer: left of the side panel)
    let room = if wide() { (screen_width() - side_w()).min(screen_height()) } else { screen_width().min(screen_height() * 0.5) };
    (room as f64 * 0.42) / reach.max(1.0)
}

pub fn scrubber(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) {
    let w = screen_width();
    let span = g.scrub_span;
    let r = Rect::new(margin(), scrub_top(), w - 2.0 * margin(), SCRUB_H * s() - margin());
    inp.block(r);
    cel::panel(r, PANEL);
    let btn_w = 54.0 * s();
    let track = Rect::new(r.x + 14.0 * s(), r.y + r.h * 0.42, r.w - 2.0 * btn_w - 40.0 * s(), 8.0 * s());
    let x_of = |dt: f64| track.x + track.w * (dt / span).clamp(0.0, 1.0) as f32;
    let (tick, label_every) = scrub_ticks(span);

    // Drag anywhere on the bar to scrub.
    let grab = Rect::new(track.x - 12.0 * s(), r.y, track.w + 24.0 * s(), r.h);
    if let Some(p) = inp.held {
        if inp.gesture_on_ui() && grab.contains(p) {
            let new = (((p.x - track.x) / track.w).clamp(0.0, 1.0) as f64) * span;
            if (new / tick).floor() != (g.scrub / tick).floor() {
                art::sfx("slider_tick");
            }
            g.scrub = new;
            g.ranges_open = false;
        }
    }

    widgets::bar(track, (g.scrub / span) as f32, ACCENT);
    // Not simulated yet (the forecast fills in on a worker thread): dim the rest of the bar.
    let until = be.forecast_until() - view.now;
    if until < span {
        let x0 = x_of(until.max(0.0));
        let pulse = 0.25 + 0.15 * (get_time() * 4.0).sin() as f32;
        draw_rectangle(x0, track.y, track.x + track.w - x0, track.h, alpha(PANEL, 0.6 + pulse));
        text("simulating...", track.x + track.w, track.y - 9.0 * s(), 11.0, MUTED, FontKind::Body, Align::Right);
    }
    let mut k = 0.0;
    while k <= span + 1.0 {
        let x = x_of(k);
        let tall = (k / label_every).fract().abs() < 1e-6 || ((k / label_every).fract() - 1.0).abs() < 1e-6;
        draw_line(x, track.y + track.h + 3.0 * s(), x, track.y + track.h + if tall { 10.0 } else { 6.0 } * s(), 2.0 * s(), MUTED);
        if tall {
            let l = if k == 0.0 { "NOW".to_string() } else { format!("+{}", tick_label(k)) };
            text(&l, x, track.y + track.h + 20.0 * s(), 12.0, MUTED, FontKind::Title, Align::Center);
        }
        k += tick;
    }
    // Transfer windows from the active plan.
    if let Mode::Plan(p) = &g.mode {
        for &wt in &p.windows {
            let dt = wt - view.now;
            if (0.0..=span).contains(&dt) {
                let c = vec2(x_of(dt), track.y - 9.0 * s());
                cel::diamond(c, 6.0 * s(), if (wt - p.depart_t).abs() < 60.0 { ACCENT } else { GOOD });
            }
        }
        let a = p.arrive_t - view.now;
        if (0.0..=span).contains(&a) {
            let c = vec2(x_of(a), track.y - 9.0 * s());
            cel::burst(c, 8.0 * s(), 6, DANGER, 0.0);
        }
    }
    // Knob.
    let kc = vec2(x_of(g.scrub), track.y + track.h / 2.0);
    draw_circle(kc.x, kc.y, 11.0 * s(), alpha(ACCENT, 0.35));
    draw_circle(kc.x, kc.y, 7.0 * s(), PAPER);
    if g.scrub > 1.0 && !g.ranges_open {
        let label = format!("+{}", fmt_dur(g.scrub));
        let wl = measure(&label, FontKind::Title, 14.0).x + 14.0 * s();
        let br = Rect::new((kc.x - wl / 2.0).clamp(r.x, r.x + r.w - wl), r.y - 34.0 * s(), wl, 24.0 * s());
        cel::bubble(br, Some(vec2(kc.x, kc.y - 12.0 * s())), PAPER);
        text(&label, br.center().x, br.center().y, 14.0, INK, FontKind::Title, Align::Center);
    }
    // Range picker: the button shows the current range; tapping it opens a row of chips above.
    let rr = Rect::new(r.x + r.w - 2.0 * btn_w - 14.0 * s(), r.y + 9.0 * s(), btn_w, r.h - 18.0 * s());
    let cur = SCRUB_RANGES.iter().find(|x| x.0 == span).map(|x| x.1).unwrap_or("?");
    if widgets::button(inp, rr, cur, if g.ranges_open { Style::Primary } else { Style::Secondary }) {
        g.ranges_open = !g.ranges_open;
        art::sfx(if g.ranges_open { "menu_open" } else { "menu_close" });
    }
    if g.ranges_open {
        let n = SCRUB_RANGES.len() as f32;
        let gap = 8.0 * s();
        let cw = 50.0 * s();
        let pw = n * cw + (n + 1.0) * gap;
        let pr = Rect::new((r.x + r.w - pw).max(r.x), r.y - 54.0 * s(), pw, 46.0 * s());
        inp.block(pr);
        cel::panel(pr, PANEL);
        for (i, &(secs, label)) in SCRUB_RANGES.iter().enumerate() {
            let cr = Rect::new(pr.x + gap + i as f32 * (cw + gap), pr.y + 7.0 * s(), cw, pr.h - 14.0 * s());
            if widgets::chip(inp, cr, label, secs == span, ACCENT) {
                art::sfx("ui_click");
                g.scrub_span = secs;
                g.scrub = g.scrub.min(secs);
                g.ranges_open = false;
            }
        }
    }
    let nr = Rect::new(r.x + r.w - btn_w - 8.0 * s(), r.y + 9.0 * s(), btn_w, r.h - 18.0 * s());
    if widgets::button(inp, nr, "NOW", if g.scrub > 1.0 { Style::Primary } else { Style::Ghost }) {
        g.scrub = 0.0;
    }
}

/// (small tick, labelled tick) spacing in seconds for a scrubber span.
fn scrub_ticks(span: f64) -> (f64, f64) {
    const H: f64 = 3600.0;
    match span {
        x if x <= H => (5.0 * 60.0, 15.0 * 60.0),
        x if x <= 10.0 * H => (H, 2.0 * H),
        x if x <= 24.0 * H => (2.0 * H, 6.0 * H),
        x if x <= 72.0 * H => (6.0 * H, 24.0 * H),
        _ => (12.0 * H, 24.0 * H),
    }
}

fn tick_label(secs: f64) -> String {
    let m = (secs / 60.0).round() as u64;
    if m % 1440 == 0 {
        format!("{}d", m / 1440)
    } else if m % 60 == 0 {
        format!("{}h", m / 60)
    } else {
        format!("{}m", m)
    }
}

pub fn prediction_banner(scrub: f64, simulating: bool) {
    let w = screen_width();
    let y = hud_bottom() + 6.0 * s();
    // Thin amber frame around the map = "you are looking at the future".
    let (t, b) = (hud_bottom(), scrub_top() - 8.0 * s());
    let k = 2.0 * s();
    let col = alpha(ACCENT, 0.55);
    draw_rectangle(0.0, t, w, k, col);
    draw_rectangle(0.0, b - k, w, k, col);
    draw_rectangle(0.0, t, k, b - t, col);
    draw_rectangle(w - k, t, k, b - t, col);
    let label = if simulating { format!("SIMULATING  +{}...", fmt_dur(scrub)) } else { format!("PREDICTED  +{}", fmt_dur(scrub)) };
    let lw = measure(&label, FontKind::Title, 15.0).x + 26.0 * s();
    let r = Rect::new((w - lw) / 2.0, y, lw, 26.0 * s());
    cel::round_rect(r, r.h / 2.0, alpha(ACCENT, 0.92));
    text(&label, r.center().x, r.center().y + s(), 15.0, hex(0x1a1430), FontKind::Title, Align::Center);
}

// ---------------------------------------------------------------- Bottom sheet

pub(super) fn sheet_rect(h_dp: f32) -> Rect {
    let w = screen_width();
    let h = h_dp * s();
    if wide() {
        // Computer mode: a panel on the right under the HUD, leaving the map wide open.
        let h = h.min(scrub_top() - hud_bottom() - 26.0 * s());
        return Rect::new(w - margin() - side_w(), hud_bottom() + 8.0 * s(), side_w(), h);
    }
    Rect::new(margin(), scrub_top() - h - 18.0 * s(), w - 2.0 * margin(), h)
}

pub fn sheet(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) {
    if let Some(res) = g.res_open {
        return resource_sheet(g, inp, be, view, res);
    }
    match g.mode.clone() {
        Mode::Plan(plan) => plan_sheet(g, inp, be, view, &plan),
        Mode::Nodes(ne) => node_sheet(g, inp, be, view, ne),
        Mode::Target(ship) => {
            if let Some(sh) = view.ship(ship) {
                ship_sheet(g, inp, be, view, sh, true);
            }
        }
        Mode::Aim(ast) => aim_bar(g, inp, view, ast),
        Mode::Windows(_) => super::windows::windows_sheet(g, inp, be, view),
        Mode::Impact(ip) => impact_sheet(g, inp, view, &ip),
        Mode::Planning { .. } | Mode::Impacting { .. } | Mode::Intercepting { .. } => {
            let name = match g.mode {
                Mode::Planning { target, .. } | Mode::Impacting { target, .. } => view.body(target).map(|b| b.name.to_uppercase()).unwrap_or_default(),
                Mode::Intercepting { target, .. } => view.ship(target).map(|s| s.name.to_uppercase()).unwrap_or_default(),
                _ => String::new(),
            };
            let label = if matches!(g.mode, Mode::Intercepting { .. }) {
                format!("PLOTTING INTERCEPT OF {name}...")
            } else if matches!(g.mode, Mode::Impacting { .. }) {
                format!("COMPUTING IMPACT ON {name}...")
            } else {
                format!("PLOTTING COURSE TO {name}...")
            };
            let w = measure(&label, FontKind::Title, 16.0).x + 32.0 * s();
            let r = Rect::new((screen_width() - w) / 2.0, scrub_top() - 70.0 * s(), w, 40.0 * s());
            cel::round_rect(r, r.h / 2.0, PANEL);
            cel::round_rect_lines(r, r.h / 2.0, 1.5 * s(), ACCENT);
            text(&label, r.center().x, r.center().y + s(), 16.0, ACCENT, FontKind::Title, Align::Center);
        }
        Mode::Idle if g.fleet.is_some() => {
            if g.fleet.as_ref().is_some_and(|f| f.opened_on != g.selected) {
                g.fleet = None;
            } else {
                super::fleet::fleet_sheet(g, inp, view);
            }
        }
        Mode::Idle if g.stations_for.is_some() => {
            let body = g.stations_for.unwrap();
            if g.selected != Some(Focus::Body(body)) {
                g.stations_for = None;
            } else {
                super::research::stations_sheet(g, inp, be, view, body, sheet_rect(332.0));
            }
        }
        Mode::Idle => match g.selected {
            Some(Focus::Ship(id)) => {
                if let Some(sh) = view.ship(id) {
                    ship_sheet(g, inp, be, view, sh, false);
                }
            }
            Some(Focus::Body(id)) => {
                if let Some(b) = view.body(id) {
                    // your bodies, and any body where your shipyard station lets you build
                    match be.body_info(id).filter(|i| b.owner == Some(view.me) || !i.buildable.is_empty()) {
                        Some(info) => owned_body_sheet(g, inp, be, view, b, &info),
                        None => body_sheet(g, inp, be, view, b, be.stations(id).is_some_and(|v| v.present)),
                    }
                }
            }
            None => {
                // Research shortcut floating above the scrubber.
                let c = vec2(screen_width() - margin() - 24.0 * s(), scrub_top() - 34.0 * s());
                if widgets::round_button(inp, c, 22.0 * s(), "@research", ACCENT, 0) {
                    g.research_open = true;
                    art::sfx("menu_open");
                }
            }
        },
    }
}

/// "Aim at a planet" entry on our own asteroid's panel. Returns the button's bottom edge.
fn aim_button(g: &mut GameScreen, inp: &mut Input, view: &GameView, b: &Body, r: Rect) {
    if b.kind != BodyKind::Asteroid || b.owner != Some(view.me) {
        return;
    }
    let label = if r.w < 200.0 * s() { "AIM ROCK" } else { "AIM AT A PLANET" };
    if widgets::button(inp, r, label, Style::Danger) {
        art::sfx("ui_click");
        g.mode = Mode::Aim(b.id);
        g.selected = Some(Focus::Body(b.id));
    }
}

fn aim_bar(g: &mut GameScreen, inp: &mut Input, view: &GameView, ast: BodyId) {
    let r = sheet_rect(100.0);
    inp.block(r);
    cel::panel(r, PANEL);
    let name = view.body(ast).map(|b| b.name.to_uppercase()).unwrap_or_default();
    inked(&format!("AIM {name}"), r.x + 16.0 * s(), r.y + 22.0 * s(), 20.0, DANGER, Align::Left);
    text("Tap the planet or moon to crash it into", r.x + 16.0 * s(), r.y + 44.0 * s(), 13.0, PAPER, FontKind::Body, Align::Left);
    let br = Rect::new(r.x + r.w - 120.0 * s(), r.y + r.h - 50.0 * s(), 108.0 * s(), 40.0 * s());
    if widgets::button(inp, br, "CANCEL", Style::Ghost) {
        g.mode = Mode::Idle;
    }
}

fn severity_label(sev: f64) -> (String, Color) {
    if sev >= 4.0 {
        ("SHATTERS THE PLANET".into(), DANGER)
    } else if sev >= 1.0 {
        ("WIPES UPGRADES + EVERYTHING IN ORBIT".into(), DANGER)
    } else {
        (format!("DAMAGES ~{:.0}% OF UPGRADES", (sev * 100.0).clamp(1.0, 99.0)), ACCENT)
    }
}

fn impact_sheet(g: &mut GameScreen, inp: &mut Input, view: &GameView, ip: &ImpactPreview) {
    let r = sheet_rect(214.0);
    inp.block(r);
    cel::panel(r, PANEL);
    let ast = view.body(ip.asteroid).map(|b| b.name.to_uppercase()).unwrap_or_default();
    let tgt = view.body(ip.target).map(|b| b.name.to_uppercase()).unwrap_or_default();
    inked(&format!("{ast} AT {tgt}"), r.x + 16.0 * s(), r.y + 22.0 * s(), 20.0, DANGER, Align::Left);
    let hits = ip.impact_t.is_some();
    let cols: [(&str, String); 3] = [
        ("IMPACT", ip.impact_t.map(|t| format!("in {}", fmt_dur(t - view.now))).unwrap_or_else(|| "MISSES".into())),
        ("SPEED", format!("{:.1} km/s", ip.impact_speed)),
        ("ENERGY", format!("{:.1e} J", ip.impact_energy)),
    ];
    let cw = (r.w - 32.0 * s()) / 3.0;
    for (i, (k, v)) in cols.iter().enumerate() {
        let x = r.x + 16.0 * s() + i as f32 * cw;
        text(k, x, r.y + 48.0 * s(), 12.0, MUTED, FontKind::Title, Align::Left);
        inked(v, x, r.y + 68.0 * s(), 17.0, PAPER, Align::Left);
    }
    if hits {
        let (label, col) = severity_label(ip.severity);
        text(&label, r.x + 16.0 * s(), r.y + 94.0 * s(), 14.0, col, FontKind::Title, Align::Left);
    } else {
        text("This push doesn't hit. Try another target or wait for a better window.", r.x + 16.0 * s(), r.y + 94.0 * s(), 13.0, MUTED, FontKind::Body, Align::Left);
    }
    let fuel_ok = ip.fuel <= ip.fuel_available;
    let cost_ok = affordable(view, &ip.drive_cost);
    let mut line = format!(
        "Burn {} starting in {} · dv {:.1} km/s · fuel {:.0}/{:.0}",
        fmt_dur(ip.burn_s),
        fmt_dur((ip.burn_t - view.now).max(0.0)),
        ip.dv,
        ip.fuel,
        ip.fuel_available
    );
    if ip.drive_cost.matter > 0.0 {
        line += &format!(" · drive {}", cost_line(&ip.drive_cost));
    }
    for (k, l) in wrap(&line, FontKind::Body, 12.0, r.w - 32.0 * s()).iter().take(2).enumerate() {
        text(l, r.x + 16.0 * s(), r.y + 116.0 * s() + k as f32 * 16.0 * s(), 12.0, if fuel_ok && cost_ok { PAPER } else { DANGER }, FontKind::Body, Align::Left);
    }
    let ok = hits && fuel_ok && cost_ok;
    let by = r.y + r.h - 54.0 * s();
    let bw = (r.w - 3.0 * 12.0 * s()) / 2.0;
    if widgets::button(inp, Rect::new(r.x + 12.0 * s(), by, bw, 44.0 * s()), "LAUNCH PUSH", if ok { Style::Danger } else { Style::Ghost }) {
        if ok {
            art::sfx("transfer_scheduled");
            g.emit(UiAction::AimAsteroid { body: ip.asteroid, target: ip.target });
            g.push_notice(Notice { kind: NoticeKind::Battle, title: format!("{ast} is on its way"), body: format!("Impact on {tgt} in {}", fmt_dur(ip.impact_t.unwrap_or(view.now) - view.now)), focus: Some(Focus::Body(ip.target)) });
            g.mode = Mode::Idle;
        } else {
            art::sfx("ui_error");
        }
    }
    if widgets::button(inp, Rect::new(r.x + 24.0 * s() + bw, by, bw, 44.0 * s()), "CANCEL", Style::Ghost) {
        g.mode = Mode::Idle;
    }
}

pub fn impact_world(ip: &ImpactPreview, be: &dyn Backend, view: &GameView, cam: &Camera, t: f64) {
    let pts = poly(cam, &ip.path, f64::MIN);
    cel::inked_path(&pts, 4.0 * s(), DANGER);
    if let Some(&(_, d)) = ip.path.points.first() {
        cel::diamond(cam.to_screen(d), 8.0 * s(), DANGER);
    }
    if let (Some(it), Some(b)) = (ip.impact_t, view.body(ip.target)) {
        let c = cam.to_screen(be.body_pos(ip.target, it));
        let r = map::body_radius_px(b, cam);
        cel::burst(c, r + 14.0 * s(), 10, alpha(DANGER, 0.75), get_time() as f32 * 0.5);
    }
    if let Some(sp) = ip.path.sample(t) {
        draw_circle(cam.to_screen(sp).x, cam.to_screen(sp).y, 5.0 * s(), alpha(NEUTRAL.0, 0.8));
    }
}

/// Where matter / energy / fuel come from: per owned body, plus refining and other spending.
fn resource_sheet(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView, res: usize) {
    let (name, stock, net) = match res {
        0 => ("MATTER", view.resources.matter, view.resources.matter_rate),
        1 => ("ENERGY", view.resources.energy, view.resources.energy_rate),
        _ => ("FUEL", view.resources.fuel, view.resources.fuel_rate),
    };
    // (label, per hour, body to jump to)
    let mut rows: Vec<(String, f64, Option<BodyId>)> = vec![];
    let mut refining = 0.0;
    for b in view.bodies.iter().filter(|b| b.owner == Some(view.me)) {
        let Some(info) = be.body_info(b.id) else { continue };
        let v = [info.matter_rate, info.energy_rate, info.fuel_rate][res.min(2)];
        refining += info.fuel_rate * 0.5;
        if v.abs() > 0.05 {
            rows.push((b.name.clone(), v, Some(b.id)));
        }
    }
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    if res == 1 && refining > 0.05 {
        // sim: refining costs 0.5 energy per tonne of fuel
        rows.push(("Fuel refining".into(), -refining, None));
    }
    let listed: f64 = rows.iter().map(|r| r.1).sum();
    if (net - listed).abs() > 0.5 {
        rows.push((if res == 2 { "Ships refuelling / other".into() } else { "Other (stations, upkeep)".into() }, net - listed, None));
    }

    let row_h = 40.0 * s();
    let shown = rows.len().min(7);
    let r = sheet_rect(120.0 + shown as f32 * 40.0);
    inp.block(r);
    cel::panel(r, alpha(PANEL, 0.98));
    let (x, w) = (r.x + 16.0 * s(), r.w - 32.0 * s());
    inked(&format!("{name} SOURCES"), x, r.y + 22.0 * s(), 20.0, ACCENT, Align::Left);
    text(&format!("stock {}", fmt_amount(stock)), x, r.y + 44.0 * s(), 13.0, MUTED, FontKind::Title, Align::Left);
    if close_x(inp, r) {
        g.res_open = None;
        return;
    }
    let mut y = r.y + 62.0 * s();
    let fmt = |v: f64| format!("{}{}/h", if v >= 0.0 { "+" } else { "" }, fmt_amount(v));
    if rows.is_empty() {
        text("Nothing produces this yet. Upgrade a planet you own.", x, y + 14.0 * s(), 14.0, MUTED, FontKind::Body, Align::Left);
    }
    for (label, v, body) in rows.iter().take(shown) {
        let rr = Rect::new(r.x + 8.0 * s(), y, r.w - 16.0 * s(), row_h - 4.0 * s());
        cel::round_rect(rr, 6.0 * s(), Color::new(1.0, 1.0, 1.0, if body.is_some() && inp.holding(rr) { 0.12 } else { 0.05 }));
        if let Some(b) = body.and_then(|id| view.body(id)) {
            let c = vec2(rr.x + 20.0 * s(), rr.center().y);
            map::draw_body(b, c, 10.0 * s(), c + vec2(-80.0, -50.0), view.color_of(b.owner), get_time() as f32);
        }
        text(label, rr.x + 40.0 * s(), rr.center().y, 15.0, PAPER, FontKind::Body, Align::Left);
        text(&fmt(*v), rr.x + rr.w - 10.0 * s(), rr.center().y, 15.0, if *v >= 0.0 { GOOD } else { DANGER }, FontKind::Title, Align::Right);
        if let Some(id) = body {
            if inp.tapped(rr) {
                inp.consume_tap();
                g.res_open = None;
                g.mode = Mode::Idle;
                g.focus(be, Focus::Body(*id), view.now + g.scrub);
                return;
            }
        }
        y += row_h;
    }
    draw_line(x, y + 4.0 * s(), x + w, y + 4.0 * s(), 1.0, BORDER);
    text("NET", x, y + 24.0 * s(), 14.0, MUTED, FontKind::Title, Align::Left);
    text(&fmt(net), x + w, y + 24.0 * s(), 17.0, if net >= 0.0 { GOOD } else { DANGER }, FontKind::Title, Align::Right);
}

fn affordable(view: &GameView, c: &Cost) -> bool {
    view.resources.matter >= c.matter && view.resources.energy >= c.energy
}

fn cost_line(c: &Cost) -> String {
    format!("{}M  {}E  {}", fmt_amount(c.matter), fmt_amount(c.energy), fmt_dur(c.seconds))
}

/// Our own planet/moon: upgrades and shipyard tabs, build queue, power in orbit.
fn owned_body_sheet(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView, b: &Body, info: &BodyInfo) {
    let r = sheet_rect(332.0);
    if super::details::draw(g, inp, be, view, info, r) {
        return;
    }
    inp.block(r);
    cel::panel(r, PANEL);
    let c = vec2(r.x + 30.0 * s(), r.y + 30.0 * s());
    map::draw_body(b, c, 14.0 * s(), c + vec2(-100.0, -60.0), view.color_of(b.owner), get_time() as f32);
    inked(&b.name.to_uppercase(), r.x + 56.0 * s(), r.y + 22.0 * s(), 22.0, PAPER, Align::Left);
    let mine: f64 = info.power.iter().filter(|p| p.0 == view.me).map(|p| p.1).sum();
    let enemy: f64 = info.power.iter().filter(|p| p.0 != view.me).map(|p| p.1).sum();
    orbit_strip(be, inp, b, r);
    let mut sub = format!("YOURS · power {:.0}", mine.max(0.0));
    if enemy > 0.0 {
        sub += &format!(" vs {:.0} hostile", enemy);
    }
    text(&sub, r.x + 56.0 * s(), r.y + 44.0 * s(), 13.0, if enemy > mine { DANGER } else { MUTED }, FontKind::Title, Align::Left);
    let rates = format!("+{}M +{}E +{}F /h · fuel {}", fmt_amount(info.matter_rate), fmt_amount(info.energy_rate), fmt_amount(info.fuel_rate), fmt_amount(info.fuel));
    text(&rates, r.x + 16.0 * s(), r.y + 66.0 * s(), 13.0, PAPER, FontKind::Body, Align::Left);
    if close_x(inp, r) {
        g.selected = None;
        return;
    }

    // Tabs (+ the aim control on our asteroids).
    let ty = r.y + 82.0 * s();
    // Our asteroids can be aimed at planets (button sits under the close X).
    aim_button(g, inp, view, b, Rect::new(r.x + r.w - 126.0 * s(), r.y + 40.0 * s(), 110.0 * s(), 28.0 * s()));
    let tw = ((r.w - 32.0 * s() - 20.0 * s()) / 3.0).min(116.0 * s());
    for (i, label) in ["UPGRADES", "SHIPYARD", "STATIONS"].iter().enumerate() {
        let tr = Rect::new(r.x + 16.0 * s() + i as f32 * (tw + 10.0 * s()), ty, tw, 30.0 * s());
        if widgets::chip(inp, tr, label, g.body_tab == i, ACCENT) && g.body_tab != i {
            if i == 2 {
                // Stations open their own sheet (shared with bodies we don't own).
                g.stations_for = Some(b.id);
            } else {
                g.body_tab = i;
            }
            art::sfx("menu_tab");
        }
    }
    let grid = Rect::new(r.x + 12.0 * s(), ty + 42.0 * s(), r.w - 24.0 * s(), r.h - 42.0 * s() - 82.0 * s() - 34.0 * s());
    let cols = 2;
    let gap = 8.0 * s();
    let cw = (grid.w - gap) / cols as f32;
    let cell = |i: usize, rows: usize| {
        let ch = (grid.h - gap * (rows as f32 - 1.0)) / rows as f32;
        Rect::new(grid.x + (i % cols) as f32 * (cw + gap), grid.y + (i / cols) as f32 * (ch + gap), cw, ch)
    };
    if g.body_tab == 0 {
        let rows = info.upgrades.len().div_ceil(cols).max(1);
        for (i, u) in info.upgrades.iter().enumerate() {
            let cr = cell(i, rows);
            cel::round_rect(cr, 6.0 * s(), Color::new(1.0, 1.0, 1.0, 0.05));
            text(&u.name.to_uppercase(), cr.x + 10.0 * s(), cr.y + cr.h * 0.32, 14.0, PAPER, FontKind::Title, Align::Left);
            for k in 0..u.max {
                let pc = vec2(cr.x + 14.0 * s() + k as f32 * 12.0 * s(), cr.y + cr.h * 0.7);
                draw_circle(pc.x, pc.y, 4.0 * s(), if k < u.level { ACCENT } else { Color::new(1.0, 1.0, 1.0, 0.18) });
            }
            if let Some(cost) = &u.next {
                let ok = affordable(view, cost);
                let br = Rect::new(cr.x + cr.w - 44.0 * s(), cr.y + (cr.h - 32.0 * s()) / 2.0, 36.0 * s(), 32.0 * s());
                let cx = cr.x + 14.0 * s() + u.max as f32 * 12.0 * s();
                // Narrow cell: matter + energy only (the queue shows build time).
                let short = format!("{}M {}E", fmt_amount(cost.matter), fmt_amount(cost.energy));
                text(&short, cx, cr.y + cr.h * 0.7, 11.0, if ok { MUTED } else { DANGER }, FontKind::Body, Align::Left);
                // can't afford it yet: queue it, it starts once income covers it
                if widgets::button(inp, br, "+", if ok { Style::Primary } else { Style::Secondary }) {
                    art::sfx(if ok { "purchase" } else { "menu_select" });
                    g.emit(UiAction::Upgrade { body: b.id, upgrade: u.name.clone() });
                    if !ok {
                        g.hint = Some(("Queued: starts when you can afford it".into(), get_time()));
                    }
                }
            } else {
                text("MAX", cr.x + cr.w - 12.0 * s(), cr.center().y, 13.0, GOOD, FontKind::Title, Align::Right);
            }
            // Tap the rest of the cell for what it does.
            if inp.tapped(cr) {
                inp.consume_tap();
                art::sfx("menu_select");
                g.detail = Some(Detail::Upgrade { body: b.id, name: u.name.clone() });
            }
        }
    } else if info.buildable.is_empty() {
        text("No shipyard here yet. Upgrade Shipyard to build ships.", grid.x + 4.0 * s(), grid.y + 16.0 * s(), 14.0, MUTED, FontKind::Body, Align::Left);
    } else {
        let rows = info.buildable.len().div_ceil(cols).max(1);
        let player = view.player(view.me);
        let skin = player.map(|p| art::skin_of(&p.faction)).unwrap_or("vanguard");
        for (i, (class, cost)) in info.buildable.iter().enumerate() {
            let cr = cell(i, rows);
            cel::round_rect(cr, 6.0 * s(), Color::new(1.0, 1.0, 1.0, 0.05));
            let ic = vec2(cr.x + 22.0 * s(), cr.center().y);
            match art::ship_tex(*class, skin, player.map(|p| p.color)) {
                Some(t) => art::draw_centered(&t, ic, vec2(34.0, 34.0) * s(), -0.5, WHITE),
                None => cel::ship(ic, vec2(1.0, -0.5), 10.0 * s(), PAPER),
            }
            let ok = affordable(view, cost);
            text(&class.label().to_uppercase(), cr.x + 44.0 * s(), cr.y + cr.h * 0.34, 14.0, PAPER, FontKind::Title, Align::Left);
            text(&cost_line(cost), cr.x + 44.0 * s(), cr.y + cr.h * 0.7, 11.0, if ok { MUTED } else { DANGER }, FontKind::Body, Align::Left);
            let br = Rect::new(cr.x + cr.w - 44.0 * s(), cr.y + (cr.h - 32.0 * s()) / 2.0, 36.0 * s(), 32.0 * s());
            if widgets::button(inp, br, "+", if ok { Style::Primary } else { Style::Secondary }) {
                art::sfx(if ok { "purchase" } else { "menu_select" });
                g.emit(UiAction::Build { body: b.id, class: *class });
                if !ok {
                    g.hint = Some(("Queued: starts when you can afford it".into(), get_time()));
                }
            }
            if inp.tapped(cr) {
                inp.consume_tap();
                art::sfx("menu_select");
                g.detail = Some(Detail::Ship { body: b.id, class: *class });
            }
        }
    }
    // Build queue.
    let qy = r.y + r.h - 22.0 * s();
    let empty = info.queue.is_empty() && info.waiting.is_empty();
    let q = if empty {
        "Queue empty · tap an item for details".to_string()
    } else {
        // what each lane is working on now first
        let mut items: Vec<String> = info.queue.iter().filter(|q| q.start_t <= view.now).chain(info.queue.iter().filter(|q| q.start_t > view.now)).take(if info.waiting.is_empty() { 3 } else { 2 }).map(|q| format!("{} {}", q.label, fmt_dur(q.done_t - view.now))).collect();
        if !info.waiting.is_empty() {
            items.push(format!("{} waiting", info.waiting.len()));
        }
        format!("Queue: {}", items.join(" · "))
    };
    let qr = Rect::new(r.x + 8.0 * s(), qy - 12.0 * s(), r.w - 170.0 * s(), 24.0 * s());
    if !empty {
        cel::round_rect(qr, 6.0 * s(), Color::new(1.0, 1.0, 1.0, if inp.holding(qr) { 0.12 } else { 0.05 }));
    }
    let q = if empty { q } else { format!("{q} ›") };
    let q = wrap(&q, FontKind::Body, 13.0, qr.w - 12.0 * s()).into_iter().next().unwrap_or_default();
    text(&q, r.x + 16.0 * s(), qy, 13.0, if empty { MUTED } else { PAPER }, FontKind::Body, Align::Left);
    if !empty && inp.tapped(qr) {
        inp.consume_tap();
        art::sfx("menu_open");
        g.detail = Some(Detail::Queue { body: b.id });
    }
    ships_here_button(g, inp, view, b, Rect::new(r.x + r.w - 150.0 * s(), qy - 10.0 * s(), 138.0 * s(), 22.0 * s()));
}

/// Who can fight / take this body right now ("Standoff…", "Battle…") as a strip attached to its sheet:
/// above it on phones, below it in computer mode (the HUD is above there).
fn orbit_strip(be: &dyn Backend, inp: &mut Input, b: &Body, r: Rect) {
    let Some(st) = be.orbit_status(b.id) else { return };
    let lower = st.to_lowercase();
    let col = if lower.starts_with("battle") { DANGER } else if lower.starts_with("standoff") { ACCENT } else { MUTED };
    let lines: Vec<String> = wrap(&st, FontKind::Body, 13.0, r.w - 40.0 * s()).into_iter().take(2).collect();
    let h = (12.0 + 17.0 * lines.len() as f32) * s();
    let sr = if wide() { Rect::new(r.x, r.y + r.h + 6.0 * s(), r.w, h) } else { Rect::new(r.x, r.y - h - 6.0 * s(), r.w, h) };
    inp.block(sr);
    cel::round_rect(sr, 8.0 * s(), alpha(PANEL, 0.96));
    cel::round_rect_lines(sr, 8.0 * s(), 1.2 * s(), col);
    draw_circle(sr.x + 14.0 * s(), sr.y + 14.5 * s(), 4.0 * s(), col);
    for (i, l) in lines.iter().enumerate() {
        text(l, sr.x + 26.0 * s(), sr.y + (14.5 + 17.0 * i as f32) * s(), 13.0, PAPER, FontKind::Body, Align::Left);
    }
}

fn body_sheet(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView, b: &Body, can_build_station: bool) {
    let own_rock = b.kind == BodyKind::Asteroid && b.owner == Some(view.me);
    let r = sheet_rect(if own_rock { 204.0 } else { 150.0 });
    inp.block(r);
    cel::panel(r, PANEL);
    orbit_strip(be, inp, b, r);
    if own_rock {
        aim_button(g, inp, view, b, Rect::new(r.x + 12.0 * s(), r.y + r.h - 54.0 * s(), r.w - 24.0 * s(), 44.0 * s()));
    }
    let c = vec2(r.x + 36.0 * s(), r.y + 36.0 * s());
    map::draw_body(b, c, 16.0 * s(), c + vec2(-100.0, -60.0), view.color_of(b.owner), get_time() as f32);
    inked(&b.name.to_uppercase(), r.x + 66.0 * s(), r.y + 26.0 * s(), 24.0, PAPER, Align::Left);
    let kind = match b.kind {
        BodyKind::Star => "STAR",
        BodyKind::Planet => "PLANET",
        BodyKind::Moon => "MOON",
        BodyKind::Asteroid => "ASTEROID",
    };
    let owner = match b.owner.and_then(|o| view.player(o)) {
        Some(p) if p.id == view.me => "YOURS".to_string(),
        Some(p) => format!("held by {}", p.name.to_uppercase()),
        None => "neutral".to_string(),
    };
    text(&format!("{kind} · {owner}"), r.x + 66.0 * s(), r.y + 50.0 * s(), 14.0, MUTED, FontKind::Title, Align::Left);
    let body_r = Rect::new(r.x + 16.0 * s(), r.y + 70.0 * s(), r.w - 32.0 * s(), r.h - 80.0 * s());
    match &b.intel {
        Some(i) => {
            let age = view.now - i.seen_at;
            let mut lines = vec![format!("Power in orbit {:.0} · scanned {} ago", i.power.max(0.0), fmt_dur(age))];
            if !i.upgrades.is_empty() {
                lines.push(format!("Upgrades: {}", i.upgrades.join(", ")));
            }
            if !i.stations.is_empty() {
                lines.push(format!("Stations: {}", i.stations.join(", ")));
            }
            for (k, line) in lines.iter().take(3).enumerate() {
                text(line, body_r.x, body_r.y + 10.0 * s() + k as f32 * 22.0 * s(), 15.0, PAPER, FontKind::Body, Align::Left);
            }
        }
        None => {
            text("Contents unknown. Build a telescope to see inside.", body_r.x, body_r.y + 12.0 * s(), 15.0, MUTED, FontKind::Body, Align::Left);
        }
    }
    ships_here_button(g, inp, view, b, Rect::new(r.x + r.w - 150.0 * s(), r.y + r.h - 40.0 * s(), 138.0 * s(), 30.0 * s()));
    if can_build_station {
        let br = Rect::new(r.x + r.w - 128.0 * s(), r.y + 36.0 * s(), 112.0 * s(), 30.0 * s());
        if widgets::button(inp, br, "STATIONS", Style::Secondary) {
            g.stations_for = Some(b.id);
        }
    }
    if close_x(inp, r) {
        g.selected = None;
    }
}

fn ship_icon(view: &GameView, sh: &Ship, c: Vec2, px: f32) {
    let player = view.player(sh.owner);
    let skin = player.map(|p| art::skin_of(&p.faction)).unwrap_or("vanguard");
    match art::ship_tex(sh.class, skin, player.map(|p| p.color)) {
        Some(t) => art::draw_centered(&t, c, vec2(px, px), -0.5, WHITE),
        None => cel::ship(c, vec2(1.0, -0.5), px * 0.4, team(player.map(|p| p.color)).0),
    }
}

/// Compact bar while picking a target, so the map above stays tappable.
fn target_bar(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView, sh: &Ship) {
    // A constructor at a site with our stations still being built can leave once they're done.
    let is_builder = sh.class == ShipClass::Constructor || sh.members.iter().any(|m| m.class == ShipClass::Constructor);
    let done_at = sh.parked_at.filter(|_| is_builder && !map::is_flying(sh, view.now)).and_then(|b| be.construction_done(b)).filter(|&t| t > view.now);
    if done_at.is_none() && g.leave_after.is_some_and(|l| l.0 == sh.id) {
        g.leave_after = None;
    }
    let extra = if done_at.is_some() { 44.0 } else { 0.0 };
    let r = sheet_rect(112.0 + extra);
    inp.block(r);
    cel::panel(r, PANEL);
    ship_icon(view, sh, vec2(r.x + 28.0 * s(), r.y + 28.0 * s()), 34.0 * s());
    let title = if sh.count > 1 { format!("{} ×{}", sh.name.to_uppercase(), sh.count) } else { sh.name.to_uppercase() };
    inked(&title, r.x + 54.0 * s(), r.y + 20.0 * s(), 18.0, PAPER, Align::Left);
    let building = sh.planned.filter(|&t| t > view.now);
    let hint = match building {
        Some(t) => format!("Ready in {} · tap a target to send it then", fmt_dur(t - view.now)),
        None => "Tap a planet or moon to plot a course".to_string(),
    };
    text(&hint, r.x + 54.0 * s(), r.y + 40.0 * s(), 13.0, ACCENT, FontKind::Body, Align::Left);
    if let Some(t) = done_at {
        let on = g.leave_after.is_some_and(|l| l.0 == sh.id);
        let cr = Rect::new(r.x + 12.0 * s(), r.y + 56.0 * s(), 190.0 * s(), 32.0 * s());
        let paused = !t.is_finite();
        let label = if paused { "LEAVE WHEN DONE (paused)" } else { "LEAVE WHEN DONE" };
        if widgets::chip(inp, cr, label, on, ACCENT) && !paused {
            g.leave_after = if on { None } else { Some((sh.id, t)) };
            art::sfx(if on { "toggle_off" } else { "toggle_on" });
        }
        let note = if paused {
            "build paused".to_string()
        } else if on {
            format!("then departs · {}", fmt_dur(t - view.now))
        } else {
            format!("station done in {}", fmt_dur(t - view.now))
        };
        text(&note, cr.x + cr.w + 10.0 * s(), cr.center().y, 12.0, if on { ACCENT } else { MUTED }, FontKind::Body, Align::Left);
    }
    let by = r.y + r.h - 52.0 * s();
    let group = super::fleet::can_group(view, sh);
    let n = if group { 4 } else { 3 };
    let bw = (r.w - (n + 1) as f32 * 10.0 * s()) / n as f32;
    let b = |i: usize| Rect::new(r.x + 10.0 * s() + i as f32 * (bw + 10.0 * s()), by, bw, 42.0 * s());
    // Construction ships in orbit build stations instead of plotting burns.
    let builds = sh.class == ShipClass::Constructor || sh.members.iter().any(|m| m.class == ShipClass::Constructor);
    let site = sh.parked_at.filter(|_| builds && !map::is_flying(sh, view.now));
    if let Some(body) = site {
        if widgets::button(inp, b(0), if n > 3 { "BUILD" } else { "BUILD STATION" }, Style::Primary) {
            g.mode = Mode::Idle;
            g.selected = Some(Focus::Body(body));
            g.stations_for = Some(body);
        }
    } else if building.is_none() && widgets::button(inp, b(0), "MANEUVER", Style::Secondary) {
        g.mode = Mode::Nodes(NodeEdit {
            ship: sh.id,
            nodes: if sh.nodes.is_empty() { vec![ManeuverNode { t: view.now + 1800.0, prograde: 0.0, radial: 0.0 }] } else { sh.nodes.clone() },
            sel: 0,
            preview: None,
            dirty: true,
            drag: None,
        });
    }
    if widgets::button(inp, b(1), "DETAILS", Style::Secondary) {
        g.mode = Mode::Idle;
    }
    if group && widgets::button(inp, b(2), "FLEET", Style::Secondary) {
        open_fleet(g, sh.id, Some(Focus::Ship(sh.id)));
    }
    if widgets::button(inp, b(n - 1), "CANCEL", Style::Ghost) {
        g.mode = Mode::Idle;
        g.selected = None;
    }
}

fn open_fleet(g: &mut GameScreen, anchor: ShipId, on: Option<Focus>) {
    g.mode = Mode::Idle;
    g.selected = on;
    g.fleet = Some(super::fleet::FleetSheet::new(anchor, on));
    art::sfx("menu_open");
}

/// "Ships here (n)" entry on a body sheet; opens the fleet sheet on the first of our ships there.
fn ships_here_button(g: &mut GameScreen, inp: &mut Input, view: &GameView, b: &Body, r: Rect) {
    let here = super::fleet::ships_here(view, b.id);
    let Some(first) = here.first().map(|s| s.id) else { return };
    let n: u32 = here.iter().map(|s| s.count).sum();
    if widgets::button(inp, r, &format!("SHIPS HERE ({n})"), Style::Secondary) {
        open_fleet(g, first, Some(Focus::Body(b.id)));
    }
}

fn ship_sheet(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView, sh: &Ship, targeting: bool) {
    let mine = sh.owner == view.me;
    if targeting {
        return target_bar(g, inp, be, view, sh);
    }
    let abilities = if mine && sh.planned.is_none() { be.ship_abilities(sh.id) } else { vec![] };
    let r = sheet_rect(if mine { 200.0 } else { 110.0 } + if abilities.is_empty() { 0.0 } else { 44.0 });
    inp.block(r);
    cel::panel(r, PANEL);
    ship_icon(view, sh, vec2(r.x + 34.0 * s(), r.y + 34.0 * s()), 40.0 * s());
    let title = if sh.count > 1 { format!("{} ×{}", sh.name.to_uppercase(), sh.count) } else { sh.name.to_uppercase() };
    inked(&title, r.x + 64.0 * s(), r.y + 24.0 * s(), 22.0, PAPER, Align::Left);
    let building = sh.planned.filter(|&t| t > view.now);
    let status = if let Some(ready) = building {
        let dest = sh.destination.and_then(|d| view.body(d)).map(|d| format!(" · then {}", d.name)).unwrap_or_default();
        format!("{} · ready in {}{}", sh.class.label(), fmt_dur(ready - view.now), dest)
    } else if map::is_flying(sh, view.now) {
        let dest = sh.destination.and_then(|d| view.body(d)).map(|b| b.name.clone()).unwrap_or_else(|| "deep space".into());
        let eta = sh.trajectory.as_ref().and_then(|t| t.end()).map(|e| fmt_dur(e - view.now)).unwrap_or_default();
        format!("{} · to {} · ETA {}", sh.class.label(), dest, eta)
    } else if let Some(b) = sh.parked_at.and_then(|b| view.body(b)) {
        let dep = sh.trajectory.as_ref().and_then(|t| t.start()).filter(|&t0| t0 > view.now);
        match dep {
            Some(t0) => format!("{} · at {} · departs in {}", sh.class.label(), b.name, fmt_dur(t0 - view.now)),
            None => format!("{} · orbiting {}", sh.class.label(), b.name),
        }
    } else {
        sh.class.label().to_string()
    };
    text(&status, r.x + 64.0 * s(), r.y + 48.0 * s(), 14.0, MUTED, FontKind::Body, Align::Left);
    if mine && building.is_some() {
        // Still in the build queue: only a transfer can be queued (tap a target), or cancelled.
        text("Tap a target to send it there once it's built", r.x + 16.0 * s(), r.y + 92.0 * s(), 14.0, PAPER, FontKind::Body, Align::Left);
        let by = r.y + r.h - 58.0 * s();
        let bw = (r.w - 3.0 * 12.0 * s()) / 2.0;
        let b = |i: usize| Rect::new(r.x + 12.0 * s() + i as f32 * (bw + 12.0 * s()), by, bw, 46.0 * s());
        if sh.destination.is_some() && widgets::button(inp, b(0), "CANCEL ORDER", Style::Danger) {
            g.emit(UiAction::CancelFlight { ship: sh.id });
        }
        if widgets::button(inp, b(1), "DONE", Style::Ghost) {
            g.mode = Mode::Idle;
            g.selected = None;
        }
    } else if mine {
        let fr = Rect::new(r.x + 64.0 * s(), r.y + 64.0 * s(), r.w - 150.0 * s(), 10.0 * s());
        widgets::bar(fr, sh.fuel / sh.fuel_max.max(1.0), SUN_SHADE);
        text(&format!("FUEL {:.0}/{:.0}", sh.fuel, sh.fuel_max), fr.x + fr.w + 10.0 * s(), fr.center().y, 13.0, PAPER, FontKind::Title, Align::Left);
        // Standing order while we're offline.
        let oy = r.y + 86.0 * s();
        text("IF ATTACKED", r.x + 16.0 * s(), oy + 15.0 * s(), 13.0, MUTED, FontKind::Title, Align::Left);
        let ow = 96.0 * s();
        for (i, (label, retreat)) in [("HOLD", false), ("RETREAT", true)].iter().enumerate() {
            let cr = Rect::new(r.x + 110.0 * s() + i as f32 * (ow + 10.0 * s()), oy, ow, 30.0 * s());
            if widgets::chip(inp, cr, label, sh.retreat == *retreat, if *retreat { TEAMS[0].0 } else { ACCENT }) && sh.retreat != *retreat {
                art::sfx(if *retreat { "toggle_on" } else { "toggle_off" });
                g.emit(UiAction::SetOrder { ship: sh.id, retreat: *retreat });
            }
        }
        if !abilities.is_empty() {
            ability_row(g, inp, &abilities, Rect::new(r.x + 16.0 * s(), oy + 40.0 * s(), r.w - 32.0 * s(), 32.0 * s()), |key| UiAction::ShipAbility { ship: sh.id, key });
        }
        let by = r.y + r.h - 58.0 * s();
        let bw = (r.w - 4.0 * 12.0 * s()) / 3.0;
        let b = |i: usize| Rect::new(r.x + 12.0 * s() + i as f32 * (bw + 12.0 * s()), by, bw, 46.0 * s());
        if widgets::button(inp, b(0), "MANEUVER", Style::Secondary) {
            g.mode = Mode::Nodes(NodeEdit {
                ship: sh.id,
                nodes: if sh.nodes.is_empty() { vec![ManeuverNode { t: view.now + 1800.0, prograde: 0.0, radial: 0.0 }] } else { sh.nodes.clone() },
                sel: 0,
                preview: None,
                dirty: true,
                drag: None,
            });
        }
        let flying = sh.trajectory.is_some();
        if flying && widgets::button(inp, b(1), "ABORT", Style::Danger) {
            g.emit(UiAction::CancelFlight { ship: sh.id });
        }
        // where can it go, and when: the transfer-window tool
        if !flying && sh.parked_at.is_some() && widgets::button(inp, b(1), "WINDOWS", Style::Secondary) {
            g.mode = Mode::Windows(super::windows::WindowsSheet::new(sh.id));
        }
        if widgets::button(inp, b(2), "DONE", Style::Ghost) {
            g.mode = Mode::Idle;
            g.selected = None;
        }
    } else if close_x(inp, r) {
        g.selected = None;
        g.mode = Mode::Idle;
    }
}

/// A row of special-action chips (go dark, decoy look, lidar sweep...) with the first note or
/// blocker beside them. Tapping a usable one emits `act(key)`.
pub(crate) fn ability_row(g: &mut GameScreen, inp: &mut Input, abilities: &[Ability], r: Rect, act: impl Fn(String) -> UiAction) {
    let gap = 8.0 * s();
    let mut x = r.x;
    let mut note = abilities.iter().find_map(|a| a.blocker.clone().or_else(|| (!a.note.is_empty()).then(|| a.note.clone()))).unwrap_or_default();
    for a in abilities {
        let w = (measure(&a.label, FontKind::Title, 13.0).x + 28.0 * s()).max(84.0 * s());
        if x + w > r.x + r.w {
            break;
        }
        let cr = Rect::new(x, r.y, w, r.h);
        let usable = a.blocker.is_none();
        if widgets::chip(inp, cr, &a.label, a.on, if usable { ACCENT } else { MUTED }) {
            if usable {
                art::sfx(if a.on { "toggle_off" } else { "toggle_on" });
                g.emit(act(a.key.clone()));
            } else {
                art::sfx("ui_error");
                note = a.blocker.clone().unwrap_or_default();
                g.hint = Some((note.clone(), get_time()));
            }
        }
        x += w + gap;
    }
    if x < r.x + r.w - 40.0 * s() {
        let room = r.x + r.w - x;
        if let Some(line) = wrap(&note, FontKind::Body, 12.0, room).into_iter().next() {
            text(&line, x, r.center().y, 12.0, MUTED, FontKind::Body, Align::Left);
        }
    }
}

fn plan_sheet(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView, p: &TransferPlan) {
    // course options (cheapest … fastest) get a fuel ↔ time slider under the title
    let slider = p.options.len() > 1;
    let oy = if slider { 56.0 * s() } else { 0.0 };
    let icp = p.intercept.clone();
    let extra = if p.fuel_problem.is_some() { 20.0 } else { 0.0 } + if icp.is_some() { 20.0 } else { 0.0 };
    let r = sheet_rect(178.0 + if slider { 56.0 } else { 0.0 } + extra);
    inp.block(r);
    cel::panel(r, PANEL);
    let target = match &icp {
        Some(i) => view.ship(i.target).map(|s| s.name.to_uppercase()).unwrap_or_else(|| "CONTACT".into()),
        None => view.body(p.target).map(|b| b.name.to_uppercase()).unwrap_or_default(),
    };
    let via: Vec<String> = p.via.iter().filter_map(|&b| view.body(b)).map(|b| b.name.to_uppercase()).collect();
    // a ship already in flight changes course with one burn on its current path
    let rerouting = icp.is_none() && view.ship(p.ship).is_some_and(|sh| map::is_flying(sh, view.now));
    let title = if icp.is_some() {
        format!("INTERCEPT {target}")
    } else if rerouting {
        format!("NEW COURSE TO {target}")
    } else if via.is_empty() {
        format!("COURSE TO {target}")
    } else {
        format!("COURSE TO {target} VIA {}", via.join(", "))
    };
    inked(&title, r.x + 16.0 * s(), r.y + 24.0 * s(), 24.0, ACCENT, Align::Left);
    let mut p = p.clone();
    if slider {
        if let Some(np) = fuel_slider(g, inp, Rect::new(r.x + 16.0 * s(), r.y + 38.0 * s(), r.w - 32.0 * s(), 50.0 * s()), &p) {
            p = np.clone();
            g.mode = Mode::Plan(np);
        }
    }
    let p = &p;
    let cols = [
        (if rerouting { "BURN" } else { "DEPART" }, if p.depart_t - view.now < 60.0 { "NOW".to_string() } else { format!("in {}", fmt_dur(p.depart_t - view.now)) }),
        // with the slider, what changes is when you get there; a lone course shows its flight time
        if icp.is_some() {
            ("MEET", format!("in {}", fmt_dur(p.arrive_t - view.now)))
        } else if slider {
            ("ARRIVE", format!("in {}", fmt_dur(p.arrive_t - view.now)))
        } else {
            ("FLIGHT", fmt_dur(p.arrive_t - p.depart_t))
        },
        ("DELTA-V", format!("{:.2} km/s", p.dv_depart + p.dv_mid + p.dv_capture)),
    ];
    let cw = (r.w - 32.0 * s()) / 3.0;
    for (i, (k, v)) in cols.iter().enumerate() {
        let x = r.x + 16.0 * s() + i as f32 * cw;
        text(k, x, r.y + oy + 52.0 * s(), 13.0, MUTED, FontKind::Title, Align::Left);
        inked(v, x, r.y + oy + 74.0 * s(), 19.0, PAPER, Align::Left);
    }
    let fuel = p.fuel_have;
    let ok = p.fuel_problem.is_none();
    let fr = Rect::new(r.x + 16.0 * s(), r.y + oy + 96.0 * s(), r.w - 180.0 * s(), 10.0 * s());
    widgets::bar(fr, p.fuel_needed / fuel.max(1.0), if ok { SUN_SHADE } else { DANGER });
    let fl = if ok { format!("FUEL {:.0} of {:.0}", p.fuel_needed, fuel) } else { format!("NEED {:.0} FUEL, HAVE {:.0}", p.fuel_needed, fuel) };
    text(&fl, fr.x + fr.w + 10.0 * s(), fr.center().y, 13.0, if ok { PAPER } else { DANGER }, FontKind::Title, Align::Left);
    if let Some(why) = &p.fuel_problem {
        let why = why.strip_prefix("not enough fuel: ").unwrap_or(why);
        let mut why: String = why.chars().take(70).collect();
        if let Some(c) = why.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        text(&why, fr.x, fr.y + 26.0 * s(), 13.0, DANGER, FontKind::Body, Align::Left);
    }
    if let Some(i) = &icp {
        let y = fr.y + 26.0 * s() + if p.fuel_problem.is_some() { 20.0 * s() } else { 0.0 };
        let line = if i.matched {
            format!("CLOSING {:.1} km/s, MATCHES SPEED THERE AND STAYS WITH IT", i.closing)
        } else {
            format!("FLIES THROUGH AT {:.1} km/s (ONE PASS)", i.closing)
        };
        text(&line, fr.x, y, 13.0, PAPER, FontKind::Title, Align::Left);
    }

    let by = r.y + r.h - 58.0 * s();
    let bw = (r.w - 4.0 * 12.0 * s()) / 3.0;
    let b = |i: usize| Rect::new(r.x + 12.0 * s() + i as f32 * (bw + 12.0 * s()), by, bw, 46.0 * s());
    let exact = !slider || !p.token.is_empty();
    let label = if exact { "ENGAGE" } else { "PLOTTING" };
    if widgets::button(inp, b(0), label, if ok && exact { Style::Primary } else { Style::Ghost }) && ok && exact {
        g.emit(UiAction::ScheduleTransfer(p.clone()));
        art::sfx("transfer_scheduled");
        let body = if icp.is_some() {
            format!("Intercepting {target}")
        } else if rerouting {
            format!("New course for {target}")
        } else {
            format!("Departing for {target}")
        };
        g.push_notice(Notice { kind: NoticeKind::Good, title: "Course laid in".into(), body, focus: None });
        g.mode = Mode::Idle;
    }
    if let Some(i) = &icp {
        // instead of a later window: match speed at the meeting point, or fly straight through
        let label = if i.matched { "FLY-THROUGH" } else { "MATCH SPEED" };
        if widgets::button(inp, b(1), label, Style::Secondary) {
            let (ship, tgt, m) = (p.ship, i.target, !i.matched);
            g.intercept(view, ship, tgt, m);
        }
    } else if widgets::button(inp, b(1), if rerouting { "BURN LATER" } else { "NEXT WINDOW" }, Style::Secondary) {
        let after = p.windows.iter().copied().find(|&w| w > p.depart_t + 60.0).unwrap_or(p.depart_t + 3600.0);
        let (ship, tgt) = (p.ship, p.target);
        g.plan(be, view, ship, tgt, after);
    }
    if widgets::button(inp, b(2), "CANCEL", Style::Ghost) {
        g.mode = Mode::Idle;
    }
}

/// Knob positions (0..1) of the course options on the fuel ↔ time slider: by Δv, cheapest left.
fn option_stops(p: &TransferPlan) -> Vec<f32> {
    let (lo, hi) = match (p.options.first(), p.options.last()) {
        (Some(a), Some(b)) if b.total_dv() > a.total_dv() => (a.total_dv(), b.total_dv()),
        _ => return (0..p.options.len()).map(|i| i as f32 / (p.options.len().max(2) - 1) as f32).collect(),
    };
    p.options.iter().map(|o| ((o.total_dv() - lo) / (hi - lo)).clamp(0.0, 1.0) as f32).collect()
}

/// Which pair of neighbouring options knob position `x` lies between, and how far along.
fn stop_segment(stops: &[f32], x: f32) -> (usize, f32) {
    let i = stops.windows(2).position(|w| x <= w[1]).unwrap_or(stops.len().saturating_sub(2));
    let (a, b) = (stops[i], stops[(i + 1).min(stops.len() - 1)]);
    (i, if b > a { ((x - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 })
}

/// Fuel ↔ time slider (left = cheapest, right = fastest, ticks = the planned options). While dragging, the course shown is a
/// blend of the two neighbouring options (path and numbers move live); when the finger lifts the
/// backend plans the exact course there (`GameScreen::plan_want`). Returns the course to show
/// when it changed this frame.
fn fuel_slider(g: &mut GameScreen, inp: &mut Input, row: Rect, p: &TransferPlan) -> Option<TransferPlan> {
    let stops = option_stops(p);
    let track = Rect::new(row.x + 10.0 * s(), row.y + 12.0 * s(), row.w - 20.0 * s(), 6.0 * s());
    g.plan_track = Some(track);
    let x_of = |h: Vec2| ((h.x - track.x) / track.w).clamp(0.0, 1.0);
    let (lo, hi) = (p.options[0].total_dv(), p.options[p.options.len() - 1].total_dv());
    let here = if hi > lo { ((p.total_dv() - lo) / (hi - lo)).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let mut out = None;
    match (inp.held, g.plan_drag) {
        (Some(h), drag) if inp.gesture_on_ui() && (drag.is_some() || (row.contains(h) && !inp.occluded(h))) => {
            let x = x_of(h);
            if drag.is_none_or(|d| d != x) {
                // tick as the knob passes an option
                if drag.is_some_and(|d| stops.iter().any(|&st| (d - st) * (x - st) < 0.0)) {
                    art::sfx("slider_tick");
                }
                g.plan_drag = Some(x);
                g.plan_want = None;
                let (i, f) = stop_segment(&stops, x);
                let mut o = match p.options.get(i + 1) {
                    Some(b) => p.options[i].blend(b, f as f64),
                    None => p.options[i].clone(),
                };
                o.options = p.options.clone();
                o.windows = p.windows.clone();
                out = Some(o);
            }
        }
        (None, Some(x)) => {
            // finger lifted: snap onto an option when close, else plan the exact course there
            g.plan_drag = None;
            let (i, f) = stop_segment(&stops, x);
            let near = |k: usize| stops.get(k).is_some_and(|&st| (st - x).abs() * track.w < 10.0 * s());
            if near(i) || near(i + 1) || p.options.get(i + 1).is_none() {
                let k = if near(i + 1) || (p.options.get(i + 1).is_some() && f >= 0.5) { i + 1 } else { i };
                let mut o = p.options[k].clone();
                o.options = p.options.clone();
                o.windows = p.windows.clone();
                out = Some(o);
            } else {
                g.plan_want = Some((i, f, x));
            }
        }
        _ => {}
    }
    let knob = g.plan_drag.or(g.plan_want.map(|w| w.2)).unwrap_or(here);
    widgets::bar(track, knob, ACCENT);
    for (k, &st) in stops.iter().enumerate() {
        let x = track.x + track.w * st;
        draw_line(x, track.y + track.h + 3.0 * s(), x, track.y + track.h + 8.0 * s(), 1.5 * s(), if k == 0 || k + 1 == stops.len() { PAPER } else { MUTED });
    }
    let kc = vec2(track.x + track.w * knob, track.y + track.h / 2.0);
    draw_circle(kc.x, kc.y, 13.0 * s(), alpha(ACCENT, 0.35));
    draw_circle(kc.x, kc.y, 9.0 * s(), PAPER);
    let (a, b) = (&p.options[0], &p.options[p.options.len() - 1]);
    let ly = track.y + track.h + 21.0 * s();
    let end = |o: &TransferPlan, word: &str| format!("{word}  {:.0} km/s", o.total_dv());
    text(&end(a, "CHEAPEST"), track.x - 10.0 * s(), ly, 12.0, if knob < 0.02 { ACCENT } else { MUTED }, FontKind::Title, Align::Left);
    text(&end(b, "FASTEST"), track.x + track.w + 10.0 * s(), ly, 12.0, if knob > 0.98 { ACCENT } else { MUTED }, FontKind::Title, Align::Right);
    out
}

pub(super) fn close_x(inp: &mut Input, r: Rect) -> bool {
    widgets::round_button(inp, vec2(r.x + r.w - 20.0 * s(), r.y + 20.0 * s()), 14.0 * s(), "X", PAPER, 0)
}

// ---------------------------------------------------------------- Planner overlays

fn poly(cam: &Camera, tr: &Trajectory, from: f64) -> Vec<Vec2> {
    tr.points.iter().filter(|p| p.0 >= from).map(|p| cam.to_screen(p.1)).collect()
}

pub fn plan_world(p: &TransferPlan, be: &dyn Backend, view: &GameView, cam: &Camera, t: f64) {
    let pts = poly(cam, &p.path, f64::MIN);
    cel::inked_path(&pts, 4.0 * s(), ACCENT);
    map::draw_flybys(&p.path, view, cam, f64::MIN, ACCENT);
    // Intercept: the target's known path up to the meeting point and a crosshair there.
    if let Some(i) = &p.intercept {
        let pts: Vec<Vec2> = (0..=60)
            .filter_map(|k| be.ship_pos(i.target, view.now + (p.arrive_t - view.now) * k as f64 / 60.0))
            .map(|q| cam.to_screen(q))
            .collect();
        cel::dashed_path(&pts, 2.5 * s(), 7.0 * s(), DANGER);
        let c = cam.to_screen(i.meet);
        let r = 14.0 * s();
        let ring: Vec<Vec2> = (0..=48).map(|k| c + Vec2::from_angle(k as f32 / 48.0 * std::f32::consts::TAU) * r).collect();
        cel::inked_path(&ring, 2.5 * s(), DANGER);
        for a in 0..4 {
            let d = Vec2::from_angle(a as f32 * std::f32::consts::FRAC_PI_2);
            draw_line(c.x + d.x * r * 0.5, c.y + d.y * r * 0.5, c.x + d.x * r * 1.6, c.y + d.y * r * 1.6, 2.5 * s(), DANGER);
        }
    } else if let Some(b) = view.body(p.target) {
    // Ghost of the target where it will be on arrival.
        let c = cam.to_screen(be.body_pos(p.target, p.arrive_t));
        let r = map::body_radius_px(b, cam);
        let ring: Vec<Vec2> = (0..=48).map(|i| c + Vec2::from_angle(i as f32 / 48.0 * std::f32::consts::TAU) * (r + 4.0 * s())).collect();
        cel::dashed_path(&ring, 3.0 * s(), 6.0 * s(), ACCENT);
        cel::burst(c, r * 0.6 + 6.0 * s(), 8, alpha(ACCENT, 0.6), 0.0);
    }
    if let Some(&(_, d)) = p.path.points.first() {
        cel::diamond(cam.to_screen(d), 8.0 * s(), ACCENT);
    }
    // Where the ship would be at the scrubbed time.
    if let Some(sp) = p.path.sample(t) {
        let q = p.path.sample(t + 120.0).unwrap_or(sp);
        let a = cam.to_screen(sp);
        cel::ship(a, cam.to_screen(q) - a, 10.0 * s(), alpha(PAPER, 0.7));
    }
}

pub fn plan_bubble(p: &TransferPlan, _be: &dyn Backend, view: &GameView, cam: &Camera, _t: f64) {
    let Some(&(_, d)) = p.path.points.first() else { return };
    let anchor = cam.to_screen(d);
    let label = if p.depart_t - view.now < 60.0 { "BURN NOW".to_string() } else { format!("BURN IN {}", fmt_dur(p.depart_t - view.now)) };
    let w = measure(&label, FontKind::Title, 18.0).x + 28.0 * s();
    let h = 38.0 * s();
    let mut r = Rect::new(anchor.x - w - 24.0 * s(), anchor.y - h - 34.0 * s(), w, h);
    r.x = r.x.clamp(margin(), screen_width() - w - margin());
    r.y = r.y.clamp(hud_bottom() + 40.0 * s(), screen_height());
    cel::bubble(r, Some(anchor + vec2(-4.0, -8.0) * s()), PAPER);
    text(&label, r.center().x, r.center().y, 18.0, INK, FontKind::Title, Align::Center);
}

// ---------------------------------------------------------------- Node editor

struct NodeFrame {
    pos: Vec2,
    pro: Vec2,
    rad: Vec2,
    /// km/s per dp of handle drag.
    k: f64,
}

fn node_frame(ne: &NodeEdit, be: &dyn Backend, cam: &Camera) -> Option<NodeFrame> {
    let n = ne.nodes.get(ne.sel)?;
    let at = |t: f64| ne.preview.as_ref().and_then(|tr| tr.sample(t)).or_else(|| be.ship_pos(ne.ship, t));
    let p = at(n.t)?;
    let q = at(n.t - 120.0).unwrap_or(p);
    let pos = cam.to_screen(p);
    let mut pro = (pos - cam.to_screen(q)).normalize_or_zero();
    if pro == Vec2::ZERO {
        pro = vec2(1.0, 0.0);
    }
    let mut rad = pro.perp();
    if rad.dot(pos - cam.to_screen([0.0, 0.0])) < 0.0 {
        rad = -rad;
    }
    let speed = (p[0] - q[0]).hypot(p[1] - q[1]) / 120.0;
    Some(NodeFrame { pos, pro, rad, k: (speed * 0.002).max(0.001) })
}

fn handle_len() -> f32 {
    62.0 * s()
}

fn handle_dir(f: &NodeFrame, h: Handle) -> Vec2 {
    match h {
        Handle::Prograde => f.pro,
        Handle::Retrograde => -f.pro,
        Handle::RadialOut => f.rad,
        Handle::RadialIn => -f.rad,
        Handle::Time => Vec2::ZERO,
    }
}

const HANDLES: [(Handle, Color, &str); 4] = [
    (Handle::Prograde, hex(0x80ed99), "PRO"),
    (Handle::Retrograde, hex(0x80ed99), "RETRO"),
    (Handle::RadialOut, hex(0x4cc9f0), "OUT"),
    (Handle::RadialIn, hex(0x4cc9f0), "IN"),
];

/// UI-side projection from the preview path: end point, plus the closest approach to any body
/// (scored by distance / SOI so a near pass of a moon beats a far pass of a giant).
/// Only the path after `after` (the first burn) counts, and a body we're still inside the SOI of at
/// that point (where we're leaving from) only counts once we've left it.
pub fn derive_outcome(be: &dyn Backend, view: &GameView, tr: &Trajectory, after: f64) -> Option<NodeOutcome> {
    let &(end_t, end_pos) = tr.points.last()?;
    let mut best: Option<(f64, Encounter)> = None;
    for b in view.bodies.iter().filter(|b| b.kind != BodyKind::Star) {
        let soi = b.soi.max(b.radius * 3.0).max(1.0);
        let mut leaving = true;
        for &(t, p) in tr.points.iter().filter(|q| q.0 >= after) {
            let c = be.body_pos(b.id, t);
            let d = (p[0] - c[0]).hypot(p[1] - c[1]);
            if leaving {
                if d <= soi {
                    continue;
                }
                leaving = false;
            }
            let score = d / soi;
            if best.as_ref().is_none_or(|(s, _)| score < *s) {
                best = Some((score, Encounter { body: b.id, t, distance: d, inside_soi: d <= soi }));
            }
        }
    }
    // Only worth mentioning if it comes reasonably close.
    let encounter = best.filter(|(s, _)| *s < 6.0).map(|b| b.1);
    let captured = encounter.filter(|e| e.inside_soi && (end_t - e.t).abs() < 1.0).map(|e| e.body);
    Some(NodeOutcome { end_t, end_pos, captured, encounter, fuel_left: None })
}

fn fmt_km(km: f64) -> String {
    if km >= 1.0e6 {
        format!("{:.1} Gm", km / 1.0e6)
    } else if km >= 1000.0 {
        format!("{:.0} Mm", km / 1000.0)
    } else {
        format!("{km:.0} km")
    }
}

/// One-line summary for the node sheet.
fn outcome_line(o: &NodeOutcome, view: &GameView) -> (String, Color) {
    let name = |id: BodyId| view.body(id).map(|b| b.name.clone()).unwrap_or_default();
    let mut s = match (o.captured, o.encounter) {
        (Some(b), _) => (format!("Ends in orbit of {} in {}", name(b), fmt_dur(o.end_t - view.now)), GOOD),
        (None, Some(e)) if e.inside_soi => (format!("Encounter: {} in {} ({})", name(e.body), fmt_dur(e.t - view.now), fmt_km(e.distance)), GOOD),
        (None, Some(e)) => (format!("Closest: {} {} in {}", name(e.body), fmt_km(e.distance), fmt_dur(e.t - view.now)), ACCENT),
        (None, None) => (format!("No encounters · path ends in {}", fmt_dur(o.end_t - view.now)), MUTED),
    };
    if let Some(f) = o.fuel_left {
        s.0 += &format!(" · fuel left {f:.0}");
        if f < 0.0 {
            s.1 = DANGER;
        }
    }
    s
}

pub fn nodes_world(ne: &NodeEdit, outcome: Option<&NodeOutcome>, be: &dyn Backend, view: &GameView, cam: &Camera, _t: f64) {
    if let Some(tr) = &ne.preview {
        let pts = poly(cam, tr, view.now);
        cel::inked_path(&pts, 4.0 * s(), hex(0x80ed99));
        map::draw_flybys(tr, view, cam, view.now, hex(0x80ed99));
    }
    if let Some(o) = outcome {
        let col = hex(0x80ed99);
        // Closest approach: the body where it will be then, and a tie line to the ship.
        if let Some(e) = o.encounter {
            if let (Some(b), Some(p)) = (view.body(e.body), ne.preview.as_ref().and_then(|tr| tr.sample(e.t))) {
                let bc = cam.to_screen(be.body_pos(e.body, e.t));
                let sp = cam.to_screen(p);
                let rr = map::body_radius_px(b, cam) + 6.0 * s();
                let ring: Vec<Vec2> = (0..=40).map(|i| bc + Vec2::from_angle(i as f32 / 40.0 * std::f32::consts::TAU) * rr).collect();
                cel::dashed_path(&ring, 2.0 * s(), 5.0 * s(), if e.inside_soi { col } else { ACCENT });
                cel::dashed_path(&[sp, bc], 1.5 * s(), 4.0 * s(), alpha(PAPER, 0.6));
                draw_circle(sp.x, sp.y, 4.0 * s(), if e.inside_soi { col } else { ACCENT });
            }
        }
        // Where it ends up.
        let end = cam.to_screen(o.end_pos);
        let q = ne.preview.as_ref().and_then(|tr| tr.sample(o.end_t - 300.0)).map(|p| cam.to_screen(p)).unwrap_or(end);
        cel::ship(end, end - q, 9.0 * s(), alpha(PAPER, 0.75));
        let label = format!("END +{}", fmt_dur(o.end_t - view.now));
        let w = measure(&label, FontKind::Title, 12.0).x + 14.0 * s();
        let r = Rect::new(end.x + 10.0 * s(), end.y - 22.0 * s(), w, 20.0 * s());
        cel::round_rect(r, r.h / 2.0, alpha(PANEL, 0.9));
        text(&label, r.center().x, r.center().y + s(), 12.0, PAPER, FontKind::Title, Align::Center);
    }
    for (i, n) in ne.nodes.iter().enumerate() {
        let p = ne.preview.as_ref().and_then(|tr| tr.sample(n.t)).or_else(|| be.ship_pos(ne.ship, n.t));
        if let Some(p) = p {
            cel::diamond(cam.to_screen(p), if i == ne.sel { 11.0 } else { 8.0 } * s(), if i == ne.sel { ACCENT } else { PAPER });
        }
    }
}

pub fn node_handles(ne: &NodeEdit, be: &dyn Backend, _view: &GameView, cam: &Camera, _t: f64) {
    let Some(f) = node_frame(ne, be, cam) else { return };
    let n = ne.nodes[ne.sel];
    for (h, col, label) in HANDLES {
        let d = handle_dir(&f, h);
        let amount = match h {
            Handle::Prograde => n.prograde.max(0.0),
            Handle::Retrograde => (-n.prograde).max(0.0),
            Handle::RadialOut => n.radial.max(0.0),
            Handle::RadialIn => (-n.radial).max(0.0),
            Handle::Time => 0.0,
        };
        let stretch = (amount / f.k / 60.0).min(1.0) as f32 * 30.0 * s();
        let tip = f.pos + d * (handle_len() + stretch);
        let active = ne.drag == Some(h);
        cel::arrow(f.pos + d * 16.0 * s(), tip, if active { 6.0 } else { 4.5 } * s(), col);
        draw_circle(tip.x, tip.y, 12.0 * s(), INK);
        draw_circle(tip.x, tip.y, 9.0 * s(), if active { ACCENT } else { col });
        text(label, tip.x + d.x * 26.0 * s(), tip.y + d.y * 20.0 * s(), 12.0, PAPER, FontKind::Title, Align::Center);
    }
    cel::diamond(f.pos, 11.0 * s(), ACCENT);
}

/// Handle/diamond drags. Returns true while a node drag owns the gesture.
pub fn node_drag(ne: &mut NodeEdit, inp: &Input, be: &dyn Backend, view: &GameView, cam: &Camera, _t: f64) -> bool {
    let Some(f) = node_frame(ne, be, cam) else { return false };
    if let Some(p) = inp.pressed {
        ne.drag = None;
        if p.distance(f.pos) < 22.0 * s() {
            ne.drag = Some(Handle::Time);
        }
        for (h, _, _) in HANDLES {
            let tip = f.pos + handle_dir(&f, h) * handle_len();
            if p.distance(tip) < 30.0 * s() {
                ne.drag = Some(h);
            }
        }
        // Tapping another node's diamond selects it.
        for (i, n) in ne.nodes.iter().enumerate() {
            if let Some(q) = ne.preview.as_ref().and_then(|tr| tr.sample(n.t)) {
                if i != ne.sel && cam.to_screen(q).distance(p) < 18.0 * s() {
                    ne.sel = i;
                    ne.drag = Some(Handle::Time);
                }
            }
        }
    }
    if inp.released.is_some() || inp.held.is_none() {
        let was = ne.drag.take().is_some();
        return was;
    }
    let Some(h) = ne.drag else { return false };
    if h == Handle::Time {
        if let Some(p) = inp.held {
            let lo = if ne.sel > 0 { ne.nodes[ne.sel - 1].t + 60.0 } else { view.now + 60.0 };
            let hi = ne.nodes.get(ne.sel + 1).map(|n| n.t - 60.0).unwrap_or(f64::MAX);
            let track: Vec<(f64, [f64; 2])> = match ne.preview.as_ref() {
                Some(tr) => tr.points.iter().copied().filter(|q| q.0 >= lo && q.0 <= hi).collect(),
                None => (0..200).map(|i| lo + i as f64 * 600.0).filter_map(|t| be.ship_pos(ne.ship, t).map(|p| (t, p))).collect(),
            };
            if let Some(nt) = map::nearest_on(&track, cam, p, 80.0 * s()) {
                ne.nodes[ne.sel].t = nt.clamp(lo, hi);
                ne.dirty = true;
            }
        }
        return true;
    }
    if let Some(d) = inp.drag.or(Some(Vec2::ZERO)) {
        let along = d.dot(handle_dir(&f, h)) / s();
        let dv = along as f64 * f.k;
        let n = &mut ne.nodes[ne.sel];
        match h {
            Handle::Prograde => n.prograde += dv,
            Handle::Retrograde => n.prograde -= dv,
            Handle::RadialOut => n.radial += dv,
            Handle::RadialIn => n.radial -= dv,
            Handle::Time => {}
        }
        if dv != 0.0 {
            ne.dirty = true;
        }
    }
    true
}

fn node_sheet(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView, mut ne: NodeEdit) {
    if ne.dirty {
        ne.preview = be.preview_nodes(ne.ship, &ne.nodes);
        ne.dirty = false;
    }
    // Projection follows the nodes live while dragging.
    if g.node_outcome.as_ref().is_none_or(|(s, n, _)| *s != ne.ship || *n != ne.nodes) {
        let o = be.node_outcome(ne.ship, &ne.nodes).or_else(|| {
            let after = ne.nodes.iter().map(|n| n.t).fold(f64::MAX, f64::min).max(view.now);
            ne.preview.as_ref().and_then(|tr| derive_outcome(be, view, tr, after))
        });
        g.node_outcome = o.map(|o| (ne.ship, ne.nodes.clone(), o));
    }
    let r = sheet_rect(222.0);
    inp.block(r);
    cel::panel(r, PANEL);
    let Some(n) = ne.nodes.get(ne.sel).copied() else {
        g.mode = Mode::Idle;
        return;
    };
    inked(&format!("NODE {}/{}", ne.sel + 1, ne.nodes.len()), r.x + 16.0 * s(), r.y + 24.0 * s(), 22.0, ACCENT, Align::Left);
    text(&format!("burn in {}", fmt_dur(n.t - view.now)), r.x + 130.0 * s(), r.y + 25.0 * s(), 15.0, MUTED, FontKind::Body, Align::Left);
    let total: f64 = ne.nodes.iter().map(|n| n.dv()).sum();
    text(&format!("TOTAL DV {:.2} km/s", total), r.x + r.w - 16.0 * s(), r.y + 25.0 * s(), 14.0, PAPER, FontKind::Title, Align::Right);

    let k = node_frame(&ne, be, g.cam.as_ref().unwrap()).map(|f| f.k * 10.0).unwrap_or(0.1);
    if let Some((_, _, o)) = &g.node_outcome {
        let (line, col) = outcome_line(o, view);
        text(&line, r.x + 16.0 * s(), r.y + 146.0 * s(), 13.0, col, FontKind::Body, Align::Left);
    }
    let rows = [("PROGRADE", n.prograde, hex(0x80ed99)), ("RADIAL", n.radial, hex(0x4cc9f0))];
    for (i, (label, v, col)) in rows.iter().enumerate() {
        let y = r.y + 46.0 * s() + i as f32 * 46.0 * s();
        text(label, r.x + 16.0 * s(), y + 19.0 * s(), 15.0, *col, FontKind::Title, Align::Left);
        inked(&format!("{:+.2}", v), r.x + 160.0 * s(), y + 19.0 * s(), 20.0, PAPER, Align::Center);
        let bw = 44.0 * s();
        let minus = Rect::new(r.x + r.w - 16.0 * s() - 2.0 * bw - 10.0 * s(), y, bw, 38.0 * s());
        let plus = Rect::new(r.x + r.w - 16.0 * s() - bw, y, bw, 38.0 * s());
        let mut delta = 0.0;
        if widgets::button(inp, minus, "-", Style::Secondary) {
            delta = -k;
        }
        if widgets::button(inp, plus, "+", Style::Secondary) {
            delta = k;
        }
        if delta != 0.0 {
            let node = &mut ne.nodes[ne.sel];
            if i == 0 {
                node.prograde += delta;
            } else {
                node.radial += delta;
            }
            ne.dirty = true;
        }
    }
    let by = r.y + r.h - 56.0 * s();
    let bw = (r.w - 5.0 * 10.0 * s()) / 4.0;
    let b = |i: usize| Rect::new(r.x + 10.0 * s() + i as f32 * (bw + 10.0 * s()), by, bw, 44.0 * s());
    let mut close = false;
    if widgets::button(inp, b(0), "+ NODE", Style::Secondary) {
        let last = ne.nodes.last().map(|n| n.t).unwrap_or(view.now);
        ne.nodes.push(ManeuverNode { t: last + 3.0 * 3600.0, prograde: 0.0, radial: 0.0 });
        ne.sel = ne.nodes.len() - 1;
        ne.dirty = true;
    }
    if widgets::button(inp, b(1), "DELETE", Style::Danger) {
        ne.nodes.remove(ne.sel);
        ne.sel = ne.sel.saturating_sub(1);
        ne.dirty = true;
        if ne.nodes.is_empty() {
            g.emit(UiAction::SetNodes { ship: ne.ship, nodes: vec![] });
            close = true;
        }
    }
    if widgets::button(inp, b(2), "COMMIT", Style::Primary) {
        g.emit(UiAction::SetNodes { ship: ne.ship, nodes: ne.nodes.clone() });
        art::sfx("transfer_scheduled");
        g.push_notice(Notice { kind: NoticeKind::Good, title: "Burns scheduled".into(), body: format!("{} node(s), dv {:.2} km/s", ne.nodes.len(), total), focus: None });
        close = true;
    }
    if widgets::button(inp, b(3), "CANCEL", Style::Ghost) {
        close = true;
    }
    g.mode = if close { Mode::Idle } else { Mode::Nodes(ne) };
}

// ---------------------------------------------------------------- Toasts, hints, chat

pub fn toasts(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) {
    let now = get_time();
    g.toasts.retain(|t| now - t.born < 5.0);
    let w = screen_width();
    let mut y = hud_bottom() + 44.0 * s();
    let mut tapped = None;
    for (i, t) in g.toasts.iter().enumerate().take(3) {
        let age = (now - t.born) as f32;
        let slide = ((age / 0.25).min(1.0) * std::f32::consts::FRAC_PI_2).sin();
        let fade = ((5.0 - age) / 0.4).clamp(0.0, 1.0);
        let tw = (w - 2.0 * margin()).min(380.0 * s());
        // computer mode: slide in at the right edge of the map, left of the side panel
        let right = if wide() { w - 2.0 * margin() - side_w() } else { w - margin() };
        let x = right - tw * slide + (1.0 - fade) * tw;
        let r = Rect::new(x, y, tw, 62.0 * s());
        inp.block(r);
        cel::panel(r, PANEL);
        let (icon, col) = match t.notice.kind {
            NoticeKind::Battle => ("alert", DANGER),
            NoticeKind::Bad => ("warning", DANGER),
            NoticeKind::Good => ("check", GOOD),
            NoticeKind::Info => ("notification", TEAMS[0].0),
        };
        cel::round_rect(Rect::new(r.x, r.y, 4.0 * s(), r.h), 2.0 * s(), col);
        let ic = vec2(r.x + 32.0 * s(), r.center().y);
        match art::tex(&format!("sprites/icons/{icon}.png")) {
            Some(tx) => art::draw_centered(&tx, ic, vec2(30.0, 30.0) * s(), 0.0, WHITE),
            None => draw_circle(ic.x, ic.y, 9.0 * s(), col),
        }
        text(&t.notice.title, r.x + 60.0 * s(), r.y + 21.0 * s(), 17.0, col, FontKind::Title, Align::Left);
        text(&t.notice.body, r.x + 60.0 * s(), r.y + 42.0 * s(), 14.0, PAPER, FontKind::Body, Align::Left);
        let _ = age;
        if inp.tapped(r) {
            inp.consume_tap();
            tapped = Some(i);
        }
        y += 72.0 * s();
    }
    if let Some(i) = tapped {
        let t = g.toasts.remove(i);
        if let Some(f) = t.notice.focus {
            g.focus(be, f, view.now + g.scrub);
        }
    }
}

pub fn hint(g: &mut GameScreen) {
    let Some((h, born)) = g.hint.clone() else { return };
    let age = get_time() - born;
    if age > 2.2 {
        g.hint = None;
        return;
    }
    let a = ((2.2 - age) / 0.3).clamp(0.0, 1.0) as f32;
    let c = vec2(screen_width() / 2.0, hud_bottom() + 52.0 * s());
    let w = measure(&h, FontKind::Title, 17.0).x + 32.0 * s();
    let r = Rect::new(c.x - w / 2.0, c.y - 17.0 * s(), w, 34.0 * s());
    cel::round_rect(r, r.h / 2.0, alpha(PANEL, 0.92 * a));
    cel::round_rect_lines(r, r.h / 2.0, 1.5 * s(), alpha(ACCENT, a));
    text(&h, c.x, c.y + s(), 17.0, alpha(ACCENT, a), FontKind::Title, Align::Center);
}

pub fn chat(g: &mut GameScreen, inp: &mut Input, view: &GameView) {
    let w = screen_width();
    let top = hud_bottom() + 4.0 * s();
    // computer mode: a column on the left, the map stays visible
    let cw = if wide() { side_w() } else { w - 2.0 * margin() };
    let r = Rect::new(margin(), top, cw, scrub_top() - top - 12.0 * s());
    inp.block(r);
    // Opaque: the map and sheets must not show through the conversation.
    cel::panel(r, alpha(PANEL, 1.0));
    inked("CHAT", r.x + 16.0 * s(), r.y + 22.0 * s(), 24.0, ACCENT, Align::Left);
    if close_x(inp, r) {
        g.chat_open = false;
        art::sfx("menu_close");
        macroquad::miniquad::window::show_keyboard(false);
    }
    let field_h = 50.0 * s();
    let inner = Rect::new(r.x + 12.0 * s(), r.y + 44.0 * s(), r.w - 24.0 * s(), r.h - 56.0 * s() - field_h - 12.0 * s());
    // Bubbles laid out bottom-up from the newest; the list scrolls (drag / wheel) and is clipped to
    // the panel, so long conversations never push past the input row.
    let maxw = inner.w * 0.78;
    let laid: Vec<(Vec<String>, f32, f32)> = view
        .chat
        .iter()
        .map(|m| {
            let lines = wrap(&m.text, FontKind::Body, 16.0, maxw - 24.0 * s());
            let lw = lines.iter().map(|l| measure(l, FontKind::Body, 16.0).x).fold(0.0, f32::max);
            let bh = lines.len() as f32 * 20.0 * s() + 34.0 * s();
            (lines, (lw + 24.0 * s()).max(90.0 * s()), bh)
        })
        .collect();
    let content_h: f32 = laid.iter().map(|l| l.2 + 12.0 * s()).sum::<f32>() + 8.0 * s();
    let max_scroll = (content_h - inner.h).max(0.0);
    if let (Some(d), Some(p)) = (inp.drag, inp.drag_pos) {
        if inner.contains(p) {
            g.chat_scroll += d.y;
        }
    }
    if let (Some((_, k)), Some(p)) = (inp.zoom, inp.hover) {
        if inner.contains(p) {
            g.chat_scroll += k.ln() / 1.15f32.ln() * 60.0 * s();
        }
    }
    // a new message while reading old ones keeps the view where it was
    if view.chat.len() > g.chat_seen && g.chat_scroll > 0.0 {
        g.chat_scroll += laid[g.chat_seen.min(laid.len())..].iter().map(|l| l.2 + 12.0 * s()).sum::<f32>();
    }
    g.chat_scroll = g.chat_scroll.clamp(0.0, max_scroll);
    cel::clip(Some(inner));
    let mut y = inner.y + inner.h + g.chat_scroll;
    for (m, (lines, bw, bh)) in view.chat.iter().zip(laid.iter()).rev() {
        let (bw, bh) = (*bw, *bh);
        y -= bh + 12.0 * s();
        if y > inner.y + inner.h {
            continue;
        }
        if y + bh < inner.y {
            break;
        }
        let mine = m.from == view.me;
        let p = view.player(m.from);
        let x = if mine { inner.x + inner.w - bw - 6.0 * s() } else { inner.x + 6.0 * s() };
        let br = Rect::new(x, y, bw, bh);
        let tail = if mine { vec2(br.x + br.w + 8.0 * s(), br.y + br.h + 8.0 * s()) } else { vec2(br.x - 8.0 * s(), br.y + br.h + 8.0 * s()) };
        let (l, _) = team(p.map(|p| p.color));
        cel::bubble(br, Some(tail), if mine { PAPER } else { l });
        let who = p.map(|p| p.name.to_uppercase()).unwrap_or_else(|| "???".into());
        text(&format!("{who} · {}", fmt_clock(m.t)), br.x + 12.0 * s(), br.y + 14.0 * s(), 11.0, Color::new(0.0, 0.0, 0.0, 0.6), FontKind::Title, Align::Left);
        for (i, line) in lines.iter().enumerate() {
            text(line, br.x + 12.0 * s(), br.y + 34.0 * s() + i as f32 * 20.0 * s(), 16.0, INK, FontKind::Body, Align::Left);
        }
    }
    cel::clip(None);
    if g.chat_scroll > 1.0 {
        // scrolled up: a pill to jump back to the newest
        let pr = Rect::new(inner.center().x - 60.0 * s(), inner.y + inner.h - 34.0 * s(), 120.0 * s(), 28.0 * s());
        if widgets::button(inp, pr, "NEWEST", Style::Secondary) {
            g.chat_scroll = 0.0;
        }
    }
    let fr = Rect::new(r.x + 12.0 * s(), r.y + r.h - field_h - 12.0 * s(), r.w - 120.0 * s(), field_h);
    let mut send = widgets::text_field(inp, fr, &mut g.chat_field, "say something...");
    send |= widgets::button(inp, Rect::new(fr.x + fr.w + 12.0 * s(), fr.y, r.w - fr.w - 36.0 * s(), field_h), "SEND", Style::Primary);
    if send && !g.chat_field.text.trim().is_empty() {
        g.chat_scroll = 0.0;
        let msg = std::mem::take(&mut g.chat_field.text);
        g.emit(UiAction::Chat(msg.trim().to_string()));
    }
    g.chat_seen = view.chat.len();
}

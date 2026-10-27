//! Sensors report: a sheet opened from the HUD with three tabs. NOW (threats, battles in sight,
//! our ships under way), CONTACTS (enemy ships in range, last known positions, what we know about
//! each planet and moon) and LOG (events and sensor sightings, new ones marked). Everything comes
//! from the fogged view, so it never shows more than the map. Tapping a row selects it on the map.

use super::map;
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

pub struct ReportState {
    pub tab: ReportTab,
    /// Scroll offset of the list, px.
    scroll: f32,
    /// The sheet was open last frame (for the open sound and the starting tab).
    shown: bool,
}

impl Default for ReportState {
    fn default() -> Self {
        ReportState { tab: ReportTab::Now, scroll: 0.0, shown: false }
    }
}

impl ReportState {
    /// Open on `tab` (skips picking the tab with the news; screenshots).
    pub fn show(&mut self, tab: ReportTab) {
        *self = ReportState { tab, scroll: 0.0, shown: true };
    }
}

const TABS: [(ReportTab, &str); 3] = [(ReportTab::Now, "NOW"), (ReportTab::Contacts, "CONTACTS"), (ReportTab::Log, "LOG")];
const SECTIONS: [ReportSection; 7] = [
    ReportSection::Threats,
    ReportSection::Battles,
    ReportSection::Transfers,
    ReportSection::Ships,
    ReportSection::Ghosts,
    ReportSection::Bodies,
    ReportSection::Log,
];

/// HUD button that opens the report: badge = new log items, red ring while something is incoming.
pub fn hud_button(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, c: Vec2) {
    let rep = be.sensors();
    let unread = rep.map(|r| r.unread()).unwrap_or(0);
    let threat = rep.is_some_and(|r| r.threats() > 0);
    if threat {
        // slow pulse so an incoming fleet is noticed without opening anything
        let k = 0.5 + 0.5 * (get_time() as f32 * 3.0).sin();
        draw_circle_lines(c.x, c.y, 25.0 * s(), 2.0 * s(), alpha(DANGER, 0.4 + 0.5 * k));
    }
    if widgets::round_button(inp, c, 21.0 * s(), "@telescope", if threat { DANGER } else { PAPER }, unread) {
        g.sensors_open = !g.sensors_open;
        if !g.sensors_open {
            close(g, false);
        }
    }
}

fn close(g: &mut GameScreen, sound: bool) {
    if sound {
        art::sfx("menu_close");
    }
    g.sensors_open = false;
    g.report.shown = false;
    g.emit(UiAction::SensorsSeen);
}

pub fn sensors_report(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) {
    let empty = SensorsReport::default();
    let rep = be.sensors().unwrap_or(&empty);
    let count = |tab: ReportTab| rep.items.iter().filter(|i| i.section.tab() == tab && !(i.section == ReportSection::Bodies && i.focus.is_none())).count();
    if !g.report.shown {
        // open where the news is: something going on now, else fresh log entries
        g.report.shown = true;
        g.report.scroll = 0.0;
        if count(ReportTab::Now) > 0 && rep.items.iter().any(|i| i.section == ReportSection::Threats || i.section == ReportSection::Battles) {
            g.report.tab = ReportTab::Now;
        } else if rep.unread() > 0 {
            g.report.tab = ReportTab::Log;
        }
        art::sfx("menu_open");
    }
    let m = 12.0 * s();
    let rw = (screen_width() - 2.0 * m).min(760.0 * s());
    let r = Rect::new((screen_width() - rw) / 2.0, safe_top() + m, rw, screen_height() - safe_top() - safe_bottom() - 2.0 * m);
    inp.block(r);
    cel::panel(r, alpha(PANEL, 0.98));
    cel::round_rect_lines(r, 14.0 * s(), 1.0 * s(), BORDER);
    text("SENSORS REPORT", r.x + 18.0 * s(), r.y + 26.0 * s(), 22.0, PAPER, FontKind::Title, Align::Left);
    let unread = rep.unread();
    let threats = rep.threats();
    let mut sub = vec![];
    if threats > 0 {
        sub.push(plural(threats, "threat"));
    }
    if unread > 0 {
        sub.push(format!("{unread} new since {}", fmt_clock(rep.seen_until)));
    }
    if sub.is_empty() {
        sub.push("Nothing new".into());
    }
    text(&sub.join(" · "), r.x + 18.0 * s(), r.y + 50.0 * s(), 14.0, if threats > 0 { DANGER } else { MUTED }, FontKind::Body, Align::Left);
    let mut shut = widgets::round_button(inp, vec2(r.x + r.w - 24.0 * s(), r.y + 24.0 * s()), 14.0 * s(), "X", PAPER, 0);

    // tabs
    let pad = 16.0 * s();
    let gap = 8.0 * s();
    let tw = (r.w - 2.0 * pad - 2.0 * gap) / 3.0;
    let ty = r.y + 66.0 * s();
    for (i, (tab, label)) in TABS.iter().enumerate() {
        let tr = Rect::new(r.x + pad + i as f32 * (tw + gap), ty, tw, 36.0 * s());
        let n = count(*tab);
        let on = g.report.tab == *tab;
        let col = if *tab == ReportTab::Now && threats > 0 { DANGER } else { ACCENT };
        let label = if n > 0 { format!("{label} {n}") } else { label.to_string() };
        if widgets::chip(inp, tr, &label, on, col) && !on {
            g.report.tab = *tab;
            g.report.scroll = 0.0;
            art::sfx("menu_tab");
        }
    }

    let bh = 50.0 * s();
    let br = Rect::new(r.x + 16.0 * s(), r.y + r.h - bh - 14.0 * s(), r.w - 32.0 * s(), bh);
    shut |= widgets::button(inp, br, "TO THE MAP", Style::Ghost);
    shut |= inp.take_back();

    // list, scrolled by dragging (or the mouse wheel) inside it
    let list = Rect::new(r.x + 10.0 * s(), ty + 48.0 * s(), r.w - 20.0 * s(), br.y - ty - 58.0 * s());
    if let (Some(d), Some(p)) = (inp.drag, inp.drag_pos) {
        if list.contains(p) {
            g.report.scroll -= d.y;
        }
    }
    if let Some((c, k)) = inp.zoom {
        if list.contains(c) {
            g.report.scroll -= k.ln() / 1.15f32.ln() * 60.0 * s();
        }
    }
    let row_text_w = list.w - 64.0 * s();
    // lay out first so the scroll can be clamped
    let mut rows: Vec<(f32, Option<&str>, Option<&ReportItem>, Vec<String>)> = vec![];
    let mut h = 0.0;
    for sec in SECTIONS.iter().filter(|s| s.tab() == g.report.tab) {
        let mut items: Vec<&ReportItem> = rep.items.iter().filter(|i| i.section == *sec).collect();
        if *sec == ReportSection::Log {
            items.sort_by(|a, b| b.t.total_cmp(&a.t));
        }
        if items.is_empty() {
            continue;
        }
        if !sec.label().is_empty() {
            rows.push((h, Some(sec.label()), None, vec![]));
            h += 30.0 * s();
        }
        for it in items {
            let lines = wrap(&it.text, FontKind::Body, 14.0, row_text_w);
            let rh = (26.0 + 18.0 * lines.len() as f32 + 10.0) * s();
            rows.push((h, None, Some(it), lines));
            h += rh;
        }
    }
    g.report.scroll = g.report.scroll.clamp(0.0, (h - list.h).max(0.0));
    let y0 = list.y - g.report.scroll;
    let mut tapped: Option<Focus> = None;
    let time = get_time() as f32;
    cel::clip(Some(list));
    for (i, (y, label, item, lines)) in rows.iter().enumerate() {
        let y = y0 + y;
        let next = rows.get(i + 1).map(|r| y0 + r.0).unwrap_or(y0 + h);
        if next < list.y || y > list.y + list.h {
            continue;
        }
        if let Some(label) = label {
            text(label, list.x + 8.0 * s(), y + 18.0 * s(), 13.0, MUTED, FontKind::Title, Align::Left);
            continue;
        }
        let Some(it) = item else { continue };
        let row = Rect::new(list.x, y, list.w, next - y - 4.0 * s());
        let tone = match it.tone {
            t if t < 0 => DANGER,
            t if t > 0 => GOOD,
            _ => MUTED,
        };
        let fresh = it.section == ReportSection::Log && it.t > rep.seen_until;
        cel::panel(row, alpha(PANEL_HI, if fresh { 0.85 } else { 0.5 }));
        draw_rectangle(row.x, row.y + 6.0 * s(), 3.0 * s(), row.h - 12.0 * s(), tone);
        let icon = vec2(row.x + 30.0 * s(), row.y + 26.0 * s());
        row_icon(view, it.focus.or(it.fallback), icon, time);
        let tx = row.x + 56.0 * s();
        text(&it.title.to_uppercase(), tx, row.y + 18.0 * s(), 13.0, tone, FontKind::Title, Align::Left);
        let (when, wcol) = when_label(it, view.now);
        let wx = row.x + row.w - 10.0 * s();
        text(&when, wx, row.y + 18.0 * s(), 12.0, wcol, FontKind::Title, Align::Right);
        if fresh {
            let ww = measure(&when, FontKind::Title, 12.0).x;
            text("NEW", wx - ww - 10.0 * s(), row.y + 18.0 * s(), 12.0, ACCENT, FontKind::Title, Align::Right);
        }
        for (k, l) in lines.iter().enumerate() {
            text(l, tx, row.y + (38.0 + 18.0 * k as f32) * s(), 14.0, PAPER, FontKind::Body, Align::Left);
        }
        let visible = Rect::new(row.x, row.y.max(list.y), row.w, (row.y + row.h).min(list.y + list.h) - row.y.max(list.y));
        if it.focus.is_some() && visible.h > 0.0 && inp.tapped(visible) {
            inp.consume_tap();
            tapped = match it.focus {
                Some(Focus::Ship(id)) if view.ships.iter().all(|x| x.id != id) => it.fallback,
                f => f,
            };
        }
    }
    cel::clip(None);
    if rows.is_empty() {
        let msg = match g.report.tab {
            ReportTab::Now => "All quiet. Nothing incoming, no fighting in sight, none of your ships under way.",
            ReportTab::Contacts => "No contacts. Build telescopes or send scouts to see further.",
            ReportTab::Log => "Nothing logged yet.",
        };
        for (k, l) in wrap(msg, FontKind::Body, 14.0, list.w - 16.0 * s()).iter().enumerate() {
            text(l, list.x + 8.0 * s(), list.y + (24.0 + 18.0 * k as f32) * s(), 14.0, MUTED, FontKind::Body, Align::Left);
        }
    }
    if let Some(f) = tapped {
        art::sfx("menu_select");
        g.focus(be, f, view.now + g.scrub);
        g.follow = true;
        close(g, false);
    } else if shut {
        close(g, true);
    }
}

/// Right-hand time label of a row and its colour.
fn when_label(it: &ReportItem, now: f64) -> (String, Color) {
    match it.when {
        When::At => (fmt_clock(it.t), MUTED),
        When::Eta if it.t <= now + 30.0 => ("ARRIVING".into(), if it.tone < 0 { DANGER } else { ACCENT }),
        When::Eta => (format!("IN {}", fmt_dur(it.t - now)).to_uppercase(), if it.tone < 0 { DANGER } else { ACCENT }),
        When::Seen if now - it.t < 60.0 => ("JUST NOW".into(), MUTED),
        When::Seen => (format!("{} AGO", fmt_dur(now - it.t)).to_uppercase(), MUTED),
        When::Live => ("LIVE".into(), GOOD),
        When::Never => (String::new(), MUTED),
    }
}

fn row_icon(view: &GameView, focus: Option<Focus>, c: Vec2, time: f32) {
    match focus {
        Some(Focus::Body(id)) => {
            if let Some(b) = view.body(id) {
                let col = b.owner.and_then(|o| view.player(o)).map(|p| p.color);
                map::draw_body(b, c, 13.0 * s(), c + vec2(-200.0, -80.0), col, time);
            }
        }
        Some(Focus::Ship(id)) => {
            if let Some(sh) = view.ships.iter().find(|x| x.id == id) {
                let player = view.player(sh.owner);
                let skin = player.map(|p| art::skin_of(&p.faction)).unwrap_or("vanguard");
                let tint = if sh.seen_at.is_some() { Color::new(1.0, 1.0, 1.0, 0.5) } else { WHITE };
                if let Some(t) = art::ship_tex(sh.class, skin, player.map(|p| p.color)) {
                    art::draw_centered(&t, c, vec2(40.0 * s(), 40.0 * s()), -0.6, tint);
                }
            } else {
                draw_circle_lines(c.x, c.y, 10.0 * s(), 1.5 * s(), MUTED);
            }
        }
        None => draw_circle_lines(c.x, c.y, 10.0 * s(), 1.5 * s(), MUTED),
    }
}

fn plural(n: usize, w: &str) -> String {
    if n == 1 {
        format!("1 {w}")
    } else {
        format!("{n} {w}s")
    }
}

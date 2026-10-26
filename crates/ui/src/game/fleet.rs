//! Fleet sheet: pick ships parked together and group them, or split / merge / rename a fleet.
//! Opened from FLEET on the ship bar or "Ships here" on a body sheet.

use super::panels::{close_x, sheet_rect};
use super::GameScreen;
use crate::art;
use crate::backend::UiAction;
use crate::cel;
use crate::input::Input;
use crate::model::*;
use crate::theme::*;
use crate::widgets::{self, Style, TextField};
use macroquad::prelude::*;

const ROW_H: f32 = 40.0;
const MAX_ROWS: usize = 6;

pub struct FleetSheet {
    /// The token the sheet is about (a fleet or a single ship).
    pub anchor: ShipId,
    /// Selection when it opened; the sheet closes when the selection changes.
    pub opened_on: Option<Focus>,
    /// Checked members of the anchor fleet (split off) and checked other tokens here (add/form).
    pub members: Vec<ShipId>,
    pub others: Vec<ShipId>,
    pub name: TextField,
}

impl FleetSheet {
    pub fn new(anchor: ShipId, opened_on: Option<Focus>) -> Self {
        let mut name = TextField::new(false);
        name.max_len = 32;
        FleetSheet { anchor, opened_on, members: vec![], others: vec![], name }
    }
}

/// Own tokens parked at `body` (what the sheet can group).
pub fn ships_here(view: &GameView, body: BodyId) -> Vec<&Ship> {
    view.ships
        .iter()
        .filter(|s| s.owner == view.me && s.seen_at.is_none() && s.parked_at == Some(body) && !super::map::is_flying(s, view.now))
        .collect()
}

/// Whether the FLEET button makes sense for this token: it is a fleet, or has company to group with.
pub fn can_group(view: &GameView, sh: &Ship) -> bool {
    sh.owner == view.me && (sh.is_fleet() || sh.parked_at.is_some_and(|b| ships_here(view, b).len() > 1))
}

fn toggle(list: &mut Vec<ShipId>, id: ShipId) {
    if let Some(i) = list.iter().position(|&x| x == id) {
        list.remove(i);
        art::sfx("toggle_off");
    } else {
        list.push(id);
        art::sfx("toggle_on");
    }
}

/// One checkable row: icon, name, class and a fuel bar. Returns true when tapped.
#[allow(clippy::too_many_arguments)]
fn row(inp: &mut Input, view: &GameView, r: Rect, on: bool, class: ShipClass, name: &str, sub: &str, fuel: f32, fuel_max: f32) -> bool {
    cel::round_rect(r, 6.0 * s(), if on { alpha(ACCENT, 0.10) } else { Color::new(1.0, 1.0, 1.0, 0.04) });
    // checkbox
    let cb = Rect::new(r.x + 8.0 * s(), r.center().y - 9.0 * s(), 18.0 * s(), 18.0 * s());
    if on {
        cel::round_rect(cb, 4.0 * s(), ACCENT);
        let ink = hex(0x1a1430);
        let (a, m, e) = (cb.point() + vec2(4.0, 9.5) * s(), cb.point() + vec2(7.5, 13.0) * s(), cb.point() + vec2(14.0, 5.0) * s());
        draw_line(a.x, a.y, m.x, m.y, 2.2 * s(), ink);
        draw_line(m.x, m.y, e.x, e.y, 2.2 * s(), ink);
    } else {
        cel::round_rect_lines(cb, 4.0 * s(), 1.2 * s(), MUTED);
    }
    let player = view.player(view.me);
    let skin = player.map(|p| art::skin_of(&p.faction)).unwrap_or("vanguard");
    let ic = vec2(r.x + 46.0 * s(), r.center().y);
    match art::ship_tex(class, skin, player.map(|p| p.color)) {
        Some(t) => art::draw_centered(&t, ic, vec2(30.0, 30.0) * s(), -0.5, WHITE),
        None => cel::ship(ic, vec2(1.0, -0.5), 9.0 * s(), PAPER),
    }
    text(&name.to_uppercase(), r.x + 70.0 * s(), r.y + 14.0 * s(), 14.0, PAPER, FontKind::Title, Align::Left);
    text(sub, r.x + 70.0 * s(), r.y + 29.0 * s(), 12.0, MUTED, FontKind::Body, Align::Left);
    let fr = Rect::new(r.x + r.w - 118.0 * s(), r.center().y - 3.0 * s(), 70.0 * s(), 6.0 * s());
    widgets::bar(fr, fuel / fuel_max.max(1.0), SUN_SHADE);
    text(&format!("{:.0}", fuel), r.x + r.w - 10.0 * s(), r.center().y, 12.0, PAPER, FontKind::Title, Align::Right);
    let hit = inp.tapped(r);
    if hit {
        inp.consume_tap();
    }
    hit
}

pub fn fleet_sheet(g: &mut GameScreen, inp: &mut Input, view: &GameView) {
    let Some(mut fs) = g.fleet.take() else { return };
    let Some(anchor) = view.ship(fs.anchor).cloned() else { return };
    let here: Vec<Ship> = anchor.parked_at.map(|b| ships_here(view, b)).unwrap_or_default().into_iter().filter(|s| s.id != anchor.id).cloned().collect();
    // forget picks that went away (ships left, view refreshed)
    fs.members.retain(|id| anchor.members.iter().any(|m| m.id == *id));
    fs.others.retain(|id| here.iter().any(|s| s.id == *id));

    let n_members = anchor.members.len().min(MAX_ROWS);
    let n_here = here.len().min(MAX_ROWS);
    let sections = (n_members > 0) as usize + (n_here > 0) as usize;
    let h = 128.0 + sections as f32 * 26.0 + (n_members + n_here) as f32 * (ROW_H + 6.0) + 56.0;
    let r = sheet_rect(h);
    inp.block(r);
    cel::panel(r, alpha(PANEL, 0.98));
    cel::round_rect_lines(r, 10.0 * s(), 1.0 * s(), BORDER);

    let pad = 16.0 * s();
    let place = match anchor.parked_at.and_then(|b| view.body(b)) {
        Some(b) if !super::map::is_flying(&anchor, view.now) => format!("at {}", b.name),
        _ => "in flight".to_string(),
    };
    let (title, sub) = if anchor.is_fleet() {
        (format!("{} ×{}", anchor.name.to_uppercase(), anchor.count), format!("Fleet · {place} · flies and fights as one"))
    } else {
        ("NEW FLEET".to_string(), format!("Pick ships {place} to group with {}", anchor.name))
    };
    text(&title, r.x + pad, r.y + 24.0 * s(), 20.0, PAPER, FontKind::Title, Align::Left);
    text(&sub, r.x + pad, r.y + 46.0 * s(), 13.0, MUTED, FontKind::Body, Align::Left);

    // name field (+ RENAME for an existing fleet)
    let fy = r.y + 64.0 * s();
    let bw = 96.0 * s();
    let fw = if anchor.is_fleet() { r.w - 2.0 * pad - bw - 10.0 * s() } else { r.w - 2.0 * pad };
    let placeholder = if anchor.is_fleet() { anchor.name.clone() } else { "Fleet name".to_string() };
    widgets::text_field(inp, Rect::new(r.x + pad, fy, fw, 40.0 * s()), &mut fs.name, &placeholder);
    if anchor.is_fleet() {
        let br = Rect::new(r.x + r.w - pad - bw, fy, bw, 40.0 * s());
        let ok = !fs.name.text.trim().is_empty();
        if widgets::button(inp, br, "RENAME", if ok { Style::Secondary } else { Style::Ghost }) && ok {
            g.emit(UiAction::RenameFleet { fleet: anchor.id, name: fs.name.text.trim().to_string() });
            fs.name.text.clear();
        }
    }

    let mut y = fy + 52.0 * s();
    let rw = r.w - 2.0 * pad;
    if n_members > 0 {
        text("SHIPS IN FLEET", r.x + pad, y + 8.0 * s(), 13.0, MUTED, FontKind::Title, Align::Left);
        y += 20.0 * s();
        for m in anchor.members.iter().take(MAX_ROWS) {
            let rr = Rect::new(r.x + pad, y, rw, ROW_H * s());
            if row(inp, view, rr, fs.members.contains(&m.id), m.class, &m.name, m.class.label(), m.fuel, m.fuel_max) {
                toggle(&mut fs.members, m.id);
            }
            y += (ROW_H + 6.0) * s();
        }
        if anchor.members.len() > MAX_ROWS {
            text(&format!("+{} more", anchor.members.len() - MAX_ROWS), r.x + r.w - pad, y - 2.0 * s(), 12.0, MUTED, FontKind::Body, Align::Right);
        }
        y += 6.0 * s();
    }
    if n_here > 0 {
        let label = match anchor.parked_at.and_then(|b| view.body(b)) {
            Some(b) => format!("ALSO AT {}", b.name.to_uppercase()),
            None => "ALSO HERE".into(),
        };
        text(&label, r.x + pad, y + 8.0 * s(), 13.0, MUTED, FontKind::Title, Align::Left);
        y += 20.0 * s();
        for o in here.iter().take(MAX_ROWS) {
            let rr = Rect::new(r.x + pad, y, rw, ROW_H * s());
            let sub = if o.is_fleet() { format!("Fleet of {}", o.count) } else { o.class.label().to_string() };
            if row(inp, view, rr, fs.others.contains(&o.id), o.class, &o.name, &sub, o.fuel, o.fuel_max) {
                toggle(&mut fs.others, o.id);
            }
            y += (ROW_H + 6.0) * s();
        }
    }
    if n_members == 0 && n_here == 0 {
        text("No other ships of yours are parked here.", r.x + pad, y + 10.0 * s(), 14.0, MUTED, FontKind::Body, Align::Left);
    }

    // actions
    let by = r.y + r.h - 54.0 * s();
    let mut close = false;
    let name = fs.name.text.trim().to_string();
    if anchor.is_fleet() {
        let w3 = (rw - 20.0 * s()) / 3.0;
        let b = |i: usize| Rect::new(r.x + pad + i as f32 * (w3 + 10.0 * s()), by, w3, 42.0 * s());
        let k = fs.members.len();
        let split_ok = k > 0 && k < anchor.members.len();
        if widgets::button(inp, b(0), &if k > 0 { format!("SPLIT OFF {k}") } else { "SPLIT OFF".into() }, if split_ok { Style::Secondary } else { Style::Ghost }) && split_ok {
            g.emit(UiAction::LeaveFleet { ships: fs.members.clone(), split: (k >= 2).then(|| if name.is_empty() { format!("{} B", anchor.name) } else { name.clone() }) });
            close = true;
        }
        let j = fs.others.len();
        if widgets::button(inp, b(1), &if j > 0 { format!("ADD {j}") } else { "ADD".into() }, if j > 0 { Style::Primary } else { Style::Ghost }) && j > 0 {
            // other fleets merge in; loose ships join
            let (fleets, loose): (Vec<&Ship>, Vec<&Ship>) = here.iter().filter(|s| fs.others.contains(&s.id)).partition(|s| s.is_fleet());
            for f in fleets {
                g.emit(UiAction::MergeFleets { into: anchor.id, from: f.id });
            }
            if !loose.is_empty() {
                g.emit(UiAction::JoinFleet { fleet: anchor.id, ships: loose.iter().map(|s| s.id).collect() });
            }
            art::sfx("menu_select");
            fs.others.clear();
        }
        if widgets::button(inp, b(2), "DISBAND", Style::Danger) {
            g.emit(UiAction::DisbandFleet { fleet: anchor.id });
            close = true;
        }
    } else {
        let j = fs.others.len();
        let ok = j > 0;
        let label = if ok { format!("FORM FLEET OF {}", j + here.iter().filter(|s| fs.others.contains(&s.id)).map(|s| s.count as usize - 1).sum::<usize>() + 1) } else { "PICK SHIPS TO GROUP".into() };
        if widgets::button(inp, Rect::new(r.x + pad, by, rw, 42.0 * s()), &label, if ok { Style::Primary } else { Style::Ghost }) && ok {
            let ships = std::iter::once(anchor.id).chain(fs.others.iter().copied()).collect();
            g.emit(UiAction::FormFleet { ships, name });
            art::sfx("menu_select");
            close = true;
        }
    }
    if close_x(inp, r) {
        close = true;
    }
    if close {
        art::sfx("menu_close");
        g.selected = None;
    } else {
        g.fleet = Some(fs);
    }
}

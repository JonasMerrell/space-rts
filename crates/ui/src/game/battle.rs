//! Live battle screen (rules 6): the arena around a fight, drawn from `Backend::battles`. Our ships
//! can be picked (one, or its whole fleet), set to a stance, steered by hand (drag from a picked
//! ship to burn) or handed back to the autopilot, and pointed at a target. Salvos heading for our
//! ships are drawn red with the seconds until they pass.

use super::map::Camera;
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

/// Δv per screen px of a burn drag (km/s), and the most one drag orders.
const DV_PER_PX: f64 = 0.004;
const DV_MAX: f64 = 2.0;

pub struct BattleScreen {
    pub id: u32,
    cam: Option<Camera>,
    /// our picked units (ships only)
    pub sel: Vec<BattleRef>,
    /// picking a ship picks its whole fleet
    pub fleet: bool,
    /// next tap on an enemy sets the target
    pub targeting: bool,
    /// a burn drag from this screen point is under way
    aim: Option<Vec2>,
    /// the battle was there last frame (for "it's over")
    seen: bool,
}

impl BattleScreen {
    pub fn new(id: u32) -> Self {
        BattleScreen { id, cam: None, sel: vec![], fleet: true, targeting: false, aim: None, seen: false }
    }

    /// Where an arena point was drawn last frame (tests).
    pub fn to_screen(&self, p: [f64; 2]) -> Option<Vec2> {
        self.cam.map(|c| c.to_screen(p))
    }
}

/// HUD button: shown while we're in a battle, pulsing red; opens the first one.
pub fn hud_button(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, c: Vec2) {
    let battles = be.battles();
    let Some(first) = battles.first() else { return };
    let k = 0.5 + 0.5 * (get_time() as f32 * 4.0).sin();
    draw_circle_lines(c.x, c.y, 25.0 * s(), 2.5 * s(), alpha(DANGER, 0.4 + 0.5 * k));
    if widgets::round_button(inp, c, 21.0 * s(), "!", DANGER, battles.len()) {
        open(g, first.id);
    }
    inked("BATTLE", c.x, c.y + 34.0 * s(), 12.0, DANGER, Align::Center);
}

pub fn open(g: &mut GameScreen, id: u32) {
    art::sfx("menu_open");
    g.battle = Some(BattleScreen::new(id));
}

fn ship_ids(sel: &[BattleRef]) -> Vec<ShipId> {
    sel.iter().filter_map(|u| if let BattleRef::Ship(id) = u { Some(*id) } else { None }).collect()
}

fn size_of(u: &BattleUnit) -> f32 {
    let px: f32 = match u.class {
        Some(ShipClass::Scout) | Some(ShipClass::Decoy) | Some(ShipClass::Torpedo) => 9.0,
        Some(ShipClass::Interceptor) => 10.0,
        Some(ShipClass::Gunship) => 12.0,
        Some(ShipClass::Bomber) | Some(ShipClass::Tanker) => 13.0,
        Some(ShipClass::Dreadnought) => 17.0,
        Some(_) => 11.0,
        None => 12.0,
    };
    px * s()
}

/// Frame every unit (and the body) with some room around them.
pub(super) fn fit_view(b: &BattleView) -> Camera {
    fit(b)
}

fn fit(b: &BattleView) -> Camera {
    let mut lo = [-b.body_radius.max(500.0), -b.body_radius.max(500.0)];
    let mut hi = [b.body_radius.max(500.0), b.body_radius.max(500.0)];
    for u in &b.units {
        for k in 0..2 {
            lo[k] = lo[k].min(u.pos[k]);
            hi[k] = hi[k].max(u.pos[k]);
        }
    }
    let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let span = ((hi[0] - lo[0]) / screen_width() as f64).max((hi[1] - lo[1]) / (screen_height() as f64 * 0.55));
    Camera { center, zoom: 0.8 / span.max(1e-6) }
}

pub fn battle_screen(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) {
    let Some(mut st) = g.battle.take() else { return };
    let battles = be.battles();
    let Some(b) = battles.iter().find(|b| b.id == st.id) else {
        // over (or never ours): back to the map
        if st.seen {
            g.say("The battle is over");
        }
        art::sfx("menu_close");
        return;
    };
    let mut out: Vec<UiAction> = vec![];
    let st = &mut st;
    st.seen = true;
    st.sel.retain(|u| b.unit(*u).is_some());
    let cam0 = *st.cam.get_or_insert_with(|| fit(b));
    let mut cam = cam0;
    let me = view.me;

    // ---- layout: top bar, bottom sheet, the arena in between
    let top = safe_top() + 8.0 * s();
    let bar = Rect::new(8.0 * s(), top, screen_width() - 16.0 * s(), 58.0 * s());
    let sheet_h = if st.sel.is_empty() { 96.0 } else { 210.0 } * s();
    let sheet = Rect::new(8.0 * s(), screen_height() - safe_bottom() - sheet_h - 8.0 * s(), screen_width() - 16.0 * s(), sheet_h);

    // ---- input on the arena: burn drags from a picked ship, pan/zoom, taps
    let to_s = |c: &Camera, p: [f64; 2]| c.to_screen(p);
    if let Some(p) = inp.pressed {
        if !inp.occluded(p) && st.aim.is_none() {
            let near = st.sel.iter().filter_map(|u| b.unit(*u)).any(|u| to_s(&cam, u.pos).distance(p) < 36.0 * s());
            if near {
                st.aim = Some(p);
            }
        }
    }
    let aim_vec = match (st.aim, inp.held.or(inp.released)) {
        (Some(a), Some(h)) => Some(h - a),
        _ => None,
    };
    if st.aim.is_some() {
        if inp.released.is_some() || inp.held.is_none() {
            if let Some(v) = aim_vec.filter(|v| v.length() > 12.0 * s()) {
                let mut dv = [v.x as f64 / s() as f64 * DV_PER_PX, v.y as f64 / s() as f64 * DV_PER_PX];
                let n = dv[0].hypot(dv[1]);
                if n > DV_MAX {
                    dv = [dv[0] / n * DV_MAX, dv[1] / n * DV_MAX];
                }
                out.push(UiAction::BattleBurn { ships: ship_ids(&st.sel), dv });
                art::sfx("ui_tap");
            }
            st.aim = None;
        }
        inp.consume_tap();
    } else if inp.for_map() {
        if let Some((c, k)) = inp.zoom {
            let f = fit(b).zoom;
            cam.zoom_at(c, k, f * 0.1, f * 50.0);
        }
        if let Some(d) = inp.drag {
            cam.pan_px(d);
        }
        if let Some(p) = inp.tap.filter(|p| !inp.occluded(*p)) {
            inp.consume_tap();
            let hit = b.units.iter().filter(|u| to_s(&cam, u.pos).distance(p) < size_of(u) + 14.0 * s()).min_by(|x, y| {
                to_s(&cam, x.pos).distance(p).total_cmp(&to_s(&cam, y.pos).distance(p))
            });
            match hit {
                Some(u) if u.owner == me && matches!(u.unit, BattleRef::Ship(_)) => {
                    st.targeting = false;
                    st.sel = match (st.fleet, u.fleet) {
                        (true, Some(f)) => b.units.iter().filter(|x| x.fleet == Some(f)).map(|x| x.unit).collect(),
                        _ => vec![u.unit],
                    };
                    art::sfx("menu_select");
                }
                Some(u) if u.owner != me && !st.sel.is_empty() => {
                    out.push(UiAction::BattleTarget { ships: ship_ids(&st.sel), target: Some(u.unit) });
                    st.targeting = false;
                    art::sfx("ui_click");
                }
                Some(_) => {}
                None => {
                    st.sel.clear();
                    st.targeting = false;
                }
            }
        }
    }
    st.cam = Some(cam);

    draw_arena(b, view, &cam, &st.sel);
    // the burn being aimed
    if let (Some(a), Some(v)) = (st.aim, aim_vec) {
        let len = (v.length() / s()) as f64 * DV_PER_PX;
        let dv = len.min(DV_MAX);
        cel::arrow(a, a + v, 4.0 * s(), ACCENT);
        let dv_left = st.sel.iter().filter_map(|u| b.unit(*u)).map(|u| u.dv_left).fold(f32::INFINITY, f32::min);
        let note = if dv_left.is_finite() { format!("BURN {dv:.2} km/s · {dv_left:.1} left") } else { format!("BURN {dv:.2} km/s") };
        inked(&note, a.x + v.x, a.y + v.y - 22.0 * s(), 16.0, if (dv as f32) > dv_left { DANGER } else { ACCENT }, Align::Center);
    }

    // ---- top bar
    inp.block(bar);
    cel::panel(bar, alpha(PANEL, 0.95));
    let clock = b.now - b.start_t;
    text(&format!("BATTLE · {}", b.place.to_uppercase()), bar.x + 14.0 * s(), bar.y + 22.0 * s(), 19.0, DANGER, FontKind::Title, Align::Left);
    let mine_n = b.units.iter().filter(|u| u.owner == me && matches!(u.unit, BattleRef::Ship(_))).count();
    let theirs_n = b.units.iter().filter(|u| u.owner != me && matches!(u.unit, BattleRef::Ship(_))).count();
    let lost_me = b.lost.get(me as usize).copied().unwrap_or(0);
    let lost_them: u32 = b.lost.iter().enumerate().filter(|(p, _)| *p != me as usize).map(|(_, l)| l).sum();
    let sub = format!("{} · {mine_n} vs {theirs_n} ships · lost {lost_me} / {lost_them}", fmt_dur(clock));
    text(&sub, bar.x + 14.0 * s(), bar.y + 44.0 * s(), 13.0, MUTED, FontKind::Body, Align::Left);
    let mut shut = widgets::round_button(inp, vec2(bar.x + bar.w - 26.0 * s(), bar.center().y), 15.0 * s(), "X", PAPER, 0);
    shut |= inp.take_back();
    if widgets::round_button(inp, vec2(bar.x + bar.w - 70.0 * s(), bar.center().y), 15.0 * s(), "[ ]", PAPER, 0) {
        st.cam = Some(fit(b));
    }

    // ---- bottom sheet
    inp.block(sheet);
    cel::panel(sheet, alpha(PANEL, 0.96));
    let pad = 14.0 * s();
    if st.sel.is_empty() {
        let hint = if mine_n > 0 { "Tap one of your ships to command it" } else { "Only your defenses are fighting here" };
        text(hint, sheet.x + pad, sheet.y + 26.0 * s(), 16.0, PAPER, FontKind::Title, Align::Left);
        text("Slugs coming for you turn red: drag from your ship to dodge", sheet.x + pad, sheet.y + 50.0 * s(), 13.0, MUTED, FontKind::Body, Align::Left);
        // quick pick: each of our fleets / loose ships
        let mut x = sheet.x + pad;
        let y = sheet.y + 62.0 * s();
        let mut done: Vec<Option<ShipId>> = vec![];
        for u in b.units.iter().filter(|u| u.owner == me && matches!(u.unit, BattleRef::Ship(_))) {
            if u.fleet.is_some() && done.contains(&u.fleet) {
                continue;
            }
            done.push(u.fleet);
            let n = if u.fleet.is_some() { b.units.iter().filter(|x| x.fleet == u.fleet).count() } else { 1 };
            let label = if n > 1 { format!("{} ×{n}", u.label) } else { u.label.clone() };
            let w = measure(&label, FontKind::Title, 13.0).x + 26.0 * s();
            if x + w > sheet.x + sheet.w - pad {
                break;
            }
            if widgets::chip(inp, Rect::new(x, y, w, 26.0 * s()), &label, false, ACCENT) {
                st.sel = match u.fleet {
                    Some(f) if st.fleet => b.units.iter().filter(|x| x.fleet == Some(f)).map(|x| x.unit).collect(),
                    _ => vec![u.unit],
                };
                if let Some(c) = st.cam.as_mut() {
                    c.center = u.pos;
                }
            }
            x += w + 8.0 * s();
        }
    } else {
        let units: Vec<&BattleUnit> = st.sel.iter().filter_map(|u| b.unit(*u)).collect();
        let first = units[0];
        let title = if units.len() > 1 { format!("{} ships", units.len()) } else { first.label.clone() };
        text(&title, sheet.x + pad, sheet.y + 24.0 * s(), 18.0, PAPER, FontKind::Title, Align::Left);
        let hp: f32 = units.iter().map(|u| u.hp).sum();
        let hp_max: f32 = units.iter().map(|u| u.hp_max).sum();
        let dv_left = units.iter().map(|u| u.dv_left).fold(f32::INFINITY, f32::min);
        let status = if units.iter().all(|u| u.manual) { "YOU STEER" } else if units.iter().any(|u| u.manual) { "PARTLY STEERED" } else { "AUTOPILOT" };
        text(status, sheet.x + sheet.w - pad, sheet.y + 24.0 * s(), 14.0, if status == "AUTOPILOT" { GOOD } else { ACCENT }, FontKind::Title, Align::Right);
        let bw = (sheet.w - 3.0 * pad) / 2.0;
        let by = sheet.y + 38.0 * s();
        text("HULL", sheet.x + pad, by + 8.0 * s(), 11.0, MUTED, FontKind::Title, Align::Left);
        widgets::bar(Rect::new(sheet.x + pad, by + 14.0 * s(), bw, 8.0 * s()), hp / hp_max.max(1e-3), widgets::hp_color(hp / hp_max.max(1e-3)));
        text(&format!("DV LEFT {dv_left:.1} KM/S"), sheet.x + 2.0 * pad + bw, by + 8.0 * s(), 11.0, MUTED, FontKind::Title, Align::Left);
        let fuel: f32 = units.iter().map(|u| u.fuel).sum();
        let fuel_max: f32 = units.iter().map(|u| u.fuel_max).sum();
        widgets::bar(Rect::new(sheet.x + 2.0 * pad + bw, by + 14.0 * s(), bw, 8.0 * s()), fuel / fuel_max.max(1e-3), ACCENT);
        // stance
        let sy = by + 34.0 * s();
        let cw = (sheet.w - 2.0 * pad - 3.0 * 8.0 * s()) / 4.0;
        for (i, stance) in Stance::ALL.iter().enumerate() {
            let on = units.iter().all(|u| u.stance == *stance);
            if widgets::chip(inp, Rect::new(sheet.x + pad + i as f32 * (cw + 8.0 * s()), sy, cw, 32.0 * s()), stance.label(), on, ACCENT) && !on {
                out.push(UiAction::SetStance { ships: ship_ids(&st.sel), stance: *stance });
                art::sfx("menu_tab");
            }
        }
        // steer / autopilot, whole fleet, target
        let ry = sy + 42.0 * s();
        let rw = (sheet.w - 2.0 * pad - 2.0 * 8.0 * s()) / 3.0;
        let manual = units.iter().all(|u| u.manual);
        let r0 = Rect::new(sheet.x + pad, ry, rw, 40.0 * s());
        if widgets::button(inp, r0, if manual { "AUTOPILOT" } else { "STEER" }, if manual { Style::Secondary } else { Style::Primary }) {
            out.push(UiAction::BattleSteer { ships: ship_ids(&st.sel), manual: !manual });
        }
        let r1 = Rect::new(r0.x + rw + 8.0 * s(), ry, rw, 40.0 * s());
        if widgets::chip(inp, r1, "WHOLE FLEET", st.fleet, ACCENT) {
            st.fleet = !st.fleet;
            if st.fleet {
                let fleets: Vec<ShipId> = units.iter().filter_map(|u| u.fleet).collect();
                st.sel = b.units.iter().filter(|x| st.sel.contains(&x.unit) || x.fleet.is_some_and(|f| fleets.contains(&f))).map(|x| x.unit).collect();
            } else {
                st.sel.truncate(1);
            }
        }
        let r2 = Rect::new(r1.x + rw + 8.0 * s(), ry, rw, 40.0 * s());
        let has_target = units.iter().any(|u| u.target.is_some());
        if widgets::button(inp, r2, if st.targeting { "TAP AN ENEMY" } else if has_target { "FREE FIRE" } else { "TARGET" }, Style::Secondary) {
            if has_target && !st.targeting {
                out.push(UiAction::BattleTarget { ships: ship_ids(&st.sel), target: None });
            } else {
                st.targeting = !st.targeting;
            }
        }
        let hint = if st.targeting { "Tap an enemy to focus fire on it" } else { "Drag from your ship to burn · tap an enemy to target it" };
        text(hint, sheet.x + pad, ry + 58.0 * s(), 13.0, MUTED, FontKind::Body, Align::Left);
    }

    for a in out {
        g.emit(a);
    }
    if shut {
        art::sfx("menu_close");
    } else {
        g.battle = Some(std::mem::replace(st, BattleScreen::new(0)));
    }
}

/// The arena: scale rings, the body, weapon reach of `sel`, salvos, recent hits and the units (the
/// live battle screen and replays).
pub(super) fn draw_arena(b: &BattleView, view: &GameView, cam: &Camera, sel: &[BattleRef]) {
    let me = view.me;
    let team_of = |o: PlayerId| team(view.color_of(Some(o))).0;
    cel::backdrop();
    let t_anim = get_time() as f32;
    // scale rings around the centre, every 1000 km
    let c0 = cam.to_screen([0.0, 0.0]);
    for k in 1..=12 {
        let r = (k as f64 * 1000.0 * cam.zoom) as f32;
        if r > 20.0 * s() && r < screen_width().max(screen_height()) * 1.5 {
            draw_circle_lines(c0.x, c0.y, r, 1.0, Color::new(1.0, 1.0, 1.0, if k % 5 == 0 { 0.10 } else { 0.05 }));
        }
    }
    if b.body_radius > 0.0 {
        let r = (b.body_radius * cam.zoom) as f32;
        let (l, d) = match b.body.and_then(|x| view.body(x)) {
            Some(body) => team(view.color_of(body.owner)),
            None => (Color::new(0.6, 0.6, 0.7, 1.0), Color::new(0.3, 0.3, 0.4, 1.0)),
        };
        cel::body(c0, r.max(6.0 * s()), alpha(l, 0.9), alpha(d, 0.9));
    }
    // weapon reach of what we picked
    for u in sel.iter().filter_map(|u| b.unit(*u)) {
        if u.weapon != WeaponKind::None {
            let p = cam.to_screen(u.pos);
            let ring: Vec<Vec2> = (0..=64).map(|i| p + Vec2::from_angle(i as f32 / 64.0 * std::f32::consts::TAU) * (u.range * cam.zoom) as f32).collect();
            cel::dashed_path(&ring, 1.5 * s(), 6.0 * s(), alpha(ACCENT, 0.35));
        }
    }
    // salvos: a dot and where it'll be in 20 s; red with a countdown when it will burst on us
    for sv in &b.salvos {
        let p = cam.to_screen(sv.pos);
        let ahead = cam.to_screen([sv.pos[0] + sv.vel[0] * 20.0, sv.pos[1] + sv.vel[1] * 20.0]);
        let col = if sv.threat.is_some() { DANGER } else if sv.owner == me { alpha(PAPER, 0.8) } else { alpha(team_of(sv.owner), 0.9) };
        draw_line(p.x, p.y, ahead.x, ahead.y, 1.5 * s(), alpha(col, 0.35));
        draw_circle(p.x, p.y, 3.5 * s(), INK);
        draw_circle(p.x, p.y, 2.5 * s(), col);
    }
    // laser shots and salvo bursts of the last few seconds
    for h in &b.hits {
        let a = (1.0 - h.age / 1.5).clamp(0.0, 1.0) as f32;
        let (p, q) = (cam.to_screen(h.from), cam.to_screen(h.to));
        if h.beam {
            draw_line(p.x, p.y, q.x, q.y, 3.0 * s(), alpha(WHITE, 0.5 * a));
            draw_line(p.x, p.y, q.x, q.y, 1.5 * s(), alpha(ACCENT, a));
        } else if a > 0.0 {
            cel::burst(q, (10.0 + 10.0 * (1.0 - a)) * s(), 8, alpha(ACCENT, a), t_anim * 3.0);
        }
        let k = (h.age / 2.5).clamp(0.0, 1.0) as f32;
        if k < 1.0 {
            inked(&format!("-{}", widgets::hp_num(h.dmg)), q.x, q.y - 18.0 * s() - 24.0 * s() * k, 14.0, alpha(DANGER, 1.0 - k), Align::Center);
        }
    }
    // units
    for u in &b.units {
        let p = cam.to_screen(u.pos);
        let size = size_of(u);
        let picked = sel.contains(&u.unit);
        if picked {
            let pulse = 1.0 + 0.1 * (t_anim * 6.0).sin();
            draw_circle_lines(p.x, p.y, (size + 9.0 * s()) * pulse, 3.0 * s(), ACCENT);
        }
        let l = team_of(u.owner);
        match (u.unit, u.class) {
            (BattleRef::Ship(_), Some(class)) => {
                let v = vec2(u.vel[0] as f32, u.vel[1] as f32);
                let dir = match u.target.and_then(|t| b.unit(t)) {
                    Some(t) => vec2((t.pos[0] - u.pos[0]) as f32, (t.pos[1] - u.pos[1]) as f32),
                    None if v.length() > 1e-3 => v,
                    None => vec2(if u.owner == me { 1.0 } else { -1.0 }, 0.0),
                };
                let player = view.player(u.owner);
                let skin = player.map(|p| art::skin_of(&p.faction)).unwrap_or("vanguard");
                match art::ship_tex(class, skin, player.map(|p| p.color)) {
                    Some(t) => art::draw_centered(&t, p, vec2(size, size) * 2.8, dir.y.atan2(dir.x), WHITE),
                    None => cel::ship(p, dir, size, if u.owner == me { PAPER } else { l }),
                }
                // where it's drifting (30 s ahead) and the burn still to come
                let drift = cam.to_screen([u.pos[0] + u.vel[0] * 30.0, u.pos[1] + u.vel[1] * 30.0]);
                if drift.distance(p) > 3.0 * s() {
                    draw_line(p.x, p.y, drift.x, drift.y, 1.5 * s(), alpha(l, 0.5));
                }
                let bl = vec2(u.burn_left[0] as f32, u.burn_left[1] as f32);
                if bl.length() > 1e-3 {
                    cel::arrow(p, p + bl / DV_PER_PX as f32 * s(), 3.0 * s(), alpha(ACCENT, 0.8));
                }
                if u.manual && u.owner == me {
                    text("M", p.x - size - 4.0 * s(), p.y - size, 12.0, ACCENT, FontKind::Title, Align::Center);
                }
            }
            (BattleRef::Station(_), _) => cel::diamond(p, size, if u.owner == me { PAPER } else { l }),
            _ => {
                // planetary guns: a ring of turrets on the surface
                draw_circle_lines(p.x, p.y, (b.body_radius * cam.zoom) as f32 + 4.0 * s(), 3.0 * s(), alpha(l, 0.8));
            }
        }
        let bar_at = if matches!(u.unit, BattleRef::Defense(_)) { p + vec2(0.0, (b.body_radius * cam.zoom) as f32 + 14.0 * s()) } else { p + vec2(0.0, size + 8.0 * s()) };
        mini_bar(bar_at, (u.hp / u.hp_max.max(1e-3)).clamp(0.0, 1.0));
    }
    // seconds until each threatening salvo passes, over the ship it's coming for
    for sv in &b.salvos {
        let Some((u, dt)) = sv.threat else { continue };
        let Some(unit) = b.unit(u) else { continue };
        let p = cam.to_screen(unit.pos);
        let r = size_of(unit) + 16.0 * s();
        let k = 0.5 + 0.5 * (t_anim * 8.0).sin();
        draw_circle_lines(p.x, p.y, r, 2.0 * s(), alpha(DANGER, 0.5 + 0.4 * k));
        inked(&format!("{:.0}s", dt.max(0.0)), p.x + r, p.y - r, 14.0, DANGER, Align::Center);
    }
}

/// Small hull bar under a unit.
fn mini_bar(c: Vec2, frac: f32) {
    let (w, h) = (24.0 * s(), 4.0 * s());
    let r = Rect::new(c.x - w / 2.0, c.y - h / 2.0, w, h);
    draw_rectangle(r.x - s(), r.y - s(), r.w + 2.0 * s(), r.h + 2.0 * s(), INK);
    draw_rectangle(r.x, r.y, r.w, r.h, Color::new(1.0, 1.0, 1.0, 0.15));
    draw_rectangle(r.x, r.y, r.w * frac, r.h, widgets::hp_color(frac));
}

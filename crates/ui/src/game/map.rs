//! System map: camera, orbits, bodies, ships, trajectories and hit-testing.

use crate::backend::Backend;
use crate::art;
use crate::cel;
use crate::model::*;
use crate::theme::*;
use macroquad::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    /// World point (Gm) at the screen centre.
    pub center: [f64; 2],
    /// px per Gm.
    pub zoom: f64,
}

impl Camera {
    pub fn to_screen(&self, p: [f64; 2]) -> Vec2 {
        let c = vec2(screen_width() / 2.0, screen_height() / 2.0);
        c + vec2(((p[0] - self.center[0]) * self.zoom) as f32, ((p[1] - self.center[1]) * self.zoom) as f32)
    }
    pub fn to_world(&self, v: Vec2) -> [f64; 2] {
        let c = vec2(screen_width() / 2.0, screen_height() / 2.0);
        let d = v - c;
        [self.center[0] + d.x as f64 / self.zoom, self.center[1] + d.y as f64 / self.zoom]
    }
    pub fn pan_px(&mut self, d: Vec2) {
        self.center[0] -= d.x as f64 / self.zoom;
        self.center[1] -= d.y as f64 / self.zoom;
    }
    /// Zoom by `k` keeping the world point under `at` fixed.
    pub fn zoom_at(&mut self, at: Vec2, k: f32, lo: f64, hi: f64) {
        let before = self.to_world(at);
        self.zoom = (self.zoom * k as f64).clamp(lo, hi);
        let after = self.to_world(at);
        self.center[0] += before[0] - after[0];
        self.center[1] += before[1] - after[1];
    }
    /// Frame the whole system.
    pub fn fit(view: &GameView) -> Camera {
        let extent = view
            .bodies
            .iter()
            .filter(|b| b.kind == BodyKind::Planet)
            .flat_map(|b| b.orbit_path.iter())
            .map(|p| p[0].abs().max(p[1].abs()))
            .fold(1.0f64, f64::max);
        let short = screen_width().min(screen_height()) as f64;
        Camera { center: [0.0, 0.0], zoom: short * 0.46 / extent }
    }
}

/// Minimum on-screen radius per body kind, in dp.
fn min_radius(kind: BodyKind) -> f32 {
    match kind {
        BodyKind::Star => 18.0,
        BodyKind::Planet => 13.0,
        BodyKind::Moon => 7.0,
        BodyKind::Asteroid => 5.0,
        BodyKind::Outpost => 6.0,
    }
}

pub fn body_radius_px(b: &Body, cam: &Camera) -> f32 {
    let true_px = (b.radius * cam.zoom) as f32;
    match b.kind {
        // Planets and moons start growing as soon as you zoom in (min dot + 1.6× true size) instead of
        // sitting at the minimum until the true size catches up; close up they approach true size.
        BodyKind::Planet | BodyKind::Moon => {
            let m = min_radius(b.kind) * s();
            (m + true_px * 1.6).min(true_px.max(m) + m * 1.5)
        }
        _ => true_px.max(min_radius(b.kind) * s()),
    }
}

/// Where a ship token is drawn. Parked ships sit on a little ring around their body.
pub fn ship_screen(be: &dyn Backend, view: &GameView, cam: &Camera, ship: &Ship, t: f64) -> Option<(Vec2, Vec2)> {
    let flying = ship.trajectory.as_ref().is_some_and(|tr| tr.start().is_some_and(|s0| t >= s0) && tr.end().is_some_and(|e| t <= e));
    if !flying {
        if let Some(bid) = ship.parked_at {
            let b = view.body(bid)?;
            let c = cam.to_screen(be.body_pos(bid, t));
            let parked: Vec<&Ship> = view.ships.iter().filter(|s| s.parked_at == Some(bid) && !is_flying(s, t)).collect();
            let i = parked.iter().position(|s| s.id == ship.id).unwrap_or(0);
            let n = parked.len().max(1);
            let a = i as f32 / n as f32 * std::f32::consts::TAU - 0.9 + (t as f32 / 3600.0) * 0.4;
            let rr = body_radius_px(b, cam) + 18.0 * s();
            let dir = Vec2::from_angle(a + std::f32::consts::FRAC_PI_2);
            return Some((c + Vec2::from_angle(a) * rr, dir));
        }
    }
    let p = be.ship_pos(ship.id, t)?;
    let q = be.ship_pos(ship.id, t + 120.0).unwrap_or(p);
    let a = cam.to_screen(p);
    Some((a, cam.to_screen(q) - a))
}

pub fn is_flying(s: &Ship, t: f64) -> bool {
    s.trajectory.as_ref().is_some_and(|tr| tr.start().is_some_and(|s0| t >= s0) && tr.end().is_some_and(|e| t <= e))
}

pub fn draw_orbits(be: &dyn Backend, view: &GameView, cam: &Camera, t: f64) {
    for b in &view.bodies {
        let Some(parent) = b.parent else { continue };
        if b.orbit_path.len() < 2 {
            continue;
        }
        let pc = be.body_pos(parent, t);
        let pts: Vec<Vec2> = b.orbit_path.iter().map(|p| cam.to_screen([pc[0] + p[0], pc[1] + p[1]])).collect();
        let (min, max) = pts.iter().fold((pts[0], pts[0]), |(a, b), p| (a.min(*p), b.max(*p)));
        let size = (max - min).length();
        if size < 14.0 * s() {
            continue;
        }
        let w = match b.kind {
            BodyKind::Planet => 3.0,
            BodyKind::Moon => 2.0,
            _ => 1.5,
        } * s();
        let faint = matches!(b.kind, BodyKind::Asteroid | BodyKind::Outpost);
        let col = Color::new(1.0, 1.0, 1.0, if faint { 0.12 } else { 0.25 });
        if faint {
            cel::dashed_path(&pts, w, 6.0 * s(), col);
        } else {
            for seg in pts.windows(2) {
                draw_line(seg[0].x, seg[0].y, seg[1].x, seg[1].y, w, col);
            }
        }
    }
}

pub fn draw_bodies(be: &dyn Backend, view: &GameView, cam: &Camera, t: f64, selected: Option<Focus>) {
    let time = get_time() as f32;
    // Stars, then planets, then moons so labels stack sensibly.
    let mut order: Vec<&Body> = view.bodies.iter().collect();
    order.sort_by_key(|b| b.kind as u8);
    for b in order {
        let c = cam.to_screen(be.body_pos(b.id, t));
        let r = body_radius_px(b, cam);
        if c.x < -r * 3.0 || c.y < -r * 3.0 || c.x > screen_width() + r * 3.0 || c.y > screen_height() + r * 3.0 {
            continue;
        }
        if selected == Some(Focus::Body(b.id)) {
            let pulse = 1.0 + 0.08 * (time * 5.0).sin();
            draw_circle_lines(c.x, c.y, (r + 12.0 * s()) * pulse, 4.0 * s(), ACCENT);
        }
        draw_body(b, c, r, cam.to_screen([0.0, 0.0]), view.color_of(b.owner), time);
    }
}

/// Names and intel tags, drawn after ships so tokens don't hide them.
pub fn draw_labels(be: &dyn Backend, view: &GameView, cam: &Camera, t: f64) {
    for b in &view.bodies {
        let c = cam.to_screen(be.body_pos(b.id, t));
        let r = body_radius_px(b, cam);
        // Battle / standoff in orbit: small badge at the lower left of the body.
        if b.kind != BodyKind::Star {
            if let Some(st) = be.orbit_status(b.id) {
                let lower = st.to_lowercase();
                let (col, glyph) = if lower.starts_with("battle") { (DANGER, "!") } else if lower.starts_with("standoff") { (ACCENT, "=") } else { (MUTED, "·") };
                let bc = c + vec2(-r * 0.8 - 6.0 * s(), r * 0.8 + 6.0 * s());
                draw_circle(bc.x, bc.y, 9.0 * s(), INK);
                draw_circle(bc.x, bc.y, 7.5 * s(), PANEL);
                draw_circle_lines(bc.x, bc.y, 7.5 * s(), 1.4 * s(), col);
                text(glyph, bc.x, bc.y + s(), 13.0, col, FontKind::Title, Align::Center);
            }
        }
        // Something in the build queue: grey "+" badge with the count at the body's upper right.
        if b.owner == Some(view.me) && b.kind != BodyKind::Star {
            if let Some(n) = be.body_info(b.id).map(|i| i.queue.len() + i.waiting.len()).filter(|&n| n > 0) {
                let bc = c + vec2(r * 0.8 + 6.0 * s(), -r * 0.8 - 6.0 * s());
                status_badge(bc, ShipStatus::Queued);
                text(&n.to_string(), bc.x + 11.0 * s(), bc.y, 12.0, PAPER, FontKind::Title, Align::Left);
            }
        }
        let show_label = match b.kind {
            BodyKind::Star => false,
            BodyKind::Planet => true,
            BodyKind::Moon => cam.zoom * b.soi * 2.0 > 40.0 * s() as f64,
            BodyKind::Outpost => true,
            BodyKind::Asteroid => cam.zoom * b.soi * 2.0 > 60.0 * s() as f64,
        };
        if show_label {
            let size = if b.kind == BodyKind::Planet { 18.0 } else { 14.0 };
            // Keep names on screen when the body sits at the edge.
            let name = b.name.to_uppercase();
            let half = measure(&name, FontKind::Title, size).x / 2.0 + 6.0 * s();
            let lx = c.x.clamp(half, (screen_width() - half).max(half));
            inked(&name, lx, c.y - r - 14.0 * s(), size, PAPER, Align::Center);
            if let Some(i) = &b.intel {
                if b.kind == BodyKind::Planet {
                    text(&format!("PWR {:.0}", i.power.max(0.0)), c.x, c.y + r + 14.0 * s(), 13.0, MUTED, FontKind::Title, Align::Center);
                }
            } else if b.kind == BodyKind::Planet {
                text("???", c.x, c.y + r + 14.0 * s(), 13.0, MUTED, FontKind::Title, Align::Center);
            }
        }
    }
}

thread_local! {
    static ENEMY_PATHS: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Map layer switch (game menu): draw other players' predicted courses. The client may persist it.
pub fn enemy_paths() -> bool {
    ENEMY_PATHS.with(|c| c.get())
}

pub fn set_enemy_paths(on: bool) {
    ENEMY_PATHS.with(|c| c.set(on));
}

pub fn draw_trajectories(be: &dyn Backend, view: &GameView, cam: &Camera, t: f64, highlight: Option<ShipId>) {
    let show_enemy = enemy_paths();
    for sh in &view.ships {
        let Some(tr) = &sh.trajectory else { continue };
        let mine = sh.owner == view.me;
        // hidden enemy courses still show for the one you've selected
        if !mine && !show_enemy && highlight != Some(sh.id) {
            continue;
        }
        let (l, _) = team(view.color_of(Some(sh.owner)));
        let (past, future): (Vec<&(f64, [f64; 2])>, Vec<_>) = tr.points.iter().partition(|p| p.0 < t);
        let past: Vec<Vec2> = past.iter().map(|p| cam.to_screen(p.1)).collect();
        let mut future: Vec<Vec2> = future.iter().map(|p| cam.to_screen(p.1)).collect();
        if let Some(p) = be.ship_pos(sh.id, t) {
            if !future.is_empty() && is_flying(sh, t) {
                future.insert(0, cam.to_screen(p));
            }
        }
        // Only the selected ship's course is drawn in full colour; the rest are faint grey dashes so the
        // map doesn't turn into spaghetti.
        let sel = highlight == Some(sh.id);
        cel::dashed_path(&past, 2.0 * s(), 5.0 * s(), alpha(l, if sel { 0.35 } else { 0.12 }));
        if !sel {
            let grey = Color::new(0.75, 0.75, 0.85, if mine { 0.32 } else { 0.22 });
            cel::dashed_path(&future, 2.0 * s(), 6.0 * s(), grey);
        } else if mine {
            cel::inked_path(&future, 4.0 * s(), l);
        } else {
            cel::dashed_path(&future, 3.0 * s(), 9.0 * s(), alpha(l, 0.85));
        }
        if mine && sel {
            draw_flybys(tr, view, cam, t, alpha(l, 0.9));
            for n in &sh.nodes {
                if let Some(p) = tr.sample(n.t) {
                    cel::diamond(cam.to_screen(p), 7.0 * s(), PAPER);
                }
            }
        }
        if let (Some(dest), Some(end)) = (sh.destination, tr.end()) {
            if end > t {
                let c = cam.to_screen(be.body_pos(dest, end));
                if let Some(b) = view.body(dest) {
                    let r = body_radius_px(b, cam) + 6.0 * s();
                    let ring: Vec<Vec2> = (0..=40).map(|i| c + Vec2::from_angle(i as f32 / 40.0 * std::f32::consts::TAU) * r).collect();
                    cel::dashed_path(&ring, 2.5 * s(), 5.0 * s(), alpha(l, if sel { 0.9 } else { 0.25 }));
                }
            }
        }
    }
}

/// Gravity-assist markers along a path: a dashed ring at closest approach and the speed change.
pub fn draw_flybys(tr: &Trajectory, view: &GameView, cam: &Camera, t: f64, color: Color) {
    for f in tr.flybys.iter().filter(|f| f.t >= t) {
        let c = cam.to_screen(f.pos);
        let r = 9.0 * s();
        let ring: Vec<Vec2> = (0..=24).map(|i| c + Vec2::from_angle(i as f32 / 24.0 * std::f32::consts::TAU) * r).collect();
        cel::dashed_path(&ring, 2.0 * s(), 4.0 * s(), color);
        let name = view.body(f.body).map(|b| b.name.to_uppercase()).unwrap_or_default();
        let label = if f.gain >= 0.0 { format!("FLYBY {name} +{:.1} km/s", f.gain) } else { format!("FLYBY {name} {:.1} km/s", f.gain) };
        // same pill as the node editor's END marker
        let w = measure(&label, FontKind::Title, 12.0).x + 14.0 * s();
        let pill = Rect::new(c.x + 24.0 * s(), c.y - 40.0 * s(), w, 20.0 * s());
        cel::inked_path(&[c + vec2(r * 0.7, -r * 0.7), vec2(pill.x, pill.y + pill.h)], 1.5 * s(), color);
        cel::round_rect(pill, pill.h / 2.0, alpha(PANEL, 0.9));
        text(&label, pill.center().x, pill.center().y + s(), 12.0, color, FontKind::Title, Align::Center);
    }
}

pub fn draw_ships(be: &dyn Backend, view: &GameView, cam: &Camera, t: f64, selected: Option<Focus>) {
    let time = get_time() as f32;
    for sh in &view.ships {
        let Some((p, dir)) = ship_screen(be, view, cam, sh, t) else { continue };
        let (l, _) = team(view.color_of(Some(sh.owner)));
        // Manifest `size` per class, so the RPS classes read differently at a glance.
        let size = match sh.class {
            ShipClass::Scout => 6.5,
            ShipClass::Interceptor => 8.0,
            ShipClass::Gunship => 10.0,
            ShipClass::Bomber => 11.0,
            ShipClass::Dreadnought => 14.0,
            ShipClass::Tanker => 11.0,
            ShipClass::Constructor => 9.0,
            ShipClass::Decoy => 6.0,
            ShipClass::Torpedo => 5.0,
            ShipClass::Slug => 4.0,
        } * s();
        if selected == Some(Focus::Ship(sh.id)) {
            let pulse = 1.0 + 0.1 * (time * 6.0).sin();
            draw_circle_lines(p.x, p.y, (size + 9.0 * s()) * pulse, 3.5 * s(), ACCENT);
        }
        let player = view.player(sh.owner);
        let skin = player.map(|p| art::skin_of(&p.faction)).unwrap_or("vanguard");
        if let Some(seen) = sh.seen_at {
            // fog of war: last known position, fading over a day
            let age = ((view.now - seen) / 86_400.0).clamp(0.0, 1.0) as f32;
            let a = 0.45 - 0.25 * age;
            if let Some(t) = art::ship_tex(sh.class, skin, player.map(|p| p.color)) {
                let px = size * 2.8;
                art::draw_centered(&t, p, vec2(px, px), dir.y.atan2(dir.x), Color::new(1.0, 1.0, 1.0, a));
            }
            draw_circle_lines(p.x, p.y, size + 5.0 * s(), 1.5 * s(), Color::new(l.r, l.g, l.b, a));
            text("?", p.x + size, p.y - size, 13.0, Color::new(1.0, 1.0, 1.0, a + 0.2), FontKind::Title, Align::Center);
            continue;
        }
        // our own cloaked ("dark") ships: faded, with a dashed ring, so you can see what's hidden
        let dark = sh.owner == view.me && be.dark_until(sh.id, t).is_some();
        if dark {
            let ring: Vec<Vec2> = (0..=28).map(|i| p + Vec2::from_angle(i as f32 / 28.0 * std::f32::consts::TAU + time * 0.4) * (size + 6.0 * s())).collect();
            cel::dashed_path(&ring, 1.5 * s(), 4.0 * s(), Color::new(0.75, 0.85, 1.0, 0.7));
        }
        if let Some(t) = art::ship_tex(sh.class, skin, player.map(|p| p.color)) {
            let px = size * 2.8;
            art::draw_centered(&t, p, vec2(px, px), dir.y.atan2(dir.x), if dark { Color::new(1.0, 1.0, 1.0, 0.38) } else { WHITE });
        } else if dark {
            cel::ship(p, dir, size, Color::new(1.0, 1.0, 1.0, 0.38));
        } else {
            let fill = if sh.owner == view.me { PAPER } else { l };
            cel::ship(p, dir, size, fill);
        }
        if sh.count > 1 {
            let b = p + vec2(size, -size);
            draw_circle(b.x, b.y, 8.5 * s(), INK);
            draw_circle(b.x, b.y, 7.0 * s(), ACCENT);
            text(&sh.count.to_string(), b.x, b.y, 11.0, INK, FontKind::Title, Align::Center);
        }
        if sh.owner == view.me {
            if let Some(st) = ship_status(sh, view.now) {
                status_badge(p + vec2(-size, size), st);
            }
        }
    }
}

/// What a ship is busy with, for the little badge on its token.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShipStatus {
    /// Still in a build queue (scrubber preview).
    Queued,
    /// Parked with a departure scheduled.
    Departing,
    /// A manual burn is still ahead.
    Burn,
    /// Coasting on a transfer.
    Transfer,
}

pub fn ship_status(sh: &Ship, now: f64) -> Option<ShipStatus> {
    if sh.planned.is_some() {
        return Some(ShipStatus::Queued);
    }
    let tr = sh.trajectory.as_ref()?;
    if tr.start().is_some_and(|t0| t0 > now) {
        Some(ShipStatus::Departing)
    } else if sh.nodes.iter().any(|n| n.t > now) {
        Some(ShipStatus::Burn)
    } else if tr.end().is_some_and(|e| e > now) {
        Some(ShipStatus::Transfer)
    } else {
        None
    }
}

/// Fixed-size (zoom-independent) glyph: clock = departing, diamond = burn ahead, chevron = in
/// transfer, plus = being built.
pub fn status_badge(c: Vec2, st: ShipStatus) {
    let r = 7.5 * s();
    let col = match st {
        ShipStatus::Queued => MUTED,
        ShipStatus::Departing => ACCENT,
        ShipStatus::Burn => TEAMS[0].0,
        ShipStatus::Transfer => GOOD,
    };
    draw_circle(c.x, c.y, r + 1.5 * s(), INK);
    draw_circle(c.x, c.y, r, PANEL);
    draw_circle_lines(c.x, c.y, r, 1.4 * s(), col);
    let k = r * 0.55;
    match st {
        ShipStatus::Departing => {
            draw_circle_lines(c.x, c.y, k, 1.3 * s(), col);
            draw_line(c.x, c.y, c.x, c.y - k * 0.8, 1.4 * s(), col);
            draw_line(c.x, c.y, c.x + k * 0.6, c.y, 1.4 * s(), col);
        }
        ShipStatus::Burn => {
            let p = [c + vec2(0.0, -k), c + vec2(k, 0.0), c + vec2(0.0, k), c + vec2(-k, 0.0)];
            draw_triangle(p[0], p[1], p[2], col);
            draw_triangle(p[0], p[2], p[3], col);
        }
        ShipStatus::Transfer => {
            for dx in [-0.35, 0.35] {
                let o = c + vec2(dx * k, 0.0);
                draw_line(o.x - k * 0.4, o.y - k * 0.7, o.x + k * 0.3, o.y, 1.6 * s(), col);
                draw_line(o.x + k * 0.3, o.y, o.x - k * 0.4, o.y + k * 0.7, 1.6 * s(), col);
            }
        }
        ShipStatus::Queued => {
            draw_line(c.x - k, c.y, c.x + k, c.y, 1.6 * s(), col);
            draw_line(c.x, c.y - k, c.x, c.y + k, 1.6 * s(), col);
        }
    }
}

/// What's under a tap. Bodies win: a tap on or near a planet/asteroid selects the body, and the ships
/// parked around it only become tappable once that body (or one of its ships) is selected.
/// Ships out in open space are always tappable.
pub fn hit_test(be: &dyn Backend, view: &GameView, cam: &Camera, t: f64, p: Vec2, selected: Option<Focus>) -> Option<Focus> {
    // Nearest body whose tap zone (body + its parked-ship ring) contains p.
    let mut body: Option<(f32, &Body)> = None;
    for b in &view.bodies {
        let c = cam.to_screen(be.body_pos(b.id, t));
        let zone = (body_radius_px(b, cam) + 30.0 * s()).max(26.0 * s());
        let d = c.distance(p);
        if d < zone && body.is_none_or(|(bd, _)| d < bd) {
            body = Some((d, b));
        }
    }
    // The body the current selection "lives at": the body itself or the parking spot of a selected ship.
    let open_body = match selected {
        Some(Focus::Body(b)) => Some(b),
        Some(Focus::Ship(sid)) => view.ship(sid).filter(|sh| !is_flying(sh, t)).and_then(|sh| sh.parked_at),
        None => None,
    };
    let mut ship: Option<(f32, ShipId)> = None;
    for sh in &view.ships {
        let Some((sp, _)) = ship_screen(be, view, cam, sh, t) else { continue };
        let d = sp.distance(p);
        if d >= 22.0 * s() || ship.is_some_and(|(bd, _)| d >= bd) {
            continue;
        }
        let allowed = if is_flying(sh, t) {
            // in transit: only blocked when it's passing right over a body that isn't open
            body.is_none_or(|(_, b)| open_body == Some(b.id))
        } else {
            sh.parked_at.is_none() || sh.parked_at == open_body
        };
        if allowed {
            ship = Some((d, sh.id));
        }
    }
    match (ship, body) {
        (Some((_, id)), _) => Some(Focus::Ship(id)),
        (None, Some((_, b))) => Some(Focus::Body(b.id)),
        _ => None,
    }
}

/// Nearest time on a timed polyline to screen point `p`, if within `max_px`.
pub fn nearest_on(points: &[(f64, [f64; 2])], cam: &Camera, p: Vec2, max_px: f32) -> Option<f64> {
    let mut best: Option<(f32, f64)> = None;
    for w in points.windows(2) {
        let (a, b) = (cam.to_screen(w[0].1), cam.to_screen(w[1].1));
        let ab = b - a;
        let k = if ab.length_squared() > 0.0 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
        let d = (a + ab * k).distance(p);
        if d < max_px && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, w[0].0 + (w[1].0 - w[0].0) * k as f64));
        }
    }
    best.map(|b| b.1)
}

/// One body: sprite with the star-facing shadow overlay (vector fallback), plus an inked team
/// ring when someone owns it.
pub fn draw_body(b: &Body, c: Vec2, r: f32, star: Vec2, owner_color: Option<usize>, time: f32) {
    if b.art.as_deref() == Some("rubble") {
        // Shattered by an impact: a slowly tumbling debris cloud.
        for i in 0..9u32 {
            let a = i as f32 * 2.399 + time * 0.05;
            let d = r * (0.25 + 0.12 * (i % 4) as f32) * 2.2;
            let rr = r * (0.18 + 0.06 * (i % 3) as f32);
            let rock = Body { id: b.id.wrapping_mul(31).wrapping_add(i), kind: BodyKind::Asteroid, art: None, ..b.clone() };
            draw_body(&rock, c + Vec2::from_angle(a) * d, rr, star, None, time);
        }
        return;
    }
    if b.kind == BodyKind::Outpost {
        // just a beacon: the stations there are drawn around it
        let (l, _) = team(owner_color);
        cel::diamond(c, r + 2.0 * s(), INK);
        cel::diamond(c, r, l);
        return;
    }
    let (key, ratio) = art::body_art(b);
    match art::tex(&key) {
        Some(t) => {
            if b.kind == BodyKind::Star {
                if let Some(g) = art::tex("sprites/bodies/star_glow.png") {
                    let gs = r * 6.0 * (1.0 + 0.03 * (time * 1.7).sin());
                    art::draw_centered(&g, c, vec2(gs, gs), 0.0, Color::new(1.0, 0.85, 0.4, 0.55));
                }
            }
            let size = r / ratio;
            let rot = if b.kind == BodyKind::Star { time * 0.03 } else { 0.0 };
            art::draw_centered(&t, c, vec2(size, size), rot, WHITE);
            if matches!(b.kind, BodyKind::Planet | BodyKind::Moon) {
                if let Some(sh) = art::tex("sprites/bodies/shadow_overlay.png") {
                    let d = c - star;
                    let size = r / (112.0 / 256.0);
                    art::draw_centered(&sh, c, vec2(size, size), d.y.atan2(d.x), WHITE);
                }
            }
        }
        None => match b.kind {
            BodyKind::Star => cel::star(c, r, time),
            BodyKind::Asteroid => cel::rock(c, r, b.id),
            _ => {
                let (l, d) = team(owner_color);
                cel::body(c, r, l, d);
            }
        },
    }
    if let (Some(col), BodyKind::Planet | BodyKind::Moon | BodyKind::Asteroid) = (owner_color, b.kind) {
        let (l, _) = team(Some(col));
        let rr = r + 6.0 * s();
        draw_circle_lines(c.x, c.y, rr, 6.0 * s(), INK);
        draw_circle_lines(c.x, c.y, rr, 3.0 * s(), l);
    }
}

/// Hover outline + name pill for whatever is under the mouse.
pub fn draw_hover(be: &dyn Backend, view: &GameView, cam: &Camera, t: f64, f: Focus) {
    let (c, r, name) = match f {
        Focus::Body(id) => {
            let Some(b) = view.body(id) else { return };
            (cam.to_screen(be.body_pos(id, t)), body_radius_px(b, cam) + 8.0 * s(), b.name.clone())
        }
        Focus::Ship(id) => {
            let Some(sh) = view.ship(id) else { return };
            let Some((p, _)) = ship_screen(be, view, cam, sh, t) else { return };
            let who = view.player(sh.owner).map(|p| p.name.clone()).unwrap_or_default();
            let n = if sh.count > 1 { format!("{} ×{} · {who}", sh.name, sh.count) } else { format!("{} · {who}", sh.name) };
            (p, 16.0 * s(), n)
        }
    };
    draw_circle_lines(c.x, c.y, r, 2.0 * s(), Color::new(1.0, 1.0, 1.0, 0.7));
    let w = measure(&name, FontKind::Title, 13.0).x + 16.0 * s();
    let pr = Rect::new(c.x - w / 2.0, c.y + r + 6.0 * s(), w, 22.0 * s());
    cel::round_rect(pr, pr.h / 2.0, alpha(PANEL, 0.92));
    text(&name, pr.center().x, pr.center().y + s(), 13.0, PAPER, FontKind::Title, Align::Center);
}

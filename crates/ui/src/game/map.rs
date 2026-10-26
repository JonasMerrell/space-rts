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
    }
}

pub fn body_radius_px(b: &Body, cam: &Camera) -> f32 {
    ((b.radius * cam.zoom) as f32).max(min_radius(b.kind) * s())
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
        let col = Color::new(1.0, 1.0, 1.0, if b.kind == BodyKind::Asteroid { 0.12 } else { 0.25 });
        if b.kind == BodyKind::Asteroid {
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
        let show_label = match b.kind {
            BodyKind::Star => false,
            BodyKind::Planet => true,
            BodyKind::Moon => cam.zoom * b.soi * 2.0 > 40.0 * s() as f64,
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
                    text(&format!("PWR {:.0}", i.power), c.x, c.y + r + 14.0 * s(), 13.0, MUTED, FontKind::Title, Align::Center);
                }
            } else if b.kind == BodyKind::Planet {
                text("???", c.x, c.y + r + 14.0 * s(), 13.0, MUTED, FontKind::Title, Align::Center);
            }
        }
    }
}

pub fn draw_trajectories(be: &dyn Backend, view: &GameView, cam: &Camera, t: f64, highlight: Option<ShipId>) {
    for sh in &view.ships {
        let Some(tr) = &sh.trajectory else { continue };
        let mine = sh.owner == view.me;
        let (l, _) = team(view.color_of(Some(sh.owner)));
        let (past, future): (Vec<&(f64, [f64; 2])>, Vec<_>) = tr.points.iter().partition(|p| p.0 < t);
        let past: Vec<Vec2> = past.iter().map(|p| cam.to_screen(p.1)).collect();
        let mut future: Vec<Vec2> = future.iter().map(|p| cam.to_screen(p.1)).collect();
        if let Some(p) = be.ship_pos(sh.id, t) {
            if !future.is_empty() && is_flying(sh, t) {
                future.insert(0, cam.to_screen(p));
            }
        }
        cel::dashed_path(&past, 2.0 * s(), 5.0 * s(), alpha(l, 0.35));
        let w = if highlight == Some(sh.id) { 4.0 } else { 3.0 } * s();
        if mine {
            cel::inked_path(&future, w, l);
        } else {
            cel::dashed_path(&future, w, 9.0 * s(), alpha(l, 0.85));
        }
        if mine {
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
                    cel::dashed_path(&ring, 2.5 * s(), 5.0 * s(), alpha(l, 0.9));
                }
            }
        }
    }
}

pub fn draw_ships(be: &dyn Backend, view: &GameView, cam: &Camera, t: f64, selected: Option<Focus>) {
    let time = get_time() as f32;
    for sh in &view.ships {
        let Some((p, dir)) = ship_screen(be, view, cam, sh, t) else { continue };
        let (l, _) = team(view.color_of(Some(sh.owner)));
        // Manifest `size` per class, so the RPS classes read differently at a glance.
        let size = match sh.class {
            ShipClass::Interceptor => 8.0,
            ShipClass::Gunship => 10.5,
            ShipClass::Bomber => 11.5,
            ShipClass::Tanker => 12.0,
        } * s();
        if selected == Some(Focus::Ship(sh.id)) {
            let pulse = 1.0 + 0.1 * (time * 6.0).sin();
            draw_circle_lines(p.x, p.y, (size + 9.0 * s()) * pulse, 3.5 * s(), ACCENT);
        }
        let player = view.player(sh.owner);
        let skin = player.map(|p| art::skin_of(&p.faction)).unwrap_or("vanguard");
        if let Some(t) = art::ship_tex(sh.class, skin, player.map(|p| p.color)) {
            let px = size * 3.4;
            art::draw_centered(&t, p, vec2(px, px), dir.y.atan2(dir.x), WHITE);
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
    }
}

/// What's under a tap: ships first (they're small), then bodies.
pub fn hit_test(be: &dyn Backend, view: &GameView, cam: &Camera, t: f64, p: Vec2) -> Option<Focus> {
    let mut best: Option<(f32, Focus)> = None;
    for sh in &view.ships {
        if let Some((sp, _)) = ship_screen(be, view, cam, sh, t) {
            let d = sp.distance(p);
            if d < 22.0 * s() && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, Focus::Ship(sh.id)));
            }
        }
    }
    if best.is_some() {
        return best.map(|b| b.1);
    }
    for b in &view.bodies {
        let c = cam.to_screen(be.body_pos(b.id, t));
        let r = body_radius_px(b, cam).max(22.0 * s());
        let d = c.distance(p);
        if d < r && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, Focus::Body(b.id)));
        }
    }
    best.map(|b| b.1)
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

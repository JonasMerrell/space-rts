//! Cel-shaded drawing primitives: inked bodies, outlined strokes, halftone backdrop,
//! speech bubbles and comic bursts. All sizes are in px unless named `_dp`.

use crate::theme::*;
use macroquad::prelude::*;
use std::cell::RefCell;
use std::f32::consts::{PI, TAU};

/// Filled polygon fan around `c` through `pts` (must be star-shaped from `c`).
fn fan(c: Vec2, pts: &[Vec2], color: Color) {
    for w in pts.windows(2) {
        draw_triangle(c, w[0], w[1], color);
    }
}

fn arc_pts(c: Vec2, r: f32, a0: f32, a1: f32, n: usize) -> Vec<Vec2> {
    (0..=n).map(|i| c + Vec2::from_angle(a0 + (a1 - a0) * i as f32 / n as f32) * r).collect()
}

/// Planet/moon/star with 2-tone shading, a rim-light dot and a thick ink outline.
pub fn body(c: Vec2, r: f32, light: Color, shade: Color) {
    let ink = (r * 0.22).clamp(2.0, 5.0 * s());
    draw_circle(c.x, c.y, r + ink, INK);
    draw_circle(c.x, c.y, r, light);
    // Shadow side, lit from the upper left as in the mock-up.
    let pts = arc_pts(c, r, -0.6, 2.2, 24);
    fan(c + vec2(r * 0.2, r * 0.2), &pts, shade);
    draw_circle(c.x - r * 0.4, c.y - r * 0.4, r * 0.18, Color::new(1.0, 1.0, 1.0, 0.55));
}

pub fn star(c: Vec2, r: f32, t: f32) {
    // Comic sun: wobbling spiky corona, then a body.
    let n = 16;
    let mut pts = Vec::with_capacity(n * 2 + 1);
    for i in 0..=n * 2 {
        let a = i as f32 / (n * 2) as f32 * TAU + t * 0.05;
        let rr = if i % 2 == 0 { r * (1.45 + 0.06 * (t * 2.0 + i as f32).sin()) } else { r * 1.12 };
        pts.push(c + Vec2::from_angle(a) * rr);
    }
    let big: Vec<Vec2> = pts.iter().map(|p| *p + (*p - c).normalize_or_zero() * 4.0 * s()).collect();
    fan(c, &big, INK);
    fan(c, &pts, SUN_SHADE);
    draw_circle(c.x, c.y, r + 4.0 * s(), INK);
    draw_circle(c.x, c.y, r, SUN);
    let pts = arc_pts(c + vec2(r * 0.22, r * 0.22), r * 0.7, 0.0, PI * 1.2, 20);
    fan(c + vec2(r * 0.22, r * 0.22), &pts, SUN_SHADE);
    draw_circle(c.x - r * 0.4, c.y - r * 0.4, r * 0.16, Color::new(1.0, 1.0, 1.0, 0.7));
}

/// Lumpy asteroid.
pub fn rock(c: Vec2, r: f32, seed: u32) {
    let n = 9;
    let mut pts = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let k = ((seed.wrapping_mul(2654435761).wrapping_add(i as u32 % n as u32 * 40503)) >> 24) as f32 / 255.0;
        pts.push(c + Vec2::from_angle(i as f32 / n as f32 * TAU) * r * (0.75 + 0.35 * k));
    }
    let outline: Vec<Vec2> = pts.iter().map(|p| *p + (*p - c).normalize_or_zero() * 2.5 * s()).collect();
    fan(c, &outline, INK);
    fan(c, &pts, NEUTRAL.0);
    let half: Vec<Vec2> = pts[n / 3..=n * 5 / 6].to_vec();
    fan(c + vec2(r * 0.1, r * 0.1), &half, NEUTRAL.1);
}

/// Ship token: arrowhead pointing along `dir`.
pub fn ship(c: Vec2, dir: Vec2, size: f32, fill: Color) {
    let d = if dir.length_squared() > 0.0 { dir.normalize() } else { vec2(1.0, 0.0) };
    let n = d.perp();
    let tip = c + d * size;
    let l = c - d * size * 0.7 + n * size * 0.6;
    let r = c - d * size * 0.7 - n * size * 0.6;
    let notch = c - d * size * 0.3;
    let o = 3.0 * s();
    let grow = |p: Vec2| c + (p - c) + (p - c).normalize_or_zero() * o;
    draw_triangle(grow(tip), grow(l), grow(notch), INK);
    draw_triangle(grow(tip), grow(notch), grow(r), INK);
    draw_triangle(tip, l, notch, fill);
    draw_triangle(tip, notch, r, fill);
}

/// Polyline with an ink casing: drawn as black at `w + 2*ink`, then colour at `w`.
pub fn inked_path(pts: &[Vec2], w: f32, color: Color) {
    if pts.len() < 2 {
        return;
    }
    let ink = 2.0 * s();
    for pass in 0..2 {
        let (ww, col) = if pass == 0 { (w + ink * 2.0, INK) } else { (w, color) };
        for seg in pts.windows(2) {
            draw_line(seg[0].x, seg[0].y, seg[1].x, seg[1].y, ww, col);
        }
        for p in pts {
            draw_circle(p.x, p.y, ww / 2.0, col);
        }
    }
}

/// Dashed polyline, no casing (orbits, predicted paths).
pub fn dashed_path(pts: &[Vec2], w: f32, dash: f32, color: Color) {
    if dash <= 0.0 {
        return;
    }
    // `phase` = distance into the current dash+gap period, carried across segments.
    let mut phase = 0.0f32;
    for seg in pts.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let len = a.distance(b);
        if len <= f32::EPSILON {
            continue;
        }
        let dir = (b - a) / len;
        let mut t = 0.0f32;
        while t < len {
            let on = phase < dash;
            let left = if on { dash - phase } else { 2.0 * dash - phase };
            let step = left.min(len - t).max(0.01);
            if on {
                let (p0, p1) = (a + dir * t, a + dir * (t + step).min(len));
                draw_line(p0.x, p0.y, p1.x, p1.y, w, color);
            }
            t += step;
            phase = (phase + step) % (2.0 * dash);
        }
    }
}

fn round_rect_pts(r: Rect, rad: f32) -> Vec<Vec2> {
    let rad = rad.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    let n = ((rad / s()).ceil() as usize).clamp(2, 10);
    let corners = [
        (vec2(r.x + r.w - rad, r.y + r.h - rad), 0.0),
        (vec2(r.x + rad, r.y + r.h - rad), PI * 0.5),
        (vec2(r.x + rad, r.y + rad), PI),
        (vec2(r.x + r.w - rad, r.y + rad), PI * 1.5),
    ];
    let mut pts = Vec::with_capacity(4 * (n + 1) + 1);
    for (c, a0) in corners {
        for i in 0..=n {
            pts.push(c + Vec2::from_angle(a0 + PI * 0.5 * i as f32 / n as f32) * rad);
        }
    }
    pts.push(pts[0]);
    pts
}

/// Filled rounded rect as one convex fan, so translucent fills don't double up at the corners.
pub fn round_rect(r: Rect, rad: f32, color: Color) {
    fan(r.center(), &round_rect_pts(r, rad), color);
}

/// Rounded-rect outline of thickness `w`, centred on the rect edge.
pub fn round_rect_lines(r: Rect, rad: f32, w: f32, color: Color) {
    let pts = round_rect_pts(r, rad);
    // Thin quads per edge segment; tiny overlaps at joints are invisible at hairline widths.
    for seg in pts.windows(2) {
        draw_line(seg[0].x, seg[0].y, seg[1].x, seg[1].y, w, color);
    }
}

/// UI panel: dark translucent glass with a hairline border.
pub fn panel(r: Rect, fill: Color) {
    let b = s().max(1.0);
    let rad = 8.0 * s();
    round_rect(r, rad, fill);
    round_rect_lines(r, rad, b, BORDER);
}

/// Callout box (light tooltip) with a small pointer toward `tail`.
pub fn bubble(r: Rect, tail: Option<Vec2>, fill: Color) {
    let c = r.center();
    if let Some(t) = tail {
        let d = (t - c).normalize_or_zero();
        let n = d.perp() * 7.0 * s();
        let edge = vec2(c.x + d.x * r.w * 0.45, c.y + d.y * r.h * 0.45);
        let tip = edge + (t - edge).clamp_length_max(14.0 * s());
        draw_triangle(tip, edge + n, edge - n, fill);
    }
    round_rect(Rect::new(r.x + 1.0, r.y + 2.0 * s(), r.w, r.h), 8.0 * s(), Color::new(0.0, 0.0, 0.0, 0.35));
    round_rect(r, 8.0 * s(), fill);
}

/// Spiky "POW!" burst.
pub fn burst(c: Vec2, r: f32, spikes: usize, fill: Color, phase: f32) {
    let mut pts = Vec::with_capacity(spikes * 2 + 1);
    for i in 0..=spikes * 2 {
        let a = i as f32 / (spikes * 2) as f32 * TAU + phase;
        let k = if i % 2 == 0 { 1.0 } else { 0.68 + 0.08 * ((i * 7) % 3) as f32 };
        pts.push(c + Vec2::from_angle(a) * r * k);
    }
    let ink: Vec<Vec2> = pts.iter().map(|p| *p + (*p - c).normalize_or_zero() * 3.5 * s()).collect();
    fan(c, &ink, INK);
    fan(c, &pts, fill);
}

/// Small diamond marker (maneuver nodes, windows).
pub fn diamond(c: Vec2, r: f32, fill: Color) {
    let o = 2.5 * s();
    for (rr, col) in [(r + o * 1.4, INK), (r, fill)] {
        let p = [c + vec2(0.0, -rr), c + vec2(rr, 0.0), c + vec2(0.0, rr), c + vec2(-rr, 0.0)];
        draw_triangle(p[0], p[1], p[2], col);
        draw_triangle(p[0], p[2], p[3], col);
    }
}

/// Arrow from `a` to `b` with ink casing.
pub fn arrow(a: Vec2, b: Vec2, w: f32, color: Color) {
    let d = (b - a).normalize_or_zero();
    let n = d.perp();
    let head = w * 2.6;
    let shaft_end = b - d * head;
    inked_path(&[a, shaft_end], w, color);
    let o = 2.5 * s();
    draw_triangle(b + d * o * 1.6, shaft_end + n * (head * 0.75 + o), shaft_end - n * (head * 0.75 + o), INK);
    draw_triangle(b, shaft_end + n * head * 0.75, shaft_end - n * head * 0.75, color);
}

thread_local! {
    // ManuallyDrop: see theme::FONTS.
    static PENDING: std::cell::Cell<(u32, u32, f64)> = const { std::cell::Cell::new((0, 0, 0.0)) };
    static BACKDROP: RefCell<std::mem::ManuallyDrop<Option<(u32, u32, Texture2D)>>> = const { RefCell::new(std::mem::ManuallyDrop::new(None)) };
}

/// Deep purple field with faint blotchy stars and an offset halftone dot grid, baked once per
/// screen size into a texture.
pub fn backdrop() {
    let (w, h) = (screen_width().max(1.0) as u32, screen_height().max(1.0) as u32);
    let now = get_time();
    // While a browser window is being resized the size changes every frame: stretch the old bake
    // and only re-bake once the size has held still for a moment.
    let settled = PENDING.with(|p| {
        let (pw, ph, since) = p.get();
        if (pw, ph) != (w, h) {
            p.set((w, h, now));
            false
        } else {
            now - since > 0.25
        }
    });
    let tex = BACKDROP.with(|b| {
        let mut b = b.borrow_mut();
        let stale = !matches!(&**b, Some((bw, bh, _)) if *bw == w && *bh == h);
        if stale && (settled || b.is_none()) {
            if let Some((_, _, old)) = b.take() {
                drop(old);
            }
            **b = Some((w, h, bake_backdrop(w, h)));
        }
        b.as_ref().unwrap().2.clone()
    });
    draw_texture_ex(&tex, 0.0, 0.0, WHITE, DrawTextureParams { dest_size: Some(vec2(w as f32, h as f32)), ..Default::default() });
}

fn bake_backdrop(w: u32, h: u32) -> Texture2D {
    let mut img = Image::gen_image_color(w as u16, h as u16, BG);
    let sc = s();
    let px = img.get_image_data_mut();
    let mut blend = |x: i32, y: i32, a: f32| {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            return;
        }
        let p = &mut px[(y as u32 * w + x as u32) as usize];
        for c in p.iter_mut().take(3) {
            *c = (*c as f32 + (255.0 - *c as f32) * a) as u8;
        }
    };
    let mut seed: u64 = 2;
    let mut rnd = || {
        seed = seed * 16807 % 2147483647;
        seed as f32 / 2147483647.0
    };
    let disc = |cx: f32, cy: f32, r: f32, a: f32, blend: &mut dyn FnMut(i32, i32, f32)| {
        let ri = r.ceil() as i32;
        for dy in -ri..=ri {
            for dx in -ri..=ri {
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                if d <= r {
                    blend(cx as i32 + dx, cy as i32 + dy, a * (r - d).min(1.0));
                }
            }
        }
    };
    let area = (w * h) as f32 / (788.0 * 500.0);
    for _ in 0..(500.0 * area / (sc * sc)).min(6000.0) as usize {
        let (x, y) = (rnd() * w as f32, rnd() * h as f32);
        disc(x, y, 3.0 * sc, 0.06, &mut blend);
    }
    let step = 12.0 * sc;
    let mut row = 0;
    let mut y = 0.0;
    while y < h as f32 {
        let mut x = if row % 2 == 1 { step / 2.0 } else { 0.0 };
        while x < w as f32 {
            disc(x, y, 1.6 * sc, 0.04, &mut blend);
            x += step;
        }
        y += step;
        row += 1;
    }
    // A few crisp stars for depth.
    for _ in 0..(90.0 * area / sc).min(900.0) as usize {
        let (x, y) = (rnd() * w as f32, rnd() * h as f32);
        disc(x, y, (0.6 + rnd() * 1.1) * sc, 0.5 + rnd() * 0.4, &mut blend);
    }
    let tex = Texture2D::from_image(&img);
    tex.set_filter(FilterMode::Nearest);
    tex
}

/// Halftone wash over a rect (used to mark "this is a prediction").
pub fn halftone(r: Rect, color: Color) {
    let step = 9.0 * s();
    let mut row = 0;
    let mut y = r.y;
    while y < r.y + r.h {
        let mut x = r.x + if row % 2 == 1 { step / 2.0 } else { 0.0 };
        while x < r.x + r.w {
            draw_circle(x, y, 1.4 * s(), color);
            x += step;
        }
        y += step;
        row += 1;
    }
}

/// Clip subsequent drawing to `r` (screen px), or stop clipping with None (scrolling lists).
pub fn clip(r: Option<Rect>) {
    let mut gl = unsafe { get_internal_gl() };
    gl.flush();
    // The scissor rect is in framebuffer (physical) pixels, while we lay out in logical pixels
    // (screen_width() = physical / dpi_scale). At dpi 1 they match; at dpi 2–3 (high-DPI phones,
    // browsers) an unscaled rect clipped the list to a sliver in the top-left.
    let k = screen_dpi_scale();
    gl.quad_gl.scissor(r.map(|r| ((r.x * k) as i32, (r.y * k) as i32, (r.w.max(0.0) * k).ceil() as i32, (r.h.max(0.0) * k).ceil() as i32)));
}

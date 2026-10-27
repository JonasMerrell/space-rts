//! Research screen (full-height overlay) and the station picker sheet for a body.

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

fn margin() -> f32 {
    12.0 * s()
}

fn close_x(inp: &mut Input, r: Rect) -> bool {
    widgets::round_button(inp, vec2(r.x + r.w - 22.0 * s(), r.y + 22.0 * s()), 14.0 * s(), "X", PAPER, 0)
}

fn icon(name: &str, c: Vec2, px: f32, tint: Color) -> bool {
    match art::tex(&format!("sprites/icons/{name}.png")) {
        Some(t) => {
            art::draw_centered(&t, c, vec2(px, px), 0.0, tint);
            true
        }
        None => false,
    }
}

fn affordable(view: &GameView, c: &Cost) -> bool {
    view.resources.matter >= c.matter && view.resources.energy >= c.energy
}

fn cost_line(c: &Cost) -> String {
    format!("{}M  {}E  {}", fmt_amount(c.matter), fmt_amount(c.energy), fmt_dur(c.seconds))
}

fn branch_icon(branch: &str) -> &'static str {
    match branch {
        "Weapons" => "weapon_kinetic",
        "Engines" => "fuel",
        "Industry" => "extraction",
        "Defense" => "shield",
        "Transit" => "maneuver_node",
        "Strike" => "target",
        "Bastion" => "station_defense",
        "Shadow" => "eye_off",
        "Sight" => "eye",
        _ => "telescope",
    }
}

// ---------------------------------------------------------------- research

/// Full-height research overlay: lab status, base/advanced switch, branch tabs, the tiers of the branch.
pub fn research_screen(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) {
    let w = screen_width();
    let rw = (w - 2.0 * margin()).min(760.0 * s());
    let r = Rect::new((w - rw) / 2.0, safe_top() + margin(), rw, screen_height() - safe_top() - safe_bottom() - 2.0 * margin());
    inp.block(r);
    cel::panel(r, alpha(PANEL, 0.98));
    let Some(rv) = be.research() else {
        text("Research data unavailable.", r.center().x, r.center().y, 15.0, MUTED, FontKind::Body, Align::Center);
        if close_x(inp, r) {
            g.research_open = false;
        }
        return;
    };
    let pad = 16.0 * s();
    let done = rv.techs.iter().filter(|t| t.state == TechState::Done).count();
    inked("RESEARCH", r.x + pad, r.y + 24.0 * s(), 24.0, PAPER, Align::Left);
    text(&format!("{done} / {} technologies", rv.techs.len()), r.x + pad, r.y + 48.0 * s(), 13.0, MUTED, FontKind::Title, Align::Left);
    if close_x(inp, r) {
        g.research_open = false;
        art::sfx("menu_close");
        return;
    }

    // Labs.
    let mut y = r.y + 70.0 * s();
    if rv.labs.is_empty() {
        for line in wrap("No research lab yet. Upgrade Research on one of your planets to start.", FontKind::Body, 14.0, r.w - 2.0 * pad) {
            text(&line, r.x + pad, y + 8.0 * s(), 14.0, ACCENT, FontKind::Body, Align::Left);
            y += 20.0 * s();
        }
    }
    for lab in rv.labs.iter().take(3) {
        let lr = Rect::new(r.x + pad, y, r.w - 2.0 * pad, 30.0 * s());
        cel::round_rect(lr, 6.0 * s(), Color::new(1.0, 1.0, 1.0, 0.05));
        icon("research", vec2(lr.x + 15.0 * s(), lr.center().y), 20.0 * s(), WHITE);
        text(&format!("{} · LAB {}", lab.name.to_uppercase(), lab.level), lr.x + 32.0 * s(), lr.center().y, 13.0, PAPER, FontKind::Title, Align::Left);
        let (status, col) = match &lab.busy {
            Some((name, t)) => (format!("{name} · {}", fmt_dur(t - view.now)), ACCENT),
            None => ("idle".to_string(), GOOD),
        };
        text(&status, lr.x + lr.w - 10.0 * s(), lr.center().y, 13.0, col, FontKind::Body, Align::Right);
        y += 36.0 * s();
    }

    // Base tree / advanced tier switch (wars with the advanced tier only).
    let has_adv = rv.techs.iter().any(|t| t.advanced);
    let adv = has_adv && g.research_branch >= TECH_BRANCHES.len();
    y += 4.0 * s();
    if has_adv {
        let cw = (r.w - 2.0 * pad - 8.0 * s()) / 2.0;
        let adv_done = rv.techs.iter().filter(|t| t.advanced && t.state == TechState::Done).count();
        let adv_n = rv.techs.iter().filter(|t| t.advanced).count();
        let base_done = done - adv_done;
        let base_n = rv.techs.len() - adv_n;
        // all of the tier is locked behind the same "needs N base techs" until it opens
        let adv_locked = rv.techs.iter().filter(|t| t.advanced && t.tier == 1).all(|t| matches!(&t.state, TechState::Locked(w) if w.contains("base techs")));
        let adv_label = if adv_locked { "ADVANCED · LOCKED".to_string() } else { format!("ADVANCED · {adv_done}/{adv_n}") };
        for (k, label) in [format!("BASE TREE · {base_done}/{base_n}"), adv_label].iter().enumerate() {
            let cr = Rect::new(r.x + pad + k as f32 * (cw + 8.0 * s()), y, cw, 30.0 * s());
            let on = (k == 1) == adv;
            if widgets::chip(inp, cr, label, on, ACCENT) && !on {
                g.research_branch = if k == 1 { TECH_BRANCHES.len() } else { 0 };
                art::sfx("menu_tab");
            }
        }
        y += 38.0 * s();
    }

    // Branch tabs with progress.
    let (branches, tiers, off) = if adv { (&ADV_BRANCHES, 3, TECH_BRANCHES.len()) } else { (&TECH_BRANCHES, 4, 0) };
    let gap = 6.0 * s();
    let tw = (r.w - 2.0 * pad - 4.0 * gap) / 5.0;
    for (i, b) in branches.iter().enumerate() {
        let tr = Rect::new(r.x + pad + i as f32 * (tw + gap), y, tw, 54.0 * s());
        let on = g.research_branch == i + off;
        inp.block(tr);
        cel::round_rect(tr, 8.0 * s(), if on { alpha(ACCENT, 0.18) } else { Color::new(1.0, 1.0, 1.0, 0.04) });
        cel::round_rect_lines(tr, 8.0 * s(), (1.2 * s()).max(1.0), if on { ACCENT } else { BORDER });
        icon(branch_icon(b), vec2(tr.center().x, tr.y + 17.0 * s()), 22.0 * s(), if on { WHITE } else { Color::new(1.0, 1.0, 1.0, 0.7) });
        let n = rv.techs.iter().filter(|t| t.branch == *b && t.state == TechState::Done).count();
        let busy = rv.techs.iter().any(|t| t.branch == *b && matches!(t.state, TechState::Running { .. }));
        // one pip per tier researched
        for k in 0..tiers {
            let pc = vec2(tr.center().x + (k as f32 - (tiers as f32 - 1.0) / 2.0) * 9.0 * s(), tr.y + 38.0 * s());
            let col = if k < n {
                ACCENT
            } else if busy && k == n {
                alpha(ACCENT, 0.45)
            } else {
                Color::new(1.0, 1.0, 1.0, 0.18)
            };
            draw_circle(pc.x, pc.y, 3.2 * s(), col);
        }
        text(&b.to_uppercase(), tr.center().x, tr.y + tr.h + 11.0 * s(), 11.0, if on { ACCENT } else { MUTED }, FontKind::Title, Align::Center);
        if inp.tapped(tr) {
            inp.consume_tap();
            if !on {
                g.research_branch = i + off;
                art::sfx("menu_tab");
            }
        }
    }
    y += 54.0 * s() + 24.0 * s();

    // The tiers of the branch, top to bottom, joined by a spine.
    let branch = branches[(g.research_branch - off).min(4)];
    let techs: Vec<&TechInfo> = rv.techs.iter().filter(|t| t.branch == branch).collect();
    let bottom = r.y + r.h - pad;
    let n = techs.len().max(1) as f32;
    let cg = 10.0 * s();
    let ch = ((bottom - y - cg * (n - 1.0)) / n).min(124.0 * s());
    let spine_x = r.x + pad + 14.0 * s();
    if techs.len() > 1 {
        draw_line(spine_x, y + ch / 2.0, spine_x, y + (n - 1.0) * (ch + cg) + ch / 2.0, 2.0 * s(), BORDER);
    }
    for (i, t) in techs.iter().enumerate() {
        let cr = Rect::new(r.x + pad, y + i as f32 * (ch + cg), r.w - 2.0 * pad, ch);
        tech_card(g, inp, view, t, cr, spine_x);
    }
}

fn tech_card(g: &mut GameScreen, inp: &mut Input, view: &GameView, t: &TechInfo, cr: Rect, spine_x: f32) {
    let done = t.state == TechState::Done;
    let locked = matches!(t.state, TechState::Locked(_));
    let inner = Rect::new(cr.x + 34.0 * s(), cr.y, cr.w - 34.0 * s(), cr.h);
    cel::round_rect(inner, 8.0 * s(), if done { alpha(GOOD, 0.12) } else { Color::new(1.0, 1.0, 1.0, if locked { 0.03 } else { 0.06 }) });
    if matches!(t.state, TechState::Ready { .. }) {
        cel::round_rect_lines(inner, 8.0 * s(), (1.2 * s()).max(1.0), alpha(ACCENT, 0.6));
    }
    // tier node on the spine
    let nc = vec2(spine_x, cr.center().y);
    draw_circle(nc.x, nc.y, 12.0 * s(), if done { GOOD } else { PANEL_HI });
    draw_circle_lines(nc.x, nc.y, 12.0 * s(), 1.5 * s(), if done { GOOD } else if locked { BORDER } else { ACCENT });
    text(&format!("{}", t.tier), nc.x, nc.y + s(), 13.0, if done { INK } else { PAPER }, FontKind::Title, Align::Center);

    let x = inner.x + 12.0 * s();
    let right_w = 104.0 * s();
    let text_w = inner.w - 24.0 * s() - right_w;
    let title_col = if locked { MUTED } else { PAPER };
    text(&t.name.to_uppercase(), x, inner.y + 20.0 * s(), 16.0, title_col, FontKind::Title, Align::Left);
    let mut ly = inner.y + 40.0 * s();
    let max_lines = (((inner.h - 62.0 * s()) / (17.0 * s())).floor() as usize).clamp(1, 3);
    for line in wrap(&t.desc, FontKind::Body, 13.0, text_w).into_iter().take(max_lines) {
        text(&line, x, ly, 13.0, if locked { MUTED } else { alpha(PAPER, 0.85) }, FontKind::Body, Align::Left);
        ly += 17.0 * s();
    }
    let ok = affordable(view, &t.cost);
    if !done {
        let lab_note = if t.min_lab > 1 { format!("  · lab {}", t.min_lab) } else { String::new() };
        text(&format!("{}{lab_note}", cost_line(&t.cost)), x, inner.y + inner.h - 12.0 * s(), 12.0, if ok || locked { MUTED } else { DANGER }, FontKind::Body, Align::Left);
    }

    // Right column: state.
    let rc = Rect::new(inner.x + inner.w - right_w - 10.0 * s(), inner.y, right_w, inner.h);
    match &t.state {
        TechState::Done => {
            if !icon("check", vec2(rc.center().x, rc.center().y - 8.0 * s()), 28.0 * s(), WHITE) {
                text("✓", rc.center().x, rc.center().y - 8.0 * s(), 22.0, GOOD, FontKind::Title, Align::Center);
            }
            text("DONE", rc.center().x, rc.center().y + 18.0 * s(), 13.0, GOOD, FontKind::Title, Align::Center);
        }
        TechState::Running { lab, done_t } => {
            let total = t.cost.seconds.max(1.0);
            let left = (done_t - view.now).max(0.0);
            let frac = (1.0 - left / total).clamp(0.02, 1.0) as f32;
            widgets::bar(Rect::new(rc.x, rc.center().y - 3.0 * s(), rc.w, 6.0 * s()), frac, ACCENT);
            text(&fmt_dur(left), rc.center().x, rc.center().y - 14.0 * s(), 14.0, ACCENT, FontKind::Title, Align::Center);
            let lab_name = view.body(*lab).map(|b| b.name.to_uppercase()).unwrap_or_default();
            text(&lab_name, rc.center().x, rc.center().y + 18.0 * s(), 11.0, MUTED, FontKind::Title, Align::Center);
        }
        TechState::Ready { lab } => {
            let br = Rect::new(rc.x, rc.center().y - 20.0 * s(), rc.w, 40.0 * s());
            if widgets::button(inp, br, if ok { "RESEARCH" } else { "QUEUE" }, if ok { Style::Primary } else { Style::Secondary }) {
                art::sfx(if ok { "purchase" } else { "menu_select" });
                g.emit(UiAction::Research { body: *lab, tech: t.id.clone() });
                if !ok {
                    g.hint = Some(("Queued: starts when you can afford it".into(), get_time()));
                }
            }
            let lab_name = view.body(*lab).map(|b| format!("at {}", b.name)).unwrap_or_default();
            text(&lab_name, rc.center().x, br.y + br.h + 12.0 * s(), 11.0, MUTED, FontKind::Body, Align::Center);
        }
        TechState::Locked(why) => {
            icon("lock", vec2(rc.center().x, rc.center().y - 12.0 * s()), 22.0 * s(), Color::new(1.0, 1.0, 1.0, 0.6));
            let mut yy = rc.center().y + 12.0 * s();
            for line in wrap(why, FontKind::Body, 11.0, rc.w).into_iter().take(2) {
                text(&line, rc.center().x, yy, 11.0, MUTED, FontKind::Body, Align::Center);
                yy += 14.0 * s();
            }
        }
    }
}

// ---------------------------------------------------------------- stations

/// Station picker for one body: what's in orbit, and the six kinds we could build here.
pub fn stations_sheet(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView, body: BodyId, r: Rect) {
    inp.block(r);
    cel::panel(r, PANEL);
    let pad = 16.0 * s();
    let name = view.body(body).map(|b| b.name.to_uppercase()).unwrap_or_default();
    inked("STATIONS", r.x + pad, r.y + 22.0 * s(), 22.0, PAPER, Align::Left);
    if close_x(inp, r) {
        g.stations_for = None;
        art::sfx("menu_close");
        return;
    }
    let Some(sv) = be.stations(body) else { return };
    // Crew line: who is building here and how fast.
    let crew = match sv.constructors {
        0 => "no construction ships here".to_string(),
        n => format!("{n} construction ship{} · {}× speed", if n == 1 { "" } else { "s" }, sv.workers),
    };
    text(&format!("in orbit of {name} · {crew}"), r.x + pad, r.y + 44.0 * s(), 13.0, if sv.present { MUTED } else { DANGER }, FontKind::Title, Align::Left);

    // Existing stations as a row of small icons.
    let mut x = r.x + pad;
    let y = r.y + 66.0 * s();
    if sv.here.is_empty() {
        text("None here yet.", x, y, 13.0, MUTED, FontKind::Body, Align::Left);
    }
    for st in &sv.here {
        let c = vec2(x + 12.0 * s(), y);
        let tint = if st.building_until.is_some() { Color::new(1.0, 1.0, 1.0, 0.45) } else { WHITE };
        if st.building_until.is_some() {
            // progress ring under the icon
            draw_circle_lines(c.x, c.y, 14.0 * s(), 2.0 * s(), Color::new(1.0, 1.0, 1.0, 0.15));
            let n = 24;
            let k = (st.progress * n as f32).round() as i32;
            for j in 0..k {
                let a0 = -std::f32::consts::FRAC_PI_2 + j as f32 / n as f32 * std::f32::consts::TAU;
                let a1 = a0 + std::f32::consts::TAU / n as f32;
                let p0 = c + vec2(a0.cos(), a0.sin()) * 14.0 * s();
                let p1 = c + vec2(a1.cos(), a1.sin()) * 14.0 * s();
                draw_line(p0.x, p0.y, p1.x, p1.y, 2.5 * s(), ACCENT);
            }
        }
        if !icon(&format!("station_{}", snake(&st.kind)), c, 24.0 * s(), tint) {
            cel::diamond(c, 8.0 * s(), team(view.player(st.owner).map(|p| p.color)).0);
        }
        let mine = st.owner == view.me;
        draw_circle(c.x + 9.0 * s(), c.y + 9.0 * s(), 3.5 * s(), if mine { team(view.player(view.me).map(|p| p.color)).0 } else { DANGER });
        let label = match st.building_until {
            Some(t) if t.is_infinite() => format!("{:.0}% paused", st.progress * 100.0),
            Some(t) => format!("{:.0}% · {}", st.progress * 100.0, fmt_dur(t - view.now)),
            None => format!("{:.0}%", st.hp * 100.0),
        };
        text(&label, c.x + 16.0 * s(), y, 11.0, MUTED, FontKind::Title, Align::Left);
        x += 30.0 * s() + measure(&label, FontKind::Title, 11.0).x + 10.0 * s();
        if x > r.x + r.w - 60.0 * s() {
            break;
        }
    }

    // Station actions (lidar sweep...), then the options grid, 2 columns.
    let mut top = 84.0 * s();
    if !sv.actions.is_empty() {
        super::panels::ability_row(g, inp, &sv.actions, Rect::new(r.x + pad, r.y + top, r.w - 2.0 * pad, 30.0 * s()), |key| UiAction::StationAbility { body, key });
        top += 40.0 * s();
    }
    let grid = Rect::new(r.x + 12.0 * s(), r.y + top, r.w - 24.0 * s(), r.h - top - 48.0 * s());
    let gap = 8.0 * s();
    let cw = (grid.w - gap) / 2.0;
    let rows = sv.options.len().div_ceil(2).max(1);
    let ch = (grid.h - gap * (rows as f32 - 1.0)) / rows as f32;
    let mut info: Option<String> = None;
    for (i, o) in sv.options.iter().enumerate() {
        let cr = Rect::new(grid.x + (i % 2) as f32 * (cw + gap), grid.y + (i / 2) as f32 * (ch + gap), cw, ch);
        let blocked = o.blocker.is_some();
        let sel = g.station_pick.as_deref() == Some(o.kind.as_str());
        cel::round_rect(cr, 6.0 * s(), Color::new(1.0, 1.0, 1.0, if sel { 0.10 } else { 0.05 }));
        if sel {
            cel::round_rect_lines(cr, 6.0 * s(), (1.2 * s()).max(1.0), alpha(ACCENT, 0.5));
            info = Some(o.blocker.clone().map(|b| format!("{} — {}", o.desc, b)).unwrap_or_else(|| o.desc.clone()));
        }
        let ic = vec2(cr.x + 20.0 * s(), cr.center().y);
        let tint = if blocked { Color::new(1.0, 1.0, 1.0, 0.4) } else { WHITE };
        if !icon(&format!("station_{}", snake(&o.kind)), ic, 30.0 * s(), tint) {
            cel::diamond(ic, 9.0 * s(), if blocked { NEUTRAL.1 } else { NEUTRAL.0 });
        }
        let ok = affordable(view, &o.cost);
        text(&o.name.to_uppercase(), cr.x + 40.0 * s(), cr.y + cr.h * 0.36, 13.0, if blocked { MUTED } else { PAPER }, FontKind::Title, Align::Left);
        let sub = match &o.blocker {
            Some(b) => b.clone(),
            None => format!("{}M {}E {}", fmt_amount(o.cost.matter), fmt_amount(o.cost.energy), fmt_dur(o.cost.seconds / sv.workers.max(1) as f64)),
        };
        let sub_w = cr.w - 40.0 * s() - 48.0 * s();
        let sub = wrap(&sub, FontKind::Body, 11.0, sub_w).into_iter().next().unwrap_or_default();
        text(&sub, cr.x + 40.0 * s(), cr.y + cr.h * 0.7, 11.0, if blocked || ok { MUTED } else { DANGER }, FontKind::Body, Align::Left);
        let br = Rect::new(cr.x + cr.w - 44.0 * s(), cr.y + (cr.h - 32.0 * s()) / 2.0, 36.0 * s(), 32.0 * s());
        // no construction ship there yet / can't afford it yet: queue it, it starts once both are true
        let later = !ok || blocked;
        if (!blocked || o.queueable)
            && widgets::button(inp, br, "+", if later { Style::Secondary } else { Style::Primary })
        {
            art::sfx(if later { "menu_select" } else { "purchase" });
            g.emit(UiAction::BuildStation { body, kind: o.kind.clone() });
            if later {
                g.hint = Some(("Queued: starts once it can".into(), get_time()));
            }
        }
        if inp.tapped(cr) {
            inp.consume_tap();
            g.station_pick = Some(o.kind.clone());
            art::sfx("menu_select");
        }
    }
    let foot = info.unwrap_or_else(|| if sv.present { "Tap a station for details. More construction ships here build faster.".into() } else { sv.options.iter().find_map(|o| o.blocker.clone().filter(|b| b.contains("construction ship"))).unwrap_or_else(|| "Send a construction ship here to build stations.".into()) });
    for (k, line) in wrap(&foot, FontKind::Body, 12.0, r.w - 2.0 * pad).into_iter().take(2).enumerate() {
        text(&line, r.x + pad, r.y + r.h - 32.0 * s() + k as f32 * 16.0 * s(), 12.0, MUTED, FontKind::Body, Align::Left);
    }
}

/// "FuelDepot" → "fuel_depot" (icon names).
fn snake(kind: &str) -> String {
    let mut out = String::new();
    for (i, ch) in kind.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}
